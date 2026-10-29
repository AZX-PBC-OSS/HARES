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
//! overrides and patches. The record is checked before anything is
//! applied: a value with no recorded target, a patch target naming an
//! unknown field, or a settings override map of another shape is refused
//! naming the object, so a partially applicable record never lands half
//! applied.

use crate::dwelling::DwellingSettings;
use hares_types::{HaresError, ScenarioSample};

/// Merge `patch` into the overrides map `base`: the sampled equipment's
/// fields overlay the base's (a scenario's declared draw is the applied
/// value), the base's other equipment and fields pass through. Both
/// sides must be JSON objects: a settings override of another shape
/// cannot be merged into, and replacing it would silently discard the
/// caller's configuration.
fn merge_overrides(
    base: &mut serde_json::Value,
    patch: &serde_json::Value,
) -> Result<(), HaresError> {
    let base_map = base.as_object_mut().ok_or_else(|| {
        HaresError::Equipment(
            "the settings' overrides are not a JSON object: the scenario's sampled \
             overrides cannot merge into them"
                .to_string(),
        )
    })?;
    let patch_map = patch.as_object().ok_or_else(|| {
        HaresError::Equipment("the scenario's sampled overrides are not a JSON object".to_string())
    })?;
    for (key, value) in patch_map {
        match base_map.get_mut(key) {
            Some(slot) if slot.is_object() && value.is_object() => {
                merge_overrides(slot, value)?;
            }
            _ => {
                base_map.insert(key.clone(), value.clone());
            }
        }
    }
    Ok(())
}

