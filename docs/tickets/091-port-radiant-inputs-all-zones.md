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

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match — `apply_port_radiant_inputs` starts at line 34 (ticket says 36–43; actual range is lines 34–76 after prior edits, but the function body is the same as described)
- [x] Described logic matches current implementation — confirmed: lines 35–41 of `ports.rs` collect `total_radiant_w` by filtering `ports.thermal` to `t.zone == indoor_zone` only; lines 50–75 then look up surface config keyed on `indoor_zone` only. Equipment in any other zone contributes nothing.
- [x] OCHRE cross-check: **N/A (architecture diverges intentionally)** — OCHRE (`Models/Envelope.py`) uses a different design: each `Zone` object holds its own `surfaces` list and radiation is computed per-zone in a loop (`for zone in self.zones.values(): zone.calculate_interior_radiation(zone.temperature)` at line 1188). OCHRE never has a single-zone filter bug because its per-zone structure eliminates it by construction. HARES explicitly port-dispatches equipment gains through `apply_port_radiant_inputs`, which is the new codepath that introduced the bug; there is no corresponding OCHRE function to diverge from.
- [x] EnergyPlus cross-check: **confirms the bug is a violation of E+ semantics** — EnergyPlus Engineering Reference ("Zone Internal Gains", versions 8.3–25.2, bigladdersoftware.com) states: *"Long wavelength radiation from all internal sources, such as people, lights and equipment, is combined and then distributed over surfaces"* and *"the radiation is distributed in proportion to the area×absorptance product of each surface"*. The Inside Heat Balance reference ("Inside Heat Balance", v9.6) adds: *"The radiative part is then distributed over the surfaces within the zone in some prescribed manner."* Both sources confirm that the distribution is **per-zone** — gains originate in the zone where the source is located and are distributed to surfaces of *that* zone. The current HARES code violates this by forcing all radiant distribution through the `indoor_zone_id` regardless of which zone the equipment occupies.

### Web-Verified Citations

**Citation 1**: EnergyPlus Engineering Reference §3.5 "Inside Surface Heat Balance" — every zone's surface energy balance receives all radiant gains emitted within that zone.

- **Source found**: https://bigladdersoftware.com/epx/docs/9-6/engineering-reference/inside-heat-balance.html (also verified at v8.3 and v25.2)
- **Quoted passage**: *"The radiative part is then distributed over the surfaces within the zone in some prescribed manner."* and *"q″LWS = Longwave radiation flux from equipment in a zone or group of zones (enclosure)"* — both passages confirm zone-scoped distribution.
- **Verdict**: **Confirmed** — the physical principle is correct. However the ticket's section reference "§3.5" is not a numbered section in the EnergyPlus Engineering Reference (it uses unnumbered chapter headings). The correct citation is the "Inside Heat Balance" chapter (Chapter 6 in v25.2). The physical claim stands; the section number is approximate/informal but not wrong in intent.

**Citation 2**: ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.2 "Internal Heat Gains" — sensible internal gains decompose into convective and radiant fractions; both fractions must be applied to the zone in which the equipment resides.

- **Source found**: https://handbook.ashrae.org/Handbooks/F17/IP/f17_ch18/f17_ch18_ip.aspx (F17 proxy — 2021 structure is the same chapter; also confirmed via ASHRAE ToC at ashrae.org)
- **Quoted passage**: Chapter 18 is titled *"Nonresidential Cooling and Heating Load Calculations"*; Section 2 is titled *"INTERNAL HEAT GAINS"* with subsections 2.1 People, 2.2 Lighting, 2.3 Electric Motors, 2.4 Appliances. The section specifies convective/radiant splits (e.g., computers: ~90% convective / 10% radiative) and states that both components must be accounted for. The document treats the split as a property of the equipment and the zone where it resides.
- **Verdict**: **Partially correct** — the ticket cites "§18.2" but Section 2 of the chapter is simply titled "Internal Heat Gains" (not "§18.2"). The physical claim (convective and radiant fractions both belong to the resident zone) is substantively correct. The section numbering is informal; the underlying principle is well-supported.

### Legitimacy

- **Verdict**: **Legitimate**
- **Rationale**: The bug is unambiguously present in the current code. `apply_port_radiant_inputs` (`crates/hares-envelope/src/thermal_solver/ports.rs` lines 34–76) hard-codes `indoor_zone_id` in two places: (1) the summation of `total_radiant_w` only sums gains whose `t.zone == indoor_zone` (line 39), and (2) the surface-config lookup is scoped to `interior_lwr_zones` and `interior_solar_zones` entries matching `indoor_zone` (lines 50–65). Any equipment whose `zone` is not the configured `indoor_zone_id` has its radiant fraction silently discarded. The companion function `apply_port_sensible_inputs` (lines 14–22) correctly iterates all zones via `zone_sensible_input_indices`, confirming the asymmetry is unintentional. EnergyPlus Engineering Reference and ASHRAE HoF Ch. 18 both confirm that both convective and radiant fractions must be delivered to the zone of origin. OCHRE avoids the issue by design. The regression test written for this audit (`radiant_port_in_non_indoor_zone_reaches_zone_surface` in `crates/hares-envelope/tests/multi_zone_coupling.rs`) confirms the defect: `u[2]` (the ZONE2 surface heat-gain input) remains 0.0 after a step with 40 W radiant gain from ZONE2 equipment.

### Proposed Fix Summary

1. Remove the `indoor_zone` variable from `apply_port_radiant_inputs`.
2. Change the radiant summation to iterate all entries in `ports.thermal`, keyed by `t.zone`, grouping into per-zone totals (e.g., a small stack-allocated map or a second pass over the `thermal` vec).
3. For each zone with non-zero total radiant gain, perform the existing LWR / solar surface lookup scoped to that zone's `zone_id` in `interior_lwr_zones` / `interior_solar_zones`.
4. The convective residual for each zone should go to `zone_sensible_input_indices[zone]`, not always `indoor_zone`.
5. The `build_input_vector` diagnostic snapshot in `zone_sensible_breakdown_debug` should be updated similarly.
6. No callsite signature change is required because `apply_port_radiant_inputs` already takes `&PortSlots` — the `indoor_zone_id` is accessed via `self.config`, so no argument removal is needed, just internal loop restructuring.

### Test Written

- **File**: `crates/hares-envelope/tests/multi_zone_coupling.rs` — function `radiant_port_in_non_indoor_zone_reaches_zone_surface`
- **What it tests**: Places a `ThermalAccumulator` with 40 W `radiant_gain_w` in ZONE2 (non-indoor), configures an `InteriorLwrZoneConfig` for ZONE2 with one opaque surface at input index 2, runs one solver step, and asserts that `last_u[2]` ≈ 40 W. With the current bug `last_u[2] == 0.0` and the test panics. Also asserts `last_u[3]` (ZONE2 air convective fallback) is non-zero as a secondary check that the sensible path is unaffected.
- **Status**: **Failing** (confirmed with `cargo test -p hares-envelope --test multi_zone_coupling radiant_port_in_non_indoor_zone_reaches_zone_surface`)
