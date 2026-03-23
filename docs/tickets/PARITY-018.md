---
id: PARITY-018
title: "Solar distribution to interior surfaces"
kind: implement
depends_on: [PARITY-015, PARITY-016]
files_to_touch:
  - crates/hares-envelope/src/thermal_solver/solar.rs
  - crates/hares-envelope/src/thermal_solver/config.rs
  - crates/hares-envelope/src/thermal_solver/mod.rs
  - crates/hares-core/src/dwelling/solver_builder.rs
references:
  - docs/equipment/ochre-parity-gaps.md (Gap 3)
  - EnergyPlus Engineering Reference Ch. 3.11.4 (Interior Solar Distribution)
  - vendors/OCHRE/ochre/models/Envelope.py (window view factors, lines 548-558)
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace -- -D warnings
---

## Background/Context

HARES currently routes all transmitted window solar directly to the zone air node. EnergyPlus distributes transmitted solar to interior surfaces based on area-weighted absorptance, then accounts for reflected solar redistribution. OCHRE distributes proportionally via window view factors.

**Target**: EnergyPlus "FullInteriorAndExterior" solar distribution method — transmitted beam solar hits floor first (60% default for residential), remainder distributed to walls/ceiling by area. Diffuse distributed to all surfaces by area×absorptance.

Depends on PARITY-001 (interior surface info) and PARITY-002 (window transmittance curves).

## Work to Do

- [ ] In `thermal_solver/config.rs`, extend `InteriorSurfaceInfo` (from PARITY-001):
  - Add `solar_absorptance: f64` (default 0.6 for floors, 0.5 for walls per EnergyPlus)
  - Add `is_floor: bool` (receives beam solar preferentially)
- [ ] In `thermal_solver/solar.rs`, add `distribute_transmitted_solar()`:
  - Beam: 60% to floor surfaces (split by area), 40% to walls/ceiling (split by area×absorptance)
  - Diffuse: distribute to all surfaces by area×absorptance
  - Reflected fraction (1 - absorptance) redistributed to all other surfaces (single-bounce approximation)
  - Write solar injection to each surface's B-matrix input column (using surface state_index)
- [ ] Reduce zone air direct injection to only the reflected remainder that isn't absorbed by any surface
- [ ] In `solver_builder.rs`, populate `solar_absorptance` and `is_floor` from building boundary data
- [ ] Add tests: single-zone box with south window, verify floor gets majority of beam, total solar conserved

## Files to Touch

- `crates/hares-envelope/src/thermal_solver/solar.rs`: Solar distribution logic
- `crates/hares-envelope/src/thermal_solver/config.rs`: Extended surface info
- `crates/hares-envelope/src/thermal_solver/mod.rs`: Wire into resolve pipeline
- `crates/hares-core/src/dwelling/solver_builder.rs`: Populate surface solar properties

## Measures of Success

- [ ] Beam solar primarily heats floor surfaces (60%+ to floor)
- [ ] Total solar energy conserved (input = sum of all surface absorptions + reflected-to-air)
- [ ] Floor surface temperature increases noticeably vs previous (direct-to-air) approach
- [ ] No hot-path allocation

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
