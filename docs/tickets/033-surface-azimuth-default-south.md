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

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-20

### Code Confirmation

- [x] Referenced line numbers still match (correct location confirmed)
  - `build_surface_geometry` at `crates/hares-core/src/environment.rs:731–746`
  - The offending line is **line 740**: `azimuth_deg: boundary.azimuth_deg.unwrap_or(180.0),`
  - `EnvironmentManagerError` enum: `crates/hares-core/src/environment.rs:41–60` — confirmed no `MissingAzimuth` variant
  - `SurfaceGeometry.azimuth_deg` is non-optional `f64` at line 34
  - `EnvironmentManager::new` returns `Result<Self, EnvironmentManagerError>` at line 130 (already a `Result`)
- [x] Described logic matches current implementation
  - `build_surface_geometry` (line 731) is a non-`Result` private function returning `Vec<SurfaceGeometry>`
  - Called from `new_with_resample` at line 494 (via `perez_tilted_irradiance` loop)
  - `surface.azimuth_deg` (the silently-defaulted value) flows directly into `perez_tilted_irradiance` (line 502) and thence into `angle_of_incidence` at `crates/hares-physics/src/solar.rs:157`
  - The angle of incidence formula `az_delta = (solar_az - surface_azimuth_deg).to_radians()` (line 157 of solar.rs) requires a physically meaningful azimuth — confirming the bug's impact on solar gain computation
- [x] OCHRE cross-check result: **PARTIALLY matches — ticket's OCHRE claim is inaccurate**
  - `vendors/OCHRE/ochre/models/envelope.py` line 265–266:
    ```python
    default_azimuth = [0] if self.tilt == 0 else None
    self.azimuths = kwargs.get("Azimuth (deg)", default_azimuth)
    ```
  - OCHRE does NOT raise `ValueError` for a missing azimuth on exterior surfaces. For flat roofs (`tilt == 0`) it defaults to `[0]` (north); for all other surfaces it sets `self.azimuths = None` and defers error handling downstream.
  - The ticket states "OCHRE raises `ValueError` for any exterior surface missing orientation — stronger behavior than HARES." This is **incorrect**: OCHRE's behavior is also a silent non-error for pitched surfaces, though it is structurally different (it preserves `None` rather than substituting 180°).
  - The core concern raised in the ticket (silent substitution prevents callers from detecting missing geometry) is still valid regardless of this OCHRE inaccuracy.
- [x] EnergyPlus cross-check result: **partially matches — citation wording is imprecise but conceptually supported**
  - EnergyPlus `BuildingSurface:Detailed` does not have a standalone `Azimuth` input field; orientation is fully determined by vertex coordinates. The ticket's claim that EnergyPlus "requires orientation for all solar-receiving surfaces" is conceptually correct (surface geometry must be fully specified) but the cited input object does not match.
  - EnergyPlus Engineering Reference (Sky Radiance Model, §Perez anisotropic model) confirms that the diffuse sky irradiance calculation uses surface tilt and the incidence angle, which itself depends on surface azimuth relative to solar azimuth: `a = max(0, cos α)` where α is the angle of incidence. A wrong azimuth directly distorts `α` and hence `a`, `b`, and all three irradiance components (horizon, dome, circumsolar).
  - Source: https://bigladdersoftware.com/epx/docs/9-5/engineering-reference/sky-radiance-model.html

### Web-Verified Citations

**Citation 1**:
- **Citation**: "HPXML v4.2 schema requires `<Azimuth>` on all `<Wall>` elements"
- **Source found**: OpenStudio-HPXML documentation v0.11.0-beta and v1.8.1 — https://openstudio-hpxml.readthedocs.io/en/v1.8.1/workflow_inputs.html
- **Quoted passage**: "wall and roof surfaces do not require an azimuth/orientation to be specified. Rather, only the windows/skylights themselves require an azimuth/orientation."
- **Verdict**: **Incorrect.** HPXML `<Wall>/<Azimuth>` is **optional**, not required. The ticket's claim that HPXML v4.2 mandates `<Azimuth>` on walls is false. Windows must have azimuth; walls may omit it. This materially changes the required fix: for walls without azimuth (a valid HPXML input), returning `Err` is too strict — the correct behavior may be to emit a warning and treat the wall as omnidirectional (or four equal cardinal faces), mirroring the proposed roof treatment.

