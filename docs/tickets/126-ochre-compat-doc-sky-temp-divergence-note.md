# `ochre_compat()` Doc Should Note Sky-Temperature Divergence

**Severity**: Nit
**Priority**: P4
**Status**: Open
**Areas**: hares-io/weather

## Problem

`ochre_compat()` at `crates/hares-io/src/weather.rs:320` is documented as the OCHRE-compatible weather-resampling configuration, but the doc does not note that the resulting sky temperature stream still diverges from OCHRE's pre-computed zero-order-hold (ZOH) sky temperature. HARES recomputes sky temperature from the interpolated dew-point and dry-bulb inputs at each resampled timestep, so the result is physically consistent with the (smooth) interpolated humidity inputs — but it is not bit-identical to OCHRE, which carries forward whatever sky temperature the source weather file declared at the hour boundary.

A user enabling `ochre_compat()` expecting bit-identical OCHRE behaviour will be surprised by sky-temperature differences that are entirely correct but not reproduced.

## Current Behavior

`crates/hares-io/src/weather.rs:320` (approximately):
```rust
/// OCHRE-compatible defaults: ZOH for all channels.
pub fn ochre_compat() -> Self { ... }
```

Doc does not mention sky temperature.

## Required Behavior

The doc comment must add a note explaining:

1. HARES recomputes sky temperature at each resampled timestep from interpolated dry-bulb and dew-point inputs (Berdahl-Martin form, see `epw.rs`).
2. OCHRE carries the source-file sky temperature forward via ZOH and never recomputes.
3. Therefore, even with `ochre_compat()` selected, sky temperature will differ from OCHRE between hour boundaries — HARES is more physically consistent (no humidity/sky-temperature mismatch) but not bit-identical.

## Approach

Update the doc comment at `crates/hares-io/src/weather.rs:320`:

```rust
/// OCHRE-compatible defaults: zero-order-hold for all interpolated channels.
///
/// Note: even with this configuration selected, the resampled sky temperature
/// stream is not bit-identical to OCHRE's. OCHRE carries the source-file sky
/// temperature forward via ZOH, while HARES recomputes sky temperature at
/// each resampled timestep from the interpolated dry-bulb and dew-point
/// inputs (see `epw::berdahl_martin_sky_temp`). HARES's behaviour is
/// physically more consistent — the sky temperature always agrees with the
/// humidity that produced it — but the resulting stream will differ from
/// OCHRE between hour boundaries.
pub fn ochre_compat() -> Self { ... }
```

## Definition of Done

- [ ] Doc comment at `crates/hares-io/src/weather.rs:320` updated with the divergence note
- [ ] Comment cites the recomputation site (`epw::berdahl_martin_sky_temp`) so a future reader can find the source

## Verification

```bash
cargo doc -p hares-io --no-deps
cargo test -p hares-io weather
```

## References

- HARES `crates/hares-io/src/epw.rs:529-531` — sky-temperature recomputation site (Berdahl-Martin form).
- HARES `vendors/OCHRE/ochre/utils/schedule.py` — OCHRE's ZOH-only sky-temperature handling.
- EnergyPlus Engineering Reference §3.5.6 "Sky Emissivity Calculations" — primary-source justification for the recomputation approach.

## Related Tickets

- 097-tmy3-midpoint-offset-regression-test (related ochre_compat regression coverage)
- 098-triangular-resample-docstring-or-mean-preserving (related resample-doc accuracy)
