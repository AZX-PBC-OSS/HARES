# Demand response compliance actor: DR event enforcement
**Review ID**: agents-05
**Category**: agents
**Date**: 2026-05-26

## Files Reviewed
crates/hares-core/src/actors/dr_compliance.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Dwelling.py

## Findings

### Finding 1: [Severity: critical]
**Description**: `set_dr_level()` is never called in production code — the actor is always idle. The `DrCompliance` actor initializes `current_dr_level` to `DRLevel::Normal` (line 256) and the only path to change it is the public method `set_dr_level()` (line 286). Grep across the entire crates tree shows `set_dr_level` is called exclusively in tests (`dr_compliance.rs:510,535,554,581,606,633,650,666,676,679,711,737` and `actor_telemetry_regressions.rs:170,299`). No integration code in `crates/hares-core/src/dwelling/mod.rs` or any actor wiring calls this method. The `decide()` method (line 349) returns early at line 356-360 when `current_dr_level == DRLevel::Normal`, emitting zero dispatch requests. The actor is therefore completely inert in any production simulation run.
**Code Location**: `crates/hares-core/src/actors/dr_compliance.rs:256,286-287,349-360`
**Root Cause**: Incomplete integration — the actor API exposes `set_dr_level()` but no external scheduler, event bus, or dwelling loop calls it. DR events from config/simulation inputs are not wired to the actor.
**Impact**: No DR signals are ever dispatched. All demand response modeling is non-functional.

### Finding 2: [Severity: critical]
**Description**: No multi-DR event overlap handling. The actor stores a single `current_dr_level: DRLevel` field (line 237). When multiple DR events overlap (e.g., a scheduled utility curtailment event at `DRLevel::High` and a simultaneous grid emergency priced-based event at `DRLevel::GridEmergency`), the `set_dr_level()` method overwrites the previous level without computing the most restrictive combined limit. There is no tracking of active event instances, no vector of active events, and no logic to select `max()` across concurrent levels. The spec requires the actor to "apply the most restrictive limit rather than summing or alternating."
**Code Location**: `crates/hares-core/src/actors/dr_compliance.rs:237,286-287`
**Root Cause**: The `current_dr_level` field is a scalar, not a set or priority-ordered collection. `set_dr_level` is a simple assignment.
**Impact**: Overlapping DR events will apply whichever was set last, possibly a weaker limit, violating DR contract requirements and underestimating curtailment for simultaneous events.

### Finding 3: [Severity: critical]
**Description**: Sticky control signals persist after DR event end — no "clear" signal dispatched. The actor dispatches `PowerLimit` and `ModeOverride` signals during DR events (lines 325-327, 322-324). On equipment side, `ctrl_power_limit_kw` and `ctrl_mode_override` are **sticky** — they persist across steps until explicitly reset:
- Heat pump heater: `ctrl_power_limit_kw` only resets in `reset_operating_state()` (heater.rs:556). Not reset in `update_control()`.
- Air conditioner: `ctrl_power_limit_kw` only resets in `reset_operating_state()` (air_conditioner.rs:480). Not reset in `step()`.
- `ctrl_mode_override` similarly sticky on all HVAC equipment.

When the DR level returns to `DRLevel::Normal`, `decide()` returns early (line 356-360) without dispatching any signal, including a "reset" signal (e.g., `PowerLimit { max_kw: f64::INFINITY }` or a `ModeOverride` returning to `OperatingMode::Heating`). `LoadFraction` is safe because equipment auto-resets `ctrl_load_fraction = 1.0` each step (heater.rs:895, air_conditioner.rs:989).
**Code Location**: `crates/hares-core/src/actors/dr_compliance.rs:322-329,349-360`
**Root Cause**: The actor is stateless with respect to prior dispatch — it does not track what signals it previously sent and therefore cannot issue reversal signals when the event ends.
**Impact**: After a DR event that deployed `PowerLimit` or `ModeOverride::Off`, equipment may remain permanently curtailed or off until a full operational reset (e.g., simulation restart). This silently corrupts post-DR simulation results.

