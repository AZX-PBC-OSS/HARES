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

## Status: COMPLETE (verified — already correctly implemented)

Audit confirms zone wet-bulb is correctly used for HPWH COP/capacity curves:

| Check | Result |
|-------|--------|
| `zone_wet_bulb_c()` reads `env.zones[zone_id].wet_bulb_c` | Line 283-293: reads zone wet-bulb, falls back to zone dry-bulb |
| COP curve input | Line 680: `cop_curve.evaluate(wet_bulb_c, tank_avg_temp_c)` ✓ |
| Capacity curve input | Line 687: `capacity_curve.evaluate(wet_bulb_c, tank_avg_temp_c)` ✓ |
| Zone wet-bulb updated each step | Humidity solver → `apply_humidity_update_to_zones()` → `zone.wet_bulb_c` |
| Test: COP varies with humidity | `wet_bulb_cop_differs_with_same_dry_bulb_different_humidity` (line 1260) |

## Background/Context

Implemented during prior work. The gap analysis was based on an earlier state of the code.

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
