---
id: HARES-032
title: "hares-equipment — Event-Based Loads and Wet Appliance"
kind: implement
depends_on: [HARES-018]
files_to_touch:
  - crates/hares-equipment/src/event_load.rs
references:
  - docs/architecture/02-equipment-and-ports.md
verification:
  - cargo check -p hares-equipment
  - cargo test -p hares-equipment
  - cargo clippy -p hares-equipment -- -D warnings
---

## Background/Context
Lighting, miscellaneous plug loads, and appliances such as clothes washers, dishwashers, and dryers are modelled as event-based loads. The `WetAppliance` subtype adds a multi-phase cycle state machine to capture the distinctive power shape of wash cycles. Both types write thermal gains to the zone, which matters for building envelope interactions.

Stochastic event timing is included in this ticket as an intentional scope expansion from the arch v1.1 deferred list. OCHRE's `EventBasedLoad` has stubs (`NotImplementedError`) for stochastic timing, but residential fleet simulations require appliance event diversity to avoid unrealistic simultaneity. The architecture already supports stochastic timing through the per-building RNG (`derive_dwelling_rng`) and the sequential execution model, so pulling it forward avoids a second pass over this equipment type. The roadmap deferred table should be updated to reflect that this item is now included here.

**Soft dependency**: This ticket has a soft dependency on HARES-041 (Clock and RNG). The `ChaCha8` hierarchical seeding scheme — where the per-dwelling RNG is derived from a master seed and building ID — must be in place before stochastic event timing can produce deterministic, reproducible results. If HARES-041 is not yet complete, use a placeholder RNG seeded from `building_id` directly and leave a TODO to wire up the hierarchical seeding when HARES-041 lands.

## Work to Do
- [ ] Implement `EventBasedLoad` struct implementing `Equipment`
  - [ ] Assign `ExecutionStage::Independent` (Stage 1) — event-based loads require no thermal feedback and run before electrical and thermal equipment
  - [ ] Set `control_capabilities`: `LOAD_FRACTION` (scale active power) and `MODE_OVERRIDE` (force Idle or Active)
  - [ ] Set `telemetry_fields`: `active_power_kw`, `sensible_gain_w`, `latent_gain_w`, `state` (ordinal)
  - [ ] Event scheduling from schedule CSV data passed via `EquipmentConfig`
  - [ ] State machine: `Idle → Active → Cooldown → Idle`; transition times from config
  - [ ] Power profile during `Active` state: configurable duration and power level
  - [ ] Stochastic event timing: sample event start from probability distribution when schedule indicates an event window
  - [ ] Write Electrical port during `Active`; write Thermal port scaled by `sensible_gain_fraction` and `latent_gain_fraction`
  - [ ] Write `reactive_power_kvar = 0.0` explicitly on every Electrical port contribution (not modeled, but must not be left uninitialised)
  - [ ] Implement `save_state`: serialise current state machine position (`Idle`/`Active`/`Cooldown`) and remaining time in current phase as `Vec<u8>`
  - [ ] Implement `load_state`: deserialise and restore state machine position and phase timer; return error on malformed bytes
- [ ] Implement `WetAppliance` struct implementing `Equipment` (extends event-based pattern)
  - [ ] Assign `ExecutionStage::Independent` (Stage 1)
  - [ ] Set `control_capabilities`: `LOAD_FRACTION`, `MODE_OVERRIDE`
  - [ ] Set `telemetry_fields`: `active_power_kw`, `sensible_gain_w`, `latent_gain_w`, `cycle_phase` (ordinal)
  - [ ] Multi-phase cycle state machine; each phase has configurable power (kW) and duration (s); phases are defined in config (e.g. `[{power: 0.5, duration: 600}, {power: 1.2, duration: 1200}, ...]`)
  - [ ] Phase transitions are deterministic once a cycle starts
  - [ ] Multi-capacity support: `n_units` in config, events scale power by unit count
  - [ ] Write Electrical + Thermal ports per phase; `reactive_power_kvar = 0.0` explicitly
  - [ ] Implement `save_state`/`load_state`: serialise current phase index and elapsed time within phase
- [ ] Register both types in `EquipmentRegistry`
  - Register `WetAppliance` variants with OCHRE-compatible names: `'Clothes Washer'`, `'Dishwasher'`, `'Clothes Dryer'`
- [ ] Document how OCHRE's 1440-minute probability vector format (from `pdf_*.csv` files) maps to the new multi-phase cycle config. Provide a conversion function or document that the HPXML/schedule ingestion layer (HARES-036) must perform this mapping.

## Files to Touch
- `crates/hares-equipment/src/event_load.rs`: new file — `EventBasedLoad`, `WetAppliance`, shared state machine infrastructure

## Measures of Success
- [ ] `EventBasedLoad` and `WetAppliance` descriptors both report `ExecutionStage::Independent`
- [ ] Events trigger at the scheduled time (within one timestep tolerance)
- [ ] Multi-phase cycle produces the configured power profile shape across all phases in sequence
- [ ] Thermal port sensible and latent gains are zero when equipment is in `Idle` state
- [ ] `WetAppliance` with `n_units = 2` draws twice the single-unit power during `Active`
- [ ] `save_state` followed by `load_state` on a fresh instance restores identical behaviour for subsequent steps

## Verification
- [ ] `cargo check -p hares-equipment` passes
- [ ] `cargo test -p hares-equipment` passes
- [ ] `cargo clippy -p hares-equipment -- -D warnings` passes
