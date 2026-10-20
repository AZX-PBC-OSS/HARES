# Fix: head audit of OD-13 and BR-22, and the BR-21 fold-integrity lows

> Rebased onto `2f85338d` (clean, 15 commits) and extended with four
> commits for `verification/review-headaudit-7a3cf56c.md`; see "Review
> round" at the end. Commit hashes in the first table are pre-rebase.

Sources: `verification/audit-OD-13-BR-22-head.md`,
`verification/fold-integrity-BR-21.md`. Worktree `.workstrees/fix-headaudit`,
branch `fix-headaudit` from `249912f9` (not merged). CARGO_TARGET_DIR
`target-fix-headaudit` (deleted at the end). 15 commits, `6a499504..7a3cf56c`.

## Findings, fixes, commits, tests

| Finding | Fix | Commit | Test / evidence |
|---|---|---|---|
| M1 OCHRE tests run only on dispatch; fleet tests wrongly ochre-marked | New CI job `ochre-python-test` (push, PR, dispatch; submodule, ochre group, release build) runs `pytest -m ochre`; slow job keeps `slow and not ochre` on dispatch. Thermal-trace module now carries `pytestmark = ochre` (it needs OCHRE). Six fleet tests lose the marker; conversions module imports directly (no skip paths). Matrix artifact names made unique (two jobs uploaded the same name per matrix leg). pyproject marker help updated. | `f31e8282`, `64abd53c` | default selector now runs all 14 conversion tests (was 8); `pytest -m ochre` locally: 50 passed, 4 xfailed |
| M2 Lighting xfail cites a non-existent safeguard | `test_parity_columns_present` (no mark) pins every parity column on both sides; a missing column inside `test_parity` raises `LookupError`, which the `raises=AssertionError` mark does not swallow; comment cites the real test | `7f5f558e` | new unmarked test passes; the mark can no longer absorb a missing column |
| M3 thermal-trace reasons uncited, stale "~0.87" | Root cause found: OCHRE ran **unseeded** (it seeds numpy only with `seed` or `output_path`), so the reference moved run to run (unseeded runs: overnight worst 4.07/4.69/4.35 C, mean 0.860/0.904/0.870 C; 6 h worst 2.34/2.38/2.35 C; OCHRE heating 47.04/47.84/47.35 kWh). Both OCHRE modules now pass `seed=42`; three seeded runs agree to the last bit. Measured values are module records (`TRACE_*`, `OVERNIGHT_*`, `SOLAR_*`), each test checks its measurements against the records within `RECORD_REL_MARGIN = 2%` via `pytest.fail` (not an AssertionError, so a drift FAILS instead of XFAILing), and the xfail reasons are f-strings built from the records, in plain words, no ticket IDs, no unsupported "IAM/SHGC calibration" claim. | `64abd53c`, `7f5f558e` | Setting `OVERNIGHT_MEAN_C = 0.5` temporarily: `test_overnight_losses` reported FAILED (`Failed: overnight mean ... update the record`), not XFAIL; restored |
| M3 ledger owner | **No ledger entry owns the three thermal-trace divergences** (grep of `09-numeric-fixes.md`, `11-other-defects.md`: only the OD-11/OD-13 test-hygiene entries and the 02-measurement record). Reported, not invented; see "Defects with no ledger entry". | none | |
| M3b duplicate OD-34 | Already resolved before this run: the Lighting entry is `## OD-47` (`11-other-defects.md:2619`); no shipped text names any ticket ID (`git grep 'OD-34\|OD-47'` outside `.shipwright`: none). | none | grep |
| M4 deprecated `save_versioned` shim | Deleted with its `allow(deprecated)` re-export; module docs name `try_save_versioned` | `6a499504` | `[clippy*]`, `[test-*]` green; no caller existed |
| M5 empty ordering test | Replaced by `nonthermal_equipment_reads_the_previous_steps_zone_temperatures`: per step, the Independent-stage stub's view equals `latest_env()` zone temperatures before the step (bitwise `Vec<f64>` equality), and the run must move the zone temperature so prior-step and integrated values differ (if the stub saw integrated values, the step where the temperature moved fails). The weaker finite/plausible test it subsumes and the stale line-number docs went. | `b17195ff` | 2/2 pass |
| L1 loose tolerances | Every agreeing case matches to rounding (max rel 1.8e-15, Exterior Lighting): tolerance now `AGREEMENT_REL_TOL = 1e-9`; the two zero cases require `|HARES| <= 1e-12 kWh` (was 1e-3). Moving the window was measured and rejected: 06:00 gives heating 8.7%, a 24 h window exposes large divergences (below) that would need ledger entries. The heating regression the audit worried about is now caught by the thermal-trace record check of 48 h heating (39.266 kWh, 2%). | `7f5f558e` | parity 11 pass + 1 xfail |
| L2 entry text vs `write_output=True` | `11-other-defects.md` OD-13 Change text corrected | ledger | |
| L3 skip sites | EV tests assert events exist (seeded); package/core-dependency/committed-input skips replaced by direct imports in `test_py_fleet`, `test_fleet_bindings`, `test_py_steppable_fleet`, `test_py_conversions`, `test_ev_tariff_integration`, `test_py_dwelling_integration`, `test_rl_warning_surfacing` (+ `batch_step` asserted present), `test_gym_env`, `test_ochre_parity` (polars). Remaining skips are only the optional extras (gymnasium, HELICS) and the ochre group, each with that reason. pyarrow stub accepts string arrays (pyright saw real types once the import was static). | `4e7aaff4`, `ee22a105` | `[pytest]` 1413 passed, 4 xfailed, 0 SKIPPED |
| L4 docstring | names the block's build (`maturin develop --release`, no feature) | `64abd53c` | |
| L5 census history not in repo | Not fixable in the repository by design: `.shipwright/` is gitignored planning (public repo, no planning artifacts). The property the census guards is enforced: empty census and zero SKIPPED lines in this run. | none | |
| L6 schedule enum reasons | one-line reason at each site (`value_at` reads the profile arrays every step; config mirrors runtime so `into_runtime` moves without allocating) | `40e5ff99` | |
| L7 multi-line reasons | all one line (benches, frame_golden helpers, approx_constant, panic_hook x2, py_dwelling, py_pv_sizing x3) | `40e5ff99` | |
| L8 census | Corrected in `01-baseline-repair.md` BR-22 (32 kept at `8082592e`, names `longwave.rs:626`, acceptance grep now `\b(allow\|expect)\(`). `longwave.rs` allow removed by moving the diagnostic push to its own gated loop (same pairs, same order). `ashrae152.rs` test-only field now `#[cfg(test)]` (allow gone). Kept census now **29**, zero `#[expect(`. | `45510f3e`, `0776f9c1`, ledger | goldens identical |
| L9 expect sites | dead `zone_capacitances_j_k` removed; `scheduler.build()` result iterated (no `unused_must_use`); `record_step_impl` gone (scratch in a `StepScratch` moved out with `mem::take` and back, no allocation); coupled solves take `ScalarSolveTarget` | `260cae7b`, `9f144058`, `13087791` | observe/profiling diagnostics tests, 452 envelope tests |
| L10 warm-up `expect` | single `match`: Err arm returns the typed error, observe logs from the Ok arm | `260cae7b` | all clippy profiles |
| L11 lost height assertion | height derivation extracted to `garage_wall_height_m`; test asserts it | `edb812b5` | 21 garage tests |
| L12 doc link | `StratifiedTank::step_tempered`, and a second broken one in the same file (`Self::step_tempered`) | `40e5ff99` | rustdoc reports no non-unit broken link in `tank.rs` |
| L13 stale comments | thermostat names the telemetry flag; EV constructor rationale replaced; scheduler "previously hard-coded" history removed | `40e5ff99`, `260cae7b` | |
| L14 PvRoofTuning override | `size_pv_from_dwelling` takes `PvRoofSiting` (no panel fields), so the override is impossible by construction; the panel spec comes only from the system tuning | `7a3cf56c` | existing `size_pv_from_dwelling_respects_panel_overrides`, `size_pv_inverter_clamp_through_binding` |
| L15 MSRV | **Finding is wrong.** `<uN>::is_multiple_of` is `#[stable(feature = "unsigned_is_multiple_of", since = "1.87.0")]` (local std source `core/src/num/uint_macros.rs`; Rust 1.87.0 release notes, releases.rs/docs/1.87.0). `rust-version = "1.89"` is sufficient; no change. | none | |
| BR-21 fold L1 | `[test-observe]` ran (7329 passed), which compiles and runs the observe-gated `conditioned_oracle` items | block | |
| BR-21 fold L2 | Fold note added to `01-baseline-repair.md` BR-21 (base already carried both hunks; `[clippy-release-observe]` is the evidence) | ledger | |

