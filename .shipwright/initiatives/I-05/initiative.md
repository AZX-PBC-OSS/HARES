---
"id": "I-05"
"title": "Fix additional EV issues"
"status": "active"
"kind": "fix"
"description": "What is already fixed, so it isn't re-investigated

  Verified against HARES f1b9e1f8 on bldg0168875 / G4804390, 30 days, 15-minute:

  An EV with Immediate or with no override now charges — 329.8 kWh over 35 of 35
  days, where both previously delivered 0.0 kWh. That was the larger half of the
  original report.

  The override now reaches the engine and validates. A mistyped field raises
  equipment error: equipment 'EV' init failed instead of being silently
  accepted. #[serde(deny_unknown_fields)] on ChargingStrategy is doing its job.

  What still does not work

  The strategy's content does not govern behaviour. Varying only
  charging_strategy, every variant produces the same charging:

  override

  charging energy

  peak hour

  nights

  hourly profile (0→23)

  Nightly 22:00–06:00

  329.9 kWh

  20

  28

  ##+...............######

  Nightly 02:00–06:00

  329.8 kWh

  20

  28

  #++...............######

  LowSoc 0.15

  329.8 kWh

  20

  28

  ##+...............######

  Immediate

  329.8 kWh

  20

  28

  ##+...............######

  no override

  329.8 kWh

  20

  28

  ##+...............######

  Two things make this more than a totals coincidence. Moving the off-peak
  window from 22:00 to 02:00 — a four-hour shift — changes total energy by 0.1
  kWh in 330 and leaves the peak hour and night count identical. And LowSoc is
  indistinguishable from Immediate, despite being a different policy class
  entirely (charge when SOC falls below a threshold, versus charge on plug-in).

  All five profiles concentrate charging at 18:00–23:00 plus 00:00–02:00 —
  charge-on-plug-in from about 18:00, regardless of what was configured.

  This is specific to the HPXML override route

  Dwelling.add_ev_with_driver(VehicleId, EvArchetypeId, seed) differentiates
  properly — we verified that separately and it produces distinct energy,
  charger rating, night counts and start-time concentration per archetype. So
  the driver machinery works; it is the path from an HPXML-declared EV plus a
  charging_strategy override that does not carry the configuration through.

  Reproduce

  Two things will cost you an hour if missed, both of which cost us one:

  The equipment is named EV, not the HPXML SystemIdentifier (CalibEV1 in our
  fixture). An override keyed on the identifier is silently ignored — every run
  then reads as \"no override\", which looks like all strategies agreeing.

  Field names are exact and per-variant: Nightly { off_peak_start_hour,
  off_peak_end_hour, target_soc }, LowSoc { threshold, target_soc }, Immediate {
  target_soc }. Note threshold, not threshold_soc.


  Compare the hourly profiles, not just the totals — two strategies can coincide
  on annual energy while differing in shape, and shape is what a utility cares
  about for managed charging.

  Why it matters\ 

  EV Identification is validated by injecting a known charging behavior and
  scoring detection against it. With the window and threshold discarded we can
  only inject one behavior — charge-on-plug-in — so we cannot test detection
  against off-peak or SOC-triggered charging, which is what a managed-charging
  program actually produces.

  Complete when

  Two Nightly overrides differing only in off_peak_start_hour produce measurably
  different hourly charging profiles, and LowSoc differs from Immediate."
"created_at": "2026-09-14T17:08:49.045Z"
"created_by": "operator"
regression_tests:
  - crates/hares-core/src/actors/ev_driver/mod.rs
  - crates/hares-core/src/actors/ev_driver/departure.rs
  - crates/hares-equipment/src/ev/tests.rs
  - crates/hares-core/src/checkpoint.rs
  - crates/hares-core/src/actors/ev_driver/composer.rs
  - crates/hares-core/src/dwelling/mod.rs
  - crates/hares-equipment/src/battery/mod.rs
---

