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
