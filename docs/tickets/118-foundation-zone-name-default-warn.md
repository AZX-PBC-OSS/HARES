# Foundation Zone Name Silent Default to `attic_vented`

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-io/hpxml/resolve_hvac

## Problem

`crates/hares-io/src/hpxml/resolve_hvac.rs:1203` silently maps an absent foundation zone name to `"attic_vented"`. A foundation is unambiguously not an attic; the substitution produces a mis-classified zone that is then routed through wrong defaults for ground coupling, infiltration, and conditioning state.

## Current Behavior

`crates/hares-io/src/hpxml/resolve_hvac.rs:1203`:
```rust
let zone_name = foundation.name.unwrap_or("attic_vented");  // wrong default
```

A misnamed or absent foundation element silently maps to attic-vented, with no diagnostic.

## Required Behavior

1. If the foundation zone name is absent, emit `tracing::warn!` with the foundation type (slab / basement / crawlspace) and substitute a foundation-appropriate default name (`"basement_unconditioned"`, `"crawlspace_vented"`, `"slab_on_grade"` — whichever matches the FoundationType element).
2. If the foundation type itself is absent, return `HpxmlError::MissingField { field: "Foundation/FoundationType" }` — this is unrecoverable.
3. Never substitute `"attic_vented"` for a foundation.

## Approach

1. Open `crates/hares-io/src/hpxml/resolve_hvac.rs:1203` and replace the silent fallback with foundation-type-aware logic.
2. Add a small helper `default_foundation_zone_name(foundation_type: FoundationType) -> &'static str` that returns the right default name for each HPXML `FoundationType` variant.
3. Add `tracing::warn!` when the default is taken, including the foundation type and the substituted name.
4. Add a unit test for each FoundationType variant.
5. Add a unit test asserting the missing-FoundationType case errors.

## Definition of Done

- [ ] `attic_vented` no longer used as a foundation default
- [ ] Foundation-type-aware default applied via `default_foundation_zone_name`
- [ ] `tracing::warn!` emitted when the default is taken
- [ ] Missing FoundationType returns `HpxmlError::MissingField`
- [ ] Unit tests cover all FoundationType variants
- [ ] No `_ =>` catch-all in the foundation-type match arm

## Verification

```bash
cargo test -p hares-io resolve_hvac foundation
cargo test -p hares-io hpxml_parity
```

## References

- HPXML Specification v4.x §3.1.1 "Foundation" — `FoundationType` enumeration: SlabOnGrade, Basement (Conditioned/Unconditioned), Crawlspace (Vented/Unvented), Ambient.
- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.4 "Foundations" — foundation zones have distinct heat transfer characteristics from attics.
- Project policy `feedback_no_silent_defaults.md`.

## Related Tickets

- 034-kusuda-ground-model-parameter-derivation
- 035-slab-f-factor-method-not-called
- 050-synthetic-weather-ground-temp-equals-outdoor

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match — **with a critical correction** (see below)
- [x] Described logic matches current implementation — **partially**: the bug is real, but the description misidentifies the mechanism
- [x] OCHRE cross-check result: **OCHRE does not produce `"attic_vented"` for any foundation zone**. OCHRE (`vendors/OCHRE/ochre/utils/hpxml.py` lines 278–290) derives `foundation_name` as `"Crawlspace"`, `"Finished Basement"`, or `"Unfinished Basement"` from HPXML `FoundationType` child tags, or `None` for SlabOnGrade/Ambient/AboveApartment (no zone created). An unknown type raises `OCHREException`, never silently maps to attic. HARES diverges: the `_ =>` arm at line 1232 produces `"attic_vented"` for any unrecognized `ZoneType` variant.
- [x] EnergyPlus cross-check result: **N/A** — the ASHRAE 152 zone-type string mapping is an OCHRE-internal duct derating concept, not directly from EnergyPlus Engineering Reference. EnergyPlus itself uses zone objects with explicit names; the `"attic_vented"` strings are OCHRE LUT keys. No EnergyPlus engineering reference section applies here.

### Line Number Correction (Critical)

The ticket cites `resolve_hvac.rs:1203` as the location of the bad default. **This is wrong.**

