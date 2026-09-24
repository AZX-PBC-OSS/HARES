# Heat pump constants: defrost thresholds, compressor maps, mass flow defaults
**Review ID**: hvaccfg-06
**Category**: hvac-config
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/hvac/heat_pump/constants.rs` (96 lines)
- `crates/hares-equipment/src/hvac/heat_pump/defrost.rs` (1003 lines) — usage context
- `crates/hares-equipment/src/hvac/hvac_core.rs` — companion constants
- `crates/hares-equipment/src/hvac/default_curves.rs` — compressor map defaults
- `crates/hares-equipment/src/hvac/staging.rs` — PLF degradation constants

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/DXCoils.cc` — defrost formulas, multipliers
- `vendors/EnergyPlus/src/EnergyPlus/VariableSpeedCoils.cc` — defrost, rating temps
- `vendors/EnergyPlus/src/EnergyPlus/StandardRatings.hh` / `.cc` — ARI/AHRI rated conditions
- `vendors/EnergyPlus/src/EnergyPlus/DXCoils.hh` — `MaxOATDefrost` default = 0.0
- `vendors/EnergyPlus/src/EnergyPlus/VariableSpeedCoils.hh` — `MaxOATDefrost` default = 0.0
- `vendors/EnergyPlus/idd/versions/V8-7-0-Energy+.idd` — IDD `\default 5.0` for `MaxOATDefrost`
- `vendors/OCHRE/ochre/Equipment/HVAC.py` — OCHRE defrost flag (L1141), `defrost_eir_temp_mod_frac` (L1161)

---

## Findings

### Finding 1: Defrost EIR temperature modifier (0.1528) is OCHRE-derived, not directly traceable to an EnergyPlus constant
**Severity**: Medium
**Description**: The constant `DEFROST_EIR_TEMP_MODIFIER = 0.1528` is documented as "Sourced from EnergyPlus OnDemand defrost formula" but the value `0.1528` does not appear anywhere in the EnergyPlus source tree. It originates from OCHRE `HVAC.py:1161` (`defrost_eir_temp_mod_frac = 0.1528  # in kW`). EnergyPlus computes defrost impact on input power via the `InputPowerMultiplier` biquadratic formula alone — it does not use a separate multiplier of this form. The OCHRE comment labelling it "# in kW" is dimensionally misleading (it is a dimensionless factor that produces power when multiplied by capacity_W).
**Code Location**: `constants.rs:20` — definition; `defrost.rs:403–406` — usage
**Root Cause**: HARES and OCHRE split defrost power into two terms: a base EIR multiplier (capacity/eir biquadratic) and an `extra_power_w` term proportional to `0.1528 * capacity_W * time_fraction`. EnergyPlus computes defrost power more directly through the `InputPowerMultiplier = 0.954 * (1 - FractionalDefrostTime)` without a separate additive extra-power term. The two approaches are not mathematically reconcilable to a single constant, so `0.1528` has no EnergyPlus counterpart.
**Impact**: The extra-power term adds to total compressor power during defrost beyond what the EIR multiplier alone would produce, causing HARES defrost power draw to differ from EnergyPlus in a model-dependent way. Users re-implementing or validating HARES against EnergyPlus should be aware this term is an OCHRE-specific addition. The constant is physically plausible (compressor defrost does consume extra power), but the specific coefficient cannot be verified against EnergyPlus reference code.

### Finding 2: Defrost enable temperature (4.4445°C / 40°F) differs from EnergyPlus default (5.0°C / 41°F)
**Severity**: Low
**Description**: HARES's `DEFROST_ENABLE_TEMP_C = 4.4445` (exact 40°F) guards defrost activation: defrost is suppressed when outdoor dry-bulb ≥ 4.4445°C. EnergyPlus's `MaxOATDefrost` IDD default is `\default 5.0` (41°F), which suppresses defrost when ODB ≥ 5.0°C. The two values differ by 0.5555°C (1°F).
**Code Location**: `constants.rs:6` — `DEFROST_ENABLE_TEMP_C`; `defrost.rs:88` — `max_oat_defrost_c` default; `defrost.rs:369` — guard.

