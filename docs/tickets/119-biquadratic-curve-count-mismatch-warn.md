# Mismatched Biquadratic Cap/EIR Curve Counts Should Warn, Not Silent-Fill

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-equipment/hvac/hvac_core

## Problem

`crates/hares-equipment/src/hvac/hvac_core.rs:427-434` silently fills mismatched biquadratic capacity vs EIR curve counts with unity polynomials. If a multi-speed unit has, say, 4 capacity curves and 3 EIR curves, the resolver pads the missing EIR curve with `[1, 0, 0, 0, 0, 0]` (the identity biquadratic). The unit operates as if EIR is constant 1.0 at one speed step — physically impossible for any real heat pump or AC.

## Current Behavior

`crates/hares-equipment/src/hvac/hvac_core.rs:427-434`:
```rust
while eir_curves.len() < cap_curves.len() {
    eir_curves.push(IDENTITY_BIQUADRATIC);  // silent fill
}
```

No warning. The resulting unit silently has wrong EIR for one or more speed steps.

## Required Behavior

1. Emit `tracing::warn!` listing the equipment ID, the count mismatch, and the speed step(s) being filled with identity curves.
2. Optionally elevate to `tracing::error!` if the user has opted into strict mode.
3. Document the silent-fill behaviour in the function doc comment so it is discoverable.
4. If the typed config exposes a strict-mode flag, default to error rather than warn for new users.

## Approach

1. Open `crates/hares-equipment/src/hvac/hvac_core.rs:427-434` and add `tracing::warn!` immediately before the fill loop.
2. Include the equipment ID, the cap/EIR count mismatch, and the specific speed steps being filled.
3. Add a doc comment on the surrounding function describing the silent-fill behaviour.
4. Add a unit test asserting the warning fires when curve counts mismatch.
5. Consider whether to elevate to error — discuss in the ticket with the team.

## Definition of Done

- [ ] `tracing::warn!` emitted on mismatched curve counts
- [ ] Log includes equipment ID, count mismatch, and speed step indices
- [ ] Function doc comment describes the silent-fill behaviour
- [ ] Unit test asserts the warning fires
- [ ] No `_ =>` catch-all elsewhere in the curve-loading code

## Verification

```bash
cargo test -p hares-equipment hvac_core
```

## References

- AHRI Standard 210/240-2023 — multi-speed heat pumps have one capacity curve and one EIR curve per speed step; counts must match.
- EnergyPlus I/O Reference `Coil:Cooling:DX:MultiSpeed` — distinct capacity and EIR curves per stage; identity is invalid for real equipment.
- Project policy `feedback_no_silent_defaults.md`.

## Related Tickets

- 002-ideal-hvac-biquadratic-fallback
- 003-tighten-biquadratic-default-bounds
- 010-default-biquadratic-performance-curves
