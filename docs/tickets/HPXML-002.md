---
id: HPXML-002
title: Parse battery UsableCapacity, NominalVoltage, and Location
kind: implement
depends_on:
  - HPXML-000
files_to_touch:
  - crates/hares-io/src/hpxml/equipment.rs
references:
  - "HPXML spec: Battery/UsableCapacity, Battery/NominalCapacity, Battery/NominalVoltage, Battery/Location"
verification:
  - cargo build -p hares-io
  - cargo test -p hares-io
  - cargo clippy -p hares-io
---

## Background/Context

HPXML provides both `NominalCapacity` and `UsableCapacity` for batteries. The ratio usable/nominal directly implies SOC bounds (`min_soc`, `max_soc`). HPXML also provides `NominalVoltage` (maps to `v_cell` for series/parallel derivation). Currently only `NominalCapacity` and `RatedPowerOutput` are parsed.

**Wiring status:** `min_soc` ✅, `max_soc` ✅, `v_cell` ✅ — all consumed in `battery.rs`. Battery uses `zone_id` not `location` for zone assignment. This is pure HPXML extraction, no equipment changes needed.

## Work to Do

- [ ] In `resolve_batteries`, extract `UsableCapacity` (kWh) using `child_energy_kwh`
- [ ] When both nominal and usable capacity are present, compute and insert `"min_soc"` and `"max_soc"`: if usable < nominal, set `min_soc = (nominal - usable) / (2 * nominal)` and `max_soc = 1.0 - min_soc` (symmetric bounds) — battery.rs already consumes both
- [ ] Extract `NominalVoltage` and insert as `"nominal_voltage_v"` (pack-level voltage in V; battery.rs can derive `n_series` from this)
- [ ] Extract `Location` and insert as `"location"` (string like "garage", "conditioned space")
- [ ] Add unit test verifying SOC derivation from nominal=13.5 / usable=12.0

## Files to Touch

- `crates/hares-io/src/hpxml/equipment.rs`: Extend `resolve_batteries`

## Measures of Success

- [ ] Battery with NominalCapacity=13.5, UsableCapacity=12.0 produces min_soc≈0.056, max_soc≈0.944
- [ ] Battery with NominalVoltage=48.0 produces v_cell=48.0 or nominal_voltage_v=48.0
- [ ] Battery with Location="garage" produces location="garage"
- [ ] Missing UsableCapacity does not affect existing behavior

## Verification

- [ ] `cargo build -p hares-io` passes
- [ ] `cargo test -p hares-io` passes
- [ ] `cargo clippy -p hares-io` passes
