//! The `DwellingConfig.schedule_path` contract.
//!
//! `None` requests the default generated schedule. A set path must be
//! readable: construction fails naming the path when it is not, so a
//! mistyped path can never silently run on a generated schedule's different
//! occupancy and loads. A generated schedule needs the default schedule
//! profiles: no defaults directory, an unreadable profile file, a missing
//! profile, or a malformed profile row is a construction error naming what
//! is wrong.

use hares_core::DwellingSettings;
use hares_io::{DefaultsSource, InputSource, WeatherInput};
use std::path::PathBuf;

use chrono::{Duration, TimeZone};
use hares_core::dwelling::DwellingBlueprint;
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

#[path = "../../../tests/support/denver_offset.rs"]
mod denver_offset;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The default generated schedule for `base.xml` over the Denver TMY3
/// weather with the repo defaults: a 24 h run starting 2023-01-15 00:00
/// UTC-7 at 900 s steps. The digest moved four times. Twice before the
/// calendar-anchor contract: once when the constant 1.0 `occupants`
/// column in front of the `Occupancy` profile column was deleted and the
/// last column survived the resample; once when the two constant setpoint
/// columns, which overrode the HVAC's own hourly setpoints, were dropped.
/// The third move is the anchor itself: the rows are addressed by
/// calendar date (row 0 = Jan 1, the CSV path's convention), so the
/// table spans Jan 1 of the start's year to the run's end instead of
/// starting at the run's start date; a Jan 15 start's 24 h table now
/// carries the Jan 1 to Jan 15 rows the addressing reads. On every
/// column the index names the schedule is bitwise the earlier one's.
/// The fourth move is the behavioral diversity layer: the generated
/// family is diversified under the home's scenario-mixed per-dwelling
/// key (this config's bldg_id 1, master seed 0, no scenario), so the
/// pinned digest is the diversified family's (the Off mode's digest is
/// the third state's, and `diversity_off_generates_the_shared_profile`
/// pins that escape hatch against the undiversified generator).
/// The generation path must stay bitwise-equal to it.
const GENERATED_SCHEDULE_DIGEST: &str =
    "41c43b36412ca27a2a419ec7e2bdfd713ef7c9d8757668d08b43e0a946b91bcf";

