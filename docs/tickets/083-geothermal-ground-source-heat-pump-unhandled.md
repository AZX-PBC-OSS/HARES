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

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation
- [x] Referenced line numbers still match — `resolve_hvac.rs:1451–1455` matches exactly (`let split = match heat_pump_type.as_str() { "air-to-air" => Some(...), "mini-split" => Some(...), _ => None, }`), confirmed by direct read.
- [x] Described logic matches current implementation — the `if let Some((heater_name, cooler_name)) = split` guard at line 1573 is confirmed; any unrecognised type sets `split = None` and silently skips the entire heater/cooler spec block. The building loop continues without emitting any equipment spec.
- [x] OCHRE cross-check result: **DIVERGES from ticket claim — with important correction.** The ticket states OCHRE "routes [ground-to-air] to the same ASHP model with modified default parameters." This is **incorrect**. OCHRE `vendors/OCHRE/docs/source/ModelingApproach.rst:424` explicitly lists "Ground source heat pumps" in its _unsupported technologies_ section, stating: _"errors are used for a feature with a substantial impact (such as a ground source heat pump)"_. Furthermore, `vendors/OCHRE/ochre/utils/equipment.py:14–45` shows `EQUIPMENT_NAMES_BY_TYPE` has no entry for `("ground-to-air", "Electricity")`; `equipment.py:120–122` would raise an `OCHREException` for this combo. HARES's silent-skip behaviour is arguably _worse_ than OCHRE's intended behaviour (an explicit error), but it is at least consistent with OCHRE's current _de-facto_ lack of support.
- [x] EnergyPlus cross-check result: **Partially confirmed — no section number mismatch.** The ticket cites "EnergyPlus Engineering Reference §16.2: Ground Source Heat Pump coil model (distinct from air-source; entering water temperature replaces OAT)." The EnergyPlus Engineering Reference (bigladdersoftware.com/epx/docs/24.1/engineering-reference/) contains dedicated sections for `Coil:Cooling:WaterToAirHeatPump:EquationFit` and `Coil:Heating:WaterToAirHeatPump:EquationFit` (confirmed via table-of-contents fetches from versions 8.0–24.1). Multiple EnergyPlus community sources confirm that these models use entering water temperature as the source-side performance variable in place of outdoor air temperature used by `Coil:Cooling:DX:SingleSpeed`. The section-number claim ("§16.2") could not be independently verified from the fetched HTML (the page structure uses named anchors, not numbered sections), but the substantive claim — that EnergyPlus has a distinct GSHP coil model driven by EWT — is confirmed directionally.

### Web-Verified Citations

**Citation 1:**
- **Citation**: "HPXML 4.x valid values for `HeatPumpType` (hpxml.nrel.gov): air-to-air, mini-split, ground-to-air, water-loop-to-air, water-source"
- **Source found**: `https://hpxml.nlr.gov/datadictionary/4.0.0/Building/BuildingDetails/Systems/HVAC/HVACPlant/HeatPump/HeatPumpType` (fetched directly; hpxml.nrel.gov redirects to hpxml.nlr.gov)
- **Quoted passage**: The HPXML 4.0.0 data dictionary lists 11 valid enumeration values: "room air conditioner with reverse cycle", "packaged terminal heat pump", "variable refrigerant flow", "water-loop-to-air", "ground-to-water", "ground-to-air", "mini-split", "air-to-water", "air-to-air", "water-to-water", "water-to-air".
- **Verdict**: **Partially correct.** The ticket lists five values ("air-to-air", "mini-split", "ground-to-air", "water-loop-to-air", "water-source") as a representative sample. The schema actually has 11 values, and the ticket omits six ("room air conditioner with reverse cycle", "packaged terminal heat pump", "variable refrigerant flow", "ground-to-water", "air-to-water", "water-to-water", "water-to-air") and misnames "water-source" (the schema value is "water-to-air", not "water-source"). The core claim — that "ground-to-air" and "water-loop-to-air" are valid and unhandled — is confirmed.

