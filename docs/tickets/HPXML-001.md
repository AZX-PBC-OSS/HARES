---
id: HPXML-001
title: Parse dehumidifier setpoint and type from HPXML
kind: implement
depends_on:
  - HPXML-000
files_to_touch:
  - crates/hares-io/src/hpxml/equipment.rs
references:
  - "HPXML spec: Dehumidifier/DehumidistatSetpoint, Dehumidifier/Type"
verification:
  - cargo build -p hares-io
  - cargo test -p hares-io
  - cargo clippy -p hares-io
---

## Background/Context

The HPXML spec provides `DehumidistatSetpoint` (target RH fraction) and `Type` ("portable" or "whole-home") on the `Dehumidifier` element. Neither is currently parsed. The dehumidifier equipment model **already consumes** `target_rh` and `fraction_load_served` — this is pure HPXML extraction, no equipment changes needed.

**Wiring status:** `target_rh` ✅ consumed in `dehumidifier.rs`, `fraction_load_served` / `fraction_dehumidification_load_served` ✅ consumed. `dehumidifier_type` is not consumed — forward it as a passthrough string for future use.

## Work to Do

- [ ] In `resolve_scheduled_loads` or wherever dehumidifiers are resolved in `equipment.rs`, locate the dehumidifier resolution logic
- [ ] Extract `DehumidistatSetpoint` and insert as `"target_rh"` (dehumidifier.rs already accepts this key; value is 0–1 fraction)
- [ ] Extract `Type` and insert as `"dehumidifier_type"` ("portable" or "whole-home")
- [ ] Extract `FractionDehumidificationLoadServed` and insert as `"fraction_dehumidification_load_served"` (dehumidifier.rs already accepts this key)
- [ ] Add a unit test with a minimal HPXML snippet containing a dehumidifier with setpoint + type

## Files to Touch

- `crates/hares-io/src/hpxml/equipment.rs`: Add dehumidifier field extraction

## Measures of Success

- [ ] A dehumidifier with `<DehumidistatSetpoint>0.45</DehumidistatSetpoint>` produces `target_rh: 0.45` in the equipment spec
- [ ] A dehumidifier with `<Type>whole-home</Type>` produces `dehumidifier_type: "whole-home"` in the equipment spec
- [ ] Missing fields fall back gracefully (no panic, no required field)

## Verification

- [ ] `cargo build -p hares-io` passes
- [ ] `cargo test -p hares-io` passes
- [ ] `cargo clippy -p hares-io` passes
