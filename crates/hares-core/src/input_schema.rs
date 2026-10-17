//! The machine-readable input schema: `input_schema()` assembles, from
//! the typed definitions, the schema of every input the engine reads —
//! the config keys, the dwelling keys, the override targets, the schedule
//! columns, the defaults files and the HPXML elements.

use hares_io::input_schema::{self, InputSchema, OverrideClassSchema};
use hares_types::DescribeFields;

use crate::dwelling::{DwellingConfig, DwellingSettings};

/// The machine-readable schema of every input the engine reads. Every
/// section derives from the definitions beside the engine's own surfaces
/// (the serde deserialisations, the equipment registry, the schedule
/// column table, the defaults loader's read list, the HPXML element
/// lists), so a consumer that classifies its fields reads the schema
/// instead of HARES source.
///
/// The defaults files section is the read list of the shipped defaults
/// tree (the repository's `python/hares/defaults`, the packaged copy the
/// Python surface resolves): a missing tree fails the schema instead of
/// leaving the section silently empty.
#[must_use]
pub fn input_schema() -> InputSchema {
    let defaults_files =
        hares_io::input_schema::defaults_file_schemas(&crate::dwelling::shipped_defaults_dir())
            .expect("the shipped defaults tree reads");
    assemble(defaults_files)
}

fn assemble(defaults_files: Vec<String>) -> InputSchema {
    let registry = hares_equipment::EquipmentRegistry::new();
    let mut override_classes: Vec<OverrideClassSchema> = registry
        .known_names()
        .into_iter()
        .map(|class| {
            let fields = hares_equipment::sample_config::override_fields_for_class(class)
                .unwrap_or_else(|| {
                    panic!("the registry class '{class}' has no override field list")
                });
            OverrideClassSchema {
                name: class.to_string(),
                description: format!("the '{class}' equipment's config fields"),
                fields,
            }
        })
        .collect();
    // The reserved `envelope` override: not an equipment class, but an override
    // target the schema publishes the same way, from the scales' own
    // `DescribeFields` impl beside the definition (the input-schema row's owed
    // fold addition). The assembly's override-key validation exempts the key and
    // the scales' own deserialisation and bounds validation enforce it.
    override_classes.push(OverrideClassSchema {
        name: hares_io::ENVELOPE_OVERRIDE_KEY.to_string(),
        description: "the reserved 'envelope' override: the calibration scales the resolved \
             envelope layers multiply by, applied after the LUT row selection"
            .to_string(),
        fields: <hares_io::EnvelopeScales as DescribeFields>::fields(),
    });

    InputSchema {
        config_fields: <hares_io::SimulationConfig as DescribeFields>::fields(),
        dwelling_fields: <DwellingConfig as DescribeFields>::fields(),
        settings_fields: <DwellingSettings as DescribeFields>::fields(),
        override_classes,
        removed_keys: input_schema::removed_key_schemas(),
        schedule_columns: input_schema::schedule_column_schemas().0,
        unused_schedule_columns: input_schema::schedule_column_schemas().1,
        defaults_files,
        hpxml_elements: input_schema::hpxml_element_schemas(),
        rejected_hpxml_elements: input_schema::rejected_hpxml_element_schemas(),
    }
}

/// A sample spec of one override class: the class's sample config, so a
/// test's override lands on the same merge the assembly runs. A raw
/// class's sample keeps its config in the parameter bag (the raw
/// channel's own shape); `None` for a name no registry builds.
#[must_use]
pub fn sample_spec(class: &str) -> Option<hares_io::EquipmentSpec> {
    let config = hares_equipment::sample_config::sample_config_for_class(class)?;
    let mut parameters = serde_json::Map::new();
    let mut typed_config = None;
    match &config.payload {
        hares_equipment::ConfigPayload::Typed { .. } => typed_config = Some(config),
        hares_equipment::ConfigPayload::Raw { data } => {
            for (key, value) in data {
                parameters.insert(key.clone(), serde_json::to_value(value).unwrap_or_default());
            }
        }
    }
    Some(hares_io::EquipmentSpec {
        name: class.to_string(),
        instance_name: None,
        fuel_type: hares_types::FuelType::Electric,
        parameters,
        typed_overrides: serde_json::Map::new(),
        zip_params: None,
        typed_config,
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
        heating_setpoint_source: None,
        cooling_setpoint_source: None,
    })
}

#[cfg(test)]
mod tests {
    use super::{input_schema, sample_spec};
    use hares_io::input_schema::InputSchema;

    fn schema() -> InputSchema {
        input_schema()
    }

