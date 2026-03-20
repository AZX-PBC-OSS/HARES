---
id: HPXML-011
title: Parse ventilation adjusted recovery efficiencies and hours from HPXML
kind: implement
depends_on:
  - HPXML-000
files_to_touch:
  - crates/hares-io/src/hpxml/equipment.rs
references:
  - "HPXML spec: VentilationFan/AdjustedSensibleRecoveryEfficiency"
  - "HPXML spec: VentilationFan/AdjustedTotalRecoveryEfficiency"
  - "HPXML spec: VentilationFan/HoursInOperation"
verification:
  - cargo build -p hares-io
  - cargo test -p hares-io
  - cargo clippy -p hares-io
---

## Background/Context

HPXML provides both raw and adjusted recovery efficiencies for ERV/HRV ventilation fans. The adjusted values account for duct leakage, fan heat, and other installation factors — they're more accurate for energy modeling. `HoursInOperation` indicates intermittent ventilation (default assumption is 24hr continuous).

## Work to Do

- [ ] In `resolve_ventilation`, prefer `AdjustedSensibleRecoveryEfficiency` over `SensibleRecoveryEfficiency` when present
- [ ] Prefer `AdjustedTotalRecoveryEfficiency` over `TotalRecoveryEfficiency` when present
- [ ] Still fall back to unadjusted values when adjusted are absent
- [ ] Extract `HoursInOperation` (hrs/day) and insert as `"hours_in_operation"` param
- [ ] Add unit test for adjusted vs unadjusted fallback

## Files to Touch

- `crates/hares-io/src/hpxml/equipment.rs`: Extend `resolve_ventilation`

## Measures of Success

- [ ] ERV with both adjusted (0.72) and raw (0.80) sensible recovery → uses 0.72
- [ ] ERV with only raw (0.80) → uses 0.80
- [ ] Fan with `<HoursInOperation>8</HoursInOperation>` → `hours_in_operation: 8.0`

## Verification

- [ ] `cargo build -p hares-io` passes
- [ ] `cargo test -p hares-io` passes
- [ ] `cargo clippy -p hares-io` passes
