---
id: ACTOR-006
title: Wire IdealHvac + SolverFeedbackActor in Dwelling orchestrator
kind: implement
depends_on:
  - ACTOR-001
  - ACTOR-003
  - ACTOR-004
  - ACTOR-005
files_to_touch:
  - crates/hares-core/src/actors/solver_feedback.rs
  - crates/hares-core/src/actors/mod.rs
  - crates/hares-core/src/dwelling/mod.rs
  - crates/hares-core/src/dwelling/solver_builder.rs
references:
  - crates/hares-equipment/src/hvac/ideal_hvac.rs
  - docs/tickets/ACTOR-INDEX.md
verification:
  - cargo build -p hares-core
  - cargo clippy -p hares-core
---

## Background/Context

Wire IdealHvac equipment and SolverFeedbackActor into the Dwelling. ALL control signals flow through the actor→dispatch pipeline — the dwelling never directly calls `apply_control` on equipment.

The SolverFeedbackActor bridges the physics layer (solver back-solve) to the equipment layer (IdealCapacity signals). It implements the Actor trait and dispatches through the normal channel.

## Work to Do

- [x] Create `crates/hares-core/src/actors/solver_feedback.rs` — SolverFeedbackActor with pre-allocated `pending: Vec<(usize, f64)>` buffer (equipment index, capacity_w)
- [x] Store SolverFeedbackActor as a separate Dwelling field (not in actors Vec) so dwelling can call `collect_and_solve()` with typed access
- [x] In `run_timestep()`: thermal equipment update_control → solver feedback actor collects targets and solves → solver feedback actor resolves → regular actors decide → dispatch all
- [x] Zero per-step allocation: equipment indices stored (not DispatchTarget), dispatch targets pre-computed at init, drain reuses Vec
- [x] Pre-compute equipment execution order and dispatch targets at init time
- [x] Add `add_equipment()` and `refresh_equipment_caches()` methods for post-init equipment modification

## Files Touched

- `crates/hares-core/src/actors/mod.rs`: **New** — Module exporting SolverFeedbackActor
- `crates/hares-core/src/actors/solver_feedback.rs`: **New** — SolverFeedbackActor implementation
- `crates/hares-core/src/lib.rs`: Export actors module
- `crates/hares-core/src/dwelling/mod.rs`: Add solver_feedback_actor, equipment_execution_order, equipment_dispatch_targets fields; add add_equipment(), refresh_equipment_caches() methods; wire into timestep loop

## Timestep Ordering

The timestep loop is restructured to support ideal capacity feedback:

1. **Step 1**: Update environment
2. **Step 1b**: Thermal equipment `update_control()` — determines mode and target temperature
3. **Step 1c**: SolverFeedbackActor collects ideal targets and solves for capacities (stores equipment indices)
4. **Step 1d**: SolverFeedbackActor resolves indices to dispatch targets → regular actors decide
5. **Step 2**: Dispatch queued controls (IdealCapacity signals applied to equipment)
6. **Step 2b**: Apply occupancy gains
7. **Step 3a**: Non-thermal equipment `update_control()` + `step()`
8. **Step 3b**: Thermal equipment `step()` (IdealCapacity already applied)
9. **Step 4**: Envelope/domain resolution

## Verification

- [x] `cargo build -p hares-core` passes
- [x] `cargo clippy -p hares-core` passes (only pre-existing warnings)
- [x] `cargo test -p hares-core` passes
- [x] No direct `apply_control` calls from dwelling to equipment outside dispatcher

## Tests Added

### hares-core (actors/solver_feedback.rs)
- `name_returns_expected_value`
- `decide_produces_correct_dispatch_request`
- `decide_drains_pending_buffer`
- `decide_with_no_pending_produces_nothing`

### hares-core (dwelling/mod.rs)
- `solver_feedback_collect_and_decide_dispatches_ideal_capacity` — collect → decide → verify request
- `solver_feedback_collect_decide_dispatch_full_pipeline` — full pipeline with TestIdealEquipment
- `solver_feedback_multiple_equipment_dispatches_correctly` — multi-zone
- `solver_feedback_skips_equipment_without_ideal_target` — edge case: mixed equipment
- `equipment_execution_order_precomputed_at_init`
- `equipment_dispatch_targets_precomputed_at_init`
- `equipment_caches_sorted_by_stage_rank`

## Implementation Notes

### Performance

- **Equipment indices**: `SolverFeedbackActor` stores `(usize, f64)` tuples internally (zero allocation)
- **Pre-cached dispatch targets**: Owned by the actor, set via `set_dispatch_targets()`. Built once at init via `compute_equipment_dispatch_targets()`
- **Per-step clone cost**: `decide()` clones the cached `DispatchTarget` into each `DispatchRequest`. For `ByName` targets this is a short string clone (1-2 per step for typical dwellings). True zero-copy would require `Arc<str>` in `DispatchTarget` — deferred
- **Pre-computed execution order**: `equipment_execution_order` sorted once at init
- **DRY dispatch**: `dispatch_into` and `dispatch_into_observed` share a single `drain_tiers` implementation

### Equipment Encapsulation

`equipment` is a private field. Access via:
- `dwelling.equipment()` — read-only slice
- `dwelling.add_equipment(eq)` — add + refresh caches
- `dwelling.clear_equipment()` — clear + refresh caches

### Solver Feedback Flow

1. `collect_and_solve()` iterates equipment, calls `ideal_target()` on each, solves via `solve_ideal_capacity_for_target()`, stores `(idx, capacity_w)`
2. `decide()` drains pending buffer, indexes into pre-cached dispatch targets, emits `DispatchRequest`s at `Schedule` priority
3. User actors decide after solver feedback — can override at higher priority tiers

### Thermal Equipment Update Timing

Thermal equipment `update_control()` is called BEFORE the solver feedback phase so that `ideal_target()` returns the correct target temperature based on current thermostat mode. The thermal loop in Step 3b skips `update_control()` since it was already called in Step 1b.