    /// A fully populated `SimulationConfig` serialised to TOML has exactly
    /// the schema's field names as keys; any other key is rejected.
    #[test]
    fn input_schema_config_fields_equal_accepted_keys() {
        let config = hares_io::SimulationConfig {
            start_time: chrono::DateTime::parse_from_rfc3339("2023-01-01T00:00:00-07:00")
                .expect("valid start time"),
            duration: chrono::Duration::hours(48),
            time_res: chrono::Duration::seconds(900),
            reports: vec![hares_io::ReportSpec {
                name: "frame".to_string(),
                selection: hares_io::Selection::Verbosity(2),
                frequency: hares_io::Frequency::Timestep,
                destination: hares_io::Destination::Memory,
            }],
            diagnostics: false,
            metrics: false,
            output_chunk_size: 10_000,
            setpoint_deadband_c: Some(1.0),
            master_seed: 7,
            civil_timezone: Some("America/Denver".to_string()),
            site_location: hares_io::SiteLocationOverride {
                latitude_deg: Some(39.7),
                longitude_deg: Some(-104.9),
                elevation_m: Some(1609.0),
                utc_offset_h: Some(-7.0),
            },
            max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
            health_policy: hares_io::HealthPolicy::default(),
            first_hour_interpolation: hares_io::FirstHour::default(),
            weather_year: hares_io::WeatherYear::default(),
            warmup: hares_io::Warmup::default(),
        };
        let toml = toml::to_string(&config).expect("the config serialises to TOML");
        let table: toml::Table = toml.parse().expect("the TOML parses");
        let schema_names: Vec<String> = schema()
            .config_fields
            .iter()
            .map(|field| field.name.clone())
            .collect();
        assert!(!schema_names.is_empty());
        for key in table.keys() {
            assert!(
                schema_names.contains(&key.to_string()),
                "the config accepts '{key}' but the schema does not list it"
            );
        }
        for name in &schema_names {
            assert!(
                table.contains_key(name),
                "the schema lists '{name}' but a fully populated config does not carry it"
            );
        }
        // Any other key is rejected by the config's own strict
        // deserialisation.
        let stray = "not_a_config_key = 1\nduration = 3600\ntime_res = 900\
                     \nstart_time = \"2023-01-01T00:00:00-07:00\"";
        assert!(
            toml::from_str::<hares_io::SimulationConfig>(stray).is_err(),
            "a key the schema does not list is rejected"
        );
    }

    /// For every class and field in the schema, an in-bounds override is
    /// accepted by assembly, and a field name not in the schema is
    /// rejected naming the class and the field. The test iterates the
    /// schema and holds no list of its own.
    #[test]
    fn input_schema_overrides_match_enforcement() {
        let schema = schema();
        assert!(!schema.override_classes.is_empty());
        for class in &schema.override_classes {
            assert!(
                !class.fields.is_empty(),
                "class '{}' lists its fields",
                class.name
            );
            if class.name == hares_io::ENVELOPE_OVERRIDE_KEY {
                // The reserved `envelope` override has no equipment sample and no
                // merged equipment config: its enforcement is the scales' own
                // deserialisation and bounds validation, pinned in
                // `envelope_scales_described_with_bounds` below.
                continue;
            }
            let spec = sample_spec(&class.name)
                .unwrap_or_else(|| panic!("class '{}': the registry builds a sample", class.name));
            let sample_data = spec
                .typed_config
                .as_ref()
                .and_then(|typed| match &typed.payload {
                    hares_equipment::ConfigPayload::Typed { data, .. } => Some(data.clone()),
                    hares_equipment::ConfigPayload::Raw { data } => serde_json::to_value(data).ok(),
                })
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
            let current = sample_data.clone();
            // Every field of the class: the sample's own value when it
            // carries one, else the first shape the merge accepts against
            // the sample's base (an `Option`'s null among them). The
            // found override map lands in one merge, accepted.
            // The override carries exactly the schema's fields: the
            // sample's own value when it carries one, else the first
            // shape the merge accepts against the sample's base (an
            // `Option`'s null among them). The found override map lands
            // in one merge, accepted.
            let mut overrides = serde_json::Map::new();
            for field in &class.fields {
                if let Some(own) = current.get(&field.name) {
                    overrides.insert(field.name.clone(), own.clone());
                    continue;
                }
                let candidates = [
                    serde_json::json!(1.0),
                    serde_json::json!("s"),
                    serde_json::json!(true),
                    serde_json::json!([1.0]),
                    serde_json::json!({}),
                    serde_json::Value::Null,
                ];
                let found = candidates
                    .into_iter()
                    .find(|value| {
                        let mut trial = sample_data.clone();
                        if let serde_json::Value::Object(trial_map) = &mut trial {
                            // The reserved keys the assembly assigns (the
                            // equipment id) are not the override's, so
                            // the trial carries only the schema's fields,
                            // the ones already resolved beside the
                            // candidate (a count parameter the family's
                            // keys read, `phase_len` for `phase_0_*`).
                            trial_map.remove("equipment_id");
                            for (resolved, value) in &overrides {
                                trial_map.insert(resolved.clone(), value.clone());
                            }
                            trial_map.insert(field.name.clone(), value.clone());
                        }
                        crate::dwelling::conversions::merged_equipment_config(
                            &spec,
                            &serde_json::json!({ class.name.clone(): trial }),
                        )
                        .is_ok()
                    })
                    .unwrap_or_else(|| {
                        panic!(
                            "class '{}': the field '{}' accepts no probe \
                             value against the sample",
                            class.name, field.name
                        )
                    });
                overrides.insert(field.name.clone(), found);
            }
            crate::dwelling::conversions::merged_equipment_config(
                &spec,
                &serde_json::json!({ class.name.clone(): overrides }),
            )
            .unwrap_or_else(|err| {
                panic!(
                    "class '{}': an in-bounds override of every listed field is accepted: {err}",
                    class.name
                )
            });
            // A field name not in the schema is rejected naming the class
            // and the field.
            let bogus =
                serde_json::json!({ class.name.clone(): {"not_a_field_of_this_class": 1.0} });
            let err = crate::dwelling::conversions::merged_equipment_config(&spec, &bogus)
                .expect_err("an unknown field is rejected");
            let message = err.to_string();
            assert!(
                message.contains(&class.name) && message.contains("not_a_field_of_this_class"),
                "the rejection names the class and the field: {message}"
            );
        }
    }

