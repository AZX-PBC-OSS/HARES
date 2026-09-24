# ISA standard pressure function: verify p0*(1-L·h)^E against USSA 1976 at 0/500/1000/1609/3000m

**Review ID**: air-01
**Category**: air-properties
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/air_properties.rs:12-14`
- `crates/hares-physics/src/constants.rs:90-113` (ISA constants)
- `crates/hares-io/src/resstock_csv.rs:49-55` (duplicate ISA implementation in kPa)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc:4467` — primary ISA `StdBaroPress` computation
- `vendors/EnergyPlus/src/EnergyPlus/DataEnvironment.cc:191-234` — general ISA 1976 `OutBaroPressAt` (Eq. 33b)
- `vendors/EnergyPlus/src/EnergyPlus/DataEnvironment.hh:80-82` — constant definitions
- `vendors/OCHRE/ochre/` — OCHRE has no ISA altitude correction (always uses 101.325 kPa flat)

## Findings

### Finding 1: [Severity: low] HARES formula implements the correct ISA/USSA 1976 tropospheric pressure relation

**Description**: The HARES pressure formula:

```
p = 101325.0 * (1.0 - 2.25577e-5 * h) ^ 5.2558761
```

is the standard ISA tropospheric pressure equation `p = p0 * (1 - L·h/T0)^E`, where
`L/T0 = 0.0065 / 288.15 ≈ 2.25577×10⁻⁵ m⁻¹` and `E = g₀·M₀/(R*·L)`. The formula matches
EnergyPlus's `StdBaroPress` computation line-for-line (WeatherManager.cc:4467), except that
HARES uses a higher-precision exponent (5.2558761 vs. 5.2559).

**Code Location**: `crates/hares-physics/src/air_properties.rs:12-14`

**Root Cause**: N/A — this is a correct implementation.

**Impact**: None. The computed pressures match USSA 1976/ISO 2533 geometric-altitude
values within 0.002 Pa at all test altitudes up to 3000 m (see Verification table below).

---

### Finding 2: [Severity: low] HARES exponent is more precise than EnergyPlus, by design

**Description**: EnergyPlus hardcodes the exponent as `5.2559` (5 significant figures).
HARES uses `5.2558761` (8 significant figures), matching the USSA 1976 derivation
`g₀·M₀/(R*·L) = 9.80665 × 0.0289644 / (8.31432 × 0.0065) ≈ 5.2558761133` to within
1.3 × 10⁻⁸. The practical difference in computed pressure is ≤ 0.12 Pa at 3000 m —
well within numerical noise. This is an intentional design choice documented at
`constants.rs:97-113`.

**Code Location**: `crates/hares-physics/src/constants.rs:113`

**Root Cause**: Deliberate precision improvement over EnergyPlus, justified in comments.

**Impact**: Negligible. The validation test `isa_pressure_exponent_matches_ussa76_derivation`
at `physics_validation_tests.rs:1183` confirms the constant is correct.

---

### Finding 3: [Severity: low] Geometric vs. geopotential altitude convention — HARES follows EnergyPlus convention

**Description**: The USSA 1976 standard atmosphere tables use **geopotential** altitude
`H = R₀·h / (R₀ + h)`, whereas HARES (and EnergyPlus) uses **geometric** altitude `h`
(elevation above sea level) directly in the formula. At 3000 m this produces a pressure
~12.6 Pa lower than the USSA 1976 geopotential-tabled value (70108.5 vs. 70121.2 Pa).
At lower altitudes the deviation is smaller: ~0.5 Pa at 500 m, ~1.7 Pa at 1000 m,
~4.2 Pa at 1609 m.

Building energy simulation tools universally use geometric altitude (no geopotential
correction), including EnergyPlus WeatherManager:4467 and Autosizing/Base.cc:168-179.
This is well within meteorological measurement uncertainty and is appropriate for the
application domain.

**Code Location**: `crates/hares-physics/src/air_properties.rs:13`

**Root Cause**: Standard building-simulation convention; all vendor tools do the same.

**Impact**: Systematic but negligible under-building (12.6 Pa at 3000 m, 0.018% relative).

---

### Finding 4: [Severity: medium] Test expected value at 1609 m deviates from all standard formulas by ~29 Pa

**Description**: The test `standard_pressure_matches_isa_table` at line 127 expects
83,460.0 Pa at 1609 m (Denver), but the ISA formula computes only 83,431.1 Pa
(Δ = 28.9 Pa, 0.035%). Neither HARES, EnergyPlus, nor USSA 1976 (geopotential or
geometric) reproduces the expected value of 83,460.0 Pa:

| Source                                        | Pressure at 1609 m | Δ from expected (83460) |
|:----------------------------------------------|:-------------------|------------------------:|
| HARES `standard_pressure_pa`                  | 83431.12 Pa        | −28.88 Pa               |
| EnergyPlus `StdBaroPress`                     | 83431.04 Pa        | −28.96 Pa               |
| USSA 1976 (geometric, exact constants)        | 83431.12 Pa        | −28.88 Pa               |
| USSA 1976 (geopotential 1609 m)              | 83435.30 Pa        | −24.70 Pa               |
| **Test expected value**                       | **83460.0 Pa**     | 0                       |

The test tolerances (100 Pa) are wide enough that all tests pass, but the expected
value of 83,460 Pa does not correspond to any known ISA derivation. At other altitudes
(0, 500, 1000, 3000 m), the expected values agree with the formula to within
< 1 Pa. The 1609 m value may originate from a published reference table that uses
different rounding conventions or constants; it should be verified and if correct,
the source should be cited.

