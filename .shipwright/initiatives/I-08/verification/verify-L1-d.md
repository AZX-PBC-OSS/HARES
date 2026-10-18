# Verification L1-d: consumer, numeric fixes, schedule

Scope: `ledger/08-consumer.md`, `ledger/09-numeric-fixes.md`, `ledger/00-schedule.md`,
checked against HARES at `ebdb1a17`, the consumer at `c71c542` (submodule pin `0ec757cb`,
which is `ebdb1a17` plus the PR #27 commits), the vendored EnergyPlus source and docs under
`vendors/EnergyPlus/`, and `decision-log.md`.

Counts: 4 blockers, 15 majors, 17 minors.

## Findings

### Blockers

**1. [blocker] Engine-under-test mechanism (08 preamble; plan.md section 3) cannot run.**
Evidence:
- `maturin develop` has no `--active` flag (maturin 1.15.0, `maturin develop --help`); the
  ledger's `uvx --from maturin maturin develop --active --release` exits on an unknown argument.
- plan.md section 3 gives a different command, `uv run --active --no-sync maturin develop --release`;
  maturin is not installed in the consumer venv (`.venv/bin` has no maturin, pip or pytest), so
  that fails too.
- The consumer venv has no pip. maturin needs `--uv` to install into it.
- Neither text gives the working directory (the HARES worktree) or `VIRTUAL_ENV` (the consumer's
  `.venv`), and maturin needs both.
- `paths.py:28-35` resolves `HARES_REPO` to the consumer's `./HARES` submodule unless
  `$HARES_REPO` is set. So `DEFAULTS`, `FIXTURES` and the contract-drift check
  (`hares_contract.py`) read the stale submodule tree, not the worktree being tested.
- `uv sync --active` without the engine extra is an exact sync, so it uninstalls `pyarrow`
  (an engine-only dependency, `uv.lock:281`). maturin then reinstalls it outside the lock, so
  the environment can differ from the one B0 was captured in.

Correction: replace both texts with one sequence:
- `cd ~/src/HARES_CALIBRATION_SYSTEM && uv sync --extra engine --no-install-package ochre-next`
  (dependencies stay locked; only the engine is left out).
- `cd <HARES worktree> && VIRTUAL_ENV=~/src/HARES_CALIBRATION_SYSTEM/.venv uvx --from 'maturin>=1.15,<2' maturin develop --release --uv`.
- `cd ~/src/HARES_CALIBRATION_SYSTEM && HARES_REPO=<HARES worktree> uv run --no-sync <command>`.
- Record `paths.summary()` (engine sha, `hares_repo`) with every gate run.

Make plan.md section 3 and the 08 preamble identical.

**2. [blocker] Every consumer gate command is wrong, and the real gate passes on skips (CN-1..CN-12 Verification).**
Evidence:
- The ledger's gate is `uv run --active --no-sync pytest`, but pytest is not a consumer
  dependency (`pyproject.toml` dependencies; the consumer venv has no pytest).
- The consumer's gate is `uv run python -m calibration.tests.run_all` plus `build_bundle.py` and
  `ruff check` (AGENTS.md "Verify before claiming anything").
- `run_all.main()` returns 1 only on failures, not on skips (`calibration/tests/run_all.py`,
  `return 1 if failed else 0`). Engine suites skip when the engine or caches are missing.
- The gate is not green at base: `docs/Test_Report.md` at `c71c542` reads "325 passed, 3 failed,
  7 skipped". The ledger never states which failures and skips are expected.

Correction:
- Verification for every CN entry: `run_all` plus `build_bundle.py` plus `ruff check`, run
  under the mechanism in finding 1.
- Record the base gate's exact pass/fail/skip list at `c71c542` in `02-measurement.md`.
- A gate passes only when (a) failures are exactly the recorded base failures, less any fixed
  ones, and (b) skips are exactly the recorded named skips (GridLAB-D/HELICS binaries, and so
  on), with zero skips in `suite_engine`, `suite_fit`, `suite_premise` and `suite_ami2`
  engine cases.
- Write the acceptance tests as testkit cases in named suites, not pytest functions, and
  register every new file in `build_bundle.py` (AGENTS rule 1).

**3. [blocker] B0 and the CN-1 bitwise gate cannot run as written, and the comparison does not isolate the consumer change (CN-1 acceptance; 00-schedule "B0 captured" row and the gate row).**
Evidence:
- CN-1 re-runs the migrated consumer "against the same unmodified engine" (pin `0ec757cb`).
  The migrated consumer uses `ReportSpec`/`RunResult.health` (OR-3/OR-5), which do not exist
  at `0ec757cb`.
- The schedule captures B0 "on unmigrated engine/consumer" after row 45. By then OR-3 (row 21)
  has removed `write_output`/`retain_batches`, so the unmigrated consumer cannot run at that
  commit.
- Engine rows 1-45 contain changes that can move the consumer's DE trace:
  - BR-6's nalgebra upgrade ("any output change ... recorded").
  - OD-2's strict override validation plus the precedence rule "overrides the engine cannot
    apply are rejected". The consumer's `Injector` override dicts and `schema.py` probes
    would then fail and be scored as `PENALTY`.
  - OR-5 makes NaN and wiring checks fatal in release builds.

  A bitwise failure against B0 would therefore not identify its cause, and a pass needs an
  argument the ledger does not make.

Correction:
- Capture B0 with the unmigrated consumer on the last engine commit it can run: row 20, before
  OR-3.
- Capture B0-pin on `0ec757cb`, and record the B0-pin to B0 delta (attributing it to BR-6/OD-2).
- Require rows 21-45 to be bitwise-neutral on a golden fixture that mirrors the consumer's call
  (15-min grid `grid.py:55`, verbosity 2, no warm-up, `resample_overrides=None`, a ResStock AMY
  2018 CSV, the consumer's override dict shape). Add that fixture to M-2.
- Gate CN-1 through CN-11 against B0 on the row-45 engine.
- Fix CN-1's acceptance text and the schedule row to say exactly this.

**4. [blocker] B-1 (weather) has a false premise, defers the defect to implementation time, and contradicts EnergyPlus on every sub-item.**

Evidence:

- **Point fields.** B-1 says the midpoint-aligned PCHIP path "already matches pvlib/EnergyPlus
  convention". It does not.
  - EnergyPlus uses "Last Hour Interpolation": `Value = LastHour*(1-ts/N) + ThisHour*(ts/N)`,
    with the file value reached at the hour's end. Sources: Input Output Reference, "Weather
    Data Hourly Interpolation" (`doc/input-output-reference/src/weather-data/weather-data-hourly-interpolation.tex`);
    source `WeatherManager.cc:8338-8341` for the weights and `:3066-3095` for dry bulb, dew
    point, RH, pressure, wind speed, sky cover and circular wind direction.
  - HARES instead uses PCHIP/PchipCyclic (`weather.rs:597-627`) with a 1800 s shift for all
    fields (`environment.rs:823-847`), so file values land at the half hour.
  - HARES holds wind speed with ZOH (`weather.rs:698-702`); EnergyPlus interpolates it linearly.
- **Solar.**
  - The quoted sentence "solar is interpolated between those midpoint values" is not in the
    reference. The Engineering Reference section is "Weather File Solar Interpolation"
    (`doc/engineering-reference/src/climate-sky-and-solar-shading-calculations/climate-calculations.tex:304-306`),
    and it says only that the average "is assumed to be the value at the midpoint of the hour".
  - The actual scheme is in `WeatherManager.cc:8343-8375` (`SolarInterpolation` weights, for
    example N=4 gives [0.75, 1.0, 0.75, 0.5]) and `:3072-3090` (previous hour before the half
    hour, next hour after). The weight is evaluated at the end of timestep ts.
  - Current HARES behaviour is not "to be confirmed". It is ZOH (`weather.rs:668-696`) plus the
    global 1800 s index shift, so each hour's average is held from :30 to :30 of the next hour.
- **Wrap.** In EnergyPlus, typical (non-actual) full-year data wraps implicitly
  (`WeatherManager.cc:929-941`). Actual weather must lie inside the data period and is fatal
  otherwise (`:904-923, 964-991`). B-1's "`NoWrap` rejects an overrun; a typical-year run
  states cycling explicitly" inverts the typical-year rule.
  - B-1's test also misses the wrap HARES does at step 0: `compute_annual_offset` subtracts
    1800 s modulo the year, so the first 30 min read Dec 31 data.
  - EnergyPlus uses hour 24 or hour 1 of the first day for first-hour interpolation instead:
    I/O Reference, RunPeriod "First Hour Interpolation Starting Values"; source
    `WeatherManager.cc:3053-3060`.
- **Leap day.**
  - Actual weather without a leap day when the run period includes a leap year is a fatal
    error (`WeatherManager.cc:888-899, 993-996`).
  - For non-actual weather, "the leap day will be skipped if it is not present" (I/O Reference
    `group-location-climate-weather-file-access.tex:597-601`).
  - B-1 leaves this "documented ..., not invented here", so no rule is specified.
  - HARES has a concrete defect here that B-1 misses: `compute_annual_offset` uses
    `start_time.ordinal0()` against an 8760-row file, so a leap-year start after Feb 29 reads
    weather one day late.

Correction: rewrite B-1 with:
- Evidence: `weather.rs:597-706`, `environment.rs:531-536, 823-847`, `WeatherMeta.midpoint_offset_secs`
  per format (`tmy3.rs:254`, `resstock_csv.rs:362`, `psm3.rs:309`).
- Per-field convention, citing the two reference sections and the source lines above:
  - Point fields: Last Hour linear, with the file value at hour end. Wind direction: circular.
    Precipitation: distributed.
  - Solar (GHI/DNI/DHI): EnergyPlus `SolarInterpolation` weights, each timestep evaluated at
    its end, and HARES interval-start labelling mapped explicitly to it.
  - Sky temperature: recomputed from the interpolated inputs (already done, `WeatherManager.cc:3096-3105`).
- The midpoint shift is removed for all fields.
- First-hour rule, chosen per run: hour 24 by default, or hour 1.
- Typical year: wraps by day of year, and Feb 29 is skipped when absent.
- Actual year: the run must lie inside the file's dated period, and a missing leap day is a
  named error.
- Tests pinning the EnergyPlus weight tables (N=4 and N=6 from the I/O Reference tables), the
  first-hour rule, and the leap-year ordinal case.
- Solar-energy and temperature deltas recorded separately.

### Majors

**5. [major] CN-1 call-site list is incomplete, and one cited site is wrong.**
Evidence:
- `inject.py:203` is `rotation=rotation` in a `RunSpec`, not a removed kwarg.
- Missing sites:
  - `forward_model.py:237-293` (from_hpxml, simulate and `output_verbosity`).
  - `ami2/cvr_actuation.py:133-134`, `ami2/dr_actuation.py:211-212`, `ami2/tariff.py:85-89`
    (tariff output also moves under B-4).
  - `ami2/adapter.py:350-372`, which reads `res.metrics` and `res.health["nan_temperature_count"]`.
  - `parallel.py:57-70` (`worker_runner(scratch, ...)`) and `:208` (`health_*` keys).
  - `engine.py:224, 237, 329-334, 406-407, 507, 517-527`.
  - `hares_contract.py:147-150` (the `SimulationConfig` field set that the drift check compares
    against the engine source).
  - `coverage.py:112-116`, `tests/suite_engine.py:53-54, 80, 95-97`, `scale.py:342`,
    `paths.py:222-227`.
- `Runner` is shared by the fit path and non-fit callers: `forecast.py:247` needs the HVAC
  end-use columns, and `probe.py` hashes the whole frame (`parallel.py:249`). A fit-only
  one-column `Selection` inside `Runner` breaks them.

Correction: list every site above. `Runner.run` takes the `Selection` from its caller: the fit
path passes `Total Electric Power (kW)`, and forecast and probe keep their columns. List the
test updates.

**6. [major] The CN-1/CN-2 health mapping has no target.**
Evidence:
- `RunHealth` (plan.md section 2.3) has `port_rollbacks`, `rejected_control_signals`,
  `clamped_actions`, `warmup` and `metrics_status`. It has no NaN count and no
  unresolved-column count.
- The consumer's health dict is keyed on `nan_temperature_count`, `unresolved_column_count` and
  `port_rollback_count` (`engine.py:64-66, 384-397`).
- CN-2 step 2 arms "port rollbacks and unresolved columns". Unresolved columns is not a
  `RunHealth` counter.
- At `0ec757cb` the counters are written only under `feature = "observe"` (OR-5's own
  evidence). With the release build the consumer uses, its NaN and unresolved gates are
  always zero today. CN-2's premise that they are live gates is false.

Correction: state the mapping per key.
- NaN becomes an engine run failure, which reaches the consumer as `FailedRun`. Say whether
  that equals today's `PENALTY` path.
- Unresolved columns are removed, because the catalogue makes them a build error (cite the
  OR-1 behaviour).
- Rollbacks come from `RunHealth.port_rollbacks`.

Drop "unresolved columns" from CN-2 step 2. Record that the current consumer health gates were
inert in release builds.

**7. [major] CN-6's precondition is invented, and its call-count change is not neutral.**
Evidence:
- The registry's ABSOLUTE factors are `wh_energy_factor`, `wh_ua_scale` and
  `charge_defect_ratio` (`registry.py:190, 202, 218`). Their nominal value is the box midpoint
  (`fit.py:178-179`). Nothing "composes differently across a probe-then-batch pattern", so
  asserting "no ABSOLUTE factor free" would reject real fits: `wh_energy_factor` is free in the
  fit path, per CN-2's own text.
- The real precondition is that `Injector.build` is a pure function of `x` across slots:
  standalone `ws` is built on `injector(0)` (`fit.py:262, 224-229`), while in the batch it is
  built on `injector(1)`.
- Removing the two standalone `evaluate` calls lowers `FitResult.n_eval` by 2 (`fit.py:240`).
  `n_eval` is BEHAVIOURAL in `baseline.py:174`. It feeds `scale.py:505-507` (`engine_runs = n_eval + 2`),
  `frontier.py:31` (`2 + P + G*P`) and `flag.py:161`. This happens inside the B0-gated window
  without being declared.

Correction:
- Replace the ABSOLUTE assertion with a bitwise test that `loss(standalone nominal/ws)` equals
  `fx[0]/fx[1]` on the base code.
- Declare the `n_eval` change, update `scale.py:505-507`, `frontier.py:31` and `_mix_for`'s
  docstring, and state how `baseline.diff_fits` treats it.
- Retaining the optimum's series needs the P per-member series from the generation that last
  improved `best`. State that.

**8. [major] CN-10 cannot work as designed.**
Evidence:
- The cited `scale.py:667` is the premise-level outer pool: different buildings per premise.
- The per-evaluation pool is `parallel.py:37-45` (persistent spawned `ProcessPoolExecutor`).
- Every DE candidate gets its own injected HPXML and a per-slot defaults variant
  (`defaults.py`; `fit.py:214-221`). So "one `Arc<Blueprint>` shared across threads" cannot
  serve the population.
- The decision log rules out an edit language now (`dl-i08-architecture.inputs`).

Correction: define what is shared. Parsed weather and schedule content are shared through the
section 2.7 content inputs. HPXML and defaults stay per candidate. Target the evaluation pool in
`parallel.py`, and handle the outer premise pool separately. Recompute the RSS target on that
basis.

**9. [major] B-13 has no evidence and an undefined mechanism, and CN-3 depends on something that does not exist.**
Evidence:
- Current precedence: `conversions.rs:101-160` (LUT `precomputed_rc` first, then
  `assembly_r_value_m2_k_w`, then layer sum, then a 2.5 m2K/W default) and
  `boundary_rc.rs:926-935` (lumped fallback).
- The consumer documents the LUT as outranking `AssemblyEffectiveRValue`, which "survives only
  as a nearest-row selector" (`calibration/defaults.py:1-6`, `registry.py:162-169`).
- B-13 does not say what happens to the LUT's capacitance and layer split when the declared R
  wins.
- CN-3's "engine's existing layer decomposition logic runs on the already-scaled value" has no
  existing counterpart.
- CN-3 ignores `thermal_mass_scale` (`registry.py:169`), which uses the same defaults route,
  and the notes at `registry.py:164` and `coverage.py:136, 226-228`.

Correction:
- B-13 cites the lines above and specifies the rule: scale the LUT row's layer resistances
  uniformly so their sum plus films equals the declared assembly R, keeping the LUT
  capacitances; or name an alternative.
- B-13 adds a test on the resulting per-layer R and C.
- CN-3 covers both `opaque_r_scale` and `thermal_mass_scale` and the registry and coverage
  notes.

**10. [major] CN-2b adds a second consumer baseline.**
Evidence: schedule row 69 says CN-2b is "gated against its own new baseline". Row 70 then
records B1. The decision log says "the consumer re-baselines once, after all engine numeric
fixes".

Correction: land CN-2b before CN-4, with its acceptance test only, and let B1 absorb its delta.
Or land it after CN-4 and gate it against B1, requiring trace equality whenever no evaluation
was newly rejected.

**11. [major] B-7 and B-12 are not in their own commits, and they land before the consumer migration.**
Evidence: B-7 "folds into OR-8's commit" (row 26) and B-12 "folds into FL-4's commit" (row 34).
The decision says each numeric fix is "in its own commit with golden re-capture and a recorded
delta", and the consumer migration lands "before the engine numeric fixes". Schedule rows 62
and 66 are placeholders with no commit.

Correction (operator decision): either amend `dl-i08-architecture.numeric_fixes` and `.consumer`
to allow these two folds, and move their rows to 26 and 34 with the delta recorded there; or
split each into a structural commit that is bitwise-neutral on the goldens and the consumer
fixture, followed by its own numeric commit at rows 62 and 66.

**12. [major] B-2 is not "EnergyPlus-grade", and its change 3 contradicts EnergyPlus.**
Evidence:
- EnergyPlus checks, per zone, the day-over-day change in daily maximum and daily minimum zone
  temperature against `TempConvergTol` (0.4), and the relative change in peak heating and
  cooling load against `LoadsConvergTol` (0.04). It enforces a minimum number of warmup days
  (default 1) and a maximum (default 25). Sources: `HeatBalanceManager.cc:3308-3400`;
  `Energy+.idd.in` N2 at 536-542, N3 at 543-548, N4 at 555-557 ("should NOT be set less than
  25"), N5 at 564; `DataHeatBalance.hh:96-97`.
- HARES compares only the last-step temperatures (`mod.rs:5031-5044`).
- Change 3 makes `initialization_duration` (Rust default 7 days, `mod.rs:1812`) the maximum
  day count, which caps warm-up at 7.
- The code comment at `mod.rs:4998` ("EnergyPlus defaults: threshold = 0.5 C") is itself wrong.

Correction:
- Implement the EnergyPlus criterion: per-zone daily max and min temperature (0.4 C), peak
  heating and cooling loads (0.04), minimum 1 day and maximum 25, as typed config fields.
- Replace `initialization_duration`'s overloaded meaning with those fields. The strict config
  names the replacement.
- NaN aborts (see minor 30).
- Fix the comment.

**13. [major] B-4 puts the fix in the wrong place.**
Evidence:
- The evaluator's contract is end-of-step time: "passing end-of-step time as current_time"
  (`evaluator.rs:1166-1180`, test helper). The close check is at `:630`.
- The dwelling passes step-start time: `self.latest_env.current_time` (`mod.rs:6523-6526`) is
  set from `clock.current_time()` (`environment.rs:777`) before `clock.next()` (`mod.rs:6581`).
- Reordering the evaluator would break its contract and its tests. The step that ends at
  `period_end` would move into the next month.

Correction: pass `current_time + dt` at `mod.rs:6525` and leave `evaluator.rs:618-634` as it
is. Add the dwelling-level acceptance test. Record that the consumer's `ami2/tariff.py` output
moves.

**14. [major] B-6 replaces every `unwrap_or(0.0)` wholesale.**
Evidence: the sites at `mod.rs:6753-6791` mix three cases:
- Not applicable: SHR while not cooling.
- Genuinely zero: runtime fraction and backup ER kW while off.
- An accumulation: `row[idx] += ...DUCT_LOSS_W` at `:6786`.

Also, `record_scratch.fill(0.0)` at `:6607` is the reset for the `+=` accumulations.

Correction: classify every site: an absent telemetry key or an inapplicable source is null; a
component that is present but off is 0.0. Keep a zeroed accumulator for summed cells. Specify
how metrics treat nulls: sums skip them, and means use a denominator that excludes nulls,
declared per `VariableDef`.

**15. [major] The package rename has no ledger entry, and the consumer side is unplanned.**
Evidence:
- `dl-i08-architecture.package` requires distribution `hares-py`, import name `hares`, cache
  `~/.cache/hares`, and no `ochre_next` alias.
- No ledger file implements it; grep finds none. HARES `pyproject.toml` still says
  `name = "ochre_next"` and `module-name = "ochre_next._hares"`.
- Consumer impact:
  - `pyproject.toml` `engine = ["ochre-next"]` and `[tool.uv.sources] ochre-next`.
  - `paths.py:69-90` imports `ochre_next` then `hares`, which acts as an alias.
  - `paths.py:111` has `CACHE = ... / "ochre_next"`. If the engine moves its cache, the consumer
    finds no cached buildings, the engine suites skip, and `run_all` exits 0.

Correction: add an engine entry (05) and a consumer entry (08) that renames the extra, source,
import and cache path together, and add both to the schedule. That entry depends on finding 2's
zero-skip rule.

**16. [major] Advancing the submodule pin is undefined and has no operator checkpoint.**
Evidence:
- "The submodule pin advances to the worktree commit at every consumer commit" means fetching
  unpushed commits from the HARES worktree into `~/src/HARES_CALIBRATION_SYSTEM/HARES` and
  checking them out there. The operator's standing rule forbids `git checkout` without
  permission.
- AGENTS rule 5 requires every pin bump to carry the full gate, a regenerated
  `docs/Test_Report.md` (`run_all --report`) and a recorded engine commit.
- AGENTS rule 1 requires new files to be registered.

Correction: give the exact commands (`git -C HARES fetch <worktree> <sha>` followed by the
checkout, under an operator checkpoint, or leave the gitlink untouched until the final pin).
Add `--report` regeneration and `build_bundle.py` to every pin-moving commit. Add an operator
checkpoint: no consumer push carries a pin to an engine commit that is not on public origin.

**17. [major] The B0/B1 capture is undefined.**
Evidence: "production O5 cells (`decision-log.md` section `consumer`)". The decision log does
not define them. The O5 proof in the consumer is 4 premises x 365 days x 16 generations through
`scale.calibrate_group` (`docs/Optimization_Register.md` O5; `docs/Red_Team_Record.md` Round 2).
No tool dumps the DE traces with `float.hex`.

Correction:
- Name the exact call: `scale.calibrate_group(n_homes=4, days=365.0, generations=16, fleet_workers=<recorded>)`,
  with the county default `G4802010`.
- Add a capture script (registered) that writes, per premise, `trace`, `loss`, `n_eval`,
  `n_rejected`, `x` and `components` as `float.hex`, with the engine sha and lock hash.
- Use the same script for B0, the gate and B1.

**18. [major] The schedule breaks the verification block for rows 8-22.**
Evidence: BR-5's three release-build failures are fixed only by OR-5 (row 23;
`01-baseline-repair.md:150-200`). plan.md section 3 requires the full block, including plain
`--release`, after every row, and "no entry may narrow this block". Rows 8-22 therefore cannot
pass.

Correction: move OR-5, with its OR-1 prerequisite or a split of its invariant half, ahead of
row 8. Otherwise add a plan rule naming exactly these three tests as the only tolerated
failures until row 23.

**19. [major] B-11 contradicts the aggregation decision and cites the wrong file.**
Evidence:
- B-11 applies Mode to "connection state and similar". The decision says "binary state columns
  average (duty fraction)", and EV connection state is binary (`columns.rs:164`).
- `writer.rs:1765` does not contain the mode column. The cited line is a test at
  `aggregation.rs:1765`.
- The real mechanism is `ColumnAggregation::for_column`, which maps unit `enum` to Mean and is
  pinned by the test `enum_unit_maps_correctly` (`aggregation.rs:1759-1776`). The column
  suffix is `columns.rs:102` (`MODE_SUFFIX`).

Correction: restrict Mode to multi-valued enums (`* Mode (-)`) and keep binary state as mean.
Cite the correct lines. State that the `enum_unit_maps_correctly` test flips.

### Minors

20. [minor] CN-1 evidence: `write_output`/`output_to_parquet` are at `engine.py:332-333`, as well as `:334`.
21. [minor] CN-1 dependencies omit OR-6 (`RunResult`), OR-7 (`Time` type; affects `frame_hash`), PY-1 (frame export), PY-2 (strict kwargs), PY-4 and OD-2. The schedule order already satisfies them; list them.
22. [minor] CN-8: `scratch/` currently has 61 top-level entries and 7.0 GB, of which `scratch/fitrun` is 6.2 GB. Record the values measured at execution rather than "at least 62". Also, `paths.sweep_scratch` remains and is exactly the broad sweep CN-8 forbids; name it as not to be used.
23. [minor] CN-9:
    - `PINNED` (`check_losses.py:24-26`) holds 4-decimal values. Moving to `float.hex` needs re-pinning on the B0 engine, and again at CN-4 after the numeric fixes. Neither step is stated.
    - The test must monkeypatch `scale.calibrate_group`, since `main()` otherwise runs a 20-minute fit.
    - `tools/` is not a package, so import it by path.
24. [minor] CN-4: "the delta between B0 and B1 is exactly the sum of the numeric fixes' individual deltas" is false for a data-dependent DE trace. Drop it.
25. [minor] CN-5: the test needs `calibrate_group`'s sizing extracted into a function; say so. Defect confirmed at `scale.py:630-637`.
26. [minor] CN-11: fit-time `RunResult` per CN-7 has one column and no metrics. Replay "with every report destination attached" cannot be bitwise equal to it as a whole. Compare the fit-time selection only, then emit the full reports.
27. [minor] CN-12: egobox is a new consumer dependency, which contradicts "changes no production code". Add it via `uv add --group` under the new-dependency operator checkpoint.
28. [minor] B-3:
    - The consumer reaches `Dwelling.from_hpxml` kwargs at `py_dwelling.rs:2420-2430`. `py_config.rs:582-648` is `DwellingConfig`. Fix both paths.
    - Specify how an explicitly passed `None` differs from an absent kwarg.
    - "The same class of delta that CN-1's baseline gate absorbs" is wrong: CN-1 is bitwise, and CN-4's B1 absorbs it.
29. [minor] B-9: the `write_output`/`retain_batches` half cannot wait until row 64, because OR-3 (row 21) removes those fields and `fleet.rs:779-799` must change then. Limit B-9 to year and offset, and add a B-1 dependency for the actual-versus-typical rules.
30. [minor] B-2 item 2: OR-5 already makes NaN during warm-up fatal, so `warmup_fails_on_nan_zone_delta` passes before B-2. State this and keep only the explicit NaN-aware fold as defence in depth.
31. [minor] B-10: the CWD-relative default `PathBuf::from("defaults")` (`blueprint.rs:120-123`) survives the change. Remove it: require an explicit path, or use the packaged defaults for the wheel. Record a zero-delta golden measurement rather than "none".
32. [minor] B-5 evidence: `"PV #1"` reaches `OTHER` only where the `" #N"` suffix is not stripped (`mod.rs:606`, `blueprint.rs:201`). `metrics.rs:1252-1258` does strip it. `is_ev` is at `columns.rs:1140` and is used at `columns.rs:870` and `mod.rs:534`. Cite these exactly.
33. [minor] B-1 evidence: `compute_annual_offset` starts at `environment.rs:823`.
34. [minor] Numeric entries B-2, B-3, B-5, B-6, B-8, B-10, B-11 and B-13 need golden re-capture, but none lists M-1/M-2 as a dependency. The schedule order satisfies this; list them.
35. [minor] Order B-6 before B-11, or have B-11 state Mode's null handling. B-6 introduces nulls that Mode and the fleet fold must treat.
36. [minor] Schedule: BR-5 and SL-1/2/4/5/6 have no row. Say explicitly that they close in rows 23, 43, 51, 48 and 49. OD-5's "(or folds into row 30)" must be decided.

## Verified correct

- Consumer base: `c71c542` exists. The submodule is pinned at `0ec757cb` = `ebdb1a17` + `4fa9bc43`/`6ec4fead`/`0ec757cb` (PR #27 content). `pyproject.toml` has an editable path source `ochre-next = { path = "HARES" }`.
- Consumer call sites confirmed: `engine.py:311-315` (scratch/out) and `:384-405`; `hares_der_remeasure.py:67`; `hares_l1_archetype_sweep.py:79`; `hares_ua_xval.py:74`; `scale.py:271`; `schema.py:375-376`.
- CN-5 defect is real: `scale.py:630` defaults `workers` before the `is None` guard at `:633`.
- CN-6's description of the evaluation pattern is real: `fit.py:315-323` evaluates nominal and warm start standalone and again in the batch, and `:352-357` re-runs the optimum. The "Latin-hypercube" comment is false (`fit.py:319-321`).
- CN-9 defect is real: `check_losses.py:34` rounds, and `:44` returns 0 unconditionally.
- CN-11's slot-overwrite premise is real (`fit.py:214-221`, `m{i:03d}` per slot).
- `scratch/fitrun` exists (6.2 GB of 7.0 GB).
- `uv run --active` and `uv sync --active` exist in uv 0.12.8.
- B-2 evidence is accurate: the call at `mod.rs:2740` with `(0.5, 25)`; `_init_dur` at `:2725`; the `f64::max` fold at `:5040-5044` (NaN is swallowed); the idd default `.4` at `Energy+.idd.in:543-548`; `DefaultMaxNumberOfWarmupDays(25)` at `DataHeatBalance.hh:96`; the Rust default 7 days at `mod.rs:1812`; the warning at `:2779-2786`.
- B-4 symptom is real: the boundary step goes to the closing month in the dwelling path. The location of the fix is wrong (major 13).
- B-6 sites are real (`mod.rs:6607`, `6753-6791`, `6955-6976`).
- B-7 is real: `has_envelope_columns` depends on which columns exist (`metrics.rs:593-604`).
- B-8 is real (`fluid_solver.rs:786-787`, `resolved.values().sum()`).
- B-9 is real (`fleet.rs:779-799`: 2019-01-01, UTC+0, `write_output: false`, `retain_batches: true`).
- B-10 is real (`blueprint.rs:124-139`).
- B-11's dtype-only selection is real (`aggregation.rs:428-441`).
- B-12 is real (`aggregation.rs:509, 636`, rfc3339 with a `Z` suffix).
- B-5's Pool/Spa-heater and `is_ev` ("television" contains "ev") mis-classifications are real.
- Schedule dependency order is otherwise sound:
  - OR-3/OR-5 come before CN-1; PY-6 before CN-11; RT-5 and PY-3 before CN-10; RT-4 and B-13 before CN-3.
  - M-1/M-2 come before all B entries; OR-1 before B-5, B-6 and B-11; OR-2 before B-6; B-2 before B-3; FL-1 before B-9.
  - B-5/B-11 and B-2/B-3 are adjacent.
  - Every B-1..B-13 and CN-1..CN-12 entry appears exactly once.
  - CN-12 is last, behind its operator checkpoint.
- Operator checkpoints present in plan.md section 3: public push, stray-file deletion, `scratch/` deletion by named directory, new consumer dependency, and the surrogate trial.
