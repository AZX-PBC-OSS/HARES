# Calibration consumer migration and performance

`~/src/HARES_CALIBRATION_SYSTEM`, branch `i08-consumer` from `c71c542`. At
that commit the engine gitlink `HARES` is `0ec757cb`. All consumer evidence
below is at `c71c542` unless an entry names `aec2e3f` (the consumer head
after CN-0, CN-16 and the capture provenance guard).

**Pin provenance (facts):**
- `0ec757cb` is `ebdb1a17` plus three commits, `4fa9bc43`, `6ec4fead` and
  `0ec757cb`: the PR #27 changes (NLR host fix, profiling bindings,
  profiling example) rebased onto `ebdb1a17`, without the fmt commit. It was
  created in the consumer's nested engine clone
  (`~/src/HARES_CALIBRATION_SYSTEM/HARES`) and is on no `origin` ref.
- PR #27 on `origin` (`refs/pull/27/head` = `89f32822`, branch
  `fix/data-host-renamed-nlr`) is four commits on `2ee83a86`: `88d8aaa4`,
  `a20f8372`, `3703b941` and `89f32822` ("style: cargo fmt").
- The Task 0 head `1d60f478` is the local merge of `89f32822` into
  `ebdb1a17` (`dl-i08-execution.task0-state`; nothing pushed). Its tree
  differs from `0ec757cb`'s only by the fmt commit's reformatting of
  `crates/hares-core/examples/profile_sim.rs` and
  `crates/hares-python/src/py_dwelling.rs` (B0 below records the check).

`decision-log.md` `dl-i08-architecture.consumer`: the mechanical consumer
migration lands before the engine numeric fixes, gated bitwise against
baseline B0; the consumer re-baselines once, after all engine numeric
fixes (B1, CN-4) and once more at the 24o checkpoint's drain, where the
reviewed physics folds (the attic, zone-volume, terrain, roughness and
capacitance work; the appliance gain split and the schedule-row event
replay) moved the estimator's fits: premise 0 now bounds at
heating_eff_scale and the estimator's own review flag names it, its
improvement moved 0.2235 to 0.2072, premise 3's 0.5166 to 0.5936; the
new baseline is captured at the engine head `585aece4` and the old
baseline's last identical check was at `699cfad2`. There is no other
baseline. B0 is captured before any
engine change that can move numbers. The order, which `00-schedule.md`
carries row by row:

1. CN-0 and CN-16 land, then B0 is captured on the Task 0 engine head (the
   `feat/output-results` commit after the PR #27 merge, before any engine
   entry) and the consumer at CN-16 (neither consumer commit changes a
   number). The consumer is at `01-baseline-repair.md` BR-6d (`5c651bc`)
   while the engine rows through PR-6 (`afa9a071`, row 18) and the rows
   committed after them land (`00-schedule.md` State column). CN-17 (gate
   and capture corrections, `00-schedule.md` row 16a) is gated on the
   latest engine commit when its gate runs (the gate engine), before the
   next consumer gate.
2. B0 check: after every engine row that the consumer at its current
   commit can still run, that consumer is captured again with the same
   command and must equal B0 bitwise. No committed engine row has a check
   run through the engine-under-test mechanism: the log holds a check for
   rows 1 to 3 only, every opening row's recorded commands ran the
   consumer against an editable install of the main engine tree, as did
   BR-11's check, and PR-1 to PR-6 have none recorded. The consumer at
   CN-17 checks them: the latest engine row through CN-17's own gate, and
   every other committed engine row in the re-check step (`00-schedule.md`
   row 16g, below), on worktrees of `8b09ac4a`, `d7f0e381`, `21d90588`,
   `26626710`, `1d471677`, `afa9a071` and `b8195c83` (OR-11). BR-11 changed only the golden
   tool, so the `8b09ac4a` capture is the cumulative check of rows 1 to
   12; a difference there is bisected over the engine commits
   `1d60f478`..`75a182a0` before any row is named. No
   gate or capture runs against an editable install of
   `/home/rich/src/HARES`; the dry-run line of step 4, which names the
   engine the venv holds, is recorded with every run. The consumer at
   CN-17 then runs every later engine row before `05-python-surface.md`
   PY-7 (the rename). CN-13 lands
   directly after PY-7, gated against that engine commit, and the CN-13
   consumer runs every engine row before PY-8 (the first row that changes
   the Python construction surface it calls). An engine row that fails its
   B0 check moves numbers the consumer sees: it is reordered after the
   consumer gate as its own numeric commit with a recorded delta, or its
   cause is fixed in that row. For a dependency upgrade
   (`01-baseline-repair.md` BR-6b, BR-6c, BR-6d) the fix is a split: the
   upgrade of the one package whose new release moves the numbers
   (identified by upgrading packages one at a time) leaves the row and
   becomes its own entry at the head of the numeric block, with its delta
   recorded like any numeric fix, and the rest of the upgrade lands in
   place.
3. From PY-8 on, the consumer cannot run the engine until it migrates. Every
   engine row from PY-8 to the end of the engine block is proven neutral by
   comparing the consumer-shaped golden fixture (`02-measurement.md` M-2,
   `consumer_shape_900s`, which has existed since before the first row that
   can move numbers) bitwise after the row.
4. Consumer block, on the last engine commit before the numeric fixes:
   CN-1, CN-5, CN-9, CN-7, CN-6, CN-10, CN-11, then the CN-8 checkpoint.
   Each commit is gated against B0 (CN-6 with its one declared rule).
5. Engine numeric fixes (`09-numeric-fixes.md`), with CN-14 after the
   weather entries, then CN-3 (after B-13, the three numeric commits of
   B-14, B-15, the B-16 triage and, when it confirms a defect, B-17,
   `11-other-defects.md` OD-9, OD-2's site 6, OD-10's numeric commit
   and OD-20, `12-inputs.md` IN-8 and IN-9, and `04-fleet.md` FL-5), CN-2
   (strict policy), CN-4 (B1), CN-12;
   after the operator's pushes, CN-15 (the one pin advance). CN-1, CN-5
   to CN-11 and CN-13 are the mechanical migration that precedes the
   numeric fixes; CN-2, CN-3, CN-4, CN-12, CN-14 and CN-15 follow them.

The performance entries (CN-5, CN-6, CN-7, CN-10) change call patterns, not
the points evaluated.

**Contract-drift rule.** The consumer's drift check
(`calibration/hares_contract_check.py:65-126`, run by the gate case
`hares_contract.contract_matches_HARES_source_or_skips`
(`calibration/tests/suite_calibration.py:249, 264-278`) and by `build_bundle.py`,
where a FAIL blocks green, `build_bundle.py:376-377` at `aec2e3f`) compares
`calibration/hares_contract.py` with the engine under test in six
categories:
- the `EndUse::` variant set across every `.rs` file;
- `CORE_OUTPUT_COLUMNS`, `END_USE_AGGREGATE_SUFFIX` and the end-use display
  names, as literals in `crates/hares-io/src/output/columns.rs`;
- `SCHEDULE_COLUMNS`, including `heating_setpoint` and `cooling_setpoint`,
  as literals in `crates/hares-io/src/hpxml_schedule.rs`;
- six equipment config structs' fields, including `HvacSetpointConfig`
  (`heating_setpoint_c`, `cooling_setpoint_c`) in `hvac/heating_config.rs`;
- `Building` fields, including `heating_weekday_setpoints_c` and
  `cooling_weekday_setpoints_c`;
- `SimulationConfig` fields (`start_time`, `duration`, `time_res`,
  `output_verbosity`, `master_seed`, `civil_timezone`) in
  `hares-io/src/config.rs` and `DwellingConfig` fields (`hpxml_path`,
  `schedule_path`, `weather_path`, `initialization_duration`, `patches`) in
  `hares-core/src/dwelling/mod.rs` (`hares_contract.py:146-150`).

The kwarg rules at `calibration/coverage.py:111-127` are checked against
the engine's kwargs list by `suite_coverage`. Rule: the first consumer row
gated after one or more engine rows runs
`HARES_REPO=<worktree> uv run --no-sync python calibration/hares_contract_check.py`
(run as a script: it imports `hares_contract` as a top-level module) and,
in that row's commit, updates `hares_contract.py`, the adapter code that
writes the item and the `coverage.py` rules for every DRIFT item it reports,
in every category. The census "`hares_contract_check.py` prints
`[hares-contract] PASS` on this row's engine" is in that row's
verification; a `SKIP` line (the check found no engine source, and exits 0
anyway, `hares_contract_check.py:27-58` at `5c651bc`) fails it. Assignment:
- CN-17 for every engine row committed up to and including its gate
  engine, each named in its record (BR-6d's gate ran the drift case too;
  OR-11 deleted the `check_invariants` feature and changed no contract
  item, so no drift item is expected from it and the census proves it);
- CN-13 for every engine row committed after CN-17's gate engine and up
  to row 26, each named in its record;
- CN-1 for rows 28 to 71a, which change: `SimulationConfig.output_verbosity`
  (removed by OR-3; `reports`, `metrics` and `health_policy` arrive with
  OR-3, OR-8 and OR-12); the column literals of `columns.rs` (OR-1 builds
  the catalogue from typed declarations); `DwellingConfig`'s path fields
  (RT-4 replaces `hpxml_path`, `schedule_path`, `weather_path` and the
  defaults path with the parsed-input sources and `settings`, `plan.md`
  §2.7); where `SimulationConfig` is defined (PY-8); the setpoint config
  (`12-inputs.md` IN-5 records the active setpoint source on it); the
  `resample_overrides` kwarg rule (IN-6);
- CN-14 for rows 79 to 81 (`first_hour_interpolation` and `weather_year`
  join `SimulationConfig`);
- CN-3 for rows 83 to 94l: `initialization_duration` leaves `DwellingConfig`
  and `warmup` joins `SimulationConfig` (B-2); the name classifiers leave
  `columns.rs` (B-5); `11-other-defects.md` OD-9 removes the generated
  `heating_setpoint` and `cooling_setpoint` literals from
  `hpxml_schedule.rs`; `12-inputs.md` IN-8 gives the two setpoint columns
  the unit degree Fahrenheit; `09-numeric-fixes.md` B-14 renames the
  equipment config fields stored in kW, kWh and kVA to SI (the contract's
  `PvConfig.capacity_kw`, `BatteryConfig.capacity_kwh`, `max_charge_kw`,
  `max_discharge_kw` and `EvConfig.max_charging_power_kw` among them, and
  the Python `PV(capacity_kw=...)` keyword the consumer's DER code calls,
  `calibration/hares_der_remeasure.py:85`); IN-9 reads HPXML vehicles into
  `EvConfig`'s SI fields; B-14's run-state commit renames the step dict key
  `net_electric_power_kw` to `net_electric_power_w`
  (`calibration/ami2/dr_actuation.py:257` reads it at `5c651bc`) and its
  control-layer commit renames the control-signal fields, so
  `PowerLimit`'s `max_kw` in the docstring at `dr_actuation.py:31` becomes
  the W field; `09-numeric-fixes.md` B-15 moves the `Vacancy`,
  `Power Outage`, `No Space Heating` and `No Space Cooling` schedule
  columns from not used to read (each checked against the HPXML periods),
  and B-17, when it lands, reads the `hot_water_clothes_washer` and
  `hot_water_dishwasher` columns and requires each wet appliance's power
  column;
- CN-15 for the engine rows committed after CN-3's engine: `02-measurement.md`
  M-4a and M-4b and any CI fix entry of the closing sequence (`plan.md` §3).

Where the engine publishes a category, CN-1 moves the check from the source
scan to the published surface, because a source scan breaks whenever a
literal moves while the contract stands: config fields, schedule columns
(with their units) and override-class fields to `hares.input_schema()`
(`12-inputs.md` IN-3, which reads schedule columns from the schedule
parser's column table, so OD-9's change to the generator does not touch
them); output column names and end uses to the catalogue
(`05-python-surface.md` PY-4). A category the engine does not publish keeps
the source scan, pointed at the file that holds the item at that commit.

## Engine-under-test mechanism (used by every entry below)

Each gated engine commit gets its own worktree. Creating a worktree never
checks anything out in an existing tree; worktrees are left in place and
removed only by the operator.

Worktrees live under `/home/rich/src/HARES/.workstrees/` (gitignored), with
one shared build directory `/home/rich/src/HARES/.workstrees/target`
(`dl-i08-execution.worktrees`). Every output a gate or a capture writes in
the consumer goes to `scratch/i08/<sha>/` (`<sha>` the engine commit under
test): `scratch/` is gitignored and in `build_bundle.py`'s `IGNORE_DIRS`
(`build_bundle.py:173-174` at `aec2e3f`), so no output can appear as an
UNTRACKED file or be zipped into the bundle.

```
# 1. engine worktree for commit <sha>
git -C /home/rich/src/HARES worktree add --detach /home/rich/src/HARES/.workstrees/<sha> <sha>

# 2. consumer venv: every locked dependency, the engine left out
cd /home/rich/src/HARES_CALIBRATION_SYSTEM
mkdir -p scratch/i08/<sha>
uv sync --locked --extra engine --no-install-package ochre-next

# 3. build the worktree engine (release profile) into the consumer venv, no dependency
#    changes, with the build backend pinned to the maturin version of the engine's own lock
printf 'maturin==%s\n' "$(sed -n '/^name = "maturin"$/{n;s/^version = "\(.*\)"$/\1/p}' \
  /home/rich/src/HARES/.workstrees/<sha>/uv.lock)" > scratch/i08/<sha>/build-constraints.txt
CARGO_TARGET_DIR=/home/rich/src/HARES/.workstrees/target \
  uv pip install --python .venv/bin/python --no-deps --reinstall-package ochre-next \
  --build-constraints scratch/i08/<sha>/build-constraints.txt \
  --editable /home/rich/src/HARES/.workstrees/<sha>
uv pip install --python .venv/bin/python "typing-extensions>=4.16"
#    the engine's runtime deps are not installed by --no-deps: the engine's
#    TypedDict module imports typing_extensions at import time; the engine
#    extra's declaration carries it so a locked sync installs it too
grep -h '^Generator:' .venv/lib/python3.*/site-packages/ochre_next-*.dist-info/WHEEL
#    prints "Generator: maturin (<the version in build-constraints.txt>)"

# 4. environment check: the only difference from the lock is the engine
uv sync --locked --extra engine --no-install-package ochre-next --dry-run
#    prints exactly one "Would uninstall" line: ochre-next from .workstrees/<sha>

# 4b. regenerate the engine-derived schema the coverage suite reads
HARES_REPO=/home/rich/src/HARES/.workstrees/<sha> \
  uv run --no-sync python -m calibration.schema --emit --all-cached

# 5. run anything
HARES_REPO=/home/rich/src/HARES/.workstrees/<sha> uv run --no-sync <command>
```

Why each step is this way:

- Step 2: plain `uv sync --extra engine` would build the engine from the
  pinned `./HARES` submodule (`pyproject.toml` `[tool.uv.sources]`
  `ochre-next = { path = "HARES", editable = true }`), and `uv sync` without
  the extra is an exact sync that uninstalls `pyarrow`, an engine-only
  dependency (`uv.lock:181-188`). `--no-install-package ochre-next` keeps
  every locked package and removes only the pinned engine (measured with
  `--dry-run`).
- Step 3: `uv pip install --editable` builds through the engine's own
  maturin build backend (`pyproject.toml` `build-backend = "maturin"`),
  release profile, and with `--no-deps` touches nothing else (measured on a
  scratch venv: `Finished release profile`, one package installed).
  `maturin develop` is not used: it installs the engine's `dev` dependency
  group into the target venv (measured: pytest, maturin and psychrolib),
  so the consumer's gate would run in an environment its own lock file
  does not describe.
  (`maturin develop --help` at 1.15.0 shows it installs into "the current
  virtualenv", the one `VIRTUAL_ENV` names, so no `--active` flag is
  needed, and it has `--uv` to install with `uv` instead of `pip`, so a
  missing `pip` does not block it either; neither is a reason to avoid it.)
  `uv pip install` resolves the build backend from the engine's
  `build-system.requires` (`maturin>=1.15.0`) at install time, not from any
  lock, so without `--build-constraints` the backend version could float
  between gated rows. The constraint names the maturin version in the
  engine's own `uv.lock` at that commit (1.15.0 at `872cf289` and at
  `75a182a0`), and the installed wheel's `Generator:` line proves which
  version built it.
- Step 4b: four `suite_coverage` cases read `results/engine_schema.json`
  (`calibration/schema.py:527`; `every_accepted_field_is_classified`,
  `static_scan_agrees_with_the_engine`,
  `kwargs_and_name_lists_match_the_binding` and
  `rotation_kwarg_is_not_building_orientation`), and all eight cases of the
  suite skip with "run `python3 -m calibration.schema --emit --all-cached`
  first" when it is absent, through the suite-level `_needs_schema` guard
  (`calibration/tests/suite_coverage.py:23-31`). The file is gitignored and
  no written gate step produced it: through B0 it was the copy written on
  2026-09-22, before Task 0, so the M-0, CN-0, CN-16 and B0 gates asserted
  the four reading cases against an engine older than the pin. Regenerating it
  against the engine under test before `run_all`, on every gated commit,
  makes them test the right engine and makes the base skip set hold on any
  clean checkout. It writes only under `results/` (gitignored, in
  `IGNORE_DIRS`).
- Step 5: `--no-sync` stops `uv run` from reinstalling the pinned engine.
  `HARES_REPO` must point at the worktree because `calibration/paths.py:24-35`
  resolves `HARES_REPO`, and `DEFAULTS` and `FIXTURES` from it, in this
  order: `$HARES_REPO`, else the pinned submodule `./HARES`, else the
  legacy sibling checkout `../HARES` (`/home/rich/src/HARES`, the main
  tree). Every gate and capture step sets `HARES_REPO` to the worktree, so
  neither fallback is reached; a run that omits it would read the
  submodule if present and the main tree otherwise, never a worktree. The
  contract-drift check reads `$HARES_REPO` itself, falling
  back to a stale default path (`hares_contract_check.py:27-31` at
  `5c651bc`); from CN-17 it resolves the source tree through
  `paths.HARES_REPO`.
- Every gate run records `calibration.paths.summary()` (a module function,
  `paths.py:188-199`: `hares_repo`, `engine_so`, `engine_sha256`,
  `resstock_cache`, `meta_parquet`, `n_cached_buildings`, `n_tmy3_epw`),
  the engine commit and the `Generator:` line of step 3. After PY-6,
  `build_info().commit` is recorded as well.
- From CN-13 on, the engine under test is the renamed distribution: step 3
  reads `--reinstall-package hares-py` and greps
  `hares_py-*.dist-info/WHEEL`, and step 4's one expected line names
  `hares-py`. Steps 2 and 4 keep `--no-install-package ochre-next` until
  CN-15, because the consumer's lock names the pinned engine
  (`0ec757cb`, still `ochre-next`) until the pin advances.

