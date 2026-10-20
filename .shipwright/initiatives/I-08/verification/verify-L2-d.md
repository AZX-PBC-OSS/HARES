# Verification L2-d: consumer, numeric fixes, schedule

Scope: `ledger/08-consumer.md`, `ledger/09-numeric-fixes.md`, `ledger/00-schedule.md`
(the whole schedule), plus the verify-L1-d findings that `L1-resolution-d.md` marks fixed.
Checked against HARES `ebdb1a17` (the worktree HEAD), the consumer at `c71c542` (gitlink
`0ec757cb`, objects in the consumer's `HARES` submodule), the vendored EnergyPlus tree at
`vendors/EnergyPlus` (`f7c103167a`), and the published EnergyPlus 24.1 pages on
bigladdersoftware.com ("Weather Data Hourly Interpolation", "Climate Calculations").

Consumer mechanics were exercised in scratch copies only (`git archive` of `c71c542` and
of `0ec757cb`, fresh `.venv`, scratch `CARGO_TARGET_DIR`). Nothing in either real repository
or venv was touched.

Counts: 0 blockers, 6 majors, 16 minors.

## Findings

### Majors

**1. [major] CN-0 / "DE-trace capture": `compare` fails every B0 check because the header differs by construction.**
Evidence: CN-0 writes a header of engine commit, `paths.summary()` (which holds
`hares_repo`, `engine_so` and `engine_sha256`, `paths.py:188-199`), consumer commit,
`uv.lock` sha256, `fleet_workers`, `workers` and host name. `compare <a> <b>` "exits 1 on
any difference, printing each differing field path", and the only named rule is CN-6's.
Every B0 check compares a capture on a different engine commit, a different worktree path
and a different `.so` hash against B0. From BR-6d on the `uv.lock` hash also differs, and
from CN-5 on `workers` differs. As written, no B0 check and no CN gate can pass.
Correction: in CN-0, `compare` compares only the per-premise payload (`premises[*]`, every
field listed). The header is recorded and printed side by side but never compared. Add an
acceptance case, `compare_ignores_the_header`: two payloads that differ only in every
header field make `compare` exit 0. Do the same for CN-15's "a DE-trace capture equals B1".

**2. [major] Consumer gate: `run_all > gate.txt` in the consumer root makes `build_bundle.py` report UNTRACKED, so the gate cannot pass.**
Evidence: the gate runs from `~/src/HARES_CALIBRATION_SYSTEM`
(`HARES_REPO=<worktree> uv run --no-sync python -m calibration.tests.run_all > gate.txt`).
`build_bundle.walk_files()` skips only `IGNORE_DIRS` (`__pycache__ .claude .git .venv
.ruff_cache .pytest_cache reference scratch results HARES`, `build_bundle.py:168-169`), and
`.gitignore` does not list `gate.txt`. So `verify()` lists `gate.txt` as UNTRACKED, which
breaks the gate's own "no UNTRACKED file" rule. The file would also be zipped into the
bundle. CN-0 and B0 `capture --out <file>` have the same problem wherever the file is not
under `baselines/`. `build_bundle.py` also writes
`~/src/HARES_CALIBRATION_SYSTEM_BUNDLE_<date>.zip` beside the repository on every gate run
(`build_bundle.py:380-394`). The ledger never mentions this.
Correction: write gate and capture outputs under `scratch/i08/<engine sha>/`, which is
gitignored and in `IGNORE_DIRS` (`gate.txt`, `capture.json`). Record the bundle zip path as
an expected side effect in the gate text.

**3. [major] Consumer gate: eight `suite_coverage` cases read an engine-derived artifact that no step regenerates, so the base skip set of seven holds only because a stale file exists.**
Evidence: the gate was run in a clean scratch copy against the `0ec757cb` release build
(engine sha `13a4d73d9786a092`, the same build as `docs/Red_Team_Record.md` Round 4):
`321 passed, 0 failed, 15 skipped`. The seven base skips appear, plus eight skips reading
"run `python3 -m calibration.schema --emit --all-cached` first"
(`calibration/tests/suite_coverage.py:27`). Those eight cases read
`results/engine_schema.json` (`schema.py:527`). The file is gitignored and was last written
on 2026-09-22, against an older engine. In the real tree the eight cases therefore assert
against that old engine on every gated commit. They will pass or fail for the wrong engine
once OD-2, PY-2 or PY-8, IN-3, IN-7 or CN-3 change the kwarg and override surface. Their
sources are `schema.py:251` (`KNOWN_KWARGS` from `py_dwelling.rs`) and the
`every_registry_factor_targets_a_real_field` case.
Correction: add a step 4b to the engine-under-test mechanism:
`HARES_REPO=<worktree> uv run --no-sync python -m calibration.schema --emit --all-cached`,
run before `run_all` on every gated engine commit. M-0 records the base run after the same
step. The seven-skip base set then holds on any clean checkout.

**4. [major] 08 preamble, contract-drift rule: the drift check covers far more than the `SimulationConfig` field set, and engine rows break it with no consumer row planned to follow.**
Evidence: `calibration/hares_contract_check.py:65-126` checks six things:
- the `EndUse::` set across every `.rs` file;
- `CORE_OUTPUT_COLUMNS`, `END_USE_AGGREGATE_SUFFIX` and the end-use display names as
  literals in `crates/hares-io/src/output/columns.rs`;
- `SCHEDULE_COLUMNS`, including `"heating_setpoint"` and `"cooling_setpoint"`, as literals
  in `crates/hares-io/src/hpxml_schedule.rs` (`hares_contract.py:126-131`);
- six equipment config structs, including `HvacSetpointConfig` in `hvac/heating_config.rs`;
- `Building` fields, including `heating_weekday_setpoints_c`;
- `SimulationConfig` in `hares-io/src/config.rs` and `DwellingConfig` in `dwelling/mod.rs`,
  with fields `initialization_duration`, `patches`, `hpxml_path`, `schedule_path` and
  `weather_path` (`hares_contract.py:146-150`).

The preamble assigns only the `SimulationConfig` field set and the kwarg rules (to CN-1,
CN-14 and CN-3). Engine rows that move the other checks include:
- OR-1 and B-5 (the catalogue replaces the name classifiers in `columns.rs`);
- OD-9 (removes the generated `heating_setpoint`/`cooling_setpoint` columns from
  `hpxml_schedule.rs`, row 94a, directly before the CN-3 gate at row 95);
- RT-4 (paths or content in the blueprint changes `DwellingConfig`);
- PY-8 (where `SimulationConfig` lives);
- IN-5 (setpoint source on the setpoint config);
- B-2 (`initialization_duration` leaves `DwellingConfig`).

A drift FAIL is a failing case and also blocks `build_bundle.py` (`contract_blocking`). So
the gates at CN-1, CN-3, CN-2 and CN-4 fail on items no entry plans.
Correction: rewrite the rule. "The first consumer row gated after an engine row runs
`hares_contract_check` and updates `hares_contract.py` for every drift item it reports, in
every category. CN-1 does it for rows 26 to 71, CN-14 for rows 79 to 81, and CN-3 for rows
83 to 94a (including OD-9's schedule columns and B-5's column literals)." Add to each of
those entries the census `python -m calibration.hares_contract_check` exits 0 on its engine.
Where an engine change removes the source-literal convention itself (the catalogue), the
check moves to the catalogue (PY-4) in CN-1.

**5. [major] 08 preamble and B0: the pinned engine is misidentified, and B0's tree comparison cannot be run as written.**
Evidence:
- 08 says `0ec757cb` "is `ebdb1a17` plus the three PR #27 commits (`4fa9bc43`,
  `6ec4fead`, `0ec757cb`)".
