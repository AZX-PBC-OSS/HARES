---
id: HARES-024
title: "hares-equipment — Heat Pump (ASHP + MSHP)"
kind: implement
depends_on: [HARES-022, HARES-023]
files_to_touch:
  - crates/hares-equipment/src/hvac/heat_pump.rs
references:
  - docs/architecture/02-equipment-and-ports.md
verification:
  - cargo check -p hares-equipment
  - cargo test -p hares-equipment
  - cargo clippy -p hares-equipment -- -D warnings
---

## Background/Context
Heat pumps are the most complex HVAC equipment: they combine cooling performance (from HARES-023) with a defrost model, backup resistance heating with multi-mode lockout logic, and minisplit-specific speed mapping. Implementing them in a single file keeps heating and cooling mode state in one place.

## Sub-scope Breakdown

This ticket contains two independent sub-scopes. Heater variants can be implemented and reviewed before HARES-023 completes, since they depend only on HARES-022.

**Sub-scope A — Heater variants** (depends on HARES-022 only, does not require HARES-023):
- `ASHPHeater`
- `MinisplitHeater` (referred to in OCHRE as `MSHP Heater`)

**Sub-scope B — Cooler variants** (depends on HARES-023 for SHR/crankcase logic):
- `ASHPCooler`
- `MSHPCooler`

Sub-scope A may be merged independently. Sub-scope B requires HARES-023 to be complete.

## Work to Do

### Shared: DefrostConfig
- [ ] Define `DefrostConfig` struct loaded from `EquipmentConfig` (OCHRE/EnergyPlus defaults when absent):
  - `capacity_reduction_factor: f64` — multiplicative reduction to heating capacity during defrost (OCHRE default: 0.75)
  - `defrost_power_w: f64` — supplemental electric resistance power during defrost (used in resistive-defrost models). For reverse-cycle defrost, `defrost_power_w = 0.0` because compressor power is already counted in the main electrical draw. Do not double-count. (OCHRE default: 0.0)
- [ ] Implement dynamic humidity-based defrost model per OCHRE `HVAC.py` lines 1128-1173 (EnergyPlus methodology). The effective defrost time fraction is NOT a static constant; it is computed each timestep from outdoor humidity ratio and coil temperature. OCHRE applies regression-derived multipliers to capacity and power based on these conditions. Do not use a fixed `time_fraction = 0.058333`; this was a placeholder and does not match OCHRE behavior.
  - Specifically: defrost capacity and power multipliers are functions of `T_outdoor_c` and `W_outdoor` (outdoor humidity ratio). Reference OCHRE `HVAC.py:1128-1173` for the regression coefficients and the exact formula. If the exact coefficients cannot be extracted from OCHRE before implementation begins, document the gap explicitly and use the static fallback only as a temporary stub with a TODO.
- [ ] Apply `DefrostConfig` corrections when outdoor dry-bulb < 4.4°C

### Sub-scope A: Heater Variants
- [ ] Implement `ASHPHeater`: HP-only, HP+ER (emergency/supplemental resistance), and ER-only operating modes; HP lockout temperature (compressor forced off below); ER lockout temperature (resistance off above); ER setpoint offset
- [ ] Add `min_er_cycle_time_s: f64` field to `ASHPHeater` config — minimum elapsed time before ER element may re-engage after shutting off, preventing rapid toggling. Load from `EquipmentConfig`; default to 300 seconds (5 minutes) when absent. OCHRE has no explicit constant for this parameter; 300 s is chosen as a reasonable minimum cycle guard for resistance elements, consistent with residential thermostat practice.
- [ ] Implement `MinisplitHeater`: pan heater power draw; map 10-speed OCHRE speed indices to 4-speed internal representation. Mapping must be loaded from `EquipmentConfig` as `mshp_speed_map: [u8; 4]` (4 entries, each an OCHRE speed index 0–9), or use OCHRE's built-in default mapping. Reference OCHRE source for exact default mapping table before hardcoding any values.
  - The 10-to-4 speed mapping for MSHP must be resolved BEFORE implementation. Check OCHRE `utils/` directory and parameter CSV files for the default mapping table. Include the literal mapping array in this ticket once located. Do not hardcode arbitrary values.

### Sub-scope B: Cooler Variants
- [ ] Implement `ASHPCooler` and `MSHPCooler` cooling variants, reusing `AirConditioner` SHR and crankcase heater logic from HARES-023

### All Variants
- [ ] All variants write Thermal port (heating or cooling sensible + latent) and Electrical port
- [ ] Declare `control_capabilities` in `EquipmentDescriptor`: `ThermalSetpoint` for all heat pump variants
- [ ] Declare `telemetry_fields` in `EquipmentDescriptor`: at minimum `electric_kw`, `operating_mode`, `speed_index`, `defrost_active`, and thermal port fields
- [ ] Assign `ExecutionStage::Thermal` for all variants in `EquipmentDescriptor`
- [ ] Implement `save_state` / `load_state` for all variants — include operating mode, speed index, defrost accumulator, cycle-time counters, and ER lockout timer
- [ ] Register all variants in `EquipmentRegistry` with exact OCHRE name strings:
  - `"Heat Pump Heater"` (base heater, if used)
  - `"ASHP Heater"`
  - `"MSHP Heater"`
  - `"ASHP Cooler"`
  - `"MSHP Cooler"`

## Files to Touch
- `crates/hares-equipment/src/hvac/heat_pump.rs`: new file — `ASHPHeater`, `MinisplitHeater`, `ASHPCooler`, `MSHPCooler`

## Measures of Success

### Defrost
- [ ] At -5°C OAT with a known outdoor humidity ratio: heating capacity and power multipliers match the OCHRE `HVAC.py:1128-1173` dynamic defrost formula at the same conditions (not the static time-fraction formula)
- [ ] No defrost correction applied when OAT >= 4.4°C
- [ ] Defrost multipliers vary with outdoor humidity ratio — a drier outdoor condition produces a different multiplier than a humid condition at the same OAT

### ASHPHeater Mode Transitions
- [ ] HP On → HP+ER when load exceeds HP capacity
- [ ] HP → ER Only when below HP lockout temperature
- [ ] ER element does not re-engage within `min_er_cycle_time_s` after shutting off
- [ ] ER element does not activate above the ER lockout temperature

### MinisplitHeater
- [ ] MSHP 10-to-4 speed mapping: default `mshp_speed_map` matches OCHRE source; override via `EquipmentConfig` is accepted
- [ ] Mapped speed indices are monotonically increasing and within [0, 9]

### Cooler Variants
- [ ] `ASHPCooler` at AHRI rated conditions (95°F outdoor, 80°F/67°F indoor DB/WB) produces capacity and EIR matching `AirConditioner` with equivalent config (same biquadratic coefficients, same rated capacity)
- [ ] `MSHPCooler` crankcase heater and SHR logic match `AirConditioner` behavior at identical operating conditions

### General
- [ ] `save_state` / `load_state` round-trips without loss for all variants
- [ ] All five OCHRE name strings register and resolve correctly via `EquipmentRegistry::create`
- [ ] `ExecutionStage` is `Thermal` for all variants

## Verification
- [ ] `cargo check -p hares-equipment` passes
- [ ] `cargo test -p hares-equipment` passes
- [ ] `cargo clippy -p hares-equipment -- -D warnings` passes
