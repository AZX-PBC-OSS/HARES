# Tankless water heater model physics audit
**Review ID**: equip-wh-04
**Category**: equipment-wh
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/water_heater/tankless.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Equipment/WaterHeater.py (`TanklessWaterHeater`, `GasTanklessWaterHeater`, lines 727–806)
vendors/OCHRE/ochre/Models/Water.py (`IdealWaterModel`, lines 478–494; `_water_draw_general`, line 16)

## Findings

### Finding 1: [Severity: medium]
**Description**: No minimum flow activation threshold. Heating activates for any `total_draw_kg_s > 0.0`. Real tankless water heaters have a minimum flow rate (typically ~0.5 GPM ≈ 0.03 kg/s) below which the flow sensor does not trigger burner ignition. This causes the model to deliver heat for infinitesimally small draws, which can produce spurious micro-heating at negligible flows or trickle-through leakage.

**Code Location**: `tankless.rs:322` — the condition `if mode == OperatingMode::Heating && total_draw_kg_s > 0.0` gates heat delivery on *any* positive flow, with no lower bound.

**Root Cause**: The energy-balance equation `demand_w = total_draw_kg_s * Cp * deltaT * duty` is physically correct for any flow rate, but real hardware has a mechanical minimum flow sensor threshold below which the burner cannot sustain stable combustion and will not fire.

**Impact**: At simulation timesteps of 1–60 minutes, the error is negligible for normal draws, but could cause unrealistic heat delivery during near-zero flow conditions (e.g., fixture leak, pressure-equalization trickle, or timesteps where aggregate flow is near zero but not exactly zero). OCHRE has the same omission — its `update_internal_control` returns `"On" if self.heat_from_draw > 0` with no minimum threshold (WaterHeater.py:746).

### Finding 2: [Severity: medium]
**Description**: In the over-capacity branch, the outlet temperature calculation uses the immutable `self.rated_thermal_power_w` (full nameplate capacity) rather than the possibly-reduced `effective_max_w` when a `PowerLimit` control signal is active. This means that when the power limit reduces the effective capacity, the time-averaged thermal output correctly drops to `effective_max_w * duty`, but the reported outlet temperature is computed as if the burner still fires at full nameplate rating during its on-time — which is physically impossible when the burner's firing rate is actively limited.

**Code Location**: `tankless.rs:335-336`:
```rust
let outlet_c = inlet_temp_c
    + self.rated_thermal_power_w / (total_draw_kg_s * CP_LIQUID_WATER_J_KG_K);
```

**Root Cause**: The comment at lines 332–334 states *"the heater fires at rated capacity during its on-fraction regardless of duty or power-limit accounting"*, treating power limits and duty cycles identically. However, a duty cycle achieves time-averaged reduction by cycling full-on/full-off — during the on-phase the burner fires at rated power, so using `rated_thermal_power_w` for outlet temp is correct. A `PowerLimit` signal, by contrast, directly limits the firing rate of the burner, meaning the burner *never* reaches rated power. Using `rated_thermal_power_w` for outlet temperature when `PowerLimit` is active overstates the maximum achievable outlet temperature.

**Impact**: If a controller applies `PowerLimit { max_power_kw: 5.0 }` to a 20 kW gas tankless, the effective capacity drops to 5 kW. For a 0.2 kg/s draw with 30 K delta-T, demand (25 kW) exceeds effective capacity (5 kW). The time-averaged thermal output correctly reports ~5 kW, but the outlet temperature computed from `20_000 / (0.2 * 4184) = 23.9 K` rise (43.9 °C outlet) instead of the correct `5_000 / (0.2 * 4184) = 6.0 K` rise (26.0 °C outlet). The outlet telemetry is thus misleading during power-limited over-capacity events.

Note that OCHRE applies its `max_power` clip differently — it clips `heat_from_draw` (the demand side) before the capacity comparison, so the two implementations diverge in their over-capacity handling under power limits. In OCHRE, a post-clip demand below `capacity_rated` takes the "within capacity" path (outlet = setpoint), masking the issue. HARES clips the capacity side and the demand remains unclipped, making the discrepancy between `rated_thermal_power_w` and `effective_max_w` visible.

### Finding 3: [Severity: low]
**Description**: No heat exchanger thermal mass is modeled. The outlet temperature responds instantaneously to changes in flow rate — the model uses a purely algebraic energy balance `T_out = T_in + Q / (m_dot * Cp)`. Real tankless units have a heat-exchanger assembly (copper/stainless tubing, fin-and-tube) with non-negligible thermal mass, creating a first-order lag on outlet temperature when flow starts or stops. There is also a burner ignition delay (typically 1–3 seconds) during which the heat exchanger warms from ambient to operating temperature.

**Code Location**: The entire `step()` method (`tankless.rs:288-421`) uses only algebraic energy balance equations; there is no differential state variable for heat exchanger temperature, no time constant, and `dt` is explicitly discarded (line 310): `let _ = dt; // dt not used for tankless (on-demand model)`.

