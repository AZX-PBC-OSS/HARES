//! The IEEE 1547 DER response layer end to end: a scenario override's
//! `der_1547` block configures the PV's inverter, and the per-DER trip
//! state surfaces in the dwelling's per-equipment telemetry. The
//! unconfigured control run pins the pre-1547 contract (the inverter
//! rides the excursion, the run's first excursion is counted).

use hares_core::{Dwelling, DwellingConfig, DwellingSettings};
use hares_io::{DefaultsSource, InputSource, WeatherInput};
use serde_json::json;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sim_config() -> hares_io::SimulationConfig {
    hares_io::SimulationConfig::from_toml(
        r#"
start_time = "2023-06-01T00:00:00-07:00"
duration = 21600
time_res = 900
"#,
    )
    .expect("the sim config parses")
}

/// The base-pv-battery sample dwelling (the DER golden fixture's inputs).
fn pv_battery_config(overrides: serde_json::Value) -> DwellingConfig {
    let root = repo_root();
    DwellingConfig {
        hpxml: InputSource::Path(
            root.join("tests/fixtures/hpxml/ochre_samples/base-pv-battery.xml"),
        ),
        schedule: Some(InputSource::Path(
            root.join("data/examples/BEopt_example_schedule.csv"),
        )),
        weather: WeatherInput::Path(
            root.join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        ),
        defaults: DefaultsSource::directory(root.join("python/hares/defaults")),
        settings: DwellingSettings::new(sim_config(), Some(overrides), 1, None),
    }
}

fn pv_telemetry(dwelling: &Dwelling) -> &hares_types::Telemetry {
    let env = dwelling.latest_env();
    let entry = env
        .equipment_telemetry
        .iter()
        .find(|(name, _)| name.contains("PV"))
        .expect("the PV's telemetry snapshot is populated after a step");
    entry.1
}

/// The brownout round's F8 finding, end to end: a 0.6 pu sag (inside
/// UV1's 0.70 pu band; the Category II default clearing time 10 s is
/// reached within a 900 s step) trips the configured PV's inverter, and
/// the trip state reads in the per-equipment telemetry: `der1547_tripped`
/// 1, `der1547_trip_reason` uv1, the ride-through accumulator at the 10 s
/// window. The unconfigured control run rides the same sag with the
/// inverter online and counts the excursion.
#[test]
fn der_1547_override_trips_the_pv_and_the_trip_state_reads_end_to_end() {
    // The configured run: the scenario's PV override carries the 1547
    // block (the Category II defaults; no overrides).
    let config = pv_battery_config(json!({
        "PV": {"der_1547": {"category": "II"}},
        "Battery": {"bms_mode": "\"Manual\""},
    }));
    let mut dwelling = Dwelling::from_config(config).expect("construction");
    dwelling.step().expect("the first step");
    dwelling
        .set_grid_voltage(0.6)
        .expect("the sag override is in contract");
    dwelling.step().expect("the sag step");

    let telemetry = pv_telemetry(&dwelling);
    assert_eq!(
        telemetry.get("der1547_tripped"),
        Some(1.0),
        "the 0.6 pu sag trips the configured inverter, end to end"
    );
    assert_eq!(
        telemetry.get("der1547_trip_reason"),
        Some(2.0),
        "uv1 is the fired condition"
    );
    assert_eq!(
        telemetry.get("der1547_ride_through_s"),
        Some(10.0),
        "the accumulator caps at the 10 s window"
    );
    assert_eq!(telemetry.get("der1547_unconfigured_excursions"), Some(0.0));

    // The dense telemetry's per-DER trip channel carries the same state,
    // aligned with the equipment names.
    let dense = dwelling.telemetry().expect("dense telemetry");
    let idx = dense
        .equipment_names
        .iter()
        .position(|n| n.contains("PV"))
        .expect("the PV is in the dense telemetry's names");
    assert_eq!(dense.equipment_der1547_tripped[idx], 1.0);

    // The unconfigured control run: the pre-1547 contract (the inverter
    // rides the excursion as a pinned source) with the excursion counted.
    let config = pv_battery_config(json!({
        "Battery": {"bms_mode": "\"Manual\""},
    }));
    let mut dwelling = Dwelling::from_config(config).expect("construction");
    dwelling.step().expect("the first step");
    dwelling
        .set_grid_voltage(0.6)
        .expect("the sag override is in contract");
    dwelling.step().expect("the sag step");

    let telemetry = pv_telemetry(&dwelling);
    assert_eq!(
        telemetry.get("der1547_tripped"),
        Some(0.0),
        "unconfigured: the pre-1547 pinned-source behavior"
    );
    assert_eq!(
        telemetry.get("der1547_unconfigured_excursions"),
        Some(1.0),
        "the run's first excursion is one counted event"
    );
}
