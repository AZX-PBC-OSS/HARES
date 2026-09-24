# Dehumidifier model completeness: capacity curves, energy factor, water removal
**Review ID**: equip-hvac-10
**Category**: equipment-hvac
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/hvac/dehumidifier.rs crates/hares-equipment/src/hvac/dehumidifier_defaults.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Equipment/HVAC.py (OCHRE — no dehumidifier model; marked as TODO in HPXML parser)

## Findings
### Finding 1: [Severity: high]
**Description**: No part-load fraction (PLF) correlation for cycling losses.

**Code Location**: `dehumidifier.rs:165–206` (`performance_snapshot`) and `dehumidifier.rs:338–402` (`step`).

**Root Cause**: The HARES dehumidifier runs at full rated capacity whenever `is_on` is true and draws zero power otherwise. There is no part-load derating for cycling losses (PLF = f(PLR)), runtime fraction calculation, or sub-timestep startup/shutdown degradation. EnergyPlus `ZoneHVAC:Dehumidifier:DX` (`ZoneDehumidifier.cc:763–767`) applies a `PartLoadCurve` (optional, defaulting to 1.0) and computes `RunTimeFraction = PLR / PLF`; average electric power includes the off-cycle parasitic term (`ZoneDehumidifier.cc:854–855`). Without PLF, any timestep coarser than the actual compressor cycle time will overestimate efficiency during part-load operation because the model implicitly assumes zero cycling loss.

**Impact**: Overestimation of energy factor / underestimation of electric energy consumption at part load. Units operating near the deadband boundaries will have inflated efficiency. For coarse simulation timesteps (e.g. 15-minute), the error is material; for sub-minute timesteps the on/off hysteresis captures cycling explicitly, so the error shrinks.

### Finding 2: [Severity: medium]
**Description**: Energy Factor (EF) and Integrated Energy Factor (IEF) treated as interchangeable rated values without curve re-normalisation.

**Code Location**: `dehumidifier.rs:242–245` (`init_from_typed`).

**Root Cause**: `rated_energy_factor_l_kwh` is populated with `cfg.integrated_energy_factor` preferentially, falling back to `cfg.energy_factor`. Both metrics share the same configuration fields (`DehumidifierConfig` in `cooling_config.rs:408–429`) and are applied identically. However, EF is a single-condition rating (80°F/60%RH per AHAM DH-1-2008) that aligns with the curve normalisation point (26.7°C / 60% RH). IEF (per 10 CFR Part 430 Appendix X1) is a weighted composite across multiple test conditions (e.g. 65°F and 73°F) and already embeds off-design performance. Applying a biquadratic curve normalised at the EF rating point on top of an IEF-rated value double-counts condition-dependent adjustments. EnergyPlus uses `RatedEnergyFactor` and does not distinguish between the two rating paradigms, but its documentation implies the curve is applied to the single-condition EF, not the composite IEF.

**Impact**: If a user provides IEF data, the model will produce results that diverge from the intended rated performance. The fix should either (a) document that only EF is supported, or (b) add a flag to select the curve normalisation temperature appropriate for portable vs. whole-home IEF rating conditions.

### Finding 3: [Severity: medium]
**Description**: No off-cycle parasitic load.

**Code Location**: `dehumidifier.rs:165–172` (zero power when `is_on` is false).

**Root Cause**: When the dehumidifier is off (`is_on == false`), `performance_snapshot` returns zero for all fields including `electric_power_w`. EnergyPlus `ZoneDehumidifier:DX` accepts `OffCycleParasiticLoad` as a user input (`ZoneDehumidifier.hh:93`) and applies it proportionally to the off-cycle portion of the timestep (`ZoneDehumidifier.cc:854–855, 885–889`). Standby controls, crankcase heaters, and always-on electronics contribute to parasitic loads that are non-zero even when the compressor is idle.

**Impact**: Underestimation of annual electric energy consumption, most pronounced in low-humidity climates where the unit rarely runs but still draws standby power.

### Finding 4: [Severity: medium]
**Description**: No minimum/maximum inlet air temperature operating limits.

**Code Location**: `dehumidifier.rs:338–403` (`step` — unconditionally runs performance_snapshot when `is_on`).

**Root Cause**: The model evaluates capacity curves at any temperature as long as the RH deadband triggers. EnergyPlus (`ZoneDehumidifier.cc:690–691`) checks `InletAirTemp >= MinInletAirTemp && InletAirTemp <= MaxInletAirTemp` and disables the unit entirely if outside these bounds. Real dehumidifiers have low-temperature cutouts (to prevent evaporator freeze-up) and high-temperature cutouts (compressor thermal protection). Without these limits, the model can produce physically unreasonable water removal at extreme temperatures where the unit would not operate. The DEFAULT_DB_BOUNDS_C of (10.0, 40.0) set on the curves only clamp curve inputs; they do not disable the unit.

**Impact**: Physically implausible operation at low or high temperatures. In heating-dominated climates with cool basements (<15°C), the model may predict full dehumidifier operation when a real unit would be locked out.

### Finding 5: [Severity: low]
**Description**: Biquadratic curve bounds are wider than the EnergyPlus calibration range, with clamping warnings suppressed.

**Code Location**: `dehumidifier.rs:127–138` (curve construction with `warn_on_clamp: false`) and `dehumidifier_defaults.rs:66–76` (EnergyPlus original curve bounds documented as (21.0, 32.22)°C DB and (0.40, 0.80) RH fraction).

