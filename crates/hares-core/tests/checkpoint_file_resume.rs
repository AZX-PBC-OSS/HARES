//! A run resumed from a checkpoint file continues bitwise as the run that
//! wrote it: the checkpoint's floats round-trip through the file exactly.

use std::path::PathBuf;

use chrono::{DateTime, Duration};
use hares_core::{Dwelling, DwellingCheckpoint, DwellingConfig, SimulationConfig, StepResult};
use hares_io::OutputFormat;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

struct Case {
    fixture: &'static str,
    start_time: &'static str,
    time_res_s: i64,
    checkpoint_step: usize,
}

fn dwelling(case: &Case) -> Dwelling {
    let fixture = project_root()
        .join("tests/fixtures/parity")
        .join(case.fixture);
    Dwelling::from_config(DwellingConfig {
        hpxml_path: fixture.join("building.xml"),
        schedule_path: Some(fixture.join("schedule.csv")),
        weather_path: fixture.join("weather.epw"),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: SimulationConfig {
            start_time: DateTime::parse_from_rfc3339(case.start_time).expect("start time"),
            duration: Duration::days(7),
            time_res: Duration::seconds(case.time_res_s),
            output_verbosity: 2,
            write_output: false,
            output_path: None,
            output_format: OutputFormat::Csv,
            output_chunk_size: 1024,
            setpoint_deadband_c: None,
            master_seed: 0,
            civil_timezone: None,
            site_location: hares_io::SiteLocationOverride::default(),
            retain_batches: false,
            rotation: hares_io::RotationPolicy::None,
            max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
        },
        overrides: None,
        bldg_id: 1,
        initialization_duration: Some(std::time::Duration::from_secs(7 * 86_400)),
        resample_overrides: None,
        patches: None,
    })
    .unwrap_or_else(|err| panic!("assemble {}: {err}", case.fixture))
}

fn steps(dwelling: &mut Dwelling, count: usize) -> Vec<StepResult> {
    (0..count).map(|_| dwelling.step().expect("step")).collect()
}

fn assert_file_resume_is_continuous(case: &Case) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("checkpoint.json");

    let mut continuous = dwelling(case);
    steps(&mut continuous, case.checkpoint_step);
    continuous
        .save_checkpoint()
        .expect("checkpoint")
        .save(&path)
        .expect("write the checkpoint file");
    let after_continuous = steps(&mut continuous, 48);

    let mut resumed = dwelling(case);
    resumed
        .load_checkpoint(DwellingCheckpoint::load(&path).expect("read the checkpoint file"))
        .expect("restore");
    let after_resume = steps(&mut resumed, 48);

    assert!(
        after_resume == after_continuous,
        "{}: the run resumed from the file diverges from the continuous run at step {:?}",
        case.fixture,
        after_resume
            .iter()
            .zip(&after_continuous)
            .position(|(r, c)| r != c)
    );
}

#[test]
fn full_year_hourly_resumes_from_a_checkpoint_file_bitwise() {
    assert_file_resume_is_continuous(&Case {
        fixture: "cz2a_gas_furnace_ac_res_wh",
        start_time: "2023-01-01T00:00:00-07:00",
        time_res_s: 3600,
        checkpoint_step: 29,
    });
}

#[test]
fn weather_wrap_resumes_from_a_checkpoint_file_bitwise() {
    assert_file_resume_is_continuous(&Case {
        fixture: "cz4a_ashp_hpwh",
        start_time: "2023-12-30T00:00:00-07:00",
        time_res_s: 900,
        checkpoint_step: 191,
    });
}

#[test]
fn leap_year_february_resumes_from_a_checkpoint_file_bitwise() {
    assert_file_resume_is_continuous(&Case {
        fixture: "cz4a_ashp_hpwh",
        start_time: "2024-02-27T00:00:00-07:00",
        time_res_s: 900,
        checkpoint_step: 203,
    });
}
