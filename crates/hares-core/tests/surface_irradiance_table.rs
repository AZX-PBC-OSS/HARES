//! The per-surface irradiance table: computed once per content, shared
//! bitwise by every dwelling that content builds -- within one blueprint
//! and, through the process's content registry, across independently
//! constructed blueprints of identical content; different content never
//! shares.
//!
//! These are the row's acceptance tests (the RT-2 record's per-surface
//! paragraph, the phase-share record's second proposal, the content
//! registry the bench-gate diagnosis added). At the parent the table does
//! not exist, so the sharing test fails to compile -- the forcing table's
//! RED shape -- and the bitwise witnesses are the goldens' own
//! compare-all.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};

use arrow::array::{Array, Float64Array, RecordBatch};
use arrow::compute::concat_batches;
use chrono::{DateTime, Duration};

use hares_core::{
    Dwelling, DwellingBlueprint, DwellingBlueprintBuilder, DwellingSettings,
    FORCE_PER_STEP_FORCING, FORCING_COMPUTATIONS, SimulationConfig,
};

/// The forcing counter probes (`FORCING_COMPUTATIONS`) are process-global,
/// and the probes' assertions assume no sibling construction or drop runs
/// between their samples (the forcing tests' exclusivity rule; nextest
/// runs each test in its own process, in-process runners share threads).
static COUNTER_EXCLUSIVITY: Mutex<()> = Mutex::new(());

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cz4a() -> PathBuf {
    repo_root().join("tests/fixtures/parity/cz4a_ashp_hpwh")
}

fn cz4a_pv() -> PathBuf {
    repo_root().join("tests/fixtures/parity/cz4a_pv_only")
}

fn start_at(rfc3339: &str) -> DateTime<chrono::FixedOffset> {
    DateTime::parse_from_rfc3339(rfc3339).expect("start time")
}

fn sim_config(start_time: DateTime<chrono::FixedOffset>, days: i64) -> SimulationConfig {
    SimulationConfig {
        start_time,
        duration: Duration::days(days),
        time_res: Duration::seconds(900),
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
        civil_timezone: None,
        site_location: hares_io::SiteLocationOverride::default(),
        max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
        health_policy: hares_io::HealthPolicy::default(),
        first_hour_interpolation: Default::default(),
        weather_year: hares_io::WeatherYear::default(),
        warmup: hares_io::Warmup::Disabled,
    }
}

fn cz4a_settings(start_time: DateTime<chrono::FixedOffset>, days: i64) -> DwellingSettings {
    DwellingSettings {
        sim_config: sim_config(start_time, days),
        overrides: None,
        bldg_id: 1,
        patches: None,
    }
}

fn cz4a_config(start_time: DateTime<chrono::FixedOffset>, days: i64) -> hares_core::DwellingConfig {
    hares_core::DwellingConfig {
        hpxml: hares_io::InputSource::Path(cz4a().join("building.xml")),
        schedule: Some(hares_io::InputSource::Path(cz4a().join("schedule.csv"))),
        weather: hares_io::WeatherInput::Path(cz4a().join("weather.epw")),
        defaults: hares_io::DefaultsSource::directory(repo_root().join("python/hares/defaults")),
        settings: cz4a_settings(start_time, days),
    }
}

fn cz4a_pv_config(
    start_time: DateTime<chrono::FixedOffset>,
    days: i64,
) -> hares_core::DwellingConfig {
    hares_core::DwellingConfig {
        hpxml: hares_io::InputSource::Path(cz4a_pv().join("building.xml")),
        schedule: Some(hares_io::InputSource::Path(cz4a_pv().join("schedule.csv"))),
        weather: hares_io::WeatherInput::Path(cz4a_pv().join("weather.epw")),
        defaults: hares_io::DefaultsSource::directory(repo_root().join("python/hares/defaults")),
        settings: cz4a_settings(start_time, days),
    }
}

