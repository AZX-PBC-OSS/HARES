# Window U-factor decomposition three-resistance vs OCHRE

**Review ID**: types-physics-03
**Category**: types-physics
**Date**: 2026-05-26

## Files Reviewed

- `crates/hares-envelope/src/rc_network.rs` — RC network infrastructure; contains window-node cascading elimination tests (lines 648–906)
- `crates/hares-envelope/src/thermal_solver/config.rs` — `WindowSolarProperties` and `ExteriorSurfaceInfo` window fields (lines 198–221, 403–411)
- `crates/hares-io/src/hpxml/building.rs` — HPXML `Window` struct and `parse_windows()` (lines 158–180, 975–1094)
- `crates/hares-envelope/src/boundary_rc.rs` — StarMesh window decomposition into conv + rad branches (lines 755–888)
- `crates/hares-core/src/dwelling/conversions.rs` — calls `window_u_factor_decomposition` and sets `interior_emissivity` to `EMISSIVITY_WINDOW` (lines 267–318)
- `crates/hares-physics/src/solar.rs` — `window_u_factor_decomposition` and `calculate_window_parameters` (lines 667–739)
- `crates/hares-physics/src/film_coefficients.rs` — DOE-2 exterior film model and ASHRAE Simple interior coefficients (full file)

## Vendor/Reference Files Consulted

- `vendors/EnergyPlus/src/EnergyPlus/Material.cc:3088–3259` — `SetupSimpleWindowGlazingSystem` — Step 1 U-factor decomposition, Steps 4–5 solar parameters
- `vendors/EnergyPlus/src/EnergyPlus/WindowManager.cc:6248–6299` — Nominal conductance adjustment ratio (`CoeffAdjRatio`) for simple glazing
- `vendors/OCHRE/ochre/utils/envelope.py:294–304` — `create_rc_data` with `u_window` — OCHRE's window RC construction
- `vendors/OCHRE/ochre/utils/envelope.py:405–431` — `calculate_window_parameters` — OCHRE's SHGC decomposition
- `vendors/OCHRE/ochre/Models/Envelope.py:345–411` — `Boundary.__init__` — how OCHRE constructs window RC from U-factor
- `vendors/OCHRE/ochre/utils/hpxml.py:206–217` — OCHRE's HPXML window parsing

## Findings

### Finding 1: Fixed exterior film coefficient for windows ignores wind speed [Severity: medium]

**Description**: The window U-factor decomposition in `window_u_factor_decomposition` (`solar.rs:700`) uses the EnergyPlus Step 1 correlation `Ro,w = 1/(0.025342 * U + 29.163853)`, which produces a fixed exterior film coefficient of approximately 29.4 W/(m²·K) independent of actual wind speed. During simulation, this resistance is baked into the RC network at construction time and never adjusted for varying wind conditions. Opaque surfaces, by contrast, use the DOE-2 wind-speed-dependent model in `film_coefficients.rs:284–292` (`h_glass = sqrt(h_natural² + (3.40 * V^0.75)²)` with roughness correction).

**Code Location**: `crates/hares-physics/src/solar.rs:698–700`, `crates/hares-envelope/src/boundary_rc.rs:830–834`

**Root Cause**: The EnergyPlus Simple Window Model uses this fixed correlation only for initial material property setup; at runtime, EnergyPlus recomputes exterior film coefficients from its exterior surface heat balance model using actual wind speed and temperature. HARES carries the init-time resistance into runtime without a per-timestep window film coefficient update.

**Impact**: At high wind speeds (>10 m/s), true h_se may reach 50+ W/(m²·K) and window heat loss is under-predicted. At calm conditions (<2 m/s), h_se may be ~15 W/(m²·K) and heat loss is over-predicted. The error magnitude is approximately (Δh_se / h_se_base) × U × A × ΔT, typically 5–15% of window conduction load under extreme wind. This is within the accuracy bounds of a simplified RC network model but would be measurable in a BESTEST-like comparison with EnergyPlus detailed-mode results.

