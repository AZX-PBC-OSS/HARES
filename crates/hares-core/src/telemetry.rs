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
    /// Outdoor humidity ratio [kg water / kg dry air], typically 0.001–0.030.
    pub outdoor_humidity_ratio: f64,
    /// Per-actor telemetry: actor_name → channel_name → value.
    pub actor_telemetry: HashMap<String, HashMap<String, f64>>,
    /// 0/1 flag indicating whether the dwelling has been marked as permanently
    /// failed after a prior panic and will not be stepped again.
    pub dwelling_failed: bool,
    /// False when telemetry self-consistency checks (electrical sum, per-zone
    /// thermal totals) detect a mismatch. Consumers should inspect this flag
    /// before using the snapshot. Always `true` in release builds where the
    /// checks are not compiled in.
    pub telemetry_consistency_flag: bool,
}

impl DwellingTelemetry {
    /// Verifies that `sum(equipment_power_kw) ≈ total_power_kw` within
    /// `max(0.001, 1e-6 * |total_power_kw|)`. Sets `telemetry_consistency_flag`
    /// to `false` and emits a `tracing::warn!` on mismatch.
    ///
    /// Gated behind `debug_assertions` or `check_invariants` for zero
    /// production overhead.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    pub fn verify_consistency(&mut self, step: u64) {
        let sum_equip: f64 = self.equipment_power_kw.iter().sum();
        let total = self.total_power_kw;
        let tolerance = 0.001_f64.max(1e-6 * total.abs());
        let diff = (sum_equip - total).abs();
        if diff > tolerance {
            self.telemetry_consistency_flag = false;
            tracing::warn!(
                step = step,
                sum_equipment_power_kw = sum_equip,
                total_power_kw = total,
                diff_kw = diff,
                "Telemetry consistency: sum(equipment_power_kw) does not match total_power_kw"
            );
        }
    }

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
            if field == "outdoor_humidity_ratio" {
                out.push(self.outdoor_humidity_ratio);
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
            outdoor_humidity_ratio: 0.008,
            actor_telemetry: HashMap::new(),
            dwelling_failed: false,
            telemetry_consistency_flag: true,
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
    fn observation_vec_length_matches_field_count_narrow() {
        let t = sample();
        let fields = &[
            "outdoor_temp",
            "zone_temp[Indoor]",
            "total_power_kw",
            "equipment_soc[Battery]",
            "setpoint_heat[Indoor]",
        ];
        let obs = t.to_observation_vec(fields).unwrap();
        assert_eq!(obs.len(), fields.len());
    }

    #[test]
    fn observation_vec_length_matches_field_count_wide() {
        let t = sample();
        let base = &[
            "outdoor_temp",
            "outdoor_humidity_ratio",
            "total_power_kw",
            "reactive_power_kvar",
            "zone_temp[Indoor]",
            "setpoint_heat[Indoor]",
            "setpoint_cool[Indoor]",
            "zone_energy_balance[Indoor]",
            "equipment_soc[Battery]",
            "equipment_power[Battery]",
        ];
        let mut fields = Vec::new();
        while fields.len() < 55 {
            fields.extend_from_slice(base);
        }
        fields.truncate(55);
        let obs = t.to_observation_vec(&fields).unwrap();
        assert_eq!(obs.len(), 55);
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

    #[test]
    fn outdoor_humidity_ratio_observation_has_plausible_range() {
        let t = sample();
        // Humidity ratio in mild conditions is typically 0.001–0.030 kg/kg.
        assert!(
            (0.001..=0.030).contains(&t.outdoor_humidity_ratio),
            "outdoor_humidity_ratio={} outside plausible outdoor range 0.001–0.030 kg/kg",
            t.outdoor_humidity_ratio,
        );
        let obs = t.to_observation_vec(&["outdoor_humidity_ratio"]).unwrap();
        assert_eq!(obs, vec![t.outdoor_humidity_ratio]);
    }

    #[test]
    fn old_outdoor_rh_key_is_rejected() {
        let t = sample();
        let err = t.to_observation_vec(&["outdoor_rh"]).unwrap_err();
        assert!(err.to_string().contains("unknown telemetry field"));
    }

    #[test]
    fn consistency_flag_defaults_true() {
        let t = sample();
        assert!(t.telemetry_consistency_flag);
    }

    #[test]
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    fn consistency_check_flags_mismatched_electrical_power() {
        let mut t = sample();
        t.equipment_power_kw = vec![1.0, 2.0];
        t.total_power_kw = 10.0;
        t.telemetry_consistency_flag = true;
        t.verify_consistency(0);
        assert!(
            !t.telemetry_consistency_flag,
            "consistency flag should be false when sum(equipment_power) != total_power"
        );
    }

    #[test]
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    fn consistency_check_passes_when_power_matches() {
        let mut t = sample();
        t.equipment_power_kw = vec![1.0, 2.0, 3.0];
        t.total_power_kw = 6.0;
        t.telemetry_consistency_flag = true;
        t.verify_consistency(0);
        assert!(
            t.telemetry_consistency_flag,
            "consistency flag should remain true when sum(equipment_power) == total_power"
        );
    }

    #[test]
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    fn consistency_check_detects_small_mismatch() {
        let mut t = sample();
        t.equipment_power_kw = vec![10.0];
        t.total_power_kw = 10.1;
        t.telemetry_consistency_flag = true;
        t.verify_consistency(0);
        assert!(
            !t.telemetry_consistency_flag,
            "0.1 kW mismatch exceeds 0.001 kW tolerance and should be flagged"
        );
    }

    #[test]
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    fn consistency_check_tolerates_sub_milliwatt_error() {
        let mut t = sample();
        t.equipment_power_kw = vec![100.0];
        t.total_power_kw = 100.0001;
        t.telemetry_consistency_flag = true;
        t.verify_consistency(0);
        assert!(
            t.telemetry_consistency_flag,
            "0.0001 kW error is within 0.001 kW absolute tolerance"
        );
    }
}
