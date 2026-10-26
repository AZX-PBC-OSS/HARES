//! The `Time` column's Arrow-Timestamp acceptance: the column is
//! `Timestamp(Millisecond, None)` in naive local wall-clock milliseconds
//! labelled at the step's start, the schema metadata carries the site's
//! standard UTC offset and the catalogue digest and nothing else, and a
//! warmed-up run's frame starts at the start time with one row per step.

use hares_core::DwellingSettings;
use hares_io::{DefaultsSource, InputSource, WeatherInput};
use std::path::PathBuf;

use arrow::array::{RecordBatch, TimestampMillisecondArray};
use arrow::datatypes::{DataType, TimeUnit};
use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use hares_core::{Dwelling, DwellingConfig};
use hares_io::{Destination, ReportSpec, Selection, SimulationConfig, SiteLocationOverride};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn site_offset() -> FixedOffset {
    FixedOffset::east_opt(-7 * 3600).expect("offset")
}

fn cz2a_config(
    start: DateTime<FixedOffset>,
    duration_s: i64,
    step_s: i64,
    verbosity: u8,
    warmup_s: Option<i64>,
) -> DwellingConfig {
    let fixture = project_root().join("tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh");
    DwellingConfig {
        hpxml: InputSource::Path(fixture.join("building.xml")),
        schedule: Some(InputSource::Path(fixture.join("schedule.csv"))),
        weather: WeatherInput::Path(fixture.join("weather.epw")),
        defaults: DefaultsSource::directory(project_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(
            SimulationConfig {
                scenario_seed: None,
                start_time: start,
                duration: chrono::Duration::seconds(duration_s),
                time_res: chrono::Duration::seconds(step_s),
                reports: vec![ReportSpec {
                    name: "frame".to_string(),
                    selection: Selection::Verbosity(verbosity),
                    frequency: hares_io::Frequency::default(),
                    destination: Destination::Memory,
                }],
                diagnostics: false,
                metrics: false,
                output_chunk_size: 96,
                setpoint_deadband_c: None,
                master_seed: 0,
                civil_timezone: None,
                site_location: SiteLocationOverride::default(),
                max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                health_policy: hares_io::HealthPolicy::default(),
                first_hour_interpolation: Default::default(),
                weather_year: hares_io::WeatherYear::default(),
                warmup: if warmup_s.is_some() {
                    hares_io::Warmup::default()
                } else {
                    hares_io::Warmup::Disabled
                },
            },
            None,
            1,
            None,
        ),
    }
}

fn run_cz2a(
    start: DateTime<FixedOffset>,
    duration_s: i64,
    step_s: i64,
    verbosity: u8,
    warmup_s: Option<i64>,
) -> Dwelling {
    let mut dwelling =
        Dwelling::from_config(cz2a_config(start, duration_s, step_s, verbosity, warmup_s))
            .expect("cz2a dwelling builds");
    dwelling.simulate().expect("cz2a run completes");
    dwelling
}

/// Every batch's Time column, read as the milliseconds the
/// `Timestamp(Millisecond, None)` field carries.
fn time_values(batches: &[RecordBatch]) -> Vec<i64> {
    batches
        .iter()
        .flat_map(|batch| {
            batch
                .column(0)
                .as_any()
                .downcast_ref::<TimestampMillisecondArray>()
                .expect("Time is a millisecond timestamp array")
                .iter()
                .map(|v| v.expect("Time is not nullable"))
        })
        .collect()
}

/// The first and last rows of the `full_year_hourly` inputs over 48 h
/// decode to the start's and the run's end wall clock, and applying the
/// metadata offset recovers the UTC instant.
#[test]
fn time_column_is_millisecond_timestamp_local() {
    let start = site_offset()
        .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
        .single()
        .expect("start");
    let dwelling = run_cz2a(start, 48 * 3600, 3600, 2, None);
    let batches: Vec<RecordBatch> = dwelling.flushed_batches().to_vec();
    assert_eq!(batches.iter().map(RecordBatch::num_rows).sum::<usize>(), 48);

    let schema = batches[0].schema();
    let field = schema.field(0);
    assert_eq!(field.name(), "Time");
    assert_eq!(
        field.data_type(),
        &DataType::Timestamp(TimeUnit::Millisecond, None),
        "Time is a millisecond timestamp with no embedded time zone"
    );

    let times = time_values(&batches);
    assert_eq!(times.len(), 48);
    let offset_s: i64 = schema
        .metadata()
        .get("hares:utc_offset_s")
        .expect("the schema metadata carries the site's standard UTC offset")
        .parse()
        .expect("the offset is decimal seconds");
    assert_eq!(offset_s, -25_200, "the cz2a site's standard offset is -7 h");

    // The wall clock the column spells: the start's local midnight, and
    // 47 hours later for the last row.
    let first = DateTime::from_timestamp_millis(times[0])
        .expect("ms in range")
        .naive_utc();
    let last = DateTime::from_timestamp_millis(times[47])
        .expect("ms in range")
        .naive_utc();
    assert_eq!(
        first.format("%Y-%m-%dT%H:%M").to_string(),
        "2023-01-01T00:00"
    );
    assert_eq!(
        last.format("%Y-%m-%dT%H:%M").to_string(),
        "2023-01-02T23:00"
    );

    // Applying the offset (UTC = local wall clock - offset) recovers the
    // UTC instant.
    let utc_first = Utc
        .timestamp_millis_opt(times[0] - offset_s * 1000)
        .single()
        .expect("instant");
    let utc_last = Utc
        .timestamp_millis_opt(times[47] - offset_s * 1000)
        .single()
        .expect("instant");
    assert_eq!(
        utc_first.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "2023-01-01T07:00:00Z"
    );
    assert_eq!(
        utc_last.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "2023-01-03T06:00:00Z"
    );
}

/// A warmed-up run's frame starts at the start time's wall clock and
/// carries exactly one row per simulated step.
#[test]
fn frame_starts_at_start_time_with_exact_row_count() {
    let start = site_offset()
        .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
        .single()
        .expect("start");
    let dwelling = run_cz2a(start, 48 * 3600, 3600, 2, Some(6 * 3600));
    let batches: Vec<RecordBatch> = dwelling.flushed_batches().to_vec();
    let times = time_values(&batches);
    assert_eq!(
        times.len(),
        48,
        "the row count is duration / time_res; warm-up rows are not recorded"
    );
    let first = DateTime::from_timestamp_millis(times[0])
        .expect("ms in range")
        .naive_utc();
    assert_eq!(
        first.format("%Y-%m-%dT%H:%M").to_string(),
        start.format("%Y-%m-%dT%H:%M").to_string(),
        "the first row's wall clock is the start time's"
    );
}

/// A report's schema metadata carries exactly the site offset and the
/// catalogue digest.
#[test]
fn schema_metadata_holds_only_offset_and_digest() {
    let start = site_offset()
        .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
        .single()
        .expect("start");
    let dwelling = run_cz2a(start, 48 * 3600, 3600, 8, None);
    let batches: Vec<RecordBatch> = dwelling.flushed_batches().to_vec();
    let schema = batches[0].schema();
    let metadata = schema.metadata();
    let mut keys: Vec<&str> = metadata.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["hares:catalogue_digest", "hares:utc_offset_s"],
        "the schema metadata carries the offset and the catalogue digest and nothing else"
    );
}