TDD note: M2, M3, M5, L1, L3, L11 are test changes; M3's drift check was shown
to fail on a wrong record. The Rust refactors (M4, L8 to L10, L14) are
behaviour-neutral by intent; the goldens prove it.

## Verification block (plan.md section 3), on `7a3cf56c`

Env: `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_{DEV,TEST}_DEBUG=line-tables-only`
(disk at 91 to 97%; no semantic effect). Census read by absolute path (the
worktree has no `.shipwright`). rustc 1.94.1.

| Line | Result |
|---|---|
| `[fmt]` | exit 0 |
| nine `[clippy*]` | all exit 0 |
| `[test-debug]` | 7271 passed, 0 skipped |
| `[test-observe]` | 7329 passed |
| `[test-dst]` | 7284 passed |
| `[test-profiling]` | 7277 passed |
| `[test-observe-profiling]` | 1402 passed |
| `[test-release]` | 7228 passed |
| `[bench]` | compiled |
| `[advisories]` | ok |
| `[golden]` | 6/6 identical; `-F dst` 1/1 identical |
| `[python-build]` | ok |
| `[pytest]` | 1413 passed, 4 xfailed, 0 SKIPPED; census diff empty |
| `[stray]` tree check | no new path |
| `[stray]` TMPDIR check | **FAILS, pre-existing**: ~6,800 files left by the regression, fleet, batch-step, PV-LUT and engine Rust tests and Python `tempfile.mkdtemp` users. This is exactly BR-24 (row 21l, "in implementation on branch `fix-testhyg`"), not landed at `249912f9`; no commit here adds a temp-file writer. |
| `[writing]` | all 15 commits ok |
| parity + thermal trace `-rxX` | parity 12 passed (11 cases + presence) + 1 XFAIL (Lighting); thermal trace 1 passed + 3 XFAIL (reasons below) |
| pyright | 0 errors; ruff: no new finding on any changed file versus `249912f9` |

