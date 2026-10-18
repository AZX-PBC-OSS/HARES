---
"id": "I-06"
"title": "Fix gh issue 26, 27"
"status": "active"
"kind": "fix"
"description": "On a 45-day run, an EV under a Nightly charging strategy charges
  on the first few nights only, then never charges again for the rest of the
  run. The vehicle keeps driving daily (trips are then cancelled for range),
  stays plugged in, and the pack ends the run empty.


  What I ran

  Fixture tests/fixtures/resstock/2025.1/bldg0000007, EV added to the HPXML text
  (75 kWh pack, 11.5 kW charger, Level 2), Nightly strategy override, 45 days at
  900 s:


  nightly = {\"Nightly\": {\"off_peak_start_hour\": 22.0, \"off_peak_end_hour\":
  6.0, \"target_soc\": 0.90}}

  overrides = {\"EV\": {\"charging_strategy\": json.dumps(nightly)}}

  kw = dict(start_time=\"2018-01-01T00:00:00\", duration_s=45*24*3600,
  time_res_s=900,

  \          master_seed=0, output_verbosity=1, write_output=True,
  output_path=str(tmp/\"run\"),

  \          defaults_path=str(HARES/\"defaults\"),

  \          overrides=overrides)

  dw = Dwelling.from_hpxml(hpxml, schedule, weather, **kw)

  dw.initialize()

  df = dw.simulate()

  Same result on the debug and release builds:


  2 charge-days in 45; 71.7 kWh charged total (a 75 kWh pack)

  SOC 1.00 -> 0.072; actor:EvDriver:EV:drive_cancelled reaches 31 by day 45

  sessions only around day 6, and not all inside the window (start hours 10, 16,
  21, 23)

  after that: plugged in, soc_gate_charging_allowed = 1, needed_charge_hours
  around 2, and no charging for the remaining ~38 days

  A second home (injected via the calibrator's injection path, Tarrant County
  home 46363) shows the same shape: 5 charge-days (all sessions starting exactly
  22:00), SOC pinned near 0.001 from day 8, drive_cancelled climbing 1 -> 11 by
  day 20, and needed_charge_hours still reporting ~2 h per night. So the nightly
  window itself works when charging happens; the driver just stops initiating
  sessions, and the range-anxiety backstop never fires despite an effectively
  empty pack.


  Expected: a Nightly strategy keeps topping the pack to target on plugged-in
  nights for the whole horizon, or at minimum the range-anxiety backstop charges
  an empty pack.


  Build: engine_build_id 50f0e3d060f6ed90, from 9f23024b (post-#25), Linux
  x86_64, Python 3.13.


  Worth noting for a test: the existing EV tests run 120 one-minute steps, which
  is not enough horizon to see this - it needs a multi-week run to show up."
"created_at": "2026-09-22T19:47:47.275Z"
"created_by": "operator"
regression_tests:
  - crates/hares-core/tests/equipment_id_regressions.rs
  - tests/python/test_py_equipment_descriptor.py
  - crates/hares-core/tests/pv_override_observation.rs
  - crates/hares-io/tests/equipment_id_assignment.rs
  - crates/hares-core/tests/equipment_identity_boundaries.rs
  - crates/hares-core/src/actors/ev_driver/mod.rs
  - crates/hares-core/tests/interest_filtering_regression.rs
---

On a 45-day run, an EV under a Nightly charging strategy charges on the first few nights only, then never charges again for the rest of the run. The vehicle keeps driving daily (trips are then cancelled for range), stays plugged in, and the pack ends the run empty.

What I ran
Fixture tests/fixtures/resstock/2025.1/bldg0000007, EV added to the HPXML text (75 kWh pack, 11.5 kW charger, Level 2), Nightly strategy override, 45 days at 900 s:

nightly = {"Nightly": {"off_peak_start_hour": 22.0, "off_peak_end_hour": 6.0, "target_soc": 0.90}}
overrides = {"EV": {"charging_strategy": json.dumps(nightly)}}
kw = dict(start_time="2018-01-01T00:00:00", duration_s=45*24*3600, time_res_s=900,
          master_seed=0, output_verbosity=1, write_output=True, output_path=str(tmp/"run"),
          defaults_path=str(HARES/"defaults"),
          overrides=overrides)
dw = Dwelling.from_hpxml(hpxml, schedule, weather, **kw)
dw.initialize()
df = dw.simulate()
Same result on the debug and release builds:

2 charge-days in 45; 71.7 kWh charged total (a 75 kWh pack)
SOC 1.00 -> 0.072; actor:EvDriver:EV:drive_cancelled reaches 31 by day 45
sessions only around day 6, and not all inside the window (start hours 10, 16, 21, 23)
after that: plugged in, soc_gate_charging_allowed = 1, needed_charge_hours around 2, and no charging for the remaining ~38 days
A second home (injected via the calibrator's injection path, Tarrant County home 46363) shows the same shape: 5 charge-days (all sessions starting exactly 22:00), SOC pinned near 0.001 from day 8, drive_cancelled climbing 1 -> 11 by day 20, and needed_charge_hours still reporting ~2 h per night. So the nightly window itself works when charging happens; the driver just stops initiating sessions, and the range-anxiety backstop never fires despite an effectively empty pack.

Expected: a Nightly strategy keeps topping the pack to target on plugged-in nights for the whole horizon, or at minimum the range-anxiety backstop charges an empty pack.

Build: engine_build_id 50f0e3d060f6ed90, from 9f23024b (post-#25), Linux x86_64, Python 3.13.

Worth noting for a test: the existing EV tests run 120 one-minute steps, which is not enough horizon to see this - it needs a multi-week run to show up.

## Implementation Notes

Landed on the working tree (uncommitted, single change) against 9f23024b.

### Root cause (WI-1..3, gh #26/#27)

- `hares-io/src/hpxml/equipment.rs` gains `assign_equipment_ids`, called from
  dwelling assembly only (beside `assign_instance_names`): every spec without
  an explicit id gets the next id from a counter starting at 1, written into
  the config channel every constructor already reads (raw `parameters` and
  the typed payload's top-level JSON object, where `#[serde(flatten)]`-ed
  heat-pump configs expose `common.equipment_id`). Explicit ids are preserved
  and the counter advances past them.
- `hares-equipment/src/config.rs` is the single home for id reads:
  `equipment_id_from_config` (tri-state: absent / present / malformed → typed
  error) and `constructor_equipment_id` (absent or malformed → sentinel 0,
  documented). The six one-sided readers (EV, Battery, Ventilation,
  ProtocolBridge, PV raw-only; Generator typed-only) are aligned onto it; the
  duplicated readers in `hvac/helpers.rs` and `schedule_helpers.rs` are
  deleted. The dehumidifier's init site is preserve-when-absent like its six
  siblings (it used to clobber to 0).
- `equipment_id` is reserved against user overrides: appearing in any of the
  three override sources (`all`, `*`, named) is a typed error at dwelling
  build (conversions.rs, checked on the override delta).
- `Equipment::set_equipment_id` — required trait method, guard-checked via
  the shared `hares_equipment::apply_identity_write`; `delegate_equipment!`
  now forwards it plus the three guard methods (the pre-existing guard-blind
  window). Assembly rejects id 0 and duplicate ids; `add_equipment` and
  `replace_equipment` auto-assign never-reused ids, reject explicit
  collisions, and verify the write landed (postcondition with its own
  diagnostic). A static guard test
  (`hares-equipment/tests/identity_guard.rs`) scans workspace `src` for
  unauthorized call sites.
- **Correction to the fix plan**: it claimed exactly two equipment
  entrances; `Dwelling::replace_equipment` is a third (Python-reachable via
  `py_dwelling.rs`). The identity invariant is enforced at all three, and
  `replace_equipment` also now marks its replacement initialized (a
  pre-existing registration-guard gap in a function this change touched).
- Debug desync invariants at both `equipment_core` snapshot sites.

### Secondary fix (WI-4)

The range-anxiety override now yields in-window to a strategy target at or
above the band: the composer resolves first, the override stands down when
its target covers the band, and path exclusivity is preserved (a step's
charging dispatch comes from exactly one path). Out-of-window, targetless,
and below-band cases unchanged.

### Formerly-tracked defects — fixed in-tree by operator instruction (no deferral)

The fix plan's WI-5 created two tracking initiatives; the operator ordered
them deleted and the defects fixed instead. Both are fixed end to end in
this change:

1. **Checkpoint restore is now identity-keyed** (`DwellingCheckpoint` format
   v8): `equipment_states` entries carry the equipment's name and id, and
   restore matches by name and validates the id — a spec reorder between
   save and restore is a typed error naming both sides, where the positional
   `zip` silently loaded one equipment's state into another. Actor states
   were already name-keyed (audited: no other positional restore exists).
2. **Port slots are rebuilt for post-construction equipment**:
   `refresh_equipment_caches` performs a monotonic (never-dropping) slot
   rebuild while no step has run — coverage the previous table had is
   preserved (the envelope solvers step against env-zone accumulators even
   in a cleared dwelling) and newly declared ports gain accumulators. After
   the first step the table is frozen, and `add_equipment`/`replace_equipment`
   reject equipment whose declarations the frozen table cannot satisfy, plus
   validate declared zones against the environment model (the thermal
   solver's wiring is built from env zones — an unknown zone's contributions
   would be silently dropped).

### Same-class audit beyond the plan

The positional `equipment_column_map`/`actor_column_map` (zipped with the
live equipment/actor vectors in `record_step`) desynced after any mid-run
removal/reorder once rows had been recorded — silently misattributed output
values. `refresh_equipment_caches` now re-derives both maps against the
frozen column index whenever rows exist, so names keep their own columns and
post-recording adds read as missing values, never misattributed ones. All
fixes are pinned by tests proven (by neutering) to fail without them.

A second audit pass (attacking the replace entrance and actor wiring)
found and fixed two more members of the class:

3. **Actor equipment bindings never followed identity changes.** Only the
   auto-seed loop resolved `equipment_id`; actors added via `add_actor`
   never resolved at all, and an actor surviving a `replace_equipment`
   (eviction is name-based and fires only on removal) kept reading the
   evictee's now-absent id — its observation channel decayed to the 0.0
   no-observation sentinel, the original bug's symptom through a new path.
   `Actor::resolve_equipment_id` is now a trait method (default no-op,
   implemented by the EV driver and BMS) called for every actor on every
   identity refresh. Pinned end to end (`actor_rebinds_to_replacement_equipment_after_replace`):
   with the resolve loop neutered the test fails ("last soc = 0"), with it
   the BMS observes the replacement battery's SOC through the output CSV.
4. **Equipment entering mid-run had no `equipment_core` entry until its
   first step completed** — actors read nothing for it on that step, and
   the dwelling's own start-of-step core-entry invariant fired. The
   identity refresh now seeds any missing entry with the equipment's own
current `core_output()` (the truthful value the end-of-step snapshot will
   re-write); the end-of-step retain drops ids that have left.

### Expected shifts (per the plan's protocol, traces recorded in the tests)

- `dwelling::tests` `TestEquipment` stub: explicit id 1 collided with the
  dwelling's assembly-assigned id 1 → the stub now arrives unassigned and is
  auto-assigned (the DER-add path).
- Six actor-level range-anxiety tests used in-window strategies whose
  target covered the band; under the new precedence the override correctly
  stands down, so those arms now use out-of-window `Nightly` strategies (the
  override's designed domain), with the trace written into each test.
- User-visible API: `PyEquipmentDescriptor.id` returns `1..N` instead of `0`
  (release-note item — correct identity surfacing).

### Coverage-triage findings (routed back as defects — both resolved)

1. **Mid-run equipment add with recording active panicked in debug**
   (`resolve_col` demanded columns for equipment the frozen schema never
   allocated — CI's python-test job builds the dev profile, so
   `add_ev` mid-simulation panicked). Fixed: `build_equipment_column_map`
   now applies the `expected` predicate only to equipment the schema knows
   (any `{name} ` column present); schema-unknown equipment get empty maps
   — the documented "missing values, never misattributed ones" contract.
   Pinned in Rust (`midrun_equipment_add_with_recording_gets_empty_map_not_panic`,
   neuter-verified) and by the previously-red Python test
   (`test_add_ev_mid_simulation_attaches_driver_and_ev_soc_moves`, green).
2. **Tolerated equipment-step failure vs `equipment_core` observability.**
   The triage's repro (PV under a surface-less solar override → every PV
   step fails, tolerated → `equipment_core` loses the entry → start-of-step
   invariant panics; release builds: silent permanent unobservability) ran
   against a build that predated this change's identity-refresh seeding
   (the panic's quoted line numbers match the pre-seeding source). On the
   current tree the mechanism is closed by that seeding — every equipment
   entering the vector is seeded into `equipment_core` with its current
   core output, and a tolerated failure retains the last committed entry
   instead of dropping it — verified by execution: a new behavioral Rust
   pin (`pv_override_observation.rs`, neuter-verified: fails without the
   seeding), the previously-red Python test green (5 consecutive runs),
   and the full file green. The start-of-step invariant's message no longer
   misattributes the cause to checkpoint restore alone; it names the
   actual contract (identity-refresh seeding, restore snapshot, retained
   last-committed entries).

### Red-team findings (three, all fixed at the cause)

1. **A malformed explicit `equipment_id` was silently overwritten at
   assembly** — `explicit_equipment_id` classified malformed the same as
   absent, so `equipment_id: -1` / `1.5` / `"3"` in a spec was silently
   replaced by an assigned id, violating the pass's own documented
   contract. Fixed: the classifier is now tri-state
   (`ExplicitEquipmentId::{Absent, Valid, Malformed}`), the pass leaves a
   malformed value untouched (the constructor's reader maps it to the
   unassigned sentinel and assembly validation rejects the build loudly),
   and `null` — how a typed config serializes an unset `Option<u32>` —
   classifies as Absent (a first-cut fix that missed this re-broke every
   typed-spec assignment and was caught by the dwelling-building suite
   before landing). Pinned:
   `assign_equipment_ids_preserves_a_malformed_explicit_id_for_loud_rejection`.
2. **The `in_schema` prefix match false-positived on name-prefix
   collisions** ("Electric" vs "Electric Resistance Water Heater"),
   re-opening the mid-run-add debug panic fix-phase-6 closed. Fixed:
   membership is now exact — the schema's unconditional
   `{name} Electric Power (kW)` column — which cannot be inherited from a
   longer name. The drift check still fires for known equipment (the
   `#[should_panic]` pin was re-derived for the new membership rule).
   Pinned: `midrun_add_whose_name_prefix_matches_another_equipment_does_not_panic`.
3. **The never-reused counter saturated at `u32::MAX` and silently
   re-issued the in-use boundary id.** Fixed: the counter advance is
   checked (an explicit `u32::MAX` — accepted, unique — cannot pin the
   counter on itself), and auto-assignment skips any in-use candidate
   (normally zero iterations; the boundary case hands the add the next
   genuinely free id), with exhaustion a loud typed error naming the
   boundary. Pinned:
   `auto_assignment_never_reissues_an_in_use_id_at_the_u32_max_boundary`.

Red-team items attacked and dropped (composer label staleness, surplus
actor checkpoint entries, raw/typed channel mismatch, nested override
keys) were read and agreed — reasons stated in `red-team-note.md` (no
contract / pre-existing untouched surface / unreachable); no action
required.

### Round-out finding (fix-phase-8): malformed typed-channel `equipment_id` silently dropped non-critical equipment

Routed by the coverage triage
 (`coverage-note-quality.md`, `defect_found`): a malformed explicit
 `equipment_id` in the **typed** payload (e.g. `-1`) built a silently
 incomplete dwelling — `DwellingBlueprint::build()` returned `Ok` with the
 equipment absent. Chain: the assignment pass correctly leaves the
 malformed value untouched; the constructor's reader maps it to the
 unassigned sentinel 0; the init re-read of the same malformed value fails
 `init`; and the assembly's non-critical init-failure skip drops the spec
 with only a warning — so the entrance's unassigned-id rejection, which ran
 after the skip decision, was never reached. The raw channel's equivalent
 failed loudly only because its malformed value never reaches the merged
 typed init config, so `init` succeeded with the sentinel and the
 post-loop rejection caught it — asymmetry, not design.

Fixed at the cause: the entrance-1 unassigned-id rejection now runs in the
 assembly loop **immediately after construction, before `init`** — hence
 before the non-critical skip can swallow it — uniformly for both config
 channels. The error construction has one home
 (`unassigned_equipment_id_error` in `dwelling/mod.rs`), shared by the
 pre-init check (the constructor's config-channel stamp) and the retained
 post-loop check, which now serves its distinct purpose: guarding the init
 re-assignment sites against ever writing the sentinel back. No test was
 edited: the triage's red pin
 `malformed_equipment_id_in_the_typed_payload_fails_the_build_loudly`
 went green on real code (re-verified red on this tree immediately before
 the fix), and the raw-channel pin
 `malformed_explicit_equipment_id_fails_the_build_loudly` stayed green.

The triage's two non-routing residuals: the stale `i06_ev_nightly_diag.rs`
 claims (this file's Known Limitations entry and `decision-log.md`) were
 factually wrong — the file is absent from the tree — and are corrected in
 this phase; the python gate remains recorded-but-not-encoded in
 `flow.yaml`, a CI-policy decision that belongs to the flow owner it was
 recorded for (the Python suite itself was re-run green for this change —
 see below).

Verification: `cargo nextest run --workspace --no-fail-fast` — 6921 run,
 6921 passed, 12 skipped (the triage's 1 red now green);
 `cargo clippy --workspace -- -D warnings` clean;
 `cargo fmt --check --all` clean. Because the rejection is
 Python-reachable (`DwellingBlueprint::build` via `py_blueprint`), the
 python gate was re-run for this change (`uv run maturin develop` +
 `pytest tests/python -m "not slow and not ochre"`) — **1172 passed,
 12 skipped**, matching the fix-phase-7 baseline.

### Red-team round 2 findings (two, both fixed at the cause)

1. **An equipment named "Total" silently claimed the aggregate total
   column.** The exact-membership `in_schema` check matched the schema's
   level-0 aggregate `"Total Electric Power (kW)"` as an equipment named
   `"Total"`'s own column, so `record_step`'s later per-equipment write
   silently replaced the dwelling's headline total. The same collision has
   a second face the red test did not reach: an add before any recorded row
   makes `build_schema` emit the per-equipment column under the aggregate's
   name — a duplicate field name that collapses in the name-keyed column
   index, leaving the advertised aggregate column as the one never written
   (verified red pre-fix by a new test, not inferred). Fixed at the source
   of both faces with one mechanism: a reserved-namespace predicate
   `is_reserved_output_column_name` in `hares-io/output/columns.rs` (single
   home — the `LEVEL_0_COLUMNS` const plus the existing end-use-aggregate
   parser, so it cannot drift from the emission sites); `build_schema`
   filters shadowing names out of the per-equipment name list (one site,
   covering every verbosity level, with a `warn!`); the dwelling's column
   map gates both membership and resolution on it — a schema-unknown
   equipment now claims *nothing*, not even a column that happens to exist
   under its name. Contract: such equipment get no per-equipment columns;
   their values are missing, never misattributed; their energy still
   reaches the totals and end-use aggregates (those are computed from
   port/core outputs, not the column map). Pinned:
   `midrun_added_equipment_cannot_claim_the_aggregate_total_column` (red
   test, green), `prerecord_added_equipment_cannot_shadow_the_aggregate_total_column`
   (new, proven red pre-fix), plus unit pins
   `is_reserved_output_column_name_flags_the_aggregate_namespace`,
   `build_schema_emits_no_per_equipment_columns_for_a_name_shadowing_an_aggregate`,
   and `column_map_claims_nothing_for_an_equipment_name_shadowing_the_aggregate_column`
   (the no-panic face: the aggregate's presence must not be read as
   membership).
2. **The malformed-`equipment_id` rejection misdirected the user.** The
   entrance rejected the unassigned sentinel with "its config channel did
   not deliver one" — a false diagnosis when the channel delivered a
   malformed value (`equipment_id: -1`): the value was named only in a
   `tracing::warn!`, and the user was directed to delete the field instead
   of correcting it. Fixed: the assignment pass's classifier
   (`explicit_equipment_id` / `ExplicitEquipmentId`) is now public and
   carries the received value (`Malformed(Value)`), and the entrance
   rejection re-uses it to diagnose each cause — malformed (names the
   field, the received value, and the expected form), explicit `0` (names
   the sentinel), a valid id the constructor did not apply (names it and
   points at the `Equipment` implementation), and genuinely-absent (the
   original message, now accurate). The unreachable post-loop safety net
   gets its own accurate message (an init re-assignment site wrote the
   sentinel). The stale/duplicated doc text in `hpxml/equipment.rs` the
   red-team flagged as a cosmetic broken window was cleaned in the same
   edit (those exact lines were being modified). Pinned:
   `malformed_equipment_id_rejection_names_the_received_value` (red test,
   green); the existing boundary pins (`unassigned equipment id` wording,
   both channels end-to-end) stay green.

   Verification for this round: `cargo nextest run --workspace
   --no-fail-fast` — 6928 run, 6928 passed, 12 skipped (the red-team's 2
   red now green; 4 new pins added); `cargo clippy --workspace
   --all-targets -- -D warnings` clean; `cargo fmt --check --all` clean;
   the python gate re-run for this change (`uv run maturin develop` +
   `pytest tests/python -m "not slow and not ochre"`) — 1172 passed,
   12 skipped, matching the baseline.

### Review findings (three, all fixed at the cause)

1. **(Critical) The range-anxiety backstop stood down for a strategy that
   was *discharging* the pack.** The WI-4 stand-down rule read only the
   composer's resolved target (`strategy_target >= anxiety_soc`), assuming
   a target at or above the band means "the band is covered". A
   rate-bearing strategy breaks that assumption: TouAware at a peak-price
   step resolves target 0.9 *and* a `-max_discharge` rate in the same
   vote, so the override stood down and the EV discharged a pack already
   below the band — a regression introduced by WI-4's precedence
   inversion (the old early-return ordering never produced the discharge
   vote on a below-band step), silently breaking the ticket's acceptance
   floor ("at minimum the range-anxiety backstop charges an empty
   pack") for TouAware and any rate-bearing strategy pairing a high
   ceiling with a non-charging rate. Fixed: the composer now records the
   resolved *rate* beside the resolved target
   (`last_resolved_power_kw`, set in all three resolution arms), and the
   stand-down predicate requires the resolved plan to actually charge —
   rate absent (a target-only plan charges toward the target at the
   equipment's own rate) or positive; a zero or negative rate means the
   plan holds or discharges, the band is not covered, and the override
   fires. Pinned: the reviewer's
   `range_anxiety_must_fire_when_strategy_target_is_not_backed_by_charging`
   (red test, green) and the existing stand-down pin
   `range_anxiety_stands_down_when_strategy_target_covers_band` (Immediate,
   target-only — stays green); all 201 ev-driver tests green.
2. **(Warning, debug/CI failure + release undercount) `record_step`
   panicked on a mid-run equipment add at verbosity ≥ 5.** The
   `in_schema` gate (this change's own fix for the aggregate-column
   claim) scoped *all* resolution — including the global `HVAC Duct
   Losses (W)` aggregate column — to schema-known equipment, but
   `record_step`'s debug invariant demands that column for **every**
   equipment at v ≥ 5 (it is an accumulator: `row[idx] +=` sums every
   equipment's duct-loss telemetry into one slot). Debug builds panicked
   on any post-recording add at v ≥ 5 (the CI python-test profile);
   release builds silently undercounted the aggregate. Fixed: `duct_losses`
   resolves for any equipment when the schema carries it (demanded at
   v ≥ 5, which the schema's unconditional v5+ emission always
   satisfies) — schema membership is the wrong predicate for a global
   column. Pinned: the reviewer's
   `midrun_equipment_add_with_recording_at_high_verbosity_does_not_panic`
   (red test, green — the v1 twin pin stays green, and the v0/shadow unit
   pins are unaffected: the column is absent below v5).
3. **(Warning, low) Duplicated actor-column-map derivation.** The
   fresh-schema and frozen-schema branches of `refresh_equipment_caches`
   each built the `actor:{name}:{key}` → index map inline — identical
   logic in two places, free to drift. Fixed: one helper
   (`build_actor_column_map`), called from both branches; behavior
   identical (all tests green).
4. **(Found while fixing 3, same class) The frozen branch never
   re-derived `end_use_aggregate_indices`.** `record_step` positionally
   zips that map against the live equipment vector, but only the
   per-equipment and actor maps were re-derived when rows were already
   recorded — so a mid-run remove left every survivor writing its power
   into its departed neighbour's end-use aggregate column (proven red by
   a new test: `end_use_aggregates_match_per_equipment_columns_after_midrun_remove`,
   failing with "Cooking End Use … aggregate = 0.0025, member sum = 0"
   before the fix). The assembly site had the same disease in a milder
   form: it mapped over the full spec list while the zip target is the
   surviving equipment vector, misaligning whenever a non-critical init
   failure drops a spec. Fixed with two shared helpers —
   `equipment_descriptor_specs` (also removing the fresh branch's own
   duplicated spec derivation) and `build_end_use_aggregate_indices` —
   called from assembly, the fresh branch, and the frozen branch; every
   site now derives the map from the same vector record_step zips it
   against.

The review's two acknowledged limitations (warm-climate second fixture;
process-wide `RangeViolationEvent` id scope) and its two minor
observations (`replace_equipment`'s one-step stale core output on a
contrived explicit-id reuse; `apply_overrides` invoked only with the
empty overrides object) were read and conceded/noted — no action, per
the review's own disposition.

Verification for this round: `cargo nextest run --workspace
--no-fail-fast` — 6932 run, 6932 passed, 12 skipped (the review's 2 red
now green; the 4th finding's new pin green); `cargo clippy --workspace
--all-targets -- -D warnings` clean; `cargo fmt --check --all` clean; the python gate re-run
(`maturin develop` + `pytest -m "not slow and not ochre"`) — 1172
passed, 12 skipped, matching the baseline. (One unrelated
envelope allocation-counting flake —
`radiant_gain_weight_distribution_zero_allocations`, documented in the
coverage triage — failed once under full-suite parallel memory pressure
and passed both in isolation and in the complete re-run.)

### Red-team round 4 findings (three, all fixed at the cause)

1. **(Critical) `Dwelling::replace_equipment` admitted a replacement
   whose name collided with a surviving equipment.** The entrance
   validated ids (collision rejection, sentinel auto-assignment,
   landed-write verification) but never the replacement's *name* against
   the surviving vector: replacing "A" with a replacement named after
   surviving "B" returned `Ok` and pushed a second "B" —
   `equipment_id_by_name` collapsed (a last-wins `collect()`), every
   name-keyed snapshot lookup misbound, debug builds hit the id-drift
   assert with a message pointing at a mutator that did not exist, and
   release builds wrote one equipment's core output under the other's id:
   the I-06 observation-misbinding class, through a public,
   Python-reachable entrance. Fixed: `replace_equipment` now mirrors
   `add_equipment`'s duplicate-name rejection with the evictee exempt
   (replace-in-kind keeps the replaced equipment's own name), checked
   before identity assignment so a rejected name never enters the vector;
   the doc comment states the name contract beside the identity contract.
   Pinned by the red team's pair — the red test
   `replace_equipment_rejects_a_name_colliding_with_a_surviving_equipment`
   (green: typed `Err` naming the collision, vector unchanged) and the
   exempt boundary
   `replace_equipment_with_the_same_name_keeps_the_identity_contract`
   (unchanged green, so the fix cannot over-reject renames).
2. **(Warning) The WI-4 stand-down ignored the resolved plan's `max_soc`
   cap.** The precedence condition read only the resolved target and
   rate: a resolution of {target 0.9, rate `None`, `max_soc: Some(0.1)`}
   stood the override down although the pack can only charge to 0.1 —
   below the band — the same "pack held away from the band" geometry the
   rate condition was written to catch, in the dimension it did not read
   (the fix round's own test
   `range_anxiety_fires_when_strategy_target_caps_below_the_band_via_max_soc`
   was red on the tree, waiting for the production condition to learn
   it). Fixed: the composer now records the folded most-restrictive cap
   beside the resolved target and rate (`last_resolved_max_soc`, set in
   every resolution arm), and the stand-down requires the plan's
   effective ceiling — the cap, defaulting to the target itself when the
   plan carries none — to sit at or above the band. Pinned: the red test
   (green) plus a new stand-down boundary pin,
   `range_anxiety_stands_down_when_the_resolved_cap_still_covers_the_band`
   (a 0.5 cap over a 0.9 target on a ~0.28 band must defer to the
   strategy and dispatch the composer's own capped plan — the clause
   cannot over-fire); all existing stand-down/rate pins stay green.
3. **(Warning, CI blocker) The telemetry guard's hand-rolled scanner
   false-positived on the EV driver's test module.**
   `no_string_telemetry_reads_in_core` sanitized source text and
   brace-balanced `mod tests` spans by hand; its apostrophe arm treated
   every `'` as a char-literal opener, so the new `FixedVotePref` test's
   `&'static str` swallowed text to the next apostrophe, tipped the
   file's brace balance, and the test-module span never resolved — the
   guard reported test helpers as production violations. Fixed at the
   mechanism, per the red team's direction: the scanner is replaced
   wholesale with the sibling guards' `syn` pattern
   (`hares-equipment/tests/*_guard.rs`) — each file is parsed, test code
   is exempted structurally (modules named `tests`, any
   `#[cfg(test)]`-family-gated item, including impl and trait items),
   and banned reads are detected by AST shape (a `.get` whose receiver
   is a telemetry-channel identifier or a tuple-field `.0`), so
   lifetimes, char literals, and formatting cannot perturb either
   exclusion or detection. The `// allowed:` per-site contract mechanism
   is preserved (same line, next line, or leading comments — the
   positions the existing contracts occupy). Pinned by a new vacuity
   test, `telemetry_read_guard_flags_and_excludes_by_syntax`, covering
   every arm — including the exact `&'static str`-in-`mod tests`
   construct that broke the old scanner.

   **Surfaced by the repaired guard (the old scanner's silent
   false-negative face): five pre-existing production telemetry reads
   carried no `// allowed:` contract** — the EV driver's
   `observed_charge_kw` away arm and `observed_pack_kwh` capacity read
   (both flagged by the guard's first accurate run) and
   `dr_compliance.rs::should_reject_dispatch` (the name-channel lookup,
   the defrost check, and the water-heater tank-temperature check —
   identified by applying the same detector rule set in the pre-run
   audit, then proven load-bearing: with the contracts stripped, the
   guard flags all three, at `dr_compliance.rs:454`, `:457`, `:463`).
   Each was verified against the typed surface before contracting rather
   than allowlisted blindly: `CoreState` has no defrost, tank-temperature, or capacity
   field to migrate to; away charge power is telemetry-only *by design*
   (while `AwayPluggedIn` the equipment zeroes its residential
   `active_power_kw` — the off-site charger must not enter the dwelling's
   electrical balance — so `core_output.flows.electric_kw` cannot carry
   it); and `equipment_telemetry` is the only name-keyed channel (the
   dispatch target is a name; `equipment_core` is id-keyed). All five
   now carry explicit `// allowed:` contracts naming their constraint —
   the guard's designed mechanism, the same contract shape the dwelling's
   own invariant-checker reads operate under. Recorded in
   `decision-log.md` (fix-phase-9).

Verification for this round: `cargo nextest run --workspace
--no-fail-fast` — 6940 run, 6940 passed, 12 skipped (the red team's 3
red now green; 2 new pins added: the guard vacuity test and the
cap stand-down boundary); `cargo clippy --workspace --all-targets
-- -D warnings` clean; `cargo fmt --check --all` clean; the python
gate re-run for this change (`uv run maturin develop` + `pytest -m
"not slow and not ochre"`) — 1172 passed, 12 skipped, matching the
baseline.

### Fix-done interrogation round (class coverage of the review's Finding 2)

The done-signal interrogation ("name where you looked for the same mistake
next door") found the review's Finding 2 had been fixed for its instance
only: the `v >= 5` duct-losses demand (a **global** aggregate column, fixed
by resolving it for any equipment) — but `record_step`'s six **type-gated
v≥7 per-equipment** demands (`defrost_state`, `er_power`, `shr`,
`latent_gains`, `fan_power`, `runtime_fraction`, at
`dwelling/mod.rs`'s invariant block) still fired on the name predicate
alone. A mid-run add whose name matches an HVAC predicate (real HVAC
equipment, or any name containing e.g. "baseboard"/"heater") is
schema-unknown after the schema freezes, resolves those columns to `None`
by design ("missing values, never misattributed ones"), and panicked the
debug invariant — the same defect, one predicate set over. Proven red by a
new test (`midrun_add_of_hvac_named_equipment_at_high_verbosity_does_not_panic`,
panicking at the `cols.fan_power` assert) before the fix. Fixed at the
cause: the invariant's predicate conflated "equipment type emits this
column at this verbosity" with "the schema emitted this column for this
equipment" — the membership rule now has a single home
(`is_schema_known_equipment`, extracted from the column-map builder's
inline computation, called from both the builder and the invariant block),
and the type-gated v≥7 demands (observe counter and debug asserts alike)
apply only to schema-known equipment. The demand for schema-known
equipment is unchanged — the existing
`build_equipment_column_map_still_panics_on_schema_drift_for_known_equipment`
pin stays green — and the global duct-losses demand stays ungated (that
column resolves for any equipment).

Verification for this round: `cargo nextest run --workspace
--no-fail-fast` — 6943 run, 6943 passed, 12 skipped (the new pin green,
both reproduction gates green in the same file); `cargo clippy
--workspace --all-targets -- -D warnings` clean; `cargo fmt --check --all`
clean.

### Red-team round 5 finding (evictee exemption leaks into the add path)

The shared identity collision check (`assign_equipment_identity`,
`dwelling/mod.rs`) exempted incumbents by name comparison against
`replace_name.unwrap_or("")` — correct at `replace_equipment` (names are
unique, so it exempts exactly the evictee, whose id leaves with the swap)
but on the add path (`replace_name: None`) the fallback exempted any
incumbent named `""`: an explicit id held by an empty-named incumbent
passed the check, a second equipment joined the vector on that id, and
every `EquipmentId`-keyed map collapsed — the I-06 misbinding through a
public entrance, with the drift assert unable to fire (the name→id map and
the descriptors agree under the collapse). Proven red by the red team's
`add_equipment_rejects_an_explicit_id_held_by_an_empty_named_incumbent`
(`add_equipment_empty_name_exemption.rs`).

Fixed at the origin — the collision predicate itself:
`replace_name.is_none_or(|evictee| name != evictee)`. The add path exempts
nobody (every incumbent holding the requested id collides); the replace
path exempts exactly the evictee. **Correction to the red team's suggested
expression, recorded for the next reader**: their note's literal
`replace_name.is_some_and(|evictee| name != evictee)` is inverted — as
written it exempts *every* incumbent on the add path, disabling the
add-path collision check entirely. Applied verbatim first, it left the
red test red (the add returned `Ok` for the colliding id), which is the
pin doing its job; the note's stated *intent* ("the exemption must exempt
nobody" on the add path) is what the landed `is_none_or` form implements.

Sibling audit: the other evictee-exemption site — `replace_equipment`'s
name-collision check — keys its exemption on vector position (`i != pos`),
not name, so it cannot leak the same way; `add_equipment`'s duplicate-name
rejection holds for the empty name too, so at most one `""`-named
equipment can exist and the fixed predicate is exhaustive. Companion pins
all green: `add_equipment_rejects_explicit_id_collision_with_existing_equipment`
(normal-named incumbent — the pin that would have caught the inverted
first attempt), `replace_equipment_rejects_collision_with_survivors_but_accepts_the_evictees_id`,
`replace_equipment_with_the_same_name_keeps_the_identity_contract`, and
the `equipment_identity_boundaries` set (7/7). The red team's air-cover
pin `assign_equipment_ids_is_idempotent_on_repeated_invocation` is green
on this tree unchanged.

Verification for this round: `cargo nextest run --workspace
--no-fail-fast` — 6945 run, 6945 passed, 12 skipped; `cargo clippy
--workspace --all-targets -- -D warnings` clean; `cargo fmt --check --all`
clean.

### Verification

- `cargo nextest run --workspace` — 6940 passed, 0 failed (12 pre-existing
  documented skips, unchanged).
- **Python suite via CI's own flow** (`uv run maturin develop` +
  `pytest -m "not slow and not ochre"`) — **1172 passed, 12 skipped**;
  both frontmatter failures green. The earlier phases' Rust-only gate set
  was blind to `tests/python/`; this gap is why the two defects above
  escaped — the triage's recommendation that fix work beneath `hares-python`
  run this gate is recorded here for the flow owner.
- `cargo clippy --workspace --tests -- -D warnings` 0 errors;
  `cargo fmt --check --all` clean; `cargo check --release --workspace`
  0 errors; `cargo test --doc --workspace` green.

### Known Limitations

- **What (resolved):** The superseded diagnose-phase harness
  `crates/hares-core/tests/i06_ev_nightly_diag.rs` duplicated
  `equipment_id_regressions.rs` and its names were ticket ids, not
  behaviours. **Constraint:** this session's permission layer denied every
  file-deletion command outright (`rm` refused; the write tool cannot delete
  files), so only the operator could remove it. **Tried:** `rm` (denied by
  the permission layer); flagged here instead. **Resolution:** the file has
  been removed from the tree (its distinct-id probe was long since absorbed
  into `equipment_id_regressions.rs`); earlier revisions of this entry and
  `decision-log.md` stale-claimed it was still pending operator removal —
  corrected in fix-phase-8.
  (The withdrawn I-07/I-08 initiative stubs were likewise removed by the
  operator on 2026-09-23 — resolved.)
- **What:** The behavioral-breadth Nightly test uses a warm-climate second
  fixture (bldg0000006), not an arbitrary one.
  **Constraint:** the EV model's cold-pack charge derate
  (`linear_temp_derate` below `min_charge_temp_c`) legitimately blocks
  charging through a cold winter — verified on bldg0000002 (January mean
  −3.4 °C) where the pack pins at its floor with zero charging; that is the
  thermal model working, not an identity failure.
  **Tried:** bldg0000002 as the second fixture (failed for exactly this
  reason; the constraint is named in the test).
  **Resolvable when:** never — fundamental model behaviour; the identity
  mechanism itself is proven fixture-independent by the structural gates.
