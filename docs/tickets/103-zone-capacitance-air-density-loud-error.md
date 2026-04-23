# `derive_zone_capacitances` Must Error When `site_pressure_pa <= 0`

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-envelope/boundary_rc

## Problem

`derive_zone_capacitances` at `crates/hares-envelope/src/boundary_rc.rs:362-375` silently substitutes the sea-level air density `AIR_DENSITY_KG_M3 = 1.2041` when `site_pressure_pa <= 0`. A non-positive pressure is unambiguously a configuration or parsing error — there is no physical site at or below zero absolute pressure. Substituting a sea-level constant masks the error and produces zone capacitances that are wrong for any non-sea-level site.

## Current Behavior

`crates/hares-envelope/src/boundary_rc.rs:362-375`:
```rust
let rho_air = if site_pressure_pa > 0.0 {
    compute_air_density(site_pressure_pa, ...)
} else {
    AIR_DENSITY_KG_M3  // silent sea-level fallback
};
```

A misconfigured site (e.g. weather file with missing pressure column, EPW field 9 sentinel 999999) silently uses 1.2041 kg/m³ regardless of actual elevation. At a 1500 m site (Denver, e.g.) actual rho_air is ~1.05 kg/m³ — a 14% bias in zone air capacitance.

## Required Behavior

1. If `site_pressure_pa <= 0`, return `Err(BoundaryRcError::InvalidSitePressure { value })` with the offending value.
2. Do not substitute any fallback density.
3. The error propagates to the caller and surfaces as a hard initialisation failure.
4. The companion weather-loader path that produces `site_pressure_pa` must also surface the issue at parse time (separate ticket if not already addressed by ticket 029 / EPW pressure handling).

## Approach

1. Open `crates/hares-envelope/src/boundary_rc.rs:362-375` and replace the `else` branch with `return Err(...)`.
2. Add the error variant to the local error enum.
3. Plumb the error through the caller (`Dwelling::new` or solver builder).
4. Add a unit test asserting the error fires for `site_pressure_pa = 0.0` and `-1.0`.
5. Add a sanity test asserting valid pressures still produce non-zero capacitances.

## Definition of Done

- [ ] Silent fallback to `AIR_DENSITY_KG_M3` at `boundary_rc.rs:362-375` removed
- [ ] New error variant `BoundaryRcError::InvalidSitePressure` carries the offending value
- [ ] Error propagates to caller
- [ ] Unit tests cover `site_pressure_pa = 0.0`, `< 0.0`, and a representative valid value
- [ ] No other silent default in `derive_zone_capacitances` (audit complete)

## Verification

```bash
cargo test -p hares-envelope boundary_rc
cargo test -p hares-core dwelling
```

## References

- ASHRAE Handbook of Fundamentals 2021 Ch. 1 §1.2 "Atmospheric Pressure" — pressure is strictly positive; standard ISA formula `p(h) = 101325 * (1 - 2.25577e-5 * h)^5.2559`.
- EnergyPlus Engineering Reference §1 "Site:Location" — site pressure derived from elevation when not explicitly provided; never zero or negative.
- Project policy `feedback_no_silent_defaults.md`.

## Related Tickets

- 029-resstock-csv-constant-pressure (related ResStock pressure handling)
- 102-thermal-solver-init-indoor-zone-loud-error (parallel silent-default fix)
