//! The surface-independent forcing is computed once per weather file and
//! time grid and shared across the dwellings on that grid.
//!
//! These are the RED tests: at the parent the forcing tables and the
//! grid key do not exist, so the table tests fail to compile, and the
//! duplicate-computation test fails behaviourally (every dwelling computed
//! the forcing on every of its steps).

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use arrow::array::{Array, Float64Array, RecordBatch};
use arrow::compute::concat_batches;
use chrono::{DateTime, Duration};
use hares_core::{Dwelling, DwellingConfig, FORCE_PER_STEP_FORCING, SimulationConfig};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cz4a() -> PathBuf {
    repo_root().join("tests/fixtures/parity/cz4a_ashp_hpwh")
}

/// A `FixedOffset` parsed from an RFC 3339 string: the offset travels with
/// the instant, which is what the site-offset tests need.
fn start_at(rfc3339: &str) -> DateTime<chrono::FixedOffset> {
    DateTime::parse_from_rfc3339(rfc3339).expect("start time")
}

fn sim_config(
    start_time: DateTime<chrono::FixedOffset>,
    days: i64,
    time_res_s: i64,
    civil_timezone: Option<&str>,
) -> SimulationConfig {
    SimulationConfig {
        start_time,
        duration: Duration::days(days),
        time_res: Duration::seconds(time_res_s),
        reports: vec![hares_io::ReportSpec {
            name: "frame".to_owned(),
            selection: hares_io::Selection::Verbosity(2),
            frequency: Default::default(),
            destination: hares_io::Destination::Memory,
        }],
        diagnostics: false,
        metrics: false,
        output_chunk_size: 10_000,
        setpoint_deadband_c: None,
        master_seed: 0,
        civil_timezone: civil_timezone.map(str::to_owned),
        site_location: hares_io::SiteLocationOverride::default(),
        max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
        health_policy: hares_io::HealthPolicy::default(),
    }
}

fn cz4a_config(
    start_time: DateTime<chrono::FixedOffset>,
    days: i64,
    warmup_s: Option<u64>,
    civil_timezone: Option<&str>,
) -> DwellingConfig {
    DwellingConfig {
        hpxml_path: cz4a().join("building.xml"),
        schedule_path: Some(cz4a().join("schedule.csv")),
        weather_path: cz4a().join("weather.epw"),
        defaults_path: Some(repo_root().join("defaults")),
        sim_config: sim_config(start_time, days, 900, civil_timezone),
        overrides: None,
        bldg_id: 1,
        initialization_duration: warmup_s.map(std::time::Duration::from_secs),
        patches: None,
    }
}

/// Runs one dwelling twice, once with the shared forcing table and once
/// with the per-step computation, and returns both runs' memory frames.
fn table_and_per_step_frames(config: DwellingConfig) -> (RecordBatch, RecordBatch) {
    let table_run = run_frame(&config);

    FORCE_PER_STEP_FORCING.store(true, Ordering::SeqCst);
    let per_step_run =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_frame(&config)));
    FORCE_PER_STEP_FORCING.store(false, Ordering::SeqCst);
    (table_run, per_step_run.expect("per-step run"))
}

fn run_frame(config: &DwellingConfig) -> RecordBatch {
    let mut dwelling = Dwelling::from_config(config.clone()).expect("build");
    let result = dwelling.simulate().expect("run");
    let batches: Vec<_> = result
        .reports
        .iter()
        .flat_map(|report| report.batches().to_vec())
        .collect();
    let schema = batches.first().expect("the run recorded rows").schema();
    concat_batches(&schema, &batches).expect("batches of one run share a schema")
}

/// Asserts every column of `actual` equals `expected` bitwise; `Float64`
/// columns compare bit patterns so a `-0.0`/`0.0` or NaN-payload change is
/// a difference, not an equality.
fn assert_frames_bitwise_equal(expected: &RecordBatch, actual: &RecordBatch, context: &str) {
    assert_eq!(
        expected.schema(),
        actual.schema(),
        "{context}: column sets differ"
    );
    assert_eq!(
        expected.num_rows(),
        actual.num_rows(),
        "{context}: row counts differ"
    );
    for (field, (want, got)) in expected
        .schema()
        .fields()
        .iter()
        .zip(expected.columns().iter().zip(actual.columns()))
    {
        let name = field.name();
        match (
            want.as_any().downcast_ref::<Float64Array>(),
            got.as_any().downcast_ref::<Float64Array>(),
        ) {
            (Some(want), Some(got)) => {
                assert_eq!(
                    want.nulls(),
                    got.nulls(),
                    "{context}: column '{name}': null masks differ"
                );
                for (row, (w, g)) in want.values().iter().zip(got.values()).enumerate() {
                    assert_eq!(
                        w.to_bits(),
                        g.to_bits(),
                        "{context}: column '{name}' row {row}: {w} vs {g}"
                    );
                }
            }
            _ => assert_eq!(
                want.to_data(),
                got.to_data(),
                "{context}: column '{name}' differs"
            ),
        }
    }
}

