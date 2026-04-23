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
