# `reconcile_setpoint_pair` Must Surface Setpoint Mutation Loudly

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-io/hpxml/resolve_hvac

## Problem

`crates/hares-io/src/hpxml/resolve_hvac.rs:2286-2319` `reconcile_setpoint_pair` silently mutates HPXML setpoints when the heating-cooling setpoint gap is less than 2°C. Only a `tracing::warn!` is emitted. The user's input data is changed without a machine-readable signal — downstream Python code, dashboards, and CSV diagnostics have no way to know that the setpoints they see are not what was supplied.

A heating setpoint of 21°C and a cooling setpoint of 22°C is a 1°C gap. The reconciliation widens the gap (typically to 2°C or 4°C) so the thermostat FSM has a deadband. The widening is reasonable physics, but silently changing the user's input violates `feedback_no_silent_defaults`.

## Current Behavior

`crates/hares-io/src/hpxml/resolve_hvac.rs:2286-2319`:
```rust
if (cooling_setpoint - heating_setpoint).abs() < 2.0 {
    tracing::warn!("setpoints too close; widening");
    heating_setpoint -= 1.0;
    cooling_setpoint += 1.0;
}
```

The warning logs; no error is returned and no flag is exposed in the resolved config.

## Required Behavior

Choose one:

A. **Return a hard error** — `HpxmlError::InvalidField { field: "HVACPlant", reason: "heating and cooling setpoints differ by less than 2°C" }`. The user must explicitly fix the input before the simulation will run.

B. **Surface a machine-readable signal** — add a `setpoints_reconciled: Option<SetpointReconciliation>` field to the resolved config carrying the original and adjusted values. Downstream consumers can detect and report the mutation.

