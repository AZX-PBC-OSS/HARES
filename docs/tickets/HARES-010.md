---
id: HARES-010
title: "hares-control — Signal Definitions and Capabilities"
kind: implement
depends_on: [HARES-004]
files_to_touch:
  - crates/hares-control/src/signal.rs
  - crates/hares-control/src/capabilities.rs
  - crates/hares-control/src/lib.rs
references:
  - docs/architecture/03-control-interfaces.md
verification:
  - cargo check -p hares-control
  - cargo test -p hares-control
  - cargo clippy -p hares-control -- -D warnings
---

## Background/Context
Controllers and optimisers need ergonomic constructors for `ControlSignal` and a way to ask "can this equipment accept this signal?" before dispatching. Keeping constructors and capability validation in `hares-control` avoids polluting the shared `hares-types` crate with control-layer logic.

## Work to Do
- [ ] Implement constructor helpers in `signal.rs` for each `ControlSignal` variant (all 12):
  - `ControlSignal::thermal_setpoint(heating_setpoint_c: Option<f64>, cooling_setpoint_c: Option<f64>, deadband_c: Option<f64>) -> ControlSignal`
  - `ControlSignal::power_setpoint(active_power_kw: f64, reactive_power_kvar: Option<f64>) -> ControlSignal`
  - `ControlSignal::power_limit(max_power_kw: f64, ramp_rate_kw_per_s: Option<f64>) -> ControlSignal`
  - `ControlSignal::soc_target(target_soc: f64, min_soc: Option<f64>, max_soc: Option<f64>) -> ControlSignal`
  - `ControlSignal::mode_override(mode: OperatingMode) -> ControlSignal`
  - `ControlSignal::duty_cycle(on_fraction: f64, period_s: Option<f64>) -> ControlSignal`
  - `ControlSignal::load_fraction(fraction: f64) -> ControlSignal`
  - `ControlSignal::grid_connect(connected: bool) -> ControlSignal`
  - `ControlSignal::self_consumption(enabled: bool, solar_only_charging: bool) -> ControlSignal`
  - `ControlSignal::demand_response(level: DRLevel, duration_s: Option<f64>) -> ControlSignal`
  - `ControlSignal::humidity_setpoint(target_rh: f64, min_rh: Option<f64>, max_rh: Option<f64>) -> ControlSignal`
  - `ControlSignal::protocol_native(protocol: ProtocolId, payload: Vec<u8>) -> ControlSignal`
- [ ] Re-export `ControlCapabilities` from `hares-types` in `capabilities.rs`
- [ ] Implement `can_accept(caps: ControlCapabilities, signal: &ControlSignal) -> bool` in `capabilities.rs`
- [ ] Write tests:
  - Construct each variant using its constructor helper and confirm the fields are set correctly
  - Test `can_accept` accepts a signal when the matching capability bit is set
  - Test `can_accept` rejects a signal when the capability bit is absent — all 12 signal types must have at least one reject-case test

## Files to Touch
- `crates/hares-control/src/signal.rs`: new file — constructor helpers
- `crates/hares-control/src/capabilities.rs`: new file — `ControlCapabilities` re-export and `can_accept`
- `crates/hares-control/src/lib.rs`: module declarations and public re-exports

## Measures of Success
- [ ] One constructor helper exists for every `ControlSignal` variant (12 total)
- [ ] `can_accept` correctly gates each variant against its corresponding capability flag
- [ ] All 12 signal types have at least one reject-case test for `can_accept`
- [ ] No logic that belongs in `hares-core` is added here (routing stays out of this crate)

## Notes
- Canonical field names and types for all `ControlSignal` variants are defined in HARES-004. When there is any ambiguity, HARES-004 is the source of truth — for example, `SelfConsumption::solar_only_charging` is `bool`, not `Option<bool>`, as listed there.
- HARES-004 supersedes `docs/architecture/03-control-interfaces.md` for the canonical variant list. The architecture doc lists only 10 variants; HARES-004 defines the full set of 12 (including `HumiditySetpoint`).

## Verification
- [ ] `cargo check -p hares-control` passes
- [ ] `cargo test -p hares-control` passes
- [ ] `cargo clippy -p hares-control -- -D warnings` passes
