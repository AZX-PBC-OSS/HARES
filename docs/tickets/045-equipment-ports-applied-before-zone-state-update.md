# Equipment Ports Applied to Stale Zone State in Same-Timestep Thermal Solve

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-core, hares-envelope
**Related**: Ticket 047 (interior LWR uses last-step zone temperature) — addresses the LWR convergence loop's stale zone-air reference and defective convergence criterion. The two defects share a root class (stale prior-step values) but are caused by different code paths and require separate fixes.

## Problem

`run_timestep` in `hares-core/src/dwelling/mod.rs` has two temporal-consistency defects caused by the two-phase prepare/integrate design:

### Defect 1: Non-thermal equipment sees post-integrate zone temperatures

Ordering in `run_timestep`:

1. `apply_thermal_update_to_zones` writes post-integrate zone temperatures to `latest_env.zones` — `mod.rs:2240`
2. Non-thermal equipment step (`mod.rs:2168`) runs **after** this write

Non-thermal equipment (PV, battery, EV) receives `&self.latest_env` with already-updated zone temperatures, while thermal equipment stepped before the integrate call and saw last-step temperatures. All equipment in a single timestep should observe the same environmental state. EnergyPlus Engineering Reference §"Predictor-Corrector Zone Air Heat Balance": loads are computed at a consistent predictor zone air state before any corrector update is applied. ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.2 "Heat Balance Method": loads computed at the same zone air temperature within a timestep.

### Defect 2: One-step humidity lag in the humidity solver

The humidity solver at `mod.rs:2260–2265` is called after `apply_thermal_update_to_zones` (`mod.rs:2240`) but before `apply_humidity_update_to_zones` (`mod.rs:2279`). It reads `zone.humidity_ratio` from `env.zones`, which still carries the last-step value. This produces a one-step lag in air density used to compute the humidity ratio increment.

Mitigating factor: `humidity_solver.rs:129–133` reads `w_old` from `self.humidity_ratios` (the solver's own committed state), not from `env.zones`. The `env.zones[i].humidity_ratio` field is used only as a fallback for zones absent from `humidity_ratios`. If that fallback path is never reached in steady-state operation, Defect 2 has no practical effect — but this must be confirmed by a `debug_assert!`.

## Current Behavior

`hares-core/src/dwelling/mod.rs:2240`: `apply_thermal_update_to_zones` writes post-step zone temperatures to `latest_env`.
`hares-core/src/dwelling/mod.rs:2168`: non-thermal equipment step uses `&self.latest_env` — receives post-integrate zone temperatures.
`hares-core/src/dwelling/mod.rs:2260–2265`: humidity solver called with `env.zones[i].humidity_ratio` still at last-step value.

## Required Behavior

1. **Non-thermal equipment temporal ordering**: Either move the non-thermal equipment step to before `integrate` (Step 4 in the timestep sequence) so it observes predictor-consistent zone temperatures, or add an explicit code comment and an invariant assertion confirming that non-thermal equipment writes no thermal ports and the ordering has no physics consequence.

2. **Humidity fallback path**: Add a `debug_assert!` inside the `env.zones` fallback branch at `humidity_solver.rs:129–133` confirming it is never reached in normal simulation paths. If it is reachable, read from `self.humidity_ratios` (the committed state) instead of `env.zones` to eliminate the one-step lag.

3. **Invariant check**: Add a debug-mode assertion at the start of each timestep that `env.zones[i].humidity_ratio == humidity_solver.committed_humidity_ratio(zone_id)` for every conditioned zone. This confirms that the humidity state in `latest_env` is consistent with the solver's committed state at step entry.

Reference: EnergyPlus Engineering Reference §"Zone Air Heat Balance Predictor-Corrector"; ASHRAE HoF 2021 Ch. 18 §18.2.

## Approach

In `run_timestep` (`mod.rs`):
- Either shift the non-thermal stage (currently Step 3b) to before `integrate`, or add a comment and assertion.
- In `humidity_solver.rs`, add `debug_assert!` inside the `env.zones` fallback branch.
- Add the step-start invariant check comparing `env.zones` humidity to solver committed state.

## Definition of Done

- [ ] Non-thermal equipment step ordering is either moved before `integrate` or documented with a `debug_assert!` that it writes no thermal ports
- [ ] `debug_assert!` inside the humidity fallback branch at `humidity_solver.rs:129–133` confirms it is unreachable in steady-state
- [ ] Step-start invariant: `debug_assert!(env.zones[i].humidity_ratio == humidity_solver.committed_humidity_ratio(zone_id))` for all conditioned zones
- [ ] `cargo test -p hares-core` passes

## Verification

```bash
cargo test -p hares-core
```

## References

- EnergyPlus Engineering Reference §"Zone Air Heat Balance Predictor-Corrector" — consistent predictor zone state for all load computations before corrector
- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.2 "Heat Balance Method" — loads computed at the same zone air temperature within a timestep
- `hares-core/src/dwelling/mod.rs:2168–2279` — `run_timestep` orchestration
- `hares-envelope/src/humidity_solver.rs:129–133` — `w_old` from `self.humidity_ratios`
