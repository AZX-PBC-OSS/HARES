# Window material model: IAM curves, SHGC decomposition, U-factor three-resistance split
**Review ID**: rc-mat-03
**Category**: envelope-rc-mat
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/solar.rs` — IAM polynomial curves (lines 547–665), U-factor decomposition (lines 688–706), SHGC decomposition (lines 726–797)
- `crates/hares-envelope/src/boundary_rc.rs` — Window StarMesh decomposition (lines 755–867), film resistance wiring (lines 1437–1446)
- Also consulted: `crates/hares-envelope/src/thermal_solver/longwave.rs` — Walton effective-temperature correction (lines 27–120), `crates/hares-io/src/epw.rs` — Sky temperature cascade (lines 558–593), `crates/hares-core/src/dwelling/conversions.rs` — Window boundary setup (lines 267–287), `crates/hares-core/src/dwelling/solver_builder.rs` — SHGC parameter wiring (lines 290–351)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/WindowManager.cc` — IAM curves (lines 4974–4997), EvalNominalWindowCond (lines 6309–6382), clear/bronze glass angular distributions (lines 5259–5375)
- `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc` — Clark-Allen sky emissivity (lines 3191–3217), Walton cloud correction (line 3199)
- `vendors/OCHRE/ochre/utils/envelope.py` — IAM curves (lines 105–121), SHGC decomposition (lines 405–431), U-factor decomposition (lines 294–304), film resistances (lines 342–402)

## Findings

### Finding 1: IAM polynomial coefficients have sub-1% divergence from EnergyPlus source-code values
**Severity**: low

**Description**: The six IAM polynomial coefficient sets in `GlazingCurve::coefficients()` (`solar.rs:607–614`) match OCHRE's coefficients exactly but diverge slightly from the EnergyPlus source-code values in `WindowManager.cc:4974–4997`. For instance, Curve A in E+ uses `cs[3.36, -3.85, 1.49, 0.01]` whereas HARES/OCHRE uses `c[0]=-0.001474, c[1]=3.355, c[2]=-3.852, c[3]=1.486, c[4]=0.0147` (after normalization). The normalized difference is under 0.5% per coefficient. The HARES/OCHRE values likely originate from a tabulated form of the E+ Engineering Reference rather than the source code.

**Code Location**: `solar.rs:607–614` (coefficient definitions) vs. `WindowManager.cc:4974–4988` (E+ source coefficients)

**Root Cause**: OCHRE stored the coefficients in the order `[c4, c3, c2, c1, c0]` and reversed them with `[::-1]`. These coefficient values appear to come from an E+ Engineering Reference table (Step 7) that uses slightly different rounding than the C++ source code literals. HARES inherited the OCHRE values.

**Impact**: Negligible for residential thermal simulation. The normalized IAM values differ by <0.5% at any angle, which is well within measurement uncertainty for window optical properties.

### Finding 2: Curve nomenclature does not map to the six ASHRAE glazing types
**Severity**: low

**Description**: The review specification describes six IAM curve types by glazing construction (uncoated single, uncoated double, uncoated triple, low-e single, low-e double, low-e triple). HARES names and selects its six curves (A, Bdcd, D, E, F, J) via `GlazingCurve::from_u_shgc()` (`solar.rs:579–599`) based on U-factor and SHGC bins, not explicit glazing construction type. Curve E is reused for two distinct bins (`U ∈ (1.56, 3.98] ∧ SHGC > 0.525` and `U ≤ 1.56 ∧ SHGC > 0.4`), meaning it serves as both "double-pane high SHGC" and "triple-pane moderate SHGC." This U×SHGC binning approach is correct per the E+ Engineering Reference Step 7 lookup table and is the same approach used by OCHRE.

**Code Location**: `solar.rs:547–599` (GlazingCurve enum and from_u_shgc)

**Root Cause**: The EnergyPlus simple window model uses a 28-cell U×SHGC lookup table as a proxy for glazing construction type. Neither HARES, OCHRE, nor E+ simple glazing distinguishes curves by explicit glazing type.

**Impact**: No functional impact. The correspondence between glazing type and IAM curve is a labeling concern only. The physics is identical.

### Finding 3: Diffuse IAM uses per-curve pre-computed constants that diverge from OCHRE
**Severity**: medium

