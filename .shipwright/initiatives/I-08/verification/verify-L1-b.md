# Verification: ledger/03-output-reporting.md and ledger/05-python-surface.md

HEAD ebdb1a17. Read-only. Crate graph checked from manifests: hares-io depends on
hares-types/physics/envelope/control/equipment (no pyo3, no sha2); hares-core depends
on hares-io and hares-tariff (has sha2); hares-tariff depends only on hares-types;
hares-python depends on all plus pyo3. Registry has pyo3 0.29.2, arrow 59.3.0,
pyo3-arrow 0.19.0 (depends on arrow-* 59, pyo3 0.29, numpy 0.29, chrono-tz 0.10,
thiserror 1).

Counts: 2 blockers, 16 majors, 12 minors.

## Blockers

### 1. `Destination::Callback` has no implementing entry and its declared home breaks the crate graph (OR-3, 05 intro)
Evidence: plan.md §2.2 declares `Callback(Py<PyAny>)` inside the hares-io `Destination`
enum. `crates/hares-io/Cargo.toml` has no pyo3. OR-3 lists `Destination::{Memory, Parquet,
Csv, Callback}` but implements only files and memory. 05-python-surface.md line 3 says it
"Implements plan.md §2.2's Callback destination", but no PY entry does. `reports:
Vec<ReportSpec>` goes into `SimulationConfig`, which is serde/TOML (`crates/hares-io/src/config.rs`),
and a `Py<PyAny>` or `Box<dyn ..>` cannot be serialised or deserialised.
Correction: (a) in hares-io, define a narrow `pub trait BatchCallback: Send { fn on_batch(&mut self, batch: &RecordBatch) -> Result<(), CallbackError>; }`.
Keep serialisable destinations (`Memory`, `Parquet`, `Csv`) in `SimulationConfig`. Attach
callbacks at runtime through a separate `Dwelling::attach_callback(report_name, Box<dyn BatchCallback>)`,
so they are never part of serde config. (b) Add a PY entry after OR-3 and OR-6: hares-python
implements `BatchCallback` over `Py<PyAny>` using `Python::attach`. A Python exception becomes
a `HaresError` carrying the chained `PyErr`, and the run fails. Tests:
`callback_receives_every_batch_in_order`, `callback_exception_fails_run_with_cause_chained`,
and `no_callback_no_python_call` (zero-cost when unused). (c) Rewrite plan.md §2.2's enum to match.

### 2. OR-5 cannot make invariants unconditional: every `InvariantChecker::check_*` is a no-op stub in release, and most of `check_invariants()` is unclassified
Evidence: `crates/hares-core/src/invariants.rs:705-800`,
`#[cfg(not(any(test, debug_assertions, feature = "check_invariants")))]`, defines `check_thermal`,
`check_reactive`, `check_electrical`, `check_moisture`, `check_soc`, `check_ev_capacity_degraded`,
`check_temperatures`, `check_fuel_coverage`, `check_fuel_electric_absent`, `check_nan_screen`,
`check_hvac_power_non_negative` and `check_heating_accumulator` as `Ok(())` stubs. Two
consequences follow:
- Moving `soc_bounds` and `ev_capacity_degraded` into the always-on block, as OR-5 says, makes only
  their missing-key branches fire (`mod.rs:7113-7119`, `7141-7162`). The value checks
  (`checker.check_soc`, `checker.check_ev_capacity_degraded`) stay no-ops in release. OR-5's tests
  (the two missing-key tests) would pass while the defect remains.
- `check_invariants()` (`mod.rs:7026-~7310`) also holds `timestep_dt`, tank temperatures,
  electrical balance, reactive balance, fuel-electric-absent, fuel coverage, `thermal_balance`
  (energy conservation), the heating accumulator, HVAC power non-negative and
  `humidity_payload_finite`. OR-5 names only the zone-temperature bounds and the two wiring
  checks. The whole function returns early during warm-up (`mod.rs:7043-7045`). The
  `check_nan_screen` calls at `mod.rs:4298, 5670, 5946` are no-ops in release too.