XFAIL reasons as shipped (seeded, recorded):
- 48 h: worst first-6 h step 2.41 C (0.1), 48 h mean 0.82 C (0.5), heating 39.3 kWh vs OCHRE 47.1 kWh; first component past 5%: infiltration heat gain at step 1.
- overnight: up to 4.72 C (0.3), mean 0.88 C.
- solar peak: mean ratio 1.305, all 36 sampled steps outside 5%, worst 31.5%.

## Defects with no ledger entry (reported, not invented)

1. The three thermal-trace divergences above (zone temperature, overnight,
   window solar +30%), plus 48 h heating 16.5% low. The only record is
   `02-measurement.md` (OD-33 + OD-11), which notes infiltration as the
   first divergent input.
2. Measured while assessing L1 (seeded, `target/measure_window.py`): over 24 h
   from 2019-05-05 00:00 HARES vs OCHRE: Water Heating 14.76 vs 1.01 kWh
   (14.6x), HVAC Cooling 1.94 vs 0.95 kWh (2x), HVAC Heating 24.08 vs 31.00
   kWh (-22%), Other 8.77 vs 7.41 kWh (+18%), Total 52.2 vs 43.1 kWh (+21%).
   The one-hour 13:00 parity window hides all of these. Owners may be B-16/
   B-17 (wet-appliance hot water) for part of Water Heating; not verified.
3. Residual: `cargo doc` reports about 8 non-unit broken intra-doc links in
   `hares-equipment` alone (e.g. `ResolvedZip`, `Dwelling::add_equipment`)
   and many `[W]`/`[J]` unit-bracket warnings workspace-wide; `cargo doc` is
   not a block line. `python/ochre_next/rl/vec_env.py` keeps a pure-Python
   fallback for a `batch_step` the extension always registers.

## Review round (`review-headaudit-7a3cf56c.md`, CHANGES-REQUIRED)

