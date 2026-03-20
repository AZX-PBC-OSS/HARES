---
id: HPXML-004
title: Parse HPWH operating mode from HPXML
kind: implement
depends_on:
  - HPXML-000
files_to_touch:
  - crates/hares-io/src/hpxml/equipment.rs
references:
  - "HPXML spec: WaterHeatingSystem/HPWHOperatingMode ('hybrid/auto' vs 'heat pump only')"
verification:
  - cargo build -p hares-io
  - cargo test -p hares-io
  - cargo clippy -p hares-io
---

## Background/Context

Heat pump water heaters can operate in "hybrid/auto" mode (compressor + electric backup) or "heat pump only" mode (compressor only, no resistance element). HPXML provides `HPWHOperatingMode` to distinguish these. Currently all HPWHs are treated as hybrid.

**Wiring status:** The equipment model already consumes `"hp_only_mode"` (boolean) in `heat_pump_wh.rs`. This is pure HPXML extraction with correct key mapping.

## Work to Do

- [ ] In `resolve_water_heaters`, when `wh_type` contains "heat pump", extract `HPWHOperatingMode`
- [ ] Insert as `"hpwh_operating_mode"` string param ("hybrid/auto" or "heat pump only")
- [ ] Equipment model (after HPXML-000) accepts this key and derives hp-only behavior internally
- [ ] Add unit test for both modes

## Files to Touch

- `crates/hares-io/src/hpxml/equipment.rs`: Extend HPWH resolution

## Measures of Success

- [ ] HPWH with `<HPWHOperatingMode>heat pump only</HPWHOperatingMode>` → `hpwh_operating_mode: "heat pump only"`
- [ ] HPWH with `<HPWHOperatingMode>hybrid/auto</HPWHOperatingMode>` → `hpwh_operating_mode: "hybrid/auto"`
- [ ] Missing element → no change (backward compatible)

## Verification

- [ ] `cargo build -p hares-io` passes
- [ ] `cargo test -p hares-io` passes
- [ ] `cargo clippy -p hares-io` passes
