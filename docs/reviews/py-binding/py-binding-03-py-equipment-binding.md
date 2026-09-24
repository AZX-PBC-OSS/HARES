# Python equipment binding: descriptor, mutation, LUT injection

**Review ID**: py-binding-03
**Category**: py-binding
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-python/src/py_equipment.rs` (primary)
- `crates/hares-equipment/src/ndinterp.rs` (RegularGridInterpolator validation & interpolation)
- `crates/hares-equipment/src/battery/ocv.rs` (OcvTable, UNegTable validation)
- `crates/hares-equipment/src/battery/mod.rs` (Battery clamp_power, set_* methods)
- `crates/hares-python/src/py_dwelling.rs` (add_battery, add_ev — registration flow)
- `crates/hares-equipment/src/registry.rs` (EquipmentRegistry duplicate checking)
- `crates/hares-core/src/dwelling/mod.rs` (add_equipment, duplicate-name check)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/api/datatransfer.cc`
- `vendors/EnergyPlus/src/EnergyPlus/api/runtime.cc`

## Findings

### Finding 1: [Severity: critical] `RegularGridInterpolator::interpolate()` panics on dimension mismatch instead of returning error

**Description**: The `interpolate` method at `crates/hares-equipment/src/ndinterp.rs:124` uses `assert_eq!(point.len(), self.axes.len(), ...)` to validate coordinate count. Both `Battery::clamp_power` (battery/mod.rs:612) and `EV::update_control` (ev/mod.rs:428) call `interpolate` with exactly 4 coordinates: `[soc, cell_temp_c, c_rate, soh]`. A Python user could construct a `RegularGridInterpolator` with any dimensionality from 1 to 8 (the constructor validates up to 8D), and `extract_charging_lut` imposes no dimensionality constraint. If a user supplies a 3D or 5D LUT (e.g., from a different battery model with fewer/more axes), the simulation panics at runtime with an unhandled Rust assertion rather than returning a Python exception.

**Code Location**: `crates/hares-equipment/src/ndinterp.rs:123-130` (assert), `py_equipment.rs:48` (no dimension check in `extract_charging_lut`)

**Root Cause**: `extract_charging_lut` and the `RegularGridInterpolator` constructor validate structural correctness (non-empty, strictly ascending, finite, cardinality match) but do not validate semantic dimensionality against equipment model expectations. No guard in the Python binding layer rejects an N-dimensional LUT when Battery/EV expect exactly 4D.

**Impact**: Program crash from Python. Not a clean `PyValueError` — the Rust `assert!` unwinds through PyO3, which may produce a cryptic panic message or abort depending on panic strategy. This is the only assertion-based panic path in the code review scope.

**Vendor comparison**: EnergyPlus uses range checks and returns error flags (`apiErrorFlag = true`) with zero/default return values (see `datatransfer.cc:477-493`, `setActuatorValue`). No `assert!`-style aborts on user input.


### Finding 2: [Severity: high] Duplicate equipment names silently coexist when using runtime `add_equipment()`

