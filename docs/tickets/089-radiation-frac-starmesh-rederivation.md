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

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation
- [x] Referenced line numbers still match (ticket cites 248-254; actual code at lines 251-255 — shifted by 3 due to added comment lines since ticket was written, but same formula)
- [x] Described logic matches current implementation: `interior_rad_frac = r_film_int / (r_film_int + r_inner_half)` at `solver_builder.rs:251-255`
- [x] OCHRE cross-check result: **DIVERGES from ticket's claim** — HARES actually *matches* OCHRE's formula for the `run_internal_rad = True` ("full") mode; the divergence is that OCHRE's `radiation_frac` in full mode also uses `res_film / (res_film + res_material)` (Envelope.py:254), and the comment in HARES at line 248 explicitly says "OCHRE 'full' mode". The real issue is that *neither* OCHRE nor HARES derives this fraction from the Y-Δ admittance of the assembled StarMesh network — the formula is a pre-topology-change carry-over.
- [x] EnergyPlus cross-check result: **No direct contradiction, but EnergyPlus does not use this type of splitting fraction at all** — EnergyPlus uses a full heat balance where convection and radiation are parallel independent paths computed at each timestep (Inside Heat Balance: `q″conv = hc(Ts − Ta)` and `q″LWX = ScriptF T⁴` exchange, never a pre-computed splitting scalar). The `radiation_frac` concept is OCHRE-specific and has no direct E+ equivalent. EnergyPlus Engineering Reference (bigladdersoftware.com, v24.1, "Inside Heat Balance") states: *"The model, which considers room air to be completely transparent, is reasonable physically because of the low water vapor concentrations and the short mean path lengths. It also permits separating the radiant and convective parts of the heat transfer at the surface."* The separation is handled by explicit per-timestep heat balance, not a fixed routing scalar.

### Web-Verified Citations

**Citation 1**
- **Citation**: "EnergyPlus Engineering Reference §3.5 'Inside Surface Heat Balance' — surface-to-air convection and surface-to-surface radiation are separate conductances; routing of an injected gain depends on the relative admittances."
- **Source found**: https://bigladdersoftware.com/epx/docs/24-1/engineering-reference/inside-heat-balance.html
- **Quoted passage**: *"q′′LWX + q′′SW + q′′LWS + q′′ki + q′′sol + q′′conv = 0"*; *"q′′conv = hc(Ts − Ta)"*; the document confirms convection and radiation are separate terms in the heat balance. However, **no section numbered §3.5 or §3.5.10 exists** in the current EnergyPlus Engineering Reference. The "Inside Heat Balance" is found under "Surface Heat Balance Manager / Processes" with no §3.5 numbering. The statement about "routing depends on relative admittances" is the ticket's interpretation, not a quoted EnergyPlus passage — the reference does not contain that phrase.
- **Verdict**: **Partially correct** — the separation of convective and radiative heat transfer is confirmed, but the section number §3.5 is wrong (no such section exists), and the phrase about "relative admittances" is not from the cited source.

**Citation 2**
- **Citation**: "ASHRAE Handbook of Fundamentals 2021 Ch. 4 §4.3 'Heat Transfer at Surfaces' — convective and radiative components combine in parallel from a surface node, not in series."
- **Source found**: https://handbook.ashrae.org/Handbooks/F21/IP/F21_Ch04/F21_Ch04_ip.aspx
- **Quoted passage**: *"R3 is the parallel combination of the convection and radiation resistances on the right-hand surface, 1/hc A and 1/hr A. Equivalently, R3 = 1/hrc A, where hrc on the air side is the sum of the convection and radiation heat transfer coefficients (i.e., hrc = hc + hr)."* The chapter confirms that convection and radiation act in parallel at a surface. However, **no section specifically labelled §4.3 'Heat Transfer at Surfaces' was found**; the combined-resistances content appears within the "Overall Resistance and Heat Transfer Coefficient" subsection of Chapter 4 "Heat Transfer." The section reference is slightly imprecise but the underlying physics claim is confirmed.
- **Verdict**: **Partially correct** — the parallel convection+radiation physics is confirmed by ASHRAE HoF, but the section number §4.3 is imprecise and the phrase "Heat Transfer at Surfaces" does not appear as a section title in the fetched content.

