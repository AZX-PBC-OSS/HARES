# Combi Boiler + Indirect Tank Configuration Unsupported

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-io/hpxml/resolve_hvac, hares-equipment/water_heater

## Problem

HPXML `WaterHeatingType = "space-heating boiler with storage tank"` (combi boiler + indirect tank) is not implemented. The resolver emits a parse error rather than producing a working configuration. Combi boilers with indirect tanks are a common European and Northeast US residential configuration; rejecting the input silently restricts HARES's coverage of the HPXML spec.

## Current Behavior

`crates/hares-io/src/hpxml/resolve_hvac.rs` (water heating section): `WaterHeatingType = "space-heating boiler with storage tank"` triggers `HpxmlError::UnsupportedConfiguration` with a parse-time error.

## Required Behavior

Choose one:

A. **Implement the configuration** — model the combi boiler as the existing boiler equipment with a coupled indirect tank (no internal heat source; receives heat from the boiler heat exchanger). The tank temperature drives the boiler's space-heating demand modifier.

B. **Document as unsupported** — keep the parse error but document the limitation in HPXML import documentation, and provide a clear error message pointing the user to a workaround (e.g. configure a separate boiler and tank as independent equipment).

Recommended path: A (implement) for spec coverage. The combi-boiler + indirect-tank topology is well-defined in ASHRAE HVAC Systems and Equipment 2020 Ch. 50 "Service Water Heating".

## Approach

If A:

1. Define a new `IndirectTankConfig` typed config with fields: tank volume, U-value, heat-exchanger UA (boiler side), setpoint, deadband.
2. Add an `IndirectTank` equipment type that consumes a coupled-boiler reference. The tank pulls heat from the boiler's secondary loop when its temperature drops below setpoint.
3. The boiler's space-heating demand is augmented by the tank's heat draw (the boiler now serves two loads: space heating + DHW).
4. Wire `WaterHeatingType = "space-heating boiler with storage tank"` to construct the `IndirectTank` and reference the existing space-heating boiler.
5. Add fixtures and tests covering combi-boiler operation under simultaneous space-heating and DHW demand.

If B:

1. Replace the parse error with a clearer message: "Combi boiler with indirect tank not currently supported. Workaround: configure separate boiler and tank as independent equipment. See ticket 121 for tracking."
2. Update HPXML import documentation.

## Definition of Done

- [ ] Decision made between path A (implement) and path B (document)
- [ ] If A: `IndirectTank` equipment type implemented with coupled-boiler heat exchange
- [ ] If A: HPXML resolver constructs the combi configuration correctly
- [ ] If A: Tests cover simultaneous space-heating + DHW operation
- [ ] If B: Clear error message and documentation update
- [ ] Either way: no silent acceptance of the configuration with wrong behaviour

## Verification

```bash
cargo test -p hares-io resolve_hvac combi
cargo test -p hares-equipment water_heater
cargo test -p hares-io hpxml_parity
```

## References

- HPXML Specification v4.x §8.5 "Water Heating Systems" — `WaterHeatingSystemType = "space-heating boiler with storage tank"`.
- ASHRAE Handbook HVAC Systems and Equipment 2020 Ch. 50 "Service Water Heating" — combi-boiler topology and indirect-tank heat exchanger sizing.
- ANSI/RESNET/ICC 301-2022 §4.2 — combi boiler simulation requirements.

## Related Tickets

