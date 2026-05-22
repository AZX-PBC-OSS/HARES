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

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match — `canonical_hvac_heating_name` is at lines **1665–1691** (ticket cited 1673–1691 for the match block, which is the inner match; the function itself starts at 1665). Functionally correct.
- [x] Described logic matches current implementation — the match block has no arm for `FuelType::Propane` or `FuelType::Oil` for Furnace/WallFurnace/FloorFurnace/Boiler; all such combinations fall through to the `_ =>` error arm returning `HpxmlError::Parse(...)`.
- [x] OCHRE cross-check: **matches OCHRE intent but OCHRE routes before the name resolver**. OCHRE `ochre/utils/equipment.py` lines 117–119 contain `if eq_fuel not in ["Electricity", "Natural gas", None]: print(f"WARNING: Converting {eq_fuel} to natural gas for {end_use}."); eq_fuel = "Natural gas"`. OCHRE normalises propane/oil to `"Natural gas"` *before* looking up the equipment-class name, so it never hits the equivalent of HARES's `canonical_hvac_heating_name`. The fix in the ticket (adding explicit arms that map Propane/Oil to "Gas Furnace"/"Gas Boiler") achieves the same end result.
- [x] EnergyPlus cross-check: N/A — this is a HPXML-to-equipment-name mapping issue, not an EnergyPlus physics algorithm. EnergyPlus itself accepts propane and fuel oil for furnaces; see EnergyPlus I/O Reference §Fuel Equipment objects which lists "NaturalGas", "PropaneGas", "FuelOil#1", "FuelOil#2" as valid fuel inputs.

**Additional bug found (not in ticket):** `parse_fuel` at `crates/hares-io/src/hpxml/xml_helpers.rs:15` only matches `"oil" | "fuel oil" | "fuel_oil"`. The HPXML schema (and `normalize_ascii`) would produce `"fuel oil 1"`, `"fuel oil 2"`, `"fuel oil 4"`, `"fuel oil 5/6"` as normalized strings. These do **not** match any arm and fall through to the warn-and-default-to-Electric path. Consequence: a "fuel oil 2" boiler silently becomes `FuelType::Electric`, is routed to `("Boiler", FuelType::Electric)` → "Electric Boiler", and the actual fuel is lost with no error. This is confirmed by the regression test `fuel_oil_2_boiler_resolves_to_gas_boiler_config_with_oil_fuel` (see § Test Written), which fails because `specs` contains "Electric Boiler" rather than "Gas Boiler". The ticket's Definition of Done item "parse_fuel coverage confirmed for 'fuel oil', 'fuel oil 1', 'fuel oil 2' (existing, no change expected)" is **incorrect** — a change to `parse_fuel` is required.

### Web-Verified Citations

**Citation 1**: HPXML 4.x schema §HeatingSystem/HeatingSystemFuel: valid values include "natural gas", "propane", "fuel oil 1", "fuel oil 2", "fuel oil 4", "wood", "wood pellets", "coal", "kerosene" (hpxml.nrel.gov)

