---
id: HARES-023
title: "hares-equipment — Air Conditioner and Room AC"
kind: implement
depends_on: [HARES-022, HARES-005]
files_to_touch:
  - crates/hares-equipment/src/hvac/air_conditioner.rs
references:
  - docs/architecture/02-equipment-and-ports.md
verification:
  - cargo check -p hares-equipment
  - cargo test -p hares-equipment
  - cargo clippy -p hares-equipment -- -D warnings
---

## Background/Context
`AirConditioner` and `RoomAC` are the primary cooling equipment types and exercise the full biquadratic performance path, SHR latent/sensible split, and the crankcase heater correction. Implementing them after HARES-022 validates the dynamic HVAC foundation end-to-end.

## Work to Do
- [ ] Implement `AirConditioner` struct implementing `Equipment` via `HvacEquipment`
- [ ] Implement `RoomAC` struct implementing `Equipment` via `HvacEquipment`. `RoomAC` behavioral differences from `AirConditioner`:
  - No duct DSE (duct_dse = 1.0 always; RoomAC delivers directly to zone)
  - Always single-speed (multi-speed control modes from HARES-022 are not applicable)
  - Different AHRI rating conditions than central AC — verify exact rating point against OCHRE source before implementing
  - Specify whether SHR model applies: `RoomAC` uses the same coil Ao / SHR calculation as `AirConditioner` unless OCHRE source shows otherwise; document the finding
- [ ] Implement coil Ao factor calculation matching OCHRE `utils_equipment.coil_ao_factor`
- [ ] Implement SHR calculation from coil Ao factor and wet-bulb temperature input, matching OCHRE `utils_equipment.calculate_shr`
- [ ] Read wet-bulb temperature from `env.zones[zone_id].wet_bulb_c`. This field is always populated by the envelope solver before equipment runs. If it reads as 0.0 or NaN, that is an ordering bug upstream — do not silently re-derive wet-bulb; instead assert or return `Err` with a clear message
- [ ] Implement crankcase heater: 50 W when compressor is off and outdoor dry-bulb < 12.8°C (not 55°F — use Celsius only throughout the codebase); add to Electrical port write
- [ ] Compute cooling supply temperature from SHR and capacity
- [ ] Write Thermal port: negative sensible gain (sensible cooling) + negative latent gain (latent cooling dehumidification)
- [ ] Write Electrical port: compressor + fan + crankcase heater power
- [ ] Declare `control_capabilities` in `EquipmentDescriptor`: `ThermalSetpoint`
- [ ] Declare `telemetry_fields` in `EquipmentDescriptor`: at minimum `electric_kw`, `sensible_cooling_w`, `latent_cooling_w`, `shr`, `operating_mode`
- [ ] Assign `ExecutionStage::Thermal` for both types in `EquipmentDescriptor`
- [ ] Implement `save_state` / `load_state` for both types — include thermostat FSM state, crankcase heater state, and cycle-time counters
- [ ] Register both types in `EquipmentRegistry` with exact OCHRE name strings:
  - `"Air Conditioner"`
  - `"Room AC"`

## Files to Touch
- `crates/hares-equipment/src/hvac/air_conditioner.rs`: new file — `AirConditioner`, `RoomAC`, coil Ao factor, SHR calculation

## Measures of Success
- [ ] SHR varies correctly with humidity ratio — higher humidity produces lower SHR
- [ ] Crankcase heater draws 50 W when compressor off and outdoor temp < 12.8°C; zero otherwise
- [ ] At rated conditions (AHRI 95°F outdoor, 80°F/67°F indoor DB/WB), capacity and EIR match expected SEER-derived values
- [ ] Thermal port sensible and latent components sum to total capacity at computed SHR
- [ ] `RoomAC` applies duct DSE of 1.0 regardless of HPXML duct config
- [ ] `RoomAC` cannot be set to multi-speed mode (single-speed only)
- [ ] Wet-bulb read from `env.zones[zone_id].wet_bulb_c` — test that a NaN/zero value produces an error, not a silently wrong SHR
- [ ] `save_state` / `load_state` round-trips without loss for both types
- [ ] Both OCHRE name strings register and resolve correctly via `EquipmentRegistry::create`
- [ ] `ExecutionStage` is `Thermal` for both types

## Verification
- [ ] `cargo check -p hares-equipment` passes
- [ ] `cargo test -p hares-equipment` passes
- [ ] `cargo clippy -p hares-equipment -- -D warnings` passes
