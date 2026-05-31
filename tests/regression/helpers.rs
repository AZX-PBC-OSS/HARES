//! Shared helpers for regression sub-suites.
//!
//! All tests use the real OCHRE vendor fixtures (BEopt_example + Denver EPW)
//! rather than synthetic stubs, so determinism and performance results are
//! meaningful.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Datelike, Duration, FixedOffset, NaiveDate, Timelike, Utc};
use hares_core::{DwellingConfig, SimulationConfig};
use hares_io::OutputFormat;

static SEQ: AtomicU64 = AtomicU64::new(1);

pub fn unique_temp_path(prefix: &str, ext: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_nanos();
    let id = SEQ.fetch_add(1, Ordering::Relaxed);
    p.push(format!("{prefix}-{nanos}-{id}.{ext}"));
    p
}

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/examples")
}

pub fn ochre_hpxml_path() -> PathBuf {
    examples_dir().join("BEopt_example.xml")
}

pub fn ochre_schedule_path() -> PathBuf {
    examples_dir().join("BEopt_example_schedule.csv")
}

pub fn ochre_weather_path() -> PathBuf {
    examples_dir().join("USA_CO_Denver.Intl.AP.725650_TMY3.epw")
}

pub fn resstock_hpxml_path() -> PathBuf {
    examples_dir().join("bldg0112631-up00.xml")
}

pub fn resstock_schedule_path() -> PathBuf {
    examples_dir().join("bldg0112631_schedule.csv")
}

pub fn assert_vendor_fixtures_exist() {
    let paths = [
        ochre_hpxml_path(),
        ochre_schedule_path(),
        ochre_weather_path(),
        resstock_hpxml_path(),
        resstock_schedule_path(),
    ];
    for p in &paths {
        assert!(
            p.exists(),
            "required vendor fixture missing: {}",
            p.display()
        );
    }
}

pub fn build_beopt_dwelling_config(bldg_id: i64, duration: Duration, seed: u64) -> DwellingConfig {
    DwellingConfig {
        hpxml_path: ochre_hpxml_path(),
        schedule_path: ochre_schedule_path(),
        weather_path: ochre_weather_path(),
        sim_config: SimulationConfig {
            start_time: Utc::now().into(),
            duration,
            time_res: Duration::minutes(1),
            output_verbosity: 0,
            output_path: Some(unique_temp_path("hares-regr-output", "csv")),
            write_output: true,
            output_format: OutputFormat::Csv,
            output_chunk_size: 1024,
            setpoint_deadband_c: None,
            master_seed: seed,
            civil_timezone: None,
        },
        overrides: None,
        bldg_id,
        initialization_duration: None,
        resample_overrides: None,
        defaults_path: None,
        patches: None,
    }
}

pub fn build_resstock_dwelling_config(
    bldg_id: i64,
    duration: Duration,
    seed: u64,
) -> DwellingConfig {
    DwellingConfig {
        hpxml_path: resstock_hpxml_path(),
        schedule_path: resstock_schedule_path(),
        weather_path: ochre_weather_path(),
        sim_config: SimulationConfig {
            start_time: Utc::now().into(),
            duration,
            time_res: Duration::minutes(1),
            output_verbosity: 0,
            output_path: Some(unique_temp_path("hares-regr-output", "csv")),
            write_output: true,
            output_format: OutputFormat::Csv,
            output_chunk_size: 1024,
            setpoint_deadband_c: None,
            master_seed: seed,
            civil_timezone: None,
        },
        overrides: None,
        bldg_id,
        initialization_duration: None,
        resample_overrides: None,
        defaults_path: None,
        patches: None,
    }
}

pub fn cleanup_paths(paths: &[PathBuf]) {
    for p in paths {
        let _ = fs::remove_file(p);
    }
}

pub fn fixture_hpxml_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/hpxml/ochre_samples/base.xml")
}

pub fn write_schedule_csv(path: &PathBuf) {
    let rows = [
        "Time,Clothes Washer (kW),HVAC Heating (C),HVAC Cooling (C)",
        "2021-01-01T00:00:00-07:00,0.10,20.0,25.0",
        "2021-01-01T00:15:00-07:00,0.20,20.5,25.5",
        "2021-01-01T00:30:00-07:00,0.15,21.0,26.0",
        "2021-01-01T00:45:00-07:00,0.12,20.0,25.0",
    ]
    .join("\n");
    fs::write(path, rows).expect("failed to write schedule fixture");
}

pub fn write_weather_epw(path: &PathBuf) {
    let mut lines = vec![
        "LOCATION,Test Site,CO,USA,TMY3,999999,39.74,-104.99,-7.0,1609.3".to_string(),
        "DESIGN CONDITIONS,0".to_string(),
        "GROUND TEMPERATURES,0".to_string(),
        "TYPICAL/EXTREME PERIODS,0".to_string(),
        "HOLIDAYS/DAYLIGHT SAVINGS,Yes,0,0,0".to_string(),
        "COMMENTS 1,synthetic".to_string(),
        "COMMENTS 2,synthetic".to_string(),
        "DATA PERIODS,1,1,Data,Sunday, 1/ 1,12/31".to_string(),
    ];

    let start_date = NaiveDate::from_ymd_opt(2021, 1, 1).expect("valid date");
    let start_time = start_date
        .and_hms_opt(0, 0, 0)
        .expect("valid midnight timestamp");
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

    fs::write(path, lines.join("\n")).expect("failed to write weather fixture");
}

pub fn build_dwelling_config(
    bldg_id: i64,
    schedule_path: PathBuf,
    weather_path: PathBuf,
    duration: Duration,
    seed: u64,
) -> DwellingConfig {
    DwellingConfig {
        hpxml_path: fixture_hpxml_path(),
        schedule_path,
        weather_path,
        sim_config: SimulationConfig {
            start_time: Utc::now().with_timezone(&FixedOffset::east_opt(0).expect("UTC offset")),
            duration,
            time_res: Duration::minutes(1),
            output_verbosity: 0,
            output_path: Some(unique_temp_path("hares-regr-output", "csv")),
            write_output: true,
            output_format: OutputFormat::Csv,
            output_chunk_size: 1024,
            setpoint_deadband_c: None,
            master_seed: seed,
            civil_timezone: None,
        },
        overrides: None,
        bldg_id,
        initialization_duration: None,
        resample_overrides: None,
        defaults_path: None,
        patches: None,
    }
}