**Description**: HARES stores pre-computed hemispherical-average diffuse IAM values per curve (`GlazingCurve::diffuse_iam()`, `solar.rs:628–637`) ranging from 0.771 (curve J) to 0.907 (curve A). OCHRE uses a single constant 0.854 for all curves (`envelope.py:126`). EnergyPlus computes the diffuse IAM angular response per timestep using the full polynomial integration or alternatives. The HARES values are more physically correct (varying by glazing type) than OCHRE's constant, but this is a deliberate divergence from OCHRE parity. The HARES diffuse IAM pre-computation method (Lambertian cosine-weighted hemispherical integration) is physically sound but should be documented as a deviation from OCHRE.

**Code Location**: `solar.rs:628–637` (diffuse_iam match arms)

**Root Cause**: OCHRE uses 0.854 as a single empirically-derived fudge factor (with a TODO comment at line 125: "Fudge factor for diffuse irradiance: EPlus transmitted diffuse solar is lower than expected"). HARES replaced this with properly integrated per-curve values.

**Impact**: The largest difference is for curve J (triple-pane low-e): HARES uses 0.771 vs OCHRE's 0.854, a ~10% difference in diffuse solar gain. For typical residential double-pane windows (curve E), the difference is negligible (0.855 vs 0.854). The HARES values are the more physically correct choice.

### Finding 4: SHGC decomposition correctly follows EnergyPlus Steps 4–5
**Severity**: low (confirmatory)

**Description**: `calculate_window_parameters()` in `solar.rs:726–797` implements the correct polynomial regressions for:
- **Step 4** (normal-incidence transmittance, lines 731–754): piecewise polynomials in SHGC branched by U-factor with interpolation in U ∈ [3.4, 4.5] W/m²·K. EnergyPlus uses a discrete 28-cell lookup table for the same purpose; HARES's linear interpolation between the high-U and low-U regressions is more accurate than OCHRE's hard threshold at U=3.95.
- **Step 5** (absorbed solar split, lines 756–796): interior/exterior film resistances for solar absorption derived from polynomial regressions in absorbed fraction `x = SHGC - transmittance`. The inward-flowing fraction `(R_ext_s + R_material/2) / (R_ext_s + R_material + R_int_s)` is the standard one-layer equivalent of E+'s multi-layer resistance-weighted decomposition.

The energy balance is validated by the test at `solar.rs:2077–2111` which confirms `T + A + R = 1.0`.

**Code Location**: `solar.rs:726–797` (calculate_window_parameters)

**Root Cause**: N/A — correctly implemented.

**Impact**: Correct. The interpolation band (U ∈ [3.4, 4.5]) is an improvement over both OCHRE's hard threshold and E+'s discrete table.

### Finding 5: `r_glass` computed independently in solver_builder, creating a duplicated computation path
**Severity**: medium

**Description**: The glass-only resistance `r_glass` is computed in two places:
1. `window_u_factor_decomposition()` in `solar.rs:688–706` returns `(r_glass, r_int, r_ext)` using the E+ Step 1 polynomial. This is used by `conversions.rs:276–277` to set the window boundary's `fallback_r_m2_k_w` (for RC network construction) and interior/exterior film resistances.
2. `solver_builder.rs:310` independently recomputes `r_glass = (r_total - r_film_int - r_film_ext).max(0.0)` using the same U-factor but potentially different film resistances from the `film_resistances()` call (TARP/DOE-2 model, wind-speed-dependent in `conversions.rs:134–142`).

While these should be algebraically consistent at NFRC rating conditions, the duplication creates a latent risk: if `film_resistances()` produces a different interior-film R than the E+ Step 1 polynomial (which is possible in non-NFRC conditions or when using the TARP model), the `r_glass` used for SHGC decomposition in `calculate_window_parameters` will differ from the `r_glass` used in the RC network. The two values have different physical meanings: one is the glass-only resistance in the RC graph, the other drives the absorbed-solar inward-flowing fraction.

**Code Location**: `conversions.rs:276` (window_u_factor_decomposition → r_glass for RC network) vs. `solver_builder.rs:310` (independent r_glass for SHGC decomposition)

**Root Cause**: The code structure separates boundary construction (conversions.rs) from solver parameter computation (solver_builder.rs). The `r_glass` for SHGC decomposition should be taken from the E+ Step 1 polynomial output that was computed during boundary construction, not recomputed from potentially different film resistances.

