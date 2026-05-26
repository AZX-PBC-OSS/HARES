# OMNI_AZIMUTH_SAMPLES=12: verify integration quality for tilts 0-90°; quantify integration error
**Review ID**: solar-deep-05
**Category**: solar-deep
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/solar.rs:308-384` — `OMNI_AZIMUTH_SAMPLES` constant, `omni_directional_irradiance` function, integration loop
- `crates/hares-physics/src/solar.rs:393-483` — `perez_tilted_irradiance` (azimuth-dependent components)
- `crates/hares-physics/src/solar.rs:149-161` — `angle_of_incidence` (AOI formula)
- `crates/hares-core/src/environment.rs:527-561` — call site
- `crates/hares-physics/src/solar.rs:2235-2292` — test `omni_vertical_wall_direct_matches_analytical_dni_over_pi`
- `crates/hares-physics/src/solar.rs:2369-2453` — test `omni_sample_counts_converge_for_vertical_wall`

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/SolarShading.cc:145-150` — NTheta=24 azimuth patches for shading ratios
- `vendors/EnergyPlus/src/EnergyPlus/SurfaceGeometry.cc:1304` — geometric sky view factor `(1 + cos β)/2`
- `vendors/EnergyPlus/third_party/ssc/shared/lib_irradproc.cpp:1860-2021` — isotropic/Perez tilted irradiance (no azimuth integration)
- `vendors/OCHRE/ochre/utils/envelope.py:54-75` — pvlib Perez with cardinal-direction defaults

## Methodology

The `omni_directional_irradiance` function averages the Perez tilted irradiance model across N uniformly-spaced azimuth samples to handle surfaces with unknown orientation. Only two terms in the Perez decomposition depend on surface azimuth γ:

1. **Direct beam**: `DNI × max(0, cos(AOI))`
2. **Circumsolar diffuse**: `DHI × F1 × max(0, cos(AOI)) / b`

All other components — isotropic diffuse, horizon brightening, and ground-reflected — are azimuth-independent and exact under arithmetic mean.

The integration error was quantified by comparing N=12 vs N=720 azimuth samples of `max(0, cos(AOI))` across tilt angles 0°, 15°, 30°, 45°, 60°, 75°, 90° and solar zenith angles 10°–85°. The AOI formula used matches the HARES implementation at `solar.rs:158-159`:

```
cos(AOI) = sin(alt)×cos(β) + cos(alt)×sin(β)×cos(γ_s − γ)
```

## Findings

### Finding 1: [Severity: high] Documentation claims 0.5% convergence; actual error is ~2.3% for vertical walls
**Description**: The doc comment on `OMNI_AZIMUTH_SAMPLES` at line 319 states "12 samples (every 30°) provides convergence within 0.5% of the analytical cos-weighted mean for direct beam on vertical surfaces." Quantified analysis shows the actual error is ~2.3% across all zenith angles.

**Code Location**: `crates/hares-physics/src/solar.rs:312-318`, `crates/hares-physics/src/solar.rs:329`

**Root Cause**: The claim is arithmetically incorrect. For a vertical wall (tilt=90°), the analytical azimuth-averaged `max(0, cos(AOI))` equals `sin(θ_z) / π`. The N=12 arithmetic mean converges to a constant 2.30% error below the analytical value regardless of zenith angle:

| Zenith | Analytical sin(θz)/π | N=12 | Error | N=24 | N=36 |
|--------|---------------------|------|-------|------|------|
| 30° | 0.159155 | 0.155502 | 2.30% | 0.158245 (0.57%) | 0.158751 (0.25%) |
| 45° | 0.225079 | 0.219913 | 2.30% | 0.223792 (0.57%) | 0.224507 (0.25%) |
| 60° | 0.275664 | 0.269338 | 2.30% | 0.274088 (0.57%) | 0.274964 (0.25%) |
| 85° | 0.317099 | 0.309821 | 2.30% | 0.315285 (0.57%) | 0.316293 (0.25%) |

The convergence rate is O(1/N²) for this smooth periodic integrand. N=30 is the minimum required for <0.5% error; N=36 achieves ~0.25%.

**Impact**: Misleading documentation. The actual ~2.3% error is tolerable for engineering purposes (see Finding 4), but the docstring should be corrected. The test at line 2263 correctly uses a 5% tolerance, which is internally consistent with the true error but inconsistent with the docstring claim.

