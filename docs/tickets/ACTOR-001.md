---
id: ACTOR-001
title: Add IdealCapacity control signal and Equipment::ideal_target()
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-types/src/control_signal.rs
  - crates/hares-equipment/src/lib.rs
  - crates/hares-control/src/dispatch.rs
  - crates/hares-control/src/lib.rs
  - crates/hares-core/src/dwelling/mod.rs
references:
  - docs/tickets/ACTOR-INDEX.md
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace
---

## Background/Context

The thermal solver currently has ideal HVAC back-calculation baked into `resolve_internal()`. We are extracting this into equipment. Equipment needs a way to (a) tell the dwelling what ideal capacity it wants, and (b) receive the solver's answer. This ticket adds the type-level infrastructure for that communication.

## Work to Do

- [x] Add `IdealCapacity { capacity_w: f64 }` variant to `ControlSignal` enum in `crates/hares-types/src/control_signal.rs`
- [x] Add `IDEAL_CAPACITY` flag to `ControlCapabilities` bitflags in `crates/hares-types/src/control_signal.rs` (next available bit after existing flags)
- [x] Add `required_capability()` match arm for `IdealCapacity` returning `IDEAL_CAPACITY`
- [x] Add default method `fn ideal_target(&self) -> Option<(ZoneId, f64)> { None }` to `Equipment` trait in `crates/hares-equipment/src/lib.rs`
- [x] Add `PriorityTier` enum to `crates/hares-control/src/dispatch.rs`:
  ```rust
  pub const PRIORITY_TIER_COUNT: usize = 4;
  
  #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
  #[repr(u8)]
  pub enum PriorityTier {
      #[default]
      Schedule = 0,     // Default setpoints, internal schedules
      UserOverride = 1, // Manual thermostat changes, occupant actions
      Grid = 2,         // DR signals, price response
      Safety = 3,       // Emergency shutoffs, equipment protection
  }
  
  impl PriorityTier {
      pub const fn index(self) -> usize { self as usize }
  }
  ```
- [x] Add `priority: PriorityTier` field to `DispatchRequest`
- [x] Export `PriorityTier` and `PRIORITY_TIER_COUNT` from `crates/hares-control/src/lib.rs`
- [x] Update `ControlDispatcher` in `crates/hares-core/src/dwelling/mod.rs` to use tiered queues:
  - Replace single `VecDeque<DispatchRequest>` with `[VecDeque<DispatchRequest>; PRIORITY_TIER_COUNT]`
  - Use `std::array::from_fn` for default initialization
  - `queue()` uses `request.priority.index()` for explicit indexing
  - `dispatch_into()` drains queues in order (Schedule → UserOverride → Grid → Safety) - O(1) insertion, no sorting/allocation
  - Extract `apply_to_matching()` helper to eliminate duplicated dispatch target handling logic

## Files to Touch

- `crates/hares-types/src/control_signal.rs`: Add `IdealCapacity` variant, `IDEAL_CAPACITY` bitflag, `required_capability()` match arm
- `crates/hares-equipment/src/lib.rs`: Add `ideal_target()` default method to Equipment trait
- `crates/hares-control/src/dispatch.rs`: Add `PRIORITY_TIER_COUNT` constant, `PriorityTier` enum with `#[repr(u8)]` and `index()` method, add `priority` field to `DispatchRequest`
- `crates/hares-control/src/lib.rs`: Export `PriorityTier` and `PRIORITY_TIER_COUNT`
- `crates/hares-core/src/dwelling/mod.rs`: Update `ControlDispatcher` with tiered queues, add `apply_to_matching()` helper, fix duplicated docstring on `apply_occupancy_gains()`

## Measures of Success

- [x] `ControlSignal::IdealCapacity { capacity_w: 1000.0 }` compiles and can be matched
- [x] `ControlCapabilities::IDEAL_CAPACITY` is a valid bitflag
- [x] `IdealCapacity` signal requires `IDEAL_CAPACITY` capability (verified by `required_capability()`)
- [x] Existing equipment implementations compile without changes (default method returns `None`)
- [x] Existing equipment rejects `IdealCapacity` signal with `UnsupportedSignal` error (capability check fails)
- [x] `PriorityTier::Safety > Grid > UserOverride > Schedule` ordering works correctly
- [x] `PriorityTier` derives `Default` with `Schedule` as default variant
- [x] `PriorityTier::index()` returns correct values (0-3)
- [x] `PRIORITY_TIER_COUNT` matches variant count (validated by test)
- [x] `ControlDispatcher` routes signals to correct tier queues (O(1) insertion)
- [x] Higher priority signals win over lower priority when targeting same equipment
- [x] Warnings generated for missing targets and failed control applications
- [x] All tests pass with no regressions

## Tests Added

### hares-types
- `ideal_capacity_variant_constructs_and_matches`
- `ideal_capacity_capability_flag_is_valid`
- `ideal_capacity_signal_requires_ideal_capacity_capability`
- `ideal_capacity_signal_rejected_without_capability`
- `ideal_capacity_signal_accepted_with_capability`

### hares-control
- `priority_tier_ordering`
- `priority_tier_count_matches_variants`
- `priority_tier_index_returns_correct_value`

### hares-equipment
- `equipment_ideal_target_default_returns_none`
- `equipment_without_ideal_capacity_capability_rejects_ideal_capacity_signal`

