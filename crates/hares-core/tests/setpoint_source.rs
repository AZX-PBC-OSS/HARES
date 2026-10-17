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
//!
//! The generated-schedule tests run `tests/fixtures/resstock/2025.1/bldg0000002/home.xml`
//! with no schedule file: the generated schedule carries no setpoint column,
//! so each side resolves by the rule to the HPXML's own profiles (heating:
//! weekday 67 F at hours 0 to 2 and 5 to 12, 70 F at hours 3, 4 and 13 to
//! 17; weekend 70 F from hour 3 to hour 17; cooling: the constant 67 F
//! arrays), or, with the HVACControl removed, to the manual-thermostat
//! default profile.

use std::path::PathBuf;

use arrow::array::{Float64Array, RecordBatch};
use hares_core::{Dwelling, DwellingConfig, DwellingSettings};
use hares_physics::units::temperature_f_to_c;

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

/// The generated-schedule run's config: the ResStock home, no schedule file
/// (the schedule generated from the defaults' profiles), the home's county
/// weather, a Friday start, 3 d at 3600 s, no warm-up.
fn generated_sim_toml() -> String {
    r#"
start_time = "2018-01-05T00:00:00-05:00"
duration = 259200
time_res = 3600
warmup = "disabled"

[[reports]]
name = "frame"
selection = { verbosity = 8 }
destination = "memory"
"#
    .to_string()
}