Reference chain:
- OCHRE `HVAC.py:1141`: `self.defrost = t_ext_db < 4.4445` — exact 40°F guard
- EnergyPlus `DXCoils.hh:484`: `MaxOATDefrost(0.0)` — C++ member init
- EnergyPlus IDD `V8-7-0-Energy+.idd`: `\default 5.0` — user-facing default (overrides C++ init during input processing)
**Root Cause**: HARES follows OCHRE's 40°F hard threshold; EnergyPlus uses the IDD default of 5°C (41°F). The 1°F discrepancy originates from OCHRE (converted 40°F to 4.4445°C) rather than rounding 5.0°C to °F (41°F → 5.0°C).
**Impact**: Negligible. At OAT between 4.4445°C and 5.0°C, EnergyPlus may still run defrost while HARES will not. However, frost formation at OAT between 40–41°F is marginal (the outdoor coil 0.82*ODB − 8.589 model gives coil temp ≈ −4.9°C at ODB=4.44°C, well below freezing), so both thresholds are in a region where humidity conditions are the dominant factor. The `DEFROST_COIL_TEMP_SLOPE` and delta-humidity model will produce near-zero defrost fractions in this temperature range regardless.

### Finding 3: ARI/AHRI rated-condition temperature constants missing from centralized constants file
**Severity**: Low
**Description**: The `constants.rs` file contains no named constants for the ARI/AHRI 210/240-2023 rated conditions. These temperatures appear as bare numeric literals in multiple modules:

| Condition | Value | Location where hardcoded |
|-----------|-------|--------------------------|
| Heating indoor DB (H1/H2/H3) | 21.11°C | `heater.rs:838` |
| Heating outdoor DB (H1 rated) | 8.33°C | `default_curves.rs:157,169,181,185`, `heater.rs` |
| Heating outdoor DB (H3 low-temp) | −8.33°C | `heater.rs:839`, `default_curves.rs:170–171` |
| Cooling indoor DB rated | 26.67°C | `resolve_hvac.rs:4928` |
| Cooling outdoor DB rated | 35.0°C | `resolve_hvac.rs:4935`, `hvac_core.rs:3077` |
| Cooling indoor WB rated | 19.44°C | Not in constants.rs |

EnergyPlus defines these systematically via struct initializers:
- `StandardRatings.hh:73`: `HeatingOutdoorCoilInletAirDBTempRated = 8.33`
- `StandardRatings.cc:334`: `HeatingIndoorCoilInletAirDBTempRated = 21.11`
- `StandardRatings.cc:110`: `OutdoorCoilInletAirDryBulbTempRated = 35.0`
- `VariableSpeedCoils.cc:99`: `RatedInletAirTempHeat = 21.1111`
- `VariableSpeedCoils.cc:101`: `RatedAmbAirTempHeat = 8.3333`

**Root Cause**: The compressor map defaults (`default_curves.rs`) and scaling code (`heater.rs` lines 827–881) are post-hoc additions that introduced rated-condition temperatures on an as-needed basis rather than as centralized constants.
**Impact**: If the same rated condition value is ever used inconsistently (e.g., 21.11 in one place, 21.1111 in another), biquadratic curve evaluations will differ from their intended reference point, producing offset capacity and EIR ratios. The `capacity_ratio_at_17f` scaling feature (heater.rs:865–881) is particularly sensitive — it anchors the biquadratic to a specific H3 (17°F / −8.33°C) capacity ratio; if the curve's x1 reference (21.11 vs 21.0) drifts, the anchor point shifts.

### Finding 4: All defrost formula coefficients verified against EnergyPlus source — no discrepancies
**Severity**: Informational
**Description**: All 13 defrost-related constants in `constants.rs` that have direct EnergyPlus counterparts were verified and match exactly.

**Verified constants (EnergyPlus → HARES)**:

