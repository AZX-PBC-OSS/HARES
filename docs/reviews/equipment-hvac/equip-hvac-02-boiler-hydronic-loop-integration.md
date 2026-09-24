# Boiler hydronic loop flow rate and return temperature integration
**Review ID**: equip-hvac-02
**Category**: equipment-hvac
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/hvac/boiler.rs crates/hares-equipment/src/hvac/hvac_core.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Equipment/HVAC.py

## Findings
### Finding 1: [Severity: high]
**Description**: Boiler delivers `thermal_output_w` to both the hydronic Fluid port and directly to the zone Thermal port in the same timestep, but there is no terminal-unit model (radiator/baseboard) that transfers heat from the fluid loop to zone air. The fluid loop is effectively a passive state-tracking construct, while zone heating bypasses the hydronic water mass entirely.

**Code Location**: `crates/hares-equipment/src/hvac/boiler.rs:242-253` (ElectricBoiler) and `crates/hares-equipment/src/hvac/boiler.rs:546-558` (GasBoiler)

**Root Cause**: OCHRE models boilers as forced-air equipment (no hydronic loop at all). HARES extends the model with hydronic loop state tracking via `PortContribution::Fluid` but retains OCHRE's direct-to-zone heat delivery via `write_zone_thermal_contributions`. There is no terminal-unit equipment type that reads fluid loop state and transfers heat to zones. The fluid solver (`crates/hares-envelope/src/fluid_solver.rs`) computes net power and mean temperatures but does not interact with zone thermal balance in any way.

**Impact**: 
- The hydronic water mass (`flow_rate_kg_s * pipe_volume`) contributes zero thermal capacitance to the zone air heat balance. Heat is injected instantaneously with no transport delay or water thermal buffering, which overestimates short-timestep responsiveness (sub-5-minute resolution) compared to real hydronic systems where boiler firing rate, pipe mass, and emitter response create a ~5–30 minute lag.
- The fluid loop state (`mean_return_temp_c`) provides only telemetry-level feedback; the loop's net power is tracked but never coupled back into the zone thermal model.
- Direct-to-zone delivery routes boiler output through `duct_dse` (distribution system efficiency) which is a forced-air concept not applicable to hydronic systems (hydronic distribution losses are piping jacket losses, not duct leakage).

### Finding 2: [Severity: medium]
**Description**: Default boiler return temperature of 40.0 °C (104 °F) is physically inconsistent with the non-condensing boiler outlet temperature of 82.22 °C (180 °F). The resulting delta-T of 42.22 °C (76 °F) far exceeds typical residential hydronic design practice of 180 °F supply / 160 °F return (20 °F delta-T, ~11 °C).

**Code Location**: `crates/hares-equipment/src/hvac/boiler.rs:40` (DEFAULT_RETURN_TEMP_C) and `crates/hares-equipment/src/hvac/heating_config.rs:130-136, 207-209` (default_return_temp_c)

**Root Cause**: The 40 °C default is cited as a "typical condensing-boiler return temperature" (ASHRAE HVAC Systems and Equipment Ch.32), but it is applied uniformly to both condensing and non-condensing boilers. Non-condensing boilers operate at 71–82 °C return temperatures to prevent flue-gas condensation damage to cast-iron heat exchangers. The boiler code distinguishes condensing versus non-condensing for the EIR polynomial and outlet temperature (`boiler.rs:469-474`) but uses the same `default_return_temp_c` for both paths.

**Impact**:
- Combined with the default flow rate of 0.5 kg/s, a 42 °C delta-T implies a heat transfer capacity of ~88 kW (`0.5 × 4186 × 42.22`), exceeding typical residential boiler sizes (10–30 kW). For a 15 kW boiler, the actual operating delta-T would be ~7 °C with the default flow rate, making the supply temperature only ~47 °C instead of the design 82 °C — well below the expected condensing/non-condensing outlet values.
- HPXML files that omit the `return_temp_c` field will silently get 40 °C for non-condensing boilers, producing supply temperatures well below the equipment design point.

### Finding 3: [Severity: medium]
**Description**: Supply temperature is computed as `return_temp_c + ΔT` from heat balance but is never clamped to the boiler's design outlet temperature constraints, allowing supply temperatures far from physical boiler limits.

**Code Location**: `crates/hares-equipment/src/hvac/boiler.rs:228-232` (ElectricBoiler) and `crates/hares-equipment/src/hvac/boiler.rs:521-525` (GasBoiler)

**Root Cause**: The boiler has well-defined design outlet temperatures (`DEFAULT_CONDENSING_OUTLET_TEMP_C = 65.56 °C`, `DEFAULT_NON_CONDENSING_OUTLET_TEMP_C = 82.22 °C`) and chooses the appropriate one during `init()` based on condensing mode. However, the computed `supply_temp_c` in `step()` is derived from return temperature and heat flux, not clamped to the boiler's nameplate outlet setpoint. A low-flow condition (e.g., 0.1 kg/s with 15 kW) produces supply_temp = 40 + 15000/(0.1 × 4186) = 75.8 °C, exceeding the condensing design point of 65.6 °C.

