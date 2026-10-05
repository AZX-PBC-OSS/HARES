// Each bench binary compiles this module and uses a different subset of it.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Duration;
use hares_core::{DwellingConfig, SimulationConfig};
use hares_io::OutputFormat;

#[path = "../tests/support/fixture_start.rs"]
mod fixture_start;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// `dir/<prefix>-<n>.<ext>` with a process-wide counter `n`, so every call
/// names a new file. `dir` is the benchmark's own directory, which each
/// benchmark function creates with `tempfile::tempdir()` before any timed
/// region and which is removed with everything in it when dropped.
pub fn numbered_path(dir: &Path, prefix: &str, ext: &str) -> PathBuf {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    dir.join(format!("{prefix}-{id}.{ext}"))
}

pub fn fixture_hpxml_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/hpxml/ochre_samples/base.xml")
}

/// The shipped defaults directory: the benchmark schedule has no lighting or
/// plug-load columns, so those loads take their default profiles from here.
pub fn repo_defaults_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../defaults")
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
    use chrono::{Datelike, Duration as ChronoDuration, NaiveDate, Timelike};

    let mut lines = vec![
        "LOCATION,Benchmark,CZ4A,USA,TMY3,999999,39.74,-104.99,-7.0,1609.3".to_string(),
        "DESIGN CONDITIONS,0".to_string(),
        "GROUND TEMPERATURES,0".to_string(),
        "TYPICAL/EXTREME PERIODS,0".to_string(),
        "HOLIDAYS/DAYLIGHT SAVINGS,No,0,0,0".to_string(),
        "COMMENTS 1,synthetic benchmark weather".to_string(),
        "COMMENTS 2,synthetic benchmark weather".to_string(),
        "DATA PERIODS,1,1,Data,Sunday, 1/ 1,12/31".to_string(),
    ];

    let start_date = NaiveDate::from_ymd_opt(2021, 1, 1).expect("valid date");
    let start_time = start_date.and_hms_opt(0, 0, 0).expect("valid time");
    for i in 0..8760 {
        let timestamp = start_time + ChronoDuration::hours(i as i64);
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
            "19.0".to_string(),
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
    dir: &Path,
    bldg_id: i64,
    schedule_path: PathBuf,
    weather_path: PathBuf,
    defaults_path: Option<PathBuf>,
    duration: Duration,
) -> DwellingConfig {
    DwellingConfig {
        hpxml_path: fixture_hpxml_path(),
        schedule_path: Some(schedule_path),
        weather_path,
        sim_config: SimulationConfig {
            start_time: fixture_start::fixture_start(),
            duration,
            time_res: Duration::minutes(1),
            output_verbosity: 0,
            output_path: Some(numbered_path(dir, "hares-bench-output", "csv")),
            write_output: true,
            output_format: OutputFormat::Csv,
            output_chunk_size: 1024,
            setpoint_deadband_c: None,
            master_seed: 0,
            retain_batches: false,
            civil_timezone: None,
            rotation: hares_io::RotationPolicy::default(),
            site_location: hares_io::SiteLocationOverride::default(),
        },
        defaults_path,
        overrides: None,
        bldg_id,
        initialization_duration: None,
        resample_overrides: None,
        patches: None,
    }
}

pub fn synthetic_toml_case(path: &PathBuf, duration_s: i64) {
    let toml = format!(
        r#"building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 60
duration_s = {duration_s}

[geometry]
floor_area_m2 = 48.0
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

[schedule]
occupancy = 1.0

[output]
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
master_seed = 0
"#
    );
    fs::write(path, toml).expect("failed to write synthetic TOML fixture");
}
