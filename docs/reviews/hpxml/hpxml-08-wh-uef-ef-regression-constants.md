# Water heater UEF->EF regression constants are hardcoded OCHRE-specific values
**Review ID**: hpxml-08
**Category**: hpxml
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-io/src/hpxml/water_heater_ua.rs`
- `crates/hares-io/src/hpxml/resolve_water_heater.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/hpxml.py`
- `vendors/OCHRE/ochre/Equipment/WaterHeater.py`
- ResStock `waterheater.rb` (NREL/ResStock GitHub; no local copy in repository)

## Findings

### Finding 1: [Severity: medium]
**Description**: Six UEF-to-EF and UEF-to-COP regression constants are hardcoded as Rust `const` values with no path to update them without recompilation. HARES already has a `defaults/water_heating/default_paramters.csv` pattern for water heater defaults, but none of the UEF regression constants are loaded from it. The OCHRE reference implementation hardcodes the same constants identically in Python, so this is an inherited design limitation rather than a porting defect.

The six constants and their locations are:

| Constant | Value | HARES Location | OCHRE Location |
|---|---|---|---|
| Gas UEF→EF slope | 0.9066 | `water_heater_ua.rs:153` | `hpxml.py:1115` |
| Gas UEF→EF intercept | 0.0711 | `water_heater_ua.rs:156` | `hpxml.py:1115` |
| Electric UEF→EF slope | 2.4029 | `water_heater_ua.rs:137` (inline) | `hpxml.py:1125` (inline) |
| Electric UEF→EF intercept | 1.2844 | `water_heater_ua.rs:137` (inline) | `hpxml.py:1125` (inline) |
| HPWH UEF→COP factor | 1.174536058 | `resolve_water_heater.rs:194` (fn-local const) | `hpxml.py:1187` (inline) |
| HPWH EF→UEF slope | 0.60522 | `resolve_water_heater.rs:190` (fn-local const) | `hpxml.py:1170` (inline) |
| HPWH EF→UEF denominator | 1.2101 | `resolve_water_heater.rs:191` (fn-local const) | `hpxml.py:1170` (inline) |

**Code Location**: Gas/electric constants at `water_heater_ua.rs:137,153,156`; HPWH constants at `resolve_water_heater.rs:190-194`.

**Root Cause**: The constants are literal transcriptions from OCHRE Python into Rust. OCHRE itself hardcodes these values; there is no precedent in the reference implementation for loading them from a config file. The HARES defaults system (`crates/hares-io/src/defaults.rs`) evolved to load HVAC biquadratic curves and ZIP parameters, but was never extended to cover water-heater regression constants.

**Impact**: These regression coefficients are empirical fits from Maguire & Roberts (2020) based on late-2010s DOE CCMS appliance data. When the DOE test procedure is next revised or when ResStock calibration updates its regression coefficients, a recompilation of HARES will be required rather than a simple CSV edit. In practice this is low-frequency (the last revision was 2017 → 2020), and the values must be validated against test data before use, so silent CSV changes are arguably more dangerous than a deliberate code change with regression test validation. However, as a data-driven parameter the current arrangement violates the maintainability pattern already established for HVAC curves.

### Finding 2: [Severity: medium]
**Description**: HPWH UEF→COP constant (1.174536058) carries 9 significant digits of precision, but its source (OCHRE `WaterHeater.py:443-444`) describes it as "Nominal COP based on simulation of the UEF test procedure at varying COPs" and it was derived from GE GeoSpring calibration data. The excessive precision implies a direct numerical fit result that was transcribed without rounding, but the underlying data likely does not justify 9 significant figures. The HPWH UEF→COP derivation is also multiply indirect: the UEF→COP factor converts the DOE test result into an equipment-rated COP that then serves as input to a biquadratic curve model with 6 coefficients (`cop_coeff` in `WaterHeater.py:467-474`), each with only 3-4 significant digits. The mismatch in precision between the scaling factor and the curve coefficients it feeds is a red flag for spurious precision.

**Code Location**: `resolve_water_heater.rs:194` — `const HPWH_UEF_TO_COP: f64 = 1.174_536_058`.

**Root Cause**: Direct transcription of OCHRE value with no rounding or sensitivity analysis. OCHRE's value itself may have been produced by a solver convergence producing all available digits.

**Impact**: Low practical impact on results (well within 1%), but the pattern reveals a data-validation gap: the factor was not re-derived or independently verified during the port.

### Finding 3: [Severity: low]
**Description**: Gas UEF→EF constants are properly extracted as module-level `const` values with doc-comments citing their source, but the electric UEF→EF numbers (`2.4029 * uef - 1.2844`) are used inline at `water_heater_ua.rs:137` without named constants or a source citation comment on the same line. The source citation appears only in the module-level doc comment ("Maguire & Roberts (2020) methodology for UEF", line 5) but is not repeated at the point of use. This is inconsistent with the gas constants (`water_heater_ua.rs:153-156`), which have their own `/// Source:` doc-comments and named `const` declarations.

**Code Location**: `water_heater_ua.rs:137` — `let ef_equiv = 2.4029 * uef - 1.2844;`

**Root Cause**: The gas constants were extracted from inline use into named `const` values during a previous cleanup pass, but the electric constants were not given the same treatment.

