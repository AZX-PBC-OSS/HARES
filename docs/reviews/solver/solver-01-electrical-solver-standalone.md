# Electrical domain solver: ZIP model, voltage dependency, complex power
**Review ID**: solver-01
**Category**: solver
**Date**: 2026-05-26

## Files Reviewed
crates/hares-envelope/src/electrical_solver.rs

## Vendor/Reference Files Consulted
vendors/EnergyPlus/src/EnergyPlus/ElectricPowerServiceManager.cc

## Findings
### Finding 1: [Severity: high] No voltage range clamping — ANSI C84.1 bounds not enforced
**Description**: The solver does not clamp or reject `voltage_pu` against the ANSI C84.1 Range A [0.9, 1.05] pu or the wider modeling range [0.9, 1.1] pu specified in the review brief. The `resolve()` method at line 117 reads `env.grid.voltage_pu` and uses it unconditionally in the ZIP polynomial evaluation. The `GridState` struct (`hares-types/src/environment.rs:305-308`) has no validation on `voltage_pu`, and the `set_grid_voltage()` accessor (`hares-core/src/dwelling/mod.rs:1937-1942`) writes it directly with no bounds check.

**Code Location**: `crates/hares-envelope/src/electrical_solver.rs:117`
```
let v = env.grid.voltage_pu / self.config.nominal_voltage_pu;
```

**Root Cause**: The ZIP model was implemented as a pure algebraic transform with no domain-guard logic around voltage inputs. Neither the `ElectricalSolverConfig`, the `resolve()` method, nor the `GridState` type enforce ANSI C84.1 bounds.

**Impact**: Out-of-range voltages (e.g., grid fault at 0.0 pu, overvoltage at 1.5 pu) produce physically invalid ZIP-scaled power without warning. A voltage of 0.0 pu would yield `load_scale = zip.p` only (since z*v^2 = 0 and i*v = 0), understating load — not the correct behavior for a zero-voltage condition. A voltage of 2.0 pu would produce exaggerated load scaling. In production, simulation scenarios with grid voltage excursions outside the normal operating envelope would propagate silently, potentially misleading CVR and voltage-stability studies.

**Comparison to vendor**: EnergyPlus's `ElectricPowerServiceManager` does not implement a ZIP model at the service level — it dispatches generators to meet building electrical demand and does not apply voltage-dependent load scaling. Voltage-dependent behavior in EnergyPlus is handled per-equipment-model (e.g., induction motor models within chiller/compressor sub-models). Neither codebase has explicit ANSI voltage-range clamping at the service level, so this is a gap in both implementations, but HARES's explicit ZIP model makes voltage range enforcement more important.

---

### Finding 2: [Severity: medium] Missing apparent power (S) and power factor (PF) from solver output
**Description**: The `resolve()` method outputs only raw P and Q as a 2-element vector (`net_active_kw` and `net_reactive_kvar` at lines 131-132) but never computes apparent power `S = sqrt(P^2 + Q^2)` or power factor `PF = P/S`. These are standard outputs of any electrical bus analysis and essential for: (a) sizing service-entrance equipment and transformer capacity based on kVA rather than kW, (b) computing utility demand charges based on kVA, (c) identifying leading vs. lagging power factor for voltage regulation studies, and (d) reporting power quality metrics. The sign convention for PF (leading vs. lagging) is critical: lagging PF (inductive load, Q > 0) and leading PF (capacitive/generation, Q < 0) have different impacts on voltage regulation and utility penalties.

**Code Location**: `crates/hares-envelope/src/electrical_solver.rs:122-132`
```
self.net_reactive_kvar = ports.electrical.reactive_power_kvar;
// ...
payload.push(self.net_active_kw);
payload.push(self.net_reactive_kvar);
```

**Root Cause**: The solver was designed as a simple aggregation and ZIP-scaling pass-through. S and PF were deferred to downstream consumers but never implemented anywhere in the current codebase.

**Impact**: Consumers of `DomainUpdate.custom_payload` must implement their own S and PF calculations from the raw P/Q pair. Without a centralized computation, different consumers may use inconsistent sign conventions for PF (some may report |PF|, others may report signed PF where negative indicates leading). No apparent power value is available in the simulation output for transformer sizing or kVA-based demand charge calculations.

---

### Finding 3: [Severity: medium] Reactive power ZIP not modeled in the domain-level solver
**Description**: The `ZipCoefficients` struct (line 11-15) contains only real-power coefficients `{z, i, p}`. There is no counterpart for reactive-power ZIP coefficients (`zq`, `iq`, `pq`). The solver's `resolve()` method at line 122 passes `ports.electrical.reactive_power_kvar` through unchanged without applying any voltage-dependent ZIP correction to reactive power. This means reactive power is always treated as constant-power regardless of voltage — equivalent to implicit `zq=0, iq=0, pq=1`. In contrast, the equipment-level `WaterHeaterZip` struct (`crates/hares-equipment/src/water_heater/mod.rs:243-259`) includes full real and reactive ZIP coefficients and applies voltage-dependent scaling to both P and Q via its `apply()` method. Scheduled loads that use `WaterHeaterZip` will compute reactive ZIP at the equipment level before writing to ports, so reactive power arriving at the port may already be ZIP-corrected. However, the domain-level solver makes this correction invisible and non-configurable; if any equipment writes raw (pre-ZIP) reactive power to the port, the solver will not correct it.

