# Window Exterior Film Resistance Hardcoded to Zero, Violating EnergyPlus Simple Window Model

**Severity**: High
**Priority**: P2
**Status**: Open
**Areas**: hares-core/dwelling/conversions.rs, hares-physics/solar.rs

## Problem

`window_u_factor_decomposition` (solar.rs:560–572) correctly splits a window's
NFRC U-factor into glass resistance and interior film resistance:

```
U_window = 1 / (R_film_ext + R_glass + R_film_int)
R_glass = 1/U_window - R_film_int - R_film_ext
```

The function returns `(r_glass, r_film_interior)`. The caller in
`conversions.rs:220–222` then hardcodes `r_film_exterior = 0.0`:

```rust
let (r_glass, r_int) = window_u_factor_decomposition(u);
(r_glass, r_int, 0.0)  // ← exterior film = 0.0 hardcoded!
```

The EnergyPlus Simple Window Model (Engineering Reference §7.4.1) explicitly
requires exterior film resistance in the U-factor decomposition:

```
R_film_ext = 0.0440 m²·K/W  (NFRC exterior film: 15 mph wind, 0°F outdoor)
R_film_int = 0.1220 m²·K/W  (NFRC interior film: still air, 70°F indoor)
```

These are the NFRC 100-2020 standardized film conditions used when measuring
window U-factors. Both film resistances must be subtracted from `1/U_window`
to obtain the glass-only resistance.

### Consequence

If `window_u_factor_decomposition` internally subtracts both films but the
caller resets `r_film_ext = 0.0`, the total window R assembled in
`boundary_rc.rs` is:

```
R_total = r_film_int + r_glass + 0.0  (missing exterior film)
```

instead of:

```
R_total = r_film_int + r_glass + r_film_ext
```

The NFRC-rated U-factor for the full assembly (including both films) was
`1/U`. By discarding `r_film_ext`, the assembled R is lower than `1/U` by
`R_film_ext = 0.044 m²·K/W`. For a U=2.0 W/m²·K (R=0.50) window, the
missing 0.044 represents an 8.8% underestimate of total R — an 8.8%
overestimate of U and window heat loss.

For a high-performance U=0.5 window (R=2.0 m²·K/W), the error is:
0.044/2.0 = 2.2% — smaller but still systematic and present for all windows
regardless of performance level.

## Evidence

```
crates/hares-core/src/dwelling/conversions.rs:220–222
    if let Some(u) = u_factor.filter(|&u| u > 0.0) {
        let (r_glass, r_int) = window_u_factor_decomposition(u);
        (r_glass, r_int, 0.0)   // ← r_film_ext = 0.0 hardcoded
    }
```

```
crates/hares-physics/src/solar.rs:560–572 (window_u_factor_decomposition)
    // Verify that this function subtracts both films:
    // R_glass = 1/U - R_film_int - R_film_ext
```

## Required Behavior

Per NFRC 100-2020 (Procedure for Determining Fenestration Product U-factors),
window U-factors are measured under standardized film conditions:

- Exterior film: h_o = 22.7 W/(m²·K) → R_ext = 0.0440 m²·K/W
  (15 mph wind, -18°C outdoor)
- Interior film: h_i = 8.2 W/(m²·K) → R_int = 0.1220 m²·K/W
  (still air, 21°C indoor)

Both film resistances must be subtracted from `1/U_window` to obtain the
glass-only resistance, and both must be included when assembling the total
window thermal path. The assembled R must equal `1/U_window` exactly.

Per EnergyPlus Engineering Reference §7.4.1 (Simple Window Model), the
glass resistance plus both film resistances reconstructs the NFRC assembly.

## Approach

1. Change `window_u_factor_decomposition` in `crates/hares-physics/src/solar.rs`
   to return a three-tuple `(r_glass, r_film_interior, r_film_exterior)`.
   The constants `NFRC_R_FILM_EXT_M2_K_W = 0.0440` and
   `NFRC_R_FILM_INT_M2_K_W = 0.1220` must be defined once in that module.
2. Update the call site in `crates/hares-core/src/dwelling/conversions.rs:220–222`
   to use all three return values.
3. No hardcoded `0.0` at the call site.

## Definition of Done

- [ ] `window_u_factor_decomposition` returns `(r_glass, r_film_int, r_film_ext)`.
- [ ] Call site in `conversions.rs` uses all three values.
- [ ] Round-trip test: `U = 2.0 W/m²·K` → `r_glass + r_int + r_ext` = 0.500 m²·K/W
      within f64 tolerance.
- [ ] Round-trip test: `U = 0.5 W/m²·K` → `r_glass + r_int + r_ext` = 2.000 m²·K/W.

## Verification