**Impact**:
- `supply_temp_c` can exceed the condensing boiler design temperature, violating the physical assumption that the boiler controls its outlet to ~66 °C / 150 °F for condensing or ~82 °C / 180 °F for non-condensing.
- This also affects the non-condensing EIR polynomial which uses `outlet_temp_c` (a constant field set at init time) while the actual fluid supply temperature differs, creating an internal inconsistency between the efficiency calculation and the fluid temperature.

### Finding 4: [Severity: low]
**Description**: Default flow rate of 0.5 kg/s lacks explicit source citation and may not be accurate for HPXML-building-specific hydronic design.

**Code Location**: `crates/hares-equipment/src/hvac/heating_config.rs:356-361` (default_flow_rate_kg_s)

**Root Cause**: The comment references "ASHRAE HVAC Systems and Equipment Ch.13" with the historical rule-of-thumb "1 gpm per 10 000 Btu/h" but does not provide a specific table or section number. This rule of thumb is typically applied to branch circuits, not the boiler primary loop. Primary boiler loops are sized for the boiler's full capacity, typically giving flow rates of 0.06–0.19 kg/s for 10–30 kW residential boilers at a 20 °F delta-T.

**Impact**: The 0.5 kg/s default (≈8 gpm) is oversized for most residential boilers. At a 20 °F (11 °C) delta-T, this supports ~23 kW — adequate but generous. The mismatch is benign when `return_temp_c` is also poorly matched (Finding 2) but masks the underlying inconsistency in the temperature set.

### Finding 5: [Severity: low]
**Description**: `write_zone_thermal_contributions` applies duct DSE (distribution system efficiency) routeing to boiler output, but hydronic systems do not have ducts.

**Code Location**: `crates/hares-equipment/src/hvac/boiler.rs:249-254` and `crates/hares-equipment/src/hvac/duct_distribution.rs:94-131`

**Root Cause**: The boiler inherits the forced-air HVAC duct distribution model. Hydronic piping losses are jacket/convection losses from the piping to unconditioned spaces (typically 5–15% for well-insulated pipes), not duct leakage fractions (DSE). The `duct_dse` field defaults to 1.0 so this is latent until explicitly configured, but the `DuctConfig` is present in both `GasBoilerConfig` and `ElectricBoilerConfig` via `#[serde(flatten)]`.

**Impact**: If a user configures `duct_dse = 0.8` for a boiler (perhaps unintentionally through an HPXML template that applies DSE to all heating equipment), 20% of boiler output would be silently discarded or routed to a duct zone that may not exist for a hydronic system.

## Summary
- Total findings: 5
- Critical: 0 / High: 1 / Medium: 2 / Low: 2

## Recommendations
1. Implement or plan for a hydronic terminal-unit model (baseboard/radiator/radiant floor) that reads fluid loop state and transfers heat to zones. Until then, document clearly that the boiler's fluid port is telemetry-only and zone heating is direct (instantaneous), with the water thermal capacitance not participating in the zone energy balance.
2. Set `default_return_temp_c` conditionally based on condensing mode: use ~40 °C (104 °F) for condensing boilers and ~71 °C (160 °F) for non-condensing boilers, consistent with ASHRAE HVAC Systems and Equipment Ch.13/32 design guidance.
3. Clamp `supply_temp_c` to the boiler's design outlet temperature (`DEFAULT_CONDENSING_OUTLET_TEMP_C` or `DEFAULT_NON_CONDENSING_OUTLET_TEMP_C`) in `step()`, with overflow/underflow handled as reduced capacity or excess return temperature.
4. Tighten the source citation for `default_flow_rate_kg_s` to include a specific section/table in ASHRAE Ch.13, and consider making the default scale with `rated_capacity_w` using the 11 °C (20 °F) delta-T rule: `flow = capacity_w / (4186 * 11.0)`.
5. Remove `DuctConfig` from `GasBoilerConfig` and `ElectricBoilerConfig` serialization, or guard `write_zone_thermal_contributions` against DSE < 1.0 for boiler equipment types so that piping jacket losses are modeled separately from duct losses.

## References / Citations
- ASHRAE Handbook — HVAC Systems and Equipment (2020), Chapter 32: "Boilers" — flue-gas dewpoint and condensing return water temperature guidance
- ASHRAE Handbook — HVAC Systems and Equipment (2020), Chapter 13: "Hydronic Heating and Cooling" — design supply/return temperatures and flow rate conventions
- OCHRE HVAC.py, `GasBoiler` class (lines 687–732) — boiler treats heat delivery as forced-air (inherits `add_gains_to_zone` from `HVAC` base class); no hydronic loop modeled
- DOE 10 CFR Part 430, Subpart B, Appendix N — residential boiler AFUE test procedure and default federal minimum efficiency
- I=B=R (Hydronics Institute) — residential hydronic design guidelines: 180 °F supply, 20 °F delta-T for fin-tube baseboard sizing
