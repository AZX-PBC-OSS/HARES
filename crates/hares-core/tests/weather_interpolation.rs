//! Weather reads follow the EnergyPlus interpolation rules for weather
//! files: a sub-hourly file's records are used directly, every step reads
//! the record covering it, and an interval that differs from the step is
//! a construction error. The differentials here run a sub-hourly weather
//! file whose read differs between the engine's previous rule and
//! EnergyPlus's: they fail before the change and match after.

use std::path::PathBuf;

use arrow::array::{Array, Float64Array, RecordBatch};
use chrono::{DateTime, Duration};
use hares_core::{Dwelling, DwellingConfig, DwellingSettings, SimulationConfig, resample_weather};
use hares_io::FirstHour;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cz4a_dir() -> PathBuf {
    project_root().join("tests/fixtures/parity/cz4a_ashp_hpwh")
}

fn psm3_fixture() -> PathBuf {
    project_root().join("tests/fixtures/weather/psm3_15min_2019.csv")
}

fn config(time_res: i64, duration_secs: i64, start: &str) -> SimulationConfig {
    SimulationConfig {
        start_time: DateTime::parse_from_rfc3339(start).expect("valid start time"),
        duration: Duration::seconds(duration_secs),
        time_res: Duration::seconds(time_res),
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
        warmup: hares_io::Warmup::Disabled,
    }
}

/// The run's resolved site location against one weather file's header,
/// the resolution the build itself runs.
fn resolved_site(building: &std::path::Path, weather: &std::path::Path) -> hares_io::SiteLocation {
    let parsed_building = hares_io::parse_hpxml(building).expect("the fixture parses");
    let meta = hares_io::read_weather_meta(weather).expect("the weather header parses");
    hares_io::resolve_site_location(
        &parsed_building.site,
        &meta,
        &hares_io::SiteLocationOverride::default(),
    )
}

fn dry_bulb_bits(batch: &RecordBatch) -> Vec<u64> {
    let column = batch
        .column_by_name("Outdoor Dry Bulb (C)")
        .unwrap_or_else(|| panic!("the batch carries the outdoor dry bulb"));
    let floats = column
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("a weather channel is f64");
    floats
        .iter()
        .map(|value| value.expect("no null weather channel").to_bits())
        .collect()
}

/// A sub-hourly EPW file at a step equal to its interval: every step
/// reads the record covering it, in file order, from the run's first
/// step on. No record wraps to the file's end and no record lags.
#[test]
fn subhourly_epw_record_covers_the_step() {
    let weather = cz4a_dir().join("weather_ochre_hold_60s.epw");
    let site = resolved_site(&cz4a_dir().join("building.xml"), &weather);
    let cfg = config(60, 86_400, "1995-01-01T00:00:00-07:00");
    let batch = resample_weather(&hares_io::WeatherInput::Path(weather.clone()), &site, &cfg)
        .expect("the run completes");
    let parsed = hares_io::parse_weather(&weather).expect("the weather parses");
    let read = dry_bulb_bits(&batch);
    assert_eq!(read.len(), 1440, "one row per step of the day");
    for (step, record) in parsed.dry_bulb_c.iter().take(1440).enumerate() {
        assert_eq!(
            read[step],
            record.to_bits(),
            "step {step} reads the record covering it"
        );
    }
}

/// A sub-hourly file whose interval differs from the step is a
/// construction error naming both intervals, as EnergyPlus refuses the
/// run when the weather file's intervals per hour differ from the
/// configured timesteps per hour.
#[test]
fn subhourly_file_interval_must_equal_the_step() {
    let weather = hares_io::WeatherInput::Path(psm3_fixture());
    let site = resolved_site(&cz4a_dir().join("building.xml"), &psm3_fixture());
    for time_res in [300, 1800] {
        let err = resample_weather(
            &weather,
            &site,
            &config(time_res, 86_400, "2019-01-01T00:00:00-07:00"),
        )
        .expect_err("the interval mismatch is a construction error");
        let message = err.to_string();
        assert!(
            message.contains("900"),
            "the error names the file's interval: {message}"
        );
        assert!(
            message.contains(&time_res.to_string()),
            "the error names the run's interval: {message}"
        );
    }
}

/// A sub-hourly PSM3 file at a step equal to its interval: record k
/// covers the interval it starts, so step k reads record k. The
/// fixture's dry bulb is -20.0 + 0.5 * (k mod 96) over a full 2019 year.
#[test]
fn psm3_record_covers_the_interval_it_starts() {
    let weather = hares_io::WeatherInput::Path(psm3_fixture());
    let site = resolved_site(&cz4a_dir().join("building.xml"), &psm3_fixture());
    let cfg = config(900, 86_400, "2019-01-01T00:00:00-07:00");
    let batch = resample_weather(&weather, &site, &cfg).expect("the run completes");
    let read = dry_bulb_bits(&batch);
    assert_eq!(read.len(), 96, "one row per step of the day");
    for (step, expected) in read.iter().enumerate() {
        let expected_c = -20.0 + 0.5 * (step % 96) as f64;
        assert_eq!(*expected, expected_c.to_bits(), "step {step}");
    }
}

/// A step shorter than an hour must divide the hour: the config's
/// validation names the value and the rule; a step that divides the hour
/// and an hourly step pass.
#[test]
fn subhour_step_must_divide_the_hour() {
    let mut cfg = config(420, 20_160, "2023-01-01T00:00:00-07:00");
    let err = cfg.validate().expect_err("420 s does not divide the hour");
    assert!(
        err.to_string().contains("420"),
        "the error names the value: {err}"
    );
    assert!(
        err.to_string().contains("divide the hour"),
        "the error names the rule: {err}"
    );
    cfg.time_res = Duration::seconds(900);
    cfg.duration = Duration::seconds(86_400);
    cfg.validate().expect("900 s divides the hour");
    cfg.time_res = Duration::seconds(3600);
    cfg.validate().expect("an hourly step passes");
}

