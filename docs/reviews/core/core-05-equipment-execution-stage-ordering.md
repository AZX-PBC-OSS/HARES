# Equipment execution stage ordering within time step
**Review ID**: core-05
**Category**: core
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-core/src/dwelling/mod.rs` — `run_timestep()` and `compute_equipment_execution_order()` drive the per-step equipment execution sequence.
- `crates/hares-core/src/dwelling/conversions.rs` — `stage_rank()` defines the numeric ordering of `ExecutionStage` variants.
- `crates/hares-types/src/equipment.rs:164-170` — `ExecutionStage` enum declaration with four variants.
- `crates/hares-equipment/src/registry.rs` — `EquipmentRegistry` with no stage validation.
- `crates/hares-core/tests/equipment_ordering_tests.rs` — Integration tests for the stage rank chain.

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/HVACManager.cc` — `ManageHVAC()`, `SimHVAC()`, and `SimSelectedEquipment()` define the HVAC calling-tree order: zone equipment → air loop → plant loop, with iterative flag-based re-simulation.

## Findings

### Finding 1: Thermal equipment `.step()` executes before Independent-stage equipment (PV, scheduled loads)
**Severity**: high

**Description**: In `run_timestep()` (`crates/hares-core/src/dwelling/mod.rs:2412-2511`), thermal-stage equipment (HVAC, water heaters, ventilation) has its `.step()` called in **Step 3a** (line 2422), while Independent-stage equipment (PV, scheduled loads, event-based loads) and Electrical-stage equipment (battery, EV, generators) are stepped later in **Step 3b** (line 2471). This means PV generation is computed *after* HVAC has already committed its heating/cooling output to the ports for the current time step.

**Code Location**:
- Step 3a — Thermal equipment step: `crates/hares-core/src/dwelling/mod.rs:2422-2455`
- Step 3b — Non-thermal equipment step: `crates/hares-core/src/dwelling/mod.rs:2471-2503`
- `compute_equipment_execution_order` sorts by `stage_rank`: `crates/hares-core/src/dwelling/mod.rs:497-501`
- `stage_rank` ranking: `crates/hares-core/src/dwelling/conversions.rs:461-468`

**Root Cause**: The `run_timestep()` method makes two separate passes through the `equipment_execution_order` sorted list: the first pass calls `.step()` only on Thermal equipment, the second pass calls `.update_control()` + `.step()` on non-Thermal equipment (Independent then Electrical, respecting the `stage_rank` order). The declared `stage_rank` ordering (`Independent=0 < Electrical=1 < Thermal=2`) is therefore **only followed within the non-thermal pass**, not across the full equipment `.step()` sequence. The actual `.step()` execution order is:

```
Thermal (rank 2) → Independent (rank 0) → Electrical (rank 1)
```

This is the reverse of the dependency chain described in the `stage_rank` documentation and in `equipment_ordering_tests.rs:5-8`.

**Impact**: HVAC equipment's `update_control()` (which runs pre-step at line 2327 and post-dispatch at line 2403) and `.step()` (line 2430) execute without knowledge of current time-step PV generation. While the thermal solver's `integrate()` (Step 4, line 2553) runs after all equipment and therefore sees updated ports, the HVAC equipment's capacity decisions are made blind to available solar generation. For most thermostatically-controlled HVAC, this is tolerable due to thermal inertia, but for use cases where HVAC dispatch strategies depend on real-time PV availability (e.g., pre-cooling when solar surplus is high, heat pump water heater timing), the lag is one full time step.

### Finding 2: Stage assignment is hardcoded per equipment type with no configuration override
**Severity**: medium

**Description**: Every equipment type assigns its `ExecutionStage` as a hardcoded constant in its constructor. For example:
- `PV` → `ExecutionStage::Independent` (`crates/hares-equipment/src/pv/mod.rs:147`)
- `Battery` → `ExecutionStage::Electrical` (`crates/hares-equipment/src/battery/mod.rs:387`)
- `EV` → `ExecutionStage::Electrical` (`crates/hares-equipment/src/ev/mod.rs:133`)
- All HVAC types → `ExecutionStage::Thermal` (e.g., `crates/hares-equipment/src/hvac/furnace.rs:92`)
- Water heaters → `ExecutionStage::Thermal` (e.g., `crates/hares-equipment/src/water_heater/gas.rs:139`)

There is no mechanism for a user or configuration file to adjust the stage assignment. If a downstream application needed to run a specific equipment in a different stage (e.g., treating a heat pump water heater as Electrical for dispatch purposes), there is no path to do so.

**Code Location**:
- `ExecutionStage` enum: `crates/hares-types/src/equipment.rs:164-170`
- Registry comment acknowledging caller responsibility: `crates/hares-equipment/src/registry.rs:92`

**Root Cause**: The `ExecutionStage` is a field on `EquipmentDescriptor`, set at construction time. The `Equipment::descriptor()` method returns `&EquipmentDescriptor`, which is immutable after construction. The `EquipmentRegistry` merely maps class names to factory closures; it performs no stage validation.

**Impact**: The fixed stage assignment prevents custom equipment ordering without modifying the equipment type's source code or the `run_timestep()` pass structure. Combined with Finding 1, this means there is no way to make PV run before HVAC at the `.step()` level without refactoring the time step loop.

### Finding 3: Registry has no stage-assignment validation
**Severity**: low

