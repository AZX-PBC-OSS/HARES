# Handoff: fix-headbreaks follow-up (review-headbreaks-cd1b5972.md findings)

## Where the work is
- Handoff branch: `handoff/fix-headbreaks`, commit `9ee0f6d7f2e51182c16d0fb1cb09f8c00dba4fda` (one commit).
- Worktree: `/home/rich/src/HARES/.workstrees/fix-headbreaks`, checked out on the handoff branch, clean.
- Base: `cd1b5972` (tip of `fix-headbreaks`, unchanged; 20 commits on `db5efe80`, the `feat/output-results` head).
- Submodules `vendors/OCHRE` and `vendors/EnergyPlus` are initialised in the worktree.
- An earlier commit `107c1867` was undone; it is orphaned, ignore it.
- My scratch directory and target dir are deleted.

## Files in 9ee0f6d7
- `crates/hares-fleet/src/fleet.rs`: `Fleet::from_resstock` (line 149) takes `defaults_path: Option<&Path>` and copies it into each config (line 209). The two existing tests (from 2176 and 2268) pass `None`. New test at 2212.
- `crates/hares-python/src/py_fleet.rs`: lines 120, 130, 164 take `defaults_path=None` and pass it through, with `#[allow(clippy::too_many_arguments)]`.
- `python/ochre_next/_hares.pyi`: `Fleet.from_resstock` gains `defaults_path: str | None = ...` (line 2643).
- `tests/python/test_py_fleet.py`: `test_from_resstock_accepts_a_defaults_path` (line 295).
- `benches/common.rs`: `repo_defaults_path()` (line 35). `build_dwelling_config` takes `defaults_path: Option<PathBuf>` (lines 121 and 144). `benches/fleet.rs:28` and `benches/single_building.rs:32` pass `Some(common::repo_defaults_path())`.
- `crates/hares-io/src/schedule_resolve.rs`: `ensure_specs_for_csv_columns` (line 1617) takes the schedule columns, and its gate is around line 1658. Helper `inject_into_crawlspace_home` (4504). Tests at 4525, 4547 and 4564 replace the two old warning tests.

## State per finding

### F1: done and tested (Rust); Python test not run
- `defaults_path` is optional, matching `DwellingConfig.defaults_path: Option<PathBuf>` and the Python `DwellingConfig(defaults_path=None)`.
- `from_resstock_passes_the_defaults_directory_to_each_dwelling` builds a fleet from the parquet fixture with and without the path. It points the entries at `base.xml`, `data/examples/BEopt_example_schedule.csv` (no lighting or plug-load columns) and the Denver EPW. It asserts that `Dwelling::from_config` builds with the path and fails naming `defaults_path` without it.
- Mutation: copying `None` instead of the argument makes the test fail. Restored, it passes.
- Benchmarks:
  - With `None`, `cargo bench -p hares-core --bench single_building -- --test 30_day` panics: "equipment 'Refrigerator' has no schedule source ... set defaults_path". This breakage is from this branch's `215398f3`; `[bench]` is `--no-run` and cannot see it.
  - With the path it prints Success.
- Pre-existing, not fixed, report it:
  - `Fleet::from_resstock` points every `hpxml_path` and `schedule_path` at the dataset `.zip` (`hares-io/src/resstock.rs` `build_zip_path`), and nothing in the engine unpacks it.
  - `weather_fips` is always `None`.
  - So its configs cannot build a dwelling as-is, which is why the test swaps in unpacked files.

### F4: done and tested
- **Garage rule:** a `lighting_garage` column with no garage is unread and raises no warning, as in OS-HPXML v1.12.0.
  - `HPXMLtoOpenStudio/resources/lighting.rb:113-122` reads the column only when `grg_kwh > 0`.
  - `schedules.rb:1098-1101` warns only for unknown column names.
  - The v1.12.0 generator writes the column only when a garage exists (`BuildResidentialScheduleFile/resources/schedules.rb:1068-1070`).
  - The ResStock 2024.2 fixtures carry a non-zero column for all 11 homes, 8 of which have no garage.
