//! Dwelling telemetry payloads used by control and RL integrations.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, FixedOffset, Timelike};
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
    /// `true` when at least one simulation `step()` has completed.
    /// Before the first step all fields reflect construction-time defaults
    /// rather than simulated state; consumers should treat all observation
    /// vector entries as `f64::NAN` when this flag is `false`.
    pub initialized: bool,
}

/// Throttle gate: warn once per process when actor_telemetry dot-notation resolution is used.
static WARNED_ACTOR_TELEMETRY_DOT: AtomicBool = AtomicBool::new(false);

/// Throttle gate: warn once per process when actor_telemetry bare-name resolution is used.
static WARNED_ACTOR_TELEMETRY_BARE: AtomicBool = AtomicBool::new(false);

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
        if !self.initialized {
            return Ok(vec![f64::NAN; fields.len()]);
        }
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

            // Temporal encoding: sin/cos of fractional hour for diurnal RL patterns.
            if field == "time_sin" {
                let fhour = self.current_time.hour() as f64
                    + self.current_time.minute() as f64 / 60.0
                    + self.current_time.second() as f64 / 3600.0;
                out.push((2.0 * std::f64::consts::PI * fhour / 24.0).sin());
                continue;
            }
            if field == "time_cos" {
                let fhour = self.current_time.hour() as f64
                    + self.current_time.minute() as f64 / 60.0
                    + self.current_time.second() as f64 / 3600.0;
                out.push((2.0 * std::f64::consts::PI * fhour / 24.0).cos());
                continue;
            }

            // Actor telemetry fallback: dot-separated <actor>.<channel>.
            let mut found_in_actors = false;
            if let Some((actor_name, channel_name)) = field.split_once('.') {
                if let Some(channels) = self.actor_telemetry.get(actor_name) {
                    // allowed: actor_telemetry keys are user-defined strings, not static tk:: constants
                    if let Some(&value) = channels.get(channel_name) {
                        if WARNED_ACTOR_TELEMETRY_DOT
                            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
                            .is_ok()
                        {
                            tracing::warn!(
                                field = field,
                                actor_telemetry_key = channel_name,
                                "resolving observation field via actor_telemetry (throttled to once per process)"
                            );
                        }
                        out.push(value);
                        found_in_actors = true;
                    }
                }
            }
            // Bare field name: search all actors (non-deterministic on duplicates).
            if !found_in_actors {
                for channels in self.actor_telemetry.values() {
                    if let Some(&value) = channels.get(field) {
                        if WARNED_ACTOR_TELEMETRY_BARE
                            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
                            .is_ok()
                        {
                            tracing::warn!(
                                field = field,
                                "resolving observation field via actor_telemetry (bare name, throttled to once per process)"
                            );
                        }
                        out.push(value);
                        found_in_actors = true;
                        break;
                    }
                }
            }
            if found_in_actors {
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
            initialized: true,
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

    // --- actor_telemetry resolution ---

    #[test]
    fn actor_telemetry_dot_notation_resolves_field() {
        let mut t = sample();
        let mut channels = HashMap::new();
        channels.insert("price".to_string(), 0.12);
        t.actor_telemetry.insert("market".to_string(), channels);
        let obs = t.to_observation_vec(&["market.price"]).unwrap();
        assert_eq!(obs, vec![0.12]);
    }

    #[test]
    fn actor_telemetry_bare_name_resolves_field() {
        let mut t = sample();
        let mut channels = HashMap::new();
        channels.insert("dr_flag".to_string(), 1.0);
        t.actor_telemetry.insert("dr_actor".to_string(), channels);
        let obs = t.to_observation_vec(&["dr_flag"]).unwrap();
        assert_eq!(obs, vec![1.0]);
    }

    #[test]
    fn actor_telemetry_builtin_wins_over_actor_fallback() {
        let mut t = sample();
        // Put a bogus outdoor_temp in actor_telemetry: built-in must win.
        let mut channels = HashMap::new();
        channels.insert("outdoor_temp".to_string(), 999.0);
        t.actor_telemetry.insert("weather".to_string(), channels);
        let obs = t.to_observation_vec(&["outdoor_temp"]).unwrap();
        assert_eq!(
            obs,
            vec![10.0],
            "built-in outdoor_temp must not be overridden by actor_telemetry"
        );
    }

    #[test]
    fn actor_telemetry_respects_dot_notation_priority() {
        // Dot notation is tried before bare-name fallback.  If both an
        // actor.xyz and a bare xyz exist, dot notation wins.
        let mut t = sample();
        let mut ch_a = HashMap::new();
        ch_a.insert("val".to_string(), 1.0);
        t.actor_telemetry.insert("a".to_string(), ch_a);
        let mut ch_b = HashMap::new();
        ch_b.insert("a.val".to_string(), 2.0);
        t.actor_telemetry.insert("b".to_string(), ch_b);
        let obs = t.to_observation_vec(&["a.val"]).unwrap();
        assert_eq!(obs, vec![1.0]);
    }

    #[test]
    fn actor_telemetry_nonexistent_actor_still_fails() {
        let t = sample();
        let err = t.to_observation_vec(&["nonexistent.channel"]).unwrap_err();
        assert!(err.to_string().contains("unknown telemetry field"));
    }

    // --- time_sin / time_cos ---

    #[test]
    fn time_sin_at_midnight_is_zero() {
        use chrono::TimeZone;
        let mut t = sample();
        t.current_time = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .unwrap();
        let obs = t.to_observation_vec(&["time_sin"]).unwrap();
        assert!((obs[0] - 0.0).abs() < 1e-10, "sin(0) ≈ 0, got {}", obs[0]);
    }

    #[test]
    fn time_cos_at_midnight_is_one() {
        use chrono::TimeZone;
        let mut t = sample();
        t.current_time = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .unwrap();
        let obs = t.to_observation_vec(&["time_cos"]).unwrap();
        assert!((obs[0] - 1.0).abs() < 1e-10, "cos(0) ≈ 1, got {}", obs[0]);
    }

    #[test]
    fn time_sin_cos_at_six_am() {
        use chrono::TimeZone;
        // 6:00 → 6/24 = 0.25 → 2π·0.25 = π/2
        // sin(π/2) = 1, cos(π/2) ≈ 0
        let mut t = sample();
        t.current_time = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 1, 1, 6, 0, 0)
            .single()
            .unwrap();
        let obs = t.to_observation_vec(&["time_sin"]).unwrap();
        assert!((obs[0] - 1.0).abs() < 1e-10, "sin(π/2) ≈ 1, got {}", obs[0]);
        let obs = t.to_observation_vec(&["time_cos"]).unwrap();
        assert!((obs[0] - 0.0).abs() < 1e-10, "cos(π/2) ≈ 0, got {}", obs[0]);
    }

    #[test]
    fn time_sin_cos_at_noon() {
        use chrono::TimeZone;
        // 12:00 → 12/24 = 0.5 → 2π·0.5 = π
        // sin(π) ≈ 0, cos(π) = -1
        let mut t = sample();
        t.current_time = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 1, 1, 12, 0, 0)
            .single()
            .unwrap();
        let obs = t.to_observation_vec(&["time_sin"]).unwrap();
        assert!((obs[0] - 0.0).abs() < 1e-10, "sin(π) ≈ 0, got {}", obs[0]);
        let obs = t.to_observation_vec(&["time_cos"]).unwrap();
        assert!((obs[0] + 1.0).abs() < 1e-10, "cos(π) = -1, got {}", obs[0]);
    }

    #[test]
    fn time_sin_cos_respects_minutes() {
        use chrono::TimeZone;
        // 1:30 = 1.5 hours. sin(2π·1.5/24) ≈ sin(π/8) ≈ 0.382683
        let mut t = sample();
        t.current_time = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 1, 1, 1, 30, 0)
            .single()
            .unwrap();
        let obs = t.to_observation_vec(&["time_sin"]).unwrap();
        let expected = (2.0 * std::f64::consts::PI * 1.5 / 24.0).sin();
        assert!((obs[0] - expected).abs() < 1e-10);
    }

    // --- initialized flag ---

    fn sample_uninitialized() -> DwellingTelemetry {
        let mut t = sample();
        t.initialized = false;
        t.timestep_index = 0;
        t
    }

    #[test]
    fn to_observation_vec_returns_nan_when_uninitialized() {
        let t = sample_uninitialized();
        let fields = &[
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
            "battery_soc",
            "ev_soc",
            "time_sin",
            "time_cos",
        ];
        let obs = t.to_observation_vec(fields).unwrap();
        assert_eq!(obs.len(), fields.len());
        for &v in &obs {
            assert!(v.is_nan(), "expected NaN when initialized=false, got {v}");
        }
    }

    #[test]
    fn to_observation_vec_returns_valid_when_initialized() {
        let t = sample();
        assert!(t.initialized);
        let fields = &["outdoor_temp", "zone_temp[Indoor]", "total_power_kw"];
        let obs = t.to_observation_vec(fields).unwrap();
        assert_eq!(obs.len(), fields.len());
        for &v in &obs {
            assert!(
                v.is_finite(),
                "expected finite when initialized=true, got {v}"
            );
        }
    }

    #[test]
    fn to_observation_vec_initialized_flag_is_settable() {
        let mut t = sample();
        t.initialized = false;
        let obs = t.to_observation_vec(&["outdoor_temp"]).unwrap();
        assert!(obs[0].is_nan(), "expected NaN for uninitialized");

        t.initialized = true;
        let obs = t.to_observation_vec(&["outdoor_temp"]).unwrap();
        assert!(obs[0].is_finite(), "expected finite for initialized");
    }
}
