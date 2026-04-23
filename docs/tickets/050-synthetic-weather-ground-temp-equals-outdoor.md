# Synthetic Weather Ground Temperature Tracks Outdoor Air Temperature Instantaneously

**Severity**: Low
**Status**: Open
**Areas**: hares-core/synthetic

## Problem

`build_synthetic_weather` at `crates/hares-core/src/dwelling/synthetic.rs:794` sets:
```
let ground_temp_c = outdoor_temp_c;
```
and fills every timestep in the `ground_temp_c` vector with this single value (`synthetic.rs:810`).
The comment at lines 781–793 acknowledges the approximation is only physically correct when the
synthetic outdoor temperature equals the annual mean (zero seasonal variation). For any synthetic
profile that varies — a step cold-snap, a ramp, or a diurnal cycle — ground temperature tracks
outdoor air with zero thermal lag, which is physically impossible: the thermal mass of the soil
means real ground temperatures lag outdoor air by weeks to months (Kusuda and Achenbach (1965)
ASHRAE Transactions 71(1):61-74 §2).

The practical impact is on tests that use non-constant synthetic weather. For a synthetic
step-change to -20 °C, ground coupling heat loss is computed against a -20 °C boundary instead of
a physically realistic 5–10 °C, producing errors of 2–5× in the ground coupling load term. This
causes synthetic-weather integration tests to be unreliable predictors of real EPW-path results.

There is no validation that the caller's synthetic profile is constant-temperature before the
shortcut is applied. The existing LIMITATION comment is accurate but provides no enforcement.

## Current Behavior

`crates/hares-core/src/dwelling/synthetic.rs:794`:
```
let ground_temp_c = outdoor_temp_c;
```
`synthetic.rs:810`:
```
ground_temp_c: vec![ground_temp_c; n],
```
Every record in the synthetic weather series receives the same `outdoor_temp_c` value as the
ground temperature, with no check that `outdoor_temp_c` is a steady-state mean.

## Required Behavior

1. In `SyntheticTomlConfig` (or its weather sub-struct), add an optional field
   `ground_temp_c: Option<f64>`. When `Some(t)`, use that value as the constant ground temperature.
   When `None`, use the temporal mean of the synthetic dry-bulb series as the ground temperature.
   Do not use `outdoor_temp_c` directly — it is the per-record value, not the mean.
2. Remove the code path that sets `ground_temp_c = outdoor_temp_c`. The mean of the dry-bulb
   series is the correct approximation for a constant-diffusivity soil at depth: it equals
   `t_mean_annual` in the Kusuda-Achenbach model with amplitude set to zero (no seasonal variation),
   per EnergyPlus Engineering Reference §3.1 (simplified ground heat transfer for
   constant-boundary cases).
3. The existing LIMITATION comment must be replaced with a brief statement of what the code
   actually does (uses temporal mean), not a caveat about what it doesn't do.

## Approach

1. Add `ground_temp_c: Option<f64>` to the weather section of `SyntheticTomlConfig`.
2. In `build_synthetic_weather`, compute the temporal mean of the dry-bulb output vector
   (`vec![outdoor_temp_c; n]` — since synthetic weather is currently constant, the mean equals
   `outdoor_temp_c`, making step 2 a no-op for existing constant-profile callers but correct for
   future time-varying synthetic profiles).
3. Set `let ground_temp_c = config.weather.ground_temp_c.unwrap_or(temporal_mean_c)`.
4. Remove the LIMITATION comment block at lines 781–793. Replace with a single line documenting
   that `ground_temp_c` is the temporal mean of the dry-bulb series (or explicit override), which
   represents an undisturbed deep-ground approximation with zero seasonal amplitude.

## Definition of Done

- `SyntheticTomlConfig` has `ground_temp_c: Option<f64>`.
- `build_synthetic_weather` computes ground temperature from the temporal mean of the dry-bulb
  series, not from the per-record outdoor temperature.
- When `ground_temp_c` is explicitly set in config, that value is used without modification.
- Test: a synthetic profile with `outdoor_temp_c = -20.0` and no `ground_temp_c` override
  produces `ground_temp_c = -20.0` (the mean of a constant series) — identical to current
  behavior for constant-temperature profiles, confirming no regression.
- Test: when `ground_temp_c = 8.0` is set in config, the weather series carries 8.0 regardless
  of the outdoor temperature.

## Verification

```
cargo test -p hares-core synthetic
```

The synthetic test suite must include both cases above. Numeric assertions must use exact equality
for constant-temperature profiles and within-0.01 °C tolerance for mean computations.

## References

- Kusuda, T. and Achenbach, P.R. (1965), ASHRAE Transactions Vol. 71(1), pp. 61-74 — ground
  temperature at depth approaches annual mean as amplitude approaches zero
- EnergyPlus Engineering Reference §3.1 "Ground Heat Transfer Calculations Using a Simplified
  Approach" — undisturbed ground temperature at depth as function of annual mean and amplitude

## Cross-Cluster Note

This ticket is adjacent to ticket 028 (wind-speed-no-terrain-correction), which also concerns the
synthetic weather builder producing physically incorrect boundary conditions. If ticket 028 is
reworked, the synthetic weather builder tests may share fixtures.
