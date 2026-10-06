use std::fs;
use std::path::PathBuf;

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::{DwellingConfig, SimStatus, SimulationConfig, SimulationEngine};
use hares_io::OutputFormat;
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
    [
        "Time,Clothes Washer (kW),HVAC Heating (C)",
        "2021-01-01T00:00:00-07:00,0.10,20.0",
        "2021-01-01T00:15:00-07:00,0.20,20.5",
        "2021-01-01T00:30:00-07:00,0.30,21.0",
        "2021-01-01T00:45:00-07:00,0.40,21.5",
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

fn simulation_config(output_path: PathBuf) -> SimulationConfig {
    simulation_config_with_flags(output_path, true, true)
}

fn simulation_config_with_flags(
    output_path: PathBuf,
    retain_batches: bool,
    write_output: bool,
) -> SimulationConfig {
    SimulationConfig {
        start_time: FixedOffset::east_opt(0)
            .expect("UTC offset")
            .with_ymd_and_hms(2026, 3, 21, 12, 0, 0)
            .unwrap(),
        duration: Duration::hours(1),
        time_res: Duration::minutes(1),
        output_verbosity: 0,
        output_path: Some(output_path),
        write_output,
        output_format: OutputFormat::Csv,
        output_chunk_size: 128,
        setpoint_deadband_c: None,
        master_seed: 0,
        civil_timezone: None,
        site_location: hares_io::SiteLocationOverride::default(),
        retain_batches,
        rotation: hares_io::RotationPolicy::None,
    }
}

#[test]
fn run_success_produces_metrics_elapsed_and_output_path() {
    let scratch = Scratch::with_inputs();
    let output_path = scratch.path("output.csv");

    let config = DwellingConfig {
        hpxml_path: fixture_hpxml_path(),
        schedule_path: Some(scratch.schedule()),
        weather_path: scratch.weather(),
        sim_config: simulation_config(output_path.clone()),
        defaults_path: Some(repo_defaults_path()),
        overrides: None,
        bldg_id: 123,
        initialization_duration: None,
        resample_overrides: None,
        patches: None,
    };

    let engine = SimulationEngine::new();
    let result = engine.run(config).expect("engine run should succeed");

    assert!(matches!(
        result.status,
        SimStatus::Ok | SimStatus::Flagged(_)
    ));
    assert!(result.elapsed > std::time::Duration::ZERO);
    assert!(result.metrics.total_energy_kwh.net_energy_kwh.is_finite());
    assert!(result.metrics.total_energy_kwh.net_energy_kwh > 0.0);
    assert!(
        result
            .metrics
            .peak_power_kw
            .rolling
            .peak_15min_kw
            .is_finite()
    );
    assert!(result.timeseries.is_some());
    assert!(!result.timeseries.unwrap().is_empty());
    assert_eq!(result.timeseries_path, Some(output_path.clone()));
    assert!(output_path.exists());
}

#[test]
fn run_with_zero_duration_reports_zero_step() {
    // The genuine zero-step edge case (duration == 0) stays distinguishable
    // from the not-retained case.
    let scratch = Scratch::with_inputs();

    let mut sim_config = simulation_config(scratch.path("output.csv"));
    sim_config.duration = Duration::zero();
    sim_config.retain_batches = true;

    let config = DwellingConfig {
        hpxml_path: fixture_hpxml_path(),
        schedule_path: Some(scratch.schedule()),
        weather_path: scratch.weather(),
        sim_config,
        defaults_path: Some(repo_defaults_path()),
        overrides: None,
        bldg_id: 123,
        initialization_duration: None,
        resample_overrides: None,
        patches: None,
    };

    let engine = SimulationEngine::new();
    let result = engine.run(config).expect("engine run should succeed");

    match &result.status {
        SimStatus::Flagged(reason) => assert!(
            reason.contains("zero-step"),
            "duration == 0 must be reported as zero-step, got: {reason}"
        ),
        other => panic!("expected Flagged status, got: {other:?}"),
    }
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
        resample_overrides: None,
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
        resample_overrides: None,
        patches: None,
    };
    let weather_err = engine
        .run(weather_missing)
        .expect_err("missing weather path should return error");
    assert!(weather_err.to_string().contains("weather_path"));
}

