# PriorityTier assignment for every signal type, safety vs. DR conflict prevention
**Review ID**: ctrldp-03
**Category**: control-deep
**Date**: 2026-05-26

## Files Reviewed
crates/hares-control/src/dispatch.rs
crates/hares-types/src/control_signal.rs
crates/hares-control/src/signal.rs
crates/hares-core/src/dwelling/mod.rs
crates/hares-core/src/actors/dr_compliance.rs
crates/hares-core/src/actors/occupant.rs
crates/hares-core/src/actors/solver_feedback.rs
crates/hares-core/src/actors/bms.rs
crates/hares-core/src/actors/ev_driver/mod.rs
crates/hares-core/src/actors/ev_driver/composer.rs
crates/hares-core/src/actors/ideal_thermostat.rs
crates/hares-core/tests/dispatch_ordering_regressions.rs

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: No explicit tier assignment exists per signal type — silent, implicit defaults
**Severity**: critical
**Description**: The `ControlSignal` enum has 25 variants (ThermalSetpoint, PowerSetpoint, SOCTarget, ModeOverride, DutyCycle, DemandResponse, CurtailmentPercent, IdealCapacity, ProtocolNative, etc.), but there is no function, match table, or trait that assigns a `PriorityTier` to each variant. Instead, tier assignment is delegated entirely to individual actors, each of which attaches a tier when constructing a `DispatchRequest`. Across the codebase, the assignment is as follows (arranged by tier, inferred from actor source):

| Tier | Actual Signal Types Assigned | Actors |
|---|---|---|
| Safety (3) | **None** — never assigned by any built-in actor | (unused) |
| Grid (2) | LoadFraction, ThermalSetpointDelta, ThermalSetpoint, ModeOverride, PowerLimit | DrCompliance |
| UserOverride (1) | ModeOverride, PowerSetpoint, LoadFraction, ThermalSetpoint | Occupant, IdealThermostat |
| Schedule (0) | IdealCapacity, PowerSetpoint, GridConnect, SelfConsumption, SOCTarget, EvPlugIn, EvSetReadyBy, EvAwayCharge | SolverFeedback, BMS, Occupant (EV only), EvDriverActor |

Critically, there is no compiler-enforced guarantee that every `ControlSignal` variant is assigned to an appropriate tier. Adding a new signal variant (e.g., `DemandResponse`, `CurtailmentPercent`, `ReactiveSetpoint`, `ProtocolNative`) carries no lint, compile-time error, or runtime check ensuring a tier is chosen. If a new actor is written, it can attach any tier to any signal without restriction.
**Code Location**: The `dispatch.rs` `DispatchRequest` structure (line 62-67) has an unconstrained `priority: PriorityTier` field; no centralized mapping exists anywhere in the codebase.
**Root Cause**: The design separates control signal type (`ControlSignal`) from priority (`PriorityTier`) as orthogonal fields without a validation layer. There is no `impl From<ControlSignal> for PriorityTier` or equivalent central routing table.
**Impact**: Signal types that should be high-priority (e.g., `DemandResponse` at Grid tier, or future safety-protection signals) may be mistakenly assigned to Schedule by a new actor. Conversely, low-importance signals could be assigned to Grid or Safety, diluting the tier semantics. This is a latent correctness hazard that grows with each new actor.

### Finding 2: Safety tier is defined but never used by any built-in actor
**Severity**: critical
**Description**: `PriorityTier::Safety` (value 3, the highest tier) is declared and ordered correctly in the enum (`dispatch.rs:15-21`), and cross-tier dispatching drains it last (so it wins over all other tiers). However, a search of the entire codebase reveals zero non-test uses of `PriorityTier::Safety` by any built-in actor, equipment model, or dwelling subsystem. The Safety tier is only used in unit/regression tests and in the Python bindings (`py_actor.rs:417`).

