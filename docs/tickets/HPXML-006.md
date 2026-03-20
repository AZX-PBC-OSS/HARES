---
id: HPXML-006
title: Parse PV tracking type and system losses from HPXML
kind: implement
depends_on:
  - HPXML-000
files_to_touch:
  - crates/hares-io/src/hpxml/equipment.rs
references:
  - "HPXML spec: PVSystem/Tracking ('fixed', '1-axis', '1-axis backtracked', '2-axis')"
  - "HPXML spec: PVSystem/SystemLossesFraction"
verification:
  - cargo build -p hares-io
  - cargo test -p hares-io
  - cargo clippy -p hares-io
---

## Background/Context

HPXML provides `Tracking` (fixed, 1-axis, 2-axis) and `SystemLossesFraction` for PV systems.

**Wiring status:** `system_losses_fraction` ✅ already consumed in `pv.rs` (applied as `dc_power *= 1.0 - losses`). `tracking` is NOT consumed — forward as passthrough string. This is pure HPXML extraction for system_losses_fraction; tracking is a passthrough for future use.

## Work to Do

- [ ] In `resolve_pv`, extract `SystemLossesFraction` and insert as `"system_losses_fraction"` param
- [ ] Extract `Tracking` and insert as `"tracking"` string param
- [ ] Extract `NumberOfPanels` and insert as `"number_of_panels"` if present (informational, useful for validation)
- [ ] Add unit test verifying extraction

## Files to Touch

- `crates/hares-io/src/hpxml/equipment.rs`: Extend `resolve_pv`

## Measures of Success

- [ ] PV with `<SystemLossesFraction>0.14</SystemLossesFraction>` → `system_losses_fraction: 0.14`
- [ ] PV with `<Tracking>1-axis</Tracking>` → `tracking: "1-axis"`
- [ ] Missing fields don't affect existing behavior

## Verification

- [ ] `cargo build -p hares-io` passes
- [ ] `cargo test -p hares-io` passes
- [ ] `cargo clippy -p hares-io` passes
