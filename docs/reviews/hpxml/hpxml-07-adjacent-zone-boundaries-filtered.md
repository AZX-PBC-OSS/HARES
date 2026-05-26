# Adjacent (adiabatic) zone boundaries silently filtered out
**Review ID**: hpxml-07
**Category**: hpxml
**Date**: 2026-05-25

## Files Reviewed
crates/hares-io/src/hpxml/building.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/utils/hpxml.py
vendors/OCHRE/ochre/Models/Envelope.py
vendors/OCHRE/ochre/utils/envelope.py

## Findings
### Finding 1: [Severity: medium]
**Description**: ZoneType::Adjacent is silently filtered from the thermal zone list during zone assembly (line 783), and boundaries with an Adjacent exterior resolve their exterior target to zone index 0 (the Conditioned zone) via an implicit fallback in `find_zone_idx` (`crates/hares-core/src/dwelling/conversions.rs:405`). No diagnostic, warning, or configuration option is emitted. The user receives no indication that party walls to adjacent dwelling units are being remapped to same-zone internal mass rather than treated as heat-transfer boundaries to a distinct thermal environment.

**Code Location**:
- `crates/hares-io/src/hpxml/building.rs:778-786` — Adjacent is filtered alongside Outdoor and Ground in the zone assembly filter.
- `crates/hares-core/src/dwelling/conversions.rs:392-410` — `find_zone_idx` returns `unwrap_or(0)` when the Adjacent zone type is not found in `building.zones`.
- `crates/hares-core/src/dwelling/conversions.rs:412-424` — `resolve_exterior` matches Adjacent into the catch-all `Some(zt)` arm, sending it through `find_zone_idx`.

