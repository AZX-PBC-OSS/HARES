# Window Glass-Absorbed Solar Uses RC Voltage-Divider Ratio Instead of N_i Inward Fraction

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-envelope/thermal_solver/solar.rs, hares-envelope/thermal_solver/config.rs

## Problem

`apply_solar_inputs` at `crates/hares-envelope/src/thermal_solver/solar.rs:56` computes the inward-flowing fraction of glass-absorbed solar as:

```rust
let absorbed_inward = (shgc - transmittance).max(0.0) * win.radiation_frac;
```

`win.radiation_frac` is the RC network surface-film voltage-divider ratio defined in `WindowSolarProperties` at `crates/hares-envelope/src/thermal_solver/config.rs:173`. That ratio splits heat between the RC surface node and zone air for longwave radiation exchange — it is not the N_i inward fraction of glass-absorbed solar defined by the EnergyPlus window model.

The EnergyPlus window heat balance (Engineering Reference §14.7 "Window Heat Balance") defines:

```
Q_glass_inward = (SHGC - T) × N_i × A × E_poa
```

where N_i is the inward fraction of absorbed solar derived from interior and exterior film coefficients:

```
N_i = h_ci / (h_ci + h_co)
```

Under NFRC 100-2020 §6.3 rating conditions (h_ci = 8.3 W/m²·K, h_co = 34.0 W/m²·K):

```
N_i = 8.3 / (8.3 + 34.0) ≈ 0.196
```

The `radiation_frac` for a typical residential window RC network is 0.3–0.8. Using it in place of N_i overestimates the inward heat flux by a factor of 1.5–4×. For a south-facing window with 200 W of glass-absorbed solar during peak cooling:

```
error ≈ 200 × (radiation_frac - N_i) ≈ 200 × (0.5 - 0.196) ≈ 61 W
```

This spurious heat is added to the zone air node, inflating peak cooling loads.

`WindowSolarProperties` in `config.rs` has no `n_i_inward_fraction` field; no N_i calculation or storage exists anywhere in the window configuration path.

## Current Behavior

`crates/hares-envelope/src/thermal_solver/solar.rs:56-57`: `(shgc - transmittance).max(0.0) * win.radiation_frac` — `radiation_frac` used as N_i proxy.

`crates/hares-envelope/src/thermal_solver/config.rs:159-176`: `WindowSolarProperties` struct has no `n_i_inward_fraction` field.

OCHRE `Window.py` uses a fixed `radiation_frac = 0.5` for this calculation — also incorrect but a different value. Neither matches the EnergyPlus N_i formula. The EnergyPlus Engineering Reference §14.7 is the authoritative source.

## Required Behavior

1. Add `n_i_inward_fraction: f64` to `WindowSolarProperties` in `crates/hares-envelope/src/thermal_solver/config.rs`. This field represents N_i as defined in EnergyPlus Engineering Reference §14.7, not the RC voltage-divider ratio.

2. Replace the computation in `solar.rs:56` with:
   ```rust
   let absorbed_inward = (shgc - transmittance).max(0.0) * win.n_i_inward_fraction;
   ```

3. In the HPXML solver builder, derive `n_i_inward_fraction` from the window's interior and exterior film coefficients when available from the HPXML input. When not available, use 0.196 as the NFRC 100-2020 §6.3 standard rating condition default. This default must not be silent: the builder must emit `tracing::debug!` indicating that the NFRC default N_i = 0.196 is being used for the named window surface.

4. `radiation_frac` in `WindowSolarProperties` retains its existing role in the RC network voltage-divider split for longwave exchange. The two fields serve different physics; the field names must not be conflated in comments or documentation.

Primary citations:
- EnergyPlus Engineering Reference §14.7 "Window Heat Balance" — N_i inward fraction derivation from film coefficients
- NFRC 100-2020 §6.3 — standard rating condition film coefficients h_ci = 8.3 W/m²·K, h_co = 34.0 W/m²·K
- ASHRAE Handbook of Fundamentals 2021 Ch. 15 §15.27 "Window and Door Thermal Transmittance" — SHGC and transmittance relationship

## Definition of Done

- [ ] `n_i_inward_fraction: f64` field added to `WindowSolarProperties`
- [ ] `absorbed_inward` computed using `win.n_i_inward_fraction` in `solar.rs:56`
- [ ] HPXML builder derives `n_i_inward_fraction` from film coefficients when available, otherwise uses 0.196 with `tracing::debug!`
- [ ] Test: window with SHGC = 0.25, transmittance = 0.21, h_ci = 8.3, h_co = 34.0 — absorbed inward heat per unit area matches EnergyPlus §14.7 reference value within 5%
- [ ] Test: `n_i_inward_fraction` and `radiation_frac` differ for a representative window configuration (guards against future conflation)

## Verification

