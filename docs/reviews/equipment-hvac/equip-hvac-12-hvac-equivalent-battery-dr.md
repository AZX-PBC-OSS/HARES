# HVAC equivalent battery model for demand response
**Review ID**: equip-hvac-12
**Category**: equipment-hvac
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/hvac/equivalent_battery.rs crates/hares-equipment/src/hvac/air_conditioner.rs crates/hares-equipment/src/hvac/heat_pump.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Equipment/HVAC.py

## Findings

### Finding 1: [Severity: critical]
**Description**: The equivalent battery model does not use the building's thermal capacitance to compute energy storage capacity. Instead it computes `max_energy_kwh` as the equipment power capacity multiplied by one timestep's worth of energy, which conflates equipment capacity with building thermal mass. A well-insulated 1000 m² warehouse and a small 80 m² apartment with the same HVAC unit would produce **identical** EBM parameters — there is no path for building capacitance to enter the model.

**Code Location**: `crates/hares-equipment/src/hvac/equivalent_battery.rs:54`

**Root Cause**: Line 54 computes `max_kwh = max_kw * time_res_s / 3600.0` — this is "energy deliverable in one timestep" rather than "energy stored in building mass across the thermostat bandwidth." The OCHRE reference (HVAC.py:620-641) computes `total_capacitance = convert(self.zone.capacitance, "kJ", "kWh")` from the building RC network and uses it as the scaling factor: `energy = total_capacitance * (zone.temperature - ref_temp) * hvac_mult`. The HARES `HvacEquipment` struct has no field for zone thermal capacitance, and `make_equivalent_battery_model` receives only `zone_temp_c` and `time_res_s` — it has no way to access the building's thermal mass.

**Impact**: During demand response optimization, the EBM will predict identical state-of-charge trajectories for buildings with wildly different thermal masses. A lightweight building that really stores only 15 minutes of cooling energy and a heavy-mass building that stores 6 hours will appear identical to the DR controller, leading to either unnecessary curtailment or thermal comfort violations.

### Finding 2: [Severity: high]
**Description**: The energy state (`energy_kwh`) is computed as a dimensionless deadband fill fraction times the one-timestep equipment energy, rather than tracking absolute zone temperature deviation. This makes the model dimensionally inconsistent with energy conservation and coupling to the envelope solver.

**Code Location**: `crates/hares-equipment/src/hvac/equivalent_battery.rs:55-56`

**Root Cause**: Lines 55-56 compute `fill = ((zone_temp_c - t_min) / deadband_range).clamp(0.0, 1.0)` followed by `energy_kwh = fill * max_kwh`. The fill fraction is a normalized 0-1 value that collapses to the same range regardless of building size. In OCHRE (HVAC.py:635), energy is `total_capacitance * (zone.temperature - ref_temp) * hvac_mult` — it directly tracks the absolute thermal energy stored in the building envelope relative to a fixed reference temperature (10°C for heating, 30°C for cooling). When a DR event pre-cools the zone by 2°C, the OCHRE EBM registers an energy change proportional to `2 * C` kWh; the HARES EBM registers only the proportional shift within the deadband, losing the absolute energy signal needed for energy-balance closure.

**Impact**: Changes in EBM "energy" during DR events are not physically meaningful. Shifting the timestep from 1 minute to 15 minutes changes `max_kwh` by 15x, which changes the reported energy state by 15x — even though the building's actual stored thermal energy is unchanged. This violates the core requirement that the EBM's energy balance matches actual thermal energy stored in the building mass.

### Finding 3: [Severity: high]
**Description**: The EBM efficiency is hardcoded to 1.0 and baseline power to 0.0 kW, meaning the model assumes the HVAC system converts electricity to stored thermal energy with 100% efficiency and requires zero power to maintain the current state. These are both physically incorrect for any real HVAC system.

**Code Location**: `crates/hares-equipment/src/hvac/equivalent_battery.rs:81-82`

**Root Cause**: Lines 81-82 set `efficiency: 1.0, baseline_power_kw: 0.0` as hardcoded constants with no connection to the equipment's actual EIR/COP. The OCHRE reference (HVAC.py:639-640) uses `1 / self.eir` for efficiency (the actual coefficient of performance) and `self.capacity_ideal` for baseline power (the power needed to hold setpoint, computed from the envelope solver). The HARES `HvacEquipment` struct stores `eir_by_stage` and the equipment's rated EIR is available, but neither value is used in `make_equivalent_battery_model`.

**Impact**: A DR optimization using this model would reach incorrect conclusions about the net energy cost of load-shifting. Pre-cooling a building by 2°C during cheap electricity and allowing it to float during peak hours appears costless (baseline = 0), but the actual system needs power to pump the pre-cooled energy into the building mass (COP ~3-5 for heat pumps, so 1 kWh electricity → 3-5 kWh thermal storage) and continuous power to offset envelope losses.

### Finding 4: [Severity: medium]
**Description**: The docstring reference to OCHRE source lines is incorrect. Line 8 states the function matches `Equipment.make_equivalent_battery_model()` at `Equipment.py:260-286`, but the function is defined at lines 620-641 and is invoked from `generate_results()` at line 602. Lines 260-286 cover the `update_external_control` method, not the equivalent battery model.

**Code Location**: `crates/hares-equipment/src/hvac/equivalent_battery.rs:8`

**Root Cause**: Line-number drift between codebase versions. The docstring was presumably written against an earlier version of OCHRE's HVAC.py.

