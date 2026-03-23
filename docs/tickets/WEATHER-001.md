---
id: WEATHER-001
title: Physics-aware sub-hourly interpolation for continuous weather fields
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-io/src/weather.rs
references:
  - vendors/OCHRE/ochre/utils/schedule.py (resample_and_reindex, lines 480-568)
  - crates/hares-io/src/epw.rs (field definitions)
  - https://bigladdersoftware.com/epx/docs/9-6/engineering-reference/climate-calculations.html
  - Fritsch & Carlson 1980 "Monotone Piecewise Cubic Interpolation" (SIAM J. Numer. Anal.)
verification:
  - cargo build --workspace
  - cargo clippy --workspace -- -D warnings
  - cargo test --workspace
---

## Background/Context

HARES currently resamples all weather fields via zero-order-hold (`replicate_zoh`),
which copies each hourly value unchanged into every sub-hourly slot. For continuous
physical quantities (temperature, dew point, pressure, sky temperature, ground
temperature, infrared radiation), this creates non-physical step discontinuities at
hour boundaries during sub-hourly simulation.

At 1-minute timesteps, outdoor temperature jumps instantaneously every 60 steps.
This causes spurious thermal-load spikes, HVAC cycling artifacts, and numerical
instability in stiff thermal models.

### EnergyPlus reference behavior

EnergyPlus uses the following approach for sub-hourly weather (from the Engineering
Reference, Climate Calculations):

- **Temperature, humidity, pressure** (state variables): Linear interpolation between
  hourly values. EPW values are treated as **instantaneous measurements at the hour
  boundary** (1:00, 2:00, etc.).
- **Solar radiation** (GHI, DNI, DHI): Values are period-average energy fluxes.
  The average rate is assumed to be the value at the **midpoint** of the hour.
  Sub-hourly values are then linearly interpolated between consecutive midpoints.
- **Wind**: Not explicitly documented; treated as instantaneous.

### Exceeding EnergyPlus: PCHIP interpolation

Linear interpolation produces C0-continuous curves (continuous but with slope
discontinuities at knots). PCHIP (Piecewise Cubic Hermite Interpolating Polynomial,
Fritsch-Carlson 1980) produces C1-continuous curves that:

1. Pass exactly through original data points (interpolating, not approximating)
2. Preserve monotonicity between data points (no overshoot)
3. Are smooth (continuous first derivative) at knot points
4. Preserve monotonicity within each segment (no oscillation in monotonic runs)

Unlike cubic splines, PCHIP eliminates oscillation within monotonic data segments.
However, PCHIP does NOT guarantee global boundedness for non-monotonic data — e.g.,
a sequence [98, 2, 98] could produce slight overshoot near the local minimum.
Therefore, bounded fields (RH, sky cover) require **defensive clamping** after
interpolation.

The Fritsch-Carlson algorithm is ~50 lines of code with no external dependencies.

### Field categorization

| Field category | Fields | Method | Anchor point |
|---|---|---|---|
| Continuous instantaneous | dry_bulb, dew_point, pressure, horizontal_infrared, sky_temp, ground_temp | PCHIP | Hour boundary |
| Bounded continuous | rel_humidity (0-100%), opaque_sky_cover (0-10) | PCHIP + clamp | Hour boundary |
| Period-average energy flux | GHI, DNI, DHI | ZOH | Midpoint of hour |
| Turbulent / stochastic | wind_speed, wind_dir | ZOH | N/A |
| Accumulated depth | liquid_precip | Distribute (already correct) | N/A |

Note: `rel_humidity_pct` and `opaque_sky_cover` are physically bounded. PCHIP
prevents oscillation in monotonic segments but can overshoot at non-monotonic
extrema. Defensive clamping (RH to [0, 100], sky cover to [0, 10]) is applied
after interpolation as a safety net.

## Work to Do

- [ ] Implement `fritsch_carlson_slopes(x: &[f64], y: &[f64]) -> Vec<f64>` in
      `weather.rs` — computes monotone-preserving slopes at each knot.
      Doc comment MUST cite:
      ```
      /// Fritsch-Carlson monotone cubic interpolation slopes.
      ///
      /// Algorithm: Fritsch, F.N. and Carlson, R.E. (1980)
      /// "Monotone Piecewise Cubic Interpolation",
      /// SIAM Journal on Numerical Analysis, 17(2), pp. 238-246.
      /// doi:10.1137/0717021
      ```
      Implementation of the Fritsch-Carlson algorithm:
      1. Compute secant slopes `delta_k = (y[k+1] - y[k]) / (x[k+1] - x[k])`
      2. Initialize tangent `d_k = (delta_{k-1} + delta_k) / 2`
      3. Where `delta_{k-1}` and `delta_k` have different signs, set `d_k = 0`
      4. Apply Fritsch-Carlson correction: let `alpha_k = d_k / delta_k`,
         `beta_k = d_{k+1} / delta_k`. If `alpha_k^2 + beta_k^2 > 9`:
         ```
         tau_k = 3.0 / sqrt(alpha_k^2 + beta_k^2)
         d_k   = tau_k * alpha_k * delta_k
         d_k+1 = tau_k * beta_k  * delta_k
         ```
      5. Boundary slopes: use one-sided difference