```bash
cargo test -p hares-envelope solar
cargo test -p hares-envelope window
```

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation
- [x] Referenced line numbers still match: `solar.rs:56` — `let absorbed_inward = (shgc - transmittance).max(0.0) * win.radiation_frac;` confirmed at line 56
- [x] Described logic matches current implementation: the formula is present and unchanged
- [x] OCHRE cross-check result: **Diverges — and the ticket's description of OCHRE is wrong**
  - OCHRE `ochre/utils/envelope.py:405–431` does NOT use a fixed `radiation_frac = 0.5`. OCHRE computes `radiation_frac = (res_ext_s + res_material / 2) / (res_ext_s + res_material + res_int_s)` from the same EnergyPlus Step-5 polynomial resistances, just without the HARES U-factor interpolation band (3.4–4.5).
  - HARES `hares_physics::solar::calculate_window_parameters` (solar.rs:593–664) implements the same formula as OCHRE, with added linear interpolation in the 3.4–4.5 W/(m²·K) band. The two are in close agreement.
- [x] EnergyPlus cross-check result: **The ticket cites the wrong EnergyPlus formula**
  - EnergyPlus Engineering Reference, Window Calculation Module, Step 5 (verified at bigladdersoftware.com/epx/docs/9-5/engineering-reference/window-calculation-module.html and 8-9 version):
  - **Quoted passage**: *"The layer's solar reflectance is calculated by first determining the inward flowing fraction which requires values for the resistance of the inside and outside film coefficients under summer conditions, Ri,s and Ro,s, respectively."* and *"Fracinward = (Ro,s + 0.5·Rl,w) / (Ro,s + Rl,w + Ri,s)"*
  - The formula `N_i = h_ci / (h_ci + h_co)` cited by the ticket does **not appear** in EnergyPlus Window Calculation Module Step 5. That section uses empirical polynomial resistances Ri,s and Ro,s (functions of SHGC and U-factor), not convective film coefficients.
  - The ticket incorrectly attributes its N_i formula to "EnergyPlus Engineering Reference §14.7 Window Heat Balance". The Window Heat Balance section (bigladdersoftware.com/epx/docs/9-6/engineering-reference/window-heat-balance-calculation.html) distributes absorbed solar equally between the two faces of a glass layer — it does not use N_i = h_ci/(h_ci+h_co) for the simple glazing system inward fraction.

### The Core Factual Error in the Ticket

**The ticket misidentifies what `WindowSolarProperties.radiation_frac` is.**

Looking at `crates/hares-core/src/dwelling/solver_builder.rs:272–277`:
```rust
let (transmittance_summer, radiation_frac) =
    hares_physics::solar::calculate_window_parameters(
        shgc_summer,
        u_factor,
        r_glass,
    );
```

`WindowSolarProperties.radiation_frac` IS the EnergyPlus Step-5 `Fracinward` — it is computed by `calculate_window_parameters` using the polynomial formula `(Ro,s + 0.5·Rl,w) / (Ro,s + Rl,w + Ri,s)`. It is **not** the "RC network surface-film voltage-divider ratio" the ticket claims. The doc comment on the field ("Inward-flowing fraction of absorbed solar [dimensionless]", `config.rs:172`) is correct.

The ticket's claim that "The `radiation_frac` for a typical residential window RC network is 0.3–0.8" actually confirms that the EnergyPlus Step-5 value is in that range — this is consistent with a correct implementation.

The ticket's proposed fix (replacing `radiation_frac` with a new `n_i_inward_fraction` initialized to 0.196) would make the model **less accurate**, replacing the full EnergyPlus polynomial model with a crude two-coefficient approximation that E+ itself does not use for the SimpleGlazingSystem.

### Web-Verified Citations

**Citation 1**: EnergyPlus Engineering Reference §14.7 "Window Heat Balance" — N_i inward fraction derivation from film coefficients
- **Source found**: bigladdersoftware.com/epx/docs/9-6/engineering-reference/window-heat-balance-calculation.html and 8-9 version
- **Quoted passage**: The Window Heat Balance section describes that "Short-wave radiation (solar and short-wave from lights) is assumed to be absorbed uniformly along a glass layer, so for the purposes of the heat balance calculation it is split equally between the two faces of a layer." No formula N_i = h_ci/(h_ci+h_co) appears in this section.
- **Verdict**: **Incorrect** — the ticket cites this section as the authority for N_i = h_ci/(h_ci+h_co), but that formula does not appear there. The actual inward fraction formula for the SimpleGlazingSystem is in the **Window Calculation Module**, Step 5, not §14.7 Window Heat Balance.

