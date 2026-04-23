# Envelope Surface Missing Azimuth Silently Defaults to 180° (South-Facing)

**Severity**: Medium
**Impact on annual kWh**: Medium
**Status**: Open
**Areas**: hares-core/environment, hares-io/hpxml

## Problem

`build_surface_geometry` at `crates/hares-core/src/environment.rs:740` silently substitutes 180° for any surface missing an azimuth:

```rust
azimuth_deg: boundary.azimuth_deg.unwrap_or(180.0),
```

South-facing surfaces receive maximum beam solar gain in the northern hemisphere. Assigning 180° to every surface without an explicit orientation makes the building appear to face south from all sides, producing systematically high solar heat gain in summer and artificially large solar heating benefit in winter. The magnitude depends on the fraction of surfaces lacking orientation in the HPXML input.

This violates the no-silent-defaults policy. An exterior wall or window boundary without an azimuth in HPXML is a data defect: HPXML v4.2 schema requires `<Azimuth>` on all `<Wall>` elements (hpxml.nrel.gov schema reference). Substituting a value instead of failing means callers receive results with no indication that their geometry is incomplete.

The tilt default of 90° (`crates/hares-core/src/environment.rs:737`) is physically reasonable for a wall and may be retained with a `tracing::warn!`.

HPXML roofs may have `<Pitch>` (slope angle from horizontal) without `<Azimuth>`, because hip and flat roofs have no single orientation. The HPXML resolver at `crates/hares-io/src/hpxml/building.rs` sets `azimuth_deg = None` for such roofs. Assigning 180° to a hip or flat roof is incorrect; these surfaces receive solar from all azimuth angles and require a hemispherical treatment rather than a single-direction azimuth.

## Current Behavior

`crates/hares-core/src/environment.rs:740`: `azimuth_deg: boundary.azimuth_deg.unwrap_or(180.0)` — silent south default for any surface missing orientation.

`build_surface_geometry` returns `Vec<SurfaceGeometry>` with no error path; missing azimuth is undetectable by callers.

OCHRE `ochre/models/envelope.py` raises `ValueError` for any exterior surface missing orientation — stronger behavior than HARES.

## Required Behavior

1. `build_surface_geometry` must return `Result<Vec<SurfaceGeometry>, EnvironmentManagerError>`. Add `MissingAzimuth { boundary_idx: usize, surface_type: String }` to `EnvironmentManagerError`.

2. For exterior walls and windows: `boundary.azimuth_deg == None` → return `Err(EnvironmentManagerError::MissingAzimuth { ... })`. The error message must identify the boundary by index and type so the caller can trace it to the HPXML input.

3. For roof/ceiling surfaces (detectable via an `is_roof` flag from the HPXML resolver, or heuristically from `tilt_deg < 45.0`): when azimuth is `None`, emit `tracing::warn!` naming the boundary index, use 180° as a documented approximation, and note in the warning that the surface is treated as south-facing though its actual orientation is omnidirectional. This approximation is not physically correct for hip or flat roofs; it is accepted as a known limitation pending a hemispherical model.

4. No surface may reach Perez transposition computation with a silently substituted azimuth.

Primary citations:
- HPXML v4.2 schema: `<Wall>/<Azimuth>` required; `<Roof>/<Azimuth>` optional when `<Pitch>` present — hpxml.nrel.gov schema reference
- EnergyPlus Input-Output Reference §"BuildingSurface:Detailed" — orientation required for all solar-receiving surfaces
- ASHRAE Handbook of Fundamentals 2021 Ch. 15 §15.12 — solar gain equations require surface azimuth

## Approach

In `crates/hares-core/src/environment.rs`, change `build_surface_geometry` signature to return `Result<Vec<SurfaceGeometry>, EnvironmentManagerError>`. In the `map` over boundaries, check `boundary.azimuth_deg`: if `None` and the surface is a wall or window, return the `MissingAzimuth` error. If `None` and the surface is a roof (use `is_roof` flag from the boundary type, or `tilt_deg.unwrap_or(90.0) < 45.0` heuristic), emit the warning and continue with 180°. Propagate the `Result` through `EnvironmentManager::new`.

## Definition of Done

- [ ] `build_surface_geometry` returns `Result<Vec<SurfaceGeometry>, EnvironmentManagerError>`
- [ ] `MissingAzimuth { boundary_idx: usize, surface_type: String }` variant added to `EnvironmentManagerError`
- [ ] Missing azimuth on an exterior wall or window → `Err(EnvironmentManagerError::MissingAzimuth { ... })`
- [ ] Missing azimuth on a roof surface → `tracing::warn!` with boundary index, construction continues
- [ ] Test: HPXML fixture with a wall missing `<Azimuth>` causes `EnvironmentManager::new` to return `Err` with a message identifying the boundary
- [ ] Test: HPXML fixture with a hip roof missing `<Azimuth>` constructs successfully with a warning in the trace output

## Verification

```bash
cargo test -p hares-core engine
cargo test -p hares-io hpxml_parsing_tests
```
