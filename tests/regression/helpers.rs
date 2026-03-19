//! Shared helpers for regression sub-suites.
//!
//! All tests use the real OCHRE vendor fixtures (BEopt_example + Denver EPW)
//! rather than synthetic stubs, so determinism and performance results are
//! meaningful.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Duration, Utc};
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

fn vendor_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendors/OCHRE")
}

pub fn ochre_hpxml_path() -> PathBuf {
    vendor_dir().join("ochre/defaults/Input Files/BEopt_example.xml")
}

pub fn ochre_schedule_path() -> PathBuf {
    vendor_dir().join("ochre/defaults/Input Files/BEopt_example_schedule.csv")
}

pub fn ochre_weather_path() -> PathBuf {
    vendor_dir().join("ochre/defaults/Weather/USA_CO_Denver.Intl.AP.725650_TMY3.epw")
}

pub fn resstock_hpxml_path() -> PathBuf {
    vendor_dir().join("ochre/defaults/Input Files/bldg0112631-up00.xml")
}

pub fn resstock_schedule_path() -> PathBuf {
    vendor_dir().join("ochre/defaults/Input Files/bldg0112631_schedule.csv")
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

pub fn build_beopt_dwelling_config(
    bldg_id: i64,
    duration: Duration,
    seed: u64,
) -> DwellingConfig {
    DwellingConfig {
        hpxml_path: ochre_hpxml_path(),
        schedule_path: ochre_schedule_path(),
        weather_path: ochre_weather_path(),
        sim_config: SimulationConfig {
            start_time: Utc::now(),
            duration,
            time_res: Duration::minutes(1),
            output_verbosity: 0,
            output_path: Some(unique_temp_path("hares-regr-output", "csv")),
            output_format: OutputFormat::Csv,
            output_chunk_size: 1024,
            setpoint_deadband_c: None,
            master_seed: seed,
        },
        overrides: None,
        bldg_id,
        initialization_duration: None,
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
            start_time: Utc::now(),
            duration,
            time_res: Duration::minutes(1),
            output_verbosity: 0,
            output_path: Some(unique_temp_path("hares-regr-output", "csv")),
            output_format: OutputFormat::Csv,
            output_chunk_size: 1024,
            setpoint_deadband_c: None,
            master_seed: seed,
        },
        overrides: None,
        bldg_id,
        initialization_duration: None,
    }
}

pub fn cleanup_paths(paths: &[PathBuf]) {
    for p in paths {
        let _ = fs::remove_file(p);
    }
}
