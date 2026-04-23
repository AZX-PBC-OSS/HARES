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