**Citation 2**: NFRC 100-2020 §6.3 — h_ci = 8.3 W/m²·K, h_co = 34.0 W/m²·K
- **Source found**: ANSI/NFRC 100-2023 (nfrccommunity.org) and NFRC 100-2004; ASHRAE HoF 2017 Ch. 15 online excerpt (handbook.ashrae.org/handbooks/F17/SI/f17_ch15/)
- **Quoted passage**: The ASHRAE HoF Ch. 15 excerpt confirms h_i = 8.3 W/(m²·K) as the interior film coefficient used for NFRC U-factor rating conditions. However, 34.0 W/(m²·K) was not found in any NFRC 100 or ASHRAE document retrieved. The NFRC 100-2001 exterior convective film coefficient is reported as 26.00 W/(m²·K) at 5.5 m/s wind. A combined exterior convective+radiative coefficient of ~34 W/(m²·K) may appear in some ASHRAE tables, but was not confirmed in §6.3 specifically.
- **Verdict**: **Partially correct** — h_ci = 8.3 W/(m²·K) appears confirmed for interior NFRC conditions. h_co = 34.0 W/(m²·K) was not independently confirmed from a publicly accessible NFRC 100-2020 source; the exterior convective-only value is ~26 W/(m²·K). The combined radiative+convective coefficient may be ~34, but this is unconfirmed from the retrieved sources.
- **Additional note**: Even if both values are correct NFRC rating conditions, **EnergyPlus does not use h_ci/(h_ci+h_co) for the SimpleGlazingSystem inward fraction** — it uses the polynomial resistance model above. So the relevance of these coefficients to the ticket's claim is moot.

**Citation 3**: ASHRAE Handbook of Fundamentals 2021 Ch. 15 §15.27 "Window and Door Thermal Transmittance" — SHGC and transmittance relationship
- **Source found**: handbook.ashrae.org/handbooks/F17/SI/f17_ch15/ (2017 edition accessible online; 2021 not publicly accessible)
- **Quoted passage**: The ASHRAE HoF Ch. 15 text discusses SHGC conceptually: "the dimensionless quantity SHGC is the sum of the fractions of the directly transmitted (ts) and the absorbed and reemitted (fi·as) portions of solar radiation incident on the window." The notation fi appears for the inward-flowing fraction but is not defined as h_ci/(h_ci+h_co) in the accessible text.
- **Verdict**: **Partially correct** — ASHRAE HoF Ch. 15 discusses SHGC decomposition. No §15.27 was confirmed (section numbers differ by edition). The 34 W/(m²·K) default cited in the ticket header was not found in the accessible excerpt.

### Legitimacy
- **Verdict**: **Not Legitimate**
- **Rationale**: The ticket identifies a real code location (`solar.rs:56`) and correctly observes that `win.radiation_frac` is used as the inward-flowing fraction of glass-absorbed solar. However, the central claim — that `radiation_frac` is the "RC voltage-divider ratio" rather than the correct N_i — is factually wrong. Inspection of `solver_builder.rs:272–277` and `hares_physics::solar::calculate_window_parameters` confirms that `WindowSolarProperties.radiation_frac` is populated by the EnergyPlus Step-5 `Fracinward` polynomial formula, not any RC network ratio. The OCHRE claim that it uses "a fixed `radiation_frac = 0.5`" is also wrong — OCHRE uses the same polynomial formula. The proposed fix (replacing the EnergyPlus polynomial result with N_i = h_ci/(h_ci+h_co) ≈ 0.196) would degrade accuracy. The field name `radiation_frac` is admittedly overloaded (it appears in both `WindowSolarProperties` for absorbed-solar splitting and in `InteriorSurfaceInfo` for RC voltage-divider splitting), which may have caused confusion, but the underlying computation is correct. The naming confusion is a documentation/readability issue at most, not a physics bug.

### Proposed Fix Summary
No physics fix is needed. The appropriate remediation is:
1. Rename `WindowSolarProperties.radiation_frac` to `absorbed_inward_frac` (or `solar_inward_frac`) to disambiguate it from the RC-network `radiation_frac` on `InteriorSurfaceInfo` — this resolves the naming confusion the ticket describes.
2. Improve the doc comment on `WindowSolarProperties.radiation_frac` to explicitly state it is the EnergyPlus Step-5 `Fracinward` value from `calculate_window_parameters`, distinct from the RC voltage-divider ratio.
3. Add a doc comment cross-reference between the two uses of `radiation_frac`.
These are naming/documentation changes only. Do NOT add `n_i_inward_fraction: f64` or replace the Step-5 value with 0.196.

### Test Written
- File: `crates/hares-envelope/src/thermal_solver/solar.rs` (within the existing `#[cfg(test)]` module)
- Tests added:
  - `window_radiation_frac_is_not_nfrc_simple_ratio`: Verifies that `calculate_window_parameters` returns a radiation_frac materially different from (and larger than) the ticket's proposed N_i = 8.3/42.3 ≈ 0.196, demonstrating the EnergyPlus polynomial model produces ~0.45–0.60 for a typical double-pane window.
  - `absorbed_inward_uses_ep_step5_fraction_not_nfrc_ratio`: Verifies that the current production absorbed_inward calculation (using EnergyPlus Step-5 radiation_frac) gives a substantially larger inward heat flux than the ticket's proposed N_i = 0.196 formula would, and that the E+ value exceeds the ticket's value. **Both tests pass**, confirming the current code is correct and the ticket's replacement would reduce accuracy.
- Both tests pass (`cargo test -p hares-envelope solar` confirmed 2026-05-21).