/// A streaming run (retain_batches=false, the default configuration shape)
/// must still produce real run metrics: they are collected incrementally at
/// flush time. The previous behavior returned zeroed metrics with a false
/// "zero-step" flag for every non-retaining run.
#[test]
fn run_with_streaming_output_computes_metrics() {
    let scratch = Scratch::with_inputs();

    let build_config = |retain: bool| DwellingConfig {
        hpxml_path: fixture_hpxml_path(),
        schedule_path: Some(scratch.schedule()),
        weather_path: scratch.weather(),
        sim_config: simulation_config_with_flags(
            scratch.path(&format!("retain_{retain}.csv")),
            retain,
            true,
        ),
        defaults_path: Some(repo_defaults_path()),
        overrides: None,
        bldg_id: 123,
        initialization_duration: None,
        resample_overrides: None,
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
    // timeseries is empty -- but the metrics must be fully computed.
    assert!(
        streamed.timeseries.as_ref().is_some_and(Vec::is_empty),
        "streaming run must not retain batches"
    );
    assert_eq!(
        streamed.metrics.simulation_duration_hours, 1.0,
        "streaming run must compute metrics (zeroed metrics report 0 hours)"
    );
    assert!(
        streamed.metrics.total_energy_kwh.net_energy_kwh > 0.0,
        "streaming run must report real energy totals"
    );
    assert!(
        !matches!(&streamed.status, SimStatus::Flagged(msg)
            if msg.contains("zero-step") || msg.contains("metrics unavailable")),
        "a fully-recorded streaming run must not be flagged as zero-step or \
         metrics-unavailable, got: {:?}",
        streamed.status
    );

    // The incremental calculator sees the same batches in the same order as
    // a post-hoc pass over retained batches, so both paths must agree.
    assert_eq!(
        streamed.metrics, retained.metrics,
        "streaming (incremental) metrics must equal retained-batch metrics"
    );
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
            let output_path = scratch.path(&format!("retain_{retain}.csv"));
            let mut cfg = simulation_config_with_flags(output_path, retain, true);
            cfg.output_verbosity = 6;
            cfg
        },
        defaults_path: Some(repo_defaults_path()),
        overrides: None,
        bldg_id: 123,
        initialization_duration: None,
        resample_overrides: None,
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
    assert!(
        streamed.metrics.envelope_loads_kwh.is_some(),
        "verbosity 6 must produce envelope-load columns"
    );
    assert_eq!(
        streamed.metrics.envelope_loads_kwh, retained.metrics.envelope_loads_kwh,
        "streaming and batch envelope loads must agree exactly"
    );
    assert_eq!(
        streamed.metrics, retained.metrics,
        "streaming (incremental) metrics must equal retained-batch metrics \
         field-for-field, including energy totals"
    );
}

/// A run with no timesteps must be flagged as a true zero-step run -- not
/// reported as Ok with zeroed metrics.
#[test]
fn run_with_zero_duration_flags_zero_step() {
    let scratch = Scratch::with_inputs();

    let mut sim_config = simulation_config_with_flags(scratch.path("output.csv"), false, true);
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
        resample_overrides: None,
        patches: None,
    };

    let engine = SimulationEngine::new();
    let result = engine
        .run(config)
        .expect("zero-duration run should succeed");

    assert!(
        matches!(&result.status, SimStatus::Flagged(msg) if msg.contains("zero-step")),
        "zero-duration run must be flagged as zero-step, got: {:?}",
        result.status
    );
    assert_eq!(
        result.metrics.simulation_duration_hours, 0.0,
        "zero-step run must report zero duration"
    );
}

/// A run with output disabled has nothing to compute metrics from; the
/// status must say so honestly instead of misreporting a zero-step run.
#[test]
fn run_without_output_recorder_reports_metrics_unavailable() {
    let scratch = Scratch::with_inputs();

    let config = DwellingConfig {
        hpxml_path: fixture_hpxml_path(),
        schedule_path: Some(scratch.schedule()),
        weather_path: scratch.weather(),
        sim_config: simulation_config_with_flags(scratch.path("output.csv"), false, false),
        defaults_path: Some(repo_defaults_path()),
        overrides: None,
        bldg_id: 123,
        initialization_duration: None,
        resample_overrides: None,
        patches: None,
    };

    let engine = SimulationEngine::new();
    let result = engine
        .run(config)
        .expect("output-disabled run should succeed");

    assert!(
        matches!(&result.status, SimStatus::Flagged(msg) if msg.contains("no output recorder")),
        "output-disabled run must be flagged with the no-recorder reason, got: {:?}",
        result.status
    );
    assert_eq!(
        result.metrics.simulation_duration_hours, 0.0,
        "output-disabled run has no metrics source"
    );
}