The engine gitlink `HARES` stays at `0ec757cb` in every consumer commit
before CN-15; each such commit message ends with the trailer
`Engine: <HARES commit>`, naming the engine commit its gate ran against, and
is runnable only through the mechanism above
(`dl-i08-architecture.consumer_gate`). CN-15 moves the gitlink once, to a
pushed engine commit.

## Consumer gate (the verification step of every entry below)

After step 4b of the mechanism:

```
HARES_REPO=<worktree> uv run --no-sync python -m calibration.tests.run_all > scratch/i08/<sha>/gate.txt
HARES_REPO=<worktree> uv run --no-sync python calibration/hares_contract_check.py > scratch/i08/<sha>/contract.txt
HARES_REPO=<worktree> uv run --no-sync ruff check
# only on a gate run that ends in a commit: build_bundle.py on the staged change,
# writing on the commit once it is made
HARES_REPO=<worktree> uv run --no-sync python build_bundle.py --out scratch/i08/<sha>/bundle.zip > scratch/i08/<sha>/bundle.txt
# ids and writing() exactly as plan.md §3's verification block defines them
writing ~/src/HARES_CALIBRATION_SYSTEM                                            # [writing]
```

- `run_all` returns 1 only on failures (`calibration/tests/run_all.py:127-141`),
  never on skips, so its exit code alone is not the gate. The gate reads
  `gate.txt`: each case line starts with ` FAIL `, ` skip ` or `  ok  `
  under its suite name line (`calibration/tests/testkit.py:190-213`).
- `contract.txt` holds the line `[hares-contract] PASS` (the contract-drift
  rule above; the first consumer row after engine rows updates the
  contract until it does). Its exit code alone is not the gate: a check
  that finds no engine source prints `[hares-contract] SKIP` and exits 0
  (`hares_contract_check.py:52-58` at `5c651bc`). A `SKIP` or `FAIL` line
  fails the gate.
- The gate passes only when the set of failing cases is empty and the set
  of skipped cases equals the base skip set exactly
  (`dl-i08-architecture.consumer_gate`). A missing engine, a missing cache
  or an import failure shows up as new skips and fails the gate.
- Base skips, the accepted environmental base (external tools not
  installed): `fit.a_furnace_home_fits_without_proposing_impossible_equipment`
  (gas-furnace cell definition not available) and six `ami2` GridLAB-D cases
  (`GridLAB_D_reproduces_the_sweep_when_asked_to_model_the_same_feeder`,
  `the_HELICS_federation_and_batch_GridLAB_D_agree_on_the_same_feeder`,
  `coupled_mode_puts_the_phases_on_one_conductor_so_they_can_actually_interact`,
  `core_loss_is_a_standing_draw_that_does_not_depend_on_the_load`,
  `a_voltage_regulator_holds_the_feeder_up_instead_of_letting_it_sag`,
  `the_solver_reproduces_a_published_test_feeder`; GridLAB-D and the IEEE 13
  model are not installed). `02-measurement.md` M-0 recorded seven skips
  and no failure at `c71c542` (329 passed, 7 skipped) and B0 the same seven
  at `aec2e3f` (332 passed, 7 skipped), both on the 2026-09-22 schema. The
  base on a schema regenerated against the engine under test is recorded by
  CN-17's gate, the first that runs step 4b on an engine-under-test
  worktree. The base skip set
  is those seven names; a skip of any of the eight `suite_coverage` cases,
  or of `hares_contract.contract_matches_HARES_source_or_skips`, is a gate
  failure (step 4b did not run or failed, or the engine source was not
  found), never part of the base.
- Base failures: the three cases the committed `docs/Test_Report.md` lists
  as failing were CN-16's; none fails on the Task 0 engine (M-0). No case
  may fail.
- `ruff check` exits 0.
- `build_bundle.py` runs only on a gate run that ends in a commit, on the
  staged change: its VERIFY section shows no MISSING and no UNTRACKED file,
  and its test-count line matches `gate.txt`. It exits 1 while any base
  skip remains (green needs zero failures and zero skips,
  `build_bundle.py:376-377` at `aec2e3f`); that exit is expected and
  recorded. It rewrites `INDEX.txt`, `MANIFEST.txt` and `SETUP.txt`, which
  are staged and committed with the entry, and writes the bundle to
  `scratch/i08/<sha>/bundle.zip` through its `--out` argument (CN-17 adds
  it: at `aec2e3f` the zip always goes beside the repository,
  `build_bundle.py:384-391`). A gate run that ends in no commit (a B0
  check, the row 16g re-check, a measurement) runs the other gate lines
  and records that the bundle step did not run, so it changes no file in
  either repository.
- `writing` runs on the entry's commit, after it is made (the consumer
  half of `plan.md` §3's `[writing]` line: no em dash, planning ID, review
  reference or agent reference in the commit message or an added line; the
  `Engine:` trailer carries only a commit hash). A match fails the gate;
  the line is reworded in the entry's commit before the next entry starts.
- New tests are testkit cases in a named suite
  (`calibration/tests/suite_*.py`), not pytest functions; pytest is not a
  consumer dependency. Every new file is registered in `build_bundle.py`
  `SOURCE` (consumer `AGENTS.md` rule 1).

## DE-trace capture (the bitwise gate)

The production calibration set is the one the consumer's fleet-concurrency
proof used (`docs/Red_Team_Record.md:94-99`): four premises, 365 days, 16
generations, through `scale.calibrate_group`, county `G4802010`
(`scale.py:64`, `DEFAULT_COUNTY`), premises
`feeder.build_group("G4802010", mix=scale._mix_for(4))[:4]`
(`scale.py:638-639`), seed 0 (`scale.py:569`).

```
HARES_REPO=<worktree> uv run --no-sync python tools/capture_de_trace.py capture --out scratch/i08/<sha>/capture.json
HARES_REPO=<worktree> uv run --no-sync python tools/capture_de_trace.py compare \
  /home/rich/src/HARES/.shipwright/initiatives/I-08/baselines/B0-de-trace.json scratch/i08/<sha>/capture.json
```

The tool (CN-0) runs `scale.calibrate_group(n_homes=4, county="G4802010",
days=365.0, generations=16, fleet_workers=4, verbose=False)`. A capture
"equals" a baseline when `compare` exits 0: `compare` diffs only the
per-premise payload (`premises[*]`, every recorded field); the header
(engine commit, `paths.summary()`, consumer commit, `uv.lock` sha256, pool
sizes, host) is provenance, written and never compared, because the engine
commit, the worktree path, the `.so` hash, the lock hash (from BR-6d) and
the inner pool size (from CN-5) differ between any two gated runs by
design. B0 and B1 are stored in `.shipwright/initiatives/I-08/baselines/`
with the engine commit, the consumer commit and the `uv.lock` sha256.

## CN-0: bitwise DE-trace capture tool and registry baseline

**Kind:** initiative work (the tool every consumer gate below depends on).

**Problem:** the bitwise gate the decision log requires has no tool. No
script dumps the DE traces, `HomeCalibration` does not carry the fitted
vector, and `tools/check_losses.py` compares rounded losses (CN-9). The
consumer's registry check fails at base on three unregistered files, so the
gate's no-UNTRACKED rule cannot pass.

**Evidence:** `calibration/scale.py:467-501` (`HomeCalibration` fields:
trace, loss, n_eval, n_rejected, scores and three wall-clock fields
`calibrate_s`, `truth_run_s`, `score_s`; no fitted vector);
`scale.py:573-583` (fields filled from the `FitResult`, `r.x` dropped);
`build_bundle.verify()` at `c71c542` returns untracked
`docs/Red_Team_Record.md`, `tools/check_losses.py`, `tools/race_gridlabd.py`.

**Change:**
- `HomeCalibration` gains `x: dict`, filled from `r.x` in
  `_calibrate_one_premise` (`scale.py:574` region). A recorded value; no
  engine call and no numeric change.
- New `tools/capture_de_trace.py` with two subcommands:
  - `capture --out <file>`: runs the production calibration set above and
    writes JSON. Header: engine commit, `paths.summary()`, consumer commit,
    `uv.lock` sha256, `fleet_workers`, `workers`, host name. Per premise:
    `truth_bldg`, `cand_bldg`, `county`, `hvac`, `trace`, `loss`, `nominal_loss`,
    `improvement`, `n_eval`, `n_rejected`, `n_free`, `free_set`,
    `dropped_factors`, `at_bound`, `flag`, `flag_codes`, `x`, `in_sample`,
    `out_sample`. Every float is written with `float.hex`. The wall-clock
    fields are excluded.
  - `compare <a> <b> [--rule <name>]`: diffs the two `premises` lists
    field by field (parsed values, so key order is not data and element
    order is), exits 1 on any difference printing each differing field
    path, exits 2 on an unreadable or malformed input, exits 0 when the
    payloads are identical. The header is never compared. The only named
    rule is CN-6's `n-eval-without-parent-probes`; until CN-6 implements
    it, passing any rule is an "unknown rule" error.
- Register `tools/capture_de_trace.py`, `tools/check_losses.py`,
  `tools/race_gridlabd.py` and `docs/Red_Team_Record.md` in
  `build_bundle.py` `SOURCE`.

**Acceptance test (testkit, `suite_scale`):**
- `capture_payload_is_hex_exact_and_has_no_wall_clock`: with
  `scale.calibrate_group` monkeypatched to return two fixed
  `HomeCalibration` records, the written JSON round-trips every float
  bitwise through `float.fromhex` and contains none of the three `*_s`
  fields.
- `compare_reports_a_one_ulp_difference`: two payloads that differ by one
  ULP in one trace value make `compare` exit 1 and name that path.
- `compare_ignores_the_header` lands with CN-17.

**Verification:** consumer gate.

**Commit:** `feat: add a bitwise DE-trace capture and compare tool`

**Landed:** `824805e`, with the trailer
`Engine: 1d60f47879e9719b9e63cfc73bb3a1e621e8efe5`. The capture's
provenance guard (a capture refuses, exit 2, when the installed engine's
`engine_so` is not inside `paths.HARES_REPO`) landed after it as
`f7131d7` and `aec2e3f`, recorded as `01-baseline-repair.md` BR-19
(`00-schedule.md` row 0e); its test lands with CN-17.

**Dependencies:** none. Lands before B0.

## CN-16: the three recorded gate failures

**Kind:** defect fix (`dl-i08-architecture.consumer_gate`: the consumer's
three existing gate failures are fixed in I-08).

