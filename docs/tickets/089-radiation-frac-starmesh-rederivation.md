# Re-derive `radiation_frac` Voltage-Divider Under StarMesh Topology

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-core/dwelling, hares-envelope

## Problem

`radiation_frac` at `crates/hares-core/src/dwelling/solver_builder.rs:248-254` is computed as a voltage-divider expression that was originally derived against a fully-resolved series film+wall network (combined exterior + interior film resistance in series with the wall capacitor branch). After the S1 fix lowered the interior film coefficient to convection-only and the longwave radiation network was promoted to a separate explicit linearised h_rad branch, the StarMesh Y-Δ topology that emerges in the assembled state-space model no longer matches the network the divider was derived against.

The empirical bias measured in the Review 02 report is approximately 27%: solar plus radiant gains are misrouted between the zone-air node and the interior thermal-mass nodes. Energy is conserved (the total injected gain equals the algebraic sum of the routed components), but the spatial distribution between the air node and the surface mass nodes is wrong. Downstream effects:

- Zone air temperature swings are biased (under- or over-shoot relative to first-principles solution)
- Surface temperatures used for MRT and longwave fallback are wrong
- HVAC sizing and runtime computed off the biased zone temperatures inherit the bias

## Current Behavior

`crates/hares-core/src/dwelling/solver_builder.rs:248-254`:

```rust
let radiation_frac = R_film_combined / (R_film_combined + R_wall_to_air);
```

This expression assumes a single lumped film resistance in series between the inner surface and the zone air node — the OCHRE topology before the S1/S5 fixes. After S1 separated convection (R_film_conv) from radiation (1/h_rad) into parallel paths, and after the longwave network was reformulated as a StarMesh Y-Δ transformation among interior surfaces and the zone air node, the correct splitting fraction is no longer the simple voltage divider above.

## Required Behavior

Re-derive `radiation_frac` from first principles under the StarMesh Y-Δ network now used by the interior longwave model. The derivation must:

1. Identify the actual conductances seen by an injected radiant flux at an interior surface node — namely:
   - The convective branch from the surface to zone air via R_film_conv
   - The radiant star-network branch to all other interior surfaces via h_rad linearisations
   - The conductive branch into the wall RC mass node
2. Express the radiant fraction reaching the zone air as a current-divider over the combined Y-Δ network admittance to air, not over the pre-S1 series resistance pair.
3. Match the value the assembled state-space matrix actually produces when a unit radiant heat flux is injected at the inner surface node — verify by impulse test against the dense state-space model.

## Approach

1. Derive the closed-form current-divider expression for the StarMesh interior LWR network. The reference network is documented in `crates/hares-envelope/src/thermal_solver/longwave.rs` and `crates/hares-envelope/src/rc_network.rs`. The Y-Δ transformation maps the radiosity star at the mean radiant temperature node to pairwise surface-to-surface conductances; the splitting fraction at any one surface is its own admittance to air divided by the sum of all admittances at the surface node (to air, to mass, and to all other surfaces).
2. Implement the new `radiation_frac` computation as a function over the per-surface conductance set. Take the convection conductance, the linearised radiation conductance, and the wall-mass conductance as inputs; return the fraction routed to the zone air node.
3. Add an impulse-response test in `crates/hares-envelope/tests/` that injects 1 W at an interior surface and asserts the steady-state air-node temperature rise matches the closed-form `radiation_frac × ...` expression to within 1e-6.
4. Validate on a single-zone test fixture with one surface and known geometry that the rederived value matches a first-principles hand calculation.

## Definition of Done

- [ ] `radiation_frac` re-derived from StarMesh Y-Δ topology and documented inline with a reference to the derivation
- [ ] Inputs to the new computation are convection conductance, radiation conductance (linearised h_rad), and wall-mass conductance — not a single "combined R_film"
- [ ] Impulse-response test verifies the new fraction matches assembled state-space behaviour to 1e-6
- [ ] Hand-derivation test for a single-surface single-zone fixture passes
- [ ] BESTEST 900FF radiant/convective split in zone-air gain breakdown matches EnergyPlus reference within 5%
- [ ] No `radiation_frac` callsite uses the old `R_film_combined / (R_film_combined + R_wall_to_air)` formula

## Verification

```bash
cargo test -p hares-envelope radiation_frac
cargo test -p hares-envelope --test thermal_solver
cargo test -p hares-core dwelling
```

Compare BESTEST 900FF zone-air gain breakdown against EnergyPlus reference: zone-air convective gain fraction must be within 5% of E+.

## References

- EnergyPlus Engineering Reference §3.5 "Inside Surface Heat Balance" — surface-to-air convection and surface-to-surface radiation are separate conductances; routing of an injected gain depends on the relative admittances.
- ASHRAE Handbook of Fundamentals 2021 Ch. 4 §4.3 "Heat Transfer at Surfaces" — convective and radiative components combine in parallel from a surface node, not in series.
- Y-Δ (Star-Mesh) network transformation: any electrical-network text; see Bondy-Murty *Graph Theory* or EnergyPlus Engineering Reference §3.5.10 "Network Solution".
- HARES `crates/hares-envelope/src/thermal_solver/longwave.rs` — current StarMesh implementation.

## Related Tickets

- 044-lwr-fallback-linearised-not-scriptf (LWR linearisation network this divider must match)
- 047-interior-lwr-uses-last-step-zone-temp (related interior LWR consistency issue)
- 094-bestest-tests-still-ignored (BESTEST is the validation gate for this fix)
