//! The defaults-store policy: a defaults load failure :  a
//! missing directory, an unreadable file, malformed data :  is an error
//! naming the path. No dwelling reaches construction through a degraded,
//! empty store: the equipment's resolved defaults (ZIP sidecars, HVAC
//! curves, the envelope LUT) would silently fall to class tables and the
//! run would report success on different numbers.

use std::path::PathBuf;

use chrono::{DateTime, Duration, FixedOffset};
use hares_core::dwelling::DwellingBlueprint;
use hares_core::{DwellingConfig, SimulationConfig};
use hares_io::OutputFormat;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn config_with_defaults(defaults: PathBuf) -> DwellingConfig {
    let fixture = project_root().join("tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh");
    DwellingConfig {
        hpxml_path: fixture.join("building.xml"),
        schedule_path: Some(fixture.join("schedule.csv")),
        weather_path: fixture.join("weather.epw"),
        defaults_path: Some(defaults),
        sim_config: SimulationConfig {
            start_time: DateTime::<FixedOffset>::parse_from_rfc3339("2023-01-01T00:00:00-07:00")
                .expect("start time parses"),
            duration: Duration::hours(1),
            time_res: Duration::seconds(900),
            output_verbosity: 0,
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
        initialization_duration: None,
        resample_overrides: None,
        patches: None,
    }
}

/// A defaults path that does not exist fails the blueprint naming the path.
/// The pre-fix code warned and continued on an empty store.
#[test]
fn a_missing_defaults_path_is_an_error_naming_the_path() {
    let missing = project_root().join("tests/scratch/no-such-defaults-dir");
    let err = match DwellingBlueprint::from_config(config_with_defaults(missing.clone())) {
        Err(err) => err,
        Ok(_) => panic!("a missing defaults path must fail the blueprint"),
    };
    let message = err.to_string();
    assert!(
        message.contains("defaults load failed"),
        "the error must name the defaults load failure, got: {message}"
    );
    assert!(
        message.contains("no-such-defaults-dir"),
        "the error must name the path, got: {message}"
    );
}

/// A defaults directory whose mandatory files are absent fails the same
/// way: the store, not the profile loader, is the boundary that refuses.
#[test]
fn a_defaults_directory_without_its_mandatory_files_is_an_error() {
    let empty = tempfile::tempdir().expect("create temp dir");
    let err = match DwellingBlueprint::from_config(config_with_defaults(empty.path().to_path_buf()))
    {
        Err(err) => err,
        Ok(_) => panic!("an empty defaults directory must fail the blueprint"),
    };
    let message = err.to_string();
    assert!(
        message.contains("defaults load failed"),
        "the error must name the defaults load failure, got: {message}"
    );
}

/// The shipped defaults build the blueprint: the happy path every other
/// construction test exercises through the one helper.
#[test]
fn the_shipped_defaults_build_the_blueprint() {
    DwellingBlueprint::from_config(config_with_defaults(hares_core::shipped_defaults_dir()))
        .expect("the shipped defaults load and build");
}
