# Electrical power accumulation: generation vs load sign conventions
**Review ID**: wiring-03
**Category**: wiring
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-types/src/ports.rs` (full file, 1216 lines)
- `crates/hares-core/src/dwelling/mod.rs` (portions: lines 254–330, 534–598, 2679–2928, 3230–3259)
- `crates/hares-envelope/src/electrical_solver.rs` (full file, 389 lines)
- `crates/hares-types/src/equipment.rs` (lines 782–831 — ElectricPower enum)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Dwelling.py` (full file, 371 lines)
- `vendors/OCHRE/ochre/Equipment/Equipment.py` (lines 185–218 — ZIP model)
- `vendors/EnergyPlus/src/EnergyPlus/ElectricPowerServiceManager.cc` (lines 111–190, 479–530 — manageElectricPowerService, updateWholeBuildingRecords)

## Findings

### Finding 1: [Severity: high]
**Description**: The electrical balance invariant check compares ZIP-adjusted solver output against raw (unadjusted) port accumulation. These diverge whenever a non-default ZIP model is configured at non-nominal voltage.

**Code Location**:
- `crates/hares-core/src/dwelling/mod.rs:3257-3259` — call site
- `crates/hares-core/src/invariants.rs:61-80` — `check_electrical` function
- `crates/hares-envelope/src/electrical_solver.rs:120-122` — ZIP adjustment logic

**Root Cause**: The invariant check asserts `|p_grid + p_sum| < 0.001` where `p_grid = solver.net_active_kw()` (ZIP-adjusted: `p_load * scale + p_gen`) and `p_sum = -ports.net_active_kw()` = `-(p_load + p_gen)`. The residual is `p_load * (scale - 1)`. With the default ZIP config `(Z=0, I=0, P=1)`, `scale = 1.0` regardless of voltage and the check always passes. With non-default ZIP coefficients at non-unity voltage, the residual becomes:

```
residual = p_load * ((Z·V² + I·V + P) - 1) [kW]
```

For example, 10 kW load at 0.95 pu with `Z=0.2, I=0.2, P=0.6`:
- scale = 0.2·0.9025 + 0.2·0.95 + 0.6 = 0.9705
- residual = 10 · (0.9705 − 1.0) = −0.295 kW

This exceeds the 0.001 kW tolerance, producing a false-positive invariant violation.

The check is gated on `#[cfg(any(debug_assertions, feature = "check_invariants"))]` (`invariants.rs:66`), so release builds are unaffected.

**Impact**: Users enabling `check_invariants` with custom ZIP coefficients will see spurious invariant violations. The check's stated purpose — verifying that the solver's electrical output matches port accumulation — is only valid for the constant-power (P=1.0, Z=0, I=0) ZIP model. The test `electrical_balance_identity_holds` at `electrical_solver.rs:282-302` masks this by using the default config.

### Finding 2: [Severity: medium]
**Description**: Reactive power is passed through the electrical solver without ZIP voltage correction, unlike OCHRE which scales both active and reactive power by separate ZIP coefficients.

**Code Location**:
- `crates/hares-envelope/src/electrical_solver.rs:123` — `self.net_reactive_kvar = ports.electrical.reactive_power_kvar;`
- `vendors/OCHRE/ochre/Equipment/Equipment.py:217-218` — OCHRE's ZIP model scaling both active and reactive

**Root Cause**: The solver applies the ZIP formula `load_scale = Z·V² + I·V + P` only to `p_load` (active power on `load_power_kw`). Reactive power (`reactive_power_kvar`) is unconditionally forwarded without scaling. In power systems, inductive reactive power from motor-driven loads (HVAC compressors, fans, pumps) also varies with voltage:
- Constant-impedance loads: Q ∝ V²
- Constant-current loads: Q ∝ V
- Constant-power loads: Q = constant

OCHRE's `Equipment.py:200-218` includes separate ZIP coefficient triplets (`zip_p`, `zip_q`) for active and reactive power, scaling both independently:
```python
self.reactive_kvar = self.electric_kw * pf_mult * zip_p.dot(v_quadratic)
self.electric_kw     = self.electric_kw * zip_q.dot(v_quadratic)
```

