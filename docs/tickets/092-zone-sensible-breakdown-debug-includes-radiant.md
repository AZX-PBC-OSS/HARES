# `zone_sensible_breakdown_debug` Must Apply Radiant Port Inputs

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-envelope/thermal_solver

## Problem

`zone_sensible_breakdown_debug()` at `crates/hares-envelope/src/thermal_solver/mod.rs:246` calls `apply_port_sensible_inputs` but does NOT call `apply_port_radiant_inputs`. The breakdown is therefore the convective-only contribution and excludes the convective residual that the radiant path adds to the zone air node after Y-Δ StarMesh distribution.

For a typical 30% radiant fraction on internal gains, the debug breakdown understates zone-air contribution by ~21% (radiant fraction × (1 − fraction routed to surfaces)). This breakdown is the data source used by `tests/bestest/mod.rs:478` for BESTEST 900FF analysis — the BESTEST diagnostic itself is computed off a biased value.

## Current Behavior

`crates/hares-envelope/src/thermal_solver/mod.rs:246` (current):
```rust
pub fn zone_sensible_breakdown_debug(...) -> ZoneSensibleBreakdown {
    apply_port_sensible_inputs(...);
    // apply_port_radiant_inputs NOT called
    ...
}
```

Result: the per-source contribution to the zone air node sums to less than the true total. The BESTEST 900FF analysis at `tests/bestest/mod.rs:478` reports a biased convective/radiant split.

## Required Behavior

`zone_sensible_breakdown_debug` must apply both the sensible and radiant port paths, using the same call sequence as `prepare_inputs_inner` and `integrate_inner`. The reported zone-air contribution must include the convective residual that emerges from the radiant distribution after StarMesh routing.

The breakdown must separately attribute:
- Direct convective gain (from `apply_port_sensible_inputs`)
- Radiant-routed convective residual (from the air-node fraction of `apply_port_radiant_inputs`)
- Radiant gain reaching surfaces (sum of per-surface radiant injections)

so that the BESTEST diagnostic can verify both the total and the split.

## Approach

1. After the existing `apply_port_sensible_inputs` call in `zone_sensible_breakdown_debug`, add `apply_port_radiant_inputs` with the same arguments used by the production path.
2. Track the air-node-bound vs surface-bound radiant components separately so the breakdown can attribute them.
3. Update `ZoneSensibleBreakdown` to expose:
   - `convective_direct_w` (existing)
   - `radiant_to_air_residual_w` (new)
   - `radiant_to_surfaces_w` (new)
4. Update the BESTEST 900FF diagnostic at `tests/bestest/mod.rs:478` to consume the new fields and assert against EnergyPlus 900FF reference convective/radiant split.

## Definition of Done

- [ ] `zone_sensible_breakdown_debug` calls both `apply_port_sensible_inputs` and `apply_port_radiant_inputs`
- [ ] `ZoneSensibleBreakdown` exposes convective direct, radiant-to-air residual, and radiant-to-surfaces components
- [ ] BESTEST 900FF diagnostic verified against EnergyPlus reference split
- [ ] Sum of breakdown fields equals total zone-air gain to within 1e-9 W (energy balance)
- [ ] Test: 30% radiant fraction on a 1000 W gain produces breakdown with ~700 W convective direct + radiant routing matching the StarMesh weights

## Verification

```bash
cargo test -p hares-envelope thermal_solver
cargo test --test bestest 900ff
```

## References

- EnergyPlus Engineering Reference §3.5 "Inside Surface Heat Balance" and §3.6 "Zone Air Heat Balance" — radiant gains route partly to surface energy balance and partly back to the zone air node via convection.
- BESTEST 900FF (free-floating, no HVAC) procedure: convective and radiant gain components must be tracked separately for the diagnostic comparison against the EnergyPlus reference.
- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.2.2 "Convective and Radiant Components".

## Related Tickets

- 091-port-radiant-inputs-all-zones (companion fix — radiant must iterate all zones)
- 089-radiation-frac-starmesh-rederivation (StarMesh derivation underlying the routing)
- 094-bestest-tests-still-ignored (BESTEST gate)