What is already fixed, so it isn't re-investigated
Verified against HARES f1b9e1f8 on bldg0168875 / G4804390, 30 days, 15-minute:
An EV with Immediate or with no override now charges — 329.8 kWh over 35 of 35 days, where both previously delivered 0.0 kWh. That was the larger half of the original report.
The override now reaches the engine and validates. A mistyped field raises equipment error: equipment 'EV' init failed instead of being silently accepted. #[serde(deny_unknown_fields)] on ChargingStrategy is doing its job.
What still does not work
The strategy's content does not govern behaviour. Varying only charging_strategy, every variant produces the same charging:
override
charging energy
peak hour
nights
hourly profile (0→23)
Nightly 22:00–06:00
329.9 kWh
20
28
##+...............######
Nightly 02:00–06:00
329.8 kWh
20
28
#++...............######
LowSoc 0.15
329.8 kWh
20
28
##+...............######
Immediate
329.8 kWh
20
28
##+...............######
no override
329.8 kWh
20
28
##+...............######
Two things make this more than a totals coincidence. Moving the off-peak window from 22:00 to 02:00 — a four-hour shift — changes total energy by 0.1 kWh in 330 and leaves the peak hour and night count identical. And LowSoc is indistinguishable from Immediate, despite being a different policy class entirely (charge when SOC falls below a threshold, versus charge on plug-in).
All five profiles concentrate charging at 18:00–23:00 plus 00:00–02:00 — charge-on-plug-in from about 18:00, regardless of what was configured.
This is specific to the HPXML override route
Dwelling.add_ev_with_driver(VehicleId, EvArchetypeId, seed) differentiates properly — we verified that separately and it produces distinct energy, charger rating, night counts and start-time concentration per archetype. So the driver machinery works; it is the path from an HPXML-declared EV plus a charging_strategy override that does not carry the configuration through.
Reproduce
Two things will cost you an hour if missed, both of which cost us one:
The equipment is named EV, not the HPXML SystemIdentifier (CalibEV1 in our fixture). An override keyed on the identifier is silently ignored — every run then reads as "no override", which looks like all strategies agreeing.
Field names are exact and per-variant: Nightly { off_peak_start_hour, off_peak_end_hour, target_soc }, LowSoc { threshold, target_soc }, Immediate { target_soc }. Note threshold, not threshold_soc.

Compare the hourly profiles, not just the totals — two strategies can coincide on annual energy while differing in shape, and shape is what a utility cares about for managed charging.
Why it matters 
EV Identification is validated by injecting a known charging behavior and scoring detection against it. With the window and threshold discarded we can only inject one behavior — charge-on-plug-in — so we cannot test detection against off-peak or SOC-triggered charging, which is what a managed-charging program actually produces.
Complete when
Two Nightly overrides differing only in off_peak_start_hour produce measurably different hourly charging profiles, and LowSoc differs from Immediate.

## Implementation Notes

Fixes 1, 2, 3, 5, and 6 of `fix-plan.md` (revision 9) landed as one change, per the plan's staging decision. The four `reproduce` gate tests pass, and two new integration tests on the literal completion bar (`nightly_start_hour_governs_hourly_charging_profile`, `low_soc_gate_delays_charging_beyond_immediate`, both in `crates/hares-core/src/actors/ev_driver/mod.rs`) verify equipment-observed hourly charging energy through the real `EvConfig` → `actor_seed()` → `build_actors_from_seeds` path over multi-day runs.

The plan's `blocked` verdict put the ship/hold question to the operator; the operator's launch of the fix run is the decision to proceed with ticket 132 (`docs/tickets/132-ev-hpxml-element-non-standard-anxiety-band-hardcoded.md`) tracking the separately-diagnosed HPXML-parser/hardcoded-anxiety-band gap.

