---
id: HPXML-005
title: Parse HVACControl setback/setup temps and heating/cooling seasons
kind: implement
depends_on:
  - HPXML-000
files_to_touch:
  - crates/hares-io/src/hpxml/equipment.rs
  - crates/hares-io/src/hpxml/building.rs
references:
  - "HPXML spec: HVACControl/SetbackTempHeatingSeason, SetupTempCoolingSeason, TotalSetbackHoursperWeekHeating, TotalSetupHoursperWeekCooling"
  - "HPXML spec: HVACControl/HeatingSeason/BeginMonth..EndMonth, CoolingSeason/BeginMonth..EndMonth"
verification:
  - cargo build -p hares-io
  - cargo test -p hares-io
  - cargo clippy -p hares-io
---

## Background/Context

HPXML provides standard thermostat setback/setup fields (`SetbackTempHeatingSeason`, `SetupTempCoolingSeason`) with hours per week. These can synthesize 24-hour setpoint profiles when the OpenStudio-HPXML extension profiles are absent. HPXML also provides heating/cooling season date ranges that could disable equipment outside season.

Currently only the extension `WeekdaySetpointTemps*` profiles are parsed; the standard setback fields are ignored.

## Work to Do

- [ ] In `parse_hvac_setpoint_params`, add fallback logic: if no extension weekday/weekend profiles found, check for `SetpointTempHeatingSeason` + `SetbackTempHeatingSeason` + `TotalSetbackHoursperWeekHeating`
- [ ] Synthesize a 24-hour profile: distribute setback hours evenly across nighttime (e.g., 10pm-6am) with setback temp, remainder at setpoint temp
- [ ] Same for cooling: `SetpointTempCoolingSeason` + `SetupTempCoolingSeason` + `TotalSetupHoursperWeekCooling`
- [ ] Convert all temps from °F to °C
- [ ] Extract `HeatingSeason/BeginMonth`, `EndMonth` (and cooling equivalents) and insert as season bound params
- [ ] Add unit test: setpoint=70°F, setback=65°F, 49hrs/week → reasonable 24h profile

## Files to Touch

- `crates/hares-io/src/hpxml/equipment.rs`: Extend `parse_hvac_setpoint_params` with setback fallback
- `crates/hares-io/src/hpxml/building.rs`: Extract season date ranges if stored at building level

## Measures of Success

- [ ] HPXML with setback fields but no extension profiles still produces 24h setpoint arrays
- [ ] Extension profiles take priority when both are present
- [ ] Season bounds are available as params (e.g., `heating_season_begin_month: 10`, `heating_season_end_month: 5`)

## Verification

- [ ] `cargo build -p hares-io` passes
- [ ] `cargo test -p hares-io` passes
- [ ] `cargo clippy -p hares-io` passes
