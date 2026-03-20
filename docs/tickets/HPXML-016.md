---
id: HPXML-016
title: Parse HVAC design airflow CFM from HPXML extensions
kind: implement
depends_on:
  - HPXML-009
files_to_touch:
  - crates/hares-io/src/hpxml/equipment.rs
references:
  - "HPXML spec: extension/HeatingDesignAirflowCFM"
  - "HPXML spec: extension/CoolingDesignAirflowCFM"
verification:
  - cargo build -p hares-io
  - cargo test -p hares-io
  - cargo clippy -p hares-io
---

## Background/Context

HPXML provides design airflow rates via extensions. These are more accurate than the default 400 CFM/ton assumption currently used to derive fan power and coil physics parameters. When available, design airflow should be used to compute fan power and air-side heat transfer.

Depends on HPXML-009 which adds fan motor type extraction from the same extension block.

## Work to Do

- [ ] In the extension parsing for HeatingSystem, extract `HeatingDesignAirflowCFM` → convert to m³/s (× 0.000_471_947) and insert as `"design_airflow_m3_s"` or keep as CFM in `"design_airflow_cfm"`
- [ ] Same for CoolingSystem and HeatPump: extract `CoolingDesignAirflowCFM`
- [ ] For heat pumps, extract both heating and cooling design airflows
- [ ] When explicit design CFM is present and fan_power_w is not set, derive fan_power from CFM × W/CFM
- [ ] Add unit test

## Files to Touch

- `crates/hares-io/src/hpxml/equipment.rs`: Extend HVAC extension parsing

## Measures of Success

- [ ] HeatingSystem with `<HeatingDesignAirflowCFM>1200</HeatingDesignAirflowCFM>` → param is inserted
- [ ] Fan power derivation uses design CFM when available instead of capacity-based default
- [ ] Missing design airflow doesn't change existing behavior

## Verification

- [ ] `cargo build -p hares-io` passes
- [ ] `cargo test -p hares-io` passes
- [ ] `cargo clippy -p hares-io` passes