**Comparison with EnergyPlus**: EnergyPlus `SetupSimpleWindowGlazingSystem` (`Material.cc:3136`) computes the same `Row` during setup, but during actual heat balance, EnergyPlus calls `CalcWindowHeatBalance` (`WindowManager.cc:2063`) which uses `HextConvCoeff` from the exterior surface heat balance — this is recomputed per timestep. EnergyPlus also applies a `CoeffAdjRatio` factor (lines 6265–6281) to scale h_c and h_out for simple glazing systems whose nominal U-factor exceeds the center-of-glass value.

**Comparison with OCHRE**: OCHRE sets `res_ext_w = 0` (`envelope.py:302`), absorbing the exterior film resistance into the window material resistance. OCHRE then adds a film resistance from `calculate_film_resistances` as a separate series term. This means OCHRE effectively uses a standard film resistance rather than the E+ correlation for the exterior side. HARES correctly separates the exterior film but freezes it at construction time.

### Finding 2: Interior film coefficient decomposition avoids double-counting of film resistance [Severity: low]

**Description**: The three-resistance model correctly decomposes the E+ combined interior film coefficient h_si into convective and radiative components in StarMesh mode, avoiding double-counting of the interior film resistance against the star-mesh radiation conductances.

**Code Location**: `crates/hares-envelope/src/boundary_rc.rs:804–827`

**Root Cause**: This is correct behavior, not a defect. The logic is: (1) The EnergyPlus Simple Window Model Step 1 correlation gives h_si (combined convective + radiative) at ε = 0.84. (2) HARES computes h_rad_glass at ε = 0.84 using linearised Stefan-Boltzmann at T_ref = 293.15 K. (3) h_conv = h_si − h_rad_glass (minimum clamped to 0.1). (4) The zone_air ↔ window_node path uses only h_conv (convection-only), while inter-surface radiation passes through the star-mesh with ε = 0.84. The total interior coupling reconstructs exactly to h_si, with no double-counting.

**Impact**: For U = 3.0 W/(m²·K): h_si ≈ 7.34 (from E+ correlation), h_rad_glass ≈ 4.58 (at ε = 0.84, T_ref = 293.15 K), h_conv ≈ 2.76. The decomposition sum h_conv + h_rad_glass ≈ 7.34 = h_si, confirming no double-counting. Using ε = 0.9 for h_rad_glass (instead of the E+-consistent 0.84) would give h_conv ≈ 7.34 − 4.91 = 2.43, undertempering the convective path by ~12% and increasing total coupling to 7.34 + 0.33 = 7.67 (over-coupling by +4.5%). HARES correctly uses ε = 0.84.

**Comparison with EnergyPlus**: EnergyPlus Option 2 (used for interior heat balance with iterative radiation) performs the same conceptual decomposition: the interior surface convection model provides h_c only, and longwave radiation is computed separately via ScriptF or star-mesh radiosity. The E+ Engineering Reference states: "the interior surface convection coefficient is convection only, the radiant part of the interior surface conductance is accounted for separately in the radiant exchange calculation." HARES follows this pattern exactly.

### Finding 3: Glazing conductance is derived from NFRC-rated U-factor minus fixed film coefficients [Severity: low]

**Description**: The glass layer conductance is computed as `1/U − Ri,w − Ro,w` using the EnergyPlus Step 1 film coefficient correlations. This is the correct NFRC → glazing-only decomposition, and no independent data source (e.g., WINDOW/THERM output) is used or needed for this model tier.

**Code Location**: `crates/hares-physics/src/solar.rs:688–706` (the `window_u_factor_decomposition` function)

**Root Cause**: By design — the Simple Window Model is the appropriate tier for HPXML-based simulation where only U-factor and SHGC are available. The film coefficients are not ASHRAE fixed handbook values but rather are derived from the EnergyPlus Step 1 correlation, which provides a U-factor-dependent interior film coefficient that accounts for the warmer glass surface temperature of higher-U windows (stronger interior natural convection).

