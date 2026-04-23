# Geothermal / Ground-Source Heat Pump HeatPumpType Unhandled — Silent Skip

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-io

## Problem

HPXML `<HeatPump>/<HeatPumpType>` can be "ground-to-air" (geothermal / GSHP) in
addition to "air-to-air" and "mini-split". The resolver at lines 1451–1455 of
`resolve_hvac.rs` only handles "air-to-air" and "mini-split"; any other type results
in `split = None`, the heater and cooler configs are never built, and no equipment
spec is emitted. The building silently loses its HVAC.

No error or warning is logged for unrecognised `HeatPumpType`.

## Evidence

`resolve_hvac.rs:1451–1455`:

```
let split = match heat_pump_type.as_str() {
    "air-to-air" => Some(("ASHP Heater", "ASHP Cooler")),
    "mini-split" => Some(("MSHP Heater", "MSHP Cooler")),
    _ => None,
};
```

When `split` is `None`, the `if let Some((heater_name, cooler_name)) = split`
block (line 1573) is skipped and the loop continues to the next heat pump element.

HPXML 4.x valid values for `HeatPumpType` (hpxml.nrel.gov):
- "air-to-air"
- "mini-split"
- "ground-to-air" (geothermal)
- "water-loop-to-air"
- "water-source"

## OCHRE Cross-check

OCHRE `hpxml.py` handles "ground-to-air" by routing to the same ASHP model with
modified default parameters (COP from EER/COP rather than HSPF, no defrost model,
no minimum OAT lockout). OCHRE does not have a dedicated GSHP physics model.

## Required Behavior

Replace the `_ => None` arm at `resolve_hvac.rs:1454` with a hard error:

```
_ => {
    return Err(HpxmlError::Parse(format!(
        "HeatPump: unsupported HeatPumpType '{}'; \
         supported types are air-to-air, mini-split",
        heat_pump_type
    )));
}
```

The `if let Some((heater_name, cooler_name)) = split` guard at line 1573 is then
unreachable for the error path (which has already returned). Remove the `Option`
wrapper: change `let split = match ...` to `let (heater_name, cooler_name) = match ...`
and return `Err` directly from each unsupported arm.

## Approach

Change location: `resolve_hvac.rs:1451–1455`.

Before change:
```
let split = match heat_pump_type.as_str() {
    "air-to-air" => Some(("ASHP Heater", "ASHP Cooler")),
    "mini-split" => Some(("MSHP Heater", "MSHP Cooler")),
    _ => None,
};
```

After change:
```
let (heater_name, cooler_name) = match heat_pump_type.as_str() {
    "air-to-air" => ("ASHP Heater", "ASHP Cooler"),
    "mini-split" => ("MSHP Heater", "MSHP Cooler"),
    other => return Err(HpxmlError::Parse(format!(
        "HeatPump: unsupported HeatPumpType '{other}'; \
         supported types are: air-to-air, mini-split"
    ))),
};
```

Update the downstream `if let Some((heater_name, cooler_name)) = split` block at
line 1573 to remove the `Option` unwrap, using `heater_name` and `cooler_name` directly.
Similarly update all other `split`-dependent checks.

## Citation

- HPXML 4.x schema §HeatPump/HeatPumpType: enumeration including "ground-to-air"
  (hpxml.nrel.gov)
- EnergyPlus Engineering Reference §16.2: Ground Source Heat Pump coil model
  (distinct from air-source; entering water temperature replaces OAT)
- ResStock exposure: ground-source HPs represent ~1–2% of US housing stock

## Annual kWh Impact Rank

**Medium** (affected homes lose all HVAC). The silent skip means the building runs
with no space conditioning, producing zero heating/cooling consumption — a 100% error
for the affected ~1–2% of homes.

## Definition of Done

- [ ] `resolve_hvac.rs:1451–1455`: `split` Option removed; unrecognised type returns `Err` immediately
- [ ] The `if let Some(...)` guard at line 1573 removed; `heater_name`/`cooler_name` used directly
- [ ] Test: HPXML with `<HeatPumpType>ground-to-air</HeatPumpType>` → parse returns `Err` containing "unsupported HeatPumpType"
- [ ] Test: HPXML with `<HeatPumpType>water-loop-to-air</HeatPumpType>` → same error pattern
- [ ] Test: HPXML with `<HeatPumpType>air-to-air</HeatPumpType>` → parses successfully, produces ASHP equipment specs

## Verification

```bash
cargo test -p hares-io -- resolve_hvac::tests::heat_pump_type
```

Add test fixtures for `ground-to-air` and `water-loop-to-air` HPXML fragments in `crates/hares-io/tests/fixtures/`. Assert the error message includes the unsupported type string.
