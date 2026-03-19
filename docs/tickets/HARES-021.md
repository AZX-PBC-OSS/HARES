---
id: HARES-021
title: "hares-equipment — Furnace, Baseboard, Boiler"
kind: implement
depends_on: [HARES-020, HARES-017, HARES-006]
files_to_touch:
  - crates/hares-equipment/src/hvac/furnace.rs
  - crates/hares-equipment/src/hvac/baseboard.rs
  - crates/hares-equipment/src/hvac/boiler.rs
  - crates/hares-equipment/src/hvac/common.rs
  - crates/hares-equipment/src/hvac/mod.rs
references:
  - docs/architecture/02-equipment-and-ports.md
verification:
  - cargo check -p hares-equipment
  - cargo test -p hares-equipment
  - cargo clippy -p hares-equipment -- -D warnings
---

## Background/Context
Furnaces, baseboards, and boilers are the simplest resistive and combustion heating equipment types. Implementing them after the HVAC common base keeps each implementation small and validates that `HvacEquipment` is sufficient for non-dynamic heating plant.

HARES-017 is required for `ElectricBoiler` and `GasBoiler` because those types write `PortContribution::Fluid` to a hydronic loop — which requires `FluidAccumulator` to be defined and the `FluidSolver` to be in place.

## Work to Do
- [ ] `ElectricFurnace`: `electric_kw = capacity * EIR`; supply air temperature default from `HvacEquipment` base (48.9°C / 120°F), overridable via `EquipmentConfig`; writes Electrical + Thermal ports via duct DSE
- [ ] `GasFurnace`: gas consumption derived from heating capacity and fuel efficiency; separate fan-only electric draw; supply air temperature default from `HvacEquipment` base (54.4°C / 130°F), overridable via `EquipmentConfig`; writes Fuel + Electrical + Thermal ports
- [ ] `ElectricBaseboard`: force duct DSE to 1.0 (direct zone delivery); simple resistance heating `electric_kw = capacity`; writes Electrical + Thermal ports; `supply_air_temp_c` field unused (no forced air)
- [ ] `ElectricBoiler`: resistance heating with configurable efficiency; writes Electrical + Fluid ports
- [ ] `GasBoiler`:
  - Condensing boiler: 6-coefficient polynomial `[1, plr, plr², t_in, t_in², plr*t_in]` (config key: `condensing_eir_coeffs: [f64; 6]`). Reference OCHRE `HVAC.py:697-731` for default coefficient values.
  - Non-condensing boiler: 10-coefficient polynomial `[1, plr, plr², t_out, t_out², plr*t_out, plr³, t_out³, plr²*t_out, plr*t_out²]` (config key: `non_condensing_eir_coeffs: [f64; 10]`). Reference OCHRE `HVAC.py:697-731` for default coefficient values. Do NOT fall back to constant efficiency for non-condensing — implement the full polynomial.
- [ ] For each equipment type, declare `control_capabilities` in `EquipmentDescriptor` (furnaces/boilers: `ThermalSetpoint`; baseboard: `ThermalSetpoint`)
- [ ] For each equipment type, declare `telemetry_fields` in `EquipmentDescriptor` — at minimum: power draw fields and operating mode
- [ ] Assign `ExecutionStage::Thermal` for all five types in `EquipmentDescriptor`
- [ ] Implement `save_state` / `load_state` for all five types — include thermostat FSM state and any cycle-time counters
- [ ] Register all five equipment types in `EquipmentRegistry` with exact OCHRE name strings:
  - `"Electric Furnace"`
  - `"Gas Furnace"`
  - `"Electric Baseboard"`
  - `"Electric Boiler"`
  - `"Gas Boiler"`
- [ ] Refactor for separation of concerns: shared HVAC parsing/validation/runtime helpers must live in common/shared code (not duplicated across furnace/baseboard/boiler implementations)
- [ ] Keep implementation files maintainable in size (soft target: split by concern if a file grows beyond ~500 LOC)
- [ ] Ensure tests are organized by concern (furnace/baseboard/boiler/common) with no large monolithic test blocks

## Files to Touch
- `crates/hares-equipment/src/hvac/furnace.rs`: new file — `ElectricFurnace`, `GasFurnace`
- `crates/hares-equipment/src/hvac/baseboard.rs`: new file — `ElectricBaseboard`
- `crates/hares-equipment/src/hvac/boiler.rs`: new file — `ElectricBoiler`, `GasBoiler`
- `crates/hares-equipment/src/hvac/common.rs`: shared HVAC helpers/state extensions used by these implementations
- `crates/hares-equipment/src/hvac/mod.rs`: module wiring for any refactoring splits

## Measures of Success
- [ ] `ElectricFurnace` power equals `capacity * EIR` at rated conditions
- [ ] `GasFurnace` gas consumption plus fan electric matches expected total input energy
- [ ] `GasBoiler` condensing EIR curve: at partial load (PLR = 0.5) with known coefficients loaded from `EquipmentConfig`, computed EIR matches expected value to within 1e-4 (derive expected from OCHRE `HVAC.py:697-731` reference output)
- [ ] `GasBoiler` condensing efficiency is higher at partial load than the non-condensing variant when using typical coefficients
- [ ] Non-condensing boiler EIR varies with PLR and outlet temperature per the 10-coefficient polynomial; test at PLR=0.5 against OCHRE output
- [ ] All five types pass through duct DSE correctly (except `ElectricBaseboard` which bypasses it)
- [ ] Supply air temperatures are defaults from `HvacEquipment` base, not hardcoded — overriding via `EquipmentConfig` changes the value used in `step()`
- [ ] `save_state` / `load_state` round-trips without loss for each type
- [ ] `ExecutionStage` is `Thermal` for all five types
- [ ] All five OCHRE name strings register and resolve correctly via `EquipmentRegistry::create`
- [ ] Shared helper code is reused across implementations (no duplicated numeric parsing and mode/telemetry boilerplate)
- [ ] Per-file scope remains focused (furnace/baseboard/boiler files each primarily contain equipment-specific behavior, shared logic extracted)
- [ ] Test coverage remains strong after refactoring (behavioral parity tests continue to pass with no dropped assertions)

## Verification
- [ ] `cargo check -p hares-equipment` passes
- [ ] `cargo test -p hares-equipment` passes
- [ ] `cargo clippy -p hares-equipment -- -D warnings` passes
