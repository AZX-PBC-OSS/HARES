# Review of fix-headbreaks at cd1b5972

Reviewer and red team: independent. Branch `fix-headbreaks`, tip `cd1b5972`, 20 commits on `db5efe80` (the current `feat/output-results` head). Sources read first: `fix-headbreaks.md`, `audit-OR-19-fold-and-operator-1b708ccb.md`, `plan.md` section 3. The write-up was not trusted; every row below was re-run.

## Verdict: SHIP

No blocking defect. All 7 audit items that carry code (H-1/M-2, M-3, M-4, L-1 to L-7) are fixed as claimed, each new test fails when its fix is reverted, and the full verification block is green except `[stray]` TMPDIR, whose leaks all come from tests this branch did not touch. There are one medium and three low findings, none introduced by this branch; the medium one is a residual of M-4 that should be ticketed or fixed before the contract is called closed.

## Findings

### F1 Medium (residual of M-4, not a regression): the ResStock fleet constructor cannot supply `defaults_path`

- `crates/hares-fleet/src/fleet.rs:204`: `Fleet::from_resstock` builds every `DwellingConfig` with `defaults_path: None` and has no parameter to set one (signature at `fleet.rs:141-148`). `benches/common.rs:137` does the same.
- Under the new contract (`crates/hares-core/src/dwelling/mod.rs:228-233`) a fleet dwelling whose ResStock schedule lacks a column that an equipment maps to now fails construction with an error naming `defaults_path`, and the caller has no way to act on it. The tests that exercise `from_buildings` pass `Some(repo_defaults_path())` (`fleet.rs:1201`); production `from_resstock` does not.
- Not reproduced end to end: I did not construct a ResStock fleet whose schedule omits a mapped column, so whether real ResStock schedules hit this is unconfirmed. The gap in the API is certain; the frequency is not.
- Behaviour is the same at `db5efe80` (it comes from `952e5486`); the branch's M-4 text lists the other callers but not this one.
- Fix: add a `defaults_path` argument to `Fleet::from_resstock` (and the Python binding in `py_fleet.rs`), or record it against OD-49.

### F2 Low: the "never a working-directory guess" wording is true of the profiles only

- `crates/hares-core/src/dwelling/blueprint.rs:117-119`: with no `defaults_path` the `DefaultsStore` still loads from the relative path `"defaults"` and, if absent, warns and runs on an empty store. A config with no `defaults_path` therefore gets a cwd-guessed store but no schedule profiles.
- The field doc (`mod.rs:228-232`) says "The profiles never come from a working-directory guess", which is accurate for schedule profiles. The write-up discloses the store fallback as OD-49. No defect in the diff; recorded so the residual is not lost. The consumer's `engine.py:337` comment ("The engine's fallback is the relative string 'defaults'") is still true.

### F3 Low: the heater change is correct, but its "real bug" claim is not demonstrated end to end

- `crates/hares-equipment/src/hvac/heat_pump/heater.rs` (`e113a61f`). With the constructor read restored and the init `set(FUEL, ...)` removed (a true revert of the source), only `fuel_capability_follows_the_config_init_applies` fails. The four dwelling-level tests in `crates/hares-core/tests/backup_fuel_override_contract.rs` (override to Electric, override to Gas, and both no-override samples) still pass. So through the `DwellingConfig.overrides` path the capability was already right before the fix.
- The fix is still sound: it removes a `typed().ok()` swallow, and capability now follows the config `init` applies (the same pattern as `scheduled_load.rs:360`). After an override-merged re-init the capability is consistent in both directions (unit test covers Electric to Gas and Gas to Electric). The commit message and write-up overstate it as a reproduced stale-capability bug. Reword, or find the path that constructs with one config and inits with another.

### F4 Low: the new lighting warnings fire on routine homes

- `crates/hares-io/src/schedule_resolve.rs` (`c71a360b`): its own comment says the schedule templates carry `lighting_garage` and `lighting_basement` for every home, so every dwelling with no garage or no finished basement now carries one or two construction warnings. This is the observability OD-57 asked for; the cost is noise in the warning list. I did not measure warning counts on the corpus (inference from the comment and the code). No test failed because of it.

### Not findings

- `notebooks/simulation_explorer.py:528` keeps one `type: ignore`; disclosed, notebooks are outside pyright's `include`.
- `tests/python/test_resstock_data.py::TestFetchResStockFleet` depends on the network: `fetch_resstock_fleet` goes through `_fetch_fleet_async` / `_download_building_async` (`resstock.py:1000-1046`), while the tests patch `fetch_resstock_building`, which that path never calls. In one isolated rerun of that class in my sandbox, 5 of 6 tests tried to reach `oedi-data-lake.s3.amazonaws.com` and failed; in the full block run and in an earlier isolated run all passed. The branch's only change to that file is `type: ignore` removal and a frozen-dataclass test. This is the class `fix-testhyg` (`458a9879`) fixes.

