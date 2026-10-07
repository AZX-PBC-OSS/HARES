//! The `DwellingConfig.schedule_path` contract.
//!
//! `None` requests the default generated schedule. A set path must be
//! readable: construction fails naming the path when it is not, so a
//! mistyped path can never silently run on a generated schedule's different
//! occupancy and loads. A generated schedule needs the default schedule
//! profiles: no defaults directory, an unreadable profile file, a missing
//! profile, or a malformed profile row is a construction error naming what
//! is wrong.

use std::path::PathBuf;

use chrono::{Duration, TimeZone};
use hares_core::dwelling::DwellingBlueprint;
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
use hares_io::OutputFormat;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

#[path = "../../../tests/support/denver_offset.rs"]
mod denver_offset;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The default generated schedule for `base.xml` over the Denver TMY3
/// weather with the repo defaults: a 24 h run starting 2023-01-15 00:00
/// UTC-7 at 900 s steps. The digest moved twice. Once when the constant
/// 1.0 `occupants` column in front of the `Occupancy` profile column was
/// deleted and the last column survived the resample; once when the two
/// constant setpoint columns, which overrode the HVAC's own hourly
/// setpoints, were dropped. On every column the index names the schedule
/// is bitwise the earlier one's. The generation path must stay
/// bitwise-equal to it.
const GENERATED_SCHEDULE_DIGEST: &str =
    "a30b9593fcc0fa406e2ce833b43db6b24cd7f5b7a4489a12cf5d4015f10d84ca";

fn generated_schedule_config(schedule_path: Option<PathBuf>) -> DwellingConfig {
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
        bldg_id: 1,
        initialization_duration: None,
        resample_overrides: None,
        patches: None,
    }
}