**Red-team round (v2-fix-a0befa61):** the landed fix introduced a deadlock it itself masked — `V2GExport`/`V2HDischarge`'s soc-floor `Constraint::Override(idle)` short-circuits the composer before the stack's own `SocTarget` is consulted, and under the explicit-hold contract that idle dispatches a latching `PowerSetpoint{0}` whose only release is a fresh target — which the override itself prevents. A vehicle that exported/discharged down to its floor could never recharge by its strategy (pre-fix, the BMS charge-to-full accident was that path's only recovery). Fixed by moving the floor from `constraint()` (a stack-wide override) into `score()` (the preference declines its own discharge vote at/below the floor, letting the stack's charging target govern); the equipment independently enforces the floor on any discharge in `compute_v2g_discharge`/`compute_v2l_discharge` via the `min_soc` the discharge votes carry on their setpoints. Pinned by the red team's three tests (`v2g/v2h_soc_floor_override_does_not_deadlock_stack_target_recovery`, `v2g_vehicle_below_soc_floor_recovers_overnight`) and the re-baselined preference tests (`declines_export_at_soc_floor_even_at_high_price`, `declines_discharge_at_soc_floor_even_with_deficit`).

**Red-team pass 2 (v2-fix-e406634f):** two findings. (1) A duplicated dead guard in `V2GExport::score` — the soc-floor check and its comment had landed twice verbatim (a mechanical editing accident from the previous round's revert/restore verification); the unreachable second block deleted, one guard remains. (2) The equipment's discharge-floor backstop overshot the strategy floor on every floor landing: `compute_v2g_discharge`/`compute_v2l_discharge` budgeted the floor-landing cap in AC watts (`available_kwh / dt_hours`) while `apply_soc_and_thermal` draws the pack down in DC at `1/charging_efficiency` — landing one step's `(1/efficiency − 1)` below the floor, systematically (bounded by one step's deficit × (1/η − 1), ~0.0004 SOC in the failing test, in the violating direction every time, and only observable once the actor-side floor override was removed, since a stale driver belief keeps voting export). Fixed by budgeting the cap in the DC domain the pack draws from — `available_kwh × charging_efficiency / dt_hours` — the same AC↔DC relation the codebase already establishes at `apply_soc_and_thermal` (DC = AC/η) and in the battery's discharge budgeting (AC = DC·η); the charge-direction twin (`taper_limit`, which divides by efficiency) was audited and is already correct. Pinned by `discharge_floor_holds_when_stale_belief_keeps_voting_export` (run red before the fix, green after, unedited).

**Red-team pass 3 (v2-fix-b56a3759):** the hold contract made low-SOC departures reachable, and exposed that a drive exceeding the pack's energy silently vanished: `EvDrive`'s `kwh > available` guard rejected the tail steps (surfaced only as a repeated dwelling warning string), while the actor subtracted the dispatched energy from its belief regardless — the profile reported mobility the pack never delivered (observed: 9.99 kWh dispatched, 7.16 delivered, 19 steps dropped). Pinned by `drive_shortfall_is_accounted_not_silently_dropped` and the re-baselined `soc_clamps_at_boundaries` overdraw section (truncation, exact-zero landing, shortfall accounting and publication).

**Operator-directed driver-response model (same run, later in the round — supersedes the pass-3 departure semantics):** the operator directed that HARES capture and report the human driver's shortfall responses, not just account for the physics. What landed: (1) the range-anxiety override now charges *minimally* to the band (trip + safe reserve) instead of to full; (2) a trip the observed pack cannot cover at the departure minute is **cancelled** — the driver stays home, the vehicle stays plugged in and self-heals overnight, and the cancellation is counted in the new cumulative `drive_cancelled` channel (`drive_shortfall_is_accounted_not_silently_dropped` re-baselined to this contract, its protective invariants preserved); (3) a driver with away charging (`away_charge_fraction > 0`) who arrives short of their usual amount charges out back up to it — the away recoup takes the full gap to usual over the habitual fraction, the away session is bounded by an explicit `SOCTarget{usual}` (cleared on the away disconnect), and the evening's home cycle transfers nothing (pinned by `away_charger_short_of_usual_tops_up_out_to_usual_amount`); (4) new `range_anxiety_active` telemetry channel reports the driver-is-short state (pinned by `range_anxiety_active_reports_short_driver`). The equipment's mid-trip truncation accounting (`DRIVE_SHORTFALL_KWH`) remains as the physical backstop for mid-trip capacity fade. Both actor snapshot (`drive_cancelled`) and equipment checkpoint (v4) persist the new state. Full details and the operator quotes in the initiative decision log.

