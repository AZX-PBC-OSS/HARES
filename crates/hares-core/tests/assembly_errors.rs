//! Dwelling assembly fails with a typed error naming what it could not use;
//! it never drops equipment or substitutes a fallback and carries on.

use hares_core::DwellingSettings;
use hares_io::{DefaultsSource, InputSource, WeatherInput};
use std::path::PathBuf;

use chrono::{DateTime, Duration, FixedOffset};
use hares_core::dwelling::{DwellingBlueprint, DwellingBlueprintBuilder};
use hares_core::{DwellingConfig, SimulationConfig};
use hares_types::{EndUse, FuelType};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cz2a_config(defaults: PathBuf) -> DwellingConfig {
    let fixture = project_root().join("tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh");
    DwellingConfig {
        hpxml: InputSource::Path(fixture.join("building.xml")),
        schedule: Some(InputSource::Path(fixture.join("schedule.csv"))),
        weather: WeatherInput::Path(fixture.join("weather.epw")),
        defaults: DefaultsSource::directory(defaults),
        settings: DwellingSettings::new(
            SimulationConfig {
                start_time: DateTime::<FixedOffset>::parse_from_rfc3339(
                    "2023-01-01T00:00:00-07:00",
                )
                .expect("start time parses"),
                duration: Duration::hours(1),
                time_res: Duration::seconds(900),
                reports: Vec::new(),
                diagnostics: false,
                metrics: false,
                output_chunk_size: 1024,
                setpoint_deadband_c: None,
                master_seed: 0,
                civil_timezone: None,
                site_location: hares_io::SiteLocationOverride::default(),
                max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                health_policy: hares_io::HealthPolicy::default(),
                first_hour_interpolation: Default::default(),
                weather_year: hares_io::WeatherYear::default(),
            },
            None,
            1,
            None,
            None,
        ),
    }
}

/// A ventilation fan whose typed config cannot be read fails the build,
/// instead of falling back to its raw parameters.
#[test]
fn unreadable_ventilation_config_fails_the_build() {
    let builder =
        DwellingBlueprintBuilder::from_config(cz2a_config(project_root().join("defaults")))
            .expect("blueprint");
    let mut builder = builder;
    builder.remove_equipment_by_end_use(&[EndUse::VENTILATION]);
    let typed = hares_equipment::EquipmentConfig::with_payload(
        "Ventilation Fan".to_string(),
        "Ventilation Fan".to_string(),
        hares_equipment::ConfigPayload::Typed {
            type_name: "Ventilation".to_string(),
            version: 1,
            data: serde_json::json!({ "flow_rate_m3_s": "not a number" }),
        },
    );
    builder
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
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
        })
        .expect("add spec");
    let err = builder
        .freeze()
        .and_then(|blueprint| blueprint.build())
        .err()
        .expect("an unreadable ventilation config must fail the build");
    assert!(
        err.to_string().contains("Ventilation Fan"),
        "the error names the ventilation fan, got: {err}"
    );
}

/// A site UTC offset outside the range a fixed offset can represent is a
/// construction error naming the value; it never silently becomes UTC.
#[test]
fn an_out_of_range_utc_offset_is_a_construction_error() {
    let mut config = cz2a_config(project_root().join("defaults"));
    config.settings.sim_config.site_location.utc_offset_h = Some(30.0);
    let err = DwellingBlueprint::from_config(config)
        .err()
        .expect("a 30 h UTC offset cannot become a fixed offset");
    assert!(
        err.to_string().contains("30"),
        "the error names the value, got: {err}"
    );
}

/// A non-finite site UTC offset is a construction error naming the value;
/// it never rounds to zero and silently becomes UTC.
#[test]
fn a_non_finite_utc_offset_is_a_construction_error() {
    let mut config = cz2a_config(project_root().join("defaults"));
    config.settings.sim_config.site_location.utc_offset_h = Some(f64::NAN);
    let err = DwellingBlueprint::from_config(config)
        .err()
        .expect("a NaN UTC offset cannot become a fixed offset");
    assert!(
        err.to_string().contains("NaN"),
        "the error names the value, got: {err}"
    );
}

/// An offset that pre-rounding sits inside ±24 h but rounds to exactly
/// 86 400 s (the boundary `FixedOffset::east_opt` rejects) is a
/// construction error naming the value; it never reaches the type's
/// constructor and panics there.
#[test]
fn an_offset_that_rounds_to_24_hours_is_a_construction_error() {
    let mut config = cz2a_config(project_root().join("defaults"));
    // 23.9999 h is 86 399.64 s: inside the pre-rounding guard, and exactly
    // 86 400 s once rounded.
    config.settings.sim_config.site_location.utc_offset_h = Some(23.9999);
    let err = DwellingBlueprint::from_config(config)
        .err()
        .expect("an offset rounding to 86 400 s cannot become a fixed offset");
    assert!(
        err.to_string().contains("23.9999"),
        "the error names the value, got: {err}"
    );
}
