# Envelope Construction Rewrite: OCHRE Parity Plan

## Context

HARES cools 1.8x faster than OCHRE for the same building (BEopt example). The effective UA is ~1000+ W/K vs OCHRE's 568 W/K. Root causes identified through code exploration and cross-validated by Kimi K2.5 review:

1. **Slab connects to outdoor air instead of ground** (known bug, documented in test)
2. **Window U-factor not decomposed** — generic film R used instead of EnergyPlus Simple Window Model Step 1
3. **Raised floor maps to wrong LUT boundary name** ("Attic Floor" instead of "Raised Floor")
4. **Foundation wall/slab insulation details not parsed** from HPXML (DepthBelowGrade, PerimeterInsulation, UnderSlabInsulation)
5. **Insulation details string generated for regular walls** when OCHRE doesn't pass them (could cause LUT mismatch)

### Clarification: Film Resistance Handling (from Kimi review)

Both OCHRE and HARES use:
- **0.9/1.5 IP (0.1585/0.2642 SI) fixed constants** for LUT R-value matching only
- **TARP/DOE-2 dynamic models** for actual simulation film coefficients

This is NOT a divergence — both codebases work the same way. The original plan incorrectly claimed HARES had "better" film coefficients.

### What to PRESERVE (do NOT regress)
- Per-orientation walls (needed for solar irradiance)
- TARP/DOE-2 film coefficients for simulation (both OCHRE and HARES use these)
- Material layer parsing from HPXML (fallback when LUT misses)
- EnergyPlus IAM corrections for windows
- `calculate_window_parameters` in `hares-physics/src/solar.rs` (already matches OCHRE)
- Interior LWR radiation exchange
- Observer capture infrastructure (`observer_capture.rs`)

## Principles

- **Incremental, validated**: Each fix is a separate commit with its own oracle test
- **Debuggable**: Add `EnvelopeDiagnostics` struct for per-boundary UA/R/C capture; integrate with existing observer capture pattern
- **DRY**: Shared helpers for HPXML insulation parsing; no duplicate zone-label logic
- **Testable**: Each phase adds or extends oracle tests comparing against OCHRE reference values
- **Separation of concerns**: HPXML parsing → boundary resolution → RC construction → solver wiring remain distinct layers
- **Correct, performant**: No allocations in hot paths; strong types; debug_assert for invariants

---

## Phase 0: Diagnostic Infrastructure

### 0.1: `EnvelopeDiagnostics` capture struct

Add to `crates/hares-envelope/src/boundary_rc.rs`:

```rust
/// Per-boundary diagnostic data captured during RC construction.
#[derive(Debug, Clone)]
pub struct BoundaryDiagnostic {
    pub boundary_idx: usize,
    pub ua_w_k: f64,           // area / R_total
    pub r_total_m2_k_w: f64,   // sum of all layer R + film R
    pub capacitance_j_k: f64,  // sum of all layer C
    pub n_rc_nodes: usize,
    pub interior_zone_idx: usize,
    pub exterior_target: ExteriorTarget,
    pub area_m2: f64,
    pub r_film_int: f64,
    pub r_film_ext: f64,
    pub path: RCPath,          // Precomputed | MaterialLayer | FallbackR
}

#[derive(Debug, Clone, Copy)]
pub enum RCPath { Precomputed, MaterialLayer, FallbackR }

#[derive(Debug, Clone)]
pub struct EnvelopeDiagnostics {
    pub boundaries: Vec<BoundaryDiagnostic>,
    pub zone_capacitances_j_k: Vec<f64>,
    pub total_ua_w_k: f64,
}
```

Modify `assemble_building_rc` to return `(BuildingRC, EnvelopeDiagnostics)`. Each boundary path (precomputed, layered, fallback) populates its diagnostic entry during construction.

### 0.2: Boundary-by-boundary diagnostic test (capture-only, no assertions yet)

Extend `tests/structural_envelope_oracle.rs` with `beopt_boundary_ua_diagnostics` that:

1. Builds the RC network and captures `EnvelopeDiagnostics`
2. Prints a comparison table: boundary index, type, area, R_total, UA, path used
3. Prints total UA and zone capacitances
4. **No assertions** — this is a diagnostic-only test used during development. Hard assertions are added in Phase 5 after all fixes land.

### 0.3: Python oracle extraction script

Add `tests/fixtures/parity/extract_ochre_rc.py` that:
- Loads BEopt_example.xml through OCHRE
- Dumps per-boundary resistances, capacitances, areas, zone connections
- Outputs JSON for Rust test consumption