### Finding 4: [Severity: high]
**Description**: Actor registry construction omits target and action configuration. The registry factory for `"DrCompliance"` (actor_registry.rs:201-215) creates the actor and optionally configures a `ComplianceModel`, but never sets `hvac_target`, `hvac_action`, or `load_targets` from config parameters. The registry test (actor_registry.rs:400-407) verifies only `actor.name()` — it does not exercise dispatch. A registry-created `DrCompliance` will have `hvac_target: None` and empty `load_targets`, so even if DR level were injected, `dispatch_for_action` (line 301) would produce zero signals (the `hvac_target` branch at line 381 is skipped because target is `None`, and the `load_targets` loop at line 385 iterates an empty `Vec`).
**Code Location**: `crates/hares-core/src/actor_registry.rs:201-215`, `crates/hares-core/src/actors/dr_compliance.rs:253-255,381-387`
**Root Cause**: The registry factory only maps `compliance_rate`, `seed`, and `always_comply` parameters; no code reads `hvac_target`, `hvac_action`, or `load_targets` from `ActorConfig`.
**Impact**: Any `DrCompliance` actor instantiated via simulation config (the standard path) will have zero configured dispatch targets and will never send any control signals regardless of DR level.

### Finding 5: [Severity: high]
**Description**: `Proportional` compliance decision ignores DR severity. The `Probabilistic::should_comply()` method (line 127-131) includes `dr_level` in its hash but the compliance threshold comparison (`normalized < self.compliance_rate`) is independent of DR severity. A `DRLevel::GridEmergency` (level 4) and `DRLevel::Moderate` (level 1) have the same probability of compliance for a given `compliance_rate`. This contradicts expected behavior where higher-severity events should have a higher probability of occupant compliance. OCHRE does not model occupant compliance decisions — it applies DR signals directly to equipment — but the HARES behavioral model should scale compliance probability with event severity.
**Code Location**: `crates/hares-core/src/actors/dr_compliance.rs:127-131`
**Root Cause**: The hash incorporates `dr_level` as a mixing input (line 139-140) but the binary threshold comparison (line 130) is a single fixed `compliance_rate` with no severity-dependent scaling.
**Impact**: Occupant compliance decisions are unrealistically uniform across all DR event severities.

### Finding 6: [Severity: medium]
**Description**: No enforcement of equipment operational constraints. The `dispatch_for_action()` method (line 301-337) generates `ModeOverride(Off)`, `PowerLimit`, and `LoadFraction` signals without consulting equipment minimum runtime, minimum off-time, or safety limits. Equipment-side validation happens in `apply_control_unchecked()` which may silently reject invalid mode changes (e.g., short-cycle protection), but the actor reports success in telemetry (`signals_count` incremented at line 390). There is no feedback path from equipment to actor — the actor cannot know whether its signal was effective.
**Code Location**: `crates/hares-core/src/actors/dr_compliance.rs:301-337,389-390`
**Root Cause**: The actor-signal-equipment pipeline is fire-and-forget; equipment rejections are logged as warnings but not surfaced back to the actor.
**Impact**: DR compliance telemetry may report signals as "dispatched" while the equipment actually ignored them. Water heater safety limits (e.g., freeze protection temperatures, max element temperature) are not validated by the actor before dispatch.

### Finding 7: [Severity: medium]
**Description**: `DemandResponse` control signal variant unused by DR compliance actor. The `ControlSignal` enum defines a dedicated `DemandResponse { level, duration_s }` variant (control_signal.rs:84-87) which triggers equipment's `apply_dr_level()` method and has built-in duration-based auto-reversion to Normal (heater.rs:898-908, air_conditioner.rs:673). However, `dispatch_for_action()` (dr_compliance.rs:306-329) never generates this variant — it maps `DrAction` variants to `PowerLimit`, `LoadFraction`, `ModeOverride`, and `ThermalSetpointDelta` instead. The equipment-side `dr_duration_remaining_s` auto-revert mechanism (which correctly clears sticky DR state when the duration expires) is therefore unused, leaving the sticky-state problem from Finding 3 without a safety net.
**Code Location**: `crates/hares-core/src/actors/dr_compliance.rs:306-329`, `crates/hares-types/src/control_signal.rs:84-87`
**Root Cause**: The `DrAction` enum (line 149) has no variant that wraps `DemandResponse`; the actor's dispatch path only handles `LoadCurtailment`, `SetpointAdjust`, `AbsoluteSetpoint`, `TurnOff`, `PowerLimit`, and `None`.
**Impact**: Equipment-native DR leak rate auto-reversion (a built-in safety mechanism) is unavailable. See Finding 3 for the sticky-signal consequence.

