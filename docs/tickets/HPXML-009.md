---
id: HPXML-009
title: Parse HVAC fan motor type and crankcase heater from HPXML extensions
kind: implement
depends_on:
  - HPXML-000
files_to_touch:
  - crates/hares-io/src/hpxml/equipment.rs
references:
  - "HPXML spec: extension/FanMotorType ('PSC', 'BPM')"
  - "HPXML spec: extension/CrankcaseHeaterPowerWatts"
verification:
  - cargo build -p hares-io
  - cargo test -p hares-io
  - cargo clippy -p hares-io
---

## Background/Context

Crankcase heaters draw power when the compressor is off in cold weather; `extension/CrankcaseHeaterPowerWatts` captures this parasitic load.

**Wiring status — crankcase heater:** ✅ `crankcase_heater_kw` and `crankcase_heater_threshold_c` are already consumed in `air_conditioner.rs`. HPXML value is in watts, equipment expects kW — convert W→kW during extraction.

**Wiring status — fan motor type:** ❌ `fan_motor_type` is NOT consumed by any equipment model. Forward as passthrough string; applying W/CFM lookup by motor type requires equipment-side changes (backlogged, not this ticket).

## Work to Do

- [ ] In CoolingSystem and HeatPump extension blocks, extract `CrankcaseHeaterPowerWatts` and insert as `"crankcase_heater_w"` (value in watts; equipment accepts watts after HPXML-000)
- [ ] Extract `FanMotorType` and insert as `"fan_motor_type"` string param ("PSC" or "BPM")
- [ ] Add unit test

## Files to Touch

- `crates/hares-io/src/hpxml/equipment.rs`: Extend HVAC extension parsing

## Measures of Success

- [ ] HeatPump with `<extension><CrankcaseHeaterPowerWatts>50</CrankcaseHeaterPowerWatts></extension>` → `crankcase_heater_w: 50.0`
- [ ] CoolingSystem with `<extension><FanMotorType>BPM</FanMotorType></extension>` → `fan_motor_type: "BPM"`
- [ ] Missing extensions don't affect existing behavior

## Verification

- [ ] `cargo build -p hares-io` passes
- [ ] `cargo test -p hares-io` passes
- [ ] `cargo clippy -p hares-io` passes