**Root Cause**: Deliberate simplification — a tankless model by definition has zero storage volume. OCHRE uses the same approach (the `IdealWaterModel` class is a 1-node tank with 1000 L volume and near-zero UA, with its state forced to setpoint every timestep, making it effectively instantaneous: WaterHeater.py:741).

**Impact**: During the first seconds of a draw event (sub-minute timescale), the model over-predicts outlet temperature because it ignores:
1. The cold slug of water already sitting in the heat exchanger at ambient temperature
2. The burner ignition and heat exchanger warm-up delay (1–3 seconds to reach steady-state outlet temperature)

At typical simulation timesteps of 1 minute or longer, this error is negligible. For sub-minute simulation (e.g., 10-second timesteps for demand response studies), the error could be meaningful for short draws like handwashing (~30 seconds). Adding a simple first-order lag time constant (e.g., `tau = C_hex / (m_dot * Cp)`) or a fixed warmup delay would improve fidelity without adding a tank model.

### Finding 4: [Severity: low]
**Description**: Efficiency is modeled as a single constant (`efficiency_factor`, derived from Energy Factor or Uniform Energy Factor). There is no efficiency curve expressed as a function of the current flow rate (fraction of rated flow). Real tankless units have flow-rate-dependent efficiency that typically peaks near 50–70% of rated flow and drops at both very low and very high flow rates due to combustion instability and heat-exchanger saturation effects respectively.

**Code Location**: `tankless.rs:67` (`efficiency_factor: f64`), `tankless.rs:347` (`let fuel_input_w = thermal_output_w / self.efficiency_factor;`). The efficiency is applied as a single scalar divisor to compute fuel input across all operating points.

**Root Cause**: OCHRE uses the same simplification — a constant `self.efficiency` parameter (WaterHeater.py:66). Neither model imports manufacturer-provided efficiency-vs-flow curves. The Energy Factor / UEF rating from HPXML/ASHRAE is a single number representing annual-average efficiency, and both models apply it uniformly.

**Impact**: For annual energy consumption estimates, using the EF/UEF value as a constant efficiency is standard practice and produces reasonable results. The model will slightly misestimate fuel consumption at extreme flow rates but the integrated energy error over typical usage patterns is small (<5%). For gas vs. electric comparison:
- **Gas tankless**: Real efficiency drops at low flow (incomplete combustion, higher standby losses as a fraction of output) and at very high flow (heat exchanger saturation). EF values for gas tankless are typically 0.80–0.96.
- **Electric tankless**: Efficiency is nearly constant (~0.98–0.99) across flow rates since there are no combustion losses, only minor ohmic heating losses in wiring and controls.

HARES correctly distinguishes these by allowing separate EF/UEF configuration per fuel type via `TanklessWaterHeaterConfig.fuel_type` and `energy_factor`/`uniform_energy_factor`.

### Finding 5: [Severity: low]
**Description**: No standby heat injection at zero flow — correct. When `total_draw_kg_s == 0`, thermal_output_w is set to `0.0` (line 341) and fuel_input_w is `0.0`. The only power draw is the gas ignition controller parasitic electric (for gas units), which is applied unconditionally whether the burner fires or not (line 370-378). This correctly models that a tankless unit has no tank to keep warm and therefore zero standby thermal loss.

**Code Location**: `tankless.rs:339-341` (zero-flow thermal output), `tankless.rs:370-378` (gas parasitic electric).

**Impact**: This is physically correct and matches OCHRE behavior. No action needed.

### Finding 6: [Severity: low]
**Description**: Simultaneous draws are correctly handled by summing flow rates before computing single-pass heating. At line 319: `let total_draw_kg_s = schedule_draw_kg_s + appliance_demand_kg_s`. The `appliance_demand_kg_s` is read from the `DHW_DEMAND_LOOP` fluid port accumulator (line 318), which aggregates flow from all wet appliances (clothes washer, dishwasher, etc.).

**Code Location**: `tankless.rs:318-319`.

**Impact**: This is correct physics — a tankless unit sees one combined flow stream through its heat exchanger, regardless of how many downstream fixtures are open. Matches OCHRE approach where multiple draw types are summed (Water.py:287-292). No action needed.

### Finding 7: [Severity: low]
**Description**: HARES supports continuous `LoadFraction` control (0.0–1.0) for tankless units via `ctrl_load_fraction` (line 296) and `dr_load_fraction`, which multiply into the duty calculation (`duty = duty_cycle * dr_load_fraction * ctrl_load_fraction`). OCHRE's `TanklessWaterHeater` raises an exception for non-0/1 load fractions (WaterHeater.py:132-133). This is an enhancement over OCHRE, but introduces subtle behavior: when duty < 1.0 in the within-capacity regime, the thermal output equals `demand_w * duty` (line 324), which means the heater delivers partial heat to the full flow — physically, this would mean the outlet temperature is below setpoint, but the model reports `setpoint_c` as the outlet (line 330). This is inconsistent: if time-averaged output is reduced, the outlet cannot simultaneously be at setpoint for the full flow.