### Finding 2: [Severity: medium] Integration error grows monotonically with tilt angle; maximum at vertical (tilt=90°)
**Description**: The N=12 maximum absolute error in `avg[max(0, cos(AOI))]` ranges from 0 (tilt=0°) to 0.0073 (tilt=90°). Vertical surfaces experience the largest integration error.

**Code Location**: `crates/hares-physics/src/solar.rs:338-384`, integration loop lines 356-369

**Root Cause**: At tilt=0°, `cos(AOI) = sin(alt)`, which is azimuth-independent — the arithmetic mean is exact. As tilt increases, the azimuth-dependent term `cos(alt)×sin(β)×cos(γ_s−γ)` grows in magnitude, and the `max(0, ...)` clipping creates non-zero regions that the discrete sampling must approximate.

Quantified maximum absolute errors in `avg[max(0, cos(AOI))]`:

| Tilt | N=12 max abs err | N=12 max rel err | N=24 max abs err | N=36 max abs err |
|------|-----------------|------------------|-----------------|-----------------|
| 0° | 0.000000 | 0.00% | 0.000000 | 0.000000 |
| 15° | 0.001127 | 0.70% | 0.000213 | 0.000062 |
| 30° | 0.002908 | 1.00% | 0.000692 | 0.000293 |
| 45° | 0.003389 | 0.89% | 0.000945 | 0.000504 |
| 60° | 0.003366 | 0.85% | 0.001333 | 0.000483 |
| 75° | 0.004885 | 1.53% | 0.001218 | 0.000516 |
| 90° | 0.007264 | 2.30% | 0.001808 | 0.000802 |

**Impact**: Vertical surfaces with unknown azimuth (the primary use case for omnidirectional averaging) experience the largest integration error. For N=36, the max error drops to ≤0.0008 across all tilts.

### Finding 3: [Severity: medium] Worst-case irradiance error reaches ~10 W/m² for vertical surfaces at high zenith
**Description**: Translating the cos(AOI) integration error into irradiance terms, the worst-case combined direct+circumsolar error for a vertical surface at zenith=85° under clear-sky conditions (DNI=850, DHI=150, F1≈0.3) is approximately 8.8 W/m².

**Code Location**: `crates/hares-physics/src/solar.rs:356-382` (meaning of the accumulated sums)

**Root Cause**: The error in `avg[max(0, cos(AOI))]` propagates directly into the direct beam term (`DNI × error`) and the circumsolar diffuse term (`DHI × F1 × error / b`). At high zenith angles, the `b = max(cos(zenith), cos(85°))` floor amplifies the circumsolar error. Per-tilt worst-case irradiance errors:

| Tilt | Worst zenith | Direct err (W/m²) | Circum err (W/m²) | Total err (W/m²) |
|------|-------------|--------------------|--------------------|--------------------|
| 15° | 82° | 0.96 | 0.30 | 1.26 |
| 30° | 74° | 2.47 | 0.40 | 2.87 |
| 45° | 64° | 2.88 | 0.29 | 3.17 |
| 60° | 50° | 2.86 | 0.20 | 3.06 |
| 75° | 84° | 4.15 | 1.75 | 5.90 |
| 90° | 84° | 6.17 | 2.61 | 8.78 |

Mean absolute errors (averaged over all zeniths) are significantly smaller:
| Tilt | Mean abs err in cos(AOI) | Mean irrad err (W/m²) |
|------|-------------------------|-----------------------|
| 90° | 0.0050 | ~4.3 |
| 75° | 0.0015 | ~1.3 |
| 45° | 0.0007 | ~0.6 |

**Impact**: For annual whole-building energy simulation, the mean errors (~1-4 W/m²) are well within typical engineering tolerances. Peak-condition errors at high zenith angles (~6-9 W/m²) are borderline but typically occur at low absolute irradiance (sunrise/sunset), limiting their thermal significance.

### Finding 4: [Severity: low] Existing tests correctly use 5% tolerance; docstring should reflect this reality
**Description**: The test `omni_vertical_wall_direct_matches_analytical_dni_over_pi` (line 2263) uses `tolerance = analytical * 0.05` (5%), which correctly accounts for the ~2.3% N=12 sampling error with headroom. The convergence test `omni_sample_counts_converge_for_vertical_wall` (line 2425) similarly uses 5% for N=12 vs N=36. These test tolerances match the actual error characteristics but contradict the docstring claim of 0.5%.