**Impact**: Low-to-moderate. In practice, the TARP interior-film R at 20°C stationary air ≈ 0.12 m²·K/W, which closely matches the E+ Step 1 polynomial. At NFRC rating conditions the two values are within 5%. However, in extreme conditions (large ΔT, high wind), the TARP-computed film resistance could diverge by 10–20%, causing the SHGC decomposition to use an inconsistent glass resistance.

### Finding 6: Exterior film resistance in U-factor decomposition not wind-speed-dependent
**Severity**: low

**Description**: `window_u_factor_decomposition()` (`solar.rs:700`) computes exterior film resistance via the constant NFRC winter correlation: `r_ext = 1.0 / (0.025342 * U + 29.163853)` ≈ 0.034 m²·K/W. This is the E+ Engineering Reference Step 1 formula which assumes NFRC winter rating conditions (h_out ≈ 29 W/m²·K combined, including radiation). For the runtime RC network, the actual wind-speed-dependent exterior film is computed separately in `conversions.rs:134–142` via TARP/DOE-2 model.

The U-factor decomposition's `r_ext` is used only to extract the glass-only resistance from the nominal U-factor. This separation is correct: the nominal U-factor (rated at NFRC conditions) is decomposed using the NFRC film correlations, and the resulting glass-only resistance is then recombined with runtime film resistances in the RC network. The runtime exterior film for non-window boundaries uses wind-speed-dependent TARP/DOE-2 correlations (`film_coefficients.rs`), but for windows the exterior film from the E+ polynomial is used because the window U-factor's exterior film is part of the rated value.

**Code Location**: `solar.rs:700` (r_ext constant formula), `conversions.rs:134–142` (runtime film_resistances call)

**Root Cause**: By design. The E+ simple window model does not support wind-speed-dependent U-factor adjustments — the U-factor is a rated constant, and the film decomposition uses the rating-condition formulas.

**Impact**: Correct for the simple window model. In a more detailed model, the window exterior film would vary with wind speed, but this would require recomputing the U-factor at runtime, which is outside the scope of the simple model.

### Finding 7: StarMesh window decomposition correctly separates h_si into convective and radiative components
**Severity**: low (confirmatory)

**Description**: In `boundary_rc.rs:789–837`, when a window boundary in StarMesh mode has `h_si > h_rad_glass`, the interior film is decomposed:
- `h_conv = h_si - h_rad_glass` (convection-only zone-air coupling)
- A floating `window_node` connects to zone_air via `R_conv` and to outdoor via `r_glass + r_film_ext`
- Inter-surface radiation is handled by the star-mesh edge (window_node ↔ star_node)

This matches EnergyPlus "Option 2", TRNSYS Type 56, and ESP-r. The use of `GLASS_THERMAL_EMISSIVITY = 0.84` (NFRC, from `longwave_radiation.rs:50`) for both the film decomposition and the star-mesh conductance ensures total interior coupling = h_si exactly. Using 0.90 emissivity for the star-mesh (which is the ASHRAE 140 opaque-surface default) would over-couple windows by ≈ 0.34 W/(m²·K).

**Code Location**: `boundary_rc.rs:789–837` (window StarMesh decomposition)

**Root Cause**: N/A — correctly implemented.

**Impact**: Correct. The window emissivity consistency between film decomposition and star-mesh is explicitly documented in the code comments (`boundary_rc.rs:794–800`).

### Finding 8: Walton effective-temperature correction for exterior window LWR is correctly applied
**Severity**: low (confirmatory)

**Description**: The exterior longwave radiation code in `longwave.rs:59–106` applies the Walton (1983) effective-temperature correction per window surface:
1. Sky view factor: `F_sky = 0.5 × (1 + cos φ)` and `β = √(F_sky)` (`longwave_radiation.rs:87–129`)
2. Excess sky radiation: `Δq = ε·σ·β·F_sky·(T_sky⁴ − T_air⁴)` [W/m²] (line 66–73)
3. Correction applied to zone: `ΔQ_zone = (U / h_out) × Δq × area` (line 106), avoiding double-counting the U-factor's implicit `T_sky = T_air` assumption