**Files modified:**
- `crates/hares-envelope/src/boundary_rc.rs` — add diagnostic types + capture
- `tests/structural_envelope_oracle.rs` — add diagnostic test
- `tests/fixtures/parity/extract_ochre_rc.py` — new oracle extraction script

---

## Phase 1: Fix Slab Ground Connection

### Problem
`parse_zone_label` at `building.rs:1307` maps `"ground"` to `ZoneType::Outdoor`. Then `infer_exterior_zone` at line 745 maps Slab to `ZoneType::Outdoor` with comment "ground handled later by resolve_exterior". But `resolve_exterior` only catches `Foundation` zone type for slabs.

### Fix

In `parse_zone_label` (`building.rs:1294`):
```rust
// Move "ground" from outdoor branch to foundation branch:
} else if norm.contains("foundation")
    || norm.contains("basement")
    || norm.contains("crawl")
    || norm == "ground"
{
    ZoneType::Foundation
} else if norm.contains("out") || norm.contains("ambient") {
    ZoneType::Outdoor
```

This works because `resolve_exterior` at `conversions.rs:192-195` already maps Foundation+Slab → `ExteriorTarget::Ground`.

Update `infer_exterior_zone` comment at line 745 to reflect the fix.

### Validation
- `beopt_boundary_inputs` test: assert `ground_count >= 1` (remove known-bug comment at lines 327-336)
- Re-run diagnostic test: slab should now show `ExteriorTarget::Ground`

**Files modified:**
- `crates/hares-io/src/hpxml/building.rs:1294-1313` — fix `parse_zone_label`
- `crates/hares-io/src/hpxml/building.rs:745` — update comment
- `tests/structural_envelope_oracle.rs` — update slab assertion

---

## Phase 2: Window U-Factor Decomposition

### Problem
Windows parsed as `Boundary` structs have `assembly_r_value_m2_k_w = None` and empty `r_value_layers_m2_k_w`. So they fall through to `DEFAULT_R_M2_K_W` (2.5 m²K/W) in `conversions.rs:52-59`. For a typical U=2.1 W/m²K window, correct R ≈ 0.48 m²K/W — **HARES uses 5x the correct resistance**.

Additionally, generic TARP/DOE-2 film resistances are used instead of window-specific values. OCHRE decomposes window U-factor using EnergyPlus Simple Window Model Step 1:
- Interior film R = U-factor-dependent logarithmic fit (NOT generic TARP)
- Exterior film R = 0 (NOT generic DOE-2)
- Glass R = 1/U - r_int - 0

This affects BOTH the RC boundary resistance path AND the solar parameter calculation in `solver_builder.rs:317-323` which currently uses `bd_input.r_film_interior_m2_k_w` (generic) instead of window-specific film R.

### Fix

**Step 1**: Add to `crates/hares-physics/src/solar.rs` (alongside existing `calculate_window_parameters`):

```rust
/// EnergyPlus Simple Window Model Step 1: U-factor → (r_glass, r_film_int).
/// Exterior film = 0 for windows per EnergyPlus convention.
/// Reference: bigladdersoftware.com/epx/docs/8-9/engineering-reference/window-calculation-module.html
pub fn window_u_factor_decomposition(u_factor_w_m2_k: f64) -> (f64, f64) {
    let r_int = if u_factor_w_m2_k < 5.85 {
        1.0 / (0.359073 * u_factor_w_m2_k.ln() + 6.949915)
    } else {
        1.0 / (1.788041 * u_factor_w_m2_k - 2.886625)
    };
    let r_glass = (1.0 / u_factor_w_m2_k - r_int).max(0.0);
    (r_glass, r_int) // exterior film = 0
}
```

**Step 2**: In `conversions.rs:building_to_boundary_inputs`, for Window boundaries:
- Find associated `Window` by matching boundary ID
- Compute `(r_glass, r_int) = window_u_factor_decomposition(u_factor)`
- Set `fallback_r_m2_k_w = r_glass`
- Override `r_film_interior_m2_k_w = r_int`
- Set `r_film_exterior_m2_k_w = 0.0`

**Step 3**: In `solver_builder.rs:317-323`, the `r_glass` calculation already reads from `bd_input.r_film_interior_m2_k_w` and `bd_input.r_film_exterior_m2_k_w`. Since Step 2 sets these correctly for windows, `solver_builder.rs` needs NO changes — it will automatically use the correct EnergyPlus film R values.

### Validation
- Unit test for `window_u_factor_decomposition` against OCHRE: for U=2.1 W/m²K, verify r_int and r_glass match OCHRE's values
- Oracle test: compare window transmittance/radiation_frac against OCHRE

