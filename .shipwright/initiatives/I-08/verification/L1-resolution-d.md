# L1 resolution, pass d: consumer and numeric fixes

Files revised in place: `ledger/08-consumer.md`, `ledger/09-numeric-fixes.md`.
Findings resolved: every finding in `verify-L1-d.md`, and every finding in
`verify-L1-e.md` that concerns these two files. Findings about other files are
answered with what these files now require of them (sections "Contract
additions" and "Schedule constraints").

Verified against: HARES `ebdb1a17`; consumer `c71c542` (gitlink `0ec757cb`);
`vendors/EnergyPlus` at `f7c103167a`; the published EnergyPlus 24.1 pages
(Input Output Reference "Weather Data Hourly Interpolation", Engineering
Reference "Climate Calculations"), which match the vendored text. Three
mechanism claims were measured on a scratch copy of the engine and a scratch
venv (nothing in either repository was written):
- `VIRTUAL_ENV=<venv> uvx --from 'maturin>=1.15,<2' maturin develop --uv`
  installs the engine plus the engine's `dev` dependency group (pytest,
  maturin, psychrolib, ruff 0.16.9) into the target venv.
- `uv pip install --python <venv>/bin/python --no-deps --reinstall-package
  ochre-next --editable <tree>` builds with the release profile and installs
  only the engine.
- `uv sync --locked --extra engine --no-install-package ochre-next --dry-run`
  in the consumer would uninstall only `ochre-next`; plain `uv sync --locked
  --dry-run` would also uninstall `pyarrow`.

Counts: verify-L1-d: 36 findings, 34 fixed, 2 fixed in these files with the
remainder handed to the integrator (18, 36), 0 rejected. verify-L1-e: 27
findings concern these files: 24 fixed, 3 fixed in part with the rest
rejected as outside these files (5, 22, 55). 12 contract additions, 9
schedule constraints, 9 operator questions.

## verify-L1-d

1. fixed: `08-consumer.md` "Engine-under-test mechanism". "uv sync --locked
   --extra engine --no-install-package ochre-next" / "uv pip install --python
   .venv/bin/python --no-deps --reinstall-package ochre-next --editable
   /home/rich/src/hares-eut/<sha>" / "HARES_REPO=/home/rich/src/hares-eut/<sha>
   uv run --no-sync <command>". `maturin develop` is not used (measured: it
   installs the engine's `dev` group); step 4 checks the venv against the
   lock; `paths.summary()` recorded per gate run. plan.md section 3 must carry
   the same text (contract addition 1).
2. fixed: `08-consumer.md` "Consumer gate". "The gate passes only when the set
   of failing cases is a subset of the base failure set and the set of skipped
   cases equals the base skip set exactly." The base lists are named (3
   failures, 7 skips), `ruff check` and `build_bundle.py` included, testkit
   cases only, registration rule 1.
3. fixed: `08-consumer.md` preamble steps 1 to 4 and "B0: capture". "B0 is
   captured on the unmodified engine ... before any engine entry"; "After every
   engine row up to the last row before OR-3, the unmigrated consumer is
   captured again ... and must equal B0 bitwise"; "Every engine row from OR-3
   ... is proven neutral by comparing the consumer-shaped golden fixture". CN-1
   acceptance is now "DE-trace gate: `compare B0 <capture>` exits 0" on the
   consumer-block engine. The finding's B0-at-row-20 variant is replaced by
   B0 at row 0 plus per-row B0 checks, which isolates each engine row
   instead of recording a B0-pin to B0 delta.
4. fixed: `09-numeric-fixes.md` "Weather: what EnergyPlus specifies", B-1a,
   B-1b, B-1c. "Last Hour Interpolation ... a record's value is reached at the
   end of its hour"; "The weights are `SolarInterpolation` ... N = 4: 0.75,
   1.0, 0.75, 0.5"; "the step `[t, t + dt)` takes the EnergyPlus value of the
   timestep that ends at `t + dt`". The missed defects are stated: "at 900 s the
   first 30 minutes read the Dec 31 records"; "a run starting in a leap year
   after Feb 29 reads weather one day late". First-hour rule, wrap, actual and
   typical years, per-format label table, weight tests for N = 4 and N = 6,
   separate temperature and solar deltas.
