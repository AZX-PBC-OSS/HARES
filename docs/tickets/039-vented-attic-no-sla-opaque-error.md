# Vented Attic Without Explicit SLA/ACH Fails Solver Construction With Opaque Error

**Severity**: Medium
**Priority**: P3
**Status**: Open
**Areas**: hares-core/dwelling/solver_builder.rs

## Problem

`attic_infiltration_method` (`crates/hares-core/src/dwelling/solver_builder.rs:1213–1217`)
returns an opaque error when a vented attic zone provides neither `ventilation_ach`
nor `ventilation_sla`:

```rust
if zone.vented {
    return Err(HaresError::Envelope(format!(
        "Vented attic zone {zone_idx} requires explicit ventilation data (ACH or SLA)"
    )));
}
```

The error message does not name the HPXML element that must be added, does not
cite the governing standard, and does not provide the correct value to use.
ResStock HPXML files routinely omit `<VentilationRate>` for standard vented
attics because the building code provides a well-known default. The simulation
fails without telling the user how to fix it.

## Current Behavior

`crates/hares-core/src/dwelling/solver_builder.rs:1213–1221`:

```rust
if zone.vented {
    return Err(HaresError::Envelope(format!(
        "Vented attic zone {zone_idx} requires explicit ventilation data (ACH or SLA)"
    )));
}
// Unvented attic default matches OCHRE attic handling: 0.1 ACH.
Ok(InfiltrationMethod::Ach { ach: 0.1 })
```

The unvented path silently defaults to 0.1 ACH; the vented path rejects.

## Required Behavior

Per project policy (feedback_no_silent_defaults), missing inputs must produce
explicit errors, not silent substitution. However, the error must be actionable.
The correct fix is to improve the error message — not to silently apply any
default — for the case where HPXML omits `<VentilationRate>`.

The error must:
- Name the required HPXML element and its location in the schema.
- Cite the authoritative default value so the user knows what to add.
- Reference the governing standard.

Per ASHRAE 62.2-2022 §8.2, the minimum vented attic ventilation ratio is 1/150
of the ceiling area (SLA = 0.0067). Per IRC 2021 R806.2, the same 1/150 ratio
applies. EnergyPlus uses SLA = 0.003 (balanced inlet/outlet assumption). The
user must supply one of these values explicitly in the HPXML file.

## Approach

Replace the error at `solver_builder.rs:1213–1217` with:

```rust
if zone.vented {
    return Err(HaresError::Envelope(format!(
        "Vented attic zone {zone_idx}: <VentilationRate> is required but absent. \
         Add <VentilationRate><UnitofMeasure>SLA</UnitofMeasure><Value>0.003</Value>\
         </VentilationRate> under the attic zone element. \
         ASHRAE 62.2-2022 §8.2 minimum is SLA=0.0067 (1/150); \
         EnergyPlus uses SLA=0.003 for balanced inlet/outlet vented attics \
         (IRC 2021 R806.2)."
    )));
}
```

No default is applied silently. The user must provide the value explicitly.

## Definition of Done

- [ ] Error message names `<VentilationRate>`, the unit (`SLA`), and two
      authoritative reference values (ASHRAE 62.2 and EnergyPlus).
- [ ] Error message cites ASHRAE 62.2-2022 §8.2 and IRC 2021 R806.2 by section.
- [ ] Test: vented attic zone with no `ventilation_sla` and no `ventilation_ach`
      returns `Err` whose message contains "VentilationRate" and "SLA".
- [ ] Test: vented attic zone with `ventilation_sla = 0.003` builds successfully.

## Verification

```bash
cargo test -p hares-core attic_infiltration
```

Expected: missing SLA → `Err` containing "VentilationRate"; explicit SLA=0.003 → `Ok`.

## References

- ASHRAE 62.2-2022 §8.2 (Ventilated Attic Spaces — minimum 1/150 ratio).
- IRC 2021 R806.2 (Attic ventilation — 1/150 net free area ratio).
- EnergyPlus Engineering Reference §17.3 (Vented Attic Model — SLA = 0.003
  as balanced inlet/outlet default).
- `crates/hares-core/src/dwelling/solver_builder.rs:1182–1221`
  (`attic_infiltration_method`).

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match — `attic_infiltration_method` is at lines
      1180–1221; the opaque error is at lines 1213–1217, matching the ticket exactly.
