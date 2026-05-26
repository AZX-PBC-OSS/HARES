# Python PV sizing bindings: compute_usable_area, size_pv_system, enumerate_pv_candidates, infer_roof_shape, error propagation
**Review ID**: pygap-01
**Category**: python-gaps
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-python/src/py_pv_sizing.rs` (234 lines)
- `crates/hares-python/src/py_dwelling.rs` (lines 1249–1302, the Dwelling-side PV method exposures)
- `crates/hares-python/src/conversions.rs` (200 lines, numpy/array conversion utilities)
- `crates/hares-physics/src/pv_sizing.rs` (746 lines, the Rust implementation behind the bindings)
- `crates/hares-io/src/pv_sizing.rs` (47 lines, roof data extraction from HPXML)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/api/datatransfer.py` — reference for Python-to-native error propagation patterns (EnergyPlusException pattern, argument validation)
- `vendors/EnergyPlus/src/EnergyPlus/api/state.py` — reference for state management API surface
- `vendors/EnergyPlus/src/EnergyPlus/api/common.py` — reference for custom exception hierarchy (`EnergyPlusException`) and input validation (`is_number`)

## Findings

### Finding 1: [Severity: high]
**Description**: Panel wattage, panel area, and system losses are permanently hardcoded in every Python wrapper function. The four helper functions in `py_pv_sizing.rs` all pass `None` for `panel_watts`, `panel_area_m2`, and (where applicable) `system_losses`, locking Python users into defaults (420 W panel, 2.0 m² footprint, 14% losses). No inverter model parameter exists in the Rust API surface at all — the `size_pv_system` function has no `inverter_max_dc_watts`, `inverter_max_ac_watts`, or `max_dc_ac_ratio` parameter.
**Code Location**: `crates/hares-python/src/py_pv_sizing.rs:213`, `:229`, `:231`
**Root Cause**: The binding layer treats the caller-facing `Option<u32>` / `Option<f64>` parameters as implementation-internal defaults rather than as an extensibility point for Python. The wrapper functions (`pv_candidates_from_dwelling`, `size_pv_from_dwelling`) are `pub(crate)` helpers that hardcode `None` — they are not `#[pyfunction]`s with user-facing signatures.
**Impact**: Python developers cannot select panel models (fixed at 420 W), set system DC/AC ratios, model inverter clipping, or adjust loss factors without modifying Rust code. This eliminates the use case of "try different panel/inverter combinations" that `enumerate_pv_candidates` is intended to support.

### Finding 2: [Severity: high]
**Description**: `compute_usable_area` and `infer_roof_shape` are never exposed as standalone Python-callable functions. `compute_usable_area` is only reachable through `PyDwelling.estimate_pv_capacity()` (which chains `compute_usable_area` → `size_pv_system` internally). `infer_roof_shape` is always called internally and its result is consumed immediately — Python callers have no way to obtain or override the inferred roof shape. The review specification requires all four functions to be exposed as directly callable Python bindings.
**Code Location**: `crates/hares-python/src/py_pv_sizing.rs:195–234` (entire module provides only `pub(crate)` helpers, no `#[pyfunction]`s)
**Root Cause**: The `py_pv_sizing.rs` module was written as internal plumbing for `py_dwelling.rs` rather than as a public API surface. All four functions are helper methods wired to `PyDwelling` rather than standalone `#[pyfunction]` entries in `lib.rs`.
**Impact**: Python users cannot run `compute_usable_area` to inspect the intermediate usable-area calculation before sizing. They cannot call `infer_roof_shape` to diagnose classification decisions. This contradicts the documented API surface of four discrete functions.

### Finding 3: [Severity: medium]
**Description**: Error handling is inconsistent between `compute_usable_area` (returns `Result<UsableRoofArea, PvSizingError>`) and `enumerate_pv_candidates` (returns `Vec<PvCandidate>`, infallible). When all roof planes are north-facing, `compute_usable_area` raises `PyValueError("all roof planes are north-facing or missing azimuth data")` after being unwrapped, but `enumerate_pv_candidates` silently returns an empty list. The Python caller has no way to distinguish "valid empty result" from "all planes were filtered out due to north-facing azimuths."
**Code Location**: `crates/hares-physics/src/pv_sizing.rs:384–455` (infallible `enumerate_pv_candidates`), `:199–338` (fallible `compute_usable_area`); `crates/hares-python/src/py_dwelling.rs:1260` (no error wrapping for `pv_candidates`)
**Root Cause**: `enumerate_pv_candidates` was designed as an always-infallible enumeration that silently filters north-facing planes, while `compute_usable_area` defensively rejects the all-north-facing case. The Python binding layer does not add a post-hoc check on the empty-return case.
**Impact**: Silently empty `pv_candidates()` output could mislead automation scripts into believing a building has no viable PV placement when the real issue is missing azimuth data.