- [ ] Implement `pchip_resample(values: &[f64], factor: usize) -> Vec<f64>`.
      Doc comment MUST cite:
      ```
      /// Resample hourly data to sub-hourly using PCHIP interpolation.
      ///
      /// Uses Piecewise Cubic Hermite Interpolating Polynomials (PCHIP)
      /// with Fritsch-Carlson monotonicity-preserving slopes.
      /// Guarantees: C1 continuity, monotonicity preservation,
      /// interpolating (passes through original data points).
      /// Hermite basis functions per Press et al., Numerical Recipes, 3rd ed., §3.3.
      ///
      /// Reference: Fritsch & Carlson (1980), SIAM J. Numer. Anal. 17(2).
      ```
      Implementation:
      1. Constructs uniform x-coordinates for hourly knots (0, 1, 2, ...)
      2. Computes Fritsch-Carlson slopes
      3. Evaluates the cubic Hermite polynomial at each sub-hourly sample point
      4. For each interval [x_k, x_{k+1}], uses the standard Hermite basis:
         `p(t) = h00(t)*y_k + h10(t)*d_k + h01(t)*y_{k+1} + h11(t)*d_{k+1}`
         where `t = (x - x_k) / (x_{k+1} - x_k)` and h00..h11 are Hermite basis
         functions

- [ ] Update `WeatherTimeSeries::resample()` to call `pchip_resample` for:
      `dry_bulb_c`, `dew_point_c`, `rel_humidity_pct`, `pressure_kpa`,
      `horizontal_infrared_w_m2`, `sky_temp_c`, `ground_temp_c`, `opaque_sky_cover`

- [ ] After PCHIP interpolation, apply defensive clamping for bounded fields:
      - `rel_humidity_pct`: clamp each value to `[0.0, 100.0]`
      - `opaque_sky_cover`: clamp each value to `[0.0, 10.0]`
      This is needed because PCHIP preserves monotonicity within each segment but
      can produce slight overshoot at non-monotonic extrema (e.g., [98, 2, 98]).

- [ ] Keep `replicate_zoh` for: `ghi_w_m2`, `dni_w_m2`, `dhi_w_m2`, `wind_speed_m_s`,
      `wind_dir_deg`

- [ ] Keep `distribute_accumulated` for: `liquid_precip_m` (already correct)

- [ ] Handle edge cases:
      - Single-element input → replicate (no neighbor to interpolate toward)
      - Two-element input → fall back to linear interpolation
      - Year boundary: do NOT wrap (TMY data stitches different years at the
        boundary; smooth wrapping is physically meaningless). Use flat extrapolation
        (repeat boundary value) for the last/first half-intervals.
      - NaN in source data → propagate NaN through interpolation (NaN in → NaN out).
        The EPW parser should have already rejected/replaced invalid values, but
        defensive NaN propagation prevents silent corruption.

- [ ] Update the existing `zoh_resample_repeats_each_hourly_value` test in
      `weather.rs:208` — it currently asserts ZOH on `dry_bulb_c`, which will now
      use PCHIP. Change it to assert PCHIP behavior (smooth transition) or split
      into separate tests for ZOH fields (solar/wind) and PCHIP fields (temp/humidity).

- [ ] Make `pchip_resample` and `fritsch_carlson_slopes` visibility `pub(crate)` so
      that integration tests in `crates/hares-io/tests/weather_parity.rs` (WEATHER-002)
      can call them directly for unit-level verification.

- [ ] Add unit tests in the `tests` module of `weather.rs`

## Files to Touch

- `crates/hares-io/src/weather.rs`: Add `fritsch_carlson_slopes`, `pchip_resample`,
  update `resample()`

## Measures of Success

- [ ] Continuous fields produce smooth C1-continuous transitions between hourly
      values at sub-hourly timesteps (no step or slope discontinuities)
- [ ] PCHIP preserves monotonicity within each segment (no oscillation in monotonic
      runs); defensive clamping catches any overshoot at non-monotonic extrema
- [ ] `rel_humidity_pct` never exceeds 100% or drops below 0% after interpolation
- [ ] `opaque_sky_cover` stays within [0, 10] after interpolation
- [ ] Solar fields (GHI/DNI/DHI) remain unchanged (ZOH)
- [ ] Wind fields remain unchanged (ZOH)
- [ ] Precipitation distribution is unchanged
- [ ] All existing tests still pass
- [ ] NaN values in source data propagate correctly (no silent corruption)
- [ ] Year boundary does not introduce artificial smooth transition between
      unrelated TMY months

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo test --workspace` passes
