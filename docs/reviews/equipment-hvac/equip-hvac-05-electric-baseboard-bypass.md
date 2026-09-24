# Electric baseboard bypass verification: no efficiency loss pathway
**Review ID**: equip-hvac-05
**Category**: equipment-hvac
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/hvac/baseboard.rs`
- `crates/hares-equipment/src/hvac/hvac_core.rs`
- `crates/hares-equipment/src/hvac/duct_distribution.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/HVAC.py`

## Findings

### Finding 1: [Severity: low]
**Description**: PLF part-load degradation coefficient (`Cd = 0.25`) is default-configured for `Baseboard` equipment type even though the baseboard `step()` method never applies it. The baseboard step directly computes `thermal_output_w = rated_capacity_w * duty_cycle * space_fraction` without calling `part_load_factor()`. This is currently benign but creates a latent risk if the baseboard step is ever refactored to use shared staging/PLF infrastructure.

**Code Location**: `hvac_core.rs:338-341` sets the default `Cd` via the `_ => DEFAULT_PLF_DEGRADATION_COEFF` fallback branch, which catches `Baseboard`. `baseboard.rs:140-143` computes thermal output directly using the raw `duty_cycle` — no `part_load_factor()` call.

**Root Cause**: The `HvacEquipment::new()` constructor applies a single default for all non-MSHP equipment types. `Baseboard` is not differentiated from forced-air types.

**Impact**: Currently zero — baseboard correctly bypasses the PLF formula. If future code evolution adds a `part_load_factor()` call to `ElectricBaseboard::step()`, efficiency would be incorrectly derated (e.g., at `duty_cycle=0.5`, PLF would be `1 - 0.25*(1-0.5) = 0.875`, implying a non-physical 12.5% efficiency loss for electric resistance heat). OCHRE similarly avoids this because `ElectricBaseboard` inherits from `Heater` (not `DynamicHVAC`) and never enters the `_biquadratic`/PLR path.

**Vendor Comparison**: OCHRE `ElectricBaseboard` (HVAC.py:672-679) does not override EIR calculation; like HARES, it inherits the base `HVAC.update_eir()` which returns `eir_list[n_speeds]` without applying PLF — the PLF code path lives in `DynamicHVAC` only, which `ElectricBaseboard` does not inherit. Both codebases agree.

### Finding 2: [Severity: medium]
**Description**: All zone thermal contributions from baseboard (and all other HVAC equipment) are delivered as purely convective heat (`radiant_gain_w = 0.0`). Real electric baseboard heaters deliver approximately 20-30% of their output as radiant heat to surfaces, with the remainder as convective. The HARES thermal accumulator infrastructure (`ThermalAccumulator`, `ports.rs:176-252`) tracks `sensible_gain_w` (convective) and `radiant_gain_w` separately, indicating the envelope solver could distinguish between the two modes. Routing 100% of baseboard heat to the convective channel means radiant-driven surface temperature effects on mean radiant temperature and comfort are not captured.

**Code Location**: `duct_distribution.rs:121-127` — `write_zone_thermal_contributions` passes `radiant_gain_w: 0.0` unconditionally for all zones and all categories. No equipment-specific override exists.

**Root Cause**: `write_zone_thermal_contributions` is a shared helper with a fixed zero-radiant assumption. There is no mechanism for an equipment to pass a radiant fraction through this interface.

**Impact**: Medium — if the envelope solver differentiates convective from radiant heat transfer (the separate tracking implies it does), then the thermal response of the zone will skew toward faster air-temperature response and slower surface-temperature response compared to real baseboard behavior. This may produce slightly optimistic time-to-comfort metrics. For an energy-only simulation (not thermal comfort), the impact is negligible since total heat delivered is correct.

**Vendor Comparison**: OCHRE `add_gains_to_zone()` (HVAC.py:563-566) adds all HVAC heat as `hvac_sens_gain` to the zone with no radiant/convective split. OCHRE makes the same simplification. Neither codebase implements a radiant fraction for resistance heaters.

### Finding 3: [Severity: low]
**Description**: EIR validation only checks `<= 0.0` and `!is_finite()`, allowing user-configured EIR values above 1.0 that would incorrectly derate electric resistance efficiency (EIR must equal 1.0 for a purely resistive load). No upper-bound check or warning exists for `EquipmentType::Baseboard`.

**Code Location**: `baseboard.rs:114-119`.

**Root Cause**: Validation is generic (positive, finite) rather than equipment-type-aware.

**Impact**: Low — a misconfigured input file would produce incorrect results, but the EIR defaults to 1.0 via `ElectricBaseboardConfig::default()`. No simulation-internal pathway can silently change EIR away from 1.0. A `warn!` on EIR values `> 1.0` for electric resistance equipment would improve input validation hygiene.

**Vendor Comparison**: OCHRE `ElectricBaseboard` (HVAC.py:672-679) also does not validate EIR. Both codebases trust the input configuration.

## Summary
- Total findings: 3
- Critical / High / Medium / Low: 0 / 0 / 1 / 2

## Recommendations
1. **Explicitly zero the `plf_cooling_degradation_coeff` for Baseboard** in `ElectricBaseboard::init()` or `HvacEquipment::new()` to eliminate the latent PLF risk. Add a `HvacEquipmentType::Baseboard => 0.0` arm to the Cd selection in `hvac_core.rs:338-341`. (Addresses Finding 1)
2. **Consider adding a radiant fraction parameter** to `write_zone_thermal_contributions` or as a per-equipment config field. Electric baseboards should default to ~0.25 radiant fraction per ASHRAE Handbook (HVAC Systems and Equipment, Ch. 10). This would require coordination with the envelope solver's radiant/convective handling. (Addresses Finding 2)
3. **Add an EIR upper-bound warning** for `EquipmentType::Baseboard` (and possibly `ElectricFurnace`) — if EIR > 1.0, emit a `tracing::warn!` noting that pure electric resistance should have EIR = 1.0 (100% conversion). (Addresses Finding 3)

## References / Citations
- OCHRE `ElectricBaseboard` class forces `duct_dse = 1`: `vendors/OCHRE/ochre/Equipment/HVAC.py:672-679`
- OCHRE duct zone routing: `vendors/OCHRE/ochre/Equipment/HVAC.py:188-197`
- OCHRE `add_gains_to_zone()` (no radiant split): `vendors/OCHRE/ochre/Equipment/HVAC.py:563-566`
- HARES duct loss `write_zone_thermal_contributions` with zero radiant: `crates/hares-equipment/src/hvac/duct_distribution.rs:121-127`
- HARES baseboard `step()` direct thermal calculation: `crates/hares-equipment/src/hvac/baseboard.rs:140-157`
- HARES baseboard test confirming DSE=1.0 and 100% efficiency: `crates/hares-equipment/src/hvac/baseboard.rs:358-374`
- ASHRAE Handbook — HVAC Systems and Equipment, Chapter 10 (Electric Resistance Heating), radiant fraction ~25% for baseboard heaters