## Checks

### 1. Golden re-captures: pass
- `git diff db5efe80 cd1b5972 -- tests/fixtures/golden`: 7 files, 14 lines (7 insertions, 7 deletions). With `git_head` normalised, the only lines are `git_dirty: true` to `false`, 7 each. Every digest line is unchanged.
- Each commit's `git_head` is its parent, and `git merge-base --is-ancestor` passed for 7 of 7 (`9e3fc147`, `36fd815d`, `40e6d68c`, `244089da`, `e9382a47`, `857a1b21`, `21c6f894`). All 7 now say `git_dirty: false`.
- `[golden]` on the tip: default `compare-all` 6 of 6 identical (`consumer_shape_900s`, `fleet_mixed_offset`, `full_year_hourly`, `leap_year_february_900s`, `month_boundary_billing_3600s`, `weather_wrap_900s`); `-F dst` 1 of 1 identical (`full_year_900s_dst`).

### 2. M-3 pyright: pass
- `requires-python = ">=3.12,<3.14"`. `typing-extensions>=4.16.0` is a runtime dependency (`pyproject.toml:14`). `TypeIs` is imported from `typing_extensions` at `python/ochre_next/helics/_types.py:5` and `python/ochre_next/data/resstock.py:22`. `typing.TypeIs` is 3.13+, so this is correct for the floor.
- `[tool.pyright] pythonVersion = "3.12"` (`pyproject.toml:95`). `pyright` 1.1.414 on the tip: 0 errors, 0 warnings.
- `6a887993` has patch-id `027d889e...`, identical to `fix-headaudit`'s `4061da95`.
- Mutation: restoring `from typing import ... TypeIs` in `_types.py` gives 5 pyright errors (`is_json_object` unknown, plus consequences in `helics/fleet.py`).
- Ruff: for the 12 changed `.py` files the per-file finding count is the same or lower than at `db5efe80` (bench_4month_der 8 to 5, test_ashrae_reference 2 to 0, the rest equal). The repo-wide 449 ruff findings are pre-existing.

### 3. M-4 defaults path: contract holds, caller gap in F1
- The schedule resolver reads profiles only from `config.defaults_path` (`mod.rs:2369`). The `"defaults"` fallback now exists only for the `DefaultsStore` (`blueprint.rs:117-119`, F2).
- Error text (`schedule_resolve.rs`, `DefaultProfiles::missing_profile`): "no 'X' profile, because no defaults directory is configured to load 'Default Schedule Parameters.csv' from; set defaults_path". For a generated schedule (`mod.rs:270-275`): "no defaults directory is configured: a generated schedule needs the default schedule profiles file ... set defaults_path". Both name the setting and the file. Useful.
- `csv_path` is now `Option<PathBuf>`; the empty-path sentinel is gone.
- Callers in the engine: `py_config.rs` and `py_dwelling.rs` pass the Python argument through; `gym_env.py:433`, `simulation_plan.py:56`, `frame_golden` (`adapter.rs:183,290`) pass it; `synthetic.rs` (`mod.rs:2114`) builds its schedule from TOML and never reads it; `Fleet::from_resstock` and `benches/common.rs` do not (F1). Docs: `docs/python.md` example passes `defaults_path` and states the rule; the `_hares.pyi` stub has no docstrings, so the Python API itself carries no note (minor).
- Consumer `~/src/HARES_CALIBRATION_SYSTEM`, branch `i08-consumer`. Constructions that would now fail when the schedule lacks a mapped column and no `defaults_path` is given:
  - `calibration/forward_model.py:257,289-290`: `HaresForwardModel(defaults_path=None)` omits the kwarg when unset.
  - `calibration/run_calibration.py:198,210-211`: `--defaults` is optional and passes `args.defaults` (None) through to `HaresForwardModel`.
  - All others pass or inherit it: `Runner.build_kwargs` always sets an absolute `defaults_path` (`engine.py:322,336`), used by `bench_calibration.py:86`, `ami2/cvr_actuation.py:133`, `ami2/tariff.py:85`, `ami2/dr_actuation.py:217` and `tests/suite_ami2.py:813,862,1752`; `hares_ua_xval.py:72`, `hares_l1_archetype_sweep.py:77`, `hares_der_remeasure.py:64`, `scale.py:267,336`, `schema.py:374` and `inject.py:201,251` pass it explicitly.
  - The consumer's `HARES/` is a submodule checkout, not examined.
