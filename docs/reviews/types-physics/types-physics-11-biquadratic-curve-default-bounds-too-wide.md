# Biquadratic curve default bounds too wide (documented bug)
**Review ID**: types-physics-11
**Category**: types-physics
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/biquadratic.rs` (core curve type, evaluate, tests)
- `crates/hares-equipment/src/hvac/hvac_core.rs` (HVAC defaults, evaluate wrappers, config loading)
- `crates/hares-equipment/src/hvac/ideal_hvac.rs` (ideal HVAC curve bounds)
- `crates/hares-equipment/src/hvac/dehumidifier.rs` (dehumidifier curve init)
- `crates/hares-equipment/src/hvac/dehumidifier_defaults.rs` (dehumidifier default curves)
- `crates/hares-equipment/src/hvac/core_config.rs` (bounds loading helpers)
- `crates/hares-equipment/src/water_heater/heat_pump_wh.rs` (HPWH curve init and evaluation)
- `crates/hares-equipment/src/water_heater/hpwh_compressor.rs` (HPWH default bounds/curves)
- `crates/hares-io/src/hpxml/resolve_hvac.rs` (HPXML bounds extraction)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/CurveManager.cc` — lines 236–294 (biquadratic evaluation with clamping, output limiting)
- `vendors/EnergyPlus/src/EnergyPlus/CurveManager.hh` — lines 134–159 (Limits struct, inputLimits array)

## Findings

### Finding 1: [Severity: high] `ff_bounds` default upper bound is `f64::INFINITY`
**Description**: The flow-fraction clamping bounds used in `evaluate_biquadratic_with_flow()` default to `(0.0, f64::INFINITY)`. The upper bound of infinity means flow fractions above the equipment's design operating range pass through unclamped, allowing unbounded extrapolation of the flow-fraction quadratic correction.

**Code Location**:
- `crates/hares-equipment/src/hvac/hvac_core.rs:377` — `ff_bounds: (0.0, f64::INFINITY)` in `HvacEquipment::new()`
- `crates/hares-equipment/src/hvac/hvac_core.rs:770` — `let ff_clamped = flow_fraction.clamp(self.config.ff_bounds.0, self.config.ff_bounds.1);` — since `ff_bounds.1` is `INFINITY`, an input of e.g. 10.0 is not clamped

**Root Cause**: The default was chosen to avoid constraining flow fractions that HPXML-sourced configs might not define. However, the OCHRE reference and the test code at `biquadratic.rs:309` use `(0.75, 1.25)` as standard bounds. Equipment performance curves (capacity, EIR) are only validated by manufacturers within that narrow range.

**Impact**: During simulation transients (startup cycling) or with malformed HPXML, flow fractions can reach extreme values. A flow fraction of 10.0 with an EIR flow-fraction quadratic of `[1.0, 0.5, 0.0]` would produce an ff_ratio of 6.0, inflating EIR 6× and distorting power consumption to non-physical levels.

**Recommendation**: Set the default upper bound to `1.25` (matching OCHRE convention) or at least `2.0` as a generous safety limit. EnergyPlus CurveManager.cc does not use flow-fraction correction directly; however, the EnergyPlus DX coils do apply part-load correction curves that are clamped to their specified input bounds — EnergyPlus never allows unbounded inputs to performance curves.

**Comparison vs EnergyPlus**: EnergyPlus uses per-curve `inputLimits[].min/max` specified in the IDF for every curve. There is no universal "default" fallback — if the user omits bounds, the limits struct defaults to `(0.0, 0.0)`, which effectively constrains all inputs to zero (a severe but safe failure). EnergyPlus never uses infinity as a bound.

---

### Finding 2: [Severity: medium] No NaN/Inf guard in raw biquadratic polynomial evaluation
**Description**: The raw `biquadratic()` and `quadratic()` functions perform no input validation. If the zone wet-bulb temperature or tank average temperature is NaN (possible during early-init edge cases), `f64::clamp()` propagates NaN silently.

**Code Location**:
- `crates/hares-physics/src/biquadratic.rs:9–16` — `biquadratic()` computes `a + b*x1 + c*x1^2 + d*x2 + e*x2^2 + f*x1*x2` with no input checks
- `crates/hares-physics/src/biquadratic.rs:39–40` — `let x1_clamped = x1.clamp(...)` — `f64::clamp` returns NaN when `x1` is NaN
- `crates/hares-equipment/src/water_heater/heat_pump_wh.rs:297–307` — `zone_wet_bulb_c()` can fall back to dry-bulb from `env.zones[].wet_bulb_c`, which is computed from relative humidity and could theoretically be NaN before zone initialization
- `crates/hares-equipment/src/water_heater/heat_pump_wh.rs:647` — `cop_curve.evaluate(wet_bulb_c, tank_avg_temp_c)` passes NaN if `wet_bulb_c` is NaN

