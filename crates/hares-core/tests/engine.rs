use std::fs;
use std::path::PathBuf;

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::{DwellingConfig, SimulationConfig, SimulationEngine};
use tempfile::TempDir;

/// The test's own directory, removed on drop (panics included), holding the
/// schedule and weather inputs the runs read and the outputs they write.
struct Scratch(TempDir);

impl Scratch {
    fn with_inputs() -> Self {
        let scratch = Scratch(tempfile::tempdir().expect("temp dir"));
        fs::write(scratch.schedule(), build_schedule_csv()).expect("write schedule");
        fs::write(scratch.weather(), build_epw_8760()).expect("write weather");
        scratch
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.path().join(name)
    }

    fn schedule(&self) -> PathBuf {
        self.path("schedule.csv")
    }

    fn weather(&self) -> PathBuf {
        self.path("weather.epw")
    }
}

fn build_schedule_csv() -> String {
    // The columns the base fixture's event equipment and wet appliances run
    // on: a schedule file without an appliance's power column is a
    // construction error, and the washer and dishwasher draw their hot water
    // through their hot-water columns.
    [
        "Time,occupants,clothes_washer,clothes_dryer,dishwasher,cooking_range,hot_water_clothes_washer,hot_water_dishwasher,heating_setpoint",
        "2021-01-01T00:00:00-07:00,1.0,0.10,0.10,0.10,0.10,0.10,0.10,20.0",
        "2021-01-01T00:15:00-07:00,1.0,0.20,0.20,0.20,0.20,0.20,0.20,20.5",
        "2021-01-01T00:30:00-07:00,1.0,0.30,0.30,0.30,0.30,0.30,0.30,21.0",
        "2021-01-01T00:45:00-07:00,1.0,0.40,0.40,0.40,0.40,0.40,0.40,21.5",
    ]
    .join("\n")
}

fn build_epw_8760() -> String {
    use chrono::{Datelike, Duration, NaiveDate, Timelike};

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

fn fixture_hpxml_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/hpxml/ochre_samples/base.xml")
}

/// The repo's defaults directory: equipment whose schedule source is missing
/// (no schedule column, no HPXML fractions) resolves its default profile
/// there instead of erroring.
fn repo_defaults_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../defaults")
}

fn simulation_config(csv_path: PathBuf) -> SimulationConfig {
    simulation_config_with_frame(csv_path, true)
}

/// `with_frame` is the retained run (a memory report beside the csv file,
/// the frame and the file both read back); without it the run streams to
/// the file alone.
fn simulation_config_with_frame(csv_path: PathBuf, with_frame: bool) -> SimulationConfig {
    let mut reports = Vec::new();
    if with_frame {
        reports.push(memory_report("frame", 0));
    }
    reports.push(csv_report("file", &csv_path, 0));
    SimulationConfig {
        start_time: FixedOffset::east_opt(0)
            .expect("UTC offset")
            .with_ymd_and_hms(2026, 3, 21, 12, 0, 0)
            .unwrap(),
        duration: Duration::hours(1),
        time_res: Duration::minutes(1),
        reports,
        diagnostics: false,
        metrics: true,
        output_chunk_size: 128,
        setpoint_deadband_c: None,
        master_seed: 0,
        civil_timezone: None,
        site_location: hares_io::SiteLocationOverride::default(),
        max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
        health_policy: hares_io::HealthPolicy::default(),
    }
}

fn memory_report(name: &str, verbosity: u8) -> hares_io::ReportSpec {
    hares_io::ReportSpec {
        name: name.to_string(),
        selection: hares_io::Selection::Verbosity(verbosity),
        frequency: hares_io::Frequency::default(),
        destination: hares_io::Destination::Memory,
    }
}

fn csv_report(name: &str, path: &std::path::Path, verbosity: u8) -> hares_io::ReportSpec {
    hares_io::ReportSpec {
        name: name.to_string(),
        selection: hares_io::Selection::Verbosity(verbosity),
        frequency: hares_io::Frequency::default(),
        destination: hares_io::Destination::Csv {
            path: path.to_path_buf(),
            overwrite: false,
        },
    }
}

