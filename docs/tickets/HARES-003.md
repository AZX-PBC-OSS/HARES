---
id: HARES-003
title: "hares-types — Port Types"
kind: implement
depends_on: [HARES-001, HARES-002]
files_to_touch:
  - crates/hares-types/src/ports.rs
  - crates/hares-types/src/lib.rs
references:
  - docs/architecture/02-equipment-and-ports.md
verification:
  - cargo check -p hares-types
  - cargo test -p hares-types
  - cargo clippy -p hares-types -- -D warnings
---

## Background/Context
Every equipment model communicates its physical effect on the simulation through typed port contributions. Defining these types centrally in `hares-types` keeps the accumulation logic decoupled from any single equipment crate and allows the scheduler to aggregate effects without knowing equipment internals.

## Work to Do
- [ ] Define `PortContribution` enum with variants:
  - `Thermal { zone: ZoneId, sensible_gain_w: f64, latent_gain_w: f64 }`
  - `Electrical { active_power_kw: f64, reactive_power_kvar: f64 }`
  - `Fuel { fuel_type: FuelType, consumption_w: f64 }`
  - `Fluid { loop_id: LoopId, flow_rate_kg_s: f64, supply_temp_c: f64, return_temp_c: f64, fluid_type: FluidType }`
  - `Custom { domain_id: DomainId, payload: [f64; 16] }`
- [ ] Define accumulator types that sum contributions:
  - `ThermalAccumulator` — sensible and latent totals per zone
  - `ElectricalAccumulator` — split active power tracking: `load_kw: f64` (positive contributions), `generation_kw: f64` (negative contributions), `reactive_power_kvar: f64` total. Accumulation routes `active_power_kw >= 0` to `load_kw` and `active_power_kw < 0` to `generation_kw`, preserving the load/generation split needed by the ZIP model in HARES-016 without per-step allocation. Exposes `net_active_kw() -> f64` returning `load_kw + generation_kw`.
  - `FuelAccumulator` — consumption total per fuel type
  - `FluidAccumulator { loop_id: LoopId, fluid_type: FluidType, total_flow_kg_s: f64, mean_supply_temp_c: f64, mean_return_temp_c: f64 }`
  - `CustomAccumulator { domain_id: DomainId, payload: [f64; 16] }`
- [ ] Define `PortSlots` struct with preallocated accumulators:
  - `thermal: Vec<ThermalAccumulator>` — one per zone
  - `electrical: ElectricalAccumulator` — single bus (v1)
  - `fuel: FuelAccumulator`
  - `fluid: Vec<FluidAccumulator>` — one per loop (sized if hydronic present)
  - `custom: Vec<CustomAccumulator>` — one per registered custom domain
- [ ] Implement `PortSlots::zero()` method that resets all accumulators to zero
- [ ] Define `PortDeclaration` struct with fields: `port_type` (enum matching `PortContribution` variant tags), `zone: Option<ZoneId>`, `loop_id: Option<LoopId>`, `domain_id: Option<DomainId>` — used at init time to validate equipment-port wiring and pre-size PortSlots accumulators
- [ ] Derive `serde::Serialize` and `serde::Deserialize` on all public types
- [ ] Re-export from `lib.rs`
- [ ] `FluidAccumulator::add` must use `new_total_flow.abs() > f64::EPSILON` guard instead of `new_total_flow != 0.0`; reject negative `flow_rate_kg_s` with `Err`
- [ ] `PortSlots::accumulate` must return `Err` when writing to a zone not declared in any `PortDeclaration` — do NOT dynamically grow the Vec
- [ ] `FuelAccumulator` must be keyed by `FuelType` — either `HashMap<FuelType, f64>` or `Vec<(FuelType, f64)>` — to support buildings with multiple fuel types (gas furnace + oil boiler)
- [ ] `FuelAccumulator::add` should return `Err` (or log warning) when `FuelType::None` is used — equipment with no fuel should not write Fuel port contributions
- [ ] Write tests:
  - Accumulate 3 separate `Thermal` contributions into `PortSlots` and verify the sum matches expectations
  - Call `PortSlots::zero()` and verify all accumulators are reset to zero (including `fluid` and `custom` accumulators)
  - Accumulate `Fluid` contributions into `PortSlots` and verify `FluidAccumulator::zero()` resets correctly
  - Accumulate `Custom` contributions into `PortSlots` and verify `CustomAccumulator::zero()` resets correctly

## Files to Touch
- `crates/hares-types/src/ports.rs`: new file — `PortContribution`, `ThermalAccumulator`, `ElectricalAccumulator`, `FuelAccumulator`, `FluidAccumulator`, `CustomAccumulator`, `PortSlots`, `PortDeclaration`
- `crates/hares-types/src/lib.rs`: add `pub mod ports` and re-exports

## Measures of Success
- [ ] `PortContribution` has exactly five variants as specified
- [ ] `Fluid` variant uses `loop_id: LoopId` (not `u16`) and includes `fluid_type: FluidType`
- [ ] `Custom` variant uses `domain_id: DomainId` (not `u16`) and payload is fixed-size `[f64; 16]` (not `Vec`)
- [ ] `PortSlots` includes all five accumulator fields: `thermal`, `electrical`, `fuel`, `fluid`, `custom`
- [ ] `PortDeclaration` has `port_type`, `zone: Option<ZoneId>`, `loop_id: Option<LoopId>`, `domain_id: Option<DomainId>`
- [ ] `PortSlots::zero()` demonstrably resets all accumulator fields including `fluid` and `custom` vecs
- [ ] Thermal accumulation test passes with correct summed values
- [ ] Reset test confirms all fields are zero after `zero()`
- [ ] `FluidAccumulator::zero()` and `CustomAccumulator::zero()` tests pass
- [ ] `FluidAccumulator::add` returns `Err` for negative `flow_rate_kg_s`
- [ ] `FluidAccumulator::add` uses epsilon guard, not exact float equality
- [ ] Accumulating a contribution to an undeclared zone returns `Err`, not a silent `Vec::push`
- [ ] `FuelAccumulator` correctly separates accumulation by `FuelType` for a two-fuel building

## Performance Notes
- **P5 — FuelAccumulator**: Replaced `HashMap<FuelType, f64>` with `[f64; 4]` array indexed by `FuelType` ordinal. Eliminates per-step hashing overhead and makes `FuelAccumulator` `Copy`. OCHRE profiling showed fuel accumulation as a minor but frequent allocation site.
- **P6 — PortContribution by-ref**: `PortSlots::accumulate` now takes `&PortContribution` instead of by-value. `PortContribution` is 128+ bytes (large enum with `Fluid` variant); passing by reference avoids memcpy on every equipment step call.

## Verification
- [ ] `cargo check -p hares-types` passes
- [ ] `cargo test -p hares-types` passes
- [ ] `cargo clippy -p hares-types -- -D warnings` passes