**Root Cause**: The evaluation path trusts that upstream inputs are always finite numbers, which is true in normal operation but not guaranteed during initialization transients or when zones are being created.

**Impact**: Silent NaN propagation through COP/capacity computation → `compressor_power_w = (capacity_actual_w / cop)` produces NaN power → `delivered_hp_w = compressor_power_w * cop` = NaN heat injection → tank node temperatures become NaN, corrupting the entire simulation state.

**Comparison vs EnergyPlus**: EnergyPlus CurveManager.cc line 242–243 does not guard against NaN either — it uses the same `max(min(V, max), min)` pattern. However, EnergyPlus heavily validates input data at parse time (CurveManager.cc lines 773–791 validate min > max) and requires explicit bounds, making NaN-producing edge cases less likely.

**Recommendation**: Add `debug_assert!(x1.is_finite() && x2.is_finite())` or `tracing::warn!` in the `evaluate()` method when inputs are non-finite. For production-hardened guarding, clamp NaN inputs to the midpoint of the bound. This is a low-cost safety net since the check is branch-predictable (NaN is rare).

---

### Finding 3: [Severity: medium] Dehumidifier and HPWH initialization ignores config-specified curve bounds
**Description**: The HVAC path (`hvac_core.rs:560–571`) and ideal HVAC path (`ideal_hvac.rs:427–433`) read biquadratic curve bounds from config keys (`biquadratic_x1_min`, `biquadratic_x1_max`, `biquadratic_x2_min`, `biquadratic_x2_max`) via `load_bounds_pair()`. The dehumidifier and HPWH paths do NOT — they hardcode compile-time defaults and ignore any per-equipment bound overrides in the config or HPXML data.

**Code Location**:
- HVAC path (correct): `crates/hares-equipment/src/hvac/hvac_core.rs:560–571` — uses `load_bounds_pair()` with config keys
- Ideal HVAC path (correct): `crates/hares-equipment/src/hvac/ideal_hvac.rs:427–433` — uses `load_bounds_pair()` with config keys
- Dehumidifier path (missing): `crates/hares-equipment/src/hvac/dehumidifier.rs:260–271` — hardcodes `DEFAULT_DB_BOUNDS_C` and `DEFAULT_RH_BOUNDS`; never reads from config
- HPWH path (missing): `crates/hares-equipment/src/water_heater/heat_pump_wh.rs:411–416` — hardcodes `DEFAULT_ZONE_TEMP_BOUNDS_C` and `DEFAULT_TANK_TEMP_BOUNDS_C`; never reads from config
- HPXML bounds extraction: `crates/hares-io/src/hpxml/resolve_hvac.rs:616–627` — `extract_curve_bounds()` correctly reads `biquadratic_x1_min/max` and `biquadratic_x2_min/max` from HPXML params into `CurveBounds`

**Root Cause**: The `CurveBounds` struct and `extract_curve_bounds()` were designed for the central HVAC path only. The HPXML-to-dehumidifier and HPXML-to-HPWH code paths do not propagate these bounds into the equipment config, and the equipment init code does not look for them.

**Impact**: Manufacturer-specified curve bounds from HPXML (e.g., a heat pump water heater with COP curve valid only for tank temperatures 10°C–60°C from the manufacturer spec sheet) are silently ignored. The equipment uses the compile-time defaults instead, which may be wider than the manufacturer's valid range.

**Comparison vs EnergyPlus**: EnergyPlus requires per-curve bounds as mandatory input fields for every `Curve:Biquadratic` object. The bounds are always user-specified per curve, never derived from global defaults. The HPXML workflow is designed to carry these bounds through, but HARES drops them before they reach the equipment.

**Recommendation**: Add bound-loading to `dehumidifier.rs:init_from_typed()` and `heat_pump_wh.rs:init_typed()`. For the dehumidifier, add `db_bounds_min`/`db_bounds_max`/`rh_bounds_min`/`rh_bounds_max` fields to `DehumidifierConfig` or read them from the generic `EquipmentConfig`. For the HPWH, read `biquadratic_x1_min/max` and `biquadratic_x2_min/max` from `EquipmentConfig` using `load_bounds_pair()`.

---

