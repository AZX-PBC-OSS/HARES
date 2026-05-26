# Ideal HVAC back-solve correctness: capacity vs load matching
**Review ID**: equip-hvac-09
**Category**: equipment-hvac
**Date**: 2026-05-25

## Files Reviewed
- `crates/hares-equipment/src/hvac/ideal_hvac.rs`
- `crates/hares-equipment/src/hvac/hvac_core.rs`
- `crates/hares-core/src/actors/solver_feedback.rs`
- `crates/hares-envelope/src/thermal_solver/stepping.rs`
- `crates/hares-envelope/src/state_space.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/HVAC.py`
- `vendors/OCHRE/ochre/Models/RCModel.py`

## Findings

### Finding 1: [Severity: medium]
**Description**: The `solve_ideal_capacity_for_target` solver computes the net sensible heat gain required at the zone thermal port to reach the setpoint. However, `IdealHvac::step()` interprets that value as gross coil capacity and adds fan heat on top, resulting in systematic zone-temperature drift when fan power is configured (non-zero `rated_fan_power_w`). OCHRE correctly applies a fan-power feedback correction.

**Code Location**:
- Solver (computes net sensible gain): `crates/hares-envelope/src/thermal_solver/stepping.rs:321-413` (`solve_ideal_capacity_for_target`), which delegates to `crates/hares-envelope/src/state_space.rs:488-543` (`solve_for_scalar_input_coupled`) or `:545-593` (`solve_for_scalar_input`). The return value is the additional input `Δu` that makes `y_next == y_target` — i.e., the net sensible W for the zone port.
- Equipment (adds fan heat on top): `crates/hares-equipment/src/hvac/ideal_hvac.rs:512-519` uses `ideal_capacity_w` directly as coil capacity. Lines 544–553 compute `fan_power_w = |capacity_w| * effective_eir * fan_power_ratio`. Line 569 writes `sensible_gain_w: sensible_w + fan_power_w` to the thermal port.
- OCHRE counterexample: `vendors/OCHRE/ochre/Equipment/HVAC.py:411-429` (`solve_ideal_capacity`). After obtaining `h_desired` from `envelope_model.solve_for_inputs()`, it divides by the fan-power correction: `h_desired / (self.shr + self.eir * self.fan_power_ratio)` for heating, `-h_desired / (self.shr - self.eir * self.fan_power_ratio)` for cooling.

**Root Cause**: Semantic mismatch between the solver's output (net sensible port gain) and the equipment's interpretation (gross coil capacity). The solver's contract — confirmed by the test at `thermal_solver/mod.rs:2064-2130` — is that its return value, when set directly as `ThermalAccumulator::sensible_gain_w`, drives the zone to the target. But `IdealHvac::step()` adds fan power on top of what it receives, inflating the delivered heat.

Concrete arithmetic for heating (shr = 1.0 by default):
- Solver returns `h_desired` (net sensible W needed).
- Equipment delivers `h_desired * (1 + effective_eir * fan_power_ratio)`, overshooting the target.

For cooling (capacity_w < 0):
- Delivered sensible = `capacity_w * shr + |-capacity_w| * eir * fan_power_ratio = capacity_w * (shr - eir * fan_power_ratio)`.
- Since `shr - eir * fan_power_ratio < shr`, the cooling magnitude is reduced (less negative), under-cooling the zone.

**Impact**: When `rated_fan_power_w` is zero (the default), `fan_power_ratio = 0.0` and there is no error. When fan power is configured — a common realistic case — the ideal HVAC systematically overshoots in heating and under-cools in cooling. For a typical residential system with `rated_eir = 1.0` (COP=1 ideal), `shr = 0.8`, and a 500 W fan on a 10 kW system (`fan_power_ratio = 0.05`), the heating overshoot is ~5% and the cooling shortfall is ~6%. This grows linearly with fan power.

### Finding 2: [Severity: low]
**Description**: The ideal capacity back-solve is a one-shot analytical solve with no iteration to convergence. This is correct for linear discrete-time state-space models — but means the solver does not account for nonlinear, state-dependent effects that emerge when the zone temperature changes within the timestep (e.g., natural ventilation driven by ΔT, changing infiltration rates, or temperature-dependent UA). OCHRE uses the same one-shot approach (see `RCModel.py:305-333`) and explicitly documents in its class docstring that it "does not account for heat gains from other equipment in the same time step."

