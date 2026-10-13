//! Dwelling telemetry payloads used by control and RL integrations.

use std::collections::BTreeMap;

use chrono::{DateTime, FixedOffset};
use hares_types::{HaresError, ZoneId};

use crate::observation::{ObservationPlan, ObservationSurface, telemetry_observe_into};

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
    /// Per-zone energy balance residual (W) from the zone-air first-law check.
    /// One entry per zone in the same order as `zone_names`.
    pub energy_balance_residuals: Vec<f64>,
    pub total_power_kw: f64,
    pub reactive_power_kvar: f64,
    /// Load (kW) the island sources failed to cover during islanded operation
    /// (would-be phantom grid import). Always 0.0 when not islanded.
    pub island_unserved_kw: f64,
    /// Surplus on-site generation (kW) the island could not absorb during
    /// islanded operation (would-be phantom grid export). Always 0.0 when not
    /// islanded.
    pub island_excess_kw: f64,
    pub outdoor_temp_c: f64,
    /// Outdoor humidity ratio [kg water / kg dry air], typically 0.001–0.030.
    pub outdoor_humidity_ratio: f64,
    /// Per-actor telemetry: actor_name → channel_name → value.
    ///
    /// `BTreeMap` (not `HashMap`) so iteration order is deterministic:
    /// bare-name observation fields resolve to the same actor on every run,
    /// and downstream views (e.g. the Python `actors` dict) list actors and
    /// channels in a stable order.
    pub actor_telemetry: BTreeMap<String, BTreeMap<String, f64>>,
    /// 0/1 flag indicating whether the dwelling has been marked as permanently
    /// failed after a prior panic and will not be stepped again.
    pub dwelling_failed: bool,
    /// `true` when at least one simulation `step()` has completed.
    /// Before the first step all fields reflect construction-time defaults
    /// rather than simulated state; consumers should treat all observation
    /// vector entries as `f64::NAN` when this flag is `false`.
    pub initialized: bool,
}

/// Per-run warn-once gates for the observation dispatch's actor-telemetry
/// fallbacks. The run's dwelling owns the state and the caller that owns
/// the run's observation field list threads it through the dispatch, so
/// the first dot-notation and bare-name fallback resolution in a run each
/// warn once, and a fresh run (a dwelling rebuilt from its blueprint)
/// starts with fresh gates instead of inheriting the previous run's.
#[derive(Debug, Default, Clone)]
pub struct ObservationWarnOnce {
    /// The dot-notation fallback (`<actor>.<channel>`) warned this run.
    pub(crate) dot: bool,
    /// The bare-name fallback warned this run.
    pub(crate) bare: bool,
}

impl DwellingTelemetry {
    /// Verifies that the per-equipment electric power sum matches the
    /// solver's total within `max(0.001, 1e-6 * |total|)`.
    ///
    /// Raised from `run_timestep` (where the inputs are computed) in every
    /// build profile: a mismatch is a physics violation and fails the step
    /// with a typed error.
    pub fn verify_consistency(
        step: u64,
        sum_equipment_power_kw: f64,
        total_power_kw: f64,
    ) -> Result<(), HaresError> {
        let tolerance = 0.001_f64.max(1e-6 * total_power_kw.abs());
        let diff = (sum_equipment_power_kw - total_power_kw).abs();
        if diff > tolerance {
            return Err(HaresError::InvalidState(format!(
                "telemetry electrical consistency at step {step}: \
                 sum(equipment_power_kw) {sum_equipment_power_kw:.6} kW does not match \
                 total_power_kw {total_power_kw:.6} kW (diff {diff:.6} kW)"
            )));
        }
        Ok(())
    }

    /// Selects named observation channels into a flat contiguous vector.
    ///
    /// `warned` is the run's own [`ObservationWarnOnce`] state, supplied by
    /// the caller that owns the field list across the run; the fallback
    /// resolutions consume it so each warns once per run.
    pub fn observation_vec(
        &self,
        fields: &[&str],
        warned: &mut ObservationWarnOnce,
    ) -> Result<Vec<f64>, HaresError> {
        if !self.initialized {
            return Ok(vec![f64::NAN; fields.len()]);
        }
        let plan = ObservationPlan::from_telemetry(self, fields, warned)?;
        let mut out = vec![0.0; fields.len()];
        telemetry_observe_into(self, &plan, &mut out)?;

        #[cfg(feature = "observe")]
        tracing::debug!(
            fields = ?fields,
            observation = ?out,
            "observation vector built via Rust dispatch"
        );

        Ok(out)
    }

    /// A thin wrapper over the frozen observation plan: the fields resolve
    /// once into an [`ObservationPlan`] and the same executor
    /// `Dwelling::observe_into` runs on the live dwelling fills the vector
    /// from this snapshot, so the two readers produce the same values on
    /// bits by construction. A one-shot read: the fallback warnings consume
    /// a throwaway run's warn gates.
    pub fn to_observation_vec(&self, fields: &[&str]) -> Result<Vec<f64>, HaresError> {
        self.observation_vec(fields, &mut ObservationWarnOnce::default())
    }