- **Line 1203** (current code): `let fnd_name = building.foundation_name.as_deref().unwrap_or("");`
  — defaults to empty string `""`, not `"attic_vented"`. When `fnd_name` is `""` and `ZoneType::Foundation`, the code falls through to the `else` branch (lines 1223–1230) and returns `"vent_unins_crawlspace"` or `"unvent_unins_crawlspace"` — **a crawlspace fallback, not attic**.
- **Line 1232** (actual bug): `_ => "attic_vented".into()`
  — the catch-all arm of the outer `match zone.zone_type` block. Any `ZoneType` variant **not** explicitly handled (`Conditioned`, `Outdoor`, `Ground`, `Adjacent`, `Other(_)`) returns `"attic_vented"`.
- **Line 143**: `ZoneType::Conditioned` zones are skipped before `zone_type_to_ashrae152_str` is ever called (the function is only called from `compute_duct_dse_params` at line 154, which skips Conditioned zones at line 143).
- Remaining reachable cases through the `_ =>` arm in practice: `Outdoor`, `Ground`, `Adjacent`, `Other(_)` — these zone types can carry duct systems in unusual HPXML inputs and would silently emit `"attic_vented"` as their ASHRAE 152 zone-type key.

The ticket's **core concern is real**: `"attic_vented"` should not be produced by any non-attic zone type path. The framing (foundation zone name defaulting) is incorrect; the actual bug is an unexhaustive match arm.

### Web-Verified Citations

**Citation 1**: HPXML Specification v4.x §3.1.1 "Foundation" — `FoundationType` enumeration: SlabOnGrade, Basement (Conditioned/Unconditioned), Crawlspace (Vented/Unvented), Ambient.

