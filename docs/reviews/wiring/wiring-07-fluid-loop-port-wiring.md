# Fluid loop port wiring: water heater <-> hydronic heating loop connectivity
**Review ID**: wiring-07
**Category**: wiring
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-types/src/fluid.rs`
- `crates/hares-types/src/ports.rs`
- `crates/hares-types/src/equipment.rs` (FluidType enum)
- `crates/hares-envelope/src/fluid_solver.rs`
- `crates/hares-equipment/src/water_heater/mod.rs`
- `crates/hares-equipment/src/water_heater/resistance.rs`
- `crates/hares-equipment/src/water_heater/heat_pump_wh.rs`
- `crates/hares-equipment/src/hvac/boiler.rs`
- `crates/hares-equipment/src/hvac/baseboard.rs`
- `crates/hares-equipment/src/event_load.rs`
- `crates/hares-core/src/dwelling/solver_builder.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/WaterHeater.py`
- `vendors/EnergyPlus/src/EnergyPlus/Plant/DataPlant.hh`
- `vendors/EnergyPlus/src/EnergyPlus/Plant/Loop.cc`
- `vendors/EnergyPlus/src/EnergyPlus/Plant/PlantManager.cc`

## Findings

### Finding 1: [Severity: high] — Fluid solver uses water cp for all fluid types

**Description**: The `FluidSolver::resolve` method computes net power using `self.config.cp_water_j_kg_k` (default 4186 J/(kg·K)) regardless of the loop's declared `FluidType`. Glycol mixtures (e.g., 50% propylene glycol at 60°C) have a specific heat of ~3550–4020 J/(kg·K), and refrigerants have dramatically different properties. EnergyPlus, by contrast, calls `glycol->getSpecificHeat()` per loop based on fluid type (`PlantManager.cc:2749`).

**Code Location**: `crates/hares-envelope/src/fluid_solver.rs:132-140`
```rust
let net_power_w: f64 = entries
    .iter()
    .map(|e| {
        self.config.cp_water_j_kg_k    // <-- always water cp
            * e.total_flow_kg_s
            * (e.mean_supply_temp_c - e.mean_return_temp_c)
    })
    .sum();
```

`FluidSolverConfig` is intentionally misnamed (`cp_water_j_kg_k`) and only supports one cp value:
```rust
// fluid_solver.rs:14-16
pub struct FluidSolverConfig {
    pub cp_water_j_kg_k: f64,
}
```

**Root Cause**: `FluidSolverConfig` only stores a single specific-heat value with a water-centric name. The `FluidType` discriminator on each loop is recorded in `loop_types` and propagated to `FluidLoopState`, but is never consulted when computing energy balance.

**Impact**: Net power calculations for glycol loops are off by 5–15%. Refrigerant loops would be off by >60%. This affects any simulation that configures non-water fluid types for a loop, including future glycol-filled hydronic systems and refrigerant-based heat-recovery loops.

---

### Finding 2: [Severity: high] — Boiler supply temperature calculation uses water cp constant

**Description**: Both `ElectricBoiler` and `GasBoiler` compute the hydronic supply temperature as:
```rust
// boiler.rs:229
supply_temp_c = return_temp_c + thermal_output_w / (self.flow_rate_kg_s * CP_LIQUID_WATER_J_KG_K)
```
This uses `CP_LIQUID_WATER_J_KG_K` (4186 J/(kg·K)) from `hares_physics::constants`. The boiler config accepts `fluid_type: FluidType` (defaulting to `FluidType::Water` via `heating_config.rs:373`), but the value is only written to the port contribution and never used for property lookups.

**Code Location**: `crates/hares-equipment/src/hvac/boiler.rs:229` (ElectricBoiler) and `boiler.rs:522` (GasBoiler)

**Root Cause**: The `fluid_type` field is plumbed through config, stored on the struct, and emitted on the port — but the temperature rise calculation ignores it. The boiler uses `use hares_physics::constants::CP_LIQUID_WATER_J_KG_K` as a hardcoded import (line 30) with no branch on `self.fluid_type`.

**Impact**: If a boiler is configured with `fluid_type: Glycol`, the predicted supply temperature will be incorrect. The temperature lift ΔT = Q / (ṁ × cp) is inversely proportional to cp; using water cp for glycol undervalues ΔT by 5–15%, overpredicting the outlet temperature.

---

### Finding 3: [Severity: medium] — `PortSlots` allows same-loop-different-fluid-type accumulators to silently diverge

**Description**: `PortSlots::from_declarations` (ports.rs:456–463) creates separate `FluidAccumulator` entries keyed by `(loop_id, fluid_type)`. If equipment A declares `(LoopId(1), Water)` and equipment B declares `(LoopId(1), Glycol)`, they get two separate accumulators. The `accumulate` method (ports.rs:558–561) matches both fields, so contributions land in different accumulators. The fluid solver then groups by `loop_id` alone and uses only the first entry's fluid_type as a fallback (line 130):
```rust
// fluid_solver.rs:126-130
let fluid_type = self
    .loop_types
    .get(&loop_id)
    .copied()
    .unwrap_or(entries[0].fluid_type);