- Mutations (item 5) confirm the test pins both the message and the no-guess rule.

### 4. L-1 to L-7: each correct and tested
- L-1 (`3c279795`): stale "non-critical init skip" text reworded at the 5 sites; test renamed. Included in the 452 tests below, all pass.
- L-2 (`c71a360b`): warning only when the column exists and the zone rule turns it away (the existence check now precedes the gates). Both tests fail when the push is removed (mutation).
- L-3 (`ea4f12e8`): `Option<LoopId>`, `declared_fluid_port`, `require_loop_id` and `bound_loop_id` share code between both boilers; the step cost is one `Option` match, no allocation on the success path. Read in full; not mutated (the write-up says both tests failed at the parent).
- L-4 (`c425a2f6`): precedence (ACH50, then CFM50, then ELA) and the volume condition (constant ACH is none and a leakage input exists) are identical to the old code. The constant-ACH addition to the test would pass on the parent too; it pins the new property, not a regression.
- L-5 (`e62afddf`): 17 `type: ignore` removed from `tests/` and `scripts/`; `grep` finds none left outside `notebooks/`. No test weakened: the wrong-type cases route through `Any` and assert the same `TypeError` or `RuntimeError`; the HTTP tests keep status code and `response` shape (`SimpleNamespace` instance instead of a class, same attribute); the frozen-dataclass check is stricter (`FrozenInstanceError` instead of `Exception`) and the removed class was an exact duplicate of it. Tests in the touched files: 210 passed, 3 xfailed.
- L-6 (`9e3fc147`): default collection `1/2 tests collected (1 deselected)`; the full block (`-m ""`) runs it and passes; CI's `-m "slow or ochre"` job picks it up (`ci.yml:280`).
- L-7: `serde_path_to_error` is in `[workspace.dependencies]`; duplicate row error (`04395092`): the 4 `N/A` rows in the shipped CSV are skipped before the keyed insert, and the loader tests pass on the shipped file. Heater: see F3 (correct; claim overstated). Loop allocator (`d1ca7a0d`): `DHW_DEMAND_LOOP = LoopId(u16::MAX - 1)` (`water_heater/mod.rs:80`), so the counter stops at 65533. No corpus config is rejected: `[test-debug]` runs the whole Rust corpus and passes, and `frame_golden` builds 7 dwellings identically.

### 5. Mutations: each fails a test
Run in a scratch worktree of `cd1b5972`, restored after each.

