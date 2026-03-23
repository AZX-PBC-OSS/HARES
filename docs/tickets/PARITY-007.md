---
id: PARITY-007
title: "V2G / V2H enablement for battery and EV"
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-equipment/src/battery/mod.rs
  - crates/hares-equipment/src/ev/mod.rs
  - crates/hares-types/src/control_signal.rs
references:
  - docs/equipment/ochre-parity-gaps.md (Gap 6)
  - docs/equipment/battery.md (control modes section)
  - docs/equipment/ev.md (V2L section)
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace -- -D warnings
---

## Background/Context

Battery and EV both have V2G/V2H explicitly blocked. Battery already supports discharge (negative power) via PowerSetpoint and SelfConsumption modes but does not declare DEMAND_RESPONSE or POWER_LIMIT capabilities. EV has V2L (vehicle-to-load) implemented but V2G returns an explicit error.

**Target**: Full bidirectional support. Battery should support all control signals including DemandResponse and PowerLimit. EV should support V2G (grid export) in addition to existing V2L.

## Work to Do

### Battery
- [ ] Add `POWER_LIMIT | DEMAND_RESPONSE` to battery `ControlCapabilities`
- [ ] Implement `PowerLimit` handler in `apply_control_unchecked()`:
  - Store `power_limit_kw: Option<f64>` on Battery
  - Apply as ceiling in `clamp_power()`: `min(target, limit)`
- [ ] Implement `DemandResponse` handler:
  - Store `dr_level: DRLevel`, `dr_duration_remaining_s: Option<f64>`
  - Map DR levels to power curtailment (Critical → 50% max discharge, GridEmergency → 0)
  - Auto-revert to Normal when duration expires
- [ ] Add DR state to `update_control()` duration countdown

### EV
- [ ] Remove explicit V2G block at line ~1078
- [ ] Add `v2g_enabled: bool` config flag (default false — opt-in)
- [ ] When `v2g_enabled && power_setpoint < 0`: allow grid discharge up to `v2g_max_discharge_kw`
- [ ] Add V2G-specific SOC reserve: `v2g_soc_reserve` (default 0.3, higher than V2L 0.2)
- [ ] Add `DEMAND_RESPONSE` to EV capabilities when v2g_enabled
- [ ] Implement DR handler for EV: GridEmergency → force discharge to grid at max rate

### Tests
- [ ] Battery: PowerLimit caps discharge; DemandResponse reduces available power; duration auto-revert
- [ ] EV: V2G discharges to grid with SOC reserve; V2G disabled by default; DR triggers discharge

## Files to Touch

- `crates/hares-equipment/src/battery/mod.rs`: PowerLimit + DemandResponse handlers, capability declaration
- `crates/hares-equipment/src/ev/mod.rs`: Remove V2G block, add v2g_enabled, implement grid discharge
- `crates/hares-types/src/control_signal.rs`: No changes needed (DemandResponse already defined)

## Measures of Success

- [ ] Battery accepts PowerLimit and DemandResponse signals without error
- [ ] Battery DR auto-reverts after duration expires
- [ ] EV V2G discharges to grid when enabled and signal received
- [ ] EV V2G respects SOC reserve (never discharges below v2g_soc_reserve)
- [ ] Default behavior unchanged (V2G off by default, battery DR not triggered without signal)

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
