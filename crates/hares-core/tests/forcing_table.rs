//! The surface-independent forcing is computed once per parsed-weather
//! handle and time grid and shared across the dwellings built from that
//! handle.
//!
//! These are the RED tests: at the parent the forcing tables and the
//! grid key do not exist, so the table tests fail to compile, and the
//! duplicate-computation test fails behaviourally (every dwelling computed
//! the forcing on every of its steps).

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};

use arrow::array::{Array, Float64Array, RecordBatch};
use arrow::compute::concat_batches;
use chrono::{DateTime, Duration};
use hares_core::{
    Dwelling, DwellingBlueprint, DwellingConfig, DwellingSettings, FORCE_PER_STEP_FORCING,
    SimulationConfig,
};

/// The forcing counters this file probes (`FORCING_TABLES_LIVE`,
/// `FORCING_COMPUTATIONS`) are process-global, and the probes' assertions
/// assume no sibling construction or drop runs between their samples.
/// nextest runs each test in its own process, where the lock is
/// uncontended; in-process runners (plain `cargo test`, the coverage
/// job's llvm-cov run) execute the tests on shared threads, so every
/// test here holds the lock for its whole body and the samples read the
/// counters alone.
static COUNTER_EXCLUSIVITY: Mutex<()> = Mutex::new(());

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
        scenario_seed: None,
        schedule_diversity: Default::default(),
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
        first_hour_interpolation: Default::default(),
        weather_year: hares_io::WeatherYear::default(),
        warmup: hares_io::Warmup::Disabled,
    }
}

