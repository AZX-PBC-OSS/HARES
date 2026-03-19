use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Duration, Utc};
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
    SimulationConfig {
        start_time: Utc::now(),
        duration: Duration::hours(1),
        time_res: Duration::minutes(1),
        output_verbosity: 0,
        output_path: Some(output_path),
        output_format: OutputFormat::Csv,
        output_chunk_size: 128,
        setpoint_deadband_c: None,
        master_seed: 0,
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
    };

    let engine = SimulationEngine::new();
    let result = engine.run(config).expect("engine run should succeed");

    assert!(matches!(
        result.status,
        SimStatus::Ok | SimStatus::Flagged(_)
    ));
    assert!(result.elapsed > std::time::Duration::ZERO);
    assert!(result.metrics.annual_energy_kwh.total.is_finite());
    assert!(result.metrics.annual_energy_kwh.total > 0.0);
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
    };

    let engine = SimulationEngine::new();
    let err = engine
        .run(config)
        .expect_err("missing hpxml path should return error");
    assert!(err.to_string().contains("hpxml_path"));
}

#[test]
fn run_returns_err_for_missing_schedule_or_weather_path() {
    let schedule_missing = DwellingConfig {
        hpxml_path: fixture_hpxml_path(),
        schedule_path: PathBuf::from("/tmp/does-not-exist-schedule.csv"),
        weather_path: PathBuf::from("/tmp/weather.epw"),
        sim_config: simulation_config(unique_temp_path("csv")),
        defaults_path: None,
        overrides: None,
        bldg_id: 2,
        initialization_duration: None,
    };

    let engine = SimulationEngine::new();
    let schedule_err = engine
        .run(schedule_missing)
        .expect_err("missing schedule path should return error");
    assert!(schedule_err.to_string().contains("schedule_path"));

    let weather_missing = DwellingConfig {
        hpxml_path: fixture_hpxml_path(),
        schedule_path: unique_temp_path("csv"),
        weather_path: PathBuf::from("/tmp/does-not-exist-weather.epw"),
        sim_config: simulation_config(unique_temp_path("csv")),
        defaults_path: None,
        overrides: None,
        bldg_id: 3,
        initialization_duration: None,
    };
    fs::write(&weather_missing.schedule_path, build_schedule_csv())
        .expect("failed to write temp schedule");
    let weather_err = engine
        .run(weather_missing.clone())
        .expect_err("missing weather path should return error");
    assert!(weather_err.to_string().contains("weather_path"));

    let _ = fs::remove_file(weather_missing.schedule_path);
}