**Problem:** the committed `docs/Test_Report.md` lists three failing cases:
`der_inject.an_ev_with_no_dispatch_policy_now_charges_like_one_with_a_policy`,
`der_inject.an_injected_der_reaches_the_whole_home_meter` and
`ami2.a_detected_but_undeclared_ev_reaches_f5_and_its_unknowable_half_is_marked_as_such`.
All three share one engine cause that is already fixed: at the pin the
report was generated against (`7a3366a7`, 2026-09-22), every equipment kept
the default `EquipmentId(0)`, so the EV driver read another device's
output as its battery state and charged on 5 of 45 days. HARES `2ee83a86`
assigns identities at assembly (`dwelling/mod.rs:1477, 2414, 3490` at
`0ec757cb`). The consumer's record attributes the three failures to
"upstream HARES#26" (`docs/Red_Team_Record.md:97-98`: "Full gate unchanged:
325 pass / 3 fail (upstream HARES#26) / 7 named skips"), and its Round 4
entry, headed "engine `ebdb1a17` (...), build `13a4d73d9786a092`", reads
"Gate: 329 passed, 0 failed, 7 skipped, 336 total." (`:144-155`). Build
`13a4d73d9786a092` is the release build of `0ec757cb` (measured), so that
gate ran on `0ec757cb`, not on `ebdb1a17` alone. Two consumer defects remain
from the episode, and the report itself is stale:
- `calibration/der_verify.py:146-148`: the assertion message blames "an
  Immediate strategy creates no driver actor", which has not been true
  since every charging strategy provisions a driver; it sends a reader to
  the wrong cause.
- `calibration/tests/suite_ami2.py:1441-1442`: the check that a detection
  exists is soft (`c.true(det is not None and det.level == 2, ...)`), so a
  missed detection continues into `f5_ev.compute` and `dataclasses.replace`
  and surfaces as `TypeError: replace() should be called on dataclass
  instances` (`suite_ami2.py:1481`) instead of the assertion that names the
  miss.

**Evidence:** `docs/Test_Report.md:3, 522-524, 548, 939-944` (committed in
`015e7dc`, pin `7a3366a7`); `docs/Red_Team_Record.md:97-98`, `:144-155`
(quoted above);
`suite_der_inject.py:201-226, 257-270`; `suite_ami2.py:1390-1473`.

**Change:**
- `der_verify.py:146-148`: the message states the measured symptom (the EV
  charged on N of M days, below the expected share) without naming a cause
  the code does not establish.
- `suite_ami2.py`: when `det is None` the case fails at once with the
  assertion naming the missed detection, before any use of `det`.
- None of the three fails on the Task 0 engine: M-0's base gate at
  `c71c542` against `1d60f478` reads 329 passed, 0 failed, 7 skipped
  (`02-measurement.md` M-0 item 3). No engine-side change is needed and
  the failing set is empty from M-0 on.
- `docs/Test_Report.md` is regenerated by CN-15 with the pin advance
  (consumer `AGENTS.md` rule 5), not here.

**Acceptance test (testkit):**
- `suite_ami2.a_missed_detection_fails_with_its_own_message`: with the
  screen monkeypatched to return no detection, the case under test fails
  with the missed-detection assertion and raises no `TypeError`.
- The three named cases pass on the Task 0 engine.

**Verification:** consumer gate, with an empty failing set.

**Commit:** `fix: name the measured EV symptom and stop the AMI case at a missed detection`

**Landed:** `a6d1345` (trailer `Engine: 1d60f47879e9719b9e63cfc73bb3a1e621e8efe5`).

**Dependencies:** CN-0; M-0. Lands before B0.

## B0: capture (measurement, no commit)

On the Task 0 engine head `1d60f478`, with the consumer at CN-16 (and the
capture provenance guard), record:
- the tree comparison with the pin: `0ec757cb` fetched from the nested
  clone into the engine repository (`git -C ~/src/HARES fetch
  ~/src/HARES_CALIBRATION_SYSTEM/HARES 0ec757cb`), then `git -C ~/src/HARES
  diff --stat 0ec757cb 1d60f478` and the two tree ids. Expected result:
  formatting-only differences in `crates/hares-core/examples/profile_sim.rs`
  and `crates/hares-python/src/py_dwelling.rs`, whose hunks equal those of
  `git diff 3703b941 89f32822` (the fmt commit) apart from blob ids and line
  offsets. Any other difference stops B0;
- `capture_de_trace.py capture` output;
- the 13 losses of `tools/check_losses.py`'s fit, with `float.hex` (used by
  CN-9);
- the gate's failure and skip lists.

All are recorded in `02-measurement.md`'s log, and the files are kept in
`baselines/`.

**Recorded:** `02-measurement.md` M-0 and B0 (2026-09-30). Tree ids
`81dc6655` (`0ec757cb`) and `b48a53cb` (`1d60f478`); `diff --stat`: the two
files above, 18 insertions and 4 deletions, hunks equal to `89f32822`'s.
Baseline files `B0-de-trace.json`, `B0-losses.json` and `B0-record.json`
at consumer `aec2e3f`, engine build sha256 `28be65c0835f3622`; the
production set reproduced bitwise across three captures; gate 332 passed,
0 failed, 7 skipped (on the 2026-09-22 schema, see CN-17).

## CN-17: gate and capture corrections before the next consumer gate

**Kind:** initiative work (the gate every consumer entry depends on).

**Problem:** six defects in the gate as CN-0, CN-16, B0 and BR-6d ran it.
1. `compare` already diffs only the per-premise payload
   (`tools/capture_de_trace.py` `cmd_compare`, `diff_fields(a["premises"],
   b["premises"], "premises")` at `aec2e3f`), but no case pins that the
   header is ignored, so a later edit could compare it and fail every B0
   check on provenance alone.
2. The engine-derived schema the coverage suite reads was not a gate step
   (step 4b of the mechanism): the M-0, CN-0, CN-16 and B0 gates asserted
   the four `suite_coverage` cases that read it against the copy written
   on 2026-09-22.
3. The gate text wrote `gate.txt` into the consumer root (UNTRACKED for
   `build_bundle.verify()`, and zipped into the bundle) and did not pin the
   maturin build backend (mechanism steps 2 and 3).
4. A contract check that cannot find the engine passes. The check reads
   `$HARES_REPO`, else a stale default path that exists on no host
   (`DEFAULT_REPO`, `hares_contract_check.py:27`), not the resolver every
   other consumer path uses (`paths.HARES_REPO`: `$HARES_REPO`, else the
   `./HARES` submodule, else the legacy sibling checkout `../HARES`,
   `paths.py:24-33`); when the tree is absent it prints `SKIP` and
   exits 0. The gate case skips only when `paths.have_repo()` is false, so
   a run without `HARES_REPO` (CN-15's) finds the submodule in the case,
   while the script SKIPs with exit 0, and "exits 0" could not tell PASS
   from SKIP.
5. `build_bundle.py` always writes its zip beside the repository,
   `~/src/HARES_CALIBRATION_SYSTEM_BUNDLE_<date>.zip`, replacing the same
   day's zip, and takes no argument that writes it elsewhere, so every gate
   run wrote a file outside both repositories.
6. The capture's provenance guard (`01-baseline-repair.md` BR-19) has no
   case that drives its refusal.

**Evidence:** `results/engine_schema.json` was dated 2026-09-22 13:04 at
`aec2e3f` (B0); it was rewritten on 2026-10-02 06:01 with `engine_sha256`
`0e673f6cd097168b`, the sha256 prefix of the engine built in
`/home/rich/src/HARES` at 06:00, which the consumer venv holds as an
editable install at `5c651bc` (BR-6d, trailer `Engine: 75a182a0`); `calibration/tests/suite_coverage.py:23-31`;
`calibration/schema.py:508-550` (`emit`, `--emit --all-cached`);
`suite_scale.py` at `aec2e3f` (the two CN-0 cases; neither varies the
header); at `5c651bc`, `calibration/hares_contract_check.py:27-31`
(`DEFAULT_REPO`, `_repo()`), `:52-58` (the `SKIP` branch returns 0),
`:142-144` (the `PASS` line), `calibration/tests/suite_calibration.py:264-278`
(the case skips on `not paths.have_repo()` and otherwise asserts
`check() == 0`), `calibration/paths.py:24-41` (`HARES_REPO`,
`have_repo()`); `build_bundle.py:344` (`main()`, no argument parsing),
`:384-391` (`zip_path` beside `ROOT`, an existing zip of that name
removed) at `5c651bc`; `tools/capture_de_trace.py` `cmd_capture` and the
two `suite_scale` capture cases at `aec2e3f` (no case stubs an
`engine_so` outside `paths.HARES_REPO`).

**Change:**
- `suite_scale` gains `compare_ignores_the_header`.
- `hares_contract_check._repo()` returns `paths.HARES_REPO` when
  `paths.have_repo()` holds and `None` otherwise; `DEFAULT_REPO` is
  deleted and the `SKIP` message names `paths.HARES_REPO`. The script and
  the gate case then resolve the same tree in every run, CN-15's included.
  The module docstring says so. The script imports `paths` flat (as it
  imports `hares_contract`; `paths.py` imports only the standard library)
  and reads `paths.HARES_REPO` at call time, not at import.
- The gate reads `contract.txt` for the `[hares-contract] PASS` line
  (consumer gate above); a `SKIP` line fails it.
- `build_bundle.py` gains `--out <file>` (parsed in `main()`): the zip is
  written to that path, its parent directory created, and an existing file
  at that path replaced; no other path is written or removed. The zip
  writing moves into one function, `write_zip(root: str, dest: str)`, that
  `main()` calls. Without `--out` the zip goes where it goes today, the
  consumer's own release use (`AGENTS.md:30`); every gate passes `--out`
  (consumer gate above).
- `suite_scale` gains
  `capture_refuses_an_engine_built_outside_the_resolved_tree`, the case
  `01-baseline-repair.md` BR-19's guard lacks.
- The gate of this entry runs the corrected mechanism in full on a
  worktree of the gate engine (the latest commit on `feat/output-results`
  when the gate runs, named in the record; its row and every earlier
  engine row are in this entry's census):
  step 3 with `--build-constraints`, step 4b, outputs under
  `scratch/i08/<sha>/`. It records the base skip set on a regenerated
  schema; the BR-6d gate's 06:01 schema came from an editable install of
  the main engine tree and does not count. If any of the eight
  `suite_coverage` cases fails against the regenerated schema, the row
  stops: the cause is traced and fixed in this entry's commit (consumer
  side) or, if it is an engine change since `ebdb1a17`, reported for an
  operator decision before CN-13.
- This gate establishes `01-baseline-repair.md` BR-6d's acceptance: the
  consumer gate passes and the DE-trace capture equals B0 on the upgraded
  lock. If the trace differs, the same consumer is captured against a
  worktree of `1d60f478` (the Task 0 head): a difference there is the lock
  (BR-6d's split rule applies), and equality there puts the cause in an
  engine row, found by the re-check step below in row order. This gate's
  capture is also the B0 check of the gate engine's row.
- The contract-drift census for every engine row committed when this
  entry's gate runs (preamble), up to and including the gate engine.
- This entry changes no capture path (the compare and refusal tests, the
  contract check, the bundle's output path and the gate text only), so the
  consumer at this entry captures the same
  DE trace as the consumer at `5c651bc`; the re-check step below uses it
  for the engine rows committed before this entry.

**Acceptance test (testkit):**
- `suite_scale.compare_ignores_the_header`: two payloads with identical
  `premises` and headers that differ in every header field (engine commit,
  every `paths_summary` field, consumer commit, `uv_lock_sha256`,
  `fleet_workers`, `workers`, `host`) make `compare` exit 0 and print
  `identical`; the same pair with one `premises[0].n_eval` changed exits 1
  naming `premises[0].n_eval`.
- `suite_calibration.contract_check_resolves_the_tree_the_gate_resolves`:
  `hares_contract_check._repo()` equals `paths.HARES_REPO` when
  `paths.have_repo()` holds; with `HC.paths.HARES_REPO` (the module object
  the script holds, not `calibration.paths.HARES_REPO`) patched to an empty
  temporary directory, `check()` prints a `SKIP` line naming that
  directory; the module has no `DEFAULT_REPO`.
- `suite_scale.capture_refuses_an_engine_built_outside_the_resolved_tree`:
  with `paths.summary()` stubbed to report an `engine_so` outside
  `paths.HARES_REPO`, `capture --out <tmp>/c.json` exits 2 naming both
  trees and `<tmp>` stays empty.
- `suite_calibration.bundle_zip_goes_to_the_named_path`: `write_zip` over a
  temporary root holding two files and a `scratch/` directory, with `dest`
  under a second temporary directory, writes exactly that zip, holding the
  two files and nothing under `scratch/`, and creates nothing beside
  either temporary directory.

**Verification:** consumer gate (with step 4b), `contract.txt` holding
the `[hares-contract] PASS` line on the engine under test; DE-trace gate
against B0; no file named `*_BUNDLE_*.zip` beside the consumer repository
is created or modified by the gate run (listing before and after).

**Commit:** `fix: the contract check reads the engine tree every gate resolves, and the bundle is written where the gate names`

**Dependencies:** CN-0, B0, `01-baseline-repair.md` BR-6d,
`03-output-reporting.md` OR-11, `10-slop-cleanup.md` SL-7. Position:
`00-schedule.md` row 16a. Its gate engine is the latest commit on
`feat/output-results` when the entry runs (the gate engine, named in the
entry's record), and its census covers every engine row committed up to
and including it; before CN-13.

## B0 re-check (`00-schedule.md` row 16g)

**Kind:** measurement (no commit).

**Problem:** no committed engine row has a B0 check run through the
engine-under-test mechanism. The log holds a check for rows 1 to 3 only;
every opening row's recorded commands, BR-11's B0 check among them, ran
the consumer against an editable install of the main engine tree; PR-1 to
PR-6 (rows 14 to 18) and every engine row committed since (OR-11,
SL-7, RT-3, RT-3a and on) have none recorded through the mechanism. The
gate engine's row is checked by CN-17's gate.

**Change:** the consumer at CN-17 is captured through the
engine-under-test mechanism (steps 1 to 5, the dry-run line of step 4
recorded) on worktrees of these commits, in row order, and each capture is
compared with B0: `8b09ac4a` (BR-11, row 13a), `d7f0e381` (PR-1),
`21d90588` (PR-2), `26626710` (PR-3), `1d471677` (PR-5), `afa9a071`
(PR-6), `b8195c83` (OR-11), `7b7a6a7d` (SL-7), `44da330c` (RT-3), and
every later engine commit on `feat/output-results` before the gate
engine (`git log --oneline 44da330c..<gate engine>^`, each listed in the
record). The capture of CN-17's gate (the gate engine) closes the
sequence, entered in this step's record and not repeated. BR-11 changed only
`tools/frame_golden`, so the engine at `8b09ac4a` is row 12's
(`75a182a0`) and its capture is the cumulative check of rows 1 to 12.
Each result is recorded in `02-measurement.md`.

**Acceptance:** every capture compares `identical` with B0. A difference
at `8b09ac4a` is bisected over the engine rows' commits
`1d60f478`..`75a182a0` (worktrees, in row order, each capture recorded)
until the first commit whose capture differs is found; no row is named
before that. A difference first seen at a later commit stops the step
there and names that commit's row. The cause is a number that row moved,
and it becomes a new `09-numeric-fixes.md` entry with its own row in the
numeric block (`00-schedule.md`, contingent rows); the committed row is
not rewritten.

**Dependencies:** CN-17 (the consumer and the corrected mechanism, and
the gate engine that closes the sequence).

## CN-1: migrate every call site to the report API and `RunHealth`

**Kind:** initiative work (`context.md` item 16).

**Problem:** the consumer drives the engine through the removed output
surface (`write_output`, `output_to_parquet`, `output_path`, `rotation`), pays
for a Parquet file it never reads, derives health from frame columns that
release builds never fill, and checks its column contract with a live
two-day simulation.

**Evidence (census at `c71c542`):**
- `calibration/engine.py:64-66` (`COL_NAN_TEMP`, `COL_UNRESOLVED`,
  `COL_ROLLBACK`), `:78`, `:114` (`RunSpec.rotation`), `:219`, `:224`,
  `:237`, `:301-317` (`Runner.scratch`, `self.out = ... "run.parquet"`),
  `:329-334` (`output_verbosity`, `write_output`, `output_to_parquet`,
  `output_path`), `:338-339` (`rotation`), `:384-397` (health from frame
  columns; raises only on NaN and unresolved counts), `:406-413`
  (`_engine_metrics(dw)`, `_equipment_metadata(dw)`), `:494-508` (`smoke`),
  `:511-535` (`contract()`: a two-day run, then a column check that includes
  the three health columns).
- `calibration/parallel.py:1-16` (docstring: one `run.parquet` per runner),
  `:57-70` (`worker_runner(scratch, grid)`), `:75-82` (`_Job.scratch`),
  `:103-108` (`_execute`), `:190-249` (reducers; `:208` writes `health_*`
  keys).
- Engine-output scratch (`Runner(scratch, ...)` and `run_specs(...,
  scratch=...)`, each writing `run.parquet` under its directory):
  `calibration/fit.py:236,265,356`, `calibration/forecast.py:214,248`,
  `calibration/probe.py:91-94,125-128,159-162,273-274`,
  `calibration/truth.py:155,228`, `calibration/identify.py:108,110,158`,
  `calibration/der_experiment.py:156,309`, `calibration/flag.py:215`,
  `calibration/reactive.py:108-109,268,373`, `calibration/residual.py:235`,
  `calibration/parallel.py:257-266` (`scaling(..., scratch=)`),
  `calibration/engine.py:496,513` (`smoke`, `contract`),
  `calibration/ami2/cvr_actuation.py:79`, `calibration/ami2/dr_actuation.py:206`,
  `calibration/ami2/run.py:196`, `calibration/ami2/phase_experiment.py:127`,
  `tools/bench_calibration.py:55`.
- Fit-level `scratch` arguments, which are a different thing: the
  directory under which the `Injector` writes each population slot's
  injected HPXML, schedule and defaults variant (`inject.py:44`,
  `Injector.scratch`; `fit.py:155`, `Fitter(scratch=...)`; slot
  directories `m{i:03d}`, `fit.py:215-222`), passed through by
  `der_experiment.py:110,119,129-167,318-332`, `extrapolate.py:112`,
  `feeder.py:96`, `forecast.py:185,282,301`, `frontier.py:212`,
  `identify.py:119`, `premise.py:140,172`, `scale.py:569`,
  `flag.py:230`, `ami2/run.py:207`.
- Direct engine construction outside `Runner`:
  `calibration/forward_model.py:256-291` (`from_hpxml` with `duration_s`,
  `time_res_s`, `defaults_path` and `output_verbosity`; an `import HARES`
  fallback at `:260-262`), `calibration/scale.py:262-274` (`from_hpxml`
  with `duration_s`, `time_res_s`, `defaults_path`, `write_output=False`),
  `:334-338` (`DwellingConfig(..., defaults_path=)`), `:362`
  (`SteppableFleet.from_configs`), `calibration/inject.py:248-252` and
  `calibration/tests/suite_der_inject.py:342-345`
  (`DwellingBlueprint.from_hpxml` with the old keywords),
  `calibration/ami2/cvr_actuation.py:133-134`,
  `calibration/ami2/dr_actuation.py:211-212`,
  `calibration/ami2/tariff.py:85-89`, `calibration/run_calibration.py:209-211`
  (`HaresForwardModel(defaults_path=...)`),
  `calibration/hares_der_remeasure.py:64-71`,
  `calibration/hares_l1_archetype_sweep.py:77-81`,
  `calibration/hares_ua_xval.py:72-75` (`write_output=True`),
  `calibration/schema.py:363-424` (below), `tools/bench_calibration.py:86-105`,
  and the tests `suite_ami2.py:813,862,1752,1970-1972` (at `5c651bc`; CN-16 inserted
  41 lines ahead of these, so the pre-CN-16 line numbers no longer point here).
- `calibration/ami2/tariff.py:79-91`: `bill()` runs
  `set_electric_tariff`, `simulate`, `finalize_billing` and
  `billing_summaries`; PY-9 deletes `finalize_billing`.
- The global verbosity: `engine.py:68` (`OUTPUT_VERBOSITY = 2`), read at
  `:330` and `:533`; callers set it to 6 (`ADAPTER_VERBOSITY`,
  `ami2/adapter.py:72`) or 0 and restore it: `ami2/cvr_actuation.py:81-89`,
  `ami2/run.py:192-225`, `suite_ami2.py:54-63, 400-411, 882-888,
  1182-1189, 1302-1306, 1771-1776, 1964-1978, 2062-2066, 2122-2131,
  2187-2197` (at `5c651bc`: the last five ranges shift 41 lines later than a
  pre-CN-16 reading).
- `calibration/engine.py:411-472` (`_engine_metrics`: reads `dw.metrics()`,
  which PY-9 deletes, through `getattr` inside an `except Exception` that
  returns `{}`, so a missing method would silently empty every run's
  metrics; and `dw.dwelling_fatal` at `:471`), `:272-298`
  (`_equipment_metadata`, whose `except Exception` at `:296` likewise
  swallows a failure); the readers of those metrics keys:
  `ami2/adapter.py:342-372`, `ami2/services.py:40-49`,
  `suite_ami2.py:683-694, 743, 764, 772, 1208`.
- `calibration/schema.py:249-251` (`known_kwargs()` regex-scans
  `KNOWN_KWARGS` in `py_dwelling.rs`, which `05-python-surface.md` PY-8
  deletes) and `:363-424` (`Prober` sends `duration_s`, `time_res_s`,
  `defaults_path`, `output_verbosity` and `write_output` to `from_hpxml`
  and calls `initialize()`, `:392-395, 421-424`). After PY-8 every probe
  would fail inside the broad `except`, every class would read
  `NOT_WITNESSED`, the kwarg surface would be empty, and the
  `suite_coverage` cases would fail at this entry's gate.
- `calibration/ami2/adapter.py:342-372` (reads `res.metrics` and
  `res.health["nan_temperature_count"]`).
- `calibration/hares_contract.py:147-150` (the `SimulationConfig` field set
  the drift check compares against the engine source).
- `calibration/coverage.py:111-127` (`KWARG_RULES`: `:112-115`
  output plumbing including `rotation`; `:116-117` `start_time`,
  `duration_s`, `time_res_s`, `initialization_duration`; `:118-119` the
  seeds; `:120-121` `defaults_path`, `config`, `overrides`,
  `resample_overrides`; `:122-123` the site keywords; `:124-126`
  `setpoint_deadband_c`), `calibration/paths.py:222-227` (`sweep_scratch`
  docstring: "`write_output=False` truncates to two columns").
- `calibration/inject.py:115,156-158,203` and `calibration/registry.py:40,
  70, 305-307` (`rotation` routed from a factor into `RunSpec.rotation`;
  `inject.py:203` is the `RunSpec(rotation=...)` field, not an engine
  kwarg).
- `calibration/registry.py:25-28` (docstring calls `charge_defect_ratio` the
  negative control; the factor note at `:221-224` measures it as reachable),
  `:189-200` (`wh_energy_factor` reachability control "must come back
  INERT").
- `calibration/engine.py:32` (imports `RESAMPLE_OVERRIDES`), `grid.py:168-173`
  (its comment) and `:173`.
- Stale text on the removed surface, hits of this entry's census the
  Change must touch: `calibration/ami2/functions/p4_data_quality.py:13`
  ("`Dwelling.metrics()` … whether the SIMULATION was sound"),
  `calibration/ami2/adapter.py:23` and its docstring at `:342-353`
  (references `res.metrics` and health frame columns), `engine.py:11-12`
  (the three health counters), `:128`, `:151` (`Dwelling.metrics()`),
  `parallel.py:68` (`run.parquet`), `ami2/tariff.py:79` (the
  `finalize_billing` docstring).
- Tests: `calibration/tests/suite_engine.py:49-54,63,77,88-97,106,118,131,133`,
  `suite_der_inject.py:57,175,291,313,342-345,416,481`, `suite_fit.py:45,320`,
  `suite_ami2.py:61,406,809,857,1186,1253,1298,1749,1972,2127,2193` (at `5c651bc`),
  `suite_scale.py:181,197` (`:197` writes a fixture file named
  `run.parquet` that has nothing to do with the engine),
  `suite_coverage.py:141-153` (`kwargs_and_name_lists_match_the_binding`
  asserts `overrides`, `defaults_path`, `time_res_s`, `resample_overrides`
  and `rotation` are in the engine's kwargs list) and `:156-170`
  (`rotation_kwarg_is_not_building_orientation` pins a `rotation` row on
  the kwarg surface).

**Change:**
- `Runner(*, worker: int = 0, grid: Grid)`: `Runner.scratch`, `Runner.out`
  and `run.parquet` go. `worker_runner(grid)`, `_Job`, `run_specs`,
  `parallel.scaling` and `reactive._run` drop `scratch`, and every
  engine-output site in the evidence drops its argument. Nothing is
  written to disk for any evaluation unless a caller passes a file
  destination.
- A fit writes no file (requirement R7): every evaluation's injected
  inputs are content handed to the engine, which takes content since
  `06-runtime-memory.md` RT-4 (`content_source_run_equals_path_source_run`
  proves a content run bitwise equal to the path run).
  - `HpxmlVariant` keeps the source text and returns the edited text
    instead of writing `dst`; `schedules.variant` returns the variant's CSV
    text instead of writing it (its `_WRITTEN_CACHE` goes);
    `DefaultsVariant.write` becomes `DefaultsVariant.files(*, r_scale,
    c_scale) -> dict[str, str]`, the edited `envelope/Envelope
    Materials.csv` text keyed by its path relative to the defaults
    directory, empty at `r_scale == c_scale == 1.0`.
  - `RunSpec.hpxml` and `RunSpec.schedule` take a `Path` or a
    `Text(name: str, text: str)` (a frozen dataclass in `engine.py`;
    `name` labels the run and its provenance); `RunSpec.defaults_path`
    becomes `RunSpec.defaults`, a `Path` or a `Defaults(base: Path, files:
    dict[str, str])`. One function, `engine.engine_inputs(spec)`, turns a
    spec into the engine's input arguments: a `Path` as its string, a
    `Text` as `hares.ParsedHpxml.from_text(text)` or
    `hares.ParsedSchedule.from_text(text)`, a `Defaults` as
    `hares.ParsedDefaults(base, files=files)`. `Runner.build_kwargs` and
    every direct construction site in the evidence that passes
    `str(spec.hpxml)` (`ami2/cvr_actuation.py:133`,
    `ami2/dr_actuation.py:211`, `ami2/tariff.py:85`,
    `suite_ami2.py:813, 862, 1752, 1971` (at `5c651bc`), `suite_der_inject.py:343`,
    `tools/bench_calibration.py:86`) call it. The readers of a spec input as
    a path (`engine.py:220-221` provenance, `:349` `require_files`, `:392,
    396` messages; `suite_premise.py:340`, `suite_fit.py:300`) read a
    `Text`'s `text` or `name`.
  - `Injector.build` returns a spec of content and writes nothing. The
    fit-level `scratch` argument (`Injector.scratch`, `Fitter(scratch=...)`
    and every pass-through in the evidence) is deleted, so after this
    commit `scratch` names nothing and the census below can come back
    empty. With no file written, the injector per population slot, which
    kept slots from overwriting each other's files (`fit.py:187-200,
    215-222`: `_root`, `_pool`, `injector(i)` and their comment), becomes
    one `Injector` per `Fitter`.
  - `equipment_of` (`inject.py:238-253`) only reads the home's own files
    and stays.
- Every direct construction site in the evidence is migrated to the
  construction surface `Runner.build_kwargs` uses (below): `config=`,
  `defaults=`, a report selection, `metrics=` where the site reads
  metrics. `forward_model.HaresForwardModel` imports `hares` only (its
  `import HARES` fallback names a package that never existed and goes),
  takes `duration` and `time_res` and a `defaults` path, and reads its
  frame from the `RunResult` report; `scale.py:334-362` builds
  `SimulationConfig`-carrying `DwellingConfig`s for `SteppableFleet`;
  `inject.py:248-252` and `suite_der_inject.py:342-345` pass `config=`
  and `defaults=` to `DwellingBlueprint.from_hpxml`.
- The global `E.OUTPUT_VERBOSITY` is deleted. `engine.py` gains the named
  presets the callers use, each a report selection: `VERBOSITY_0`,
  `VERBOSITY_2`, `VERBOSITY_5` and `VERBOSITY_6` (the `ami2` adapter's,
  replacing `ADAPTER_VERBOSITY`, `ami2/adapter.py:72`). Every site that
  set and restored the global (listed in the evidence) passes its preset
  as the explicit selection argument of `Runner.run` or `run_specs`.
- `ami2/tariff.py:79-91`: `bill()` drops `finalize_billing()`.
  `simulate()` returns after `finish()`, which closes the final billing
  period, partial or not (`plan.md` §2.1 lifecycle); `RunResult` carries
  no billing, so the bill is read from `Dwelling.billing_summaries()` after
  `simulate()`, which then holds every period. The docstring at `:79-80`
  says so.
- `_engine_metrics` (`engine.py:411-472`) is rebuilt from
  `RunResult.metrics`, the `RunMetrics` pyclass of `05-python-surface.md`
  PY-9: each key the consumer records maps to a named `RunMetrics` field,
  read directly, and a missing field raises naming it (no `getattr`
  default, no `except`). A key with no `RunMetrics` field is removed with
  its readers in this commit. `dwelling_fatal` leaves the metrics dict: it
  is read from `Dwelling.dwelling_fatal` (the binding's poisoned flag,
  `crates/hares-python/src/py_dwelling.rs:996-999` at `21d90588`) into its
  own consumer `RunResult.dwelling_fatal` field, which a run that raises
  never reaches (the `FailedRun` path); `ami2/adapter.py:359`,
  `ami2/services.py:40-49` and `suite_ami2.py:683-694` read that field.
  `_equipment_metadata`'s `except Exception` (`:296`) goes the same way: a
  failure raises.
- `calibration/schema.py`: `Prober` builds its probe dwellings through
  `config=SimulationConfig(start_time=..., duration=3600, time_res=3600)`,
  `defaults=paths.DEFAULTS` and the `VERBOSITY_0` selection, with no
  `write_output`; its accepted-field list per class is read from
  `hares.input_schema()["overrides"]` (`12-inputs.md` IN-3, whose lists
  equal the strict override enforcement of `11-other-defects.md` OD-2), and
  each probe run stays as the live witness that the engine rejects an
  unknown key, so a class whose probe is accepted is still reported.
  `known_kwargs()` (`:249-251`) returns the published surface: the
  keywords of `Dwelling.from_hpxml` and the `SimulationConfig` field names
  from `hares.input_schema()`, and `removed_keys` from the same schema; it
  no longer reads Rust source.
- `KWARG_RULES` (`coverage.py:111-127`) is rewritten over that surface,
  every keyword classified once: `config`, `defaults` and `overrides`
  (the injection channels) and `reports`, `metrics`, `health_policy` and
  `output_chunk_size` (output plumbing) are `NOT_PHYSICS`; `start_time`,
  `duration`, `time_res` and `initialization_duration` (until CN-3 moves
  it to `warmup`) are the experiment's time base, `NOT_PHYSICS`;
  `master_seed` and `bldg_id` as today; `site_location` and
  `civil_timezone` `DEPLOYMENT`; `setpoint_deadband_c` as today. Each key
  in `removed_keys` is classified `REMOVED` and must be absent from the
  accepted surface. `suite_coverage.kwargs_and_name_lists_match_the_binding`
  (`suite_coverage.py:141-153`) asserts `config`, `defaults` and
  `overrides` on the surface and `defaults_path`, `time_res_s`,
  `duration_s`, `output_verbosity`, `resample_overrides` and `rotation` in
  `removed_keys` only; `rotation_kwarg_is_not_building_orientation`
  (`:156-170`) asserts that `rotation` is a removed key and keeps its
  azimuth half.
- `Runner.build_kwargs` builds `config=SimulationConfig(...)` (`05-python-surface.md`
  PY-2: simulation settings go only through the config; the Python
  keywords are the Rust field names, `plan.md` §2.7, so the consumer's
  `duration_s` and `time_res_s` become `duration` and `time_res`, and any
  site override is `site_location=SiteLocationOverride(...)`) with
  `reports=[ReportSpec(name="run", selection=<caller's>, frequency=Timestep,
  destination=Memory)]` and `metrics=True` (every caller reads metrics
  today; CN-7 turns them off on the fit path), and passes `bldg_id`,
  `defaults` and `overrides` as the dwelling-level keywords. None of
  `write_output`, `output_to_parquet`, `output_path`, `output_verbosity` or
  `rotation` is sent. The `rotation` plumbing goes with the engine key
  (`dl-i08-architecture.destinations`: no file rotation): `RunSpec.rotation`,
  `inject.py:115,156-158,203`, `registry.py:40, 70, 305-307`,
  `engine.py:114,219,338-339`, and the `coverage.py:112-115` rule text.
- The selection comes from the caller: `Runner.run(spec, selection)` and
  `run_specs(specs, reduce_fn, *, selection, ...)` carrying it in `_Job`.
  Each module-level reducer in `parallel.py` states the columns it reads:
  `reduce_total_kw` reads `Total Electric Power (kW)`;
  `reduce_total_and_reactive` adds `Total Reactive Power (kVAR)`;
  `reduce_total_and_hvac` adds the two HVAC end-use columns;
  `reduce_summary` adds `COL_INDOOR_T`; `reduce_hash` and
  `reduce_hash_and_summary` use `VERBOSITY_2` (the frame `frame_hash`
  covers today). Callers that use `Runner.run` directly (forecast, ami2,
  der, tests) pass the preset whose frame they read today: `VERBOSITY_2`
  by default, `VERBOSITY_5` when `spec.verbose_electrical`, and the
  `VERBOSITY_6` or `VERBOSITY_0` the sites that set the global used. The
  fit path (`fit.py`) therefore requests one column.
- Health comes from `RunResult.health` (`plan.md` section 2.3), per key:
  - `nan_temperature_count`: removed. NaN always fails the run
    (`dl-i08-architecture.health`); the engine error raised by `simulate()`
    becomes a `FailedRun` in `parallel._execute` (`parallel.py:103-108`) and
    `PENALTY` in `Fitter.evaluate` (`fit.py:233-247`), the same path the
    consumer's own `RuntimeError` takes today (`engine.py:390-393`).
  - `unresolved_column_count`: removed. The catalogue resolves every
    variable at assembly (OR-1) and a missing observation channel is a
    wiring defect that fails the run (OR-5); there is nothing left to count.
  - `port_rollback_count`: read from `RunResult.health.port_rollbacks`.
    Recorded, not gated, exactly as today (`engine.py:384-397` raises only on
    NaN and unresolved counts). CN-2 gates it.
  - Every `RunHealth` field is recorded as `health_<field>` in
    `reduce_summary` and in `RunResult`'s health section;
    `WarmupOutcome` serializes as `health_warmup = {state: "ran"|"disabled",
    days_run, converged}` (the JSON form `reduce_summary` records).
  - Recorded fact: at `0ec757cb` all three counters are written only under
    `cfg(feature = "observe")` (HARES `mod.rs:5306-5311, 6275-6277,
    6623-6634, 6658-6680`), so in the release build the consumer uses, its
    NaN and unresolved gates always read zero. A NaN conditioned-zone
    temperature after warm-up already fails the run as `InvariantViolation`
    (`mod.rs:6795-6820`) and reaches the same `FailedRun` path.
- `ami2/adapter.py:342-372` reads metrics from `RunResult.metrics` and
  stops reading `nan_temperature_count` (a returned run has none).
- `contract()` (`engine.py:511-535`) no longer simulates: it builds the
  dwelling and resolves each required name in the engine catalogue (PY-4)
  with its expected unit. That resolution is the schema assertion
  (`context.md` item 16): a missing name or a different unit raises naming
  the name, the expected unit and the catalogue's. It records
  `build_info()` and the catalogue digest (PY-6) without pinning the
  digest, which changes whenever the engine adds a variable the consumer
  does not read. The three health columns leave the required list.
- The contract-drift rule (preamble) for rows 28 to 71a: every DRIFT item
  is resolved in this commit, and the categories the engine publishes move
  from the source scan to `hares.input_schema()` (IN-3) and the catalogue
  (PY-4).
- The reachability control: after OD-2 the engine rejects an override it
  cannot apply, so `wh_energy_factor` comes back as a typed rejection naming
  the key, not as an inert run. The probe (`probe.py:229-306`) turns the
  control into an assertion that the engine rejects the unapplied override
  (`dl-i08-architecture.consumer_inert_override_probe`), and the notes at
  `registry.py:189-200` and the stale docstring at `registry.py:25-28` say
  so.
- TRAP 2: after `11-other-defects.md` OD-8 a battery with no declared
  control is a construction error, so
  `suite_der_inject.a_battery_with_no_dispatch_policy_is_inert`
  (`suite_der_inject.py:178-198`) becomes
  `a_battery_with_no_dispatch_policy_is_refused`: the run without a policy
  raises the engine's construction error naming the battery, as the TRAP 1
  case already asserts for an override naming undeclared equipment
  (`suite_der_inject.py:166-175`); the live arm keeps its cycling check.
- Setpoint factors (`dl-i08-architecture.setpoint_sources`): the
  `heating_setpoint_shift_c` and `cooling_setpoint_shift_c` factors
  (`registry.py:117-126`) edit the HPXML thermostat elements, which are the
  active source only when the schedule file has no setpoint column for that
  side (`12-inputs.md` IN-5). The `Injector` reads the nominal variant's
  `ResolvedModel` setpoint sources once per premise, through
  `DwellingBlueprint.validate()` (`12-inputs.md` IN-2; the active source
  depends on which inputs exist, not on factor values), and fails naming
  the column when a free setpoint factor would edit an inactive source;
  the factor note at `registry.py:12-13` says so.
- The OCHRE resampling plumbing goes with the engine keyword
  (`12-inputs.md` IN-6): `grid.py:168-173` (`RESAMPLE_OVERRIDES` and its
  comment), the import at `engine.py:32`, `engine.py:214, 342-343`, the
  `resample_overrides` entry of the `coverage.py:120-121` rule, and the
  `irradiance.py:166` docstring.
- Stale text naming the removed surface is corrected at `engine.py:11-12,
  78, 128, 151, 237`, `parallel.py:1-16, 68`, `scale.py:342`,
  `paths.py:222-227`, `ami2/functions/p4_data_quality.py:13`,
  `ami2/adapter.py:23` and its docstring at `:342-353`,
  `ami2/tariff.py:79-80`, `suite_engine.py:133` (the provenance keys it
  lists become `time_res` and lose `resample_overrides`) and
  `suite_scale.py:181`; the unrelated fixture file `run.parquet` at
  `suite_scale.py:197` is renamed `stale_output.bin`.
- Census, run in the commit's verification, each returning nothing:
  `git grep -n -E
  'write_output|retain_batches|output_to_parquet|output_path|output_format|output_chunk_size|rotation=|RunSpec\.rotation|nan_temperature_count|unresolved_column_count|port_rollback_count|COL_NAN_TEMP|COL_UNRESOLVED|COL_ROLLBACK|run\.parquet|resample_overrides|RESAMPLE_OVERRIDES|\.metrics\(\)|\.results\(\)|finalize_billing|defaults_path|output_verbosity|OUTPUT_VERBOSITY|ADAPTER_VERBOSITY|\bscratch=|KNOWN_KWARGS'
  -- calibration tools build_bundle.py`; and, for the two keyword names
  the consumer also uses for its own fields, `git grep -n -P
  '\b(duration_s|time_res_s)=' -- calibration tools ':!calibration/grid.py'
  ':!calibration/tests/suite_grid.py' ':!calibration/ami2/adapter.py'
  ':!calibration/ami2/synthetic.py'` (the four excluded files use
  `duration_s` and `time_res_s` as fields of the consumer's own `Grid` and
  AMI series types and construct no engine object). The old keywords of a
  `DwellingConfig(...)` or `from_hpxml(...)` call (`defaults_path`,
  `duration_s`, `time_res_s`, `output_verbosity`, `write_output`) are
  all in these patterns, so a construction site left on the old surface
  is a hit.
- `suite_coverage.removed_kwargs_leave_the_surface`: every key of
  `hares.input_schema()["removed_keys"]` that `KWARG_RULES` names is
  classified `REMOVED` and is absent from the accepted surface, and every
  accepted keyword is classified once; with the two
  `suite_coverage.py:141-170` cases migrated as above.
- This is also the mechanical half of health: reading `RunResult.health`
  with an empty `HealthPolicy` escalates nothing new, so it is in this
  commit and gated bitwise with it.

**Acceptance test (testkit):**
- `suite_engine.fit_selection_returns_only_the_requested_column`.
- `suite_engine.health_is_read_from_run_health` (replaces
  `health_counters_are_clean`, `suite_engine.py:88-97`): every `RunHealth`
  field is present in `res.health`; the two removed keys are absent.
- `suite_engine.an_evaluation_writes_no_file`: the listing of `SCRATCH`
  before and after `Runner.run` is identical.
- `suite_engine.contract_resolves_required_columns_without_simulating`.
- `suite_engine.kwargs_carry_reports_and_no_removed_key` (replaces the
  check at `suite_engine.py:49-54`).
- `suite_inject.reachability_control_is_rejected_by_the_engine`.
- `suite_der_inject.a_battery_with_no_dispatch_policy_is_refused`.
- `suite_inject.setpoint_factor_edits_the_active_source`: a study home's
  variant reports `SetpointSource` HPXML for both sides; the same home with
  a schedule carrying a `heating_setpoint` column makes a
  `heating_setpoint_shift_c` variant fail naming the column.
- `suite_calibration.contract_drift_reads_the_published_schema`: with the
  contract's `SimulationConfig` field set extended by one name the engine
  does not have (monkeypatched), `hares_contract_check.check()` returns 1
  and its DRIFT line names that field and `hares.input_schema()` as the
  source; unpatched it returns 0.
- `suite_engine.engine_metrics_raise_on_a_missing_field`: with the
  engine's `RunMetrics` replaced by an object lacking one field the
  consumer records (monkeypatched), `Runner.run` raises naming that field
  instead of returning empty metrics.
- `suite_engine.presets_are_the_engine_verbosity_selections`: each of
  `VERBOSITY_0`, `VERBOSITY_2`, `VERBOSITY_5` and `VERBOSITY_6` equals the
  engine's named verbosity selection of that tier
  (`03-output-reporting.md` OR-3, `selection = { verbosity = N }`); on
  `engine._default_spec(2.0)`, `Runner.run` with each returns that
  selection's catalogue columns; `engine` has no `OUTPUT_VERBOSITY` and
  `ami2.adapter` no `ADAPTER_VERBOSITY`.
- `suite_ami2.bill_reads_every_period_without_finalize_billing`: the F9
  case's premise and tariff (`suite_ami2.py:1253`) bill a run whose last
  period is partial; `n_periods` counts it and `total_import_kwh` equals
  the sum over the returned periods.
- `suite_coverage.prober_reads_the_published_field_lists`: for a class the
  study home has, `Prober.probe` returns the schema's field list for that
  class with outcome `DENY_LISTED`, and `known_kwargs()` holds `config`
  and `defaults` and no removed key.
- `suite_fit.fit_writes_no_slot_files`: with `paths.SCRATCH` patched to a
  fresh temporary directory and the process's working directory set to a
  second one, a 1-generation fit of the suite's study home (`TRUTH` =
  `bldg0050945`, county `G4804390`, `STUDY_GRID`, 2 days, population 12,
  the full free set, so `opaque_r_scale` and `thermal_mass_scale`, the two
  `DEFAULTS` factors, are in it) leaves both directories empty.
- `suite_engine.content_spec_runs_like_a_path_spec`: on
  `engine._default_spec(2.0)`, the spec rebuilt with its HPXML and
  schedule as `Text` and its defaults as `Defaults(paths.DEFAULTS, {})`
  returns a total-power series bitwise equal to the path spec's.
- DE-trace gate: `compare B0 <capture>` exits 0.

**Verification:** consumer gate; DE-trace gate; census above;
`contract.txt` holds `[hares-contract] PASS` on this row's engine.

**Commit:** `fix: drive the engine through the report API and read health from RunHealth`

**Dependencies:** CN-0, CN-13; `03-output-reporting.md` OR-1, OR-3, OR-5
(`RunHealth`), OR-6, OR-7, OR-8; `05-python-surface.md` PY-1, PY-2, PY-4,
PY-6, PY-8 (the one `SimulationConfig`; `KNOWN_KWARGS` deleted), PY-9
(the Python `finish()`/`RunResult`/`health()`/`RunMetrics` surface);
`11-other-defects.md` OD-2, OD-8; `06-runtime-memory.md` RT-4, RT-6;
`12-inputs.md` IN-2, IN-3, IN-5, IN-6.
`frame_hash` values change with OR-7's `Time` type (`engine.py:249-264`);
no gated field hashes the frame, and `probe.py`'s hashes are compared only
within one engine build (consumer `AGENTS.md`: "do not compare numbers
across engine builds").

**State:** committed the consumer `f21e34c` (branch `row/cn1-report-api`, cut
from the closed rename row's tip `cf7f2c3` — the branch `main` still sits at
the pre-rename pin `78568b5`, which cannot drive this engine) and the engine
`d790a6f3` + `d3119aa9` (branch `row/r72-cn1`, a worktree of the row base
`e3bc4870`; the rename on the first, the report pyclasses' equality and
pickling and the setpoint readback on the second).
The surface audit first: the RunResult/reports/health surfaces rows 43a, 55
and 57 landed satisfy the entry's list as it stands — `RunResult.reports`
(the memory report's `table()` frame), `.metrics` (the named `RunMetrics`
fields, every key the consumer records among them), `.health` (every
`RunHealth` field plus the warm-up outcome), `.catalogue` and the
provenance's build identity and catalogue digest, the parsed-content handles
(`ParsedHpxml.from_text`, `ParsedSchedule.from_text`,
`ParsedDefaults(base, files=...)`), `input_schema()` with its override
classes and removed keys, and the `ReportSpec`/`Selection`/`Destination`/
`Frequency` constructors — and three pieces were missing, all landed on the
engine commits. One: the dwelling-level defaults keyword is still
`defaults_path` — the PY-2 rename the phase-1 fold did not carry (the folded
row-35 commit kept the old keyword and added no removed-keys row). Landed on
`d790a6f3`: the three constructors take `defaults=`, the `defaults_path`
`PythonKeyword` REMOVED_KEYS row names it, the gym's `GymDwellingConfig`
field and `SimulationPlan` renamed with it, the stub and docs updated, and
every engine caller migrated (about 40 test files); RED first, four failures
at the base with only the pin applied (the old keyword did not raise, the
new one was an unexpected keyword). Two: the report pyclasses had no Python
equality — `Selection`, `Frequency` and `Destination` gained `__eq__` over
their Rust value equality, and `Selection` pickles through `__reduce__`
over its own serde JSON (a rebuild function on the extension module, picked
by reference), because the consumer's spawn pool pickles every `_Job`'s
selection and died on the pyclass instead. Three: `validate()`'s
resolved-model readback carries the active `SetpointSource` per side (the
`setpoints` section, the first non-None across the equipment specs, serde's
own tagged form), which the injector's setpoint guard reads; pinned by
`test_validate_readback_carries_the_active_setpoint_sources`. Both landed on
`d3119aa9`. The consumer commit then lands the entry's Change list whole, as
the row 72 record in `00-schedule.md` carries; the acceptance cases
(fifteen, added or replacing, across seven suites), the RED witnesses (the
four rename failures at the engine base; the consumer's RED is the forced
migration itself — the old consumer against this engine fails 87 of 360
cases at the removed surface, witnessed before the migration ran), the gates
on both repos, the census's residual hits (the tests' own absence-pins, the
coverage rule's split literal, the mandated case name, and the second grep's
exclusions grown by the consumer's own `Grid`/`TwinOutput` constructors — no
call site among them) and the B0 account (twice identical at the final
engine state; the shipped baseline's compare the recorded 260-path
divergent-audit-line delta) are all in the row 72 State, which is the row's
record.

## CN-2: the calibration system arms a strict health policy

**Kind:** initiative work (`dl-i08-architecture.health`: "consumers that
gate on health (the calibration system) set a strict policy").

**Problem:** after CN-1 the consumer reads `RunResult.health` with an empty
`HealthPolicy`, so a port rollback, a rejected control signal, a clamped
action or a warm-up that did not converge is recorded but never stops an
evaluation from being scored. The fit should score only runs whose health
is clean.

**Evidence:** `calibration/engine.py:384-397` (today only NaN and
unresolved counts raise; rollbacks are recorded, not gated);
`calibration/fit.py:233-247` (a `FailedRun` is scored `PENALTY` and counted
in `n_rejected`).

**Change:** `Runner.build_kwargs` builds its `SimulationConfig` with
`health_policy=HealthPolicy.strict()` (`plan.md` §2.3,
`03-output-reporting.md` OR-12), which escalates every event kind
`RunHealth` records (port rollbacks, rejected control signals, clamped
actions, curve-index clamps, warm-up non-convergence) to a run failure
naming the step and the source. The failure reaches the fit as a
`FailedRun` and is scored `PENALTY`, like any other rejected corner. This
changes which evaluations are rejected, so it lands after the numeric fixes
and before CN-4; B1 absorbs its effect and records how many evaluations it
newly rejected. There is no separate baseline.

**Acceptance test (testkit, `suite_engine`):**
`strict_policy_fails_a_run_whose_warmup_did_not_converge`: the spec of the
`suite_fit` study home (`TRUTH` = `bldg0050945`, county `G4804390`,
`STUDY_GRID`, 2 days)
with `WarmupConfig(max_days=1)` (the first warm-up day has no predecessor
and is never converged, B-2) returns a result whose `health.warmup` is
`Ran` with `converged == False` under the empty policy, and comes back
from `run_specs` as a `FailedRun` naming warm-up non-convergence under the
strict policy.

**Verification:** consumer gate. No DE-trace gate: this entry is absorbed
by B1.

**Commit:** `feat: reject evaluations whose engine health is not clean`

**Dependencies:** CN-1, CN-3; `03-output-reporting.md` OR-5, OR-12;
`09-numeric-fixes.md` B-2 (warm-up outcome), B-13.

**State:** landed (2026-10-17). Consumer `24c27a72` on branch
`row/cn2-strict-health` (cut from the landed main `494b5cf3`), one
commit, gated on the engine `aa1a3ad5` (branch `row/r96-cn2`, worktree
`r96-cn2`, where the row's schedule record lands). The engine's policy
surface needed nothing: row 42's `HealthPolicy` already carries
`strict()` over every event kind this entry names (the four counter
limits `Some(0)` and the warm-up requirement, every violation naming
the event, the step, the source, the count and the limit), so the
row's engine side is its ledger record alone. Landed whole: the fit's
kwargs builder builds its `SimulationConfig` with
`health_policy=HealthPolicy.strict()` on every evaluation's config,
whatever path reaches the engine (the fit's per-evaluation runs and
every other runner alike), so a degraded evaluation reaches the fit as
a `FailedRun` (scored `PENALTY`, counted in `n_rejected`) and can
never come back as a value that looks like a fit input.

RED first: the entry's acceptance case
`strict_policy_fails_a_run_whose_warmup_did_not_converge`
(`suite_engine`; the `suite_fit` study home `bldg0050945`, county
`G4804390`, `STUDY_GRID`, 2 days, `WarmupConfig(max_days=1)` - the
first warm-up day has no predecessor and is never converged, row 83's
record), witnessed at the base: under the empty policy the run
completed with `health.warmup` `Ran` and `converged == False` (the
degraded run scoreable, exactly the pre-policy behavior), and
`run_specs` returned a scored result from the same degraded run - no
refusal. GREEN after: the same run comes back from `run_specs` as a
`FailedRun` whose message names the event
(`HaresHealthPolicyError: health policy: WarmupNotConverged from
'warmup': count 1 exceeds the limit 0`).

One consequence the gate caught: the F15 shape case's precondition was
the converging pair (heating up, cooling down), which the engine
rejects and counts since its one-band rule - never applied - so the
shape the case measured was half-executed, its precondition inert by
refusal rather than by physics, and a run carrying that rejection now
fails at the step it occurs. The case's precondition is the engine's
single-axis form now (the heating setpoint up, the engine's own
preconditioning rule in `dr_compliance.rs`), the shape is accepted end
to end, and the executed shape re-measured: peak 2.7422 -> 1.0412 kW,
+6.937 kWh stored before the peak, 5.103 kWh shed in it, the day
+2.391 kWh (the real cost of storing: the old reading, -1.714 kWh, was
a saving of a shape whose storing half never ran), comfort 1.955 C
max; the conservation bound (no day saving beyond the coast's own
shed) is kept, and the case records the executed measurements.

Movement declared: the DE-trace on the row's final state captured
twice, identical, and identical to the recorded state (the consumer at
the landed main, the same engine): on the production calibration set
the strict policy newly rejected zero evaluations - all four premises'
`n_eval` (459, 408, 510, 510), `n_rejected` (0), losses, nominal
losses, traces and fitted vectors bitwise unchanged, `first_rejection`
empty in both - so the one re-baseline's absorption carries no delta
from this row. The row's refusal path is exercised by the acceptance
case and by the F15 consequence; the fit's own corners never tripped
it.

Gates: the consumer `run_all` 383 passed, 0 failed, 7 skipped (the
base seven; the +1 the entry's acceptance case), ruff clean,
`hares_contract_check.py` prints `[hares-contract] PASS` on this row's
engine, with two pre-existing NEW items (the catalogue's `Other` and
`Refrigeration` end-use aggregates outside the contract's five read
aggregates; present on the unchanged main consumer against the same
engine, considered, no drift item), `build_bundle.py`'s registry every
file present and none untracked; the engine fmt clean, clippy
`--workspace --all-targets --all-features -D warnings` clean, nextest
`--workspace --all-features -j 2` once 8145/8145 (8 skipped),
`pytest -m ''` 1766 passed + 6 xfailed, 0 failed, 0 errors, pyright 0
errors, the goldens bitwise (compare-all 8 + dst 1 identical, no
re-capture). Absorbed by B1.

## CN-3: envelope factors through the engine's envelope scales

**Kind:** initiative work, the consumer side of `09-numeric-fixes.md` B-13
and `12-inputs.md` IN-7 (`dl-i08-architecture.calibration_r_factor`).

**Problem:** `opaque_r_scale` reaches opaque R today only through the
defaults tree: it scales the `Resistance (m^2-K/W)` column of
`envelope/Envelope Materials.csv` in a per-evaluation defaults variant
(a copied tree at `c71c542`, replacement content from CN-1 on), because
the LUT's precomputed layers outrank `<AssemblyEffectiveRValue>`. After
B-13 the declared assembly R wins and the LUT layer resistances are rescaled
to it, so scaling the CSV resistance column no longer changes anything.
Moving the factor onto `<AssemblyEffectiveRValue>` would re-select the LUT
row as R crosses a row boundary and step the capacitances with it, which
the decision forbids. `thermal_mass_scale` scales the
`Capacitance (kJ/m^2-K)` column through the same defaults variant.

**Evidence:** `calibration/registry.py:161-172` (`opaque_r_scale` and
`thermal_mass_scale`, both `DEFAULTS` route, with notes that the LUT
outranks the declared R); `calibration/defaults.py:1-6, 32-33`
(`COL_RESISTANCE = 10`, `COL_CAPACITANCE = 11`);
`calibration/inject.py:113, 138-142, 177` (`r_scale`/`c_scale` routed into
`DefaultsVariant.write`); `calibration/coverage.py:133-136, 225-228` (notes
classifying `Assembly R Value` as a row selector and `AssemblyEffectiveRValue`
as unreachable), `:137` (the `Resistance|Capacitance` rule that marks the
two columns `CALIBRATED`), `:281-296` (`_registry_index` maps
`defaults_key` to factor), `:355-359` (the defaults-CSV rows take their
factor from that map); `calibration/registry.py:37` (the `DEFAULTS` route
constant), `:66` (`Factor.defaults_key`), `:661-662` (its validation),
`:692-693` (its description); `tools/bench_calibration.py:62-78` (times
`inj._defaults.write`); `build_bundle.py:46` (registers
`calibration/defaults.py` in `SOURCE`, so deleting the module without this
line reports it MISSING); `calibration/tests/suite_coverage.py:111-125`
(`every_registry_factor_targets_a_real_field` resolves every `OVERRIDE`
factor's equipment to a config struct, and `envelope` is not one);
HARES `crates/hares-core/src/dwelling/conversions.rs:147-151, 240-291`
(not `crates/hares-python/src/conversions.rs`, a different file of the
same name) and `crates/hares-io/src/envelope_lut.rs:550-590` (today's
precedence and row selection, see B-13).

**Change:**
- `opaque_r_scale` and `thermal_mass_scale` become `OVERRIDE` route
  factors on the engine's reserved `envelope` key
  (`overrides["envelope"] = {"opaque_r_scale": r, "thermal_mass_scale": m}`,
  IN-7), bounds unchanged. The engine applies them after LUT row
  selection, so the selected row, and with it the capacitances, never
  moves with `opaque_r_scale`; capacitance changes only through
  `thermal_mass_scale`.
- `DefaultsVariant` (`calibration/defaults.py:60`), its use in
  `inject.py:32, 59` and its cases (`suite_inject.py:353-398`), and the
  `DEFAULTS` route of the registry are deleted: these two are the only
  `DEFAULTS` factors (`registry.py:162, 169`), so every candidate then
  shares the repository defaults (and, with CN-10, the engine's parsed
  defaults). With the route go the `DEFAULTS` constant, `Factor.defaults_key`,
  its validation and its description (`registry.py:37, 66, 661-662,
  692-693`), the `by_defaults` map and its use (`coverage.py:281-296,
  355-359`), the defaults-variant timing of `tools/bench_calibration.py:62-78`,
  and `calibration/defaults.py` itself with its `build_bundle.py:46`
  registration (`AGENTS.md` rule 1: the registry lists exactly the
  shipped files). With no defaults file replaced by anything, CN-1's
  `Defaults(base, files)` form of `RunSpec.defaults` and its branch in
  `engine.engine_inputs` go too: `RunSpec.defaults` is a `Path`.
- The `Resistance|Capacitance` rule (`coverage.py:137`) stops marking the
  two columns `CALIBRATED`: no factor moves them any more, and the
  envelope factors are classified on the override surface instead.
- `suite_coverage.every_registry_factor_targets_a_real_field`
  (`suite_coverage.py:111-125`) resolves an `OVERRIDE` factor's target
  through `hares.input_schema()["overrides"]` (the source CN-1 moves the
  override-class fields to): the reserved `envelope` key resolves to its
  schema class, whose fields are `opaque_r_scale` and
  `thermal_mass_scale` (`12-inputs.md` IN-7, IN-3), so the case asserts
  both factors' paths against it instead of skipping or exempting the key.
- The notes at `registry.py:161-172`, and the rules at
  `coverage.py:133-136, 225-228`, state the new routes.
- The consumer's reads of B-14's renamed run-state and control-layer
  names. Every read of the engine's step dict key `net_electric_power_kw`
  (at `5c651bc`: `calibration/ami2/dr_actuation.py:257`,
  `calibration/cosim.py:94, 107`, `calibration/coupling.py:412`,
  `calibration/tests/suite_ami2.py:822, 879, 1767`) reads
  `net_electric_power_w` and converts to the kW the consumer's own series
  hold, with the consumer's unit conversion, in one helper the sites
  share; the read of the premise ZIP weight `mean_power_kw`
  (`calibration/ami2/cvr_actuation.py:137`) reads `mean_power_w` through
  the same helper; every read of the telemetry field
  `Dwelling.telemetry().reactive_power_kvar` (`calibration/cosim.py:81`,
  `calibration/coupling.py:393`) reads `reactive_power_var` and converts
  to the consumer's kVAR through that helper, and the timing of that read
  at `calibration/scale.py:316` (a microbenchmark that keeps no value)
  reads `reactive_power_var`; the docstrings and messages at
  `coupling.py:359-361`, `cosim.py:25`, `scale.py:319`,
  `dr_actuation.py:31`, the comment at
  `calibration/tests/suite_ami2.py:2016` and the case text at
  `calibration/tests/suite_scale.py:160` name the SI keys. The override
  rule `^power_limit_kw$` (`calibration/coverage.py:106`), which classifies
  the `EvConfig` field B-14's equipment config commit renames, becomes
  `^power_limit_w$`. The consumer's own record fields
  (`IntervalRecord.net_electric_power_kw` and `reactive_power_kvar`,
  `calibration/ami2/contract.py:96, 100`) and their readers are the
  consumer's reporting types in kW and kVAR and stay. The census
  `git grep -nE '\["(net_electric_power_kw|mean_power_kw)"\]|step\(\)\["[a-z_]*_kw"\]|telemetry\(\)\.[a-z_]*_(kw|kvar)\b|\^power_limit_kw\$' -- calibration tools`
  prints nothing.
- The contract-drift rule (preamble) for rows 83 to 94l: `hares_contract.py`
  and `KWARG_RULES` (`coverage.py:111-127`) move from
  `initialization_duration` to `warmup` (B-2), the equipment config
  fields and the DER code that writes them move to B-14's SI names
  (`calibration/hares_der_remeasure.py:85`, `PV(capacity_kw=...)`; the
  `DerSpec` to engine conversion in `calibration/der_inject.py`, including
  the `surplus_deadband_kw` it writes into the `bms_mode` payload at
  `:157-180`, which becomes `surplus_deadband_w`), with the conversion
  from the consumer's own kW and kWh values done with the consumer's unit
  conversion at that one boundary, and every other DRIFT item the census
  reports for B-5, OD-9, IN-8, B-14, IN-9, OD-2's site 6, OD-10, OD-20,
  B-15 and B-17 is resolved in this commit. From B-15 the engine checks
  the `Vacancy`, `Power Outage`, `No Space Heating` and `No Space Cooling`
  schedule columns against the HPXML periods row by row, so
  `Schedule.rotated` (`calibration/schedules.py:115-124` at `5c651bc`)
  leaves those four columns at their calendar rows and rotates every
  other column (`FLAG_COLUMNS`, `:44`, keeps `occupants`, which rotates
  with the behaviour it counts); `suite_inject`'s rotation case
  (`suite_inject.py:292`) asserts both halves on the engine fixture
  `tests/fixtures/resstock/2025.1/bldg0176227/in.schedules.csv` (through
  `paths.FIXTURES`; its `No Space Heating` column flags Oct 4) rotated by
  2 weeks.
  IN-9 replaces the HPXML vehicle elements `der_inject.py` writes at
  `:207-209`.

**Acceptance test (testkit, `suite_inject`, engine-backed, on the
`suite_fit` study home `bldg0050945`, county `G4804390`):**
- `opaque_r_scale_never_reselects_a_row`: for the study home's variants at
  `opaque_r_scale` 0.5, 1.0, 1.3 and 2.0, every boundary's `ResolvedModel`
  layer capacitances equal the nominal variant's on bits and its layer
  resistances are the nominal ones times the factor.
- `thermal_mass_scale_moves_resolved_capacitance_only`: `thermal_mass_scale=1.5`
  scales every resolved layer capacitance by 1.5 and leaves every resolved
  resistance unchanged.
- `no_evaluation_replaces_a_defaults_file`: every spec of a population
  built at `opaque_r_scale` and `thermal_mass_scale` values other than 1.0
  carries `paths.DEFAULTS` itself as its defaults, with no replaced file.
- `suite_coverage.every_registry_factor_targets_a_real_field` passes with
  `opaque_r_scale` and `thermal_mass_scale` resolved to the `envelope`
  schema class.

**Verification:** consumer gate (`build_bundle.py` reports no MISSING
file after `defaults.py` leaves); `contract.txt` holds
`[hares-contract] PASS` on this row's engine. Absorbed by B1.

**State:** landed (2026-10-16). Consumer `a7b35cdd` on branch
`row/r95-cn3` (cut from the landed main `78f20657`), carrying the row 82
consumer commit replayed as `1316d184` (branch tip `c7b56ddb`; its
weather alignment this row's gate needs on this engine, the rebase's
shared kwarg-rules text resolved to this row's wording) and `973f57e8`
(the two AMI2 bands re-measured, below); engine `482323b9` on the row
branch
(worktree `r95-cn3`, based on `fc705e43`), the input-schema fold
addition the entry needs: `impl DescribeFields for EnvelopeScales`
beside the definition (the two fields, default 1.0, bounds (0, 10]) and
the reserved `envelope` key among the schema's override classes, from
that impl, with the `envelope_scales_described_with_bounds` acceptance
and the enforcement test treating the reserved key as its own class
(no equipment sample: the scales' own deserialisation and bounds
validation enforce it). Landed whole: the two factors are OVERRIDE
factors with `equipment="envelope"` and `path=("opaque_r_scale",)` /
`path=("thermal_mass_scale",)`, so the injector's existing override
dispatch writes `overrides["envelope"]` and the engine applies the
scales after the LUT row selection; the `DEFAULTS` route constant,
`Factor.defaults_key`, its validation and its `_route_target` arm are
deleted with `calibration/defaults.py` (the `build_bundle.py`
registration with it, the registry reporting no MISSING and none
untracked after), `DefaultsVariant` and its `suite_inject` case, the
`by_defaults` map and its use, and the `Defaults(base, files)` form of
`RunSpec.defaults` with its `engine_inputs` branch (`RunSpec.defaults`
is a `Path`; every spec carries `paths.DEFAULTS` itself); the
`Resistance|Capacitance` rule classifies the two columns STRUCTURAL
naming the override route (no factor edits them any more), the
`Assembly R Value` and `NominalRValue|AssemblyEffectiveRValue` notes
state the declared-R precedence and the new route (the latter's
`UNREACHABLE` premise, the LUT outranking the declared R, is gone with
that precedence: the element is live physics again, UNIDENTIFIABLE, not
in the declared free set), the override surface enumerates the
schema's reserved class beside the equipment classes, the accepted
keyword set drops the removed `initialization_duration` and `warmup`,
`first_hour_interpolation` and `weather_year` classify (the gate reads
the published schema of an engine that carries them; the row 82
consumer commit classifies the same three and the folds reconcile),
`known_kwargs`' accepted set loses the removed key, the
bench's defaults-variant timing is gone, and the four-route prose
follows. The SI renames of the entry's later range are consistent
rather than moved: this row's engine carries no equipment SI renames
(their rows unlanded), so the consumer's kW spellings stay consistent
with it, and the entry's post-rename census form (written for the state
where the engine's SI commits have landed and prints nothing there)
still matches these same consistent reads on this row's engine; the
renames this entry's range owns resolve on the consumer row that gates
on an engine carrying them.

RED first, all four consumer pieces, witnessed at the base (the
consumer at main plus the defaults-path fix only, the engine at
`fc705e43`): `opaque_r_scale_never_reselects_a_row` failed on every
opaque boundary (got nominal, want nominal times the factor: the old
route was inert under the declared-R rescale), 
`thermal_mass_scale_moves_resolved_capacitance_only` failed off bits at
1.5 (the CSV text's 10-significant-digit rounding),
`no_evaluation_replaces_a_defaults_file` failed (the spec carried a
replaced file), and `every_registry_factor_targets_a_real_field` failed
on the two factors not being OVERRIDE on the published schema. GREEN
after, on the row's engine.

Movement declared: the DE-trace on the row's final state captured
twice, identical. Against the recorded B0: 269 of the 379 common
per-premise leaves differ, every premise's loss and fitted vector
moved, and `n_eval` is at -2 per premise (the recorded parent-probe
rule, not this row); the attribution: the recorded engine rows 1 to 93
and consumer rows 72 to 82 carry most of it, each with its own delta
record, and the row's own routing is isolated by a differential capture
on the same engine with the old routing: every premise's trace diverges
at generation 0, because the envelope factors' evaluations move bitwise
under the new routing, and the DE's early exits propagate the
divergence from the first generation (every fitted vector differs;
premise 2's `n_rejected` 1 to 0). Two AMI2 measured bands the engine
rows since the last consumer-gated engine crossed are re-measured and
re-pinned with the cause: F7's event-window band 0.10 to 0.15 (measured
-13.1% against the realised shed; the own-event form stays inside 10%)
and the GEB shape's comfort bound 2.0 to 2.5 (measured 2.116 C; the
assertion stays a bound on the executed shape's comfort cost, no longer
the era's claim). Absorbed by B1.

Gates: the consumer `run_all` 382 passed, 0 failed, 7 skipped (the base
seven), ruff clean, `hares_contract_check.py` prints
`[hares-contract] PASS` on this row's engine, `build_bundle.py`'s
registry no MISSING file and none untracked; the engine fmt clean,
clippy `--workspace --all-targets --all-features -D warnings` clean,
nextest `--workspace --all-features -j 2` once 8145/8145 (8 skipped;
the first run's one failure was the fresh worktree's uninitialised
OCHRE submodule, populated and the suite re-run), `pytest -m ''`
1766 passed + 6 xfailed, 0 failed, 0 errors, pyright 0 errors, the
goldens bitwise (compare-all 8 + dst 1 identical, no re-capture).

**Commit:** `fix: route the envelope factors through the engine's envelope scales`

**Dependencies:** `09-numeric-fixes.md` B-2, B-5, B-13, B-14 (its three
numeric commits), B-15, B-16 and, when B-16 confirms a defect, B-17;
`11-other-defects.md` OD-9, OD-2's site 6, OD-10's
numeric commit and OD-20; `12-inputs.md` IN-3, IN-7, IN-8, IN-9;
`04-fleet.md` FL-5 (preamble order step 5);
`06-runtime-memory.md` RT-6 (`ResolvedModel`); CN-1.

## CN-4: consumer re-baselines once, after every numeric fix

**Kind:** initiative work.

**Problem:** the decision log requires exactly one consumer re-baseline,
after all engine numeric fixes. The consumer's own loss regression
(`tools/check_losses.py`) must also move to the new engine at that point.

**Change:**
- After every entry in `09-numeric-fixes.md`, `04-fleet.md` FL-5,
  `11-other-defects.md` OD-9, OD-2's site 6, OD-10's numeric commit and
  OD-20, `12-inputs.md` IN-8 and IN-9, CN-14, CN-3 and CN-2 have landed,
  run `capture_de_trace.py capture` once on that engine and consumer
  commit and store it as B1 in `baselines/` (`B1-de-trace.json`,
  `B1-losses.json` and `B1-record.json`, in B0's form), with the engine
  commit, the consumer commit and the `uv.lock` sha256.
- Record the B0 to B1 attribution in `02-measurement.md`'s log: for each
  numeric entry, the delta that entry recorded on the consumer-shaped golden
  fixture; for CN-14, CN-3 and CN-2, what they change (CN-2: the number of
  evaluations newly rejected per premise). The attribution is a record, not
  an equation: the DE trace is data-dependent, so the B0 to B1 difference is
  not a sum of the entries' deltas. The note also states the run-shape
  difference behind the per-entry deltas: `consumer_shape_900s` declares
  `warmup = "disabled"` (it mirrored the consumer's no-warm-up call),
  while from `09-numeric-fixes.md` B-3 on the consumer, which passes no
  `warmup` keyword, warms up by default; so the deltas the numeric entries from B-4 on record
  on that golden are measured on a cold-start run the consumer no longer
  makes, and B1 is the only measurement of the consumer's own shape.
- Re-pin `tools/check_losses.py`'s `PINNED` to the 13 losses measured on this
  engine, as `float.hex`.
- From this commit on, every consumer and engine change is gated against B1.

**Acceptance test:** `tools/check_losses.py` exits 0 on this engine; B1 is
present with its header fields; a second capture on the same commits
compared with B1 exits 0 (`compare` diffs the per-premise payload only).

**Verification:** consumer gate.

**Commit:** `fix: re-pin the calibration loss regression to the corrected engine`

**Dependencies:** every entry in `09-numeric-fixes.md` (B-17 when the
B-16 triage confirms a defect); `11-other-defects.md` OD-9, OD-2's site 6,
OD-10's numeric commit and OD-20; `12-inputs.md` IN-8, IN-9; `04-fleet.md`
FL-5; CN-14, CN-3, CN-2, CN-9.

## CN-5: worker pools sized to the work

**Kind:** initiative work (`context.md` item 17).

**Problem:** two sizing defects.
1. The fleet-aware inner-pool shrink in `calibrate_group` is dead code:
   `workers` gets its default before the `is None` guard that should shrink
   it.
2. A lone fit sizes its evaluation pool at `default_workers()` regardless of
   the population, so a generation of P members runs as full waves plus an
   idle tail (P = 30 on 26 workers is a wave of 26 and a wave of 4), and a
   population smaller than the core count starts processes that never
   receive work.

**Evidence:** `calibration/scale.py:630` (`workers = workers or
default_workers()`) before `:633` (`if fleet_workers > 1 and workers is
None:`), so `:634-637` never runs; `calibration/fit.py:165`
(`self.workers = workers or default_workers()`) and `:236-237` (every
generation's `run_specs(..., workers=self.workers)`);
`calibration/parallel.py:111-113` (`default_workers()`: cores minus two).

**Change:**
- Extract the sizing in `calibrate_group` (`scale.py:629-637`) into
  `_pool_sizes(n_homes, workers, fleet_workers, cores) -> tuple[int, int]`
  that shrinks before it defaults, so an explicit `workers` is honoured and
  an unset one gets `max(1, cores // fleet_workers)` when `fleet_workers >
  1`.
- Add `parallel.wave_balanced_workers(n_jobs, max_workers)`, which returns
  `ceil(n_jobs / ceil(n_jobs / max_workers))`: the same number of waves with
  no idle tail, and `n_jobs` itself when `n_jobs <= max_workers`.
  `Fitter` computes its pool size once, `wave_balanced_workers(P,
  self.workers)` with P the population size, and passes it to every
  `run_specs` call of the fit, the one-spec warm-start regression run
  included. `_get_pool` (`parallel.py:40-47`) keeps one persistent spawned
  pool per size, so a size per batch would leave several pools alive at
  once until CN-10 replaces them; one size per fit keeps one pool. Results
  come back in submission order either way, so the trace does not change.
- `tools/capture_de_trace.py` `_workers_used()`, which mirrors today's
  sizing for the header (its comment says a sizing fix must update it),
  returns `_pool_sizes(N_HOMES, None, FLEET_WORKERS, os.cpu_count())[1]`,
  so the header records the inner pool the capture really ran with. The
  header is not compared, so B0 checks are unaffected.

**Acceptance test (testkit, `suite_scale`):**
- `fleet_pool_shrinks_inner_workers_when_workers_unset`:
  `_pool_sizes(4, None, 4, 28) == (4, 7)`.
- `explicit_workers_are_honoured`: `_pool_sizes(4, 3, 4, 28) == (4, 3)`.
- `lone_fit_pool_equals_population_when_smaller_than_cores`:
  `wave_balanced_workers(12, 26) == 12`.
- `pool_waves_are_balanced`: `wave_balanced_workers(30, 26) == 15`.
- `a_fit_uses_one_pool_size`: with `run_specs` wrapped to record its
  `workers` argument, a 1-generation fit of population 12 with
  `workers=26` (the `suite_fit` fixture of CN-6) calls it only with 12.

**Verification:** consumer gate; DE-trace gate against B0.

**Commit:** `fix: size worker pools to the work instead of the core count`

**Dependencies:** CN-1.

**Measurement:** wall time of one production-set capture before and after,
recorded in `02-measurement.md`.

**State:** landed as `60ccfbc` (consumer branch `row/cn5-pool-sizing`, cut
from the CN-13 head `cf7f2c3` at engine pin `a169354e`). The change as
specified: `_pool_sizes(n_homes, workers, fleet_workers, cores)` shrinks
before it defaults, so the shrink is no longer dead and the docstring's
`cores // fleet_workers` promise describes code that runs (SL-5's check);
`wave_balanced_workers` is `ceil(n_jobs / ceil(n_jobs / max_workers))` with
`n_jobs` itself at or under the budget; the `Fitter` computes its pool size
once per `run` from the population and submits every `run_specs` call of
the fit at it, the one-spec warm-start and optimum runs included; a direct
`evaluate` call outside a run sizes itself from its own batch;
`_workers_used()` reads the sizing from `_pool_sizes` so the header records
the inner pool the capture ran with. The engine side of the row is a
verification, not a change: the fleet's caller-declared threading
(`Fleet::simulate(report, n_threads, chunk_size)`, `n_threads == 0` the
global pool, `> 0` a local pool of exactly `n_threads` threads) is sane for
the consumer's use precisely because the consumer's fit path never calls
it, it drives one single-dwelling `Dwelling.simulate()` per worker process,
and the sizing defect was entirely the consumer's own pool arithmetic.

Adaptations and findings, in the row's order. The acceptance fixture is not
the one named for the later parent-probe row (that row has not landed): the
`a_fit_uses_one_pool_size` case runs a stubbed `run_specs` over the cached
study home bldg0046363 on a 30-day synthetic meter, engine-free, and
asserts every recorded `run_specs` call of a one-generation population-12
fit with `workers=26` carried 12. The over-threaded probe the row was
committed with is its own case, `a_two_member_generation_is_not_pooled_at_the_core_count`:
at the base, on the 28-core machine, a two-member
generation requested a 26-worker pool (the cores-minus-two default) and the
one-spec runs requested 1; after, every call of the fit carries 2. RED was
witnessed at the parent on all six new cases (four ImportError on the
missing API, the two fit cases behaviourally). The consumer gate ran in the
row's own worktree env (own venv, the engine built from the pin) because
the shared checkout's venv was re-pointed mid-row by the row-72 work: 353
passed, 1 failed, 7 skipped, the one failure
`a_battery_with_no_dispatch_policy_is_inert` the TRAP-2 refusal the true
pin engine makes, failing identically at the base (proved by running the
case at `cf7f2c3` against the same build) and owned by row 72 per the
battery-control entry's consumer-effect note; the main checkout's gates had
been running a stale `.so` (built hours before the pin's strictness
landed), which is why the case had been passing there. build_bundle's
filesystem walk now ignores a worktree's `.git` pointer file (the bundle
gate's first worktree run; no MISSING, no UNTRACKED after). The DE-trace
capture is bitwise identical before and after, five captures across two
engine builds of the same pin source; the compare against the shipped B0
still prints its recorded 260 differing paths (the divergent audit line the
baseline was captured on). The measurement is honest about direction: the
capture's wall went from 186/203 s before to 226/241/255 s after on the
busy 28-core host, total CPU about 3 minutes lower, because the
oversubscription the fix removes (104 processes on 28 cores) was smoothing
the premises' serial phases across each other; the load averages and the
CPU totals are in `02-measurement.md`.

## CN-6: no duplicate parent-side evaluations

**Kind:** initiative work (`context.md` item 18).

**Problem:** `Fitter.run` simulates the nominal vector standalone and again
as population member 0, the warm start standalone and again as member 1,
and after the search simulates the optimum once more only to compute its
component breakdown, although that member's series was produced by the
generation that accepted it.

**Evidence:** `calibration/fit.py:315` (`nominal_loss =
float(self.evaluate(self.nominal[None, :])[0])`), `:317` (`warm_loss =
float(self.evaluate(ws[None, :])[0])`), `:321-324` (`X[0] = self.nominal`,
`X[1] = ws`, `fx = self.evaluate(X)`), `:342` (accepted trials replace
members), `:352-357` (final `run_specs([spec], ...)` at the optimum,
`components=self.scorer.components(sim)` at `:373`). The standalone calls
build on `injector(0)` (`:224-229`); in the batch, `ws` is built on
`injector(1)`. The comment at `:319-320` calls the random fill a Latin
hypercube; it is uniform IID sampling (`self.lo + (self.hi - self.lo) *
rng.random((P, n))`).

**Precondition:** the batch values equal the standalone ones only if
`Injector.build` is a pure function of `x` whatever the member's position
in the batch: from CN-1 one injector builds every member and writes
nothing, so the position changes only the run label. This is proven
bitwise on the base code by the first acceptance test below, written and
passing before the change. `context.md` item 18's caveat ("the probe equals
nominal only when no ABSOLUTE factor is free") concerns substituting the
reachability probe's run for the nominal evaluation; this entry never does:
it reuses `fx[0]` and `fx[1]`, built from the same vectors (`self.nominal`,
`ws`) the standalone calls build, so whether an ABSOLUTE factor is free
does not matter here.

**Change:**
- Delete the standalone calls at `:315` and `:317`; `nominal_loss = fx[0]`
  and `warm_loss = fx[1]` from the initial batch.
- `evaluate` also returns the per-member series it already receives from
  `run_specs` (`fit.py:236-247`). `run` keeps the series of the current
  population, replaced whenever a trial member is accepted, so the series of
  `X[argmin(fx)]` is the one that produced `fx.min()`. `components` is
  computed from it and the final run at `:352-357` is deleted. If every
  member is `PENALTY`, `run` raises `RuntimeError` naming `first_rejection`
  (today the final run fails on the same input with an unrelated error).
- Declared count changes (the only difference the DE-trace gate allows,
  rule `n-eval-without-parent-probes`). `evaluate` counts in `n_eval` only
  the specs it submits and in `n_rejected` only submitted specs that come
  back `None` or `FailedRun`; a vector whose spec fails to build is
  `PENALTY` and counted in neither (`fit.py:226-247`). So per premise:
  - `n_eval` drops by `s`, the number of standalone probes (nominal, warm
    start) that were submitted;
  - `n_rejected` drops by `r`, the number of those submitted probes that
    came back `None` or `FailedRun`;
  - `s` and `r` per premise of the production set are measured at the
    parent commit, in the before-measurement run below, with
    `Fitter.evaluate` wrapped to count the two standalone calls' submitted
    and rejected specs; they are written to
    `scratch/i08/<sha>/declared-probes.json` and passed to `compare --rule
    n-eval-without-parent-probes --declared <file>`. `compare` has no
    `--declared` option today (`tools/capture_de_trace.py:224-229` at
    `5c651bc`); this entry adds it: a JSON file mapping each premise index
    to its `s` and `r`, required with this rule and refused without it;
  - `flag_codes` may differ only by the presence of
    `fit_infeasible_corners`, and only for a premise where the rejection
    share test of `flag.py:161` (`n_rejected / n_eval > 0.05`) gives a
    different result on B0's counts and on the adjusted counts, in the
    direction the adjusted counts give; `flag` may differ only where that
    code is the premise's only code, between the no-code level and the
    `review` level the code carries (`Flag.add` keeps the highest level,
    `flag.py:74-79`; `fit_infeasible_corners` is `review`, `:161-164`);
  - every other field is bitwise equal.
- `tools/capture_de_trace.py` implements the rule in `RULES` (empty until
  this entry, so passing it earlier is an error).
- Update the dependants of `n_eval`: `HomeCalibration.engine_runs`
  (`scale.py:504-507`) becomes `n_eval + 1` (the warm-start regression run
  remains, the final run is gone) with its docstring; `frontier.py:31-35`
  (`2 + P + G*P` becomes `P + G*P`, with its three worked figures 512, 206
  and 494 recomputed); the `_mix_for` docstring (`scale.py:691`). `baseline.py:174`
  lists `n_eval` as behavioural: `baseline.diff_fits` run against a document
  recorded before this commit reports a difference of exactly `s` per fit
  (2 when both standalone probes were submitted), which is this declared
  change; no stored document is rewritten.
  `flag.py:161`'s rejection share uses the new counts.
- The comment at `fit.py:319-320` says uniform random fill.

**Acceptance test (testkit, `suite_fit`, engine-backed, on the suite's study
home `TRUTH` = `bldg0050945`, county `G4804390`, `STUDY_GRID`, a 2-day fit,
1 generation, observed series from `_truth_series()` cut to 2 days):**
- `batch_slot_reproduces_standalone_evaluation`: on the fixture,
  `evaluate(nominal[None])[0]` and the batch value at slot 0, and
  `evaluate(ws[None])[0]` and the batch value at slot 1, are bitwise equal.
  Written first; passes on the base code.
- `no_duplicate_parent_evaluations`: on the same fixture with the free set
  `effective` (every vector builds), with `run_specs` wrapped to count
  submitted specs, one `run()` submits exactly `P * len(trace) + 1` specs
  (the initial batch, one batch per completed generation, and the warm-start
  regression run).
- `suite_scale.compare_accepts_only_the_declared_probe_counts`: on two
  copies of the fixed `HomeCalibration` records of CN-0's cases, with
  declared `s = 2`, `r = 0` for premise 0: `n_eval` lower by 2 passes; lower
  by 3 fails naming `premises[0].n_eval`; a changed `trace` value fails even
  with the rule; a `fit_infeasible_corners` code added where the share test
  does not change fails naming `premises[0].flag_codes`.

**Verification:** consumer gate; DE-trace gate against B0 with rule
`n-eval-without-parent-probes`.

**Commit:** `perf: stop re-simulating the nominal, warm-start and optimum candidates`

**Dependencies:** CN-1, CN-0.

**Measurement:** wall time of one production-set capture before and after
(`context.md` item 18: about 3.5 to 6 s per fit), recorded in
`02-measurement.md`.

## CN-7: the fit path requests no metrics and skips equipment metadata it never reads

**Kind:** initiative work (`context.md` item 19, first half;
`dl-i08-architecture.metrics_optional`: the calibration fit path does not
request metrics).

**Problem:** the fit objective reads one series (`reduce_total_kw`), yet
every evaluation also has the engine compute its run metrics, flattens
them, walks every equipment for ZIP coefficients and end-use labels, and
ships them back to the parent inside `RunResult`.

**Evidence:** `calibration/engine.py:406-407` (`metrics=_engine_metrics(dw)`,
`**_equipment_metadata(dw)` on every run; the helper is `:411-472`); `:272-298`
(`_equipment_metadata`: `dw.equipment()` and one `dw.equipment_zip()` per
equipment); `calibration/fit.py:236` (the fit reduces with
`reduce_total_kw`); `context.md` item 19 (about 7.4 ms per evaluation).

**Change:** `Runner.run(spec, selection, *, metadata: bool = True)`; when
`metadata` is false, the engine config carries `metrics=False`, so the
engine builds no metrics accumulator (`plan.md` §2.2) and its
`RunResult.metrics` is `None`; the consumer's `RunResult.metrics`,
`equipment_zip` and `equipment_end_use` stay empty and neither helper
runs. `run_specs` carries the flag in `_Job`; `Fitter.evaluate` and
`Fitter.warm_start` pass `metadata=False`. Every other caller keeps
today's behaviour (`metrics=True`).

**Acceptance test (testkit, `suite_engine`, on `engine._default_spec(2.0)`:
`bldg0008590`, AMY 2018 weather of county `G4802010`, 2 days):**
- `fit_evaluation_requests_no_metrics`: a run with `metadata=False` builds
  an engine config with `metrics == False`, the engine result's `metrics`
  is `None`, and with `_engine_metrics` and `_equipment_metadata` wrapped
  to count calls neither is called; the consumer result's `metrics`,
  `equipment_zip` and `equipment_end_use` are empty.
- `default_run_keeps_metrics`: a default run requests metrics and calls
  each helper once.

**Verification:** consumer gate; DE-trace gate against B0.

**Commit:** `perf: request no metrics and skip equipment metadata on the fit path`

**Dependencies:** CN-1; `03-output-reporting.md` OR-8.

**Measurement:** per-evaluation wall time before and after, recorded in
`02-measurement.md`.

## CN-8: delete the fit path's scratch output

**Kind:** initiative work (fixes `context.md` item 19's second half).
Operator checkpoint required.

**Problem:** the consumer's `scratch/` holds the Parquet files every past
fit evaluation wrote and nothing reads. At `c71c542` on this host:
`scratch/` is 7.0 GB in 61 top-level entries, of which `scratch/fitrun`
(one directory per worker pid) is 6.2 GB. After CN-1 nothing writes there.

**Evidence:** `calibration/fit.py:236,265,356` (`scratch=paths.SCRATCH /
"fitrun"`); `calibration/engine.py:315` (`run.parquet` per runner);
`calibration/paths.py:218-240` (`sweep_scratch`).

**Change:** after CN-1, the operator confirms deletion of the named
directory `scratch/fitrun` only. `paths.sweep_scratch` is not used: it
removes every top-level `scratch/` subtree older than a cutoff, including
evidence directories unrelated to fits. The sizes and entry counts are
measured at execution, before and after.

**Acceptance test:** none (a deletion). Recorded: `du -sb scratch/fitrun`
and the `scratch/` top-level listing with sizes before and after; every
entry other than `fitrun` is unchanged.

**Verification:** `git status --porcelain` in the consumer is unchanged
(`scratch/` is ignored, consumer `AGENTS.md` rule 4).

**Commit:** none; recorded in `02-measurement.md`'s log with the
operator's confirmation.

**Dependencies:** CN-1. Operator checkpoint (`plan.md` section 3).

## CN-9: `check_losses.py` is a gate that can never fail

**Kind:** defect fix (consumer test integrity).

**Problem:** the loss regression the consumer uses to clear an engine pin
bump always exits 0, and it compares losses rounded to four decimals, so a
small real movement passes.

**Evidence:** `tools/check_losses.py:24-26` (`PINNED` holds four-decimal
values), `:34` (`got[r.hvac].append(round(r.loss, 4))`), `:44` (`return 0`
whatever `ok` is). The tool is not registered in `build_bundle.py` at
`c71c542` (registered by CN-0).

**Change:**
- `main()` returns `0 if ok else 1`.
- Losses are compared as `float.hex(r.loss)` strings; `PINNED` holds the
  `float.hex` values of the 13 losses recorded with B0 (the unmodified
  engine). CN-4 re-pins them on the corrected engine.

**Acceptance test (testkit, `suite_scale`; `tools/` is not a package, so the
case loads `tools/check_losses.py` by path with `importlib.util`):**
- `check_losses_fails_when_a_loss_moves`: with `scale.calibrate_group`
  monkeypatched to return records whose losses equal `PINNED` except one
  value one ULP away, `main()` returns 1.
- `check_losses_passes_on_identical_losses`: the same with every loss equal,
  `main()` returns 0.

**Verification:** consumer gate; `tools/check_losses.py` exits 0 on the
consumer-block engine (which is neutral against B0).

**Commit:** `fix: make check_losses.py fail when a calibration loss moves`

**Dependencies:** CN-0 (registration, B0 losses).

## CN-10: threads instead of processes in the evaluation pool

**Kind:** initiative work, consumer half of `06-runtime-memory.md` RT-5
(`context.md` item 6).

**Problem:** every fit evaluation runs in a persistent spawned process pool,
one process per worker, each holding its own interpreter, engine module and
parsed weather. Every candidate has its own injected HPXML, schedule and
defaults variant (content from CN-1 on), so those cannot be shared; the
weather file is the same for every candidate of a fit and is parsed once
per process per evaluation.

**Evidence:** `calibration/parallel.py:37-47` (`_POOLS` and `_get_pool`: a
spawned `ProcessPoolExecutor` per worker count), `:1-16` (docstring: processes
because `from_hpxml` and `initialize` hold the GIL); `calibration/inject.py:170-205`
(HPXML, defaults variant and schedule built per evaluation; the weather path
is passed through unchanged at `:198`). The premise-level pool at
`calibration/scale.py:667-677` runs different buildings and is not changed
by this entry.

**Change:**
- `run_specs` runs jobs on a `ThreadPoolExecutor` sized as CN-5 sizes it;
  the spawned process pool, `_POOLS`, `_join_pools` and the pickling
  constraints in the docstring go (no second path is kept).
- Each distinct weather file of a batch is parsed once into a
  `hares.ParsedWeather` (`plan.md` §2.7, built by `06-runtime-memory.md`
  RT-4) and that handle is passed to every job's `from_hpxml`; HPXML,
  schedule and defaults stay per candidate, as today.
- Jobs run engine construction, initialisation and simulation with the GIL
  released (PY-3); `reduce_fn` runs in the worker thread as today.

**Acceptance test:**
- DE-trace gate: the capture under the thread pool equals B0 bitwise.
- `suite_fit.thread_pool_returns_results_in_submission_order`: jobs whose
  specs finish in reverse order come back in submission order.

**Verification:** consumer gate; DE-trace gate against B0.

**Commit:** `perf: run fit evaluations on threads sharing the parsed weather`

**Dependencies:** CN-5; `06-runtime-memory.md` RT-4 (content inputs and
shared parsed handles, including their Python constructors), RT-5;
`05-python-surface.md` PY-3.

**Measurement:** proportional set size summed over the fit's processes
(`/proc/<pid>/smaps_rollup`, as the consumer measured before) during one
generation at the production population, before and after, recorded in
`02-measurement.md`. The `context.md` figure (about 4.7 GB for 26 processes,
about 1 GB for threads sharing every parsed input) assumed every input
shared; with per-candidate HPXML and defaults the target is re-derived from
the before measurement and recorded, not assumed.

## CN-11: per-evaluation provenance and replay

**Kind:** initiative work (`context.md` item 20).

**Problem:** no fit evaluation can be reproduced after the fact.
`RunResult.provenance()` records input paths, and an evaluation's injected
inputs have no lasting path: at `c71c542` each population slot's files
are overwritten every generation, so a recorded path points at whatever
the slot last wrote, and from CN-1 they exist only in memory.

**Evidence:** `calibration/engine.py:209-225` (`provenance()`: paths,
`engine_sha256`, seeds); `calibration/fit.py:215-222` (slot `i` always
writes under `m{i:03d}`), `:224-247` (`evaluate` keeps only the loss);
`calibration/inject.py:170-205` (the build writes HPXML, defaults and
schedule into the slot directory).

**Change:**
- `Fitter.evaluate` appends one `EvaluationRecord` per submitted spec to
  `FitResult.evaluations`: generation index, population member index, `x` (as `float.hex`),
  the loss or the `FailedRun` message, the sha256 of the un-injected source
  HPXML text (`inject.source_text`), of the schedule and of the weather
  file, the sha256 of the source defaults tree files the variant is built
  from, the consumer commit, the engine `build_info()` (commit and dirty
  flag, PY-6), `bldg_id` and `master_seed`.
- New `tools/replay_evaluation.py <fit record> <generation> <member>`:
  refuses if any recorded source hash differs from the current file;
  rebuilds the inputs with `Injector.build` from `x`; runs with the
  fit-time selection and checks the loss equals the recorded one bitwise;
  then writes the full verbosity-2 report to a Parquet path the caller
  names, for inspection. Registered in `build_bundle.py`.

**Acceptance test (testkit, `suite_fit`, engine-backed, on the suite's study
home `TRUTH` = `bldg0050945`, county `G4804390`, `STUDY_GRID`, 2-day fit, 1
generation, population 12):**
- `replay_reproduces_a_recorded_evaluation_bitwise`: a record from
  generation 1, replayed after the fit has finished and its `Fitter` is
  gone (nothing of its inputs is kept but the record), reproduces the
  recorded loss and total-power series bitwise.
- `replay_refuses_a_changed_source`: after one byte of the source schedule
  copy changes, replay exits with an error naming the file.

**Verification:** consumer gate; DE-trace gate against B0 (recording moves
no number).

**Commit:** `feat: record per-evaluation provenance and replay any fit evaluation`

**Dependencies:** CN-1, CN-6; `05-python-surface.md` PY-6.

## CN-12: surrogate search trial (report only)

**Kind:** initiative work, report only. Adopting any method the trial
reports on needs a later operator decision and is not part of this ledger.

**Problem:** the fit uses differential evolution seeded with the nominal
and warm-start vectors plus uniform random fill (`calibration/fit.py:321-324`),
at about 500 engine runs per fit (`context.md`). Whether a surrogate or a
screening method reaches the same fit quality with fewer runs is unmeasured.
The loss is a weighted sum of mostly absolute-error terms plus one RMSE
term (`calibration/objective.py:357-376`), not a sum of squares, so a least-squares
method is not a drop-in replacement.

**Change:** a trial, report only:
- Operator checkpoint for the new dependency, then `uv add --group trial
  egobox` (no other dependency, no production dependency).
- `tools/surrogate_trial.py` (registered) runs, on the production
  calibration set (the same four premises, seed 0), egobox Bayesian
  optimisation and Morris screening followed by DE on the retained factors,
  each against the same `Fitter` objective, and records loss, out-of-sample
  scores and engine runs per premise next to the B1 values.
- `docs/Surrogate_Trial.md` (registered) reports the table and the command.
- Nothing under `calibration/` changes. Adoption is a later operator
  decision and a new entry.

- The trial's runs and its check use the mechanism with the group
  installed by the step-2 sync, because `uv run --no-sync --group trial`
  installs nothing (`--no-sync` skips the sync that would add the group;
  measured in a scratch copy of the consumer venv: `uv run --no-sync
  --extra docs python -c 'import markdown'` raises `ModuleNotFoundError`):
  step 2 reads `uv sync --locked --extra engine --group trial
  --no-install-package ochre-next` (the pinned engine keeps its
  `ochre-next` name in the lock until CN-15), step 4's dry run uses the
  same flags and still prints exactly one "Would uninstall" line (the
  engine), and every command then runs with `uv run --no-sync`.
  `uv run --no-sync ruff --version` is recorded before and after the
  group is installed and is the same, so the gate's `ruff check` is
  unchanged by the group. A gate run of any other entry keeps step 2
  without the group.

**Acceptance test:** `surrogate_trial_reproduces_report`, a check mode of
the tool rather than a testkit case (a case would skip whenever the `trial`
group is absent and break the gate's exact skip set): after step 2 with
`--group trial`, `HARES_REPO=<worktree> uv run --no-sync python
tools/surrogate_trial.py --check docs/Surrogate_Trial.md --premise 0`
re-runs the first premise of the production set with the report's seeds
and exits 0 only when its loss, out-of-sample scores and engine-run count
equal the report's table row for that premise on bits (every trial method
is seeded). The report states the engine and consumer commits and its
commands, the step-2 sync included.

**Verification:** consumer gate (the new files are registered).

**Commit:** `docs: report the surrogate-search trial`

**Dependencies:** CN-4. Operator checkpoints (`plan.md` section 3): the new
dependency and running the trial.

**State:** landed as the report-only execution (2026-10-17, the ledger worktree
`r98-cn12`, branch `row/r98-cn12` on `eeef5700`); the full record in
`02-measurement.md` ("The surrogate-search trial, 2026-10-17"). The trial ran as
this entry specifies on the production set (the four premises, seed 0), each
method against the same `Fitter` objective, with the numbers recorded next to the
baseline values: row 97's B1 has not landed, so the baseline is the recorded B0
production capture, which row 96's gate re-captured twice on this tree bitwise
identical, so its values are this tree's DE behaviour. The scoping this row's
report-only deliverable forced, recorded rather than hidden: egobox 1.0.0 went
into the consumer venv only (`uv pip install egobox`; the manifest and `uv.lock`
unchanged, `uv sync --locked --extra engine --dry-run` reports no changes), the
tool lives at `scratch/surrogate_trial.py` with its raw numbers in
`scratch/surrogate_trial_results.json` (scratch/ is gitignored by design), and
the registered `tools/surrogate_trial.py`, `docs/Surrogate_Trial.md` and the
`trial` dependency group with the step-2 sync mechanism belong to the adoption
row an operator decision would open, not to this row. The numbers: BO at half the
recorded runs per premise (231, 203, 255, 255 against 462, 411, 513, 513) matches
or beats the baseline's loss on 3 of 4 premises (-3.21, -3.72, -7.61 percent) and
improves the out-of-sample monthly CVRMSE on the same three (5.08 to 3.28, 4.02
to 2.53, 3.97 to 3.34 percent), and regresses the fourth (premise 3: +3.12
percent, 3.56 to 4.40); Morris screening then DE on the retained factors is
rejected (the surface is not sparse: 9/9, 8/8, 10/10 and 9/10 factors move at
least 2 percent of span in the baseline fits; the method totals 106 and 105
percent of the baseline's runs on premises 0 and 1 for a worse loss, and the
retained box misses the heating directions on the gas furnaces, premise 3's loss
+38.94 percent at 16.56 percent out-of-sample monthly CVRMSE, with the retention
unstable across two same-archetype homes); the GP's measured fidelity (5-fold CV
q2 on the penalty-free points) is 0.841, 0.622, 0.931 and 0.594. The verdict:
NOT WORTH ADOPTING as the production fit stands; the one positive worth a new
entry is the hybrid shape (BO's first 100 runs sit within about 3 percent of the
DE's final loss on 3 of 4 premises, at a lower wall cost than a DE stage), which
needs more premises and seeds and an operator decision. Every trial method is
seeded and the tool's check mode re-ran premise 0 with the report's seeds and
compared its loss, in- and out-of-sample scores and engine-run counts with the
stored row on bits: PASS, exit 0. Gates: the consumer `run_all` 383 passed, 0
failed, 7 skipped (the base seven) on the same tree, ruff (the configured select,
E9 and F821) clean, the engine untouched; no golden, baseline or recorded number
moves (report only).

## CN-13: the consumer follows the engine package rename

**Kind:** initiative work, consumer half of `dl-i08-architecture.package`
(distribution `hares-py`, import name `hares`, cache `~/.cache/hares`, no
`ochre_next` alias).

**Problem:** the consumer names the engine `ochre-next`/`ochre_next` in its
dependency, its import, its data import and its cache path. With no alias,
the import fails; with the cache moved, every cached building disappears
and the engine-backed cases skip.

**Evidence** (`git grep -n -i -E 'ochre[_-]next'` at `5c651bc`, the
consumer this entry builds on; lines at `c71c542` are the same unless
stated): `pyproject.toml:22, 32` (`engine = ["ochre-next"]` and
`[tool.uv.sources] ochre-next = { path = "HARES", editable = true }`);
`uv.lock:181-188`. Imports of `ochre_next`:
- `calibration/paths.py:76-93` (`engine()` imports `ochre_next`, then
  `hares`), `:105` (`on._hares`), `:116-117` (`CACHE = ... / "ochre_next"`);
- `calibration/stock.py:331` (`from ochre_next.data import resstock`);
- `calibration/ami2/cvr_actuation.py:126-128`;
- `calibration/ami2/dr_actuation.py:77, 174`;
- `calibration/ami2/tariff.py:71, 82`;
- `calibration/forward_model.py:260-262` (`import ochre_next`, with a
  fallback `import HARES`, a package name that never existed);
- `calibration/hares_der_remeasure.py:44`, `hares_l1_archetype_sweep.py:42`,
  `hares_ua_xval.py:50`;
- `calibration/tests/suite_ami2.py:799, 850, 1737, 1956` (`:1696, 1915` at
  `c71c542`);
- `tools/provision_data.py:65-66` (`ochre_next.data.resstock` and
  `.weather`).

Text naming it: `calibration/forward_model.py:232-237, 284`;
`calibration/coupling.py:373`; `calibration/der_evidence.py:46`;
`calibration/run_calibration.py:195`; `calibration/hares_der_remeasure.py:5,
11, 16, 48`; `hares_l1_archetype_sweep.py:19, 45`; `hares_ua_xval.py:25,
53`; `calibration/paths.py:76, 89`; `calibration/ami2/cvr_actuation.py:128`;
`tools/provision_data.py:6`; `calibration/tests/suite_scale.py:335` (at
`5c651bc`, a stubbed `engine_so` path under `python/ochre_next`);
`AGENTS.md:12, 22, 24`; `STRUCTURE.md:33, 122`; `CONTRIBUTING.md:28`; and
the current-tense instructions `docs/GridLABD_Install.md:189` and
`docs/HARES_Calibration_Code_and_Verification.md:600, 652`. The other
`docs/` and `project_history/` hits are dated records of what was true
when they were written (`docs/Red_Team_Record.md`, the Discussion State,
Open Items, AMI2 Roadmap, Engine Findings and HVAC Change Request
documents, `Code_and_Verification.md:524`), which this entry does not
rewrite.

**Change:**
- Every `import ochre_next` imports `hares`: `paths.engine()` imports
  `hares` only (the `ochre_next` attempt goes); `stock.py` and
  `tools/provision_data.py` import `hares.data`; the `ami2`, DER tool and
  `suite_ami2` sites above import `hares` (or call `paths.engine()`, which
  returns it); the `forward_model` fallback to `import HARES` goes.
  `CACHE` (`paths.py:116-117`) is `$XDG_CACHE_HOME/hares` or
  `~/.cache/hares`.
- The dependency name (`pyproject.toml` extra and `[tool.uv.sources]`, and
  `uv.lock`) is not changed here: the source is the `./HARES` submodule,
  still at `0ec757cb`, whose distribution is `ochre-next`, and `uv lock`
  reads the name from it. CN-15 renames the extra and the source to
  `hares-py` and regenerates the lock in the same commit that advances the
  pin (no other package moves; checked by diffing the lock's package list).
- The existing cache moves from `~/.cache/ochre_next` to `~/.cache/hares`
  under an operator checkpoint (nothing deleted). The command refuses to
  nest the source inside an existing target (for example one created by a
  `[pytest]` run after PY-7):
  `c="${XDG_CACHE_HOME:-$HOME/.cache}"; test ! -e "$c/hares" && mv "$c/ochre_next" "$c/hares"`.
  If `$c/hares` exists, the row stops and the operator decides; nothing is
  merged by hand. A sha256 manifest of every AMY, TMY3 and metadata file B0
  used is taken before the move and must equal the manifest after it.
- Every text site in the evidence names `hares` (the `suite_scale.py:335`
  stub path reads `python/hares/_hares.so`).
- Census, each printing nothing: `git grep -n -i -E 'ochre[_-]next' --
  calibration tools AGENTS.md STRUCTURE.md CONTRIBUTING.md
  docs/GridLABD_Install.md` (CN-15 runs it again with `pyproject.toml
  uv.lock` added); `git grep -n -F 'needs a built `ochre_next`' --
  docs/HARES_Calibration_Code_and_Verification.md` (the two current-tense
  lines; the file's dated section 4.7 keeps its record of the 2026-07-02
  build); `git grep -n -E 'import HARES\b' -- calibration tools`.
- The contract-drift rule (preamble) for the engine rows after CN-17's
  range up to row 26.
- Recorded fact for the rows this consumer is checked on: from
  `05-python-surface.md` PY-12 (row 30a) a ResStock metadata file without a
  weight column, or a requested building absent from it, is an error.
  `stock.fetch` passes `paths.META_PARQUET` (`stock.py:333-337`), whose
  `upgrade0_2025_1.parquet` carries a `weight` column (549,971 rows), so
  the CN-13 consumer runs unchanged on those rows.

**Acceptance test (testkit, `suite_engine`):**
- `engine_module_is_hares`: `paths.engine().__name__ == "hares"`.
- `cache_root_is_hares`: `paths.CACHE.name == "hares"`.
- The gate's skip rule: a missing cache shows as new skips and fails the
  gate.

**Verification:** consumer gate and DE-trace gate against B0, both on the
engine commit of `05-python-surface.md` PY-7 (the renamed engine with
today's construction surface, which the consumer at this commit still
calls); `contract.txt` holds `[hares-contract] PASS` on that engine. From
this commit on, steps 2 to 4 of the mechanism name `hares-py`.

**Commit:** `fix: follow the engine rename to hares-py`

**Dependencies:** CN-16, B0; `05-python-surface.md` PY-7 (directly
before it in the schedule). Operator checkpoint for the cache move.

**Landed:** consumer `70ebbfc` on branch `row/cn13-follow-rename` (cut
from the pin `78568b5`); gate engine `b2419945` (the engine worktree
branch `row/r27-cn13`: row 26's rename with rows 28 to 30b and this
row's engine-side fix on top, so this gate's capture is also rows 28 to
30b's B0 check). The pyproject's engine extra and the uv source key are
`hares-py` (the source stays the `./HARES` submodule, `editable`;
the gitlink untouched at `5ece4060`), `uv.lock` regenerated with the
package list exactly the rename (revision 3 to 5 with the current uv).
Every old-name import and text site migrated: `paths.engine()` imports
`hares` only, `CACHE` is `$XDG_CACHE_HOME/hares`, `stock.fetch` and
`provision_data.py` import `hares.data`, the ami2 and DER tool sites
import `hares`, the `forward_model` fallback to `import HARES` is gone,
`suite_scale`'s stubbed `engine_so` paths read `python/hares`, and the
AGENTS/STRUCTURE/CONTRIBUTING/GridLABD_Install/Code_and_Verification
text sites name the new package; the three census greps print nothing.
The engine-side fix `b2419945` the gate needed: the sample-weight lookup
recognized only a `sample_weight` variant while the shipped 2025.1
metadata parquet names its weight column exactly `weight` (549,971
rows), so every `stock.fetch` failed up front; `_weight_column` accepts
the exact name now (the `calc.weighted.*` aggregates cannot match it),
pinned in `tests/python/test_resstock_weights.py`. Two consumer-side
robustness fixes the gate needed: `cached_homes` hands out the same
repaired HPXML `fetch` does (the raw cache documents hit the spa
strictness and crashed the schema emit) and skips the two cache entries
whose upstream zips ship no schedule member (`bldg0000100`,
`bldg0000673`, measured at OEDI). The cache moved to `~/.cache/hares`
under the operator checkpoint: 6741 files, sha256 manifests identical
before and after. Two acceptance cases (`engine_module_is_hares`,
`cache_root_is_hares`) and the four schema-drift classifications
(the `max_consecutive_step_failures` kwarg as a strictness policy, the
skylight `AttachedToRoof`/`AttachedToFloor` idrefs and the
`IsSharedAppliance` marker as structural). The error-transport pin now
pins the round trip (the exceptions pickle as their own classes with
their args, row 28's contract). Gate: 348 passed, 0 failed, 7 skipped
(the base seven; the two acceptance cases are the +2), ruff clean,
`contract.txt` PASS on the schema regenerated against the gate engine
(sha `da6f1391ab79bcc0`), bundle VERIFY clean. Captures recorded in
`02-measurement.md`: the capture is `identical` twice, `identical`
across the rename itself (the old consumer at the pre-rename engine
`97e96368` versus this consumer at `f35eef8a`), and the delta against
the shipped B0 file belongs to the divergent audit line the baseline
was captured on (`27db4a1e`, not an ancestor of the gate engine),
present before the rename. Steps 2 to 4 of the mechanism name
`hares-py` from this commit on (step 4's one `Would uninstall` line
names `hares-py`).

## CN-14: consumer weather alignment follows the engine

**Kind:** initiative work, consumer half of `09-numeric-fixes.md` B-1a to
B-1c.

**Problem:** the consumer shifts every weather series by one row to match
the engine's hourly lag, and interpolates sub-hourly outdoor temperature
itself. After B-1a the engine reads the record of the hour a step ends in,
so the shift double-counts, and the consumer's own interpolation no longer
matches the engine's.

**Evidence:** `calibration/irradiance.py:48-67` (`ENGINE_ROW_OFFSET = 1`,
measured at 3600 s: `engine_row[i] == file_row[i - 1]`), `:85-92` (the
roll), `:123, 207` (references); `calibration/der_screen.py:21`;
`calibration/ami2/metered.py:22, 60-64` (`OAT_HOUR_CENTRE_OFFSET_H`, a
measured half-hour offset), `:67-82` (`oat_steps`, its own interpolation,
documented as an approximation of the engine's), `:125`;
`calibration/tests/suite_network.py:3006, 3022`.

**Change:**
- `ENGINE_ROW_OFFSET`, `OAT_HOUR_CENTRE_OFFSET_H` and the roll go;
  `Weather.read()` returns file order, which after B-1a is engine row order
  at 3600 s.
- `oat_steps` returns the engine's own resampled outdoor dry bulb for the
  grid, from the engine's `resample_weather` function, so the consumer's
  temperature series equals the engine's by construction.
- The docstrings at `irradiance.py`, `der_screen.py:21` and
  `metered.py:22` state the new rule.
- The row-count check of `Runner.run` (`engine.py:371-376`, `expect =
  self.grid.n_steps(spec.duration_days)`) assumes every calendar step is
  simulated, which B-1c makes false for a leap-year run on a typical file
  without Feb 29. It compares the table's row count with the simulated
  step count the engine reports for the run (B-1c: the resolved clock of
  `DwellingBlueprint.validate()`, `12-inputs.md` IN-2), and `Grid.n_steps`
  stays the request, not the expectation.
- The contract-drift rule (preamble) for rows 79 to 81:
  `hares_contract.py:147-150` and `KWARG_RULES` (`coverage.py:111-127`)
  gain `first_hour_interpolation` and `weather_year` where the adapter
  sets them, and every DRIFT item the census reports is resolved here.

**Acceptance test (testkit, `suite_engine`, engine-backed, on
`engine._default_spec`: `bldg0008590`, AMY 2018 weather of county
`G4802010`):**
- `consumer_weather_equals_engine_outdoor_temperature`: over 2 days at
  3600 s and at 900 s, the consumer's series equals the engine's outdoor
  dry-bulb output column bitwise (the same oracle that measured the
  offset).
- `row_check_uses_the_engines_step_count`: the same home on the TMY3 file
  of its county (`<cache>/weather/BuildStock_TMY3_FIPS/G4802010.epw`, header
  "HOLIDAYS/DAYLIGHT SAVINGS,No", 8760 records) from 2024-02-27 for 5 days
  at 3600 s returns 96 rows, the engine's reported count, and `Runner.run`
  accepts it.

**Verification:** consumer gate; `contract.txt` holds
`[hares-contract] PASS` on this row's engine. Absorbed by B1 (the fit's
warm start regresses on outdoor temperature).

**Commit:** `fix: align consumer weather with the engine's EnergyPlus convention`

**Dependencies:** `09-numeric-fixes.md` B-1a, B-1b, B-1c; `12-inputs.md`
IN-2 (`validate()`), IN-4 (`resample_weather`, Python binding included);
CN-1.

**State:** committed `c7b56ddb` (consumer, branch `row/cn14-weather`, cut
from the landed main `78f20657`); the engine side is the row's surface fix
on branch `row/r82-cn14`, `b7b2159e` (the ledger record names it). Landed:
`ENGINE_ROW_OFFSET`, `to_engine_rows`, the `align` kwarg and
`OAT_HOUR_CENTRE_OFFSET_H` deleted; `Weather.read()` returns file order,
which the engine's record-of-the-hour-ending convention makes engine row
order at 3600 s; `oat_steps` returns the engine's own resampled outdoor
dry bulb, from `hares.resample_weather` (the site the weather file's own
header carries through `ParsedWeather`: the site moves the solar and
mains channels, never the dry bulb; and the window the file's start on a
non-leap year, so a typical-year read maps day-of-year in file order);
the row-count check of `Runner.run` compares the frame with the engine's
reported simulated step count (the resolved clock, read through the
dwelling's resolved-model read-back the engine fix added; `validate()`
carries the same form) and `Grid.n_steps` stays the request; the
contract's `SimulationConfig` set gains `first_hour_interpolation` and
`weather_year`, the accepted-keyword classification gains the two weather
fields and `warmup` (the census engine carries the warm-up row already,
whose contract movement is the next consumer rows' range; classified here
because the gate reads the published schema), and the accepted list no
longer holds the removed warm-up keyword. RED first: the two acceptance
cases failed at the base on the row engine (the bitwise dry-bulb
comparison at 3600 s and 900 s; `expected 120 rows for 5d at 3600s, got
96`), and the coverage suite named the three unclassified kwargs and the
removed key the accepted list still held (the binding refuses the
keyword; the stub advertised it: the engine fix's RED is the read-back
clock pin failing on `KeyError: 'clock'`). The der-inject alignment case
states the new rule and its raw-order converse case is deleted (raw file
order is engine order now; the discrimination probe carries the
could-have-failed half); the network dry-bulb case asserts the engine
series' shape and skips loudly without the engine. Gates: run_all
378 passed / 2 failed / 7 skipped (the base seven) on the staged change
and again on the commit; the two failures fail byte-identically at the
base consumer on this engine and are owned by the weather and warm-up
rows' declared movement (their bands were measured on the pre-initiative
engine; the re-measure is the re-baseline row's), the row-73 rule. ruff
clean; `contract.txt` PASS on the row engine (3 NEW items informational).
The B0 production set captured three times (twice on the staged change,
once on the commit) `identical`; the compare against the shipped B0: 272
differing paths raw, 268 under the recorded declared-counts rule: the
recorded 260 divergent-audit-line paths plus 8 new (premises 2 and 3's
fitted parameters, scores and bound flags), attributed to the weather and
warm-up rows' declared engine movement, not this row: the fit path calls
none of the changed code and the capture is bitwise stable across the
three runs. The bundle ran on the staged change (every registry file
present, no untracked files, contract PASS, NOT GREEN on the same two
owned failures).

## CN-15: advance the engine pin

**Kind:** initiative work (consumer `AGENTS.md` rule 5).

**Problem:** the consumer's gitlink stays at `0ec757cb` while every
consumer commit is gated against an engine worktree; the final consumer
state must pin the engine it was proven against, and only an engine commit
that is on public origin.

**Evidence:** `git submodule status` at `c71c542`: `0ec757cb HARES`;
consumer `AGENTS.md` rule 5 (a pin bump carries the full gate, a
regenerated `docs/Test_Report.md` and the recorded engine commit).

**Change:** after the operator has pushed `feat/output-results` for the
last time (operator checkpoint) and with the operator's permission for the
checkout in the nested clone:

```
git -C HARES fetch origin <sha>
git -C HARES checkout --detach <sha>
git add HARES
# pyproject.toml: extra engine = ["hares-py"], [tool.uv.sources] hares-py = { path = "HARES", editable = true }
uv lock
uv sync --locked --extra engine
mkdir -p scratch/i08/<sha>
export HARES_REPO="$PWD/HARES"
uv run python -m calibration.schema --emit --all-cached
uv run python -m calibration.tests.run_all --report > scratch/i08/<sha>/gate.txt
uv run python calibration/hares_contract_check.py > scratch/i08/<sha>/contract.txt
uv run ruff check
uv run python tools/capture_de_trace.py capture --out scratch/i08/<sha>/capture.json
uv run python tools/capture_de_trace.py compare \
  /home/rich/src/HARES/.shipwright/initiatives/I-08/baselines/B1-de-trace.json scratch/i08/<sha>/capture.json
git grep -n -i -E 'ochre[_-]next' -- calibration tools pyproject.toml uv.lock AGENTS.md STRUCTURE.md CONTRIBUTING.md docs/GridLABD_Install.md
uv run python build_bundle.py --out scratch/i08/<sha>/bundle.zip > scratch/i08/<sha>/bundle.txt   # staged change
# after the commit; ids and writing() exactly as plan.md §3's verification block defines them
writing ~/src/HARES_CALIBRATION_SYSTEM                                                            # [writing]
```

The contract-drift rule (preamble) for the engine rows committed after
CN-3's engine (`02-measurement.md` M-4a and M-4b, and any CI fix entry of
the closing sequence): `contract.txt` is read on the pinned engine, and
every DRIFT item it reports is resolved in this commit. Those rows change
no number by their kind (a workflow, a published document); a CI fix entry
that moves a number, which the golden comparison of its own row shows,
stops the closing sequence before M-4b for an operator decision, because
`dl-i08-architecture.consumer` allows one consumer re-baseline (B1) and
such an entry would need a second.

`HARES_REPO` names the pinned submodule for every command, so the
capture's provenance guard, the schema emit, the gate case and the
contract check all read the tree the pinned engine was built from (from
CN-17 the check resolves through `paths.HARES_REPO`, which this export
fixes to the same tree). The lock diff moves only the engine package (its
name, version and source). The commit carries the gitlink,
`pyproject.toml`, `uv.lock`, `docs/Test_Report.md` (which replaces the
stale report CN-16 names), the bundle's index files and the engine commit
in its message; `docs/Red_Team_Record.md:116`'s rebuild instruction
(`--reinstall-package ochre-next`) names `hares-py` in the same commit, the
one current-tense instruction in that record. No consumer push carries a
gitlink to an engine commit that is not on public origin.

**Acceptance test:** the consumer gate passes with the pinned engine built
by plain `uv sync` (no worktree; `HARES_REPO` is the pinned submodule),
`contract.txt` holds `[hares-contract] PASS`, the DE-trace `compare`
against B1 exits 0 (per-premise payload only; the header differs by
design), the census prints nothing, and `writing` passes on the commit.

**Verification:** as above.

**Commit:** `chore: pin the engine to <sha>`

**Dependencies:** CN-4, CN-12; `02-measurement.md` M-4b; the operator's
last push of `feat/output-results` (`plan.md` §3 closing sequence), so the
pin names the final pushed engine commit.
