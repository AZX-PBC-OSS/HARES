# Boiler `flow_rate_kg_s` and `return_temp_c` Silent Defaults

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-io/hpxml/resolve_hvac

## Problem

`crates/hares-io/src/hpxml/resolve_hvac.rs:639,643,680,684` boiler resolution uses silent defaults:
- `flow_rate_kg_s = .unwrap_or(0.5)`
- `return_temp_c = .unwrap_or(40.0)`

Both are reasonable order-of-magnitude values for residential hydronic systems, but neither is cited and both are silently substituted when the HPXML file omits the data. Boiler part-load efficiency depends sensitively on return water temperature (condensing boilers especially: efficiency drops 5-10% as return temp crosses the dewpoint of the flue gas). A wrong return_temp default biases annual heating energy.

## Current Behavior

`crates/hares-io/src/hpxml/resolve_hvac.rs:639,643,680,684`:
```rust
let flow_rate_kg_s = params.get("flow_rate_kg_s").copied().unwrap_or(0.5);
let return_temp_c = params.get("return_temp_c").copied().unwrap_or(40.0);
```

No diagnostic on the substitution. No citation for the default values.

## Required Behavior

Choose one for each:

A. **Cite and log** — keep the default but add inline citation (e.g. ASHRAE HoF 2021 Ch. 36 "Hydronic Heating and Cooling" Table 5 — typical residential flow rate and return temperatures) and emit `tracing::debug!` when the default is taken.

B. **Error loudly** — return `HpxmlError::MissingField` when the value is absent; require the input to supply both.

Recommended path: A with citations, since residential HPXML files commonly omit hydronic loop details and the user expects a sensible default. The debug log must include the equipment ID and the substituted value.

## Approach

1. Open `resolve_hvac.rs` at the four cited line numbers.
2. For each silent default:
   - Add `tracing::debug!(equipment_id = ..., field = "flow_rate_kg_s", value = 0.5, "boiler flow rate not specified; using ASHRAE typical default");` (analogous for return_temp_c).
   - Add an inline comment citing ASHRAE HoF 2021 Ch. 36 Table 5 (or the appropriate primary source — verify chapter/table numbers).
3. Add a unit test asserting the debug message fires and the substituted value is consumed.
4. Document the defaults in the boiler config doc-comment so users know what is being assumed.

## Definition of Done

- [ ] `flow_rate_kg_s` default 0.5 cited inline at `resolve_hvac.rs:639,680`
- [ ] `return_temp_c` default 40.0 cited inline at `resolve_hvac.rs:643,684`
- [ ] `tracing::debug!` emitted when each default is taken
- [ ] Unit test asserts debug message and substituted value
- [ ] Boiler config doc-comment documents the defaults

## Verification

```bash
cargo test -p hares-io resolve_hvac boiler
cargo test -p hares-io hpxml_parity
```

## References

- ASHRAE Handbook of Fundamentals 2021 Ch. 36 "Hydronic Heating and Cooling" — typical residential boiler flow rates and return temperatures (verify table numbers).
- ASHRAE Handbook HVAC Systems and Equipment 2020 Ch. 32 "Boilers" — condensing boiler efficiency vs return water temperature.
- HPXML Specification v4.x §8.4 "Heating Systems" — boiler-specific extension fields.

## Related Tickets

- 079-propane-oil-furnace-boiler-unsupported-parse-error
- 105-default-hp-lockout-temp-citation

---

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers **shifted slightly** — actual locations in current code:
  - `flow_rate_kg_s` gas boiler: `resolve_hvac.rs:636–639` (ticket cited 639)
  - `return_temp_c` gas boiler: `resolve_hvac.rs:640–643` (ticket cited 643)
  - `flow_rate_kg_s` electric boiler: `resolve_hvac.rs:677–680` (ticket cited 680)
  - `return_temp_c` electric boiler: `resolve_hvac.rs:681–684` (ticket cited 684)
  - All four `.unwrap_or()` calls are present; the line numbers cited in the ticket are out by 3 lines due to an existing comment block (lines 632–635) that was inserted before the reads. The bug described is otherwise an exact match.
