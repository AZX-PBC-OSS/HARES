---
id: HARES-009
title: "hares-physics — Units Module"
kind: implement
depends_on: [HARES-001, HARES-005, HARES-008]
files_to_touch:
  - crates/hares-physics/src/units.rs
  - crates/hares-physics/src/lib.rs
references:
  - docs/architecture/00-overview.md
  - docs/architecture/06-input-output.md
  - docs/PHYSICS_DECISIONS.md
verification:
  - cargo check -p hares-physics
  - cargo test -p hares-physics
  - cargo clippy -p hares-physics -- -D warnings
---

## Background/Context
Raw `f64` fields for physical quantities make it easy to pass values in wrong units silently. Introducing `uom` quantity type aliases at API boundaries encodes the intended unit in the type system and eliminates a class of physics bugs. This module is a prerequisite for other physics modules that need strongly-typed quantity arguments.

## Work to Do
- [ ] Add the `uom` crate as a dependency of `hares-physics`
- [ ] Define quantity type aliases in `units.rs` for:
  - `Temperature` (thermodynamic temperature, SI: kelvin)
  - `Power` (SI: watt)
  - `Energy` (SI: joule)
  - `Pressure` (SI: pascal)
  - `MassFlowRate` (SI: kg/s)
  - `Length` (SI: metre)
  - `Area` (SI: m²)
  - `Volume` (SI: m³)
  - `Velocity` (SI: m/s)
  - `HeatCapacity` (SI: J/K)
- [ ] Convert public boundaries of at minimum: `saturation_pressure_pa`, `moist_air_density_kg_m3`, and `standard_pressure_pa` to use `uom` quantity types. Document which boundaries stay as raw `f64` and why in `docs/PHYSICS_DECISIONS.md`
- [ ] Add conversion helpers where the ergonomic cost of `uom` at an internal boundary would outweigh the safety benefit

## Files to Touch
- `crates/hares-physics/src/units.rs`: new file — `uom` type aliases and conversion helpers
- `crates/hares-physics/src/lib.rs`: add `pub mod units`

## Measures of Success
- [ ] All ten quantity types are defined and publicly exported
- [ ] Typed `uom` wrappers exist for all psychrometric, air-property, and applicable infiltration functions (see HARES-005 and HARES-008 for full list)
- [ ] Module-level doc clearly states the boundary policy so future contributors know when to use typed vs raw quantities
- [ ] No `uom` types leak into `hares-types` — verify by confirming `uom` is absent from `crates/hares-types/Cargo.toml`
- [ ] The boundary policy decision is recorded in `docs/PHYSICS_DECISIONS.md` (create or append an entry if the file does not yet exist)

## Verification
- [ ] `cargo check -p hares-physics` passes
- [ ] `cargo test -p hares-physics` passes
- [ ] `cargo clippy -p hares-physics -- -D warnings` passes
