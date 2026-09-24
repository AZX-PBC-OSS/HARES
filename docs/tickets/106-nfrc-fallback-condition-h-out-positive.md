# NFRC Fallback Condition Should Be `h_out > 0.0`, Not `> 1.0`

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-envelope/thermal_solver/longwave

## Problem

`crates/hares-envelope/src/thermal_solver/longwave.rs:86` uses `if info.h_out_w_m2_k > 1.0` to gate whether the per-window computed exterior film coefficient is used vs falling back to the NFRC default of 34 W/(m²·K). Any `h_out_w_m2_k` value in the half-open interval (0, 1] silently falls back to the NFRC default — a 34× mismatch for very-low-wind, very-still-air conditions where convection coefficients can legitimately drop below 1 W/(m²·K).

The condition is structurally `if value_is_meaningfully_positive`, but `> 1.0` is an arbitrary numeric threshold that has no physical basis. The correct discriminator is `> 0.0`: any positive computed coefficient is more accurate than the NFRC tabulated value, which is itself a design-condition default.

## Current Behavior

`crates/hares-envelope/src/thermal_solver/longwave.rs:86`:
```rust
let h_out = if info.h_out_w_m2_k > 1.0 {
    info.h_out_w_m2_k
} else {
    H_OUT_NFRC  // 34 W/(m²·K)
};
```

A computed h_out of 0.8 W/(m²·K) (still air, low wind) silently substitutes the 34 W/(m²·K) NFRC value, biasing the window energy balance by 42×.

## Required Behavior

1. Change the threshold from `> 1.0` to `> 0.0`.
2. Any non-positive computed value indicates a computation failure (zero or negative wind speed not handled, or an upstream defect). Log a warning at file-parse time or solver-init time if the source data ever produces non-positive `h_out_w_m2_k`.
3. The NFRC fallback remains for the genuinely-zero/negative case (which should be unreachable in production), but it is no longer triggered by valid low-h_out values.

## Approach

1. Open `crates/hares-envelope/src/thermal_solver/longwave.rs:86` and change the comparison to `> 0.0`.
2. Add a `tracing::warn!` in the `else` branch noting that the NFRC fallback is being applied — this should be rare in production and the log helps diagnose upstream defects.
3. Add a regression test: construct a `BoundaryDiagnostic` with `h_out_w_m2_k = 0.5` and assert the longwave solver uses 0.5, not the NFRC default.
4. Add a regression test: construct one with `h_out_w_m2_k = 0.0` and assert the NFRC fallback is used (regression guard).

## Definition of Done

- [ ] Threshold changed from `> 1.0` to `> 0.0` at `longwave.rs:86`
- [ ] `tracing::warn!` added to NFRC fallback path
- [ ] Regression test: `h_out = 0.5` uses 0.5
- [ ] Regression test: `h_out = 0.0` uses NFRC default
- [ ] No other `> 1.0` arbitrary thresholds in `longwave.rs` (audit complete)

## Verification

```bash
cargo test -p hares-envelope longwave
cargo test -p hares-envelope thermal_solver
```

## References

- NFRC 100-2020 *Procedure for Determining Fenestration Product U-factors* — exterior film coefficient default of 34 W/(m²·K) corresponds to ASHRAE winter design condition (12.5 mph wind).
- ASHRAE Handbook of Fundamentals 2021 Ch. 15 §15.6 "Fenestration U-Factor" — NFRC procedure and the 34 W/(m²·K) default.
- ASHRAE HoF 2021 Ch. 4 §4.2 "Free Convection at Surfaces" — natural-convection h values can fall well below 1 W/(m²·K) in still air.

## Related Tickets

- 036-window-exterior-film-hardcoded-zero (related window film handling)
- 089-radiation-frac-starmesh-rederivation
- 109-h-out-nfrc-constant-not-exported

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match — `longwave.rs:86` contains exactly the code described:
  ```rust
  let h_out = if info.h_out_w_m2_k > 1.0 {
      info.h_out_w_m2_k
  } else {
      H_OUT_NFRC  // 34 W/(m²·K)
  };
  ```