fn run_frame(config: &hares_core::DwellingConfig) -> RecordBatch {
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

/// Runs one config twice, once over the shared table and once with the
/// per-step computation (the per-step forcing path: the per-surface
/// irradiance computes per step there, the table is neither built nor
/// read), and returns both runs' memory frames.
fn table_and_per_step_frames(config: &hares_core::DwellingConfig) -> (RecordBatch, RecordBatch) {
    let table_run = run_frame(config);
    FORCE_PER_STEP_FORCING.store(true, Ordering::SeqCst);
    let per_step_run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_frame(config)));
    FORCE_PER_STEP_FORCING.store(false, Ordering::SeqCst);
    (table_run, per_step_run.expect("per-step run"))
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

    /// A blueprint over the shared handles: the construction whose table
    /// resolution the tests probe. The parsed weather handle is shared, so
    /// the blueprint's forcing table (and the per-surface table built on
    /// it) resolve once across every blueprint these tests build.
    fn blueprint(&self, bldg_id: i64) -> Arc<DwellingBlueprint> {
        let inputs = hares_io::ParsedInputs::from_handles(
            Arc::clone(&self.hpxml),
            Some(Arc::clone(&self.schedule)),
            Arc::clone(&self.weather),
            Arc::clone(&self.defaults),
            None,
        )
        .expect("the shared inputs assemble");
        let mut settings = cz4a_settings(start_at("2023-01-01T00:00:00-07:00"), 2);
        settings.bldg_id = bldg_id;
        DwellingBlueprintBuilder::new(inputs, settings)
            .expect("the builder builds")
            .freeze()
            .expect("the blueprint freezes")
    }
}

/// The row's named acceptance: two dwellings built from ONE blueprint hold
/// the same per-surface irradiance table (`Arc::ptr_eq`), and the
/// blueprint-built dwellings' frames equal the uncached run bitwise.
#[test]
fn per_surface_table_shared_across_builds_of_one_blueprint() {
    let _counter_exclusive = COUNTER_EXCLUSIVITY
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let shared = SharedCz4a::new();
    let bp = shared.blueprint(1);

    let dwelling_a = bp.build().expect("first build");
    let dwelling_b = bp.build().expect("second build");

    let table_a = dwelling_a
        .environment
        .shared_surface_irradiance()
        .expect("the blueprint's build carries the table")
        .clone();
    let table_b = dwelling_b
        .environment
        .shared_surface_irradiance()
        .expect("the blueprint's build carries the table")
        .clone();
    assert!(
        Arc::ptr_eq(&table_a, &table_b),
        "two dwellings of one blueprint hold one per-surface table"
    );
    // The horizon rows: a two-day 900 s run's steps, one row per step.
    assert_eq!(table_a.len(), 192, "one row per step of the run's horizon");

    // Two runs of one config read the same table and produce bitwise
    // identical frames; the table run equals the per-step computation's
    // own frame bitwise (the table computes the same functions of the
    // same inputs).
    let config = cz4a_config(start_at("2023-01-01T00:00:00-07:00"), 2);
    let frame_a = run_frame(&config);
    let frame_b = run_frame(&config);
    assert_frames_bitwise_equal(&frame_a, &frame_b, "two runs of one config");
    let (table_run, per_step_run) = table_and_per_step_frames(&config);
    assert_frames_bitwise_equal(&table_run, &per_step_run, "table run vs per-step run");
}

/// The synthetic bench fixture's shape: a dwelling from a written TOML,
/// each construction an independent build (no shared handles, a fresh
/// parse per construction). The `floor_area_m2` sets the envelope's
/// surface areas, so two values are two different contents.
fn synthetic_dwelling(dir: &Path, floor_area_m2: f64, name: &str) -> Dwelling {
    let path = dir.join(format!("{name}.toml"));
    std::fs::write(
        &path,
        format!(
            r#"building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res = 60
duration = 86400
output_chunk_size = 1000
master_seed = 0
[geometry]
number_of_bedrooms = 3.0
floor_area_m2 = {floor_area_m2}
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 25.0

[weather]
outdoor_temp_c = 8.0
dew_point_c = 4.0
rel_humidity_pct = 55.0
pressure_kpa = 101.325

[infiltration]
ach = 3.0

[schedule]
occupancy = 1.0
"#
        ),
    )
    .expect("write the synthetic TOML");
    Dwelling::from_toml_config_with_reports(&path, Some(Vec::new())).expect("build")
}