**Root Cause**: The zone filter at `building.rs:778-786` correctly excludes `ZoneType::Adjacent` from being a thermal zone (matching OCHRE's approach of not creating a separate zone for adjacent dwelling units). However, `find_zone_idx` at `conversions.rs:405` falls back to index 0 without distinguishing between "zone not yet added" and "zone intentionally excluded." This silently merges Adjacent exterior targets onto whatever zone occupies index 0 (Conditioned), with no audit trail.

**Impact**:
- Party walls/floors to "other housing unit" (HPXML multifamily) are parsed correctly to `ZoneType::Adjacent` and receive construction-appropriate LUT names (e.g., "Adjacent Wall") via `envelope_lut.rs:318-331`.
- Because both interior and exterior resolve to the same thermal node (the Conditioned zone at index 0), `same_zone` is set to `true` in `boundary_rc.rs:527`, and the boundary is treated as internal thermal mass (BoundaryCategory::InternalMass) — construction halved, exterior film excluded, star-mesh LWR excluded.
- This is qualitatively consistent with OCHRE's behavior, which also models party walls as same-zone internal mass (`COMPONENT_LOAD_MAP` entry "Internal Mass" for "Adjacent Wall" in `envelope.py:33-35`).
- However, the HARES approach has two edge-case defects:
  1. If the interior zone is NOT the Conditioned zone (e.g., an Attic→Adjacent party wall), `interior_zone_idx` ≠ `exterior_zone_idx` (exterior still resolves to 0), and `same_zone` = `false`. The boundary is then incorrectly treated as a heat-transfer path between two different zones (e.g., Attic↔Conditioned) rather than as adiabatic.
  2. The user receives no diagnostic that Adjacent boundaries were collapsed to same-zone mass, making debugging difficult.

### Finding 2: [Severity: low]
**Description**: No configurable mechanism exists for specifying a temperature boundary condition or schedule for adjacent dwelling units. In cases where the adjacent space is unconditioned, vacant, or maintained at a substantially different temperature, the adiabatic-same-zone assumption (which assumes symmetric temperature on both sides of the party wall) is physically incorrect. OCHRE has the same limitation — both models treat party walls as same-zone internal mass with no configurable override.

**Code Location**: No relevant code exists — this is a missing feature. Relevant extension points would be in `building.rs` (zone construction), `conversions.rs` (exterior target resolution), and `solver_builder.rs` (boundary category assignment).

**Root Cause**: Neither HARES nor OCHRE implements a boundary-condition-driven party wall model. Both follow the standard residential energy modeling assumption that adjacent dwelling units are at similar temperatures, making the adiabatic approximation acceptable.

**Impact**: For typical multi-family simulations where adjacent units are conditioned similarly, the impact is negligible. For edge cases (unconditioned adjacent unit, significant thermostat setback in neighbor), the modeled heat transfer is zero when it should be non-zero, potentially underestimating heating/cooling loads.

### Finding 3: [Severity: medium]
**Description**: OCHRE's `get_boundaries_by_zones` (`hpxml.py:87-106`) explicitly rewrites `exterior = interior` when the exterior zone is `"Adjacent"` (line 96-97), ensuring that party wall boundaries are categorized as same-zone *before* boundary-type classification. HARES instead retains the original `(Conditioned, Adjacent)` or `(Attic, Adjacent)` zone pair throughout boundary processing and relies on the downstream `find_zone_idx` fallback to produce the same-zone effect. This approach is fragile and differs from OCHRE in both timing and mechanism:

- **OCHRE**: Rewrites at parse time → boundary becomes `(Indoor, Indoor)` or `(Attic, Attic)` → `get_boundaries_by_zones` groups it under a same-zone key → filtered to "Adjacent Wall", "Adjacent Attic Wall", etc. → Boundary constructor sees `same_zones = True` → construction halved, single-sided surface.
- **HARES**: Retains original zone types → LUT returns "Adjacent Wall", "Adjacent Attic Wall", etc. → `resolve_exterior` → `find_zone_idx` fallback returns 0 → `same_zone` depends on whether interior zone also resolves to index 0.

**Code Location**:
- `crates/hares-io/src/hpxml/building.rs:2361-2364` — parse_zone_label classifies "other housing unit" as Adjacent but does not rewrite it.
- `vendors/OCHRE/ochre/utils/hpxml.py:96-97` — OCHRE's explicit rewrite `exterior = interior`.

**Impact**: The HARES approach produces correct results only when the interior zone is the Conditioned zone. For other interior zone types, the same-zone detection fails and incorrect heat transfer is modeled. This is a latent correctness bug that would manifest for multi-family buildings with party walls between non-conditioned spaces (e.g., an attic party wall to an adjacent unit's attic).

## Summary
- Total findings: 3
- Critical / High / Medium / Low: 0 / 0 / 2 / 1

## Recommendations
1. **Match OCHRE's rewrite approach**: In the boundary parsing/classification stage (`building.rs`), when either the interior or exterior zone is `ZoneType::Adjacent`, rewrite it to match the other zone. For example, `(Conditioned, Adjacent)` → `(Conditioned, Conditioned)`. This makes the same-zone intent explicit and eliminates the fragile dependency on `find_zone_idx` fallback behavior. This can be done in `parse_zone_label` or at the point where boundary zone references are assigned.

2. **Add a diagnostic warning**: When Adjacent boundaries are collapsed to same-zone internal mass, emit a `tracing::info!` or `tracing::warn!` message listing the affected boundary IDs. This gives users visibility into the model simplification.

3. **Consider a configurable boundary schedule**: For multi-family accuracy, add an optional parameter to the `Zone` struct (or a per-boundary override) that specifies an ambient temperature or temperature schedule for the adjacent dwelling unit. When set, the boundary would have `ExteriorTarget::Outdoor` but with the specified schedule temperature rather than weather-derived ambient temperature. This extends the model without adding a new thermal zone. Flag as a low-priority enhancement deferred until a multi-family validation dataset becomes available.

4. **Add a test for Attic→Adjacent party wall**: Existing test `adjacent_zone_label_parsed` (`building.rs:3748`) only verifies that "other housing unit" parses to ZoneType::Adjacent on a Conditioned-interior wall. Add a test case with Interior=Attic and Exterior=Adjacent to exercise the edge case identified in Finding 1 and ensure the same-zone rewrite covers it.

## References / Citations
- `crates/hares-io/src/hpxml/building.rs:43-44` — ZoneType::Adjacent doc comment: "Adiabatic boundary to another dwelling unit (multifamily). OCHRE: 'other housing unit', 'other heated space', etc. → same-zone thermal mass."
- `crates/hares-io/src/hpxml/building.rs:778-786` — Zone assembly filter excluding Adjacent.
- `crates/hares-core/src/dwelling/conversions.rs:392-410` — `find_zone_idx` with `unwrap_or(0)` fallback.
- `crates/hares-core/src/dwelling/conversions.rs:412-424` — `resolve_exterior` with Adjacent falling into catch-all.
- `crates/hares-io/src/envelope_lut.rs:307-331` — LUT dispatch for same-zone and Adjacent boundaries.
- `vendors/OCHRE/ochre/utils/hpxml.py:87-106` — OCHRE's `get_boundaries_by_zones` with explicit `exterior = interior` rewrite.
- `vendors/OCHRE/ochre/utils/envelope.py:28-35` — OCHRE's COMPONENT_LOAD_MAP classifying Adjacent Wall/Ceiling/Floor as "Internal Mass".
- `vendors/OCHRE/ochre/utils/envelope.py:294-339` — OCHRE's `create_rc_data` with `same_zones` construction halving.
- `vendors/OCHRE/ochre/Models/Envelope.py:371-410` — OCHRE's Boundary constructor handling same_zone.