/// The run's own outdoor dry-bulb column: at an hourly step every step
/// reads the record of the hour it ends in, from the run's first row on.
#[test]
fn hourly_step_reads_the_record_of_the_hour_it_ends() {
    let weather_path = cz4a_dir().join("weather.epw");
    let parsed = hares_io::parse_weather(&weather_path).expect("the weather parses");
    let mut cfg = config(3600, 86_400, "2023-01-01T00:00:00-07:00");
    cfg.reports = vec![hares_io::ReportSpec {
        name: "weather".to_string(),
        selection: hares_io::Selection::Named(vec!["Outdoor Dry Bulb (C)".to_string()]),
        frequency: hares_io::Frequency::Timestep,
        destination: hares_io::Destination::Memory,
    }];
    let mut dwelling = Dwelling::from_config(DwellingConfig {
        hpxml: hares_io::InputSource::Path(cz4a_dir().join("building.xml")),
        schedule: Some(hares_io::InputSource::Path(cz4a_dir().join("schedule.csv"))),
        weather: hares_io::WeatherInput::Path(weather_path),
        defaults: hares_io::DefaultsSource::directory(project_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(cfg, None, 1, None),
    })
    .expect("the dwelling builds");
    let result = dwelling.simulate().expect("the run completes");
    let batches = result.reports[0].batches();
    let frame = arrow::compute::concat_batches(&batches[0].schema(), batches).expect("concat");
    let read = dry_bulb_bits(&frame);
    assert_eq!(read.len(), 24, "one row per hour of the day");
    for (row, record) in parsed.dry_bulb_c.iter().take(24).enumerate() {
        assert_eq!(
            read[row],
            record.to_bits(),
            "the row labelled {row}:00 reads the record of the hour it ends in"
        );
    }
}

/// The run's first hour interpolates from hour 24 of the run's first day
/// by default: the row labelled 00:00 reads `h24 * 0.75 + h1 * 0.25` of
/// Jan 1's records, never a Dec 31 record.
#[test]
fn first_hour_uses_hour24_of_the_first_day_by_default() {
    let weather_path = cz4a_dir().join("weather.epw");
    let parsed = hares_io::parse_weather(&weather_path).expect("the weather parses");
    let (h1, h24) = (parsed.dry_bulb_c[0], parsed.dry_bulb_c[23]);
    let read = first_hour_run_frame(FirstHour::Hour24);
    let n = 4.0_f64;
    for ts in 1..=4_usize {
        let w = (ts as f64 / n).min(1.0);
        let expected = h24 * (1.0 - w) + h1 * w;
        assert_eq!(
            read[ts - 1],
            expected.to_bits(),
            "the run's first hour's step {ts} interpolates from the first day's hour 24"
        );
    }
}

/// With `Hour1` selected, the run's first hour interpolates from the
/// hour-1 record: the first step reads `h1 * 0.75 + h1 * 0.25` (h1) and
/// the other three follow the usual weights toward hour 1's own record.
#[test]
fn first_hour_uses_hour1_when_selected() {
    let weather_path = cz4a_dir().join("weather.epw");
    let parsed = hares_io::parse_weather(&weather_path).expect("the weather parses");
    let (h1, h2) = (parsed.dry_bulb_c[0], parsed.dry_bulb_c[1]);
    let read = first_hour_run_frame(FirstHour::Hour1);
    let n = 4.0_f64;
    for ts in 1..=4_usize {
        let w = (ts as f64 / n).min(1.0);
        let expected = h1 * (1.0 - w) + h1 * w;
        assert_eq!(
            read[ts - 1],
            expected.to_bits(),
            "the run's first hour's step {ts} interpolates from the hour-1 record"
        );
    }
    // The second hour interpolates from hour 1 normally: its first step
    // reads h1 * 0.75 + h2 * 0.25.
    let expected = h1 * (1.0 - 0.25) + h2 * 0.25;
    assert_eq!(read[4], expected.to_bits(), "the second hour reads hour 1");
}

/// The run's outdoor dry-bulb column for one day at 900 s from the cz4a
/// fixtures, warm-up disabled, with the first-hour rule given.
fn first_hour_run_frame(first_hour: hares_io::FirstHour) -> Vec<u64> {
    let weather_path = cz4a_dir().join("weather.epw");
    let mut cfg = config(900, 86_400, "2023-01-01T00:00:00-07:00");
    cfg.first_hour_interpolation = first_hour;
    cfg.reports = vec![hares_io::ReportSpec {
        name: "weather".to_string(),
        selection: hares_io::Selection::Named(vec!["Outdoor Dry Bulb (C)".to_string()]),
        frequency: hares_io::Frequency::Timestep,
        destination: hares_io::Destination::Memory,
    }];
    let mut dwelling = Dwelling::from_config(DwellingConfig {
        hpxml: hares_io::InputSource::Path(cz4a_dir().join("building.xml")),
        schedule: Some(hares_io::InputSource::Path(cz4a_dir().join("schedule.csv"))),
        weather: hares_io::WeatherInput::Path(weather_path),
        defaults: hares_io::DefaultsSource::directory(project_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(cfg, None, 1, None),
    })
    .expect("the dwelling builds");
    let result = dwelling.simulate().expect("the run completes");
    let batches = result.reports[0].batches();
    let frame = arrow::compute::concat_batches(&batches[0].schema(), batches).expect("concat");
    dry_bulb_bits(&frame)
}
