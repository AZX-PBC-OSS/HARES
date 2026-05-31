//! Dwelling telemetry payloads used by control and RL integrations.

use std::collections::HashMap;

use chrono::{DateTime, FixedOffset};
use hares_types::HaresError;
use hares_types::normalize_ascii;

/// Dense telemetry snapshot for one dwelling timestep.
#[derive(Debug, Clone, PartialEq)]
pub struct DwellingTelemetry {
    pub timestep_index: u64,
    pub current_time: DateTime<FixedOffset>,
    pub zone_names: Vec<String>,
    pub zone_temperatures_c: Vec<f64>,
    pub equipment_names: Vec<String>,
    pub equipment_modes: Vec<f64>,
    pub equipment_soc: Vec<f64>,
    pub equipment_power_kw: Vec<f64>,
    pub setpoint_heat_c: Vec<f64>,
    pub setpoint_cool_c: Vec<f64>,
    /// Per-zone energy balance residual [W] from the zone-air first-law check.
    /// One entry per zone in the same order as `zone_names`.
    pub energy_balance_residuals: Vec<f64>,
    pub total_power_kw: f64,
    pub reactive_power_kvar: f64,
    pub outdoor_temp_c: f64,
    pub outdoor_rh: f64,
    /// Per-actor telemetry: actor_name → channel_name → value.
    pub actor_telemetry: HashMap<String, HashMap<String, f64>>,
    /// 0/1 flag indicating whether the dwelling has been marked as permanently
    /// failed after a prior panic and will not be stepped again.
    pub dwelling_failed: bool,
}

impl DwellingTelemetry {
    /// Selects named observation channels into a flat contiguous vector.
    pub fn to_observation_vec(&self, fields: &[&str]) -> Result<Vec<f64>, HaresError> {
        let mut out = Vec::with_capacity(fields.len());
        let zone_index = index_map(&self.zone_names);
        let equip_index = index_map(&self.equipment_names);

        for &field in fields {
            if field == "outdoor_temp" {
                out.push(self.outdoor_temp_c);
                continue;
            }
            if field == "outdoor_rh" {
                out.push(self.outdoor_rh);
                continue;
            }
            if field == "total_electric_kw" || field == "total_power_kw" {
                out.push(self.total_power_kw);
                continue;
            }
            if field == "reactive_power_kvar" {
                out.push(self.reactive_power_kvar);
                continue;
            }
            if let Some(name) = bracket_name(field, "zone_temp") {
                let idx = *zone_index.get(&normalize_ascii(name)).ok_or_else(|| {
                    HaresError::Control(format!("unknown zone in field `{field}`"))
                })?;
                out.push(self.zone_temperatures_c[idx]);
                continue;
            }
            if let Some(name) = bracket_name(field, "setpoint_heat") {
                let idx = *zone_index.get(&normalize_ascii(name)).ok_or_else(|| {
                    HaresError::Control(format!("unknown zone in field `{field}`"))
                })?;
                out.push(self.setpoint_heat_c[idx]);
                continue;
            }
            if let Some(name) = bracket_name(field, "setpoint_cool") {
                let idx = *zone_index.get(&normalize_ascii(name)).ok_or_else(|| {
                    HaresError::Control(format!("unknown zone in field `{field}`"))
                })?;
                out.push(self.setpoint_cool_c[idx]);
                continue;
            }
            if let Some(name) = bracket_name(field, "zone_energy_balance") {
                let idx = *zone_index.get(&normalize_ascii(name)).ok_or_else(|| {
                    HaresError::Control(format!("unknown zone in field `{field}`"))
                })?;
                out.push(self.energy_balance_residuals[idx]);
                continue;
            }
            if let Some(name) = bracket_name(field, "equipment_soc") {
                let idx = *equip_index.get(&normalize_ascii(name)).ok_or_else(|| {
                    HaresError::Control(format!("unknown equipment in field `{field}`"))
                })?;
                out.push(self.equipment_soc[idx]);
                continue;
            }
            if let Some(name) = bracket_name(field, "equipment_power") {
                let idx = *equip_index.get(&normalize_ascii(name)).ok_or_else(|| {
                    HaresError::Control(format!("unknown equipment in field `{field}`"))
                })?;
                out.push(self.equipment_power_kw[idx]);
                continue;
            }

            // Flat aliases used in architecture docs for single-instance cases.
            if field == "battery_soc" {
                let idx = single_instance_alias(&equip_index, "Battery", field)?;
                out.push(self.equipment_soc[idx]);
                continue;
            }
            if field == "ev_soc" {
                let idx = single_instance_alias(&equip_index, "Electric Vehicle", field)?;
                out.push(self.equipment_soc[idx]);
                continue;
            }

            return Err(HaresError::Control(format!(
                "unknown telemetry field key `{field}`"
            )));
        }

        Ok(out)
    }
}

