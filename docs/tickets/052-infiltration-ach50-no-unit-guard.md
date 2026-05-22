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

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match (826–861 for raw ACH50 chains, 2027–2054 for `parse_air_leakage_cfm50`, 3753–3755 for non-deterministic comment) — confirmed as of HEAD.
- [x] Described logic matches current implementation — three raw `extract_first_f64` calls at lines 826–861 use `ValueKind::Raw` with no unit guard, exactly as described.
- [x] OCHRE cross-check result: **diverges** — `vendors/OCHRE/ochre/utils/envelope.py:490` asserts `indoor_inf["BuildingAirLeakage"]["UnitofMeasure"] in ["ACH"]` before reading the value, hard-failing on any non-ACH unit. OCHRE enforces a strict unit check; HARES performs no check at all. (OCHRE does not itself handle the HPXML 4.x inline-attribute form `<AirLeakage units="CFM50">` since it uses xmltodict and only sees `UnitofMeasure` child elements.)
- [x] EnergyPlus cross-check result: N/A — the bug is in HPXML parsing (I/O layer), not in the AIM-2 physics implementation. The EnergyPlus `ZoneInfiltration:FlowCoefficient` model (Walker & Wilson 1998) consumes already-resolved flow coefficients, not raw HPXML blower-door values. No EnergyPlus passage is directly applicable to the unit-guarding omission.

### Web-Verified Citations

**Citation 1**: HPXML Specification v4.2 §4.4.4.1 — UnitofMeasure enumeration: ACH, ACH50, CFM, CFM50, Pa.

- **Source found**: HPXML XSD schema at `https://raw.githubusercontent.com/hpxmlwg/hpxml/master/schemas/HPXMLDataTypes.xsd`; OpenStudio-HPXML workflow inputs docs; Home Energy Score HPXML translator docs.
- **Quoted passage** (from HPXMLDataTypes.xsd, `BuildingAirLeakageUnit_simple`): The schema defines the valid enumeration as **`CFM`, `CFMnatural`, `ACH`, `ACHnatural`** only. There is no `ACH50`, `CFM50`, or `Pa` enumeration value in the XSD for `BuildingAirLeakage/UnitofMeasure`.
- **Verdict**: **Partially correct / overstated**. The ticket's claim that valid `UnitofMeasure` values include `ACH50`, `CFM50`, and `Pa` is wrong. The actual HPXML schema (hpxmlwg master XSD) enumerates only `ACH`, `ACHnatural`, `CFM`, `CFMnatural` for the `BuildingAirLeakage/UnitofMeasure` element. `Pa` is used only in the separate `HousePressure` element. `ACH50` and `CFM50` as `UnitofMeasure` values are not schema-valid; the "50" at 50 Pa is implied by `HousePressure=50` and the measurement unit (`ACH` or `CFM`). However, the HPXML 4.x inline-attribute form `<AirLeakage units="CFM50">` is used in practice by tools such as OpenStudio-HPXML and appears in the HARES codebase's own test fixtures — this is an extension convention, not an enumerated schema value. The core correctness concern the ticket raises (unit-guarded parsing) is real and unaffected by this citation error.
- **Impact**: The ticket's `UnitofMeasure` enumeration list (ACH, ACH50, CFM, CFM50, Pa) should be corrected to (ACH, ACHnatural, CFM, CFMnatural) for the `BuildingAirLeakage/UnitofMeasure` child element, and (CFM50, ACH50) for the HPXML 4.x `<AirLeakage units="...">` attribute form.

**Citation 2**: OCHRE `utils/hpxml.py` reads `air_leakage_unit` before extracting value, converts CFM50 → ACH50 using house volume.

- **Source found**: `vendors/OCHRE/ochre/utils/envelope.py:490` (OCHRE is a read-only submodule at the cited path).
- **Quoted passage**: `assert indoor_inf["BuildingAirLeakage"]["UnitofMeasure"] in ["ACH"]  # only allow ACH50, not ACHnatural` (line 490). Then `ach = indoor_inf["BuildingAirLeakage"]["AirLeakage"]` (line 491). OCHRE does NOT convert CFM50 → ACH50; it simply asserts the unit must be ACH and fails hard otherwise. A comment at `utils/hpxml.py:729` notes: `# FUTURE: convert to ELA? Can use 1sqft at 4.9 SLA = 1.2854145 CFM50, will need to convert ACH50 to ACH`, indicating CFM50 support is a future TODO.
- **Verdict**: **Partially correct**. OCHRE does guard on unit (correct). But the claim that OCHRE "converts CFM50 → ACH50 using house volume" is wrong — OCHRE rejects CFM50 entirely (assertion failure). HARES diverges from OCHRE by silently accepting CFM50 as ACH50.