/// Apply the sample to the settings: the Override targets merge into the
/// settings' overrides map (created when the settings carried none), the
/// Patch targets set the settings' patches (created when the settings
/// carried none). The sampled values land in declaration order, a later
/// sample of the same parameter clobbering an earlier one's field.
///
/// The record is checked before anything is applied: a value with no
/// recorded target, a patch target naming an unknown field, or a
/// settings override map of another shape is refused naming the object
/// it cannot apply. Nothing is applied and then dropped.
///
/// # Errors
///
/// [`HaresError`] naming the value, field or override map the record
/// cannot apply.
pub fn apply_sample(
    sample: &ScenarioSample,
    settings: &mut DwellingSettings,
) -> Result<(), HaresError> {
    sample.validate_application()?;
    let overrides = sample.overrides_json();
    if overrides.as_object().is_some_and(|map| !map.is_empty()) {
        let base = settings.overrides.take().unwrap_or(serde_json::json!({}));
        let mut merged = base;
        merge_overrides(&mut merged, &overrides)?;
        settings.overrides = Some(merged);
    }
    let patches = sample.patches();
    if !patches.is_empty() {
        let mut hpxml_patches = settings.patches.take().unwrap_or_default();
        for (field, value) in patches {
            match field.as_str() {
                "number_of_bedrooms" => hpxml_patches.number_of_bedrooms = Some(value),
                // The record's own check refused an unknown field above;
                // the arm stays so a new patch field must be named here
                // as well as in the record's table.
                other => {
                    return Err(HaresError::Equipment(format!(
                        "the scenario sample patches the unknown field '{other}': the \
                         patch fields are {}",
                        hares_types::PATCH_FIELDS.join(", ")
                    )));
                }
            }
        }
        settings.patches = Some(hpxml_patches);
    }
    Ok(())
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

    /// A sample whose recorded patch target names a field the patches do
    /// not carry is refused with the field's name, not applied as an
    /// empty patch set. The record is deserialized, the shape a manifest
    /// written by a hand-edited or older spec arrives in.
    #[test]
    fn an_unknown_patch_field_is_refused_naming_it() {
        let sample: ScenarioSample = serde_json::from_value(serde_json::json!({
            "scenario_id": 1,
            "bldg_id": 7,
            "values": [{"name": "bedrooms", "value": 3.0, "unit": "count"}],
            "targets": {"bedrooms": {"patch": {"field": "not_a_patch_field"}}}
        }))
        .expect("the record parses");
        let mut settings = DwellingSettings::new(test_sim_config(), None, 7, None);
        let err = apply_sample(&sample, &mut settings)
            .expect_err("an unknown patch field must be refused");
        assert!(
            err.to_string().contains("not_a_patch_field"),
            "the error must name the field: {err}"
        );
        assert!(
            settings.patches.is_none(),
            "the refused record must not touch the settings"
        );
    }

    /// A value with no recorded target cannot be applied anywhere: the
    /// application refuses the whole record naming the parameter rather
    /// than applying its siblings and dropping it (a record written
    /// before the target map existed deserializes with an empty map).
    #[test]
    fn a_value_without_a_target_is_refused_naming_it() {
        let sample: ScenarioSample = serde_json::from_value(serde_json::json!({
            "scenario_id": 1,
            "bldg_id": 7,
            "values": [
                {"name": "bedrooms", "value": 3.0, "unit": "count"},
                {"name": "orphan", "value": 5.0, "unit": "W"}
            ],
            "targets": {"bedrooms": {"patch": {"field": "number_of_bedrooms"}}}
        }))
        .expect("the record parses");
        let mut settings = DwellingSettings::new(test_sim_config(), None, 7, None);
        let err = apply_sample(&sample, &mut settings)
            .expect_err("a value with no target must be refused");
        assert!(
            err.to_string().contains("orphan"),
            "the error must name the value with no target: {err}"
        );
        assert!(
            settings.patches.is_none(),
            "the refused record must not apply its targeted sibling either"
        );
    }

    /// A settings override map of another shape cannot be merged into,
    /// and replacing it would discard the caller's configuration: the
    /// application refuses it naming the map.
    #[test]
    fn a_non_object_override_map_is_refused_naming_it() {
        let spec = ScenarioSpec {
            name: "non-object".to_string(),
            parameters: vec![hares_types::ScenarioParameter {
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
            }],
        };
        let sample = spec.sample(42, 1, 7).expect("the sample draws");
        let mut settings = DwellingSettings::new(
            test_sim_config(),
            Some(serde_json::json!("not a map")),
            7,
            None,
        );
        let err = apply_sample(&sample, &mut settings)
            .expect_err("a non-object override map must be refused");
        assert!(
            err.to_string().contains("overrides"),
            "the error must name the settings' overrides: {err}"
        );
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
        apply_sample(&sample, &mut settings).expect("the sample applies");
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

    /// A sampled override whose target names no field the equipment's
    /// config carries fails the build at the override, naming the field:
    /// the scenario's application is the existing override path, so the
    /// assembly's own rejection is the loud error a scenario author sees.
    #[test]
    fn an_override_target_naming_no_field_fails_the_build() {
        let spec = ScenarioSpec {
            name: "unknown-field".to_string(),
            parameters: vec![hares_types::ScenarioParameter {
                name: "capacity".to_string(),
                unit: "W".to_string(),
                target: hares_types::ScenarioTarget::Override {
                    equipment: "Electric Resistance Water Heater".to_string(),
                    field: "no_such_field".to_string(),
                },
                distribution: hares_types::ScenarioDistribution::Uniform {
                    low: 4000.0,
                    high: 6000.0,
                },
                correlation: None,
            }],
        };
        let sample = spec.sample(42, 1, 7).expect("the sample draws");
        let mut settings = DwellingSettings::new(test_sim_config(), None, 7, None);
        apply_sample(&sample, &mut settings).expect("the sample applies");
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
        let err = match Dwelling::from_config(config) {
            Ok(_) => panic!("an override field the equipment cannot take must fail the build"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("no_such_field"),
            "the error must name the unknown field: {err}"
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
        apply_sample(&sample, &mut settings).expect("the sample applies");
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