- **Source found**: OpenStudio-HPXML Workflow Inputs documentation (https://openstudio-hpxml.readthedocs.io/en/latest/workflow_inputs.html); NREL OpenStudio-HPXML PDF v1.2.0 (https://openstudio-hpxml.readthedocs.io/_/downloads/en/v1.2.0/pdf/)
- **Quoted passage**: Multiple search queries returned the consistent enumeration: "HeatingSystemFuel choices are 'electricity', 'natural gas', 'fuel oil', 'fuel oil 1', 'fuel oil 2', 'fuel oil 4', 'fuel oil 5/6', 'diesel', 'propane', 'kerosene', 'coal', 'coke', 'bituminous coal', 'wood', or 'wood pellets'." The NREL HPXML toolbox and OpenStudio-HPXML documentation both confirm this.
- **Verdict**: **Confirmed** — "propane", "fuel oil 1", "fuel oil 2", and "fuel oil 4" are all valid HPXML 4.x HeatingSystemFuel values.

**Citation 2**: EIA RECS 2020 shows ~5% propane heat, ~4% fuel oil heat in US residential housing stock

- **Source found**: EIA RECS 2020 data (https://www.eia.gov/consumption/residential/data/2020/); EIA Today in Energy "Beyond natural gas and electricity; more than 10% of U.S. homes use heating oil or propane" (https://www.eia.gov/todayinenergy/detail.php?id=4070); National Propane Gas Association analysis of 2020 RECS (https://www.npga.org/news-resources/2020-residential-energy-consumption-survey-recs/)
- **Quoted passage**: EIA 2020 RECS via NPGA analysis: "4.2% of U.S. households use propane as their primary space heating fuel." EIA Today in Energy (id=4070): "more than 10% of U.S. homes use heating oil or propane" and "propane space heating has broader geographic distribution than heating oil, heating between 3% and 8% of households in every region." A separate EIA search query returned: "Home heating oil and propane account for another 5 million households (4 percent) each" for fuel oil/heating oil.
- **Verdict**: **Confirmed** — combined propane (~4%) and fuel oil (~4%) represent approximately 8–9% of US residential space heating, consistent with the ticket's "5–10%" figure. The specific "~5% propane heat, ~4% fuel oil heat" in the ticket are slightly off (propane is ~4%, not ~5%) but the order of magnitude and the claim that this is a significant fraction of the housing stock is accurate.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: The core bug is real and confirmed by live test execution: `canonical_hvac_heating_name("Furnace", FuelType::Propane)` returns `Err(HpxmlError::Parse(...))` with the exact message quoted in the ticket. The propane furnace test fails at line 1008 with `Parse("unsupported HPXML heating system type/fuel combination: HeatingSystemType='Furnace', fuel='Propane'")`. OCHRE cross-check confirms the intended behavior is to route propane/oil combustion heaters to the Gas Furnace / Gas Boiler equipment class. HPXML schema confirms propane and fuel oil are valid schema values. The EIA RECS data confirms significant real-world exposure. However, the ticket contains one **incorrect claim** in the Definition of Done: it asserts `parse_fuel` already handles "fuel oil 1" and "fuel oil 2" with no change expected. In fact, `parse_fuel` (`xml_helpers.rs:15`) only matches `"oil" | "fuel oil" | "fuel_oil"` and will silently misclassify "fuel oil 2" as `FuelType::Electric` (via the warn-and-fallback path), causing it to resolve as an Electric Boiler instead of a Gas Boiler. The fix must therefore also extend `parse_fuel` to handle `"fuel oil 1"`, `"fuel oil 2"`, `"fuel oil 4"`, and `"fuel oil 5/6"`.

### Proposed Fix Summary

Two changes are required (do not implement — audit only):

1. **`crates/hares-io/src/hpxml/xml_helpers.rs`, `parse_fuel` function**: Extend the `"oil" | "fuel oil" | "fuel_oil"` arm to also match `"fuel oil 1"`, `"fuel oil 2"`, `"fuel oil 4"`, `"fuel oil 5/6"`, `"kerosene"`, and `"diesel"` (all valid HPXML HeatingSystemFuel values that are combustion-equivalent to oil/propane).

2. **`crates/hares-io/src/hpxml/resolve_hvac.rs:1673–1689`, `canonical_hvac_heating_name` match block**: Insert two new arms before the catch-all `_ =>` error arm:
   ```rust
   ("Furnace" | "WallFurnace" | "FloorFurnace",
    FuelType::Propane | FuelType::Oil) => "Gas Furnace",
   ("Boiler", FuelType::Propane | FuelType::Oil) => "Gas Boiler",
   ```

### Test Written

- **File**: `crates/hares-io/tests/hpxml_parity.rs` (end of file, lines 984–1075)
- **Tests**:
  - `propane_furnace_resolves_to_gas_furnace_config_with_propane_fuel` — parses a propane furnace HPXML fragment and asserts it resolves to a "Gas Furnace" spec with `fuel_type == FuelType::Propane`. Currently **FAILS** with `Parse("unsupported HPXML heating system type/fuel combination: HeatingSystemType='Furnace', fuel='Propane'")`.
  - `fuel_oil_2_boiler_resolves_to_gas_boiler_config_with_oil_fuel` — parses a fuel oil 2 boiler HPXML fragment and asserts it resolves to a "Gas Boiler" spec with `fuel_type == FuelType::Oil`. Currently **FAILS** because "fuel oil 2" silently maps to `FuelType::Electric` in `parse_fuel`, producing an "Electric Boiler" instead.
- **Run**: `cargo test -p hares-io --test hpxml_parity -- propane_furnace fuel_oil_2`