- [x] Described logic matches current implementation — the vented branch returns
      `Err(HaresError::Envelope(...))` with message
      `"Vented attic zone {zone_idx} requires explicit ventilation data (ACH or SLA)"`.
      The unvented branch defaults to 0.1 ACH. Both match the ticket description.
- [x] OCHRE cross-check: **matches** — `vendors/OCHRE/ochre/utils/hpxml.py:662–670`
      uses identical logic: unvented attic with no `VentilationRate` → 0.1 ACH;
      vented attic with no `VentilationRate` → `raise OCHREException(...)`.
      OCHRE does NOT silently apply a default SLA for vented attics either.
- [x] EnergyPlus cross-check: **N/A for §17.3** — see "Web-Verified Citations" below.
      EnergyPlus's general infiltration reference does not prescribe SLA=0.003 as an
      attic-specific default. The closest real-world default (per ANSI/RESNET/ICC 301
      and OpenStudio-HPXML) is SLA=1/300 ≈ 0.00333.

### Web-Verified Citations

#### Citation 1 — ASHRAE 62.2-2022 §8.2 (vented attic 1/150 ratio)

- **Citation**: "Per ASHRAE 62.2-2022 §8.2, the minimum vented attic ventilation ratio
  is 1/150 of the ceiling area (SLA = 0.0067)."
- **Source found**: ASHRAE 62.2-2022 Table of Contents (via `store.accuristech.com` and
  `www.ashrae.org/technical-resources/bookstore/standards-62-1-62-2`; full text
  paywalled). Table of contents structure confirmed via ANSI Blog
  (`https://blog.ansi.org/ansi/ansi-ashrae-62-2-2022-ventilation-residential-air/`)
  and secondary sources.
- **Quoted passage**: Per confirmed ToC, ASHRAE 62.2-2022 sections are:
  1 Scope, 2 Normative References, 3 Definitions, 4 Ventilation Rate,
  5 Local Mechanical Exhaust, 6 Ventilation Opening Area and System Design,
  7 Equipment, **8 Operations and Maintenance**, 9 Climate Data, Normative
  Appendices A–D. Section 8 is "Operations and Maintenance", not "Ventilated
  Attic Spaces." A secondary source explicitly states: "ASHRAE 62.2 has nothing
  to do with attic ventilation" — the standard addresses whole-building
  mechanical ventilation in occupied dwelling units, not passive attic ventilation
  ratios.
- **Verdict**: **INCORRECT**. ASHRAE 62.2-2022 §8.2 does not govern vented attic
  ventilation. The 1/150 ratio originates from IRC 2021 R806.2 (and RESNET 301).
  The ticket misattributes the IRC provision to ASHRAE 62.2.

#### Citation 2 — IRC 2021 R806.2 (1/150 net free area ratio)

- **Source found**: `https://up.codes/s/minimum-vent-area` (ICC Digital Codes
  mirror), confirmed independently by `https://codes.iccsafe.org/s/IRC2021P3/...`
  (paywalled but table confirmed by redirect).
- **Quoted passage**: "The minimum net free ventilating area shall be 1/150 of the
  area of the vented space. Exception: The minimum net free ventilation area shall
  be 1/300 of the vented space provided [Climate Zone 6/7/8 vapor retarder +
  balanced upper/lower vent placement conditions are met]."
- **Verdict**: **Confirmed**. IRC 2021 R806.2 does specify the 1/150 minimum ratio.
  The citation and section number are correct.

#### Citation 3 — EnergyPlus Engineering Reference §17.3 (SLA=0.003)

- **Citation**: "EnergyPlus Engineering Reference §17.3 (Vented Attic Model —
  SLA = 0.003 as balanced inlet/outlet default)."
- **Source found**: EnergyPlus 9.3 Engineering Reference Table of Contents
  (`https://bigladdersoftware.com/epx/docs/9-3/engineering-reference/`) fetched
  and reviewed. Multiple versions (8.0–24.1) also searched.