fn index_map(names: &[String]) -> HashMap<String, usize> {
    names
        .iter()
        .enumerate()
        .map(|(idx, name)| (normalize_ascii(name), idx))
        .collect()
}

fn bracket_name<'a>(field: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}[");
    field
        .strip_prefix(&prefix)
        .and_then(|tail| tail.strip_suffix(']'))
}

fn single_instance_alias(
    equip_index: &HashMap<String, usize>,
    canonical_name: &str,
    field: &str,
) -> Result<usize, HaresError> {
    if let Some(&idx) = equip_index.get(&normalize_ascii(canonical_name)) {
        return Ok(idx);
    }
    Err(HaresError::Control(format!(
        "field `{field}` alias requires `{canonical_name}` equipment"
    )))
}

#[cfg(test)]
mod tests {
    use super::DwellingTelemetry;
    use chrono::{FixedOffset, TimeZone};
    use std::collections::HashMap;

    fn sample() -> DwellingTelemetry {
        DwellingTelemetry {
            timestep_index: 3,
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 1, 1, 0, 3, 0)
                .single()
                .unwrap(),
            zone_names: vec!["Indoor".to_string()],
            zone_temperatures_c: vec![21.0],
            equipment_names: vec!["Battery".to_string()],
            equipment_modes: vec![1.0],
            equipment_soc: vec![0.5],
            equipment_power_kw: vec![1.2],
            setpoint_heat_c: vec![20.0],
            setpoint_cool_c: vec![24.0],
            energy_balance_residuals: vec![0.0],
            total_power_kw: 1.2,
            reactive_power_kvar: 0.0,
            outdoor_temp_c: 10.0,
            outdoor_rh: 0.45,
            actor_telemetry: HashMap::new(),
            dwelling_failed: false,
        }
    }

    #[test]
    fn observation_fields_follow_order() {
        let t = sample();
        let obs = t
            .to_observation_vec(&[
                "zone_temp[Indoor]",
                "equipment_soc[Battery]",
                "outdoor_temp",
            ])
            .unwrap();
        assert_eq!(obs, vec![21.0, 0.5, 10.0]);
    }

    #[test]
    fn observation_vec_includes_reactive_power() {
        let mut t = sample();
        t.reactive_power_kvar = 1.5;
        let obs = t.to_observation_vec(&["reactive_power_kvar"]).unwrap();
        assert_eq!(obs, vec![1.5]);
    }

    #[test]
    fn unknown_field_returns_err() {
        let t = sample();
        let err = t.to_observation_vec(&["nope"]).unwrap_err();
        assert!(err.to_string().contains("unknown telemetry field"));
    }

    #[test]
    fn zone_energy_balance_bracket_key() {
        let mut t = sample();
        t.energy_balance_residuals = vec![150.0];
        let obs = t
            .to_observation_vec(&["zone_energy_balance[Indoor]"])
            .unwrap();
        assert_eq!(obs, vec![150.0]);
    }
}