**Code Location**: `crates/hares-envelope/src/electrical_solver.rs:11-15` (ZipCoefficients struct), line 122 (reactive bypass)

**Root Cause**: Architectural layering — some equipment types apply their own ZIP scaling (using `WaterHeaterZip`), while the domain solver applies ZIP only to active power loads. There is no consistent contract about whether equipment should write pre-ZIP or post-ZIP power to ports. The solver's `ZipCoefficients` was designed only for active power, reflecting the original scope of the model.

**Impact**: Moderate. Equipment types that do not self-apply reactive ZIP (e.g., `EventBasedLoad`, `WetAppliance` — which hard-code `reactive_power_kvar: 0.0`) will have zero reactive power regardless of voltage, losing motor-driven reactive effects. Equipment types that do self-apply reactive ZIP will have it applied but the solver emits no signal about whether Q was ZIP-corrected. A future refactor could move all ZIP application into the domain solver for a single point of truth.

---

### Finding 4: [Severity: low] Unnecessarily restrictive non-negativity constraint on real-power ZIP coefficients
**Description**: `ZipCoefficients::new()` (line 30-39) enforces `z >= 0.0`, `i >= 0.0`, `p >= 0.0`:
```rust
if z < 0.0 || i < 0.0 || p < 0.0 {
    return Err(ElectricalSolverError::NegativeZipCoefficient { z, i, p });
}
```
This constraint is overly restrictive. The IEEE Task Force on Load Representation (1993) and modern experimental determinations (Bokhari et al., IEEE Trans. Power Delivery, 2014) document valid ZIP models with negative coefficients. For example, a residential refrigerator documented in Bokhari 2014 has `Z = 5.03, I = -8.48, P = 4.45` — all three coefficients far from the `[0, inf)` range, yet the sum is exactly 1.0. OCHRE's `ZIP Parameters.csv` includes these exact values. The HARES defaults file `defaults/zip_parameters.toml` contains these values and they pass validation because equipment-level ZIP validation in `water_heater/mod.rs:335-339` uses a laxer constraint (sum error < 0.01, no sign check). But the domain-solver-level `ZipCoefficients` would reject them if applied at the bus level.

**Code Location**: `crates/hares-envelope/src/electrical_solver.rs:30-33`

**Root Cause**: The sign constraint was added as a safety check but is based on an oversimplified assumption that ZIP coefficients represent physical impedance/current/power fractions that must be non-negative. In reality, negative coefficients arise from the polynomial fitting process and are physically meaningful as long as the polynomial value at normal voltage is positive. The IEEE ZIP model as formalized in IEEE T-PWRS 8(2):472-482 (1993) does not impose a sign restriction.

**Impact**: Low for the default use case (constant-power loads). Affects users who attempt to apply literature-derived ZIP models (e.g., Refrigerator, Freezer, electronics) at the bus level rather than at the equipment level. These models are currently applied at the equipment level via `WaterHeaterZip` which has no sign constraint, so the domain-solver constraint does not block existing functionality. It would block anyone trying to model a bus with aggregated, literature-based ZIP coefficients that include negative terms.

---

### Finding 5: [Severity: low] `resolve_new` test helper not defined in the electrical_solver module; relies on a method from the `DomainSolver` trait
**Description**: All unit tests in the module call `solver.resolve_new(&ports, &env, Duration::from_secs(60))` (e.g., lines 223, 244, 272, 276, 298, 309, 335, 371) rather than the trait method `solver.resolve(&mut ports, &env, dt, &mut out)`. The `resolve_new` method is not defined in this file — it is presumably a convenience method on the `DomainSolver` trait that creates a fresh `DomainUpdate` and delegates to `resolve()`. This is confirmed to work correctly from the test assertions and is described as a trait method. Not a defect.

---

### Finding 6: [Severity: low] No documentation of sign convention in the solver doc comments
**Description**: The sign convention for `net_active_kw` is described in `ElectricalAccumulator::net_active_kw()` (`hares-types/src/ports.rs:263-264`) as "Net active power: load (positive) + generation (negative)." The solver's `resolve()` method at line 120-122 implements `net_active_kw = p_load_adj + p_gen_adj`, where `p_gen_adj = p_gen` is negative for generation. This yields positive values for net consumption and negative values for net export — the standard "consumed power" convention used in load-flow studies. However, this convention is never stated in the solver's own doc comments. The method comment at lines 82-85 mentions "net active power result" without specifying the sign convention, and the single-line comment at lines 115-116 covers nominal voltage but not the sign convention.

