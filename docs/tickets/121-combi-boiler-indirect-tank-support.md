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