    /// The schema's override classes are the equipment registry's names
    /// plus the reserved `envelope` key.
    #[test]
    fn input_schema_classes_equal_registry() {
        let registry = hares_equipment::EquipmentRegistry::new();
        let mut expected: Vec<String> = registry
            .known_names()
            .into_iter()
            .map(str::to_string)
            .collect();
        expected.push(hares_io::ENVELOPE_OVERRIDE_KEY.to_string());
        expected.sort_unstable();
        let mut actual: Vec<String> = schema()
            .override_classes
            .iter()
            .map(|class| class.name.clone())
            .collect();
        actual.sort();
        assert_eq!(actual, expected);
    }

    /// The reserved `envelope` class lists the two scales with default 1.0
    /// and bounds (0, 10], and the enforcement still rejects an unknown
    /// field naming it (the schema's own envelope-scales acceptance).
    #[test]
    fn envelope_scales_described_with_bounds() {
        use hares_types::ValueType;
        let schema = schema();
        let class = schema
            .override_classes
            .iter()
            .find(|class| class.name == hares_io::ENVELOPE_OVERRIDE_KEY)
            .expect("the reserved 'envelope' override is published");
        let mut names: Vec<&str> = class.fields.iter().map(|f| f.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["opaque_r_scale", "thermal_mass_scale"]);
        for field in &class.fields {
            assert_eq!(
                field.value_type,
                ValueType::Number,
                "{}: a scale is a float",
                field.name
            );
            assert_eq!(field.default, serde_json::json!(1.0), "the unit default");
            let bounds = field.bounds.expect("the bounds are published");
            assert_eq!(bounds.min, Some(0.0));
            assert!(bounds.min_exclusive, "zero is excluded");
            assert_eq!(bounds.max, Some(10.0));
            assert!(!bounds.max_exclusive, "ten is included");
        }
        let err = hares_io::EnvelopeScales::validate(
            &serde_json::from_value::<hares_io::EnvelopeScales>(serde_json::json!({
                "opaque_r_scale": 0.0,
            }))
            .expect("the field is a known one"),
        )
        .expect_err("an out-of-bounds scale is rejected");
        assert!(
            err.to_string().contains("opaque_r_scale") && err.to_string().contains("(0, 10]"),
            "the rejection names the field and the bounds: {err}"
        );
    }

    /// Two calls serialise to the same JSON bytes.
    #[test]
    fn input_schema_json_is_deterministic() {
        let first = serde_json::to_string(&schema()).expect("the schema serialises");
        let second = serde_json::to_string(&schema()).expect("the schema serialises");
        assert_eq!(first, second);
    }

    /// The schema's JSON round-trips.
    #[test]
    fn input_schema_json_round_trips() {
        let schema = schema();
        let json = serde_json::to_string(&schema).expect("serialises");
        let round: InputSchema = serde_json::from_str(&json).expect("round-trips");
        assert_eq!(round, schema);
    }

    /// The schedule columns' schemas are the column table's reads and the
    /// NotUsed entries (the hares-io half's own pins carry the table
    /// equality; this one pins the assembly carries them).
    #[test]
    fn input_schema_carries_the_schedule_columns() {
        let schema = schema();
        assert!(!schema.schedule_columns.is_empty());
        assert!(!schema.unused_schedule_columns.is_empty());
        assert!(
            schema
                .schedule_columns
                .iter()
                .any(|column| column.name == "occupants")
        );
    }

    /// The HPXML elements' schemas are the declared lists.
    #[test]
    fn input_schema_carries_the_hpxml_elements() {
        let schema = schema();
        assert!(!schema.hpxml_elements.is_empty());
        assert!(!schema.rejected_hpxml_elements.is_empty());
        // The choice children the parser reads and errors on: the
        // type-element dispatches' vocabulary is declared beside every
        // other element and published the same way.
        for name in [
            "AboveApartment",
            "Ambient",
            "Basement",
            "Crawlspace",
            "FlatRoof",
            "SlabOnGrade",
        ] {
            assert!(
                schema
                    .hpxml_elements
                    .iter()
                    .any(|element| element.name == name),
                "the schema's HPXML elements list the choice child {name}"
            );
        }
    }
}
