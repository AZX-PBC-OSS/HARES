---
id: HELICS-001
title: "Add reactive_power_kvar to DwellingTelemetry"
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-core/src/telemetry.rs
  - crates/hares-core/src/dwelling/mod.rs
  - crates/hares-python/src/py_telemetry.rs
  - python/ochre_next/_hares.pyi
references:
  - docs/tickets/HARES-065.md
verification:
  - cargo check -p hares-core
  - cargo test -p hares-core
  - cargo clippy -p hares-core -- -D warnings
  - cargo check -p hares-python
  - cargo clippy -p hares-python -- -D warnings
---

## Background/Context
HELICS co-simulation with GridLAB-D requires publishing both active power (`total_power_kw`) and reactive power (`reactive_power_kvar`) per dwelling at each timestep. `DwellingTelemetry` currently exposes `total_power_kw` but has no reactive power field. The `ElectricalSolver` already tracks `net_reactive_kvar()` — it just needs to be surfaced through telemetry and the Python binding.

## Work to Do
- [ ] Add `reactive_power_kvar: f64` field to `DwellingTelemetry` in `telemetry.rs`
- [ ] Populate the new field from `ElectricalSolver::net_reactive_kvar()` in `Dwelling::telemetry()` (dwelling/mod.rs)
- [ ] Expose `reactive_power_kvar` as a Python-accessible getter on `PyTelemetry` in `py_telemetry.rs`
- [ ] Add test: after a dwelling step with equipment contributing reactive power, `telemetry().reactive_power_kvar` reflects the accumulated value
- [ ] Update `python/ochre_next/_hares.pyi` to add `reactive_power_kvar` property to the `Telemetry` stub class

## Files to Touch
- `crates/hares-core/src/telemetry.rs`: add `reactive_power_kvar` field to `DwellingTelemetry`
- `crates/hares-core/src/dwelling/mod.rs`: populate reactive power from solver in `telemetry()` method
- `crates/hares-python/src/py_telemetry.rs`: add `reactive_power_kvar()` getter to `PyTelemetry`
- `python/ochre_next/_hares.pyi`: add `reactive_power_kvar` property to `Telemetry` stub

## Measures of Success
- [ ] `DwellingTelemetry` contains `reactive_power_kvar: f64`
- [ ] `PyTelemetry.reactive_power_kvar` is accessible from Python
- [ ] Existing tests continue to pass (field defaults to 0.0 for zero-reactive-load scenarios)

## Verification
- [ ] `cargo check -p hares-core` passes
- [ ] `cargo test -p hares-core` passes
- [ ] `cargo clippy -p hares-core -- -D warnings` passes
- [ ] `cargo check -p hares-python` passes
- [ ] `cargo clippy -p hares-python -- -D warnings` passes
