# Refrigerator gain fraction zeroing uses fragile string matching
**Review ID**: hpxml-05
**Category**: hpxml
**Date**: 2026-05-26

## Files Reviewed
crates/hares-io/src/hpxml/resolve_loads.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/utils/hpxml.py

## Findings
### Finding 1: [Severity: high]
**Description**: `is_conditioned_location()` uses exact string matching on only three strings (`"conditioned space"`, `"living space"`, `"indoor"`). A refrigerator reported by HPXML as located in `"basement - conditioned"`, `"conditioned basement"`, `"finished basement"`, or any semantically conditioned-but-differently-named zone is treated as non-conditioned, causing its sensible and latent gain fractions to be zeroed. This silently drops internal heat gains from the simulation for those refrigerators.

**Code Location**: `crates/hares-io/src/hpxml/resolve_loads.rs:754-758` — the `is_conditioned_location()` function, called at line 376.

```rust
fn is_conditioned_location(location: &str) -> bool {
    matches!(
        location.to_ascii_lowercase().as_str(),
        "conditioned space" | "living space" | "indoor"
    )
}
```

**Root Cause**: The function independently reimplements location-to-zone classification using brittle exact matching, rather than delegating to the existing, comprehensive `parse_zone_label()` function already available at `building.rs:2347-2368`. That function uses substring-keyword matching (`contains("condition")`, `contains("basement")`, etc.) to classify arbitrary HPXML location strings into the `ZoneType` enum, and is used throughout the envelope boundary parsing pipeline. The refrigerator handling code was written as an isolated check without leveraging this existing infrastructure.

**Impact**:
1. **Correctness**: Refrigerators in conditioned basements or other conditioned zones with non-standard location strings silently have their internal heat gains set to zero at lines 377-378. This under-reports zone thermal loads.
2. **Divergence from codebase patterns**: The rest of the codebase (`parse_zone_label`, `parse_duct_location`) uses keyword-based matching that would correctly classify these locations. The refrigerator path alone uses fragile exact matching, creating an inconsistent classification surface.
3. **Maintainability**: Any HPXML file using location strings outside the hardcoded set of three requires code changes to handle.

**Existing codepath that would correctly classify** (not used by refrigerator handling):
`crates/hares-io/src/hpxml/building.rs:2347-2368`:

```rust
pub(crate) fn parse_zone_label(text: &str) -> ZoneType {
    let norm = normalize_ascii(text);
    if norm.contains("attic") {
        ZoneType::Attic
    } else if norm.contains("garage") {
        ZoneType::Garage
    } else if norm.contains("foundation") || norm.contains("basement") || norm.contains("crawl") {
        ZoneType::Foundation
    } else if norm.contains("condition") || norm == "living space" {
        ZoneType::Conditioned
    } else if norm == "ground" {
        ZoneType::Ground
    } else if norm.contains("out") || norm.contains("ambient") {
        ZoneType::Outdoor
    } else if norm.contains("other") {
        ZoneType::Adjacent
    } else {
        ZoneType::Other(text.trim().to_string())
    }
}
```

### Finding 2: [Severity: medium]
**Description**: The existing test suite (`resolve_loads.rs:1015-1024`) tests `is_conditioned_location` only with the already-matched strings and a few negative cases. There is no test for `"basement - conditioned"`, `"conditioned basement"`, `"finished basement"`, or any location string from ResStock HPXML datasets that includes the word "conditioned" but would fail the exact-match check.

**Code Location**: `crates/hares-io/src/hpxml/resolve_loads.rs:1015-1024`.

**Root Cause**: Test coverage was written to match the function's current behavior rather than to validate semantic correctness against real-world HPXML location strings.

**Impact**: The bug has no test to catch it, and no test that would fail if the implementation were fixed to use `parse_zone_label`.

### Finding 3: [Severity: low]
**Description**: The comment at line 752 claims the function "Mirrors OCHRE `parse_zone_name` returning `'Indoor'`", but the mapping is not equivalent. OCHRE's `parse_zone_name` (line 65-84) uses a dictionary of `ZONE_NAME_OPTIONS` (lines 12-28) that maps 20+ location strings to zone types including Foundation, Garage, and Attic — not just Indoor. The HARES function's behavior matches OCHRE's for only the 3 Indoor-mapped strings and fails for all other mapped strings (including Foundation zone strings that would be conditioned basements).

**Code Location**: `crates/hares-io/src/hpxml/resolve_loads.rs:752-753`.

**Root Cause**: The function name and comment suggest it was intended to mirror OCHRE's zone name parsing but it only implements the Indoor check, not the full mapping.

## Summary
- Total findings: 3
- Critical: 0 / High: 1 / Medium: 1 / Low: 1

## Recommendations
1. Replace `is_conditioned_location()` with a call to the existing `parse_zone_label()` function from `building.rs`, then check whether the returned `ZoneType` is `ZoneType::Conditioned` or whether it represents a conditioned space (e.g., a `ZoneType::Foundation` with a "Finished Basement" foundation type). This brings the refrigerator path into alignment with how the rest of the codebase classifies HPXML locations.
2. Add test cases for location strings that contain "conditioned" but are not among the three currently hardcoded strings, including at minimum `"basement - conditioned"`, `"conditioned basement"`, `"finished basement"`, and `"Indoor"` (mixed case as OCHRE passes it).
3. Correct or remove the misleading doc comment claiming the function mirrors OCHRE's `parse_zone_name`, since OCHRE's function handles the full `ZONE_NAME_OPTIONS` dictionary, not just the Indoor subset.

## References / Citations
- HARES `is_conditioned_location`: `crates/hares-io/src/hpxml/resolve_loads.rs:754-758`
- HARES `parse_zone_label` (existing keyword-based mapper): `crates/hares-io/src/hpxml/building.rs:2347-2368`
- HARES `parse_duct_location` (similar keyword-based approach for ducts): `crates/hares-io/src/hpxml/building.rs:2370-2384`
- HARES `ZoneType` enum: `crates/hares-io/src/hpxml/building.rs:35-47`
- OCHRE `parse_zone_name` with full `ZONE_NAME_OPTIONS` dictionary: `vendors/OCHRE/ochre/utils/hpxml.py:12-84`
- OCHRE refrigerator location check (for comparison): `vendors/OCHRE/ochre/utils/hpxml.py:1400-1401`
- HPXML schema for `<Refrigerator>/<Location>`: HPXML Building America data dictionary — Location field accepts zone names like "conditioned space", "living space", "basement - conditioned", "basement - unconditioned", "garage", etc.
