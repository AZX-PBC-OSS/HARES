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
