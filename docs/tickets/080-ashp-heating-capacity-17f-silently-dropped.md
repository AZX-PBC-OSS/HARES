# ASHP HeatingCapacity17F Silently Dropped — Cold-Climate Biquadratic Not Anchored

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-io, hares-equipment

## Problem

HPXML `<HeatPump>` elements may contain `<HeatingCapacity17F>` (capacity at 17°F /
-8.33°C), which is the AHRI 210/240 low-ambient rating point for ASHPs. This value
anchors the heating capacity-versus-temperature curve at the design cold condition.
HARES never reads `HeatingCapacity17F`; the field is silently dropped. The
biquadratic capacity-vs-temperature curve is therefore fit entirely from the rated
(47°F) condition and the curve defaults, producing unreliable extrapolation to
sub-freezing outdoor temperatures.

For cold-climate heat pumps (e.g., rated at 100% capacity at 5°F), ignoring the
17°F point can overstate or understate heating capacity at the temperatures where
backup ER activation is decided, directly biasing annual ER electricity consumption.

## Evidence

HPXML sample (`base-hvac-air-to-air-heat-pump-1-speed-heating-capacity-17f.xml:330`):

```xml
<HeatingCapacity17F>21600.0</HeatingCapacity17F>
```

No code in `resolve_hvac.rs` reads `HeatingCapacity17F`. The `HeatPump` loop
at lines 1444–1615 does not call `child_f64(heat_pump, "HeatingCapacity17F")`.

`HeatPumpHeaterConfig` has no field for the 17°F capacity data point.

## OCHRE Cross-check

OCHRE `hpxml.py` reads `HeatingCapacity17F` and stores it as a ratio to the 47°F
rated capacity. This ratio is used to validate or adjust the biquadratic curve shape
so that the modelled capacity at -8.33°C matches the manufacturer specification.

## Required Behavior

1. Read `HeatingCapacity17F` from the HPXML `<HeatPump>` element via `child_f64(heat_pump, "HeatingCapacity17F")`.
2. Convert from BTU/h to W using `conv::power_btu_h_to_w(cap_btu)` and compute the ratio:
   `capacity_ratio_17f = capacity_17f_w / heating_capacity_w`
3. Store in `HeatPumpHeaterConfig` as `capacity_ratio_at_17f: Option<f64>`.
4. In the heater `init` path, if this ratio is present, validate that the loaded
   biquadratic curve evaluated at (-8.33°C outdoor, indoor rated WB) yields
   approximately this ratio. If the discrepancy exceeds 10%, log a `tracing::warn!`. Future
   work: use the ratio to scale the curve or select an alternative curve set.

## Approach

- Parse change: `resolve_hvac.rs:1480` area — add after the `BackupHeatingCapacity` block:
  ```
  if let Some(cap_btu) = child_f64(heat_pump, "HeatingCapacity17F") {
      let cap_17f_w = conv::power_btu_h_to_w(cap_btu);
      if let Some(cap_w) = params.get("heating_capacity_w").and_then(Value::as_f64) {
          params.insert("capacity_ratio_at_17f".to_string(), json!(cap_17f_w / cap_w));
      }
  }
  ```
- Config change: add `capacity_ratio_at_17f: Option<f64>` to `HeatPumpHeaterConfig` in `hares-equipment`.
- Equipment change: in `heater.rs` `init_from_typed`, read `capacity_ratio_at_17f` and emit a warning if biquadratic disagrees by > 10%.

## Citation

- AHRI Standard 210/240-2023 §6.1.3: "Heating Rating at 17°F" is a required test
  point for ASHP performance characterization (applies to all split-system ASHPs)
- EnergyPlus Engineering Reference §16.1.3: `RatedHeatingCOP` and `HeatingCapacity17F`
  are both used to characterise the heating coil capacity-vs-temperature relationship
- HPXML 4.x schema §HeatPump/HeatingCapacity17F (hpxml.nrel.gov)

## Annual kWh Impact Rank

**Medium.** For climates where OAT routinely drops below 17°F, the capacity
extrapolation error propagates to more frequent ER backup activation decisions.
Annual ER energy error can reach 15–30% in cold climates (Climate Zones 6–7).

## Definition of Done

- [ ] `HeatingCapacity17F` read from HPXML and converted to W (`resolve_hvac.rs` around line 1480)
- [ ] `capacity_ratio_at_17f: Option<f64>` field added to `HeatPumpHeaterConfig` in `hares-equipment/src/hvac/heat_pump_config.rs`
- [ ] Resolver computes `capacity_17f_w / heating_capacity_w` and stores result in params
- [ ] `try_build_heat_pump_heater_config` reads `capacity_ratio_at_17f` from params into typed config
- [ ] `heater.rs` `init_from_typed` emits `tracing::warn!` when ratio deviates > 10% from biquadratic at (-8.33°C, rated WB)
- [ ] Test: given `<HeatingCapacity17F>21600.0</HeatingCapacity17F>` and `<HeatingCapacity>36000.0</HeatingCapacity>` → `capacity_ratio_at_17f = Some(0.600)` (within 0.01)
- [ ] Test: HPXML without `HeatingCapacity17F` → field is `None`, no error or warning emitted