- **Source found**: OCHRE OS-HPXML sample files (authoritative HPXML v4 examples in `vendors/OCHRE/test/OS-HPXML Sample Files/`) + [hpxmlwg/hpxml GitHub](https://github.com/hpxmlwg/hpxml) + [OpenStudio-HPXML documentation](https://openstudio-hpxml.readthedocs.io/en/v0.9.0-beta/hpxml_to_openstudio.html)
- **Quoted passage**: From `base-foundation-slab.xml`: `<FoundationType><SlabOnGrade/></FoundationType>`. From `base-foundation-ambient.xml`: `<FoundationType><Ambient/></FoundationType>`. From `base-foundation-multiple.xml`: `<FoundationType><Basement><Conditioned>false</Conditioned></Basement></FoundationType>` and `<FoundationType><Crawlspace><Vented>false</Vented></Crawlspace></FoundationType>`. From `base-foundation-conditioned-crawlspace.xml`: `<FoundationType><Crawlspace><Conditioned>true</Conditioned></Crawlspace></FoundationType>`.
- **Verdict**: **Confirmed** — the child elements of `FoundationType` are `SlabOnGrade`, `Basement`, `Crawlspace`, and `Ambient` (plus `AboveApartment` as a less common variant). The HARES parser at `building.rs:479–500` correctly recognises these. The ticket's enumeration list is accurate; the section number "§3.1.1" could not be independently verified from public sources (HPXML spec is not freely available), but the element names match the schema samples.

**Citation 2**: ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.4 "Foundations" — foundation zones have distinct heat transfer characteristics from attics.

- **Source found**: [2021 ASHRAE Handbook—Fundamentals Table of Contents](https://www.ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals) + [ASHRAE Chapter 17 online](https://handbook.ashrae.org/Handbooks/F21/IP/F21_Ch17/F21_Ch17_ip.aspx) + [ASHRAE Chapter 18 online](https://handbook.ashrae.org/Handbooks/F21/IP/F21_Ch18/F21_Ch18_ip.aspx)
- **Quoted passage**: Chapter 17 states "Heating calculations must include loss via slabs and basement walls and floors" and "heat flow into the ground is usually ignored [for cooling]... surfaces adjacent to the ground are modeled as if well insulated." Chapter 17 refers to Chapter 18 for "simplified procedures for estimating heat loss through below-grade walls and below- and on-grade floors." Chapter 18 itself covers **Nonresidential** Cooling and Heating Load Calculations and contains no content about foundations or below-grade surfaces.
- **Verdict**: **Incorrect chapter cited**. The ticket cites Ch. 18 §18.4 "Foundations," but:
  - Chapter 18 of the 2021 ASHRAE HoF covers *nonresidential* load calculations and has no §18.4 about foundations.
  - Foundation heat transfer appears in **Chapter 17** (Residential Cooling and Heating Load Calculations), specifically in the "Below-Grade and On-Grade Surfaces" subsection of Section 8 (Heating Load).
  - There is no dedicated "foundations" chapter in the 2021 ASHRAE HoF at all.
  - The underlying point — that foundation zones have distinct heat transfer from attics — is **correct** (ground-coupled soil vs. ventilated air is a fundamentally different thermal path), even though the chapter citation is wrong.

**Citation 3**: Project policy `feedback_no_silent_defaults.md`.

- **Source found**: Memory reference only (internal project policy file, not externally verifiable).
- **Verdict**: Cannot verify via web search; assumed internally consistent with the no-silent-defaults policy documented in the project.

### Legitimacy

- **Verdict**: Partially Legitimate

- **Rationale**: The core issue is real and confirmed by a failing regression test: `zone_type_to_ashrae152_str` at `resolve_hvac.rs:1232` has a `_ => "attic_vented".into()` catch-all that incorrectly maps any unrecognized `ZoneType` (including `Outdoor`, `Ground`, `Adjacent`, and `Other(_)`) to `"attic_vented"`. This string is then used as an ASHRAE 152 LUT key for duct derating, potentially routing non-attic zones through attic-specific efficiency parameters. The OCHRE reference implementation raises an exception for unknown types rather than silently defaulting. However, the ticket misidentifies the bug location (citing line 1203 instead of line 1232), mischaracterises the mechanism (framing it as a foundation zone name defaulting, when it is actually a `ZoneType` match catch-all), and cites the wrong ASHRAE chapter (Ch. 18 §18.4 instead of Ch. 17). The proposed fix (foundation-type-aware default name + `tracing::warn!`) addresses only part of the problem; the more pressing issue is the `_ =>` arm, which should either be removed (by making the match exhaustive over all `ZoneType` variants) or replaced with a `tracing::warn!` + sensible non-attic default.

### Proposed Fix Summary

Do NOT implement this fix — for reference only:

1. Make the `match zone.zone_type` in `zone_type_to_ashrae152_str` (lines 1193–1233) exhaustive by adding explicit arms for `ZoneType::Conditioned`, `ZoneType::Outdoor`, `ZoneType::Ground`, `ZoneType::Adjacent`, and `ZoneType::Other(_)`. Each should either `unreachable!()` (if the call site guarantees they never occur), or emit a `tracing::warn!` and return a sensible non-attic default (e.g. `"garage"` as the least-wrong unconditioned zone for duct derating purposes).
2. Separately, the foundation zone name empty-string fallback at line 1203 (`unwrap_or("")`) already falls through to a crawlspace default at lines 1223–1230, which is documented with a comment. The ticket's proposed `default_foundation_zone_name` helper and `tracing::warn!` could additionally be applied there, but this is lower severity than the `_ =>` arm issue.
3. Delete the `_ => "attic_vented".into()` arm (line 1232).
4. Update the `Definition of Done` in this ticket to reflect the corrected location (line 1232) and the exhaustive-match approach.

### Test Written

- **File**: `crates/hares-io/src/hpxml/resolve_hvac.rs` (within the `#[cfg(test)]` module, after line 2976)
- **Test name**: `ticket_118_non_attic_zone_types_must_not_return_attic_vented`
- **What it tests**: Asserts that `ZoneType::Outdoor`, `ZoneType::Ground`, `ZoneType::Adjacent`, and `ZoneType::Other("Unknown")` do not produce `"attic_vented"` from `zone_type_to_ashrae152_str`. The test **currently fails** (confirmed: `cargo test -p hares-io --lib ticket_118` → FAILED, `left: "attic_vented"` for `ZoneType::Outdoor`), demonstrating the bug is present and unresolved.
