//! The thermostat setpoints resolve by the OpenStudio-HPXML rule, run
//! level: a schedule-file column wins over the HPXML setpoints and the
//! ignored HPXML input is named in a warning; the values the engine runs
//! are the column's own.
//!
//! The rule is OpenStudio-HPXML's `HPXMLtoOpenStudio/resources/hvac.rb`
//! `apply_setpoints` (master `57bd1b88`, lines 1641-1741): per side, a
//! schedule-file column when present, else the HPXML setpoints, else the
//! default profile; when both a column and HPXML setpoints are given it
//! warns that the latter will be ignored (hvac.rb:1669-1675). The fixture
//! is `tests/fixtures/parity/cz4a_ashp_hpwh`, whose HPXML declares hourly
//! `Weekday/WeekendSetpointTempsHeatingSeason` arrays and a simple
//! `SetpointTempCoolingSeason`, and whose schedule.csv carries no setpoint
//! column; each test appends one constant setpoint column and runs the
//! dwelling on it.

use std::path::PathBuf;

use arrow::array::{Float64Array, RecordBatch};
use hares_core::{Dwelling, DwellingConfig, DwellingSettings};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .parent()
        .expect("repo root")
        .to_path_buf()
}

fn fixture() -> PathBuf {
    repo_root().join("tests/fixtures/parity/cz4a_ashp_hpwh")
}

/// The fixture's schedule.csv with one constant column appended, written to
/// a temporary directory the config reads the file from.
fn schedule_with_appended_column(
    dir: &tempfile::TempDir,
    column: &str,
    value: f64,
) -> std::path::PathBuf {
    let original = std::fs::read_to_string(fixture().join("schedule.csv"))
        .expect("the fixture's schedule reads");
    let mut out = String::with_capacity(original.len() + 32);
    let mut lines = original.lines();
    let header = lines.next().expect("the schedule has a header");
    assert!(
        !header.contains(column),
        "the fixture's schedule has no {column} column to start from"
    );
    out.push_str(&format!("{header},{column}\n"));
    for line in lines {
        if line.is_empty() {
            continue;
        }
        out.push_str(&format!("{line},{value}\n"));
    }
    let path = dir.path().join("schedule.csv");
    std::fs::write(&path, out).expect("the appended schedule writes");
    path
}

fn sim_toml() -> String {
    r#"
start_time = "2023-01-01T00:00:00-07:00"
duration = 172800
time_res = 900

[[reports]]
name = "frame"
selection = { verbosity = 8 }
destination = "memory"
"#
    .to_string()
}

fn dwelling_with_schedule(schedule_path: PathBuf) -> Dwelling {
    let config = DwellingConfig {
        hpxml: hares_io::InputSource::Path(fixture().join("building.xml")),
        schedule: Some(hares_io::InputSource::Path(schedule_path)),
        weather: hares_io::WeatherInput::Path(fixture().join("weather.epw")),
        defaults: hares_io::DefaultsSource::directory(repo_root().join("defaults")),
        settings: DwellingSettings::new(
            hares_io::SimulationConfig::from_toml(&sim_toml()).expect("the config parses"),
            None,
            1,
            None,
            None,
        ),
    };
    Dwelling::from_config(config).expect("the dwelling builds")
}

fn column_values(batches: &[RecordBatch], name: &str) -> Vec<f64> {
    batches
        .first()
        .map(|first| {
            let column = first
                .schema()
                .fields()
                .iter()
                .position(|field| field.name() == name)
                .unwrap_or_else(|| panic!("the schema carries '{name}'"));
            batches
                .iter()
                .flat_map(|batch| {
                    batch
                        .column(column)
                        .as_any()
                        .downcast_ref::<Float64Array>()
                        .unwrap_or_else(|| panic!("'{name}' is a float64 column"))
                        .iter()
                        .map(|v| v.unwrap_or(f64::NAN))
                        .collect::<Vec<f64>>()
                })
                .collect::<Vec<f64>>()
        })
        .unwrap_or_default()
}

/// With a heating setpoint column and HPXML heating setpoints both given,
/// the run's heating setpoint is the column's value on every step and one
/// warning names the column and the ignored HPXML heating elements: the
/// OS-HPXML rule's record (hvac.rb:1669-1670).
#[test]
fn schedule_column_wins_over_hpxml_and_warns() {
    let dir = tempfile::tempdir().expect("temp dir");
    let schedule_path = schedule_with_appended_column(&dir, "heating_setpoint", 21.0);
    let mut dwelling = dwelling_with_schedule(schedule_path);
    dwelling.simulate().expect("the run completes");

    let warnings = dwelling.take_warnings();
    let overlaps: Vec<_> = warnings
        .iter()
        .filter(|w| w.message.contains("heating_setpoint") && w.message.contains("ignored"))
        .collect();
    assert_eq!(
        overlaps.len(),
        1,
        "one warning names the column and the ignored HPXML heating elements, got {warnings:?}"
    );
    assert!(
        overlaps[0]
            .message
            .contains("WeekdaySetpointTempsHeatingSeason"),
        "the warning names the ignored HPXML heating elements: {}",
        overlaps[0].message
    );

    let batches: Vec<RecordBatch> = dwelling.flushed_batches().to_vec();
    let setpoints = column_values(&batches, "Scheduled Heating Setpoint (C)");
    assert_eq!(setpoints.len(), 192, "48 h at 900 s");
    for (step, value) in setpoints.iter().enumerate() {
        assert!(
            (*value - 21.0).abs() < 1e-12,
            "step {step}: the heating setpoint is the column's 21.0 C, got {value}"
        );
    }
}

/// Without a setpoint column the run's setpoints are the HPXML's: the
/// heating profile's 65 F nights and 71 F days (building.xml:794-795),
/// with no setpoint warning.
#[test]
fn hpxml_setpoints_used_without_a_column() {
    let mut dwelling = dwelling_with_schedule(fixture().join("schedule.csv"));
    dwelling.simulate().expect("the run completes");

    let warnings = dwelling.take_warnings();
    assert!(
        !warnings
            .iter()
            .any(|w| w.message.contains("setpoint") && w.message.contains("ignored")),
        "no setpoint warning without an overlap, got {warnings:?}"
    );

    let batches: Vec<RecordBatch> = dwelling.flushed_batches().to_vec();
    let setpoints = column_values(&batches, "Scheduled Heating Setpoint (C)");
    assert_eq!(setpoints.len(), 192, "48 h at 900 s");
    let night = (65.0 - 32.0) * 5.0 / 9.0;
    let day = (71.0 - 32.0) * 5.0 / 9.0;
    for (step, value) in setpoints.iter().enumerate() {
        // The step's hour within the run's first 48 h: hour h of day d,
        // the local wall clock the schedule's profiles follow.
        let hour = (step / 4) % 24;
        let expected = if (6..=22).contains(&hour) { day } else { night };
        assert!(
            (*value - expected).abs() < 1e-9,
            "step {step} (hour {hour}): the heating setpoint is the HPXML profile's \
             {expected} C, got {value}"
        );
    }
}
