//! The scenario sample's application: the sampled values land through the
//! EXISTING overrides and patch paths, nothing new.
//!
//! The Override targets' values merge into `DwellingSettings.overrides`
//! (the equipment-keyed map the assembly validates field-by-field with
//! its loud unknown-field rejection, so a sampled value whose target
//! names no real field fails the build at the point the override is
//! applied, before that equipment is constructed or initialized); the
//! Patch targets' values
//! set the HPXML data patches (`HpxmlDataPatches`) the assembly consumes
//! the same way external metadata's patches arrive. The application
//! never invents a second path: a scenario is only a sampler over
//! overrides and patches. The record is checked before anything is
//! applied: a value with no recorded target, a patch target naming an
//! unknown field, or a settings override map (or one of its equipment
//! entries) of another shape is refused naming the object, so a
//! partially applicable record never lands half applied.

use crate::dwelling::DwellingSettings;
use hares_types::{HaresError, ScenarioSample};

/// Merge `patch` into the overrides map `base`: the sampled equipment's
/// fields overlay the base's (a scenario's declared draw is the applied
/// value), the base's other equipment and fields pass through. Both
/// sides must be JSON objects at every level: a settings override, or
/// one of its equipment entries, of another shape cannot be merged
/// into, and replacing it would silently discard the caller's
/// configuration.
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
            None => {
                base_map.insert(key.clone(), value.clone());
            }
            Some(slot) if slot.is_object() && value.is_object() => {
                merge_overrides(slot, value)?;
            }
            Some(_) => {
                return Err(HaresError::Equipment(format!(
                    "the settings' override entry '{key}' is not a JSON object of fields: \
                     the scenario's sampled overrides cannot merge into it, and replacing \
                     it would silently discard the caller's configuration"
                )));
            }
        }
    }
    Ok(())
}

/// The patches a sample carries, applied to `base`: the sample's fields
/// overlay the base's, every other field passes through. A field the
/// patches do not carry is an error naming it, and `base` is not touched.
fn patches_applied_to(
    base: Option<&hares_io::HpxmlDataPatches>,
    patches: &[(String, f64)],
) -> Result<hares_io::HpxmlDataPatches, HaresError> {
    let mut out = base.cloned().unwrap_or_default();
    for (field, value) in patches {
        match field.as_str() {
            "number_of_bedrooms" => out.number_of_bedrooms = Some(*value),
            // The record's own check refused an unknown field before this
            // runs; this arm is the application's write a new patch field
            // must join as well as the record's table (the two lists' drift
            // is pinned by every_recorded_patch_field_applies), and an error
            // here leaves the caller's patches untouched.
            other => {
                return Err(HaresError::Equipment(format!(
                    "the scenario sample patches the unknown field '{other}': the \
                     patch fields are {}",
                    hares_types::PATCH_FIELDS.join(", ")
                )));
            }
        }
    }
    Ok(out)
}

