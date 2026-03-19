---
id: HARES-017
title: "hares-envelope — Fluid Solver (minimal v1)"
kind: implement
depends_on: [HARES-014, HARES-003]
files_to_touch:
  - crates/hares-envelope/src/fluid_solver.rs
  - crates/hares-envelope/src/lib.rs
  - crates/hares-types/src/fluid.rs
references:
  - docs/architecture/01-sim-core-and-solver.md
  - docs/architecture/02-equipment-and-ports.md
verification:
  - cargo check -p hares-envelope
  - cargo test -p hares-envelope
  - cargo clippy -p hares-envelope -- -D warnings
---

## Background/Context

The Generator equipment (CHP) computes waste heat output but OCHRE never routes it anywhere — it is dead code (see `02-equipment-and-ports.md`). HARES fixes this by routing CHP thermal output through `PortContribution::Fluid` to a `FluidSolver`. This v1 implementation is deliberately minimal: a simple loop energy balance with no hydraulic network model. The architecture supports upgrading to a full hydronic network model in a later ticket without changing the `DomainSolver` interface or the port types.

v1 commits to minimal fluid balance for CHP. Generator thermal output uses `PortContribution::Thermal` for zone heating and `PortContribution::Fluid` only for DHW preheat loops. Full hydronic loop balance deferred to v2.

**Dependency note**: HARES-003 must define `FluidAccumulator` (the per-loop accumulation buffer in `PortSlots::fluid`) before this ticket can proceed. Confirm `FluidAccumulator` is present in HARES-003 before starting.

## Work to Do

- [ ] Define `FluidLoopState` struct with fields: `loop_id: LoopId`, `fluid_type: FluidType`, `net_power_w: f64`, `mean_supply_temp_c: f64`, `mean_return_temp_c: f64`
  - `FluidLoopState` must be defined in `hares-types` (not `hares-envelope`) to avoid a circular crate dependency. Add `crates/hares-types/src/fluid.rs` to files_to_touch.
- [ ] Define `FluidDomainPayload` helper in `hares-types/src/fluid.rs` that serializes `Vec<FluidLoopState>` to/from `Vec<f64>` for transport via `DomainUpdate::custom_payload`. This keeps fluid state out of the shared `DomainUpdate` struct.
- [ ] Validate single-fluid-type-per-loop at construction time: `FluidSolver::new()` returns `Result`, rejecting configurations where the same `LoopId` is registered with different `FluidType` values. At runtime, fluid-type mismatches during accumulation are already caught by `PortSlots::accumulate` (which returns `Err` for undeclared loop/fluid-type combinations) — the solver does not need to re-validate this.
- [ ] Define `FluidSolverConfig` struct with field: `cp_water_j_kg_k: f64` (default `4186.0`)
- [ ] Define `FluidSolver` struct implementing `DomainSolver`:
  - Owns `config: FluidSolverConfig` and `loop_states: HashMap<LoopId, FluidLoopState>`
  - `new()` returns `Result<Self, HaresError>`, validating that no `LoopId` appears with conflicting `FluidType` values
  - `domain_id()` returns `FLUID` (3u16)
  - `resolve()` (infallible, returns `DomainUpdate`):
    1. Clear `loop_states` from the previous step
    2. Iterate over all `PortSlots::fluid` contributions, grouping by `loop_id`
    3. For each loop, compute `net_power_w = sum(flow_rate_kg_s * cp * (supply_temp_c - return_temp_c))` across all contributors on that loop
    4. Compute flow-weighted mean `supply_temp_c` and `return_temp_c` across contributors
    5. **Zero-flow sentinel**: when the sum of `flow_rate_kg_s` across all contributors on a loop equals 0.0, set `mean_supply_temp_c` and `mean_return_temp_c` to the values from the previous timestep's `loop_states` entry (if present) or 0.0 (first step). Never produce NaN.
    6. Store result in `loop_states`
    7. Return `DomainUpdate` with fluid state packed into `custom_payload` via `FluidDomainPayload::encode()`
- [ ] Expose `FluidSolver::loop_state(&self, loop_id: LoopId) -> Option<&FluidLoopState>` for upstream equipment queries and telemetry
- [ ] Handle the no-contribution case (no fluid ports written this step) — returns `DomainUpdate` with empty `custom_payload` without panic

## `FluidDomainPayload` Encoding

Fluid state is transported via the existing `DomainUpdate::custom_payload: Option<Vec<f64>>` field. Do **not** add first-class fields to `DomainUpdate` — that struct is shared across all domains and must remain domain-agnostic.

Define a `FluidDomainPayload` helper in `hares-types/src/fluid.rs`:

```rust
/// Encode/decode `Vec<FluidLoopState>` to/from `Vec<f64>` for `DomainUpdate::custom_payload`.
///
/// Layout per loop: [loop_id as f64, fluid_type as f64, net_power_w, mean_supply_temp_c, mean_return_temp_c]
/// Total length = states.len() * 5
pub struct FluidDomainPayload;

impl FluidDomainPayload {
    pub fn encode(states: &[FluidLoopState]) -> Option<Vec<f64>> { ... }
    pub fn decode(payload: &[f64]) -> Result<Vec<FluidLoopState>, HaresError> { ... }
}
```

`encode` returns `None` when `states` is empty (maps to `custom_payload: None`). `decode` returns `Err` if the payload length is not a multiple of 5.

## Files to Touch

- `crates/hares-types/src/fluid.rs`: new file — `FluidLoopState`, `FluidDomainPayload` (canonical definitions to avoid circular dependency)
- `crates/hares-envelope/src/fluid_solver.rs`: new file — `FluidSolver`, `FluidSolverConfig` (imports `FluidLoopState` from `hares-types`)
- `crates/hares-envelope/src/lib.rs`: add `pub mod fluid_solver` and re-export public types

**Not in scope**: `crates/hares-types/src/domain_solver.rs` — the shared `DomainUpdate` struct must not be modified by this ticket.

## Measures of Success

- [ ] Single fluid loop: `flow_rate = 0.5 kg/s`, `supply_temp = 60 C`, `return_temp = 40 C`, `cp = 4186 J/(kg K)` — `net_power_w = 0.5 * 4186 * 20 = 41860 W` to within 1e-6
- [ ] Two contributors on the same loop with different flow rates and temperatures — `net_power_w` equals the sum of individual contributions; mean supply/return temps are flow-weighted
- [ ] Zero-flow contribution (`flow_rate_kg_s = 0.0`) does not produce `NaN` in mean temperature calculations; mean temps fall back to previous-timestep values
- [ ] All-zero flow across entire loop (`total flow_rate = 0.0`) returns previous-timestep mean temps without NaN
- [ ] Empty `PortSlots::fluid` (no fluid equipment active) returns a `DomainUpdate` with `custom_payload: None` and does not panic
- [ ] `loop_state()` returns `None` for a `LoopId` with no contributions this step
- [ ] `FluidSolver::new()` returns `Err` when the same `LoopId` is configured with conflicting `FluidType` values
- [ ] `PortSlots::accumulate` returns `Err` when a `Fluid` contribution targets an undeclared loop/fluid-type combination (existing behavior, just confirm via test)
- [ ] `FluidDomainPayload::decode(FluidDomainPayload::encode(&states).unwrap())` round-trips correctly

## Verification

- [ ] `cargo check -p hares-envelope` passes
- [ ] `cargo test -p hares-envelope` passes
- [ ] `cargo clippy -p hares-envelope -- -D warnings` passes