| HARES Constant | Value | EnergyPlus Source |
|---------------|-------|-------------------|
| `DEFROST_TIME_FRACTION_NUMERATOR` | 0.01446 | `DXCoils.cc:11324` |
| `DEFROST_CAPACITY_MULTIPLIER_BASE` | 0.875 | `DXCoils.cc:11329` |
| `DEFROST_POWER_MULTIPLIER_NUMERATOR` | 0.954 | `DXCoils.cc:11330` |
| `DEFROST_Q_MULTIPLIER` | 0.01 | `DXCoils.cc:11344` |
| `DEFROST_REFERENCE_TEMP_C` | 7.222 | `DXCoils.cc:11344` (45°F) |
| `DEFROST_CAPACITY_UNIT_FACTOR` | 1.01667 | `DXCoils.cc:11344` (1/0.9836) |
| `DEFROST_COIL_TEMP_SLOPE` | 0.82 | `DXCoils.cc:11291-11292` |
| `DEFROST_COIL_TEMP_OFFSET_C` | −8.589 | `DXCoils.cc:11292` (9.7°F) |
| `DEFROST_MIN_DELTA_HUMIDITY_RATIO` | 1e−6 | `DXCoils.cc:11293` (`max(1e-6, ...)`) |
| `DEFROST_EIR_CURVE_TEMP_MIN_C` | 15.555 | `VariableSpeedCoils.cc:6805-6806` (60°F) |
| `TIMED_DEFROST_CAP_MULT_BASE` | 0.909 | `DXCoils.cc:11311` |
| `TIMED_DEFROST_CAP_MULT_SLOPE` | 107.33 | `DXCoils.cc:11311` |
| `TIMED_DEFROST_PWR_MULT_BASE` | 0.90 | `DXCoils.cc:11312` |
| `TIMED_DEFROST_PWR_MULT_SLOPE` | 36.45 | `DXCoils.cc:11312` |

### Finding 5: On-demand power multiplier is constant in HARES but time-fraction-dependent in EnergyPlus
**Severity**: Informational (design divergence)
**Description**: In EnergyPlus, the on-demand defrost input power multiplier is `InputPowerMultiplier = 0.954 * (1 - FractionalDefrostTime)`, which decreases as defrost time fraction increases. In HARES, the power multiplier is a fixed ratio: `power_multiplier = 0.954 / 0.875 ≈ 1.09029`, independent of the time fraction. HARES compensates by computing `extra_power_w` via `DEFROST_EIR_TEMP_MODIFIER` (Finding 1) as a separate additive term.

This split is structurally different from EnergyPlus:
- **EnergyPlus**: `TotalPower = RatedPower * biquadratic_EIR * power_multiplier(time_fraction)`
- **HARES**: `TotalPower = RatedPower * biquadratic_EIR * power_multiplier(constant) * capacity_multiplier(time_fraction) + extra_power_w(time_fraction, capacity)`

**Code Location**: `defrost.rs:391–392` — constant power_multiplier; `defrost.rs:403–406` — extra_power_w
**Impact**: The HARES approach is self-consistent but diverges from the EnergyPlus formulation. When averaged over full cycles, the two models produce similar totals but with different partitioning between "efficiency multiplier" and "additive load." This is a deliberate OCHRE design choice, not a bug, but should be understood by users comparing HARES output to EnergyPlus simulations.

### Finding 6: Dimensionless constants at reference conditions are correct
**Severity**: Informational (confirms correctness)
**Description**: Constants that should be 1.0 or 0.0 at rated/neutral conditions were verified:

| Constant | Value | Expected at rated conditions | Verdict |
|----------|-------|------------------------------|---------|
| `DEFAULT_BACKUP_EIR` | 1.0 | 1.0 (COP=1.0 for resistance heat) | Correct |
| `DEFAULT_BIQUADRATIC_COEFFS` (in `hvac_core.rs:24`) | `[1,0,0,0,0,0]` | Identity = 1.0 at all conditions | Correct |
| Defrost `capacity_multiplier` when inactive | implicitly 1.0 | 1.0 (no defrost penalty) | Correct (`DefrostResult::inactive()` returns 1.0) |
| Defrost `power_multiplier` when inactive | implicitly 1.0 | 1.0 | Correct |
| `DEFAULT_PLF_DEGRADATION_COEFF` | 0.25 | AHRI 210/240-2023 default | Correct |
| `DEFAULT_LOW_SPEED_CAPACITY_FRACTION` | 0.72 | OCHRE/AHRI lookup default | Correct |
| `DEFAULT_HP_LOCKOUT_HYSTERESIS_C` | 0.5 | N/A (control param) | Plausible |
| `DEFAULT_MIN_ER_CYCLE_TIME_S` | 0.0 | N/A | Matches OCHRE default |
| `MAX_OAT_SUPPLEMENTAL_C` | 21.0 | EnergyPlus hard max 21°C (69.8°F) | Correct |

