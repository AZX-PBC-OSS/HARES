//! Shared helpers for regression sub-suites.
//!
//! All tests use the real OCHRE vendor fixtures (BEopt_example + Denver EPW)
//! rather than synthetic stubs, so determinism and performance results are
//! meaningful.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{Datelike, Duration, NaiveDate, Timelike};
use hares_core::{DwellingConfig, SimulationConfig};

#[path = "../support/fixture_start.rs"]
mod fixture_start;
use fixture_start::fixture_start;

fn csv_path(output_dir: &Path, bldg_id: i64) -> PathBuf {
    output_dir.join(format!("dwelling-{bldg_id}.csv"))
}

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/examples")
}

/// The repo's defaults directory: equipment whose schedule source is missing
/// (no schedule column, no HPXML fractions) resolves its default profile
/// there instead of erroring.
fn repo_defaults_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../defaults")
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

pub fn build_beopt_dwelling_config(
    output_dir: &Path,
    bldg_id: i64,
    duration: Duration,
    seed: u64,
) -> DwellingConfig {
    DwellingConfig {
        hpxml_path: ochre_hpxml_path(),
        schedule_path: Some(ochre_schedule_path()),
        weather_path: ochre_weather_path(),
        sim_config: SimulationConfig {
            start_time: fixture_start(),
            duration,
            time_res: Duration::minutes(1),
            reports: vec![
                hares_io::ReportSpec {
                    name: "frame".to_string(),
                    selection: hares_io::Selection::Verbosity(0),
                    frequency: hares_io::Frequency::default(),
                    destination: hares_io::Destination::Memory,
                },
                hares_io::ReportSpec {
                    name: "file".to_string(),
                    selection: hares_io::Selection::Verbosity(0),
                    frequency: hares_io::Frequency::default(),
                    destination: hares_io::Destination::Csv {
                        path: csv_path(output_dir, bldg_id),
                        // The harness reruns a scenario (the determinism
                        // sweep's thread counts) into the same directory;
                        // the old writer truncated the file in place, and
                        // the reruns keep that behaviour.
                        overwrite: true,
                    },
                },
            ],
            diagnostics: false,
            output_chunk_size: 1024,
            setpoint_deadband_c: None,
            master_seed: seed,
            civil_timezone: None,
            site_location: hares_io::SiteLocationOverride::default(),
            max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
            health_policy: hares_io::HealthPolicy::default(),
        },
        overrides: None,
        bldg_id,
        initialization_duration: None,
        defaults_path: Some(repo_defaults_path()),
        patches: None,
    }
}

pub fn build_resstock_dwelling_config(
    output_dir: &Path,
    bldg_id: i64,
    duration: Duration,
    seed: u64,
) -> DwellingConfig {
    DwellingConfig {
        hpxml_path: resstock_hpxml_path(),
        schedule_path: Some(resstock_schedule_path()),
        weather_path: ochre_weather_path(),
        sim_config: SimulationConfig {
            start_time: fixture_start(),
            duration,
            time_res: Duration::minutes(1),
            reports: vec![
                hares_io::ReportSpec {
                    name: "frame".to_string(),
                    selection: hares_io::Selection::Verbosity(0),
                    frequency: hares_io::Frequency::default(),
                    destination: hares_io::Destination::Memory,
                },
                hares_io::ReportSpec {
                    name: "file".to_string(),
                    selection: hares_io::Selection::Verbosity(0),
                    frequency: hares_io::Frequency::default(),
                    destination: hares_io::Destination::Csv {
                        path: csv_path(output_dir, bldg_id),
                        // The harness reruns a scenario (the determinism
                        // sweep's thread counts) into the same directory;
                        // the old writer truncated the file in place, and
                        // the reruns keep that behaviour.
                        overwrite: true,
                    },
                },
            ],
            diagnostics: false,
            output_chunk_size: 1024,
            setpoint_deadband_c: None,
            master_seed: seed,
            civil_timezone: None,
            site_location: hares_io::SiteLocationOverride::default(),
            max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
            health_policy: hares_io::HealthPolicy::default(),
        },
        overrides: None,
        bldg_id,
        initialization_duration: None,
        defaults_path: Some(repo_defaults_path()),
        patches: None,
    }
}

pub fn fixture_hpxml_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/hpxml/ochre_samples/base.xml")
}

pub fn write_schedule_csv(path: &PathBuf) {
    // The columns the base fixture's event equipment and wet appliances run
    // on: a schedule file without an appliance's power column is a
    // construction error, and the washer and dishwasher draw their hot water
    // through their hot-water columns.
    let rows = [
        "Time,occupants,clothes_washer,clothes_dryer,dishwasher,cooking_range,hot_water_clothes_washer,hot_water_dishwasher,heating_setpoint,cooling_setpoint",
        "2021-01-01T00:00:00-07:00,1.0,0.10,0.10,0.10,0.10,0.10,0.10,20.0,25.0",
        "2021-01-01T00:15:00-07:00,1.0,0.20,0.20,0.20,0.20,0.20,0.20,20.5,25.5",
        "2021-01-01T00:30:00-07:00,1.0,0.15,0.15,0.15,0.15,0.15,0.15,21.0,26.0",
        "2021-01-01T00:45:00-07:00,1.0,0.12,0.12,0.12,0.12,0.12,0.12,20.0,25.0",
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
    output_dir: &Path,
    bldg_id: i64,
    schedule_path: PathBuf,
    weather_path: PathBuf,
    duration: Duration,
    seed: u64,
) -> DwellingConfig {
    DwellingConfig {
        hpxml_path: fixture_hpxml_path(),
        schedule_path: Some(schedule_path),
        weather_path,
        sim_config: SimulationConfig {
            start_time: fixture_start(),
            duration,
            time_res: Duration::minutes(1),
            reports: vec![
                hares_io::ReportSpec {
                    name: "frame".to_string(),
                    selection: hares_io::Selection::Verbosity(0),
                    frequency: hares_io::Frequency::default(),
                    destination: hares_io::Destination::Memory,
                },
                hares_io::ReportSpec {
                    name: "file".to_string(),
                    selection: hares_io::Selection::Verbosity(0),
                    frequency: hares_io::Frequency::default(),
                    destination: hares_io::Destination::Csv {
                        path: csv_path(output_dir, bldg_id),
                        // The harness reruns a scenario (the determinism
                        // sweep's thread counts) into the same directory;
                        // the old writer truncated the file in place, and
                        // the reruns keep that behaviour.
                        overwrite: true,
                    },
                },
            ],
            diagnostics: false,
            output_chunk_size: 1024,
            setpoint_deadband_c: None,
            master_seed: seed,
            civil_timezone: None,
            site_location: hares_io::SiteLocationOverride::default(),
            max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
            health_policy: hares_io::HealthPolicy::default(),
        },
        overrides: None,
        bldg_id,
        initialization_duration: None,
        defaults_path: Some(repo_defaults_path()),
        patches: None,
    }
}