### Finding 4: [Severity: low] Test code in biquadratic.rs still references stale `(-100, 100)` as "current" defaults
**Description**: Three regression tests in `biquadratic.rs` construct "current" curves with `(-100, 100)` bounds and assert `assert_ne!` to document the bug. The production defaults in `hvac_core.rs` have already been tightened to `(-10, 50)` for x1 and `(-50, 60)` for x2. The test comments misleadingly label `(-100, 100)` as "current" and the fixed bounds as "proposed."

**Code Location**:
- `crates/hares-physics/src/biquadratic.rs:145–187` — `default_x2_lower_bound_clamps_at_neg50_not_neg100` test
- `crates/hares-physics/src/biquadratic.rs:189–223` — `default_x2_upper_bound_clamps_at_pos60_not_pos100` test
- `crates/hares-physics/src/biquadratic.rs:225–259` — `default_x1_lower_bound_clamps_at_neg10_not_neg100` test

**Root Cause**: The fix was applied to production constants but the bug-documentation tests were not updated to reflect that the bug has been resolved. The test block header at line 134–140 explains they use locally-scoped bounds and do not reference production constants, but the individual test comments at lines 142, 144, 220 say "BUG present" and "current x2_bounds=(-100,100)" which is inaccurate.

**Impact**: Code readability and maintainability issue only. Developers may be confused about whether the fix has been applied or is still pending. No functional impact.

**Recommendation**: Update test comments to clarify that the `(-100, 100)` bounds represent the *prior* buggy behavior and that the production defaults have already been fixed. Optionally, reference the production-path regression tests at `hvac_core.rs:1469–1490` which prove the fix is in place.

---

## Summary
- **Total findings**: 4
- **Critical**: 0
- **High**: 1 (unbounded `ff_bounds` upper limit = infinity)
- **Medium**: 2 (no NaN guard in evaluation, dehumidifier/HPWH ignore config bounds)
- **Low**: 1 (stale test documentation)

## Recommendations
1. **Change `ff_bounds` default from `(0.0, f64::INFINITY)` to `(0.0, 2.0)`** in `hvac_core.rs:377`. This provides a generous safety net while preventing unbounded extrapolation. Ideally, match OCHRE convention `(0.75, 1.25)` for production-grade enforcement. Add a config key override path so HPXML-sourced `ff_min`/`ff_max` values can tighten bounds further.

2. **Add NaN/Inf detection in `BiquadraticCurve::evaluate()`** in `biquadratic.rs:38–55`. At minimum, add a `tracing::warn!` when inputs are non-finite. For robustness, clamp NaN to the bound midpoint to produce a safe fallback value rather than propagating NaN through the simulation.

3. **Add curve-bound loading to dehumidifier and HPWH init paths:**
   - In `dehumidifier.rs:init_from_typed`, add `db_bounds` and `rh_bounds` fields to `DehumidifierConfig` (or read from `EquipmentConfig` generic keys `biquadratic_x1_min/max` and `biquadratic_x2_min/max`)
   - In `heat_pump_wh.rs:init_typed`, add `load_bounds_pair()` calls around lines 411–434 to read `biquadratic_x1_min/max` and `biquadratic_x2_min/max` from the `EquipmentConfig`

4. **Update test comments** in `biquadratic.rs:134–259` to clarify that the `(-100, 100)` bounds represent the *resolved* prior behavior, not the current production defaults.

## References / Citations
- EnergyPlus Curve:value(V1,V2) — `CurveManager.cc:236–294`: input clamping via `max(min(V, max), min)`, plus optional output limits
- EnergyPlus `Limits` struct — `CurveManager.hh:134–140`: `min=0.0`, `max=0.0`, `minPresent=false`, `maxPresent=false`
- EnergyPlus biquadratic input loads — `CurveManager.cc:760–763`: bounds from `Numbers(7)` through `Numbers(10)` (mandatory user-specified fields)
- EnergyPlus bounds validation — `CurveManager.cc:773–791`: rejects `min > max` at parse time
- OCHRE flow-fraction bounds — `biquadratic.rs:309` test: `ff_bounds: (0.75, 1.25)`
- OCHRE reference test — `biquadratic.rs:307–309`: `twb_bounds: (13.88, 23.88)`, `tdb_bounds: (18.33, 51.66)`, `ff_bounds: (0.75, 1.25)`, `plf_bounds: (0.7, 1.0)`
- HARES production-path clamping tests — `hvac_core.rs:1469–1490`: confirm `DEFAULT_BIQUADRATIC_X2_BOUNDS = (-50, 60)` clamps correctly
