---
id: HPXML-014
title: Parse heat pump detailed performance data from HPXML
kind: implement
depends_on:
  - HPXML-008
files_to_touch:
  - crates/hares-io/src/hpxml/equipment.rs
references:
  - "HPXML spec: HeatingDetailedPerformanceData/PerformanceDataPoint"
  - "HPXML spec: CoolingDetailedPerformanceData/PerformanceDataPoint"
  - "Fields: outdoor_temperature, capacity, efficiency_cop, capacity_description (min/nominal/max)"
verification:
  - cargo build -p hares-io
  - cargo test -p hares-io
  - cargo clippy -p hares-io
---

## Background/Context

HPXML 4.0 supports detailed performance data tables for heat pumps: capacity and COP at multiple outdoor (and indoor) temperature points, with "minimum", "nominal", and "maximum" capacity descriptions for variable-speed units. This data can directly populate biquadratic performance curve coefficients or multi-speed capacity/COP tables, replacing generic defaults with manufacturer-specific data.

This depends on HPXML-008 (HeatingCapacity17F) which establishes the capacity-at-temperature infrastructure.

## Work to Do

- [ ] In heat pump resolution, check for `HeatingDetailedPerformanceData` child element
- [ ] Iterate `PerformanceDataPoint` children, extracting: outdoor_temperature (°F→°C), capacity (Btu/hr→W), efficiency_cop, capacity_description
- [ ] Group by capacity_description to build multi-speed tables (min/nominal/max → speed indices)
- [ ] Insert as array params: `"perf_heating_outdoor_temps_c"`, `"perf_heating_capacities_w"`, `"perf_heating_cops"` (parallel arrays)
- [ ] Same for `CoolingDetailedPerformanceData` → `"perf_cooling_*"` arrays, also extracting indoor_wetbulb if present
- [ ] Add unit test with 4-point heating performance data

## Files to Touch

- `crates/hares-io/src/hpxml/equipment.rs`: Add detailed performance data extraction to heat pump resolution

## Measures of Success

- [ ] Heat pump with 4 heating performance data points at 5°F, 17°F, 35°F, 47°F → produces parallel arrays of temps, capacities, and COPs
- [ ] Variable-speed data with min/max descriptions → grouped into speed-level arrays
- [ ] Missing performance data → no arrays inserted, existing behavior preserved
- [ ] Data points are properly converted to SI units

## Verification

- [ ] `cargo build -p hares-io` passes
- [ ] `cargo test -p hares-io` passes
- [ ] `cargo clippy -p hares-io` passes
