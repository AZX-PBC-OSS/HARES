---
id: HPXML-010
title: Parse airflow and charge defect ratios from HPXML extensions
kind: implement
depends_on:
  - HPXML-000
files_to_touch:
  - crates/hares-io/src/hpxml/equipment.rs
references:
  - "HPXML spec: extension/AirflowDefectRatio (CoolingSystem, HeatPump)"
  - "HPXML spec: extension/ChargeDefectRatio (CoolingSystem, HeatPump)"
  - "ANSI/RESNET/ICC 301 Standard for installation quality adjustments"
verification:
  - cargo build -p hares-io
  - cargo test -p hares-io
  - cargo clippy -p hares-io
---

## Background/Context

Real-world HVAC installations often have airflow or refrigerant charge defects that derate capacity and efficiency. HPXML captures these as `AirflowDefectRatio` (e.g., -0.25 means 25% below design airflow) and `ChargeDefectRatio` (e.g., -0.10 means 10% undercharged). These are used by OpenStudio-HPXML to apply ANSI 301 installation quality adjustments.

**Wiring status:** `airflow_defect_ratio` ✅ already consumed in `hvac/common.rs` (multiplied with `airflow_cfm_per_ton`). `charge_defect_ratio` ❌ NOT consumed — forward as passthrough string for future use. The config key in equipment is `"AirflowDefectRatio"` or `"airflow_defect_ratio"` (both accepted).

## Work to Do

- [ ] In the CoolingSystem and HeatPump extension parsing, extract `AirflowDefectRatio` and insert as `"airflow_defect_ratio"` (equipment already accepts this)
- [ ] Extract `ChargeDefectRatio` and insert as `"charge_defect_ratio"`
- [ ] Also extract for HeatingSystem (airflow defect applies to furnace blowers too)
- [ ] Add unit test

## Files to Touch

- `crates/hares-io/src/hpxml/equipment.rs`: Extend HVAC extension parsing

## Measures of Success

- [ ] CoolingSystem with `<extension><AirflowDefectRatio>-0.25</AirflowDefectRatio><ChargeDefectRatio>-0.10</ChargeDefectRatio></extension>` produces both params
- [ ] Values can be negative (undercharge) or positive (overcharge)
- [ ] Missing defect ratios don't insert params (equipment model uses default of 0.0)

## Verification

- [ ] `cargo build -p hares-io` passes
- [ ] `cargo test -p hares-io` passes
- [ ] `cargo clippy -p hares-io` passes