**Description**: The `EquipmentRegistry` (`crates/hares-equipment/src/registry.rs`) maps OCHRE class strings to factory closures but performs no validation of the `ExecutionStage` that each registered equipment type declares. The comment at line 92 explicitly states: "Stage-assignment validation is caller responsibility in v1." This means a custom equipment type registered by a downstream consumer could assign any stage, potentially breaking the dependency chain if, for example, a thermal-load-creating device were registered as `Independent`.

**Code Location**: `crates/hares-equipment/src/registry.rs:90-96`

**Root Cause**: The registry is designed as an open extension point with minimal invariants enforced. No `stage` field is associated with the registration entry — the stage is only read from the constructed `Equipment` instance at runtime.

**Impact**: Low in practice because all built-in equipment types hardcode their stages correctly and the `CANONICAL_EQUIPMENT_NAMES` test suite (`all_built_in_equipment_types_are_registered`) verifies registration completeness. A mis-assigned stage by a custom equipment would only affect ordering within that equipment's relative position in the sorted list, not across the Thermal/non-Thermal divide (since Step 3a and Step 3b filter by stage explicitly).

### Finding 4: Compare/contrast — EnergyPlus uses iterative convergence; HARES uses single linear pass
**Severity**: low

**Description**: EnergyPlus's `SimSelectedEquipment()` (`vendors/EnergyPlus/src/EnergyPlus/HVACManager.cc:1774-1920`) uses a flag-based iteration loop:

```
First pass: AirLoops → ZoneEquipment → NonZoneEquipment → ElectricPower → PlantLoops → ElectricPower
Iteration loop: while (SimAirLoops || SimZoneEquipment || ...) { AirLoops → ZoneEquipment → ... → PlantLoops }
```

EnergyPlus iterates until convergence or `MaxIter`, with each loop manager setting its simulation flag to `false` when converged. This means equipment dependencies that cross loop boundaries (e.g., a plant loop component that affects zone conditions) are resolved through re-simulation rather than enforced by a single-pass ordering.

HARES, by contrast, uses a single linear pass per time step with no intra-step iteration across equipment boundaries. Dependencies are resolved through:
1. The thermal solver's `prepare_inputs` / `integrate` running at the boundaries of the equipment pass
2. `update_control()` being called pre- and post-dispatch for thermal equipment
3. The `solver_feedback_actor` bridging thermal solver targets to equipment dispatch

**Code Location**:
- EnergyPlus iterative loop: `vendors/EnergyPlus/src/EnergyPlus/HVACManager.cc:862-919`
- HARES linear pass: `crates/hares-core/src/dwelling/mod.rs:2209-2557`

**Impact**: The single-pass approach is simpler and faster but relies entirely on the correctness of stage ordering. If stage ordering is wrong, there is no iterative fallback to correct for stale dependencies. This makes Finding 1 more consequential than it would be in an iterative architecture.

## Summary
- Total findings: 4
- Critical: 0 / High: 1 / Medium: 1 / Low: 2

## Recommendations

1. **Reorder equipment `.step()` calls so Independent (PV, scheduled loads) and Electrical (battery, EV) run before Thermal (HVAC, water heaters).** Move the thermal equipment `.step()` from Step 3a to after the non-thermal pass in Step 3b, or merge both passes into a single loop that respects `stage_rank` order. Thermal equipment's `update_control()` should still run pre- and post-dispatch (as it does today), but the actual `.step()` that commits heating/cooling power to ports should occur after PV and battery have established the current step's generation and storage dispatch. This aligns with EnergyPlus's approach where plant loops (which include generation) resolve before final zone equipment application.

2. **Consider making `ExecutionStage` configurable via `EquipmentSpec` parameters**, so downstream users can override the default stage for specific equipment (e.g., placing a heat pump water heater in the Electrical stage for dispatch purposes). If implemented, add registry-level validation that rejects obviously invalid stage assignments (e.g., a gas furnace in `Independent`).

3. **Add registry-level stage validation in v2**, as the comment at `crates/hares-equipment/src/registry.rs:92` anticipates. At minimum, ensure that each registered equipment type's declared stage is one of the four valid `ExecutionStage` variants. Consider adding a `stage` metadata field to the registry entry so validation can happen at registration time rather than at runtime.

4. **Consider an optional intra-step iteration flag** for high-fidelity scenarios where equipment interdependency cannot be linearized. Even a single re-pass with updated port state could eliminate the one-step PV lag for HVAC dispatch-aware control strategies without requiring a full EnergyPlus-style convergence loop.

## References / Citations
- `stage_rank()` function and its documented ordering: `crates/hares-core/src/dwelling/conversions.rs:461-468`
- `compute_equipment_execution_order()`: `crates/hares-core/src/dwelling/mod.rs:497-501`
- `equipment_ordering_tests.rs` confirming stage rank chain: `crates/hares-core/tests/equipment_ordering_tests.rs:5-8`
- EnergyPlus `SimSelectedEquipment` iterative ordering: `vendors/EnergyPlus/src/EnergyPlus/HVACManager.cc:1774-1920`, calling order at lines 1827-1843 and iteration loop at lines 1847-1918
- Equipment stage assignment examples: PV (`crates/hares-equipment/src/pv/mod.rs:147`), Battery (`crates/hares-equipment/src/battery/mod.rs:387`), Furnace (`crates/hares-equipment/src/hvac/furnace.rs:92`)
- Registry stage validation gap: `crates/hares-equipment/src/registry.rs:90-96`