### Finding 4: [Severity: medium]
**Description**: No electrical code constraint (e.g., NEC 120% busbar backfeed rule, 690.12 rapid shutdown, 705.12 load-side connection limits) is implemented or parameterized anywhere in the sizing pipeline. The `size_pv_system` function merely clamps capacity to a user-provided `max_kw` and the roof's `max_capacity_kw`. The review spec requires that sizing "respects electrical code constraints (e.g., NEC 120% rule for backfeed on busbar)."
**Code Location**: `crates/hares-physics/src/pv_sizing.rs:341–378` (`size_pv_system`)
**Root Cause**: The function operates purely on the DC side (panel count × panel wattage) with no AC-side modeling and no awareness of main service panel ampacity, busbar rating, or backfeed breaker sizing.
**Impact**: A system sized at 14 kW AC could be recommended for a dwelling with a 100 A main panel (where the 120% rule would limit backfeed to ~20 A / 4.8 kW on a typical residential panel). The sizing result is electrically unsafe for downstream interconnection applications.

### Finding 5: [Severity: medium]
**Description**: `infer_roof_shape` does not consider climate zone (IECC climate zone or ASHRAE classification) for regional defaults. The review specification states "flat roofs common in Southwest, pitched roofs in Northeast." The function uses latitude only to signal hip roofs in low-latitude regions (`lat < 30.0`), but makes no climate-zone distinction for flat vs. pitched defaults. Additionally, the `RoofShape` enum is missing a `Shed` variant despite the specification requiring "gable, hip, flat, shed."
**Code Location**: `crates/hares-physics/src/pv_sizing.rs:476–521` (`infer_roof_shape`), `:10–15` (`RoofShape` enum)
**Root Cause**: The `RoofShape` enum was designed with three variants (Gable, Hip, Flat), omitting Shed. The inference function uses building-type and roof-plane heuristics but has no climate-zone input parameter. Only `facility_type`, `RoofInfo`, and `latitude` are accepted.
**Impact**: Roof shape can be misclassified for regions where the climate-zone default differs from the heuristic. Buildings in Phoenix AZ (IECC 2B, predominantly flat roofs) may be classified as Gable if they have pitched trusses. Shed-roof buildings (common on additions, modern designs) are forced into Gable.

### Finding 6: [Severity: low]
**Description**: Error messages in the binding layer lose context when propagated to Python. In `size_pv_from_dwelling`, errors from `compute_usable_area` and `size_pv_system` are mapped to `String` via `.to_string()`, then in `estimate_pv_capacity` wrapped as `PyValueError`. The original error discriminant (`NoRoofPlanes` vs `AllNorthFacing` vs `InsufficientRoof`) is discarded. A Python developer seeing `PyValueError("roof capacity 3.5 kW below minimum 4.0 kW")` has no indication of which step failed (usable area computation vs. sizing) or which building ID triggered the error.
**Code Location**: `crates/hares-python/src/py_pv_sizing.rs:227–233` (`size_pv_from_dwelling`), `crates/hares-python/src/py_dwelling.rs:1301`
**Root Cause**: Flat conversion from `PvSizingError` → `String` → `PyValueError` without retaining error type metadata.
**Impact**: Debugging PV sizing failures in a fleet simulation of 10,000+ dwellings requires reading Rust source to trace the call path. EnergyPlus by contrast defines a dedicated `EnergyPlusException` hierarchy.

### Finding 7: [Severity: low]
**Description**: The `PvCandidate` pyclass does not expose the `roof_shape` field present in the underlying Rust `PvCandidate` struct. Python callers iterating candidate results cannot determine which roof shape classification produced a given candidate.
**Code Location**: `crates/hares-python/src/py_pv_sizing.rs:60–128` (missing `#[getter] fn roof_shape`)
**Root Cause**: The `PyPvCandidate` wrapper was written before `roof_shape` was added to the Rust struct, or the field was deemed internal. The struct has the field at the Rust level (`pv_sizing.rs:66`).
**Impact**: Auditing of PV candidate quality (e.g., "why did this candidate get low usable area?") requires cross-referencing with the `infer_roof_shape` result, which is not directly accessible from the candidate.

### Finding 8: [Severity: low]
**Description**: No dedicated custom Python exception types exist for PV sizing errors. Failures surface as generic `PyValueError`. The HARES Python module defines `HaresConfigError`, `HaresEquipmentError`, and `HaresSimulationError`, but none specific to PV sizing geometry.
**Code Location**: `crates/hares-python/src/py_dwelling.rs:26–36` (custom exception definitions), line 1301 (using `PyValueError`)
**Root Cause**: The PV sizing binding was added after the initial exception hierarchy was defined, and no additional exception classes were created.
**Impact**: Python `except` clauses cannot catch PV sizing errors specifically; they must catch `ValueError` broadly, which may mask unrelated validation errors from other parts of the API. EnergyPlus defines `EnergyPlusException` for this purpose.