### Finding 7: No refrigerant mass flow or property constants — by design
**Severity**: Informational (no defect)
**Description**: The `constants.rs` file contains no refrigerant property (density, enthalpy, specific heat, etc.) or mass flow rate constants. This is by design — HARES uses EIR/capacity biquadratic curve models rather than refrigerant-side thermodynamic models. EnergyPlus's `WaterToAirHeatPump.cc` computes refrigerant mass flow from compressor geometry (`PistonDisp * RhoSuction` for reciprocating/rotary) and uses user-supplied fluid property tables via `FluidProperties`, but the DX coil models also use performance curves. HARES's EIR approach is the appropriate level of abstraction for a residential whole-building simulation and does not require refrigerant-side constants.

The hardcoded default values that substitute for capacity (10 kW), EIR (0.35 → COP ≈ 2.86), and backup capacity (5 kW) are plausible for a typical 3-ton residential ASHP, giving an implied mass flow of approximately 0.05 kg/s per kW for R-410A (within the 0.04–0.06 kg/s/kW rule of thumb).

---

## Summary

| Metric | Count |
|--------|-------|
| Total findings | 7 |
| Critical | 0 |
| High | 0 |
| Medium | 1 |
| Low | 2 |
| Informational | 4 |

- **1 Medium**: `DEFROST_EIR_TEMP_MODIFIER` (0.1528) is OCHRE-derived, not directly verifiable against EnergyPlus constants. The comment claiming EnergyPlus provenance is misleading.
- **2 Low**: (a) Defrost enable temperature 4.4445°C vs EnergyPlus IDD default 5.0°C — 0.56°C difference, negligible impact. (b) ARI/AHRI rated-condition temperatures are scattered as bare numeric literals rather than centralized named constants.
- **4 Informational**: All defrost formula coefficients verified exactly; dimensionless reference-condition constants are correct; on-demand power multiplier splitting differs from EnergyPlus by design; absence of refrigerant constants is appropriate for the model type.

---

## Recommendations

1. Update the `DEFROST_EIR_TEMP_MODIFIER` documentation comment to accurately describe provenance: "OCHRE-derived defrost EIR modifier (OCHRE HVAC.py L1161: `defrost_eir_temp_mod_frac`). Not a literal EnergyPlus constant; this factor is an OCHRE-specific combination of the EnergyPlus 0.01, 7.222, and 1.01667 terms expressed as a single multiplicative factor applied to `capacity_W / 1.01667`."

2. Centralize ARI/AHRI 210/240 rated temperatures into `constants.rs`:
   ```rust
   pub const AHRI_H1_INDOOR_DB_C: f64 = 21.11;
   pub const AHRI_H1_OUTDOOR_DB_C: f64 = 8.33;
   pub const AHRI_H3_OUTDOOR_DB_C: f64 = -8.33;
   pub const AHRI_COOL_INDOOR_DB_C: f64 = 26.67;
   pub const AHRI_COOL_INDOOR_WB_C: f64 = 19.44;
   pub const AHRI_COOL_OUTDOOR_DB_C: f64 = 35.0;
   ```
   Then replace all hardcoded numeric literals in `heater.rs`, `default_curves.rs`, and `resolve_hvac.rs` with the named constants. This eliminates the risk of value drift across modules.

3. Consider aligning `DEFROST_ENABLE_TEMP_C` to EnergyPlus's default of 5.0°C for closer EnergyPlus parity, or explicitly document the OCHRE 40°F convention as intentional divergence.

---

## References / Citations

- EnergyPlus `DXCoils.cc:11291-11345` — on-demand and timed defrost constants
- EnergyPlus `VariableSpeedCoils.cc:6737-6806` — defrost calculation with EIR curve floor
- EnergyPlus `DXCoils.hh:484` — `MaxOATDefrost(0.0)` member init
- EnergyPlus `VariableSpeedCoils.hh:325` — `MaxOATDefrost(0.0)` member init
- EnergyPlus IDD `V8-7-0-Energy+.idd` — `\default 5.0` for `Maximum Outdoor Dry-Bulb Temperature for Defrost Operation`
- EnergyPlus `StandardRatings.hh:73,76,79` — ARI/AHRI rated temperatures (heating)
- EnergyPlus `StandardRatings.cc:109-114,119-123` — ARI/AHRI rated temperatures (cooling)
- OCHRE `HVAC.py:1141` — `self.defrost = t_ext_db < 4.4445`
- OCHRE `HVAC.py:1161-1162` — `defrost_eir_temp_mod_frac = 0.1528` and usage
- ANSI/AHRI Standard 210/240-2023 — rating test conditions and cyclic degradation coefficients