**Review round (fix run v2-fix-bd428aa6):** the review's one Warning — `EvDriverSnapshot` gained `drive_cancelled` with no version gate (`CHECKPOINT_VERSION` stayed 6, and actors had no per-actor version, so an actor-only snapshot change would surface as a raw postcard decode error inside `load_state` instead of a version mismatch, with the equipment-side `Ev::checkpoint_version()` 3→4 bump masking it only by restore order). Fixed at both levels: `CHECKPOINT_VERSION` bumped 6→7, and the class closed with the actor-side counterpart of the equipment's `checkpoint_version()`/`load_versioned` — a new `Actor::checkpoint_version()` trait method (default 1), `DwellingCheckpoint::actor_states` entries are now `ActorStateCheckpoint { name, schema_version, blob }` records, and `load_checkpoint` validates each blob's version against the live actor before `load_state` runs, erroring with the actor's name and both versions. Because the wrapping and validation live in the dwelling (where actor checkpoints are already assembled and restored), every actor — Bms, Occupant, DrCompliance, IdealThermostat, SolverFeedback, SafetyMonitor, EvDriver, and Python-defined actors — is gated uniformly without each implementing framing itself. The bump alone was not sufficient: `DwellingCheckpoint::load` ran the full schema parse *before* the `format_version` check, so a pre-v7 file would have failed with `checkpoint parse failed: invalid type ...` — the same misleading-failure shape the review flagged, moved from inside `load_state` to the file boundary. The gate now runs first, in one home (`DwellingCheckpoint::deserialize_gated`), and the Python binding's two byte-level restore paths (`load_state`, `restore_building_state`) route through it too. Pinned by `checkpoint_actor_state_round_trips_with_schema_version` (actor state survives the round-trip, observable through the dwelling's public actor telemetry), `checkpoint_actor_schema_version_mismatch_rejected_with_named_error` (a tampered version is rejected with the named error, not a postcard decode failure), and `old_version_checkpoint_rejected_as_version_mismatch_not_parse_error` (a hand-built pre-v7 file with tuple-shaped `actor_states` is rejected by the version gate; verified red under the pre-fix parse-first ordering with exactly the `checkpoint parse failed: invalid type: sequence, expected u32` failure), all in `crates/hares-core` tests; the pre-existing `version_mismatch_rejected` test pins the file-level gate mechanism. `tests/regression/checkpoint_restart.rs` was updated to the new `actor_states` record shape (same assertions, unchanged).