**Impact**: Minor maintainability inconsistency. If someone needs to trace the provenance of the electric regression, they must follow the module-level doc-comment rather than a localized source citation.

### Finding 4: [Severity: low]
**Description**: The HARES codebase only loads `defaults/water_heating/default_paramters.csv` for OCHRE compatibility tests (`equipment.rs:583-584`) but does **not** read UEF regression parameters from it in the production parser. The CSV currently contains only 4 rows (`T_set`, `T_db`, `P_hw1`, `P_hw2`) and has no columns for regression coefficients. If the intent is to eventually make these configurable, the CSV schema needs an additional column for equation type and coefficient index. As it stands, the defaults mechanism is disconnected from these constants.

**Code Location**: `defaults/water_heating/default_paramters.csv` (too small / no regression rows); `equipment.rs:583-584` (test-only loading path).

**Root Cause**: The `Water Heating` defaults directory was copied from OCHRE's defaults directory but was never extended to hold regression constants.

**Impact**: No impact on current operation. Only relevant if the team decides to make these constants configurable (see Finding 1) — the CSV schema needs updating first.

## HPXML 4.x UEF Validity Assessment

The question of whether these constants remain valid for HPXML 4.x UEF values requires examining the DOE test procedure timeline:

- **DOE 10 CFR 430 Appendix E (pre-2017)**: Defines the Energy Factor (EF) test procedure
- **DOE 10 CFR 430 Appendix E (revised, effective June 2017)**: Defines the Uniform Energy Factor (UEF) test procedure, which uses a different draw pattern (heavy-usage bin selection by FHR), different inlet temperature, and different setpoint temperature from the EF test
- **HPXML 4.x**: Carries both `EnergyFactor` (legacy) and `UniformEnergyFactor` (current) elements

The Maguire & Roberts (2020) regressions were specifically derived to bridge these two test procedures. The constants were obtained by:
1. Simulating both EF and UEF test procedures on a large sample of water heaters from the CCMS database
2. Fitting linear regressions between the two metrics for each water heater category

The constants **remain valid for HPXML 4.x** because:
- HPXML 4.x UEF values are still from the same DOE test procedure (10 CFR 430 App E, 2017)
- The Burch & Erickson (2004) physics equations are defined in the EF domain, so the UEF→EF conversion is still necessary
- ResStock continues to use these same constants as of its 2025.1 release
- No DOE test procedure revision has occurred since 2017 that would invalidate the regressions

However, the constants would need updating if:
1. The DOE revises 10 CFR 430 Appendix E test procedure
2. NREL publishes updated regressions from newer CCMS data
3. Product-specific regressions become available for emerging categories (e.g., CO2 heat pump water heaters)

## Summary
- Total findings: 4
- Critical: 0 / High: 0 / Medium: 2 / Low: 2

## Recommendations

1. Extract the electric UEF→EF regression coefficients into named `const` values with source citations matching the gas constants' pattern (`water_heater_ua.rs:137` becomes `const ELEC_UEF_TO_EF_SLOPE: f64 = 2.4029; const ELEC_UEF_TO_EF_INTERCEPT: f64 = 1.2844;`)

2. Optionally add a `try_from_defaults()` constructor or a CSV-based lookup that allows the six constants to be overridden from `defaults/water_heating/` without recompilation. The `DefaultSchedule Parameters.csv` precedent shows the CSV→TOML→serde pipeline already exists. This should default to the current hardcoded values when no override is present to preserve backward compatibility.

3. Round `HPWH_UEF_TO_COP` to 4 significant digits (`1.175`), matching the significant-figure precision of the biquadratic coefficients it feeds. If the extra precision is needed for test reproducibility with OCHRE, document that explicitly in the const comment.

4. Verify the HPWH UEF→COP factor against current GE GeoSpring product specifications and/or re-derive from ResStock's most recent calibration data (waterheater.rb L1100+) to ensure the factor still holds for current-model HPWHs with UEF ≥ 3.5.

## References / Citations

- Maguire, J., & Roberts, D. (2020). *Evaluation of Residential Water Heater Uniform Energy Factor to Energy Factor Conversion*. NREL/TP-5500-68035. ASHRAE 2020 Building Performance Analysis Conference. https://www.ashrae.org/file%20library/conferences/specialty%20conferences/2020%20building%20performance/papers/d-bsc20-c039.pdf
- Burch, J., & Erickson, P. (2004). *Using Ratings Data to Derive Simulation-Model Inputs for Storage-Tank Water Heaters*. NREL/CP-550-36035. http://www.nrel.gov/docs/gen/fy04/36035.pdf
- DOE 10 CFR 430, Subpart B, Appendix E (Uniform Energy Factor test procedure, effective June 2017)
- RESNET EF Calculator spreadsheet (2017): https://www.resnet.us/wp-content/uploads/RESNET-EF-Calculator-2017.xlsx (cited by OCHRE `hpxml.py:1099`)
- ResStock waterheater.rb: https://github.com/NREL/resstock/blob/run/restructure-v3/resources/hpxml-measures/HPXMLtoOpenStudio/resources/waterheater.rb (cited by OCHRE `hpxml.py:1069`)
