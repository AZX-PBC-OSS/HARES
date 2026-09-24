# Generator fuel cell vs combustion path completeness and CHP routing
**Review ID**: dercat-09
**Category**: der-catalog
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/generator.rs

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/Generator.py` — OCHRE generator, GasGenerator, GasFuelCell hierarchy
- `vendors/OCHRE/ochre/Equipment/Equipment.py` — Base equipment thermal zone infrastructure
- `vendors/EnergyPlus/src/EnergyPlus/ICEngineElectricGenerator.hh` / `.cc` — IC engine CHP model with multi-stream heat recovery
- `vendors/EnergyPlus/src/EnergyPlus/FuelCellElectricGenerator.hh` / `.cc` — SOFC fuel cell with inverter, stack cooler, exhaust HX
- `vendors/EnergyPlus/src/EnergyPlus/DataGenerators.hh` — Shared generator enums and data structures
- `vendors/EnergyPlus/src/EnergyPlus/GeneratorFuelSupply.hh` / `.cc` — Fuel chemistry (LHV, HHV, gas composition)
- `vendors/EnergyPlus/src/EnergyPlus/GeneratorDynamicsManager.hh` / `.cc` — Dynamic state management (warm-up, cool-down, fuel ramp)

## Findings

### Finding 1: Fuel cell and combustion share identical physics — no electrochemical or inverter modeling
**Severity: high**

**Description**: HARES models `FuelCell` and `GasGenerator` as the same `Generator` struct with identical physics. The only difference is the default efficiency type (`"curve"` vs `"constant"`) and the equipment-type label string. There is no DC intermediate, no inverter model (DC-to-AC conversion), no electrochemical stack model, no stack cooler, and no gas thermochemistry. EnergyPlus models fuel cells with seven subsystems (power module, air supply, fuel supply, water supply, aux heat, exhaust HX, stack cooler, inverter, electrical storage) and solves chemical equilibrium via Shomate/NASA polynomial equations. OCHRE similarly differentiates `GasFuelCell` only by efficiency curve default, but HARES inherits this simplicity without adding the physics detail expected by the review rubric.

**Code Location**: `generator.rs:406-427` (GeneratorKind enum with only `equipment_type` and `default_efficiency_type` differing); `generator.rs:959-968` (GasGenerator and FuelCell newtypes delegating to the same `Generator` core)

**Root Cause**: The design note at lines 1-5 states "Both types share identical physics." Fuel cell specifics (DC stack output, inverter efficiency, stack heat at 60–80°C) are entirely absent despite being called out explicitly in the review rubric.

**Impact**: Users expecting a fuel cell model get a gas generator with a different label. DC-to-AC conversion losses (typically 3–10%) are not modeled. Stack waste heat cannot be distinguished from engine waste heat. This limits the model's usefulness for comparing fuel cell vs. combustion CHP performance in residential energy system design.

---

### Finding 2: Single lumped `eta_thermal` replaces multi-stream waste heat recovery with different temperatures
**Severity: high**

**Description**: HARES uses a single `eta_thermal` parameter (line 48) to recover all waste heat as a constant fraction of fuel input: `q_thermal_w = fuel_w * eta_thermal` (line 757). EnergyPlus separates IC engine waste heat into three streams with distinct temperatures and individual PLR-dependent curves:
- Jacket water heat (`RecJacHeattoFuelCurve`, ~90°C)
- Lube oil heat (`RecLubeHeattoFuelCurve`)
- Exhaust heat (`TotExhausttoFuelCurve`, ~400–500°C, with NTU-effectiveness HX model)

Each stream has a different recoverable fraction and temperature quality. EnergyPlus also uses NTU-effectiveness for exhaust HX (`UA = UACoef(1) * RatedPower^UACoef(2)`, lines 640–649 of `ICEngineElectricGenerator.cc`) and scales all heat recovery streams proportionally when the loop outlet would exceed `HeatRecMaxTemp`. HARES has none of this.

**Code Location**: `generator.rs:48` (`eta_thermal: Option<f64>`); `generator.rs:757` (`q_thermal_w = fuel_w * self.eta_thermal`)

**Root Cause**: The design collapses all waste heat sources into a single efficiency parameter with no temperature-qualified heat streams and no HX effectiveness modeling.

**Impact**: Cannot differentiate between high-grade heat (exhaust, usable for absorption chilling or high-temperature hydronic) and low-grade heat (jacket/lube, usable only for DHW preheat or low-temp space heating). Cannot model the physical limit where a heat recovery loop reaches its maximum temperature and must reject additional available heat. This is a significant simplification versus EnergyPlus, though it is an improvement over OCHRE (which computes `power_chp` but never connects it to anything).

---

### Finding 3: No heat recovery temperature capping or feedback from thermal loop saturation
**Severity: medium**

**Description**: HARES delivers thermal energy to the fluid port as a declared flow rate and supply/return temperatures (lines 806–816), but the `q_thermal_w` computed at line 757 is not passed to the fluid loop. The fluid port carries only `flow_rate_kg_s`, `supply_temp_c`, `return_temp_c`, and `fluid_type` — the hydronic loop must independently determine if it can absorb the heat. There is no mechanism for the generator to reduce its thermal output when the thermal load is saturated. EnergyPlus computes an `HRecRatio` (line 782 of `ICEngineElectricGenerator.cc`) that scales down all heat recovery streams when `HeatRecMdot < MinHeatRecMdot` (i.e., the loop cannot absorb all heat without exceeding its max temperature setpoint). HARES has no analog to this feedback.

**Code Location**: `generator.rs:806-816` (fluid port contribution with temperatures but no `q_thermal_w`); `generator.rs:757` (q_thermal computed but not delivered to any port in energy terms)

**Root Cause**: The fluid port abstraction carries flow and temperatures but not thermal power. The loop is expected to compute its own energy balance, but the generator provides no information about how much heat is actually available for recovery.

**Impact**: In a saturated thermal loop scenario, the generator would continue reporting `q_thermal_w` telemetry even though the loop cannot absorb that energy. The system-level energy balance may diverge if the loop rejects some fraction of the generator's declared thermal output.

---

### Finding 4: Fluid port energy routing does not carry `q_thermal_w` quantitatively
**Severity: medium**

**Description**: When CHP is active with a fluid port, `q_thermal_w` is reported to telemetry (line 831) but is NOT written to the fluid port. The fluid port gets only flow/temperature metadata (line 808–815). The zone port receives `q_flue_w` as a thermal contribution. This means the energy conservation equation `fuel_w = electrical_w + q_thermal_w + q_flue_w` holds only within the generator's internal accounting but not necessarily at the system level, because `q_thermal_w` is never deposited into any accumulator. The fluid port must compute `flow_rate * Cp * (T_supply - T_return)` independently, and this may or may not equal `q_thermal_w`.

**Code Location**: `generator.rs:757` (`q_thermal_w` computed), `generator.rs:806-816` (fluid port gets only temperature/flow), `generator.rs:793-803` (zone port gets `zone_heat_w` which is either `q_flue_w` or `q_thermal_w + q_flue_w`)

**Root Cause**: The `PortContribution::Fluid` variant does not carry a `thermal_power_w` field that would allow the generator to transfer energy to the loop model.

**Impact**: No downstream consumer can validate that the total energy delivered by the generator (electric + thermal + flue) equals the fuel energy. This breaks the chain of energy conservation that the reviewer explicitly requires.

---

### Finding 5: Constant supply/return temperatures ignore load dependence
**Severity: medium**

**Description**: `supply_temp_c` (default 70°C, line 211) and `return_temp_c` (default 60°C, line 214) are static configuration values, not functions of generator load. In real engines, exhaust and coolant temperatures vary significantly with part-load ratio. EnergyPlus models exhaust temperature as a curve function of PLR (`ExhaustTempCurve`) and computes stack outlet temperature from HX effectiveness. The static 10°C ΔT implies a constant `Q = mdot * Cp * ΔT` that may not match `q_thermal_w` at different loads.

**Code Location**: `generator.rs:469-470` (`supply_temp_c` and `return_temp_c` fields); `generator.rs:211-214` (defaults)

**Root Cause**: The model treats fluid temperatures as user inputs rather than physics outputs derived from engine thermodynamics.

**Impact**: At part load, a real engine would deliver lower-temperature coolant heat, affecting whether it can meet a given thermal demand. HARES always pretends the supply temperature is 70°C regardless of electrical output, which may overestimate the quality of recoverable heat at part load.

---

### Finding 6: `capacity_min_kw` clamps UP below threshold rather than shutting down
**Severity: low**

**Description**: When the raw target power is between 0 and `capacity_min_kw`, HARES clamps it UP to `capacity_min_kw` (lines 628–633). This means a request for 1 kW with `capacity_min_kw = 3 kW` results in 3 kW of generation — 2 kW excess. EnergyPlus clamps the part-load ratio (PLR) itself between `MinPartLoadRat` and `MaxPartLoadRat` and recomputes electric power from the clamped PLR (lines 571–577 of `ICEngineElectricGenerator.cc`). This means EnergyPlus constrains the operating point to the valid range; it does NOT inject extra power. In self-consumption mode, a small net load would trigger 3 kW of generation when only 1 kW is needed, causing 2 kW of unrequested export. This matches OCHRE's `get_power_limits` behavior but differs from EnergyPlus and may surprise users.

**Code Location**: `generator.rs:628-633` (capacity_min clamping logic)

**Root Cause**: Deliberate design choice to match OCHRE semantics where `capacity_min` is interpreted as a minimum operating power (clamp up) rather than a minimum-viable PLR (forbid operation below).

**Impact**: In islanded/self-consumption mode, the generator may over-produce when load is slightly above zero. Users must set `capacity_min_kw` carefully, knowing it will cause overproduction rather than shutdown at low loads.

---

### Finding 7: `eta_thermal` is constant — no load-dependent heat recovery fraction
**Severity: low**

**Description**: `eta_thermal` is a constant scalar applied uniformly at all loads (line 757). In EnergyPlus, jacket water recovery (`RecJacHeattoFuelCurve`), lube oil recovery (`RecLubeHeattoFuelCurve`), and exhaust energy fraction (`TotExhausttoFuelCurve`) are all curve functions of PLR. At low loads, real engines produce proportionally MORE waste heat (lower electrical efficiency) but a different fraction of that waste is recoverable (exhaust temperature drops, jacket coolant temperature falls). HARES cannot capture this effect with a constant `eta_thermal`.

**Code Location**: `generator.rs:757` (`q_thermal_w = fuel_w * self.eta_thermal` with constant multiplier)

**Root Cause**: The model uses a single scalar where EnergyPlus uses multiple PLR-dependent curves.

**Impact**: Thermal output at part load is modeled as a fixed fraction of fuel input, overestimating recovered heat at light loads (where real engines have lower exhaust temperatures and less recoverable heat) and underestimating it at high loads.

---

### Finding 8: `core_output` reset to default on `load_state` loses state visibility
**Severity: low**

**Description**: In `load_state()` (line 903), `self.core_output = CoreOutput::default()` sets all flows (electric, fuel, thermal) to zero/none. Telemetry is correctly recomputed at lines 884–901, so `telemetry()` returns valid data. However, any system that reads `core_output()` between `load_state()` and the next `step()` will see zero flows. This is not a correctness bug (the next step will repopulate `core_output`) but is a minor inconsistency: the generator reports valid telemetry but zero core output after restore.

**Code Location**: `generator.rs:903` (`self.core_output = CoreOutput::default()`)

**Root Cause**: `CoreOutput` is not reconstructed from restored state in the same way telemetry is.

**Impact**: If energy accounting or balancing code reads `core_output` after a state restore but before the first timestep, it would undercount generation and fuel consumption for the reporting interval. This is mitigated if the outer simulation always calls `step()` before reading `core_output()`.

---

## Summary
- **Total findings**: 8
- **Critical**: 0
- **High**: 2 (fuel cell/combustion identical physics; single lumped thermal recovery)
- **Medium**: 3 (no heat recovery temperature cap; fluid port missing q_thermal_w; static supply/return temperatures)
- **Low**: 3 (capacity_min clamps up; constant eta_thermal; core_output reset on load_state)

## Recommendations

1. **Differentiate fuel cell physics from combustion**: Add a FuelCell-specific model with at minimum: DC stack power, inverter efficiency (constant or quadratic), and stack cooling heat routed to a separate low-temperature thermal port (~60–80°C). This aligns with the review rubric's explicit requirement for "electrochemical conversion with DC output, inverter to AC, waste heat captured from stack."

2. **Split waste heat into temperature-qualified streams**: Replace single `eta_thermal` with separate recoverable fractions for jacket water (~90°C) and exhaust (~400–500°C), each as PLR-dependent curves (matching EnergyPlus's `RecJacHeattoFuelCurve` and `TotExhausttoFuelCurve`). This enables correct routing of high-grade vs. low-grade heat to different thermal loads (DHW preheat vs. hydronic space heating).

3. **Add heat exchanger effectiveness modeling**: Model exhaust-to-water heat recovery with an NTU-effectiveness approach (as EnergyPlus does at `ICEngineElectricGenerator.cc:639–649`). Add a `HeatRecMaxTemp` parameter that caps the loop outlet temperature and scales down heat recovery when the loop is saturated (`HRecRatio`).

4. **Carry `thermal_power_w` in the fluid port contribution**: Add a power field to `PortContribution::Fluid` so the generator can declare the energy it is delivering to the loop, enabling system-level energy balance verification.

5. **Make supply/return temperatures load-dependent**: Derive `supply_temp_c` and `return_temp_c` from the generator's current electrical output and exhaust/coolant temperature curves, rather than treating them as static user inputs.

6. **Reconstruct `core_output` on `load_state`**: Compute `CoreOutput` from restored state (electric_kw, fuel_w, q_thermal_w) in the same way `load_state` already reconstructs telemetry.

## References / Citations

- **OCHRE Generator.py**, lines 128–138: `get_power_limits()` defines `capacity_min` semantics that HARES follows for clamping-up below minimum
- **OCHRE Generator.py**, lines 148–171: `calculate_efficiency()` — three efficiency types (constant, curve, quadratic)
- **OCHRE Generator.py**, lines 187–201: `calculate_power_and_heat()` — `power_chp = power_input * efficiency_chp` but NEVER routed; `sensible_gain` computed but `zone_name=None` prevents routing
- **EnergyPlus ICEngineElectricGenerator.cc**, lines 571–577: PLR clamping between `MinPartLoadRat` and `MaxPartLoadRat`
- **EnergyPlus ICEngineElectricGenerator.cc**, lines 596–649: Jacket, lube oil, and exhaust heat recovery with PLR-dependent curves and NTU-effectiveness exhaust HX
- **EnergyPlus ICEngineElectricGenerator.cc**, lines 729–793: `CalcICEngineGenHeatRecovery` — temperature capping with `HRecRatio`
- **EnergyPlus FuelCellElectricGenerator.cc**, lines 1691–1711: DC power efficiency with degradation factors and fuel flow from `Pel / (Eel * LHV)`
- **EnergyPlus FuelCellElectricGenerator.cc**, lines 2104–2124: Inverter model (constant or quadratic efficiency), DC-to-AC with losses recovered as intake air heat
- **EnergyPlus FuelCellElectricGenerator.cc**, lines 1859–1865: Stack cooler polynomial `qs_cool = f(Tstack, Pel)`
- **EnergyPlus FuelCellElectricGenerator.cc**, lines 2981–3231: Exhaust HX with four methods (fixed effectiveness, empirical UA, fundamental UA, condensing)