**Red-team pass 5 (fix run v2-fix-3845041d):** one finding — the range-anxiety override's minimal-charge dispatch (`SOCTarget{anxiety band}`) landing on a latched urgent `EvSetReadyBy` silenced charging entirely (the red team's `band_soc_target_must_not_veto_urgent_ready_by_charging`, delivered red). Root cause, single site: `compute_charging_power_kw` computed `target = soc_target.or(ready_by_soc)` — a controller ceiling *replacing* the deadline's own departure target — which poisoned three downstream consults at once: the `soc >= soc_limit` early return (vetoed charging above the band, short of the departure target), `bms_ready_by_power`'s deficit basis (a maximally urgent deadline recomputed its deficit against the band, concluded "plenty of time", and commanded zero), and the taper limit. Fixed by making the effective destination the **max** of the two when both are latched (`crates/hares-equipment/src/ev/mod.rs`, `compute_charging_power_kw`): under the default `ChargingPriority::DeadlineGuarantee` the deadline is the hard contract — the same contract that lets an urgent deadline charge through a latched zero-power hold — so a controller target above the deadline's raises the destination and one below it (the band top-up) cannot veto it. Whenever the override fires, the BMS's pacing to the max target crosses the band en route by the departure hour, so the driver is never stranded below the band; only the charging start time can differ from the override's 1.2× margin preference. The red team's alternative — dispatching a rate alongside the override's target (actor side) — was considered and rejected as the primary fix: the early-return veto fires *before* the DeadlineGuarantee arm could raise a setpoint, so an actor-side change alone cannot close the class. Pinned by the red team's test (now green, unedited) plus a new sibling test `band_soc_target_ceiling_must_not_stop_charging_short_of_urgent_deadline` covering the early-return branch; both verified red under the pre-fix `.or()` shadowing (temporary revert, restored). The prior `urgent_ready_by_deadline_charges_through_latched_hold` contract is unaffected (a hold sets no `soc_target`), and the red team's two air-cover tests (`latched_hold_and_target_survive_equipment_checkpoint_round_trip`, `target_with_zero_rate_dispatches_target_then_zero_setpoint`) remain green.

**Operator-directed broken-window fix (fix run v2-fix-3845041d, same round):** the red team's traced-not-tested item — a trip whose driving steps cross midnight into a day that rolls non-driving stalls mid-route — was verified real **and pre-existing at HEAD** (HEAD's `decide` non-driving-day branch has the identical early return for every non-`HomePluggedIn` phase; this initiative's rounds neither introduced nor worsened it), and the operator directed it fixed in-place rather than ticketed. The class, all rooted in `decide` tying in-flight trip state to the *current* day's `todays_event` instead of the departed trip's own plan: (a) the freeze itself — no `EvDrive` steps, no away charging, no arrival until the next driving day; (b) the trip-completion away recoup using `event.drive_kwh` — the *rolled* day's energy, wrong whenever a roll precedes completion; (c) arrival timed on the current event's `arrival_minute` rather than the departed trip's. Fixed by making the trip phase-carried: `DriverPhase::Driving` gains `plan: DayEvent` and `DriverPhase::Away` becomes `Away { plan }` (`crates/hares-core/src/actors/ev_driver/mod.rs`); the non-driving-day branch now handles only the home phase, and the `Driving`/`Away` arms run on their carried plan regardless of the roll. The `steps_left == 0` early return also gained the `populate_telemetry` call every other exit path already had (a pre-existing one-step telemetry gap in the same function). Because the phase payload changes `EvDriverSnapshot`'s schema, `EvDriverActor::checkpoint_version()` is overridden to 2 — the first real use of the per-actor version gate added earlier this initiative (a v1 blob from a pre-change build is rejected at the checkpoint boundary with a named error instead of a postcard failure). Pinned by `midnight_crossing_trip_continues_into_non_driving_day` (deterministic `event_day_ratio = 0.0`, late departure, 60 one-minute steps across midnight; asserts `EvDrive` continues post-roll, completion into `Away`, deferred away-charge signals firing on the non-driving day, and arrival home at the departed plan's own arrival minute), verified red under the pre-fix freeze (temporary re-insertion via the edit tool: `got: []`, restored).

