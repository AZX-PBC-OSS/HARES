# Perez epsilon formula code duplication
**Review ID**: types-physics-02
**Category**: types-physics
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/solar.rs`
- `crates/hares-envelope/src/thermal_solver/solar.rs`
- `crates/hares-envelope/src/longwave_radiation.rs`

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/SolarShading.cc` (lines 2670–2741)

## Findings

### Finding 1: [Severity: low] Intra-file duplication of epsilon formula in `hares-physics`, not cross-crate
**Description**: The review premise identified epsilon duplication across three crates. On inspection, the Perez epsilon formula is **not** duplicated across crates. The `hares-envelope` thermal solver and longwave radiation modules do not compute epsilon at all — the thermal solver receives pre-computed `SurfaceIrradiance` values, and the longwave module deals with thermal infrared radiation (Stefan-Boltzmann T⁴ exchange). The actual duplication is **within a single file**: `crates/hares-physics/src/solar.rs`.

**Code Location**:
- `crates/hares-physics/src/solar.rs:266-268` — in `perez_sky_diffuse()`
- `crates/hares-physics/src/solar.rs:446-448` — in `perez_tilted_irradiance()`

Both compute:
```rust
let zenith_rad_cubed = zenith_rad * zenith_rad * zenith_rad;
let epsilon = ((dhi + dni) / dhi + PEREZ_KAPPA * zenith_rad_cubed)
    / (1.0 + PEREZ_KAPPA * zenith_rad_cubed);
```

These are byte-for-byte identical and use the same `PEREZ_KAPPA = 1.041` constant at line 64.

**Root Cause**: `perez_sky_diffuse` is a standalone function computing only the sky-diffuse irradiance component, while `perez_tilted_irradiance` is the "full" surface irradiance API. Both independently compute epsilon rather than sharing it through a helper. However, the two functions use **different** airmass and delta formulations — `perez_sky_diffuse` uses simple secant airmass (`1/cos(zenith)`) and accepts `dni_extra` as an explicit parameter, while `perez_tilted_irradiance` uses the Kasten-Young airmass and computes extraterrestrial irradiance from day-of-year. These differences are intentional and documented (lines 258–259), meaning a naive extraction of epsilon into a shared helper would need to also parameterise airmass/delta computation, which reduces the benefit.

**Impact**: The code is correct and the duplication is minimal (5 lines). Both callers already route through the same `perez_bin()` function and `PEREZ_COEFFICIENTS` table, which are correctly non-duplicated. Refactoring epsilon into a standalone function would add an abstraction with marginal value.

### Finding 2: [Severity: low] Epsilon bins and coefficients consistent across all occurrences, match EnergyPlus exactly
**Description**: The epsilon binning thresholds and Perez (1990) all-weather coefficients are consistent across all internal references and match the EnergyPlus SolarShading.cc reference implementation byte-for-byte.

**Code Location**:
- Bins: `crates/hares-physics/src/solar.rs:61` — `[1.065, 1.23, 1.5, 1.95, 2.8, 4.5, 6.2]`
- Kappa: `crates/hares-physics/src/solar.rs:64` — `1.041`
- Coefficients: `crates/hares-physics/src/solar.rs:33-58` — `PEREZ_COEFFICIENTS`

**Vendor comparison**:
- EnergyPlus `SolarShading.cc:2680`: `{1.065, 1.23, 1.5, 1.95, 2.8, 4.5, 6.2}` — identical bins
- EnergyPlus `SolarShading.cc:2727`: `1.041 * pow_3(ZenithAng)` — identical kappa
- EnergyPlus `SolarShading.cc:2728`: `((BeamSolarRad + DifSolarRad) / DifSolarRad + KappaZ3) / (1.0 + KappaZ3)` — identical formula
- EnergyPlus `SolarShading.cc:2682-2688`: F11R–F23R arrays — identical coefficients

**Root Cause**: N/A — this is correct behavior.

**Impact**: No issue. Consistency is maintained with the reference implementation.

### Finding 3: [Severity: low] Longwave radiation module does not erroneously recompute epsilon
**Description**: The `crates/hares-envelope/src/longwave_radiation.rs` module was flagged as a potential erroneous epsilon recomputation site. It does not compute or reference Perez epsilon at all. The module deals exclusively with thermal longwave (infrared) radiation exchange using sky view factors, β splitting factors, and Stefan-Boltzmann T⁴ radiosity. The variable name `epsilon` appears in its tests only as **emissivity** (ε for glass at 0.84), which is an unrelated physical quantity.

**Code Location**: `crates/hares-envelope/src/longwave_radiation.rs:1-514` (entire module)

**Impact**: No issue. The module correctly relies on upstream solar computation.

## Summary
- Total findings: 3
- Critical: 0 / High: 0 / Medium: 0 / Low: 3

## Recommendations
1. **No action required.** The epsilon formula duplication within `hares-physics/src/solar.rs` is low-severity, involving 5 lines of identical code across two functions that intentionally differ in their airmass and delta formulations. The binning thresholds and coefficients are consistent and match EnergyPlus. The `hares-envelope` crate correctly consumes pre-computed irradiance without recomputing epsilon.
2. If the long-term preference is a single-source-of-truth for epsilon, a private `fn perez_epsilon(dhi: f64, dni: f64, zenith_rad: f64) -> f64` helper could be extracted in `solar.rs`. However, the delta computation (airmass × DHI / ETI) would remain function-specific by design, limiting the refactoring benefit.

## References / Citations
- Perez et al. (1990), "An anisotropic hourly diffuse radiation model for sloping surfaces." *Solar Energy* 44(5):271–289.
- EnergyPlus Engineering Reference, "Anisotropic Sky Model" (SolarShading.cc `AnisoSkyViewFactors`).
- `crates/hares-physics/src/solar.rs:266-268` — `perez_sky_diffuse` epsilon computation.
- `crates/hares-physics/src/solar.rs:446-448` — `perez_tilted_irradiance` epsilon computation.
- `crates/hares-physics/src/solar.rs:61` — `PEREZ_EPSILON_BINS` definition.
- `vendors/EnergyPlus/src/EnergyPlus/SolarShading.cc:2680` — EnergyPlus epsilon bin definition.
- `vendors/EnergyPlus/src/EnergyPlus/SolarShading.cc:2727-2728` — EnergyPlus epsilon formula.
