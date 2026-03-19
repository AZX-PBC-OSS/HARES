---
id: HARES-019
title: "hares-equipment — Scheduled Load"
kind: implement
depends_on: [HARES-018]
files_to_touch:
  - crates/hares-equipment/src/scheduled_load.rs
references:
  - docs/architecture/02-equipment-and-ports.md
verification:
  - cargo check -p hares-equipment
  - cargo test -p hares-equipment
  - cargo clippy -p hares-equipment -- -D warnings
---

## Background/Context
`ScheduledLoad` is the first concrete `Equipment` implementation and serves as the reference pattern for all subsequent equipment. It covers the common case of a load whose power draw is driven by an external schedule, with thermal gains to the zone and voltage-dependent adjustment via the ZIP model.

## Work to Do
- [ ] Implement `ScheduledLoad` struct implementing the `Equipment` trait
- [ ] Define `ScheduleRef` as a pre-indexed array of `f64` values loaded at `init` from `EquipmentConfig`. During `step()`, read the current value using the index derived from `env.current_time` (e.g. `floor((current_time - start_time) / time_res)` as `usize`). Schedule ownership lives on the equipment instance — no shared schedule state.
- [ ] Treat schedule value <= 0 as off (zero port writes)
- [ ] Apply `sensible_gain_fraction` and `latent_gain_fraction` fields from config to derive zone thermal port writes
- [ ] Write Electrical port with active power equal to ZIP-adjusted draw. Voltage `V` is read from `env.grid.voltage_pu` (not hardcoded to 1.0)
- [ ] Implement ZIP voltage-dependency model: `P_adj = P * (Z*V^2 + I*V + P_coeff)` where `Z + I + P_coeff = 1.0`; default to constant-power (`P_coeff = 1.0`) when coefficients are absent from config
- [ ] Validate at `init` that ZIP coefficients sum to 1.0
- [ ] Declare `control_capabilities` in `EquipmentDescriptor`: `ScheduledLoad` accepts no external control signals (empty capability flags)
- [ ] Declare `telemetry_fields` in `EquipmentDescriptor`: at minimum `electric_kw`, `sensible_gain_w`, `latent_gain_w`, `gas_consumption_w`
- [ ] Support optional gas schedule column (units: therms/hour or W equivalent). When non-zero, write `PortContribution::Fuel { fuel_type: FuelType::Gas, consumption_w }` to the fuel port. Add `gas_consumption_w` to telemetry_fields.
- [ ] Assign `ExecutionStage::Independent` in `EquipmentDescriptor`
- [ ] Implement `save_state` / `load_state`: serialize current schedule index and any stateful fields (e.g. last non-zero power)
- [ ] Register in `EquipmentRegistry` with the following 21 OCHRE equipment name strings (verified against OCHRE source and implementation): `"Lighting"`, `"Plug Loads"`, `"Other"`, `"Refrigerator"`, `"Freezer"`, `"MELs"`, `"TV"`, `"Well Pump"`, `"Pool Pump"`, `"Pool Heater"`, `"Spa Pump"`, `"Spa Heater"`, `"Gas Grill"`, `"Gas Fireplace"`, `"Gas Lighting"`, `"Ceiling Fan"`, `"Ventilation Fan"`, `"Indoor Lighting"`, `"Exterior Lighting"`, `"Basement Lighting"`, `"Garage Lighting"`. Note: `"ScheduledEV"` and `"PV"` are also `ScheduledLoad` subclasses in OCHRE but are handled by dedicated equipment types in HARES (HARES-029 and HARES-030 respectively).

**Units note**: v1 uses plain `f64` with documented units (kW for power, W for gains, per-unit for voltage). Physical-quantity types (`uom` crate) are deferred to HARES-009, which is the gate for `uom` adoption across the codebase.

## Files to Touch
- `crates/hares-equipment/src/scheduled_load.rs`: new file — `ScheduledLoad` struct and full `Equipment` implementation

## Measures of Success
- [ ] Known schedule value produces electric_kw matching input power through ZIP model
- [ ] `sensible_gain_fraction = 0.5`, `latent_gain_fraction = 0.2` produce correct thermal port values
- [ ] ZIP model at V=0.95 with known Z/I/P coefficients matches analytical result
- [ ] ZIP voltage `V` is sourced from `env.grid.voltage_pu` — test with `voltage_pu = 0.95` and verify port write differs from `voltage_pu = 1.0`
- [ ] Equipment is off (zero port writes) when schedule value is zero or negative
- [ ] `init` returns `Err` when ZIP coefficients do not sum to 1.0
- [ ] `save_state` / `load_state` round-trips schedule index and stateful fields without loss
- [ ] `apply_control` called with any signal returns `Err` (no capability declared)
- [ ] `ExecutionStage` is `Independent`
- [ ] A `ScheduledLoad` with a gas schedule column writes both Electrical and Fuel port contributions
- [ ] When `current_time` exceeds the schedule length, `step()` returns `Err` (not a panic or silent clamp)

## Verification
- [ ] `cargo check -p hares-equipment` passes
- [ ] `cargo test -p hares-equipment` passes
- [ ] `cargo clippy -p hares-equipment -- -D warnings` passes