**Red-team pass 6 (fix run v2-fix-94655832):** one finding — the destination fix's `max(controller, deadline)` applied unconditionally, leaking into `ChargingPriority::ExternalAuthority`, whose contract is the opposite ("external controller bears sole responsibility; BMS deadline logic is not applied"): a latched 0.9 ready-by target silently raised a controller-commanded 0.3 destination, and the pack rode past it (observed soc 0.5448 after 23 h at the commanded 1 kW). Fixed by scoping the destination rule to the priority that defines it (`crates/hares-equipment/src/ev/mod.rs`, `compute_charging_power_kw`): under `DeadlineGuarantee` the destination is `max(controller, deadline)` (unchanged — the band-veto fix); under `ExternalAuthority` the controller's target shadows a deadline target latched earlier in the session, exactly as its setpoint shadows the BMS's pacing; with no controller target set, the deadline's own target is the destination under both. Pinned by the red team's test (green, unedited) — verified red under the unconditional `max()` (temporary revert via the edit tool reproduced the exact `soc 0.544834665735292`, restored) — with `external_authority_respects_setpoint_despite_urgent_deadline` and both `band_soc_target_*` tests confirming each priority's contract unchanged. The red team's air-cover test `snapshot_round_trip_preserves_mid_trip_driving_plan` (mid-trip `Driving` payload round-trip) is green. Its traced-not-tested items each carry a named disposition (explicit `soc_target_max` ceiling outranking a convenience deadline — defensible contract; the override's band top-up subsumed by a latched deadline — documented trade; `record_anxiety_plan_hours` clamp asymmetry — safe direction; degenerate direct-arrival sampling — incoherent schedule no fixture or archetype produces) — no action required beyond the scoped fix.

**Red-team pass 7 (fix run v2-fix-3da2f572):** one finding — a truncated stateful actor blob (empty `blob` with a *matching* `schema_version`, since truncation predates the stamp) restored as silent success: the version gate passes, `load_state`'s empty-blob early return (the "no mutable state" convention) turns it into a no-op, the dwelling reports `Ok`, `restored_count` increments, and a resumed run silently diverges on freshly-constructed decision state. Fixed with the dwelling-side rule the finding named — an empty blob is valid only for an actor that also *saves* empty (`crates/hares-core/src/dwelling/mod.rs`, `load_checkpoint`'s restore loop, beside the version gate): when a checkpoint entry's blob is empty, the live actor's `save_state` is probed; a non-empty save proves the actor carries persistent state, so the empty entry is rejected with an error naming the actor ("truncated or corrupted checkpoint"), and a probe failure is itself a named rejection. Soundness verified by reading every `save_state` impl: all five stateful actors (EvDriver, Bms, Occupant, DrCompliance, IdealThermostat) serialize unconditionally non-empty blobs, and every stateless actor (SolverFeedback, SafetyMonitor, Python wrappers, stubs) uses the trait default that always saves empty — the probe separates the classes in every reachable combination, with no trait-contract change and automatic coverage for out-of-tree actors. Pinned by the red team's test (green, unedited; verified red under the disabled gate via temporary edit — `load_checkpoint` returned `Ok` on the truncated blob — restored) plus a new sibling test `checkpoint_stateless_actor_empty_blob_restores_cleanly` pinning the other side of the rule (the stateless convention must not be over-rejected), which no prior test covered.

### Deviations

- **`resolve()` target selection was extended beyond the plan's `>`→`>=` tie-break (fix-plan W4).** The plan's W4 response claimed the `emit_vote` restructure delivers "SolarSurplus/TouAware charging steps"' configured `target_soc` ceiling to the equipment. As written, that claim was true only for stacks where a target-bearing vote happens to precede the rate winner (`V2H`/`V2G`); in `TouAware`, `PriceOptimizer` (a targetless rate vote) is first in the stack, won the score, and the ceiling never reached the resolved vote — the equipment charged toward `ready_soc` (default 1.0) instead of the configured 0.9. The implemented rule — `target_soc` resolves from the highest-scored *target-bearing* vote, independent of stack order and of targetless rate winners — preserves the composer's pinned "highest score wins among competing targets" contract (`highest_score_wins`) and makes the plan's W4 claim true for every stack. Pinned by `targetless_rate_winner_keeps_ceiling_target` and exercised end-to-end by the TouAware tests.
- **`build_actors_from_seeds` is now `pub(crate)`** (was module-private). Its doc already declared it a "pure function for testability"; the integration tests need the real seed→actor construction path rather than re-deriving the `ActorSeed::Ev` arm's wiring by hand.

