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
        let path = bench_common::numbered_path(dir.path(), "hares-bench-rl-episode", "toml");
        std::fs::write(&path, bench_common::synthetic_toml(86_400, time_res_s))
            .expect("write the rl_episode bench's TOML");
        Dwelling::from_toml_config_with_reports(&path, Some(Vec::new())).unwrap_or_else(|e| {
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

/// The checkpoint bench is registered: the `[[bench]]` entry the
/// bench-check job compiles and the CI bench job gates. RED as the missing
/// bench: the suite names the registration, so a bench deleted from the
/// manifests (or a bench file added without one) fails here with the
/// missing name.
#[test]
fn bench_checkpoint_bench_is_registered() {
    let manifest = include_str!("../crates/hares-core/Cargo.toml");
    assert!(
        manifest.contains("[[bench]]\nname = \"checkpoint\""),
        "the hares-core manifest does not register the checkpoint bench: \
         the checkpoint save/restore bench's [[bench]] entry is missing"
    );
}

/// The CI bench job's gate set is the complete registered set: every
/// `[[bench]]` entry across EVERY workspace member's manifest is named
/// with `--bench <name>` on EVERY gated `cargo bench` command line the
/// bench job runs: the baseline generation, the PR run, and the
/// confirmation re-run of flagged benches (a bench missing from the
/// confirmation line's flags could never be re-run, so a regression on
/// it could never confirm and the job would pass it silently). The
/// members are enumerated by `cargo metadata` at test time, the bench
/// names parsed from each member's manifest, so a bench added to any
/// crate's manifest (hares-fleet's or hares-io's included) without a CI
/// flag fails this test the same way the five ungated benches this test
/// was written against do.
#[test]
fn ci_bench_job_gates_every_registered_bench() {
    let registered = registered_bench_names();
    let ci = include_str!("../.github/workflows/ci.yml");
    let commands = bench_job_command_lines(ci);
    assert!(
        commands.len() >= 2,
        "the bench job's baseline and PR command lines did not parse \
         from ci.yml: {commands:?}"
    );
    for command in &commands {
        let missing = ungated_benches(&registered, command);
        assert!(
            missing.is_empty(),
            "a gated CI bench command line does not name every registered \
             bench: missing {missing:?} on {command:?}"
        );
    }
}

/// The gate's teeth: a bench that is registered but named on no gated
/// command line is refused. The fixture world is the real enumerated
/// set plus one phantom bench no ci.yml line carries; the same refusal
/// the coverage gate runs must name the phantom on every gated command
/// line. A parser that refuses nothing fails here; a parser that
/// refuses everything fails the coverage gate above.
#[test]
fn ci_bench_gate_refuses_a_registered_but_ungated_bench() {
    let mut registered = registered_bench_names();
    registered.push("phantom_ungated_bench".to_string());
    let ci = include_str!("../.github/workflows/ci.yml");
    let commands = bench_job_command_lines(ci);
    assert!(
        commands.len() >= 2,
        "the bench job's baseline and PR command lines did not parse \
         from ci.yml: {commands:?}"
    );
    for command in &commands {
        let missing = ungated_benches(&registered, command);
        assert!(
            missing.contains(&"phantom_ungated_bench"),
            "the gate did not refuse a registered bench no gated command \
             line names: refused only {missing:?} on {command:?}"
        );
    }
}

/// The `[[bench]]` names across every workspace member's manifest,
/// enumerated by `cargo metadata` at test time: the complete registered
/// set the CI bench job must gate.
fn registered_bench_names() -> Vec<String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = std::process::Command::new(cargo)
        .args([
            "metadata",
            "--no-deps",
            "--offline",
            "--format-version",
            "1",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo metadata runs");
    assert!(
        output.status.success(),
        "cargo metadata fails: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the workspace metadata parses");
    let manifest_paths: Vec<String> = metadata["packages"]
        .as_array()
        .expect("the metadata carries the workspace member list")
        .iter()
        .filter_map(|package| package["manifest_path"].as_str().map(str::to_string))
        .collect();
    assert!(
        manifest_paths.len() >= 10,
        "the workspace members enumerate to fewer than the ten crates the \
         workspace declares: {manifest_paths:?}"
    );

    let mut registered: Vec<String> = Vec::new();
    for path in &manifest_paths {
        let manifest = std::fs::read_to_string(path)
            .unwrap_or_else(|err| panic!("the member manifest {path} reads: {err}"));
        registered.extend(
            registered_benches(&manifest)
                .into_iter()
                .map(str::to_string),
        );
    }
    assert!(
        registered.contains(&"single_building".to_string()),
        "the manifest parse found no benches: {registered:?}"
    );
    registered
}

/// The bench job's gated `cargo bench` command lines: every workflow
/// line that statically enumerates bench targets with `--bench` flags.
/// The gate runs three of them (the baseline generation, the PR run,
/// and the confirmation re-run of flagged benches, whose flags
/// enumerate the search space its positional filter selects from); the
/// step summary's echoed example command mentions `cargo bench` with no
/// flag at all, so the `--bench ` requirement is what tells an
/// invocation from prose.
fn bench_job_command_lines(ci: &str) -> Vec<&str> {
    ci.lines()
        .filter(|line| line.contains("cargo bench") && line.contains("--bench "))
        .collect()
}

/// The registered benches one gated command line fails to name: the
/// meta-gate's refusal list for that line. Empty means the line's gate
/// set is the complete registered set.
fn ungated_benches<'a>(registered: &'a [String], command: &str) -> Vec<&'a str> {
    registered
        .iter()
        .map(String::as_str)
        .filter(|name| !command.contains(&format!("--bench {name} ")))
        .collect()
}

/// The `[[bench]]` entries' names in one manifest's text, in declaration
/// order: the `[[bench]]` section header followed by its `name` line.
fn registered_benches(manifest: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let mut lines = manifest.lines();
    while let Some(line) = lines.next() {
        if line.trim() != "[[bench]]" {
            continue;
        }
        let name = lines
            .next()
            .map(str::trim)
            .and_then(|line| line.strip_prefix("name = \""))
            .and_then(|line| line.strip_suffix('"'))
            .unwrap_or_else(|| panic!("a [[bench]] section without a name line"));
        names.push(name);
    }
    names
}

/// The checkpoint bench's two rosters and its own setup assertions, at
/// suite cost: both rosters construct, the prefix-stepped state saves to
/// the pinned file path, the gated parse restores into a fresh dwelling
/// and re-saves the same checkpoint, and the restored run's continuation
/// holds the continuous run's state. The bench runs the same assertions
/// once per roster size in its own setup; here they fail the suite with a
/// named test when the benches' fixture handling rots.
#[test]
fn bench_checkpoint_round_trip_assertions_hold_at_both_roster_sizes() {
    let dir = tempdir().expect("bench temp dir");
    bench_common::assert_checkpoint_round_trip(
        dir.path(),
        || bench_common::checkpoint_small_dwelling(dir.path()),
        bench_common::CHECKPOINT_SMALL_PREFIX_STEPS,
        bench_common::CHECKPOINT_SMALL_CONTINUATION_STEPS,
        "the checkpoint bench's small roster",
    );
    let schedule_path =
        bench_common::numbered_path(dir.path(), "hares-bench-checkpoint-schedule", "csv");
    let weather_path =
        bench_common::numbered_path(dir.path(), "hares-bench-checkpoint-weather", "epw");
    bench_common::write_schedule_csv(&schedule_path);
    bench_common::write_weather_epw(&weather_path);
    bench_common::assert_checkpoint_round_trip(
        dir.path(),
        || bench_common::checkpoint_large_dwelling(dir.path(), 1, &schedule_path, &weather_path),
        bench_common::CHECKPOINT_LARGE_PREFIX_STEPS,
        bench_common::CHECKPOINT_LARGE_CONTINUATION_STEPS,
        "the checkpoint bench's large roster",
    );
}

/// The rl_episode bench's year episode runs to completion: the 8760-step
/// bench's dwelling (the shared synthetic fixture, the shared
/// YEAR_EPISODE_* shapes, the leap-year start 2024-01-01) steps its full
/// 8,760-step loop without meeting the run's end. The bench was ungated
/// in CI until the complete bench gate set ran it, and its first gated
/// run panicked at the boundary: at a 365-day duration the
/// typical-weather rule's Feb 29 skip leaves 8,736 steps, and the bench's
/// 8,760th step was past the configured end (RED at the base `de259c8e`'s
/// own probe; the fix, the 366-day duration, is in the bench). The
/// non-leap-start case starts 2023-01-01, where the run year carries no
/// Feb 29 to skip and the same 366-day duration holds the full 8,784
/// steps: the witness steps every one of them. The suite runs the
/// episodes' constructions and full step loops at suite cost (about two
/// seconds), so the boundary can never again sit outside every test's
/// reach.
#[test]
fn bench_rl_episode_year_episode_steps_to_completion() {
    let dir = tempdir().expect("bench temp dir");
    // The leap start: with the skip, exactly 8,760 steps, the bench's own
    // loop length.
    let path = bench_common::numbered_path(dir.path(), "hares-bench-rl-episode-year", "toml");
    std::fs::write(
        &path,
        bench_common::synthetic_toml(
            bench_common::YEAR_EPISODE_DURATION_S,
            bench_common::YEAR_EPISODE_TIME_RES_S,
        ),
    )
    .expect("write the year episode's TOML");
    let mut dwelling = Dwelling::from_toml_config_with_reports(&path, Some(Vec::new()))
        .expect("the year episode's dwelling must construct");
    for step in 0..bench_common::YEAR_EPISODE_STEPS {
        dwelling
            .step()
            .unwrap_or_else(|err| panic!("the year episode's step {step} fails: {err}"));
    }

    // The non-leap start: no Feb 29 in the run year, no skip, and the
    // duration's full 8,784 steps exist to run.
    let path = bench_common::numbered_path(dir.path(), "hares-bench-rl-episode-year", "toml");
    std::fs::write(
        &path,
        bench_common::synthetic_toml_with_start(
            bench_common::YEAR_EPISODE_DURATION_S,
            bench_common::YEAR_EPISODE_TIME_RES_S,
            "2023-01-01T00:00:00Z",
        ),
    )
    .expect("write the non-leap year episode's TOML");
    let mut dwelling = Dwelling::from_toml_config_with_reports(&path, Some(Vec::new()))
        .expect("the non-leap year episode's dwelling must construct");
    let non_leap_steps: u64 =
        (bench_common::YEAR_EPISODE_DURATION_S / bench_common::YEAR_EPISODE_TIME_RES_S) as u64;
    assert_eq!(
        non_leap_steps, 8_784,
        "the 366-day duration at 3600 s holds 8,784 steps"
    );
    for step in 0..non_leap_steps {
        dwelling
            .step()
            .unwrap_or_else(|err| panic!("the non-leap year episode's step {step} fails: {err}"));
    }
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
        if let hares_io::Destination::Csv { path, .. } = &mut report.destination
            && let Some(name) = path.file_name()
        {
            *path = out_dir.join(name);
        }
    }
    config
}
