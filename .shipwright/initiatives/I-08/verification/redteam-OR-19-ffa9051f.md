# Red team: OR-19 roster plan/install (ffa9051f)

Verdict: ATTACK-SUCCEEDED

Counts: blocker 0, major 2, minor 3, info 3.

Method: a detached worktree at ffa9051f with scratch tests only (an include
file in the dwelling test module, `tests/rt_dump.rs`, `tests/rt_badpath.rs`;
none committed). Baseline outputs came from a `git archive` of 7b7a6a7d built
in the session scratch directory. The existing hares-core suite (1010 tests,
all integration binaries, goldens, parity, profiling_alloc) passes, and
`cargo check --workspace --all-targets` is clean.

## What held

- Every entrance rejection (add_equipment, replace_equipment,
  remove_equipment, remove_equipment_by_end_use, add_actor,
  auto_register_actors) leaves the dwelling exactly as it was. The fingerprint
  covered equipment and actor order, ids, `next_equipment_id`, name map,
  execution order, ports, rollback ports, loop-type map, thermal consistency,
  column index, equipment, end-use and actor column maps, value count, scratch
  length, zone caches, latest_env core keys, auto actor names, scheduler plan,
  recorder row count and schema, and warning count. The drifted-schema case
  (rows already recorded) was used for the actor, removal and auto-register
  entrances, and the fluid-type conflict for add and replace. Post-step
  rejection leaves `save_checkpoint` Debug output identical.
- Replace with the same name, remove then re-add (counter unchanged at 4),
  clear then add on a freed loop (Glycol accepted on loop 7), and the
  write_output=false path (StepResult identical to an untouched dwelling over
  four steps after an add and remove) all behave.
- Assembled output versus 7b7a6a7d: 14 OCHRE sample fixtures x write_output
  {true,false} x tariff {none,default} = 56 runs (134 files: step results and
  recorded CSVs), byte-identical. One fixture (windows-physical-properties)
  fails construction identically in both.
- Schema metadata: for every fixture, removing and re-adding every equipment
  one at a time gives the same field set with the same metadata (compared
  canonically, since Arrow metadata is a HashMap and its Debug order is not
  stable). The only difference is the EvDriver actor columns, see M-3.
- The two `AsRef` impls are sound: the Equipment impl is
  `impl<'a> AsRef<dyn Equipment + 'a> for &'a dyn Equipment`, so a shorter
  lifetime object cannot be widened, and Actor is `'static`. No misuse path.
- Roster changes are O(N) per call (quadratic cumulatively): 2000 adds take
  about 1 to 5 s on both 7b7a6a7d and ffa9051f in this noisy WSL2 session. This
  is not a regression. The step path is untouched and profiling_alloc passes.

## Findings

### M-1 (major, new, regression of an error path): assembly now swallows a recorder creation failure

Reproduction (`tests/rt_badpath.rs`): `write_output: true`,
`output_verbosity: 3`, `output_path: /proc/nonexistent/x/out.csv`,
`Dwelling::from_config(base.xml)`.

- 7b7a6a7d: `Err(io error: output recorder init failed: ... No such file or directory)`.
- ffa9051f: `Ok`; the warning log holds two copies of "output recorder re-init
  failed ... subsequent output is dropped"; `step()` and `simulate()` succeed
  and write nothing.

Cause: assembly now builds its recorder inside `install_roster_caches`
(`crates/hares-core/src/dwelling/mod.rs:4651-4675`), which treats
`StreamingRecorder::new` failure as a warning. The warning branch was written
for a runtime re-init, but assembly used to propagate this as a hard error. A
Python user with a bad output directory now gets a "successful" run with no
results file, which also violates the project rule of never substituting silent
fallbacks. (At verbosity >= 4 the diagnostic file create fails first, which is
why only verbosity <= 3 shows it.)

Fix: make recorder construction part of the plan. In `plan_roster_caches`
(`FreshSchema` arm, `:4390-4408`), construct the `StreamingRecorder` (and its
`enable_metrics`) there and return `Err` on failure, carrying the finished
recorder in `OutputCachesPlan::FreshSchema`; install then only assigns it. That
restores the Err at assembly and makes the "install is infallible" claim true.
Metrics init failure may stay a warning (it was before).