**Description**: `Dwelling::add_equipment()` at `crates/hares-core/src/dwelling/mod.rs:1710-1713` pushes equipment to a `Vec` and calls `refresh_equipment_caches()`, which rebuilds `equipment_id_by_name` by collecting from all equipment. If two equipment items share the same name, the later entry silently overwrites the earlier in the HashMap. There is no duplicate-name check in this code path. By contrast, during dwelling construction (`dwelling/mod.rs:1168-1179`), duplicate names are explicitly rejected with `"duplicate equipment name '{}' is not allowed"`. Since `add_battery`, `add_pv`, and `add_ev` (py_dwelling.rs:792, 819, 851) all call `add_equipment` without an intervening name check, calling `dwelling.add_battery(b1)` then `dwelling.add_battery(b2)` with the same name succeeds silently.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:1710-1713` (no check); contrast with lines 1168-1179 (construction-time check)

**Root Cause**: The duplicate-name guard exists only in the static config-path `Dwelling::init`, not in the runtime `add_equipment` path. `refresh_equipment_caches` uses `HashMap::insert` which silently overwrites.

**Impact**: Ambiguous control and telemetry — Python code iterating equipment will see both instances, but name-lookup will return only the most recently added. Hard-to-diagnose silent misbehavior.

**Vendor comparison**: EnergyPlus `getActuatorHandle` (datatransfer.cc:414-473) detects duplicate handle access via `handleCount > 0` and issues `ShowWarningError`, but still returns the handle. This is a more graceful degradation than silent overwrite.


### Finding 3: [Severity: high] Post-init mutation of LUT tables via trait setters bypasses configuration lifecycle

**Description**: `add_battery()` at `py_dwelling.rs:800-813` calls `eq.init()` first, then mutates the equipment via trait-level setters (`set_charging_curve_lut`, `set_ocv_table`, `set_u_neg_table`) before calling `dwelling.add_equipment(eq)`. The comment at line 803 explains: *"Set LUTs after init -- init may reset internal state"*. However, these setters are exposed on the `Equipment` trait (`lib.rs:179-198`), meaning any `dyn Equipment` can receive these calls at any time, not just during registration. There is no state guard (e.g., a "frozen" flag or an `initialized` boolean) that prevents post-registration mutation. After `add_equipment`, the equipment lives in the dwelling's `Vec<Box<dyn Equipment>>`, and the setters are still callable on a downcast or direct reference — a Python callback handler during simulation could inadvertently mutate the LUT.

**Code Location**: `crates/hares-python/src/py_dwelling.rs:800-813`, `crates/hares-equipment/src/lib.rs:179-198` (trait setters), `crates/hares-equipment/src/battery/mod.rs:1251-1269` (Battery implementations)

**Root Cause**: Design necessity — `init()` / `init_typed()` resets certain internal state (e.g., chemistry-default OCV/UNeg tables at battery/mod.rs:718, 722; resetting `charging_curve_lut` to `None`), so custom tables must be injected post-init. However, the mutation interface is permanent and un-guarded. The `PyBattery` struct itself is constructor-only (no `#[pyo3(set)]` attr), creating a misleading sense of immutability for the LUT tables.

**Impact**: If a user (or simulation plugin) holds a `&Battery` reference and calls `set_ocv_table` mid-simulation, voltages shift without warning and results are silently corrupted.


### Finding 4: [Severity: high] No semantic validation of LUT axis order or physical range

**Description**: `extract_charging_lut` at `py_equipment.rs:38-41` extracts axis arrays by hardcoded key names (`soc_grid`, `temp_grid`, `crate_grid`, `soh_grid`), but never validates that the axis arrays represent physically meaningful ranges for their respective coordinates. For example:
- SOC axis from -0.5 to 0.5 (while the model operates SOC from 0.0 to 1.0) passes validation.
- Temperature axis in Kelvin instead of Celsius passes validation.
- C-rate axis from -10 to 10 (including negative values) passes validation.
- Axes in wrong order (e.g., temp before SOC) pass validation.

Only structural properties are validated by `RegularGridInterpolator::new`: non-empty, strictly ascending, all finite, correct value cardinality. The `interpolate` method clamps coordinates to axis bounds, so out-of-range SOC queries silently map to the nearest axis endpoint — no warning, no error. Similarly, `OcvTable::new` and `UNegTable::new` validate structure but not physical domain (e.g., SOC < 0.0 or > 1.0 is not rejected).

**Code Location**: `py_equipment.rs:38-49`, `ndinterp.rs:40-107` (construction validation), `ndinterp.rs:143` (clamp), `battery/ocv.rs:131-164` (OcvTable::new), `battery/ocv.rs:248-280` (UNegTable::new)

**Root Cause**: The design separates structural validation from semantic validation — the former lives in `RegularGridInterpolator`, `OcvTable`, `UNegTable`; the latter is absent. No single point in the Python→Rust pipeline checks that the physical domain of the LUT matches the equipment model's expected operating range.

**Impact**: Physically impossible LUT data (SOC < 0, temperature 5000 K, negative C-rate axis) is accepted silently and produces physically meaningless but numerically finite interpolation results. No NaN is generated (interpolation always returns finite values from finite inputs), so the failure is silent.