**Code Location**:
- HARES: `crates/hares-envelope/src/state_space.rs:545-593` — `solve_for_scalar_input` computes `Δu = (y_target - y_fixed) / effective_gain` in a single pass.
- OCHRE: `vendors/OCHRE/ochre/Models/RCModel.py:305-333` — `solve_for_inputs` computes `u_desired = (y_desired - y_current) / u_factor`.

**Root Cause**: Both systems use a linear state-space model where the relationship between a scalar input and the next-step output is linear and time-invariant within the step. A closed-form solution is exact for this formulation. No convergence issues exist so long as the modeling assumptions (constant input over the timestep, fixed matrices) hold.

**Impact**: For HARES, the implicit integration scheme (M matrix, M = I − h*A) is more accurate than an explicit Euler step for stiff thermal problems. So this is not a bug — it's a design characteristic shared with the reference implementation. However, systems with strong temperature-dependent coupling (e.g., buoyancy-driven natural ventilation) would get a more accurate capacity estimate from an iterative approach, and neither HARES nor OCHRE provides that.

### Finding 3: [Severity: low]
**Description**: The `SolverFeedbackActor` solves for each equipment's ideal capacity independently (per-zone), with no coordination when multiple ideal HVAC units serve the same zone. If two `IdealHvac` instances both return `ideal_target()` for the same `ZoneId`, the solver computes the full capacity for each, and both units inject the full load into the thermal accumulator, doubling the delivered heat.

**Code Location**:
- `crates/hares-core/src/actors/solver_feedback.rs:72-83` — `collect_with` iterates equipment and calls `solve(zone, target_c)` for each independently.
- `crates/hares-envelope/src/thermal_solver/stepping.rs:321-413` — `solve_ideal_capacity_for_target` computes capacity for a single zone, unaware of what other equipment will inject.

**Root Cause**: The solver has no concept of shared zones or load splitting. The solver feedback loop treats equipment-zone pairs independently.

**Impact**: Multiple ideal HVAC units on the same zone are an unusual configuration in residential simulation, but if configured, they would over-condition the zone. A production fix could divide by the count of ideal equipment per zone, or the solver could model a shared input.

## Summary
- Total findings: 3
- Critical: 0
- High: 0
- Medium: 1
- Low: 2

## Recommendations

1. **Fix fan-power feedback (Finding 1, medium):** In `IdealHvac::step()`, when `use_ideal_cached` is true, divide the solver-provided `ideal_capacity_w` by the fan-power correction factor before computing sensible gain, mirroring OCHRE:
   - Heating: `capacity = ideal_capacity_w / (shr + rated_eir * fan_power_ratio)`
   - Cooling: `capacity = ideal_capacity_w / (shr - rated_eir * fan_power_ratio)`
   
   Guard against division by zero or negative denominator (pathological configurations where `shr <= eir * fan_power_ratio` for cooling should emit a warning and fall through to uncorrected capacity). This change brings HARES into parity with OCHRE's formula at `HVAC.py:427-429`.

2. **Document the one-shot solve assumption (Finding 2, low):** Add a module-level doc comment on `solver_feedback.rs` or `stepping.rs` stating that the capacity back-solve is a closed-form linear solve, exact for the discretized state-space model but not accounting for state-dependent nonlinear coupling within the timestep. This sets user expectations and reduces false bug reports.

3. **Mitigate multi-equipment double-counting (Finding 3, low):** Add a guard in `SolverFeedbackActor::collect_with` that tracks which zones already have a pending ideal capacity and skips (or divides by count) for subsequent equipment serving the same zone. Alternatively, document that only one ideal HVAC unit per zone is supported.

## References / Citations
- OCHRE `solve_ideal_capacity()`: `vendors/OCHRE/ochre/Equipment/HVAC.py:411-429`
- OCHRE `solve_for_inputs()`: `vendors/OCHRE/ochre/Models/RCModel.py:305-333`
- HARES `solve_for_scalar_input()`: `crates/hares-envelope/src/state_space.rs:545-593`
- HARES `solve_for_scalar_input_coupled()`: `crates/hares-envelope/src/state_space.rs:488-543`
- HARES `collect_and_solve()`: `crates/hares-core/src/actors/solver_feedback.rs:52-60`
- HARES test confirming solver output = net sensible gain: `crates/hares-envelope/src/thermal_solver/mod.rs:2064-2130`
- HARES `IdealHvac::step()` fan power computation: `crates/hares-equipment/src/hvac/ideal_hvac.rs:541-569`
