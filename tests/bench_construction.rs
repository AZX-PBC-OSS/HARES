//! The benches' dwelling construction paths, exercised by the suite.
//!
//! The bench binaries compile the same fixtures the CI bench job runs, but
//! `cargo bench --no-run` only compiles them: a bench fixture that no
//! longer constructs (a construction-time hardening guard rejecting it)
//! fails a CI bench run, not a build. These tests construct every bench
//! binary's dwellings through the bench's own fixture code, so bench rot
//! fails the suite with a named test instead.

#[path = "../benches/common.rs"]
mod bench_common;

use std::path::Path;

use hares_core::{Dwelling, SimulationEngine};
use hares_fleet::Fleet;
use tempfile::tempdir;

/// The rl_step bench's construction path: the shared synthetic TOML
/// fixture through `Dwelling::from_toml_config_with_reports`, the exact
/// call the bench's `dwelling_step_single` setup makes.
#[test]
fn bench_rl_step_dwelling_constructs() {
    let dir = tempdir().expect("bench temp dir");
    let path = bench_common::numbered_path(dir.path(), "hares-bench-rl-single", "toml");
    bench_common::synthetic_toml_case(&path, 86_400);
    let mut dwelling = Dwelling::from_toml_config_with_reports(&path, Some(Vec::new()))
        .expect("the rl_step bench's dwelling must construct");
    dwelling
        .step()
        .expect("the rl_step bench's dwelling must step");
}

/// The rl_episode bench's construction path: the same shared fixture at
/// the episode resolutions the bench builds (1-minute steps and the
/// 900-second episode step) through `make_dwelling`'s call shape.
#[test]
fn bench_rl_episode_dwellings_construct() {
    let dir = tempdir().expect("bench temp dir");
    for time_res_s in [60, 900] {
        let path =
            bench_common::numbered_path(dir.path(), "hares-bench-rl-episode", "toml");
        std::fs::write(&path, bench_common::synthetic_toml(86_400, time_res_s))
            .expect("write the rl_episode bench's TOML");
        Dwelling::from_toml_config_with_reports(&path, Some(Vec::new()))
            .unwrap_or_else(|e| {
                panic!("the rl_episode bench's dwelling must construct at {time_res_s} s: {e}")
            });
    }
}

/// The fleet bench's construction path: the shared HPXML fixture through
/// `build_dwelling_config` with the fleet shape (no own reports, 900 s
/// steps) into `Fleet::from_buildings`, exactly what the bench's fold
/// setup builds.
#[test]
fn bench_fleet_constructs() {
    let dir = tempdir().expect("bench temp dir");
    let schedule_path = bench_common::numbered_path(dir.path(), "hares-bench-schedule", "csv");
    let weather_path = bench_common::numbered_path(dir.path(), "hares-bench-weather", "epw");
    bench_common::write_schedule_csv(&schedule_path);
    bench_common::write_weather_epw(&weather_path);
    let configs = (0..3)
        .map(|idx| {
            let mut config = bench_common::build_dwelling_config(
                dir.path(),
                idx + 1,
                schedule_path.clone(),
                weather_path.clone(),
                bench_common::repo_defaults_path(),
                chrono::Duration::hours(24),
            );
            config.settings.sim_config.reports.clear();
            config.settings.sim_config.time_res = chrono::Duration::seconds(900);
            config
        })
        .collect::<Vec<_>>();
    Fleet::from_buildings(configs).expect("the fleet bench's fleet must construct");
}

/// The single_building bench's construction path: the shared HPXML
/// fixture through `build_dwelling_config` and `SimulationEngine::run`,
/// the construction the bench performs per iteration. One day at the
/// fleet's 900 s steps witnesses the full construction (HPXML
/// resolution, defaults, weather parse) at suite cost.
#[test]
fn bench_single_building_constructs_and_runs() {
    let dir = tempdir().expect("bench temp dir");
    let schedule_path = bench_common::numbered_path(dir.path(), "hares-bench-schedule", "csv");
    let weather_path = bench_common::numbered_path(dir.path(), "hares-bench-weather", "epw");
    bench_common::write_schedule_csv(&schedule_path);
    bench_common::write_weather_epw(&weather_path);
    let mut config = bench_common::build_dwelling_config(
        dir.path(),
        1,
        schedule_path,
        weather_path,
        bench_common::repo_defaults_path(),
        chrono::Duration::hours(24),
    );
    config.settings.sim_config.time_res = chrono::Duration::seconds(900);
    let engine = SimulationEngine::new();
    let dir_out = tempdir().expect("bench output temp dir");
    let _witness = engine
        .run(relocate_reports(&config, dir_out.path()))
        .expect("the single_building bench's run must construct and run");
}

/// The shared fixture's report destinations are relative to the bench
/// directory the caller passes; a re-run in a test writes its own files,
/// so the report paths point at the test's own directory.
fn relocate_reports(
    config: &hares_core::DwellingConfig,
    out_dir: &Path,
) -> hares_core::DwellingConfig {
    let mut config = config.clone();
    for report in &mut config.settings.sim_config.reports {
        if let hares_io::Destination::Csv { path, .. } = &mut report.destination {
            if let Some(name) = path.file_name() {
                *path = out_dir.join(name);
            }
        }
    }
    config
}
