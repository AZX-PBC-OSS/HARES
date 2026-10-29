//! The scenario sample's application: the sampled values land through the
//! EXISTING overrides and patch paths, nothing new.
//!
//! The Override targets' values merge into `DwellingSettings.overrides`
//! (the equipment-keyed map the assembly validates field-by-field with
//! its loud unknown-field rejection, so a sampled value whose target
//! names no real field fails the build at the point the override is
//! applied, before any equipment init runs); the Patch targets' values
//! set the HPXML data patches (`HpxmlDataPatches`) the assembly consumes
//! the same way external metadata's patches arrive. The application
//! never invents a second path: a scenario is only a sampler over
//! overrides and patches.

use crate::dwelling::DwellingSettings;
use hares_types::ScenarioSample;

/// Merge `patch` into the overrides map `base`: the sampled equipment's
/// fields overlay the base's (a scenario's declared draw is the applied
/// value), the base's other equipment and fields pass through.
fn merge_overrides(base: &mut serde_json::Value, patch: &serde_json::Value) {
    let (Some(base_map), Some(patch_map)) = (base.as_object_mut(), patch.as_object()) else {
        *base = patch.clone();
        return;
    };
    for (key, value) in patch_map {
        match base_map.get_mut(key) {
            Some(slot) if slot.is_object() && value.is_object() => {
                merge_overrides(slot, value);
            }
            _ => {
                base_map.insert(key.clone(), value.clone());
            }
        }
    }
}