**Files modified:**
- `crates/hares-physics/src/solar.rs` — add `window_u_factor_decomposition`
- `crates/hares-core/src/dwelling/conversions.rs` — apply window-specific film R and glass R

---

## Phase 3: Raised Floor Boundary Name

### Problem
`resolve_boundary_name` in `envelope_lut.rs` maps `BoundaryType::Floor` with `(Conditioned, Outdoor)` to the catch-all `_ => Some("Attic Floor")` instead of `"Raised Floor"`.

### Fix

In `resolve_boundary_name`, add arm before the catch-all:
```rust
BoundaryType::Floor => match (int, ext) {
    (ZoneType::Conditioned, ZoneType::Attic) => Some("Attic Floor"),
    (ZoneType::Conditioned, ZoneType::Foundation) => Some("Foundation Ceiling"),
    (ZoneType::Conditioned, ZoneType::Garage) => Some("Garage Interior Ceiling"),
    (ZoneType::Garage, ZoneType::Attic) => Some("Garage Ceiling"),
    (ZoneType::Conditioned, ZoneType::Outdoor) => Some("Raised Floor"),  // NEW
    _ => Some("Attic Floor"),
},
```

### Validation
- Unit test: `resolve_boundary_name(Floor, Conditioned, Outdoor) == "Raised Floor"`

**Files modified:**
- `crates/hares-io/src/envelope_lut.rs` — add Raised Floor arm

---

## Phase 4: Foundation/Slab Insulation Details

### Problem
OCHRE reads slab perimeter/underslab insulation and foundation wall depth-below-grade to generate insulation detail strings for LUT matching. HARES doesn't parse these HPXML elements, so the LUT may match wrong R-value variants.

### Fix

#### 4a: Foundation Wall Insulation (building.rs)

Add helper `extract_foundation_wall_insulation` that mirrors OCHRE's `get_fnd_wall_insulation` (envelope.py:434-459):

```rust
/// Returns (insulation_details, area_scale_factor).
/// area_scale_factor = depth_below_grade / height when they differ.
/// Mirrors OCHRE's get_fnd_wall_insulation (envelope.py:434-459).
fn extract_foundation_wall_insulation(node: &XmlNode, base_area: f64) -> (Option<String>, f64) {
    let height = parse_f64(node, "Height").unwrap_or(1.0);
    let depth_below_grade = parse_f64(node, "DepthBelowGrade").unwrap_or(height);
    let area_scale = if (depth_below_grade - height).abs() > 0.01 {
        depth_below_grade / height
    } else {
        1.0
    };

    // Sum nominal R-values from insulation layers
    let r_value = sum_insulation_layer_r(node);
    let insulation_details = if r_value > 0.0 {
        // Parse DistanceToTopOfInsulation and DistanceToBottomOfInsulation
        // from each Layer element to compute actual insulation height.
        // OCHRE: insulation_height = min(DistToBottom - DistToTop) across layers
        let insulation_height = compute_insulation_height_from_layers(node, height);
        if insulation_height > 0.0 && insulation_height <= height / 2.0 {
            format!("Half R{}", r_value.round() as i32)  // OCHRE format: "Half R10"
        } else {
            format!("R{}", r_value.round() as i32)        // OCHRE format: "R10"
        }
    } else {
        "Uninsulated".to_string()
    };

    (Some(insulation_details), area_scale)
}

/// Parse insulation height from Layer/DistanceToTopOfInsulation and
/// Layer/DistanceToBottomOfInsulation. Returns min(bottom - top) across layers,
/// defaulting to full height if elements are absent.
fn compute_insulation_height_from_layers(node: &XmlNode, wall_height: f64) -> f64 {
    // Collect all Layer elements under Insulation
    // For each: dist_bottom - dist_top (default: wall_height - 0)
    // Return minimum across layers
    // See OCHRE envelope.py:444-448
    todo!("implement")
}
```

Apply in `parse_boundary` for `BoundaryType::FoundationWall`:
- Set `insulation_details` from this function
- Multiply `area_m2` by the area scale factor

#### 4b: Slab Insulation (building.rs)

Add helper `extract_slab_insulation` that mirrors OCHRE's `get_slab_insulation` (envelope.py:462-485):

