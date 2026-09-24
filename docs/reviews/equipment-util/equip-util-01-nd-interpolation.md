# N-dimensional interpolation correctness and boundary handling
**Review ID**: equip-util-01
**Category**: equipment-util
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/ndinterp.rs` (432 lines) — primary review target
- `crates/hares-equipment/src/battery/mod.rs:595–629` — LUT usage in `Battery::clamp_power`
- `crates/hares-equipment/src/ev/mod.rs:415–431` — LUT usage in EV `charge_at_max_rate`

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/third_party/btwxt/src/regular-grid-interpolator-implementation.cpp` — BTWXT N-D interpolator core (547 lines)
- `vendors/EnergyPlus/third_party/btwxt/src/grid-axis.cpp` — BTWXT grid axis validation and cubic spacing
- `vendors/EnergyPlus/third_party/btwxt/include/btwxt/grid-axis.h` — BTWXT axis interface (methods, extrapolation enums)
- `vendors/EnergyPlus/third_party/btwxt/include/btwxt/regular-grid-interpolator.h` — BTWXT public API
- `vendors/EnergyPlus/src/EnergyPlus/CurveManager.hh` / `CurveManager.cc` — EnergyPlus polynomial curve clamping strategy
- `vendors/OCHRE/ochre/Equipment/Battery.py:133–145` — OCHRE 1D interp with constant extrapolation fill
- `vendors/OCHRE/ochre/Equipment/Generator.py:55–58` — OCHRE 1D interp with bounds_error defaults

## Findings

### Finding 1: [Severity: high] Missing index hunting / caching for temporal coherence
**Description**: The `interpolate` method performs a full binary search (`partition_point`) on every axis for every call. In time-series simulation, consecutive query points (e.g., SOC evolving from 0.51 to 0.50 across timesteps) land in the same or adjacent grid cells. Without caching, the interpolator repeats O(log n) searches when an O(1) linear scan from the last-known bracket position ("hunting") would suffice. The BTWXT reference implementation caches not just the bracket indices but the entire hypercube of grid-point data keyed by floor grid point index, avoiding redundant lookups when the target remains in the same cell. Similarly, `scipy.interpolate.RegularGridInterpolator` supports index caching via its internal `_find_indices` implementation.

**Code Location**: `ndinterp.rs:123–172` (`interpolate` method) and `ndinterp.rs:21–28` (struct lacking cache fields)

**Root Cause**: The `RegularGridInterpolator` struct holds axes, values, and strides but no mutable state for tracking the last-used bracket indices per axis. Each call to `interpolate` re-derives `lo_indices` from scratch via `bracket` at lines 142–147.

**Impact**: For large grids (thousands of breakpoints per axis) used in multi-year hourly simulations (8760+ timesteps per year), the repeated binary searches represent unnecessary overhead. The per-call impact is modest (binary search is fast), but multiplied by timesteps, equipment instances, and repeated model evaluations per timestep, it adds measurable cost. For the current 4-D use case (SOC × temp × C-rate × SOH) with typical grid sizes of 10–50 points per axis, the performance impact is low; however, the architecture is promoted as general-purpose N-D interpolation (up to 8 dimensions) where larger grids are anticipated.

---

### Finding 2: [Severity: medium] No runtime validation of query-point coordinates
**Description**: The constructor (`new`) rigorously validates all grid data: axes must be strictly ascending, non-empty, and contain only finite values; value length must match the grid size; all values must be finite. However, the `interpolate` method applies no corresponding validation to the query point. If a caller passes `NaN` or `±Inf` in any coordinate, the `f64::clamp` call (line 143) propagates `NaN`, which then flows into `bracket` (line 183). Inside `bracket`, `partition_point(|&v| v <= NaN)` always returns `0` (since every comparison with `NaN` is `false`), producing `lo = 0` and a `NaN` fraction, which then contaminates the multilinear weighted sum and produces `NaN` in the result. Non-finite output values are silently returned to the caller, potentially cascading through downstream physics calculations.

**Code Location**: `ndinterp.rs:142–147` and `ndinterp.rs:183–205`

**Root Cause**: Boundary checking is confined to construction-time validation. Query-time robustness relies entirely on callers providing finite coordinates.