- [x] Described logic matches current implementation — both `try_build_gas_boiler_config` and `try_build_electric_boiler_config` silently apply `0.5 kg/s` and `40.0 °C` with no `tracing::debug!` and no inline citation.
- [x] An existing comment was added at some point (`// Hydronic loop flow rate and return-temperature are configuration defaults …`) for the gas boiler path, but **not** for the electric boiler path, and neither path emits a debug log.
- [x] **OCHRE cross-check: diverges** — OCHRE (`vendors/OCHRE/ochre/Equipment/HVAC.py:697–703`) does not store a separate `flow_rate`; it computes flow from load/density/specific-heat at each timestep. For return temperature OCHRE uses `outlet_temp = 65.56 °C` (condensing, 150 °F) or `outlet_temp = 82.22 °C` (non-condensing, 180 °F) — the 40 °C default in HARES is significantly lower and is described as a **return** temperature, not a supply/outlet temperature. OCHRE uses a supply outlet temperature; HARES uses a return temperature for a different purpose (loop sizing). This is an intentional architectural difference but the 40 °C value is uncited.
- [x] **EnergyPlus cross-check: N/A** — EnergyPlus's hot-water boiler model (Engineering Reference, v8.9, bigladdersoftware.com) requires only "the nominal boiler capacity and thermal efficiency"; it does not expose a design water flow rate or return temperature as user inputs. EnergyPlus autosizes or derives the loop flow from plant loop demand, so there is no EnergyPlus precedent for either default value. Quoted passage: *"The model only requires the user to supply the nominal boiler capacity and thermal efficiency … An efficiency curve can also be used to more accurately represent the performance of non-electric boilers but is not considered a required input."* (EnergyPlus Engineering Reference §Boilers, v8.9, retrieved 2026-05-21.)

### Web-Verified Citations

**Citation 1**: ASHRAE Handbook of Fundamentals 2021 Ch. 36 "Hydronic Heating and Cooling" — typical residential boiler flow rates and return temperatures

- **Source found**: ASHRAE.org — "Table of Contents 2021 ASHRAE Handbook—Fundamentals" (retrieved 2026-05-21)
- **Quoted passage**: Chapter 36 of the 2021 ASHRAE Handbook—Fundamentals is titled **"Global Climate Change"**, not "Hydronic Heating and Cooling". The Fundamentals volume does not contain a chapter on hydronic heating systems; that content is in the companion *HVAC Systems and Equipment* volume.
- **Verdict**: **Incorrect** — the citation points to the wrong ASHRAE volume *and* the wrong chapter number. The correct chapter is **ASHRAE HVAC Systems and Equipment 2020 (or 2024), Chapter 13: "Hydronic Heating and Cooling"**. Chapter 13 states that the historical US residential hydronic design standard was "a temperature drop Δt of 20°F" with "1 gpm conveying 10,000 Btu/h," but does not provide a tabulated 0.5 kg/s flow-rate or 40 °C return-temperature default for residential systems. (Source: ASHRAE Handbook S20, Ch. 13, retrieved from handbook.ashrae.org 2026-05-21.)

**Citation 2**: ASHRAE Handbook HVAC Systems and Equipment 2020 Ch. 32 "Boilers" — condensing boiler efficiency vs return water temperature

- **Source found**: ASHRAE.org table of contents for 2020 HVAC Systems and Equipment (ashraepyramids.org/page.php?id=232, retrieved 2026-05-21)
- **Quoted passage**: The 2020 ASHRAE HVAC Systems and Equipment handbook, "Heating Equipment and Components" section, does include **Chapter 32: Boilers (TC 6.1)**. This chapter exists and covers boiler types and ratings. Its content on condensing efficiency vs. return temperature is consistent with the engineering claim in the ticket (condensing threshold in the 55 °C / 130 °F range for natural gas flue gas dewpoint), as confirmed by secondary sources (R.F. MacDonald Co. white paper, retrieved 2026-05-21: *"flue gases begin to condense when the hot water return temperature is between 120°F to 130°F"*).
- **Verdict**: **Partially correct** — Chapter 32 exists in the 2020 volume and covers boilers; the condensing efficiency claim is physically sound. The full chapter is behind an ASHRAE paywall and could not be directly quoted for table/section numbers. The claim about "5-10% efficiency drop as return temp crosses the dewpoint" is directionally correct but could not be confirmed verbatim from a publicly accessible primary source.

**Citation 3**: HPXML Specification v4.x §8.4 "Heating Systems" — boiler-specific extension fields

