---
id: HARES-036
title: "hares-io — HPXML Equipment Resolution and Input Validation"
kind: implement
depends_on: [HARES-035, HARES-018, HARES-037]
files_to_touch:
  - crates/hares-io/src/hpxml/equipment.rs
  - crates/hares-io/src/hpxml/validation.rs
references:
  - docs/architecture/04-data-ingestion-and-fleet.md
  - docs/architecture/06-input-output.md
  - vendors/OCHRE/ochre/utils/hpxml.py
verification:
  - cargo check -p hares-io
  - cargo test -p hares-io
  - cargo clippy -p hares-io -- -D warnings
---

## Background/Context
OCHRE's equipment resolution layer (in `hpxml.py`) translates declarative HPXML equipment specs into concrete equipment instances with the right names, default parameters, and split configurations. The most complex case is the air-source heat pump (ASHP), which OCHRE splits into separate heater and cooler instances. User-supplied kwargs are merged into HPXML-derived defaults via a recursive `nested_update()`. This ticket replicates that resolution logic in Rust so that `hares-equipment` receives fully-specified, named equipment configs without needing to understand HPXML.

## Work to Do
- [ ] Implement `hpxml/equipment.rs`: equipment resolution
  - [ ] Signature: `resolve_equipment(building: &Building, defaults: &DefaultsStore, overrides: &serde_json::Value) -> Vec<EquipmentSpec>`
  - [ ] Parse HVAC systems: `HeatingSystem`, `CoolingSystem`, `HeatPump` elements; extract fuel type, capacity (kBtu/h), efficiency ratings (SEER2, HSPF2, AFUE, COP)
  - [ ] Parse water heaters: fuel type, capacity (gal), setpoint (°C), EF/UEF, tank volume
  - [ ] Parse PV systems: system capacity (kW), tilt (°), azimuth (°), module type, inverter efficiency
  - [ ] Parse battery storage: capacity (kWh), power (kW), round-trip efficiency
  - [ ] Parse EV charger: charger level (L1/L2), max power (kW)
  - [ ] ASHP/MSHP splitting: parse `HeatPump/HeatPumpType` field value from HPXML. When value is `'air-to-air'`, emit `'ASHP Heater'` / `'ASHP Cooler'`. When value is `'mini-split'`, emit `'MSHP Heater'` / `'MSHP Cooler'`. Reference OCHRE `utils/equipment.py:14-45` `EQUIPMENT_NAMES_BY_TYPE` as the authoritative mapping and `utils/equipment.py:96-100` for the splitting procedure. Do NOT match on HPXML attribute names like `MiniSplit` or `MiniSplitHeatPump`.
  - [ ] OCHRE equipment name registry: map HPXML system types to canonical OCHRE names (e.g. `ElectricResistance` → `"Electric Furnace"`)
  - [ ] ZIP parameter fields: include `zip_params: Option<ZipParameters>` in `EquipmentSpec` — actual ZIP values are populated later during Dwelling construction (HARES-044) when `DefaultsStore` is available
  - [ ] `nested_update` semantics: implement `fn nested_update(base: &mut serde_json::Map<String, serde_json::Value>, overrides: &serde_json::Map<String, serde_json::Value>)` — recursively merge `overrides` into `base`; leaf values in `overrides` replace those in `base`; intermediate objects are merged not replaced. Use `serde_json::Value` everywhere for override representation — it is PyO3-compatible and avoids type mismatch with HARES-044's `HashMap<String, serde_json::Value>`. Replace `toml::Table` in the `resolve_equipment` signature with `serde_json::Value` as well.
  - [ ] Parse appliances, lighting, and miscellaneous loads:
    - [ ] Clothes washer, clothes dryer, dishwasher, refrigerator, freezer, cooking range → ScheduledLoad equipment specs
    - [ ] Indoor, exterior, garage, basement lighting → ScheduledLoad equipment specs
    - [ ] Plug loads (MELs including TV, well pump) → ScheduledLoad equipment specs
    - [ ] Fuel loads (MGLs) → ScheduledLoad equipment specs with FuelType::Gas
    - [ ] Pool pump, pool heater, spa pump, spa heater → ScheduledLoad equipment specs
    - [ ] Ceiling fan (with seasonal month-multiplier per OCHRE ScheduledLoad.py:38-41) → ScheduledLoad equipment spec
    - [ ] Mechanical ventilation fan (HRV/ERV) → dedicated equipment spec
    - [ ] Reference OCHRE `utils/hpxml.py:1643-1761` for the complete list
  - [ ] Accept all HPXML 4.0 efficiency unit strings: SEER, SEER2, EER, EER2, HSPF, HSPF2, AFUE, Percent, COP. For SEER2→SEER conversion, use factor 1/0.95 (approximate). For HSPF2→HSPF, use factor 1/0.95. Document the conversion in `docs/PHYSICS_DECISIONS.md`.
- [ ] Extend `hpxml/validation.rs`: cross-input validation only
  - [ ] Note: basic EPW per-record range checks (temperature, GHI, pressure, wind speed, dew point, record count) are performed in HARES-033 at parse time. This ticket only adds cross-input checks that require both EPW and HPXML data simultaneously.
  - [ ] EPW location check: confirm EPW site coordinates are within 200 km of the HPXML `Building/Site` lat/lon; warn if exceeded
  - [ ] EPW dew point ≤ dry bulb at every row: this cross-record physical consistency check may also be enforced here if not already covered by HARES-033
  - [ ] EPW time gap check: no gap > 1 h between consecutive records; error with gap location
  - [ ] Schedule CSV required columns: all expected columns present; error listing missing names
  - [ ] Schedule CSV no-NaN check: error identifying column and row index of any NaN
  - [ ] Schedule temporal coverage: schedule range fully covers simulation period; error with gap interval

## Files to Touch
- `crates/hares-io/src/hpxml/equipment.rs`: new file — equipment parsing, ASHP splitting, name registry, nested_update
- `crates/hares-io/src/hpxml/validation.rs`: extend with cross-input EPW and schedule validation

## Measures of Success
- [ ] Parsing 10 ResStock HPXML fixtures produces equipment property parity with OCHRE `load_hpxml` output (name, capacity, efficiency, fuel type)
- [ ] An ASHP `HeatPump` element (type `air-to-air`) produces exactly two `EquipmentSpec` entries named `"ASHP Heater"` and `"ASHP Cooler"`
- [ ] A mini-split `HeatPump` element (type `mini-split`) produces exactly two `EquipmentSpec` entries named `"MSHP Heater"` and `"MSHP Cooler"`
- [ ] `nested_update` with a nested override merges leaf values without clobbering sibling keys
- [ ] Parsing a ResStock HPXML fixture produces equipment specs for all appliance, lighting, MEL/MGL, and ventilation categories present in the file
- [ ] EPW coords 250 km from HPXML site lat/lon triggers a warning

## Verification
- [ ] `cargo check -p hares-io` passes
- [ ] `cargo test -p hares-io` passes
- [ ] `cargo clippy -p hares-io -- -D warnings` passes
