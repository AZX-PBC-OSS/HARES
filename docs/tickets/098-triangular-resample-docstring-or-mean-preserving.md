# `Triangular` Resample Docstring Misclaims Hourly Mean Preservation

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-io/weather

## Problem

The `Triangular` resample method docstrings at `crates/hares-io/src/weather.rs:292` and `crates/hares-io/src/weather.rs:1055` claim "hourly mean preservation". The actual hourly mean of the triangular interpolant is:

```
mean_hour_i = 0.125 * value[i-1] + 0.75 * value[i] + 0.125 * value[i+1]
```

This is a weighted blend with neighbours, not a mean-preserving filter. At a sunrise transition with `prev=0, cur=800, next=800` W/m², the resulting hourly mean is 700 W/m² — a 12.5% shortfall against the original 800 W/m². The docstring is materially wrong; users relying on the documented "hourly mean preservation" property will get incorrect annual solar gains.

Two valid resolutions:

A. **Correct the docstring** — state the actual `0.125 * prev + 0.75 * cur + 0.125 * next` formula and quantify the boundary error, point users to `Zoh` or a true mean-preserving filter when integral conservation matters.

B. **Replace the implementation** with a true mean-preserving (energy-conservative) upsampling filter — e.g. piecewise-constant ZOH, or a midpoint-anchored filter that integrates back to the cell-mean.

## Current Behavior

`crates/hares-io/src/weather.rs:292` (and:1055): docstring claims hourly mean preservation. Implementation does not preserve the mean.

Numerical example: GHI series `[0, 800, 800]` W/m² resampled to 15-minute steps via the triangular tent centred at each hour midpoint produces sub-hourly values that, when averaged back over the second hour, yield 700 W/m² rather than 800 W/m². At seasonal aggregation this introduces a ~5% bias in solar-driven loads at sunrise/sunset transitions.

## Required Behavior

Either:

A. The docstring is corrected to the exact formula `mean_hour_i = 0.125 * value[i-1] + 0.75 * value[i] + 0.125 * value[i+1]`, with a worked example showing the 12.5% sunrise-hour shortfall and a recommendation to use `Zoh` (or a future mean-preserving method) for integrals.

OR

B. The implementation is replaced with a genuinely mean-preserving filter. The simplest option is `Zoh` (already the recommendation in ticket 030 for solar channels). For users who explicitly want smooth interpolation, document the trade-off and provide a method named `TriangularLossy` or similar.

The two-pronged choice exists because some users want smoothness and accept the boundary error; the docstring lie is the violation, not the smoothness method itself.

## Approach

Recommended path: implement A (docstring correction), since `Triangular` is already used as an opt-in option and `Zoh` is the documented default for solar channels in ticket 030. The replacement-implementation path (B) is the more invasive change.

1. Open `crates/hares-io/src/weather.rs:292` and `:1055` and replace the docstring with the corrected formula and a worked example.
2. Add a `WARNING:` block in the docstring stating that integral conservation is not guaranteed and recommending `Zoh` for energy-conservative use.
3. Add a unit test asserting the mean blend formula, so any future implementation change either preserves the formula or updates the docstring.
4. Cross-link this docstring to the existing ticket 030 which addresses the default for solar channels.

## Definition of Done

- [ ] `Triangular` docstring at `crates/hares-io/src/weather.rs:292` corrected to the actual blend formula
- [ ] `Triangular` docstring at `crates/hares-io/src/weather.rs:1055` corrected likewise
- [ ] Worked example in docstring shows the sunrise-hour 12.5% shortfall
- [ ] Recommendation to use `Zoh` for integral conservation included
- [ ] Unit test asserts `mean(triangular_resample([0, 800, 800])) == [0, 700, 800]` (the documented behaviour)
- [ ] No code claims "preserves hourly mean" for the triangular method anywhere

## Verification

```bash
cargo test -p hares-io resample
cargo test -p hares-io weather
cargo doc -p hares-io --no-deps   # confirm corrected docstring renders
```

## References

- ASHRAE Handbook of Fundamentals 2021 Ch. 14 §14.6 "Solar Radiation" — hourly solar values are period averages; sub-hourly upsampling must preserve the hourly integral if used for energy calculations.
- EnergyPlus Engineering Reference §2.5.5 "Solar Radiation Calculations" — energy-conservative upsampling.
- Press et al. *Numerical Recipes* 3rd ed. §3.3 "Cubic Spline Interpolation" — smooth interpolation methods do not generally preserve cell averages.

## Related Tickets

- 030-solar-upsampling-triangular-mean-error (default for solar channels — switch to Zoh)
- 099-resstock-csv-midpoint-offset (related weather-pipeline correction)
