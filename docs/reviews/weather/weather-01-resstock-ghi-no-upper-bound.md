# ResStock CSV GHI has no upper validation bound
**Review ID**: weather-01
**Category**: weather
**Date**: 2026-05-26

## Files Reviewed
crates/hares-io/src/resstock_csv.rs crates/hares-io/src/epw.rs crates/hares-io/src/tmy3.rs crates/hares-io/src/psm3.rs

## Vendor/Reference Files Consulted
vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc

## Findings
### Finding 1: [Severity: high]
**Description**: The ResStock CSV parser validates GHI with a lower bound only (`ghi_w_m2 < 0.0`), missing the upper bound of 1500 W/m² that the EPW, TMY3, and PSM3 parsers all enforce. A corrupted value of 10000 W/m² would pass validation silently and propagate through all downstream thermal and solar calculations.

**Code Location**:
- `crates/hares-io/src/resstock_csv.rs:219-223` — GHI validation checks only `< 0.0`, no upper bound
- `crates/hares-io/src/epw.rs:182-185` — GHI validated as `!(0.0..=1500.0).contains(&ghi_w_m2)` (reference)
- `crates/hares-io/src/tmy3.rs:128-131` — GHI validated as `!(0.0..=1500.0).contains(&ghi)` (reference)
- `crates/hares-io/src/psm3.rs:178-181` — GHI validated as `!(0.0..=1500.0).contains(&ghi)` (reference)

**Root Cause**: The ResStock CSV parser was written with asymmetric validation: DNI (`<= 1100` at line 226) and DHI (`<= 800` at line 233) both have upper bounds, but GHI only has a lower bound. This appears to be a simple omission during parser development — every other solar irradiance field in `parse_resstock_csv_str` has an upper bound except GHI.

**Impact**: A physically implausible GHI value (e.g., 10000 W/m² from data corruption, unit conversion error, or a column-misaligned CSV) would pass parser validation undetected. This would produce:
- Grossly overestimated solar heat gains in building thermal models
- Sky temperature overestimated (via Clark-Allen correlation using dry-bulb temperature, not directly from GHI — but radiative gains through windows would spike)
- Silent data corruption with no error message to alert the user

### Finding 2: [Severity: medium]
**Description**: The EPW parser validates GHI upper bound but omits upper bounds for DNI and DHI. This creates an inconsistent validation surface: the ResStock parser validates DNI/DHI but not GHI, while the EPW parser validates GHI but not DNI/DHI. The TMY3 and PSM3 parsers are the only ones that validate all three.

**Code Location**:
- `crates/hares-io/src/epw.rs:188-189` — DNI (`dni_w_m2`) and DHI (`dhi_w_m2`) are parsed with no range check
- `crates/hares-io/src/resstock_csv.rs:226-237` — DNI validated `!(0.0..=1100.0)` and DHI validated `!(0.0..=800.0)`
- `crates/hares-io/src/tmy3.rs:134-145` — DNI validated `!(0.0..=1100.0)` and DHI validated `!(0.0..=800.0)`
- `crates/hares-io/src/psm3.rs:184-195` — DNI validated `!(0.0..=1100.0)` and DHI validated `!(0.0..=800.0)`

**Root Cause**: Parsers evolved independently, and the validation surface was never harmonized. The ResStock parser was likely modelled on TMY3 field-by-field patterns (which validate all three), but the GHI check was accidentally written with only `ghi_w_m2 < 0.0` instead of the range pattern `!(0.0..=1500.0).contains(&ghi_w_m2)`.

**Impact**: This is a consistency issue rather than a direct bug — EPW files are vetted by EnergyPlus tools upstream, so missing DNI/DHI validation is lower risk. However, the inconsistency makes it harder to reason about what validation guarantees each parser provides.

### Finding 3: [Severity: low]
**Description**: EnergyPlus WeatherManager performs no upper-bound validation on GHI, DNI, or DHI from EPW files. The HARES 1500 W/m² bound is a HARES-original safety net, not derived from EnergyPlus reference behavior.

**Code Location**:
- `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc:2739-2741` — GHI (`GLBHoriz`): only checks `GLBHoriz < 0.0`, replaces negative with sentinel 9999. No upper bound.
- `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc:2749-2756` — DNI (`DirectRad`) and DHI (`DiffuseRad`): only checks `< 0.0`, counts as out-of-range. No upper bound.
- `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc:8307-8308` — Out-of-range reporting confirms bounds are `>=0` / `NoLimit` for both Direct and Diffuse Radiation.

**Root Cause**: EnergyPlus relies on upstream weather-file generators to produce physically valid data. HARES is more defensive, adding limits that EnergyPlus omits.

**Impact**: Low. The 1500 W/m² bound is conservative but well-justified: the solar constant is ~1361 W/m², and cloud enhancement can briefly boost surface GHI to ~1400-1600 W/m² at high-elevation sites. 1500 W/m² catches data corruption while passing legitimate cloud-edge enhancement events. The bound should be maintained as a quality-of-implementation improvement over EnergyPlus.

## Summary
- Total findings: 3
- Critical / High / Medium / Low: 0 / 1 / 1 / 1

## Recommendations
1. **Add GHI upper bound of 1500 W/m² to `resstock_csv.rs:219`**: Change from `if ghi_w_m2 < 0.0` to `if !(0.0..=1500.0).contains(&ghi_w_m2)`, matching the EPW/TMY3/PSM3 parsers. This is a one-line change with zero risk to legitimate ResStock CSV data (GHI values in real ResStock AMY 2018 files never exceed ~1200 W/m²).
2. **Consider adding upper bound validation for DNI and DHI to `epw.rs:188-189`** to harmonize the validation surface across all four parsers, but this is lower priority since EPW inputs are typically pre-validated by EnergyPlus-compatible tools.
3. **Document the 1500 W/m² bound choice** as a physically-motivated upper limit: the solar constant (1361 W/m²) plus cloud-edge enhancement margin (~10%). High-altitude snowy sites might approach ~1600 W/m² briefly, but 1500 W/m² provides adequate headroom without admitting clearly corrupt values (like 10000 W/m² or negative readings).

## References / Citations
- Solar constant: 1361 W/m² (Kopp & Lean 2011), 1367 W/m² (EnergyPlus legacy value at `WeatherManager.cc:3501`)
- EnergyPlus WeatherManager EPW data validation: `WeatherManager.cc:2718-2917` (missing-data handling) and `WeatherManager.cc:2476-2527` (first-day range checks for Dry Bulb, Dew Point, RH, Pressure, Wind only — no solar radiation bounds)
- EnergyPlus out-of-range reporting: `WeatherManager.cc:8305-8308` — Beam/Diffuse Solar Rad reported as `>=0`/`NoLimit`
- Cloud enhancement of surface irradiance: up to ~30% above clear-sky (Yordanov et al. 2015, "1000 Irradiance Spikes Caused by Cloud Enhancement")
- HARES TMY3 reference: Wilcox & Marion 2008, NREL/TP-581-43156
