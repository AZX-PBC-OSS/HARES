//! The defaults-store policy: a defaults load failure (a missing
//! directory, an unreadable file, malformed data) is an error naming the
//! path. No dwelling reaches construction through a degraded, empty
//! store: the equipment's resolved defaults (ZIP sidecars, HVAC curves,
//! the envelope LUT) would silently fall to class tables and the run
//! would report success on different numbers. The defaults source's base
//! directory is required: no caller falls back to a working-directory
//! `defaults`, and the loader's own sub-tables (the envelope LUT, the
//! HVAC multispeed table, the water-heating CSV) are required with it.

use hares_core::DwellingSettings;
use hares_io::{DefaultsSource, InputSource, WeatherInput};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, FixedOffset};
use hares_core::dwelling::DwellingBlueprint;
use hares_core::{DwellingConfig, SimulationConfig};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn config_with_defaults(defaults: PathBuf) -> DwellingConfig {
    let fixture = project_root().join("tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh");
    DwellingConfig {
        hpxml: InputSource::Path(fixture.join("building.xml")),
        schedule: Some(InputSource::Path(fixture.join("schedule.csv"))),
        weather: WeatherInput::Path(fixture.join("weather.epw")),
        defaults: DefaultsSource::directory(defaults),
        settings: DwellingSettings::new(
            SimulationConfig {
                start_time: DateTime::<FixedOffset>::parse_from_rfc3339(
                    "2023-01-01T00:00:00-07:00",
                )
                .expect("start time parses"),
                duration: Duration::hours(1),
                time_res: Duration::seconds(900),
                reports: Vec::new(),
                diagnostics: false,
                metrics: false,
                output_chunk_size: 1024,
                setpoint_deadband_c: None,
                master_seed: 0,
                civil_timezone: None,
                site_location: hares_io::SiteLocationOverride::default(),
                max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                health_policy: hares_io::HealthPolicy::default(),
                first_hour_interpolation: Default::default(),
                weather_year: hares_io::WeatherYear::default(),
                warmup: hares_io::Warmup::Disabled,
            },
            None,
            1,
            None,
        ),
    }
}

/// The blueprint's construction failure on a defaults load failure: the
/// message names the load failure, and every pin below asserts the path
/// naming on top of it.
fn blueprint_defaults_error(config: DwellingConfig) -> String {
    match DwellingBlueprint::from_config(config) {
        Err(err) => {
            let message = err.to_string();
            assert!(
                message.contains("defaults load failed"),
                "the error must name the defaults load failure, got: {message}"
            );
            message
        }
        Ok(_) => panic!("a defaults load failure must fail the blueprint"),
    }
}

/// A defaults path that does not exist fails the blueprint naming the path.
/// The pre-fix code warned and continued on an empty store.
#[test]
fn a_missing_defaults_path_is_an_error_naming_the_path() {
    let missing = project_root().join("tests/scratch/no-such-defaults-dir");
    let message = blueprint_defaults_error(config_with_defaults(missing));
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
    let message = blueprint_defaults_error(config_with_defaults(empty.path().to_path_buf()));
    assert!(
        message.contains("zip_parameters.toml"),
        "the error must name the first mandatory file it missed, got: {message}"
    );
}

/// A defaults file that cannot parse fails the same way: malformed data
/// never degrades the dwelling to class tables behind a warning.
#[test]
fn a_malformed_defaults_file_is_an_error_naming_the_file() {
    let dir = tempfile::tempdir().expect("create temp dir");
    std::fs::write(
        dir.path().join("zip_parameters.toml"),
        "this is { not valid toml",
    )
    .expect("write malformed defaults");
    let message = blueprint_defaults_error(config_with_defaults(dir.path().to_path_buf()));
    assert!(
        message.contains("zip_parameters.toml"),
        "the error must name the malformed file, got: {message}"
    );
}

/// A defaults path that names a regular file is not a readable defaults
/// directory: the load fails naming the path instead of running on an
/// empty store.
#[test]
fn a_defaults_path_that_names_a_regular_file_is_an_error() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let file = dir.path().join("not-a-directory");
    std::fs::write(&file, "not a defaults directory").expect("write regular file");
    let message = blueprint_defaults_error(config_with_defaults(file));
    assert!(
        message.contains("not-a-directory"),
        "the error must name the path, got: {message}"
    );
}

/// The shipped defaults build the blueprint: the happy path every other
/// construction test exercises through the one helper.
#[test]
fn the_shipped_defaults_build_the_blueprint() {
    DwellingBlueprint::from_config(config_with_defaults(hares_core::shipped_defaults_dir()))
        .expect("the shipped defaults load and build");
}

// The dwelling-level acceptance tests: each builds the
// `tests/fixtures/parity/cz4a_ashp_hpwh` inputs from
// 2023-01-01T00:00:00-07:00 for 1 d at 3600 s with the stated defaults
// path. The failure tests fail at the defaults load, before any assembly,
// with the error naming the path.

use hares_core::{Dwelling, SimulationConfig as SimConfig};

