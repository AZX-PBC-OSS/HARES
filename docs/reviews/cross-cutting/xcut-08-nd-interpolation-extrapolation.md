# ND interpolation extrapolation: out-of-bounds grid lookups, clamping vs. NaN strategy
**Review ID**: xcut-08
**Category**: cross-cutting
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/ndinterp.rs`
- `crates/hares-python/src/py_equipment.rs` (LUT construction callers)
- `crates/hares-equipment/src/battery/mod.rs` (runtime callers, line 612)
- `crates/hares-equipment/src/ev/mod.rs` (runtime callers, line 428)
- `crates/hares-equipment/src/pv/lut.rs` (PV LUT interpolation for comparison)

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: [Severity: critical]
**Description**: No extrapolation strategy enum exists — the `interpolate` method hardcodes per-dimension clamping and provides no mechanism for callers to choose between Clamp, NaN, LinearExtrapolate, or NearestNeighbor strategies.

**Code Location**:
- `crates/hares-equipment/src/ndinterp.rs:123-172` — the sole `interpolate` method always clamps every coordinate via `point[dim].clamp(axis[0], axis[axis.len() - 1])` at line 143 before interpolation.
- The `RegularGridInterpolator` struct (`ndinterp.rs:21-28`) carries no `ExtrapolationStrategy` field.
- Module doc comment at `ndinterp.rs:3-5` acknowledges the scipy port is specifically the "clamp" variant: `"bounds_error=False, fill_value=None" (clamp)`.
- Runtime callers (`battery/mod.rs:612`, `ev/mod.rs:428`) call `lut.interpolate(&[...])` with no strategy parameter.

**Root Cause**: The interpolator was ported from scipy's `RegularGridInterpolator` in its clamping-only mode. No extrapolation strategy enum was introduced to support the broader set of scipy options (`fill_value=nan`, `bounds_error=True`, or `fill_value` as an arbitrary float).

**Impact**: 
- **(b)** LUT generation (offline preprocessing) cannot produce NaN or a sentinel to signal coverage gaps. When precomputing performance maps, out-of-bounds values are silently clamped, hiding data deficiencies that should be flagged before the map is deployed to the online simulator.
- **(c)** All callers — both online step simulation and offline LUT generation — are forced to use clamping, making it impossible to select a context-appropriate strategy.
- A heat pump COP map that only covers down to -15°C will silently return the -15°C COP value for a -30°C operating point, rather than raising an error or producing NaN that would alert the operator to missing data.

### Finding 2: [Severity: high]
**Description**: Linear extrapolation is entirely absent, not even as an opt-in strategy. While linear extrapolation is physically dangerous for efficiency curves (COP can be extrapolated to negative or unrealistically large values), the complete absence means there is no way to enable it for controlled experiments, verification, or cases where linear extrapolation is appropriate (e.g., OCV tables near 0% or 100% SOC).

**Code Location**: `crates/hares-equipment/src/ndinterp.rs:123-172` — the `interpolate` method body has no code path for linear extrapolation. The clamped value is used to bracket within the grid; beyond-bounds coordinates never participate in slope-based extrapolation.

**Root Cause**: The scipy port was intentionally restricted to clamping mode only, and no `ExtrapolationStrategy::Linear` variant was added.

**Impact**: Users cannot opt into linear extrapolation even when they have reason to believe the function is linear near the boundary (e.g., ohmic-region OCV curves). This limits the interpolator's applicability.

### Finding 3: [Severity: high]
**Description**: The PV LUT `axis_bracket` function uses linear scan O(N) instead of binary search O(log N) for grid cell location, inconsistent with `ndinterp::bracket` which correctly uses `partition_point` (binary search).

**Code Location**:
- `crates/hares-equipment/src/pv/lut.rs:503-514` — a `for idx in 0..last` loop iterates through all axis points to find the containing interval:
  ```rust
  for idx in 0..last {
      let lo = axis[idx];
      let hi = axis[idx + 1];
      if value >= lo && value <= hi {
          ...
      }
  }
  ```
- Compare with `crates/hares-equipment/src/ndinterp.rs:190` which uses:
  ```rust
  let pos = axis.partition_point(|&v| v <= x);
  ```

**Root Cause**: The `axis_bracket` function in `pv/lut.rs` is independent of the `ndinterp::bracket` function and was written with a simple linear scan.

**Impact**: For axes with many points (e.g., irradiance bins could have 50–200 points), this adds 50–200 comparisons per axis per query instead of ~6–8 for binary search. Since the PV LUT has 6 axes, worst-case cost is ~1000 comparisons per call vs ~48. This function is called every PV timestep.

### Finding 4: [Severity: medium]
**Description**: No test coverage for mixed boundary cases where some dimensions are within bounds and others are out of bounds.

**Code Location**:
- `crates/hares-equipment/src/ndinterp.rs:376-387` — `interp_2d_clamp_both_axes` tests only the case where both axes are out of bounds (query at (5.0, 5.0) and (-5.0, -5.0) on a [0,1]×[0,1] grid).
- `ndinterp.rs:219-226` — `interp_1d_clamp` tests only single-axis out-of-bounds.

**Root Cause**: Test coverage was written for the simple cases but not the mixed case.

**Impact**: While the implementation at `ndinterp.rs:142-147` *does* appear to handle the mixed case correctly (each dimension is clamped independently before interpolation), the absence of a test means a future refactor could break this without detection. The per-dimension clamping ensures that a query like `[0.5, 5.0]` on [0,1]×[0,1] clamps only the second coordinate (to 1.0) while interpolating the first normally at 0.5.

### Finding 5: [Severity: medium]
**Description**: The values tensor is stored as `f32` but axes and intermediate interpolation arithmetic use `f64`. The dual-precision design is intentional and documented, but the final `f32` cast discards precision and creates subtle behavioral differences from scipy (which operates entirely in float64).

**Code Location**:
- `crates/hares-equipment/src/ndinterp.rs:24` — `values: Vec<f32>`.
- `ndinterp.rs:26` — `values: Vec<f32>` (Python bindings use `extract_flat_f32` at `py_equipment.rs:61`).
- `ndinterp.rs:151` — `result = 0.0f64`; all weighted-sum arithmetic is f64.
- `ndinterp.rs:171` — `result as f32` cast truncates to single precision.

**Root Cause**: The design saves 50% memory on the values tensor at the cost of single-precision storage. This is a reasonable trade-off for simulation-scale grid data.

**Impact**: When comparing outputs with scipy reference results, differences of order 1e-7 (f32 epsilon) are expected and observed (tests tolerate 1e-5). This is acceptable for power/COP lookups but could matter for energy-accumulation calculations that integrate over many timesteps.

### Finding 6: [Severity: medium]
**Description**: The `bracket` function at `ndinterp.rs:192-193` and `ndinterp.rs:159-160` contains defensive fallbacks for cases that should be unreachable after clamping, suggesting the code guards against scenarios it can't fully reason about.

**Code Location**:
- `ndinterp.rs:191-193`:
  ```rust
  let lo = if pos == 0 {
      0
  } else if pos >= n {
      n - 2
  } else {
      pos - 1
  };
  ```
  After clamping (`ndinterp.rs:143`), `x` is guaranteed to be in `[axis[0], axis[-1]]`. The `partition_point` predicate `v <= x` should therefore always find at least one matching element (`axis[0]`), so `pos >= 1`. The `pos == 0` branch is dead code in normal operation and only activates if floating-point disagreement between `clamp` and `partition_point` causes `x < axis[0]` after clamping.

- `ndinterp.rs:159-160`:
  ```rust
  let idx = lo_indices[dim] + bit as usize;
  let idx = idx.min(self.axes[dim].len() - 1);
  ```
  This `.min()` clamp is necessary for single-element axes (where `bit=1` would push `idx` out of bounds), but is redundant for all multi-element axes where `bracket` returns valid `lo_indices`.

**Root Cause**: Defensive programming in the presence of floating-point uncertainty and the single-element axis edge case.

**Impact**: The fallbacks are correct and safe; no bug exists. However, the `pos == 0` branch being theoretically reachable only via fp-rounding inconsistency indicates that the clamp-and-bracket pattern has a design tension that could be resolved by making `bracket` accept pre-clamped values as a documented precondition, eliminating the dead branches.

### Finding 7: [Severity: low]
**Description**: The module-level doc comment does not explicitly document the absence of extrapolation strategy selection as a known limitation, despite the scipy API supporting `fill_value=nan` and `bounds_error=True`.

**Code Location**: `crates/hares-equipment/src/ndinterp.rs:1-7`.

**Root Cause**: The comment accurately describes what *is* implemented but does not state what is *not* implemented.

**Impact**: A developer unfamiliar with scipy's full `RegularGridInterpolator` API might assume NaN fill or error-on-bounds behavior is available and waste time trying to configure it.

## Summary
- Total findings: 7
- Critical: 1 (no extrapolation strategy enum; hardcoded clamp)
- High: 2 (no linear extrapolation option; PV LUT linear scan vs binary search)
- Medium: 3 (missing mixed-boundary test; f32/f64 dual precision; dead-code branches in bracket)
- Low: 1 (incomplete module doc)

## Recommendations
1. **Introduce an `ExtrapolationStrategy` enum** with variants `Clamp`, `NaN`, `Linear`, and `NearestNeighbor`. Add it as a construction-time parameter stored in `RegularGridInterpolator`. The `interpolate` method should branch on the strategy per dimension for out-of-bounds coordinates.
2. **For LUT generation** (offline preprocessing), construct the interpolator with `ExtrapolationStrategy::NaN` so that coverage gaps are detected at preprocessing time rather than silently clamped at simulation time.
3. **For online step simulation**, retain `ExtrapolationStrategy::Clamp` as the safe default for heat pump COP, battery charging curves, and similar performance maps.
4. **Refactor `pv/lut.rs:axis_bracket`** to use `partition_point` (binary search) instead of a linear scan, consistent with `ndinterp::bracket`.
5. **Add a mixed-boundary test**: e.g., a 2D grid `[0,1]×[0,1]` with query `[0.5, 5.0]` should clamp the second dim to 1.0 and interpolate the first at 0.5 normally.
6. **Document the design limitation** in the module doc comment: explicitly state which extrapolation strategies are supported and which are not, with rationale (e.g., "Linear extrapolation is not available because it can produce physically nonsensical values for efficiency curves").
7. **Consider standardizing on a single bracket implementation** across `ndinterp` and `pv/lut` to avoid the O(N)/O(log N) inconsistency.

## References / Citations
- Scipy `RegularGridInterpolator` documentation: `method="linear"`, `bounds_error=False`, `fill_value=None` (matching the current port).
- Rust std `slice::partition_point` (binary search): used at `ndinterp.rs:190`.
- Runtime call sites: `battery/mod.rs:612`, `ev/mod.rs:428`.