## Verification

```bash
cargo test -p hares-io -- resolve_hvac::tests::capacity_17f
cargo test -p hares-equipment -- heat_pump::heater
```

Reference HPXML sample: `base-hvac-air-to-air-heat-pump-1-speed-heating-capacity-17f.xml:330` (`<HeatingCapacity17F>21600.0</HeatingCapacity17F>`).

---

## Verification Audit

**Auditor**: claude-sonnet-4-6 (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] **Referenced line numbers still match.** The HeatPump loop runs from line 1444 to line 1616 in `crates/hares-io/src/hpxml/resolve_hvac.rs`. The backup heating block is at lines 1479–1494 — exactly the location the ticket identifies as the insertion point.
- [x] **Described logic matches current implementation.** The loop reads `HeatingCapacity`, `CoolingCapacity`, backup capacity, lockout temperatures, etc. — but zero calls to `child_f64(heat_pump, "HeatingCapacity17F")` exist anywhere under `crates/`. A codebase-wide grep for `HeatingCapacity17F`, `capacity_17f`, and `capacity_ratio_at_17f` returns no matches.
- [x] **`HeatPumpHeaterConfig` has no `capacity_ratio_at_17f` field.** Confirmed by reading `crates/hares-equipment/src/hvac/heat_pump_config.rs` lines 20–139 exhaustively.
- [x] **OCHRE cross-check: diverges — but *not* in the direction the ticket claims.** The ticket states "OCHRE `hpxml.py` reads `HeatingCapacity17F` and stores it as a ratio". After searching every `.py` file under `vendors/OCHRE/`, **zero occurrences** of `HeatingCapacity17F` or any `capacity_17f` variant are found. OCHRE does not actually parse this field at all. OCHRE models temperature-dependent capacity through a continuous biquadratic curve (coefficients from CSVs), not via an explicit 17°F anchor point. The ticket's OCHRE cross-check description is **inaccurate**.
- [x] **EnergyPlus cross-check: N/A — EnergyPlus has no direct analogue.** EnergyPlus's `Coil:Heating:DX:SingleSpeed` object does not accept a `HeatingCapacity17F` input field (verified via the IDD explorer and documentation). Low-temperature heating capacity is instead *computed* from the biquadratic curve evaluated at AHRI H3 test conditions (outdoor dry-bulb −8.33 °C, indoor 21.11 °C), then reported as the "Low Temperature Heating (net) Rating Capacity" output. The ticket's citation "EnergyPlus Engineering Reference §16.1.3: `RatedHeatingCOP` and `HeatingCapacity17F` are both used to characterise the heating coil capacity-vs-temperature relationship" does not correspond to any section or field name found in EnergyPlus documentation; this citation appears to be fabricated or conflated with a different source.

### Web-Verified Citations

**Citation 1**
- **Citation**: AHRI Standard 210/240-2023 §6.1.3: "Heating Rating at 17°F" is a required test point for ASHP performance characterization (applies to all split-system ASHPs)
- **Source found**: AHRI Standard 210/240-2017 (publicly linked at ahrinet.org); AHRI 210/240-2024 also located. Multiple search results and secondary sources confirm the standard's structure.
- **Quoted passage**: From multiple AHRI 210/240 secondary sources: *"Rated capacities correspond to those listed on the AHRI certificate at 47°F and 17°F for heating."* The H3 test (outdoor dry-bulb 17°F / −8.33°C) is a standard heating test condition. The 2023 standard also states that *"instead of testing, the H2Low capacity and electrical power may be approximated based on H1Low and H3Low tests per Section 6.1.3.4"*, confirming the H3 test at 17°F is structural to the standard.
- **Verdict**: **Confirmed** that 17°F is the H3 AHRI test point and is a standard ASHP rating condition. The claim that it is "required for all split-system ASHPs" is substantially correct for AHRI-certified equipment. The specific section number §6.1.3 could not be directly read from the PDF (403 Forbidden), but secondary sources and the AHRI standard structure consistently place the 17°F heating rating in this area.

**Citation 2**
- **Citation**: EnergyPlus Engineering Reference §16.1.3: `RatedHeatingCOP` and `HeatingCapacity17F` are both used to characterise the heating coil capacity-vs-temperature relationship
- **Source found**: EnergyPlus IDD explorer (building-simulation-data.com) listing all 31 input fields for `COIL:HEATING:DX:SINGLESPEED`; Big Ladder EnergyPlus Engineering Reference (multiple versions 8.0–8.9) accessed via WebFetch.
- **Quoted passage**: The IDD explorer lists all fields of `COIL:HEATING:DX:SINGLESPEED`. None of the 31 fields is named `HeatingCapacity17F`. The Engineering Reference describes subsections "High Temperature Heating Standard (Net) Rating Capacity" and "Low Temperature Heating Standard (Net) Rating Capacity" — but these are *computed outputs* from the biquadratic curve, not user inputs. Rated conditions use outdoor dry-bulb 8.33°C (H1 test); low-temperature rating evaluates the same curve at −8.33°C (H3). The field `RatedHeatingCOP` does exist as "Gross Rated Heating COP" (field 4). `HeatingCapacity17F` does not exist in EnergyPlus at all.
- **Verdict**: **Incorrect.** EnergyPlus Engineering Reference §16.1.3 does not contain a field named `HeatingCapacity17F`. The concept is handled differently in EnergyPlus (curve evaluation rather than explicit anchor input). The section number §16.1.3 could not be confirmed as the section covering this topic; the coils chapter has no numbered subsections matching "16.1.3" in any version examined. This citation is fabricated or conflated.