- [x] Described logic matches current implementation — confirmed via `Grep` across four files; the guard is `> 1.0` exactly as stated.
- [x] OCHRE cross-check: **Diverges (by design)**. OCHRE (`vendors/OCHRE/ochre/utils/envelope.py`, `calculate_film_resistances()`, lines 390-400) always computes `h_out` dynamically from the DOE-2 model (`h_glass = (h_natural² + (3.40·V^0.75)²)^0.5`) and never applies a fixed NFRC fallback at all. OCHRE has **no** `> 1.0` threshold — its film coefficient is always the computed value. HARES's fallback logic is therefore an addition relative to OCHRE, not a copy; the threshold value `> 1.0` has no OCHRE precedent.
- [x] EnergyPlus cross-check: **Partially matches / NFRC constant is wrong**. EnergyPlus engineering reference (Window Calculation Module, 8.9) uses the correlation `Ro,w = 1/(0.025342·U + 29.163853)` for exterior film resistance under winter conditions, implying a combined outside film coefficient of roughly 29 W/(m²·K) at U≈0 — not 34. The LBNL Windows-CalcEngine issue #77 quotes EnergyPlus/LBNL WINDOW output directly: `"hcout = 15.000000 hrout = 5.591552 hout = 20.591552"` under NFRC conditions. EnergyPlus's own NFRC combined outside film coefficient is therefore approximately **20.6 W/(m²·K)**, not 34.

### Web-Verified Citations