**Actual h_si values from the correlation**:
| U-factor (W/m²·K) | h_si (W/m²·K) | Notes |
|---|---|---|
| 1.0 | 6.95 | Triple-pane low-e, low h_si (cooler surface) |
| 3.0 | 7.34 | Standard double-pane |
| 5.0 | 7.53 | Old single-pane (warmer glass, stronger convection) |
| 5.85 | 7.59 | Correlation branch point |

**Comparison with ASHRAE standard values**:
- ASHRAE HoF 2021 Ch. 15 Table 1: fenestration h_si = 8.3 W/(m²·K) winter (combined conv+rad, ε = 0.84), h_si = 5.5 W/(m²·K) summer (low-e, ε = 0.1)
- The HARES/E+ correlation gives 6.95–7.6 W/(m²·K), which is 8–16% below the ASHRAE winter value of 8.3 W/(m²·K)
- Rationale: the ASHRAE table values are **design** (peak load) values. The E+ correlation was derived from WINDOW5 parametric runs and represents a best-fit to center-of-glass simulations at the specific U-factor. For peak heating load calculations the ASHRAE value would be more appropriate; for annual energy simulation the E+ correlation is the standard method.

### Finding 4: OCHRE absorbs exterior film into window material resistance (divergence from EnergyPlus) [Severity: low]

**Description**: OCHRE's `create_rc_data` (`envelope.py:302`) sets `res_ext_w = 0`, meaning the exterior film resistance is absorbed into the window material resistance `r_window = 1/U − res_int_w − 0`. This is a deliberate simplification by OCHRE. HARES correctly separates `Ro,w` per the EnergyPlus correlation, documented at `solar.rs:678–686`: "OCHRE sets res_ext_w = 0, absorbing Ro,w into r_window. HARES diverges from OCHRE here for correctness."

**Code Location**: `crates/hares-physics/src/solar.rs:698–706` (HARES), `vendors/OCHRE/ochre/utils/envelope.py:294–304` (OCHRE)

**Impact**: Negligible for total window U-factor (reconstruction is exact either way), but the separated form enables two correctness checks: (a) the exterior LWR correction in `longwave.rs:44–106` needs the true exterior film coefficient to compute `delta_q_w = (U/h_out) × delta_q_w_m2 × A`, and (b) the SHGC decomposition in `calculate_window_parameters` (`solar.rs:726–739`) needs the true glass-only resistance `res_material_m2_k_w` for computing the inward-flowing fraction. Since OCHRE's `calculate_window_parameters` also uses `res_material` passed in from the RC network, the fact that OCHRE absorbs Ro,w into r_window means its `res_material` already includes the exterior film, subtly affecting the `radiation_frac` calculation. HARES's approach is more physically consistent.

### Finding 5: No summer/winter interior film coefficient switch [Severity: low]

**Description**: The window interior film coefficient from the E+ Simple Window correlation corresponds to "winter conditions" (Ri,w). There is no seasonal switch to a lower summer interior film coefficient (Ri,s) as EnergyPlus provides in `SetupSimpleWindowGlazingSystem` Step 5 (`Material.cc:3211–3226`). EnergyPlus computes separate summer interior/exterior resistances `Ris` and `Ros` for solar absorptance calculations, but HARES only uses the winter `Ri,w` value throughout.

**Code Location**: `crates/hares-physics/src/solar.rs:688–706` (only winter correlation), `vendors/EnergyPlus/src/EnergyPlus/Material.cc:3106–3110` (both Ri,w/Row and Ris/Ros)

**Impact**: The E+ summer interior film coefficient `Ris` is computed from a different polynomial and is typically 15–20% larger than `Riw` (lower heat transfer coefficient in summer). In cooling-dominated climates, window conduction gain may be slightly over-predicted (by ~3–5% of window load) because the higher summer h_si is not applied. However, since HARES does not currently recompute film coefficients per timestep anyway (see Finding 1), and since the summer/winter difference in h_si is smaller than the wind-speed-induced variation in h_se, this is a secondary concern. The summer solar parameters in `calculate_window_parameters` do not depend on the summer film coefficients directly — they use the separate `Ris`/`Ros` polynomials which are correctly implemented in HARES's `calculate_window_parameters` (`solar.rs:726–739`).

