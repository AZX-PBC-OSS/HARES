# `solve_ideal_capacity_for_target` Failure Should Log at `warn!` Not `debug!`

**Severity**: Nit
**Priority**: P4
**Status**: Open
**Areas**: hares-envelope

## Problem

When `solve_ideal_capacity_for_target` in `crates/hares-envelope` fails to converge to a setpoint-tracking capacity, the failure is logged at `tracing::debug!` level. This is too quiet: an ideal-HVAC convergence failure is a meaningful diagnostic event that affects load output (the zone will not track its setpoint that step) and should be visible in default operator logging at `info` or `warn` level, not buried at `debug`.

A user running an annual simulation and noticing that some hours have unexpected setpoint deviations has no way to discover that the ideal-capacity solver failed unless they re-run with `RUST_LOG=hares_envelope=debug`.

## Current Behavior

`solve_ideal_capacity_for_target` in `crates/hares-envelope/` (exact location: search for the function name) emits:
```rust
tracing::debug!(target: "hares_envelope::solver", "ideal capacity solve failed: ...");
```

## Required Behavior

1. Promote the failure log to `tracing::warn!` so it surfaces by default.
2. Include sufficient structured context in the log fields: zone id, target temperature, achieved temperature, outdoor temperature, capacity tried, error from convergence test.
3. Ensure the warn does not fire on every step in a pathological run — if the failure persists for many consecutive steps (e.g. the target is unreachable due to undersized equipment), throttle the warn or accumulate a count and emit one warn per N consecutive failures.

## Approach

1. Locate `solve_ideal_capacity_for_target` in `crates/hares-envelope/`.
2. Replace `tracing::debug!` with `tracing::warn!` at the failure site.
3. Add structured fields: `zone_id`, `target_c`, `achieved_c`, `oat_c`, `capacity_w`, `residual`.
4. Add a per-zone consecutive-failure counter that suppresses the warn after the first N failures in a row, with a single info-level recovery log when the solver next succeeds.
5. Add a unit test that triggers a deliberate ideal-capacity failure (e.g. impossible target) and asserts a `warn!` line is captured by `tracing-test`.

## Definition of Done

- [ ] Failure log at `solve_ideal_capacity_for_target` is `warn!`, not `debug!`
- [ ] Structured fields include zone, target, achieved, residual
- [ ] Repeat-failure throttling prevents log flood
- [ ] Recovery log fires once when the solver next succeeds after a failure run
- [ ] Unit test asserts the warn is emitted under deliberate-failure conditions

## Verification

```bash
cargo test -p hares-envelope solve_ideal_capacity
cargo test -p hares-envelope --features tracing-test
```

## References

- EnergyPlus Engineering Reference §6.3 "Ideal Loads Air System" — convergence failure is a documented diagnostic event in E+ and is reported in the Errors output file.
- `tracing` documentation on level selection: use `warn` for "an event that may be problematic but does not prevent the operation from continuing" — exactly the semantics here.

## Related Tickets

- 022-roomac-ideal-target (related ideal-target convergence)
