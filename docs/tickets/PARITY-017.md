---
id: PARITY-017
title: "HPWH wet-bulb COP input"
kind: implement
depends_on: [PARITY-005]
files_to_touch:
  - crates/hares-equipment/src/water_heater/heat_pump_wh.rs
references:
  - docs/equipment/ochre-parity-gaps.md (Gap 9)
  - docs/equipment/water-heater.md (HPWH section)
  - vendors/OCHRE/ochre/models/WaterHeater.py (lines 461-474)
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace -- -D warnings
---

## Background/Context

HPWH COP/capacity biquadratic curves should be evaluated at (wet-bulb, tank_temp). The exploration agent found that `heat_pump_wh.rs:664-668` already uses `wet_bulb_c` as input to the COP curve. However, the gap analysis flagged that the wet-bulb value may come from dry-bulb ambient rather than true zone wet-bulb.

**Target**: Verify that zone wet-bulb temperature (computed from zone temperature + humidity ratio) is actually used as the COP curve input, not dry-bulb. If it's already correct, close as verified. If not, wire the correct value.

Depends on PARITY-011 (weather derived fields audit) to ensure wet-bulb is reliably computed.

## Work to Do

- [ ] Trace the `wet_bulb_c` value in `HeatPumpWaterHeater::step()`:
  - Where does it come from? `env.zones[zone_idx].wet_bulb_c` or `env.weather.outdoor_wet_bulb_c`?
  - If it's zone wet-bulb: verify zone wet-bulb is computed from zone temp + humidity ratio (not outdoor)
  - If it's outdoor wet-bulb or dry-bulb: fix to use zone wet-bulb
- [ ] Ensure `ZoneState.wet_bulb_c` is updated each timestep from zone temperature and humidity ratio
- [ ] If zone humidity is not tracked (humidity solver disabled): fall back to outdoor wet-bulb with warning
- [ ] Add test: COP at (20°C WB, 50°C tank) vs (30°C WB, 50°C tank) — humid conditions should give higher COP

## Files to Touch

- `crates/hares-equipment/src/water_heater/heat_pump_wh.rs`: Verify/fix wet-bulb input source

## Measures of Success

- [ ] HPWH COP varies with zone humidity (higher humidity → higher wet-bulb → different COP)
- [ ] COP matches OCHRE within 2% for same input conditions

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