### Finding 9: [Severity: low]
**Description**: No NaN validation on floating-point inputs to the PV sizing pipeline. If the HPXML parser produces roof area, tilt, or azimuth values containing NaN (e.g., from missing data or parsing errors), those NaN values propagate silently through the sizing calculations, producing NaN-tainted outputs (NaN capacity, NaN panel count) without diagnostic errors.
**Code Location**: `crates/hares-physics/src/pv_sizing.rs:199–338` (`compute_usable_area`, no `is_nan` checks); `crates/hares-io/src/pv_sizing.rs:14–47` (no validation in extraction)
**Root Cause**: The Rust implementation uses raw `f64` arithmetic and assumes valid inputs from the HPXML parsing layer. There is no upstream validation guarantee.
**Impact**: A single NaN in building geometry data can silently poison PV sizing for an entire fleet simulation, with the failure only surfacing downstream (e.g., in simulation output with NaN energy values).

### Finding 10: [Severity: low]
**Description**: The `enumerate_pv_candidates` wrapper and `compute_usable_area` produce only a single candidate per roof plane with hardcoded panel specs (420 W default). The review specification expects enumeration to try "different panel/inverter combinations, system sizes, and orientations." The enumeration currently covers orientations (one per non-north plane) but no panel/inverter dimension.
**Code Location**: `crates/hares-python/src/py_pv_sizing.rs:207–217` (`pv_candidates_from_dwelling` passes `None, None`); `crates/hares-physics/src/pv_sizing.rs:384–455`
**Root Cause**: The `enumerate_pv_candidates` function accepts `Option<u32>` / `Option<f64>` for panel specs but the Python wrapper hardcodes `None`, eliminating combinatorial exploration. There is no cross-product across panel types.
**Impact**: Python callers who expect `pv_candidates()` to explore the full design space (different panel wattages, different inverter pairings) receive a single-dimensional search (roof-plane azimuth only).

## Summary
- Total findings: 10
- Critical: 0
- High: 2
- Medium: 3
- Low: 5

## Recommendations

1. **Expose all four functions as `#[pyfunction]` standalone bindings** in `lib.rs` with `#[pyo3(signature = (...))]` signatures that accept optional panel specifications, system losses, and inverter parameters. Do not restrict them to `PyDwelling` methods.

2. **Make panel selection user-configurable.** Plumb `panel_watts`, `panel_area_m2`, and `system_losses` through all Python wrappers as keyword arguments with the current constants as defaults.

3. **Add inverter modeling parameters** to `size_pv_system` (`inverter_kw_ac`, `max_dc_ac_ratio`) and implement a basic electrical constraint check (e.g., busbar 120% rule requires main panel ampacity input).

4. **Make `enumerate_pv_candidates` fallible** by changing its return type to `Result<Vec<PvCandidate>, PvSizingError>` or add an explicit empty-result check in the Python binding that raises when all planes are filtered out.

5. **Add the `Shed` variant to `RoofShape`** and extend `infer_roof_shape` to accept an optional climate zone parameter (IECC or ASHRAE integer) for region-aware defaults.

6. **Preserve error type information** across the Python boundary. Either define a `HaresPvSizingError` exception class, or embed error discriminants in the exception message text (e.g., prefix with `[PvSizing:NoRoofPlanes]`).

7. **Add `#[getter] fn roof_shape`** to `PyPvCandidate` so Python callers can introspect the roof shape used for a given candidate.

8. **Validate floating-point inputs** in `compute_usable_area` and `extract_roof_info` by checking for NaN and negative values, returning `PvSizingError` variants with descriptive messages.

## References / Citations

- EnergyPlus Python API error patterns: defines `EnergyPlusException` at `vendors/EnergyPlus/src/EnergyPlus/api/common.py:65` and validates numeric handles with `is_number()` on every exchange call (`datatransfer.py`). HARES should follow a similar custom-exception strategy.
- `enumerate_pv_candidates` infallible design: `crates/hares-physics/src/pv_sizing.rs:384` — the doc comment says "enumerate all viable candidates" but the function silently returns an empty `Vec` for north-facing-only buildings.
- Panel defaults: `crates/hares-physics/src/pv_sizing.rs:90–93` — `DEFAULT_PANEL_WATTS = 420`, `DEFAULT_PANEL_AREA_M2 = 2.0`, `DEFAULT_SYSTEM_LOSSES = 0.14`. Python wrappers hardcode `None` at `py_pv_sizing.rs:213`, `:229`, `:231`.
- Missing Shed variant: `crates/hares-physics/src/pv_sizing.rs:10–15` — `enum RoofShape { Gable, Hip, Flat }` lacks the `Shed` variant mentioned in the specification.
- Error-to-string conversion: `crates/hares-python/src/py_pv_sizing.rs:230` — `map_err(|e| e.to_string())` discards error type information.