**Code Location**: `tankless.rs:328-330` — within-capacity branch reports outlet = setpoint regardless of duty scaling.

**Root Cause**: The within-capacity branch applies duty to demand but always reports setpoint. The intended semantics are: "If demand_w * duty ≤ capacity_w * duty, the unit can meet the load during its on-time and the outlet averages to setpoint." But this interpretation assumes heater cycling (full-on/full-off duty cycle), where the duty-scaled demand still represents how much energy is delivered per timestep. The outlet temperature as telemetry then reflects the *average* outlet, which is a reasonable modeling choice for time-averaged output.

**Impact**: The telemetry reports `OUTLET_TEMP_C` = setpoint even when the heater is operating at reduced duty. Consumers of telemetry should be aware that this is a time-averaged value, not instantaneous. No functional impact on energy accounting.

## Summary
- Total findings: 7
- Critical: 0 / High: 0 / Medium: 2 / Low: 5

| # | Severity | Area |
|---|----------|------|
| 1 | Medium | No minimum flow activation threshold |
| 2 | Medium | Over-capacity outlet temperature ignores PowerLimit on rated power |
| 3 | Low | No heat exchanger thermal mass (instantaneous response) |
| 4 | Low | Constant efficiency (no flow-rate-dependent curve) |
| 5 | Low | Correct: zero standby heat injection |
| 6 | Low | Correct: simultaneous draws summed before heating |
| 7 | Low | Within-capacity outlet temp reports setpoint despite duty < 1.0 |

## Recommendations
1. **Add a configurable minimum flow activation threshold** (`min_flow_kg_s` or `min_flow_gpm`) defaulting to 0.0 to maintain backward compatibility. Users simulating sub-hourly timesteps with realistic hardware constraints can set this to ~0.03 kg/s (0.5 GPM), matching typical tankless flow sensor specifications.

2. **Fix the over-capacity outlet temperature to use `effective_max_w` instead of `rated_thermal_power_w`** when a `PowerLimit` is active. The outlet temperature formula at line 335 should use `effective_max_w` (the smaller of `rated_thermal_power_w` and the power limit), not the unconditioned `rated_thermal_power_w`. Duty cycle remains correctly handled since duty is a time-averaging factor, not a firing-rate limit. Specifically:

   ```rust
   // Replace line 335-336:
   let outlet_c = inlet_temp_c
       + self.rated_thermal_power_w / (total_draw_kg_s * CP_LIQUID_WATER_J_KG_K);
   // With:
   let outlet_c = inlet_temp_c
       + effective_max_w / (total_draw_kg_s * CP_LIQUID_WATER_J_KG_K);
   ```

   Or, if distinguishing duty cycle from PowerLimit: compute outlet from `effective_max_w` when `power_limit_w` is active, and from `rated_thermal_power_w` when only duty cycle reductions are applied.

3. **Consider adding an optional first-order lag on outlet temperature** (e.g., a configurable `hex_time_constant_s` parameter defaulting to 0.0). This would improve fidelity for sub-minute timestep simulations where short-draw temperature dynamics matter. Implementation: track a `hex_temp_c` state variable updated as `dT_hex/dt = (T_steady_state - T_hex) / tau`, with `tau` computed from hex mass and flow rate, and report `hex_temp_c` as outlet instead of the algebraic value.

4. **Consider adding a flow-rate-dependent efficiency curve** as an optional configuration (e.g., a `flow_efficiency_curve` that maps flow-rate fraction to efficiency multiplier). This would improve fidelity for both gas and electric tankless units. Default to constant efficiency (current behavior) to maintain backward compatibility.

5. **Document the telemetry semantics** for `OUTLET_TEMP_C` when duty < 1.0: clarify whether it reports time-averaged or instantaneous outlet temperature. The current behavior (reporting setpoint when within capacity with duty < 1.0) implies time-averaged semantics, which should be documented in telemetry field descriptions.

## References / Citations
- OCHRE TanklessWaterHeater: `vendors/OCHRE/ochre/Equipment/WaterHeater.py`, lines 727–772
- OCHRE GasTanklessWaterHeater: `vendors/OCHRE/ochre/Equipment/WaterHeater.py`, lines 782–806
- OCHRE IdealWaterModel: `vendors/OCHRE/ochre/Models/Water.py`, lines 478–494
- OCHRE water draw calculation: `vendors/OCHRE/ochre/Models/Water.py`, lines 280–365
- ANSI/RESNET 301-2022 — Standard for the Calculation and Labeling of the Energy Performance of Dwelling and Sleeping Units using an Energy Rating Index (parasitic power on-time fraction table)
- ASHRAE Standard 118.2 — Method of Testing for Rating Residential Water Heaters (tankless efficiency measurement procedures)
- EnergyPlus Engineering Reference v9.6 — Water Heater Tank Model and Tankless Water Heater (instantaneous heating, no storage volume)