### Known Limitations

- **What:** An HPXML-declared EV configured with the ticket's literal `LowSoc { threshold: 0.15 }` remains governed by the range-anxiety override rather than its own gate (fix-plan Fix 4, withdrawn).
- **Constraint:** `build_actors_from_seeds`'s `ActorSeed::Ev` arm hardcodes `daily_drive_miles = 30` and `range_anxiety_miles = 20` (`crates/hares-core/src/dwelling/mod.rs`), putting `anxiety_soc ≈ 0.28` above 0.15; making those configurable from HPXML depends on first resolving the separately-diagnosed defect that HARES's EV parser reads a non-standard HPXML element (`Systems/ElectricVehicles/ElectricVehicle`, absent from the HPXML 4.x/5.x schemas) — a second root-cause investigation in a different subsystem, tracked as `docs/tickets/132-ev-hpxml-element-non-standard-anxiety-band-hardcoded.md`.
- **Tried:** fix-plan revision 7 proposed threading `daily_drive_miles`/`range_anxiety_miles` through `EvConfig`/`ActorSeed::Ev` (Fix 4); withdrawn in revision 8 after the schema check found the parser defect — the config surface would have been built on an un-diagnosed element mapping.
- **Resolvable when:** ticket 132 is resolved (the parser question answered and the driving-behaviour fields threaded from the standard `Vehicles/Vehicle` element).

- **What:** A held EV's battery preconditioning heater can run during a deliberate strategy hold (fix-plan rev-9 W1 decision).
- **Constraint:** The heater's gating condition (`would_charge_underated = !is_v2l_discharge && self.soc < self.effective_soc_limit()`, `crates/hares-equipment/src/ev/mod.rs`) cannot distinguish "charger blocked by cold-derate" (heater must fire to enable charging — pinned by `heater_only_grid_draw_when_fully_cold_derated`) from "charger held by strategy" without a why-is-it-not-charging fact that does not exist in the current `ControlSignal` vocabulary; `DEFAULT_HEATER_POWER_W = 0.0` means no fixture or completion-bar outcome is affected either way.
- **Tried:** gating the heater on the computed `charger_kw > 0` — breaks the verified cold-derate preconditioning feature (`heater_only_grid_draw_when_fully_cold_derated` requires the heater to fire precisely when `charger_kw` is zero from cold).
- **Resolvable when:** the control vocabulary carries a reason-for-no-charging distinction (a BMS preconditioning-scheduling design change), if a heater-equipped configuration ever needs it.

- **What:** The driver's idle holds widen the window during which its `Schedule`-tier `PowerSetpoint` writes can overwrite another actor's setpoint on the same EV (fix-plan W1 decision).
- **Constraint:** The dwelling's cross-actor contract is last-write-wins within a tier with tiers drained low→high (`crates/hares-core/src/dwelling/mod.rs`, `ControlDispatcher`); there is no per-target ownership arbitration anywhere in the workspace, and the one in-repo actor that dispatches `PowerSetpoint` to an EV at another tier (the occupant actor, `UserOverride`) re-sends every active step, so its later, higher-tier write always stands.
- **Tried:** none — the constraint is the workspace-wide dispatcher contract; adding per-target ownership is a new arbitration design no ticket asks for.
- **Resolvable when:** multi-actor ownership arbitration on one equipment target is designed and ticketed.

- **What:** An idle hold does not suppress charging while an urgent BMS ready-by deadline is active (fix-plan O3 observation).
- **Constraint:** `compute_charging_power_kw`'s `DeadlineGuarantee` arm (`requested = bms_power.max(requested)`) is the equipment's documented contract — deadline enforcement deliberately overrides a soft hold so the vehicle reaches its departure SOC.
- **Tried:** none — the constraint is the intended deadline-guarantee behaviour, not a defect.
- **Resolvable when:** never — fundamental to `ChargingPriority::DeadlineGuarantee`.
