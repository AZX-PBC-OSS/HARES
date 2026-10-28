//! The input records a run's provenance carries: every file input the
//! parse read is recorded with the SHA-256 of the bytes read, the path it
//! was read from kept as metadata beside the digest, and a schedule
//! generated from the HPXML carrying no record of its own. Two runs on the
//! same inputs record equal records; changing one input moves exactly that
//! input's record; the same content at a different path hashes the same.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration};
use hares_core::{Dwelling, DwellingConfig, DwellingSettings, SimulationConfig};
use tempfile::TempDir;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cz4a_paths() -> (PathBuf, PathBuf, PathBuf) {
    let fixture = project_root().join("tests/fixtures/parity/cz4a_ashp_hpwh");
    (
        fixture.join("building.xml"),
        fixture.join("schedule.csv"),
        fixture.join("weather.epw"),
    )
}

fn config_for(
    hpxml: PathBuf,
    schedule: Option<PathBuf>,
    weather: PathBuf,
    defaults: PathBuf,
) -> DwellingConfig {
    DwellingConfig {
        hpxml: hares_io::InputSource::Path(hpxml),
        schedule: schedule.map(hares_io::InputSource::Path),
        weather: hares_io::WeatherInput::Path(weather),
        defaults: hares_io::DefaultsSource::directory(defaults),
        settings: DwellingSettings::new(
            SimulationConfig {
                scenario_seed: None,
                schedule_diversity: Default::default(),
                start_time: DateTime::parse_from_rfc3339("2023-01-01T00:00:00-07:00")
                    .expect("valid start time"),
                duration: Duration::hours(1),
                time_res: Duration::seconds(60),
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

fn run(config: DwellingConfig) -> hares_io::InputRecords {
    let result = Dwelling::from_config(config)
        .expect("the dwelling builds")
        .simulate()
        .expect("the run completes");
    result
        .provenance
        .inputs
        .clone()
        .expect("a run built from files records its inputs")
}

/// Copy `src` into `dst`, skipping the EV cycle CSVs: the store reads the
/// EV directory's vehicle mapping, the dwelling reads the cycles only for
/// EV equipment, and the cycles are the tree's 135 MB.
fn copy_defaults_light(src: &Path, dst: &Path) {
    fn walk(src: &Path, dst: &Path) {
        for entry in fs::read_dir(src).expect("readable defaults tree") {
            let entry = entry.expect("directory entry");
            let name = entry.file_name();
            let name = name.to_string_lossy().into_owned();
            let from = entry.path();
            let to = dst.join(&name);
            if from.is_dir() {
                if name == "ev" {
                    fs::create_dir_all(&to).expect("ev directory");
                    fs::copy(
                        from.join("vehicle_mapping.csv"),
                        to.join("vehicle_mapping.csv"),
                    )
                    .expect("the vehicle mapping copies");
                    continue;
                }
                fs::create_dir_all(&to).expect("defaults subdirectory");
                walk(&from, &to);
            } else {
                fs::copy(&from, &to).expect("defaults file copies");
            }
        }
    }
    fs::create_dir_all(dst).expect("defaults root");
    walk(src, dst);
}

fn copy_fixture(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("fixture directory");
    for name in ["building.xml", "schedule.csv", "weather.epw"] {
        fs::copy(src.join(name), dst.join(name)).expect("fixture file copies");
    }
}

/// Two runs on the same inputs record equal input records.
#[test]
fn two_runs_on_the_same_inputs_record_equal_input_records() {
    let (hpxml, schedule, weather) = cz4a_paths();
    let defaults = project_root().join("python/hares/defaults");
    let first = run(config_for(
        hpxml.clone(),
        Some(schedule.clone()),
        weather.clone(),
        defaults.clone(),
    ));
    let second = run(config_for(hpxml, Some(schedule), weather, defaults));
    assert_eq!(first, second, "the same inputs record equal digests");
}

/// Changing one input's content moves exactly that input's digest: the
/// schedule's digest moves, every other record is identical.
#[test]
fn changing_one_input_changes_exactly_that_input_record() {
    let defaults = project_root().join("python/hares/defaults");
    let tmp = TempDir::new().expect("temp directory");
    copy_fixture(
        &project_root().join("tests/fixtures/parity/cz4a_ashp_hpwh"),
        tmp.path(),
    );
    let before = run(config_for(
        tmp.path().join("building.xml"),
        Some(tmp.path().join("schedule.csv")),
        tmp.path().join("weather.epw"),
        defaults.clone(),
    ));

    let edited = fs::read_to_string(tmp.path().join("schedule.csv"))
        .expect("the schedule copies")
        .replacen("0.0,0,0,0,0,0,0,0", "0.1,0,0,0,0,0,0,0", 1);
    fs::write(tmp.path().join("schedule.csv"), edited).expect("the schedule edits");

    let after = run(config_for(
        tmp.path().join("building.xml"),
        Some(tmp.path().join("schedule.csv")),
        tmp.path().join("weather.epw"),
        defaults,
    ));

    let (before_schedule, after_schedule) = (
        before.schedule.expect("recorded"),
        after.schedule.expect("recorded"),
    );
    assert_ne!(
        before_schedule.sha256, after_schedule.sha256,
        "the edited schedule's digest moves"
    );
    assert_eq!(before.hpxml, after.hpxml, "the HPXML record is identical");
    assert_eq!(
        before.weather, after.weather,
        "the weather record is identical"
    );
    assert_eq!(
        before.weather_format, after.weather_format,
        "the weather format is identical"
    );
    assert_eq!(
        before.defaults_files_sha256, after.defaults_files_sha256,
        "the defaults digest is identical"
    );
}

/// The digest is over the content, never the path: the same bytes at a
/// different path record the same sha256 with the other path kept.
#[test]
fn input_digests_follow_content_not_path() {
    let (hpxml, schedule, weather) = cz4a_paths();
    let original = run(config_for(
        hpxml.clone(),
        Some(schedule.clone()),
        weather.clone(),
        project_root().join("python/hares/defaults"),
    ));

    let tmp = TempDir::new().expect("temp directory");
    copy_fixture(
        &project_root().join("tests/fixtures/parity/cz4a_ashp_hpwh"),
        tmp.path(),
    );
    let defaults_copy = tmp.path().join("defaults");
    copy_defaults_light(
        &project_root().join("python/hares/defaults"),
        &defaults_copy,
    );

    let moved = run(config_for(
        tmp.path().join("building.xml"),
        Some(tmp.path().join("schedule.csv")),
        tmp.path().join("weather.epw"),
        defaults_copy,
    ));

    assert_eq!(
        original.hpxml.sha256, moved.hpxml.sha256,
        "the HPXML digest follows the content"
    );
    assert_ne!(
        original.hpxml.source, moved.hpxml.source,
        "the HPXML record keeps the path it was read from"
    );
    let (original_schedule, moved_schedule) = (
        original.schedule.expect("recorded"),
        moved.schedule.expect("recorded"),
    );
    assert_eq!(
        original_schedule.sha256, moved_schedule.sha256,
        "the schedule digest follows the content"
    );
    assert_ne!(
        original_schedule.source, moved_schedule.source,
        "the schedule record keeps the path it was read from"
    );
    assert_eq!(
        original.weather.sha256, moved.weather.sha256,
        "the weather digest follows the content"
    );
    assert_ne!(
        original.weather.source, moved.weather.source,
        "the weather record keeps the path it was read from"
    );
    assert_eq!(
        original.defaults_files_sha256, moved.defaults_files_sha256,
        "the defaults digest follows the content"
    );
    assert_eq!(original.weather_format, moved.weather_format);
}

/// Replacing one defaults file's content moves the defaults digest and
/// nothing else: every other record is identical.
#[test]
fn defaults_digest_changes_when_one_file_is_replaced() {
    let (hpxml, schedule, weather) = cz4a_paths();
    let tmp = TempDir::new().expect("temp directory");
    let defaults_before = tmp.path().join("defaults-before");
    let defaults_after = tmp.path().join("defaults-after");
    copy_defaults_light(
        &project_root().join("python/hares/defaults"),
        &defaults_before,
    );
    copy_defaults_light(
        &project_root().join("python/hares/defaults"),
        &defaults_after,
    );

    let mut edited = fs::read_to_string(defaults_after.join("zip_parameters.toml"))
        .expect("the zip parameters copy");
    edited.push_str("\n# digest probe\n");
    fs::write(defaults_after.join("zip_parameters.toml"), edited).expect("the zip parameters edit");

    let before = run(config_for(
        hpxml.clone(),
        Some(schedule.clone()),
        weather.clone(),
        defaults_before,
    ));
    let after = run(config_for(hpxml, Some(schedule), weather, defaults_after));

    assert_ne!(
        before.defaults_files_sha256, after.defaults_files_sha256,
        "the replaced file's content moves the defaults digest"
    );
    assert_eq!(before.hpxml, after.hpxml, "the HPXML record is identical");
    assert_eq!(
        before.schedule, after.schedule,
        "the schedule record is identical"
    );
    assert_eq!(
        before.weather, after.weather,
        "the weather record is identical"
    );
    assert_eq!(
        before.weather_format, after.weather_format,
        "the weather format is identical"
    );
}

/// A file schedule's run reads the schedule profiles too: the schedule
/// injection resolves the profiles a CSV schedule lacks against them, so
/// editing the profiles CSV moves the defaults digest of a file-schedule
/// run and nothing else.
#[test]
fn file_schedule_defaults_digest_moves_with_the_profiles_csv() {
    let (hpxml, schedule, weather) = cz4a_paths();
    let tmp = TempDir::new().expect("temp directory");
    let defaults_before = tmp.path().join("defaults-before");
    let defaults_after = tmp.path().join("defaults-after");
    copy_defaults_light(
        &project_root().join("python/hares/defaults"),
        &defaults_before,
    );
    copy_defaults_light(
        &project_root().join("python/hares/defaults"),
        &defaults_after,
    );

    let profiles = "Default Schedule Parameters.csv";
    let edited = fs::read_to_string(defaults_after.join(profiles))
        .expect("the profiles copy")
        .replacen(
            "0.035, 0.035, 0.035, 0.035, 0.035, 0.059",
            "0.036, 0.035, 0.035, 0.035, 0.035, 0.059",
            1,
        );
    assert_ne!(
        edited,
        fs::read_to_string(defaults_after.join(profiles)).expect("the profiles read"),
        "the probe edits the file"
    );
    fs::write(defaults_after.join(profiles), edited).expect("the profiles edit");

    let before = run(config_for(
        hpxml.clone(),
        Some(schedule.clone()),
        weather.clone(),
        defaults_before,
    ));
    let after = run(config_for(hpxml, Some(schedule), weather, defaults_after));

    assert_ne!(
        before.defaults_files_sha256, after.defaults_files_sha256,
        "the edited profiles CSV moves the defaults digest of a file-schedule run"
    );
    assert_eq!(before.hpxml, after.hpxml, "the HPXML record is identical");
    assert_eq!(
        before.schedule, after.schedule,
        "the schedule record is identical"
    );
    assert_eq!(
        before.weather, after.weather,
        "the weather record is identical"
    );
    assert_eq!(
        before.weather_format, after.weather_format,
        "the weather format is identical"
    );
}

/// A schedule generated from the HPXML carries no record: its inputs are
/// the config and the profiles CSV, both recorded. The weather format is
/// the detected one.
#[test]
fn generated_schedule_has_no_input_record() {
    let inputs = run(config_for(
        project_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"),
        None,
        project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        project_root().join("python/hares/defaults"),
    ));
    assert!(
        inputs.schedule.is_none(),
        "a generated schedule carries no record of its own"
    );
    assert_eq!(
        inputs.weather_format,
        hares_io::WeatherFormat::Epw,
        "a path source's weather format is the detected one"
    );
}