/// The content contract across independent constructions: two dwellings
/// built from independently parsed, byte-identical inputs (the benchmark
/// batch's shape: one fixture, many constructions) hold ONE table
/// instance (`Arc::ptr_eq` across blueprints, not only within one), and
/// the table is the run's own horizon (a one-day 60 s run: 1,440 rows).
#[test]
fn independently_constructed_blueprints_of_one_content_share_the_table() {
    let _counter_exclusive = COUNTER_EXCLUSIVITY
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let dir = tempfile::tempdir().expect("temp dir");
    let dwelling_a = synthetic_dwelling(dir.path(), 48.0, "content-a");
    let dwelling_b = synthetic_dwelling(dir.path(), 48.0, "content-b");

    let table_a = dwelling_a
        .environment
        .shared_surface_irradiance()
        .expect("the first build carries the table")
        .clone();
    let table_b = dwelling_b
        .environment
        .shared_surface_irradiance()
        .expect("the second build carries the table")
        .clone();
    assert!(
        Arc::ptr_eq(&table_a, &table_b),
        "two independently constructed dwellings of one content hold one table"
    );
    assert_eq!(
        table_a.len(),
        1_440,
        "one row per step of the run's horizon"
    );
}

/// Different content never shares: two independently constructed
/// dwellings whose surfaces differ (different envelope areas) hold
/// different tables. The key carries every bit the build reads, so no
/// content difference collapses into a shared table.
#[test]
fn dwellings_of_different_content_never_share_the_table() {
    let _counter_exclusive = COUNTER_EXCLUSIVITY
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let dir = tempfile::tempdir().expect("temp dir");
    let dwelling_a = synthetic_dwelling(dir.path(), 48.0, "content-a");
    let dwelling_b = synthetic_dwelling(dir.path(), 60.0, "content-b");

    let table_a = dwelling_a
        .environment
        .shared_surface_irradiance()
        .expect("the first build carries the table")
        .clone();
    let table_b = dwelling_b
        .environment
        .shared_surface_irradiance()
        .expect("the second build carries the table")
        .clone();
    assert!(
        !Arc::ptr_eq(&table_a, &table_b),
        "dwellings of different content never share a per-surface table"
    );
    assert_ne!(
        table_a.surfaces_per_row(),
        0,
        "both constructions built tables"
    );
}

/// A PV-bearing home: the surfaces a PV registration appended compute per
/// step while the blueprint's own surfaces read the table, and the mixed
/// run equals the per-step computation's frame bitwise.
#[test]
fn a_pv_surfaces_appended_after_the_blueprint_compute_per_step() {
    let _counter_exclusive = COUNTER_EXCLUSIVITY
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let start = start_at("2023-01-01T00:00:00-07:00");
    let (table_run, per_step_run) = table_and_per_step_frames(&cz4a_pv_config(start, 2));
    assert_frames_bitwise_equal(
        &table_run,
        &per_step_run,
        "the PV-bearing home's table run vs per-step run",
    );
}

/// The table's build adds no duplicate forcing computation: the blueprint's
/// first build resolves the forcing table once (the counter's construction
/// increment), the per-surface table's build reads its rows and moves the
/// counter no further.
#[test]
fn the_table_build_moves_no_forcing_computation() {
    let _counter_exclusive = COUNTER_EXCLUSIVITY
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let before = FORCING_COMPUTATIONS.load(Ordering::SeqCst);
    let shared = SharedCz4a::new();
    let bp = shared.blueprint(1);
    let _dwelling = bp.build().expect("build");
    let after = FORCING_COMPUTATIONS.load(Ordering::SeqCst);
    assert_eq!(
        after - before,
        1,
        "one blueprint build over fresh handles: one forcing construction, \
         the per-surface table's build reads the forcing rows"
    );
}
