# DehumidifierConfig defaults: water removal, target RH, deadband
**Review ID**: hvaccfg-04
**Category**: hvac-config
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/hvac/cooling_config.rs` — `DehumidifierConfig` struct definition (lines 411–464)
- `crates/hares-equipment/src/hvac/dehumidifier.rs` — runtime model, defaults, hysteresis (lines 1–1084)
- `crates/hares-equipment/src/hvac/dehumidifier_defaults.rs` — rated conditions and default biquadratic curves (lines 1–140)
- `crates/hares-equipment/src/hvac/ac_config.rs` — re-exports (line 9)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/ZoneDehumidifier.cc` — `GetZoneDehumidifierInput`, `CalcZoneDehumidifier` (lines 180–908)
- `vendors/EnergyPlus/src/EnergyPlus/ZoneDehumidifier.hh` — `ZoneDehumidifierParams` struct (lines 78–132)

## Findings

### Finding 1: [Severity: medium] Deadband half-width of 2.5% yields a 5% total deadband — wider than documented expectation of 2.5%
**Description**: `DEFAULT_DEADBAND_HALF_WIDTH_RH_FRACTION = 0.025` (2.5% RH) is applied symmetrically around the target, producing a total deadband of 5% RH (±2.5%). The review specification describes an expected deadband of 2.5% total (±1.25%). At the default target of 50% RH, the dehumidifier turns on at 52.5% RH and stays on until humidity falls below 47.5% RH — a 5% swing. The review criteria classify deadbands >5% as "too wide" (causing large humidity swings), and 5% is right at that boundary.

**Code Location**: `crates/hares-equipment/src/hvac/dehumidifier.rs:34`, lines 125–126, 251–254
**Root Cause**: The constant `DEFAULT_DEADBAND_HALF_WIDTH_RH_FRACTION` is named as a half-width but its value (0.025 = 2.5%) results in a full-width deadband of 5%, rather than 2.5%. If a total deadband of 2.5% (±1.25%) is intended, the half-width value should be 0.0125.

**Impact**: Humidity can swing from 47.5% to 52.5% before cycling initiates, which is marginally acceptable but allows more moisture accumulation than a typical residential dehumidifier humidistat (many units have a factory-set deadband of 3–5% total). The current 5% total deadband may overstate humidity excursions in tightly-controlled spaces.

### Finding 2: [Severity: high] Missing part-load fraction (PLF) cycling model
**Description**: The HARES dehumidifier uses a simple binary on/off model — when humidity exceeds the upper deadband threshold, the unit runs at full rated capacity; when humidity falls below the lower threshold, it shuts off. There is no part-load ratio (PLR), part-load fraction (PLF) curve, or runtime fraction (RTF) calculation. EnergyPlus `CalcZoneDehumidifier` (lines 763–831) implements a full part-load cycling model:
1. PLR = min(1.0, load / capacity) — scales output to actual demand
2. PLF = PartLoadCurve(PLR) — accounts for efficiency degradation at part load (compressor start-stop losses)
3. RTF = PLR / PLF — runtime fraction, bounded to [0, 1]
4. Electric power is scaled by RTF, not simply on/off

Without a PLF model, HARES cannot represent the efficiency penalty from short cycling (which is significant for dehumidifiers at low moisture loads) nor can it model partial-capacity operation beyond the hysteresis on/off decision.

**Code Location**: `crates/hares-equipment/src/hvac/dehumidifier.rs:165–206` (`performance_snapshot`), `crates/hares-equipment/src/hvac/dehumidifier.rs:145–163` (`update_is_on`)
**Root Cause**: The dehumidifier model was implemented as a simplified binary-state hysteresis model without the part-load infrastructure present in EnergyPlus. The `DehumidifierConfig` struct (`cooling_config.rs:411–429`) has no fields for a PLF curve.

**Impact**: 
- At low dehumidification loads, modeled power consumption is too high (no efficiency loss from cycling, but also no RTF scaling — the unit is either 100% on or 0% off, which may alternately over/under-predict depending on timestep resolution)
- Runtime fraction and part-load ratio telemetry fields are absent from the dehumidifier model, limiting diagnostic capability
- Cannot replicate EnergyPlus dehumidifier test results that depend on part-load behavior

### Finding 3: [Severity: low] Default energy factor 1.8 L/kWh is conservative (pre-2019 Energy Star) vs. current typical residential units (2.0–2.5 L/kWh)
**Description**: The hard-coded fallback `rated_energy_factor_l_kwh = 1.8` in `Dehumidifier::new()` and `init_from_typed()` (line 122, line 245) corresponds to the pre-2012 minimum performance standard. Current Energy Star v5.0 (effective October 2019) requires ≥2.0 L/kWh for units with rated capacity ≥50 pints/day, and many modern units achieve 2.2–2.5 L/kWh. For the default 30 L/day rated capacity, energy factor 1.8 yields electric power ≈ 694 W, which is at the high end of the typical 300–700 W range.

