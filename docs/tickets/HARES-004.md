---
id: HARES-004
title: "hares-types — Control Signal Types"
kind: implement
depends_on: [HARES-001]
files_to_touch:
  - crates/hares-types/src/control_signal.rs
  - crates/hares-types/src/lib.rs
references:
  - docs/architecture/03-control-interfaces.md
verification:
  - cargo check -p hares-types
  - cargo test -p hares-types
  - cargo clippy -p hares-types -- -D warnings
---

## Background/Context
Controllers, optimisers, and OCHRE compatibility shims all need to express intent through a common, typed signal vocabulary. Defining `ControlSignal` in `hares-types` makes it available to both the control crate and any equipment crate that needs to introspect the signal it received.

## Work to Do
- [ ] Define `ControlSignal` enum with all variants:
  - `ThermalSetpoint { heating_setpoint_c: Option<f64>, cooling_setpoint_c: Option<f64>, deadband_c: Option<f64> }`
  - `HumiditySetpoint { target_rh: f64, min_rh: Option<f64>, max_rh: Option<f64> }`
  - `PowerSetpoint { active_power_kw: f64, reactive_power_kvar: Option<f64> }`
  - `PowerLimit { max_power_kw: f64, ramp_rate_kw_per_s: Option<f64> }`
  - `SOCTarget { target_soc: f64, min_soc: Option<f64>, max_soc: Option<f64> }`
  - `ModeOverride { mode: OperatingMode }`
  - `DutyCycle { on_fraction: f64, period_s: Option<f64> }`
  - `LoadFraction { fraction: f64 }`
  - `GridConnect { connected: bool }`
  - `SelfConsumption { enabled: bool, solar_only_charging: bool }`
  - `DemandResponse { level: DRLevel, duration_s: Option<f64> }`
  - `ProtocolNative { protocol: ProtocolId, payload: Vec<u8> }`
- [ ] Define `DRLevel` enum with variants: `Normal`, `Moderate`, `High`, `Critical`, `GridEmergency`
- [ ] Define `ControlCapabilities` using `bitflags` crate with all flags: `POWER_SETPOINT`, `SOC_TARGET`, `THERMAL_SETPOINT`, `POWER_LIMIT`, `MODE_OVERRIDE`, `DUTY_CYCLE`, `LOAD_FRACTION`, `GRID_CONNECT`, `SELF_CONSUMPTION`, `DEMAND_RESPONSE`, `PROTOCOL_NATIVE`, `HUMIDITY_SETPOINT`
- [ ] Derive `serde::Serialize` and `serde::Deserialize` on `ControlSignal`, `DRLevel`, and `ControlCapabilities`
- [ ] Re-export from `lib.rs`
- [ ] Write tests:
  - Verify `ControlCapabilities` flags can be composed with `|` and tested with `contains()`
  - Verify that `ensure_signal_supported(caps, signal)` returns `Err` when the signal type is not present in `caps` (capability rejection — this function lives in `hares-types/src/control_signal.rs`)

## Files to Touch
- `crates/hares-types/src/control_signal.rs`: new file — `ControlSignal`, `DRLevel`, `ControlCapabilities`
- `crates/hares-types/src/lib.rs`: add `pub mod control_signal` and re-exports

## Measures of Success
- [ ] `ControlSignal` enum has all 12 variants with fields exactly as specified
- [ ] `SelfConsumption::solar_only_charging` is `bool` (not `Option<bool>`)
- [ ] `ProtocolNative::protocol` is `ProtocolId` (not raw `u16`)
- [ ] `DRLevel` is a separate enum with exactly five variants: `Normal`, `Moderate`, `High`, `Critical`, `GridEmergency`
- [ ] `ControlCapabilities` uses `bitflags` so capability sets can be composed with `|`, and declares all 12 flags
- [ ] All variants round-trip through serde JSON
- [ ] `ProtocolNative::payload` is `Vec<u8>` (variable length, not fixed array)
- [ ] Capability rejection test: `ensure_signal_supported(caps, signal)` (defined in `hares-types/src/control_signal.rs`) returns `Err` when the signal type is not present in `caps`

## Verification
- [ ] `cargo check -p hares-types` passes
- [ ] `cargo test -p hares-types` passes
- [ ] `cargo clippy -p hares-types -- -D warnings` passes