/// `forcing_computed_once_per_weather_and_time_grid`: three dwellings built
/// over one weather file on one time grid step ten steps each; the forcing
/// computation counter moves by one (the shared table built for the grid),
/// not by the thirty per-step computations the duplicated path performs.
#[test]
fn forcing_computed_once_per_weather_and_time_grid() {
    let before = hares_core::FORCING_COMPUTATIONS.load(Ordering::Relaxed);

    let mut dwellings = Vec::new();
    for _ in 1..=3 {
        dwellings.push(
            Dwelling::from_config(cz4a_config(
                start_at("2024-02-27T00:00:00-07:00"),
                1,
                None,
                None,
            ))
            .expect("build"),
        );
    }
    for dwelling in &mut dwellings {
        for _ in 0..10 {
            dwelling.step().expect("step");
        }
    }

    let after = hares_core::FORCING_COMPUTATIONS.load(Ordering::Relaxed);
    let computations = after - before;
    assert_eq!(
        computations, 1,
        "the forcing must be computed once for the three dwellings on one \
         weather file and time grid, got {computations} computations"
    );
}

/// `forcing_table_run_equals_uncached_run`: the cz4a fixtures run with the
/// shared table and with the per-step computation give bitwise-equal
/// frames: the leap-day window, the weather-wrap window, and the two-day
/// warm-up variant. The windows are the fixtures' shapes at reduced
/// durations so the debug test build stays inside its timeout; the
/// full-duration bitwise proof is the golden compare's own.
#[test]
fn forcing_table_run_equals_uncached_run() {
    let cases = [
        ("leap day", "2024-02-27T00:00:00-07:00", 5, Some(604_800)),
        (
            "weather wrap",
            "2023-12-30T00:00:00-07:00",
            4,
            Some(604_800),
        ),
        (
            "two-day warm-up",
            "2023-01-15T00:00:00-07:00",
            1,
            Some(172_800),
        ),
    ];
    for (name, start, days, warmup_s) in cases {
        let config = cz4a_config(start_at(start), days, warmup_s, None);
        let (with_table, per_step) = table_and_per_step_frames(config);
        assert_frames_bitwise_equal(&per_step, &with_table, name);
    }
}

/// The DST fixture's shape: a run across a spring-forward transition with a
/// civil timezone reads the civil day of year through the shared table and
/// gives bitwise-equal frames both ways.
#[test]
#[cfg(feature = "dst")]
fn forcing_table_run_equals_uncached_run_across_dst() {
    let config = cz4a_config(
        start_at("2023-03-10T00:00:00-07:00"),
        4,
        Some(604_800),
        Some("America/Denver"),
    );
    let (with_table, per_step) = table_and_per_step_frames(config);
    assert_frames_bitwise_equal(&per_step, &with_table, "dst");
}

/// The consumer run shape: a ResStock home with an equipment override and
/// no warm-up, one day at 900 s, bitwise-equal frames both ways.
#[test]
fn forcing_table_run_equals_uncached_run_consumer_shape() {
    let config = DwellingConfig {
        hpxml_path: repo_root().join("tests/fixtures/resstock/2025.1/bldg0176227/home.xml"),
        schedule_path: Some(
            repo_root().join("tests/fixtures/resstock/2025.1/bldg0176227/in.schedules.csv"),
        ),
        weather_path: repo_root().join("tests/fixtures/resstock/2025.1/weather/G0600770_2018.csv"),
        defaults_path: Some(repo_root().join("defaults")),
        sim_config: sim_config(start_at("2018-01-01T00:00:00-08:00"), 1, 900, None),
        overrides: Some(serde_json::json!({"Gas Water Heater": {"ua_w_per_k": 2.0}})),
        bldg_id: 176_227,
        initialization_duration: None,
        patches: None,
    };
    let (with_table, per_step) = table_and_per_step_frames(config);
    assert_frames_bitwise_equal(&per_step, &with_table, "consumer shape");
}

