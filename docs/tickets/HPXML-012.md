---
id: HPXML-012
title: Parse heat pump BackupType and defrost-related extensions
kind: implement
depends_on:
  - HPXML-000
files_to_touch:
  - crates/hares-io/src/hpxml/equipment.rs
references:
  - "HPXML spec: HeatPump/BackupType ('integrated', 'separate')"
  - "HPXML spec: extension/BackupHeatingActiveDuringDefrost"
  - "HPXML spec: extension/PanHeaterPowerWatts, extension/PanHeaterControlType"
verification:
  - cargo build -p hares-io
  - cargo test -p hares-io
  - cargo clippy -p hares-io
---

## Background/Context

Heat pump backup type ("integrated" = same unit, "separate" = independent system like a furnace) affects modeling of backup heating. Defrost-related extensions indicate whether backup heating activates during defrost cycles and whether a pan heater (drain pan de-icing) is present. These improve accuracy of heat pump energy use in cold climates.

**Wiring status — pan heater:** ✅ `pan_heater_kw` and `pan_heater_temp_c` are already consumed in `heat_pump/heater.rs` (minisplit heater). HPXML value is in watts — convert W→kW. Equipment also accepts `mshp_pan_heater_kw` variant.

**Wiring status — backup_type, backup_active_during_defrost:** ❌ NOT consumed. Forward as passthrough strings for future use.

## Work to Do

- [ ] In heat pump resolution, extract `BackupType` and insert as `"backup_type"` ("integrated" or "separate")
- [ ] Extract `extension/BackupHeatingActiveDuringDefrost` (boolean) → `"backup_active_during_defrost"` param
- [ ] Extract `extension/PanHeaterPowerWatts` → insert as `"pan_heater_w"` (value in watts; equipment accepts watts after HPXML-000)
- [ ] Extract `extension/PanHeaterControlType` → `"pan_heater_control"` param
- [ ] Add unit test

## Files to Touch

- `crates/hares-io/src/hpxml/equipment.rs`: Extend heat pump resolution

## Measures of Success

- [ ] Heat pump with `<BackupType>separate</BackupType>` → `backup_type: "separate"`
- [ ] Heat pump with pan heater 50W in "defrost mode" → both params inserted
- [ ] Missing elements don't affect existing behavior

## Verification

- [ ] `cargo build -p hares-io` passes
- [ ] `cargo test -p hares-io` passes
- [ ] `cargo clippy -p hares-io` passes
