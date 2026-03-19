---
id: HARES-064
title: "ControlSignal HumiditySetpoint Variant"
kind: implement
depends_on: [HARES-004]
files_to_touch:
  - crates/hares-types/src/control_signal.rs
references:
  - docs/architecture/03-control-interfaces.md
verification:
  - cargo check -p hares-types
  - cargo test -p hares-types
  - cargo clippy -p hares-types -- -D warnings
---

## Background/Context
The Dehumidifier equipment (HARES-025) responds to relative humidity setpoints, not thermal setpoints. The current `ControlSignal` enum has no humidity-aware variant. Without this, dehumidifier control is impossible through the typed signal system.

## Work to Do
- [ ] Add `ControlSignal::HumiditySetpoint { setpoint_rh_pct: f64, deadband_rh_pct: Option<f64> }` variant
- [ ] Add `HUMIDITY_SETPOINT` flag to `ControlCapabilities` bitflags
- [ ] Add `required_capability` mapping: `HumiditySetpoint => HUMIDITY_SETPOINT`
- [ ] Update `ensure_signal_supported` to handle the new variant

## Measures of Success
- [ ] `ControlSignal::HumiditySetpoint` round-trips through serde JSON
- [ ] `ensure_signal_supported` returns Ok for equipment with `HUMIDITY_SETPOINT` capability
- [ ] `ensure_signal_supported` returns Err for equipment without `HUMIDITY_SETPOINT` capability
