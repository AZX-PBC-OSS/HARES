# SolverBuilder wiring: boundary types, zone labels, surface properties, film coefficients
**Review ID**: coredeep-10
**Category**: core-deep
**Date**: 2026-05-26

## Files Reviewed
crates/hares-core/src/dwelling/solver_builder.rs

## Vendor/Reference Files Consulted
None

## Findings
### Finding 1: [Severity: high]
**Description**: Multiple zones of the same HPXML type (e.g., two `Conditioned` zones in a duplex) cause surfaces to be incorrectly assigned to the first matching zone. Both `boundary_zone_index()` and `find_zone_idx()` use `position()` which returns only the first match, ignoring subsequent zones of the same type.
**Code Location**: `crates/hares-core/src/dwelling/conversions.rs:590-604` (`boundary_zone_index`), `crates/hares-core/src/dwelling/conversions.rs:392-410` (`find_zone_idx`), `crates/hares-core/src/dwelling/solver_builder.rs:148` (call site)
**Root Cause**: Both functions iterate `building.zones` with `.position()` (returns first match) rather than resolving the interior zone by a zone-level identifier (e.g., `zone_id`). All surfaces that reference `ZoneType::Conditioned` for their interior zone get mapped to zone index 0, even in multi-unit buildings where there are multiple conditioned thermal zones.
**Impact**: In a duplex or multi-family building with two separate conditioned zones, all surfaces attached to "Conditioned" — including walls belonging to the second unit — are aggregated into the first unit's thermal node. This creates phantom thermal coupling: heat that should flow into unit 2 instead flows into unit 1's zone air, producing incorrect energy balances and zonal temperatures. The reverse path (`find_zone_idx` used in `building_to_boundary_inputs` at line 127) has the same issue for the `interior_zone_idx` field in `BoundaryInput`, affecting the RC network wiring.

### Finding 2: [Severity: medium]
**Description**: Silent fallback to `ExteriorTarget::Outdoor` and `ZoneLabel::Outdoor` when a boundary has no explicit `exterior_zone` or the zone type is unrecognised. A boundary with missing or invalid exterior zone data is treated as outdoor-facing without any warning, producing plausible-but-wrong results.
**Code Location**: `crates/hares-core/src/dwelling/conversions.rs:412-426` (`resolve_exterior`), `crates/hares-core/src/dwelling/conversions.rs:374-389` (`zone_type_to_label`)
**Root Cause**: The `match` arms for `None` and `Some(ZoneType::Other(_))` both map to outdoor equivalents. There is no validation or `tracing::warn!` for unrecognised exterior zone variants. The `zone_type_to_label` function similarly maps `Some(ZoneType::Adjacent) => ZoneLabel::Conditioned` and `Some(ZoneType::Outdoor) | None => ZoneLabel::Outdoor`, which are reasonable defaults for known semantics, but `ZoneType::Other(_)` is also flattened to `ZoneLabel::Outdoor` with no diagnostic.
**Impact**: An HPXML boundary with a typo in the zone type name (which lands in `ZoneType::Other`) or a boundary that simply omits `<ExteriorZone>` silently receives outdoor film coefficients and outdoor driving temperature, rather than failing with a clear parsing error. This could produce silently incorrect heat loss/gain estimates, for example treating an interzone partition as an external wall.

### Finding 3: [Severity: medium]
**Description**: Surfaces with missing or unrecognised `interior_zone` are silently assigned to zone index 0. Both `boundary_zone_index()` and `find_zone_idx()` fall back to index 0 when the interior zone type is `None` or not found in the building's zone list.
**Code Location**: `crates/hares-core/src/dwelling/conversions.rs:590-604` (`boundary_zone_index`), `crates/hares-core/src/dwelling/conversions.rs:392-410` (`find_zone_idx`), `crates/hares-core/src/dwelling/solver_builder.rs:148-153`
**Root Cause**: Both functions return `0` when `zone_type` is `None` or when no zone matches the target type, without emitting a warning or error. The `zone_idx` then determines which thermal node the boundary's interior side connects to.
**Impact**: A boundary whose HPXML `interior_zone` is misspelled, missing, or refers to a zone type not present in the building definition gets silently wired into zone 0 (typically the conditioned zone). This creates phantom heat coupling between unconditioned surfaces and the conditioned zone, corrupting the energy balance. If zone 0 happens to be a foundation instead of conditioned, the coupling is still wrong but less obvious.

