# Fix: audit OR-19 fold and operator findings, on head db5efe80

Branch `fix-headbreaks`, worktree `.workstrees/fix-headbreaks`, from
`feat/output-results` at `db5efe80`. Tip `cd1b5972` (20 commits).
CARGO_TARGET_DIR `target-fix-headbreaks`. Not merged.

Source audit: `audit-OR-19-fold-and-operator-1b708ccb.md` (head 1b708ccb).

## What still held at db5efe80

Checked first on the untouched head:

| Check at db5efe80 | Result |
|---|---|
| `cargo check --workspace --all-targets --all-features` | ok (H-2 fixed on the head by `a416b901`) |
| `frame_golden compare-all` | 6 of 6 identical |
| `frame_golden -F dst compare-all` | 1 of 1 identical |
| goldens' provenance | all 7 `git_dirty: true`, `git_head` 1b708ccb (`dc4a456d` re-captured the digest from a dirty tree) |
| `typing.TypeIs` imports | still in `resstock.py`, `helics/_types.py` |

## Findings

| Finding | Held at head | Fix | Commit(s) | Test / proof |
|---|---|---|---|---|
| H-1 goldens fail on the defaults digest | No. `dc4a456d` already re-recorded the digest (5339acb2) for the Ceiling Fan row, but from a dirty tree | Re-captured each golden clean, one commit per golden | `36fd815d`, `40e6d68c`, `244089da`, `e9382a47`, `857a1b21`, `21c6f894`, `cd1b5972` | Script refused to commit unless the only changed lines were `git_head`/`git_dirty`. `git diff db5efe80 cd1b5972 -- tests/fixtures/golden`: 14 lines, all `git_head`/`git_dirty`. The defaults digest and every product digest are unchanged, so the frames are bitwise |
| H-2 hares-io test target does not compile | No (fixed by `a416b901`) | none | none | cargo check ok at head and tip |
| M-1 blueprint bag skips unset optional fields | not touched, per instruction (goes to OD-46) | none | none | none |
| M-2 goldens dirty, `git_head` not in history | Yes (dirty; head now 1b708ccb, which is in history) | Same re-capture as H-1 | as H-1 | Each golden: `git_dirty: false`, `git_head` = the commit's parent, `git merge-base --is-ancestor` ok for all 7 |
| M-3 pyright does not check the 3.12 floor | Yes | Cherry-picked `4061da95` byte-identical (TypeIs from typing_extensions), then `pythonVersion = "3.12"` in `[tool.pyright]` | `6a887993`, `e842828f` | Pyright: 0 errors on the tree. The pre-fix `helics/_types.py` under the new config: `"TypeIs" is unknown import symbol` |
| M-4 no `defaults_path` drops default profiles silently | Yes (behaviour unstated; error read "no 'Refrigerator' profile in no defaults directory is configured") | Contract: the profiles come only from the directory the config names, never a working-directory guess (OD-49's direction; the store's own `"defaults"` fallback remains for OD-49). A config with none has no profiles; an equipment that needs one fails naming `defaults_path`. `DefaultProfiles.csv_path` is `Option` (was an empty-path sentinel). Field doc on `DwellingConfig.defaults_path`; `docs/python.md` example passes `defaults_path` and states the rule. Other callers already pass it (notebook, `simulation_plan.py`, `gym_env.py`, `diag_pv_trace.py`) | `215398f3` | `schedule_path_contract::schedule_file_missing_a_column_needs_a_defaults_directory`: base.xml + BEopt schedule fails naming `defaults_path` without one, builds with the shipped one. Failed at parent (message lacked `defaults_path`) |
| L-1 stale "non-critical init skip" text | Yes, all 5 sites + test name | Reworded; test renamed `non_identity_init_failure_is_fatal_after_the_identity_checks` | `3c279795` | The three touched tests pass |
| L-2 garage lighting column dropped with no trace | Yes (basement column the same) | OD-57's rule: OCHRE zone rule kept, each unread `lighting_garage` / `lighting_basement` column adds a warning to the dwelling's warning list naming the column and the rule | `c71a360b` | `garage_lighting_column_without_a_garage_is_a_warning`, `basement_lighting_column_without_finished_basement_is_a_warning`; both failed at parent |
| L-3 boiler `unwrap_or(LoopId(0))` | Yes | Loop id is `Option<LoopId>`, bound by init; pre-init fluid port has no loop; a step with no bound loop is an error naming the boiler; shared init error helper | `ea4f12e8` | `boiler_without_loop_id_declares_no_placeholder_loop`, `boiler_stepped_with_no_loop_bound_is_an_error`; both failed at parent (the second checked in a scratch worktree of `c71a360b`) |
| L-4 `unwrap_or(0.0)` volume sentinel | Yes | ACH50 and volume resolved together as one `Option<(f64, f64)>` only when a blower-door input exists and leakage is not constant ACH; error built from `HpxmlError::MissingField` | `c425a2f6` | `infiltration_without_conditioned_volume_is_an_error` extended: a constant-ACH building without a volume builds. Goldens bitwise |
| L-5 17 `type: ignore` (16 + `699cfad2`) | Yes | All 17 removed: 7 unneeded (imports and polars row); wrong-type tests route the value through `Any`; HTTP status tests use a small exception class; the frozen-dataclass check now expects `FrozenInstanceError` and its duplicate class is gone | `e62afddf` | Pyright 0 errors at 3.12; no changed line gains a ruff finding; 207 tests in the touched files pass |
| L-6 pvlib test unguarded | Yes | `@pytest.mark.ochre` plus an assert that pvlib is importable, naming the group | `9e3fc147` | Default selection: 1 deselected (was 2 collected). `-m ""`: 2 passed. Never a skip |
| L-7 `serde_path_to_error` outside workspace deps | Yes | Moved to `[workspace.dependencies]` | `4223d8c8` | `cargo check --locked` ok, lockfile unchanged |
| L-7 heater `typed().ok()` | Yes, and a real bug | FUEL capability set in init from the strictly read backup fuel; constructor no longer reads it. Assembly re-inits with the override-merged config, so the capability was stale when an override changed the backup fuel | `e113a61f` | `fuel_capability_follows_the_config_init_applies`; failed at parent |
| L-7 loop allocator | Yes | `require_typed` instead of `.ok()?`; a malformed parameter id is an error naming key and value; u32 counter stops below the reserved DHW demand loop id, never repeats | `d1ca7a0d` | `malformed_loop_id_in_params_is_an_error`, `unreadable_typed_config_is_an_error`, `exhausted_loop_ids_are_an_error`; all 3 failed at parent |
| L-7 duplicate defaults profile row | Yes | Second row for a profile and element is an error naming both lines; shipped file has none | `04395092` | `load_default_profiles_rejects_a_duplicate_profile_row`; failed at parent |
| A3 stale "prediction, not a run" line in `fold-or19d.md` | Yes | Line now points to `mutation-OR-19-roster-plan.md` (planning file, not committed) | none | none |