fn dwelling_generated(hpxml: PathBuf) -> Dwelling {
    let config = DwellingConfig {
        hpxml: hares_io::InputSource::Path(hpxml),
        schedule: None,
        weather: hares_io::WeatherInput::Path(
            repo_root().join("tests/fixtures/resstock/2025.1/weather/G0900090_2018.csv"),
        ),
        defaults: hares_io::DefaultsSource::directory(repo_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(
            hares_io::SimulationConfig::from_toml(&generated_sim_toml())
                .expect("the config parses"),
            None,
            1,
            None,
        ),
    };
    Dwelling::from_config(config).expect("the dwelling builds")
}

/// The ResStock home's HPXML with its HVACControl element removed, written
/// to a temporary directory: the home declares no thermostat input at all.
fn home_xml_without_thermostat(dir: &tempfile::TempDir) -> PathBuf {
    let original = std::fs::read_to_string(
        repo_root().join("tests/fixtures/resstock/2025.1/bldg0000002/home.xml"),
    )
    .expect("the fixture's HPXML reads");
    let start = original
        .find("<HVACControl>")
        .expect("the fixture declares an HVACControl");
    let end = original
        .find("</HVACControl>")
        .expect("the HVACControl closes")
        + "</HVACControl>".len();
    let stripped = format!("{}{}", &original[..start], &original[end..]);
    assert!(
        !stripped.contains("SetpointTempsHeatingSeason"),
        "the stripped HPXML carries no thermostat setpoints"
    );
    let path = dir.path().join("home.xml");
    std::fs::write(&path, stripped).expect("the stripped HPXML writes");
    path
}

fn scheduled_heating_setpoints(dwelling: &mut Dwelling, expected_len: usize) -> Vec<f64> {
    dwelling.simulate().expect("the run completes");
    let batches: Vec<RecordBatch> = dwelling.flushed_batches().to_vec();
    let setpoints = column_values(&batches, "Gas Boiler Setpoint (C)");
    assert_eq!(setpoints.len(), expected_len, "the run's step count");
    setpoints
}

fn heating_source_is(dwelling: &Dwelling, equipment: &str, expected: &hares_io::SetpointSource) {
    let model = dwelling.resolved_model();
    let unit = model
        .equipment
        .iter()
        .find(|eq| eq.name == equipment)
        .unwrap_or_else(|| panic!("the read-back carries '{equipment}'"));
    assert_eq!(
        unit.heating_setpoint.as_ref(),
        Some(expected),
        "{equipment}'s heating source"
    );
}

fn cooling_source_is(dwelling: &Dwelling, equipment: &str, expected: &hares_io::SetpointSource) {
    let model = dwelling.resolved_model();
    let unit = model
        .equipment
        .iter()
        .find(|eq| eq.name == equipment)
        .unwrap_or_else(|| panic!("the read-back carries '{equipment}'"));
    assert_eq!(
        unit.cooling_setpoint.as_ref(),
        Some(expected),
        "{equipment}'s cooling source"
    );
}

fn dwelling_with_schedule(schedule_path: PathBuf) -> Dwelling {
    dwelling_with_schedule_at(schedule_path, "2023-01-01T00:00:00-07:00")
}

fn dwelling_with_schedule_at(schedule_path: PathBuf, start_time: &str) -> Dwelling {
    let sim_toml = format!(
        r#"
start_time = "{start_time}"
duration = 172800
time_res = 900

[[reports]]
name = "frame"
selection = {{ verbosity = 8 }}
destination = "memory"
"#
    );
    let config = DwellingConfig {
        hpxml: hares_io::InputSource::Path(fixture().join("building.xml")),
        schedule: Some(hares_io::InputSource::Path(schedule_path)),
        weather: hares_io::WeatherInput::Path(fixture().join("weather.epw")),
        defaults: hares_io::DefaultsSource::directory(repo_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(
            hares_io::SimulationConfig::from_toml(&sim_toml).expect("the config parses"),
            None,
            1,
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
/// OS-HPXML rule's record (hvac.rb:1669-1670). The column is read in
/// degrees Fahrenheit: the 70 F values run as the parser's Fahrenheit to
/// Celsius conversion, on bits.
#[test]
fn schedule_column_wins_over_hpxml_and_warns() {
    let dir = tempfile::tempdir().expect("temp dir");
    let schedule_path = schedule_with_appended_column(&dir, "heating_setpoint", 70.0);
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
    let expected = temperature_f_to_c(70.0);
    for (step, value) in setpoints.iter().enumerate() {
        assert_eq!(
            value.to_bits(),
            expected.to_bits(),
            "step {step}: the heating setpoint is the column's 70 F as the parser's \
             Fahrenheit to Celsius conversion, got {value}"
        );
    }
}

/// A schedule file's setpoint columns are read in degrees Fahrenheit, the
/// detailed-schedule convention OS-HPXML specifies and converts
/// (`schedules.rb:1465-1484`), OCHRE converting the same columns from
/// Fahrenheit at its own schedule load (`schedule.py:408-427`): with a
/// 70 F heating column every step's heating setpoint is, on bits, the uom
/// conversion of 70 F to Celsius computed here, and with a 75 F cooling
/// column the same for the cooler's active setpoint (the cooling side's
/// own column: the dwelling-level scheduled-cooling read reports the first
/// HVAC unit's telemetry, and the heating-only unit carries no cooling
/// source). The cooling leg runs from a July start so the compressor's
/// outdoor lockout (a January run never evaluates the cooler's control)
/// leaves the column's value the one the cooler acts on. Written for
/// Fahrenheit, the same files previously drove the dwelling at 70 C and
/// 75 C.
#[test]
fn setpoint_column_is_read_in_fahrenheit() {
    let expected_heating_c = temperature_f_to_c(70.0);
    let dir = tempfile::tempdir().expect("temp dir");
    let schedule_path = schedule_with_appended_column(&dir, "heating_setpoint", 70.0);
    let mut dwelling = dwelling_with_schedule(schedule_path);
    dwelling.simulate().expect("the run completes");
    let batches: Vec<RecordBatch> = dwelling.flushed_batches().to_vec();
    let heating = column_values(&batches, "Scheduled Heating Setpoint (C)");
    assert_eq!(heating.len(), 192, "48 h at 900 s");
    for (step, value) in heating.iter().enumerate() {
        assert_eq!(
            value.to_bits(),
            expected_heating_c.to_bits(),
            "step {step}: the 70 F heating column runs as the uom conversion to Celsius, \
             got {value}"
        );
    }

    let expected_cooling_c = temperature_f_to_c(75.0);
    let dir = tempfile::tempdir().expect("temp dir");
    let schedule_path = schedule_with_appended_column(&dir, "cooling_setpoint", 75.0);
    let mut dwelling = dwelling_with_schedule_at(schedule_path, "2023-07-01T00:00:00-07:00");
    dwelling.simulate().expect("the run completes");
    let batches: Vec<RecordBatch> = dwelling.flushed_batches().to_vec();
    let cooling = column_values(&batches, "ASHP Cooler Setpoint (C)");
    assert_eq!(cooling.len(), 192, "48 h at 900 s");
    for (step, value) in cooling.iter().enumerate() {
        assert_eq!(
            value.to_bits(),
            expected_cooling_c.to_bits(),
            "step {step}: the 75 F cooling column runs as the uom conversion to Celsius, \
             got {value}"
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

/// With no schedule file the generated schedule carries no setpoint column,
/// so the HPXML's hourly profiles are the scheduled setpoints: the
/// read-back's hourly heating and cooling values on bits the resolution's
/// Fahrenheit to Celsius conversion of the profile values, the HPXML
/// recorded as the active source, and the run's active heating setpoint
/// carrying those values to the boiler at the run's hours. The home's
/// heating profiles (home.xml:837-838): weekday 67 F at hours 0 to 2 and 5
/// to 12, 70 F at hours 3, 4 and 13 to 17; weekend 70 F from hour 3 to hour
/// 17 and 67 F around it; the cooling profiles the constant 67 F arrays
/// (home.xml:839-840). The home schedules heating and cooling at the same
/// temperatures, so the resolution's setpoint-pair rule (the 2 C gap the
/// thermostat requires, matching OCHRE's reconciliation) re-centers each
/// hour's pair on the midpoint with a symmetric 1 C offset; the expectation
/// applies the same rule to the converted values, and the 70 F hours stay
/// visible as the higher branch of both sides.
#[test]
fn generated_schedule_keeps_the_hpxml_setpoint_profile() {
    // The resolution's pair rule: a gap below 2 C re-centers the hour's pair
    // on the midpoint with a symmetric 1 C offset (the average built from
    // the heating and cooling values in that order).
    let cooling_profile_c = temperature_f_to_c(67.0);
    let reconcile = |heat_f: f64| -> (f64, f64) {
        let heat_c = temperature_f_to_c(heat_f);
        if cooling_profile_c - heat_c < 2.0 {
            let avg = 0.5 * (heat_c + cooling_profile_c);
            (avg - 1.0, avg + 1.0)
        } else {
            (heat_c, cooling_profile_c)
        }
    };
    let weekday_profile_f = [
        67.0, 67.0, 67.0, 70.0, 70.0, 67.0, 67.0, 67.0, 67.0, 67.0, 67.0, 67.0, 67.0, 70.0, 70.0,
        70.0, 70.0, 70.0, 67.0, 67.0, 67.0, 67.0, 67.0, 67.0,
    ];
    let weekend_profile_f = [
        67.0, 67.0, 67.0, 70.0, 70.0, 70.0, 70.0, 70.0, 70.0, 70.0, 70.0, 70.0, 70.0, 70.0, 70.0,
        70.0, 70.0, 70.0, 67.0, 67.0, 67.0, 67.0, 67.0, 67.0,
    ];
    let mut heating_weekday_c = Vec::new();
    let mut cooling_weekday_c = Vec::new();
    for hour in weekday_profile_f {
        let (heat, cool) = reconcile(hour);
        heating_weekday_c.push(heat);
        cooling_weekday_c.push(cool);
    }
    let mut heating_weekend_c = Vec::new();
    let mut cooling_weekend_c = Vec::new();
    for hour in weekend_profile_f {
        let (heat, cool) = reconcile(hour);
        heating_weekend_c.push(heat);
        cooling_weekend_c.push(cool);
    }

    // The read-back records the HPXML as the active source on both sides,
    // every hourly value on bits the conversion above: 03:00 on the Friday
    // is the weekday profile's hour 3 (the 70 F branch), 06:00 on the Friday
    // its hour 6 (the 67 F branch), 06:00 on the Saturday the weekend
    // profile's hour 6 (the 70 F branch).
    let dwelling =
        dwelling_generated(repo_root().join("tests/fixtures/resstock/2025.1/bldg0000002/home.xml"));
    heating_source_is(
        &dwelling,
        "Gas Boiler",
        &hares_io::SetpointSource::Hpxml {
            weekday_c: heating_weekday_c.clone(),
            weekend_c: heating_weekend_c.clone(),
        },
    );
    cooling_source_is(
        &dwelling,
        "Room AC",
        &hares_io::SetpointSource::Hpxml {
            weekday_c: cooling_weekday_c.clone(),
            weekend_c: cooling_weekend_c.clone(),
        },
    );

    // The run heats to the resolved profile: the boiler's active setpoint is
    // the resolved schedule's value at the run's hour, step k starting at
    // k:00 (3 d at 3600 s from the Friday 00:00 start: Friday, Saturday,
    // Sunday).
    let mut dwelling =
        dwelling_generated(repo_root().join("tests/fixtures/resstock/2025.1/bldg0000002/home.xml"));
    let setpoints = scheduled_heating_setpoints(&mut dwelling, 72);
    for (step, value) in setpoints.iter().enumerate() {
        let hour = step % 24;
        let weekend = step >= 24;
        let expected = if weekend {
            heating_weekend_c[hour]
        } else {
            heating_weekday_c[hour]
        };
        assert_eq!(
            value.to_bits(),
            expected.to_bits(),
            "step {step} (hour {hour}): the resolved profile's heating setpoint, got {value}"
        );
    }
}

/// The same home with its HVACControl removed declares no thermostat input
/// at all: with no schedule file either, both sides resolve to the
/// manual-thermostat default profile and record it as the active source,
/// the heating setpoint the profile's 68 F on every step (the 68 F heating
/// and 78 F cooling defaults' gap exceeds the deadband, so no
/// reconciliation moves the value).
#[test]
fn generated_schedule_without_thermostat_uses_the_default_profile() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut dwelling = dwelling_generated(home_xml_without_thermostat(&dir));
    let setpoints = scheduled_heating_setpoints(&mut dwelling, 72);
    let default_c = temperature_f_to_c(68.0);
    for (step, value) in setpoints.iter().enumerate() {
        assert_eq!(
            value.to_bits(),
            default_c.to_bits(),
            "step {step}: the default profile's 68 F heating setpoint, got {value}"
        );
    }
    let default_source = hares_io::SetpointSource::DefaultProfile {
        profile: hares_io::SetpointSource::MANUAL_THERMOSTAT.to_string(),
    };
    heating_source_is(&dwelling, "Gas Boiler", &default_source);
    cooling_source_is(&dwelling, "Room AC", &default_source);
}
