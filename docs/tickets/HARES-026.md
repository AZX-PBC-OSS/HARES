---
id: HARES-026
title: "hares-equipment — Stratified Water Tank"
kind: implement
depends_on: [HARES-018]
files_to_touch:
  - crates/hares-equipment/src/water_heater/mod.rs
  - crates/hares-equipment/src/water_heater/tank.rs
references:
  - docs/architecture/02-equipment-and-ports.md
verification:
  - cargo check -p hares-equipment
  - cargo test -p hares-equipment
  - cargo clippy -p hares-equipment -- -D warnings
---

## Background/Context
All storage water heater types (resistance, gas, HPWH) share the same stratified tank physics. Extracting the tank model into its own module prevents duplication and lets tank geometry and stratification tests be written once and reused.

Note: `StratifiedTank` is an internal component, not an `Equipment` implementor. It is owned by the water heater structs defined in HARES-027. It does not declare ports, control capabilities, or telemetry fields, and is not registered in `EquipmentRegistry`.

## Work to Do
- [ ] Define `StratifiedTank` struct with configurable 1–12 nodes; store per-node temperature and volume arrays
- [ ] Implement per-node energy balance each timestep: inter-node conduction between adjacent layers (`k * A * dT / dz`); standby heat loss to ambient zone proportional to `UA * (T_node - T_ambient)`
- [ ] Implement draw mixing algorithm: hot water drawn from index-0 (top/outlet) node, cold mains water enters at index N-1 (bottom/inlet) node, conserving energy by tracking partial node fills
- [ ] Implement inversion mixing: after each thermal step, scan from index 0 (top) to index N-1 (bottom) and mix any adjacent pair where the upper node (lower index) is cooler than the lower node (higher index); repeat until density-stable. Maximum iteration count is `n_nodes` passes (always sufficient to fully sort). Terminate early if no swaps occur in a pass.
- [ ] Store tank geometry: height (m), diameter (m), node volumes (equal by default)
- [ ] Support TWO heating element node positions for dual-element water heaters; store as `element_nodes: [Option<usize>; 2]`, each independently validated against node count at `init`
- [ ] Index convention: index 0 = TOP node (hot outlet side), index N-1 = BOTTOM node (cold inlet side); all external callers must use this convention. This matches OCHRE's `Water.py` line 158: "Node 1 is at the top of the tank (at outlet)." (OCHRE uses 1-based indexing; HARES uses 0-based, so OCHRE node 1 = HARES index 0.) This is critical — HARES-027 callers reference specific node positions for upper/lower heating elements.
- [ ] Expose `node_temps(&self) -> &[f64]` and `heat_node(&mut self, node: usize, power_w: f64, dt: Duration)` for use by water heater implementations
- [ ] Implement `save_state() -> Vec<u8>`: serialize full per-node temperature array for RL checkpointing
- [ ] Implement `load_state(&mut self, state: &[u8]) -> Result<()>`: deserialize and restore per-node temperatures

## Files to Touch
- `crates/hares-equipment/src/water_heater/mod.rs`: module declaration and public re-exports
- `crates/hares-equipment/src/water_heater/tank.rs`: new file — `StratifiedTank` with energy balance, draw mixing, and inversion mixing

## Measures of Success
- [ ] 1-node tank: temperature response to constant heating matches `dT = P*dt / (m*Cp)` analytically
- [ ] 6-node tank: after a draw event, index-0 (top/outlet) nodes are cooler than pre-draw and index N-1 (bottom/inlet) nodes approach mains temperature — stratification develops
- [ ] Standby decay: with no heating and no draw, temperature falls toward ambient at the correct `UA/mCp` time constant
- [ ] Draw event: outlet temperature equals the pre-draw index-0 (top) node temperature; index N-1 (bottom) node reaches mains temperature after sufficient draw volume
- [ ] Energy conservation for draw events: `energy_out_to_draw + energy_in_from_mains` equals the net enthalpy change of tank contents to within 1 J
- [ ] Dual-element configuration: `heat_node` called on node index 0 (top) and node index N-1 (bottom) both produce correct `dT = P*dt / (m_node*Cp)` in isolation
- [ ] `save_state` / `load_state` round-trip reproduces identical node temperatures
- [ ] Inversion mixing on a fully inverted 12-node tank terminates within `n_nodes` passes and produces a density-stable temperature profile: temperature is monotonically non-increasing from index 0 (top/hot) to index N-1 (bottom/cold)
- [ ] A uniform-temperature tank terminates mixing immediately with zero swaps

## Verification
- [ ] `cargo check -p hares-equipment` passes
- [ ] `cargo test -p hares-equipment` passes
- [ ] `cargo clippy -p hares-equipment -- -D warnings` passes
