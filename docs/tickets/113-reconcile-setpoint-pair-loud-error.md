# `reconcile_setpoint_pair` Must Surface Setpoint Mutation Loudly

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-io/hpxml/resolve_hvac

## Problem

`crates/hares-io/src/hpxml/resolve_hvac.rs:2286-2319` `reconcile_setpoint_pair` silently mutates HPXML setpoints when the heating-cooling setpoint gap is less than 2°C. Only a `tracing::warn!` is emitted. The user's input data is changed without a machine-readable signal — downstream Python code, dashboards, and CSV diagnostics have no way to know that the setpoints they see are not what was supplied.

A heating setpoint of 21°C and a cooling setpoint of 22°C is a 1°C gap. The reconciliation widens the gap (typically to 2°C or 4°C) so the thermostat FSM has a deadband. The widening is reasonable physics, but silently changing the user's input violates `feedback_no_silent_defaults`.

## Current Behavior

`crates/hares-io/src/hpxml/resolve_hvac.rs:2286-2319`:
```rust
if (cooling_setpoint - heating_setpoint).abs() < 2.0 {
    tracing::warn!("setpoints too close; widening");
    heating_setpoint -= 1.0;
    cooling_setpoint += 1.0;
}
```

The warning logs; no error is returned and no flag is exposed in the resolved config.

## Required Behavior

Choose one:

A. **Return a hard error** — `HpxmlError::InvalidField { field: "HVACPlant", reason: "heating and cooling setpoints differ by less than 2°C" }`. The user must explicitly fix the input before the simulation will run.

B. **Surface a machine-readable signal** — add a `setpoints_reconciled: Option<SetpointReconciliation>` field to the resolved config carrying the original and adjusted values. Downstream consumers can detect and report the mutation.

Recommended path: A for strictness (reconciliation is silently changing the user's input); fall back to B if running an HPXML library that often produces narrow gaps.

## Approach

1. Choose path A or B based on the typical HPXML inputs encountered.
2. If A:
   - Replace the silent widening with `return Err(HpxmlError::InvalidField { ... })`.
   - Add a fixture with narrow setpoints; assert the resolver errors.
   - Update HPXML import documentation to require sensible setpoint gaps.
3. If B:
   - Add `SetpointReconciliation { original_heating_c: f64, original_cooling_c: f64, adjusted_heating_c: f64, adjusted_cooling_c: f64 }`.
   - Populate when widening occurs.
   - Expose in resolved config and Python introspection.
   - Keep the `tracing::warn!` for visibility.
   - Add a fixture and test asserting the structure is populated.

## Definition of Done

- [ ] `reconcile_setpoint_pair` either errors loudly (path A) or exposes the reconciliation as a structured field (path B)
- [ ] `tracing::warn!` retained for log-level visibility
- [ ] Fixture exercises the narrow-setpoint case and asserts the chosen behaviour
- [ ] Documentation updated explaining the reconciliation policy

## Verification

```bash
cargo test -p hares-io resolve_hvac setpoint
cargo test -p hares-io hpxml_parity
```

## References

- HPXML Specification v4.x §8.4 "HVAC Plant" — heating and cooling setpoints are user-supplied; HPXML does not constrain the gap.
- ASHRAE Standard 55-2020 *Thermal Environmental Conditions for Human Occupancy* §5.3 — typical residential heating/cooling setpoint gap is 2-3°C.
- Project policy `feedback_no_silent_defaults.md`.

## Related Tickets

- 020-setpoint-chain-visibility
- 006-extract-thermostat-fsm