- **Source found**: NREL HPXML Data Dictionary v4.0.0 (hpxml.nrel.gov) and OpenStudio-HPXML GitHub repository (github.com/NREL/OpenStudio-HPXML, retrieved 2026-05-21)
- **Quoted passage**: The HPXML standard as used by OpenStudio-HPXML defines `HeatingSystem/HeatingSystemType/Boiler` with a `BoilerType` field and a linked `HVACDistribution/DistributionSystemType/HydronicDistribution` element. **The HPXML schema does not include fields for hydronic loop flow rate or return water temperature**; these are internal simulation parameters outside the HPXML data model. The `§8.4` section reference is plausible for the HPXML specification document (not publicly verified by section number), but the key point — that HPXML omits these fields — is confirmed.
- **Verdict**: **Partially correct** — the citation correctly identifies that HPXML boiler definitions exist and that these parameters are absent from the standard schema. The specific section number (§8.4) could not be verified from publicly accessible sources; the HPXML Data Dictionary navigation does not expose section numbers.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: The core bug is real and confirmed in current code: both `try_build_gas_boiler_config` (`resolve_hvac.rs:636–643`) and `try_build_electric_boiler_config` (`resolve_hvac.rs:677–684`) silently apply `flow_rate_kg_s = 0.5` and `return_temp_c = 40.0` with no `tracing::debug!` and no inline primary-source citation. The physical concern about condensing-boiler efficiency sensitivity to return temperature is correct (dew-point condensation threshold is ~55 °C / 130 °F; 40 °C is well below this, so the default does place the system in condensing mode and the value matters). However, two of the three citations contain errors: (1) the ASHRAE HoF 2021 Ch. 36 citation is **wrong** — that chapter is "Global Climate Change"; the correct reference for hydronic heating is ASHRAE HVAC Systems and Equipment Ch. 13 (not Ch. 36 of Fundamentals); (2) the ASHRAE SE 2020 Ch. 32 citation is correct in chapter and volume but the claimed table/section numbers could not be verified. Additionally, neither standard nor OCHRE nor EnergyPlus supplies a canonical 0.5 kg/s / 40.0 °C residential default, so the values are engineering judgement that genuinely lack a citable primary source — which is precisely the problem the ticket is reporting. The recommended fix (Approach A: cite + log) is sound.

### Proposed Fix Summary

In `try_build_gas_boiler_config` and `try_build_electric_boiler_config` (both in `crates/hares-io/src/hpxml/resolve_hvac.rs`):

1. Replace the bare `.unwrap_or(0.5)` / `.unwrap_or(40.0)` calls with a pattern that captures whether the default was taken.
2. When the default is taken, emit `tracing::debug!(equipment_id = name, field = "flow_rate_kg_s", value = 0.5, "boiler hydronic flow rate not specified; using residential sizing default")` (analogously for `return_temp_c`).
3. Add an inline comment citing the correct source: **ASHRAE HVAC Systems and Equipment (2020 or 2024), Chapter 13 "Hydronic Heating and Cooling"** for flow-rate sizing conventions, and **ASHRAE HVAC Systems and Equipment, Chapter 32 "Boilers"** (or a manufacturer condensing boiler guide) for the 40 °C return temperature as a condensing-mode operating point. If no primary source precisely supports 0.5 kg/s as a residential default, note it as an engineering estimate pending calibration.
4. Update the `GasBoilerConfig` and `ElectricBoilerConfig` doc-comments to document the defaults and their provenance.
5. The fix must **not** change the default values themselves (0.5 kg/s, 40.0 °C) without a separate ticket and supporting data.

Do **not** correct the ASHRAE citation in the ticket body to point to Ch. 13 — that change belongs in the production-code comment, not the ticket.

### Test Written

- **File**: `crates/hares-io/tests/silent_default_regressions.rs`
- **What it tests**:
  - `gas_boiler_omitting_flow_rate_and_return_temp_silently_applies_defaults` — resolves a gas boiler HPXML with no `flow_rate_kg_s` or `return_temp_c` fields and asserts the typed config receives exactly `0.5` and `40.0` respectively, documenting the current silent-substitution behaviour.
  - `electric_boiler_omitting_flow_rate_and_return_temp_silently_applies_defaults` — same for the electric boiler path.
  - Both tests pass against current code (`cargo test -p hares-io --test silent_default_regressions`, 2026-05-21). They will continue to pass after the fix is applied (the values don't change, only a log is added); they would fail if the defaults were altered without a corresponding test update.