Rebased `249912f9..7a3cf56c` onto `2f85338d` without conflicts (new hashes
`ee12383c..5f31d549`). Four commits added; tip `ea2a48df`.

| Finding | Fix | Commit | Test / evidence |
|---|---|---|---|
| R1 untriaged xfails | Each thermal-trace reason quotes its record and ends "The divergence is not yet triaged to a cause." No class is claimed. Ledger owner: OD-50 (untriaged); its step-1 infiltration sentence is now labelled an unchecked lead from an unseeded run. | `24e5432c` | `-m ochre -rxX` prints the three reasons as shipped |
| R4 unchecked infiltration claim | Sentence removed from the reason (no seeded measurement or test backs it) | `24e5432c` | |
| R3 margin edge | `tests/python/ochre_records.py`: holds while `abs(measured - recorded) <= 0.02 * abs(recorded)`, edge included, stated in the docstring; `check_record` shared | `24e5432c` | `test_ochre_records.py`: exactly ±2% holds, ±2.0002% fails, for positive and negative records (6 tests, unmarked, run in every job) |
| R2 job green on skips | `importorskip` gone from the three ochre modules (plain `import ochre`, `from ochre.utils ...`, xmltodict via the generator import). Directory collection leaves those modules out unless the `-m` expression can select an ochre test (`selects_ochre`), so the default job's selector (`not slow and not ochre`) does not import them. That is not "never": a module named on the command line is always collected, and a selector such as `-m 'not slow'` or `-m ""` selects ochre tests and collects them; without the ochre group both fail with a collection error (corrected in the second review round). New `--fail-on-skip` (`tests/python/skip_gate.py`) fails a run that skips any test or module, including a run whose only module skips (exit 5 turned into 1); the CI OCHRE job passes it. | `17d721df` | `test_skip_gate.py` (11 tests: selector cases, skip in a test, skip at module level, all-modules-skipped, clean run). Broken-import proof: a scratch plugin setting `sys.modules["ochre"] = None`; `pytest -m ochre --fail-on-skip` → `ModuleNotFoundError` in `test_thermal_trace.py` and `test_ochre_parity.py`, "Interrupted: 2 errors during collection", exit 2; the default selector with the same break collects 1345 tests cleanly |
| R5 grep prints 30 | Acceptance grep `'^\s*#!?\[.*\b(allow|expect)\('` (attribute lines only) prints exactly 29; BR-22 acceptance text updated | ledger | `git grep ... | wc -l` = 29 |
| R6 untested moves | `record_from_state_returns_the_same_scratch_buffers_with_the_steps_values` (observe): buffer pointers and capacities unchanged over two steps, `zone_temps` holds the step's values, freezing counters `[2, 1]`. `interior_surface_diagnostic_pairs_follow_surface_order` (debug): the diagnostic equals the zipped (`interior_surface_temps[0]`, `lwr_net_flux_buf`) pairs in surface order, the warm surface first and losing flux | `ac9703f0` | both pass |
| R7 en dash | `// Steps 1e to 1f:` | `ea2a48df` | `[writing]`; no en dash in any added `.rs` line |

### Block on `ea2a48df` (same env and script as above; census by absolute path)

| Line | Result |
|---|---|
| `[fmt]`, nine `[clippy*]` | all exit 0 |
| `[test-debug]` | 7326 passed, 0 skipped |
| `[test-observe]` | 7385 passed |
| `[test-dst]` | 7339 passed |
| `[test-profiling]` | 7332 passed |
| `[test-observe-profiling]` | 1454 passed |
| `[test-release]` | 7282 passed |
| `[bench]`, `[advisories]` | ok |
| `[golden]` | 6/6 identical; `-F dst` 1/1 identical |
| `[python-build]` | ok |
| `[pytest]` | 1435 passed, 4 xfailed, 0 SKIPPED; census diff empty |
| `[stray]` tree | no new path |
| `[stray]` TMPDIR | 6674 entries: pre-existing BR-24 (not landed at `2f85338d`); no commit here writes temp files |
| `[writing]` | 19/19 ok |
| CI OCHRE job command (`-m ochre -rxX --fail-on-skip`) | 50 passed, 4 xfailed, exit 0 |
| ruff / pyright on changed Python files | new files clean; the two remaining ruff findings in `test_ashrae_reference.py` (I001, RUF100) exist at `2f85338d` |

