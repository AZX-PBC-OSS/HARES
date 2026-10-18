//! A constant setpoint override replaces the schedule and HPXML setpoint
//! sources (the OD-2 site 6 acceptance).
//!
//! The thermostat reads a schedule column, then the HPXML profile, then the
//! config's constant setpoint (`hvac_core.rs` init's "Build setpoint
//! sources: CSV column > daily profile > constant"), so an override of
//! `setpoint.heating_setpoint_c` was accepted and never applied whenever
//! the resolution had stamped a source for the side. The merge
//! (`merged_equipment_config`) removes the matching `*_setpoint_source`
//! when the override sets the constant, so the override is the tier the
//! engine reads.
//!
//! The fixture is `tests/fixtures/hpxml/ochre_samples/base.xml` (heating
//! setpoint 68 F, cooling 78 F, no schedule file: the generated schedule
//! carries no setpoint column, so each side resolves to the HPXML tier),
//! over the Denver TMY3 weather, 24 h at 900 s, no warm-up.

use std::path::PathBuf;

use arrow::array::{Float64Array, RecordBatch};
use hares_core::{Dwelling, DwellingConfig, DwellingSettings};
use hares_io::{DefaultsSource, InputSource, WeatherInput};
use serde_json::json;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .parent()
        .expect("repo root")
        .to_path_buf()
}