### hares-core
- `control_dispatcher_routes_by_tier_schedule_applied_first`
- `control_dispatcher_drains_all_tiers_in_order`
- `control_dispatcher_higher_priority_wins_over_lower`
- `control_dispatcher_safety_priority_wins_over_all`
- `control_dispatcher_warns_on_missing_target_by_name`
- `control_dispatcher_warns_on_missing_target_by_end_use`
- `control_dispatcher_warns_on_unsupported_signal`

## Verification

- [x] `cargo build --workspace` passes
- [x] `cargo test --workspace` passes
- [x] `cargo clippy --workspace` passes (only pre-existing warnings in untouched files)

## Post-Review Fixes (2026-03-25)

- **`Arc<str>` for `DispatchTarget::ByName`**: Changed `DispatchTarget::ByName(String)` to `ByName(Arc<str>)`. All `.clone()` calls in the dispatch hot loop are now pointer copies (atomic refcount bump) instead of heap allocations. Custom serde impls serialize as plain strings.
- **Zero-alloc conflict detection**: `DispatchTarget::conflicts_with(&Self) -> bool` for reference comparison. `ControlDispatcher` uses `drain_tiers()` shared by both `dispatch_into` and `dispatch_into_observed` (DRY). Conflict detection tracks `(target, tier_index)` and only logs when a strictly higher tier overwrites.
- **Removed `IdealCapacitySolver` legacy trait**: The old `IdealCapacitySolver` trait, `update_mode_and_duty_with_ideal_solver`, blanket impl, and `FixedIdealSolver` test helper were dead code superseded by `SolverFeedbackActor` + `Equipment::ideal_target()`. Removed from `hvac_core.rs` and `hvac/mod.rs`.
- **Added `EndUse::as_str()`**: Added explicit string representations for EndUse variants (see hares-types/src/equipment.rs).
- **Fixed conflict log message**: Changed from ambiguous "lower priority signal" to "higher priority signal overwriting earlier signal for same equipment" — tiers iterate low→high so the second signal seen is always higher priority.
- **Documented apply_control overwrite contract**: `ControlDispatcher` doc now states equipment `apply_control` must be overwrite-safe (idempotent set, not accumulate) since all tier signals fire and last write wins.

## Extensibility Design (2026-03-25)

Refactored `EndUse` from a closed enum to an extensible string-based type:

- **Before**: `EndUse` was a hardcoded enum with 13 variants. Adding new end uses required modifying the core hares-types crate.
- **After**: `EndUse` is a newtype struct wrapping `Cow<'static, str>` with predefined constants for standard types and a `custom()` constructor for user-defined types.

**Benefits**:
1. Equipment crates can define custom end uses (e.g., `EndUse::custom("heat_pump_water_heater")`)
2. Actors can target custom end uses via `DispatchTarget::ByEndUse`
3. Standard types remain as constants (`EndUse::HVAC_HEATING`, `EndUse::BATTERY`, etc.)
4. Serialization works transparently for both standard and custom types
5. Backward compatible - existing code using `EndUse::Lighting` now uses `EndUse::LIGHTING`

**Usage Examples**:
```rust
// Standard predefined end use
let heating = EndUse::HVAC_HEATING;

// Custom user-defined end use for novel equipment
let hpwh = EndUse::custom("heat_pump_water_heater");
let ice_storage = EndUse::custom("ice_storage");
let v2g_charger = EndUse::custom("vehicle_to_grid");

// Custom end use survives serialization round-trip
let json = serde_json::to_string(&hpwh)?;
let decoded: EndUse = serde_json::from_str(&json)?;
assert_eq!(decoded, hpwh);

// Dispatch to custom end use works identically
dispatcher.queue(DispatchRequest {
    target: DispatchTarget::ByEndUse(EndUse::custom("my_custom_type")),
    signal: ControlSignal::PowerSetpoint { active_power_kw: 5.0, reactive_power_kvar: None },
    priority: PriorityTier::Schedule,
});
```

### Tests Added for Custom End Use Extensibility

**hares-types** (equipment.rs):
- `custom_end_use_creation_and_comparison` - verifies custom end uses can be created and compared
- `custom_end_use_round_trips_through_json` - verifies serialization works for custom types
- `standard_end_use_is_standard_returns_true` - verifies standard types are detected correctly
- `custom_end_use_is_standard_returns_false` - verifies custom types are detected as non-standard
- `custom_end_use_in_equipment_descriptor_round_trips` - verifies custom end uses work in equipment descriptors
- `end_use_from_string_and_static_str` - verifies `From` trait implementations

**hares-control** (dispatch.rs):
- `conflicts_with_same_name` - same ByName targets conflict
- `conflicts_with_different_name` - different ByName targets don't conflict
- `conflicts_with_same_end_use` - same ByEndUse targets conflict
- `conflicts_with_different_end_use` - different ByEndUse targets don't conflict
- `conflicts_with_different_variants_never_conflict` - ByName and ByEndUse never conflict
- `conflicts_with_custom_end_use` - custom end uses conflict correctly
- `custom_end_use_dispatch_request_round_trips_through_json` - serialization of dispatch requests with custom end uses

**hares-core** (dwelling/mod.rs):
- `control_dispatcher_routes_to_custom_end_use` - full integration test: dispatch finds equipment with custom end use
- `control_dispatcher_custom_end_use_misses_different_custom` - verifies different custom types don't match
- `control_dispatcher_warns_on_missing_custom_end_use` - verifies proper warnings when custom target not found