**Root Cause**: `DEFAULT_DB_BOUNDS_C = (10.0, 40.0)` and `DEFAULT_RH_BOUNDS = (0.0, 1.0)` are applied to all curves. The EnergyPlus `Curve:Biquadratic` inputs specify `Minimum/Maximum Value of x` = (21.0, 32.22)°C and `Minimum/Maximum Value of y` = (40, 80)%. By expanding the bounds, HARES extrapolates the biquadratic fit far beyond its calibrated domain without any warning (`warn_on_clamp: false`). The `dehumidifier_defaults.rs` tests at lines 69–76 document the original E+ bounds but only expose them under `#[cfg(test)]`.

**Impact**: Unreliable capacity/energy factor predictions at extreme indoor conditions (e.g. 12°C basement air, 95% RH after a flood). The curves may produce negative or unreasonably large values when blindly extrapolated.

### Finding 6: [Severity: low]
**Description**: Constant latent heat of vaporisation used for all temperatures.

**Code Location**: `dehumidifier.rs:197` (`latent_removal_w = water_removal_kg_s * LATENT_HEAT_VAPORISATION_0C_J_KG`).

**Root Cause**: `LATENT_HEAT_VAPORISATION_0C_J_KG` (≈2,501,000 J/kg) is used regardless of inlet air temperature. At typical indoor conditions (~24°C), the temperature-corrected latent heat is approximately 2,442,000 J/kg — a 2.4% difference. EnergyPlus computes `hfg = PsyHfgAirFnWTdb(InletAirHumRat, InletAirTemp)` (`ZoneDehumidifier.cc:859`), which is temperature-dependent. The regression test at `dehumidifier.rs:937–968` (`dehumidifier_h_fg_matches_physics_constant`) intentionally pins this to the constant for solver consistency.

**Impact**: Small (≈2.4%) systematic error in sensible heat and latent removal energy calculations. The constant is deliberately aligned with the humidity solver's h_fg for moisture mass round-trip consistency (ticket 001), making this a deliberate simplification rather than a bug.

### Finding 7: [Severity: low]
**Description**: Water density assumed constant (1 L = 1 kg).

**Code Location**: `dehumidifier.rs:26` (`KG_PER_LITER_WATER: f64 = 1.0`) and `dehumidifier.rs:191`.

**Root Cause**: Conversion from L/day to kg/s divides by 86400 directly. EnergyPlus (`ZoneDehumidifier.cc:722–724`) applies `RhoH2O(max((InletAirTemp - 11.0), 1.0))` to adjust water density for temperature. The density of water varies from ~999.7 kg/m³ at 10°C to ~997.0 kg/m³ at 25°C.

**Impact**: Negligible (<0.3%) error in mass-based water removal rate. Only material if water mass flows are integrated for condensate tank sizing.

## Summary
- Total findings: 7
- Critical / High / Medium / Low: 0 / 1 / 3 / 3

## Recommendations
1. **Add part-load fraction (PLF) correlation** — Implement an optional PLF quadratic curve (a + b·PLR + c·PLR²) with runtime fraction and average power calculations. For sub-minute timesteps where cycling is explicitly modeled, PLF can remain at 1.0.

2. **Clarify EF vs IEF handling** — Either document that only single-condition EF is supported and reject (or warn on) IEF input, or add separate curve normalisation temperature parameters (26.7°C for EF, 18.3°C for portable IEF, 22.8°C for whole-home IEF).

3. **Add off-cycle parasitic load** — Extend `DehumidifierConfig` with an optional `off_cycle_parasitic_load_w` field and apply it proportionally to off-cycle time in `performance_snapshot`.

4. **Implement operating temperature limits** — Add optional `min_operating_temp_c` and `max_operating_temp_c` to `DehumidifierConfig`, gating `is_on` in `update_is_on`. Default bounds should match the EnergyPlus curve validity range before letting curves extrapolate.

5. **Tighten curve bounds to EnergyPlus calibration range** — Set curve bounds to (21.0, 32.22)°C and (0.40, 0.80) RH fraction to match the EnergyPlus curve domain. Enable `warn_on_clamp` in init paths so users are alerted when operating outside calibrated range.

## References / Citations
- EnergyPlus `ZoneDehumidifier.cc` (`CalcZoneDehumidifier`: lines 605–908) — part-load, PLF, RTF, off-cycle parasitic, min/max inlet temp, temperature-dependent h_fg and water density.
- EnergyPlus `ZoneDehumidifier.hh` (struct `ZoneDehumidifierParams`: lines 78–132) — rated air flow, part-load curve, condensate collection.
- EnergyPlus `WindACRHControl.idf` — source curves for `DEFAULT_WATER_REMOVAL_CURVE` and `DEFAULT_ENERGY_FACTOR_CURVE`.
- EnergyPlus `SingleFamilyHouse_HP_Slab_Dehumidification.idf` — additional dehumidifier curve reference.
- AHAM DH-1-2008: legacy 80°F/60%RH rating condition (EF).
- 10 CFR Part 430 Appendix X1: current IEF test procedure with multi-condition weighted rating.
- OCHRE `ochre/utils/hpxml.py:1678` — dehumidifier marked as `# TODO: add dehumidifier` (no reference implementation available).
