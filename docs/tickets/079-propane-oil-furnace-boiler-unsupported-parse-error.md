# Propane and Oil Furnaces/Boilers Rejected With Parse Error

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-io

## Problem

`canonical_hvac_heating_name` only handles `FuelType::Gas` and `FuelType::Electric`
for furnace and boiler equipment. Propane (`FuelType::Propane`) and fuel oil
(`FuelType::Oil`) are recognized by `parse_fuel` but cause an immediate hard error
in the heating name resolver. This silently fails all HPXML files with propane or
oil heating — approximately 5–10% of US residential stock per ResStock distributions.

## Evidence

`resolve_hvac.rs:1673–1691`:

```
let name = match (ty, fuel) {
    ("ElectricResistance", FuelType::Electric) => "Electric Baseboard",
    ("Furnace", FuelType::Electric) | ... => "Electric Furnace",
    ("Boiler", FuelType::Electric) => "Electric Boiler",
    ("Furnace", FuelType::Gas) | ... => "Gas Furnace",
    ("Boiler", FuelType::Gas) => "Gas Boiler",
    _ => {
        return Err(HpxmlError::Parse(format!(
            "unsupported HPXML heating system type/fuel combination: ..."
        )));
    }
};
```

No arm covers `("Furnace", FuelType::Propane)`, `("Furnace", FuelType::Oil)`,
`("Boiler", FuelType::Propane)`, or `("Boiler", FuelType::Oil)`.

HPXML 4.x example for propane furnace:

```xml
<HeatingSystem>
  <HeatingSystemType><Furnace/></HeatingSystemType>
  <HeatingSystemFuel>propane</HeatingSystemFuel>
  <AnnualHeatingEfficiency>
    <Units>AFUE</Units>
    <Value>0.80</Value>
  </AnnualHeatingEfficiency>
</HeatingSystem>
```

## OCHRE Cross-check

OCHRE routes propane and oil furnaces to the same `GasFurnace` equipment class,
treating them as combustion heaters with AFUE efficiency. The fuel type is preserved
in the descriptor for emissions accounting. HARES already has `GasFurnaceConfig`
and `GasBoilerConfig` which accept `FuelType` through the fuel field on
`EquipmentDescriptor`. The physics are identical to gas; only the CO2 emission
factor differs.

## Required Behavior

Extend the match arms to treat propane and oil as combustion systems, routing to
the same `GasFurnace` / `GasBoiler` typed config. The equipment descriptor fuel
field carries the actual fuel for emissions accounting.

Add:
```
("Furnace" | "WallFurnace" | "FloorFurnace",
 FuelType::Propane | FuelType::Oil) => "Gas Furnace",
("Boiler", FuelType::Propane | FuelType::Oil) => "Gas Boiler",
```

The `build_spec` call already passes the actual `fuel` value to the descriptor,
so downstream emission factor lookup is not affected.

## Approach

Change location: `resolve_hvac.rs:1673–1689` (`canonical_hvac_heating_name` match block).
Insert the two new arms before the catch-all `_ =>` error arm.

Also verify that `parse_fuel` (`resolve_hvac.rs` or `hares_types`) already handles
`"propane"`, `"fuel oil 1"`, `"fuel oil 2"` (it does per current HPXML fuel-string parsing).
No change to `parse_fuel` is expected.

## Citation

- HPXML 4.x schema §HeatingSystem/HeatingSystemFuel: valid values include
  "natural gas", "propane", "fuel oil 1", "fuel oil 2", "fuel oil 4", "wood",
  "wood pellets", "coal", "kerosene" (hpxml.nrel.gov)
- ResStock exposure: EIA RECS 2020 shows ~5% propane heat, ~4% fuel oil heat
  in US residential housing stock

## Annual kWh Impact Rank

**High** (affected homes fail to simulate). Propane and oil homes exit with a
parse error, producing zero HVAC output. Annual heating energy is entirely missing.

## Definition of Done

- [ ] `("Furnace" | "WallFurnace" | "FloorFurnace", FuelType::Propane | FuelType::Oil)` → "Gas Furnace" arm added at `resolve_hvac.rs:1679–1682`
- [ ] `("Boiler", FuelType::Propane | FuelType::Oil)` → "Gas Boiler" arm added at `resolve_hvac.rs:1682`
- [ ] `parse_fuel` coverage confirmed for "propane", "fuel oil", "fuel oil 1", "fuel oil 2" (existing, no change expected)
- [ ] Test: propane furnace HPXML (`<HeatingSystemFuel>propane</HeatingSystemFuel>`) → parses without error, produces `GasFurnaceConfig`, fuel field is `FuelType::Propane`
- [ ] Test: fuel oil boiler HPXML (`<HeatingSystemFuel>fuel oil 2</HeatingSystemFuel>`) → parses without error, produces `GasBoilerConfig`
- [ ] Wood / coal / wood pellets: confirm these still hit the catch-all error arm with a descriptive message

## Verification

```bash
cargo test -p hares-io -- resolve_hvac::tests
```

Add two test fixtures in `crates/hares-io/tests/fixtures/`: one propane furnace fragment and one fuel-oil-2 boiler fragment, each with a valid AFUE value. Verify `GasFurnaceConfig` and `GasBoilerConfig` are produced respectively.