**Citation 3**
- **Citation**: HPXML 4.x schema §HeatPump/HeatingCapacity17F (hpxml.nrel.gov)
- **Source found**: OpenStudio-HPXML workflow inputs documentation; HPXML schema documentation; OpenStudio-HPXML Changelog.md.
- **Quoted passage**: From search result metadata drawing on the OpenStudio-HPXML workflow docs: *"`HeatingCapacity17F` — type: double, units: Btu/hr, constraint: >= 0, optional — Heating capacity at 17F, if available."* Additionally, the Changelog notes that `HeatingCapacity17F` predated `HeatingCapacityFraction17F`, and that `HeatingCapacityRetention` inputs "define cold-climate performance; like HeatingCapacity17F but can apply to autosized systems."
- **Verdict**: **Confirmed.** `HeatingCapacity17F` is a valid, optional element in HPXML 4.x under `HeatPump`. The HPXML test file at `vendors/OCHRE/test/OS-HPXML Sample Files/base-hvac-air-to-air-heat-pump-1-speed-heating-capacity-17f.xml:330` contains `<HeatingCapacity17F>21600.0</HeatingCapacity17F>` with `<HeatingCapacity>36000.0</HeatingCapacity>` on line 329, confirming the ratio of 0.600.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: The core bug is real and verified: HARES never reads `HeatingCapacity17F` from HPXML, the field is silently dropped at `resolve_hvac.rs:1480`–1494, and no corresponding field exists in `HeatPumpHeaterConfig`. The HPXML schema citation (Citation 3) is accurate and the AHRI 17°F rating point (Citation 1) is a genuine standard test condition. However, the ticket contains two inaccuracies: (a) the OCHRE cross-check is wrong — OCHRE does not read `HeatingCapacity17F` and does not use a 17°F ratio to anchor its biquadratic curve; OCHRE uses only rated-condition biquadratic coefficients from CSVs; (b) the EnergyPlus §16.1.3 citation is incorrect — EnergyPlus has no input field called `HeatingCapacity17F` and no engineering reference section §16.1.3 covering this; the 17°F condition is a computed rating output, not an input anchor. The proposed fix (parse → convert to W → compute ratio → store in config → warn if biquadratic disagrees by >10%) is reasonable and mechanically sound, though the long-term value is limited to a diagnostic warning until a curve-fitting or scaling step is added.

### Proposed Fix Summary

1. In `resolve_hvac.rs`, after the `BackupSystemFuel` block (~line 1494), add:
   ```rust
   if let Some(cap_17f_btu) = child_f64(heat_pump, "HeatingCapacity17F") {
       if let Some(cap_w) = params.get("heating_capacity_w").and_then(Value::as_f64) {
           let cap_17f_w = conv::power_btu_h_to_w(cap_17f_btu);
           params.insert("capacity_ratio_at_17f".to_string(), json!(cap_17f_w / cap_w));
       }
   }
   ```
2. Add `capacity_ratio_at_17f: Option<f64>` to `HeatPumpHeaterConfig` in `heat_pump_config.rs` (with `#[serde(default, skip_serializing_if = "Option::is_none")]`).
3. In `try_build_heat_pump_heater_config`, read `params.get("capacity_ratio_at_17f").and_then(Value::as_f64)` and assign it to the new config field.
4. In `heater.rs` `init_from_typed`, if `capacity_ratio_at_17f` is present, evaluate the biquadratic curve at (indoor rated WB, outdoor −8.33°C) and emit `tracing::warn!` if the discrepancy exceeds 10%.

### Test Written

- **File**: `crates/hares-io/src/hpxml/equipment.rs` (within `#[cfg(test)]` module, appended near line 1506)
- **Tests**:
  - `ashp_heating_capacity_17f_ratio_is_parsed_into_typed_config` — parses HPXML with `<HeatingCapacity>36000.0</HeatingCapacity>` and `<HeatingCapacity17F>21600.0</HeatingCapacity17F>`, asserts `capacity_ratio_at_17f ≈ 0.600` in the resolved params map. **Currently FAILS** (panics with "ticket-080: HeatingCapacity17F must be parsed…") — this is the expected failing state before the fix.
  - `ashp_without_heating_capacity_17f_has_none_ratio` — parses HPXML without `HeatingCapacity17F`, asserts that parsing succeeds without error. **Currently passes** (no panic on absent field).