#[test]
fn run_success_produces_metrics_and_output_path() {
    let scratch = Scratch::with_inputs();
    let csv_path = scratch.path("output.csv");

    let config = DwellingConfig {
        hpxml_path: fixture_hpxml_path(),
        schedule_path: Some(scratch.schedule()),
        weather_path: scratch.weather(),
        sim_config: simulation_config(csv_path.clone()),
        defaults_path: Some(repo_defaults_path()),
        overrides: None,
        bldg_id: 123,
        initialization_duration: None,
        patches: None,
    };

    let engine = SimulationEngine::new();
    let result = engine.run(config).expect("engine run should succeed");

    let metrics = result
        .metrics
        .as_ref()
        .expect("the run requested metrics");
    assert!(metrics.total_energy_kwh.net_energy_kwh.is_finite());
    assert!(metrics.total_energy_kwh.net_energy_kwh > 0.0);
    assert!(metrics.peak_power_kw.rolling.peak_15min_kw.is_finite());

    // The file report's published path is the csv report's target; the
    // memory report retains the run's batches.
    let file = result
        .reports
        .iter()
        .find(|report| report.name == "file")
        .expect("the csv report is in the result");
    assert_eq!(file.path(), Some(&csv_path));
    assert!(csv_path.exists());
    let frame = result
        .reports
        .iter()
        .find(|report| report.name == "frame")
        .expect("the memory report is in the result");
    assert!(
        !frame.batches().is_empty(),
        "the frame report retains batches"
    );
}

#[test]
fn run_returns_err_for_missing_hpxml_path() {
    let scratch = Scratch::with_inputs();
    let config = DwellingConfig {
        hpxml_path: scratch.path("does-not-exist.xml"),
        schedule_path: Some(scratch.schedule()),
        weather_path: scratch.weather(),
        sim_config: simulation_config(scratch.path("output.csv")),
        defaults_path: None,
        overrides: None,
        bldg_id: 1,
        initialization_duration: None,
        patches: None,
    };

    let engine = SimulationEngine::new();
    let err = engine
        .run(config)
        .expect_err("missing hpxml path should return error");
    assert!(err.to_string().contains("hpxml_path"));
}

#[test]
fn run_returns_err_for_missing_weather_path() {
    let engine = SimulationEngine::new();
    let scratch = Scratch::with_inputs();
    let weather_missing = DwellingConfig {
        hpxml_path: fixture_hpxml_path(),
        schedule_path: Some(scratch.schedule()),
        weather_path: scratch.path("does-not-exist.epw"),
        sim_config: simulation_config(scratch.path("output.csv")),
        defaults_path: None,
        overrides: None,
        bldg_id: 2,
        initialization_duration: None,
        patches: None,
    };
    let weather_err = engine
        .run(weather_missing)
        .expect_err("missing weather path should return error");
    assert!(weather_err.to_string().contains("weather_path"));
}

/// A file-only run (a csv report with no `Memory` report beside it) must
/// still produce real run metrics: they are collected incrementally at
/// flush time. The previous behavior returned zeroed metrics with a false
/// "zero-step" flag for every run that retained no batches.
#[test]
fn run_with_streaming_output_computes_metrics() {
    let scratch = Scratch::with_inputs();

    let build_config = |retain: bool| DwellingConfig {
        hpxml_path: fixture_hpxml_path(),
        schedule_path: Some(scratch.schedule()),
        weather_path: scratch.weather(),
        sim_config: simulation_config_with_frame(
            scratch.path(&format!("retain_{retain}.csv")),
            retain,
        ),
        defaults_path: Some(repo_defaults_path()),
        overrides: None,
        bldg_id: 123,
        initialization_duration: None,
        patches: None,
    };

    let engine = SimulationEngine::new();
    let retained = engine
        .run(build_config(true))
        .expect("retained run should succeed");
    let streamed = engine
        .run(build_config(false))
        .expect("streaming run should succeed");

    // Nothing is retained in memory on the streaming run -- the honest
    // report is empty -- but the metrics must be fully computed.
    assert!(
        memory_batches(&streamed).is_empty(),
        "streaming run must not retain batches"
    );
    let streamed_metrics = streamed
        .metrics
        .as_ref()
        .expect("streaming run must compute metrics (None reports no computation)");
    assert_eq!(
        streamed_metrics.simulation_duration_hours, 1.0,
        "streaming run must compute metrics (zeroed metrics report 0 hours)"
    );
    assert!(
        streamed_metrics.total_energy_kwh.net_energy_kwh > 0.0,
        "streaming run must report real energy totals"
    );

    // One path: every run's accumulator is fed per step from the row
    // plan's producer slots, so a file-only run's metrics equal a
    // memory-report run's, on bits.
    assert_eq!(
        streamed.metrics, retained.metrics,
        "a file-only run's metrics must equal a memory-report run's metrics"
    );
}