Specifically, none of the following safety-critical conditions trigger a Safety-tier dispatch:
- Freeze protection (zone temperature approaching freezing)
- Over-temperature lockout (equipment exceeding safe operating range)
- Over-current protection
- Leak detection
- Emergency shutdown
- Under-voltage lockout

**Code Location**: `dispatch.rs:15-21` declares the tier; grep across the codebase confirms `PriorityTier::Safety` appears only in test files (`dispatch_ordering_regressions.rs:457,490`, `dwelling/mod.rs:4495`) and the Python binding mapper (`py_actor.rs:417`).
**Root Cause**: The Safety tier was designed as infrastructure but safety-critical conditions are either handled internally within equipment models (e.g., defrost logic, thermal limits in the heat pump heater's internal state machine) without producing dispatches that go through the priority system, or are simply not implemented yet.
**Impact**: Since no actor generates Safety-tier signals, the intended safety-over-DR protection is not exercised. In a scenario where a DR program requests heat-pump-disable (Grid tier) during sub-freezing weather, there is no safety actor that can override the DR signal with a Safety-tier freeze-protection signal. The DR signal would be the highest-priority signal in practice (Grid is the highest tier actually used), and the building could freeze. The tier architecture is functionally two tiers (Schedule and UserOverride) with Grid acting as the de-facto highest tier, making the Safety tier a dead variant.

### Finding 3: Same-tier conflict resolution is undocumented and last-write-wins only
**Severity**: high
**Description**: When two signals at the same priority tier target the same equipment, the dispatcher applies them in FIFO order, and the last signal applied wins (because `apply_control` is overwrite-safe, not accumulative). The comment at `dwelling/mod.rs:329-331` states: "Because every signal fires (no deduplication), the highest-priority tier writes last and wins." However, this only documents cross-tier behavior. The same-tier resolution strategy is not documented in any code comment, design doc, or module-level header. It is only identifiable by reading the `dispatch_same_tier_same_target_last_write_wins` test name (`dwelling/mod.rs:5019`).

Important scenarios where same-tier conflicts arise:
- Two DR programs in the same tier (Grid) requesting different battery SOC setpoints
- Two actors in the Schedule tier emitting conflicting `PowerSetpoint` values for the same target
- Two UserOverride actors (Occupant + IdealThermostat) targeting the same HVAC equipment

The current strategy (last-write-wins) means the outcome depends on actor registration order, which is implicit and fragile. There is no "most restrictive" (min consumption for curtailment, widest deadband for thermostats), "least restrictive", or "weighted average" strategy for same-tier resolution.

**Code Location**: `dwelling/mod.rs:408-443` (`drain_tiers` function) — the inner loop drains each tier queue via `VecDeque::drain` (FIFO), and `apply_control` is called for each request in sequence with no same-tier deduplication or merge logic. Test at `dwelling/mod.rs:5019-5048`.
**Root Cause**: The design assumes that same-tier signals from independent actors targeting the same equipment are rare enough that FIFO/last-write-wins is sufficient. For DR programs, a composition layer (like the EV composer at `ev_driver/composer.rs:100-137`, which uses "most restrictive" logic) could be applied but is not generalized.
**Impact**: In multi-actor simulations, the result of same-tier conflict resolution is non-deterministically dependent on actor registration order. If two DR programs run simultaneously (e.g., utility DR + frequency regulation, both at Grid tier), the outcome is whichever actor registered last wins — neither the most restrictive nor the economically optimal signal necessarily prevails.

### Finding 4: Cross-pass priority inversion is correctly prevented, but only within a single timestep
**Severity**: medium
**Description**: The `ControlDispatcher` maintains a `seen_targets` ledger (`dwelling/mod.rs:341-346`) that tracks which `(DispatchTarget, tier_index)` pairs have already been applied. This ledger is reset by `begin_step()` once per timestep but persists across multiple dispatch passes within the same timestep (pre-thermal-FSM flush + post-actor-decide dispatch). The logic at `dwelling/mod.rs:414-416` correctly skips lower-tier signals when a higher-tier signal has already been applied to the same target in any pass of the current step. The reverse case (higher-tier overwriting lower-tier) is explicitly allowed at lines 427-436.

This is tested by `dispatch_ordering_regressions.rs:446-499` (BLOCKER 4 tests) which verify both:
- Safety queued externally then Schedule emitted by actor: Safety wins
- Schedule queued externally then Safety emitted by actor: Safety still wins

**Code Location**: `dwelling/mod.rs:339-443` (ControlDispatcher struct and methods).
**Root Cause**: N/A — this is correctly implemented.
**Impact**: The cross-pass protection is sound for same-step multi-pass dispatch. The only concern is that the `seen_targets` vector uses linear search (`Vec::iter().any()`) rather than a `HashMap`, which could become a performance bottleneck with many targets. However, this is a performance concern, not a correctness one.

### Finding 5: The tier system is partially extensible but lacks compile-time guards
**Severity**: medium
**Description**: The `PRIORITY_TIER_COUNT` constant (`dispatch.rs:9`) is used to size the `by_tier: [VecDeque<DispatchRequest>; PRIORITY_TIER_COUNT]` array in `ControlDispatcher` (`dwelling/mod.rs:340`). Adding a new tier variant to `PriorityTier` requires:
1. Adding a variant with a new discriminant in `dispatch.rs`
2. Updating `PRIORITY_TIER_COUNT` to match
3. Updating the `py_actor.rs` Python binding mapping (4 match arms)
4. Updating any actor that should use the new tier

The unit test at `dispatch.rs:122-127` verifies that `PRIORITY_TIER_COUNT == 4` matches the variant count, providing a runtime guardrail. However:
- There is no `#[non_exhaustive]` on the `PriorityTier` enum, so external code (Python bindings, downstream crates) can match exhaustively without forward-compatibility.
- There is no compile-time assertion linking the array size to the enum variant count (e.g., a `const _: () = assert!(...)` using `std::mem::variant_count` when stabilized).
- The `PyPriority` enum in `py_actor.rs` (`py_actor.rs:412-417`) mirrors `PriorityTier` manually and would silently fail if a new tier is added without updating the Python binding.

Reordering existing tiers requires changing the explicit `#[repr(u8)]` discriminants. The `Ord` derive respects declaration order, which *can* differ from the discriminant order if the discriminants don't follow declaration order — currently they do (0, 1, 2, 3 matching declaration order). If someone reorders variants without updating discriminants, the `Ord`-based ordering would silently change while `index()` (which uses `as usize` returning the discriminant) would remain consistent with the old ordering, creating a subtle inconsistency.

**Code Location**: `dispatch.rs:8-29` (PRIORITY_TIER_COUNT, PriorityTier enum, index method). Python binding at `py_actor.rs:412-417`.
**Root Cause**: The design uses two independent ordering mechanisms (derived `Ord` via declaration order, and explicit `#[repr(u8)]` discriminants) that are currently aligned but can drift.
**Impact**: Adding a new tier requires touching 3-4 files and is error-prone. Reordering tiers could silently break ordering semantics due to the dual `Ord`/discriminant mechanism.

### Finding 6: Grid-tier DR signals can override UserOverride HVAC signals without thermal safety backstop
**Severity**: high
**Description**: The `DrCompliance` actor emits DR control signals at `PriorityTier::Grid` (`dr_compliance.rs:335`), which is higher than `UserOverride` (used by `Occupant` and `IdealThermostat`). This means:
- A DR event can override a user's explicit thermostat setting (intended: DR > user comfort)
- BUT a DR event requesting heat-pump-disable (`ModeOverride { mode: Off }`) will also override a user's freeze-protection setpoint if the user's actor tried to set heating to 10°C, because Grid(2) > UserOverride(1)

The DR actor's HVAC actions include: `SetpointAdjust` (delta), `AbsoluteSetpoint`, and `TurnOff` (`dr_compliance.rs:149-166`). The `TurnOff` action produces `ControlSignal::ModeOverride { mode: OperatingMode::Off }` at Grid tier. In sub-freezing weather with no Safety actor (see Finding 2), this effectively means a DR TurnOff is the highest-priority signal and cannot be overridden by any protective measure.

**Code Location**: `dr_compliance.rs:301-337` (dispatch_for_action), `dwelling/mod.rs:408-443` (drain_tiers tier ordering).
**Root Cause**: Same as Finding 2 — the Safety tier exists but has no actor assigned to it. The infrastructure for safety-over-DR override exists at the dispatch level but is not wired at the application level.
**Impact**: DR TurnOff during freezing weather could lead to equipment/building damage in simulation. While this might be intended for some extreme DR programs, the lack of a safety override layer means the simulation cannot model a thermostat's internal freeze-protection logic overriding the DR command.

## Summary
- Total findings: 6
- Critical: 2 (Finding 1, Finding 2)
- High: 2 (Finding 3, Finding 6)
- Medium: 2 (Finding 4, Finding 5)
- Low: 0

## Recommendations

1. **Create a centralized signal-to-tier mapping** (`impl From<&ControlSignal> for PriorityTier` or a standalone function). This ensures every signal variant has an explicit, documented tier assignment and provides a single point of review when adding new variants. The per-actor tier assignment should either delegate to this central mapping or override it explicitly with a comment justifying the override.

2. **Implement a safety actor** that monitors zone temperatures, equipment operating states, and health indicators, dispatching Safety-tier signals when thresholds are crossed (freeze protection at 5°C, over-temperature lockout above equipment max, etc.). Until this exists, consider whether the Grid tier should be demoted below UserOverride or whether a thermal guard clause should be added to the DR actor.

3. **Document the same-tier conflict resolution strategy** in a module-level comment on `ControlDispatcher`, including the rationale for choosing last-write-wins over alternatives (most-restrictive, weighted average). If last-write-wins is the intended permanent strategy, add a mechanism to make actor registration order explicit and observable.

4. **Add a `#[non_exhaustive]` attribute** to `PriorityTier` to ensure downstream consumers of the public API handle future tier additions gracefully.

5. **Replace `PRIORITY_TIER_COUNT` with a compile-time derive** or use `std::mem::variant_count` (Rust 1.77+ / nightly) when stable in the MSRV, so the array size is always in sync with the enum definition.

6. **Consider adding a DR safety-guard clause** in the `DrCompliance` actor: when the `TurnOff` action is used on HVAC equipment and the zone temperature is below a configurable freeze-risk threshold, downgrade to a setpoint adjustment instead of a full shutdown, or emit the TurnOff at Grid tier and also emit a Safety-tier minimum-heating guard.

## References / Citations

- `dispatch.rs:9` — `PRIORITY_TIER_COUNT` const
- `dispatch.rs:11-21` — `PriorityTier` enum with `#[repr(u8)]` discriminants
- `dispatch.rs:23-28` — `PriorityTier::index()` method
- `dispatch.rs:62-67` — `DispatchRequest` struct with unconstrained `priority` field
- `dwelling/mod.rs:326-443` — `ControlDispatcher` struct, `drain_tiers` conflict resolution logic
- `dwelling/mod.rs:408-416` — Lower-tier rejection when higher tier already applied
- `dwelling/mod.rs:427-436` — Higher-tier overwrites lower-tier tracking
- `dwelling/mod.rs:5019-5048` — `dispatch_same_tier_same_target_last_write_wins` test
- `dr_compliance.rs:222-224` — Doc comment confirming Grid tier
- `dr_compliance.rs:301-337` — DR signal dispatch with Grid tier
- `dispatch_ordering_regressions.rs:446-499` — BLOCKER 4: cross-pass priority inversion tests
- `py_actor.rs:412-417` — Python binding mirror of PriorityTier
- `ev_driver/composer.rs:100-137` — Example of "most restrictive" composition for same-tier EV signals