Recommended path: A for strictness (reconciliation is silently changing the user's input); fall back to B if running an HPXML library that often produces narrow gaps.

## Approach

1. Choose path A or B based on the typical HPXML inputs encountered.
2. If A:
   - Replace the silent widening with `return Err(HpxmlError::InvalidField { ... })`.
   - Add a fixture with narrow setpoints; assert the resolver errors.
   - Update HPXML import documentation to require sensible setpoint gaps.
3. If B:
   - Add `SetpointReconciliation { original_heating_c: f64, original_cooling_c: f64, adjusted_heating_c: f64, adjusted_cooling_c: f64 }`.
   - Populate when widening occurs.
   - Expose in resolved config and Python introspection.
   - Keep the `tracing::warn!` for visibility.
   - Add a fixture and test asserting the structure is populated.

## Definition of Done

- [ ] `reconcile_setpoint_pair` either errors loudly (path A) or exposes the reconciliation as a structured field (path B)
- [ ] `tracing::warn!` retained for log-level visibility
- [ ] Fixture exercises the narrow-setpoint case and asserts the chosen behaviour
- [ ] Documentation updated explaining the reconciliation policy

## Verification

```bash
cargo test -p hares-io resolve_hvac setpoint
cargo test -p hares-io hpxml_parity
```

## References

- HPXML Specification v4.x §8.4 "HVAC Plant" — heating and cooling setpoints are user-supplied; HPXML does not constrain the gap.
- ASHRAE Standard 55-2020 *Thermal Environmental Conditions for Human Occupancy* §5.3 — typical residential heating/cooling setpoint gap is 2-3°C.
- Project policy `feedback_no_silent_defaults.md`.

## Related Tickets

- 020-setpoint-chain-visibility
- 006-extract-thermostat-fsm

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match (note: ticket cites lines 2286-2319, but the actual function `reconcile_setpoint_pair` is at **lines 2271–2298**, with the constant `SETPOINT_RECONCILE_GAP_C = 2.0` at line 2269. The function call sites are at lines 2239–2240 and 2336–2337.)
- [x] Described logic matches current implementation — **partially**. The ticket's pseudocode is simplified and inaccurate. The real code iterates all 24 hourly setpoints and clips each hour to `avg ± 1 °C` (not a fixed `±1.0`), and counts `violated_hours` and `worst_inversion_c` before emitting the warning. The core complaint — that only `tracing::warn!` is emitted and no machine-readable signal is returned — is **accurate**.
- [x] OCHRE cross-check result: **diverges (gap threshold)**. OCHRE enforces a **1 °C minimum** separation (`vendors/OCHRE/ochre/utils/schedule.py:617-625`): `if setpoint_diff.min() < 1: print("WARNING: ...")` and clips to `avg ± 0.5`. HARES enforces a **2 °C minimum** (motivated by its downstream thermostat invariant `cooling - heating >= 2 * hysteresis_c` with default hysteresis = 1 °C, documented in the source comment at line 2260–2268). The divergence is **intentional** — HARES's gap threshold is tighter than OCHRE's because HARES's thermostat FSM requires the larger gap. Both codebases share the same "warn-only, no error return" pattern.
- [x] EnergyPlus cross-check result: **not directly applicable**. EnergyPlus's `ZoneControl:Thermostat` (I/O Reference, EnergyPlus 9.4, Group Zone Controls) only mandates a minimum setpoint separation when the optional "Temperature Difference Between Cutout And Setpoint" field is used: *"The heating and cooling setpoints must be separated by at least 2 times the Temperature Difference Between Cutout And Setpoint or there will be a fatal error."* For standard `ThermostatSetpoint:DualSetpoint` without that optional field, EnergyPlus imposes no minimum gap at the input level. HARES's 2 °C reconciliation is therefore a HARES-internal invariant (not an EnergyPlus requirement), and the decision to enforce it without surfacing a machine-readable signal is the bug this ticket addresses.

### Web-Verified Citations

**Citation 1:**
- **Citation**: HPXML Specification v4.x §8.4 "HVAC Plant" — heating and cooling setpoints are user-supplied; HPXML does not constrain the gap.
- **Source found**: HPXML Toolbox at hpxml.nrel.gov; OpenStudio-HPXML documentation (openstudio-hpxml.readthedocs.io). HPXML XSD schema validation uses Schematron for constraints and XSD for data types.
- **Quoted passage**: From the OpenStudio-HPXML documentation search results: *"Heating (or cooling) setpoints are only needed if heating (or cooling) equipment is present. The documentation shows that weekday heating setpoint schedules are required unless a detailed CSV schedule is provided."* No minimum gap constraint between heating and cooling setpoints appears in any accessible HPXML v4 validation resource.
- **Verdict**: **Confirmed** — HPXML does not constrain the setpoint gap. The section reference "§8.4" could not be independently verified (the HPXML spec is a paywalled BPI-2200 document), but the substance of the claim (HPXML imposes no setpoint gap constraint) is consistent with all accessible HPXML documentation and tooling.

**Citation 2:**
- **Citation**: ASHRAE Standard 55-2020 *Thermal Environmental Conditions for Human Occupancy* §5.3 — typical residential heating/cooling setpoint gap is 2-3°C.
- **Source found**: Wikipedia article on ASHRAE 55; SimScale blog article on ASHRAE 55; table of contents preview (store.accuristech.com); multiple searches for the specific section and values.
- **Quoted passage**: From Wikipedia and SimScale: ASHRAE 55 specifies *"combinations of indoor thermal environmental factors as well as personal factors"* to determine acceptable comfort ranges. Section 5.3 in ASHRAE 55-2023/2020 is titled *"Method for Determining Acceptable Thermal Environment in Occupied Spaces"* and covers PMV/PPD models and comfort zone boundaries. *"ASHRAE 55 does not address thermostat setpoints, heating/cooling setpoint gaps, or deadbands."* (SimScale analysis, confirmed by Wikipedia.)
- **Verdict**: **Incorrect** — ASHRAE 55 is a thermal comfort standard governing acceptable operative temperature ranges for occupants. It does not specify a recommended 2-3°C gap between HVAC heating and cooling setpoints. The ticket's citation is a category error: ASHRAE 55 addresses occupant comfort bounds, not thermostat deadband engineering. There is no §5.3 content in ASHRAE 55 about residential setpoint gaps. This citation should be removed or replaced with a more appropriate reference (e.g., ASHRAE Guideline 36, which does address thermostat deadbands for VAV systems, or simply the internal thermostat FSM invariant).

**Citation 3:**
- **Citation**: Project policy `feedback_no_silent_defaults.md`
- **Source found**: Searched `/Users/rich/source/HARES/docs/` — no standalone file `feedback_no_silent_defaults.md` exists. The policy name appears as a cross-reference in 23 other ticket files.
- **Quoted passage**: N/A — file does not exist as a standalone document.
- **Verdict**: **Cannot verify as a standalone document** — the policy is real (referenced consistently across many tickets) but has no canonical file. The intent is clear from context.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: The core bug is real and confirmed: `reconcile_setpoint_pair` (lines 2271–2298, not 2286–2319 as cited) silently mutates all 24 hourly setpoint pairs whenever any hour's gap is less than 2 °C, emitting only a `tracing::warn!` with no machine-readable return value or output field. Neither `apply_building_setpoint_profiles` nor `parse_hvac_setpoint_params` surface the mutation to callers. This is a genuine `feedback_no_silent_defaults` violation. However, the ticket contains inaccuracies: (1) the line numbers are wrong by ~15 lines; (2) the pseudocode omits the per-hour loop and midpoint logic; (3) the ASHRAE 55-2020 §5.3 citation is incorrect — that standard does not address thermostat setpoint gaps; (4) the gap-widening logic is already more sophisticated than the ticket implies (midpoint-preserving clip, not a naive ±1 °C shift). The existing internal test suite already verifies that reconciliation produces physically valid setpoints; what is missing is a test (and implementation) asserting that the reconciliation is observable by callers.

### Proposed Fix Summary

Do not implement. The minimal fix is path B: change `apply_building_setpoint_profiles` (and `parse_hvac_setpoint_params` if it also calls `reconcile_setpoint_pair`) to return an additional `Option<SetpointReconciliation>` value carrying the original and adjusted per-hour arrays. Insert this into the params map under the key `"setpoints_reconciled"` so downstream consumers can detect and report it. Retain the existing `tracing::warn!`. Path A (hard error) is stricter but risks breaking HPXML library workflows that regularly produce narrow setpoints (e.g., ResStock CSVs).

### Test Written

- **File**: `crates/hares-io/src/hpxml/resolve_hvac.rs` (within `#[cfg(test)] mod tests`, after line 3811)
- **Function**: `reconcile_setpoint_pair_narrow_gap_produces_no_machine_readable_signal`
- **What it tests**: Asserts that after calling `apply_building_setpoint_profiles` with a narrow setpoint gap (heating=21 °C, cooling=22 °C, gap=1 °C < 2 °C threshold), a `"setpoints_reconciled"` key is present in the output params map. The assertion currently fails (the key is absent), which is caught by `#[should_panic]`. When ticket 113 is resolved (path B), the `#[should_panic]` must be removed and the assertion will pass; for path A, the test must be restructured to check the error return instead.
