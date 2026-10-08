//! Dwelling assembly fails with a typed error naming what it could not use;
//! it never drops equipment or substitutes a fallback and carries on.

use std::path::PathBuf;

use chrono::{DateTime, Duration, FixedOffset};
use hares_core::dwelling::DwellingBlueprint;
use hares_core::{DwellingConfig, SimulationConfig};
use hares_io::OutputFormat;
use hares_types::{EndUse, FuelType};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cz2a_config(defaults: PathBuf) -> DwellingConfig {
    let fixture = project_root().join("tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh");
    DwellingConfig {
        hpxml_path: fixture.join("building.xml"),
        schedule_path: Some(fixture.join("schedule.csv")),
        weather_path: fixture.join("weather.epw"),
        defaults_path: Some(defaults),
        sim_config: SimulationConfig {
            start_time: DateTime::<FixedOffset>::parse_from_rfc3339("2023-01-01T00:00:00-07:00")
                .expect("start time parses"),
            duration: Duration::hours(1),
            time_res: Duration::seconds(900),
            output_verbosity: 0,
            write_output: false,
            output_path: None,
            output_format: OutputFormat::Csv,
            output_chunk_size: 1024,
            setpoint_deadband_c: None,
            master_seed: 0,
            civil_timezone: None,
            site_location: hares_io::SiteLocationOverride::default(),
            retain_batches: false,
            rotation: hares_io::RotationPolicy::None,
            max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
        },
        overrides: None,
        bldg_id: 1,
        initialization_duration: None,
        resample_overrides: None,
        patches: None,
    }
}

/// A ventilation fan whose typed config cannot be read fails the build,
/// instead of falling back to its raw parameters.
#[test]
fn unreadable_ventilation_config_fails_the_build() {
    let mut blueprint =
        DwellingBlueprint::from_config(cz2a_config(project_root().join("defaults")))
            .expect("blueprint");
    blueprint.remove_equipment_by_end_use(&[EndUse::VENTILATION]);
    let typed = hares_equipment::EquipmentConfig::with_payload(
        "Ventilation Fan".to_string(),
        "Ventilation Fan".to_string(),
        hares_equipment::ConfigPayload::Typed {
            type_name: "Ventilation".to_string(),
            version: 1,
            data: serde_json::json!({ "flow_rate_m3_s": "not a number" }),
        },
    );
    blueprint
        .add_equipment_spec(hares_io::EquipmentSpec {
            name: "Ventilation Fan".to_string(),
            instance_name: None,
            fuel_type: FuelType::Electric,
            parameters: serde_json::Map::new(),
            zip_params: None,
            typed_overrides: serde_json::Map::new(),
            typed_config: Some(typed),
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        })
        .expect("add spec");
    let err = blueprint
        .build()
        .err()
        .expect("an unreadable ventilation config must fail the build");
    assert!(
        err.to_string().contains("Ventilation Fan"),
        "the error names the ventilation fan, got: {err}"
    );
}

/// Basement Lighting follows the interior lighting column when the
/// schedule has no basement column. A schedule whose column index already
/// claims a basement column its names lack cannot take the copy, and the
/// build fails naming it instead of running the basement lights without a
/// schedule. Only a schedule changed through the blueprint's public field
/// can be inconsistent this way; a parsed one cannot.
#[test]
fn a_basement_lighting_schedule_that_cannot_be_copied_fails_the_build() {
    // A ResStock home with a conditioned basement, whose schedule has
    // `lighting_interior` and no `lighting_basement`; its HPXML has no
    // basement lighting group, so the lights are added to the blueprint.
    let home = project_root().join("tests/fixtures/resstock/2025.1/bldg0000011");
    let mut config = cz2a_config(project_root().join("defaults"));
    config.hpxml_path = home.join("home.xml");
    config.schedule_path = Some(home.join("in.schedules.csv"));
    config.weather_path =
        project_root().join("tests/fixtures/resstock/2025.1/weather/G3901530_2018.csv");
    config.sim_config.start_time =
        DateTime::<FixedOffset>::parse_from_rfc3339("2018-01-01T00:00:00-05:00")
            .expect("start time parses");
    let mut blueprint = DwellingBlueprint::from_config(config).expect("blueprint");
    assert!(
        blueprint
            .schedule
            .column_index
            .contains_key("lighting_interior")
    );
    assert!(
        !blueprint
            .schedule
            .column_index
            .contains_key("lighting_basement")
    );
    blueprint
        .add_equipment_spec(hares_io::EquipmentSpec {
            name: "Basement Lighting".to_string(),
            instance_name: None,
            fuel_type: FuelType::Electric,
            parameters: serde_json::Map::new(),
            zip_params: None,
            typed_overrides: serde_json::Map::new(),
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        })
        .expect("add spec");
    blueprint
        .schedule
        .column_index
        .insert("lighting_basement".to_string(), 0);

    let err = blueprint
        .build()
        .err()
        .expect("a basement lighting copy that cannot land must fail the build");
    let message = err.to_string();
    assert!(
        message.contains("lighting_basement") && message.contains("already exists"),
        "the error names the copy and why it failed, got: {message}"
    );
}