HARES's current approach is correct only for unity power factor or nominal voltage (where V=1.0 pu makes scale=1.0 regardless of ZIP). At non-nominal voltage, reactive power measured at the meter will not reflect the actual voltage-dependent load behavior.

**Impact**: Meter-grade reactive power values are correct at nominal voltage but systematically underestimate voltage dependence. Acceptable for energy consumption and cost analysis; may produce biased results for feeder-level voltage/reactive-power studies. Compare against IEEE 1547-2018 which specifies reactive power capability curves that are voltage-dependent at the point of common coupling.

### Finding 3: [Severity: medium]
**Description**: The port-layer electrical accumulation has no defensive sign-convention validation. Equipment that misuses the sign convention (e.g., reporting generation as `active_power_kw > 0`) is silently reclassified as load.

**Code Location**:
- `crates/hares-types/src/ports.rs:539-543` — load/generation split in `accumulate`:
  ```rust
  if *active_power_kw >= 0.0 {
      self.electrical.load_power_kw += active_power_kw;
  } else {
      self.electrical.generation_power_kw += active_power_kw;
  }
  ```

**Root Cause**: The `PortContribution::Electrical` carries only raw sign-encoded `f64` values (`active_power_kw`, `reactive_power_kvar`). The sign convention (positive = load, negative = generation) is implicit: it is documented on the `ElectricPower` enum in `equipment.rs:782-831` but not enforced or validated at the port boundary. The `PortSlots::accumulate` method trusts that equipment authors follow the convention.

If an equipment module mistakenly reports generation via `active_power_kw: +3.0` (i.e., using `ElectricPower::Consumption(3.0)` for exported power), it would:
1. Be added to `load_power_kw` instead of `generation_power_kw`
2. Cause net power to be too high by 2 × the generation magnitude
3. Produce no warning or error

The `ElectricPower` enum provides a type-safe construction (`consumption()` rejects negative, `generation()` rejects negative) and `signed_kw()` returns signed values, but equipment modules writing directly to `PortContribution::Electrical` bypass this safety layer.

**Impact**: Low in practice because equipment modules are co-located in the same codebase and use the `ElectricPower` enum correctly. However, the port boundary is a natural place for defensive validation that catches integration errors before they propagate to the solver.

### Finding 4: [Severity: low]
**Description**: Sign conventions for `ElectricalAccumulator` are internally consistent but store generation as a negative number, which is non-obvious given the field name.

**Code Location**:
- `crates/hares-types/src/ports.rs:256-259` — `ElectricalAccumulator` struct:
  ```rust
  pub struct ElectricalAccumulator {
      pub reactive_power_kvar: f64,
      pub load_power_kw: f64,
      pub generation_power_kw: f64,
  }
  ```
- `crates/hares-types/src/ports.rs:263-266` — `net_active_kw()`:
  ```rust
  pub fn net_active_kw(&self) -> f64 {
      self.load_power_kw + self.generation_power_kw
  }
  ```

**Root Cause**: The field `generation_power_kw` stores generation as a **negative** value (e.g., −5.0 for 5 kW of PV export), while `load_power_kw` is always ≥ 0. The field name suggests it holds a positive magnitude of generation, similar to how EnergyPlus's `electProdRate_` is always positive. A reader encountering `generation_power_kw == -5.0` may misinterpret it as a −5 kW generation magnitude rather than 5 kW of exported power. The `net_active_kw()` docstring at `ports.rs:263` clarifies: "load (positive) + generation (negative)", and the test at `ports.rs:1085-1103` confirms: `load_power_kw = 3.0`, `generation_power_kw = -5.0`, `net = -2.0`.

**Impact**: Low — behavior is correct and tested. Documentation at `ports.rs:263` mitigates confusion. Consider either renaming to `generation_export_kw` or storing positive values with sign accounted in `net_active_kw()`.

### Finding 5: [Severity: low]
**Description**: No per-contribution finiteness validation on electrical port values. Malformed equipment output (NaN, infinity) is only caught at the aggregated solver output level.

**Code Location**:
- `crates/hares-types/src/ports.rs:537-538` — `reactive_power_kvar` and `active_power_kw` are accumulated without `is_finite()` checks
- `crates/hares-core/src/dwelling/mod.rs:3248-3255` — finiteness check on solver aggregate only