**Citation 3**
- **Citation**: "Y-Δ (Star-Mesh) network transformation: any electrical-network text; see Bondy-Murty *Graph Theory* or EnergyPlus Engineering Reference §3.5.10 'Network Solution'."
- **Source found**: OCHRE `vendors/OCHRE/ochre/Models/RCModel.py:16` and `vendors/OCHRE/ochre/Models/Envelope.py:1053`
- **Quoted passage** (OCHRE RCModel.py:16): *"# use star-mesh transform to remove floating node, see https://en.wikipedia.org/wiki/Star-mesh_transform"*. OCHRE Envelope.py:1053: *"# Note: node <label>-rad is removed from envelope model using star-mesh transform"*. EnergyPlus Engineering Reference §3.5.10 "Network Solution" **does not exist** in any version found (v8.0 through v24.1 were searched). The star-mesh transform for building energy modelling is documented in OCHRE's own code comments, not in EnergyPlus.
- **Verdict**: **Incorrect citation of EnergyPlus §3.5.10** — no such section exists. The star-mesh transform is real and used in OCHRE, but the E+ reference is fabricated or mistaken.

**Citation 4**
- **Citation**: "HARES `crates/hares-envelope/src/thermal_solver/longwave.rs` — current StarMesh implementation."
- **Source found**: `crates/hares-envelope/src/thermal_solver/longwave.rs` (read directly)
- **Quoted passage**: The file implements the exterior and interior longwave solvers. For interior surfaces, at line 266: `let t_surf = s.radiation_frac * t_node + (1.0 - s.radiation_frac) * t_zone_c;` — this is the voltage-divider initialization of surface temperature. The file does **not** contain a StarMesh Y-Δ transform. The actual Y-Δ transform lives in `crates/hares-envelope/src/rc_network.rs` via `reduce_floating_nodes()`.
- **Verdict**: **Partially correct** — the longwave.rs file does use `radiation_frac` in the interior LWR temperature initialization, but the StarMesh Y-Δ implementation is in `rc_network.rs`, not `longwave.rs`.

### Legitimacy
- **Verdict**: **Partially Legitimate**
- **Rationale**: The core claim is real: `interior_rad_frac = r_film_int / (r_film_int + r_inner_half)` is a series voltage-divider formula that does not account for the parallel admittance structure of the assembled StarMesh network. The ticket correctly identifies that after S1 separated convective and radiative film paths, the correct gain-routing fraction for an injected radiant flux should be derived from the ratio of conductances at the inner surface node (`G_conv / (G_conv + G_wall)` at minimum, or a full Y-Δ admittance expression if explicit surface DOFs are present). A regression test confirms the old formula overestimates the air-routed fraction by 0.10–0.40 depending on wall construction (matching the ticket's "~27% bias" claim). However, several details need correction: (1) EnergyPlus §3.5 and §3.5.10 do not exist; the E+ heat balance formulation never uses a splitting scalar — it uses an explicit per-timestep heat balance; (2) the ticket misattributes the bug as a post-S1 regression, but OCHRE's own code has always used the same voltage-divider formula (Envelope.py:254), meaning HARES faithfully reproduced OCHRE's limitation rather than introducing a new divergence; (3) the claimed "StarMesh topology" mismatch is real but not caused by a topology change — it is the inherent limitation of a pre-computed scalar routing fraction in any network that includes both convective and conductive branches at the inner node.

### Proposed Fix Summary
Replace the series voltage-divider at `solver_builder.rs:251-255` with a current-divider expression over the parallel conductances at the inner surface node. For the opaque-surface, ScriptF-only path (where `R_film_int` is convection-only), the correct fraction routing an injected gain to zone air is:

```
G_conv  = area / r_film_int            (convective path to zone air)
G_wall  = area / r_inner_half          (conductive path into wall mass)
interior_rad_frac = G_conv / (G_conv + G_wall)
```

This equals `r_inner_half / (r_film_int + r_inner_half)`, which is the **complement** of the current formula — not `r_film_int / (r_film_int + r_inner_half)`. The current code routes more gain to the zone air than physically correct; the fix routes more to the wall mass node.

An impulse-response test against the assembled state-space model should confirm convergence to within 1e-6 as specified in the ticket's Definition of Done.

### Test Written
- **File**: `crates/hares-envelope/tests/interior_lwr.rs`
- **Function**: `ticket089_radiation_frac_old_formula_disagrees_with_current_divider`
- **What it tests**: Numerically demonstrates that the old voltage-divider formula `r_film_conv / (r_film_conv + r_inner_half)` differs from the correct current-divider `G_conv / (G_conv + G_wall)` by more than 0.10 on a representative 100 mm concrete wall, and that the old formula overestimates the air-routed fraction. The test passes (characterises the bug as present) under the current code and should be updated to assert the correct value once the fix is applied.