### Finding 5: [Severity: medium] `interpolate()` clamps point coordinates — no warning for out-of-bounds queries

**Description**: `RegularGridInterpolator::interpolate()` at `ndinterp.rs:143` uses `point[dim].clamp(axis[0], axis[axis.len() - 1])` for each coordinate. This is documented as matching scipy's `bounds_error=False, fill_value=None` behavior (comment at line 4). However, the clamping is silent — if simulation conditions produce an SOC of 1.05 (due to rounding in degradation calculations), the value is silently mapped to the last axis breakpoint with no diagnostic. In EnergyPlus, out-of-range variable access sets `apiErrorFlag` and issues a severity error (`datatransfer.cc:381-386`).

**Code Location**: `crates/hares-equipment/src/ndinterp.rs:143`, `battery/ocv.rs:110-113` (voltage_at_soc clamp), `battery/ocv.rs:227-230` (potential_at_soc clamp)

**Root Cause**: Deliberate design choice matching scipy convention. The trade-off is robustness at the cost of silent data quality degradation.

**Impact**: Simulation results can be subtly wrong without any indication. Debugging incorrect LUT values or axes requires auditing the raw LUT data and simulation log manually.


### Finding 6: [Severity: medium] `EquipmentRegistry::register` duplicate-class check is debug-only

**Description**: `EquipmentRegistry::register()` at `registry.rs:129-137` uses `#[cfg(debug_assertions)]` to guard the duplicate-check assertion. In release builds, a duplicate class registration silently overwrites the previous factory. While not directly callable from Python (factories are registered internally at `EquipmentRegistry::new()`), it means that if a custom Rust build adds a duplicate registration for `"Battery"`, the error is invisible in release.

**Code Location**: `crates/hares-equipment/src/registry.rs:132`

**Root Cause**: Cost optimization — `HashMap::contains_key` + `assert!` is zero-cost in release (removed by compiler), but also removes the safety check.

**Impact**: In release builds, a conflicting registration silently overwrites, potentially replacing the Battery factory with a different one. Low probability since this code path is compile-time, not runtime-Python-facing.


### Finding 7: [Severity: low] `PyBattery` has no Python-visible setters for LUT/OCV/UNeg fields

**Description**: All Python-visible fields on `PyBattery` (lines 99-146) use `#[pyo3(get)]` only. The `charging_curve_lut`, `ocv_table`, and `u_neg_table` fields (lines 147-149) have no `#[pyo3]` attribute at all — they are Rust-only. This means:
1. Python users cannot read back the LUT they injected to verify it was parsed correctly.
2. Python users cannot mutate these fields after construction, even though the underlying Rust equipment model supports post-init mutation (Finding 3).
3. The inconsistency between visible scalar fields and invisible LUT fields is confusing.

**Code Location**: `py_equipment.rs:99-149` (field declarations)

**Root Cause**: `RegularGridInterpolator`, `OcvTable`, and `UNegTable` are not PyO3-wrapped as Python-exposable types. They are extracted from Python input objects during construction and stored as Rust-only fields.

**Impact**: Minor usability issue. Advanced users who need to inspect LUT contents must serialize/deserialize or write Rust-side inspection code. Not blocking for the primary use case (inject and simulate).


### Finding 8: [Severity: low] Zero-capacity battery passes LUT construction but produces Inf C-rate

**Description**: `Battery::clamp_power` at `battery/mod.rs:606-611` computes `c_rate = power_kw / pack_kwh` where `pack_kwh` is `self.capacity_kwh_nominal`. A Python user creating a battery with `capacity_kwh=0.0` will pass `PyBattery::new()` validation (no range check) and `RegularGridInterpolator::new()` (the LUT is valid), but at runtime `c_rate` becomes `+Inf`, which then feeds into `lut.interpolate()`. The interpolator's `clamp` (line 143) will clamp `+Inf` to the C-rate axis maximum, producing a silently wrong derating factor. No Python exception is raised.

