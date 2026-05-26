# Dehumidifier default curves: EnergyPlus coefficient scaling and conversion
**Review ID**: hvaccfg-08
**Category**: hvac-config
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/hvac/dehumidifier_defaults.rs

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/ZoneDehumidifier.cc` — curve evaluation (lines 686–755), validation (lines 202–203, 305–336), rated-condition initialisation (lines 540–579)
- `vendors/EnergyPlus/src/EnergyPlus/ZoneDehumidifier.hh` — struct field declarations (lines 84–98)
- `vendors/EnergyPlus/testfiles/WindACRHControl.idf` — reference curve coefficients (lines 2036–2068)
- `vendors/EnergyPlus/testfiles/SingleFamilyHouse_HP_Slab_Dehumidification.idf` — identical reference curves (lines 5112–5136)
- `vendors/EnergyPlus/idd/versions/V26-1-0-Energy+.idd` — `ZoneHVAC:Dehumidifier:DX` object definition (lines 36912–37011)
- `crates/hares-physics/src/biquadratic.rs` — `BiquadraticCurve` implementation (lines 1–56)
- `crates/hares-equipment/src/hvac/dehumidifier.rs` — curve consumer (lines 233–283 init, 524–535 evaluate)

## Findings

### Finding 1: [Severity: low]
**Description**: RH extrapolation domain wider than EnergyPlus calibrated bounds.
**Code Location**: `dehumidifier.rs:43` and `dehumidifier.rs:262–263`
**Root Cause**: `DEFAULT_RH_BOUNDS` is set to `(RH_MIN_FRACTION, RH_MAX_FRACTION)` = `(0.0, 1.0)`, representing 0–100% RH. The EnergyPlus `Curve:Biquadratic` objects in `WindACRHControl.idf` define `Minimum Value of y = 40.0` and `Maximum Value of y = 80.0` (i.e., 0.40–0.80 fraction). EnergyPlus `Curve::value()` clamps inputs to the curve’s declared domain. HARES’s `BiquadraticCurve::evaluate()` also clamps to `x2_bounds`, but since those bounds are (0.0, 1.0), the HARES dehumidifier **never clamps RH inputs**, even below 40% or above 80%. At extreme RH values (e.g., 15% or 95%), HARES extrapolates the biquadratic beyond its calibration domain, producing potentially unrealistic water-removal or energy-factor values.
**Impact**: At RH < 0.40 the extrapolated curve may return negative values (e.g., at 21°C/10%RH: −1.36 for water removal, already guarded by `.max(0.0)` at `dehumidifier.rs:189`), or underpredicted outputs below the physical domain. At RH > 0.80 the extrapolation may overstate removal. EnergyPlus would clamp to 40%/80% and produce a bounded value; HARES produces an unconstrained extrapolation. In practice, dehumidifiers rarely operate at <40% RH so the discrepancy is low-severity.

### Finding 2: [Severity: low]
**Description**: `WATTS_PER_KILOWATT_HOUR` constant name is dimensionally misleading.
**Code Location**: `dehumidifier.rs:46`
**Root Cause**: The constant `const WATTS_PER_KILOWATT_HOUR: f64 = 3_600_000.0` is named as if it holds a watt-to-kWh conversion (which would be 1/1000 = 0.001). Its actual value 3,600,000 is **joules per kilowatt-hour** (1 kWh = 3.6 × 10^6 J). The name suggests watts-per-kWh when the true semantics are joules-per-kWh. This is a naming-only issue; the constant is used at line 193 in `water_removal_kg_s * WATTS_PER_KILOWATT_HOUR / energy_factor_l_kwh`, where the dimensional analysis `(kg/s) × (J/kWh) / (L/kWh) = J/s = W` (since kg ≈ L for water) is correct.
**Impact**: No computational error. Risk of future misuse if a developer reads the name literally and expects 0.001 instead of 3.6M.

## Items Verified (Correct)

The following aspects of the HARES implementation are verified correct against the EnergyPlus vendor reference:

### (a) Coefficient scaling from RH-percent to RH-fraction
Mathematically verified line-by-line. EnergyPlus curve form:
```
f_ep(T, RH_%) = a + b·T + c·T² + d·RH_% + e·RH_%² + f·T·RH_%
```
HARES curve form (RH as fraction [0,1]):
```
f_hares(T, RH_frac) = a + b·T + c·T² + d'·RH_frac + e'·RH_frac² + f'·T·RH_frac
```
Since `RH_% = 100 × RH_frac`, the correct transformation is:
- `d' = d × 100` (confirmed: 0.050053043874 × 100 = 5.0053043874)
- `e' = e × 10000` (confirmed: −0.000203629282 × 10000 = −2.03629282)
- `f' = f × 100` (confirmed: −0.000341750531 × 100 = −0.0341750531)
- `a`, `b`, `c` unchanged

Numerically verified that `f_ep(26.6667, 60.0) == f_hares(26.6667, 0.60)` to machine precision for both curves. Commentary in `dehumidifier_defaults.rs:14` accurately describes the transformation.

### (b) RATED_DB_C = 26.666…°C (80°F exact)
`RATED_DB_C = 26.666_666_666_666_7` = (80 − 32) × 5/9 = 80°F exact, verified to machine precision. EnergyPlus uses two slightly different constants:
- `RatedInletAirTemp = 26.7` for curve validation at `ZoneDehumidifier.cc:202`
- `RatedAirDBTemp = 26.6667` for mass-flow initialisation at `ZoneDehumidifier.cc:567`

