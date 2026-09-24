# Angle of incidence for tilted surfaces: verify cos(theta) formula; check gimbal lock at zenith
**Review ID**: solar-deep-02
**Category**: solar-deep
**Date**: 2026-05-26

## Files Reviewed
crates/hares-physics/src/solar.rs:149-163

## Vendor/Reference Files Consulted
- vendors/EnergyPlus/src/EnergyPlus/SolarShading.cc:2745-2783 (AOI via vector dot product, clamping, Perez application)
- vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc:4227-4313 (SOLCOS calculation, zenith/azimuth formulas)
- vendors/OCHRE/ochre/utils/envelope.py:54-59 (direct `pvlib.irradiance.aoi()` call)
- vendors/OCHRE/ochre/utils/envelope.py:137-185 (solar position via `pvlib.solarposition.get_solarposition()`)
- .venv/lib/python3.14/site-packages/pvlib/irradiance.py:162-242 (aoi_projection and aoi functions)
- crates/hares-physics/src/solar.rs:88-161 (solar_position and angle_of_incidence implementations)
- crates/hares-physics/src/solar.rs:867-869 (only dedicated AOI unit test)
- crates/hares-physics/src/solar.rs:1451-1502 (Perez cross-validation test using AOI via perez_tilted_irradiance)

## Findings

### Finding 1: [Severity: low]
**Description**: The HARES `angle_of_incidence` function operates on `solar_alt` (solar altitude), while every reference implementation — pvlib's `aoi()`, pvlib's `aoi_projection()`, EnergyPlus's `CosIncAngBeamOnSurface`, and the standard Duffie & Beckman (2020) formulation — operates on `solar_zenith`. The difference is that `alt = 90° − zenith`, making `sin(alt) = cos(zenith)` and `cos(alt) = sin(zenith)`. The parameter name `solar_alt` clearly signals the convention to human readers, but the Rust compiler provides no guard. A caller (or future test-writer) who passes zenith thinking it is altitude would produce silently wrong results.

**Code Location**: `crates/hares-physics/src/solar.rs:149-154`
```rust
pub fn angle_of_incidence(
    surface_tilt_deg: f64,
    surface_azimuth_deg: f64,
    solar_alt: f64,           // ← altitude, not zenith
    solar_az: f64,
) -> f64 {
```

**Root Cause**: The `solar_position()` function returns `altitude_deg` as a natural output (computed as `90° − zenith_deg`). The AOI function was written to accept altitude directly for convenience, avoiding a `90.0 − alt` conversion at each call site. This is a valid design choice, but it diverges from the industry-standard API surface.

**Impact**: Existing call sites in the same module all correctly compute `90.0 − zenith` before calling (solar.rs:416, 976, 1543). Risk is confined to future external callers or maintenance errors. No runtime bug exists today.

### Finding 2: [Severity: low]
**Description**: Gimbal lock at zenith is handled correctly by the AOI formula due to a numerical cancellation property, but the `solar_position()` function itself returns an arbitrary azimuth value of 180° (south) when the sun is exactly at the zenith. This stems from `atan2(0.0, 0.0)` in the azimuth computation, which Rust's IEEE 754 defines as returning `0.0` — leading to an azimuth of `(0 + π) mod 2π = π = 180°`. While the AOI formula suppresses the azimuth dependence via `cos(alt) ≈ 0` at zenith, other consumers of `SolarPosition.azimuth_deg` (e.g., display, logging, or future control-system logic) could be misled by latitude-dependent arbitrary azimuth values near zenith.

**Code Location**: `crates/hares-physics/src/solar.rs:135-140`
```rust
let numerator = hour_angle_rad.sin();
let denominator = hour_angle_rad.cos() * lat_rad.sin() - decl_rad.tan() * lat_rad.cos();
let azimuth_rad =
    (numerator.atan2(denominator) + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU);
```

**Root Cause**: Spherical coordinates have a coordinate singularity at the zenith — the azimuth component is undefined when the zenith angle is zero. This is an inherent property of any spherical-to-Cartesian decomposition, not a bug in the implementation. However, the value returned (`180°` for `atan2(0, 0)`) is an arbitrary convention in IEEE 754, not a physically meaningful result. EnergyPlus avoids this entirely by working in Cartesian sun-direction cosines (`SOLCOS` vector), never explicitly computing solar azimuth for AOI.

**Impact**: 
- AOI function is unaffected: at altitude = 90° (zenith = 0°), `cos(alt) = 0` eliminates the azimuth term entirely, giving `cos(AOI) = cos(tilt)` which is analytically correct. At altitude = 89.9°, the azimuth term is weighted by `cos(89.9°) ≈ 0.0017`, attenuating any azimuth error by ≈ 590×.
- The `solar_position()` function returns a solar azimuth of 180° at zenith regardless of the observer's latitude. For display or diagnostic purposes, this arbitrariness should be documented.
- No functional impact on irradiance computations observed in any test.

### Finding 3: [Severity: medium]
**Description**: Test coverage for the `angle_of_incidence` function is insufficient. Only one dedicated test exists (`south_facing_vertical_aoi_matches_analytic_case` at line 867), and it covers only one tilt/azimuth combination (south-facing vertical at 45° altitude). There is no systematic cross-validation against pvlib's `aoi()` output for a grid of tilt, azimuth, zenith, and solar-azimuth inputs, despite the codebase already having a full pvlib integration pipeline (`tests/python/generate_pvlib_solar_override.py`) and pvlib-generated CSV fixtures. A single sign error in the `az_delta` computation (e.g., `solar_az - surface_azimuth_deg` vs `surface_azimuth_deg - solar_az`) could go undetected for many configurations.

