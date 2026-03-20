---
id: HPXML-013
title: Parse HVAC distribution duct details from HPXML
kind: implement
depends_on:
  - HPXML-000
files_to_touch:
  - crates/hares-io/src/hpxml/building.rs
  - crates/hares-io/src/hpxml/equipment.rs
references:
  - "HPXML spec: Ducts/DuctSurfaceArea, DuctType (supply/return), DuctEffectiveRValue"
  - "HPXML spec: DuctLeakageMeasurement/DuctType, DuctLeakage/Value, TotalOrToOutside"
  - "HPXML spec: DistributionSystemType/AirDistribution/AirDistributionType"
  - "HPXML spec: DistributionSystemType/HydronicDistribution/HydronicDistributionType"
verification:
  - cargo build -p hares-io
  - cargo test -p hares-io
  - cargo clippy -p hares-io
---

## Background/Context

Duct details are partially parsed in `building.rs` but underutilized in equipment resolution. HPXML provides per-duct surface area, effective R-value, duct type (supply/return), and leakage measurements with supply/return breakdown. These could feed a proper duct conduction+leakage model instead of a single DSE number. The distribution system type (air vs hydronic, and subtypes) determines how heating is delivered.

## Work to Do

- [ ] In `parse_duct_systems` in `building.rs`, extract per-duct: `DuctType` (supply/return), `DuctSurfaceArea`, `DuctEffectiveRValue`, `DuctBuriedInsulationLevel`
- [ ] Extract separate supply and return leakage from `DuctLeakageMeasurement` elements (currently only total leakage fraction extracted)
- [ ] Extract `AirDistributionType` ("regular velocity", "high velocity", "gravity", "fan coil") from the distribution system
- [ ] Extract `HydronicDistributionType` ("baseboard", "radiant floor", "radiant ceiling", "radiator") when present
- [ ] Forward distribution type to equipment specs via params: `"distribution_type"`, `"supply_duct_area_m2"`, `"return_duct_area_m2"`, `"supply_duct_r_value"`, `"return_duct_r_value"`, `"supply_leakage_fraction"`, `"return_leakage_fraction"`
- [ ] Add unit test for duct detail extraction

## Files to Touch

- `crates/hares-io/src/hpxml/building.rs`: Extend `parse_duct_systems` with per-duct detail
- `crates/hares-io/src/hpxml/equipment.rs`: Forward duct details to HVAC equipment params

## Measures of Success

- [ ] HPXML with supply duct (50 ft², R-8) and return duct (25 ft², R-4) produces correct per-duct params in SI
- [ ] Supply leakage 0.04 and return leakage 0.06 are separately captured
- [ ] Distribution type string is forwarded to HVAC equipment specs
- [ ] Existing single-DSE path still works when detailed ducts absent

## Verification

- [ ] `cargo build -p hares-io` passes
- [ ] `cargo test -p hares-io` passes
- [ ] `cargo clippy -p hares-io` passes