HARES’s choice of the exact 80°F value is defensible for normalisation purposes. The comment at `dehumidifier_defaults.rs:17–19` correctly cites both EnergyPlus conventions and explains the divergence. No rounding error is introduced — the self-normalisation in `evaluate_normalized_curve` (`dehumidifier.rs:530–534`) divides each curve evaluation by its rated value, making the rated capacity pass through exactly at (26.6667°C, 60%RH).

### (c) RH percent-to-fraction consistency
All RH inputs and outputs are consistently fractional [0,1] throughout the dehumidifier model:
- `RATED_RH = 0.60` (dehumidifier_defaults.rs:64)
- `zone.relative_humidity` from `EnvironmentState` is fractional
- Control signal `target_rh` is accepted in both percent (>1.0) and fraction via `parse_rh_fraction` (dehumidifier.rs:484–499)
- Curve coefficients are scaled for fractional input (see Finding 1 on bounds, but the scaling itself is correct)
- Telemetry reports RH as "fraction" (dehumidifier.rs:592)

### (d) Low-RH "quadratic linear minimum" curve transition
EnergyPlus `ZoneHVAC:Dehumidifier:DX` does **not** implement a low-RH quadratic-linear transition. The `ZoneDehumidifier.cc` code was searched exhaustively for `QuadraticLinear`, `LowRH`, `MinDir`, `RhMin`, and RH-cutoff patterns — no matches were found. The zone dehumidifier model requires `numDims == 2` (biquadratic only) for both `WaterRemovalCurve` and `EnergyFactorCurve` (lines 305, 325). The only RH-related constraint in the model is the curve’s own domain clamping, which EnergyPlus enforces through `Curve::value()` internal to its `CurveManager`.

The desiccant dehumidifier model (`DesiccantDehumidifiers.cc`) uses different, unrelated polynomial forms. There is no evidence in the EnergyPlus source that a low-RH piecewise transition exists for the DX zone dehumidifier.

HARES correctly does not implement such a transition, and its approach (simple biquadratic evaluation) matches EnergyPlus behaviour modulo the clamping domain difference noted in Finding 1.

### (e) Coefficient values match stated source
The HARES default coefficients (both water-removal and energy-factor) are exact scaled copies of the `Curve:Biquadratic` objects from `WindACRHControl.idf` lines 2036–2060 (and identically from `SingleFamilyHouse_HP_Slab_Dehumidification.idf` lines 5112–5136). All six coefficients per curve were verified with bit-level precision after scaling.

The "expected" approximate values suggested in the review instructions (a≈0.6, b≈−0.01, c≈0.0002, d≈0.02×100) do not match the `WindACRHControl.idf` curves. The actual Water Removal coefficients from EnergyPlus are:
```
a = −2.724878664080     (vs expected ~0.6)
b =  0.100711983591     (vs expected ~−0.01)
c = −0.000990538285     (vs expected ~0.0002)
d =  0.050053043874     (vs expected ~0.02)
e = −0.000203629282     (matches expected ~−0.0002)
f = −0.000341750531     (vs expected ~0.0001)
```
Only coefficient `e` is within the expected range. The expected values appear to reference a different (unidentified) dehumidifier curve set, not the WindACRHControl curves that HARES explicitly sources. The HARES values are correct for their stated origin.

### Electrical power formula equivalence
EnergyPlus (`ZoneDehumidifier.cc:852–853`):
```
ElectricPower = WaterRemoval (L/day) / (EnergyFactor (L/kWh) × 24 h/d) × 1000 (W/kW)
```
HARES (`dehumidifier.rs:191–196`):
```
electric_power = water_removal_kg_s × 3_600_000 (J/kWh) / energy_factor (L/kWh)
```
Algebraically equivalent: `(L/d) / (EF × 24) × 1000 = (L/d × 1/86400 kg·s/d·L) × 3600000 / EF` (since `1000/24 = 3600000/86400 = 125/3`). Identical results confirmed numerically at the 30 L/day / 2.0 L/kWh operating point (both yield ≈625 W).

## Summary
- Total findings: 2
- Critical: 0
- High: 0
- Medium: 0
- Low: 2

## Recommendations
1. Consider setting production `x2_bounds` to `(0.40, 0.80)` to match the EnergyPlus curve domain and prevent extrapolation outside the calibrated RH range. The test-only constants `DEFAULT_RH_BOUNDS_FROM_CURVE` at `dehumidifier_defaults.rs:76` already hold these values. This would align clamping behaviour with EnergyPlus at the cost of making the dehumidifier output constant at extreme RH — which is what EnergyPlus does.
2. Rename `WATTS_PER_KILOWATT_HOUR` to `JOULES_PER_KILOWATT_HOUR` to accurately reflect that the value is 3.6 × 10^6 J/kWh, not watts-per-kWh.

## References / Citations
- EnergyPlus `ZoneDehumidifier.cc`: RH conversion at line 688 (`InletAirRH = 100.0 * PsyRhFnTdbWPb(...)`); curve evaluation at lines 695 and 733; rated validation constants at lines 202–203; mass-flow initialisation with 26.6667°C at lines 567–568
- EnergyPlus `WindACRHControl.idf`: Water Removal coefficients at lines 2038–2043; Energy Factor coefficients at lines 2051–2056; curve RH bounds (40–80%) at lines 2046–2047
- HARES `dehumidifier_defaults.rs`: scaled coefficients at lines 28–50; RATED_DB_C at line 58; RATED_RH at line 64
- HARES `dehumidifier.rs`: curve bounds at lines 42–43; normalisation at lines 277–280; performance evaluation at lines 165–206
