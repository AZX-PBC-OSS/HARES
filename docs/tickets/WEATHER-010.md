---
id: WEATHER-010
title: Full-pipeline weather integration test (EPW → resample → environment state)
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-core/tests/weather_integration.rs
references:
  - crates/hares-io/src/epw.rs
  - crates/hares-io/src/weather.rs
  - crates/hares-core/src/environment.rs
  - crates/hares-physics/src/solar.rs
  - crates/hares-physics/src/psychrometrics.rs
verification:
  - cargo build --workspace
  - cargo test -p hares-core
---

## Background/Context

Individual weather components are well-tested (PCHIP interpolation, EPW/PSM3 parsing,
solar position, psychrometrics, Perez model), but there is no integration test that
verifies the full pipeline:

```
EPW file → parse_epw → WeatherTimeSeries → resample(60s) → EnvironmentManager
→ update() → WeatherState (with derived psychrometrics + per-surface irradiance)
```

A regression in any intermediate step (e.g., wrong field index, unit mismatch,
incorrect resampling) could silently produce wrong physics without any test catching
it. This test closes that gap.

## Work to Do

- [ ] Create `crates/hares-core/tests/weather_integration.rs` with:

### End-to-end pipeline test (synthetic data)
- [ ] `full_pipeline_synthetic_weather`: Construct a synthetic 24-hour
      `WeatherTimeSeries` with known values, resample to 60s, create
      `EnvironmentManager`, step through 24 hours, verify:
  - Outdoor temperature matches expected PCHIP interpolation at sub-hourly points
  - Humidity ratio is physically consistent with dew point + pressure
  - Wet bulb < dry bulb (always, for non-saturated conditions)
  - Enthalpy increases with temperature (for constant humidity)
  - Solar altitude is 0 at known sunrise/sunset times
  - GHI = 0 when solar altitude ≤ 0
  - Per-surface irradiance is non-negative
  - Mains water temperature is finite and in reasonable range [5, 25]°C
  - Ground temperature is finite and varies less than air temperature
  - Precipitation is correctly distributed (integral preserved)

### Psychrometric consistency test
- [ ] `psychrometric_chain_consistent`: At each timestep, verify:
  - `wet_bulb < dry_bulb` (non-saturated)
  - `humidity_ratio >= 0`
  - `enthalpy > 0` (for T > 0°C)
  - `dew_point ≤ dry_bulb` (from EPW validation, should propagate)

### Solar irradiance physical constraints
- [ ] `solar_irradiance_physical_bounds`: Verify for each timestep:
  - Total surface irradiance ≤ extraterrestrial radiation (~1361 W/m²)
  - GHI ≥ DHI (global ≥ diffuse)
  - DNI × cos(zenith) ≈ GHI - DHI (closure, ±10% tolerance for Perez model)
  - Night hours: all solar components = 0

### Resample → environment consistency
- [ ] `resampled_weather_produces_smooth_environment`: Resample to 60s, step
      through, verify:
  - No NaN/Inf in any WeatherState field
  - Temperature transitions are smooth (no hour-boundary jumps > 0.5°C)
  - Pressure is constant or varies slowly (PCHIP)

## Files to Touch

- `crates/hares-core/tests/weather_integration.rs`: New file

## Measures of Success

- [ ] All physical invariants hold at every sub-hourly timestep
- [ ] No NaN/Inf values produced in the full pipeline
- [ ] Catches regressions in field indexing, unit conversions, or resampling

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test -p hares-core` passes