```
This means one set of contributions is effectively invisible.

**Code Location**:
- `crates/hares-types/src/ports.rs:456-463` — accumulator creation by (loop_id, fluid_type) pair
- `crates/hares-types/src/ports.rs:558-561` — accumulation match by both fields
- `crates/hares-envelope/src/fluid_solver.rs:120-123` — grouping by loop_id only
- `crates/hares-envelope/src/fluid_solver.rs:126-130` — fallback to first entry's fluid_type

**Root Cause**: The `FluidAccumulator` is designed as a per-(loop_id, fluid_type) accumulator, but the solver groups only by `loop_id`. The constructor guard in `FluidSolver::new` (line 42-48) rejects duplicate-style conflicts in `declared_loops`, but the `solver_builder` passes `&[]` (solver_builder.rs:1187), disabling this check entirely.

**Impact**: If equipment with different fluid types get wired to the same loop_id (a config error), the simulation silently produces incorrect results rather than failing fast.

---

### Finding 4: [Severity: medium] — Water heater DHW demand loop wiring: supply/return semantics inverted

**Description**: When a water heater emits a `PortContribution::Fluid` for a domestic hot water draw, it writes:
```rust
// resistance.rs:541-547
ports.accumulate(&PortContribution::Fluid {
    loop_id: self.loop_id,
    flow_rate_kg_s: total_draw_kg_s,
    supply_temp_c: draw.outlet_temp_c,  // hot water delivered
    return_temp_c: mains_temp_c,        // cold water entering
    fluid_type: self.fluid_type,
})?;
```
The `supply_temp_c` is the hot water *leaving* the tank (outlet), and `return_temp_c` is the *cold mains* entering the tank. This is backwards from the hydronic loop convention where `supply_temp_c` is the hot fluid *entering* the zone equipment and `return_temp_c` is the cooler fluid returning to the heat source.

**Code Location**: `crates/hares-equipment/src/water_heater/resistance.rs:541-547`, `heat_pump_wh.rs:794-800`, `gas.rs:511-517`

**Root Cause**: The DHW system is a once-through consumption loop (mains → tank → fixture), not a recirculating hydronic loop. The port convention was designed for recirculating loops where supply goes to the zone and return comes back. In the DHW context, the "supply" is actually the tank outlet (consumed flow) and the "return" is the cold mains inlet (no return path). The semantic mismatch is not a computational bug (the fluid solver just computes flow-weighted averages and power), but it means the fluid solver's `mean_supply_temp_c` and `mean_return_temp_c` for a DHW loop represent mixed outlet temperature and mixed inlet temperature rather than supply/return in the hydronic sense.

**Impact**: The fluid loop state for DHW loops has unclear semantics. The `net_power_w` computed by the solver for a DHW loop represents `cp × flow × (outlet - mains)`, which is the useful thermal energy delivered to the draw — but this is already tracked by the water heater's own telemetry. The DHW fluid port contributes duplicate information that could be misinterpreted if consumed by another piece of equipment expecting hydronic supply/return semantics.

---

### Finding 5: [Severity: medium] — No hydronic loop consumer model exists; boiler writes directly to zone thermal ports

**Description**: The boiler contributes to a fluid loop via `PortContribution::Fluid`, but simultaneously writes heat directly to zone thermal accumulators through `self.hvac.write_zone_thermal_contributions(...)` (boiler.rs:249). The boiler reads its return temperature from the fluid domain update via `loop_return_temp_c` (boiler.rs:695-707), creating a feedback loop. However, there is no separate hydronic baseboard/radiator equipment that consumes from the fluid loop. The `ElectricBaseboard` model has no fluid port (only electrical + thermal).

**Code Location**:
- `crates/hares-equipment/src/hvac/boiler.rs:249-254` — direct zone thermal contribution
- `crates/hares-equipment/src/hvac/boiler.rs:695-707` — reads return temp from fluid domain
- `crates/hares-equipment/src/hvac/baseboard.rs:79-82` — no fluid port declared

**Root Cause**: The boiler is implemented as a hybrid: it writes both to the fluid loop (for future hydronic integration) and directly to zone thermal accumulators (for current functionality). The fluid loop effectively carries monitoring/consistency data but is not the primary heat delivery path. EnergyPlus, by contrast, uses a dedicated supply-side/demand-side plant loop architecture where equipment on the supply side (boiler) feeds equipment on the demand side (baseboard coils) exclusively through the plant loop.

**Impact**: The fluid loop wiring is architecturally incomplete. The boiler's fluid contribution is redundant with its thermal contribution for current simulations. Future integration of true hydronic distribution (radiators, fan-coils, radiant floors) would require a consumer model that reads the fluid loop and emits zone thermal gains. This is a design gap rather than a bug.

---

### Finding 6: [Severity: low] — Wet appliance DHW demand uses supply_temp_c=0.0, return_temp_c=0.0

**Description**: Wet appliances (clothes washers, dishwashers) emit a `PortContribution::Fluid` on `DHW_DEMAND_LOOP` with `supply_temp_c: 0.0` and `return_temp_c: 0.0` (event_load.rs:927-933):
```rust
ports.accumulate(&PortContribution::Fluid {
    loop_id: crate::water_heater::DHW_DEMAND_LOOP,
    flow_rate_kg_s: self.hot_water_draw_rate_kg_s * self.load_fraction.max(0.0),
    supply_temp_c: 0.0,
    return_temp_c: 0.0,
    fluid_type: FluidType::Water,
})?;
```
The water heater reads only `total_flow_kg_s` from this accumulator (`read_dhw_demand_kg_s` at mod.rs:35-41) and ignores the zeroed temperatures, so there is no runtime effect. However, the zero temperatures pass through `FluidAccumulator::add` and affect the flow-weighted averages if any other equipment reads that accumulator.

**Code Location**: `crates/hares-equipment/src/event_load.rs:927-933`

**Root Cause**: The `supply_temp_c` and `return_temp_c` fields are unused for DHW demand signaling, but they still participate in the flow-weighted averaging inside `FluidAccumulator::add`. Since cold mains temperature (~10-15°C) rather than 0°C is used by the water heater tank model, the zero values in the accumulator are harmless but semantically misleading.

**Impact**: No current functional impact (temperatures are ignored by the consumer). If another piece of equipment were to read the accumulator temperatures for DHW_DEMAND_LOOP, it would see artificially low values.

---

### Finding 7: [Severity: low] — Fluid type is encoded/decoded in cross-domain payload but never used for property lookups

**Description**: The `FluidDomainPayload` encoding scheme (fluid.rs:21-36) carefully encodes `fluid_type` as a discriminator value per loop. `FluidLoopState` carries `fluid_type` (fluid.rs:11). However, the boiler's `loop_return_temp_c` function (boiler.rs:695-707) reads only `mean_return_temp_c` from the loop state, ignoring `fluid_type`. The thermal solver and other downstream consumers also ignore `fluid_type`.

**Code Location**:
- `crates/hares-types/src/fluid.rs:9-15` — `FluidLoopState` carries `fluid_type`
- `crates/hares-types/src/fluid.rs:63-68` — `fluid_type_to_f64` encodes it
- `crates/hares-equipment/src/hvac/boiler.rs:695-707` — `loop_return_temp_c` ignores it

**Root Cause**: Architectural layering: `fluid_type` is correctly stored and propagated end-to-end, but property lookups (specific heat, density, viscosity) have not been implemented. The encoded discriminator is unused payload bytes at this stage.

**Impact**: No functional bug, but wasted payload bytes (1 slot per loop in the custom_payload encoding) and a risk that consumers assume `fluid_type` is validated/used when it is not.

---

## Summary
- **Total findings**: 7
- **Critical**: 0
- **High**: 2
- **Medium**: 3
- **Low**: 2

## Recommendations

1. **Add per-fluid-type specific heat to `FluidSolverConfig`** (Finding 1). Replace the single `cp_water_j_kg_k` with a `HashMap<FluidType, f64>` or a function that maps `FluidType` to its cp. Apply the correct cp in the `resolve` method based on each loop's fluid_type. EnergyPlus demonstrates the correct pattern: `glycol->getSpecificHeat()` per loop.

2. **Fix boiler supply temperature calculation for non-water fluids** (Finding 2). Use `self.fluid_type` to select the correct specific heat value. Either add a `fn cp_j_kg_k(fluid_type: FluidType) -> f64` helper or store the cp on the boiler struct at init time.

3. **Strengthen fluid type validation at wiring time** (Finding 3). Pass the actual declared loops to `FluidSolver::new` from the solver builder rather than `&[]`. Additionally, add a dweller-level check that all equipment contributing to the same `loop_id` agree on `fluid_type`. Validate at init time before the first timestep.

4. **Document DHW port semantics** (Finding 4). Add module-level documentation clarifying that DHW fluid ports use `supply_temp_c` = tank outlet temperature and `return_temp_c` = mains inlet temperature. This is a once-through consumption loop, not a recirculating loop. Consider a dedicated `DhcpDemand` or `HotWaterDraw` variant of `PortContribution` to avoid semantic overloading.

5. **Implement hydronic consumer model or provide a roadmap** (Finding 5). For complete hydronic loop wiring, implement consumer-side models (hydronic baseboard, radiator, radiant floor) that read `FluidLoopState` from the environment and emit zone thermal contributions proportional to flow rate and temperature drop. Currently the boiler bypasses the fluid loop for zone heating.

6. **Set realistic default temperatures for wet appliance DHW demand** (Finding 6). Use `env.weather.mains_temp_c` as `return_temp_c` on the `DHW_DEMAND_LOOP` fluid contribution, or set both temperatures to a sentinel NaN so downstream code can detect them as uninitialized.

7. **Garbage-collect unused `fluid_type` or make it functional** (Finding 7). Either drop `fluid_type` from `FluidLoopState` and the encoding (saving space) or implement the property-lookup path that uses it. The current state — encoded but never consumed — is dead weight.

## References / Citations

- EnergyPlus plant loop fluid property lookup: `PlantManager.cc:2748-2749` and `Loop.cc:135-137` — demonstrates per-loop `glycol->getSpecificHeat()` pattern
- EnergyPlus plant equipment type registration: `DataPlant.hh:85-298` — shows all plant equipment types and their valid loop types
- OCHRE `WaterHeater.py:28-46` — water tank model initialization with configurable node count and tank parameters
- OCHRE `HeatPumpWaterHeater.py:642-676` — COP/capacity curve evaluation, zone heat extraction, and SHR decomposition
- OCHRE `Equipment.py` — ZIP load model for voltage-dependent power scaling (referenced by `WaterHeaterZip`)
