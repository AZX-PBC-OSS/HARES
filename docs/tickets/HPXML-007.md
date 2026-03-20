---
id: HPXML-007
title: Parse water heater StandbyLoss and UsageBin from HPXML
kind: implement
depends_on:
  - HPXML-000
files_to_touch:
  - crates/hares-io/src/hpxml/equipment.rs
references:
  - "HPXML spec: WaterHeatingSystem/StandbyLoss (Units + Value)"
  - "HPXML spec: WaterHeatingSystem/UsageBin ('very small', 'low', 'medium', 'high')"
  - "DOE 10 CFR 430 UEF test draw profiles by usage bin"
verification:
  - cargo build -p hares-io
  - cargo test -p hares-io
  - cargo clippy -p hares-io
---

## Background/Context

HPXML provides direct `StandbyLoss` measurements (in %/hr or Btu/hr) which are more accurate than the EF/UEF-derived UA when available. It also provides `UsageBin` which determines the DOE test draw profile used for UEF testing — this affects the correct `avg_water_draw_l_per_day` derivation.

## Work to Do

- [ ] In `resolve_water_heaters`, extract `StandbyLoss/Units` and `StandbyLoss/Value`
- [ ] If Units is "%/hr", convert to UA: `ua_w_per_k = standby_pct_per_hr * tank_volume_liters * 4186.0 / 3600.0 / delta_T` where delta_T is setpoint - ambient (~20°C)
- [ ] If a direct standby loss UA can be derived, prefer it over EF/UEF-derived UA (insert `ua_w_per_k` before the EF/UEF derivation, skip derivation if already set)
- [ ] Extract `UsageBin` and insert as `"usage_bin"` string param
- [ ] Add unit test for standby loss conversion

## Files to Touch

- `crates/hares-io/src/hpxml/equipment.rs`: Extend `resolve_water_heaters`

## Measures of Success

- [ ] Water heater with `<StandbyLoss><Units>%/hr</Units><Value>0.5</Value></StandbyLoss>` produces a reasonable `ua_w_per_k`
- [ ] Direct standby loss takes priority over EF/UEF derivation
- [ ] UsageBin is forwarded as a string param

## Verification

- [ ] `cargo build -p hares-io` passes
- [ ] `cargo test -p hares-io` passes
- [ ] `cargo clippy -p hares-io` passes
