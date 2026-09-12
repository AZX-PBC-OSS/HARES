use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::{DwellingConfig, SimStatus, SimulationConfig, SimulationEngine};
use hares_io::OutputFormat;

fn unique_temp_path(suffix: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos();
    path.push(format!("hares-core-engine-{nanos}.{suffix}"));
    path
}

fn write_temp_file(path: &Path, contents: &str) {
    fs::write(path, contents).expect("failed to write temp file");
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
    let schedule_path = unique_temp_path("csv");
    let weather_path = unique_temp_path("epw");
    let output_path = unique_temp_path("csv");
    write_temp_file(&schedule_path, &build_schedule_csv());
    write_temp_file(&weather_path, &build_epw_8760());

    let config = DwellingConfig {
        hpxml_path: fixture_hpxml_path(),
        schedule_path: schedule_path.clone(),
        weather_path: weather_path.clone(),
        sim_config: simulation_config(output_path.clone()),
        defaults_path: None,
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

    let _ = fs::remove_file(schedule_path);
    let _ = fs::remove_file(weather_path);
    let _ = fs::remove_file(output_path);
}

#[test]
fn run_returns_err_for_missing_hpxml_path() {
    let config = DwellingConfig {
        hpxml_path: PathBuf::from("/tmp/does-not-exist-hpxml.xml"),
        schedule_path: PathBuf::from("/tmp/schedule.csv"),
        weather_path: PathBuf::from("/tmp/weather.epw"),
        sim_config: simulation_config(unique_temp_path("csv")),
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
    let weather_missing = DwellingConfig {
        hpxml_path: fixture_hpxml_path(),
        schedule_path: unique_temp_path("csv"),
        weather_path: PathBuf::from("/tmp/does-not-exist-weather.epw"),
        sim_config: simulation_config(unique_temp_path("csv")),
        defaults_path: None,
        overrides: None,
        bldg_id: 2,
        initialization_duration: None,
        resample_overrides: None,
        patches: None,
    };
    fs::write(&weather_missing.schedule_path, build_schedule_csv())
        .expect("failed to write temp schedule");
    let weather_err = engine
        .run(weather_missing.clone())
        .expect_err("missing weather path should return error");
    assert!(weather_err.to_string().contains("weather_path"));

    let _ = fs::remove_file(weather_missing.schedule_path);
}

/// A streaming run (retain_batches=false, the default configuration shape)
/// must still produce real run metrics: they are collected incrementally at
/// flush time. The previous behavior returned zeroed metrics with a false
/// "zero-step" flag for every non-retaining run.
#[test]
fn run_with_streaming_output_computes_metrics() {
    let schedule_path = unique_temp_path("csv");
    let weather_path = unique_temp_path("epw");
    write_temp_file(&schedule_path, &build_schedule_csv());
    write_temp_file(&weather_path, &build_epw_8760());

    let build_config = |output_path: PathBuf, retain: bool| DwellingConfig {
        hpxml_path: fixture_hpxml_path(),
        schedule_path: schedule_path.clone(),
        weather_path: weather_path.clone(),
        sim_config: simulation_config_with_flags(output_path, retain, true),
        defaults_path: None,
        overrides: None,
        bldg_id: 123,
        initialization_duration: None,
        resample_overrides: None,
        patches: None,
    };

    let engine = SimulationEngine::new();
    let retained = engine
        .run(build_config(unique_temp_path("csv"), true))
        .expect("retained run should succeed");
    let streamed = engine
        .run(build_config(unique_temp_path("csv"), false))
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

    let _ = fs::remove_file(schedule_path);
    let _ = fs::remove_file(weather_path);
}

/// Streaming (incremental recorder-fed) metrics must equal batch (retained-
/// batches) metrics on a real fixture at a verbosity where the envelope-load
/// columns exist -- field-for-field, including `envelope_loads_kwh`. The two
/// paths share the calculator; this pins that no plumbing rework of either
/// path can silently diverge them (the class of breakage the resstock
/// energy identity exposed during I-02).
#[test]
fn streaming_metrics_equal_batch_metrics_at_envelope_verbosity() {
    let schedule_path = unique_temp_path("csv");
    let weather_path = unique_temp_path("epw");
    write_temp_file(&schedule_path, &build_schedule_csv());
    write_temp_file(&weather_path, &build_epw_8760());

    let build_config = |retain: bool| DwellingConfig {
        hpxml_path: fixture_hpxml_path(),
        schedule_path: schedule_path.clone(),
        weather_path: weather_path.clone(),
        sim_config: {
            let mut cfg = simulation_config_with_flags(unique_temp_path("csv"), retain, true);
            cfg.output_verbosity = 6;
            cfg
        },
        defaults_path: None,
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

    let _ = fs::remove_file(schedule_path);
    let _ = fs::remove_file(weather_path);
}

/// A run with no timesteps must be flagged as a true zero-step run -- not
/// reported as Ok with zeroed metrics.
#[test]
fn run_with_zero_duration_flags_zero_step() {
    let schedule_path = unique_temp_path("csv");
    let weather_path = unique_temp_path("epw");
    write_temp_file(&schedule_path, &build_schedule_csv());
    write_temp_file(&weather_path, &build_epw_8760());

    let mut sim_config = simulation_config_with_flags(unique_temp_path("csv"), false, true);
    sim_config.duration = Duration::zero();

    let config = DwellingConfig {
        hpxml_path: fixture_hpxml_path(),
        schedule_path: schedule_path.clone(),
        weather_path: weather_path.clone(),
        sim_config,
        defaults_path: None,
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

    let _ = fs::remove_file(schedule_path);
    let _ = fs::remove_file(weather_path);
}

/// A run with output disabled has nothing to compute metrics from; the
/// status must say so honestly instead of misreporting a zero-step run.
#[test]
fn run_without_output_recorder_reports_metrics_unavailable() {
    let schedule_path = unique_temp_path("csv");
    let weather_path = unique_temp_path("epw");
    write_temp_file(&schedule_path, &build_schedule_csv());
    write_temp_file(&weather_path, &build_epw_8760());

    let config = DwellingConfig {
        hpxml_path: fixture_hpxml_path(),
        schedule_path: schedule_path.clone(),
        weather_path: weather_path.clone(),
        sim_config: simulation_config_with_flags(unique_temp_path("csv"), false, false),
        defaults_path: None,
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

    let _ = fs::remove_file(schedule_path);
    let _ = fs::remove_file(weather_path);
}
