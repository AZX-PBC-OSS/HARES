---
id: HARES-031
title: "hares-equipment — Generator"
kind: implement
depends_on: [HARES-018]
files_to_touch:
  - crates/hares-equipment/src/generator.rs
references:
  - docs/architecture/02-equipment-and-ports.md
  - docs/architecture/03-control-interfaces.md
verification:
  - cargo check -p hares-equipment
  - cargo test -p hares-equipment
  - cargo clippy -p hares-equipment -- -D warnings
---

## Background/Context
Gas generators and fuel cells cover backup generation and combined heat and power (CHP). OCHRE's generator model omits the CHP thermal port; this implementation adds it as a configurable option, enabling district heating and DHW preheat use cases.

## Work to Do
- [ ] Add HPXML data interface: parse `Generator` elements for `SystemCapacityElectric` (kW), `AnnualConsumptionkBtu`, `AnnualOutputkWh`, and fuel type; derive `eta_electric` from the annual figures; populate `EquipmentConfig` during HPXML loading
- [ ] Implement `GasGenerator` struct implementing `Equipment`; efficiency from config (constant scalar or polynomial curve indexed by load fraction); register under the exact OCHRE registry name `"Gas Generator"`
- [ ] Implement `FuelCell` struct implementing `Equipment`; same efficiency interface but separate registry key; register under the exact OCHRE registry name `"Gas Fuel Cell"`
- [ ] Implement ramp-rate limiting: `delta_kw_per_s` from config; clamp power change each timestep to this rate
- [ ] Implement self-consumption controller: reads accumulated Stage 1 electrical power from `PortSlots.electrical` (not `EnvironmentState`) to determine net load; target power = `max(0, load_kw - grid_import_limit_kw)`; respect export limit. Generator runs in `ExecutionStage::Electrical` (Stage 2) after Stage 1 accumulation. NOTE: this requires the engine to snapshot Stage 1 totals into a read-only buffer before Stage 2 begins. See HARES-043 for the stage snapshot mechanism.
- [ ] Implement CHP thermal port (OCHRE fix): `efficiency_thermal` parameter (default 0.0); when > 0.0, compute `Q_thermal = P_fuel * eta_thermal` and write to Thermal port; optionally write to Fluid port for DHW preheat when a fluid port is declared
- [ ] CHP Fluid port: `loop_id` must reference a valid water heater loop; verify at `init` and return `Err` if no matching loop is found
- [ ] Fuel consumption: `P_fuel = P_electric / eta_electric`
- [ ] Define `efficiency_flue = 1.0 - eta_electric - eta_thermal`. Validate at init that `eta_electric + eta_thermal <= 1.0`; return `Err` if violated. Flue loss is computed as a residual: `flue_loss_w = fuel_input_w - electrical_output_w - thermal_output_w`. This is algebraically equivalent to `fuel_input_w * efficiency_flue` but makes the residual nature explicit. Note: the energy balance `electrical_output + thermal_output + flue_loss == fuel_input` is tautological by construction from this residual — it will always hold. The meaningful tests are that each individual component matches its configured efficiency: `electrical_output_w == fuel_input_w * eta_electric` and `thermal_output_w == fuel_input_w * eta_thermal`.
- [ ] Write Electrical port (negative = generation), Fuel port, and optional Thermal/Fluid ports
- [ ] Declare `control_capabilities` for both types: `POWER_SETPOINT | MODE_OVERRIDE`
- [ ] Declare `telemetry_fields`:
  - Both types: `electric_output_kw`, `fuel_input_w`, `eta_electric`, `ramp_limited`
  - When CHP active: `thermal_output_w`, `flue_loss_w`
- [ ] Implement `save_state() -> Vec<u8>`: serialize current power output (for ramp-rate continuity across checkpoints) and CHP accumulator state
- [ ] Implement `load_state(&mut self, state: &[u8]) -> Result<()>`
- [ ] Assign `ExecutionStage::Electrical` (Stage 2) in `EquipmentDescriptor`
- [ ] Register both types in `EquipmentRegistry`

## Files to Touch
- `crates/hares-equipment/src/generator.rs`: new file — `GasGenerator`, `FuelCell`, ramp-rate limiting, self-consumption controller, CHP thermal

## Measures of Success
- [ ] Fuel consumption at rated output matches `P_rated / eta_electric` analytically
- [ ] Self-consumption controller tracks net load (from Stage 1 accumulated PortSlots) and stays within import/export limits
- [ ] Ramp rate is enforced: power cannot change by more than `delta_kw * dt` in a single step
- [ ] CHP component efficiency tests (the energy balance is tautological by construction): when `efficiency_thermal = 0.35`, Thermal port write equals `P_fuel * 0.35` to floating-point tolerance; when `eta_electric = 0.40`, Electrical port write equals `P_fuel * 0.40` to floating-point tolerance; Thermal port is absent when `efficiency_thermal = 0.0`
- [ ] CHP Fluid port: `init` returns `Err` when no water heater loop matching `loop_id` exists
- [ ] `save_state` / `load_state` round-trip preserves current power and ramp state

## Verification
- [ ] `cargo check -p hares-equipment` passes
- [ ] `cargo test -p hares-equipment` passes
- [ ] `cargo clippy -p hares-equipment -- -D warnings` passes