- **Quoted passage**: The EnergyPlus Engineering Reference contains **no Section 17.3
  titled "Vented Attic Model."** The full ToC (13 top-level chapters: Overview,
  Integrated Solution Manager, Surface Heat Balance, Advanced Surface Concepts,
  Climate/Solar, Solar Reflection, Daylighting, Air Heat Balance, Building System
  Simulation, Loop/Equipment Sizing, Operational Faults, Demand Limiting,
  Alternative Modeling Processes) has no dedicated vented attic chapter. The
  "Exterior Naturally Vented Cavity" section describes exterior baffle/facade
  assemblies, not attic zones. The infiltration/ventilation section contains no
  attic-specific SLA defaults.
- **Verdict**: **INCORRECT**. No such section or default exists in the EnergyPlus
  Engineering Reference. The authoritative default for the ResStock/OCHRE ecosystem
  comes from ANSI/RESNET/ICC 301, not EnergyPlus §17.3.

#### Citation 4 — EnergyPlus SLA=0.003 value

- **Citation**: "EnergyPlus uses SLA = 0.003 for balanced inlet/outlet vented attics."
- **Source found**: OpenStudio-HPXML documentation (`openstudio-hpxml.readthedocs.io`)
  and ResStock documentation confirmed the real-world default. ANSI/RESNET/ICC
  301-2014/2019/2022 (`www.resnet.us/wp-content/uploads/...`) all confirmed SLA
  default of **1/300 ≈ 0.00333** for the reference vented attic, not 0.003.
  The OpenStudio-HPXML docs state: "If these elements are not provided, default
  values of 1/300 for vented attics and 1/150 for vented crawlspaces will be used
  based on ANSI/RESNET/ICC 301-2019."
- **Verdict**: **INCORRECT / PARTIALLY CORRECT**. The correct default in the
  ResStock/OpenStudio ecosystem is SLA=1/300 ≈ **0.00333**, not 0.003. The 0.003
  value is a close approximation but is not the canonical value, and it does not
  originate from EnergyPlus.

### Legitimacy

- **Verdict**: **Partially Legitimate**

- **Rationale**: The core bug is real and confirmed: the vented attic error message
  at `solver_builder.rs:1213–1217` is opaque and does not name the `<VentilationRate>`
  HPXML element, making it non-actionable for users. The fix direction (improve the
  error message without silently defaulting) is sound and matches the project's
  no-silent-defaults policy. However, the ticket's citations contain two material
  errors: (1) ASHRAE 62.2-2022 §8.2 is "Operations and Maintenance" — it has nothing
  to do with attic ventilation ratios; the 1/150 ratio is an IRC provision, not ASHRAE
  62.2. (2) There is no EnergyPlus Engineering Reference §17.3 titled "Vented Attic
  Model." The authoritative ecosystem default is ANSI/RESNET/ICC 301's SLA=1/300
  (≈0.00333), not SLA=0.003 attributed to EnergyPlus. The proposed error message
  should cite IRC 2021 R806.2 and ANSI/RESNET/ICC 301-2019 instead of ASHRAE 62.2-2022
  §8.2 and EnergyPlus §17.3.

### Proposed Fix Summary

Replace the error text at `solver_builder.rs:1213–1217` with a message that:

1. Names `<VentilationRate><UnitofMeasure>SLA</UnitofMeasure><Value>…</Value></VentilationRate>`
   as the required HPXML element.
2. Cites **IRC 2021 R806.2** (minimum 1/150 = SLA ≈ 0.0067) as the code minimum.
3. Cites **ANSI/RESNET/ICC 301-2019** (1/300 = SLA ≈ 0.00333) as the ResStock/energy-
   rating default — replacing the incorrect ASHRAE 62.2-2022 §8.2 and EnergyPlus §17.3
   references in the original proposed fix.
4. Does NOT apply any default silently.

### Test Written

- **File**: `crates/hares-core/src/dwelling/solver_builder.rs` (within existing
  `#[cfg(test)]` module, after line 1702)
- **Tests added**:
  - `attic_vented_no_sla_error_names_ventilation_rate_element` — **currently FAILING**
    (demonstrates the bug: current message lacks "VentilationRate")
  - `attic_vented_no_sla_error_names_sla_unit` — currently passes (message already
    contains "SLA")
  - `attic_vented_with_sla_builds_successfully` — currently passes (happy path works)
- **Cargo command**: `cargo test -p hares-core -- attic`