```rust
fn extract_slab_insulation(node: &XmlNode) -> Option<String> {
    let r_perimeter = parse_child_f64(node, "PerimeterInsulation/Layer/NominalRValue");
    let r_under = parse_child_f64(node, "UnderSlabInsulation/Layer/NominalRValue");

    match (r_perimeter, r_under) {
        (Some(rp), None) | (Some(rp), Some(0.0)) if rp > 0.0 => {
            let depth = parse_child_f64(node, "PerimeterInsulation/Layer/InsulationDepth")
                .unwrap_or(0.0);
            Some(format!("{depth:.0}ft R{rp:.0} Perimeter"))
        }
        (None, Some(ru)) | (Some(0.0), Some(ru)) if ru > 0.0 => {
            let full = parse_child_bool(node, "UnderSlabInsulation/Layer/InsulationSpansEntireSlab");
            if full {
                Some(format!("R{ru:.0} Whole Slab"))
            } else {
                let width = parse_child_f64(node, "UnderSlabInsulation/Layer/InsulationWidth")
                    .unwrap_or(0.0);
                Some(format!("{width:.0}ft R{ru:.0} Exterior"))
            }
        }
        (Some(rp), Some(ru)) if rp >= 100.0 && ru >= 100.0 => Some("Minimal".to_string()),
        _ => Some("Uninsulated".to_string()),
    }
}
```

#### 4c: Restrict insulation_details to foundation/slab only

Replace current `extract_insulation_details` usage with boundary-type-specific dispatch:

```rust
fn extract_insulation_details(node: &XmlNode, boundary_type: &BoundaryType) -> Option<String> {
    match boundary_type {
        BoundaryType::FoundationWall => extract_foundation_wall_insulation(node, 0.0).0,
        BoundaryType::Slab => extract_slab_insulation(node),
        _ => None,  // OCHRE does NOT pass insulation details for regular walls/roofs
    }
}
```

### Validation
- Unit tests for insulation format strings against OCHRE conventions
- `beopt_boundary_ua_diagnostics`: foundation/slab R-values should match OCHRE

**Files modified:**
- `crates/hares-io/src/hpxml/building.rs` — add foundation/slab insulation parsers, restrict insulation_details scope

---

## Phase 5: Validate and Iterate

After all fixes:

1. Convert `beopt_boundary_ua_diagnostics` to asserting test: total UA within 10% of OCHRE's 568 W/K
2. Run `envelope_oracle` test — empty building free-float should track OCHRE within reasonable tolerance
3. If divergences remain, the `EnvelopeDiagnostics` table pinpoints exactly which boundaries are responsible

Expected remaining differences (acceptable, documented):
- Film resistances: HARES uses TARP/DOE-2 per-boundary; OCHRE also uses TARP/DOE-2 but with slightly different temperature assumptions — small difference expected
- Per-orientation walls vs consolidated — same steady-state UA, slightly different transient
- Air density 1.2 vs 1.2041 — <0.5% difference
- EnergyPlus interpolation band for window transmittance (HARES uses 3.4-4.5 range; OCHRE uses 3.95 threshold) — minor difference for windows near that U-factor range

---

## Pre-implementation Checks

Before starting implementation, verify:
- Window `Boundary.area_m2` matches `Window.area_m2` for each window (GLM-5 flagged potential inconsistency)
- Slab insulation format strings in OCHRE's CSV LUT match the format strings we generate (e.g., "2ft R10 Perimeter" vs "2ft R-10 Perimeter")

## Verification

1. `cargo test --test structural_envelope_oracle` — all structural assertions pass
2. `cargo test --test envelope_oracle` — dynamic simulation tracks OCHRE
3. `EnvelopeDiagnostics` table printed in test output for manual review
4. Per-boundary UA within 5% of OCHRE for LUT-matched boundaries
5. Total dwelling UA within 10% of OCHRE's 568 W/K
6. Window transmittance/radiation_frac match OCHRE (verify `calculate_window_parameters` gets correct r_glass input)

## File Summary

| File | Changes |
|------|---------|
| `crates/hares-envelope/src/boundary_rc.rs` | Add `EnvelopeDiagnostics`, capture during construction |
| `crates/hares-io/src/hpxml/building.rs` | Fix ground zone label, add foundation/slab insulation parsing, restrict insulation_details scope |
| `crates/hares-io/src/envelope_lut.rs` | Add Raised Floor boundary name |
| `crates/hares-core/src/dwelling/conversions.rs` | Window U-factor decomposition wiring |
| `crates/hares-physics/src/solar.rs` | Add `window_u_factor_decomposition` alongside existing window functions |
| `tests/structural_envelope_oracle.rs` | Diagnostic + UA comparison tests, fix known-bug comments |
| `tests/fixtures/parity/extract_ochre_rc.py` | OCHRE oracle extraction script |