**Citation 2:**
- **Citation**: "EnergyPlus Engineering Reference §16.2: Ground Source Heat Pump coil model (distinct from air-source; entering water temperature replaces OAT)"
- **Source found**: EnergyPlus Engineering Reference table of contents fetched from `bigladdersoftware.com/epx/docs/24-1/engineering-reference/` (EnergyPlus 24.1) and `bigladdersoftware.com/epx/docs/8-4/engineering-reference/coils.html`. Multiple community sources (unmethours.com, BigLadder docs) confirm the existence of `Coil:Cooling:WaterToAirHeatPump:EquationFit` and `Coil:Heating:WaterToAirHeatPump:EquationFit` objects.
- **Quoted passage**: From `bigladdersoftware.com/epx/docs/8-3/engineering-reference/coils.html` table of contents: the Engineering Reference includes sections titled "Water Source Electric DX Air Cooling Coil", "Water Source Electric Heat Pump DX Air Heating Coil", and "Variable Speed Water to Air Heat Pump (Heating & Cooling)" — confirming a dedicated model exists. The Unmet Hours community source states: "The curves for `Coil:Cooling:WaterToAirHeatPump:EquationFit` are not bi-quadratic … [they] describe the change in total cooling capacity and efficiency at part-load conditions" using entering water temperature as the source-side variable.
- **Verdict**: **Confirmed in substance; section number not independently verified.** The "§16.2" section number could not be confirmed from web fetches (the HTML doc uses named anchors). The fundamental claim — that EnergyPlus has a distinct water-to-air heat pump coil model using entering water temperature — is confirmed.

**Citation 3:**
- **Citation**: "ResStock exposure: ground-source HPs represent ~1–2% of US housing stock"
- **Source found**: `resstock.nlr.gov/datasets` (ResStock 2025 Release 1, fetched directly); `oedi-data-lake.s3.amazonaws.com` ResStock 2024.2 technical documentation (fetched); market research reports (Grand View Research, GM Insights).
- **Quoted passage**: The ResStock 2025 documentation states "improved geothermal heat pump modeling" is included, and geothermal heat pump results are shown in the "Evaluating Geothermal Performance for the U.S. Building Stock" Tableau dashboard — but no housing-stock percentage is stated on the dataset page. Market sources confirm air-source units accounted for 74.83% of 2025 heat-pump revenue while ground-source had a "9.31% CAGR outlook", implying current penetration is well below air-source. The ResStock technical documentation did not contain a specific penetration statistic.
- **Verdict**: **Cannot directly verify the specific 1–2% figure from ResStock data.** The directional claim (GSHPs are a small fraction of US homes) is consistent with available evidence. The exact percentage is plausible but unconfirmed from primary sources.

### Legitimacy
- **Verdict**: **Legitimate** (with one factual correction to the OCHRE cross-check)
- **Rationale**: The core bug is real and confirmed by direct code inspection: `resolve_hvac.rs:1451–1455` uses `_ => None` for any non-ASHP/MSHP heat pump type, and the `if let Some(...)` guard at line 1573 silently drops the entire equipment spec. The HPXML 4.0.0 data dictionary (fetched from `hpxml.nlr.gov`) confirms "ground-to-air" and "water-loop-to-air" are valid schema values — HARES should handle or explicitly reject them. The ticket's OCHRE cross-check description is wrong: OCHRE explicitly classifies ground source heat pumps as *unsupported* and intends to throw an error (OCHRE `ModelingApproach.rst:424`), it does not silently re-route them to an ASHP model. HARES's silent skip is therefore *worse* than even OCHRE's intended behaviour. The proposed fix (replace `_ => None` with an explicit `Err`) is correct: since HARES has no GSHP physics, returning a hard error is the right defensive posture. The EnergyPlus citation is directionally accurate. The ResStock percentage (1–2%) is plausible but unconfirmed from primary data.

### Proposed Fix Summary
In `crates/hares-io/src/hpxml/resolve_hvac.rs:1451–1455`, replace `let split = match ... { ... _ => None }` with `let (heater_name, cooler_name) = match ... { ... other => return Err(HpxmlError::Parse(format!("HeatPump: unsupported HeatPumpType '{other}'; supported types are: air-to-air, mini-split"))) }`. Remove the `Option` wrapper and update the downstream `if let Some((heater_name, cooler_name)) = split` block at line 1573 to use `heater_name`/`cooler_name` directly (no `Option` unwrap needed). **Do not implement a GSHP physics model** — a clear error is the correct deliverable at this stage.

### Test Written
- **File**: `crates/hares-io/src/hpxml/equipment.rs` (within `#[cfg(test)]` module)
- **Tests added**:
  - `heat_pump_ground_to_air_currently_silently_skips_hvac_ticket_083` — proves the silent-skip bug: `ground-to-air` parses without error and emits zero heater/cooler specs. After the fix this must be updated to assert `Err` containing `"unsupported HeatPumpType"`.
  - `heat_pump_water_loop_to_air_currently_silently_skips_hvac_ticket_083` — same for `water-loop-to-air`.
- Both tests pass under current (broken) code: `cargo test -p hares-io --lib heat_pump_ground_to_air` and `cargo test -p hares-io --lib heat_pump_water_loop` both return `ok`.
