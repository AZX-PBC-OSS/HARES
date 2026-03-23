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

THERMAL-001 (dry air density for infiltration) and THERMAL-002 (4-component exterior LWR with sky temp) will have completed. WEATHER-001 (sub-hourly interpolation) and WEATHER-003 (PSM3 parser) may also be done, improving weather data quality. This ticket is primarily an **audit and gap-fill** — verifying that all derived fields now use EnergyPlus-grade computation after the THERMAL/WEATHER work.

## Background/Context

OCHRE computes several derived weather fields. After THERMAL-001/002, sky temperature and infiltration mass flow should be improved. This ticket audits all remaining derived fields: ground temperature (DOE-2 sinusoidal), mains water temperature (Burch-Christensen), humidity ratio, and wet-bulb — verifying they are correctly populated.

**Target**: Verify all derived fields use EnergyPlus-grade methods; fix any remaining gaps.

## Work to Do

- [ ] Audit `EnvironmentManager::update()` in `environment.rs`:
  - Is `sky_temp_c` computed from horizontal infrared radiation per EnergyPlus formula, or just set to outdoor temp?
  - Is `ground_temp_c` time-varying (Kusuda-Achenbach or DOE-2 sinusoidal), or constant?
  - Is `mains_temp_c` computed from Burch-Christensen with annual lag, or simplified?
  - Is `outdoor_wet_bulb_c` computed from psychrometric functions, or from EPW directly?
- [ ] For each field that is simplified, implement EnergyPlus-grade computation:
  - **Sky temp** (EnergyPlus Clark & Allen 1978):
    - If horizontal IR available from EPW: `T_sky = (IRH / sigma)^0.25 - 273.15`
    - If not: compute sky emissivity: `eps_clear = 0.787 + 0.764 * ln(T_dp_K / 273)`
    - Cloud correction (Walton 1983): `eps = eps_clear * (1 + 0.0224*N - 0.0035*N^2 + 0.00028*N^3)` where N = opaque sky cover tenths
    - Then: `IRH = eps * sigma * T_db_K^4`, `T_sky = (IRH / sigma)^0.25 - 273.15`
  - **Ground temp**: Kusuda-Achenbach (see PARITY-004 for formula; this ticket just wires the time-varying value into EnvironmentState)
  - **Mains temp**: Burch-Christensen with 35-day lag and annual statistics (verify existing implementation)
  - **Wet-bulb**: psychrometric calculation from dry-bulb, humidity ratio, pressure (verify existing)
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