/// Apply the sample to the settings: the Override targets merge into the
/// settings' overrides map (created when the settings carried none), the
/// Patch targets set the settings' patches (created when the settings
/// carried none). The record carries one value per target, so the values
/// land in declaration order with no field written twice; applying a
/// second record over the first lets the later record's value win at a
/// field both name.
///
/// The record is checked before anything is applied: a value with no
/// recorded target, a repeated value name, two values at one target, an
/// orphan target (a target whose name carries no value), a patch target
/// naming an unknown field, or a settings override map (or one of its
/// equipment entries) of another shape is refused naming the object it
/// cannot apply. A record with several faults names the FIRST the
/// application reaches: the record's own value checks in declaration
/// order (a repeated name at its second occurrence, then a value with no
/// target, then a repeated target at its second value, then an unknown
/// patch field), then the record's orphan targets in the target map's
/// order, then the overrides merge's
/// entries in the map's order. Both new values are
/// built beside the settings and assigned only when both have landed, so a
/// refusal leaves the caller's settings exactly as they were: nothing is
/// applied and then dropped, and no refusal consumes the caller's state.
///
/// # Errors
///
/// [`HaresError`] naming the value, name, target, field or override map
/// the record cannot apply.
pub fn apply_sample(
    sample: &ScenarioSample,
    settings: &mut DwellingSettings,
) -> Result<(), HaresError> {
    sample.validate_application()?;
    let overrides = sample.overrides_json();
    let merged = if overrides.as_object().is_some_and(|map| !map.is_empty()) {
        // The merge works on a copy: a refused record must not touch the
        // settings.
        let mut merged = settings.overrides.clone().unwrap_or(serde_json::json!({}));
        merge_overrides(&mut merged, &overrides)?;
        Some(merged)
    } else {
        None
    };
    let patches = sample.patches();
    let applied_patches = if patches.is_empty() {
        None
    } else {
        Some(patches_applied_to(settings.patches.as_ref(), &patches)?)
    };
    if let Some(merged) = merged {
        settings.overrides = Some(merged);
    }
    if let Some(applied_patches) = applied_patches {
        settings.patches = Some(applied_patches);
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

    /// A record with more than one fault names the FIRST fault the
    /// application reaches, in the values' declaration order, and applies
    /// nothing: the caller fixes the named fault and the next attempt
    /// names the next. The value checks run before the overrides merge, so
    /// a record's own faults are named before the settings' shape is
    /// examined.
    #[test]
    fn a_record_with_two_faults_names_the_first_in_declaration_order() {
        let record = |values: serde_json::Value| -> ScenarioSample {
            serde_json::from_value(serde_json::json!({
                "scenario_id": 1,
                "bldg_id": 7,
                "values": values,
                "targets": {
                    "bedrooms": {"patch": {"field": "not_a_patch_field"}}
                }
            }))
            .expect("the record parses")
        };

        // The untargeted value first: it is the fault named.
        let sample = record(serde_json::json!([
            {"name": "orphan", "value": 5.0, "unit": "W"},
            {"name": "bedrooms", "value": 3.0, "unit": "count"}
        ]));
        let mut settings = DwellingSettings::new(test_sim_config(), None, 7, None);
        let err = apply_sample(&sample, &mut settings)
            .expect_err("a record with two faults must be refused");
        assert!(
            err.to_string().contains("orphan"),
            "the first fault in declaration order is the untargeted value: {err}"
        );
        assert_eq!(
            settings.overrides, None,
            "the refused record must not apply anything"
        );
        assert!(settings.patches.is_none());

        // The unknown patch field first: it is the fault named.
        let sample = record(serde_json::json!([
            {"name": "bedrooms", "value": 3.0, "unit": "count"},
            {"name": "orphan", "value": 5.0, "unit": "W"}
        ]));
        let mut settings = DwellingSettings::new(test_sim_config(), None, 7, None);
        let err = apply_sample(&sample, &mut settings)
            .expect_err("a record with two faults must be refused");
        assert!(
            err.to_string().contains("not_a_patch_field"),
            "the first fault in declaration order is the unknown patch field: {err}"
        );
        assert_eq!(settings.overrides, None);
        assert!(settings.patches.is_none());
    }

    /// The application is all-or-nothing across the two channels: a
    /// record whose override target is valid and whose patch target names
    /// an unknown field applies NEITHER (the merge lands no field, the
    /// patches keep the caller's), so a record is never half applied.
    #[test]
    fn a_refused_record_applies_neither_channel() {
        let sample: ScenarioSample = serde_json::from_value(serde_json::json!({
            "scenario_id": 1,
            "bldg_id": 7,
            "values": [
                {"name": "mel_multiplier", "value": 0.75, "unit": "fraction"},
                {"name": "bedrooms", "value": 3.0, "unit": "count"}
            ],
            "targets": {
                "mel_multiplier": {"override": {"equipment": "MELs", "field": "usage_multiplier"}},
                "bedrooms": {"patch": {"field": "not_a_patch_field"}}
            }
        }))
        .expect("the record parses");
        let base_overrides = serde_json::json!({"Occupancy": {"number_of_occupants": 3.0}});
        let mut settings = DwellingSettings::new(
            test_sim_config(),
            Some(base_overrides.clone()),
            7,
            Some(hares_io::HpxmlDataPatches {
                number_of_bedrooms: Some(2.0),
            }),
        );
        let err = apply_sample(&sample, &mut settings).expect_err(
            "the unknown patch field must refuse the record before its valid override lands",
        );
        assert!(
            err.to_string().contains("not_a_patch_field"),
            "the error must name the field: {err}"
        );
        assert_eq!(
            settings.overrides.as_ref(),
            Some(&base_overrides),
            "the refused record must not apply its valid override either"
        );
        assert_eq!(
            settings.patches.as_ref().and_then(|p| p.number_of_bedrooms),
            Some(2.0),
            "the refused record must leave the caller's patches untouched"
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
        assert_eq!(
            settings.overrides.as_ref(),
            Some(&serde_json::json!("not a map")),
            "the refused record must not touch the settings"
        );
    }

    /// A base override ENTRY of another shape (a bare value where the
    /// sampled layer needs an object of fields) is refused naming the
    /// entry: replacing it would silently repair configuration the
    /// assembly rejects, the same silent-clobber class the top-level
    /// map's refusal covers, one level down.
    #[test]
    fn a_non_object_override_entry_is_refused_naming_it() {
        let spec = ScenarioSpec {
            name: "entry-shape".to_string(),
            parameters: vec![hares_types::ScenarioParameter {
                name: "mel_multiplier".to_string(),
                unit: "fraction".to_string(),
                target: hares_types::ScenarioTarget::Override {
                    equipment: "MELs".to_string(),
                    field: "usage_multiplier".to_string(),
                },
                distribution: hares_types::ScenarioDistribution::Uniform {
                    low: 0.7,
                    high: 1.3,
                },
                correlation: None,
            }],
        };
        let sample = spec.sample(42, 1, 7).expect("the sample draws");
        let mut settings = DwellingSettings::new(
            test_sim_config(),
            Some(serde_json::json!({"MELs": 3.0})),
            7,
            None,
        );
        let err = apply_sample(&sample, &mut settings).expect_err(
            "a base override entry the sampled layer cannot merge into must be refused",
        );
        assert!(
            err.to_string().contains("MELs"),
            "the error must name the entry: {err}"
        );
        assert_eq!(
            settings.overrides.as_ref(),
            Some(&serde_json::json!({"MELs": 3.0})),
            "the refused record must not touch the settings"
        );
    }

    /// Every patch field the record's table names applies: the table
    /// (`hares_types::PATCH_FIELDS`) and this module's application match are
    /// one contract, so a field the assembly can carry but this module
    /// fails to write (or the reverse) fails here rather than at a user's
    /// build. The struct's own field list is pinned against the table in
    /// hares-io (`the_scenario_patch_fields_are_this_structs_fields`).
    #[test]
    fn every_recorded_patch_field_applies() {
        for field in hares_types::PATCH_FIELDS {
            let sample: ScenarioSample = serde_json::from_value(serde_json::json!({
                "scenario_id": 1,
                "bldg_id": 7,
                "values": [{"name": "patch_value", "value": 3.0, "unit": "count"}],
                "targets": {"patch_value": {"patch": {"field": field}}}
            }))
            .expect("the record parses");
            let mut settings = DwellingSettings::new(test_sim_config(), None, 7, None);
            apply_sample(&sample, &mut settings)
                .unwrap_or_else(|err| panic!("the patch field '{field}' must apply: {err}"));
            assert!(
                settings.patches.is_some(),
                "the patch field '{field}' must land in the settings"
            );
        }
    }

    /// The hand-written record's extra fault classes refuse at the
    /// application with the settings untouched: a repeated value name
    /// (the duplicates would apply last wins) and a target whose name
    /// carries no value (the orphan would be silently ignored). The
    /// sampler produces neither shape; a hand-edited manifest
    /// deserializing to either is the record's own check refusing, before
    /// anything is applied.
    #[test]
    fn a_hand_written_records_duplicate_name_and_orphan_target_are_refused() {
        let duplicate: ScenarioSample = serde_json::from_value(serde_json::json!({
            "scenario_id": 1,
            "bldg_id": 7,
            "values": [
                {"name": "bedrooms", "value": 3.0, "unit": "count"},
                {"name": "bedrooms", "value": 4.0, "unit": "count"}
            ],
            "targets": {"bedrooms": {"patch": {"field": "number_of_bedrooms"}}}
        }))
        .expect("the record parses");
        let mut settings = DwellingSettings::new(test_sim_config(), None, 7, None);
        let err = apply_sample(&duplicate, &mut settings)
            .expect_err("a repeated value name must be refused");
        assert!(
            err.to_string().contains("bedrooms") && err.to_string().contains("repeats"),
            "the error must name the repeated value: {err}"
        );
        assert!(
            settings.patches.is_none() && settings.overrides.is_none(),
            "the refused record must not touch the settings"
        );

        let orphan: ScenarioSample = serde_json::from_value(serde_json::json!({
            "scenario_id": 1,
            "bldg_id": 7,
            "values": [{"name": "bedrooms", "value": 3.0, "unit": "count"}],
            "targets": {
                "bedrooms": {"patch": {"field": "number_of_bedrooms"}},
                "ghost": {"override": {"equipment": "MELs", "field": "usage_multiplier"}}
            }
        }))
        .expect("the record parses");
        let err = apply_sample(&orphan, &mut settings)
            .expect_err("a target with no value must be refused");
        assert!(
            err.to_string().contains("ghost"),
            "the error must name the orphan target: {err}"
        );
        assert!(
            settings.patches.is_none() && settings.overrides.is_none(),
            "the refused record must not touch the settings"
        );
    }

    /// Two values at one application target would apply last wins, the
    /// earlier value dropped silently (the target is the overrides map's
    /// key): the application refuses the record naming both values and
    /// leaves the settings exactly as they were. The sampler cannot
    /// produce the shape (the spec's validation refuses two parameters at
    /// one target); a hand-written record can.
    #[test]
    fn two_values_at_one_target_are_refused_at_the_application() {
        let sample: ScenarioSample = serde_json::from_value(serde_json::json!({
            "scenario_id": 1,
            "bldg_id": 7,
            "values": [
                {"name": "a", "value": 0.8, "unit": "fraction"},
                {"name": "b", "value": 1.2, "unit": "fraction"}
            ],
            "targets": {
                "a": {"override": {"equipment": "MELs", "field": "usage_multiplier"}},
                "b": {"override": {"equipment": "MELs", "field": "usage_multiplier"}}
            }
        }))
        .expect("the record parses");
        let mut settings = DwellingSettings::new(test_sim_config(), None, 7, None);
        let err = apply_sample(&sample, &mut settings)
            .expect_err("two values at one target must be refused");
        assert!(
            err.to_string().contains("'a'") && err.to_string().contains("'b'"),
            "the error must name both values: {err}"
        );
        assert!(
            settings.overrides.is_none() && settings.patches.is_none(),
            "the refused record must not touch the settings"
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

    /// The wildcard override's round-trip: a sampled value whose target
    /// is the wildcard (`all` or `*`, the overrides map's
    /// pattern-matching keys) lands in every equipment that reads the
    /// field, read back from the built dwelling's resolved model bit
    /// for bit, the same pin the direct path's witness holds. The
    /// mutation proof rides with it: a second draw's different value
    /// flips the resolved multiplier, so the pin is coupled to the
    /// sample and a wiring that dropped the application would fail the
    /// bitwise comparison.
    #[test]
    fn a_wildcard_override_sample_reaches_the_equipment_that_reads_it() {
        let spec_for = |wildcard: &str| ScenarioSpec {
            name: "wildcard".to_string(),
            parameters: vec![hares_types::ScenarioParameter {
                name: "load_multiplier".to_string(),
                unit: "fraction".to_string(),
                target: hares_types::ScenarioTarget::Override {
                    equipment: wildcard.to_string(),
                    field: "usage_multiplier".to_string(),
                },
                distribution: hares_types::ScenarioDistribution::Uniform {
                    low: 0.7,
                    high: 1.3,
                },
                correlation: None,
            }],
        };
        let config_for = |settings: DwellingSettings| DwellingConfig {
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
        // The multiplier the built load resolves: base.xml's two plug
        // loads ("MELs" and "TV") are raw-parameter loads that read the
        // wildcard's field, so the wildcard's reach is two equipment.
        let resolved_multiplier = |dwelling: &Dwelling, load: &str| {
            let resolved = dwelling.resolved_model();
            let eq = resolved
                .equipment
                .iter()
                .find(|eq| eq.name == load)
                .unwrap_or_else(|| panic!("the dwelling resolves its {load} load"));
            eq.parameters
                .iter()
                .find(|p| p.name == "usage_multiplier")
                .unwrap_or_else(|| panic!("the {load} load resolves its usage_multiplier"))
                .value
        };
        for wildcard in ["all", "*"] {
            let spec = spec_for(wildcard);
            let sample = spec.sample(42, 3, 7).expect("the sample draws");
            let sampled = sample.values[0].value;
            let mut settings = DwellingSettings::new(test_sim_config(), None, 7, None);
            apply_sample(&sample, &mut settings).expect("the sample applies");
            let dwelling =
                Dwelling::from_config(config_for(settings)).expect("the scenario dwelling builds");
            for load in ["MELs", "TV"] {
                assert_eq!(
                    resolved_multiplier(&dwelling, load),
                    sampled,
                    "the sampled multiplier must reach the {load} load bit for \
                     bit through the '{wildcard}' override"
                );
            }
        }
        // The mutation proof, on the 'all' spelling: scenario 4's draw
        // differs from scenario 3's, and the resolved multiplier flips
        // with it.
        let spec = spec_for("all");
        let sampled = spec.sample(42, 3, 7).expect("the sample draws").values[0].value;
        let perturbed = spec
            .sample(42, 4, 7)
            .expect("the perturbed sample draws")
            .values[0]
            .value;
        assert!(
            perturbed != sampled,
            "the perturbed draw must differ from the base draw for the flip \
             to be observable"
        );
        let mut settings = DwellingSettings::new(test_sim_config(), None, 7, None);
        apply_sample(
            &spec.sample(42, 4, 7).expect("the perturbed sample draws"),
            &mut settings,
        )
        .expect("the perturbed sample applies");
        let dwelling =
            Dwelling::from_config(config_for(settings)).expect("the perturbed dwelling builds");
        assert_eq!(
            resolved_multiplier(&dwelling, "MELs"),
            perturbed,
            "the perturbed sample's resolved multiplier must flip with it"
        );
    }
}