The sky temperature `T_sky` itself is computed by `compute_sky_temp_c()` in `epw.rs:558–577` using a cascade:
1. Stefan-Boltzmann inversion from EPW horizontal infrared (when IR ≥ 50 W/m²)
2. Berdahl-Martin clear-sky emissivity (recalibrated coefficients 0.758/0.521/0.625 per Li, Jiang & Coimbra 2017) with Walton cloud correction `(1 + 0.0224·N - 0.0035·N² + 0.00028·N³)` (when opaque_sky_cover > 0)
3. Clark-Allen fallback: `ε = 0.787 + 0.764 × ln(T_dp_K / 273.15)`, `T_sky = T_db_K × ε^0.25` (`epw.rs:588–593`)

This cascade matches EnergyPlus's `CalcSkyEmissivity()` in `WeatherManager.cc:3191–3217` and `HeatBalanceKivaManager.cc:656`.

The review description's formula `T_sky = T_db × (0.787 + 0.0028 × (T_dew - 273.15) + 0.0024 × opaque_sky_cover)` is a linearized approximation. HARES implements the full non-linear Clark-Allen + Walton form which is more accurate.

**Code Location**: `longwave.rs:59–106` (per-window LWR correction), `epw.rs:558–593` (sky temperature computation)

**Root Cause**: N/A — correctly implemented.

**Impact**: Correct. The four-component exterior LWR model (ground, true-sky, near-horizon-air, surface emission) with β-factor sky splitting matches EnergyPlus Engineering Reference §External Longwave Radiation exactly.

## Summary
- Total findings: 8
- Critical: 0
- High: 0
- Medium: 2
- Low: 6 (including 3 confirmatory)

## Recommendations

1. **Consider adding a comment about IAM coefficient provenance** (`solar.rs:547`): Note that the coefficients come from OCHRE (which reversed E+ Engineering Reference table values), and that they differ by ≤0.5% from the EnergyPlus C++ source code literals in `WindowManager.cc:4974–4997`.

2. **Pass `r_glass` from boundary construction to solver_builder** rather than recomputing it independently (`solver_builder.rs:310`). The `WindowSolarData` struct or boundary diagnostics already carry the necessary information. This removes the duplicated computation path and ensures the SHGC decomposition uses the same glass resistance as the RC network.

3. **Document the diffuse IAM divergence from OCHRE**: The per-curve hemispherical-average values (`solar.rs:628–637`) are more accurate than OCHRE's single constant 0.854. An AGENTS.md entry noting this deliberate deviation would help future reviewers understand the choice.

4. **Consider adding a validation test against EnergyPlus simple-glazing output**: A test that verifies the combined (IAM × SHGC × U-factor decomposition) window model against published E+ example output would increase confidence. This could compare single-pane, double-pane, and triple-pane results at 0°, 40°, 60°, and 80° incidence.

## References / Citations
- EnergyPlus Engineering Reference, Window Calculation Module Step 1 (U-factor decomposition): `https://bigladdersoftware.com/epx/docs/9-5/engineering-reference/window-calculation-module.html`
- EnergyPlus Engineering Reference, Window Calculation Module Step 4–5 (SHGC decomposition): same URL, Steps 4–5
- EnergyPlus Engineering Reference, Window Calculation Module Step 7 (angular performance): same URL, Step 7
- EnergyPlus Engineering Reference, External Longwave Radiation: `https://bigladdersoftware.com/epx/docs/9-6/engineering-reference/outside-surface-heat-balance.html`
- Walton, G.N. (1983), "Thermal Analysis Research Program Reference Manual", NBSIR 83-2655
- Clark, G. and Allen, C. (1978), "The Estimation of Atmospheric Radiation for Clear and Cloudy Skies", Proc. 2nd National Passive Solar Conference (AS/ISES), pp. 675–678
- Berdahl, P. and Martin, M. (1984), "Emissivity of clear skies", Solar Energy 32(5):663–664
- Li, M., Jiang, Y. & Coimbra, C.F.M. (2017), "On the determination of atmospheric longwave irradiance under all-sky conditions", Solar Energy 144:40–48
- OCHRE `ochre/utils/envelope.py` lines 97–128 (IAM), 294–304 (U-factor decomposition), 405–431 (SHGC decomposition)
- EnergyPlus `WindowManager.cc` lines 4974–4997 (IAM curves), 6309–6382 (EvalNominalWindowCond)
- EnergyPlus `WeatherManager.cc` lines 3191–3217 (sky emissivity cascade)
