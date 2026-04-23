# `R_FILM_INTERIOR_M2_K_W` Constant Mismatches Production Use

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-envelope/boundary_rc

## Problem

`R_FILM_INTERIOR_M2_K_W = 0.12` at `crates/hares-envelope/src/boundary_rc.rs:33-35` is the ISO 6946 *combined* (convection + radiation) interior surface resistance value. After the S1 fix promoted convection and radiation to separate paths, production code computes interior film coefficients via TARP (convection-only ~0.447 m²·K/W at ΔT=5°C) plus an explicit linearised h_rad branch. The combined-resistance constant is no longer used by the production path, but tests still pass `R_FILM_INTERIOR_M2_K_W = 0.12` to setup helpers — a 3.7× mismatch with what production actually uses.

The constant is misleading: a future contributor will read it as authoritative and either (a) recompute production using it (regressing S1) or (b) write a new test against the constant that diverges from production.

## Current Behavior

`crates/hares-envelope/src/boundary_rc.rs:33-35`:
```rust
pub const R_FILM_INTERIOR_M2_K_W: f64 = 0.12;  // ISO 6946 combined
```

Tests pass `R_FILM_INTERIOR_M2_K_W` to setup helpers. Production calls TARP and h_rad separately, ignoring this constant.

## Required Behavior

Choose one:

A. **Rename and re-purpose** — rename the constant to `R_FILM_INTERIOR_COMBINED_ISO6946_M2_K_W` and document explicitly that it is the ISO 6946 reference value provided for unit-test convenience and never used by production. Add a comment block explaining that production uses TARP (convection-only) plus explicit h_rad.

B. **Remove and migrate tests** — delete the constant. Update every test that passes it to compute its own expected value from TARP at the test's design ΔT plus a documented h_rad. This is the more correct path because it removes the misleading constant entirely.

Recommended path: B (remove). The constant has no production use and its existence is a footgun.

## Approach

1. Audit all callsites of `R_FILM_INTERIOR_M2_K_W`. Every call should be in test code; no production callsite should remain (verify against the S1 fix).
2. For each test callsite, compute the expected value from TARP (or from the test's intended ΔT and design conditions) and replace the constant reference with an inline computation or a test-local constant.
3. Delete `R_FILM_INTERIOR_M2_K_W` from `boundary_rc.rs`.
4. If any production callsite is found during the audit, migrate it to the TARP/h_rad path immediately (do not defer — `feedback_no_broken_windows`).

## Definition of Done

- [ ] Audit complete; no production code references `R_FILM_INTERIOR_M2_K_W`
- [ ] All test callsites migrated to compute expected values from first principles (or from documented test-local constants)
- [ ] `R_FILM_INTERIOR_M2_K_W` deleted from `boundary_rc.rs`
- [ ] No grep hit for `R_FILM_INTERIOR_M2_K_W` anywhere in the workspace after the fix

## Verification

```bash
cargo test -p hares-envelope boundary_rc
cargo test -p hares-envelope thermal_solver
rg R_FILM_INTERIOR_M2_K_W   # should return zero hits after fix
```

## References

- ISO 6946:2017 *Building components and building elements — Thermal resistance and thermal transmittance* — combined surface resistance values (0.13 m²·K/W interior, 0.04 m²·K/W exterior).
- ASHRAE Handbook of Fundamentals 2021 Ch. 26 §26.4 "Surface Resistances" — comparison of combined vs convection-only film models.
- EnergyPlus Engineering Reference §3.5 "Inside Surface Heat Balance" — production uses convection (TARP) plus explicit longwave network, not combined film.

## Related Tickets

- 089-radiation-frac-starmesh-rederivation
- 090-rederive-per-surface-ua-from-first-principles
- 101-interior-film-coefficients-recompute-per-step