| Mutation | Result |
|---|---|
| M-4: reverse-apply the `schedule_resolve.rs` hunks of `215398f3` | `schedule_path_contract::schedule_file_missing_a_column_needs_a_defaults_directory` FAIL (6 other tests pass) |
| M-4: reintroduce a cwd guess (`defaults_path.or("../../defaults")` at `mod.rs:2369`, which resolves from the test's working directory) | same test FAIL |
| `e113a61f`: constructor reads `typed().ok()` for FUEL again, init `set(FUEL)` removed | `fuel_capability_follows_the_config_init_applies` FAIL; the 4 dwelling-level backup-fuel tests still pass (F3) |
| Loop allocator: malformed param id read as absent | `malformed_loop_id_in_params_is_an_error` FAIL |
| Loop allocator: counter saturates instead of stopping below the reserved id | `exhausted_loop_ids_are_an_error` FAIL |
| Loop allocator: unreadable typed config skipped (in `allocated_loop_ids` only, then also in `replace_typed`) | with only the scan mutated the test still passes because `replace_typed` raises; with both mutated `unreadable_typed_config_is_an_error` FAIL |
| `c71a360b`: no warning pushed | both lighting warning tests FAIL |
| `04395092`: duplicate row overwrites | `load_default_profiles_rejects_a_duplicate_profile_row` FAIL |
| `6a887993`: `typing.TypeIs` restored | pyright 5 errors |

### 6. Full `plan.md` section 3 block on the tip
Block text taken from `plan.md` lines 1976-2018, run per line; `CARGO_TARGET_DIR` and `TMPDIR` in the scratch directory. Tools: rustc 1.94.1, cargo-nextest 0.9.137, the same as the write-up's.

| Line | Result |
|---|---|
| `[tools]`, `[fmt]` | pass |
| `[clippy]`, `-default`, `-dst`, `-profiling`, `-observe`, `-observe-profiling`, `-release`, `-release-dst`, `-release-observe` | all 9 pass |
| `[test-debug]` | 7358 passed, 0 skipped |
| `[test-observe]` | 7416 passed |
| `[test-dst]` | 7371 passed |
| `[test-profiling]` | 7364 passed |
| `[test-observe-profiling]` | 1466 passed |
| `[test-release]` | 7315 passed |
| `[bench]`, `[advisories]` | pass |
| `[golden]` | 6 of 6 and 1 of 1 identical |
| `[python-build]` | pass |
| `[pytest]` | 1419 passed, 4 xfailed; skip census diff empty |
| `[stray]` new paths | none |
| `[writing]` | pass on the tip commit |
| `[stray]` TMPDIR | FAIL, pre-existing leaks only (below) |

All counts equal the write-up's. Run notes: the scratch worktree needed the two submodules (`vendors/OCHRE`, `vendors/EnergyPlus`) initialised before the Rust tests ran (a first `[test-debug]` attempt without them failed on missing OCHRE fixtures; discarded). My first `[pytest]` attempt died when the shared disk filled (the pytest `tmp_path` root alone was 18 GB); the counts above are from the clean re-run.

TMPDIR leaks after the full block, by class: 6096 `hares-regr-output-*.csv`, 203 `hares-fleet-*.csv`, 150 `hares-test-batch-step-*.toml`, 57 `hares-fleet-*.epw`, 18 `hares-core-engine-*.csv`, about 90 `pv_lut_*` files, 5 `hares-fleet-*.hpxml_dir`, and about 58 `tmp*` directories from Python tests. Same classes as the write-up.

Proof that none come from this branch's tests, each run with an empty TMPDIR:
- Rust: 452 tests (the binaries `schedule_path_contract`, `equipment_identity_boundaries`, `equipment_id_regressions`, plus every unit test in the changed modules `loop_allocator`, `heat_pump::heater`, `hvac::boiler`, `schedule_resolve`, `solver_builder`, `dwelling::conversions`, `hpxml::equipment`, `equipment::config`, and the new L-2, L-4, L-7 tests): 452 passed, TMPDIR entries afterwards: 0.
- Python: the 8 changed test files (210 passed, 3 xfailed): the only leftover is one `tmp*` directory holding `hares_thermal_trace.csv` and `hares_thermal_trace_diagnostics.csv`, from `tests/python/test_thermal_trace.py:272` (`tempfile.mkdtemp()`). The branch's two changes to that file are `type: ignore` removals; the leak line is untouched.

### 7. Shipped text: pass
`ids`/`writing` definitions from `plan.md`, extended with the en dash, run over the message and added lines (excluding fixtures and lockfiles) of all 20 commits: no hits. All 20 commits are authored and committed by Rich Evans; no trailers.

### 8. Fold readiness
- Against `feat/output-results` (still `db5efe80`, the merge base): fast-forward.
- Against `fix-headaudit` (`4061da95`, 10 commits on `db5efe80` in `git log`): `git merge-tree` merges cleanly (tree `9a15d061`); `6a887993` and `4061da95` have the same patch-id, so a rebase drops the duplicate. I did not build or test the merged tree.
- Against `fix-testhyg` (`c1aca232`, 29 commits on `db5efe80`): textual conflicts in 6 files: `python/ochre_next/data/resstock.py`, `python/ochre_next/helics/_types.py`, `tests/python/bench_4month_der.py`, `tests/python/test_helics_federate.py`, `tests/python/test_pvlib_generator.py`, `tests/python/test_resstock_data.py`. All are import-line or adjacent-hunk conflicts. Resolution must keep this branch's `from typing_extensions import TypeIs` (testhyg still imports `TypeIs` from `typing`, which breaks on 3.12), drop the `# type: ignore` that testhyg's rewritten `bench_4month_der._run_hares` and `test_helics_federate` hunks keep, and keep both sides' new imports (`weakref`, `offline_guard`, `pytest`). Rust files, including `schedule_path_contract.rs`, auto-merge. Not verified: whether any test testhyg adds or rewrites builds a dwelling without `defaults_path` and so trips the M-4 contract; that needs a build of the merged tree. Suggested order: fold `fix-headaudit` and this branch first (clean), then rebase `fix-testhyg` once and re-run pyright at 3.12 plus the block.

## Cleanup
Scratch worktree, target directory (about 25 GB) and TMPDIR (about 19 GB) deleted; `git worktree list` has no review entry. The branch worktree `.workstrees/fix-headbreaks` is untouched (clean, `cd1b5972`). Left in the scratch directory: my driver scripts and logs (`block*.sh`, `block*.log`, `calls.txt`, `m4.patch`).