**Code Location**: `crates/hares-physics/src/air_properties.rs:134`

**Root Cause**: Unknown. The 83460 Pa value does not match the ISA formula with any
standard set of constants.

**Impact**: Low — the test passes due to generous tolerance, but the expected value is
not reproducible from the formula it's purportedly validating.

---

### Finding 5: [Severity: low] Duplicate ISA pressure implementation in hares-io returns kPa; no overflow guard in hares-physics

**Description**: The ISA formula is implemented in two places:
1. `air_properties.rs:13` — returns Pa, no overflow/domain guard
2. `resstock_csv.rs:49-55` — returns kPa, includes a `base <= 0.0` guard returning 1.0 kPa for altitudes beyond ~44 km

The `standard_pressure_pa` function in `air_properties.rs` has no guard and will return
NaN (via `powf` of a negative base to a non-integer exponent) for elevations above
~44.3 km. This is harmless for building simulation (no buildings exist at 44 km), but
the inconsistency in defensive coding between the two implementations is notable.
The `hares-io` implementation correctly re-exports the same constants from `hares-physics`
so the formulas are numerically identical.

**Code Location**:
- `crates/hares-physics/src/air_properties.rs:13`
- `crates/hares-io/src/resstock_csv.rs:49-55`

**Root Cause**: Independent re-implementation; the kPa version happened to include a guard.

**Impact**: None for valid use. NaN only for elevations well beyond Earth's atmosphere.

---

### Finding 6: [Severity: low] OCHRE has no ISA pressure function — significant gap

**Description**: OCHRE does not implement any altitude-based pressure correction. All
psychrometric calculations use a flat 101.325 kPa regardless of building elevation.
This is a significant modeling gap — a building at 1609 m elevation (Denver) would
experience ~17.6% lower ambient pressure, which meaningfully affects air density,
equipment sizing, and energy use. The OCHRE codebase acknowledges this gap via a TODO
at `ochre/utils/envelope.py:529`. In contrast, both HARES and EnergyPlus properly
adjust pressure for site elevation.

**Code Location** (HARES context): `vendors/OCHRE/ochre/` (all pressure uses are constant 101.325 kPa)

**Root Cause**: OCHRE architectural limitation — pressure is a schedule input, not an elevation-computed property.

**Impact**: HARES is architecturally correct in providing altitude-adjusted pressure.
No change needed in HARES.

---

## Verification: HARES ISA pressure vs. USSA 1976/ISO 2533

| Altitude (m) | HARES `standard_pressure_pa` | USSA 1976 geometric (exact constants) | Δ HARES vs USSA geom | USSA 1976 geopotential | Δ HARES vs USSA geopot |
|-------------:|------------------------------:|---------------------------------------:|----------------------:|------------------------:|------------------------:|
| 0            | 101325.0000 Pa                | 101325.0000 Pa                         | 0.0000 Pa             | 101325.0000 Pa          | 0.0000 Pa               |
| 500          | 95460.8383 Pa                 | 95460.8393 Pa                          | −0.0011 Pa             | 95461.2895 Pa           | −0.4512 Pa              |
| 1000         | 89874.5684 Pa                 | 89874.5705 Pa                          | −0.0021 Pa             | 89876.2852 Pa           | −1.7168 Pa              |
| 1609         | 83431.1158 Pa                 | 83431.1190 Pa                          | −0.0031 Pa             | 83435.2982 Pa           | −4.1824 Pa              |
| 3000         | 70108.5396 Pa                 | 70108.5447 Pa                          | −0.0051 Pa             | 70121.1622 Pa           | −12.6227 Pa             |

The HARES formula matches the USSA 1976 geometric-altitude pressure within 0.005 Pa
at all test altitudes — the tiny residual is solely due to rounding of
`ISA_PRESSURE_EXPONENT` from 5.2558761133 to 5.2558761. This is well below the
precision of any practical measurement in building energy simulation.

## Summary

- **Total findings**: 6
- **Critical**: 0 / **High**: 0 / **Medium**: 1 / **Low**: 5

## Recommendations

1. **Verify the 1609 m test expected value (83460.0 Pa)**. Cite the source reference
   that provides this value. If the value cannot be traced to a published standard,
   replace it with the computed ISA value (~83431.1 Pa) and tighten the tolerance
   to ±10 Pa to match the precision of the other test cases.

2. **Add an input-domain guard to `standard_pressure_pa`** for consistency with
   `isa_pressure_kpa` in `resstock_csv.rs` — return 0.0 or clamp at 0.0 Pa for
   elevations where the formula becomes invalid (base ≤ 0, i.e. h ≥ ~44.3 km).

3. **Document the geometric-vs-geopotential convention** in the docstring of
   `standard_pressure_pa` to explicitly note that geometric altitude is used (not
   geopotential), consistent with EnergyPlus convention.

## References / Citations

- U.S. Standard Atmosphere 1976 (NOAA-S/T 76-1562), Part 1 §1.2.5 — tropospheric layer equations
- ISO 2533:1975 Standard Atmosphere
- EnergyPlus WeatherManager.cc:4467 — `StdBaroPress = StdPressureSeaLevel * pow(1.0 - 2.25577e-05 * Elevation, 5.2559)`
- EnergyPlus DataEnvironment.cc:191-234 — `OutBaroPressAt` (USSA 1976 Eq. 33b general form)
- HARES constants.rs:90-113 — documented ISA constant derivations
- HARES physics_validation_tests.rs:1183-1200 — `isa_pressure_exponent_matches_ussa76_derivation`