**Impact**: Future maintainers cross-referencing the OCHRE source will find the wrong function. Low operational impact but signals that the HARES EBM implementation was not closely verified against the actual OCHRE reference after the initial port.

### Finding 5: [Severity: medium]
**Description**: The heating deadband boundaries in the HARES EBM do not account for `deadband_offset`, making the turn-on / turn-off thresholds differ from what the thermostat FSM actually uses. The FSM uses asymmetric thresholds (`deadband_offset = 0.2` by default), but the EBM assumes symmetric deadband placement.

**Code Location**: `crates/hares-equipment/src/hvac/equivalent_battery.rs:51-53` (heating), lines 65-67 (cooling)

**Root Cause**: The EBM computes the deadband range as `[setpoint - hysteresis, setpoint]` for heating and `[setpoint, setpoint + hysteresis]` for cooling — symmetric around the setpoint's extreme. The thermostat FSM (hvac_core.rs thermostat logic, documented in `thermostat.rs:19-30`) uses the OCHRE `deadband_offset = 0.2` convention: `turn_on = setpoint - hysteresis*(1-offset)`, `turn_off = setpoint + hysteresis*offset`. For heating with hysteris=1.0°C, the FSM turn-on is at `setpoint - 0.8°C` but the EBM places it at `setpoint - 1.0°C`. The energy state at any given zone temperature is therefore computed against the wrong boundary, producing a ~25% discrepancy in the fill fraction for heating mode.

**Impact**: The EBM's reported state-of-charge will not align with the thermostat's actual switching behavior during DR events. A zone at 20.5°C with a 21.0°C heating setpoint and 1.0°C hysteresis would be at 50% fill according to the FSM (it's about to turn off) but 62.5% according to the EBM (which thinks the band goes from 20.0°C to 21.0°C).

### Finding 6: [Severity: low]
**Description**: The EBM is fully implemented but not yet integrated into any equipment simulation loop. A search across the entire `crates/` directory finds no callers of `make_equivalent_battery_model`. The method is exported by `mod.rs:28` and `lib.rs:65` but never invoked, making it dead code that cannot yet fulfill its DR modeling purpose.

**Code Location**: `crates/hares-equipment/src/hvac/equivalent_battery.rs:36-84`

**Root Cause**: The function was authored as a porting target but integration into the `step()` or `update_control()` pipeline was deferred.

**Impact**: The model's correctness cannot be validated through integration tests or end-to-end simulation runs. Any DR controller that eventually calls this function will encounter the issues described in Findings 1-5 on first use.

## Summary
- Total findings: 6
- Critical: 1 / High: 2 / Medium: 2 / Low: 1

## Recommendations

1. **Add building thermal capacitance to the EBM** (Finding 1): Pass the zone's thermal capacitance (available from `hares_envelope::boundary_rc::derive_zone_capacitances` and stored in `SolverGroup.zone_capacitances_j_k`) into the EBM computation. Compute `max_energy_kwh` as `capacitance_kwh_per_K * deadband_range_C`, following OCHRE's `total_capacitance * (max_temp - min_temp)` pattern.

2. **Compute energy state from absolute temperature deviation** (Finding 2): Replace the dimensionless fill-fraction approach with `energy_kwh = capacitance_kwh_per_K * (zone_temp_c - ref_temp_c) * hvac_direction`, using a fixed reference temperature (OCHRE uses 10°C for heating, 30°C for cooling). This makes energy_kwh traceable to the envelope solver's state variables.

3. **Use actual equipment EIR/COP for efficiency and capacity for baseline** (Finding 3): Set `efficiency = 1.0 / eir` (or equivalently `COP`) from `self.config.eir_by_stage`. For the baseline, propagate `capacity_ideal` from the ideal-capacity solver path, or compute `baseline_power_kw = steady_state_capacity_w * eir / 1000` from the current operating point.

4. **Apply `deadband_offset` to EBM temperature bounds** (Finding 5): Use the same asymmetric threshold formulas as the thermostat FSM so that the EBM's state-of-charge aligns with actual equipment switching behavior.

5. **Fix the docstring line reference** (Finding 4): Update `Equipment.py:260-286` → `Equipment.py:620-641`.

6. **Integrate the EBM into the simulation loop** (Finding 6): Call `make_equivalent_battery_model` from `step()` in `air_conditioner.rs` (and the corresponding heating side) and surface the result in telemetry or `CoreOutput`, similar to how OCHRE calls it from `generate_results()` at HVAC.py:602.

## References / Citations
- OCHRE `Equipment.make_equivalent_battery_model()`: `vendors/OCHRE/ochre/Equipment/HVAC.py:620-641`
- OCHRE EBM invocation: `vendors/OCHRE/ochre/Equipment/HVAC.py:601-602`
- OCHRE `deadband_offset`: `vendors/OCHRE/ochre/Equipment/HVAC.py:222`
- HARES EBM implementation: `crates/hares-equipment/src/hvac/equivalent_battery.rs:26-84`
- HARES thermostat deadband_offset doc: `crates/hares-equipment/src/hvac/thermostat.rs:19-30`
- HARES `rated_capacity_w`: `crates/hares-equipment/src/hvac/staging.rs:318-334`
- Building capacitance derivation: `crates/hares-envelope/src/boundary_rc.rs:397` (`derive_zone_capacitances`)
- Zone capacitance in solver group: `crates/hares-core/src/dwelling/solver_builder.rs:487-493`
