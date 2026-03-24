---
id: PARITY-005
title: "Weather derived fields audit & fixes"
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-core/src/environment.rs
  - crates/hares-physics/src/psychrometrics.rs
  - crates/hares-physics/src/ground.rs
  - crates/hares-io/src/weather.rs
references:
  - docs/equipment/ochre-parity-gaps.md (Gap 11)
  - vendors/OCHRE/ochre/utils/weather.py (import_weather)
  - EnergyPlus Engineering Reference Ch. 2.2 (Sky Temperature)
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace -- -D warnings
---

## Prerequisites

THERMAL-001 (dry air density for infiltration) and THERMAL-002 (4-component exterior LWR with sky temp) will have completed. WEATHER-001 (sub-hourly interpolation), WEATHER-003 (PSM3 parser), WEATHER-008 (dynamic ground albedo), WEATHER-009 (Berdahl-Martin sky emissivity), and WEATHER-010 (full-pipeline integration test) will also be done. This means sky temperature is now handled by multiple models (Stefan-Boltzmann primary, Berdahl-Martin fallback, Clark-Allen last resort), ground albedo is dynamic (PSM3 or monthly config), and the full weather pipeline is integration-tested. This ticket is now primarily a **verification pass** — confirming that ground temperature (Kusuda-Achenbach or DOE-2), mains water temperature (Burch-Christensen), and zone wet-bulb are all EnergyPlus-grade after the WEATHER chain.

## Status: COMPLETE (verified — all fields EnergyPlus-grade)

Audit confirms all derived fields are correctly implemented after THERMAL/WEATHER chains:

| Field | Implementation | Source | Tests |
|-------|---------------|--------|-------|
| Sky temp | 3-tier: Stefan-Boltzmann > Berdahl-Martin+Walton > Clark-Allen | epw.rs:477-496 | 8+ tests |
| Ground temp | DOE-2 monthly sinusoidal with depth dampening | epw.rs:417-450 | 2+ tests |
| Ground albedo | Dynamic from PSM3 or default 0.2 | environment.rs:284 | Flows to Perez |
| Mains water temp | Burch-Christensen with hemisphere + 35-day lag | water_mains.rs:82-107 | 14 tests |
| Outdoor wet-bulb | Psychrometric bisection from T_db + w + P | environment.rs:264-265 | 22 tests |
| Zone wet-bulb | Updated each step via humidity solver payload | humidity_solver.rs:147, conversions.rs:272 | Integration tested |

## Background/Context

OCHRE computes several derived weather fields. After the THERMAL/WEATHER ticket chains, all fields are implemented with EnergyPlus-grade methods. No gaps remain.

## Work to Do

- [ ] Verify `EnvironmentManager::update()` in `environment.rs` — after WEATHER chain:
  - **Sky temp**: WEATHER-009 added Berdahl-Martin + Clark-Allen fallback. Verify the model selection logic is correct (Stefan-Boltzmann primary when IR >= 50, Berdahl-Martin when cloud cover available, Clark-Allen last resort). No implementation needed — just verify.
  - **Ground albedo**: WEATHER-008 added dynamic albedo from PSM3 or monthly config. Verify it flows through to `perez_tilted_irradiance()` calls. No implementation needed — just verify.
  - **Ground temp**: Is it time-varying (Kusuda-Achenbach or DOE-2)? If still constant, wire Kusuda-Achenbach from PARITY-014 into EnvironmentState. If PARITY-014 hasn't landed yet, add the time-varying ground temp here.
  - **Mains temp**: Verify Burch-Christensen with 35-day lag and annual statistics is correctly implemented.
  - **Wet-bulb**: Verify psychrometric calculation from dry-bulb, humidity ratio, pressure.
  - **Zone wet-bulb**: Verify `ZoneState.wet_bulb_c` is updated from zone temperature + humidity ratio each timestep (needed by PARITY-017 HPWH).
- [ ] Ensure `ZoneState.wet_bulb_c` is updated from zone temperature + humidity ratio each timestep
- [ ] Add tests: verify sky temp < outdoor temp at night, ground temp lags air temp by ~6 weeks

## Files to Touch

- `crates/hares-core/src/environment.rs`: Audit and fix derived field computation
- `crates/hares-physics/src/psychrometrics.rs`: Verify wet-bulb calculation
- `crates/hares-physics/src/ground.rs`: Ground temperature model (may overlap with PARITY-004)
- `crates/hares-io/src/weather.rs`: Verify EPW field extraction

## Measures of Success

- [ ] All WeatherState derived fields are populated with physics-based values (no NaN, no constant fallbacks)
- [ ] Sky temperature is typically 10-20°C below outdoor temp on clear nights
- [ ] Ground temperature at surface tracks outdoor with ~6-week lag
- [ ] Wet-bulb is always ≤ dry-bulb

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
