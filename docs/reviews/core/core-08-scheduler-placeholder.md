# Scheduler placeholder resolution and fallback behavior
**Review ID**: core-08
**Category**: core
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-core/src/scheduler.rs` (line 1)
- `crates/hares-core/src/actor_registry.rs` (lines 1-430)
- `crates/hares-core/src/dwelling/mod.rs` (lines 326-444, 712-713, 1475-1523, 2209-2815, 3291-3429)
- `crates/hares-core/src/actors/bms.rs` (lines 95-113)
- `crates/hares-core/src/actors/ev_driver/mod.rs` (lines 370-376)
- `crates/hares-equipment/src/battery/mod.rs` (lines 1095-1111)
- `crates/hares-equipment/src/ev/mod.rs` (lines 782-793)
- `crates/hares-equipment/src/lib.rs` (lines 30-32, 160)
- `crates/hares-types/src/schedule.rs` (lines 585-688)

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: `scheduler.rs` is an empty stub — no scheduler implementation exists
**Severity**: high

**Description**: The module `crates/hares-core/src/scheduler.rs` is declared in `lib.rs:18` and its crate-level doc comment reads "Event scheduler for time-triggered actions," but the file contains only that single doc comment line. There is no scheduler implementation for event-triggered, actor-driven execution within a time step. The concept of a scheduler that maps named actor entries to timed execution slots within a step does not exist in the codebase.

Instead, actor execution is managed by a hard-coded 11-step pipeline in `Dwelling::run_timestep()` (`dwelling/mod.rs:2209-2815`). This pipeline executes a fixed sequence: environment update → control dispatch → occupancy gains → thermal `update_control()` → thermal solver prepare → solver feedback → actor `decide()` → dispatch pass 2 → thermal equipment step → non-thermal equipment step → domain solvers → output. The ordering between these steps is implicit and immutable.

**Code Location**: `crates/hares-core/src/scheduler.rs:1`

**Root Cause**: The `scheduler` module was defined in the module tree as a future extension point but was never implemented. The doc comment suggests it was intended for time-triggered event scheduling, but the actual scheduling is done through the fixed pipeline.

**Impact**: 
1. There is no way to insert new execution phases between existing ones at runtime. All ordering decisions are compile-time decisions in the `run_timestep()` method.
2. The API for registering "schedule entries" with named actor references does not exist, so the review questions about placeholder resolution, optional-vs-required entries, overlapping time intervals, and deterministic last-write-wins for schedule entries are inapplicable to a scheduler level — these concerns apply only at the value-schedule level (`ScheduleSource`) and control-dispatch level (`ControlDispatcher`).
3. Any new simulation component that needs to run between existing phases requires modifying `run_timestep()`, which risks introducing ordering bugs and makes the pipeline difficult to compose.

---

### Finding 2: Missing-actor handling is implicit via `actor_seed()` returning `None`, not via schedule resolution
**Severity**: medium

**Description**: There is no concept of schedule entries that name actors which may or may not exist in a registry. Instead, actor creation is delegated to equipment: each `Equipment` implementation optionally provides an `ActorSeed` via `actor_seed()`. Batteries with `BmsMode::Manual` return `None` (`battery/mod.rs:1101-1104`), as do EVs with `ChargingStrategy::Immediate` (`ev/mod.rs:782-785`). When `None` is returned, `build_actors_from_seeds()` (`dwelling/mod.rs:3291-3429`) simply does not create that actor — a silent skip with no logging, warning, or diagnostic.

This pattern conflates two concepts:
- "This equipment does not need an actor" (correct silent skip — the equipment operates autonomously)
- "This actor is expected but could not be created" (should warn or error)

Because both cases use the same `None` return value, there is no distinction between "intentionally absent" and "unexpectedly missing." If a battery mode is changed from `SelfConsumption` to `Manual`, the actor silently vanishes without any notification.

**Code Location**: 
- `crates/hares-equipment/src/battery/mod.rs:1101-1104` (battery `actor_seed()` returns `None` for Manual mode)
- `crates/hares-equipment/src/ev/mod.rs:782-785` (EV `actor_seed()` returns `None` for Immediate charging)
- `crates/hares-core/src/dwelling/mod.rs:3299-3305` (`build_actors_from_seeds` filters out seeds with `None`)

**Root Cause**: `actor_seed()` returns `Option<ActorSeed>` with no mechanism to distinguish "equipment intentionally has no actor" from "an actor should exist but could not be resolved." The absence of a logging call in the `None` path makes actor absence invisible.

**Impact**: When redesigning equipment configurations (e.g., switching a battery from TOU optimization to manual control), the deletion of the companion actor is silent. This is generally acceptable for the intended use case (equipment mode changes should be intentional), but it could mask configuration errors where an actor-dependent mode is incorrectly specified.

---

### Finding 3: `resolve_equipment_id` silently drops unresolvable equipment targets
**Severity**: medium

**Description**: Both `BatteryManagementActor::resolve_equipment_id()` (`bms.rs:100-106`) and `EvDriverActor::resolve_equipment_id()` (`ev_driver/mod.rs:374-376`) look up the equipment name in the `equipment_id_by_name` map and set `self.equipment_id = equipment_id_by_name.get(name).copied()`. If the name is not found in the map, `equipment_id` is silently set to `None` — no warning, no error, no log.

Downstream, `read_soc()` (`bms.rs:108-113`) uses `self.equipment_id?` to return `None` when the ID is absent. The actor then operates without SOC feedback, effectively running blind. This silent degradation means the actor continues to emit dispatch requests (charge/discharge signals) without knowing the actual battery state, which will be silently ignored by the control dispatcher (the dispatcher's `dispatch_into` matches targets by name or end-use; an unmatched target is a no-op — also silent).

In the normal code path, `build_actors_from_seeds` iterates over the same equipment set used to build `equipment_id_by_name`, so the name should always resolve. However, if the `actor_seed()` returns a target name that differs from the equipment's registered name (e.g., due to a code defect in the battery/EV actor seed logic), the resolution fails silently.

**Code Location**: 
- `crates/hares-core/src/actors/bms.rs:100-106`
- `crates/hares-core/src/actors/ev_driver/mod.rs:374-376`
- `crates/hares-core/src/actors/bms.rs:108-113` (downstream `read_soc` returns `None`)

**Root Cause**: The `resolve_equipment_id` functions use `HashMap::get().copied()` without checking whether the key exists. This is a defensive programming gap — a `tracing::warn!` (or stronger) would surface a near-impossible-in-practice but still conceptually important invariant violation.

**Impact**: Low in practice (the equipment name should always match), but the silent failure mode could mask serious bugs during development. If someone adds a new actor type with a name-mismatch bug, the symptom would be "actor doesn't affect equipment" with no diagnostic.

---

### Finding 4: Actor execution order is registration-order with no explicit "A before B" guarantee
**Severity**: medium

**Description**: Actors execute in a fixed order within `run_timestep()` Step 1f (`dwelling/mod.rs:2341-2363`):
1. `SolverFeedbackActor::decide()` runs first (line 2342), bridging the thermal solver's ideal capacity calculation to HVAC equipment setpoints.
2. All user-added actors run next in registration order (lines 2358-2360): `for actor in &mut self.actors { actor.decide(...); }`
3. Auto-registered actors (battery management, EV driver) are prepended before user-added actors (`dwelling/mod.rs:1512-1514`), so they run before any manually added actors.

There is no explicit ordering constraint expressed in code stating that "actor A must run before actor B." The ordering is:
- **Implicit** for `SolverFeedbackActor`: it always runs first because it's called separately before the actor loop.
- **Registration-order** for all other actors: auto-registered actors run before user-added actors by construction (`self.actors = built_in_actors; self.actors.extend(user_actors)`).
- **No cycle detection or dependency validation**: if two actors have a mutual dependency (e.g., a tariff-responsive BMS actor and a DR compliance actor that modifies tariff signals), the order is determined solely by which was registered first, not by any dependency graph analysis.

For the specific case mentioned in the review instructions — "price signal before dispatch" — the price signal is embedded in `EnvironmentState` (populated at Step 1, `dwelling/mod.rs:2237-2253`) before any actor runs (Step 1f). So actors see the current price signal when making decisions. The ordering constraint is satisfied by the pipeline structure, not by an explicit schedule.

**Code Location**: 
- `crates/hares-core/src/dwelling/mod.rs:2341-2363` (actor execution loop)
- `crates/hares-core/src/dwelling/mod.rs:1512-1514` (built-in actors prepended)
- `crates/hares-core/src/dwelling/mod.rs:712-713` (actor vector declaration with doc comment "Actors execute in registration order")

**Root Cause**: The design relies on the fixed 11-step pipeline to provide ordering guarantees rather than expressing ordering constraints explicitly in a schedule or dependency graph. The doc comment on the `actors` field (line 712) states ordering intent but there is no enforcement mechanism.

**Impact**: Adding a new actor that needs to run between existing auto-registered actors (e.g., a demand response actor that must run after the BMS but before the EV driver) requires understanding the registration order details and may break silently if the registration order changes in a future refactor. The lack of explicit ordering declarations makes the system fragile to changes in auto-registration logic.

---

### Finding 5: `ScheduleSource::TimeWindows` handles overlapping windows correctly via priority ordering
**Severity**: low

**Description**: The `TimeWindows` variant of `ScheduleSource` (`schedule.rs:359-365`) evaluates windows in order and takes the first match ("first match wins"). The doc comment at lines 352-357 explicitly states: "Windows are evaluated in order; **the first matching window wins**. Overlapping windows are permitted -- use ordering to express priority (e.g. place a day-specific override before a broad weekday catch-all)."

The resolution logic at `schedule.rs:670-677` iterates `windows.iter()` and returns the first window whose `contains(weekday, minute_of_day)` returns `true`. This is deterministic and correctly handles both:
- **Overlapping windows**: The first window in the `Vec<TimeWindow>` wins. Since `Vec` ordering is stable, this is deterministic.
- **No match**: If no window matches and `default` is set, the default value is returned. If no default and no match, an error is returned (lines 678-685).

This is a well-designed pattern that closely follows how EnergyPlus schedule files handle day-type priority overrides. The "first match wins" semantic is idiomatic for schedule values.

**Code Location**: `crates/hares-types/src/schedule.rs:352-357` (doc comment), `670-677` (win logic), `678-685` (no-match error)

**Impact**: This is a working-as-intended finding. The design is correct and deterministic. No changes needed.

---

### Finding 6: Control signal dispatch uses deterministic tier-based last-write-wins with cross-pass protection
**Severity**: low (documentation finding)

**Description**: The `ControlDispatcher` (`dwelling/mod.rs:326-444`) implements deterministic conflict resolution for control signals targeting the same equipment. Instead of the schedule-level last-write-wins, it uses a priority-tier queue system:
- Signals are bucketed into `PRIORITY_TIER_COUNT` priority tiers (low→high: Grid, Schedule, UserOverride, Mandate)
- Dispatching drains low-to-high, so the highest-priority tier writes **last** and wins
- Within a tier, the last signal queued wins (FIFO drain, last-in = last-drained = last-applied)
- Cross-pass protection: lower-priority signals arriving in a later dispatch pass that conflict with an already-applied higher-priority signal are skipped (lines 397-443)

This is correctly deterministic. However, this is control-signal-level conflict resolution, not athlete-level schedule entry conflict resolution. The review's questions about "overlapping time intervals for schedule entries" and "last-write-wins resolving order" apply to the scheduler layer, which doesn't exist (see Finding 1).

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:326-444`