Zone-ordering mutation: with the operator (not attempted here). OR-19
roster mutation: `verification/mutation-OR-19-roster-plan.md`.

## Second review round (`review-headaudit-ea2a48df.md`, SHIP with 4 lows)

Not rebased yet: `fix-testhyg` folds first (it clears the `[stray]`
TMPDIR line). Four commits on `ea2a48df`; tip `4061da95`.

| Finding | Fix | Commit | Test / evidence |
|---|---|---|---|
| 1 "never imports OCHRE" overstated | The conftest comment states the rule exactly. Directory collection leaves the three modules out unless `-m` can select an ochre test (alone or with slow). A module named on the command line is always collected, because pytest does not consult `pytest_ignore_collect` for its own arguments. A collected module imports OCHRE, so a run without the group errors and never skips. The R2 row above is corrected. | `284cb2a4` | With `ochre` and `xmltodict` both set to `None` by a scratch plugin: `-m ochre` gives 3 collection errors; the default selector collects 1368 tests with no error; `-m 'not slow'` gives 3 errors; the module path alone gives 1 error. |
| 2 zero / NaN / inf unpinned | `record_holds` now rejects any non-finite value. Before, an infinite record held every finite measurement (`abs(1 - inf) <= inf`), and the new test caught that. A zero record holds only an exact zero (±0.0); 1e-300 fails. | `66c03052` | `test_ochre_records.py`: 1 zero case and 8 NaN/inf cases, plus the ±2% edge cases. |
| 3 private API, terminal-reporter dependency | `_pytest.mark.expression` is removed. `selects_ochre` parses the expression with `ast` and evaluates plain marker logic (names, and/or/not, parentheses). Any other expression (marker kwargs, a syntax error that pytest itself reports) counts as selecting, so the modules are collected and fail loudly. `--fail-on-skip` is now a `SkipGate` plugin registered on the controller only (not on xdist workers). It counts skips from `pytest_runtest_logreport` and `pytest_collectreport`, excluding xfails (`wasxfail`). It writes its message through the terminal reporter when one exists, otherwise to stderr, and fails on exit OK or NO_TESTS_COLLECTED. | `284cb2a4` | `test_skip_gate.py`, 25 tests (`test_ochre_records.py` 15): 12 selector cases including `not (slow or ochre)`, `not slow`, kwargs and a syntax error; skip in a test and at module level × serial, xdist `-n 2` and `-p no:terminal`; pass and xfail × the same three modes; a run whose only module skips. Before the fix, the two `no-terminal` skip cases and the inf record case failed. |
| (review note) scratch test covered 3 of 5 buffers | The scratch-move test gives the accumulator one equipment slot and checks all five buffers' pointers and capacities. | `77084ea8` | passes (observe) |
| 4 records under Python 3.12 | A 3.12 environment (`UV_PROJECT_ENVIRONMENT=.venv312`, `uv sync --python 3.12 --locked --all-extras --all-groups`, release build of the extension) was set up. On the first run, every record held (`-m ochre`: 50 passed, 4 xfailed, reasons identical to 3.13), but 30 collection errors appeared from a **pre-existing defect in the head**: `typing.TypeIs` is Python 3.13+, yet `python/ochre_next/data/resstock.py` and `helics/_types.py` import it (introduced in `787487df`). On 3.12 that broke 4 test modules and would fail CI's 3.12 legs. It is fixed by importing `TypeIs` from `typing_extensions`, already a dependency. | `4061da95` | **3.12.7**: OCHRE job `-m ochre -rxX --fail-on-skip` 50 passed, 4 xfailed; full `-m "" --fail-on-skip` 1458 passed, 4 xfailed, 0 skipped in 3 runs (one earlier 3.12 run reported 21 errors that did not reproduce in three reruns; its output was truncated, so the cause is not known). **3.13.14**: OCHRE job 50 passed, 4 xfailed. Records and gate tests 40/40 on both. Pyright on `python/ochre_next`: 0 errors. |

The full block runs after the rebase onto the head that includes `fix-testhyg`.
