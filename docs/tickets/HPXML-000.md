---
id: HPXML-000
title: Align config key semantics with HPXML element names
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-io/src/hpxml/equipment.rs
  - crates/hares-equipment/src/hvac/heat_pump/heater.rs
  - crates/hares-equipment/src/hvac/air_conditioner.rs
  - crates/hares-equipment/src/water_heater/heat_pump_wh.rs
  - crates/hares-equipment/src/pv.rs
  - crates/hares-equipment/src/battery.rs
references:
  - "HPXML spec element names"
  - "vendors/OCHRE/ — establishes snake_case convention"
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace
---

## Background/Context

HARES config keys should be **snake_case conversions of HPXML element names with SI unit
suffixes** — consistent with our Rust codebase and the snake_case convention inherited
from OCHRE. Most keys already follow this pattern naturally. This ticket fixes the handful
of keys where we invented a different name for the same HPXML concept, and removes
OCHRE-legacy human-readable string aliases.

**Convention:**
- HPXML-sourced params → `snake_case_of_hpxml_name_si_unit` (e.g., `compressor_lockout_temp_c`)
- HARES-only params (no HPXML equivalent) → same convention (e.g., `cell_resistance_ohm`)
- Values always SI. IO layer converts imperial → SI at parse time.
- No OCHRE-legacy aliases like `"Heat Pump Lockout Temperature (C)"`

## Semantic Mismatches to Fix

These keys use a **different concept name** than the HPXML element they represent:

### HPXML Parser (equipment.rs) — rename inserted keys

| Currently inserts | HPXML element | Rename to |
|-------------------|---------------|-----------|
| `"hp_lockout_temp_c"` | `CompressorLockoutTemperature` | `"compressor_lockout_temp_c"` |
| `"er_lockout_temp_c"` | `BackupHeatingLockoutTemperature` | `"backup_heating_lockout_temp_c"` |
| `"backup_capacity_w"` | `BackupHeatingCapacity` | `"backup_heating_capacity_w"` |
| `"backup_fuel"` | `BackupSystemFuel` | `"backup_system_fuel"` |
| `"system_capacity_kw"` (PV) | `MaxPowerOutput` | `"max_power_output_w"` (convert to W) |
| `"number_of_occupants"` | `NumberofResidents` | `"number_of_residents"` |

### Equipment Models — accept renamed keys

| File | Old key | New key |
|------|---------|---------|
| `heat_pump/heater.rs` | `"hp_lockout_temp_c"` | `"compressor_lockout_temp_c"` |
| `heat_pump/heater.rs` | `"er_lockout_temp_c"` | `"backup_heating_lockout_temp_c"` |
| `heat_pump/heater.rs` | `"Backup Capacity (W)"` | `"backup_heating_capacity_w"` |
| `heat_pump/heater.rs` | `"Backup EIR (-)"` | remove (keep `"backup_eir"`) |
| `heat_pump/heater.rs` | `"Backup Setpoint Offset (C)"` | remove (keep `"er_setpoint_offset_c"`) |
| `heat_pump/heater.rs` | `"Heat Pump Lockout Temperature (C)"` | remove OCHRE legacy |
| `heat_pump/heater.rs` | `"Backup Lockout Temperature (C)"` | remove OCHRE legacy |
| `air_conditioner.rs` | `"crankcase_heater_kw"` | `"crankcase_heater_w"` (accept W, not kW) |
| `heat_pump_wh.rs` | `"hp_only_mode"` | `"hpwh_operating_mode"` (string enum) |
| `heat_pump_wh.rs` | `"backup_element_power_w"` / `"BackupElementPower"` | `"backup_heating_capacity_w"` |
| `pv.rs` | `"SystemSize"` | `"max_power_output_w"` |
| `pv.rs` | `"capacity_kw"` | `"max_power_output_w"` (fallback) |

### Keys that are FINE as-is (snake_case of HPXML name, correct semantics)

These do NOT need renaming — they are already correct snake_case conversions:

- `"tank_volume_gal"` ← `TankVolume` ✓ (would be `_l` if we convert, but gal is fine if tank.rs expects gal)
- `"energy_factor"` ← `EnergyFactor` ✓
- `"uniform_energy_factor"` ← `UniformEnergyFactor` ✓
- `"setpoint_c"` ← `HotWaterTemperature` (different name but established convention — leave)
- `"shr"` ← `SensibleHeatFraction` (standard HVAC abbreviation — leave)
- `"heating_capacity_kbtu_h"` ← `HeatingCapacity` ✓
- `"heat_pump_type"` ← `HeatPumpType` ✓
- `"fraction_load_served"` ← `FractionHeatLoadServed` ✓ (simplified)
- `"fan_power_w"` ← `FanPowerWatts` ✓
- `"fan_power_w_per_cfm"` ← `FanPowerWattsPerCFM` ✓
- `"tilt_deg"` ← `ArrayTilt` ✓
- `"azimuth_deg"` ← `ArrayAzimuth` ✓
- `"inverter_efficiency"` ← `InverterEfficiency` ✓
- `"system_losses_fraction"` ← `SystemLossesFraction` ✓
- `"ventilation_rate_cfm"` ← `RatedFlowRate` (units differ but concept clear — leave)
- All EV keys (`"ChargingLevel"`, `"MaxChargingPower"`, `"BatteryCapacity"`) — already PascalCase from HPXML, acceptable to snake_case them but low priority

## Work to Do

- [ ] Rename the 6 parser-side keys in `equipment.rs`
- [ ] Update the ~8 equipment model keys listed above
- [ ] Remove all OCHRE-legacy string keys (`"Heat Pump Lockout Temperature (C)"` etc.)
- [ ] Update any tests that reference old key names
- [ ] Grep for old key names across workspace to catch any stragglers

## Measures of Success

- [ ] No OCHRE-legacy human-readable string keys remain
- [ ] Semantic mismatches between config keys and HPXML concepts are resolved
- [ ] All snake_case keys are recognizable as their HPXML equivalent
- [ ] All existing tests pass

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace` passes
