//! ResStock homes carry no schedule warning.
//!
//! Every schedule column the ResStock 2025.1 fixtures use is read by the
//! engine or stated as not used by a `COLUMN_MAPPINGS` entry, so a home built
//! and stepped reports no warning with source `schedule` (and the
//! `consumer_shape_900s` golden inputs report none at all).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, FixedOffset, TimeZone};
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
use hares_io::OutputFormat;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The manifest's defaults tree: the repository `defaults/` directory with the
/// manifest's `[defaults_files]` replacements copied over a symlinked copy.
/// Returns the directory the dwelling reads defaults from, dropped with the
/// returned guard.
fn prepare_defaults_tree(manifest_path: &Path) -> std::io::Result<(PathBuf, tempfile::TempDir)> {
    let manifest = std::fs::read_to_string(manifest_path)?;
    let value: toml::Value =
        toml::from_str(&manifest).map_err(|err| std::io::Error::other(err.to_string()))?;
    let replacements: BTreeMap<String, String> = value
        .get("defaults_files")
        .and_then(|files| files.as_table())
        .map(|table| {
            table
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                .collect()
        })
        .unwrap_or_default();

    let repo_defaults = project_root().join("defaults");
    let temp = tempfile::TempDir::new()?;
    copy_tree_symlinked(&repo_defaults, temp.path())?;
    for (relative, replacement) in &replacements {
        let target = temp.path().join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // The tree linked `target` into the repository's own defaults tree,
        // and fs::copy follows a destination symlink: the link must go
        // first, so the copy materializes a real file in the temporary
        // directory instead of overwriting the repository's file.
        match std::fs::remove_file(&target) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        std::fs::copy(project_root().join(replacement), &target)?;
    }
    Ok((temp.path().to_path_buf(), temp))
}

/// Mirrors `dir` under `into` with per-file symlinks, so the run reads the
/// repository defaults and the manifest's replacements in one tree.
fn copy_tree_symlinked(dir: &Path, into: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let source = entry.path();
        let target = into.join(&name);
        if source.is_dir() {
            std::fs::create_dir_all(&target)?;
            copy_tree_symlinked(&source, &target)?;
        } else {
            #[cfg(unix)]
            std::os::unix::fs::symlink(&source, &target)?;
            #[cfg(not(unix))]
            std::fs::copy(&source, &target)?;
        }
    }
    Ok(())
}

fn sim_config_from_manifest(manifest: &toml::Value) -> SimulationConfig {
    let sim = manifest.get("simulation").expect("manifest [simulation]");
    let start = sim
        .get("start_time")
        .and_then(|v| v.as_str())
        .expect("start_time string");
    let duration_s = sim
        .get("duration")
        .or_else(|| sim.get("duration_s"))
        .and_then(|v| v.as_integer())
        .expect("duration seconds");
    let time_res_s = sim
        .get("time_res")
        .or_else(|| sim.get("time_res_s"))
        .and_then(|v| v.as_integer())
        .expect("time_res seconds");
    SimulationConfig {
        start_time: DateTime::parse_from_rfc3339(start).expect("start_time parses"),
        duration: Duration::seconds(duration_s),
        time_res: Duration::seconds(time_res_s),
        output_verbosity: 0,
        write_output: false,
        output_path: None,
        output_format: OutputFormat::Csv,
        output_chunk_size: 1024,
        setpoint_deadband_c: None,
        master_seed: sim
            .get("master_seed")
            .and_then(|v| v.as_integer())
            .unwrap_or(0) as u64,
        civil_timezone: None,
        site_location: hares_io::SiteLocationOverride::default(),
        retain_batches: false,
        rotation: hares_io::RotationPolicy::None,
    }
}

/// Converts a `toml::Value` to its `serde_json::Value` equivalent (the
/// dwelling config's overrides are JSON-valued).
fn toml_to_json(value: &toml::Value) -> serde_json::Value {
    match value {
        toml::Value::String(s) => serde_json::Value::String(s.clone()),
        toml::Value::Integer(i) => serde_json::Value::Number((*i).into()),
        toml::Value::Float(f) => serde_json::json!(f),
        toml::Value::Boolean(b) => serde_json::Value::Bool(*b),
        toml::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(toml_to_json).collect())
        }
        toml::Value::Table(table) => serde_json::Value::Object(
            table
                .iter()
                .map(|(k, v)| (k.clone(), toml_to_json(v)))
                .collect(),
        ),
        toml::Value::Datetime(dt) => serde_json::Value::String(dt.to_string()),
    }
}

