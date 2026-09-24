# Soil thermal diffusivity default (0.05 m²/day) vs EnergyPlus (0.0208) — quantify effect at 0-3m depth
**Review ID**: phys-const-13
**Category**: physics-constants
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/ground.rs:28` — `DEFAULT_SOIL_DIFFUSIVITY_M2_PER_DAY = 0.05`

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/EarthTube.cc:413-414` — EarthTube soil type diffusivity/conductivity arrays
- `vendors/EnergyPlus/src/EnergyPlus/EarthTube.hh` — EarthTube soil property struct
- `vendors/EnergyPlus/src/CalcSoilSurfTemp/SoilSurfTemp.f90:126-138` — CalcSoilSurfTemp preprocessor soil type lookup table
- `vendors/EnergyPlus/idd/versions/V26-1-0-Energy+.idd:1999-2029` — `Site:GroundDomain:Slab` defaults (k=1.5, ρ=2800, cp=850 → α=0.0545 m²/day)
- `vendors/EnergyPlus/src/EnergyPlus/GroundTemperatureModeling/KusudaAchenbachGroundTemperatureModel.cc:106-108` — Kusuda-Achenbach model: diffusivity computed from k/(ρ·cp), NO default
- `vendors/EnergyPlus/src/EnergyPlus/GroundTemperatureModeling/FiniteDifferenceGroundTemperatureModel.cc:421-424` — Finite-difference model: diffusivity from k/(ρ·cp), NO default
- `vendors/EnergyPlus/tst/EnergyPlus/unit/KusudaAchenbachGroundTemperatureModel.unit.cc:64-71` — Test values: k=1.08, ρ=980, cp=2570 → α=0.0371 m²/day
- `vendors/OCHRE/ochre/utils/schedule.py:248` — DOE-2 GTEMP correlation: implicit α = 0.025 ft²/hr = 0.0557 m²/day

## Findings

### Finding 1: [Severity: high] Docblock claims EnergyPlus default of 0.0208 m²/day — value does not exist in EnergyPlus source
**Description**: The docblock at `ground.rs:28-30` states "EnergyPlus CalcSoilSurfTemp defaults to 0.0208 m²/day (2.4e-7 m²/s) for generic dry soil." This value was searched exhaustively across the entire EnergyPlus source tree (C++, Fortran, IDD definitions, unit tests, and documentation). It does **not** appear anywhere as a soil thermal diffusivity.

**Code Location**: `crates/hares-physics/src/ground.rs:28-30`

**Root Cause**: The value 0.0208 appears to be either (a) from a different version of EnergyPlus no longer in the vendor tree, (b) confused with a different parameter (e.g. a material thickness of 0.0208 m found in `testfiles/PCMThermalStorage.idf:590`), or (c) derived from a different reference entirely. A full grep of `vendors/EnergyPlus/` for `0.0208` yielded only heat pump curve coefficients, window optics data, and gas-phase chemistry polynomials — all unrelated to soil diffusivity.

**Impact**: The comparison in the docblock misrepresents EnergyPlus's actual soil thermal diffusivity defaults, potentially misleading users about how HARES's default relates to the reference tool.

**Actual EnergyPlus soil thermal diffusivity values** (from `CalcSoilSurfTemp/SoilSurfTemp.f90:128-137` and `EarthTube.cc:413`):

| Soil Type | α (m²/hr) | α (m²/day) |
|-----------|-----------|------------|
| Heavy and Saturated | 0.0032544 | 0.07811 |
| Heavy and Damp | 0.0023220 | 0.05573 |
| Heavy and Dry | 0.0018576 | 0.04458 |
| Light and Dry | 0.0010080 | 0.02419 |

