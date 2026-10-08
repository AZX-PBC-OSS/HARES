//! The `SteppableFleet::from_configs` failure contract.
//!
//! When every config fails construction, the returned
//! `FleetError::AllSteppableDwellingsFailed` must carry each dwelling's
//! cause in its `causes` field, prefixed by the dwelling's `bldg_id`, so a
//! fleet error alone is diagnosable. When one dwelling builds, the error is
//! not returned: the surviving `build_errors` vec carries the broken
//! dwelling's cause and the healthy dwelling joins the fleet.
//!
//! Both cases break construction through the missing-schedule pattern: a
//! config whose `schedule_path` names a nonexistent file. A set schedule
//! path that cannot be read is a construction error naming the path, so the
//! per-dwelling cause can be pinned to it.

use std::path::PathBuf;

use chrono::{Duration, TimeZone};
use hares_core::{DwellingConfig, SimulationConfig};
use hares_fleet::SteppableFleet;
use hares_fleet::fleet::FleetError;
use hares_io::OutputFormat;

#[path = "../../../tests/support/denver_offset.rs"]
mod denver_offset;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A config over the committed fixtures whose only variable is the schedule
/// source. `Some(path)` names the schedule file to read; `None` requests the
/// generated schedule from the shipped defaults directory. The HPXML and
/// weather stay valid either way, so a config that fails does so at the
/// schedule read and the error names its path.
fn config_with_schedule_source(bldg_id: i64, schedule_path: Option<PathBuf>) -> DwellingConfig {
    let start_time = denver_offset::denver_offset()
        .with_ymd_and_hms(2023, 1, 15, 0, 0, 0)
        .single()
        .expect("valid start time");
    DwellingConfig {
        hpxml_path: project_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"),
        schedule_path,
        weather_path: project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: SimulationConfig {
            start_time,
            duration: Duration::hours(24),
            time_res: Duration::seconds(900),
            output_verbosity: 0,
            output_path: None,
            write_output: false,
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
        bldg_id,
        initialization_duration: None,
        resample_overrides: None,
        patches: None,
    }
}

/// When both dwellings fail construction, the error's display names the
/// failure count and both per-dwelling causes, and the variant's `causes`
/// field carries each dwelling's cause naming its own missing schedule path.
#[test]
fn all_failed_error_carries_each_dwelling_cause() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let missing_1 = dir.path().join("no-such-schedule-1.csv");
    let missing_2 = dir.path().join("no-such-schedule-2.csv");

    let configs = vec![
        config_with_schedule_source(1, Some(missing_1.clone())),
        config_with_schedule_source(2, Some(missing_2.clone())),
    ];

    let err = match SteppableFleet::from_configs(configs, 0) {
        Err(err @ FleetError::AllSteppableDwellingsFailed { .. }) => err,
        Err(other) => panic!("expected AllSteppableDwellingsFailed, got {other:?}"),
        Ok(_) => panic!("a fleet whose every dwelling fails construction must return an error"),
    };

    let message = err.to_string();
    assert!(
        message.contains("all dwellings failed to initialize (2 failure(s))"),
        "the error must name the failure count, got: {message}"
    );
    assert!(
        message.contains("dwelling 1:"),
        "the error must carry dwelling 1's cause, got: {message}"
    );
    assert!(
        message.contains("dwelling 2:"),
        "the error must carry dwelling 2's cause, got: {message}"
    );

    match err {
        FleetError::AllSteppableDwellingsFailed { count, causes } => {
            assert_eq!(count, 2, "one cause per failed dwelling");
            assert_eq!(causes.len(), 2, "the causes vec mirrors the count");
            assert!(
                causes[0].starts_with("dwelling 1:")
                    && causes[0].contains(&missing_1.display().to_string()),
                "dwelling 1's cause must name its missing schedule path, got: {}",
                causes[0]
            );
            assert!(
                causes[1].starts_with("dwelling 2:")
                    && causes[1].contains(&missing_2.display().to_string()),
                "dwelling 2's cause must name its missing schedule path, got: {}",
                causes[1]
            );
            assert!(
                message.contains(&causes[0]) && message.contains(&causes[1]),
                "the display must include both causes, got: {message}"
            );
        }
        other => panic!("expected AllSteppableDwellingsFailed, got {other:?}"),
    }
}

/// When one dwelling builds, the error is not returned: the fleet holds the
/// healthy dwelling and the `build_errors` vec carries the broken dwelling's
/// cause naming its missing schedule path.
#[test]
fn partially_failed_fleet_carries_the_broken_dwelling_cause_in_build_errors() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let missing = dir.path().join("no-such-schedule.csv");

    let configs = vec![
        config_with_schedule_source(1, None),
        config_with_schedule_source(2, Some(missing.clone())),
    ];

    let (fleet, build_errors) = match SteppableFleet::from_configs(configs, 0) {
        Ok((fleet, build_errors)) => (fleet, build_errors),
        Err(err) => {
            panic!("a fleet with one healthy dwelling must not return an error, got: {err}")
        }
    };

    assert_eq!(build_errors.len(), 1, "only the broken dwelling failed");
    assert_eq!(build_errors[0].bldg_id, 2);
    assert!(
        build_errors[0]
            .message
            .contains(&missing.display().to_string()),
        "the build error must name the broken dwelling's schedule path, got: {}",
        build_errors[0].message
    );
    assert_eq!(fleet.len(), 1, "the healthy dwelling joined the fleet");
    assert_eq!(
        fleet.bldg_id(0),
        Some(1),
        "the fleet holds the healthy dwelling"
    );
}