### Finding 6: Window U-factor guard against missing data is silent default [Severity: medium]

**Description**: When a window boundary has no U-factor in the HPXML data, HARES falls back to generic film resistances with a warning trace (`conversions.rs:278–283`). This produces a `fallback_r` computed from the boundary's assembly R-value (or default), which may not represent correct window thermal behavior. The warning is via `tracing::warn!` which is typically silent unless the tracing subscriber is configured to display warnings.

**Code Location**: `crates/hares-core/src/dwelling/conversions.rs:275–283`

**Root Cause**: The code produces different thermal behavior depending on whether the U-factor is present — windows without U-factor use the opaque-boundary fallback path with generic TARP film resistances, not the EnergyPlus window decomposition. Since `u_factor_w_m2_k` is `Option<f64>` in the `Window` struct (`building.rs:162`), the HPXML parser accepts windows without U-factor silently.

**Impact**: A malformed HPXML file with missing window U-factors could produce significantly incorrect simulation results without a hard error. The window would be treated as an opaque boundary in terms of conduction (though still participating in solar gain via SHGC). The fallback resistance would typically be much larger (more insulating) than a real window's resistance, under-predicting window heat loss.

## Summary

- Total findings: 6
- Critical: 0 / High: 0 / Medium: 2 / Low: 4

## Recommendations

1. **Add runtime window exterior film coefficient update** (Finding 1): During each timestep, recompute the window exterior film coefficient using the DOE-2 model at current wind speed, and apply a corrected driving temperature to the window conduction path. This brings HARES into alignment with the EnergyPlus runtime model and eliminates systematic wind-speed bias.

2. **Emit a hard error or structured warning for windows missing U-factor** (Finding 6): Change the fallback path in `conversions.rs:275–283` to either fail with an error (recommended, since HPXML requires U-factor for windows per the standard) or emit a prominent diagnostic via the `DiagnosticsCollector` or `tracing::error!` rather than `tracing::warn!`.

3. **Consider summer interior film coefficient** (Finding 5): For cooling-dominated annual simulations, evaluate whether implementing the E+ summer interior film coefficient `Ris` per `Material.cc:3211` would improve accuracy. The impact is small (~3–5% of window load) but systematic.

4. **Document the wind-speed limitation** (Finding 1): Add a comment to `window_u_factor_decomposition` noting that the exterior film coefficient is a fixed winter-standard value and that wind-speed-dependent correction is a future enhancement.

## References / Citations

- EnergyPlus Engineering Reference, Window Calculation Module, Step 1 — `https://bigladdersoftware.com/epx/docs/9-5/engineering-reference/window-calculation-module.html`
- EnergyPlus `Material.cc:3088–3259` — `SetupSimpleWindowGlazingSystem` — Steps 1–5 of the Simple Window Model
- EnergyPlus `WindowManager.cc:6248–6299` — `EvalNominalWindowCond` and `CoeffAdjRatio` for simple glazing U-factor adjustment
- OCHRE `envelope.py:294–304` — `create_rc_data` window RC construction
- OCHRE `envelope.py:405–431` — `calculate_window_parameters`
- OCHRE `hpxml.py:206–217` — HPXML window property parsing
- NFRC 100-2020 — Standard for determining fenestration product U-factors
- ASHRAE Handbook of Fundamentals 2021, Ch. 15 — Fenestration film coefficients
- ASHRAE Handbook of Fundamentals 2021, Ch. 27 — Parallel-path method for framing factors
- ASHRAE 140-2017 §5.3.1.9, Table 25 — Interior surface heat transfer coefficients (h_si = 8.29 for opaque, ε = 0.9)
- Walton, G. N. 1983. TARP Reference Manual, NBSSIR 83-2655 — Natural convection correlation
- Arasteh, Kohler, and Griffith — "Simple Window Model" draft paper — underlying correlation methodology
