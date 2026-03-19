---
id: HARES-022
title: "hares-equipment — Dynamic HVAC and Biquadratic Performance"
kind: implement
depends_on: [HARES-020, HARES-006]
files_to_touch:
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
Air conditioners, heat pumps, and multi-speed equipment all require performance curves that depend on indoor/outdoor temperatures, airflow fraction, and part-load ratio. Centralising biquadratic evaluation and multi-speed control in `hvac/common.rs` avoids duplicating this logic across ACs, ASHPs, and MSHPs.

**Merge coordination**: This ticket extends `hvac/common.rs`, which is also created by HARES-020. Fields added here (`speed_control_mode`, PLF state, startup degradation state) must not conflict with fields added in HARES-020 (`supply_air_temp_c`, `airflow_cfm_per_ton`, thermostat FSM). Coordinate with the HARES-020 implementer before opening a PR; prefer additive changes in a clearly separated block.

## Work to Do
- [ ] Add biquadratic coefficient loading from config with fallback to built-in defaults; store as `[[f64; 6]]` per curve
- [ ] Implement biquadratic evaluation by calling `BiquadraticCurve::evaluate(x, y)` from HARES-006. Do not re-implement the evaluation kernel. For cooling curves, `x` is indoor wet-bulb temperature (not dry-bulb), consistent with AHRI 210/240 rating conditions and the OCHRE implementation. For heating curves, `x` is indoor dry-bulb. `y` is outdoor dry-bulb in both cases.
- [ ] Flow fraction correction and PLF are **separate multiplicative corrections** applied after biquadratic evaluation — do not include them as biquadratic inputs. Apply order: `param_raw = biquadratic(t_indoor, t_outdoor)`, then `param_adj = param_raw * flow_fraction_correction`, then `param_final = param_adj * plf`
- [ ] Implement single-speed control: binary on/off cycling to meet load fraction over timestep
- [ ] Implement two-speed control: setpoint-based switching between low speed and high speed (high speed engages when load fraction exceeds the low-speed capacity fraction). Note: OCHRE uses setpoint-based two-speed switching, not time-based; if time-based mode is needed, add a separate acceptance criterion or explicitly remove it from scope
- [ ] Implement 4-speed control: discrete speed selection proportional to load fraction; add acceptance criterion (see Measures of Success)
- [ ] Implement variable-speed (ideal) control: continuous speed index interpolation to exactly match load
- [ ] Implement part-load factor: `PLF = 1.0 - Cd * (1.0 - PLR)` where `Cd` is the degradation coefficient from config
- [ ] Implement startup capacity degradation: define `StartupConfig` struct with fields loaded from `EquipmentConfig` (or OCHRE defaults when absent). Apply degradation factor to capacity during the first timestep of each on-cycle. `StartupConfig` fields: at minimum `capacity_fraction: f64` (fraction of rated capacity during startup step)
- [ ] Extend `HvacEquipment` to carry speed-control mode and PLF state
- [ ] Refactor dynamic-HVAC concerns into focused units (e.g., curve loading/evaluation, speed selection, PLF/startup logic) so `common.rs` does not become a monolithic file
- [ ] Ensure DRY implementation: no duplicated curve parsing/evaluation logic across HVAC equipment types; dynamic behavior must be reusable from shared code
- [ ] Preserve or improve test coverage while refactoring; no loss of existing HARES-020 thermostat/base helper tests

## Files to Touch
- `crates/hares-equipment/src/hvac/common.rs`: extend with biquadratic evaluation (via HARES-006), multi-speed control logic, PLF, and startup degradation
- `crates/hares-equipment/src/hvac/mod.rs`: module wiring if dynamic logic is split into additional files

## Measures of Success
- [ ] Biquadratic evaluation with known 6-coefficient set matches OCHRE reference output to at least 6 significant figures
- [ ] Two-speed control (setpoint-based): transitions from low to high speed when load fraction exceeds the low-speed capacity fraction at the current conditions; transitions back to low when load drops below threshold
- [ ] 4-speed control: speed index selected matches expected discrete level for load fractions 25%, 50%, 75%, 100%
- [ ] PLF at `PLR = 0.5` with `Cd = 0.25` equals 0.875
- [ ] Variable-speed control produces exactly the requested capacity fraction (no cycling loss)
- [ ] Startup degradation reduces first-step capacity relative to steady-state; second step and beyond are at full capacity
- [ ] Flow fraction correction and PLF are applied after biquadratic, not as biquadratic inputs — verify by unit-testing each stage independently
- [ ] `ExecutionStage` assignment for all types using this module is `Thermal` (verify not changed by this extension)
- [ ] Dynamic-HVAC code remains maintainable in size (soft target: split by concern if shared module exceeds ~500 LOC)
- [ ] Refactoring does not reduce coverage; all existing HVAC common tests and new dynamic tests pass together

## Verification
- [ ] `cargo check -p hares-equipment` passes
- [ ] `cargo test -p hares-equipment` passes
- [ ] `cargo clippy -p hares-equipment -- -D warnings` passes
