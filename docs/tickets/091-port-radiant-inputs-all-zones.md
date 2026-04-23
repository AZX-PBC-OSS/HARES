# `apply_port_radiant_inputs` Must Iterate All Zones, Not Only Indoor

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-envelope/thermal_solver/ports

## Problem

`apply_port_radiant_inputs` at `crates/hares-envelope/src/thermal_solver/ports.rs:36-43` filters its iteration to `indoor_zone_id` only. The companion `apply_port_sensible_inputs` correctly iterates over all zones in `zone_state_indices`. This asymmetry silently drops radiant gains from any equipment placed in a non-indoor zone (basement, garage, attic, crawlspace) — those gains exist in the sensible path but vanish in the radiant path.

Equipment that emits a radiant fraction (radiant heaters in a basement workshop, lighting in a garage, an electric resistance heater in a conditioned crawlspace) will route its convective fraction to the host zone correctly via the sensible path but will silently fail to route its radiant fraction anywhere. Energy is not conserved in the per-zone breakdown, and the radiant component vanishes from the surface energy balance.

## Current Behavior

`crates/hares-envelope/src/thermal_solver/ports.rs:36-43` (current):
```rust
pub fn apply_port_radiant_inputs(
    ...
    indoor_zone_id: ZoneId,
    ...
) {
    let Some(&indoor_idx) = zone_state_indices.get(&indoor_zone_id) else { return; };
    // ... only processes indoor_zone_id ...
}
```

By contrast, `apply_port_sensible_inputs` iterates all entries in `zone_state_indices`. Equipment in non-indoor zones writes to its host zone's port, but that port is never visited by the radiant distribution path.

## Required Behavior

`apply_port_radiant_inputs` must iterate over all zones present in `zone_state_indices`, identical in shape to `apply_port_sensible_inputs`. For each zone:

1. Look up the per-zone port radiant value
2. Distribute it across the interior surfaces of that zone using the same Y-Δ StarMesh weighting used for the indoor zone today
3. Inject the convective residual to the zone air node

The architectural rule: every port-emitting equipment in any zone must have both its sensible (convective) and radiant fractions delivered to the correct zone, not silently dropped because the zone is not "indoor".

## Approach

1. Change `apply_port_radiant_inputs` signature to remove the `indoor_zone_id` filter. Iterate `zone_state_indices` directly.
2. For each `(zone_id, zone_idx)` pair, perform the same surface-set lookup, weight computation (via the pre-allocated `radiant_weights_buf` from ticket 040), and distribution loop currently performed only for the indoor zone.
3. Update all callsites in `crates/hares-envelope/src/thermal_solver/mod.rs` to drop the `indoor_zone_id` argument.
4. Add a regression test placing a radiant-emitting load in a non-indoor zone and asserting the radiant gain reaches the surface energy balance and the convective residual reaches the zone air node.

## Definition of Done

- [ ] `apply_port_radiant_inputs` iterates all zones in `zone_state_indices`
- [ ] No `indoor_zone_id` filter remains in the radiant-port path
- [ ] All callsites updated
- [ ] Test: equipment with `radiant_fraction = 0.4` in a basement zone delivers radiant gain to basement surfaces and convective residual to basement air node
- [ ] Test: indoor-zone behaviour unchanged (regression guard)
- [ ] `cargo test -p hares-envelope` passes

## Verification

```bash
cargo test -p hares-envelope ports
cargo test -p hares-envelope --test thermal_solver
cargo test -p hares-core dwelling
```

## References

- EnergyPlus Engineering Reference §3.5 "Inside Surface Heat Balance" — every zone's surface energy balance receives all radiant gains emitted within that zone.
- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.2 "Internal Heat Gains" — sensible internal gains decompose into convective and radiant fractions; both fractions must be applied to the zone in which the equipment resides.
- HARES `crates/hares-envelope/src/thermal_solver/ports.rs` — current radiant/sensible distribution.

## Related Tickets

- 040-radiant-gain-weights-hot-alloc (pre-allocated buffer this fix uses)
- 045-equipment-ports-applied-before-zone-state-update (port timing concerns)
- 070-hvac-thermal-port-missing-space-fraction (related port-routing energy-balance fix)
- 092-zone-sensible-breakdown-debug-includes-radiant