- PR #27 on origin (`refs/pull/27/head` = `89f32822`, `git ls-remote origin`) has four
  commits based on `2ee83a86`, not on `ebdb1a17`: `88d8aaa4`, `a20f8372`, `3703b941`,
  `89f32822` ("style: cargo fmt").
- `0ec757cb` exists only in the consumer's submodule object store. `git -C ~/src/HARES
  cat-file -t 0ec757cb` fails, and no origin ref points at it.
- The Task 0 head (the PR #27 merge) differs from `0ec757cb`'s tree: comparing the PR #27
  patch with `ebdb1a17..0ec757cb` shows rustfmt reformatting in
  `crates/hares-core/examples/profile_sim.rs` and `crates/hares-python/src/py_dwelling.rs`.
- B0 says "its tree is recorded and compared with `0ec757cb`'s tree" but gives no command
  and no expected result, so the executor finds a difference and has no rule for it.

Correction:
- In 08, state the facts above.
- Write the B0 comparison as `git -C ~/src/HARES_CALIBRATION_SYSTEM/HARES diff 0ec757cb
  <Task 0 head>` after `git -C ~/src/HARES_CALIBRATION_SYSTEM/HARES fetch
  ~/src/HARES <Task 0 head>` (an operator checkpoint, since it writes to the nested clone).
  Alternatively, diff the two exported trees with `git diff --no-index`.
- State the expected result: formatting-only differences in those two files, with the
  diffstat recorded in `02-measurement.md`. Any other difference stops B0.

**6. [major] Schedule row 64 (IN-5) moves numbers in a row marked "golden", before the consumer block.**
Evidence:
- IN-5 changes how `heating_setpoint`/`cooling_setpoint` schedule columns are read: from
  "no unit and no conversion" (`schedule_resolve.rs:176-190`) to degrees Fahrenheit
  converted at parse.
- Any schedule file with such a column changes by roughly 30 C. That is a number-moving
  change. `plan.md` §2.8 says "no numeric change is folded into a structural commit", and
  `dl-i08-architecture.consumer` puts number-moving engine changes after the consumer block.
  The row is labelled "golden" only because no golden carries a setpoint column.
- The in-memory generated schedule writes Celsius constants into the same column names
  (`hpxml_schedule.rs:66-83`) until OD-9 (row 94a). The ledger does not say whether the
  Fahrenheit rule applies to it. If it does, every generated-schedule run from row 64 to 94a
  heats to about -6.7 C.

Correction: split IN-5.
- Its structural half (warning, recorded `SetpointSource`, precedence unchanged) stays at
  row 64, bitwise neutral.
- Its Fahrenheit conversion becomes a numeric row in the numeric block, directly before
  OD-9, following 09's rules (delta recorded on a fixture with a setpoint column, commit
  body states it).
- State that the conversion applies only to parsed schedule files, never to the generated
  in-memory columns, which OD-9 then removes.

### Minors

7. [minor] CN-6: the gate rule "every other field is bitwise equal" is false for `flag` and `flag_codes`. `premise_flag` adds `fit_infeasible_corners` when `n_rejected / n_eval > 0.05` (`flag.py:161-164`), and both counts change. The `n_rejected` formula is also imprecise: a probe whose spec fails to build returns `PENALTY` without incrementing `n_rejected` (`fit.py:226-247`). Correction: rule `n-eval-without-parent-probes` also allows `flag` and `flag_codes` to differ, but only when recomputing `premise_flag` from B0's `n_eval - 2` and adjusted `n_rejected` gives the new value. Define the `n_rejected` drop as "the standalone probes that were submitted and came back `FailedRun`".
8. [minor] B-1b: at hour 24 EnergyPlus's "next hour" for solar is hour 1 of the same day (`WeatherManager.cc:3060`, `NextHr = (hour == 24) ? 1 : hour + 1` over the day's own `wvarsHr`), not the next day's hour 1. B-1b does not say which it uses. Correction: state "after the half hour of hour 24, the next-hour value is hour 1 of the same day, as `WeatherManager.cc:3060-3064`". Otherwise declare the deviation in `docs/PHYSICS_DECISIONS.md` with the reason, and add a test for the hour-24 steps.
9. [minor] B-1c: "records are addressed by calendar date and hour" does not say which clock. EPW and TMY3 hours are local standard time, and HARES's clock also carries a civil zone with DST (`clock.civil_tz`). Correction: "addressed by the local standard time date and hour (the site's standard offset), never the DST civil time". Add a test that a DST-period step reads the same record as the standard-time hour.
10. [minor] B-1c: `Typical` does not cover a file that is not a complete year. EnergyPlus wraps only a full-year data period (`WeatherManager.cc:929-941`). Otherwise the run's start and end must lie in the data period, or it is fatal (`:943-991`). Correction: add "a typical file that does not cover a complete year is a construction error when the run leaves its data period", with a test.
11. [minor] B-1c: not reproducing EnergyPlus's shifted day types after a skipped Feb 29 is a declared deviation from `dl-i08-architecture.weather_edge_cases` ("Follow EnergyPlus behaviour for Feb 29"). The ledger states it, but the binding decision does not allow it. Correction: record the deviation in the decision log entry (or follow EnergyPlus), and cite the IOR sentence that calls the result incorrect (`group-location-climate-weather-file-access.tex:597-601`).
12. [minor] B-1a: `N = 3600 / dt` is undefined for a sub-hour step that does not divide 3600 s. EnergyPlus allows only timesteps per hour that divide 60. Correction: `validate()` rejects a step under one hour that does not divide 3600 s, naming it, with a test.
13. [minor] B-13: `material_r` is clamped silently to `1e-6` when the declared R does not exceed the films (`conversions.rs:147-150`). B-13 would then scale the LUT layers to near-zero resistance without an error. Correction: a declared `AssemblyEffectiveRValue` at or below the film sum is a construction error naming the boundary. B-13 must also state that its normalisation runs before IN-7's `opaque_r_scale` (which lands earlier, at row 66a, and says "after B-13's scaling"), and add the engine test `assembly_r_and_opaque_r_scale_compose`: layers equal row times `k` times the scale, capacitances unchanged.
14. [minor] 09 preamble: "B-6 then B-11; B-5 next to B-11" cannot hold with the schedule's B-5, B-6, B-11 (rows 86 to 88). Correction: order B-6, B-11, B-5 (B-5 depends only on OR-1), or drop the B-5 adjacency claim here and in B-11 ("Adjacent to B-5").
15. [minor] 00 rules: "rows 1 to 10 each keep ... BR-6b `[advisories]`" and then "From row 11 (BR-6b) on". BR-6b is row 11. Correction: "rows 1 to 11".
16. [minor] 00 "Consumer check" column: rows 14 to 25 read "B0 (CN-16)", but from row 13 the consumer under test is the BR-6d commit. Correction: "B0 (BR-6d)" for rows 14 to 25, and define that label in the column legend.
17. [minor] B-1c (`validate()` error on a start on a skipped Feb 29) and B-2 (`validate()` rejects tolerances and day counts) use `12-inputs.md` IN-2, but neither lists it. The order is satisfied (row 66). Correction: add IN-2 to both dependency lists.
18. [minor] B-2 acceptance `frame_starts_at_start_time_with_exact_row_count` ("row count equals `duration / time_res`") conflicts with B-1c on a leap-year run with a typical file (384 rows, not 480). The consumer's `Runner.run` row check (`engine.py:361-366`, `grid.n_steps`) has the same assumption. Correction: "equals the number of simulated steps (`duration / time_res` less any skipped Feb 29)". CN-14 or CN-1 states the consumer's row check uses the engine's reported step count.
19. [minor] CN-16: "engine side as its own engine commit scheduled before B0" has no schedule row. Such a commit would land while the engine block is still red (before BR-1). Correction: add a contingent row "0c-engine" to 00 with its rule (the block lines green at the Task 0 head stay green), or say that an engine-side cause stops the opening sequence for an operator decision.
20. [minor] CN-13: `mv ~/.cache/ochre_next ~/.cache/hares` nests the source inside the target if `~/.cache/hares` already exists (for example, created by a PY-7 `[pytest]` or a manual run). Correction: the checkpoint command is `test ! -e ~/.cache/hares && mv ~/.cache/ochre_next ~/.cache/hares`, honouring `$XDG_CACHE_HOME`.
21. [minor] Engine-under-test step 3: `uv pip install --editable` resolves the build backend from `build-system.requires = ["maturin>=1.15.0,<2"]` at install time, not from any lock. The backend version therefore floats between gated rows, which conflicts with "no dependency changes". Correction: pass `--build-constraint` (or `UV_BUILD_CONSTRAINT`) naming the maturin version from HARES's `uv.lock` at that commit, and record it with `paths.summary()`.
22. [minor] CN-16 evidence wording: `docs/Red_Team_Record.md:97-98` attributes the three failures to "upstream HARES#26", and `:144-155` is labelled "engine `ebdb1a17`, build `13a4d73d9786a092`". That build hash equals the `0ec757cb` release build measured here, so the substance holds, but the citation text does not say what the ledger says it says. Correction: quote the record as written and add "build `13a4d73d9786a092`, which is the release build of `0ec757cb` (measured)".

## verify-L1-d findings marked fixed: checked in the text

All 36 are fixed in the current text, with the partial items (18, 36) closed by the
integrator:
- 1: the mechanism now works as written. It was run end to end, see below.
- 2: the gate is `run_all`, `ruff` and `build_bundle.py`, with an exact skip set. See
  majors 2 and 3 for new defects in that text.
- 3: B0 is on the Task 0 head, before any engine row, with `consumer_shape_900s` from PY-8
  on.
- 4: B-1a, B-1b and B-1c are rewritten, and their EnergyPlus content is correct (see
  below).
- 5 to 17: fixed.
- 18: OR-5 is row 4.
- 19 to 35: fixed.
- 36: BR-5 closes in the OR-5 row. SL-1, SL-4, SL-5 and SL-6 close in the RT-1, CN-6, CN-5
  and CN-9 rows. SL-2 and SL-3 are standing rules. OD-4 and OD-5 were removed with their
  reasons (`11-other-defects.md` preamble).

One inherited error: verify-L1-d's "verified correct" line ("`0ec757cb` = `ebdb1a17` +
`4fa9bc43`/`6ec4fead`/`0ec757cb` (PR #27 content)") is the source of major 5.

## Items verified correct

Consumer mechanics (run in scratch copies):
- Step 2, `uv sync --locked --extra engine --no-install-package ochre-next`, works on a
  fresh copy (11 packages, `pyarrow` kept).
- Without the extra, a dry run would uninstall `pyarrow`, as the ledger says.
- Step 3 (`uv pip install --no-deps --reinstall-package ochre-next --editable <tree>` with a
  scratch `CARGO_TARGET_DIR`) builds the release profile through the maturin backend in
  1 min 42 s and installs exactly one package.
- Step 4's dry run prints exactly one "Would uninstall" line, naming the worktree engine.
- Step 5 with `HARES_REPO` resolves `hares_repo`, `engine_so` (inside the worktree) and
  `engine_sha256 = 13a4d73d9786a092` through `paths.summary()`.
- `paths.engine()` falls back to `import hares`, so the rename row's reasoning holds.
- Gate facts:
  - `run_all` returns 1 only on failures (`run_all.py:136-141`).
  - Case marks are ` FAIL `, ` skip ` and `  ok  ` (`testkit.py:199`).
  - `ruff check` exits 0 at base.
  - `build_bundle.verify()` at base returns exactly the three unregistered files CN-0
    registers.
  - Green needs zero skips (`build_bundle.py:373`).
  - The three `Test_Report.md` failures do not reproduce on the pin build (0 failed), which
    CN-16's M-0 contingency anticipates.
  - The seven base skips are exactly as named, given major 3.
- DE-trace set: `DEFAULT_COUNTY = "G4802010"` (`scale.py:64`); `calibrate_group` signature
  and `build_group(county, mix=_mix_for(n))[:n]` (`scale.py:638-639`); `seed=0`
  (`scale.py:569`); `HomeCalibration` fields and the three wall-clock fields; `r.x` dropped
  (`scale.py:573-583`).
- Consumer defects are real:
  - CN-5: `scale.py:630` defaults `workers` before the `:633` guard. Both
    `wave_balanced_workers` examples hold (12 and 15).
  - CN-6: standalone probes at `fit.py:315, 317` and the final run at `:352-357`. The
    `P * len(trace) + 1` count is correct. `engine_runs = n_eval + 2` at `scale.py:504-507`.
  - CN-9 and CN-10 evidence.
  - CN-1 call sites, including `RESAMPLE_OVERRIDES = None` (`grid.py:173`, so no
    `resample_overrides` is sent before IN-6), and the release-build health counters.
- Pin rule: gitlink unchanged until CN-15, `Engine:` trailer, a single advance after the
  operator's last push. The submodule URL is `github.com/AZX-PBC-OSS/HARES`, so
  `git -C HARES fetch origin <sha>` targets public origin. Single re-baseline B1 at CN-4,
  with CN-14, CN-3 and CN-2 absorbed.

Numeric fixes against EnergyPlus (vendored source and docs; published 24.1 pages):
- B-1a:
  - Last Hour Interpolation formula and the published 4-step and 6-step weights
    (`weather-data-hourly-interpolation.tex`; the bigladder page matches, including the day
    boundary and the first-day hour 24).
  - Interpolation only for one record per hour with more than one timestep per hour
    (`WeatherManager.cc:3043`).
  - Fields and methods (`:3092-3121`): linear point fields, circular wind direction,
    precipitation divided by N, sky temperature recomputed by `calcSky` from interpolated
    inputs, snow from the hour's record.
  - First hour: `firstHrInterpUseHr1 ? 1 : 24` (`:3050-3056`; IOR `:717-721`, default
    Hour24).
  - Warm-up reuses the first day's interpolated arrays (weather is read only when not in
    warm-up, `:1804-1830`), so "warm-up days repeat the first day with the same rule" is
    correct.
  - HARES evidence: the modulo wrap (`environment.rs:531-536`) and the 1800 s shift
    (`:811-849`), the per-field defaults (`weather.rs:611-708`), and the per-format offsets.
  - The analysis of what HARES reads at 900 s and 3600 s is correct.
- B-1b:
  - Engineering Reference "Weather File Solar Interpolation" quote (`climate-calculations.tex:304-306`;
    bigladder matches).
  - `SetupInterpolationValues` weights (`:8318-8375`): N=4 is 0.75, 1, 0.75, 0.5; N=6 is
    4/6, 5/6, 1, 5/6, 4/6, 3/6; N=3 is 5/6, 5/6, 0.5; odd N of 5 or more is `1 - 1/(2N)` at
    `halfpoint` and `halfpoint + 1`.
  - Previous or next hour by `ts * TimeStepFraction < 0.5` (`:3074-3090`).
  - The 700, 800, 600, 400 test values.
  - Interpolation between midpoints evaluated at step end.
- Sub-hourly mismatch: fatal (`:715-722`), quoted exactly.
- B-1c:
  - Non-actual full-year wrap (`:929-941`).
  - Actual-year range checks (`:904-923, 964-991`).
  - A leap year in an actual run with a file that does not allow leap years is fatal
    (`:888-899, 993-996`).
  - `LeapYearAdd` (`:873-876`), `CurrentYearIsLeapYear` forced false (`:1896-1900`),
    `IsLeapYear` requiring the file flag (`:8456`), the header flag (`:7896`).
  - The Feb 29 skip warning (`:2816-2828`) and the actual-only missing-Feb 29 fatal
    (`:2832-2836`).
  - The IOR "Treat Weather as Actual" and leap-day text (`:597-601, 709-716`).
- B-2:
  - `CheckWarmupConvergence` (`HeatBalanceManager.cc:3308-3470`): per-zone daily max and
    min of the timestep-average zone air temperature (`:3237-3241`); peak `airSysHeatRate`
    and `airSysCoolRate`; the 1e-4 W threshold and 100 W floor on both days; relative
    change against today.
  - Min and max days and their stop rule; the min-above-max reset (`:686-696`); IDD
    defaults 0.04, 0.4, 25, 1 with `(0, 0.5]` (`Energy+.idd.in:536-575`;
    `DataHeatBalance.hh:96-97`); the first day compared against zeroed values
    (`:2888-2895`).
  - HARES evidence (`mod.rs:2724, 2740, 4992-5069, 1812, 2779-2786`).
- B-4: the evaluator closes on `current_time >= period_end` after the update
  (`evaluator.rs:618-634`); its tests pass the step end (`:1164-1182`); the dwelling passes
  the step start (`mod.rs:6520-6526`, `environment.rs:777`, `mod.rs:6581`). The fix is
  placed correctly.
- B-7 (`metrics.rs:593-604`), B-8 (`fluid_solver.rs:783-787`), B-9 (`fleet.rs:779-799,
  186-193`), B-10 (`blueprint.rs:118-140`), B-11 (`aggregation.rs:44-47, 120-123`) and B-12
  (`aggregation.rs:634-637`, rfc3339 with `Z`) are real at the cited code.
- Every numeric entry has its own row and commit, follows the materialize, delta and
  capture procedure with a recorded delta (including `consumer_shape_900s`), and lands after
  the consumer block (rows 79 to 94a). The structural halves OR-1/B-5, OR-8/B-7 and
  FL-4/B-12 are bitwise-neutral rows.

Schedule:
- Every entry in every ledger file appears exactly once: BR-1 to BR-8 with BR-5 closed by
  OR-5; M-0 to M-4b; OR-1 to OR-16; FL-1 to FL-4; PY-1 to PY-11; RT-1 to RT-7; PR-1 to PR-6;
  CN-0 to CN-16 and B0; B-1a to B-13; SL-1 to SL-8 (SL-2 and SL-3 as standing rules; SL-1,
  SL-4, SL-5 and SL-6 in their fixing rows); OD-1, 2, 3, 6, 7, 8, 9; IN-1 to IN-7.
- Every stated **Dependencies** entry in every ledger file was checked mechanically against
  row order. Every apparent inversion is a forward "lands before" or "depends on this" note,
  not a prerequisite. No prerequisite follows its dependant.
- The opening sequence matches `plan.md` §3. Consumer ordering matches
  `dl-i08-architecture.consumer`:
  - CN-0 and CN-16, then B0 before any engine row.
  - B0 checks for rows 1 to 25 with the CN-16 consumer, CN-13 directly after PY-7, and B0
    checks through OD-2, OD-8 and IN-6.
  - `consumer_shape_900s` from PY-8 on.
  - The mechanical consumer block (CN-1, 5, 9, 7, 6, 10, 11, CN-8) before the first numeric
    fix.
  - CN-14 after the weather fixes; CN-3 after B-13 and IN-7; CN-2 before CN-4.
  - One B1, then CN-12, M-4a, push, M-4b, push, CN-15, push.
