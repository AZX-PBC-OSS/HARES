# Solar Upsampling: Triangular Default Does Not Preserve Hourly Energy Budget

**Severity**: Medium
**Impact on annual kWh**: Medium
**Status**: Open
**Areas**: hares-io/weather (resample)

## Problem

GHI, DNI, and DHI use `ResampleMethod::Triangular` as the default resampling method when upsampling hourly weather data to sub-hourly resolution (`crates/hares-io/src/weather.rs:553-566`).

The triangular tent function is centered at the midpoint of each hour so that `interpolant(midpoint) == hourly_value`. However, the mean of the triangular interpolant over the full hour is not equal to the original hourly value. For a tent centered at hour `i`:

```
hourly_mean = values[i-1]/4 + values[i]/2 + values[i+1]/4
```

This is a weighted blend with neighboring hours. At sunrise/sunset transitions, where one hour has significant irradiance and the adjacent hour has zero (nighttime), the method bleeds energy across the boundary. At 15-minute resolution, a last-sunlit-hour value of 400 W/m² produces sub-hourly values of [400, 300, 200, 100] W/m² and the nighttime hour receives [100, 0, 0, 0] W/m² — a 25 W/m² mean in an hour that should be zero.

Hourly solar irradiance values represent period averages, not instantaneous midpoint measurements (ASHRAE Handbook of Fundamentals 2021 Ch. 14 §14.6). Sub-hourly upsampling must preserve the hourly integral: the mean of all sub-hourly values within an hour must equal the original hourly value.

EnergyPlus WeatherManager.cc `SetupInterpolationValues` (approximately lines 8328–8380) uses a weighted blend that satisfies this conservation constraint exactly.

## Current Behavior

`crates/hares-io/src/weather.rs:553-566`: GHI, DNI, DHI default to `ResampleMethod::Triangular`.

At sunrise/sunset, triangular resampling bleeds approximately 10–25 W/m² of solar energy into adjacent nighttime hours. The nighttime hour that physically has zero irradiance receives nonzero sub-hourly values, biasing cooling loads in hours that should have no solar input.

OCHRE uses zero-order hold (`resample().ffill()`, equivalent to `ResampleMethod::Zoh`) for solar resampling, which preserves hourly averages exactly by holding each hourly value constant within its hour.

## Required Behavior

Change the default resampling method for GHI, DNI, and DHI from `ResampleMethod::Triangular` to `ResampleMethod::Zoh`. ZOH holds each hourly value constant within its interval, preserving the hourly energy integral exactly. The reduction in temporal smoothness is the correct tradeoff: physical solar irradiance is not smooth at sub-hourly scales, and the triangular method's smoothness comes at the cost of energy conservation.

`ResampleMethod::Triangular` must remain available via `ResampleOverrides` for users who explicitly want smooth sub-hourly profiles and accept the energy conservation deviation. The `Triangular` docstring must state explicitly that it does not preserve hourly energy integrals and quantify the expected boundary error (up to ±25 W/m² at sunrise/sunset).

No silent defaults: the ZOH default is a documented, citable choice. The `Triangular` override is a documented deviation requiring explicit user opt-in.

Primary citations:
- ASHRAE Handbook of Fundamentals 2021 Ch. 14 §14.6 "Solar Radiation" — hourly values are period averages; sub-hourly upsampling must preserve the hourly integral
- EnergyPlus Engineering Reference §2.5.5 "Solar Radiation" — energy-conservative upsampling approach

## Approach

In `crates/hares-io/src/weather.rs`, locate the `resample_with` function's default method selection for `ghi`, `dni`, `dhi` columns. Change from `ResampleMethod::Triangular` to `ResampleMethod::Zoh`. Update the `Triangular` docstring to add the energy conservation warning. Update any tests that assert `Triangular` as the solar default.

## Definition of Done

- [ ] Default resampling for GHI, DNI, DHI changed to `Zoh`
- [ ] Test: hourly mean of ZOH sub-hourly GHI values equals original hourly GHI for a representative sunrise/sunset sequence (max absolute error < 1e-9 W/m²)
- [ ] Test: triangular method at a sunset boundary produces a nonzero mean in the nighttime hour (regression guard documenting the known limitation)
- [ ] `ResampleMethod::Triangular` docstring states it does not preserve hourly energy integrals
- [ ] `ResampleOverrides` allows explicit `Triangular` selection for solar channels

## Verification

```bash
cargo test -p hares-io resample
cargo test -p hares-io weather_parity
```

Specific invariant: for a sequence `[400.0, 0.0, 0.0]` (W/m², last sunlit hour followed by two nighttime hours), ZOH sub-hourly means at 15-minute resolution must equal `[400.0, 0.0, 0.0]` exactly.