    /// The names the plan resolves against this snapshot: the zone names
    /// in their (sorted) vector order with no zone ids of their own, the
    /// equipment names and the actors' published channels in sorted-name
    /// order.
    pub(crate) fn observation_surface(&self) -> ObservationSurface {
        ObservationSurface {
            zone_names: self
                .zone_names
                .iter()
                .map(|name| (name.clone(), ZoneId(0)))
                .collect(),
            equipment_names: self.equipment_names.clone(),
            actors: self
                .actor_telemetry
                .iter()
                .map(|(name, channels)| {
                    (name.clone(), channels.keys().cloned().collect::<Vec<_>>())
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DwellingTelemetry;
    use super::HaresError;
    use super::ObservationWarnOnce;
    use chrono::{FixedOffset, TimeZone};
    use std::collections::BTreeMap;

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
            island_unserved_kw: 0.0,
            island_excess_kw: 0.0,
            outdoor_temp_c: 10.0,
            outdoor_humidity_ratio: 0.008,
            actor_telemetry: BTreeMap::new(),
            dwelling_failed: false,
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
    fn actor_telemetry_fallbacks_consume_their_own_run_state_only() {
        let mut t = sample();
        t.actor_telemetry.insert(
            "hp".to_string(),
            BTreeMap::from([("power".to_string(), 2.0)]),
        );

        let mut warned = ObservationWarnOnce::default();
        t.observation_vec(&["hp.power"], &mut warned).unwrap();
        assert!(
            warned.dot,
            "the dot fallback's first use in a run consumes the run's warn gate"
        );
        t.observation_vec(&["power"], &mut warned).unwrap();
        assert!(
            warned.bare,
            "the bare fallback's first use in a run consumes its gate"
        );

        // A fresh state is a fresh run's: nothing is inherited.
        let mut fresh = ObservationWarnOnce::default();
        t.observation_vec(&["hp.power"], &mut fresh).unwrap();
        assert!(
            fresh.dot && !fresh.bare,
            "a fresh run's state starts unconsumed"
        );
    }

    #[test]
    fn consistency_check_fails_on_mismatched_electrical_power() {
        let err = DwellingTelemetry::verify_consistency(0, 3.0, 10.0)
            .expect_err("sum(equipment_power)=3 != total=10 must fail");
        assert!(err.to_string().contains("does not match"), "got: {err}");
    }

    #[test]
    fn consistency_check_passes_when_power_matches() {
        DwellingTelemetry::verify_consistency(0, 6.0, 6.0)
            .expect("equal sums must pass the consistency check");
    }

    #[test]
    fn consistency_check_detects_small_mismatch() {
        let err = DwellingTelemetry::verify_consistency(0, 10.0, 10.1)
            .expect_err("0.1 kW mismatch exceeds 0.001 kW tolerance and must fail");
        assert!(err.to_string().contains("does not match"), "got: {err}");
    }

    #[test]
    fn consistency_check_tolerates_sub_milliwatt_error() {
        DwellingTelemetry::verify_consistency(0, 100.0001, 100.0)
            .expect("0.0001 kW error is within 0.001 kW absolute tolerance");
    }

    // --- actor_telemetry resolution ---

    #[test]
    fn actor_telemetry_dot_notation_resolves_field() {
        let mut t = sample();
        let mut channels = BTreeMap::new();
        channels.insert("price".to_string(), 0.12);
        t.actor_telemetry.insert("market".to_string(), channels);
        let obs = t.to_observation_vec(&["market.price"]).unwrap();
        assert_eq!(obs, vec![0.12]);
    }

    #[test]
    fn actor_telemetry_bare_name_resolves_field() {
        let mut t = sample();
        let mut channels = BTreeMap::new();
        channels.insert("dr_flag".to_string(), 1.0);
        t.actor_telemetry.insert("dr_actor".to_string(), channels);
        let obs = t.to_observation_vec(&["dr_flag"]).unwrap();
        assert_eq!(obs, vec![1.0]);
    }

    #[test]
    fn actor_telemetry_builtin_wins_over_actor_fallback() {
        let mut t = sample();
        // Put a bogus outdoor_temp in actor_telemetry: built-in must win.
        let mut channels = BTreeMap::new();
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
        let mut ch_a = BTreeMap::new();
        ch_a.insert("val".to_string(), 1.0);
        t.actor_telemetry.insert("a".to_string(), ch_a);
        let mut ch_b = BTreeMap::new();
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

    // --- conformance: all known field keys ---

    /// All scalar (non-bracket, non-alias, non-temporal, non-actor) field keys
    /// that `to_observation_vec` handles.
    const SCALAR_OBSERVATION_KEYS: &[&str] = &[
        "outdoor_temp",
        "outdoor_humidity_ratio",
        "total_power_kw",
        "total_electric_kw",
        "reactive_power_kvar",
    ];

    /// Bracket-based keys with a known zone.
    const ZONE_BRACKET_KEYS: &[&str] = &[
        "zone_temp[Indoor]",
        "setpoint_heat[Indoor]",
        "setpoint_cool[Indoor]",
        "zone_energy_balance[Indoor]",
    ];

    /// Bracket-based keys with a known equipment.
    const EQUIP_BRACKET_KEYS: &[&str] = &["equipment_soc[Battery]", "equipment_power[Battery]"];

    /// Flat aliases whose canonical equipment is present in the sample.
    const FLAT_ALIAS_KEYS: &[&str] = &["battery_soc"];

    /// Temporal encoding keys.
    const TEMPORAL_KEYS: &[&str] = &["time_sin", "time_cos"];

    fn sample_with_ev() -> DwellingTelemetry {
        let mut t = sample();
        t.equipment_names = vec!["Battery".to_string(), "Electric Vehicle".to_string()];
        t.equipment_soc = vec![0.5, 0.8];
        t.equipment_modes = vec![1.0, 0.0];
        t.equipment_power_kw = vec![1.2, 0.0];
        t
    }

    #[test]
    fn all_scalar_observation_keys_produce_values() {
        let t = sample();
        let obs = t.to_observation_vec(SCALAR_OBSERVATION_KEYS).unwrap();
        assert_eq!(obs.len(), SCALAR_OBSERVATION_KEYS.len());
        for &v in &obs {
            assert!(
                v.is_finite() || v.is_nan(),
                "non-finite non-NaN in scalar observation: {v}"
            );
        }
    }

    #[test]
    fn reactive_power_kvar_is_observable() {
        let mut t = sample();
        t.reactive_power_kvar = 1.5;
        let obs = t.to_observation_vec(&["reactive_power_kvar"]).unwrap();
        assert_eq!(obs.len(), 1);
        assert!((obs[0] - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn zone_energy_balance_bracket_is_observable() {
        let mut t = sample();
        t.energy_balance_residuals = vec![150.0];
        let obs = t
            .to_observation_vec(&["zone_energy_balance[Indoor]"])
            .unwrap();
        assert_eq!(obs, vec![150.0]);
    }

    #[test]
    fn all_zone_bracket_keys_produce_values() {
        let t = sample();
        let obs = t.to_observation_vec(ZONE_BRACKET_KEYS).unwrap();
        assert_eq!(obs.len(), ZONE_BRACKET_KEYS.len());
    }

    #[test]
    fn all_equipment_bracket_keys_produce_values() {
        let t = sample();
        let obs = t.to_observation_vec(EQUIP_BRACKET_KEYS).unwrap();
        assert_eq!(obs.len(), EQUIP_BRACKET_KEYS.len());
    }

    #[test]
    fn flat_alias_keys_produce_values() {
        let t = sample();
        let obs = t.to_observation_vec(FLAT_ALIAS_KEYS).unwrap();
        assert_eq!(obs.len(), FLAT_ALIAS_KEYS.len());
    }

    #[test]
    fn ev_soc_alias_produces_value() {
        let t = sample_with_ev();
        let obs = t.to_observation_vec(&["ev_soc"]).unwrap();
        assert_eq!(obs.len(), 1);
        assert!((obs[0] - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn temporal_keys_produce_values() {
        let t = sample();
        let obs = t.to_observation_vec(TEMPORAL_KEYS).unwrap();
        assert_eq!(obs.len(), TEMPORAL_KEYS.len());
        for &v in &obs {
            assert!(v.is_finite());
        }
    }

    #[test]
    fn comprehensive_field_list_produces_correct_length() {
        let t = sample_with_ev();
        let mut fields: Vec<&str> = Vec::new();
        fields.extend_from_slice(SCALAR_OBSERVATION_KEYS);
        fields.extend_from_slice(ZONE_BRACKET_KEYS);
        fields.extend_from_slice(EQUIP_BRACKET_KEYS);
        fields.extend_from_slice(&["battery_soc", "ev_soc"]);
        fields.extend_from_slice(TEMPORAL_KEYS);
        let obs = t.to_observation_vec(&fields).unwrap();
        assert_eq!(obs.len(), fields.len());
        // All values should be finite (telemetry is initialized with real data).
        for &v in &obs {
            assert!(v.is_finite(), "expected finite, got {v}");
        }
    }

    #[test]
    fn unknown_zone_in_bracket_returns_err() {
        let t = sample();
        let err = t
            .to_observation_vec(&["zone_temp[NonExistent]"])
            .unwrap_err();
        assert!(err.to_string().contains("unknown zone"));
    }

    #[test]
    fn unknown_equipment_in_bracket_returns_err() {
        let t = sample();
        let err = t
            .to_observation_vec(&["equipment_power[NonExistent]"])
            .unwrap_err();
        assert!(err.to_string().contains("unknown equipment"));
    }

    #[test]
    fn ev_soc_alias_without_ev_returns_err() {
        let t = sample(); // only Battery
        let err = t.to_observation_vec(&["ev_soc"]).unwrap_err();
        assert!(
            err.to_string().contains("Electric Vehicle"),
            "expected error about missing EV, got: {err}"
        );
    }

    #[test]
    fn zone_energy_balance_unknown_zone_returns_err() {
        let t = sample();
        let err = t
            .to_observation_vec(&["zone_energy_balance[NonExistent]"])
            .unwrap_err();
        assert!(err.to_string().contains("unknown zone"));
    }
}
