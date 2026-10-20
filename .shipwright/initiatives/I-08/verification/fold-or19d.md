# OR-19 fold, take d: fold-or19d

Branch `fold-or19d`, worktree `.workstrees/fold-or19d`, tip `2f85338d`, on `13881b48` (head of `feat/output-results`, "a schedule path that does not exist is an error, not a generated schedule"). Source: `fold-or19c` (`8082592e..fe5fdfeb`, 7 commits).

## Commits

| New | Source | Subject |
|---|---|---|
| e5b9ee42 | edb83678 | reject equipment and actor changes without leaving the dwelling half changed |
| b6a6ae68 | 69df08ef | assemble through the roster plan and keep rejected changes from consuming equipment ids |
| 5929ceb6 | 7040ca09 | fail construction and roster changes when the output recorder cannot be created, ... |
| c9361105 | 1735024a | keep EV driver streams distinct across checkpoints and tariff changes, ... |
| 54de33d2 | 425d9bf8 | carry battery manager state across tariff rebuilds, ... |
| 69de950a | 444f4c6e | carry the EV driver's gate latches and health failure across rebuilds and checkpoints, ... |
| 456df878 | fe5fdfeb | name the EV driver schema gate test for its behaviour |
| 17bd7291 | new | a roster add that cannot be wired is rejected and leaves the dwelling untouched |
| 2f85338d | new | pass the cz2a fixture schedule to the Python constructor as an optional path |

Messages of the seven source commits are verbatim; the two that had conflicts carry a "Resolution against the typed-config override work" paragraph.

## Rebase onto 13881b48 and conflicts

The rebase of the seven commits from `249912f9` to `13881b48` had no textual conflicts. It had one semantic conflict: the head made `DwellingConfig::schedule_path` and the Python constructor's schedule argument optional. Tests that the fold adds or touches still built the bare path and failed to compile. Fixed in place, folded into the commit that owns each file:
- `roster_plan_schema_parity.rs` (b6a6ae68), `roster_recorder_failure.rs` (5929ceb6), `checkpoint_file_resume.rs` (54de33d2): `schedule_path: Some(...)`.

Separately, a defect already on the head: `crates/hares-python/src/py_dwelling.rs` test `cz2a fixture must load` passes a bare `String` for the now-optional schedule, so `cargo clippy --all-targets` (and the hares-python lib test) does not compile at `13881b48`. Fixed in its own commit 2f85338d, not folded into an OR-19 commit.

The three conflicts from the earlier base (import lists in `crates/hares-core/src/dwelling/mod.rs`, resolved by keeping both sides' imports; OR-19 builds the output column index in `roster.rs`, the head builds equipment configs through `merged_equipment_config`) are unchanged and replayed cleanly. Why both intents hold: OR-19 does not touch `build_from_blueprint_inner`; roster changes plan through `roster.rs` and install by running the same assembly, so every equipment is still built from `merged_equipment_config`; wiring and construction errors during a roster change surface as a rejected change that leaves the dwelling untouched. The checkpoint version stands at OR-19's 11.

## New test: the roster-plan wiring gap (17bd7291)

`crates/hares-core/tests/roster_add_wiring_failure.rs`. A dwelling holds a water boiler on loop 7. Adding a glycol boiler on loop 7 fails in the roster plan (`plan_loop_types`, conflicting fluid types), after the entrance validation has assigned the candidate an id. The test asserts the result is `HaresError::RejectedEquipment` with an `Envelope` reason, that the roster (names and ids) is unchanged, that `save_checkpoint()` equals the checkpoint taken before the attempt, and that the next accepted add gets the same id as in a reference dwelling that never saw the rejected add. It passes.

The "fails if the plan step is bypassed" proof was run separately: `mutation-OR-19-roster-plan.md` records the mutation (`)?;` to `).unwrap_or_default();` after `plan_loop_types(...)` in `plan_roster_caches`, `crates/hares-core/src/dwelling/roster.rs`) failing `an_unwirable_roster_add_is_rejected_and_leaves_the_dwelling_untouched` at its `expect_err`, and the reverted tree passing it.

The case is a fluid-type conflict, not an autosized boiler or an unwirable named control; those wiring failures happen at resolve time in `loop_allocator.rs` and cannot be reached through a roster add of a built equipment.

## Verification block (plan.md section 3), final tip 2f85338d

rustc 1.94.1, cargo-nextest 0.9.137, cargo-deny 0.20.2, uv 0.12.8. `CARGO_TARGET_DIR=/home/rich/src/HARES/target-fold-or19d` (fresh, deleted at the end). `vendors/OCHRE` and `vendors/EnergyPlus` initialised with `git submodule update --init` in the worktree. Every line ran to completion on this tip.

| Line | Result |
|---|---|
| fmt | pass |
| clippy (all-features, default, dst, profiling, observe, observe+profiling, release, release+dst, release+observe) | all 9 pass |
| test-debug | 7326 passed, 0 skipped |
| test-observe | 7384 passed, 0 skipped |
| test-dst | 7339 passed, 0 skipped |
| test-profiling | 7332 passed, 0 skipped |
| test-observe-profiling (hares-core) | 1454 passed, 0 skipped |
| test-release | 7283 passed, 0 skipped |
| bench `--no-run` | pass |
| advisories | pass |
| golden default, `-F dst`, `-F profiling` | identical, no differences |
| `make check` | pass |
| python-build | pass |
| pytest `-m ""` | 1417 passed, 4 xfailed, 0 skipped |
| pytest skip census | empty both sides, diff clean |
| stray, new paths outside target | none |
| stray, TMPDIR left empty | FAIL (see below) |
| writing, all commits above `249912f9` | pass |

Physics: `make check-ashrae-reference` pass; hares-core bestest/parity/conditioned/freefloat/structural/envelope oracles 25 passed; conditioned and freefloat oracles under `-F observe` 12 passed; hares-physics `physics_validation_tests` and `solar_parity` 53 passed; `test_ochre_parity.py` and `test_thermal_trace.py` 12 passed, 4 xfailed (known thermal-trace xfails).

### [stray] TMPDIR line: failing, from earlier work

The line fails because tests leave files in the block's TMPDIR. None comes from a commit of this fold. Prefixes leaked, for `fix-testhyg` (tip `6c2e34d5`) to confirm it covers each:
- `hares-regr-output-*.csv` (about 6000)
- `hares-fleet-*.csv`, `hares-fleet-*.epw`, `hares-fleet-*.hpxml_dir`
- `hares-test-batchstep-*.toml`
- `hares-core-engine-*.csv`
- `pv_lut_*.parquet` and `pv_lut_*.csv` (names: `pv_lut_N`, `pv_lut_oracle`, `pv_lut_no_meta`, `pv_lut_nn_telemetry`, `pv_lut_before_losses`, `pv_lut_t*_{parity,soiling_dc,read_meta,nondefault,match,diff,custom,csv,legacy}`)
- `hares-autosized-boiler-indirect-tank.xml`, `hares-unwired-indirect-tank.xml`
- `tmp*` directories and files created by Python's `tempfile` (about 57)
- `uv-*.lock` (excluded by the line itself)

The line is not weakened or altered.
