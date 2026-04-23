# `infiltration_ach50` Parsing Reads CFM50 Values as ACH50

**Severity**: Critical
**Priority**: P1
**Status**: Open
**Areas**: hares-io/hpxml/building.rs

## Problem

`infiltration_ach50` is populated by three raw `extract_first_f64` calls
(`crates/hares-io/src/hpxml/building.rs:826–861`) that traverse the HPXML tree
to `<AirLeakage>` and read its numeric text with `ValueKind::Raw` — no check on
the `<UnitofMeasure>` child element or `units` XML attribute.

When HPXML specifies leakage in CFM50 using the HPXML 4.x inline-attribute
form `<AirLeakage units="CFM50">750.0</AirLeakage>` directly under
`AirInfiltrationMeasurement`, the path:

```
["AirInfiltration", "AirInfiltrationMeasurement", "AirLeakage"]
```

matches, and `extract_first_f64` returns `750.0` into `infiltration_ach50`.
A residential home with 750 CFM50 ≈ 5 ACH50; the parser stores 750 ACH50 — a
150× overestimate. The AIM-2 model then produces natural ACH ≈ 750/17 ≈ 44 ACH,
generating annual infiltration loads 100× above correct.

The same collision exists for the HPXML 3.x `<BuildingAirLeakage>` wrapper form
when `<UnitofMeasure>CFM</UnitofMeasure>` is present.

The test at `crates/hares-io/src/hpxml/building.rs:3753–3755` acknowledges
non-deterministic behavior with the comment "infiltration_ach50 may or may not
be set depending on parse path." Non-deterministic parsing is not acceptable.

This defect is structurally identical to ticket 077 (BackupAnnualHeatingEfficiency
unit ignored in the plumbing cluster): both are HPXML numeric fields parsed
without reading the adjacent unit element. See cross-cluster note below.

## Current Behavior

`crates/hares-io/src/hpxml/building.rs:826–861`:

```rust
infiltration_ach50: extract_first_f64(
    details,
    &["Enclosure", "AirInfiltration", "AirInfiltrationMeasurement",
      "BuildingAirLeakage", "AirLeakage"],
    ValueKind::Raw,  // no unit check
)
.or_else(|| extract_first_f64(
    details,
    &["AirInfiltration", "AirInfiltrationMeasurement",
      "BuildingAirLeakage", "AirLeakage"],
    ValueKind::Raw,  // no unit check
))
.or_else(|| extract_first_f64(
    details,
    &["AirInfiltration", "AirInfiltrationMeasurement", "AirLeakage"],
    ValueKind::Raw,  // no unit check
)),
```

`parse_air_leakage_cfm50` (`building.rs:2031–2053`) already reads the unit
attribute/element and filters correctly. The ACH50 path must do the same.

## Required Behavior

Per HPXML Specification v4.2 §4.4.4.1 (AirInfiltrationMeasurement), valid
`<UnitofMeasure>` values are `ACH`, `ACH50`, `CFM`, `CFM50`, `Pa`. A value
stored in `infiltration_ach50` must be rejected unless the unit is absent,
`ACH`, or `ACH50` (case-insensitive). There is no silent conversion from CFM50
to ACH50 — that requires house volume, which is a separate concern.

## Approach

1. Implement `parse_air_leakage_ach50(details: &XmlNode) -> Option<f64>` in
   `building.rs`, modelled on `parse_air_leakage_cfm50`. Check the sibling
   `<UnitofMeasure>` element and the `units` attribute on `<AirLeakage>` before
   accepting the value.
2. Replace the three raw `extract_first_f64` chains at lines 826–861 with a
   call to `parse_air_leakage_ach50`.
3. When the unit is `CFM` or `CFM50`, return `None` from `parse_air_leakage_ach50`
   — the CFM50 parser already captures these values.
4. When the unit is unrecognised (not ACH, ACH50, CFM, CFM50, Pa), return an
   error via `HpxmlError`; do not silently drop the value.
5. Fix the test at `building.rs:3753–3755` to assert `infiltration_ach50 == None`
   when unit is CFM, and add a separate assertion that `infiltration_cfm50` holds
   the correct value.
6. Add a test: `<AirLeakage units="CFM50">750.0</AirLeakage>` →
   `infiltration_ach50 == None`, `infiltration_cfm50 == Some(750.0)`.
7. Add a test: `<AirLeakage units="ACH50">5.0</AirLeakage>` →
   `infiltration_ach50 == Some(5.0)`.

## Definition of Done

- [ ] `parse_air_leakage_ach50` function exists and is unit-guarded.
- [ ] All three raw `extract_first_f64` chains replaced.
- [ ] Non-deterministic test comment removed and replaced with deterministic assertions.
- [ ] Tests for CFM50, ACH50, and unrecognised unit inputs all pass.

## Verification

```bash
cargo test -p hares-io infiltration_ach50
cargo test -p hares-io parse_air_leakage
```

Expected: CFM50 input → `infiltration_ach50 == None`; ACH50 input → value
stored; unrecognised unit → parse error.

## Cross-Cluster Note

This defect is structurally the same as ticket 077 (BackupAnnualHeatingEfficiency
units ignored in the plumbing cluster). Consider a meta-ticket to audit all
`extract_first_f64` calls in `building.rs` that read numeric HPXML fields with
adjacent unit elements.

## References

- HPXML Specification v4.2 §4.4.4.1 (AirInfiltrationMeasurement — UnitofMeasure
  enumeration: ACH, ACH50, CFM, CFM50, Pa). hpxml.nrel.gov.
- OCHRE `utils/hpxml.py`: reads `air_leakage_unit` before extracting value,
  converts CFM50 → ACH50 using house volume.
- `crates/hares-io/src/hpxml/building.rs:2027–2054` (`parse_air_leakage_cfm50` —
  model the fix on this function).
