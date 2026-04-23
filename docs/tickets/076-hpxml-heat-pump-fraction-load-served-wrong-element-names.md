# HeatPump FractionHeatLoadServed and FractionCoolLoadServed Silently Dropped

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-io

## Problem

The `<HeatPump>` resolver in `resolve_hvac.rs` reads `FractionHeatingLoadServed` and
`FractionCoolingLoadServed` — the non-canonical HPXML 4.x element names. The canonical
names are `FractionHeatLoadServed` and `FractionCoolLoadServed`. Every OS-HPXML sample
file uses the canonical names. When a real HPXML file is processed, both fractions are
silently dropped and default to `None`, causing the equipment model to treat the heat
pump as serving 100% of all load.

HPXML input example (canonical names, from `base-hvac-air-to-air-heat-pump-1-speed-heating-capacity-17f.xml:341–342`):

```xml
<FractionHeatLoadServed>1.0</FractionHeatLoadServed>
<FractionCoolLoadServed>1.0</FractionCoolLoadServed>
```

## Current Behavior

`resolve_hvac.rs:1540–1544`: the HeatPump loop calls `child_f64(heat_pump, "FractionHeatingLoadServed")` and `child_f64(heat_pump, "FractionCoolingLoadServed")`. These element names never match real HPXML files; the values are silently absent.

The `HeatingSystem` and `CoolingSystem` resolvers (`resolve_hvac.rs:1311–1312` and `1406–1407`) correctly try the canonical name first with the non-canonical as fallback. The `HeatPump` resolver has no such dual-lookup.

## Required Behavior

Replace both lookups in the `HeatPump` loop with canonical-first, alias-fallback pattern matching `HeatingSystem` and `CoolingSystem`:

```
child_f64(heat_pump, "FractionHeatLoadServed")
    .or_else(|| child_f64(heat_pump, "FractionHeatingLoadServed"))
```

```
child_f64(heat_pump, "FractionCoolLoadServed")
    .or_else(|| child_f64(heat_pump, "FractionCoolingLoadServed"))
```

**Change location**: `resolve_hvac.rs:1540–1544`

**Citation**: HPXML 4.x schema §HVACPlant/HeatPump/FractionHeatLoadServed and
FractionCoolLoadServed (hpxml.nrel.gov); confirmed against OS-HPXML sample files.

## Annual kWh Impact Rank

**High.** In multi-system homes where a heat pump serves a fraction of load (e.g., 0.8)
alongside a gas furnace, both systems are mis-sized. Annual HVAC energy error reaches
10–20% for such configurations.

## Definition of Done

- [ ] `HeatPump` loop reads `FractionHeatLoadServed` with `FractionHeatingLoadServed` fallback
- [ ] `HeatPump` loop reads `FractionCoolLoadServed` with `FractionCoolingLoadServed` fallback
- [ ] Test: HPXML with canonical element names (`FractionHeatLoadServed`) populates both fractions correctly in the params map
- [ ] Test: HPXML with non-canonical names (`FractionHeatingLoadServed`) also populates both fractions (alias path)

## Verification

```bash
cargo test -p hares-io -- resolve_hvac::tests
```

Test fixture: `crates/hares-io/tests/fixtures/` — add or extend a heat pump HPXML fragment that includes both `<FractionHeatLoadServed>` and `<FractionCoolLoadServed>` canonical elements. A second fixture should use `<FractionHeatingLoadServed>` to exercise the fallback path. Reference HPXML sample: `base-hvac-air-to-air-heat-pump-1-speed-heating-capacity-17f.xml:341–342`.