### Finding 4: [Severity: medium]
**Description**: `Door` and `Other` HPXML boundary types get `boundary_category = None`, yet still participate fully in the thermal model through RC layer wiring. They are skipped from boundary diagnostic reporting, creating an observability gap.
**Code Location**: `crates/hares-core/src/dwelling/solver_builder.rs:206-209` (category assignment), `crates/hares-core/src/dwelling/solver_builder.rs:842-881` (diagnostic loop), `crates/hares-core/src/dwelling/solver_builder.rs:692-757` (exterior surface loop)
**Root Cause**: The `boundary_category` match (lines 188-210) maps `Wall`, `FoundationWall`, `RimJoist`, `Roof`, `Floor`, `Slab`, and `Window` to category variants, but `Door` and `Other` map to `None`. The boundaries proceed through RC network construction and get wiring, but the diagnostic loop at line 842 (`if let Some(cat) = sb.boundary_category`) skips them entirely. In the exterior surface loop (line 692), the check is `if sb.is_exterior` which does not filter on category, so exterior-facing doors do get `ExteriorSurfaceInfo` entries with `boundary_category = None` — stored but not used for diagnostics.
**Impact**: A door boundary with explicit material layers will contribute to the A-matrix and heat transfer but will not appear in boundary diagnostic reports. This makes it harder to audit the thermal model: the door's UA, driving temperature, and category are invisible in diagnostics. The thermal physics is correct (the door still transfers heat), but the observability is incomplete. If the `Door` boundary were missing material layers, the fallback R-value path applies normally — the boundary exists and conducts heat, but the user cannot confirm its behaviour through diagnostics.

### Finding 5: [Severity: low]
**Description**: Non-slab ground-facing boundaries (foundation walls, rim joists with `exterior_zone = Ground`) retain the interior-convection-based exterior film resistance from `film_resistances()`, while slab boundaries explicitly zero their exterior film. The different treatment is physically justified but undocumented at the wiring layer.
**Code Location**: `crates/hares-core/src/dwelling/conversions.rs:289-298` (slab film zeroing), `crates/hares-physics/src/film_coefficients.rs:293-294` (ground film = 1/h_conv), `crates/hares-core/src/dwelling/solver_builder.rs:231` (film read)
**Root Cause**: In `building_to_boundary_inputs`, the film resistance for ground exterior zones is set to `1.0 / h_conv` via `film_resistances()` (same as interior convection, no DOE-2 forced-convection enhancement). For slabs only, this is then zeroed out (line 294-298) because the F-factor perimeter method captures the full slab-to-ground pathway. Foundation walls keep the convection-based exterior film because the wall has an air film between itself and the soil/backfill. The wiring layer in `solver_builder.rs` does not distinguish these two cases; it simply reads `r_film_ext` and uses it.
**Impact**: The physics is correct — foundation walls should have a soil-side convective resistance, and slabs should not. However, the asymmetry between slab and non-slab ground boundaries is not documented at the wiring layer (solver_builder.rs), and a future refactor of the exterior film handling could accidentally zero foundation-wall exterior films (treating all ground boundaries like slabs). No functional bug exists, but the architectural clarity could be improved with a comment noting that foundation walls intentionally retain a ground-side film.