- **Basement rule:** `lighting_basement` (OCHRE's column; no generator writes it) warns only when it is non-zero for a home without a Finished Basement. An all-zero column raises no warning.
- **Mutations**, each of which makes its test fail:
  - re-adding the garage warning fails the garage test;
  - warning regardless of values fails the all-zero test;
  - removing the basement warning fails the load test.
- **Census** (a temporary release integration test, since deleted). It built every `tests/fixtures/resstock` home with its own schedule, and the 425 OS-HPXML samples with a generated schedule and the repo defaults.

  | | ResStock homes built | Garage warnings | Basement warnings | OS-HPXML samples built | Sample warnings | Sample build errors |
  |---|---|---|---|---|---|---|
  | Before | 31 | 8 | 0 | 365 | 0 | 60 |
  | After | 31 | 0 | 0 | 365 | 0 | 60 |

  The 60 sample build errors are pre-existing and unrelated, for example WSHP `backup_capacity_w` and multifamily TV power.

### F3: analysed, no code; reword not done
- A dwelling-level test that fails on a revert of `e113a61f` cannot exist:
  - Since `ee7bee85` (before the base), assembly constructs and inits each equipment from the same merged config (`crates/hares-core/src/dwelling/mod.rs` around 2578-2632).
  - Nothing else constructs a heat pump heater: the Python `add_*` paths make only Battery, PV, EV and Protocol Bridge; there is no runtime re-init; checkpointing does not init.
  - The `typed().ok()` swallow is also unreachable there, because init reads the same config strictly and fails.
- So `e113a61f`'s message overstates. Proposed body: "The constructor read the backup fuel with typed().ok() to set the FUEL capability, while init resolved it strictly and set the flow from it. Dwelling assembly passes one merged config to both, so they agreed there, but an equipment initialised with a config other than its constructor's would declare a capability its step does not follow, and the constructor swallowed a read error init reports. Init now sets the FUEL bit from the backup fuel it resolves, and the constructor no longer reads the typed config for it."

### mkdtemp leak (`tests/python/test_thermal_trace.py:272`): not started, on purpose
- `fix-testhyg` (`c1aca232`) already replaces it with `tmp_path_factory.mktemp("thermal_trace")`. Do not duplicate it.

### F2: left for OD-49 as instructed.

## Unverified
- `test_from_resstock_accepts_a_defaults_path`: not run (needs `[python-build]`).
- Clippy in any profile, `cargo fmt --check`, pyright on the stub: not run.
- Whether F4 moves any golden digest: not checked (`[golden]` not run).
- The fleet benchmark at run time: only `single_building` was spot-checked.
- The full `plan.md` §3 block: not run.
- F3's suggested two-sided mutation (revert `e113a61f` and make assembly construct from the unmerged config): not run.

## Remaining steps, in order
1. Review `9ee0f6d7`, then split it into two commits that each build:
   - F1: fleet, py_fleet, stub, `test_py_fleet.py`, benches;
   - F4: `schedule_resolve.rs`.

   Messages must pass `[writing]`: no em or en dashes, ticket IDs or agent words.
2. Reword `e113a61f` (or record its overstatement in the write-up if the coordinator prefers). The 7 golden re-capture commits (`36fd815d`..`cd1b5972`) record `git_head` = their parent, so any rewrite before them orphans those shas. A sequence that needs no reset, checkout or stash:
   - Make the reworded commit with `git commit-tree e113a61f^{tree} -p e113a61f^ -F msg`, keeping author and date via `GIT_AUTHOR_*`.
   - On a new branch at it, cherry-pick `d1ca7a0d 04395092 215398f3 e842828f e62afddf 9e3fc147`, then the F1 and F4 commits.
   - Re-capture the 7 goldens one per commit, from the clean tree at each parent, with `cargo run --release -p frame_golden` (capture subcommand in `tools/frame_golden/src/main.rs`; `-F dst` for `full_year_900s_dst`).
   - Commit each with `git commit -C <old golden sha>`; the old message stays true because the previous state is still the dirty `1b708ccb` capture.
   - Check that each diff changes only `git_head` and `git_dirty`. If a digest moves, the message must say so.
   - Point `fix-headbreaks` at the result.
3. Run the targeted tests:
   - `cargo nextest run -p hares-io -p hares-fleet -E 'test(lighting) | test(from_resstock)'`;
   - the `schedule_path_contract` and `backup_fuel_override_contract` binaries;
   - `cargo fmt --check` and clippy in all block profiles;
   - `cargo bench -p hares-core --bench single_building -- --test 30_day`.
4. On the final tip, run the full `plan.md` §3 block once:
   - Put `CARGO_TARGET_DIR` and `TMPDIR` in a private scratch dir. It needs about 25 GB of target and up to 19 GB of TMPDIR; check disk first.
   - `[stray]` TMPDIR may fail only from pre-existing leaks. Show that the new and changed tests leave nothing in an empty TMPDIR: the fleet test, the three `schedule_resolve` tests, and `test_py_fleet.py`.
5. Independent review, then fold fast-forward onto `feat/output-results` (still `db5efe80`). The review's suggested order: `fix-headaudit` and this branch first, then rebase `fix-testhyg` once (6 textual conflicts are listed in the review).

## Traps
- Never checkout, stash, reset or revert in the main tree. Edit files only with Edit or Write; a hook blocks sed and interpreter writes.
- `[bench]` is `--no-run`. Benchmark breakage at run time is invisible to the block.
- `Fleet::from_resstock` configs point at zip archives (see F1). Do not write a test that expects them to build as they are.
- The golden provenance chain (step 2) breaks on any history rewrite before the re-capture commits.
- 60 OS-HPXML samples fail to build with a generated schedule at this head. This is pre-existing.
