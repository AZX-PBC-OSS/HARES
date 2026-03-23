---
id: PARITY-009
title: "Ventilation fan / HRV / ERV equipment type"
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-equipment/src/ventilation.rs (new)
  - crates/hares-equipment/src/lib.rs
  - crates/hares-equipment/src/registry.rs
  - crates/hares-io/src/hpxml/building.rs
references:
  - docs/equipment/ochre-parity-gaps.md (Gap 8)
  - EnergyPlus Engineering Reference Ch. 17.2 (Heat Recovery)
  - ASHRAE 62.2 (Residential Ventilation)
  - vendors/OCHRE/ochre/models/Equipment.py (ScheduledLoad for ventilation)
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace -- -D warnings
---

## Background/Context

HARES has no ventilation equipment. OCHRE models ventilation fans as ScheduledLoads. For EnergyPlus-grade accuracy, we should model HRV/ERV with sensible and latent recovery effectiveness, fan power, defrost, and bypass control.

**Target**: Better than OCHRE — implement full HRV/ERV with recovery effectiveness per ASHRAE 62.2 and CSA C439, not just a scheduled load.

## Work to Do

- [ ] Create `crates/hares-equipment/src/ventilation.rs`:
  - `ExhaustFan`: simple fan with rated power, scheduled on/off
  - `BalancedVentilation`: supply + exhaust with optional heat recovery
  - `HRV`: sensible recovery effectiveness (temperature-dependent curve)
  - `ERV`: sensible + latent recovery effectiveness
- [ ] Implement `Equipment` trait for each variant:
  - `step()`: compute supply air temperature after recovery, fan power, thermal/latent port contributions
  - For HRV: `T_supply = T_outdoor + sensible_eff × (T_indoor - T_outdoor)`
  - For ERV: also `W_supply = W_outdoor + latent_eff × (W_indoor - W_outdoor)`
  - Thermal port: sensible ventilation load to zone
  - Electrical port: fan power
- [ ] Support bypass mode: when outdoor conditions are favorable, bypass recovery (free cooling)
- [ ] Support defrost: at low outdoor temps, reduce effectiveness or cycle supply
- [ ] Parse from HPXML: `MechanicalVentilation` element with fan type, flow rate, recovery effectiveness
- [ ] Register in `EquipmentRegistry`
- [ ] Add tests: HRV at -20°C outdoor / 20°C indoor with 70% effectiveness → supply at 8°C

## Files to Touch

- `crates/hares-equipment/src/ventilation.rs`: New equipment module
- `crates/hares-equipment/src/lib.rs`: Export module
- `crates/hares-equipment/src/registry.rs`: Register ventilation types
- `crates/hares-io/src/hpxml/building.rs`: Parse MechanicalVentilation elements

## Measures of Success

- [ ] HRV reduces ventilation heating load by ~60-80% (matching rated effectiveness)
- [ ] ERV additionally reduces latent load
- [ ] Fan power appears in electrical port
- [ ] Bypass mode activates when outdoor temp is within comfort range

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