fn cz4a_config(
    start_time: DateTime<chrono::FixedOffset>,
    days: i64,
    warmup_s: Option<u64>,
    civil_timezone: Option<&str>,
) -> DwellingConfig {
    DwellingConfig {
        hpxml: hares_io::InputSource::Path(cz4a().join("building.xml")),
        schedule: Some(hares_io::InputSource::Path(cz4a().join("schedule.csv"))),
        weather: hares_io::WeatherInput::Path(cz4a().join("weather.epw")),
        defaults: hares_io::DefaultsSource::directory(repo_root().join("python/hares/defaults")),
        settings: DwellingSettings {
            // The warm-up rides the sim config's warmup field: on for the
            // callers that pass a day count, disabled for the rest.
            sim_config: {
                let mut sim = sim_config(start_time, days, 900, civil_timezone);
                if warmup_s.is_some() {
                    sim.warmup = hares_io::Warmup::default();
                }
                sim
            },
            overrides: None,
            bldg_id: 1,
            patches: None,
        },
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

/// One set of shared cz4a handles: the weather, schedule, defaults and
/// HPXML parsed once, shared by every blueprint the tests build from them.
struct SharedCz4a {
    weather: Arc<hares_io::ParsedWeather>,
    schedule: Arc<hares_io::ParsedSchedule>,
    defaults: Arc<hares_io::ParsedDefaults>,
    hpxml: Arc<hares_io::ParsedHpxml>,
}

impl SharedCz4a {
    fn new() -> Self {
        let weather = Arc::new(
            hares_io::ParsedWeather::parse(&hares_io::WeatherInput::Path(
                cz4a().join("weather.epw"),
            ))
            .expect("weather parses"),
        );
        let schedule = Arc::new(
            hares_io::ParsedSchedule::parse(
                &hares_io::InputSource::Path(cz4a().join("schedule.csv")),
                Some(weather.meta()),
            )
            .expect("schedule parses"),
        );
        let defaults = Arc::new(
            hares_io::ParsedDefaults::parse(&hares_io::DefaultsSource::directory(
                repo_root().join("python/hares/defaults"),
            ))
            .expect("defaults load"),
        );
        let hpxml = Arc::new(
            hares_io::ParsedHpxml::parse(&hares_io::InputSource::Path(cz4a().join("building.xml")))
                .expect("hpxml parses"),
        );
        Self {
            weather,
            schedule,
            defaults,
            hpxml,
        }
    }

    /// A blueprint over the shared handles with the settings given: the
    /// construction whose forcing resolution the tests measure.
    fn blueprint(&self, settings: DwellingSettings) -> Arc<DwellingBlueprint> {
        let inputs = hares_io::ParsedInputs::from_handles(
            Arc::clone(&self.hpxml),
            Some(Arc::clone(&self.schedule)),
            Arc::clone(&self.weather),
            Arc::clone(&self.defaults),
            None,
        )
        .expect("the shared inputs assemble");
        hares_core::DwellingBlueprintBuilder::new(inputs, settings)
            .expect("the builder builds")
            .freeze()
            .expect("the blueprint freezes")
    }
}

/// The cz4a settings at the start and duration given.
fn cz4a_settings(start_time: DateTime<chrono::FixedOffset>, days: i64) -> DwellingSettings {
    DwellingSettings {
        sim_config: sim_config(start_time, days, 900, None),
        overrides: None,
        bldg_id: 1,
        patches: None,
    }
}

/// `forcing_computed_once_per_weather_and_time_grid`: three dwellings built/// from one parsed-weather handle on one time grid step ten steps each; the
/// forcing computation counter moves by one (the table resolved once on the
/// handle), not by the thirty per-step computations the duplicated path
/// performs.
#[test]
fn forcing_computed_once_per_weather_and_time_grid() {
    let _counter_exclusive = COUNTER_EXCLUSIVITY
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let before = hares_core::FORCING_COMPUTATIONS.load(Ordering::Relaxed);

    let shared = SharedCz4a::new();
    let blueprint = shared.blueprint(cz4a_settings(start_at("2024-02-27T00:00:00-07:00"), 1));
    let mut dwellings = Vec::new();
    for _ in 1..=3 {
        dwellings.push(blueprint.build().expect("build"));
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
    let _counter_exclusive = COUNTER_EXCLUSIVITY
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
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
    let _counter_exclusive = COUNTER_EXCLUSIVITY
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
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
    let _counter_exclusive = COUNTER_EXCLUSIVITY
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let config = DwellingConfig {
        hpxml: hares_io::InputSource::Path(
            repo_root().join("tests/fixtures/resstock/2025.1/bldg0176227/home.xml"),
        ),
        schedule: Some(hares_io::InputSource::Path(
            repo_root().join("tests/fixtures/resstock/2025.1/bldg0176227/in.schedules.csv"),
        )),
        weather: hares_io::WeatherInput::Path(
            repo_root().join("tests/fixtures/resstock/2025.1/weather/G0600770_2018.csv"),
        ),
        defaults: hares_io::DefaultsSource::directory(repo_root().join("python/hares/defaults")),
        settings: DwellingSettings {
            sim_config: sim_config(start_at("2018-01-01T00:00:00-08:00"), 1, 900, None),
            overrides: Some(serde_json::json!({"Gas Water Heater": {"ua_w_per_k": 2.0}})),
            bldg_id: 176_227,
            patches: None,
        },
    };
    let (with_table, per_step) = table_and_per_step_frames(config);
    assert_frames_bitwise_equal(&per_step, &with_table, "consumer shape");
}

/// `forcing_key_differs_by_first_hour`: two blueprints over one
/// parsed-weather handle that differ only in the first-hour rule hold
/// different tables on that handle: the rule changes which records the
/// run's first hour's steps read.
#[test]
fn forcing_key_differs_by_first_hour() {
    let _counter_exclusive = COUNTER_EXCLUSIVITY
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let shared = SharedCz4a::new();
    let mut first_settings = cz4a_settings(start_at("2024-02-27T00:00:00-07:00"), 1);
    let mut second_settings = first_settings.clone();
    first_settings.sim_config.first_hour_interpolation = hares_io::FirstHour::Hour24;
    second_settings.sim_config.first_hour_interpolation = hares_io::FirstHour::Hour1;

    let first = shared.blueprint(first_settings).build().expect("build");
    let second = shared.blueprint(second_settings).build().expect("build");

    let first_key = first.environment.forcing_key().clone();
    let second_key = second.environment.forcing_key().clone();
    assert_ne!(
        first_key, second_key,
        "two first-hour rules on one weather file must key differently"
    );
    let first_table = first.environment.forcing_table().expect("table").clone();
    let second_table = second.environment.forcing_table().expect("table").clone();
    assert!(
        !std::sync::Arc::ptr_eq(&first_table, &second_table),
        "different keys must not share a table"
    );
}

/// `forcing_key_differs_by_start_date`: two blueprints over one
/// parsed-weather handle that differ only in start date produce different
/// keys and different tables on that handle.
#[test]
fn forcing_key_differs_by_start_date() {
    let _counter_exclusive = COUNTER_EXCLUSIVITY
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let shared = SharedCz4a::new();
    let first = shared
        .blueprint(cz4a_settings(start_at("2024-02-27T00:00:00-07:00"), 1))
        .build()
        .expect("build");
    let second = shared
        .blueprint(cz4a_settings(start_at("2024-03-27T00:00:00-07:00"), 1))
        .build()
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

/// `forcing_key_differs_by_utc_offset_at_equal_instant`: two blueprints
/// over one parsed-weather handle whose start instants are equal but whose
/// site offsets differ get different keys and different tables on that
/// handle, and each frame equals its per-step run bitwise; a second pair
/// sharing the naive local start at two offsets keys the same except in
/// the offset field, and their tables differ in the solar position rows.
#[test]
#[cfg(feature = "dst")]
fn forcing_key_differs_by_utc_offset_at_equal_instant() {
    let _counter_exclusive = COUNTER_EXCLUSIVITY
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let shared = SharedCz4a::new();
    let settings_at_offset = |offset_h: f64, start: &str| {
        let mut settings = cz4a_settings(start_at(start), 2);
        settings.sim_config.site_location = hares_io::SiteLocationOverride {
            utc_offset_h: Some(offset_h),
            ..hares_io::SiteLocationOverride::default()
        };
        settings
    };

    // The same instant, two offsets: 2023-01-01T00:00:00-07:00 and
    // 2023-01-01T02:00:00-05:00 are the same moment.
    let west = shared
        .blueprint(settings_at_offset(-7.0, "2023-01-01T00:00:00-07:00"))
        .build()
        .expect("build");
    let mountain = shared
        .blueprint(settings_at_offset(-5.0, "2023-01-01T02:00:00-05:00"))
        .build()
        .expect("build");
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
    let config_at_offset = |offset_h: f64, start: &str| {
        let mut config = cz4a_config(start_at(start), 2, None, None);
        config.settings.sim_config.site_location = hares_io::SiteLocationOverride {
            utc_offset_h: Some(offset_h),
            ..hares_io::SiteLocationOverride::default()
        };
        config
    };
    let (west_with_table, west_per_step) =
        table_and_per_step_frames(config_at_offset(-7.0, "2023-01-01T00:00:00-07:00"));
    assert_frames_bitwise_equal(&west_per_step, &west_with_table, "west of the instant pair");
    let (east_with_table, east_per_step) =
        table_and_per_step_frames(config_at_offset(-5.0, "2023-01-01T02:00:00-05:00"));
    assert_frames_bitwise_equal(&east_per_step, &east_with_table, "east of the instant pair");

    // The same naive local start, two offsets: two instants two hours
    // apart, whose keys differ only in the offset field.
    let west = shared
        .blueprint(settings_at_offset(-7.0, "2023-01-01T00:00:00-07:00"))
        .build()
        .expect("build");
    let east = shared
        .blueprint(settings_at_offset(-5.0, "2023-01-01T00:00:00-07:00"))
        .build()
        .expect("build");
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
/// differs (one with PV, one without) over one parsed-weather handle hold
/// the same forcing table.
#[test]
fn blueprints_from_different_hpxml_share_forcing() {
    let _counter_exclusive = COUNTER_EXCLUSIVITY
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let inputs = SharedInputs::new();
    let weather = Arc::new(
        hares_io::ParsedWeather::parse(&hares_io::WeatherInput::Path(
            inputs.dir.path().join("weather.epw"),
        ))
        .expect("weather parses"),
    );
    let schedule = Arc::new(
        hares_io::ParsedSchedule::parse(
            &hares_io::InputSource::Path(inputs.dir.path().join("schedule.csv")),
            Some(weather.meta()),
        )
        .expect("schedule parses"),
    );
    let defaults = Arc::new(
        hares_io::ParsedDefaults::parse(&hares_io::DefaultsSource::directory(
            repo_root().join("python/hares/defaults"),
        ))
        .expect("defaults load"),
    );
    let settings = DwellingSettings {
        sim_config: sim_config(start_at("2023-01-01T00:00:00-07:00"), 1, 900, None),
        overrides: None,
        bldg_id: 1,
        patches: None,
    };

    let blueprint_for = |hpxml_path: &Path| {
        let hpxml = Arc::new(
            hares_io::ParsedHpxml::parse(&hares_io::InputSource::Path(hpxml_path.to_path_buf()))
                .expect("hpxml parses"),
        );
        let parsed = hares_io::ParsedInputs::from_handles(
            hpxml,
            Some(Arc::clone(&schedule)),
            Arc::clone(&weather),
            Arc::clone(&defaults),
            None,
        )
        .expect("the shared inputs assemble");
        hares_core::DwellingBlueprintBuilder::new(parsed, settings.clone())
            .expect("the builder builds")
            .freeze()
            .expect("the blueprint freezes")
    };

    let base = blueprint_for(&repo_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"))
        .build()
        .expect("build");
    let pv = blueprint_for(&repo_root().join("tests/fixtures/hpxml/ochre_samples/base-pv.xml"))
        .build()
        .expect("build");

    let base_table = base.environment.forcing_table().expect("table").clone();
    let pv_table = pv.environment.forcing_table().expect("table").clone();
    assert!(
        std::sync::Arc::ptr_eq(&base_table, &pv_table),
        "two HPXMLs over one weather handle on one grid share the forcing table"
    );
}

/// `azimuth_change_shares_the_table`: two HPXMLs that differ only in a
/// wall's azimuth, built over one parsed-weather handle, share one table
/// and produce different frames, each bitwise equal to its per-step run.
#[test]
fn azimuth_change_shares_the_table() {
    let _counter_exclusive = COUNTER_EXCLUSIVITY
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let inputs = SharedInputs::new();
    let base_xml = repo_root().join("tests/fixtures/hpxml/ochre_samples/base.xml");
    let original = std::fs::read_to_string(&base_xml).expect("read base.xml");
    let turned = original.replacen("<Azimuth>0</Azimuth>", "<Azimuth>90</Azimuth>", 1);
    assert_ne!(original, turned, "the fixture has a wall azimuth to turn");
    let turned_path = inputs.dir.path().join("turned.xml");
    std::fs::write(&turned_path, &turned).expect("write the turned HPXML");

    let weather = Arc::new(
        hares_io::ParsedWeather::parse(&hares_io::WeatherInput::Path(
            inputs.dir.path().join("weather.epw"),
        ))
        .expect("weather parses"),
    );
    let schedule = Arc::new(
        hares_io::ParsedSchedule::parse(
            &hares_io::InputSource::Path(inputs.dir.path().join("schedule.csv")),
            Some(weather.meta()),
        )
        .expect("schedule parses"),
    );
    let defaults = Arc::new(
        hares_io::ParsedDefaults::parse(&hares_io::DefaultsSource::directory(
            repo_root().join("python/hares/defaults"),
        ))
        .expect("defaults load"),
    );
    let settings = DwellingSettings {
        sim_config: sim_config(start_at("2023-01-01T00:00:00-07:00"), 1, 900, None),
        overrides: None,
        bldg_id: 1,
        patches: None,
    };
    let blueprint_for = |hpxml_path: &Path| {
        let hpxml = Arc::new(
            hares_io::ParsedHpxml::parse(&hares_io::InputSource::Path(hpxml_path.to_path_buf()))
                .expect("hpxml parses"),
        );
        let parsed = hares_io::ParsedInputs::from_handles(
            hpxml,
            Some(Arc::clone(&schedule)),
            Arc::clone(&weather),
            Arc::clone(&defaults),
            None,
        )
        .expect("the shared inputs assemble");
        hares_core::DwellingBlueprintBuilder::new(parsed, settings.clone())
            .expect("the builder builds")
            .freeze()
            .expect("the blueprint freezes")
    };

    let straight = blueprint_for(&base_xml).build().expect("build");
    let turned = blueprint_for(&turned_path).build().expect("build");

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
        hpxml: hares_io::InputSource::Path(hpxml_path.to_path_buf()),
        schedule: Some(hares_io::InputSource::Path(
            inputs.dir.path().join("schedule.csv"),
        )),
        weather: hares_io::WeatherInput::Path(inputs.dir.path().join("weather.epw")),
        defaults: hares_io::DefaultsSource::directory(repo_root().join("python/hares/defaults")),
        settings: DwellingSettings {
            sim_config: sim_config(start_at("2023-01-01T00:00:00-07:00"), 1, 900, None),
            overrides: None,
            bldg_id: 1,
            patches: None,
        },
    }
}

/// The schedule columns the base fixture's event equipment and wet
/// appliances run on. The setpoint column is read in the OS-HPXML
/// Fahrenheit convention, so its values are Fahrenheit: these resolve to
/// the 20.0-21.5 C heating setpoints the column states.
fn build_schedule_csv() -> String {
    [
        "Time,occupants,clothes_washer,clothes_dryer,dishwasher,cooking_range,hot_water_clothes_washer,hot_water_dishwasher,heating_setpoint",
        "2021-01-01T00:00:00-07:00,1.0,0.10,0.10,0.10,0.10,0.10,0.10,68.0",
        "2021-01-01T00:15:00-07:00,1.0,0.20,0.20,0.20,0.20,0.20,0.20,68.9",
        "2021-01-01T00:30:00-07:00,1.0,0.30,0.30,0.30,0.30,0.30,0.30,69.8",
        "2021-01-01T00:45:00-07:00,1.0,0.40,0.40,0.40,0.40,0.40,0.40,70.7",
    ]
    .join("\n")
}

/// A synthetic annual EPW whose solar values make the solar position and
/// the per-surface irradiance matter. `variant` shifts the dry bulb and
/// dew point, so two variants are two distinct weather contents.
fn build_epw_8760() -> String {
    build_epw_8760_variant(0)
}

fn build_epw_8760_variant(variant: i32) -> String {
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
            (10.0 + f64::from(variant) * 0.25).to_string(),
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

/// `dropped_dwellings_drop_their_forcing_tables`: the forcing tables are
/// the parsed-weather handles' own state, so a dwelling's construction
/// resolves its table once (one computation), the dwellings built from one
/// handle share it, and dropping the dwellings and their handles drops the
/// tables: the live-table count returns to its baseline. N distinct
/// weather contents, each constructed and dropped, leave no table behind:
/// the live count during holds exactly one table per live handle, and
/// after the drops exactly none.
#[test]
fn dropped_dwellings_drop_their_forcing_tables() {
    let _counter_exclusive = COUNTER_EXCLUSIVITY
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    const DISTINCT_WEATHERS: usize = 40;

    let before_live = hares_io::FORCING_TABLES_LIVE.load(Ordering::Relaxed);
    let before_computations = hares_core::FORCING_COMPUTATIONS.load(Ordering::Relaxed);

    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("schedule.csv"), build_schedule_csv()).expect("write schedule");

    let mut dwellings = Vec::new();
    for variant in 0..DISTINCT_WEATHERS {
        let weather_path = dir.path().join(format!("weather_{variant}.epw"));
        std::fs::write(&weather_path, build_epw_8760_variant(variant as i32))
            .expect("write weather");
        let config = DwellingConfig {
            hpxml: hares_io::InputSource::Path(
                repo_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"),
            ),
            schedule: Some(hares_io::InputSource::Path(dir.path().join("schedule.csv"))),
            weather: hares_io::WeatherInput::Path(weather_path.clone()),
            defaults: hares_io::DefaultsSource::directory(
                repo_root().join("python/hares/defaults"),
            ),
            settings: DwellingSettings {
                sim_config: sim_config(start_at("2023-01-01T00:00:00-07:00"), 1, 900, None),
                overrides: None,
                bldg_id: i64::try_from(variant + 1).unwrap_or(1),
                patches: None,
            },
        };
        dwellings.push(Dwelling::from_config(config).expect("build"));
    }

    let during_live = hares_io::FORCING_TABLES_LIVE.load(Ordering::Relaxed);
    assert_eq!(
        during_live,
        before_live + DISTINCT_WEATHERS as u64,
        "one table per live handle while the dwellings are held, got {} \
         live tables for {DISTINCT_WEATHERS} live dwellings",
        during_live - before_live,
    );

    drop(dwellings);

    let after_live = hares_io::FORCING_TABLES_LIVE.load(Ordering::Relaxed);
    assert_eq!(
        after_live,
        before_live,
        "dropped dwellings must drop their forcing tables with them; {} \
         tables stayed resident",
        after_live - before_live,
    );
    let after_computations = hares_core::FORCING_COMPUTATIONS.load(Ordering::Relaxed);
    assert_eq!(
        after_computations,
        before_computations + DISTINCT_WEATHERS as u64,
        "one computation per distinct weather content, got {}",
        after_computations - before_computations,
    );
}
