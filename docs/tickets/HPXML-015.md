---
id: HPXML-015
title: Parse HPWH ducting configuration from HPXML
kind: implement
depends_on:
  - HPXML-004
files_to_touch:
  - crates/hares-io/src/hpxml/equipment.rs
references:
  - "HPXML spec: WaterHeatingSystem/HPWHDucting/SupplyAirSource"
  - "HPXML spec: WaterHeatingSystem/HPWHDucting/ExhaustAirTermination"
verification:
  - cargo build -p hares-io
  - cargo test -p hares-io
  - cargo clippy -p hares-io
---

## Background/Context

Heat pump water heaters draw air from and exhaust to specific locations. HPXML provides `HPWHDucting` with `SupplyAirSource` (where air comes from) and `ExhaustAirTermination` (where exhaust goes). This affects zone interaction — a ducted HPWH drawing outdoor air doesn't cool the conditioned space, while an unducted one in a closet has significant zone cooling effects.

Depends on HPXML-004 which adds the HPWH operating mode infrastructure.

## Work to Do

- [ ] In the HPWH section of `resolve_water_heaters`, extract `HPWHDucting/SupplyAirSource` → `"hpwh_supply_air_source"` param
- [ ] Extract `HPWHDucting/ExhaustAirTermination` → `"hpwh_exhaust_air_termination"` param
- [ ] Add unit test

## Files to Touch

- `crates/hares-io/src/hpxml/equipment.rs`: Extend HPWH resolution

## Measures of Success

- [ ] HPWH with ducting to outside → both params set
- [ ] Missing ducting element → no params inserted
- [ ] Values are forwarded as-is strings for downstream equipment model consumption

## Verification

- [ ] `cargo build -p hares-io` passes
- [ ] `cargo test -p hares-io` passes
- [ ] `cargo clippy -p hares-io` passes