fn memory_batches(result: &hares_core::RunResult) -> Vec<arrow::record_batch::RecordBatch> {
    result
        .reports
        .iter()
        .flat_map(|report| report.batches().to_vec())
        .collect()
}

/// Streaming (incremental recorder-fed) metrics must equal batch (retained-
/// batches) metrics on a real fixture at a verbosity where the envelope-load
/// columns exist -- field-for-field, including `envelope_loads_kwh`. The two
/// paths share the calculator; this pins that no plumbing rework of either
/// path can silently diverge them (the class of breakage the resstock
/// energy identity exposed during I-02).
#[test]
fn streaming_metrics_equal_batch_metrics_at_envelope_verbosity() {
    let scratch = Scratch::with_inputs();
    let build_config = |retain: bool| DwellingConfig {
        hpxml_path: fixture_hpxml_path(),
        schedule_path: Some(scratch.schedule()),
        weather_path: scratch.weather(),
        sim_config: {
            let csv_path = scratch.path(&format!("retain_{retain}.csv"));
            let mut cfg = simulation_config_with_frame(csv_path, retain);
            for report in &mut cfg.reports {
                report.selection = hares_io::Selection::Verbosity(6);
            }
            cfg
        },
        defaults_path: Some(repo_defaults_path()),
        overrides: None,
        bldg_id: 123,
        initialization_duration: None,
        patches: None,
    };

    let engine = SimulationEngine::new();
    let retained = engine
        .run(build_config(true))
        .expect("retained run should succeed");
    let streamed = engine
        .run(build_config(false))
        .expect("streaming run should succeed");

    // Envelope loads must be present and compared, not just energy totals.
    let streamed_metrics = streamed
        .metrics
        .as_ref()
        .expect("streaming run must compute metrics");
    let retained_metrics = retained
        .metrics
        .as_ref()
        .expect("retained run must compute metrics");
    assert!(
        streamed_metrics.envelope_loads_kwh.is_some(),
        "verbosity 6 must produce envelope-load columns"
    );
    assert_eq!(
        streamed_metrics.envelope_loads_kwh, retained_metrics.envelope_loads_kwh,
        "streaming and batch envelope loads must agree exactly"
    );
    assert_eq!(
        streamed_metrics, retained_metrics,
        "the one path's metrics must agree field-for-field, including energy \
         totals"
    );
}

/// A zero-duration config is a validate error on the Rust construction path
/// too: the same check from_toml runs, so a run with no timesteps cannot
/// reach the engine through a built dwelling. The engine's zero-step flag
/// stays as the finalization defense for a recorder that recorded no rows.
#[test]
fn run_with_zero_duration_is_a_validate_error() {
    let scratch = Scratch::with_inputs();

    let mut sim_config = simulation_config_with_frame(scratch.path("output.csv"), false);
    sim_config.duration = Duration::zero();

    let config = DwellingConfig {
        hpxml_path: fixture_hpxml_path(),
        schedule_path: Some(scratch.schedule()),
        weather_path: scratch.weather(),
        sim_config,
        defaults_path: Some(repo_defaults_path()),
        overrides: None,
        bldg_id: 123,
        initialization_duration: None,
        patches: None,
    };

    let engine = SimulationEngine::new();
    let err = engine
        .run(config)
        .expect_err("a zero-duration config must not build a dwelling");
    assert!(
        err.to_string()
            .contains("duration must be greater than zero"),
        "the error is validate()'s, got: {err}"
    );
}

/// A run that does not request metrics carries none: the absence of the
/// request is the contract, not a defect to warn about. `RunResult.metrics`
/// is `Some` exactly when `SimulationConfig.metrics` is requested.
#[test]
fn run_without_metrics_request_returns_none_metrics() {
    let scratch = Scratch::with_inputs();

    let config = DwellingConfig {
        hpxml_path: fixture_hpxml_path(),
        schedule_path: Some(scratch.schedule()),
        weather_path: scratch.weather(),
        sim_config: SimulationConfig {
            reports: Vec::new(),
            metrics: false,
            ..simulation_config(scratch.path("output.csv"))
        },
        defaults_path: Some(repo_defaults_path()),
        overrides: None,
        bldg_id: 123,
        initialization_duration: None,
        patches: None,
    };

    let engine = SimulationEngine::new();
    let result = engine
        .run(config)
        .expect("a run without the metrics request should succeed");

    assert!(
        result.metrics.is_none(),
        "a run without the metrics request carries none, got: {:?}",
        result.metrics.as_ref().map(|m| m.simulation_duration_hours)
    );
}
