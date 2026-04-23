# Actor Trait Must Expose `telemetry()` Method

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-core/actor, hares-actors

## Problem

The `Actor` trait at `crates/hares-core/src/actor.rs:43-66` has no `telemetry()` method. Actor decision history is only visible via `tracing::debug!` calls in individual actor implementations. This violates project policy `feedback_actor_telemetry`: "actors must emit telemetry for inspecting dispatched actions/controls".

Without a structured telemetry API, downstream tools (CSV diagnostics, Python introspection, dashboard binding) cannot observe actor behaviour. Tracing logs are not a replacement: they are unstructured, only emitted at debug level, and not consumable by analysis pipelines.

## Current Behavior

`crates/hares-core/src/actor.rs:43-66`:
```rust
pub trait Actor {
    fn decide(&mut self, env: &EnvironmentState, ...) -> Result<Vec<Decision>, ActorError>;
    fn name(&self) -> &str;
    // no telemetry()
}
```

Implementors of `Actor` have no obligation to expose internal state. Inspecting an actor's decision history requires re-running with debug-level tracing enabled and parsing log lines.

## Required Behavior

1. Add a default-implemented method to the `Actor` trait:
   ```rust
   fn telemetry(&self) -> Option<&Telemetry> { None }
   ```
   The default returns `None` so existing actors compile unchanged.
2. Implement `telemetry()` to return `Some(&self.telemetry)` for every actor that holds observable internal state. The reviewed list:
   - `OccupantActor` (occupancy schedule decisions)
   - `IdealThermostatActor` (mode/setpoint/decision history)
   - `BmsActor` (control decisions and overrides)
   - `DrComplianceActor` (DR signal compliance state)
   - `EvDriverActor` and `Ev*` family (charge schedule, plug-in state, SoC tracking)
3. The `Telemetry` type is the same one used by equipment (a struct of named keyed channels per timestep). Reuse the existing definition; do not introduce a separate actor-only telemetry abstraction.
4. Per `feedback_no_silent_defaults`, a release/CI test asserts that every concrete actor type in `hares-actors` either returns `Some(&Telemetry)` from `telemetry()` or has an explicit `#[doc = "..."]` annotation justifying why it has no observable state.

## Approach

1. Add the trait default in `crates/hares-core/src/actor.rs:43-66`.
2. For each actor in `crates/hares-actors/src/`, add a `telemetry: Telemetry` field if not already present, populate it during `decide()`, and override the trait method to return `Some(&self.telemetry)`.
3. Wire actor telemetry into the dwelling step so the Python `post_solvers` dict and the diagnostic CSV expose actor channels alongside equipment channels.
4. Add a unit test per actor verifying the telemetry contains the expected keys after a representative decision.
5. Add an integration test asserting all registered actor types in a dwelling expose non-empty telemetry after a single timestep.

## Definition of Done

- [ ] `Actor::telemetry(&self) -> Option<&Telemetry>` default-implemented in trait
- [ ] `OccupantActor`, `IdealThermostatActor`, `BmsActor`, `DrComplianceActor`, `EvDriverActor`, and other Ev* actors override `telemetry()` to return `Some(&self.telemetry)`
- [ ] Each actor's telemetry contains the keys necessary to reconstruct its decision (e.g. mode, target setpoint, scheduled value, DR-compliance state)
- [ ] Actor telemetry exposed in dwelling diagnostic output and Python `post_solvers` dict
- [ ] Unit tests assert per-actor telemetry contents
- [ ] Integration test: single dwelling step exposes telemetry for every registered actor

## Verification

```bash
cargo test -p hares-core actor
cargo test -p hares-actors
cargo test -p hares-core dwelling_telemetry
```

## References

- Project policy `feedback_actor_telemetry.md` — actors must emit telemetry for inspecting dispatched actions/controls.
- Project policy `project_python_extensibility.md` — users implementing custom actors via Python callbacks need a telemetry API to surface their actor's state.
- HARES `crates/hares-equipment/src/telemetry.rs` (or wherever the equipment Telemetry type lives) — reuse this type.

## Related Tickets

- 016-thermostat-decision-tracing (existing thermostat tracing — telemetry is the structured replacement)
- 019-speed-startup-telemetry-gaps (related telemetry coverage gap)
- 020-setpoint-chain-visibility (setpoint resolution chain visibility — actor telemetry surfaces this)