**Impact**: Working as intended. The control dispatcher's priority-tier design is robust and tested.

---

### Finding 7: `ColumnRef` schedule sources fail at resolution time for missing/empty schedule data
**Severity**: low

**Description**: For `ScheduleSource::ColumnRef` (`schedule.rs:604-618`), resolution fails with an error if:
1. The `schedule_domain` custom domain is not found in the environment (lines 605-615)
2. The domain payload is `None` 
3. The payload is empty and boundary is `Error` (`resolve_index` at lines 841-858)
4. The column index is out of bounds and boundary is `Error`

If the boundary is `Clamp` or `Wrap`, empty or out-of-bounds indices are handled gracefully. This means an incorrect column index (e.g., referring to column 5 when the schedule has only 4 columns) will either:
- Panic at construction time (if validated)
- Return an error at runtime (if boundary is `Error`)
- Silently return the last/clamped value (if boundary is `Clamp`) — which is a silently incorrect value

The `Shared` variant (`schedule.rs:655-664`) similarly fails with an error only if the data is empty and boundary is `Error`. For `Clamp` or `Wrap`, empty data returns an error unconditionally (line 841-843).

**Code Location**: `crates/hares-types/src/schedule.rs:604-618`, `655-664`, `836-858`

**Root Cause**: `resolve_index` correctly errors on empty data regardless of boundary policy, but for non-empty data with out-of-bounds indices, `Clamp` and `Wrap` silently produce potentially incorrect values.