/// `forcing_key_differs_by_start_date`: two configurations that differ only
/// in start date produce different keys and different tables.
#[test]
fn forcing_key_differs_by_start_date() {
    let first = Dwelling::from_config(cz4a_config(
        start_at("2024-02-27T00:00:00-07:00"),
        1,
        None,
        None,
    ))
    .expect("build");
    let second = Dwelling::from_config(cz4a_config(
        start_at("2024-03-27T00:00:00-07:00"),
        1,
        None,
        None,
    ))
    .expect("build");

    let first_key = first.environment.forcing_key().clone();
    let second_key = second.environment.forcing_key().clone();
    assert_ne!(
        first_key, second_key,
        "two start dates on one weather file must key differently"
    );
    let first_table = first.environment.forcing_table().expect("table").clone();
    let second_table = second.environment.forcing_table().expect("table").clone();
    assert!(
        !std::sync::Arc::ptr_eq(&first_table, &second_table),
        "different keys must not share a table"
    );
}

/// `forcing_key_differs_by_utc_offset_at_equal_instant`: two runs over one
/// weather file whose start instants are equal but whose site offsets
/// differ get different keys and different tables, and each frame equals
/// its per-step run bitwise; a second pair sharing the naive local start
/// at two offsets keys the same except in the offset field, and their
/// tables differ in the solar position rows.
#[test]
#[cfg(feature = "dst")]
fn forcing_key_differs_by_utc_offset_at_equal_instant() {
    let config_at_offset = |offset_h: f64, start: &str| {
        let mut config = cz4a_config(start_at(start), 2, None, None);
        config.sim_config.site_location = hares_io::SiteLocationOverride {
            utc_offset_h: Some(offset_h),
            ..hares_io::SiteLocationOverride::default()
        };
        config
    };

    // The same instant, two offsets: 2023-01-01T00:00:00-07:00 and
    // 2023-01-01T02:00:00-05:00 are the same moment.
    let west_config = config_at_offset(-7.0, "2023-01-01T00:00:00-07:00");
    let mountain_config = config_at_offset(-5.0, "2023-01-01T02:00:00-05:00");
    let west = Dwelling::from_config(west_config.clone()).expect("build");
    let mountain = Dwelling::from_config(mountain_config).expect("build");
    assert_ne!(
        west.environment.forcing_key(),
        mountain.environment.forcing_key(),
        "the same instant at two resolved offsets must key differently"
    );
    assert_ne!(
        west.environment.forcing_key().local_start,
        mountain.environment.forcing_key().local_start,
        "the same instant at two offsets has different local wall clocks"
    );
    let west_table = west.environment.forcing_table().expect("table").clone();
    let mountain_table = mountain.environment.forcing_table().expect("table").clone();
    assert!(
        !std::sync::Arc::ptr_eq(&west_table, &mountain_table),
        "different keys must not share a table"
    );

    // Each offset's run equals its per-step run bitwise.
    let (west_with_table, west_per_step) = table_and_per_step_frames(west_config);
    assert_frames_bitwise_equal(&west_per_step, &west_with_table, "west of the instant pair");
    let east_config = config_at_offset(-5.0, "2023-01-01T02:00:00-05:00");
    let (east_with_table, east_per_step) = table_and_per_step_frames(east_config);
    assert_frames_bitwise_equal(&east_per_step, &east_with_table, "east of the instant pair");

    // The same naive local start, two offsets: two instants two hours
    // apart, whose keys differ only in the offset field.
    let same_west_config = config_at_offset(-7.0, "2023-01-01T00:00:00-07:00");
    let same_east_config = config_at_offset(-5.0, "2023-01-01T00:00:00-07:00");
    let west = Dwelling::from_config(same_west_config).expect("build");
    let east = Dwelling::from_config(same_east_config).expect("build");
    let west_key = west.environment.forcing_key().clone();
    let east_key = east.environment.forcing_key().clone();
    assert_ne!(west_key.utc_offset_secs, east_key.utc_offset_secs);
    assert_eq!(
        west_key.local_start, east_key.local_start,
        "the pair shares the naive local start"
    );
    let rest_of_west = ForcingKeyFields::of(&west_key);
    let rest_of_east = ForcingKeyFields::of(&east_key);
    assert_eq!(
        rest_of_west, rest_of_east,
        "the keys differ only in the offset field"
    );

    // The instants differ, so the solar position rows differ.
    let west_table = west.environment.forcing_table().expect("table").clone();
    let east_table = east.environment.forcing_table().expect("table").clone();
    assert!(
        !std::sync::Arc::ptr_eq(&west_table, &east_table),
        "different offsets must not share a table"
    );
    assert_ne!(
        west_table.row(12).solar_azimuth_deg,
        east_table.row(12).solar_azimuth_deg,
        "two instants two hours apart sit at different solar azimuths"
    );
}