/// Bit digest of a resolved schedule: timestamps, column names, and the bit
/// pattern of every value, so any change in content or ordering moves it.
fn schedule_digest(schedule: &hares_io::ScheduleTimeSeries) -> String {
    let mut hasher = Sha256::new();
    for ts in &schedule.timestamps {
        hasher.update(ts.timestamp().to_le_bytes());
    }
    for name in &schedule.column_names {
        hasher.update((name.len() as u32).to_le_bytes());
        hasher.update(name.as_bytes());
    }
    for column in &schedule.columns {
        for value in column {
            hasher.update(value.to_bits().to_le_bytes());
        }
    }
    hasher.update(schedule.source_step_secs.to_le_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A schedule path that does not exist fails construction, for the dwelling
/// and the blueprint constructors alike, and the error names the path.
#[test]
fn nonexistent_schedule_path_is_an_error() {
    let missing = project_root().join("data/examples/no-such-schedule-path.csv");
    assert!(
        !missing.exists(),
        "the probe path must not exist: {missing:?}"
    );

    let dwelling_err = match Dwelling::from_config(generated_schedule_config(Some(missing.clone())))
    {
        Err(err) => err,
        Ok(_) => panic!("a mistyped schedule path must fail dwelling construction"),
    };
    assert!(
        dwelling_err
            .to_string()
            .contains(&missing.display().to_string()),
        "the dwelling error must name the mistyped path, got: {dwelling_err}"
    );

    let blueprint_err =
        match DwellingBlueprint::from_config(generated_schedule_config(Some(missing.clone()))) {
            Err(err) => err,
            Ok(_) => panic!("a mistyped schedule path must fail blueprint construction"),
        };
    assert!(
        blueprint_err
            .to_string()
            .contains(&missing.display().to_string()),
        "the blueprint error must name the mistyped path, got: {blueprint_err}"
    );
}

/// `None` generates the default schedule: bitwise-equal to the generator
/// called directly on the same inputs, and to the digest pinned in
/// `GENERATED_SCHEDULE_DIGEST` (see that constant's comment for its moves).
#[test]
fn no_schedule_generates_the_default_schedule() {
    let config = generated_schedule_config(None);
    let blueprint = DwellingBlueprint::from_config(config.clone())
        .expect("a None schedule requests the generated schedule");

    let profiles = hares_io::load_default_profiles(
        config
            .defaults_path
            .as_deref()
            .expect("the config carries a defaults directory"),
    )
    .expect("the shipped defaults CSV loads");
    let generated = hares_io::hpxml_schedule::generate_default_schedule(
        config.sim_config.start_time,
        config.sim_config.duration,
        config.sim_config.time_res,
        &profiles,
    )
    .expect("the shipped profiles generate the schedule")
    .resample(config.sim_config.time_res.num_seconds() as u32)
    .expect("the generated schedule resamples");

    // Every column the index names is bitwise the generator's column: the
    // per-name values are today's, whatever index shift the column set
    // change caused.
    assert_eq!(
        blueprint.schedule.column_names, generated.column_names,
        "the dwelling's schedule must carry the generator's column names"
    );
    for name in generated.column_index.keys() {
        let built_idx = blueprint.schedule.column_index[name];
        let direct_idx = generated.column_index[name];
        assert_eq!(
            blueprint.schedule.columns[built_idx], generated.columns[direct_idx],
            "column '{name}' must be bitwise the generator's values"
        );
    }

    let built_digest = schedule_digest(&blueprint.schedule);
    assert_eq!(
        built_digest,
        schedule_digest(&generated),
        "None must run exactly the generator's schedule"
    );
    assert_eq!(
        built_digest, GENERATED_SCHEDULE_DIGEST,
        "the generated schedule must stay bitwise-equal to the pinned default schedule"
    );
}

/// A defaults directory without `Default Schedule Parameters.csv` fails a
/// generated schedule's construction naming the file.
#[test]
fn generated_schedule_without_defaults_file_is_an_error() {
    let temp = copy_shipped_defaults_csv();
    let mut config = generated_schedule_config(None);
    config.defaults_path = Some(temp.path().to_path_buf());
    std::fs::remove_file(temp.path().join("Default Schedule Parameters.csv"))
        .expect("remove the defaults CSV from the copy");

    let err = match Dwelling::from_config(config) {
        Err(err) => err,
        Ok(_) => panic!("a generated schedule without the defaults CSV must fail construction"),
    };
    assert!(
        err.to_string().contains("Default Schedule Parameters.csv"),
        "the error must name the missing file, got: {err}"
    );
}

/// A defaults CSV without the `Indoor Lighting` rows fails construction
/// naming the profile: every generated column needs its profile.
#[test]
fn missing_default_profile_is_an_error() {
    let temp = copy_shipped_defaults_csv();
    let csv_path = temp.path().join("Default Schedule Parameters.csv");
    let kept: String = std::fs::read_to_string(&csv_path)
        .expect("read the copied CSV")
        .lines()
        .filter(|line| !line.contains(",Indoor Lighting,"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&csv_path, kept).expect("write the CSV without Indoor Lighting");

    let mut config = generated_schedule_config(None);
    config.defaults_path = Some(temp.path().to_path_buf());
    let err = match Dwelling::from_config(config) {
        Err(err) => err,
        Ok(_) => panic!("a generated schedule with a missing profile must fail construction"),
    };
    assert!(
        err.to_string().contains("Indoor Lighting"),
        "the error must name the missing profile, got: {err}"
    );
}

/// A `weekday_fractions` row cut to 23 values fails construction naming the
/// line: a malformed profile row is an error, not a skipped row.
#[test]
fn malformed_default_profile_row_is_an_error() {
    let temp = copy_shipped_defaults_csv();
    let csv_path = temp.path().join("Default Schedule Parameters.csv");
    let truncated = std::fs::read_to_string(&csv_path)
        .expect("read the copied CSV")
        .lines()
        .map(|line| {
            if line.starts_with("occupants,WeekdayScheduleFractions,") {
                cut_last_value(line)
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&csv_path, truncated).expect("write the CSV with the short row");

    let mut config = generated_schedule_config(None);
    config.defaults_path = Some(temp.path().to_path_buf());
    let err = match Dwelling::from_config(config) {
        Err(err) => err,
        Ok(_) => panic!("a generated schedule with a malformed profile row must fail construction"),
    };
    let expected_line = line_number_of(&csv_path, "occupants,WeekdayScheduleFractions,");
    assert!(
        err.to_string().contains(&format!("line {expected_line}")),
        "the error must name the malformed row's line ({expected_line}), got: {err}"
    );
}

/// No defaults directory and no schedule file fails construction naming the
/// missing directory and the file the generator needs.
#[test]
fn generated_schedule_requires_a_defaults_directory() {
    let mut config = generated_schedule_config(None);
    config.defaults_path = None;

    let err = match Dwelling::from_config(config) {
        Err(err) => err,
        Ok(_) => panic!("a generated schedule without a defaults directory must fail construction"),
    };
    let message = err.to_string();
    assert!(
        message.contains("no defaults directory"),
        "the error must name the missing directory, got: {message}"
    );
    assert!(
        message.contains("Default Schedule Parameters.csv"),
        "the error must name the file the generator needs, got: {message}"
    );
}

/// A schedule file that omits a column an equipment maps to, with no
/// defaults directory, fails construction naming the equipment and the
/// `defaults_path` setting: the default profiles come only from the
/// directory the config names, never from a working-directory guess. The
/// BEopt example schedule has no lighting or plug-load columns.
///
/// With the defaults policy (a load failure is an error), a config
/// with no defaults directory fails first at the store load, naming the
/// path: no dwelling reaches schedule injection through a missing store.
#[test]
fn schedule_file_missing_a_column_needs_a_defaults_directory() {
    let mut config = generated_schedule_config(Some(
        project_root().join("data/examples/BEopt_example_schedule.csv"),
    ));
    config.defaults_path = None;

    let err = match Dwelling::from_config(config.clone()) {
        Err(err) => err,
        Ok(_) => {
            panic!("a config with no defaults directory must fail construction at the store load")
        }
    };
    let message = err.to_string();
    assert!(
        message.contains("defaults load failed"),
        "the error must name the defaults load failure, got: {message}"
    );
    assert!(
        message.contains("defaults"),
        "the error must name the path it tried, got: {message}"
    );

    config.defaults_path = Some(project_root().join("defaults"));
    if let Err(err) = Dwelling::from_config(config) {
        panic!("the same config with the shipped defaults directory must build, got: {err}");
    }
}

/// Copy the shipped defaults CSV into a temporary directory: the only file
/// the schedule generator reads.
fn copy_shipped_defaults_csv() -> TempDir {
    let temp = TempDir::new().expect("create temp dir");
    std::fs::copy(
        project_root().join("defaults/Default Schedule Parameters.csv"),
        temp.path().join("Default Schedule Parameters.csv"),
    )
    .expect("copy the shipped defaults CSV");
    temp
}

/// The 1-based line number of the first CSV line starting with `prefix`.
fn line_number_of(csv_path: &std::path::Path, prefix: &str) -> usize {
    std::fs::read_to_string(csv_path)
        .expect("read the copied CSV")
        .lines()
        .position(|line| line.starts_with(prefix))
        .expect("the CSV carries the prefixed line")
        + 1
}

/// Drop the last value of a quoted CSV Values field, leaving 23. The Values
/// field's closing quote is the line's last `"`; the cut ends there, so the
/// trailing `Data Source` cell after it is discarded too. The remaining row
/// still carries enough fields and values for the loader's line-named error.
fn cut_last_value(line: &str) -> String {
    let closing = line
        .rfind('"')
        .expect("test bug: the row has no quoted Values field");
    let last_sep = line[..closing]
        .rfind(", ")
        .expect("test bug: the Values field has a single value");
    format!("{}\"", &line[..last_sep])
}