- OR-5 keeps the gate "only if a measurement shows its cost is worth gating". That contradicts
  dl-i08-architecture.health ("NaN and physics invariant violations always fail the run, in
  every build profile, including warm-up").

Correction: OR-5 must enumerate every check in `check_invariants()` and every `check_nan_screen`
call site, and assign each one a class:
- Physics or conservation, or non-finite: always on, every profile, including warm-up.
- Transient-tolerant finite bounds: warm-up exempt, citing ERM 26.1.
- Wiring: always on, including warm-up.

Delete the release stub impl block (invariants.rs:705-800) and the `cfg` gates on
`InvariantChecker` (lines 11-52) so the real checks compile in every profile. Remove the
measurement-contingent gate; record the cost as a measurement only. Add acceptance tests that
fail today in plain release, one per class: `thermal_balance_violation_fails_in_release`,
`soc_out_of_bounds_fails_in_release` and `electrical_imbalance_fails_in_release`. Each must use a
fault injection that works in release. The existing `set_*_for_test` hooks are
`cfg(debug_assertions)` only, so either use a test-only equipment or actor that publishes bad
values, or add a `#[cfg(any(test, feature = "test-hooks"))]` hook.

## Majors

### 3. OR-1: the actor "output-field declaration" does not exist, and equipment columns do not all come from `TelemetryField`
Evidence: actor columns are discovered at schema build from runtime `actor.telemetry()` HashMap
keys (`crates/hares-core/src/dwelling/mod.rs:741-764`, `Actor::telemetry` at
`crates/hares-core/src/actor.rs:138`). Actors declare nothing, and `py_actor.rs` exposes no
telemetry. The per-equipment electric, gas, SOC and mode columns come from `CoreOutput` and
`CoreCapabilities`, not from `TelemetryField` (record_step `mod.rs:6636-6800`).
`TelemetryField` (`crates/hares-types/src/equipment.rs:1869-1874`) already has `unit: String`.
If it gains `kind` and `aggregation`, those enums must live in hares-types, because hares-types
cannot see hares-io's `catalogue.rs`. No `ActorId` type exists anywhere
(`Source::Actor(ActorId)` is undefined).
Correction:
- Put `Unit`, `VariableKind`, `Aggregation` and `Sampling` in hares-types. Keep `VariableDef` and
  `Catalogue` in hares-io.
- Replace `TelemetryField.unit: String` with `Unit`.
- Add a required `Actor::output_variables(&self) -> &[OutputVariableDecl]` (hares-core) and the
  Python-actor equivalent: a declared list passed at registration, rejected if the actor later
  publishes an undeclared key.
- Derive core-output columns from `CoreCapabilities` through a static
  per-capability `VariableDef` table.
- Define `ActorId` (or use the actor's registered name) and say which.
- State the zone declaration point concretely: zone display names from `zone_display_name`
  together with a static per-zone-template table.

### 4. Types the ledger uses are never defined, and one contract has no entry
- `HealthPolicy`: 08 CN-2 cites "OR-5's HealthPolicy", but OR-5 never defines or builds it.
- `WarmupOutcome` and `MetricsStatus`: `MetricsStatus` overlaps the existing
  `Reliability` enum at `metrics.rs:188`.
- `RunMetrics`: the existing type is `FullSimulationMetrics`.
- `Warning`: the existing warnings are `Vec<String>`, drained by `take_warnings()` for gym at
  `py_gym.rs:475-479`.
- `Provenance`: OR-6 (row 24) assembles it, but it is defined only by PY-6 (row 40).
- `StepOutcome` (OR-6).
- `RunHealth.rejected_control_signals` and `clamped_actions` have no named source. The candidate
  sources are the signal rejection sites at `mod.rs:1102, 1147, 1175, 1273` and
  `hvac_core.rs:332` `clamp_count`.

Correction: OR-5 defines `HealthPolicy` (fields, empty default, escalation semantics, where it
is evaluated), `WarmupOutcome` and `MetricsStatus`, and wires both counters from those named
sites. OR-6 defines `Warning` (it replaces `Vec<String>` and keeps the `take_warnings`
contract), `RunMetrics` (rename of `FullSimulationMetrics` or a wrapper; say which), `StepOutcome`
versus `StepResult`, and a minimal `Provenance` that PY-6 then extends.

### 5. No entry implements reporting `Frequency` or `Selection::Pattern`
Evidence: `Frequency::{Detailed, Timestep, Hourly, Daily, Monthly, RunPeriod, Annual}` appears
only in OR-2's type list. dl-i08-architecture.new_capability says "Built now: pre-run
catalogue and reporting frequency". No entry specifies the per-period reduction by
`VariableDef.aggregation`, the difference between `Detailed` and `Timestep`, or the interval
labelling of a period row. `Pattern(String)` has no matching semantics (glob or regex; hares-io
has no regex dependency). OR-10's Memory truncation on restore ignores partially filled buckets.
Correction: add an entry after OR-2 (before B-11, which adds Mode) that implements period
reduction per `Aggregation`, labels each row at interval start, and checkpoints partial-bucket
accumulator state in OR-10. Define `Pattern` semantics (for example a glob over `VariableDef.name`,
with no new dependency). Tests: `hourly_report_sums_energy_and_means_power` on a fixture with a
sub-hourly timestep, and `restore_mid_hour_matches_uninterrupted_hourly_report`.

### 6. OR-3 leaves output config fields and defaults in place that the design frame removes
Evidence:
- `SimulationConfig.output_path` (`config.rs:83`), `rotation: RotationPolicy` (`config.rs:37,121`),
  the rotating writer (`writer.rs:115-130`, `check_rotation` `writer.rs:370`) and the Python
  `rotation` kwarg (`py_dwelling.rs:2164`) all remain. §2.2 says there is no file rotation.
- `default_output_path` (`crates/hares-core/src/dwelling/conversions.rs:590`, used at
  `mod.rs:2474`) is, per plan.md §2.2, "removed ... see ledger/03", but no 03 entry mentions it.
- No entry specifies the strict "removed key names its replacement" error for
  `write_output`, `retain_batches`, `output_format`, `output_path` and `rotation`.
- M-1's `frame_golden --config <toml>` fixtures use today's keys. After OR-3 they are rejected,
  yet OR-3 claims golden fixtures compare bitwise equal.

Correction: OR-3's change lists every removed field (`write_output`, `retain_batches`,
`output_format`, `output_path`, `rotation`/`RotationPolicy`, and the Python `rotation`,
`write_output`, `output_to_parquet` and `output_path` kwargs, plus `PySimulationConfig` fields),
deletes `default_output_path`, and adds a test per removed key asserting the error names
`reports`. It also migrates the frame_golden fixture configs in the same commit, with a note that
the migration is config-only and the captures are unchanged.

### 7. OR-3 and OR-4 acceptance tests are nondeterministic or ordered after what they need
Evidence: `memory_only_produces_full_schema_no_file` and OR-4's verification assert "no new
file" through `git status --porcelain --ignored` inside a test. nextest runs tests in parallel,
and BR-7's mechanism is a whole-run before/after diff, not a per-test oracle.
`metrics_available_without_any_file_destination` says "see OR-8 for the metrics-path change
this needs", but OR-8 is row 26 and OR-3 is row 21.
Correction: per-test checks run with a tempdir as the only writable output root and assert that
the directory is empty, plus a structural assertion that no file backend was constructed. Keep
the git-status diff only as the verification-block line. Move the metrics test to OR-8, or state
that it passes at OR-3 because Memory retains batches.

### 8. OR-4 contradicts itself and the design frame about what diagnostics become
Evidence: OR-4 says diagnostics become "a `Destination` variant" and also "a diagnostic report
at a name reserved for it". plan.md §2.2's `Destination` enum has no such variant. Diagnostics
columns (`diagnostics::write_header`, `mod.rs:2790-2830`) are not catalogue variables.
Correction: choose one. Recommended: `Selection::Diagnostics`, a reserved preset whose
variables are catalogue entries with `Source::Dwelling`, allowed only with a `Csv` or `Parquet`
destination. Update §2.2 accordingly. The test uses a `ReportSpec` with the verbosity-8 preset
and no diagnostics spec.

### 9. OR-5's health-counter change is mis-specified, and it edits code OR-2 has already replaced
Evidence:
- The counters are per-step, reset every step (`mod.rs:5306-5311`). `RunHealth` needs run
  totals.
- The rollback increment site at `mod.rs:5203-5206` is not among OR-5's listed gates.
- `nan_temperature_count` is incremented at `mod.rs:6276`, immediately before the run returns
  `InvariantViolation`, so a count can never be observed.
- `unresolved_column_count` increments (`mod.rs:6660-6682`) sit inside a
  `cfg(any(debug_assertions, check_invariants))` block and can no longer occur once OR-2's row
  plan exists.
- `resolve_col` (`mod.rs:645-665`) runs at assembly from `build_equipment_column_map`, not per
  step, yet OR-5 moves it "into the always-on block that already runs every step".
- OR-5 is row 23, after OR-2 (row 20), which replaces `build_equipment_column_map` and
  `resolve_col` with `RowPlan`.

Correction: OR-5 keeps a run-cumulative `port_rollbacks` (incremented at 5203 unconditionally,
never reset per step). It deletes `nan_temperature_count` and `unresolved_column_count` and the
three per-step schema columns (`mod.rs:3900-3918`), and records that deletion as a declared schema
change against the OR-1 golden. The `resolve_col` item moves to OR-2: `RowPlan` construction
returns `HaresError::InvariantViolation` for an expected, unresolved variable in every build, and
the `#[should_panic]` test at `mod.rs:15226` is rewritten there.

### 10. The plain-release verification line is red from row 1 to row 23
Evidence: BR-5 (01) folds into OR-5 (row 23). plan.md §3 requires the full block after every
entry and says "No entry may narrow this block". BR-5 notes the three release failures persist
until OR-5.
Correction: either (a) schedule a minimal release fix as BR-5's own commit: make `resolve_col`'s
drift an error and the two missing-key checks unconditional, before row 8; OR-5 later does the
full reclassification. Or (b) add an explicit, dated exemption for that single line, rows 1-22,
to §3. (a) fits the rules.

### 11. OR-8 folds numeric fix B-7 into a structural commit ahead of the consumer baseline, and its metrics design is incomplete
Evidence: B-7 (09) says "folds into OR-8's commit". OR-8 is row 26. dl-i08-architecture.consumer
requires the consumer migration to be gated bitwise against B0 before engine numeric fixes, and
numeric_fixes requires "each in its own commit". The design gap: "MetricsCalculator takes the
catalogue and the row plan". The row plan holds only *requested* variables
(OR-2), so metrics independent of selection need their own producer slots. Today's
`MetricsCalculator::accumulate` is batch-fed (`metrics.rs:641`).
Correction: OR-8 lands bitwise-neutral. Metrics are fed per step from a fixed, always-on metrics
input plan built from the catalogue, independent of `ReportSpec`, with values unchanged for
every existing fixture. Envelope metrics are reported as `None` with a `MetricsStatus` reason
wherever today they are absent. B-7 becomes its own commit at row 62: compute envelope metrics at
every verbosity, re-capture the golden fixtures and record the delta.

### 12. OR-10's metrics snapshot cannot be bitwise with a batch-fed calculator, and its fixtures are unnamed
Evidence: the snapshot must include "rows recorded but not yet flushed". The calculator is fed per
flushed batch (`writer.rs:68`, `metrics.rs:641`), and pending rows sit in the recorder's
`value_bufs`. There is no existing mechanism to force a port rollback. `BillingState`
(`billing.rs:75`) holds `DateTime<Tz>` with no serde derives. The version gate lives in
`DwellingCheckpoint::deserialize_gated` (`checkpoint.rs:154-166`), not in `load_checkpoint`.
Correction: depend on issue 11's per-step metrics feed, so the snapshot is exact at any step.
Name the rollback fixture: a test equipment whose `step()` returns `Err` on configured step
indices. Specify the tz encoding in the tariff snapshot (IANA name plus UTC instant).
`version_8_checkpoint_is_rejected` goes through `deserialize_gated`.

### 13. PY-1: return type and rechunk site are unspecified, and the acceptance test targets the path PY-1 deletes
Evidence:
- PY-1 does not say whether `simulate()`/`results()` still return a polars DataFrame
  (`pl.DataFrame(obj_with___arrow_c_stream__)` works on polars 1.44.2; verified) or a reader
  object.
- It does not say where the rechunk happens (Rust `arrow_select::concat::concat_batches`, or
  polars) or what the opt-out API is called.
- `capsule_export_matches_ipc_export_bitwise` compares against "today's IPC path" in
  `conversions.rs:52-111`. Keeping that path alive only for a test is a shim.
- `capsule_reader_is_one_shot` tests pyo3-arrow behaviour
  (`PyRecordBatchReader::__arrow_c_stream__` raises `PyIOError("Cannot read from closed stream")`,
  `record_batch_reader.rs:179-190`), not HARES behaviour.
- The fleet path (`py_fleet.rs:303`) and `steps_to_polars_df` (`conversions.rs:33-49`) are not
  listed.

Correction:
- Specify `simulate()`/`results()` → `polars.DataFrame` built from a `pyo3_arrow::PyTable`
  (repeatable `__arrow_c_stream__`, avoiding the one-shot hazard) over a Rust-side
  `concat_batches` single batch.
- Add an explicit `results_arrow(rechunk: bool = True)` returning the `PyTable`.
- Delete `record_batches_to_polars_df`, `steps_to_polars_df` and `batches_or_steps_to_polars_df`,
  and migrate `py_fleet.rs:303`.
- Test `capsule_export_matches_golden_bitwise`: the polars frame equals `polars.read_ipc` of the
  M-1 golden capture, column by column on bits.
- Test `results_can_be_exported_twice`.

### 14. Package, config-type and "built now" decisions have no entry anywhere
Evidence:
- `pyproject.toml:6,77`: the package is `ochre_next` and the module is `ochre_next._hares`.
  dl-i08-architecture.package sets distribution `hares-py`, import `hares`, cache
  `~/.cache/hares` and no alias, but no entry renames anything. PY-6 promises
  `hares.build_info()`, and OD-1 (11) registers exceptions under `ochre_next._hares`.
- `PySimulationConfig` (`crates/hares-python/src/py_config.rs:47-65`) is a hand-written mirror of
  `SimulationConfig`, with `to_sim_config` and `from_sim_config`. `build_config`
  (`py_dwelling.rs:2228-2460`) hand-parses kwargs. dl-i08-architecture.config says "one Rust
  config type with one validate() ... Python builds it from typed keyword arguments ... No twin
  config types", yet PY-2 only patches the kwargs conflict.
- No entry exists for `validate()`, `metrics_from_table`, `resample_weather`, the
  machine-readable input schema, or `Source::{Path, Text, Bytes}`, all "built now".

Correction: add entries in 05:
- **PY-7, package rename:** pyproject name, module-name `hares._hares`, directory move, cache
  dir, OD-1 retargeted. Order it before PY-6, and before OD-1 or with OD-1 re-ordered after it.
- **PY-8:** delete `PySimulationConfig`'s mirror fields. Expose `SimulationConfig` as the one
  pyclass wrapping the Rust type, built from typed `#[pyo3(signature=...)]` kwargs, with
  `validate()` shared with TOML.
- **PY-9..:** one entry each for `metrics_from_table`, `resample_weather`, the input schema, and
  `Source` inputs (or cite which 06 entry owns them).

### 15. PY-3 covers too little, and its acceptance test is racy
Evidence: `build_config` reads `&Bound<PyDict>` and must run attached. Only
`Dwelling::from_config` (`py_dwelling.rs:645`) can be detached. Unlisted construction sites that
hold the GIL: `reset_with_seed` (`py_dwelling.rs:1522`) and `DwellingBlueprint.build`
(`py_blueprint.rs:104-114`). A counter thread "advanced during construction" is timing-dependent:
a fast construction yields a false failure.
Correction: detach exactly the `from_config` and `bp.build()` calls in `from_hpxml`,
`reset_with_seed` and `DwellingBlueprint.build`. Leave `initialize` alone (trivial after OR-9).
Deterministic test: route every construction through one `fn construct_detached(py, ..)`. A
`#[cfg(test)]` hook inside the detached closure asserts
`pyo3::ffi::PyGILState_Check() == 0` (available in pyo3-ffi 0.29.2, `cpython/pystate.rs:61`;
the build is not abi3).

### 16. PY-5 invents a reward contract that conflicts with the existing API
Evidence: `VecDwellingGymEnv` requires `reward_fn: Callable[[RewardContext], float]`, per
environment (`python/ochre_next/rl/vec_env.py:47,119`). `RewardContext` holds the step dict and
zone and equipment telemetry (`gym_env.py:88-92`). The Rust path ignores it and uses
`-net_electric_power_kw` (`py_gym.rs:493`, `vec_env.py:226-236`). PY-5 instead adds a batch-level
`reward_fn` to `batch_step_py` that is "called once per batch" and whose "return value is
distributed". That is a second, unspecified signature. Since vec_env always supplies a reward_fn,
the "unset → default" case never occurs there.
Correction: keep `batch_step` reward-free, or return the raw step dict and telemetry per env.
In `vec_env.step`'s Rust branch, build `RewardContext` per env and call `self._reward_fn(ctx)`,
matching the fallback exactly. Test `rust_and_python_paths_give_identical_rewards` uses a
non-default reward_fn and asserts equality per env.

### 17. PY-6 leaves the crate home open, and its acceptance tests are brittle
Evidence: "hares-core (or hares-types, whichever ...)" leaves the design to the implementer.
"a dirty worktree reports dirty: true" requires a test to modify the repo. build.rs output goes
stale unless `rerun-if-changed` covers `.git/HEAD` and the ref. sdist or no-`.git` builds are
unspecified. The digest encoding is unspecified: `VariableDef` has `Cow<'static, str>` and enums,
and hares-io has no sha2.
Correction: put `build.rs` and `BuildInfo` in hares-core (it already has sha2; `Provenance` lives
there). Emit `rerun-if-changed=.git/HEAD` plus the resolved ref file. With no git, write
`commit: None`, never a guess. Compute the digest in hares-core over a documented canonical
encoding (name, unit, kind, aggregation, sampling and nullable, in catalogue order). Tests:
`build_info_commit_matches_env` compares against a commit passed through an env var at build
time in CI, and there is no dirty-tree mutation test. Keep
`catalogue_digest_changes_when_a_column_is_added`.

### 18. OR-2's acceptance test needs production instrumentation, and its scope is misstated
Rated major because the test as written would put a counting shim into `Dwelling`.
Evidence: record_step runs `mod.rs:6606-~7020`, not 6606-6694, with index lookups at
`6825-6972`. The per-step path also does string-keyed `eq.telemetry().get(tk::..)` lookups
(`6753-6791`), which remain until RT-1 (row 43), so "zero string lookups" is false after OR-2.
Correction: scope the claim to the output-column index. Make the step path unable to reach it:
`output_column_index` is removed from `Dwelling`, or held only by the freeze-time builder
(compile-enforced). The acceptance test asserts that `RowPlan` places a known fixture's values at
the same column indices as the golden (bitwise compare), not a lookup count. Include the validity
bitset and the `i64` timestamp from §2.2 in the change.

## Minors

19. OR-1: "Television matches the EV name-prefix check". `is_ev` is a substring
    `contains("ev")` (`columns.rs:1140-1143`), not a prefix. `frame_golden capture
    schema_v0..v8` is not a mode M-1 defines; it needs one `--config` per verbosity. Name the
    all-equipment fixture.
20. OR-5 cites the warm-up guard at `mod.rs:6250`; it is at `6258`. The always-on block starts at
    `6166`, not `6172`.
21. OR-5 `nan_during_warmup_fails_the_run` needs a fault injection that works in release, since
    existing hooks are `cfg(debug_assertions)`. Name it (same mechanism as blocker 2).
22. OR-6 says a stepped run "never calls any of that". Python already exposes `finalize_billing`
    (`py_dwelling.rs:1827`); only the flush is unreachable. Specify that `step()` after
    `finish()` returns a typed error.
23. OR-7 evidence: `writer.rs:379` is `timestamp.to_string()` copying an already formatted
    `&str`. `aggregation.rs:509` parses (`parse_timestamp`) rather than renders; only 636
    renders `Z`.
24. OR-9 says `initial_state` is "never read". It is read at `py_dwelling.rs:662`
    (`|| self.initial_state.is_some()`) and initialised at 652 and 1954. Remove the field and all
    five sites. Replace `initialize_does_not_allocate_checkpoint_json` (it needs instrumentation
    in hares-core that does not exist) with the field deletion (compile-enforced) plus the
    existing initialize tests.
25. PY-1 lists `metrics()` as an IPC round trip. It is not (`py_dwelling.rs:1490-1516` only
    clones batches).
26. PY-2's test uses `output_verbosity`, which OR-2 and OR-3 may remove; use a kwarg that
    survives, such as `master_seed`. `retain_batches` is not in `KNOWN_KWARGS`
    (`py_dwelling.rs:2142-2165`), so "regardless of what the caller asked for" is inaccurate.
    List which kwargs are dwelling-level (`bldg_id`, `initialization_duration`,
    `defaults_path`, `overrides`, `resample_overrides`) and therefore legal next to `config=`.
    Unknown kwargs raise `ValueError` today; align them with the new `TypeError`.
27. PY-4 cites `columns.rs:36-51`; `field_with_unit` is 43-53. Specify `catalogue()`'s return
    type (list of `VariableDef` pyclass, or dict).
28. OR-3: with lazy file creation, check `overwrite: false` against an existing path at freeze,
    not at the first batch. Say what happens to `<path>.partial` on a failed run.
29. OR-10: `load_checkpoint` takes a typed struct, so route the version test through
    `deserialize_gated`. This overlaps issue 12 and is listed for completeness.
30. pyo3-arrow 0.19 pulls `thiserror 1` next to the workspace's `thiserror 2`. Note it in BR-6 or
    PY-1 so `cargo deny` duplicate-version settings do not surprise the implementer. No
    `deny.toml` exists at HEAD.

## Verified correct

- The OR-1 citations below are correct:
  - `extract_unit_from_name` (`columns.rs:27`) and `field_with_unit` (43).
  - `TIMESTAMP_COL` as Utf8 (`columns.rs:56,425`).
  - `equipment_name_to_end_use` (178-245), with a fall-through to `OTHER` for unlisted names.
  - `build_equipment_column_map` (`mod.rs:376`) and `TelemetryField` (`equipment.rs:1870`).
- OR-2: `record_step` starts at `mod.rs:6606`, and the `output_column_index` HashMap lookups
  per step are real.
- OR-3's citations are correct:
  - `write_output`, `output_format` and `retain_batches` at `config.rs:86,90,105`.
  - `write_output=false` skips schema construction (`mod.rs:2476-2480`).
  - `metrics()` raises on empty batches (`py_dwelling.rs:1494-1497`).
  - The two-column fallback frame (`conversions.rs:20-49`).
  - The eager `File::create` (`writer.rs:123`).
- OR-4: the diagnostics writer is gated only on verbosity >= 4 (`mod.rs:2790-2803`).
- OR-5 evidence that is correct:
  - `check_invariants` is called under a cfg gate at `mod.rs:6291`.
  - The `resolve_col` panic is cfg-gated (`mod.rs:658-662`).
  - The observe-gated record and reset sites are at 6623-6634 and 5306-5311.
  - The test locations `equipment_id_regressions.rs:2475,2517` and `mod.rs:15226` are correct.
- OR-6: `simulate()` is at 2849-2877. `flush_and_close` has only one call site in the dwelling
  (2869), and no `Drop` flush exists. `step()` returns `steps.last().unwrap().clone()`
  (2966-2967). `finalize_billing` is idempotent.
- OR-7: `"%+"` per-row formatting (`mod.rs:7009-7010`) and the UTC `Z` rendering
  (`aggregation.rs:636`) are real.
- OR-8: the envelope-metrics availability depends on the presence of columns
  (`metrics.rs:593-604`).
- OR-9: the checkpoint serialisation in `initialize()` (`py_dwelling.rs:661-676`) is dead cost.
- OR-10's citations are correct:
  - `DwellingCheckpoint` (`checkpoint.rs:74-108`) lacks billing, metrics and health.
  - `CHECKPOINT_VERSION = 8` (`checkpoint.rs:22`) and its gate (154-166).
  - `tariff_evaluator` and `billing_summaries` (`mod.rs:1517-1518`).
  - `BillingState` (`billing.rs:75`), `TariffEvaluator` (`evaluator.rs:8`).
  - `metrics_calculator` (`writer.rs:68`), `MetricsCalculator` (`metrics.rs:361`).
  - `save_checkpoint`, `load_checkpoint` and `restore_building_state` (4575, 4645, 4822).
  - Python `load_state` (1541-1545).
  - The snapshot-per-owning-crate plan respects the crate graph (hares-tariff, hares-io and
    hares-core all have serde).
- PY-1:
  - IPC round trip in `conversions.rs:52-111` and batch clones at `py_dwelling.rs:743-746,1484`.
  - pyo3-arrow 0.19.0 requires arrow 59 and pyo3 0.29, and is MIT/Apache.
  - `PyRecordBatchReader::new(Box<dyn RecordBatchReader + Send>)` is one-shot.
  - `FFI_ArrowArrayStream::new(Box<dyn RecordBatchReader + Send>)` exists in arrow-array 59.3.0
    (`ffi_stream.rs:175`).
  - polars 1.44.2 imports C-stream objects.
- PY-2: `config=` silently wins over sibling kwargs (`py_dwelling.rs:2246-2258`), and
  `retain_batches: true` is hardcoded (2408).
- PY-3: `from_hpxml` holds the GIL. `simulate` and `step` use `py.detach` (708, 764).
  `Python::detach` and `Python::attach` exist in pyo3 0.29.2 (`marker.rs:414,562`).
- PY-4: reproduced that polars 1.44.2 drops Arrow field metadata through both `read_ipc` and the
  `DataFrame(table)` constructor.
- PY-5: the Rust batch path hardcodes `reward = -net_electric_power_kw` (`py_gym.rs:493`) while
  the Python fallback calls `reward_fn` (`vec_env.py:279`).
- PY-6: there is no `hares_engine_version`, `__version__` or `build_info` in the tree. The
  consumer hashes the `.so` (`calibration/paths.py:97-109`, at c71c542).
- Schedule dependencies that are respected:
  - OR-1→OR-2→OR-3→OR-4, and OR-6 after OR-3 and OR-5.
  - OR-8 after OR-1, OR-3 and OR-6; OR-10 after OR-8.
  - OR-9 before PY-3; PY-1 and PY-2 after OR-3; PY-4 after OR-1; PY-6 after OR-6.
  - Issue 10 is the one exception.
- Commit isolation holds for OR-4, OR-5 and OR-7 (the declared Time dtype change is re-captured
  and recorded). OR-8/B-7 is the exception (issue 11).