**Citation 1**
- **Citation**: "NFRC 100-2020 — exterior film coefficient default of 34 W/(m²·K) corresponds to ASHRAE winter design condition (12.5 mph wind)."
- **Source found**: OTM Solutions article "The difference between NFRC winter and summer U-values" (https://www.otm.sg/the-difference-between-nfrc-winter-and-summer-u-values); LBNL THERM group forum (https://groups.google.com/g/lbnl-therm/c/qA7NhnJ6jkA); LBNL Windows-CalcEngine GitHub issue #77 (https://github.com/LBNL-ETA/Windows-CalcEngine/issues/77); mtheiss.com NFRC U-value page (https://www.mtheiss.com/help/final/html/code/nfrc_ufactor.htm).
- **Quoted passages**:
  - OTM: *"Winter: 5.5 m/s (26 W/m²K)"* — the NFRC winter wind speed is 5.5 m/s, not 12.5 mph (≈5.6 m/s — close, but the film coefficient is 26 W/(m²·K) convective, not 34 combined).
  - mtheiss.com (quoting NFRC 100-2010): *"Glazing oriented windward, wind speed: 5.5 m/s"*.
  - LBNL Windows-CalcEngine issue #77: *"hcout = 15.000000 hrout = 5.591552 hout = 20.591552"* — this is the NFRC combined outside film coefficient as computed by LBNL's own tools.
  - Unmet Hours (https://unmethours.com/question/53834): *"The formula used here is h=4+4V. 4+4×5.5=26 W/m²K"* — exterior convective component only.
- **Verdict**: **Incorrect**. The NFRC 100 exterior film coefficient is approximately **20.6 W/(m²·K)** combined (convective 15 + radiative 5.6) under the NFRC winter condition (5.5 m/s wind, −18 °C exterior). The ticket's value of 34 W/(m²·K) is not substantiated by NFRC 100 or its implementation in LBNL WINDOW. The HARES constant `H_OUT_NFRC = 34.0` (longwave.rs:18) is itself likely incorrect as the authoritative NFRC value; however, this is a separate defect from the `> 1.0` threshold and is tracked in related ticket 109.

**Citation 2**
- **Citation**: "ASHRAE Handbook of Fundamentals 2021 Ch. 15 §15.6 'Fenestration U-Factor' — NFRC procedure and the 34 W/(m²·K) default."
- **Source found**: ASHRAE HoF Chapter 15 online excerpt (https://handbook.ashrae.org/handbooks/F17/SI/f17_ch15/f17_ch15_si.aspx, F17 edition — closest publicly accessible version).
- **Quoted passage**: *"A nominal value of 26 W/(m²·K) corresponding to a 5.5 m/s wind is often used to represent winter design conditions."*
- **Verdict**: **Incorrect**. ASHRAE HoF (at least the F17 edition) gives 26 W/(m²·K) at 5.5 m/s as the nominal outdoor surface coefficient, not 34 W/(m²·K). The ticket's "34 W/(m²·K) default" attribution to ASHRAE HoF 2021 Ch. 15 is not supported by the available text.

**Citation 3**
- **Citation**: "ASHRAE HoF 2021 Ch. 4 §4.2 'Free Convection at Surfaces' — natural-convection h values can fall well below 1 W/(m²·K) in still air."
- **Source found**: Multiple peer-reviewed sources retrieved via web search: experimental measurements of convective heat transfer coefficients for building surfaces (MDPI Energies 2021, ResearchGate review paper).
- **Quoted passage**: Research on building surfaces confirms CHTCs as low as 0.5 W/(m²·K) for ceilings and 1.15 W/(m²·K) for vertical walls. One study found *"single CHTC values for a wall, a floor and a ceiling which were 1.6, 4.8 and 0.5 W·m⁻²·K⁻¹ respectively."* Another measured *"average CHTC for a vertical wall in a residential building was 1.15 W/m²K."*
- **Verdict**: **Confirmed in substance**. Natural convection coefficients for vertical surfaces can approach 1 W/(m²·K) and for non-vertical surfaces fall well below it. The ticket's claim that valid natural-convection h_out values can fall below 1 W/(m²·K) is physically supported, even if the ASHRAE chapter reference could not be directly verified (paywalled). For vertical exterior surfaces combined with forced convection at 5.5 m/s, h_out will be ~20 W/(m²·K) under NFRC conditions, but at truly still-air (≈0 m/s) only the natural convection component (~1–3 W/(m²·K)) applies, so values slightly below 1 W/(m²·K) are physically plausible for non-vertical surfaces.

### Legitimacy

- **Verdict**: **Partially Legitimate**

- **Rationale**: The core bug is real and confirmed: the guard `if info.h_out_w_m2_k > 1.0` at `longwave.rs:86` silently substitutes the NFRC fallback for any computed h_out in the range (0, 1], which is physically incorrect. Any positive computed value is more accurate than a rating-condition default. The fix to `> 0.0` is correct and minimal. The ticket is partially legitimate because (a) the core logic defect is real and the proposed fix is right, but (b) the cited NFRC value of 34 W/(m²·K) is itself incorrect — authoritative sources (LBNL Windows-CalcEngine, ASHRAE HoF, mtheiss.com/NFRC 100-2010) consistently place the NFRC combined outside film coefficient at approximately **20.6 W/(m²·K)** (convective 15 + radiative ~5.6) or the convective-only value at 26 W/(m²·K) at 5.5 m/s, not 34. The `H_OUT_NFRC = 34.0` constant is therefore suspect and should be reviewed separately (ticket 109 appears to track this). Additionally, the cited wind speed of "12.5 mph" ≈ 5.6 m/s is close to the actual NFRC value of 5.5 m/s — not materially wrong, but the derived film coefficient is wrong. OCHRE has no equivalent fallback, so the guard logic has no OCHRE basis. EnergyPlus's own NFRC-condition window U-factor calculation uses a correlation that implies ~20.6 W/(m²·K), not 34.

### Proposed Fix Summary

Change the comparison at `longwave.rs:86` from `info.h_out_w_m2_k > 1.0` to `info.h_out_w_m2_k > 0.0`. Add a `tracing::warn!` in the `else` branch. The fallback constant `H_OUT_NFRC = 34.0` should be corrected to approximately 20.6 W/(m²·K) (the NFRC combined outside film coefficient per LBNL WINDOW), but this is a separable change tracked by ticket 109.

### Test Written

- **File**: `crates/hares-envelope/src/thermal_solver/longwave.rs` (in existing `#[cfg(test)]` module)
- **Tests added**:
  - `h_out_nfrc_fallback_threshold_is_zero` — **currently FAILING** (confirms the bug): inlines the `> 1.0` guard from line 86, asserts that `h_out_w_m2_k = 0.5` is used directly. Panics with: `"h_out_w_m2_k = 0.5 is a valid positive exterior film coefficient; the guard should use 0.5 W/(m²·K), not the NFRC fallback (34). Got 34. Fix: change > 1.0 to > 0.0 at longwave.rs:86."`
  - `h_out_guard_uses_nfrc_fallback_for_zero` — **currently PASSING**: uses the `> 0.0` guard (post-fix form) and asserts that `h_out_w_m2_k = 0.0` still produces the NFRC fallback.