/// base.xml over the Denver TMY3 weather with the repo defaults, a 24 h run
/// at 900 s starting 2023-01-15 00:00 UTC-7, no warm-up, one full-verbosity
/// frame report to memory, the dwelling-level overrides as given.
fn dwelling(overrides: Option<serde_json::Value>) -> Dwelling {
    let sim_toml = r#"
start_time = "2023-01-15T00:00:00-07:00"
duration = 86400
time_res = 900
warmup = "disabled"
master_seed = 0

[[reports]]
name = "frame"
selection = { verbosity = 8 }
destination = "memory"
"#
    .to_string();
    let config = DwellingConfig {
        hpxml: InputSource::Path(
            project_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"),
        ),
        schedule: None,
        weather: WeatherInput::Path(
            project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        ),
        defaults: DefaultsSource::directory(project_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(
            hares_io::SimulationConfig::from_toml(&sim_toml).expect("the config parses"),
            overrides,
            1,
            None,
        ),
    };
    Dwelling::from_config(config).expect("the dwelling builds")
}

fn column_values(batches: &[RecordBatch], name: &str) -> Vec<f64> {
    let mut values = Vec::new();
    for batch in batches {
        let Some(position) = batch
            .schema()
            .fields()
            .iter()
            .position(|field| field.name() == name)
        else {
            continue;
        };
        let column = batch
            .column(position)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap_or_else(|| panic!("'{name}' is a float64 column"));
        values.extend(column.iter().map(|v| v.unwrap_or(f64::NAN)));
    }
    values
}

/// The override's constant is the heating setpoint on every step: the
/// merge removed the HPXML setpoint source the resolution stamped, so the
/// constant is the tier the engine reads.
#[test]
fn constant_setpoint_override_applies() {
    let mut dwelling = dwelling(Some(json!({
        "Gas Furnace": {"setpoint": {"heating_setpoint_c": 25.0}},
    })));
    dwelling.simulate().expect("the run completes");
    let batches: Vec<RecordBatch> = dwelling.flushed_batches().to_vec();

    let setpoints = column_values(&batches, "Gas Furnace Setpoint (C)");
    assert_eq!(
        setpoints.len(),
        96,
        "one heating setpoint per step of the 24 h run"
    );
    for (step, setpoint) in setpoints.iter().enumerate() {
        assert_eq!(
            *setpoint, 25.0,
            "step {step}: the override's constant must be the heating setpoint"
        );
    }

    let scheduled = column_values(&batches, "Scheduled Heating Setpoint (C)");
    assert_eq!(scheduled.len(), 96, "one schedule-stage value per step");
    for (step, setpoint) in scheduled.iter().enumerate() {
        assert_eq!(
            *setpoint, 25.0,
            "step {step}: the schedule stage reads the override's constant"
        );
    }
}

/// Both sides named by constants whose gap violates the deadband is a
/// construction error: named-constant sides never move, so there is no
/// axis to open the gap on and a silent re-center would run a pair the
/// config did not set while the read-back recorded the constants as the
/// tiers the engine reads. The same both-named rule the control-signal
/// tier raises.
#[test]
fn both_named_constant_deadband_violation_is_a_construction_error() {
    let err = build_err(Some(json!({
        "Gas Furnace": {"setpoint": {
            "heating_setpoint_c": 25.0,
            "cooling_setpoint_c": 25.0,
        }},
    })));
    let message = err.to_string();
    assert!(
        message.contains("'Gas Furnace'"),
        "the error names the equipment: {message}"
    );
    assert!(
        message.contains("heating setpoint 25") && message.contains("cooling setpoint 25"),
        "the error names both constants: {message}"
    );
    assert!(
        message.contains("named constants are never moved"),
        "the error states the rule: {message}"
    );
}

/// A heating override against a schedule cooling column whose value sits
/// below it: the constant is the heating tier and the column the surviving
/// cooling tier, and the run delivers heating and cooling on the same
/// steps (the two units' tiers cross). The init's static check cannot see
/// the surviving schedule, so the run warns once at the first
/// simultaneous-delivery step, naming the equipment and both tiers, and
/// counts every such step in the run health.
#[test]
fn a_heating_override_across_the_schedule_cooling_column_warns_once_and_counts() {
    let dir = tempfile::tempdir().expect("temp dir");
    let schedule_path = dir.path().join("schedule.csv");
    let mut csv = String::from(
        "Time,occupants,clothes_washer,clothes_dryer,dishwasher,cooking_range,\
         hot_water_clothes_washer,hot_water_dishwasher,cooling_setpoint\n",
    );
    let start = chrono::DateTime::parse_from_rfc3339("2023-01-15T00:00:00-07:00")
        .expect("the start parses");
    for offset_s in (0..96).map(|step| 900 * step as i64) {
        let instant = start + chrono::Duration::seconds(offset_s);
        csv.push_str(&format!("{instant},1.0,0.1,0.1,0.1,0.1,0.1,0.1,68.0\n"));
    }
    std::fs::write(&schedule_path, &csv).expect("the schedule writes");

    let sim_toml = r#"
start_time = "2023-01-15T00:00:00-07:00"
duration = 86400
time_res = 900
warmup = "disabled"
master_seed = 0

[[reports]]
name = "frame"
selection = { verbosity = 8 }
destination = "memory"
"#
    .to_string();
    let config = DwellingConfig {
        hpxml: InputSource::Path(
            project_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"),
        ),
        schedule: Some(InputSource::Path(schedule_path)),
        weather: WeatherInput::Path(
            project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        ),
        defaults: DefaultsSource::directory(project_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(
            hares_io::SimulationConfig::from_toml(&sim_toml).expect("the config parses"),
            Some(json!({
                "Gas Furnace": {"setpoint": {"heating_setpoint_c": 25.0}},
            })),
            1,
            None,
        ),
    };
    let mut dwelling = Dwelling::from_config(config).expect("the dwelling builds");
    let run = dwelling.simulate().expect("the run completes");
    let batches: Vec<RecordBatch> = dwelling.flushed_batches().to_vec();

    // The probe's premise: the crossed tiers deliver heating and cooling
    // on the same steps of the January day.
    let heating = column_values(&batches, "HVAC Heating Delivered (W)");
    let cooling = column_values(&batches, "HVAC Cooling Delivered (W)");
    assert_eq!(heating.len(), 96, "one heating delivery per step");
    let simultaneous = heating
        .iter()
        .zip(cooling.iter())
        .filter(|(h, c)| **h > 0.0 && **c > 0.0)
        .count();
    assert!(
        simultaneous > 0,
        "the probe's premise: the January day delivers heating and cooling on the \
         same steps, got none"
    );

    // The warnings: each unit whose tiers cross warns once per run,
    // naming the equipment and both tiers. The furnace's surviving
    // cooling tier is the HPXML profile (78 F, 25.5556 C) the override
    // crosses; the air conditioner's surviving heating tier meets the
    // schedule column's 68 F (20 C) at the same value.
    let cross_tier: Vec<_> = run
        .warnings
        .iter()
        .filter(|w| w.message.contains("below the required deadband"))
        .collect();
    assert!(
        cross_tier.iter().any(|w| w.source.as_ref() == "Gas Furnace"
            && w.message.contains("heating setpoint 25")
            && w.message.contains("cooling setpoint 25.5555")),
        "the furnace's override crosses its surviving HPXML cooling tier: {:?}",
        cross_tier
            .iter()
            .map(|w| (&w.source, &w.message))
            .collect::<Vec<_>>()
    );
    assert!(
        cross_tier
            .iter()
            .any(|w| w.source.as_ref() == "Air Conditioner"
                && w.message.contains("cooling setpoint 20")),
        "the schedule column's cooling tier is named: {:?}",
        cross_tier
            .iter()
            .map(|w| (&w.source, &w.message))
            .collect::<Vec<_>>()
    );
    for source in ["Gas Furnace", "Air Conditioner"] {
        let count = cross_tier
            .iter()
            .filter(|w| w.source.as_ref() == source)
            .count();
        assert!(
            count <= 1,
            "the collision warns once per run per equipment: {source} warned {count} times"
        );
    }
    for warning in &cross_tier {
        assert!(
            warning.step_index.is_some(),
            "the warning is stamped with the step it fired in: {warning:?}"
        );
    }

    // The count: every collision step, not just the first.
    let simultaneous = u64::try_from(simultaneous).expect("the step count is positive");
    assert!(
        run.health.cross_tier_deadband_events >= simultaneous,
        "the run health counts the collision steps: {} against {simultaneous}",
        run.health.cross_tier_deadband_events
    );
}

/// The merge-time scan: a surviving schedule whose knowable coldest step
/// sits below the override's heating constant warns at construction,
/// naming both values; a schedule kind whose values the config does not
/// carry (a schedule-file column) leaves the crossing to the runtime's
/// count.
#[test]
fn a_constant_across_the_surviving_schedule_warns_at_construction() {
    let mut dwelling = dwelling(Some(json!({
        "Gas Furnace": {"setpoint": {"heating_setpoint_c": 26.0}},
    })));
    let run = dwelling.simulate().expect("the run completes");
    let scanned: Vec<_> = run
        .warnings
        .iter()
        .filter(|w| w.message.contains("crosses the surviving cooling schedule"))
        .collect();
    assert_eq!(scanned.len(), 1, "one scan warning for the furnace");
    let warning = scanned[0];
    assert_eq!(
        warning.source.as_ref(),
        "Gas Furnace",
        "the warning names the equipment in its source"
    );
    let message = warning.message.clone();
    assert!(
        message.contains("26") && message.contains("25.5555"),
        "the warning names both values: {message}"
    );
}

fn build_err(overrides: Option<serde_json::Value>) -> hares_types::HaresError {
    let sim_toml = r#"
start_time = "2023-01-15T00:00:00-07:00"
duration = 86400
time_res = 900
warmup = "disabled"
master_seed = 0

[[reports]]
name = "frame"
selection = { verbosity = 8 }
destination = "memory"
"#
    .to_string();
    let config = DwellingConfig {
        hpxml: InputSource::Path(
            project_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"),
        ),
        schedule: None,
        weather: WeatherInput::Path(
            project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        ),
        defaults: DefaultsSource::directory(project_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(
            hares_io::SimulationConfig::from_toml(&sim_toml).expect("the config parses"),
            overrides,
            1,
            None,
        ),
    };
    match Dwelling::from_config(config) {
        Err(err) => err,
        Ok(_) => panic!("the dwelling must not build"),
    }
}

/// A schedule column's setpoint is no exemption: the override replaces the
/// schedule-file source the same way. The run carries a constant
/// `heating_setpoint` column of 68 F (20 C after the parse's Fahrenheit
/// conversion); the override's 25 C must be the tier the engine reads.
#[test]
fn constant_setpoint_override_replaces_the_schedule_column() {
    let dir = tempfile::tempdir().expect("temp dir");
    let schedule_path = dir.path().join("schedule.csv");
    let mut csv = String::from(
        "Time,occupants,clothes_washer,clothes_dryer,dishwasher,cooking_range,\
         hot_water_clothes_washer,hot_water_dishwasher,heating_setpoint\n",
    );
    let start = chrono::DateTime::parse_from_rfc3339("2023-01-15T00:00:00-07:00")
        .expect("the start parses");
    for offset_s in (0..96).map(|step| 900 * step as i64) {
        let instant = start + chrono::Duration::seconds(offset_s);
        csv.push_str(&format!("{instant},1.0,0.1,0.1,0.1,0.1,0.1,0.1,68.0\n"));
    }
    std::fs::write(&schedule_path, &csv).expect("the schedule writes");

    let sim_toml = r#"
start_time = "2023-01-15T00:00:00-07:00"
duration = 86400
time_res = 900
warmup = "disabled"
master_seed = 0

[[reports]]
name = "frame"
selection = { verbosity = 8 }
destination = "memory"
"#
    .to_string();
    let config = DwellingConfig {
        hpxml: InputSource::Path(
            project_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"),
        ),
        schedule: Some(InputSource::Path(schedule_path)),
        weather: WeatherInput::Path(
            project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        ),
        defaults: DefaultsSource::directory(project_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(
            hares_io::SimulationConfig::from_toml(&sim_toml).expect("the config parses"),
            Some(json!({
                "Gas Furnace": {"setpoint": {"heating_setpoint_c": 25.0}},
            })),
            1,
            None,
        ),
    };
    let mut dwelling = Dwelling::from_config(config).expect("the dwelling builds");
    dwelling.simulate().expect("the run completes");
    let batches: Vec<RecordBatch> = dwelling.flushed_batches().to_vec();

    let setpoints = column_values(&batches, "Gas Furnace Setpoint (C)");
    assert_eq!(
        setpoints.len(),
        96,
        "one heating setpoint per step of the 24 h run"
    );
    for (step, setpoint) in setpoints.iter().enumerate() {
        assert_eq!(
            *setpoint, 25.0,
            "step {step}: the override must replace the schedule column"
        );
    }
}
