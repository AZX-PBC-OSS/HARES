# Verification L6-a2: re-pin audit at 7b7a6a7d

Scope: `ledger/03-output-reporting.md` entries not yet committed among
OR-1 to OR-17 (OR-5 and OR-11 are already committed and out of scope per
schedule; OR-18 and OR-19 excluded per instructions), all of
`ledger/05-python-surface.md`, all of `ledger/12-inputs.md`. Code checked
at the worktree `/home/rich/src/HARES/.workstrees/audit-7b7a6a7d`, HEAD
`7b7a6a7dcbb63f99452e0f3d03a5ded2d292ee58`.

Entries read in full and checked against the schedule, plan.md §2.1-§2.7
and §3, and the code at HEAD (or at the commit an entry names, via
`git show <sha>:<path>`):

OR-1, OR-2, OR-3, OR-4, OR-6, OR-7, OR-8, OR-9, OR-10, OR-12, OR-13,
OR-14, OR-15, OR-16, OR-17 (file 03); every entry in file 05
(package rename, config type, Python export, API surface, callback and
actor outputs, RL/gym, build identity); every entry in file 12 (input
records and digests, `DwellingBlueprint::validate()`, the input schema,
`resample_weather`, the setpoint-source rule, the OCHRE resampling
removal, the envelope scales).

Counts: 0 blocker, 1 major (one finding class covering 19 confirmed
individual citations), 2 minor.

## Findings

### MAJOR: stale `crates/hares-core/src/dwelling/mod.rs` citations throughout file 03's not-yet-committed OR entries