**Code Location**: `crates/hares-equipment/src/battery/mod.rs:606-611`

**Root Cause**: No validation that `capacity_kwh` > 0 at the `PyBattery` constructor or `BatteryConfig` level.

**Impact**: Physically meaningless results for a nonsensical input. Low severity because a zero-capacity battery is an obvious input error.


## Summary
- **Total findings**: 8
- **Critical**: 1 (dimension-mismatch panic in interpolate)
- **High**: 3 (duplicate-name silent coexistence, post-init mutation un-guarded, no semantic LUT validation)
- **Medium**: 2 (silent clamp on out-of-bounds, debug-only duplicate check)
- **Low**: 2 (opaque LUT fields, zero-capacity division)

## Recommendations

1. **Replace interpolate `assert!` with error return** (Finding 1): Change `RegularGridInterpolator::interpolate()` to return `Result<f32, HaresError>` instead of panicking. Validate `point.len()` against `ndim()` and return a descriptive error. This is the single highest-impact fix.

2. **Add duplicate-name check to `add_equipment()`** (Finding 2): Add the same HashMap-based duplicate check from `dwelling/mod.rs:1168-1179` into `Dwelling::add_equipment()`, or pass through a `PyValueError` from `add_battery`/`add_ev`/`add_pv`.

3. **Add a post-init freeze flag on equipment** (Finding 3): Add an `initialized: bool` field to the `Equipment` trait or `EquipmentConfig`, and make `set_charging_curve_lut` / `set_ocv_table` / `set_u_neg_table` return `Err` if called after equipment enters the dwelling. Alternatively, inject LUTs *before* `init()` and make `init()` preserve custom tables instead of resetting them.

4. **Add semantic axis validation** (Finding 4): In `extract_charging_lut` for Battery and EV, validate that axis arrays use reasonable physical ranges (SOC ∈ [0, 1], temperature ∈ [-40, 60] C, C-rate non-negative, SOH ∈ [0, 1]). Emit `PyValueError` with a clear message if bounds are unreasonable.

5. **Add warning on out-of-bounds interpolation** (Finding 5): Log a warning (via `tracing::warn!`) when `interpolate`, `voltage_at_soc`, or `potential_at_soc` clamp a coordinate to axis bounds, including the coordinate value, axis name, and equipment name. Follow EnergyPlus's pattern of non-fatal diagnostic with optional `apiErrorFlag`.

6. **Make `EquipmentRegistry::register` duplicate check unconditional** (Finding 6): Remove the `#[cfg(debug_assertions)]` gate and return `Err` instead of `assert!` so the check works in release builds too.

7. **Expose LUT inspection fields on `PyBattery`** (Finding 7): Add `#[pyo3(get)]` getters for `charging_curve_lut_shape`, `ocv_table_points`, `uneg_table_points` that return light-weight Python representations (e.g., tuple of axis lengths, or a pandas-friendly dict of axes and values).

8. **Validate `capacity_kwh > 0` at construction** (Finding 8): Add a check in `PyBattery::new()` or `battery_config_from_py()` rejecting zero/negative capacity with a clear `PyValueError`.

## References / Citations
- EnergyPlus API handle validation pattern: `vendors/EnergyPlus/src/EnergyPlus/api/datatransfer.cc:414-522` — bounds checks with `apiErrorFlag` and severity messages
- `RegularGridInterpolator` documentation reference: scipy `RegularGridInterpolator` with `method="linear"`, `bounds_error=False`, `fill_value=None` — `ndinterp.rs:1-5`
- Battery LUT injection call site: `py_dwelling.rs:792-817` (add_battery); EV: `py_dwelling.rs:863-882` (add_ev)
- Battery LUT usage: `battery/mod.rs:596-613` (clamp_power); EV: `ev/mod.rs:428`
- Battery OCV/U_neg table usage: `battery/mod.rs:479` (compute_electrical); `battery/degradation.rs:286` (potential_at_soc)
- Duplicate name check (construction-time only): `dwelling/mod.rs:1168-1179`
- Missing check (runtime): `dwelling/mod.rs:1710-1713`