```bash
cargo test -p hares-physics window_u_factor_decomposition
cargo test -p hares-core window
```

Expected: round-trip identity holds for all physically valid U-factors
(0.2 ≤ U ≤ 6.0 W/m²·K).

## References

- NFRC 100-2020 §8.3 (Standardized boundary conditions for U-factor measurement —
  exterior film h_o = 22.7 W/(m²·K), interior film h_i = 8.2 W/(m²·K)).
- EnergyPlus Engineering Reference §7.4.1 (Simple Window Model — U-factor
  decomposition into glass and film resistances).
- ASHRAE Handbook of Fundamentals 2021, Ch. 15, Table 14 (Fenestration film
  coefficients).

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-20

### Code Confirmation

- [x] Referenced line numbers still match: `conversions.rs:221–222` and `solar.rs:560–573` are current.
- [x] Described logic matches current implementation: the hardcoded `0.0` at conversions.rs:222 is present.
- [x] OCHRE cross-check result: **matches — intentional divergence from E+ proper, shared with OCHRE**
  - `vendors/OCHRE/ochre/utils/envelope.py:302`: `res_ext_w = 0  # 1 / (0.025342 * u + 29.163853)`
  - OCHRE deliberately zeros the exterior film resistance and leaves the commented-out E+ formula
    as documentation. HARES replicates this choice identically.
- [x] EnergyPlus cross-check result: **diverges — HARES/OCHRE omit Ro,w from the decomposition**
  - EnergyPlus Engineering Reference (versions 8.2–24.2, all confirmed identical):
    > "1/U = Ri,w + Ro,w + Rl,w … so that the glass-to-glass resistance is calculated using
    > Rl,w = 1/U − Ri,w − Ro,w"
    > "Ro,w = 1/(0.025342·U + 29.163853)" [exterior film, standard winter conditions]
  - HARES computes `r_glass = 1/U − Ri,w` (omitting Ro,w), then returns `(r_glass, r_int)`.
    The caller fixes `r_film_ext = 0.0`. Net effect: Ro,w is absorbed into `r_glass` rather
    than tracked separately. Total assembled R = 1/U still holds exactly.

### Web-Verified Citations

**Citation 1**: "EnergyPlus Engineering Reference §7.4.1 (Simple Window Model — U-factor
decomposition into glass and film resistances)"

- **Source found**: BigLadder EnergyPlus Engineering Reference, Window Calculation Module,
  versions 8.2, 8.3, 8.9, 9.3, 9.5, 24.2 (all confirmed identical).
  URL: https://bigladdersoftware.com/epx/docs/9-5/engineering-reference/window-calculation-module.html
- **Quoted passage**:
  > "1/U = Ri,w + Ro,w + Rl,w"
  > "Ri,w = 1/(0.359073·Ln(U) + 6.949915)  for U < 5.85"
  > "Ri,w = 1/(1.788041·U − 2.886625)      for U ≥ 5.85"
  > "Ro,w = 1/(0.025342·U + 29.163853)"
  > "Rl,w = 1/U − Ri,w − Ro,w"
  > "Ro,w is the resistance of the exterior film coefficient under standard winter conditions
  > in units of m²·K/W."
- **Verdict**: **Partially correct** — the section exists and describes the decomposition, but
  the ticket's claim that `window_u_factor_decomposition` "internally subtracts both films" is
  **false**: the function only subtracts Ri,w (interior film). The exterior film formula exists
  in E+ but HARES/OCHRE deliberately set it to zero. Section number "§7.4.1" could not be
  confirmed (BigLadder docs do not expose that numbering level), but the module is correct.

**Citation 2**: "NFRC 100-2020 §8.3 — exterior film h_o = 22.7 W/(m²·K), interior film
h_i = 8.2 W/(m²·K)"

- **Source found**: ASHRAE Handbook of Fundamentals 2017 Ch. 15 (closest publicly accessible
  equivalent for film coefficients); NFRC 100-2023 PDF (binary, not extractable).
  URL: https://handbook.ashrae.org/handbooks/F17/SI/f17_ch15/f17_ch15_si.aspx
- **Quoted passage** (ASHRAE HoF Ch. 15):
  > "A nominal value of 26 W/(m²·K) corresponding to a 5.5 m/s wind is often used to represent
  > winter design conditions."
  > "Designers often use hi = 8.3 W/(m²·K), which corresponds to ti = 21°C, a glazing temperature
  > of −9.4°C, and emissivity of eg = 0.84."
  > Outdoor conditions: "room air temperature ti = 21°C, outdoor air temperature to = −18°C,
  > no solar radiation" with "6.7 m/s outdoor air velocity."
