//! `resample_weather`: the engine's own per-step weather channels,
//! exposed as rows. The acceptance: the exposed channels match the run's
//! internal series bitwise, at both a sub-hourly and an hourly
//! resolution.

use std::path::PathBuf;

use arrow::array::{Array, Float64Array, RecordBatch, TimestampMillisecondArray};
use arrow::compute::concat_batches;
use chrono::{DateTime, Duration};
use hares_core::{Dwelling, DwellingConfig, DwellingSettings, SimulationConfig, resample_weather};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture_dir() -> PathBuf {
    project_root().join("tests/fixtures/parity/cz4a_ashp_hpwh")
}

fn config(time_res: i64) -> SimulationConfig {
    SimulationConfig {
        scenario_seed: None,
        start_time: DateTime::parse_from_rfc3339("2023-01-01T00:00:00-07:00")
            .expect("valid start time"),
        duration: Duration::hours(48),
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

fn report_config(time_res: i64, weather_columns: &[&str]) -> SimulationConfig {
    let mut config = config(time_res);
    config.reports = vec![hares_io::ReportSpec {
        name: "weather".to_string(),
        selection: hares_io::Selection::Named(
            weather_columns.iter().map(|c| c.to_string()).collect(),
        ),
        frequency: hares_io::Frequency::Timestep,
        destination: hares_io::Destination::Memory,
    }];
    config
}

/// The run's resolved site location, the resolution the build itself
/// runs (the building's site against the weather header and the
/// config's override).
fn resolved_site() -> hares_io::SiteLocation {
    let building =
        hares_io::parse_hpxml(&fixture_dir().join("building.xml")).expect("the fixture parses");
    let weather =
        hares_io::parse_weather(fixture_dir().join("weather.epw")).expect("the weather parses");
    hares_io::resolve_site_location(
        &building.site,
        &weather.meta,
        &hares_io::SiteLocationOverride::default(),
    )
}

/// The weather variables' catalogue names, in the order
/// `resample_weather` returns them.
const WEATHER_VARIABLES: [&str; 10] = [
    "Outdoor Dry Bulb (C)",
    "Outdoor Humidity Ratio (kg/kg)",
    "Outdoor Pressure (kPa)",
    "Wind Speed (m/s)",
    "Sky Temperature (C)",
    "GHI (W/m2)",
    "DNI (W/m2)",
    "DHI (W/m2)",
    "Temperature - Ground (C)",
    "Hot Water Mains Temperature (C)",
];

fn column_values(batch: &RecordBatch, name: &str) -> Vec<u64> {
    let column = batch
        .column_by_name(name)
        .unwrap_or_else(|| panic!("the batch carries '{name}'"));
    let floats = column
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("a weather channel is f64");
    floats
        .iter()
        .map(|value| value.expect("no null weather channel").to_bits())
        .collect()
}

fn time_values(batch: &RecordBatch) -> Vec<i64> {
    let column = batch
        .column_by_name("Time")
        .expect("the batch carries Time");
    let stamps = column
        .as_any()
        .downcast_ref::<TimestampMillisecondArray>()
        .expect("Time is a millisecond timestamp");
    stamps
        .iter()
        .map(|value| value.expect("no null time"))
        .collect()
}

fn run_batches(config: &SimulationConfig) -> RecordBatch {
    let mut dwelling = Dwelling::from_config(DwellingConfig {
        hpxml: hares_io::InputSource::Path(fixture_dir().join("building.xml")),
        schedule: Some(hares_io::InputSource::Path(
            fixture_dir().join("schedule.csv"),
        )),
        weather: hares_io::WeatherInput::Path(fixture_dir().join("weather.epw")),
        defaults: hares_io::DefaultsSource::directory(project_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(config.clone(), None, 1, None),
    })
    .expect("the dwelling builds");
    let result = dwelling.simulate().expect("the run completes");
    let batches = result.reports[0].batches();
    assert!(!batches.is_empty(), "the run's report retains batches");
    concat_batches(&batches[0].schema(), batches).expect("the batches concatenate")
}

/// `resample_weather` with the run's own weather input, site and config
/// returns columns equal on bits to the run's internal weather series, at
/// 900 s and at 3600 s.
#[test]
fn resample_weather_equals_run_weather_columns_at_900_and_3600() {
    let site = resolved_site();
    for time_res in [900, 3600] {
        let run = run_batches(&report_config(time_res, &WEATHER_VARIABLES));
        let batch = resample_weather(
            &hares_io::WeatherInput::Path(fixture_dir().join("weather.epw")),
            &site,
            &config(time_res),
        )
        .expect("the resample completes");

        // The row counts agree: one row per step of the configured
        // horizon.
        assert_eq!(batch.num_rows(), run.num_rows(), "one row per step");
        assert_eq!(batch.num_columns(), WEATHER_VARIABLES.len() + 1);

        // The Time column: the run's own step times.
        assert_eq!(
            time_values(&batch),
            time_values(&run),
            "Time at {time_res} s"
        );

        // Every channel: equal on bits.
        for name in WEATHER_VARIABLES {
            assert_eq!(
                column_values(&batch, name),
                column_values(&run, name),
                "'{name}' at {time_res} s: the exposed channel equals the run's series on bits"
            );
        }
    }
}

/// The exposed columns' order and names: `Time` first, then the weather
/// channels in the catalogue's order.
#[test]
fn resample_weather_columns_are_the_catalogue_names_in_order() {
    let site = resolved_site();
    let batch = resample_weather(
        &hares_io::WeatherInput::Path(fixture_dir().join("weather.epw")),
        &site,
        &config(3600),
    )
    .expect("the resample completes");
    let schema = batch.schema();
    let mut names = vec!["Time".to_string()];
    names.extend(WEATHER_VARIABLES.iter().map(|n| n.to_string()));
    assert_eq!(
        schema
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect::<Vec<_>>(),
        names
    );
}
