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

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match (corrected: ticket says 1540–1544; current code is **1540–1545**, a one-line offset but structurally identical)
- [x] Described logic matches current implementation — confirmed:
  - `resolve_hvac.rs:1540`: `child_f64(heat_pump, "FractionHeatingLoadServed")` (non-canonical, no fallback)
  - `resolve_hvac.rs:1543`: `child_f64(heat_pump, "FractionCoolingLoadServed")` (non-canonical, no fallback)
  - `resolve_hvac.rs:1311–1312`: `HeatingSystem` correctly uses canonical-first with alias fallback
  - `resolve_hvac.rs:1406–1407`: `CoolingSystem` correctly uses canonical-first with alias fallback
- [x] **Bug is confirmed present** — regression test `heat_pump_reads_canonical_fraction_element_names` fails with `left: None / right: Some(0.8)`, proving canonical `FractionHeatLoadServed` is silently dropped
- [x] OCHRE cross-check: **Diverges**. OCHRE `ochre/utils/hpxml.py:848` reads fractions via dynamic construction `hvac.get(f"Fraction{hvac_type[:-3]}LoadServed", 1.0)` — for `hvac_type="Heating"` this resolves to `FractionHeatLoadServed`; for `hvac_type="Cooling"` to `FractionCoolLoadServed`. OCHRE **only** uses the canonical names (no alias fallback). HARES diverges in the wrong direction — it reads only the aliases.
- [x] EnergyPlus cross-check: **N/A** — this is an XML parsing / data-mapping issue, not an algorithmic/physics formula. EnergyPlus uses its own IDD input format; the element name convention is specific to the HPXML schema maintained by hpxmlwg/NREL, not EnergyPlus.

### Web-Verified Citations

**Citation 1**: "HPXML 4.x schema §HVACPlant/HeatPump/FractionHeatLoadServed and FractionCoolLoadServed (hpxml.nrel.gov); confirmed against OS-HPXML sample files."

- **Source found**: OS-HPXML sample file `base-hvac-air-to-air-heat-pump-1-speed.xml` (NREL/OpenStudio-HPXML, GitHub master branch); OS-HPXML `workflow_inputs.rst` documentation (NREL/OpenStudio-HPXML, GitHub master branch)
- **Quoted passage** (from `base-hvac-air-to-air-heat-pump-1-speed.xml`, fetched via WebFetch):
  ```xml
  <FractionHeatLoadServed>1.0</FractionHeatLoadServed>
  <FractionCoolLoadServed>1.0</FractionCoolLoadServed>
  ```
  From `base-hvac-air-to-air-heat-pump-1-speed-heating-capacity-17f.xml` (the exact file cited in the ticket):
  ```xml
  <FractionHeatLoadServed>1.0</FractionHeatLoadServed>
  <FractionCoolLoadServed>1.0</FractionCoolLoadServed>
  ```
  From `workflow_inputs.rst` (fetched via WebFetch):
  > "Currently only supports homes with at most one cooling system (including heat pumps) serving 100% of the cooling load and at most one heating system (including heat pumps) serving 100% of the heating load (i.e., **FractionHeatLoadServed** and **FractionCoolLoadServed** are 1.0)."

  From OS-HPXML documentation search results (multiple sources):
  > "Each heat pump should be entered as a `Systems/HVAC/HVACPlant/HeatPump`, with inputs including `HeatPumpType`, **`FractionHeatLoadServed`**, and **`FractionCoolLoadServed`** that must be provided."

- **Verdict**: **Confirmed** — `FractionHeatLoadServed` and `FractionCoolLoadServed` are the canonical HPXML element names for heat pumps. The non-canonical variants `FractionHeatingLoadServed`/`FractionCoolingLoadServed` do not appear in any OS-HPXML sample file or documentation.

**Note on XSD schema direct verification**: Attempts to fetch the raw HPXML.xsd from `hpxmlwg/hpxml` GitHub returned 404 (path changed); `hpxmlwg.github.io` schema docs did not include the HeatPump type in the truncated content returned. However, the OS-HPXML sample files are definitive evidence of correct element names, and they are fully consistent with the OS-HPXML workflow documentation.

### Legitimacy

- **Verdict**: **Legitimate**
- **Rationale**: The bug is real and confirmed by three independent lines of evidence. (1) Code inspection shows `resolve_hvac.rs:1540–1544` only reads `FractionHeatingLoadServed`/`FractionCoolingLoadServed` with no canonical-name lookup, while the `HeatingSystem` and `CoolingSystem` resolvers immediately above it correctly apply canonical-first + alias-fallback. (2) Two OS-HPXML sample files fetched directly from GitHub (including the exact file cited in the ticket) use only `FractionHeatLoadServed`/`FractionCoolLoadServed`. (3) A failing regression test (`heat_pump_reads_canonical_fraction_element_names`) demonstrates the runtime consequence: both fractions are `None` when canonical names are supplied, meaning the heat pump is modelled as serving 100% of all load regardless of the actual HPXML value. The ticket's line numbers are accurate (off by one line at most due to minor nearby edits), the cited sample file is correct, and the impact assessment (10–20% annual HVAC energy error for partial-load configurations) is plausible given that a fraction of 0.8 would be completely ignored.

### Proposed Fix Summary

In `crates/hares-io/src/hpxml/resolve_hvac.rs` at lines 1540–1545, replace the two single-lookup blocks with canonical-first + alias-fallback chains — exactly mirroring the `HeatingSystem` (lines 1311–1312) and `CoolingSystem` (lines 1406–1407) patterns:

```rust
// Heating fraction
if let Some(frac) = child_f64(heat_pump, "FractionHeatLoadServed")
    .or_else(|| child_f64(heat_pump, "FractionHeatingLoadServed"))
{
    params.insert("fraction_heating_load_served".to_string(), json!(frac));
}
// Cooling fraction
if let Some(frac) = child_f64(heat_pump, "FractionCoolLoadServed")
    .or_else(|| child_f64(heat_pump, "FractionCoolingLoadServed"))
{
    params.insert("fraction_cooling_load_served".to_string(), json!(frac));
}
```

No other production code changes are needed. The downstream consumers at `resolve_hvac.rs:953–958` and `1066–1071` already read `fraction_heating_load_served` and `fraction_cooling_load_served` from the params map correctly.

### Test Written

- **File**: `crates/hares-io/src/hpxml/resolve_hvac.rs` (within `#[cfg(test)] mod tests`)
- **Tests added**:
  - `heat_pump_reads_canonical_fraction_element_names` — supplies `FractionHeatLoadServed=0.8` and `FractionCoolLoadServed=0.7` (canonical names); asserts `heater_cfg.fraction_heating_load_served == Some(0.8)` and `cooler_cfg.fraction_cooling_load_served == Some(0.7)`. **Currently FAILING** (demonstrates the bug).
  - `heat_pump_reads_alias_fraction_element_names_as_fallback` — supplies `FractionHeatingLoadServed=0.6` and `FractionCoolingLoadServed=0.5` (alias names); asserts correct values are parsed. **Currently PASSING** (alias path already works; ensures fallback behaviour is preserved by the fix).
- **Run command**: `cargo test -p hares-io -- tests::heat_pump_reads_canonical_fraction_element_names tests::heat_pump_reads_alias_fraction_element_names_as_fallback`
