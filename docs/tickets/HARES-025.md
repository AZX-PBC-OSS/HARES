---
id: HARES-025
title: "hares-equipment — Dehumidifier"
kind: implement
depends_on: [HARES-004, HARES-018, HARES-006]
files_to_touch:
  - crates/hares-equipment/src/hvac/dehumidifier.rs
references:
  - docs/architecture/02-equipment-and-ports.md
  - docs/architecture/03-control-interfaces.md
  - docs/architecture/appendix-physics-improvements.md
verification:
  - cargo check -p hares-equipment
  - cargo test -p hares-equipment
  - cargo clippy -p hares-equipment -- -D warnings
---

## Background/Context
OCHRE has no DX dehumidifier model. HARES adds one based on the EnergyPlus `ZoneHVAC:Dehumidifier:DX` formulation, which is important for humid climates where latent load dominates and a dedicated dehumidifier is more efficient than running the AC at a depressed setpoint.

## Work to Do
- [ ] Define `Dehumidifier` struct implementing `Equipment`
- [ ] Implement water removal curve: `WaterRemoval(T, RH) = Rated_L_day * f_wr(T_db, RH)` using biquadratic coefficients loaded from config
- [ ] Implement energy factor curve: `EnergyFactor(T, RH) = Rated_IEF * f_ef(T_db, RH)` using biquadratic coefficients loaded from config
- [ ] Compute `ElectricPower = WaterRemoval_kg_s / EnergyFactor` (unit-consistent form)
- [ ] Compute `LatentRemoval = WaterRemoval_kg_s * 2_454_000` (latent heat of vaporisation, J/kg)
- [ ] Compute `SensibleGain = LatentRemoval + ElectricPower` (energy balance: heat deposited equals latent heat extracted plus electrical input)
- [ ] Implement RH deadband: ON when `RH > max_rh`, OFF when `RH < min_rh`; hold current state within deadband. Default deadband maps to `min_rh = target_rh - 2.5` and `max_rh = target_rh + 2.5`
- [ ] Write Thermal port: negative latent gain (moisture removed), positive sensible gain
- [ ] Write Electrical port: compressor + fan electric power
- [ ] Declare `control_capabilities`: `HUMIDITY_SETPOINT | MODE_OVERRIDE`
  - Uses `ControlSignal::HumiditySetpoint { target_rh: f64, min_rh: Option<f64>, max_rh: Option<f64> }` (defined in HARES-004) with `HUMIDITY_SETPOINT` capability flag. The dehumidifier does NOT respond to thermal setpoints.
- [ ] Declare `telemetry_fields`: `water_removal_l_day`, `electric_power_w`, `latent_removal_w`, `sensible_gain_w`, `target_rh`, `min_rh`, `max_rh`, `is_on`
- [ ] Implement `save_state() -> Vec<u8>`: serialize running state (on/off, accumulated water removal) for RL checkpointing
- [ ] Implement `load_state(&mut self, state: &[u8]) -> Result<()>`: deserialize and restore state
- [ ] Assign `ExecutionStage::Thermal` (Stage 3) in `EquipmentDescriptor`
- [ ] Add HPXML data interface: parse `Capacity` (pints/day), `IntegratedEnergyFactor` or `EnergyFactor`, `DehumidistatSetpoint`, `FractionDehumidificationLoadServed`
- [ ] Register in `EquipmentRegistry`

**EF vs IEF note**: v1 uses EF-era 80°F (26.7°C) biquadratic coefficients as the rated-condition anchor. If the HPXML element supplies `IntegratedEnergyFactor` (IEF, the DOE 2019+ test condition), log a warning that IEF values are being used with EF-era curves; the resulting water removal and efficiency predictions may be up to ~10% off at part-load conditions. A future ticket will add IEF-native coefficients.

## Files to Touch
- `crates/hares-equipment/src/hvac/dehumidifier.rs`: new file — `Dehumidifier` struct and full `Equipment` implementation

## Measures of Success
- [ ] Equipment turns on above `max_rh` and turns off below `min_rh`; stays in current state within deadband (default: `min_rh = target_rh - 2.5`, `max_rh = target_rh + 2.5`)
- [ ] Energy balance holds: `sensible_gain == latent_removal + electric_power` to floating-point tolerance
- [ ] Biquadratic curves evaluated at rated conditions (26.7°C, 60% RH) reproduce rated water removal and energy factor
- [ ] Zero output when equipment is off
- [ ] `save_state` / `load_state` round-trip preserves on/off state and produces identical subsequent output

## Verification
- [ ] `cargo check -p hares-equipment` passes
- [ ] `cargo test -p hares-equipment` passes
- [ ] `cargo clippy -p hares-equipment -- -D warnings` passes
