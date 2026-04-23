# Interior LWR Iterative Solve Uses Last-Step Zone Temperature as Radiation Reference

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-envelope
**Related**: Ticket 045 (equipment-ports-applied-before-zone-state-update) — that ticket addresses non-thermal equipment ordering and humidity lag; this ticket addresses a distinct mechanism: the LWR convergence loop's stale zone-air temperature reference and a defective convergence criterion.

## Problem

`apply_interior_longwave_inputs` in `hares-envelope/src/thermal_solver/longwave.rs` reads zone temperature from `env.zones` at lines 229–236:

```rust
let t_zone_c = env
    .zones
    .iter()
    .find(|z| z.id == zone_cfg.zone_id)
    .map(|z| z.temperature_c)
    .unwrap_or_else(|| { ... 20.0 });
```

`env.zones[i].temperature_c` is the committed value from the prior timestep, not the end-of-step temperature the ZOH integration will produce. The zone-air LWR reference therefore lags by one full timestep throughout all iterations of the convergence loop at `longwave.rs:286–318`.

For a 15-minute timestep with a 1 °C/step zone temperature ramp (aggressive HVAC cycling), this 1 °C error in the driving temperature produces an LWR flux error of approximately `4εσT³ × A_total × ΔT ≈ 4 × 0.9 × 5.67e-8 × 293³ × 30 × 1 ≈ 16 W` across 30 m² of interior surface — significant relative to typical interior LWR magnitudes of 50–200 W.

EnergyPlus Engineering Reference §13.1 "Zone Air Heat Balance Predictor-Corrector" uses a predictor-step zone temperature (not the prior-timestep value) when evaluating LWR exchange. EnergyPlus Engineering Reference §14.3 "Interior Long-Wave Radiation Exchange" specifies that the iterative surface temperature solve uses the current-step zone air temperature as its background reference.

### Secondary defect: defective convergence criterion

`longwave.rs:312–315` tests the heavy-ball step magnitude (`|buf[j] - prev_buf[j]|`) as the convergence criterion. This measures the momentum update, not the LWR flux residual. The correct convergence test is the relative flux residual `|q_new[j] - q_old[j]| / (|q_old[j]| + ε)`, which directly reflects whether the surface temperature iteration has closed the energy balance.

## Current Behavior

`hares-envelope/src/thermal_solver/longwave.rs:229–236`: `t_zone_c` read from `env.zones` (prior-step committed value), held fixed throughout all iterations.

`hares-envelope/src/thermal_solver/longwave.rs:286`:
```rust
let n_iter = (self.dt_s / 300.0_f64).floor() as u32 + 3;
```
Iteration count scales with timestep duration but is fixed regardless of transient rate-of-change.

`hares-envelope/src/thermal_solver/longwave.rs:312–315`: convergence tested on heavy-ball step magnitude, not LWR flux residual.

OCHRE's `_solve_interior_radiation` uses the current-timestep HVAC-updated zone temperature as the predictor value. HARES uses the prior-step value — a regression relative to OCHRE.

## Required Behavior

1. **Zone temperature reference**: Pass the predictor-step zone temperature to `apply_interior_longwave_inputs` instead of reading from `env.zones`. Implement a predictor-corrector: run `integrate` once with last-step LWR to obtain a predicted zone temperature; pass that predicted temperature into the LWR convergence loop; re-run `integrate` with the corrected LWR input. This matches EnergyPlus Engineering Reference §13.1.

   Minimum acceptable fix if the full predictor-corrector is deferred: add a `tracing::debug!` log per timestep reporting the maximum zone temperature change from the prior step, making the lag observable. Do not leave the stale reference without telemetry.

2. **Convergence criterion**: Replace the heavy-ball step magnitude test with a relative LWR net flux residual:
   ```
   |q_new[j] - q_old[j]| / (|q_old[j]| + 1e-6) < 1e-4
   ```
   where `q[j]` is the net LWR flux into surface `j` (W). This is the physically meaningful convergence indicator per EnergyPlus Engineering Reference §14.3.

References:
- EnergyPlus Engineering Reference §13.1 "Zone Air Heat Balance Predictor-Corrector" — predictor zone temperature used in all load calculations
- EnergyPlus Engineering Reference §14.3 "Interior Long-Wave Radiation Exchange" — iterative surface solve with current-step zone air temperature
- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.35 "Radiant Exchange Among Room Surfaces"

## Approach

In `apply_interior_longwave_inputs` (`longwave.rs`):
1. Add a `t_zone_predicted_c: Option<f64>` parameter (or a dedicated predictor value on `ThermalSolver`). When `Some`, use it instead of `env.zones[i].temperature_c`.
2. In the caller (`stepping.rs` or `mod.rs`), call `integrate` once to get the predicted temperatures; extract the zone air node temperature; pass it back to `apply_interior_longwave_inputs` before the corrector `integrate`.
3. Replace the convergence test at `longwave.rs:312–315` with the relative flux residual check.

## Definition of Done

- [ ] `apply_interior_longwave_inputs` accepts a predictor zone temperature and uses it throughout all convergence iterations
- [ ] Predictor-corrector: `integrate` called once (predictor), LWR re-evaluated with predicted zone temperature, `integrate` called again (corrector)
- [ ] Convergence criterion tests `|q_new[j] - q_old[j]| / (|q_old[j]| + 1e-6) < 1e-4` for net LWR flux
- [ ] `tracing::debug!` emits maximum zone temperature change per step (observable without full predictor-corrector)
- [ ] `cargo test -p hares-envelope` passes; no regression in radiant/LWR tests
- [ ] Test: with a 1 °C/step zone ramp, LWR flux computed with predictor temperature differs from prior-step temperature by < 2 W per surface at convergence

## Verification

```bash
cargo test -p hares-envelope
```

## References

- EnergyPlus Engineering Reference §13.1 "Zone Air Heat Balance Predictor-Corrector"
- EnergyPlus Engineering Reference §14.3 "Interior Long-Wave Radiation Exchange"
- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.35 "Radiant Exchange Among Room Surfaces"
- `hares-envelope/src/thermal_solver/longwave.rs:229–236` — stale zone temperature read
- `hares-envelope/src/thermal_solver/longwave.rs:312–315` — defective convergence criterion