/// Apply the sample to the settings: the Override targets merge into the
/// settings' overrides map (created when the settings carried none), the
/// Patch targets set the settings' patches (created when the settings
/// carried none). The sampled values land in declaration order, a later
/// sample of the same parameter clobbering an earlier one's field.
pub fn apply_sample(sample: &ScenarioSample, settings: &mut DwellingSettings) {
    let overrides = sample.overrides_json();
    if overrides.as_object().is_some_and(|map| !map.is_empty()) {
        let base = settings.overrides.take().unwrap_or(serde_json::json!({}));
        let mut merged = base;
        merge_overrides(&mut merged, &overrides);
        settings.overrides = Some(merged);
    }
    let patches = sample.patches();
    if !patches.is_empty() {
        let mut hpxml_patches = settings.patches.take().unwrap_or_default();
        for (field, value) in patches {
            match field.as_str() {
                "number_of_bedrooms" => hpxml_patches.number_of_bedrooms = Some(value),
                other => {
                    // The spec's validation rejects unknown patch fields
                    // at declaration; a sample whose target map was
                    // captured by an older spec cannot reach here, and a
                    // hand-built one is refused loudly rather than
                    // silently dropped.
                    tracing::warn!(
                        field = %other,
                        "the scenario sample patches unknown field '{other}': \
                         the patch fields are number_of_bedrooms"
                    );
                    continue;
                }
            }
        }
        settings.patches = Some(hpxml_patches);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dwelling::{Dwelling, DwellingConfig};
    use chrono::TimeZone;
    use hares_io::SimulationConfig;
    use hares_types::ScenarioSpec;
    use std::path::PathBuf;

    fn test_sim_config() -> SimulationConfig {
        SimulationConfig {
            scenario_seed: None,
            schedule_diversity: Default::default(),
            start_time: chrono::FixedOffset::east_opt(0)
                .expect("offset")
                .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
                .single()
                .expect("start"),
            duration: chrono::Duration::hours(24),
            time_res: chrono::Duration::seconds(3600),
            reports: Vec::new(),
            diagnostics: false,
            metrics: false,
            output_chunk_size: 1024,
            setpoint_deadband_c: None,
            master_seed: 0,
            civil_timezone: None,
            site_location: Default::default(),
            max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
            health_policy: Default::default(),
            first_hour_interpolation: Default::default(),
            weather_year: Default::default(),
            warmup: hares_io::Warmup::Disabled,
        }
    }

    fn project_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates dir")
            .parent()
            .expect("repo root")
            .to_path_buf()
    }

    /// The apply helper's contract: the override targets merge into the
    /// settings' overrides map, the patch targets set the patches, and
    /// both land when the settings carried neither.
    #[test]
    fn apply_merges_overrides_and_patches() {
        let spec = ScenarioSpec {
            name: "apply".to_string(),
            parameters: vec![
                hares_types::ScenarioParameter {
                    name: "wh_capacity".to_string(),
                    unit: "W".to_string(),
                    target: hares_types::ScenarioTarget::Override {
                        equipment: "Electric Resistance Water Heater".to_string(),
                        field: "heating_capacity_w".to_string(),
                    },
                    distribution: hares_types::ScenarioDistribution::Uniform {
                        low: 4000.0,
                        high: 6000.0,
                    },
                    correlation: None,
                },
                hares_types::ScenarioParameter {
                    name: "bedrooms".to_string(),
                    unit: "count".to_string(),
                    target: hares_types::ScenarioTarget::Patch {
                        field: "number_of_bedrooms".to_string(),
                    },
                    distribution: hares_types::ScenarioDistribution::Uniform {
                        low: 2.0,
                        high: 5.0,
                    },
                    correlation: None,
                },
            ],
        };
        let sample = spec.sample(42, 1, 7).expect("the sample draws");
        let mut settings = DwellingSettings::new(
            test_sim_config(),
            Some(serde_json::json!({"Occupancy": {"number_of_occupants": 3.0}})),
            7,
            None,
        );
        apply_sample(&sample, &mut settings);
        let overrides = settings.overrides.expect("the overrides land");
        let map = overrides.as_object().expect("the overrides map");
        assert!(
            map.contains_key("Occupancy"),
            "the base's own overrides pass through"
        );
        let wh = map
            .get("Electric Resistance Water Heater")
            .and_then(|v| v.as_object())
            .expect("the sampled equipment's layer lands");
        let sampled = sample.values[0].value;
        assert_eq!(
            wh.get("heating_capacity_w").and_then(|v| v.as_f64()),
            Some(sampled),
            "the sampled value lands exactly"
        );
        let patches = settings.patches.expect("the patches land");
        assert_eq!(
            patches.number_of_bedrooms,
            Some(sample.values[1].value),
            "the patch target lands exactly"
        );
    }

    /// The acceptance round-trip: the sampled values land in the typed
    /// config each target NAMES. The override target's value is read
    /// back from the built dwelling's resolved model (the water heater's
    /// heating capacity resolves the sampled value bit for bit); the
    /// patch target's value is read back from the settings the build
    /// consumed (the patches the assembly's occupancy derivation feeds).
    #[test]
    fn sampled_values_reach_the_typed_configs_they_name() {
        let spec = ScenarioSpec {
            name: "round-trip".to_string(),
            parameters: vec![
                hares_types::ScenarioParameter {
                    name: "wh_setpoint".to_string(),
                    unit: "C".to_string(),
                    target: hares_types::ScenarioTarget::Override {
                        equipment: "Electric Resistance Water Heater".to_string(),
                        field: "setpoint_c".to_string(),
                    },
                    distribution: hares_types::ScenarioDistribution::Uniform {
                        low: 45.0,
                        high: 60.0,
                    },
                    correlation: None,
                },
                hares_types::ScenarioParameter {
                    name: "bedrooms".to_string(),
                    unit: "count".to_string(),
                    target: hares_types::ScenarioTarget::Patch {
                        field: "number_of_bedrooms".to_string(),
                    },
                    distribution: hares_types::ScenarioDistribution::Uniform {
                        low: 1.0,
                        high: 5.0,
                    },
                    correlation: None,
                },
            ],
        };
        let sample = spec.sample(42, 3, 7).expect("the sample draws");
        let sampled_setpoint = sample.values[0].value;
        let sampled_bedrooms = sample.values[1].value;

        let mut settings = DwellingSettings::new(test_sim_config(), None, 7, None);
        apply_sample(&sample, &mut settings);
        let config = DwellingConfig {
            hpxml: hares_io::InputSource::Path(
                project_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"),
            ),
            schedule: Some(hares_io::InputSource::Path(
                project_root().join("data/examples/BEopt_example_schedule.csv"),
            )),
            weather: hares_io::WeatherInput::Path(
                project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
            ),
            defaults: hares_io::DefaultsSource::directory(
                project_root().join("python/hares/defaults"),
            ),
            settings,
        };
        let dwelling = Dwelling::from_config(config).expect("the scenario dwelling builds");
        let resolved = dwelling.resolved_model();
        let wh = resolved
            .equipment
            .iter()
            .find(|eq| eq.name == "Electric Resistance Water Heater")
            .expect("the water heater resolves");
        let capacity = wh
            .parameters
            .iter()
            .find(|p| p.name == "setpoint_c")
            .expect("the water heater resolves its setpoint");
        assert_eq!(
            capacity.value, sampled_setpoint,
            "the sampled setpoint must reach the water heater's typed config \
             bit for bit"
        );
        // The patch target: the settings the build consumed carried the
        // sampled bedroom count (the occupancy derivation's own input).
        // The dwelling's own settings read-back is the build's record.
        let built_settings = dwelling.blueprint().settings();
        assert_eq!(
            built_settings
                .patches
                .as_ref()
                .and_then(|p| p.number_of_bedrooms),
            Some(sampled_bedrooms),
            "the sampled bedroom count must reach the patches the build consumed"
        );
    }
}