## Seen, not fixed (ledger candidates, outside the audit's list)

- `generator.rs` `Generator::new`: `config.typed().ok()` with `fuel_type.unwrap_or(FuelType::Gas)`: the same swallow as the heater's.
- `solver_builder.rs`: a conditioned zone with no constant ACH and no blower-door input runs `InfiltrationMethod::Ach { ach: 0.0 }`, a silent zero infiltration.
- `schedule_resolve.rs` `inject_default_setpoint_profile` / `check_hvac_setpoint_invariants`: HVAC with no setpoint column, no HPXML setpoints and no defaults directory runs on static setpoints with only a tracing warning; `thermostat.rs` `value_at(env).ok()` swallows source errors.
- `boiler.rs` `loop_return_temp_c` decodes the fluid payload with `.ok()?` every step.
- `water_heater/{gas,resistance,heat_pump_wh}.rs`: `loop_id_from_config(..).unwrap_or_default()`, the boiler placeholder pattern.
- `notebooks/simulation_explorer.py:528` keeps one `type: ignore`; the notebooks are not in pyright's include.

## Verification block on the tip cd1b5972

Every line, each under `bash -euo pipefail`, in order; a failing line was recorded and the run went on. Tools: rustc 1.94.1, cargo-nextest 0.9.137, cargo-deny 0.20.2, uv 0.12.8.

| Line | Result |
|---|---|
| `[tools]` | pass |
| `[fmt]` | pass |
| `[clippy]`, `[clippy-default]`, `[clippy-dst]`, `[clippy-profiling]`, `[clippy-observe]`, `[clippy-observe-profiling]`, `[clippy-release]`, `[clippy-release-dst]`, `[clippy-release-observe]` | all pass |
| `[test-debug]` | 7358 passed, 0 skipped |
| `[test-observe]` | 7416 passed |
| `[test-dst]` | 7371 passed |
| `[test-profiling]` | 7364 passed |
| `[test-observe-profiling]` | 1466 passed |
| `[test-release]` | 7315 passed |
| `[bench]` | pass |
| `[advisories]` | pass |
| `[golden]` default | 6 of 6 identical |
| `[golden]` `-F dst` | 1 of 1 identical |
| `[python-build]` | pass |
| `[pytest]` | 1419 passed, 4 xfailed; skip census diff empty |
| `[stray]` new paths | none |
| `[stray]` TMPDIR | FAIL: temp files left by existing tests (6096 `hares-regr-output-*.csv`, 203 `hares-fleet-*.csv`, 57 `.epw`, 150 `hares-test-batch-step-*.toml`, the `pv_lut*` set, 18 `hares-core-engine*.csv`, about 60 `tmp*` dirs). None come from tests this branch adds. This is the leak class the `fix-testhyg` branch fixes ("write every test output under a per-test temporary directory"); the line is not weakened |
| `[writing]` | pass |

`[writing]` was also run over all 20 branch commits (messages and added lines, em and en dash, the `ids` pattern): no hits.