**Code Location**: `crates/hares-physics/src/solar.rs:867-869`
```rust
fn south_facing_vertical_aoi_matches_analytic_case() {
    let aoi = angle_of_incidence(90.0, 180.0, 45.0, 180.0);
    assert!((aoi - 45.0).abs() <= 0.1, "aoi={aoi}");
}
```

**Root Cause**: The AOI function was implemented as part of the broader Perez model, and its test coverage was treated as incidental to the Perez integration tests. The Perez tests do exercise the AOI function indirectly (via `perez_tilted_irradiance` → `angle_of_incidence`), but only for a small number of parameter combinations.

**Impact**: Low probability of an undetected defect given the formula's simplicity, but high impact if one exists. A sign error in `az_delta` would silently produce incorrect AOI values for any configuration where surface and solar azimuths differ, which encompasses the vast majority of real-world scenarios. The existing test (same azimuth for sun and surface, `az_delta = 0`) would not catch this class of error.

## Summary
- Total findings: 3
- Critical: 0 | High: 0 | Medium: 1 | Low: 2

## Recommendations

1. **Systematic AOI cross-validation test (Medium priority)**. Add a `#[test]` that computes AOI via `angle_of_incidence` for a grid of at least 9 parameter tuples (3 tilts × 3 azimuth differences) and compares the result against a precomputed array of pvlib-derived reference values. The reference values can be computed once via the existing `generate_pvlib_solar_override.py` script or via an inline Python `pvlib.irradiance.aoi()` call captured during test generation. This guards against sign errors in `az_delta` and validates the full spherical geometry.

2. **Document azimuth singularity at zenith (Low priority)**. Add a doc comment on `solar_position()` noting that the azimuth is arbitrary (180° north-referenced) when the sun is exactly at the zenith, and that downstream consumers requiring azimuth at near-zenith positions should be aware of potential numerical instability. Alternatively, return `Option<f64>` for azimuth above a zenith-angle threshold (e.g., `zenith < 0.5°`), though this would be an API change.

3. **Consider adding a `solar_alt` doc comment to `angle_of_incidence` (Low priority)**. Add an explicit doc comment: `solar_alt: Solar altitude angle [degrees] (90° − zenith). See pvlib.irradiance.aoi for the zenith-based equivalent.` This reduces the risk of parameter confusion for future callers.

## Formula Verification: Mathematical Proof

The HARES implementation:
```
cos(AOI) = sin(alt)·cos(β) + cos(alt)·sin(β)·cos(γ_s − γ)
```

The pvlib implementation (`aoi_projection`):
```
cos(AOI) = cos(θ_z)·cos(β) + sin(θ_z)·sin(β)·cos(γ_s − γ)
```

Since `alt = 90° − θ_z` → `sin(alt) = cos(θ_z)` and `cos(alt) = sin(θ_z)`, the two formulas are **identical**.

The Duffie & Beckman (2020) Eq. 1.6.2 five-term expansion:
```
cos(θ) = sin(δ)sin(φ)cos(β) − sin(δ)cos(φ)sin(β)cos(γ)
       + cos(δ)cos(φ)cos(β)cos(ω) + cos(δ)sin(φ)sin(β)cos(γ)cos(ω)
       + cos(δ)sin(β)sin(γ)sin(ω)
```
also reduces to the same standard form after substituting the solar position identities, as shown in the standard text.

The EnergyPlus vector dot-product approach:
```
cos(AOI) = SOLCOS · OutNormVec
```
is also mathematically equivalent (spherical-to-Cartesian conversion of unit vectors yields the same expression).

All four formulations — HARES, pvlib, Duffie & Beckman, and EnergyPlus — are mathematically identical.

## Vector Formulation Robustness at Zenith

The coordinate-free vector formulation confirms no gimbal lock in the AOI:
```
sun = [sin(θ_z)·sin(γ_s), sin(θ_z)·cos(γ_s), cos(θ_z)]
norm = [sin(β)·sin(γ), sin(β)·cos(γ), cos(β)]
cos(AOI) = sun·norm
         = sin(θ_z)·sin(β)·[sin(γ_s)·sin(γ) + cos(γ_s)·cos(γ)] + cos(θ_z)·cos(β)
         = sin(θ_z)·sin(β)·cos(γ_s − γ) + cos(θ_z)·cos(β)
         = cos(alt)·sin(β)·cos(γ_s − γ) + sin(alt)·cos(β)
```

At zenith (`θ_z = 0`): `sun = [0, 0, 1]`, and `cos(AOI) = cos(β)` regardless of the arbitrary azimuth. The formula is well-conditioned everywhere.

## References / Citations
- Duffie, J.A. & Beckman, W.A. (2020). *Solar Engineering of Thermal Processes*, 5th ed. Eq. 1.6.2 (p. 14) — five-term angle-of-incidence expansion.
- pvlib-python 0.15.0, `irradiance.py`, `aoi_projection()` and `aoi()`. Available: https://github.com/pvlib/pvlib-python
- EnergyPlus Engineering Reference v24.2, SolarShading.cc: `CosIncAngBeamOnSurface` dot-product AOI calculation.
- OCHRE v0.3, `utils/envelope.py:54-59` — delegates AOI to `pvlib.irradiance.aoi()`.