/// Builds the `consumer_shape_900s` golden inputs (bldg0176227) as the golden
/// run does: the manifest's home files, the manifest's defaults tree, its
/// overrides and its zero warm-up.
fn built_consumer_shape_home() -> (Dwelling, tempfile::TempDir) {
    let manifest_path = project_root().join("tests/fixtures/golden/consumer_shape_900s.toml");
    let manifest: toml::Value =
        toml::from_str(&std::fs::read_to_string(&manifest_path).expect("read manifest"))
            .expect("manifest parses");
    let (defaults_dir, defaults_guard) =
        prepare_defaults_tree(&manifest_path).expect("defaults tree prepares");

    let home = &manifest["home"][0];
    let sim = sim_config_from_manifest(&manifest);
    let overrides_json = home
        .get("overrides")
        .map(toml_to_json)
        .unwrap_or(serde_json::Value::Null);

    let config = DwellingConfig {
        hpxml_path: project_root().join(home["hpxml"].as_str().expect("hpxml path")),
        schedule_path: Some(project_root().join(home["schedule"].as_str().expect("schedule path"))),
        weather_path: project_root().join(home["weather"].as_str().expect("weather path")),
        defaults_path: Some(defaults_dir),
        sim_config: sim,
        overrides: Some(overrides_json),
        bldg_id: home["bldg_id"].as_integer().expect("bldg id"),
        initialization_duration: None,
        resample_overrides: None,
        patches: None,
    };
    let dwelling = Dwelling::from_config(config).expect("dwelling builds from the golden inputs");
    (dwelling, defaults_guard)
}

/// The consumer-shape golden inputs (bldg0176227) built and stepped once carry
/// no warning at all: a warning that appears on a golden home stops the entry.
#[test]
fn resstock_golden_home_carries_no_warning() {
    let (mut dwelling, _defaults_guard) = built_consumer_shape_home();
    dwelling.step().expect("a step of the golden home runs");
    let warnings = dwelling.take_warnings();
    assert!(
        warnings.is_empty(),
        "the consumer_shape_900s golden home must carry no warning, got {warnings:?}"
    );
}

/// The bldg0176775 ResStock home (its schedule carries the EV columns and the
/// wet-appliance hot-water columns) carries no warning with source `schedule`:
/// the EV columns are stated as not used and the hot-water columns are read.
#[test]
fn resstock_homes_carry_no_schedule_warning() {
    let bldg = project_root().join("tests/fixtures/resstock/2025.1/bldg0176775");
    let schedule_warnings = bldg0176775_schedule_warnings(&bldg.join("in.schedules.csv"));
    assert!(
        schedule_warnings.is_empty(),
        "the bldg0176775 schedule columns are all known, got {schedule_warnings:?}"
    );
}

/// A schedule column for equipment the home does not have, and for which
/// there is no default energy to create it with, follows no load; the
/// dwelling's warning list says so, naming the column.
#[test]
fn a_column_for_equipment_the_home_lacks_is_a_dwelling_warning() {
    let bldg = project_root().join("tests/fixtures/resstock/2025.1/bldg0176775");
    let csv = std::fs::read_to_string(bldg.join("in.schedules.csv")).expect("read the schedule");
    let with_basement_lighting: Vec<String> = csv
        .lines()
        .enumerate()
        .map(|(i, line)| {
            let lighting_interior = line.split(',').nth(3).expect("lighting_interior");
            if i == 0 {
                assert_eq!(lighting_interior, "lighting_interior");
                format!("{line},lighting_basement")
            } else {
                format!("{line},{lighting_interior}")
            }
        })
        .collect();
    let dir = tempfile::tempdir().expect("temp dir");
    let schedule_path = dir.path().join("in.schedules.csv");
    std::fs::write(&schedule_path, with_basement_lighting.join("\n")).expect("write schedule");

    let schedule_warnings = bldg0176775_schedule_warnings(&schedule_path);
    assert_eq!(
        schedule_warnings.len(),
        1,
        "one warning for the one unused column, got {schedule_warnings:?}"
    );
    assert!(
        schedule_warnings[0].contains("'lighting_basement' is not read")
            && schedule_warnings[0].contains("Basement Lighting"),
        "{}",
        schedule_warnings[0]
    );
}

/// The `schedule` warnings of the bldg0176775 home built with
/// `schedule_path` and stepped once.
fn bldg0176775_schedule_warnings(schedule_path: &Path) -> Vec<String> {
    let bldg = project_root().join("tests/fixtures/resstock/2025.1/bldg0176775");
    let sim = SimulationConfig {
        start_time: FixedOffset::west_opt(5 * 3600)
            .expect("UTC-5 offset is valid")
            .with_ymd_and_hms(2018, 1, 1, 0, 0, 0)
            .unwrap(),
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
    };
    let config = DwellingConfig {
        hpxml_path: bldg.join("home.xml"),
        schedule_path: Some(schedule_path.to_path_buf()),
        weather_path: project_root()
            .join("tests/fixtures/resstock/2025.1/weather/G3400270_2018.csv"),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: sim,
        overrides: None,
        bldg_id: 176_775,
        initialization_duration: None,
        resample_overrides: None,
        patches: None,
    };
    let mut dwelling = Dwelling::from_config(config).expect("dwelling builds from bldg0176775");
    dwelling.step().expect("a step of the ResStock home runs");
    dwelling
        .take_warnings()
        .into_iter()
        .filter(|warning| warning.starts_with("schedule: "))
        .collect()
}