**Code Location**: `crates/hares-physics/src/solar.rs:2263`, `crates/hares-physics/src/solar.rs:2425`

**Root Cause**: The doc comment at line 312-318 likely originated from an idealized continuous-integral estimate (rectangle-rule convergence) without accounting for the `max(0, ...)` clipping that introduces non-smoothness at the integration boundary. The test author correctly calibrated against empirical measurements, producing 5% tolerances that reflect actual behavior.

**Impact**: No functional impact; purely a documentation consistency issue. The 5% tolerance in tests is appropriate for N=12.

### Finding 5: [Severity: low] HARES is the only codebase performing azimuth integration for diffuse sky; other codes use geometric view factors
**Description**: EnergyPlus and OCHRE both compute sky diffuse irradiance using purely tilt-dependent view factors `(1 + cos β) / 2`, with no azimuth integration. EnergyPlus uses 24 azimuth patches (NTheta=24 at `SolarShading.cc:147`) only for shading-ratio computation, not for irradiance. HARES's omnidirectional averaging approach is novel and more sophisticated than industry practice.

**Code Location**: `crates/hares-physics/src/solar.rs:308-384`, `vendors/EnergyPlus/src/EnergyPlus/SurfaceGeometry.cc:1304`, `vendors/OCHRE/ochre/utils/envelope.py:54-75`

**Root Cause**: EnergyPlus's geometric view factors assume isotropic sky conditions for the base diffuse term. Perez anisotropy is then applied as multipliers to these tilt-only view factors. OCHRE delegates to pvlib's `get_total_irradiance` with a single azimuth. HARES's approach of averaging over azimuth samples handles the full Perez model including circumsolar asymmetry.

**Impact**: HARES's approach is more physically accurate for surfaces with unknown orientation but adds computational cost (12× per surface per timestep). The improvement over pure tilt-based view factors is moderate since isotropic diffuse dominates the sky contribution for typical sky conditions.

## Summary
- **Total findings**: 5
- **Critical**: 0
- **High**: 1 (docstring claims 0.5% convergence; actual is ~2.3%)
- **Medium**: 2 (integration error grows with tilt; worst-case irradiance error ~10 W/m²)
- **Low**: 2 (test tolerances are correct; HARES approach is novel vs industry)

## Recommendations

1. **Correct the docstring** at `solar.rs:312-318` to state the actual ~2.3% convergence for N=12 vertical walls, or reference the 5% test tolerance. Suggested replacement text: "12 samples (every 30°) provides convergence within 5% of the analytical cos-weighted mean for direct beam on vertical surfaces; N=36 converges within 0.3%."

2. **Consider raising `OMNI_AZIMUTH_SAMPLES` to 24 or 36.** N=24 reduces the max error from 2.3% to 0.57% (near the claimed 0.5%) at 2× computational cost. N=36 achieves 0.25% at 3× cost. For a typical simulation with ~5 omni-directional surfaces and 8760 timesteps, N=24 adds ~0.5M additional Perez evaluations per year — a modest overhead given the improved accuracy.

3. **If keeping N=12**, acknowledge in documentation that vertical surfaces with unknown azimuth carry a ~2.3% systematic low bias in direct beam and circumsolar diffuse irradiance relative to the true azimuth-averaged value.

4. **Add a convergence test at moderate tilt** (e.g., 45°) to complement the existing vertical-only convergence test at line 2369, since the error characteristics differ by tilt angle.

## References / Citations
- Perez, R., Ineichen, P., Seals, R., Michalsky, J., & Stewart, R. (1990). "An anisotropic hourly diffuse radiation model for sloping surfaces." *Solar Energy* 44(5):271-289.
- Duffie, J.A. & Beckman, W.A. (2020). *Solar Engineering of Thermal Processes*, 5th ed. Eq. 1.6.2 — angle of incidence formula.
- EnergyPlus Engineering Reference: sky diffuse solar radiation on tilted surfaces (geometric view factors).
- `vendors/EnergyPlus/src/EnergyPlus/SolarShading.cc:145-150` — NTheta=24, NPhi=6 sky patch discretization.
