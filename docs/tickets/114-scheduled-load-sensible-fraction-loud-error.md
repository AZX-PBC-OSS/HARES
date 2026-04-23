# `ScheduledLoad.sensible_gain_fraction` Silent 0.5 Default

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-equipment/scheduled_load

## Problem

`crates/hares-equipment/src/scheduled_load.rs:262-267` `sensible_gain_fraction` falls back to `0.5` when the field is absent in the config, with only a `tracing::warn!`. The 50/50 sensible/latent split is an arbitrary pick — the actual fraction depends on the load type (lighting is ~100% sensible, dishwasher exhaust is ~30% sensible, etc.). Substituting 0.5 silently for any load type biases the latent/sensible balance and downstream HVAC sizing.

## Current Behavior

`crates/hares-equipment/src/scheduled_load.rs:262-267`:
```rust
let sensible_fraction = config.sensible_gain_fraction.unwrap_or_else(|| {
    tracing::warn!("sensible_gain_fraction not specified; defaulting to 0.5");
    0.5
});
```

Warning logs, simulation continues with a wrong split.

## Required Behavior

1. `sensible_gain_fraction` must be required at the config level. Return `Err(EquipmentError::MissingField { field: "sensible_gain_fraction" })` when absent.
2. Validate `0.0 <= value <= 1.0`; return `Err` otherwise.
3. The accompanying `latent_gain_fraction` (if present) must satisfy `sensible + latent <= 1.0`; the residual is convective-only loss to surroundings.
4. Update all `ScheduledLoad` fixtures to supply an explicit value with a citation comment.

## Approach

1. Open `crates/hares-equipment/src/scheduled_load.rs:262-267` and replace the fallback with an explicit error.
2. Add validation for the 0–1 range.
3. Audit all `ScheduledLoad` configs in fixtures and test code; add explicit `sensible_gain_fraction` values with inline citations to the source for the load type (e.g. ASHRAE HoF 2021 Ch. 18 Table 5 for residential appliance heat gains).
4. Add a unit test asserting the error fires when the field is absent.
5. Add a unit test asserting the validation fires for out-of-range values.

## Definition of Done

- [ ] Silent 0.5 default removed from `scheduled_load.rs:262-267`
- [ ] Missing field returns `EquipmentError::MissingField`
- [ ] Out-of-range value returns `EquipmentError::InvalidField`
- [ ] All fixtures supply explicit `sensible_gain_fraction` with inline source citations
- [ ] Unit tests cover missing field and out-of-range value cases
- [ ] No `ScheduledLoad` callsite produces an implicit 0.5 split

## Verification

```bash
cargo test -p hares-equipment scheduled_load
cargo test -p hares-core dwelling
```

## References

- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.2 "Internal Heat Gains" Tables 1-5 — sensible and latent fractions for residential appliances and lighting.
- EnergyPlus Engineering Reference §3.6.3 "ZoneHVAC:EquipmentList" — sensible and latent fractions are explicit inputs per equipment.
- Project policy `feedback_no_silent_defaults.md`.

## Related Tickets

- feedback_equipment_self_contained
- 001-unify-hfg-add-humidity-port (related humidity port handling)