/// The projection of a `ForcingKey` onto every field but the UTC offset,
/// for the differ-only-in-offset assertion.
#[derive(Debug, PartialEq, Eq)]
#[cfg(feature = "dst")]
struct ForcingKeyFields {
    time_res_secs: u32,
    local_start: chrono::NaiveDateTime,
    horizon_steps: u64,
    latitude_bits: u64,
    longitude_bits: u64,
    elevation_bits: u64,
    civil_timezone: Option<String>,
}

#[cfg(feature = "dst")]
impl ForcingKeyFields {
    fn of(key: &hares_io::ForcingKey) -> Self {
        Self {
            time_res_secs: key.time_res_secs,
            local_start: key.local_start,
            horizon_steps: key.horizon_steps,
            latitude_bits: key.latitude_bits,
            longitude_bits: key.longitude_bits,
            elevation_bits: key.elevation_bits,
            civil_timezone: key.civil_timezone.clone(),
        }
    }
}

/// `blueprints_from_different_hpxml_share_forcing`: two runs whose HPXML
/// differs (one with PV, one without) over one weather file hold the same
/// forcing table.
#[test]
fn blueprints_from_different_hpxml_share_forcing() {
    let inputs = SharedInputs::new();
    let base = Dwelling::from_config(base_config(
        &repo_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"),
        &inputs,
    ))
    .expect("build");
    let pv = Dwelling::from_config(base_config(
        &repo_root().join("tests/fixtures/hpxml/ochre_samples/base-pv.xml"),
        &inputs,
    ))
    .expect("build");

    let base_table = base.environment.forcing_table().expect("table").clone();
    let pv_table = pv.environment.forcing_table().expect("table").clone();
    assert!(
        std::sync::Arc::ptr_eq(&base_table, &pv_table),
        "two HPXMLs over one weather file on one grid share the forcing table"
    );
}

/// `azimuth_change_shares_the_table`: two HPXMLs that differ only in a
/// wall's azimuth, built over one weather file, share one table and
/// produce different frames, each bitwise equal to its per-step run.
#[test]
fn azimuth_change_shares_the_table() {
    let inputs = SharedInputs::new();
    let base_xml = repo_root().join("tests/fixtures/hpxml/ochre_samples/base.xml");
    let original = std::fs::read_to_string(&base_xml).expect("read base.xml");
    let turned = original.replacen("<Azimuth>0</Azimuth>", "<Azimuth>90</Azimuth>", 1);
    assert_ne!(original, turned, "the fixture has a wall azimuth to turn");
    let turned_path = inputs.dir.path().join("turned.xml");
    std::fs::write(&turned_path, &turned).expect("write the turned HPXML");

    let straight = Dwelling::from_config(base_config(&base_xml, &inputs)).expect("build");
    let turned = Dwelling::from_config(base_config(&turned_path, &inputs)).expect("build");

    let straight_table = straight.environment.forcing_table().expect("table").clone();
    let turned_table = turned.environment.forcing_table().expect("table").clone();
    assert!(
        std::sync::Arc::ptr_eq(&straight_table, &turned_table),
        "an azimuth change is geometry, never a forcing key input: one table"
    );

    // Each azimuth's frame equals its per-step run bitwise, and the two
    // azimuths' frames differ.
    let (straight_with_table, straight_per_step) =
        table_and_per_step_frames(base_config(&base_xml, &inputs));
    assert_frames_bitwise_equal(&straight_per_step, &straight_with_table, "straight");
    let (turned_with_table, turned_per_step) =
        table_and_per_step_frames(base_config(&turned_path, &inputs));
    assert_frames_bitwise_equal(&turned_per_step, &turned_with_table, "turned");
    assert_ne!(straight_with_table.num_rows(), 0, "both runs record rows");
    assert_batches_differ(
        &straight_with_table,
        &turned_with_table,
        "the azimuths' frames",
    );
}

