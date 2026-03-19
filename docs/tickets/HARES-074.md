---
id: HARES-074
title: Weather/EPW loading correctness tests against OCHRE
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-io/tests/weather_parity.rs
  - crates/hares-physics/src/water_mains.rs
  - crates/hares-physics/tests/solar_parity.rs
  - tests/fixtures/parity/weather/
references:
  - vendors/OCHRE/ochre/Dwelling.py
  - vendors/OCHRE/ochre/utils/schedule.py (mains temp)
  - crates/hares-io/src/epw.rs
  - crates/hares-io/src/weather.rs
  - crates/hares-physics/src/solar.rs
verification:
  - cargo test -p hares-io weather_parity
  - cargo test -p hares-physics solar_parity
  - cargo clippy
---

## Background/Context

Weather loading is mostly aligned, but two critical gaps exist: (1) mains water temperature (Burch-Christensen correlation) is not implemented, causing water heater inlet temp errors; (2) ground temperature uses a simplified model vs OCHRE's DOE-2 monthly damping. Solar position and Perez model match but need reference-value tests.

## Work to Do

- [ ] Create `crates/hares-io/tests/weather_parity.rs`
- [ ] **Test: EPW column parsing** — Load a real EPW (use existing test fixture), verify all 13 columns parsed with correct SI units (°C, kPa, W/m², m/s)
- [ ] **Test: sky temperature** — For known infrared radiation values, verify sky temperature matches OCHRE's Stefan-Boltzmann inversion
- [ ] **Test: Clark-Allen fallback** — For IR < 50 W/m², verify fallback formula matches OCHRE
- [ ] **Test: mains water temperature** — Implement Burch-Christensen correlation (`T_mains = T_offset + ratio * T_amb_avg + lag * amplitude * sin(2π/365 * (day - phase))`), test against OCHRE's output for Denver CO (lat 39.7, annual avg 10°C)
- [ ] **Test: ground temperature** — Compare HARES ground temp calculation against OCHRE for same EPW, document any model differences, target < 2°C max deviation
- [ ] Create `crates/hares-physics/tests/solar_parity.rs`
- [ ] **Test: solar declination** — For known day-of-year values (equinox, solstices), verify declination within 0.1°
- [ ] **Test: Perez diffuse model** — For known GHI/DNI/DHI/zenith at lat 40°, tilt 30°, azimuth 180°, verify POA diffuse matches pvlib within 1%
- [ ] **Test: POA total irradiance** — Full POA = beam + diffuse + reflected for a south-facing 30° panel, compare to pvlib reference
- [ ] **Test: sub-hourly ZOH** — Verify 15-min resampled values exactly replicate hourly source

## Measures of Success

- [ ] All EPW parsing validated with real fixture
- [ ] Mains water temperature implemented and tested
- [ ] Solar calculations tested against pvlib reference values
- [ ] Ground temp deviation documented

## Verification

- [ ] `cargo test -p hares-io weather_parity` passes
- [ ] `cargo test -p hares-physics solar_parity` passes
- [ ] `cargo clippy` passes
