---
id: WEATHER-002
title: Sub-hourly interpolation quality and parity tests
kind: implement
depends_on:
  - WEATHER-001
files_to_touch:
  - crates/hares-io/tests/weather_parity.rs
references:
  - crates/hares-io/src/weather.rs
  - Fritsch & Carlson 1980 "Monotone Piecewise Cubic Interpolation"
  - https://bigladdersoftware.com/epx/docs/9-6/engineering-reference/climate-calculations.html
verification:
  - cargo build --workspace
  - cargo clippy --workspace -- -D warnings
  - cargo test --workspace
---

## Background/Context

WEATHER-001 adds PCHIP-based physics-aware interpolation to `resample()`. This ticket
adds dedicated test coverage verifying correctness, smoothness, monotonicity
preservation, and physical bound compliance at realistic timesteps (60s, 300s, 900s).

## Work to Do

- [ ] Add tests in `crates/hares-io/tests/weather_parity.rs`:

### PCHIP correctness tests
- [ ] `pchip_passes_through_knot_values`: verify interpolated series contains the
      original hourly values at the correct positions (interpolating, not approximating)
- [ ] `pchip_is_c1_continuous`: verify the first derivative is continuous at knot
      points (no slope jumps) — check via finite difference across knot boundaries

### Monotonicity / no-overshoot tests
- [ ] `pchip_preserves_monotonicity`: given a monotonically increasing sequence of
      hourly temperatures (e.g., 10, 12, 15, 20), verify all sub-hourly values are
      also monotonically increasing
- [ ] `pchip_no_overshoot_between_knots`: for any pair of consecutive hourly values,
      verify all interpolated sub-hourly values lie within
      [min(v_k, v_{k+1}), max(v_k, v_{k+1})]
- [ ] `pchip_handles_flat_sections`: given constant hourly values (e.g., 20, 20, 20),
      verify all sub-hourly values are exactly 20.0
- [ ] `pchip_handles_steep_gradient`: given a sharp transition (e.g., 10, 10, 30, 30),
      verify no overshoot beyond 30 or undershoot below 10

### Physical bounds tests (verify defensive clamping works)
- [ ] `rh_stays_bounded_after_interpolation`: construct a series where RH oscillates
      near bounds with non-monotonic extrema (e.g., [98, 2, 98, 5, 95]) and verify
      all interpolated values are in [0, 100] after PCHIP + clamp
- [ ] `rh_non_monotonic_extrema_clamped`: specifically test [0, 100, 0, 100] pattern
      to exercise PCHIP overshoot at local maxima, verify clamping catches it
- [ ] `sky_cover_stays_bounded_after_interpolation`: same for opaque_sky_cover [0, 10]
      with non-monotonic patterns
- [ ] `pressure_stays_positive_after_interpolation`: verify no negative pressure values

### Edge case and degenerate input tests
- [ ] `pchip_handles_single_element`: single hourly value → all sub-hourly values
      equal that value (no panic)
- [ ] `pchip_handles_two_elements`: two hourly values → linear interpolation fallback
- [ ] `pchip_handles_all_nan`: all-NaN input → all-NaN output (no panic, no corruption)

### ZOH preservation tests
- [ ] `solar_fields_remain_zoh_after_resample`: verify GHI/DNI/DHI are step-functions
      after resampling (all sub-hourly values identical within each source hour)
- [ ] `wind_fields_remain_zoh_after_resample`: same for wind_speed and wind_dir

### Year boundary test
- [ ] `year_boundary_uses_flat_extrapolation`: verify last half-interval of year does
      NOT smoothly wrap to first hour (TMY constraint); instead uses flat extrapolation

### NaN propagation test
- [ ] `nan_in_source_propagates_through_pchip`: insert NaN into one hourly value,
      verify NaN appears in the corresponding sub-hourly slots and does not corrupt
      neighboring intervals

### Real EPW fixture test
- [ ] `sub_hourly_resample_of_real_epw`: load the Denver TMY3 fixture, resample to
      60s, verify:
      - No NaN/Inf values
      - Temperature within [-60, 55]°C
      - RH within [0, 100]%
      - Solar non-negative
      - Precipitation integral preserved
      - Series length = 8760 * 60

### Multi-resolution test
- [ ] `pchip_consistent_across_timesteps`: resample to 60s, 300s, 900s and verify
      that the 300s values are a subset of (or very close to) the 60s values at
      matching time points

## Files to Touch

- `crates/hares-io/tests/weather_parity.rs`: Add sub-hourly interpolation tests

## Measures of Success

- [ ] All new tests pass at 60s, 300s, and 900s timesteps
- [ ] Tests verify PCHIP properties (monotonicity, C1 continuity, interpolating)
- [ ] Tests verify physical bounds are maintained without clamping
- [ ] No regression in existing parity tests

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo test --workspace` passes