Additionally, `Site:GroundDomain:Slab` defaults (k=1.5, ρ=2800, cp=850) imply α = 0.0545 m²/day. The Kusuda-Achenbach model itself has **no** default diffusivity (it's a required computed field). EnergyPlus tests use α ≈ 0.037–0.038 m²/day.

The lowest EnergyPlus value is 0.02419 m²/day for "Light and Dry" — still 16% higher than the claimed 0.0208.

### Finding 2: [Severity: medium] Quantified effect at 0–3m depth: HARES 0.05 matches EnergyPlus Slab/GroundDomain default (0.0545) within 8%
**Description**: Using the Kusuda-Achenbach model with realistic parameters (T_mean = 12°C, T_amp = 10°C, phase_day = 35, τ = 365 days), the amplitude attenuation A(z)/A(0) = exp(-z·√(π/(α·τ))) and phase lag Δφ_days = z·√(π/(α·τ))·τ/(2π) were computed for HARES (0.05) and the two most relevant EnergyPlus defaults.

**Code Location**: `crates/hares-physics/src/ground.rs:72-88` (function `kusuda_achenbach_temp`)

**Impact**:

#### Comparison A: HARES 0.05 vs EnergyPlus GroundDomain:Slab default (0.0545)

| Depth (m) | HARES amp fraction | EP GD:Slab amp fraction | Ratio | HARES lag (days) | EP GD:Slab lag (days) | Δ lag |
|-----------|--------------------|------------------------|-------|-------------------|------------------------|-------|
| 0.0       | 1.000              | 1.000                  | 1.00  | 0.0               | 0.0                    | 0.0   |
| 0.5       | 0.813              | 0.820                  | 0.99  | 12.1              | 11.5                   | 0.6   |
| 1.0       | 0.660              | 0.672                  | 0.98  | 24.1              | 23.1                   | 1.0   |
| 2.0       | 0.436              | 0.452                  | 0.96  | 48.2              | 46.2                   | 2.0   |
| 3.0       | 0.288              | 0.304                  | 0.95  | 72.3              | 69.3                   | 3.0   |

**Result**: HARES and EnergyPlus GroundDomain:Slab are essentially identical. At 3m, HARES predicts 5% less amplitude attenuation and a 3-day larger phase lag — differences negligible for building energy simulation.

#### Comparison B: HARES 0.05 vs EnergyPlus "Light and Dry" (0.02419) — worst case

| Depth (m) | HARES amp fraction | EP Light&Dry amp fraction | Ratio | HARES lag (days) | EP L&D lag (days) | Δ lag |
|-----------|--------------------|--------------------------|-------|-------------------|--------------------|-------|
| 0.0       | 1.000              | 1.000                    | 1.00  | 0.0               | 0.0                | 0.0   |
| 0.5       | 0.813              | 0.742                    | 1.10  | 12.1              | 17.3               | 5.2   |
| 1.0       | 0.660              | 0.551                    | 1.20  | 24.1              | 34.7               | 10.6  |
| 2.0       | 0.436              | 0.303                    | 1.44  | 48.2              | 69.3               | 21.1  |
| 3.0       | 0.288              | 0.167                    | 1.72  | 72.3              | 104.0              | 31.7  |

**Result**: Using Light & Dry soil (arid climates) with HARES's default would overestimate ground temperature amplitude by 10–72% and underestimate phase lag by 5–32 days, depending on depth.

#### Comparison C: HARES 0.05 vs OCHRE (DOE-2, 0.0557 m²/day)

| Depth (m) | HARES amp fraction | OCHRE amp fraction | Ratio | HARES lag (days) | OCHRE lag (days) | Δ lag |
|-----------|--------------------|--------------------|-------|-------------------|-------------------|-------|
| 0.0       | 1.000              | 1.000              | 1.00  | 0.0               | 0.0               | 0.0   |
| 0.5       | 0.813              | 0.822              | 0.99  | 12.1              | 11.4              | 0.7   |
| 1.0       | 0.660              | 0.675              | 0.98  | 24.1              | 22.8              | 1.3   |
| 2.0       | 0.436              | 0.456              | 0.96  | 48.2              | 45.7              | 2.5   |
| 3.0       | 0.288              | 0.308              | 0.94  | 72.3              | 68.5              | 3.8   |

**Result**: HARES and OCHRE/DOE-2 agree closely, consistent with their shared representation of moist/damp residential soil.

### Finding 3: [Severity: low] HARES 0.05 is appropriate for typical residential modeling
**Description**: Multiple independent model defaults converge near 0.05–0.056 m²/day for residential slab-on-grade and foundation heat transfer:
- EnergyPlus `Site:GroundDomain:Slab` default → 0.0545 m²/day
- EnergyPlus EarthTube "Heavy and Damp" → 0.0557 m²/day
- OCHRE/DOE-2 GTEMP correlation → 0.0557 m²/day (from 0.025 ft²/hr)
- HARES moist mixed clay/sand → 0.05 m²/day

This cluster of values represents a de-facto consensus for "typical" residential soil. Residential foundation-adjacent soil is typically disturbed, backfilled, and subject to drainage — conditions that make a damp/moist soil (0.04–0.07 m²/day) the representative default, rather than the "Light and Dry" extreme (0.0242) appropriate only for arid, unvegetated sites.

**Code Location**: `crates/hares-physics/src/ground.rs:31`

**Impact**: The default is well-chosen. The docblock's justification should be updated to reference the correct EnergyPlus defaults rather than the erroneous 0.0208 value.

## Summary
- Total findings: 3
- High: 1 (erroneous docblock claim about EnergyPlus default value)
- Medium: 1 (quantified deviation from worst-case EnergyPlus soil type)
- Low: 1 (default value appropriateness confirmed)

## Recommendations

1. **Correct the docblock** at `ground.rs:28-30`: Remove the claim about EnergyPlus defaulting to 0.0208 m²/day. Replace it with an accurate comparison — e.g. "EnergyPlus `Site:GroundDomain:Slab` defaults to k=1.5 W/m·K, ρ=2800 kg/m³, cp=850 J/kg·K yielding α ≈ 0.0545 m²/day; EnergyPlus EarthTube 'Heavy and Damp' uses 0.0557 m²/day. HARES 0.05 is consistent with these defaults for typical residential soil conditions."

2. **Verify the provenance of 0.0208**: If the value came from a specific EnergyPlus reference (e.g. older documentation, a particular Engineering Reference section, or the `GroundHeatExchanger:Slinky` test values of ~0.038/0.037 m²/day), cite that specific source. The value does not exist in the current vendor codebase.

3. **Consider exposing soil type selection**: For users in arid climates, a Light and Dry soil option (α ≈ 0.024 m²/day) would produce more conservative (damped, lagged) ground temperatures. The current single default may over-predict ground temperature variation in such climates.

## References / Citations
- Kusuda, T. and Achenbach, P.R. (1965), "Earth Temperatures and Thermal Diffusivity at Selected Stations in the United States", ASHRAE Transactions, Vol. 71(1), pp. 61-74.
- EnergyPlus EarthTube.cc:413-414 — soil type diffusivity array: [0.07811, 0.05573, 0.04458, 0.02419] m²/day
- EnergyPlus CalcSoilSurfTemp.f90:128-137 — identical values in m²/hr used by the preprocessor
- EnergyPlus IDD V26.1.0 `Site:GroundDomain:Slab` defaults (k=1.5, ρ=2800, cp=850) → α = 0.0545 m²/day
- OCHRE schedule.py:248 — DOE-2 WTH.f GTEMP subroutine: α = 0.025 ft²/hr = 0.0557 m²/day
- ASHRAE Handbook of Fundamentals 2021, Ch. 27 — below-grade heat transfer boundary conditions
- EnergyPlus Engineering Reference, Ch. 3.17 — Undisturbed Ground Temperature Model: Kusuda-Achenbach