### Finding 8: [Severity: low]
**Description**: Unnecessary allocation for `ByEndUse` target in `load_targets`. The `with_load_target()` builder method (line 280-283) accepts both `DispatchTarget::ByName` and `DispatchTarget::ByEndUse`. The `ByEndUse` variant routes to all equipment matching the end-use category (`route_request()` at mod.rs:461-473), which is appropriate. However, the DR compliance actor's `load_targets` is a `Vec<(DispatchTarget, DrAction)>` with a single `DrAction` per target. If two DR events target the same end-use with different actions, the actor dispatches both (one overwrites the other on equipment, and the sticky issue from Finding 3 applies). The `conflicts_with()` method (dispatch.rs:52-58) does not detect `ByName` vs `ByEndUse` overlap — a `ByName("HVAC")` and `ByEndUse(HVAC_HEATING)` targeting the same equipment would both be applied, potentially with contradictory signals at the same priority tier.
**Code Location**: `crates/hares-core/src/actors/dr_compliance.rs:280-283`, `crates/hares-control/src/dispatch.rs:52-58`
**Root Cause**: `conflicts_with()` returns `false` for different `DispatchTarget` variants (line 56). The control dispatcher's `drain_tiers()` (mod.rs:397-443) only prevents lower-tier overwrites via `conflicts_with()`, so same-tier conflicting signals from different target variants both execute.
**Impact**: Same-tier contradictory signals (e.g., `PowerLimit { max_kw: 2.0 }` and `PowerLimit { max_kw: 10.0 }` from different named/dispatch-target variants) both reach the equipment; last-write timing within a single dispatch pass determines the outcome.

## Summary
- Total findings: 8
- Critical: 3
- High: 2
- Medium: 2
- Low: 1

## Recommendations

1. **Wire DR event injection** — Connect an external DR event source (schedule file, pricing signal, or controller) to call `set_dr_level()` on the `DrCompliance` actor each timestep before actor dispatch. Alternatively, embed DR level awareness in the actor so it queries `EnvironmentState` for active DR conditions directly.

2. **Implement multi-event overlap resolution** — Replace scalar `current_dr_level` with an ordered set of active DR events. On each `decide()` call, compute `current_dr_level = max(active_levels)` across all active events. When an event's duration expires, remove it from the set.

3. **Dispatch clear signals at event end** — When transitioning from an active DR level to `DRLevel::Normal`, dispatch reset signals for all previously-active targets:
   - `PowerLimit { max_power_kw: f64::INFINITY }` for any target that received a power limit.
   - `ModeOverride { mode: OperatingMode::Auto }` (or equivalent) for any target that received a `TurnOff` ModeOverride.
   - Alternatively, use the `DemandResponse` control signal variant with `duration_s` to leverage equipment auto-reversion.

4. **Configure targets from actor config** — Extend the registry factory to read `hvac_target`, `hvac_action`, and `load_targets` from `ActorConfig` parameters so that config-based `DrCompliance` instances are functional.

5. **Scale compliance probability by DR severity** — Modify `Probabilistic::should_comply()` to apply a severity-dependent multiplier to `compliance_rate`. For example: `effective_rate = compliance_rate * (1.0 + (dr_level as f64 - 1.0) * 0.1)`, clamped to `[0.0, 1.0]`, so GridEmergency events have a higher compliance probability than Moderate events.

6. **Validate equipment capability before dispatch** — Before dispatching `PowerLimit` or `ModeOverride`, check that the target equipment supports the corresponding `ControlCapabilities` flag and is not in a protected state (e.g., defrost, freeze protection). Surface rejections in telemetry.

7. **Add integration test** — Write a test that creates a `DrCompliance` actor via the registry factory, wires targets/actions, injects a DR level, and verifies that dispatch requests propagate through the full dwelling loop to equipment `apply_control()`.

## References / Citations

- HARES `DrCompliance` actor: `crates/hares-core/src/actors/dr_compliance.rs`
- HARES `ControlSignal` enum and `DemandResponse` variant: `crates/hares-types/src/control_signal.rs:38-146`
- HARES `DispatchRequest` and priority tier: `crates/hares-control/src/dispatch.rs:63-67`
- HARES `ControlDispatcher` conflict resolution: `crates/hares-core/src/dwelling/mod.rs:363-444`
- HARES actor registry: `crates/hares-core/src/actor_registry.rs:201-215`
- HARES HVAC PowerLimit handling: `crates/hares-equipment/src/hvac/heat_pump/heater.rs:2131-2138,1622-1669`
- HARES AC LoadFraction handling: `crates/hares-equipment/src/hvac/air_conditioner.rs:1434-1435,988-989`
- HARES HPWH DR handling: `crates/hares-equipment/src/water_heater/heat_pump_wh.rs:1029-1061`
- OCHRE DR signal forwarding: `vendors/OCHRE/ochre/Dwelling.py:236-246` — equipment receives DR via `update_model(control_signal)` with per-end-use routing
