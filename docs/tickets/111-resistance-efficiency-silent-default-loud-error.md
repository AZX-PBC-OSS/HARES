# `resistance_efficiency_from_params` Silent 1.0 Default

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-io/hpxml/resolve_hvac

## Problem

`crates/hares-io/src/hpxml/resolve_hvac.rs:366-371` `resistance_efficiency_from_params` silently returns `1.0` when efficiency is absent in the params map. Electric resistance heating is conventionally modelled as 100% efficient, but using a silent default obscures whether the input data actually supplied an efficiency value. A misspelled key or absent field produces the same result as a correctly-supplied 1.0.

## Current Behavior

`crates/hares-io/src/hpxml/resolve_hvac.rs:366-371`:
```rust
fn resistance_efficiency_from_params(params: &Params) -> f64 {
    params.get("efficiency").copied().unwrap_or(1.0)
}
```

A typo (`"effciency"`) silently returns 1.0; an HPXML file missing the efficiency element silently returns 1.0; a correctly-supplied 1.0 returns 1.0. The three cases are indistinguishable.

## Required Behavior

Choose one:

A. **Cite the 1.0 default** — keep the default value but add an inline citation (ASHRAE HoF 2021 Ch. 33 "Furnaces" — electric resistance heating is by definition 100% efficient at the appliance) and emit a `tracing::debug!` when the default is taken so the omission is at least logged.

B. **Error loudly** — return `Result<f64, HpxmlError>`; missing efficiency for a resistance unit returns `HpxmlError::MissingField { field: "AnnualHeatingEfficiency", expected: "1.0 for electric resistance" }`.

Recommended path: B for HPXML strictness; the 1.0 value is so universally the right answer that requiring the input file to state it explicitly is a reasonable consistency check. If A is chosen instead, the citation must be inline and the debug log must include the equipment ID.

## Approach

1. Change the function signature to `fn resistance_efficiency_from_params(params: &Params) -> Result<f64, HpxmlError>`.
2. Return `Err(HpxmlError::MissingField { ... })` when the key is absent.
3. If a non-1.0 value is supplied, validate `0 < value <= 1.0` and return error otherwise.
4. Update callsites to propagate the error.
5. Update fixtures: ensure all electric resistance heaters in test HPXML files supply an explicit efficiency.

## Definition of Done

- [ ] `resistance_efficiency_from_params` returns `Result<f64, HpxmlError>`
- [ ] Missing key returns `HpxmlError::MissingField`
- [ ] Out-of-range value returns `HpxmlError::InvalidField`
- [ ] Callsites propagate the error
- [ ] HPXML fixtures supply explicit efficiency
- [ ] Inline comment cites ASHRAE HoF 2021 Ch. 33 for the conventional 1.0 value

## Verification

```bash
cargo test -p hares-io resolve_hvac resistance
cargo test -p hares-io hpxml_parity
```

## References

- ASHRAE Handbook of Fundamentals 2021 Ch. 33 "Furnaces" — electric resistance heating is by definition 100% efficient at the appliance (all electrical input becomes heat).
- HPXML Specification v4.x §8.4 "Heating Systems" — `AnnualHeatingEfficiency` element with `Units` and `Value` children; required for electric resistance.
- Project policy `feedback_no_silent_defaults.md`.

## Related Tickets

- 015-er-on-off-modeling
- 077-hpxml-backup-efficiency-units-ignored