**Root Cause**: The `PortSlots::accumulate` method performs unconditional `+=` on `reactive_power_kvar`, `load_power_kw`, and `generation_power_kw` without validating that the contribution contains finite values. A faulty equipment producing NaN power would silently propagate through the port accumulator to the solver. The final finiteness check (`electrical_net_finite`, `dwelling/mod.rs:3248`) would catch it but cannot attribute the fault to a specific equipment.

**Impact**: Low — equipment modules produce finite values under normal operation. The diagnostic gap (cannot identify the source equipment) is a minor debugging inconvenience.

## Summary
- **Total findings**: 5
- **High**: 1 (electrical balance invariant check incompatible with non-trivial ZIP)
- **Medium**: 2 (reactive power lacks ZIP adjustment; no defensive sign validation at port boundary)
- **Low**: 2 (non-obvious generation sign storage; no per-contribution finiteness checks)

## Sign Convention Audit (positive findings)

The overall electrical sign convention architecture in HARES is correct and well-tested:

1. **Load = positive, generation = negative**: The `PortContribution::Electrical` convention is consistent with both OCHRE (`equipment.electric_kw > 0` for loads at `Dwelling.py:291`) and EnergyPlus (`totalElectricDemand_ > 0` and `electProdRate_ > 0`, net = demand − production at `ElectricPowerServiceManager.cc:503,527`).

2. **Net building power matches meter**: `ElectricalAccumulator::net_active_kw()` = `load_power_kw + generation_power_kw` correctly produces positive for net import and negative for net export. This is verified by the test at `ports.rs:1085-1103` and matches EnergyPlus's `electricityNetRate_ = totalElectricDemand_ − electProdRate_` at `ElectricPowerServiceManager.cc:527`.

3. **Battery charging/discharging convention**: Battery charging (consuming grid power) produces positive `active_power_kw` → routed to `load_power_kw`. Battery discharging (exporting) produces negative `active_power_kw` → routed to `generation_power_kw`. The `ElectricalSummary::battery_power_kw` field at `environment.rs:48` reflects signed power (positive = charging, negative = discharging), consistent with the review requirements.

4. **PV generation → negative convention**: PV equipment exports power using negative `active_power_kw` → `generation_power_kw` (negative). The `ElectricalSummary::pv_generation_kw` at `environment.rs:42` applies `-pv_kw` to convert back to a positive magnitude, consistent with its docstring "positive = producing".

5. **Cross-equipment boundaries**: PV (negative) + battery charging (positive) = net negative + positive. The `net_active_kw()` sum correctly avoids double-counting or cancellation errors. Example: PV exports 3 kW (−3.0) + battery charges 2 kW (+2.0) = net −1.0 kW (exporting). Verified by deduction from `ports.rs:264-266` and the test at `ports.rs:1085-1103`.

6. **ZIP model applied only to loads**: `electrical_solver.rs:120-122` correctly excludes generation power from ZIP scaling (`p_gen_adj = p_gen`). This is validated by the test `generation_is_not_zip_scaled` at `electrical_solver.rs:250-279`.

7. **Reactive power signage**: Positive = inductive (lagging) per IEEE 1547 convention (`equipment.rs:889`). The port accumulator sums reactive power as-is, preserving the signed convention. Consistent with OCHRE's `total_q_kvar += sub.reactive_kvar` at `Dwelling.py:292`.

8. **ZIP formula correctness**: The ZIP scaling formula `Z·V² + I·V + P` at `electrical_solver.rs:118` is verified by the IEEE test case `ieee_zip_reference_residential_load` at `electrical_solver.rs:316-343`.

9. **Base load separation**: `dwelling/mod.rs:2915-2916` correctly separates non-dispatchable base load by subtracting battery charging and EV charging from `load_power_kw`. `battery_kw.max(0.0)` correctly extracts only the charging component, as discharging produces negative `signed_kw()` and `active_power_kw` which routes to `generation_power_kw` (not `load_power_kw`).

## Recommendations