### Finding 6: [Severity: low]
**Description**: `ExteriorSurfaceInfo` entries are only created for boundaries with `is_exterior == true` (i.e., `exterior_zone == ZoneType::Outdoor`). Ground-facing boundaries (foundation walls with `exterior_zone = ZoneType::Ground`) are correctly excluded from exterior solar processing, but this filtering is implicit — there is no explicit check for ground-attached boundaries in the solar injection loop.
**Code Location**: `crates/hares-core/src/dwelling/solver_builder.rs:131-135` (is_exterior definition), `crates/hares-core/src/dwelling/solver_builder.rs:693` (exterior surface push condition)
**Root Cause**: The `is_exterior` flag is defined solely by checking `exterior_zone == ZoneType::Outdoor`. Ground boundaries therefore never satisfy `is_exterior`, and never get `ExteriorSurfaceInfo` entries. The solar injection indices map at line 731-732 (`wiring.solar_input_indices.insert(sb.surface_id, input_index)`) is also inside the `if sb.is_exterior` block, so ground surfaces correctly receive no solar radiation. Both are correct behaviours, but the reasoning is not documented at the wiring level.
**Impact**: No functional defect. However, if a future HPXML boundary type has an exterior zone that is neither Outdoor nor Ground (e.g., an "Under Slab" variant), the current binary `is_exterior` check would treat it the same as Ground, which may or may not be appropriate. A more explicit surface-type-based solar decision would be more maintainable.

### Finding 7: [Severity: low]
**Description**: `inner_wiring` (and therefore interior-surface RC node injection columns in the B-matrix) is only created for boundaries whose interior zone is `Conditioned`. Attic-interior, garage-interior, and foundation-interior boundaries get LWR flux injected directly into the zone air node rather than into the surface RC node.
**Code Location**: `crates/hares-core/src/dwelling/solver_builder.rs:169-179` (inner_wiring creation), `crates/hares-core/src/dwelling/solver_builder.rs:778-788` (LWR fallback path)
**Root Cause**: The condition `if is_conditioned_interior` at line 169 gates `inner_wiring` creation. For non-conditioned interiors (attic, garage, foundation), the interior LWR injection falls through to the zone air node at lines 782-787, using `zone_state_indices` and `zone_sensible_input_indices` instead of the surface RC node's state row.
**Impact**: This is architecturally intentional — unconditioned zones typically have less thermal mass modelling, and routing LWR directly to the zone air node is a reasonable simplification. However, if an unconditioned zone (e.g., a garage with interior drywall) has explicit RC layers on its interior surface, the LWR flux bypasses that surface mass and goes directly to the zone air. The thermal dynamics are slightly distorted: solar radiation absorbed on the interior surface should heat the surface mass first, then convect to the zone air. The current path skips the surface mass and heats the zone air immediately. The impact is small for most unconditioned spaces, but worth documenting.

## Summary
- Total findings: 7
- Critical / High / Medium / Low: 0 / 1 / 3 / 3

## Recommendations
1. **Replace position-based zone indexing with explicit zone ID matching.** Use a `HashMap<ZoneId, Vec<Boundary>>` or `HashMap<String, ZoneId>` that maps HPXML zone identifiers (not just type tags) to solver zone indices. For multi-unit buildings, each unit needs a distinct thermal node, and the current type-only matching conflates them.
2. **Add explicit error handling for unrecognised or missing zone types.** When `exterior_zone` or `interior_zone` is `None` or `ZoneType::Other(_)`, emit at minimum a `tracing::warn!` and consider returning an error for ambiguous configurations.
3. **Assign boundary categories to Door and Other types.** Map `Door` to `BoundaryCategory::Wall` (same thermal physics as a wall with a lower effective R-value) and `Other` conservatively to a category or log a warning when the boundary has material layers.
4. **Document the foundation-wall vs slab exterior film asymmetry** in the wiring layer (solver_builder.rs) near the `r_film_exterior_m2_k_w` usage, so that future maintainers understand why non-slab ground boundaries keep a film and slabs do not.
5. **Consider routing interior LWR through the surface RC node for unconditioned zones** that have explicit RC layers, instead of routing directly to the zone air node. This would more accurately capture the surface-mass thermal buffering effect for garages and similar spaces with interior drywall.

## References / Citations
- ASHRAE HoF 2021 Ch. 15, Table 1 (fenestration film coefficients)
- ASHRAE HoF 2021 Ch. 18.31 (slab-on-grade F-factor perimeter method)
- EnergyPlus Engineering Reference §9.4 (ASHRAE Simple / TARP interior convection)
- EnergyPlus InputOutputRef, `ZoneCapacitanceMultiplier` (default=1.0) — mutual exclusivity with `InternalMass` objects
- HPXML Data Dictionary v4.2, `<Siding>` enumeration, `<ZoneType>` values