**Code Location**: `crates/hares-equipment/src/hvac/dehumidifier.rs:122`, `crates/hares-equipment/src/hvac/dehumidifier.rs:245`
**Root Cause**: The fallback value was chosen as a safe, conservative default that will not under-predict energy consumption but may over-predict for modern high-efficiency units.

**Impact**: Default simulations without user-specified `energy_factor` or `integrated_energy_factor` will modestly over-estimate dehumidifier power draw (by 10–25% relative to modern Energy Star units). Users configuring Energy Star-certified equipment should explicitly set the energy factor. This is a documentation/guidance gap rather than a code defect.

### Finding 4: [Severity: low] No off-cycle parasitic electric load model
**Description**: EnergyPlus `ZoneDehumidifierParams` (header line 93) includes `OffCycleParasiticLoad` — electric power consumed when the dehumidifier is available but not actively running (e.g., control board, crankcase heater). When the unit is available but moisture demand is zero, EnergyPlus charges `OffCycleParasiticLoad` continuously (cc line 886). HARES has no equivalent parasitic load; when off, power is exactly 0 W.

**Code Location**: `crates/hares-equipment/src/hvac/dehumidifier.rs:166–173` (off condition returns zero power)
**Root Cause**: `DehumidifierConfig` has no `off_cycle_parasitic_w` field. The model was simplified to omit standby power.

**Impact**: Minor under-estimation of annual energy consumption (typically 5–15 W for control electronics, ~44–131 kWh/year). For most residential simulations this is negligible, but for detailed energy compliance modeling, the omission may be noticed.

## Summary
- Total findings: 4
- Critical: 0
- High: 1 (missing PLF cycling model)
- Medium: 1 (deadband total width is 5%, not 2.5%)
- Low: 2 (conservative default EF, no off-cycle parasitic load)

## Confirmed Correct
- **Rated conditions**: `RATED_DB_C = 26.666…°C` and `RATED_RH = 0.60` (`dehumidifier_defaults.rs:58,64`) match EnergyPlus `RatedInletAirTemp(26.7)` and `RatedInletAirRH(60.0)` exactly (`ZoneDehumidifier.cc:202–203`).
- **Rated water removal default**: 30 L/day is within the typical residential range (50–70 pint/day ≈ 24–33 L/day).
- **Target RH 50%**: Correctly within ASHRAE comfort range (45–55%) and well below the 60% mold-growth threshold.
- **Hysteresis control logic**: The on/off logic (`dehumidifier.rs:145–163`) uses `current_rh > max_rh` to start and `current_rh >= min_rh` to continue running — correct asymmetric hysteresis preventing boundary chatter.
- **Curve normalisation**: Both `DEFAULT_WATER_REMOVAL_CURVE` and `DEFAULT_ENERGY_FACTOR_CURVE` are normalised at the rated condition (`dehumidifier.rs:277–280`), so `rated_capacity_liters_per_day` passes through unchanged at 26.7°C / 60% RH.
- **Energy balance**: `sensible_gain_w = latent_removal_w + electric_power_w` (`dehumidifier.rs:198`) matches EnergyPlus `SensibleOutput = (LatentOutput * hfg) + ElectricPowerAvg` (`ZoneDehumidifier.cc:860`), correctly dumping compressor + fan heat into the zone plus recovered latent heat.
- **RH parsing**: `parse_rh_fraction` (`dehumidifier.rs:484–499`) auto-detects percent vs. fraction input — a user-friendly feature not present in EnergyPlus.

## Recommendations
1. **Add part-load cycling model (priority)**. Introduce a `PartLoadCurve` field to `DehumidifierConfig` and implement `PLR`, `PLF`, and `RTF` calculations mirroring EnergyPlus `CalcZoneDehumidifier` lines 726–831. Include PLF clamping to [0.7, 1.0] per EnergyPlus convention.
2. **Clarify deadband width**. Either rename the constant to reflect total width (e.g., `DEFAULT_DEADBAND_RH_FRACTION = 0.05` with separate min/max derivation) or reduce the half-width to 0.0125 if the specification intends a 2.5% total band. Document the expected humidity swing.
3. **Update default energy factor to 2.0 L/kWh** to align with current Energy Star v5.0 minimums for units ≥50 pint/day.
4. **Consider adding off-cycle parasitic load** for energy compliance modeling accuracy (low priority).

## References / Citations
- EnergyPlus `ZoneDehumidifier.cc` — `RatedInletAirTemp = 26.7`, `RatedInletAirRH = 60.0` (line 202–203)
- EnergyPlus `CalcZoneDehumidifier` — PLF/PLR/RTF model (lines 726–831)
- EnergyPlus `CalcZoneDehumidifier` — electric power calculation (line 853)
- EnergyPlus `CalcZoneDehumidifier` — sensible output equation (line 860)
- EnergyPlus `ZoneDehumidifierParams` — `OffCycleParasiticLoad` field (header line 93)
- AHAM DH-1-2008 rated condition: 26.7°C (80°F) DB / 60% RH
- Energy Star v5.0 residential dehumidifier specification: ≥2.0 L/kWh for ≥50 pint/day capacity
- ASHRAE Standard 55 — comfort RH range 30–60%, mold prevention threshold 60%
