# `compressor_type_to_mode` Wildcard Must Warn and Fail Closed

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-io/hpxml/resolve_hvac

## Problem

`crates/hares-io/src/hpxml/resolve_hvac.rs:1783-1785` `compressor_type_to_mode` uses a `_ => "single_speed"` catch-all to map unknown HPXML `CompressorType` values. Project policy explicitly bans `_ =>` catch-alls (see project memory) and the silent default produces wrong speed-mode inference for any new HPXML CompressorType variant added in a future spec revision.

## Current Behavior

`crates/hares-io/src/hpxml/resolve_hvac.rs:1783-1785`:
```rust
match compressor_type {
    "single stage" | "SingleStage" => "single_speed",
    "two stage" | "TwoStage" => "two_speed_setpoint",
    "variable speed" | "VariableSpeed" => "variable_speed_ideal",
    _ => "single_speed",  // wildcard — silent default
}
```

A future HPXML CompressorType (e.g. "DualStage") silently maps to `"single_speed"` with no diagnostic.

## Required Behavior

1. Replace the wildcard with explicit handling: emit `tracing::warn!` listing the unknown compressor type and the equipment ID, then return `Err(HpxmlError::InvalidField { field: "CompressorType", value })`.
2. Match arms must be exhaustive over the known HPXML CompressorType enum values; any unknown value is an error.
3. If a future HPXML revision adds a new variant, the resolver fails closed (errors at parse time) rather than silently miscategorising the equipment.

## Approach

1. Define an `HpxmlCompressorType` enum mirroring the HPXML 4.x specification values.
2. Parse the raw string into the enum; an unparseable string returns `HpxmlError::InvalidField`.
3. Match the enum exhaustively in `compressor_type_to_mode`. Each arm maps to the appropriate HARES `SpeedControlMode`.
4. No `_ =>` arm. If the HPXML enum is extended in a future spec, the compiler will flag the missing match arm.
5. Add a fixture exercising the unknown-string case and assert the error.

## Definition of Done

- [ ] `_ =>` wildcard removed from `compressor_type_to_mode`
- [ ] `HpxmlCompressorType` enum defined with all HPXML 4.x variants
- [ ] String parsing returns `Err(HpxmlError::InvalidField)` for unknown values
- [ ] Match in `compressor_type_to_mode` is exhaustive over the enum
- [ ] Fixture exercises the unknown-string case
- [ ] Unit test asserts error variant and message
- [ ] No other `_ =>` catch-all in `resolve_hvac.rs` (audit complete)

## Verification

```bash
cargo test -p hares-io resolve_hvac compressor_type
cargo test -p hares-io hpxml_parity
rg "_ =>" crates/hares-io/src/hpxml/   # audit
```

## References

- HPXML Specification v4.x §8.4 "Cooling Systems" and "Heat Pumps" — `CompressorType` enumeration: SingleStage, TwoStage, VariableSpeed.
- Project policy: no `_ =>` catch-alls in match arms (per project memory).
- Project policy `feedback_no_silent_defaults.md`.

## Related Tickets

- 007-unify-variable-speed-selection
- 093-seer-silent-zero-fallback-loud-error
- 095-backup-switchover-temp-er-lockout-only