**Entries affected:** OR-1, OR-2, OR-3, OR-6, OR-7 (and, by inheritance,
OR-13, which repeats OR-7's citation). This is the exact risk the task
named: these entries were re-pinned to `7b7a6a7d` without re-deriving
their line numbers, and `dwelling/mod.rs` grew substantially between the
entries' original baseline and `7b7a6a7d` through rows that *are*
committed and in scope for these entries to build on: OR-5
(`dc033cee`/`469ea955`, health and invariant enforcement), OR-11
(`b8195c83`, every remaining invariant check classified), and PR-1/PR-2/
PR-3/PR-6 (`d7f0e381`/`21d90588`/`26626710`/`afa9a071`, profiling). All of
these inserted code into exactly the regions OR-1/OR-2/OR-3/OR-6/OR-7
cite: the assembly function body, `record_step`, and the equipment/actor
column-mapping helpers. Every other file family checked (`hares-io`'s
`columns.rs`/`writer.rs`/`metrics.rs`, `hares-tariff`'s
`evaluator.rs`/`billing.rs`, `hares-core`'s `checkpoint.rs`/`engine.rs`,
and every `hares-python` file) came back accurate on every citation
checked, confirming the drift is confined to this one file in the
regions those committed rows touched.

Confirmed instances (symbol, entry, citation, actual location at
`7b7a6a7d`):

- `build_equipment_column_map` — OR-1 (`ledger/03`:48) cites
  `mod.rs:376`; actual `fn build_equipment_column_map` is at
  `mod.rs:540`. Line 376 at head is unrelated profiling code
  (`enter_plan_entry`).
- `resolve_col` — OR-1 (`ledger/03`:276, OR-2's problem text) cites
  `mod.rs:645-665`; actual `fn resolve_col` is at `mod.rs:814`. Lines
  645-665 at head are an unrelated closure inside
  `build_equipment_column_map`.
- Actor column discovery (`format!("actor:{name}:{key}")`) — OR-1
  (`ledger/03`:56) cites `mod.rs:741-764`; the actual
  `fn build_actor_column_map` and its doc block are at `mod.rs:778-804`.
- `record_step`'s core electric/gas/SOC/mode columns — OR-1
  (`ledger/03`:59) cites `mod.rs:6636-6800`; actual `fn record_step` is
  at `mod.rs:7165`.
- `zone_display_name` — OR-1 (`ledger/03`:181) cites `mod.rs:7790`;
  actual `fn zone_display_name` is at `mod.rs:7980`.
- `output_column_index` per-step lookups — OR-2 (`ledger/03`:272) cites
  `mod.rs:6618-6634`; the real lookups inside `record_step` are at
  `mod.rs:7169+`. Lines 6618-6634 at head are an unrelated BMS
  PV-staleness diagnostic.
- OR-2 (`ledger/03`:302) cites `mod.rs:6753-6791` for the equipment
  column map rebuild path; that range at head is the islanded
  `island_unserved_kw`/`island_excess_kw` block, unrelated.
- `write_output = false` skipping schema construction — OR-3
  (`ledger/03`:343) cites `mod.rs:2476-2480`; the actual gate
  (`if config.sim_config.write_output { ... } else { ... }`) is at
  `mod.rs:2747`. Lines 2476-2480 at head build the zone-to-role map,
  unrelated.
- `default_output_path` used at assembly — OR-3 (`ledger/03`:350) cites
  `mod.rs:2474`; the actual call site is at `mod.rs:2741`.
- `flush_and_close()` — OR-3 (`ledger/03`:398) and OR-6
  (`ledger/03`:1937) both cite `mod.rs:2869`; the actual (only) call
  site is at `mod.rs:3146`.
- Diagnostics CSV writer gate (`output_verbosity >= 4`) — OR-3
  (`ledger/03`:401) cites `mod.rs:2819-2830`; the actual gate
  (`dwelling.diagnostic_writer = if dwelling.output_verbosity >= 4`) is
  at `mod.rs:3056`.
- Test equipment returning `Err` from `step()` for
  `failed_run_leaves_no_partial_file` — OR-3 (`ledger/03`:432) cites
  `mod.rs:13847`; that line at head is inside an unrelated zone-validation
  test assertion. No equipment test double matching the description
  exists at that location; the closest candidates in the file
  (`FailingProbeActor` at `mod.rs:14120`, `FailingPortEquipment` at
  `mod.rs:14548`) are different mechanisms (an actor and a port failure,
  not a `step()` error) and neither is at the cited line.
- `Dwelling::simulate()` — OR-6 (`ledger/03`:1934) cites `mod.rs:2849-2877`;
  actual `fn simulate` is at `mod.rs:3126`. The cited range at head is
  inside the `Dwelling` struct literal built during assembly, not
  `simulate()`.
- `step()`'s unbounded-history return — OR-6 (`ledger/03`:1941) cites
  `mod.rs:2966-2967`; the actual
  `self.simulation_results.steps.last().unwrap().clone()` is at
  `mod.rs:3244`.
- `SimulationResults.steps` struct field — OR-6 (`ledger/03`:1971) cites
  `mod.rs:860-862`; the actual `pub struct SimulationResults { pub steps:
  Vec<StepResult>, ... }` is at `mod.rs:1034-1035`. This same stale
  citation also appears in `plan.md` §2.1 ("no retained step history
  (`SimulationResults.steps`, `mod.rs:860-862`, is deleted)"), so the
  error is inherited from the plan rather than introduced by the ledger,
  but it still needs correcting in both places before OR-6 starts.
- The step-history push and clear sites — OR-6 (`ledger/03`:1972) cites
  the push at `mod.rs:6583` and the clears at `mod.rs:5045,5064`; the
  actual push (`self.simulation_results.steps.push(step_result)`) is at
  `mod.rs:7136`, and the actual clears
  (`self.simulation_results.steps.clear()`) are at `mod.rs:5481` and
  `mod.rs:5509`. The cited lines land in, respectively, unrelated
  humidity/electrical-solver resolve code and unrelated checkpoint-restore
  code (equipment state load and thermal-state restore).
- `Time` column formatting with chrono `"%+"` — OR-7 (`ledger/03`:2056)
  and OR-13 (`ledger/03`:2476, which repeats the same citation) cite
  `mod.rs:7009-7010`; the actual `write!(&mut self.timestamp_buf, "{}",
  step.timestamp.format("%+"))` is at `mod.rs:7532`. This citation is
  also inherited from `plan.md` §2.5, which cites the same wrong line.

**Why this matters:** OR-1/OR-2/OR-3/OR-6/OR-7 all instruct the
implementer to delete, replace or call specific functions at specific
lines as part of a mechanical migration (`output_column_index` removed
"so the step path cannot reach it", `resolve_col` "replaced by `RowPlan`
construction", `flush_and_close` moved under `finish()`, the step-history
push/clear deleted). An implementer following the cited lines literally
would edit the wrong code — in several cases, code in an unrelated
subsystem (BMS staleness logging, islanded-operation handling, checkpoint
restore) — while leaving the real target untouched. Several of the
functions involved (`record_step`, `simulate`, the assembly builder) are
large enough that a line number 150-550 lines off lands in a different
logical block of the *same* function, not just a different function nearby.

**Correction:** re-derive every `crates/hares-core/src/dwelling/mod.rs`
citation in OR-1, OR-2, OR-3, OR-6, OR-7 and OR-13 against `7b7a6a7d`
before any of these entries starts, using the actual line numbers given
above (and re-checking every other `mod.rs` citation in these entries not
explicitly itemized here, since citations in the same functions and
surrounding code were uniformly wrong once checked — e.g. all of OR-1's
remaining `mod.rs` line numbers inside `record_step`'s body, OR-6's
`py_gym.rs`-adjacent `mod.rs` references, and OR-16's `mod.rs:4475` and
the explicitly-pinned `mod.rs:3469-3841 at afa9a071`, which should be
verified at that named commit, not at head). Also correct the inherited
`plan.md` §2.1 and §2.5 citations (`mod.rs:860-862`, `mod.rs:7009-7010`)
so the design frame itself is not propagating the same two errors into
every entry that cites it.

**Scope not affected, confirmed by spot-check:** citations into
`crates/hares-io/src/output/columns.rs`, `writer.rs`, `metrics.rs`,
`crates/hares-fleet/src/aggregation.rs`, `crates/hares-tariff/src/
evaluator.rs` and `billing.rs`, `crates/hares-core/src/checkpoint.rs`,
`crates/hares-core/src/engine.rs`, `crates/hares-core/src/diagnostics.rs`,
and every `crates/hares-python/src/*.rs` citation checked in files 03, 05
and 12 (`py_config.rs:47-550`, `py_dwelling.rs:634-656`,
`py_dwelling.rs:659-673`, `py_dwelling.rs:1545-1551`,
`py_dwelling.rs:1600-1603`, `py_dwelling.rs:1882`, `py_gym.rs:475-479`)
all matched the code at `7b7a6a7d` exactly or within a few lines. The
remaining `dwelling/mod.rs` citations that fall outside the
assembly/`record_step` region — `mod.rs:210` (`DwellingConfig`, real
line 211), `mod.rs:220-223` (the `resample_overrides` field and its doc
comment, exact), `mod.rs:3000` (`run_warmup_converged(0.5, 25)` call,
exact) and its definition at `mod.rs:5360` (exact), `mod.rs:1704`
(tariff-evaluator-adjacent field list, in range), `mod.rs:4475`
(`set_tariff`, exact), `mod.rs:4495` (`billing_summaries()`, exact),
`mod.rs:4935` (`save_checkpoint`, exact), `mod.rs:5005-5168`
(`load_checkpoint`, exact), `mod.rs:5176` (`restore_building_state`,
exact) and `mod.rs:3097-3114` (the equipment provenance preamble, exact)
— also checked out, confirming the drift tracks the specific code the
dependency rows touched rather than a uniform shift of the whole file.

### MINOR: `OR-3`'s `conversions.rs:590` citation for `default_output_path` is off by 5 lines

**Entry:** OR-3 (`ledger/03`:349). Cites
`crates/hares-core/src/dwelling/conversions.rs:590`. The actual
`pub(crate) fn default_output_path` is at `conversions.rs:595`. Small
enough that it is unlikely to mislead, but it is wrong, and this file
(unlike `dwelling/mod.rs`) was not expected to drift since it is not
among the files OR-5/OR-11/PR touch; worth a one-line fix alongside the
major finding above since the same commit will already be touching this
entry's citations.

### MINOR: OR-3's file-destination deletion list should name `resolved_output_path`'s actual deletion site precisely

**Entry:** OR-3 (`ledger/03`:351). Cites
`crates/hares-core/src/engine.rs:411-426` for `resolved_output_path`,
which is accurate (`fn resolved_output_path` is exactly at
`engine.rs:411`). No defect here; noted only because it is the one
`engine.rs` citation checked against a wide list of functions in that
file (`SimulationResults` at 49, `run` at 102, `run_dwelling` at 245,
`compute_metrics_from_batches` at 428, `finalize_run_metrics` at 477,
`empty_metrics` at 522) and all matched or were within a few lines of
the cited ranges — included here for completeness of the audit trail,
not as a finding.

## Design and logic review (OR-1 to OR-17 not committed, file 05, file 12)

No defect found in the substantive design of the reviewed entries beyond
the citation drift above:

- OR-1's catalogue design, bug-compatible column-set preservation, and
  pinned kind table match `plan.md` §2.2's contract; the `EquipmentDescriptor
  .telemetry_fields` and `Actor::output_variables` seams it depends on
  (`crates/hares-types/src/equipment.rs:1868`, `crates/hares-core/src/
  actor.rs:138,477`, `crates/hares-python/src/py_actor.rs:99`) all exist
  at the cited lines and have the cited shapes.
- OR-16's `ReplayBlocker`-producing method list (`add_equipment`,
  `clear_equipment`, `remove_equipment`, `remove_equipment_by_end_use`,
  `replace_equipment`, `set_battery_lut`, `clear_battery_lut`,
  `set_ev_charging_curve_lut`, `clear_ev_charging_curve_lut`, `add_actor`,
  `add_actor_by_name`, `apply_control`, `apply_control_validated`,
  `queue_end_use_control`, `queue_dispatch`, `set_price_signal`,
  `set_grid_voltage`, `record_clamped_actions`, `load_checkpoint`,
  `restore_building_state`, `run_warmup_converged`) was checked
  method-by-method against the current `Dwelling` impl: every method
  exists under the name the entry gives it.
- OR-3's destination/file-publishing design, OR-6's `finish()`/`RunResult`
  lifecycle contract, OR-10's checkpoint section ownership, and file 12's
  `ResolvedModel`/`validate()` split are each internally consistent with
  `plan.md` §2.1-§2.7 and do not duplicate or contradict another entry's
  ownership of the same type.
- No entry reviewed introduces a silent default, a process-global
  fallback, or a shim; several (OR-3, OR-10) explicitly delete existing
  ones (`RotationPolicy`, `default_output_path`,
  `time_res_secs_u32`'s `unwrap_or(3600)`).
- Acceptance tests in the reviewed entries are deterministic: no test
  description asserts on wall-clock timing, RSS, or thread-race-dependent
  behavior (grep over all three files for `RSS`, `elapsed`,
  `Instant::now`, `sleep(` found nothing outside the file-wide rule
  statement that tests must avoid these).
- Verification commands (`cargo nextest run ...`, `cargo deny`, `frame_golden
  compare-all`, `uv run --no-sync pytest ...`) use tools actually
  installed in the worktree's toolchain (`cargo-nextest 0.9.137`,
  `cargo-deny 0.20.2`), and the nextest profile referenced
  (`test-threads = 8`) exists in `.config/nextest.toml`.
- Dependency order against `ledger/00-schedule.md`: OR-1's stated
  dependencies (OR-5, OR-11, M-2) are all committed rows before it in the
  schedule; OR-6's dependencies (OR-3, OR-4, OR-5, OR-11, OR-14) and
  OR-16's (OR-6, `12-inputs.md` IN-1, `05-python-surface.md` PY-6, PY-9,
  `06-runtime-memory.md` RT-4) are consistent with the schedule's row
  ordering (36-43a, 48, 58, 61, 69). No entry in files 05 or 12 was found
  scheduled ahead of a dependency it names.

## Testing gaps

I did not execute the acceptance tests named in these entries (most name
tests and fixtures that do not exist yet, since the entries are
unimplemented — that is expected and not a gap). I did not run the full
verification block (`plan.md` §3) against the worktree; this was a
citation- and design-level audit, not a build/test run, matching the
task's "read, not implement" framing for not-yet-committed entries. The
one thing I could and did verify mechanically — that cited symbols exist
at the cited lines — was checked for every citation in the files named
above; the systemic-drift finding reports every instance found wrong, not
a sample.