- **Verdict**: **Incorrect** — the ticket's exterior film value is wrong on two counts:
  1. h_o = 22.7 W/(m²·K) → R = 0.0441 m²·K/W is not the standard NFRC/ASHRAE value.
     ASHRAE uses h_o = 26 W/(m²·K) (R = 0.038) for 5.5 m/s wind.
  2. The E+ Ro,w formula evaluates to ≈ 0.034 m²·K/W across the full U-factor range (not 0.044).
     This corresponds to h_o ≈ 29.2 W/(m²·K) — the E+ correlation value for winter conditions.
  3. HARES itself uses H_OUT_NFRC = 34.0 W/(m²·K) as the combined (conv + rad) exterior
     coefficient for window LWR calculations (`longwave.rs:18`), giving R = 0.029 m²·K/W.
  4. The interior film value h_i = 8.2 W/(m²·K) is approximately correct (ASHRAE says 8.3);
     the corresponding R = 0.122 m²·K/W is consistent with the Ri,w formula evaluated for
     typical U-values.

**Citation 3**: "ASHRAE Handbook of Fundamentals 2021, Ch. 15, Table 14 (Fenestration film
coefficients)"

- **Source found**: ASHRAE HoF 2017 Ch. 15 (the 2021 edition is behind a paywall; the 2017
  SI edition is publicly accessible at the URL above and values are identical for this section).
- **Quoted passage**: See Citation 2 above (same source).
- **Verdict**: **Partially correct** — Ch. 15 is the correct chapter for fenestration film
  coefficients. "Table 14" could not be confirmed (table numbering differs between editions and
  the online version does not show table numbers in extracted text). The chapter citation is
  legitimate; the specific film coefficient values cited in the ticket (22.7 W/(m²·K)) do not
  appear in the ASHRAE source found.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: The ticket correctly identifies that `conversions.rs:222` hardcodes
  `r_film_ext = 0.0` and that EnergyPlus defines a non-zero exterior film resistance formula
  `Ro,w = 1/(0.025342·U + 29.163853)`. However, the ticket's description of the bug contains
  two material errors that undermine its framing as a simple omission: (1) `window_u_factor_decomposition`
  does NOT internally subtract Ro,w — so the function is consistent with the caller setting
  `r_film_ext = 0.0`; the total assembled R equals 1/U exactly. (2) The NFRC film values cited
  (h_o = 22.7 W/(m²·K), R_ext = 0.044 m²·K/W) are incorrect — the E+ formula gives
  Ro,w ≈ 0.034 m²·K/W (~29.2 W/(m²·K)), and HARES already uses 34 W/(m²·K) as the NFRC
  combined exterior coefficient. The real issue — which is genuine — is that HARES/OCHRE absorb
  Ro,w into r_glass rather than tracking it as a separate film resistance, causing r_glass to be
  ~0.034 m²·K/W higher than the true glass-only resistance. This matters for the window solar
  parameter calculation (`calculate_window_parameters` uses `r_glass` to derive optical
  properties). The total thermal resistance is correct; the optical model may carry a small error
  from the inflated r_glass. This matches OCHRE's deliberate choice (commented-out Ro,w formula),
  so "fixing" it would diverge from OCHRE without careful consideration.

### Proposed Fix Summary

If pursuing strict EnergyPlus conformance (beyond OCHRE parity): change
`window_u_factor_decomposition` to return a three-tuple `(r_glass, r_film_int, r_film_ext)`
where `r_glass = 1/U − Ri,w − Ro,w` and `r_film_ext = Ro,w = 1/(0.025342·U + 29.163853)`.
Update the call site in `conversions.rs:221–222` to destructure all three values. The constants
should NOT be the ticket's claimed NFRC values (0.044, 0.122) — they must be computed from the
E+ Ro,w and Ri,w correlation formulas, not hardcoded. This change would require verifying that
downstream uses of `r_glass` in `calculate_window_parameters` produce better solar optical
results, and confirming deliberate divergence from OCHRE is intended. Do NOT implement yet.

### Test Written

- File: `crates/hares-physics/tests/solar_parity.rs` (appended after line 474)
- Functions added:
  - `ticket036_exterior_film_absorbed_into_r_glass_matches_ochre`: pins that r_glass + r_int =
    1/U (OCHRE parity), confirms Ro,w is absorbed into r_glass, and asserts the ticket's claimed
    0.0440 m²·K/W value is incorrect (E+ Ro,w ≈ 0.034 m²·K/W).
  - `ticket036_hares_r_glass_matches_ochre_r_window`: confirms bit-for-bit parity between
    HARES r_glass and OCHRE's `r_window = 1/U − res_int_w` (with res_ext_w = 0).
- Both tests pass (`cargo test -p hares-physics ticket036`: 2 passed, 0 failed).
