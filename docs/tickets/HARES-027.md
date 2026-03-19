---
id: HARES-027
title: "hares-equipment — Water Heaters (Resistance, Gas, HPWH, Tankless)"
kind: implement
depends_on: [HARES-026, HARES-020, HARES-006]
files_to_touch:
  - crates/hares-equipment/src/water_heater/resistance.rs
  - crates/hares-equipment/src/water_heater/gas.rs
  - crates/hares-equipment/src/water_heater/heat_pump_wh.rs
  - crates/hares-equipment/src/water_heater/tankless.rs
references:
  - docs/architecture/02-equipment-and-ports.md
  - docs/architecture/03-control-interfaces.md
verification:
  - cargo check -p hares-equipment
  - cargo test -p hares-equipment
  - cargo clippy -p hares-equipment -- -D warnings
---

## Background/Context
With the stratified tank model in place (HARES-026), the four water heater equipment types are primarily control and efficiency wrappers around it. Implementing them together keeps the water heater module complete and avoids partial states that would require later integration work.

## Work to Do
- [ ] Add HPXML data interface: parse `WaterHeatingSystem` elements for `TankVolume` (gal), `HeatingCapacity` (Btu/hr), `FirstHourRating` (gal), `EnergyFactor` or `UniformEnergyFactor`, `FuelType`, and thermostat setpoint temperature (°C). Populate `EquipmentConfig` fields from parsed values during HPXML loading.
- [ ] `ResistanceWH`: thermostat sensing uses the LOWER-element node temperature (matching OCHRE's `t_lower_idx`). Upper element has priority: with a cold tank, the upper element activates first for faster top-of-tank recovery; once the upper thermostat is satisfied, the lower element activates. Deadband control using `ThermostatConfig`; writes Electrical + Fluid ports
- [ ] `GasWH`: burner efficiency curve (constant or polynomial from config); flue loss fraction; pilot light modeled as a continuous Fuel port contribution (not an electrical draw) — pilot consumption is added to the Fuel port every timestep regardless of burner state; writes Fuel + optional Electrical (fan) + Fluid ports. **Flue losses are not written to the zone Thermal port** — they represent heat leaving the building envelope and must not be included in zone heat gains.
- [ ] `HeatPumpWH`: COP from biquadratic curves `f(zone_temp, tank_temp)` loaded from config; compressor + backup resistance element sharing same thermostat deadband; condenser heat deposited at configured tank node; zone cooling effect: `Q_zone_extracted = Q_tank_delivered - P_compressor` (NOT simply `-P_compressor`); write negative sensible gain to Thermal port equal to `Q_zone_extracted`. Energy balance invariant: `Q_zone_extracted + P_compressor = Q_tank_delivered`. Zone assignment uses `EquipmentDescriptor.zone` (set during HPXML parsing, not hardcoded); writes Electrical + Thermal + Fluid ports
- [ ] `TanklessWH`: no tank model; instantaneous `P_thermal = flow_rate_kg_s * Cp_water * (T_setpoint - T_inlet)`; fuel input accounts for efficiency factor: `fuel_input = P_thermal / EF` where EF is loaded from HPXML `EnergyFactor` or `UniformEnergyFactor`; writes Fuel or Electrical port depending on fuel type
- [ ] Declare `control_capabilities` for all four types: `THERMAL_SETPOINT | DUTY_CYCLE | MODE_OVERRIDE`
- [ ] Declare `telemetry_fields` per type:
  - `ResistanceWH`: `tank_avg_temp_c`, `upper_element_power_w`, `lower_element_power_w`, `electric_power_w`, `draw_flow_rate_kg_s`
  - `GasWH`: `tank_avg_temp_c`, `burner_power_w`, `pilot_power_w`, `gas_consumption_w`, `flue_loss_w`, `fan_electric_w`, `draw_flow_rate_kg_s`
  - `HeatPumpWH`: `tank_avg_temp_c`, `cop`, `compressor_power_w`, `backup_element_power_w`, `zone_heat_extraction_w`, `draw_flow_rate_kg_s`
  - `TanklessWH`: `outlet_temp_c`, `thermal_output_w`, `fuel_input_w`, `draw_flow_rate_kg_s`
- [ ] Implement `save_state() -> Vec<u8>` for each type: serialize thermostat state and owned `StratifiedTank` state (via `StratifiedTank::save_state`)
- [ ] Implement `load_state(&mut self, state: &[u8]) -> Result<()>` for each type
- [ ] Register all four types in `EquipmentRegistry`

## Files to Touch
- `crates/hares-equipment/src/water_heater/resistance.rs`: new file — `ResistanceWH`
- `crates/hares-equipment/src/water_heater/gas.rs`: new file — `GasWH`
- `crates/hares-equipment/src/water_heater/heat_pump_wh.rs`: new file — `HeatPumpWH`
- `crates/hares-equipment/src/water_heater/tankless.rs`: new file — `TanklessWH`

## Measures of Success
- [ ] `ResistanceWH` over a 24-hour draw profile: tank node temperatures and total energy use match hand-calculated reference
- [ ] `GasWH` gas consumption versus fan electric split matches efficiency and flue loss fractions from config
- [ ] `GasWH` flue loss test: zone Thermal port sensible gain does NOT include flue loss fraction; only burner heat delivered to tank propagates to zone via tank standby losses
- [ ] `HeatPumpWH` COP increases with zone temperature and decreases as tank temperature rises; zone Thermal port shows negative sensible gain when compressor runs
- [ ] `HeatPumpWH` energy balance: `Q_zone_extracted + P_compressor = Q_tank_delivered` holds to floating-point tolerance at every step
- [ ] With a cold tank, the upper `ResistanceWH` element activates first for top-of-tank recovery; the lower element activates only after the upper thermostat is satisfied
- [ ] `TanklessWH` thermal output equals `flow_rate * Cp * (T_setpoint - T_inlet)` exactly; fuel input equals `thermal_output / EF`
- [ ] `save_state` / `load_state` round-trip preserves thermostat and tank state

## Verification
- [ ] `cargo check -p hares-equipment` passes
- [ ] `cargo test -p hares-equipment` passes
- [ ] `cargo clippy -p hares-equipment -- -D warnings` passes