### M-2 (major, found by claim, same root as M-1): install can leave a half-changed dwelling

Reproduction (`rt_recorder_failure_after_install`): assemble base.xml with
write_output true, set `output_path` to a path that cannot be created, then
`add_equipment(TestEquipment)` before the first step. `add_equipment` returns
`Ok`; `output_value_count` is 90 and the column maps are for the new schema, but
`self.recorder` is still the old 85-column recorder (the failed construction
left it in place). The next `run_timestep(true)` fails with
`record push failed: row has 90 values but schema has 85 columns`.

So the claim "install cannot fail" is false when recorder creation fails: the
roster commits, the dwelling is internally inconsistent and unusable. Same fix
as M-1 (construct in the plan). If the recorder must stay in install, the
failure arm has to at least set `self.recorder = None` and be reported by the
entrance as an `Err` with the roster rolled back, which is why the plan is the
right place.

### m-1 (minor, pre-existing shape, in the claimed scope): set_tariff is not all-or-nothing

Reproduction (`rt_rejections_all_entrances`): on the drifted dwelling,
`set_tariff(ElectricTariff::default(), UTC)` returns `Err` (from
`auto_register_actors`) but `tariff_evaluator` is now `Some` and the tariff's
parse warnings are already in the log. The dwelling bills with a tariff whose
price-aware actors were never installed.

Cause: `mod.rs:4708-4709` assigns `self.tariff_evaluator = Some(evaluator)`
before `self.auto_register_actors()?`, and parse warnings are pushed at the top
(`:4698-4702`). Fix: make `auto_register_actors` take the price schedule and
tariff presence as parameters (or split it into plan and commit like the other
entrances), call the plan with the new evaluator, and assign the evaluator and
push the warnings only on `Ok`. Add `set_tariff` to the rejection test.

### m-2 (minor): rejected add and replace write to the dwelling's warning log

`add_equipment` and `replace_equipment` drain the rejected candidate's init
warnings into `self.warnings` on every `Err` (`mod.rs:3683-3696`, `:3716-3722`,
`:4098-4106`, `:4121-4131`, `:4163-4169`). This is deliberate and documented in
the code, but it contradicts the ledger's "bit-for-bit as it was" claim; the
fingerprint's warning count changes whenever a candidate with init warnings is
rejected. Fix: either state the exception in OR-19's acceptance text, or return
the candidate's warnings inside the error. Not a correctness hazard.

### m-3 (minor): stale references to the deleted function

`refresh_equipment_caches` is still named in comments at `mod.rs:789`, `:1837`,
`:1911`, `:3075`, `:4211`, `:5480` and in
`crates/hares-core/tests/equipment_id_regressions.rs:2275`. Also at
`mod.rs:4222-4235` the old "Refreshes internal caches" doc block (with its
`# Errors` section) sits directly above the `RosterCaches` doc and is rendered as
the doc of `plan_roster_caches`; delete it. Install-time duplicated recorder
warnings (assembly plans and installs twice) also go away with M-1's fix.

### info

- M-3 (info, pre-existing): removing an EV and re-adding it evicts the EvDriver
  actor and does not re-register one (`add_equipment` never calls
  `auto_register_actors`), so the EV's actor columns vanish from the schema
  (166 to 157 columns on base-misc-loads-large-uncommon.xml). Documented
  eviction semantics, but callers must run `auto_register_actors` after
  re-adding.
- I-2: replacing equipment under a different name leaves actors targeting the
  old name registered (only removal evicts). In the base-battery run the
  replaced "Renamed" battery stepped into an `soc_bounds` error. This is
  pre-existing and a separate defect class from OR-19.
- I-3: removed equipment's `latest_env.equipment_core` entry survives until the
  end of the next step (pre-existing, documented in code).

## Residual risk

Not run: the Python test suite and maturin build (no Python runtime wired in
this worktree), `cargo test` for crates other than hares-core, and EV, PV,
protocol-bridge and water-heater schema parity through the Python constructors.
Parity was checked through the HPXML fixtures, which cover battery, PV, water
heaters and the EV, and through the single shared
`enrich_schema_with_telemetry_units` call, not through the Python entrance for
each type. Protocol bridge has no fixture; it uses the same plan path.
