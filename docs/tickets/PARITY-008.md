---
id: PARITY-008
title: "Boiler dynamic EIR curves"
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-equipment/src/hvac/boiler.rs
references:
  - docs/equipment/ochre-parity-gaps.md (Gap 7)
  - vendors/OCHRE/ochre/models/HVAC.py (GasBoiler.update_eir, lines 720-732)
  - EnergyPlus Engineering Reference Ch. 10.1.3 (Hot Water Boiler)
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace -- -D warnings
---

## Status: COMPLETE (verified — already fully implemented)

Audit confirms all boiler EIR functionality is implemented and tested:

| Check | Result |
|-------|--------|
| `current_eir()` called in step() | Line 463, every step when running |
| Condensing 6-coeff formula | Lines 341-346: `c0+c1*PLR+c2*PLR²+c3*T_in+c4*T_in²+c5*PLR*T_in` |
| Non-condensing 10-coeff formula | Lines 348-358: includes PLR³, T_out³, interaction terms |
| Default coefficients match OCHRE | Lines 41-60 vs OCHRE HVAC.py — exact match |
| PLR=0.5 test | `gas_boiler_condensing_curve_matches_expected_at_plr_half` (1e-4 tol) |
| Condensing vs non-condensing diverge | `condensing_is_more_efficient_than_non_condensing_at_partial_load` |
| OCHRE reference test | `non_condensing_eir_at_plr_half_matches_ochre_reference` |

## Background/Context

Implemented during prior work. The gap analysis was based on an earlier state of the code.

## Work to Do

- [ ] Audit `GasBoiler::current_eir()` (line 339) — verify it's actually called during `step()`
- [ ] Verify condensing 6-coeff formula matches: `c0 + c1·PLR + c2·PLR² + c3·T_in + c4·T_in² + c5·PLR·T_in`
- [ ] Verify non-condensing 10-coeff formula matches OCHRE (PLR, T_out, and interaction terms)
- [ ] Verify default coefficient values are loaded from config/defaults (not hardcoded zeros)
- [ ] If defaults are missing: add OCHRE-standard coefficients from `vendors/OCHRE/ochre/defaults/`
- [ ] Ensure `ElectricBoiler` also supports part-load efficiency if needed (currently constant)
- [ ] Add tests: PLR=0.5 produces different EIR than PLR=1.0; condensing vs non-condensing diverge

## Files to Touch

- `crates/hares-equipment/src/hvac/boiler.rs`: Verify/complete EIR implementation and defaults

## Measures of Success

- [ ] `current_eir()` is called on every step and affects fuel consumption
- [ ] At PLR=0.5, efficiency differs from PLR=1.0 by >5% (expected for real boilers)
- [ ] Default coefficients produce reasonable efficiency range (0.7–0.95 for gas, 0.85–0.98 for condensing)

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