**Impact**: Low. In practice, schedule column indices are validated at construction time (e.g., `dwelling/mod.rs:205` uses `column_index.get(&format!("{name} Schedule (-)")).copied()` which would be `None` for missing columns, and the occupancy path treats `None` as "no occupancy schedule" rather than proceeding with incorrect data). The `Error` boundary is the default, and construction-time validation should catch invalid indices.

---

### Finding 8: `ActorRegistry::create` returns a clear error for unknown actor types
**Severity**: low (positive finding)

**Description**: When `ActorRegistry::create()` is called with a `Config` whose `actor_type` does not match any registered factory, it returns `Err(HaresError::Control(format!("unknown actor type: {}", config.actor_type)))` (`actor_registry.rs:315-318`). This is clear, explicit, and tested (`actor_registry_create_unknown_type_returns_error` at line 410-422). The error is propagated to the caller, who can choose to abort or log.

**Code Location**: `crates/hares-core/src/actor_registry.rs:315-318`

**Impact**: This is the closest thing to "required actor missing" error handling in the codebase, and it is correctly implemented. The error is clear and actionable.

---

## Summary
- **Total findings**: 8
- **Critical**: 0
- **High**: 1 (empty scheduler stub)
- **Medium**: 3 (silent missing-actor handling, silent ID resolution failure, implicit actor ordering)
- **Low**: 4 (correct overlapping window handling, correct dispatch priority, column/index error handling, correct unknown-type error)