5. fixed: CN-1 evidence census and change. "`inject.py:203` is the
   `RunSpec(rotation=...)` field, not an engine kwarg"; every listed site
   (forward_model, ami2 actuation and tariff, adapter, parallel, engine lines,
   hares_contract, coverage, suite_engine, scale:342, paths:222-227) plus the
   `Runner(scratch, ...)` test sites. "The selection comes from the caller:
   `Runner.run(spec, selection)` ... The fit path (`fit.py`) therefore requests
   one column"; forecast and probe keep their columns.
6. fixed: CN-1 "Health comes from `RunResult.health` ..., per key". NaN: "the
   same path the consumer's own `RuntimeError` takes today" (FailedRun,
   PENALTY). Unresolved: "removed ... there is nothing left to count". Rollback:
   "Recorded, not gated, exactly as today". "Recorded fact: ... in the release
   build the consumer uses, its NaN and unresolved gates always read zero."
   Unresolved columns removed from the strict set (CN-2).
7. fixed: CN-6. "Precondition: the batch values equal the standalone ones only
   if `Injector.build` is a pure function of `x` across slots" proven by
   `batch_slot_reproduces_standalone_evaluation`; "`FitResult.n_eval` drops by
   exactly 2 per fit"; `engine_runs` becomes `n_eval + 1`; `frontier.py:31-35`,
   `_mix_for`, `baseline.py:174` and `flag.py:161` handled; "`run` keeps the
   series of the current population, replaced whenever a trial member is
   accepted".
8. fixed: CN-10. "the weather file is the same for every candidate of a fit";
   "HPXML, schedule and defaults stay per candidate"; targets
   `parallel.py:37-45`; "The premise-level pool at `calibration/scale.py:667-677`
   ... is not changed by this entry"; RSS target "re-derived from the before
   measurement".
9. fixed: B-13 evidence and rule; CN-3 scope. "every layer resistance of the
   row is multiplied by `k = material_r / sum(row layer resistances)` ... the
   row's layer capacitances are kept"; test on per-layer R and C. CN-3 covers
   `opaque_r_scale` and `thermal_mass_scale` and the registry, defaults and
   coverage notes.
10. fixed: CN-2 (the strict policy). "it lands after the numeric fixes and
    before CN-4; B1 absorbs its effect ... There is no separate baseline."