- 079-propane-oil-furnace-boiler-unsupported-parse-error
- 074-hpwh-zone-heat-category-mismatch

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced file exists: `crates/hares-io/src/hpxml/resolve_water_heater.rs`
- [x] **The bug is real**: `canonical_water_heater_name()` at lines 488–511 handles exactly five type/fuel combinations via explicit match arms. `"space-heating boiler with storage tank"` falls through to the wildcard `_` arm and returns `HpxmlError::Parse(format!("unsupported HPXML water heater type/fuel combination: WaterHeaterType='{ty}', fuel='{fuel:?}'"))`. This is a generic parse error, not a clear "unsupported configuration" variant.
- [x] **Ticket's description of the error is partially inaccurate**: The ticket says the error is `HpxmlError::UnsupportedConfiguration`. That variant **does not exist** in the codebase. The actual enum at `crates/hares-io/src/hpxml/mod.rs` has variants: `Io`, `Parse`, `SchemaValidation`, `DomainValidation`, `MissingField`. The resolver uses `HpxmlError::Parse(String)` for this case.
- [x] **Code location note**: The ticket cites `resolve_hvac.rs` but the water heater type is resolved in `resolve_water_heater.rs` (separate file). The `canonical_water_heater_name` function is at line 488 and the fallthrough error at lines 503–508.
- [x] **OCHRE cross-check**: OCHRE (`vendors/OCHRE/ochre/utils/hpxml.py`, function `parse_water_heater`, lines 1013–1129) handles three types: `"instantaneous water heater"`, `"heat pump water heater"`, and `"storage water heater"`. It raises `OCHREException("Unknown water heater type: ...")` for `"space-heating boiler with storage tank"`. **HARES matches OCHRE** — both reject the type, but neither implements it. OCHRE includes HPXML sample fixtures (`test/OS-HPXML Sample Files/base-dhw-indirect.xml`, `base-dhw-combi-tankless.xml`, etc.) with this type but does not parse them. The divergence is not intentional correction of an OCHRE limitation — it is a shared gap.
- [x] **EnergyPlus cross-check**: EnergyPlus models indirect tanks via `WaterHeater:Mixed` with `Source Side Inlet/Outlet Node` connections and a `Source Side Flow Control Mode` of `IndirectHeat`. The EnergyPlus 9.4 Engineering Reference states: *"When the water thermal tank is connected on the demand side of a plant loop (e.g. as for indirect water heating with a boiler)..."* (Big Ladder Software, epx/docs/9-4). EnergyPlus implements the indirect tank model; HARES has no equivalent. OpenStudio-HPXML translates HPXML combi boiler types to this EnergyPlus object and documents the `RelatedHVACSystem` pointer requirement, `StandbyLoss` field, and AHRI-based defaults.

### Web-Verified Citations

**Citation 1**:
- **Citation**: HPXML Specification v4.x §8.5 — `WaterHeatingSystemType = "space-heating boiler with storage tank"`
- **Source found**: HPXML Data Dictionary v4.0.0, fetched from `https://hpxml.nlr.gov/datadictionary/4.0.0/Building/BuildingDetails/Systems/WaterHeating/WaterHeatingSystem/WaterHeaterType`
- **Quoted passage**: The data dictionary lists seven enumerated values for `WaterHeaterType`:
  1. space-heating boiler with tankless coil
  2. **space-heating boiler with storage tank**
  3. split heat pump water heater
  4. heat pump water heater
  5. instantaneous water heater
  6. dedicated boiler with storage tank
  7. storage water heater
- **Verdict**: **Confirmed**. The type is a valid HPXML v4 enumeration. Minor note: the ticket says "§8.5" but the data dictionary does not expose section numbers — this is a plausible but unverifiable section reference.

**Citation 2**:
- **Citation**: ASHRAE Handbook HVAC Systems and Equipment 2020 Ch. 50 "Service Water Heating" — combi-boiler topology and indirect-tank heat exchanger sizing.
- **Source found**: 2020 ASHRAE Handbook—HVAC Systems and Equipment table of contents, fetched from `https://ashraepyramids.org/page.php?id=232`
- **Quoted passage**: Chapter 50 in the 2020 HVAC Systems and Equipment handbook is titled **"Thermal Storage (TC 6.9, Thermal Storage)"**, not "Service Water Heating". Service Water Heating is a chapter in the **ASHRAE Handbook—HVAC Applications** volume, not the HVAC Systems and Equipment volume. In the 2015 ASHRAE HVAC Applications handbook, Chapter 50 was "Service Water Heating" (confirmed at `handbook.ashrae.org/Handbooks/A15/IP/a15_ch50/`); in the 2023 edition it is Chapter 51. The 2020 HVAC Applications handbook uses a similar numbering (approximately Chapter 50).
- **Verdict**: **Incorrect**. The ticket cites the wrong handbook volume ("HVAC Systems and Equipment" instead of "HVAC Applications") and the chapter number is plausible only for the Applications volume. The content about combi-boiler topology **does** exist in the ASHRAE HVAC Applications "Service Water Heating" chapter (confirmed: *"A combination system (combo or combi system) provides hot water for both space heating and domestic use."*), but the citation as written — "HVAC Systems and Equipment 2020 Ch. 50" — is wrong.