fn dwelling_config(defaults: PathBuf) -> DwellingConfig {
    let fixture = project_root().join("tests/fixtures/parity/cz4a_ashp_hpwh");
    DwellingConfig {
        hpxml: InputSource::Path(fixture.join("building.xml")),
        schedule: Some(InputSource::Path(fixture.join("schedule.csv"))),
        weather: WeatherInput::Path(fixture.join("weather.epw")),
        defaults: DefaultsSource::directory(defaults),
        settings: DwellingSettings::new(
            SimConfig {
                start_time: DateTime::<FixedOffset>::parse_from_rfc3339(
                    "2023-01-01T00:00:00-07:00",
                )
                .expect("start time parses"),
                duration: Duration::hours(24),
                time_res: Duration::seconds(3600),
                reports: Vec::new(),
                diagnostics: false,
                metrics: false,
                output_chunk_size: 1024,
                setpoint_deadband_c: None,
                master_seed: 0,
                civil_timezone: None,
                site_location: hares_io::SiteLocationOverride::default(),
                max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                health_policy: hares_io::HealthPolicy::default(),
                first_hour_interpolation: Default::default(),
                weather_year: hares_io::WeatherYear::default(),
                warmup: hares_io::Warmup::Disabled,
            },
            None,
            1,
            None,
        ),
    }
}

fn dwelling_defaults_error(defaults: PathBuf) -> String {
    match Dwelling::from_config(dwelling_config(defaults)) {
        Err(err) => {
            let message = err.to_string();
            assert!(
                message.contains("defaults load failed"),
                "the error must name the defaults load failure, got: {message}"
            );
            message
        }
        Ok(_) => panic!("a defaults load failure must fail the dwelling's construction"),
    }
}

/// Every file of the shipped tree as a symbolic link under a temporary
/// directory, the stated file left out: the partial tree the acceptance
/// tests probe (the shape `02-measurement.md` M-1's adapter builds one).
fn partial_tree(left_out: &str) -> tempfile::TempDir {
    let temp = tempfile::tempdir().expect("create temp dir");
    let shipped = hares_core::shipped_defaults_dir();
    link_tree(&shipped, temp.path(), "", left_out);
    temp
}

fn link_tree(src: &Path, dst: &Path, prefix: &str, left_out: &str) {
    std::fs::create_dir_all(dst).expect("create tree directory");
    for entry in std::fs::read_dir(src).expect("read tree directory") {
        let entry = entry.expect("read tree entry");
        let name = entry.file_name();
        let target = dst.join(&name);
        let relative = if prefix.is_empty() {
            name.to_string_lossy().into_owned()
        } else {
            format!("{prefix}/{}", name.to_string_lossy())
        };
        if entry.file_type().expect("entry type").is_dir() {
            link_tree(&entry.path(), &target, &relative, left_out);
        } else if relative != left_out {
            std::os::unix::fs::symlink(entry.path(), &target).expect("link the tree file");
        }
    }
}

/// A path under a fresh temporary directory that does not exist fails the
/// dwelling's construction naming it.
#[test]
fn missing_defaults_directory_is_a_construction_error() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let missing = dir.path().join("no-such-defaults");
    let message = dwelling_defaults_error(missing);
    assert!(
        message.contains("no-such-defaults"),
        "the error must name the path, got: {message}"
    );
}

/// The defaults path naming a regular file (which no user, root included,
/// can list as a directory) fails the dwelling's construction naming it.
#[test]
fn unreadable_defaults_directory_is_a_construction_error() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let file = dir.path().join("not-a-directory");
    std::fs::write(&file, "not a defaults directory").expect("write regular file");
    let message = dwelling_defaults_error(file);
    assert!(
        message.contains("not-a-directory"),
        "the error must name the path, got: {message}"
    );
}

/// An empty defaults directory fails the dwelling's construction naming
/// it.
#[test]
fn empty_defaults_directory_is_a_construction_error() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let message = dwelling_defaults_error(dir.path().to_path_buf());
    assert!(
        message.contains("zip_parameters.toml"),
        "the error must name the first mandatory file it missed, got: {message}"
    );
}

/// A defaults tree whose envelope LUT cannot load fails the dwelling's
/// construction naming the file: the lumped fallback-R downgrade is gone,
/// the run never reaches it.
#[test]
fn envelope_lut_load_failure_is_a_construction_error() {
    let tree = partial_tree("envelope/Envelope Boundaries.csv");
    let message = dwelling_defaults_error(tree.path().to_path_buf());
    assert!(
        message.contains("Envelope Boundaries.csv"),
        "the error must name the envelope file, got: {message}"
    );
}

/// A defaults tree without the EV vehicle mapping fails the dwelling's
/// construction naming it, with `HARES_STRICT_DEFAULTS` unset: every
/// sub-table the loader reads is required, no environment variable gates
/// the strictness.
#[test]
fn missing_sub_table_is_a_construction_error() {
    // Safety: nextest runs one test per process; no concurrent load in
    // this process can observe the removal.
    unsafe { std::env::remove_var("HARES_STRICT_DEFAULTS") };
    let tree = partial_tree("ev/vehicle_mapping.csv");
    let message = dwelling_defaults_error(tree.path().to_path_buf());
    assert!(
        message.contains("vehicle_mapping.csv"),
        "the error must name the sub-table file, got: {message}"
    );
}

/// The shipped defaults build the dwelling: the happy path the failure
/// tests are measured against.
#[test]
fn the_shipped_defaults_build_the_dwelling() {
    Dwelling::from_config(dwelling_config(hares_core::shipped_defaults_dir()))
        .expect("the shipped defaults build the dwelling");
}