**Code Location**: `crates/hares-envelope/src/electrical_solver.rs:82-85, 120-122`

**Impact**: Low. The convention is correct and testable, but undocumented. A user integrating with an external system expecting the "generation positive" convention (common in PV monitoring systems) could misinterpret the sign.

---

## Summary
- Total findings: 5 (Finding 5 is an informational observation, not a defect)
- Critical: 0
- High: 1 (voltage range not clamped)
- Medium: 2 (missing S/PF, reactive ZIP not in domain solver)
- Low: 2 (restrictive non-negativity constraint, sign convention not documented)

## Recommendations
1. **Add voltage range enforcement** (Finding 1): Clamp `voltage_pu` to [0.9, 1.1] pu in the solver's `resolve()` method with a warning log for out-of-range values. Alternatively, validate and reject out-of-range voltages with a clear error. The ANSI C84.1 Range A [0.9, 1.05] pu is the tighter operational envelope; [0.9, 1.1] pu is the wider modeling range. Document which bound is used.

2. **Compute S and PF in solver output** (Finding 2): Add `S = sqrt(P^2 + Q^2)` and `PF = P/S` (signed: positive for lagging/inductive Q>0, negative for leading/capacitive Q<0) to the `DomainUpdate.custom_payload` vector alongside P and Q. Alternatively, add methods `net_apparent_kva()` and `power_factor()` to `ElectricalSolver` that return these derived values.

3. **Add reactive ZIP coefficients to the domain solver** (Finding 3): Extend `ZipCoefficients` to include `zq`, `iq`, `pq` for reactive power, validate that they sum to 1.0 within tolerance, and apply the reactive ZIP polynomial in `resolve()`: `Q_adj = Q_nominal * (zq * v^2 + iq * v + pq)`. This would make the domain solver the single point of truth for ZIP application.

4. **Remove the non-negativity constraint** (Finding 4): Remove the sign check from `ZipCoefficients::new()` and rely solely on the sum-to-1.0 validation (within 1e-6). Add a doc comment explaining that negative coefficients are valid per the IEEE ZIP model. Consider adding an additional validation that the polynomial value at any voltage in [0.9, 1.1] is non-negative (a stronger constraint that still permits negative coefficients).

5. **Document sign convention** (Finding 6): Add a doc comment on the solver's `resolve()` method stating the sign convention explicitly: "Positive net_active_kw = power consumed from grid; negative = power exported to grid."

## References / Citations
- IEEE Task Force on Load Representation for Dynamic Performance (1993). "Load representation for dynamic performance analysis." IEEE Trans. Power Systems, 8(2):472-482 — ZIP model formalism: `P = P_0 [p_z (V/V_0)^2 + p_i (V/V_0) + p_p]` with coefficients summing to 1.0.
- Bokhari, A. et al. (2014). "Experimental Determination of the ZIP Coefficients for Modern Residential, Commercial, and Industrial Loads." IEEE Trans. Power Delivery, 29(3):1372-1381 — Documents negative ZIP coefficients for residential refrigeration (Z=5.03, I=-8.48, P=4.45).
- ANSI C84.1-2020, "Electric Power Systems and Equipment — Voltage Ratings (60 Hertz)" — Range A: [0.95, 1.05] pu for service voltage; Range B (wider): [0.917, 1.058] pu. HARES review brief specifies [0.9, 1.05] pu Range A and [0.9, 1.1] pu modeling range.
- EnergyPlus ElectricPowerServiceManager (vendors/EnergyPlus/src/EnergyPlus/ElectricPowerServiceManager.cc) — Electrical load center dispatcher; generator dispatch and transformer loss accounting. No ZIP model at the service level. Net power computed as `electPurchRate_ = totalElectricDemand_ - electProdRate_` (line 503), equivalent to HARES's `net_active_kw = load + generation` with generation as negative.
- OCHRE Equipment.py:200-218 `run_zip()` — Per-equipment ZIP application in the OCHRE Python reference: `electric_kw *= z * v^2 + i * v + p` for real power and `reactive_kvar = electric_kw * pf * (zq * v^2 + iq * v + pq)` for reactive, mirrored by HARES `WaterHeaterZip::apply()` in `crates/hares-equipment/src/water_heater/mod.rs:310-323`.
- HARES `ElectricalAccumulator` sign convention: `hares-types/src/ports.rs:264-265` — "Net active power: load (positive) + generation (negative)."
- HARES prior review: `docs/reviews/core-deep/coredeep-04-domain-solver-ordering.md` Finding 4 — Notes electrical solver is a pure post-processor (ZIP correction + summation).