11. fixed: `09-numeric-fixes.md` preamble, B-7, B-12. "No numeric change is
    folded into a structural commit ... that structural commit is
    bug-compatible"; B-7 commit `fix: compute every run metric whatever the
    selected reports`; B-12 commit `fix: label fleet output in local site
    time`. The decision needs no amendment.
12. fixed: B-2. "per-zone daily maximum and minimum zone air temperature and
    the daily peak HVAC heating and cooling ... the 100 W floor and the 1e-4 W
    threshold"; "`WarmupConfig { temperature_tolerance_c, loads_tolerance,
    min_days, max_days }`, defaults 0.4, 0.04, 1, 25"; `initialization_duration`
    replaced; NaN fails; "The doc comment states the EnergyPlus criteria".
13. fixed: B-4. "`mod.rs:6525` passes the step's end time, `current_time + dt`
    ... `evaluator.rs:618-634` is unchanged"; dwelling-level test; "The
    consumer's `calibration/ami2/tariff.py` bills move".
14. fixed: B-6. Three classes (not nullable, nullable, summed) with a site
    table; "a sum skips nulls; a mean divides by the count of non-null cells";
    "Summed cells ... a zeroed accumulator per step as today".
15. fixed: CN-13 (consumer side). "The extra and the source become
    `hares-py`"; `paths.engine()` imports `hares` only; `stock.py:331`; cache
    move under an operator checkpoint with a sha256 manifest; relies on the
    gate's skip rule. The engine entry belongs to `05-python-surface.md`
    (contract addition 2).
16. fixed: preamble ("The engine gitlink `HARES` stays at `0ec757cb` in every
    consumer commit before CN-15") and CN-15 ("git -C HARES fetch origin <sha>
    / git -C HARES checkout --detach <sha>" with the operator's permission,
    `run_all --report`, `build_bundle.py`, "No consumer push carries a gitlink
    to an engine commit that is not on public origin").
17. fixed: "DE-trace capture" and CN-0. "`scale.calibrate_group(n_homes=4,
    county="G4802010", days=365.0, generations=16, fleet_workers=4,
    verbose=False)`"; "Every float is written with `float.hex`"; the same tool
    for B0, the gate and B1.
18. fixed in part: the numeric and consumer files do not own plan.md section 3
    or BR-5/OR-5. Handed to the integrator: schedule constraint 9.
19. fixed: B-11. "Mode applies to multi-valued enum variables only ... Binary
    state variables ... keep the mean"; evidence `aggregation.rs:46, 122,
    63-79, 141-157, 1761-1776`, `columns.rs:102`; "`enum_unit_maps_correctly`
    is replaced".
20. fixed: CN-1 evidence "`:329-334` (`output_verbosity`, `write_output`,
    `output_to_parquet`, `output_path`)".
21. fixed: CN-1 dependencies "OR-1, OR-3, OR-5, OR-6, OR-7; ... PY-1, PY-2,
    PY-4, PY-6 and the Python `finish()`/`RunResult`/`health()` surface; ...
    OD-2", with the `frame_hash`/`Time` note.
22. fixed: CN-8. "`scratch/` is 7.0 GB in 61 top-level entries, of which
    `scratch/fitrun` ... is 6.2 GB"; "measured at execution"; "`paths.sweep_scratch`
    is not used".
23. fixed: CN-9 and CN-4. "`PINNED` holds the `float.hex` values of the 13
    losses recorded with B0"; "CN-4 re-pins them"; "with `scale.calibrate_group`
    monkeypatched"; "loads `tools/check_losses.py` by path with
    `importlib.util`".
24. fixed: CN-4. "The attribution is a record, not an equation: the DE trace
    is data-dependent".
25. fixed: CN-5. "Extract the sizing ... into `_pool_sizes(n_homes, workers,
    fleet_workers, cores)`".
26. fixed: CN-11. "runs with the fit-time selection and checks the loss equals
    the recorded one bitwise; then writes the full verbosity-2 report".
27. fixed: CN-12. "Operator checkpoint for the new dependency, then `uv add
    --group trial egobox`"; "Nothing under `calibration/` changes."
28. fixed: B-3. Evidence `py_dwelling.rs:2420-2440` and `py_config.rs:582-648`;
    "An absent kwarg means `WarmupConfig::default()` ... `warmup=None` passed
    explicitly means no warm-up"; "B1 absorbs it".
29. fixed: B-9. "The `write_output`/`retain_batches` lines ... are removed by
    OR-3 ... this entry does not touch them"; depends on B-1c.
30. fixed: B-2 change. "OR-5 already makes a NaN zone temperature during
    warm-up fatal; this keeps the comparison itself from ever treating NaN as
    converged."
31. fixed: B-10. "`DwellingConfig.defaults_path` becomes a required `PathBuf`;
    the working-directory default goes"; "expected zero delta ... recorded as
    measured".
32. fixed: B-5 evidence. "name-based end use without suffix stripping at
    `mod.rs:606` and ... `blueprint.rs:201` (`metrics.rs:1252-1258` does strip
    it)"; "`is_ev` ... used at `columns.rs:870` and ... `mod.rs:534`".
33. fixed: B-1a evidence "`:811-849` (`compute_annual_offset` ...)".
34. fixed: `09-numeric-fixes.md` preamble. "every entry depends on M-1 and M-2
    in addition to those it lists."
35. fixed: preamble "B-6 then B-11 (B-11 must handle the nulls B-6
    introduces)"; B-11 "Nulls (B-6) are ignored; a period with no non-null value
    is null." Schedule constraint 6.
36. fixed in part (not these files): schedule rows for BR-5, SL-1/2/4/5/6 and
    OD-5 belong to the integrator; CN-2a is decided: CN-1 "This is also the
    mechanical half of health ... so it is in this commit".

## verify-L1-e (findings concerning these files)

- 1 (package, consumer side): fixed: CN-13, quoted under L1-d 15. The engine
  rename entry and OD-1 are outside these files (contract addition 2).
- 2 (weather V4): fixed: B-1a, B-1b, B-1c (split as asked), "the step
  `[t, t + dt)` reads ... the record labelled `t` ... at 3600 s. EnergyPlus
  uses file-label time `t + dt`"; per-format label table; wind direction on
  the circle; `weather_year` and `first_hour_interpolation` config fields;
  `docs/PHYSICS_DECISIONS.md` "is created"; consumer CN-14 "`ENGINE_ROW_OFFSET`,
  `OAT_HOUR_CENTRE_OFFSET_H` and the roll go". The override surface and the
  leap-day rule are operator questions 5 and 7 (the decision log does not
  settle them).
- 3 (ordering, own commits, B0): fixed: `08-consumer.md` preamble steps 1 to 5;
  `09-numeric-fixes.md` preamble; B-7, B-12 own commits; reachability control
  in CN-1 ("`wh_energy_factor` comes back as a typed rejection naming the key").
  OD-2 is covered by the per-row B0 check (a rejected fit-path override would
  fail it). Schedule constraints 1 to 5.
- 4 (health): fixed for these files: CN-2 uses "`HealthPolicy.strict()`, which
  escalates every event kind `RunHealth` records"; the policy entry itself is
  contract addition 4.
- 5 (capabilities): rejected for these files except the consumer use of
  `resample_weather` (CN-14 depends on it; contract addition 9). The engine
  entries are outside `08`/`09`.
- 6 (reporting frequency, nulls): fixed for these files: B-6 "a mean divides by
  the count of non-null cells and the count is reported as the variable's
  companion valid-count variable"; B-11 applies to "the per-dwelling
  reporting-frequency reduction". The reduction entry is contract addition 6.
- 14 (B-2 constant pin): fixed: `warmup_threshold_matches_energyplus_default`
  removed; "`warmup_tolerance_changes_days_run`" is behavioural.
- 16 (history, dangling references): fixed in `08` and `09`: no "earlier
  draft" text remains; "review.md O-H/O-I/§3" replaced by `context.md` items
  18 to 20; the production set is defined ("four premises, 365 days, 16
  generations ... county `G4802010`").
- 17 (Python surface): fixed for these files: CN-1 depends on "the Python
  `finish()`/`RunResult`/`health()` surface"; the PY entry is contract
  addition 3.
- 19 (consumer mechanics): fixed: one mechanism; CN-15 fetch and checkout
  with permission; gate "skips ... equals the base skip set exactly"; CN-9
  re-pin; payload fields listed and "The wall-clock fields are excluded";
  AGENTS rule 5 in CN-15.
- 20 (CN-7 metrics opt-out): fixed: CN-7 restated as the consumer-side skip
  ("`Runner.run(spec, selection, *, metadata: bool = True)`"); the engine
  opt-out is operator question 4.
- 21 (CN-2b strict set, second baseline): fixed, quoted under L1-d 6 and 10.
- 22 (context items): item 16 fixed: CN-1 "`contract()` ... no longer
  simulates: it builds the dwelling, resolves each required name in the engine
  catalogue"; item 17 fixed: CN-5 "`lone_fit_pool_equals_population_when_smaller_than_cores`".
  Items 3 and 15: rejected here (RT-4 and the profiler are not these files).
- 23 (result types, warm-up in results): fixed for warm-up: B-2 "`RunHealth.warmup`
  records `WarmupOutcome { days_run, converged, worst_zone, ... }` ... and a
  `RunResult.warnings` entry naming the zones". Other types: contract
  addition 5.
- 24 (`ResolvedModel`): fixed for CN-3's needs: CN-3 tests read "the
  `ResolvedModel` effective R" and "every resolved layer capacitance"; the
  field list is contract addition 7.
- 26 (warm-up after B-2/B-3): fixed: defaults "0.4, 0.04, 1, 25"; load criterion;
  outcome recorded; "`frame_starts_at_start_time_with_exact_row_count`".
- 27 (B-13 rule): fixed, quoted under L1-d 9.
- 28 (B-8 unreachable): fixed: "every dwelling's fluid solver is built with an
  empty topology map (`solver_builder.rs:1655`)"; test with flows `1e16`,
  `1.0`, `-1e16`; "goldens re-captured and recorded as unchanged".
- 29 (B-10): fixed, quoted under L1-d 31; production callers and test helpers
  listed; "M-1/M-2 record the absolute defaults path in each capture's
  sidecar" (contract addition 8).
- 30 (V42 remainder): fixed: B-9 "A building whose weather file cannot be
  resolved or read is a fleet construction error naming the building and its
  county". The `{fips}_2018.csv` pattern is kept: it is the 2025.1 dataset's
  own file name (`fleet.rs:814-825`); the year is now read from the file.
- 36 (consumer-shape golden; nalgebra before M-1): fixed for these files: the
  consumer-shaped golden is required by the preamble step 3 and the numeric
  preamble; its definition and timing are contract addition 10 and schedule
  constraint 2.
- 37 (undecided entries, missing evidence): fixed for B-1 (no "confirm at
  implementation" text), CN-6 (precondition proven, failure mode decided),
  CN-2a (in CN-1's commit); evidence added to B-13, CN-6, CN-7, CN-11.
- 40 (physics documentation, tracing): fixed: preamble "It adds a section to
  `docs/PHYSICS_DECISIONS.md` ... The changed code path logs the inputs of its
  decision with `tracing` at `debug`", named per entry.
- 44 (CN-2a commit): fixed, quoted under L1-d 36.
- 53 (B-6 bitset): fixed for B-6 ("OR-2 (the row plan's validity bitset)");
  OR-2's text is contract addition 6.
- 54 (CN-2 wording, `registry.py:25-28`): fixed: CN-2 rewritten; CN-1 corrects
  "the stale docstring at `registry.py:25-28`".
- 55 (public record of deltas): fixed without a new file: "The commit body
  states the recorded delta in plain words, so the public history carries it."
  A changelog file is rejected: no decision creates one.
- Appendix C, context items 16 to 21: 16 CN-1; 17 CN-5; 18 CN-6; 19 CN-7 and
  CN-8; 20 CN-11; 21 CN-12. All fixed as above.
- Appendix E, consumer requirements: R3 (health readable) CN-1
  `health_is_read_from_run_health`; R6 via CN-3; R13 numeric preamble; R20 B-2
  `frame_starts_at_start_time_with_exact_row_count`; R21 B-1a/B-1b/B-1c and
  CN-14; R22 CN-11 (the engine replay export is not these files: rejected
  here).

Rejected in part (reasons above): L1-e 5 (engine capability entries), 22
(items 3 and 15), 55 (changelog file).

## Contract additions needed in plan.md section 2

1. Section 3: replace the "Consumer against an unpinned engine commit"
   paragraph with the `08-consumer.md` mechanism verbatim (worktree per engine
   commit; `uv sync --locked --extra engine --no-install-package ochre-next`;
   `uv pip install --python .venv/bin/python --no-deps --reinstall-package
   ochre-next --editable <worktree>`; the `--dry-run` check; `HARES_REPO=<worktree>
   uv run --no-sync`), and the gitlink rule (stays at `0ec757cb` until CN-15).
2. Package (2.7): an engine entry that renames the distribution, import
   package, extension module (`hares._hares`) and cache directory, with no
   alias; CN-13 depends on it.
3. Python result surface: `finish()`, `RunResult` with `.health` (fields named
   as in 2.3), `.metrics`, `reports=[ReportSpec(...)]` and
   `health_policy=HealthPolicy.strict()` kwargs, a Python `WarmupConfig`, and
   the `warmup` kwarg with absent, `None` and value cases (B-3).
4. Health (2.3): `HealthPolicy` with `empty()` and `strict()`; `strict()`
   escalates every recorded event kind (port rollbacks, rejected control
   signals, clamped actions, warm-up non-convergence) to an error naming step
   and source.
5. `WarmupOutcome` in `RunHealth.warmup`: `Disabled` or `{ days_run: u32,
   converged: bool, worst_zone, max_temperature_residual_c: f64,
   min_temperature_residual_c: f64, heating_load_residual: f64,
   cooling_load_residual: f64 }`; `SimulationConfig.warmup:
   Option<WarmupConfig>` with the four fields, defaults and validation of
   B-2; removed key `initialization_duration` names `warmup`.
6. Output model (2.2): `VariableDef.nullable` semantics (nullable: a missing
   value is null; not nullable: a missing value is a wiring defect); null
   rules for Sum, Mean (with a companion valid-count variable) and Mode; the
   row plan's validity bitset in OR-2's change; a reporting-frequency reduction
   entry that applies `VariableDef.aggregation`; OR-2 tests that an unknown
   selection name errors and that a one-column selection is bitwise equal to
   the same column at full verbosity (CN-1 relies on it); `VariableKind::Enum`
   vs `State` as B-11 uses them.
7. `ResolvedModel` field list, including per-boundary effective R and per-layer
   R and C (CN-3, B-13).
8. M-1/M-2 capture sidecar records the absolute defaults path (B-10).
9. Weather (2.7 or a new weather subsection): the field-method table and
   record-label table of `09-numeric-fixes.md`; `SimulationConfig` fields
   `first_hour_interpolation: FirstHour { Hour24, Hour1 }` (default `Hour24`)
   and `weather_year: WeatherYear { Typical, Actual }` (default `Typical`);
   `WeatherMeta.record_label`; a Python `resample_weather` that returns the
   engine's per-step values (CN-14).
10. M-2 consumer-shaped golden fixture: a ResStock study home, 900 s,
    verbosity 2, no warm-up, no resample overrides, the ResStock AMY 2018 CSV
    fixture, an override dict of the consumer's shape (`Gas Water Heater`
    `ua_w_per_k`, `ASHP Cooler` `charge_defect_ratio`) and a defaults variant
    with the `Envelope Materials.csv` resistance and capacitance columns
    scaled, full year; captured frames and metrics.
11. Precedence (2.7): the B-13 rule ("layers scaled uniformly to the declared
    assembly R minus films; LUT capacitances kept; row selection unchanged").
12. Operator checkpoints (section 3): creating and removing engine-under-test
    worktrees (removal only by the operator); moving `~/.cache/ochre_next` to
    `~/.cache/hares`; the nested `git -C HARES checkout --detach` at CN-15;
    the engine push before CN-15.

## Schedule constraints for the integrator

1. Row 0 (PR #27 merge) is followed by CN-0 (consumer) and the B0 capture
   (DE trace, the 13 `check_losses` losses, the gate's failure and skip
   lists), before BR-1.
2. The consumer-shaped golden fixture must be captured on the row-0 engine
   before any engine row that can move numbers (BR-6's nalgebra and quick-xml
   upgrades included). If the golden tool cannot exist that early, BR-6's
   non-pyo3 upgrades move after M-1/M-2, and the B0 checks of point 3 still
   cover rows 1 to the row before OR-3.
3. After every engine row from 1 to the row before OR-3: a B0 check (the
   unmigrated consumer's capture equals B0). From OR-3 to the last engine row
   before the consumer block: the consumer-shaped golden compared bitwise
   after each row. A row that fails either check is reordered after the
   consumer gate as its own numeric commit, or fixed in its row.
4. The engine package rename lands after the last B0 check (the unmigrated
   consumer imports `ochre_next.data` at `stock.py:331`), and CN-13 lands in
   the consumer block.
5. OR-8 (row 26) and FL-4 (row 34) are bug-compatible structural commits.
   B-7 and B-12 get their own rows after the consumer gate. B-9 no longer
   touches `write_output`/`retain_batches`; OR-3 changes `fleet.rs:779-799`.
6. Numeric block order: B-1a, B-1b, B-1c, CN-14, B-2, B-3, B-4, B-5, B-6,
   B-11, B-7, B-8, B-9, B-10, B-12, B-13, then CN-3, CN-2, CN-4, CN-12, CN-15.
   B-9 after B-1c and FL-1; B-11 after B-6 and after the reporting-frequency
   reduction entry; B-2 after OR-5 and the config entry.
7. Consumer block order (after the last engine row before the numeric fixes,
   gated against B0): CN-1, CN-13, CN-5, CN-9, CN-7, CN-6, CN-10, CN-11, then
   the CN-8 checkpoint. CN-10 needs RT-4, RT-5, PY-3 and the content-input
   surface first.
8. Row 69's "own new baseline" and row 70's "B1 recorded" become: CN-2 absorbed
   by B1; CN-4 is the only re-baseline.
9. verify-L1-d finding 18: BR-5's three release-build failures stay red until
   OR-5 (row 23) unless OR-5's wiring-integrity half moves before row 8; the
   schedule must pick one (not these files).

## Operator questions

1. The consumer's base gate has 3 failures (EV and DER cases in `der_inject`
   and `ami2`). Are they fixed inside I-08, or carried as the named base
   failure set until a later initiative?
2. The base gate has 7 skips (six GridLAB-D and IEEE 13 cases, one missing
   gas-furnace cell). Accept them as the named base skip set, or provision
   GridLAB-D, the IEEE 13 model and the furnace cell so the gate runs with no
   skips?
3. Intermediate consumer commits keep the gitlink at `0ec757cb` and record in
   the commit body the engine commit they were gated against; only CN-15 moves
   the gitlink. Acceptable, or must every consumer commit pin its own engine
   commit (a fetch of unpushed engine commits into `./HARES` and a `checkout
   --detach` per commit, each needing permission)?
4. Should the engine offer a per-run switch to skip computing `RunMetrics`
   (the fit path never reads them), or are metrics always computed in
   `finish()` (`dl-i08-architecture.output_model`: "Metrics do not depend on
   verbosity or on the selected reports")? CN-7 skips only the consumer-side
   calls either way.
5. Leap-year run with a typical-year file that has no Feb 29: EnergyPlus does
   not simulate that day; the HARES clock is contiguous. What does Feb 29
   read: Feb 28's records, Mar 1's records (what schedules do today), or is
   such a run an error? Weather and schedules will share the answer (B-1c).
6. A sub-hourly weather file whose interval differs from the step: EnergyPlus
   rejects it; HARES resamples today (mean when finer, PCHIP or hold when
   coarser). Reject, or keep resampling (and by which rule for point and
   solar fields)?
7. `ResampleOverrides`, `ResampleMethod`, `ochre_compat()` and the Python
   `resample_overrides` kwarg: remove (removed-key error, EnergyPlus methods
   only), or keep as an explicit opt-out from the EnergyPlus methods?
8. Python `defaults_path`: a required kwarg, or the defaults directory shipped
   in the installed package (today the wheel includes `defaults/**`)?
9. CN-3: when a scaled `AssemblyEffectiveRValue` crosses a LUT row boundary on
   a boundary with no insulation description, the selected row changes and
   the capacitances step. Accept that, or have the consumer also write the
   nominal row's insulation description into the injected HPXML, which fixes
   the row (`envelope_lut.rs:550-560` selects by insulation description when
   one is given)?
