---
id: WEATHER-008
title: Dynamic ground albedo from weather data and snow model
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-physics/src/solar.rs
  - crates/hares-io/src/weather.rs
  - crates/hares-io/src/epw.rs
  - crates/hares-io/src/psm3.rs
  - crates/hares-core/src/environment.rs
  - crates/hares-types/src/environment.rs
references:
  - https://bigladdersoftware.com/epx/docs/8-4/input-output-reference/group-location-climate-weather-file-access.html
  - EnergyPlus Engineering Reference, Climate Calculations — Ground Reflectance
  - NSRDB PSM3 Surface Albedo column
verification:
  - cargo build --workspace
  - cargo clippy --workspace -- -D warnings
  - cargo test --workspace
---

## Background/Context

HARES currently uses a hardcoded ground albedo of 0.2 for all reflected solar
calculations (`crates/hares-physics/src/solar.rs:12`). This is the EnergyPlus
default for bare ground but is wrong when:

1. **Snow is present**: Fresh snow albedo is 0.6–0.9; aged snow 0.4–0.6. This
   significantly increases reflected solar on vertical surfaces (walls, windows)
   and can increase heating loads by 5–15% in cold climates.

2. **PSM3 data includes measured albedo**: The NSRDB PSM3 format has a
   `Surface Albedo` column with satellite-derived values that vary seasonally
   and account for snow cover. We currently parse this column but don't use it.

3. **EnergyPlus supports monthly ground reflectance**: `Site:GroundReflectance`
   allows 12 monthly values, and snow indicators in the weather file can
   temporarily override them with `Site:GroundReflectance:SnowModifier`.

### What OCHRE does

OCHRE also uses a fixed albedo of 0.2 (`envelope.py:54, albedo=0.2`). So this
improvement exceeds both OCHRE and basic EnergyPlus behavior.

### Approach

Three tiers of albedo support:

1. **Fixed monthly albedo** (simple): Accept 12 monthly values from building config.
   Default to 0.2 for all months. EnergyPlus-compatible.

2. **Snow-modified albedo** (moderate): Use EPW snow indicator (present day flag)
   or temperature heuristic (e.g., ground temp < 0°C + recent precipitation) to
   switch between snow and bare-ground albedo values.

3. **Measured albedo from PSM3** (best): When PSM3 data is available, use the
   satellite-derived `Surface Albedo` column directly. This captures seasonal
   vegetation changes, snow cover, and urbanization effects.

## Work to Do

- [ ] Add `ground_albedo` field to `WeatherState` in `hares-types/src/environment.rs`
      (currently not present — albedo is hardcoded in solar.rs)

- [ ] Parse `Surface Albedo` from PSM3 data in `psm3.rs`:
  - Add to `WeatherTimeSeries` as a new optional field
    `pub surface_albedo: Option<Vec<f64>>`
  - Parse from PSM3 column if present
  - EPW and ResStock CSV: `None` (not available)

- [ ] Update `perez_tilted_irradiance` and `isotropic_tilted_irradiance` in `solar.rs`:
  - Accept `ground_albedo: f64` parameter instead of using `DEFAULT_GROUND_ALBEDO`
  - All callers must pass the value

- [ ] Update `EnvironmentManager::update()` in `environment.rs`:
  - When PSM3 albedo is available, use it per-timestep
  - When not available, use a default of 0.2 (or configurable monthly values)
  - Pass albedo to Perez/Liu-Jordan calls

- [ ] Add monthly albedo configuration option (optional, for EnergyPlus parity):
  - Accept `[f64; 12]` monthly ground reflectance values
  - Interpolate daily (same pattern as ground temperature)

- [ ] Add tests:
  - Verify PSM3 surface albedo is parsed and flows to solar calculations
  - Verify snow-season albedo increases reflected solar
  - Verify default 0.2 is unchanged when no albedo data available

## Measures of Success

- [ ] PSM3 surface albedo flows through to reflected solar calculation
- [ ] Default behavior (0.2) is unchanged for EPW/ResStock CSV
- [ ] Reflected solar increases with higher albedo (snow test case)

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