**Citation 3**:
- **Citation**: ANSI/RESNET/ICC 301-2022 §4.2 — combi boiler simulation requirements.
- **Source found**: `https://codes.iccsafe.org/content/RESNET3012022P1/chapter-4-energy-rating-calculation-procedures` and `https://www.resnet.us/wp-content/uploads/ANSIRESNETICC301-2022_resnetpblshd.pdf`
- **Quoted passage**: The ICC Digital Codes page for Chapter 4 is paywalled (Digital Codes Premium required). The RESNET PDF is image-based and not machine-readable. Section 4.2 content could not be independently extracted and quoted.
- **Verdict**: **Cannot Verify**. The standard exists and §4.2 covers "Energy Rating Calculation Procedures" for rated homes including water heating. Whether §4.2 specifically mentions combi-boiler simulation requirements cannot be confirmed without paid access to the full text. This citation should be considered unverified until someone with access checks it.

### Legitimacy

- **Verdict**: **Partially Legitimate**

- **Rationale**: The core issue is real and confirmed: `"space-heating boiler with storage tank"` is a valid HPXML v4 `WaterHeaterType` (verified from the live NREL data dictionary), but HARES's `canonical_water_heater_name()` (line 503–508 of `resolve_water_heater.rs`) falls through to an error arm and rejects it with a generic parse error. OCHRE has the same gap — neither tool implements this type — so this is not an accidental regression unique to HARES but a known shared limitation. Three issues weaken the ticket: (1) the error variant name is wrong (`HpxmlError::UnsupportedConfiguration` does not exist; the actual error is `HpxmlError::Parse`); (2) the file cited is `resolve_hvac.rs` but the bug is in `resolve_water_heater.rs`; (3) the ASHRAE citation points to the wrong handbook volume ("HVAC Systems and Equipment 2020 Ch. 50" should be "HVAC Applications, Ch. ~50, 'Service Water Heating'"). The functional claim stands; the supporting detail needs correction.

### Proposed Fix Summary

**Path B (minimum viable)**: In `canonical_water_heater_name()`, add explicit match arms for `"space-heating boiler with storage tank"` and `"space-heating boiler with tankless coil"` that return a structured, actionable error message (e.g. `HpxmlError::Parse("combi boiler + indirect tank not yet supported — see ticket 121. Workaround: configure a separate boiler and tank as independent equipment.")`) instead of the current generic "unsupported type" message. This satisfies the "no silent acceptance with wrong behaviour" requirement without implementing the equipment model.

**Path A (full implementation)**: Add `IndirectTankConfig` and an `IndirectTank` equipment type in `hares-equipment`. Wire `"space-heating boiler with storage tank"` in `resolve_water_heater.rs` to construct an `IndirectTank` referencing the HPXML `RelatedHVACSystem` boiler. The boiler's total load becomes space-heating demand plus DHW draw through the heat exchanger UA. Model mirrors EnergyPlus's `WaterHeater:Mixed` with `Source Side` plant loop connections (`IndirectHeat` mode).

Do **not** implement either path here — this is an audit only.

### Test Written

- **File**: `crates/hares-io/src/hpxml/resolve_water_heater.rs` (appended to the `#[cfg(test)]` module at the end of the file)
- **What it tests**:
  1. `combi_boiler_with_storage_tank_type_currently_rejects_with_parse_error` — calls `canonical_water_heater_name("space-heating boiler with storage tank", FuelType::Gas)` and asserts it returns `Err` containing "unsupported HPXML water heater type".
  2. `combi_boiler_with_tankless_coil_type_currently_rejects_with_parse_error` — same for the companion `"space-heating boiler with tankless coil"` type.
  3. `combi_boiler_full_xml_round_trip_currently_errors` — feeds a minimal HPXML document with the combi type through `resolve_water_heaters()` end-to-end and asserts the result is `Err`.
- All three tests pass against the current (broken) codebase (`cargo test -p hares-io --lib 'resolve_water_heater::tests::combi'` → 3 passed). They will need to be updated or deleted once ticket 121 is resolved.