**Impact**: Undetected `NaN` coordinates produce garbage outputs without any error or warning. Downstream code that multiplies by the interpolated power fraction (e.g., `battery/mod.rs:612`, `ev/mod.rs:428`) would produce `NaN` power limits, which could propagate through sum-aggregation in building-level models. The cost of adding a `.is_finite()` check per coordinate is negligible compared to the interpolator call itself.

**Comparison with reference**: OCHRE's `Battery.py` clips SOC to `[0, 1]` before interpolation (line 394); EnergyPlus's `CurveManager` clamps every input variable to its min/max before any curve evaluation. Neither reference validates for `NaN` explicitly, but their input paths are more tightly controlled (Python `float` coercion, EnergyPlus's earlier input-processing guards).

---

### Finding 3: [Severity: low] Grid-point values stored as `f32` while computation uses `f64`
**Description**: The `values` tensor is stored as `Vec<f32>` (32-bit float, ~7 decimal digits of precision) while axes, fractions, and the weighted sum all operate in `f64`. The result is cast from `f64` back to `f32` at line 171. For the current use case (power fractions in [0, 1]), the ~7-digit precision is adequate. However, if the interpolator were reused for table values spanning wide dynamic ranges (e.g., from 1e-6 W to 1e6 W), the `f32` storage would lose precision in small values relative to large ones, and the computed `f64` sum would not recover the lost precision.

**Code Location**: `ndinterp.rs:25` (struct field), `ndinterp.rs:168` (computation), `ndinterp.rs:171` (cast)

**Root Cause**: Design choice to halve memory and serialized size for the values tensor.

**Impact**: Low for current deployment (battery/EV charging curve LUTs with values in [0, 1]). Would become medium if reused for equipment capacity tables (kW ranges) or refrigerant property tables (MPa, kJ/kg ranges). The BTWXT library uses `double` (`f64`) throughout.

---

### Finding 4: [Severity: low] No test coverage for highly anisotropic grids
**Description**: The interpolator is claimed to support N-dimensional rectilinear grids with independently varying axis lengths. The test suite covers:
- 1D (2-point and 3-point axes) — `interp_1d_linear`, `interp_3_point_axis`
- 2D (square) — `interp_2d_bilinear`
- 4D (uniform sizes) — `interp_4d_constant`, `interp_4d_matches_scipy_convention`
- Single-point axes (degenerate 1D) — `single_point_axis`

However, there is no test where axis lengths differ substantially (e.g., one axis with 3 points and another with 100 points). Such anisotropic grids exercise the stride computation and the corner-indexing logic in ways that uniform grids do not, because the stride products involve vastly different magnitudes.

**Code Location**: `ndinterp.rs:207–432` (test module)

**Root Cause**: Test suite focuses on mathematical correctness (scipy parity) but omits structural edge cases.

**Impact**: Latent bug risk for future deployments with irregular axis sizes. The stride computation at lines 98–101 is straightforward arithmetic and unlikely to be wrong, but a confirming test would eliminate doubt.

---

### Finding 5: [Severity: low] Single `serde` derive without input sanitization
**Description**: The `RegularGridInterpolator` struct derives `Deserialize` (line 20), which means it can be constructed via deserialization bypassing the `new()` constructor and its validation. A deserialized instance could contain non-finite axis values, non-ascending axes, or mismatched values length, which would be caught only at the next `interpolate` call (by incorrect outputs, not by error). Defensive deserialization via a custom `Deserialize` implementation or a `#[serde(try_from = "...")]` pattern would route deserialized data through the validated constructor.

**Code Location**: `ndinterp.rs:20–21`

**Root Cause**: Convenience of `#[derive(Deserialize)]` without a validation wrapper.

**Impact**: Low, provided the serialized data sources are trusted (generated by the same codebase). Would become a concern if serialized interpolator files are user-provided or externally distributed.

**Comparison**: BTWXT construction validates sortedness and duplicate detection in `GridAxis::check_grid_sorted()` and `vector_is_valid()`, which are always called during normal construction but the library does not serialize axes separately.

---

## Summary

### Mathematical Correctness of Multilinear Interpolation
**Verified correct.** The implementation computes the weighted sum over all 2^N hypercube vertices using standard multilinear weights (tensor product of 1D linear interpolation basis functions: `1-μ` and `μ`). The commutative property holds: sequential 1-D linear interpolation along each axis in any order produces identical results. This was confirmed analytically and cross-validated against the BTWXT reference implementation's `calculate_interpolation_coefficients` (lines 492–530 of `regular-grid-interpolator-implementation.cpp`), which uses identical weight formulas for the linear case.

### Boundary Handling
**Safe.** Coordinates are clamped to `[axis[0], axis[last]]` before interpolation (line 143), implementing nearest-neighbor extrapolation. This prevents unbounded extrapolation that could yield unphysical values (negative COP, infinite power). The EnergyPlus polynomial curve subsystem uses the same strategy (hard clamp to min/max before evaluation). The BTWXT library additionally supports linear extrapolation with separate extrapolation limits; the clamping-only approach in HARES is a simpler, safer subset.

### Grid Search Efficiency
**Partial.** Binary search (`partition_point`, O(log n)) is used on each axis (line 190 of `bracket`). However, no caching of the last-used bracket index is maintained between successive calls (see Finding 1). For large grids with temporal coherence, this is a gap relative to both scipy and BTWXT.

### Degenerate Grid Handling
**Handled.** Single-point axes return the sole value (line 186 of `bracket`, early return). Two-point axes compute simple linear interpolation. The code correctly handles boundary cases (query on exact grid point, query at upper/lower limits, single-point axis with clamped out-of-range values).

### Mixed Unit Axes
**Agnostic by design.** The interpolator treats all values as raw `f64`/`f32` numbers with no unit metadata. Callers must ensure consistent units between grid axes and query coordinates. This is appropriate for a low-level interpolation utility; unit normalization belongs at a higher abstraction layer.

### Total Findings
- **Total**: 5
- **High**: 1 (missing index caching)
- **Medium**: 1 (no query-point validation)
- **Low**: 3 (f32 storage precision, anisotropic test gap, serde bypass)

## Recommendations
1. **Add index caching/hunting**: Store a `cached_bracket: Vec<usize>` (one per axis) in the `RegularGridInterpolator` struct. On each `interpolate` call, start from the cached index and search linearly forward/backward to find the new bracket, falling back to binary search if the target has moved far. This leverages temporal coherence in time-series simulation. BTWXT's hypercube caching (`hypercube_cache` keyed by `{floor_grid_point_index, hypercube_size_hash}`) is a more aggressive variant worth considering.

2. **Validate query points at call time**: Add a debug-assertion or early-return check in `interpolate` for `point.iter().all(|v| v.is_finite())`, returning `f32::NAN` or a `HaresError` to prevent silent garbage propagation. Alternatively, document that callers must pre-validate.

3. **Consider generic value type**: Parameterize `RegularGridInterpolator<T>` over the value type (or at minimum use `f64` for values) to avoid precision loss when table values span wide dynamic ranges.

4. **Add anisotropic grid test**: e.g., axes `[vec![0.0, 0.5, 1.0], vec![0.0, 0.1, 0.2, ..., 1.0]]` (3 × 11 points) with a known function to verify correct interpolation at off-grid midpoints.

5. **Harden deserialization**: Use `#[serde(try_from = "...")]` to route deserialized data through the validated `new()` constructor, preventing malformed interpolator objects from coming into existence.

## References / Citations
- BTWXT linear interpolation weights: `regular-grid-interpolator-implementation.cpp:511–515`
- BTWXT binary search + bounds classification: `regular-grid-interpolator-implementation.cpp:354–389`
- BTWXT hypercube caching: `regular-grid-interpolator-implementation.cpp:532–546`
- BTWXT compute_fraction: `regular-grid-interpolator-implementation.h:254–258`
- EnergyPlus polynomial curve clamping: `CurveManager.cc` (V1 = max(min(V1, max), min))
- OCHRE 1D interp with constant extrapolation fill: `Battery.py:133–145`
- scipy RegularGridInterpolator index caching: `scipy/interpolate/_rgi.py` `_evaluate_linear`
