# Dehumidifier Written as InternalGain Instead of HVAC Category

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-equipment, hares-types
**Dependency**: Ticket 074 (HPWH category mismatch) depends on this ticket's `HvacDehumidification` variant. Ticket 072 (latent-by-category breakdown) benefits from this ticket landing first so the new category is immediately trackable in the per-category latent array.

## Problem

The dehumidifier writes its thermal port contributions under `ThermalCategory::InternalGain` at `dehumidifier.rs:338`. A standalone dehumidifier is intentional mechanical conditioning equipment; attributing its sensible heat addition and latent moisture removal to `InternalGain` conflates mechanical output with passive gains (lighting, plug loads, occupants) in per-category diagnostics and energy balance reports.

No `HvacDehumidification` (or equivalent) category exists in the `ThermalCategory` enum, so there is no way to separate dehumidifier contributions from passive internal gains in `sensible_by_category` breakdowns.

`sensible_gain_w` in the dehumidifier port at line 186 equals `latent_removal_w + electric_power_w` — the sum of condensation heat released as sensible heat plus motor dissipation. Both are real sensible gains to the zone but not passive gains.

EnergyPlus Engineering Reference, Zone Air Heat Balance §3.1: internal gains (people, lights, equipment) are explicitly a separate term from HVAC equipment contributions in the zone heat balance. ASHRAE Handbook of Fundamentals 2021 Ch. 18 Table 1: internal gains defined as occupants, lighting, and plug loads; conditioning equipment is not included.

## Current Behavior

`hares-equipment/src/hvac/dehumidifier.rs:332–339`:

```rust
ports.accumulate(&PortContribution::Thermal {
    zone: self.zone_id,
    sensible_gain_w: snapshot.sensible_gain_w,
    radiant_gain_w: 0.0,
    latent_gain_w: -snapshot.latent_removal_w,
    category: ThermalCategory::InternalGain,   // wrong
})?;
```

`hares-types/src/ports.rs:17–47`: `ThermalCategory` has five variants — `HvacHeating`, `HvacCooling`, `InternalGain`, `JacketLoss`, `DuctLoss`. `THERMAL_CATEGORY_COUNT == 5`. No dehumidification variant exists.

## Required Behavior

1. Add `ThermalCategory::HvacDehumidification` to the `ThermalCategory` enum in `hares-types/src/ports.rs` with `index()` value 5.
2. Update `THERMAL_CATEGORY_COUNT` from 5 to 6.
3. Update the `index()` match arm for the new variant.
4. In `dehumidifier.rs:338`, change `category: ThermalCategory::InternalGain` to `category: ThermalCategory::HvacDehumidification`.
5. In thermal solver diagnostics (`thermal_solver/mod.rs`), add a read of `sensible_for_category(ThermalCategory::HvacDehumidification)` for reporting.
6. Update all tests that reference `THERMAL_CATEGORY_COUNT` or iterate over the full variant set.

Reference: EnergyPlus Engineering Reference, Zone Air Heat Balance §3.1; OCHRE `dehumidifier.py` — end-use "Dehumidifier" tracked separately from "Internal Gains".

## Approach

1. Add `HvacDehumidification = 5` to the enum; update `index()` with a new match arm; set `THERMAL_CATEGORY_COUNT = 6`.
2. Update `ThermalAccumulator` array sizes — they are `[f64; THERMAL_CATEGORY_COUNT]` so they resize automatically if the constant is updated correctly.
3. Run `cargo test -p hares-types` and `cargo test -p hares-equipment` to catch any tests depending on the count or full-variant exhaustiveness.

## Definition of Done

- [ ] `ThermalCategory::HvacDehumidification` variant exists with `index() == 5`
- [ ] `THERMAL_CATEGORY_COUNT == 6`
- [ ] `dehumidifier.rs:338` writes under `ThermalCategory::HvacDehumidification`
- [ ] Thermal solver diagnostics report the dehumidification category
- [ ] All existing tests pass (array size is derived from the constant; no manual array size literals broken)
- [ ] New test: dehumidifier step writes to `HvacDehumidification` category, `InternalGain` bucket is zero for that step

## Verification

```bash
cargo test -p hares-types
cargo test -p hares-equipment
cargo test -p hares-envelope
```

## References

- EnergyPlus Engineering Reference, Zone Air Heat Balance §3.1 — HVAC equipment is a separate term from internal gains
- ASHRAE Handbook of Fundamentals 2021 Ch. 18 Table 1 — internal gains: occupants, lighting, plug loads; conditioning equipment excluded
- OCHRE `dehumidifier.py` — end-use "Dehumidifier" separate from "Internal Gains"
- `hares-types/src/ports.rs:17–47` — `ThermalCategory` enum and `THERMAL_CATEGORY_COUNT`