1. **Fix the electrical balance invariant check** (Finding 1): Either (a) apply the same ZIP scale factor to port values before comparison, or (b) gate the check to only execute when the ZIP model is effectively constant-power (`scale ≈ 1.0`). If the solver's ZIP coefficients are accessible from the dwelling, the adjusted check would be:
   ```rust
   let scale = self.electrical_solver.zip_scale(voltage_pu);
   let port_net = self.ports.electrical.load_power_kw * scale
                + self.ports.electrical.generation_power_kw;
   checker.check_electrical(net_kw, &[-port_net])?;
   ```

2. **Add reactive power ZIP scaling** (Finding 2): Optionally extend `ZipCoefficients` to include separate reactive ZIP coefficients and apply them symmetrically to `reactive_power_kvar`. If keeping the simplified approach, document the assumption that reactive power is assumed to follow a constant-power model regardless of voltage. Note that OCHRE uses separate coefficients for P and Q scaling.

3. **Add sign validation at the port boundary** (Finding 3): In `PortSlots::accumulate`, when `active_power_kw < 0` and `reactive_power_kvar < 0`, log a debug-level diagnostic or emit a warning if the convention is unusual (generation with negative reactive power implies a leading power factor, which is uncommon but valid for certain inverter modes).

4. **Clarify `generation_power_kw` storage semantics** (Finding 4): Either rename to `generation_export_kw` to signal the negated convention, or store a positive magnitude and account for the sign in `net_active_kw()` (matching EnergyPlus's approach).

5. **Add per-contribution finiteness guards** (Finding 5): In `PortSlots::accumulate`, add `debug_assert!` for finite `active_power_kw` and `reactive_power_kvar` values, or gate on `cfg(debug_assertions)` to catch NaN propagation at the source.

6. **Consider EnergyPlus-style separated metrics**: Following EnergyPlus's `electPurchRate_` (purchased, always non-negative) and `electSurplusRate_` (surplus, always non-negative) separation, consider adding `net_import_kw` and `net_export_kw` convenience accessors to `ElectricalAccumulator` for output reporting. Currently only `net_active_kw()` is exposed, requiring callers to test sign.

## References / Citations

- `crates/hares-types/src/ports.rs:256-271` — `ElectricalAccumulator` struct and `net_active_kw()`
- `crates/hares-types/src/ports.rs:534-543` — `PortSlots::accumulate` electrical branch
- `crates/hares-types/src/ports.rs:1085-1103` — test: `electrical_accumulator_tracks_load_and_generation_split`
- `crates/hares-types/src/equipment.rs:782-831` — `ElectricPower` enum, `signed_kw()`, `net_consumption_kw()`
- `crates/hares-types/src/environment.rs:40-52` — `ElectricalSummary` struct
- `crates/hares-core/src/dwelling/mod.rs:2913-2920` — `ElectricalSummary` construction from port and equipment data
- `crates/hares-core/src/dwelling/mod.rs:3257-3259` — electrical balance invariant call
- `crates/hares-core/src/invariants.rs:61-80` — `check_electrical` implementation
- `crates/hares-envelope/src/electrical_solver.rs:101-133` — `ElectricalSolver::resolve` with ZIP correction
- `crates/hares-envelope/src/electrical_solver.rs:228-279` — test: `zip_correction_matches_reference_formula`, `generation_is_not_zip_scaled`
- `crates/hares-envelope/src/electrical_solver.rs:282-302` — test: `electrical_balance_identity_holds`
- `vendors/OCHRE/ochre/Dwelling.py:230-291` — OCHRE per-timestep power accumulation
- `vendors/OCHRE/ochre/Dwelling.py:289-293` — `finish_sub_update` (equipment.kind check)
- `vendors/OCHRE/ochre/Equipment/Equipment.py:200-218` — OCHRE ZIP model (scales both active and reactive)
- `vendors/EnergyPlus/src/EnergyPlus/ElectricPowerServiceManager.cc:137-153` — EnergyPlus meter-based power aggregation
- `vendors/EnergyPlus/src/EnergyPlus/ElectricPowerServiceManager.cc:479-530` — `updateWholeBuildingRecords`: sign convention (demand − production)
- IEEE Task Force on Load Representation, IEEE T-PWRS 8(2):472–482 (1993) — residential ZIP model reference
- IEEE 1547-2018 — reactive power capability at point of common coupling
