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

## Background/Context

The exploration agent found that `GasBoiler` already has `condensing_eir_coeffs: [f64; 6]` and `non_condensing_eir_coeffs: [f64; 10]` fields and a `current_eir()` method at line 339. The gap analysis claimed constant efficiency, but the code already has the infrastructure. This ticket should verify the implementation is complete and wired correctly, fix any issues, and ensure default coefficients match OCHRE/EnergyPlus.

**Target**: Verify and complete — ensure biquadratic condensing and 10-coeff non-condensing curves are fully functional with OCHRE-matched defaults.

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
