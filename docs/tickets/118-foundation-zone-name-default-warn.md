# Foundation Zone Name Silent Default to `attic_vented`

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-io/hpxml/resolve_hvac

## Problem

`crates/hares-io/src/hpxml/resolve_hvac.rs:1203` silently maps an absent foundation zone name to `"attic_vented"`. A foundation is unambiguously not an attic; the substitution produces a mis-classified zone that is then routed through wrong defaults for ground coupling, infiltration, and conditioning state.

## Current Behavior

`crates/hares-io/src/hpxml/resolve_hvac.rs:1203`:
```rust
let zone_name = foundation.name.unwrap_or("attic_vented");  // wrong default
```

A misnamed or absent foundation element silently maps to attic-vented, with no diagnostic.

## Required Behavior

1. If the foundation zone name is absent, emit `tracing::warn!` with the foundation type (slab / basement / crawlspace) and substitute a foundation-appropriate default name (`"basement_unconditioned"`, `"crawlspace_vented"`, `"slab_on_grade"` — whichever matches the FoundationType element).
2. If the foundation type itself is absent, return `HpxmlError::MissingField { field: "Foundation/FoundationType" }` — this is unrecoverable.
3. Never substitute `"attic_vented"` for a foundation.

## Approach

1. Open `crates/hares-io/src/hpxml/resolve_hvac.rs:1203` and replace the silent fallback with foundation-type-aware logic.
2. Add a small helper `default_foundation_zone_name(foundation_type: FoundationType) -> &'static str` that returns the right default name for each HPXML `FoundationType` variant.
3. Add `tracing::warn!` when the default is taken, including the foundation type and the substituted name.
4. Add a unit test for each FoundationType variant.
5. Add a unit test asserting the missing-FoundationType case errors.

## Definition of Done

- [ ] `attic_vented` no longer used as a foundation default
- [ ] Foundation-type-aware default applied via `default_foundation_zone_name`
- [ ] `tracing::warn!` emitted when the default is taken
- [ ] Missing FoundationType returns `HpxmlError::MissingField`
- [ ] Unit tests cover all FoundationType variants
- [ ] No `_ =>` catch-all in the foundation-type match arm

## Verification

```bash
cargo test -p hares-io resolve_hvac foundation
cargo test -p hares-io hpxml_parity
```

## References

- HPXML Specification v4.x §3.1.1 "Foundation" — `FoundationType` enumeration: SlabOnGrade, Basement (Conditioned/Unconditioned), Crawlspace (Vented/Unvented), Ambient.
- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.4 "Foundations" — foundation zones have distinct heat transfer characteristics from attics.
- Project policy `feedback_no_silent_defaults.md`.

## Related Tickets

- 034-kusuda-ground-model-parameter-derivation
- 035-slab-f-factor-method-not-called
- 050-synthetic-weather-ground-temp-equals-outdoor