## Recommendations

1. **Implement the scheduler or remove the stub**: The `scheduler.rs` file is dead code. Either implement the event-triggered scheduling API it promises, or remove the module declaration from `lib.rs` and update the crate-level doc comment to avoid implying functionality that doesn't exist.

2. **Add logging for missing actor seeds**: In `build_actors_from_seeds()` (`dwelling/mod.rs:3291-3429`), add a `tracing::debug!` or `tracing::info!` call when equipment returns `None` from `actor_seed()`, recording the equipment name and the reason (e.g., "battery 'MainBattery' is in Manual mode — skipping companion BMS actor"). This would make actor absence visible in logs without being noisy.

3. **Add a diagnostic assertion or warning in `resolve_equipment_id`**: In both `BatteryManagementActor::resolve_equipment_id()` (`bms.rs:105`) and `EvDriverActor::resolve_equipment_id()` (`ev_driver/mod.rs:375`), add a `debug_assert!` or `tracing::warn!` when the equipment name is not found in the map. This invariant should always hold in the normal code path, and a failure indicates a programming error that should be caught immediately.

4. **Consider explicit actor ordering dependencies**: If cross-actor ordering constraints become more complex (e.g., with user-supplied actors that need to run between built-in actors), consider adding an explicit ordering mechanism — either a `run_before`/`run_after` declaration on actors, or a per-step phase registration API. The current registration-order approach is adequate for the current set of built-in actors but does not scale to arbitrary actor compositions.

5. **Add validation for `ColumnRef` indices against schedule dimensions**: Consider adding a construction-time validation step that ensures every `ColumnRef` in the equipment configuration references a column that exists in the schedule data, rather than relying on runtime errors during simulation.

## References / Citations
- `crates/hares-core/src/lib.rs:18` — scheduler module declaration
- `crates/hares-core/src/scheduler.rs:1` — empty scheduler stub
- `crates/hares-core/src/dwelling/mod.rs:2209-2815` — `run_timestep()` 11-step pipeline
- `crates/hares-core/src/dwelling/mod.rs:2341-2363` — actor `decide()` execution loop
- `crates/hares-core/src/dwelling/mod.rs:1475-1523` — `auto_register_actors()`
- `crates/hares-core/src/dwelling/mod.rs:3291-3429` — `build_actors_from_seeds()`
- `crates/hares-core/src/dwelling/mod.rs:326-444` — `ControlDispatcher` priority-tier implementation
- `crates/hares-core/src/actor_registry.rs:312-320` — `ActorRegistry::create()` with unknown-type error
- `crates/hares-core/src/actors/bms.rs:100-113` — `resolve_equipment_id` and `read_soc` with silent `None` handling
- `crates/hares-core/src/actors/ev_driver/mod.rs:374-376` — `resolve_equipment_id` with silent `None` handling
- `crates/hares-equipment/src/battery/mod.rs:1095-1111` — battery `actor_seed()` returns `None` for Manual mode
- `crates/hares-equipment/src/ev/mod.rs:782-793` — EV `actor_seed()` returns `None` for Immediate charging
- `crates/hares-equipment/src/lib.rs:30-32,154-160` — `ActorSeed` enum and `Equipment::actor_seed()` trait method
- `crates/hares-types/src/schedule.rs:352-357` — `TimeWindows` "first match wins" semantics
- `crates/hares-types/src/schedule.rs:585-688` — `ScheduleSource::value_at()` resolution for all variants
- `crates/hares-types/src/schedule.rs:836-858` — `resolve_index()` boundary policy logic