/// Asserts two frames differ somewhere in their f64 columns: two runs that
/// must not coincide do.
fn assert_batches_differ(a: &RecordBatch, b: &RecordBatch, context: &str) {
    assert_eq!(a.schema(), b.schema(), "{context}: schemas");
    assert_eq!(a.num_rows(), b.num_rows(), "{context}: row counts");
    let mut any_difference = false;
    for (left, right) in a.columns().iter().zip(b.columns()) {
        if let (Some(l), Some(r)) = (
            left.as_any().downcast_ref::<Float64Array>(),
            right.as_any().downcast_ref::<Float64Array>(),
        ) {
            for (w, g) in l.values().iter().zip(r.values()) {
                if w.to_bits() != g.to_bits() {
                    any_difference = true;
                }
            }
        }
    }
    assert!(any_difference, "{context}: the frames must differ");
}

/// One temp directory holding the synthetic weather and schedule both
/// runs of a differential read: the same file, so the same content
/// identity.
struct SharedInputs {
    dir: tempfile::TempDir,
}

impl SharedInputs {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("schedule.csv"), build_schedule_csv())
            .expect("write schedule");
        std::fs::write(dir.path().join("weather.epw"), build_epw_8760()).expect("write weather");
        Self { dir }
    }
}

/// The base fixture's config over the shared synthetic weather and
/// schedule.
fn base_config(hpxml_path: &Path, inputs: &SharedInputs) -> DwellingConfig {
    DwellingConfig {
        hpxml_path: hpxml_path.to_path_buf(),
        schedule_path: Some(inputs.dir.path().join("schedule.csv")),
        weather_path: inputs.dir.path().join("weather.epw"),
        defaults_path: Some(repo_root().join("defaults")),
        sim_config: sim_config(start_at("2023-01-01T00:00:00-07:00"), 1, 900, None),
        overrides: None,
        bldg_id: 1,
        initialization_duration: None,
        patches: None,
    }
}

/// The schedule columns the base fixture's event equipment and wet
/// appliances run on.
fn build_schedule_csv() -> String {
    [
        "Time,occupants,clothes_washer,clothes_dryer,dishwasher,cooking_range,hot_water_clothes_washer,hot_water_dishwasher,heating_setpoint",
        "2021-01-01T00:00:00-07:00,1.0,0.10,0.10,0.10,0.10,0.10,0.10,20.0",
        "2021-01-01T00:15:00-07:00,1.0,0.20,0.20,0.20,0.20,0.20,0.20,20.5",
        "2021-01-01T00:30:00-07:00,1.0,0.30,0.30,0.30,0.30,0.30,0.30,21.0",
        "2021-01-01T00:45:00-07:00,1.0,0.40,0.40,0.40,0.40,0.40,0.40,21.5",
    ]
    .join("\n")
}

/// A synthetic annual EPW whose solar values make the solar position and
/// the per-surface irradiance matter.
fn build_epw_8760() -> String {
    use chrono::{Datelike, NaiveDate, Timelike};

    let mut lines = vec![
        "LOCATION,Test Site,CO,USA,TMY3,999999,39.74,-104.99,-7.0,1609.3".to_string(),
        "DESIGN CONDITIONS,0".to_string(),
        "GROUND TEMPERATURES,0".to_string(),
        "TYPICAL/EXTREME PERIODS,0".to_string(),
        "HOLIDAYS/DAYLIGHT SAVINGS,No,0,0,0".to_string(),
        "COMMENTS 1,synthetic".to_string(),
        "COMMENTS 2,synthetic".to_string(),
        "DATA PERIODS,1,1,Data,Sunday, 1/ 1,12/31".to_string(),
    ];

    let start_date = NaiveDate::from_ymd_opt(2021, 1, 1).expect("valid date");
    let start_time = start_date.and_hms_opt(0, 0, 0).expect("valid time");
    for i in 0..8760 {
        let timestamp = start_time + Duration::hours(i as i64);
        let year = timestamp.year();
        let month = timestamp.month();
        let day = timestamp.day();
        let hour = timestamp.hour() + 1;

        let row = [
            year.to_string(),
            month.to_string(),
            day.to_string(),
            hour.to_string(),
            "0".to_string(),
            "A0A0A0A0*0*0*0*0*0*0*0*0*0*0".to_string(),
            "20.0".to_string(),
            "10.0".to_string(),
            "50".to_string(),
            "101325".to_string(),
            "0".to_string(),
            "0".to_string(),
            "300".to_string(),
            "100".to_string(),
            "200".to_string(),
            "50".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "180".to_string(),
            "3.5".to_string(),
            "4".to_string(),
            "4".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
        ];
        lines.push(row.join(","));
    }
    lines.join("\n")
}