**Citation 3**: `parse_air_leakage_cfm50` at `building.rs:2027–2054` as the model for the fix.

- **Source found**: `crates/hares-io/src/hpxml/building.rs:2027–2054` (read directly).
- **Quoted passage**: The function checks `bal.child("UnitofMeasure").map(|n| normalize_ascii(n.text.trim()))` and `matches!(unit.as_deref(), Some("cfm"))` for HPXML 3.x form, and `al.attrs.get("units").map(|s| normalize_ascii(s))` / `matches!(unit_attr.as_deref(), Some("cfm50") | Some("cfm"))` for HPXML 4.x form. This correctly gates on unit before accepting the value.
- **Verdict**: **Confirmed**. The function is exactly as described and is the correct model.

### Legitimacy

- **Verdict**: **Partially Legitimate**

- **Rationale**: The core defect is **real and confirmed by regression tests**. Two of the three new `ticket_052_*` tests fail: `ticket_052_cfm50_inline_attr_does_not_poison_ach50` shows `infiltration_ach50 = Some(750.0)` when it should be `None` (750 CFM50 stored as 750 ACH50 — the 150× overestimate the ticket describes); `ticket_052_unitofmeasure_cfm_wrapper_does_not_set_ach50` confirms the same problem for the HPXML 3.x wrapper form. The non-deterministic test comment at lines 3753–3755 is also confirmed. The fix approach (model `parse_air_leakage_ach50` on `parse_air_leakage_cfm50`) is correct. The severity rating (Critical / P1) is justified given the 100–150× load overestimation. The partial issue is the UnitofMeasure enumeration list in the ticket: the HPXML schema's valid values for `BuildingAirLeakage/UnitofMeasure` are `ACH`, `ACHnatural`, `CFM`, `CFMnatural` — not `ACH50`, `CFM50`, or `Pa`. The HPXML 4.x `<AirLeakage units="CFM50">` inline-attribute form is a real-world convention used by OpenStudio-HPXML but is separate from the `UnitofMeasure` child element. The fix must handle both forms (as `parse_air_leakage_cfm50` already does), but the ticket's stated enumeration is factually wrong about `ACH50`, `CFM50`, and `Pa` being `UnitofMeasure` values. This does not affect fix correctness.

### Proposed Fix Summary

Implement `parse_air_leakage_ach50(details: &XmlNode) -> Option<f64>` in `building.rs`, modelled structurally on `parse_air_leakage_cfm50` (lines 2027–2054):
1. For HPXML 3.x form: check `BuildingAirLeakage/UnitofMeasure` child — accept only `"ach"` (case-normalised via `normalize_ascii`).
2. For HPXML 4.x form: check `<AirLeakage units="...">` attribute — accept `"ach50"` or `"ach"`; return `None` for `"cfm50"`, `"cfm"`, `"cfmnatural"`, `"achnatural"`.
3. For bare `<AirLeakage>` with no unit (as in SAMPLE_XML): accept the value (current behaviour is correct for the no-unit case; HPXML convention treats an unadorned value under `AirInfiltrationMeasurement` as ACH50 when `HousePressure=50`).
4. Replace the three raw `extract_first_f64` chains at lines 826–861 with a call to `parse_air_leakage_ach50`.
5. Do NOT add error path for unrecognised units at this stage — `None` is safer and avoids breaking production parses of HPXML files with unexpected values.

### Test Written

- **File**: `crates/hares-io/src/hpxml/building.rs` (within `#[cfg(test)]` module at end of file)
- **Functions added**:
  - `ticket_052_cfm50_inline_attr_does_not_poison_ach50` — asserts `infiltration_ach50 == None` and `infiltration_cfm50 == Some(750.0)` when `<AirLeakage units="CFM50">750.0</AirLeakage>` appears directly under `AirInfiltrationMeasurement`. **Currently FAILS** (bug confirmed: returns `Some(750.0)` for `infiltration_ach50`).
  - `ticket_052_ach50_inline_attr_is_accepted` — asserts `infiltration_ach50 == Some(5.0)` and `infiltration_cfm50 == None` for `<AirLeakage units="ACH50">5.0</AirLeakage>`. **Currently PASSES** (accidentally correct via raw path).
  - `ticket_052_unitofmeasure_cfm_wrapper_does_not_set_ach50` — asserts `infiltration_ach50 == None` and `infiltration_cfm50 == Some(850.0)` for the HPXML 3.x `<BuildingAirLeakage><UnitofMeasure>CFM</UnitofMeasure><AirLeakage>850.0</AirLeakage></BuildingAirLeakage>` form. **Currently FAILS** (bug confirmed: returns `Some(850.0)` for `infiltration_ach50`).