**Citation 2**:
- **Citation**: "EnergyPlus Input-Output Reference §'BuildingSurface:Detailed' — orientation required for all solar-receiving surfaces"
- **Source found**: EnergyPlus 8.9 Input-Output Reference — https://bigladdersoftware.com/epx/docs/8-9/input-output-reference/group-thermal-zone-description-geometry.html
- **Quoted passage**: EnergyPlus `BuildingSurface:Detailed` derives orientation entirely from vertex coordinates, not from a dedicated Azimuth field. The object does not have a standalone required `Azimuth` field — geometry defines orientation. No direct quote was obtainable supporting the ticket's formulation.
- **Verdict**: **Partially correct** in spirit (surface orientation must be geometrically defined), but the specific citation is misleading — EnergyPlus does not have a separate required `Azimuth` field in `BuildingSurface:Detailed`; vertices carry the orientation.

**Citation 3**:
- **Citation**: "ASHRAE Handbook of Fundamentals 2021 Ch. 15 §15.12 — solar gain equations require surface azimuth"
- **Source found**: ASHRAE Handbook of Fundamentals 2021 Table of Contents — https://www.ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals; supplemental RTS method PDF — https://xp20.ashrae.org/SupplementalFiles/PHVAC9/Radiant_Time_Series_(RTS)_Method.pdf
- **Quoted passage**: The full text of §15.12 is behind ASHRAE paywall and not publicly accessible for quotation. Chapter 15 of the 2021 ASHRAE HoF covers Fenestration. However, solar angle-of-incidence formulas in ASHRAE literature are confirmed by the Duffie et al. (2020) formulation referenced in related documentation:
  `θ = arccos(sin δ sin φ cos β − sin δ cos φ sin β cos γ + cos δ cos φ cos β cos ω + cos δ sin φ sin β cos γ cos ω + cos δ sin β sin γ sin ω)`
  where γ = surface azimuth angle. The azimuth (γ) appears in three terms of the angle-of-incidence formula. (Source: https://cghiaus.github.io/dm4bem_book/tutorials/01WeatherData.html, citing Duffie et al. 2020 eq. 1.6.2.)
- **Verdict**: **Partially correct** — the underlying physics claim (solar gain equations require surface azimuth) is verified correct. The specific section number §15.12 could not be verified (paywall), but the physical claim is sound and supported by independent engineering literature.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: The core bug is real and confirmed: `crates/hares-core/src/environment.rs:740` does silently substitute `180.0` for any surface missing an azimuth, and this value flows without modification into `perez_tilted_irradiance` and `angle_of_incidence` in `crates/hares-physics/src/solar.rs:157`, corrupting solar heat gain for all such surfaces. The "no-silent-defaults policy" violation is genuine. However, two significant details in the ticket are incorrect:
  1. **HPXML wall azimuth is optional, not required.** The OpenStudio-HPXML documentation explicitly states walls do not require `<Azimuth>`. This means returning `Err` for a wall with `None` azimuth is too aggressive — it would reject valid HPXML inputs. The correct fix for walls without azimuth is likely a `tracing::warn!` plus a documented approximation (e.g., omnidirectional treatment), similar to what the ticket proposes for roofs.
  2. **The OCHRE claim is inaccurate.** OCHRE does not raise `ValueError` for a missing azimuth on pitched exterior surfaces; it sets `self.azimuths = None` and propagates it silently.
  The proposed fix structure (differentiate walls from roofs) is reasonable, but the proposed error-vs-warn split should likely be reconsidered: **both** wall and roof missing azimuths should warn rather than error, given that HPXML makes azimuth optional for walls.

### Proposed Fix Summary

In `crates/hares-core/src/environment.rs`:
1. Change `build_surface_geometry` signature to return `Result<Vec<SurfaceGeometry>, EnvironmentManagerError>`.
2. Add `MissingAzimuth { boundary_idx: usize, surface_type: String }` to `EnvironmentManagerError`.
3. For **all** surface types (not just roofs): when `azimuth_deg == None`, emit `tracing::warn!` identifying the boundary by index and type, use `180.0` as a documented fallback, and note in the warning that the azimuth is unknown (not south-facing). Do NOT return `Err` for walls — HPXML permits missing wall azimuth.
4. Reserve `Err(MissingAzimuth)` for a future stricter mode or only for `BoundaryType::Window` (windows must have azimuth per HPXML).
5. No surface may reach Perez computation with a silently-substituted azimuth without a prior warning in the trace log.

### Test Written

- **File**: `crates/hares-core/tests/weather_integration.rs` (appended at end)
- **Tests added**:
  - `wall_missing_azimuth_causes_error` — `#[should_panic]` test demonstrating current behavior. Uses `#[should_panic(expected = "...")]` so it passes today and must be converted to a normal `assert!(result.is_err())` test once the fix lands. Documents that a wall with `azimuth_deg: None` currently silently succeeds instead of erroring.
  - `roof_missing_azimuth_constructs_successfully` — Positive test: a `BoundaryType::Roof` with `azimuth_deg: None` must not cause `EnvironmentManager::new` to fail. Passes both before and after fix.
