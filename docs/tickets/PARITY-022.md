---
id: PARITY-022
title: Parse HPXML 4.x DuctLeakageMeasurement from AirDistribution siblings
kind: fix
depends_on: []
files_to_touch:
  - crates/hares-io/src/hpxml/building.rs
references:
  - https://hpxml.nrel.gov/datadictionary/3.0.0/Building/BuildingDetails/Systems/HVAC/HVACDistribution
  - vendors/OCHRE/ochre/utils/hpxml.py (lines 970-1010)
verification:
  - cargo test -p hares-io --test hpxml_parity duct_parameters
  - cargo test -p hares-core --test parity parity_outputs
---

## Background/Context

HPXML 4.x places `<DuctLeakageMeasurement>` as a sibling of `<Ducts>` under
`<AirDistribution>`, not as a child of `<Ducts>`. The current parser only looks
for leakage as a child element (`LeakageFraction`, `DuctLeakage`), so
`DuctSystem.leakage_fraction` is always `None` for standard HPXML 4.x inputs.

Without leakage fraction, ASHRAE 152 DSE calculation uses 0.0 leakage which
inflates DSE (less duct loss than reality). This contributes to the 13% HVAC
energy deviation in the `cz2a_gas_furnace_ac_res_wh` parity fixture.

HPXML 4.x structure:
```xml
<AirDistribution>
  <DuctLeakageMeasurement>
    <DuctType>supply</DuctType>
    <DuctLeakage>
      <Units>Percent</Units>
      <Value>0.06</Value>
    </DuctLeakage>
  </DuctLeakageMeasurement>
  <Ducts>
    <DuctType>supply</DuctType>
    ...
  </Ducts>
</AirDistribution>
```

## Work to Do

- [ ] In `parse_duct_systems`, after collecting `<Ducts>` from `<AirDistribution>`,
      also collect `<DuctLeakageMeasurement>` siblings
- [ ] Match each `DuctLeakageMeasurement` to its corresponding `Ducts` by `DuctType`
- [ ] Parse `DuctLeakage/Units` and `DuctLeakage/Value`:
  - `"Percent"` → divide by 100 to get fraction (OCHRE convention, some files use 0-1 already)
  - `"CFM25"` → store raw value for future ASHRAE 152 conversion (or convert if system CFM known)
- [ ] Set `DuctSystem.leakage_fraction` from parsed value
- [ ] All values stored in SI internally

## Files to Touch

- `crates/hares-io/src/hpxml/building.rs`: `parse_duct_systems()` lines 1476-1566

## Measures of Success

- [ ] BEopt_example.xml ducts have `leakage_fraction = Some(0.06)` (supply) and `Some(0.04)` (return)
- [ ] `compute_duct_dse_params` in resolve_hvac.rs receives nonzero leakage values
- [ ] Parity fixture `cz2a_gas_furnace_ac_res_wh` HVAC energy deviation improves

## Verification

- [ ] `cargo check` passes
- [ ] `cargo test -p hares-io --test hpxml_parity duct_parameters` passes
- [ ] `cargo test -p hares-core --test parity parity_outputs` — gas furnace HVAC deviation decreases
