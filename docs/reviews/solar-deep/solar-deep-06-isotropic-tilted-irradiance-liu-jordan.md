# Isotropic tilted irradiance (Liu-Jordan): verify view factor to sky = (1+cos(β))/2, to ground = (1-cos(β))/2 — compare pvlib-python
**Review ID**: solar-deep-06
**Category**: solar-deep
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/solar.rs:12` — `ISOTROPIC_VIEW_FACTOR` constant definition
- `crates/hares-physics/src/solar.rs:485–522` — `isotropic_tilted_irradiance` function (standalone Liu-Jordan model)
- `crates/hares-physics/src/solar.rs:394–483` — `perez_tilted_irradiance` function (incorporates isotropic view factors for the isotropic halo component and ground reflection)
- `crates/hares-physics/src/solar.rs:240–252` — `perez_sky_diffuse` fallback to isotropic (`ISOTROPIC_VIEW_FACTOR` at line 251)
- `crates/hares-physics/tests/solar_parity.rs:244–294` — pvlib reference parity tests for POA irradiance

## Vendor/Reference Files Consulted
- **EnergyPlus**: `vendors/EnergyPlus/src/EnergyPlus/SurfaceGeometry.cc` (lines 1304–1310, 4874–4895, 9384–9409, 9554–9557, 14666) — view factor definition `0.5*(1±cosβ)` in 5 code locations; `SolarShading.cc:2632–2795` — `AnisoSkyViewFactors()` uses Liu-Jordan as the isotropic component of the Perez model; `HeatBalanceSurfaceManager.cc:2829–2838` — final irradiance assembly
- **OCHRE**: `vendors/OCHRE/ochre/Models/Envelope.py:268` — sky view factor `((1+cos(β))/2)^1.5` (LW only, with EnergyPlus exponent modification); `vendors/OCHRE/ochre/utils/envelope.py:62–74` — delegates solar transposition to pvlib `get_total_irradiance(model="perez")`
- **pvlib-python 0.15.0**: referenced in parity test `solar_parity.rs:267–293` and `generate_pvlib_solar_override.py:146–155`

## Findings

### Finding 1: [Severity: low] — View factor formulas are verified correct; exact match to Liu & Jordan (1963), EnergyPlus, and pvlib-python

**Description**: The isotropic view factor to sky and ground in HARES uses the standard Liu-Jordan formulation:

| Component | HARES formula | Literal form | Verified against |
|-----------|--------------|--------------|-----------------|
| Sky view factor | `(1.0 + cos β) * 0.5` | (1+cosβ)/2 | EnergyPlus, pvlib |
| Ground view factor | `(1.0 - cos β) * 0.5` | (1-cosβ)/2 | EnergyPlus, pvlib |
| Sum at β=0° | `1.0 + 0.0 = 1.0` | View factors sum to 1 | Verified |
| Sky at β=90° | `0.5 * (1+0) = 0.5` | 0.5 | Verified |
| Ground at β=90° | `0.5 * (1-0) = 0.5` | 0.5 | Verified |

**Code Location**:
- `solar.rs:12` — `const ISOTROPIC_VIEW_FACTOR: f64 = 0.5;`
- `solar.rs:512` — diffuse: `(dhi * (1.0 + tilt_rad.cos()) * ISOTROPIC_VIEW_FACTOR)` = `DHI × (1+cosβ)/2`
- `solar.rs:513` — reflected: `(ghi * ground_albedo * (1.0 - tilt_rad.cos()) * ISOTROPIC_VIEW_FACTOR)` = `GHI × ρ × (1-cosβ)/2`
- `solar.rs:465` — Perez isotropic halo: `(1.0 - f1) * (1.0 + tilt_rad.cos()) * ISOTROPIC_VIEW_FACTOR`
- `solar.rs:474` — Perez ground reflected: `(ghi * ground_albedo * (1.0 - tilt_rad.cos()) * ISOTROPIC_VIEW_FACTOR)` (same formula, correctly independent of Perez anisotropy)

**Root Cause**: N/A — this is a confirmation that the implementation is correct.

**Impact**: None. The view factors are mathematically identical to the canonical Liu & Jordan (1963) formulation. EnergyPlus uses the same expressions verbatim (`0.5 * (1.0 + cosTilt)`, `0.5 * (1.0 - cosTilt)`) in `SurfaceGeometry.cc` at 5 initialization sites. The pvlib-python isotropic model and the ground-reflected component of the Perez model also use these exact formulas.

---

### Finding 2: [Severity: low] — Ground-reflected component matches pvlib-python to machine precision

**Description**: The pvlib Perez reference case (zenith=30°, tilt=30°, south-facing, GHI=900, albedo=0.2, day_of_year=80) produces `poa_ground_diffuse=12.0577 W/m²`. HARES computes ground-reflected identically:

```
HARES:  GHI × ρ × 0.5 × (1 − cos 30°)
     = 900 × 0.2 × 0.5 × (1 − 0.8660254…)
     = 900 × 0.2 × 0.0669873…
     = 12.0577 W/m²