fn generated_schedule_config(schedule_path: Option<PathBuf>) -> DwellingConfig {
    let start_time = denver_offset::denver_offset()
        .with_ymd_and_hms(2023, 1, 15, 0, 0, 0)
        .single()
        .expect("valid start time");
    DwellingConfig {
        hpxml: InputSource::Path(
            project_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"),
        ),
        schedule: schedule_path.map(InputSource::Path),
        weather: WeatherInput::Path(
            project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        ),
        defaults: DefaultsSource::directory(project_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(
            SimulationConfig {
                scenario_seed: None,
                schedule_diversity: Default::default(),
                start_time,
                duration: Duration::hours(24),
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
/// called directly on the same inputs DIVERSIFIED under the home's own
/// key (the build's transform, `hares_io::schedule_diversity`), and to
/// the digest pinned in `GENERATED_SCHEDULE_DIGEST` (see that constant's
/// comment for its moves). The undiversified generator is the Off mode's
/// exact output; the escape-hatch pin is
/// `diversity_off_generates_the_shared_profile`.
#[test]
fn no_schedule_generates_the_default_schedule() {
    let config = generated_schedule_config(None);
    let blueprint = DwellingBlueprint::from_config(config.clone())
        .expect("a None schedule requests the generated schedule");

    let profiles = hares_io::load_default_profiles(&config.defaults.base)
        .expect("the shipped defaults CSV loads");
    let key = hares_types::rng::dwelling_seed_scenario(
        config.settings.sim_config.master_seed,
        config.settings.sim_config.scenario_seed,
        config.settings.bldg_id,
    );
    let draws = hares_types::diversity::ScheduleDiversityDraws::draw(key);
    let diversified = hares_io::schedule_diversity::diversify_profiles(&profiles, &draws);
    let generated = hares_io::hpxml_schedule::generate_default_schedule(
        config.settings.sim_config.start_time,
        config.settings.sim_config.duration,
        config.settings.sim_config.time_res,
        &diversified,
    )
    .expect("the shipped profiles generate the schedule")
    .resample(config.settings.sim_config.time_res.num_seconds() as u32)
    .expect("the generated schedule resamples");

    // Every column the index names is bitwise the generator's column: the
    // per-name values are today's, whatever index shift the column set
    // change caused.
    let built_schedule = blueprint.resampled_schedule();
    assert_eq!(
        built_schedule.column_names, generated.column_names,
        "the dwelling's schedule must carry the generator's column names"
    );
    for name in generated.column_index.keys() {
        let built_idx = built_schedule.column_index[name];
        let direct_idx = generated.column_index[name];
        assert_eq!(
            built_schedule.columns[built_idx], generated.columns[direct_idx],
            "column '{name}' must be bitwise the generator's values"
        );
    }

    let built_digest = schedule_digest(&built_schedule);
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

/// The escape hatch's own pin: with `schedule_diversity = "off"`, the
/// generated schedule is bitwise the UNDIVERSIFIED generator's output
/// (the pre-diversity family, the digest's third state).
#[test]
fn diversity_off_generates_the_shared_profile() {
    let mut config = generated_schedule_config(None);
    config.settings.sim_config.schedule_diversity = hares_io::ScheduleDiversity::Off;
    let blueprint = DwellingBlueprint::from_config(config.clone())
        .expect("a None schedule requests the generated schedule");

    let profiles = hares_io::load_default_profiles(&config.defaults.base)
        .expect("the shipped defaults CSV loads");
    let generated = hares_io::hpxml_schedule::generate_default_schedule(
        config.settings.sim_config.start_time,
        config.settings.sim_config.duration,
        config.settings.sim_config.time_res,
        &profiles,
    )
    .expect("the shipped profiles generate the schedule")
    .resample(config.settings.sim_config.time_res.num_seconds() as u32)
    .expect("the generated schedule resamples");

    let built_digest = schedule_digest(blueprint.resampled_schedule().as_ref());
    assert_eq!(
        built_digest,
        schedule_digest(&generated),
        "the Off mode must run exactly the undiversified generator's schedule"
    );
}

/// A defaults directory without `Default Schedule Parameters.csv` fails a
/// generated schedule's construction naming the file.
#[test]
fn generated_schedule_without_defaults_file_is_an_error() {
    let temp = copy_shipped_defaults_csv();
    let mut config = generated_schedule_config(None);
    config.defaults.base = temp.path().to_path_buf();
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
    config.defaults.base = temp.path().to_path_buf();
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
    config.defaults.base = temp.path().to_path_buf();
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

/// A schedule file that omits a column an equipment maps to builds on the
/// defaults directory the config names: the default profiles come only
/// from the configured tree, never from a working-directory guess. The
/// BEopt example schedule has no lighting or plug-load columns. (The
/// defaults directory is required, so there is no "no defaults directory"
/// shape left to probe; a tree without `Default Schedule Parameters.csv`
/// fails a generated schedule's construction naming the file, the
/// `generated_schedule_without_defaults_file_is_an_error` pin.)
#[test]
fn schedule_file_missing_a_column_takes_the_default_profiles() {
    let config = generated_schedule_config(Some(
        project_root().join("data/examples/BEopt_example_schedule.csv"),
    ));
    if let Err(err) = Dwelling::from_config(config) {
        panic!("the config with the shipped defaults directory must build, got: {err}");
    }
}

/// Copy the shipped defaults CSV into a temporary directory: the only file
/// the schedule generator reads.
fn copy_shipped_defaults_csv() -> TempDir {
    let temp = TempDir::new().expect("create temp dir");
    std::fs::copy(
        project_root().join("python/hares/defaults/Default Schedule Parameters.csv"),
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
