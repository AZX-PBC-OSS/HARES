//! The `DwellingConfig.schedule_path` contract.
//!
//! `None` requests a schedule generated from the HPXML. A set path must be
//! readable: construction fails naming the path when it is not, so a
//! mistyped path can never silently run on a generated schedule's different
//! occupancy and loads. A generated schedule needs the default schedule
//! profiles: no defaults directory, an unreadable profile file, a missing
//! profile, or a malformed profile row is a construction error naming what
//! is wrong.

use std::path::PathBuf;

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::dwelling::DwellingBlueprint;
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
use hares_io::OutputFormat;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The schedule the generated path produced when it still pushed a constant
/// 1.0 `occupants` column in front of the `Occupancy` profile column and
/// dropped the last column on resample, captured for `base.xml` over the
/// Denver TMY3 weather with the repo defaults: a 24 h run starting
/// 2023-01-15 00:00 UTC-7 at 900 s steps. On every column the index names
/// the schedule is bitwise today's; the constant column is gone and the
/// last column survives the resample, so the digest changed once here.
const GENERATED_SCHEDULE_DIGEST: &str =
    "782e4dabd57fcb5e92e537c5abefba9b8efe9c80957297c40e162e5f8fa66bc6";

fn generated_schedule_config(schedule_path: Option<PathBuf>) -> DwellingConfig {
    let tz_offset = FixedOffset::west_opt(7 * 3600).expect("UTC-7 offset is valid");
    let start_time = tz_offset
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

/// `None` generates the schedule from the HPXML: bitwise-equal to the
/// generator called directly on the same parsed inputs, and bitwise-equal to
/// the schedule the pre-change behavior produced for a missing path.
#[test]
fn no_schedule_generates_from_hpxml() {
    let config = generated_schedule_config(None);
    let blueprint = DwellingBlueprint::from_config(config.clone())
        .expect("a None schedule requests the generated schedule");

    let building = hares_io::parse_hpxml(&config.hpxml_path).expect("the HPXML parses");
    let profiles = hares_io::load_default_profiles(
        config
            .defaults_path
            .as_deref()
            .expect("the config carries a defaults directory"),
    )
    .expect("the shipped defaults CSV loads");
    let generated = hares_io::hpxml_schedule::generate_schedule_from_hpxml(
        &building,
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
        "the generated schedule must stay bitwise-equal to the pinned schedule"
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
/// field's closing quote is the line's last `"`; the trailing `Data Source`
/// cell after it must survive the cut.
fn cut_last_value(line: &str) -> String {
    let closing = line
        .rfind('"')
        .expect("test bug: the row has no quoted Values field");
    let last_sep = line[..closing]
        .rfind(", ")
        .expect("test bug: the Values field has a single value");
    format!("{}\"", &line[..last_sep])
}