```

The full POA comparison (`poa_total_matches_pvlib_reference` at `solar_parity.rs:267–293`) confirms HARES POA = pvlib POA = 925.4807 W/m² within 1% tolerance. The difference in the sky-diffuse component (113.42 vs. the isotropic-only 93.30) is expected and correct — it reflects the Perez circumsolar and horizon brightening terms (f1, f2) which the Liu-Jordan model intentionally omits.

**Code Location**: `solar.rs:513` (ground reflected in isotropic), `solar.rs:474` (ground reflected in Perez), `solar_parity.rs:273` (pvlib reference value comment)

**Root Cause**: N/A — confirmation finding.

**Impact**: None. pvlib parity is verified.

---

### Finding 3: [Severity: low] — Independent test coverage for β=0° and β=90° edge cases is present and passing

**Description**: HARES includes dedicated inline tests validating the two critical edge cases:

- **β=0°** (horizontal surface): `isotropic_horizontal_receives_full_diffuse` at `solar.rs:892–907` asserts diffuse = DHI (full sky) and reflected ≈ 0.0.
- **β=90°** (vertical surface): `isotropic_vertical_receives_half_diffuse` at `solar.rs:910–920` asserts diffuse = DHI/2.
- **Isotropic vs. Perez consistency**: `perez_overcast_close_to_isotropic` at `solar.rs:987–1020` confirms that under overcast conditions (high DHI fraction) the Perez model produces diffuse within 20% of the isotropic model.
- **Perez fallback**: `perez_extreme_zenith_falls_back_to_isotropic` at `solar.rs:1023–1050` verifies that at zenith > 87° the Perez model correctly falls back to isotropic (line 424–435).

**Code Location**: `solar.rs:892–907`, `solar.rs:910–920`, `solar.rs:987–1020`, `solar.rs:1023–1050`

**Root Cause**: N/A — confirmation finding.

**Impact**: None. Edge-case coverage is adequate.

---

### Finding 4: [Severity: low] — Doc comments for `isotropic_tilted_irradiance` do not specify parameter units (degrees vs. radians)

**Description**: The function `isotropic_tilted_irradiance` (`solar.rs:489–497`) accepts `aoi: f64` and `surface_tilt: f64` without documenting that both are in degrees. The implementation applies `.to_radians()` at lines 508–509, confirming the degrees convention, but callers (including the Perez fallback path at line 426) rely on undocumented behavior. The `perez_tilted_irradiance` function at `solar.rs:394–395` has the same omission.

**Code Location**: `solar.rs:489–497`, `solar.rs:394–395`

**Root Cause**: Documentation gap — the internal convention is consistently degrees, but the public API does not state it.

**Impact**: Very low. All current callers pass degrees correctly. A future developer reading only the function signature could misuse it, though the test coverage would catch incorrect results.

**Recommendation**: Add doc comment lines clarifying that `aoi` and `surface_tilt` are in degrees, e.g.:
```
/// * `aoi` — angle of incidence [degrees]
/// * `surface_tilt` — surface tilt from horizontal [degrees]
```

---

## Summary
- Total findings: 4
- Critical / High / Medium / Low: 0 / 0 / 0 / 4

The Liu-Jordan isotropic tilted irradiance implementation in HARES is **correct**. The view factor to sky (`(1+cosβ)/2`) and to ground (`(1-cosβ)/2`) match the canonical formulation, EnergyPlus, and pvlib-python exactly. Both edge cases (β=0° and β=90°) are verified by inline tests. The ground-reflected component in the Perez model uses the same isotropic formula, which is correct per Liu-Jordan. No deviations, errors, or discrepancies were found. All four findings are confirmatory or documentation-level observations.

## Recommendations
1. Add parameter-unit documentation to `isotropic_tilted_irradiance` and `perez_tilted_irradiance` function doc comments clarifying that `aoi` and `surface_tilt`/`surface_tilt_deg` are in degrees.
2. Consider adding a `#[test]` that directly verifies ground-reflected = `GHI × albedo × 0.5 × (1 − cosβ)` against a hand-computed reference value for a specific angle (e.g., β=30° yields `12.0577` for GHI=900, albedo=0.2) — currently this is only implicitly tested through the pvlib POA parity test.

## References / Citations
- Liu, B.Y.H. & Jordan, R.C. (1963). "The long-term average performance of flat-plate solar-energy collectors." *Solar Energy* 7(2):53–74. doi:10.1016/0038-092X(63)90006-9
- Perez, R., Ineichen, P., Seals, R., Michalsky, J., & Stewart, R. (1990). "Modeling daylight availability and irradiance components from direct and global irradiance." *Solar Energy* 44(5):271–289. doi:10.1016/0038-092X(90)90055-H
- EnergyPlus Engineering Reference, "Sky Diffuse Solar Radiation on a Tilted Surface." https://bigladdersoftware.com/epx/docs/8-9/engineering-reference/sky-radiation-modeling.html
- pvlib-python 0.15.0, `pvlib.irradiance.get_total_irradiance`. https://github.com/pvlib/pvlib-python
- EnergyPlus: `vendors/EnergyPlus/src/EnergyPlus/SurfaceGeometry.cc:1304–1305` — view factor assignment
- EnergyPlus: `vendors/EnergyPlus/src/EnergyPlus/HeatBalanceSurfaceManager.cc:2829–2838` — `SurfSkySolarInc` and `SurfGndSolarInc`
