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

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] **Line numbers corrected**: The ticket cites line 246. The function `zone_sensible_breakdown_debug` is at `mod.rs:220` (signature) with `apply_port_sensible_inputs` at line 246. Confirmed correct.
- [x] **Described logic matches current implementation**: Verified. `zone_sensible_breakdown_debug` (lines 220–256) calls `apply_port_sensible_inputs` at line 246 and does NOT call `apply_port_radiant_inputs`. The production path `prepare_inputs_inner` (line ~461–498) calls both in sequence (`apply_port_sensible_inputs` at line 497, `apply_port_radiant_inputs` at line 498). The divergence is exactly as described.
- [x] **Bug is present (not already fixed)**: Confirmed by running the regression test — it fails with `breakdown[5] = 700.000 W, expected 850.000 W`.
- [x] **`ZoneSensibleBreakdown` type**: The ticket's code snippet shows `-> ZoneSensibleBreakdown` but the actual return type is `[f64; 6]`. This is a minor inaccuracy in the ticket's pseudocode (the struct doesn't exist yet — it's part of the proposed fix, not the current code). The bug description itself is accurate.
- [x] **OCHRE cross-check**: OCHRE (`Envelope.py:1195–1198`) calls `inputs_init[zone.h_idx] += zone.radiation_heat` which accumulates the air-node residual from LWR. Its debug/reporting path also reads `zone.radiation_heat` (line 1198), meaning OCHRE includes the radiant air residual in zone-level diagnostics. HARES diverges by omitting the radiant call in the debug path. **Verdict: HARES diverges from OCHRE here, and the divergence is accidental.**
- [x] **EnergyPlus cross-check**: EnergyPlus Engineering Reference (v9.6, v25.2, "Basis for the Zone and Air System Integration") gives the zone air heat balance as `CzdTz/dt = ΣQ̇ᵢ + ΣhᵢAᵢ(Tₛᵢ−Tz) + ...`, where `ΣQ̇ᵢ` is the sum of **convective** internal loads only. Radiant gains go to surfaces first and return to the air via the surface convection term `hᵢAᵢ(Tₛᵢ−Tz)`. The inside-heat-balance docs confirm: *"The traditional model for this source is to define a radiative/convective split for the heat introduced into a zone from equipment. The radiative part is then distributed over the surfaces within the zone in some prescribed manner."* HARES's `apply_port_radiant_inputs` implements this correctly (via TMULT area×emissivity weighting, with a `(1−radiation_frac)` air residual). The debug function's omission of this call means the debug path does not match the EnergyPlus model. **Verdict: EnergyPlus cross-check confirms the production path is correct and the debug path is missing a call.**

### Web-Verified Citations

**Citation 1**: EnergyPlus Engineering Reference §3.5 "Inside Surface Heat Balance" and §3.6 "Zone Air Heat Balance"

- **Source found**: BigLadder Software EnergyPlus Engineering Reference v9.6 ("Inside Heat Balance") and v25.2 ("Basis for the Zone and Air System Integration")
- **Quoted passage** (Inside Heat Balance, v25.2): *"The traditional model for this source is to define a radiative/convective split for the heat introduced into a zone from equipment. The radiative part is then distributed over the surfaces within the zone in some prescribed manner. This, of course, is not a completely realistic model, and it departs from the heat balance principles."* And: *"q′′LWX + q′′SW + q′′LWS + q′′ki + q′′sol + q′′conv = 0"* where q′′LWS is "Longwave radiation flux from equipment in a zone."
- **Quoted passage** (Zone/Air Integration): *"CzdTzdt = Σ Q̇ᵢ + Σ hᵢAᵢ(Tₛᵢ−Tz) + Σ ṁᵢCp(Tzᵢ−Tz) + ṁᵢₙ꜀Cp(T∞−Tz) + Q̇sys"* where ΣQ̇ᵢ is explicitly *"sum of the convective internal loads"* — radiant loads do not appear here directly.
- **Verdict**: **Confirmed** — radiant internal gains route to surface heat balances, not directly to zone air; only the convective residual from surface exchanges reaches zone air. The section numbers §3.5/§3.6 are a loose reference (EnergyPlus docs use chapter-level titles rather than numbered sections), but the concepts are correctly identified.

**Citation 2**: BESTEST 900FF — convective and radiant gain components must be tracked separately

- **Source found**: HARES internal documents `docs/findings/bestest_rca.md` and `crates/hares-envelope/tests/bestest_900ff_root_cause.rs` (which cite ASHRAE 140-2017 §5.2.4.3)
- **Quoted passage** (`bestest_rca.md:55–57`): *"The BESTEST specification per ASHRAE 140-2017 §5.2.4.3 and the EnergyPlus BESTEST IDF defines internal gains as 200W with Fraction Radiant = 0.3 (30% radiant, 70% convective, 0% latent, 0% lost)."* And: *"`bestest_900ff_root_cause.rs:13–15`": "Internal gains 100% convective; EnergyPlus BESTEST IDF specifies Fraction Radiant = 0.3 (30% of total gain is radiant). Measured impact: −0.295 °C."*
- **Note**: The ASHRAE 140-2017 standard is paywalled and PDFs were not directly readable via WebFetch. Web searches returned conflicting claims (one source stating 60% radiant; the project's own RCA document with cross-reference to the BESTEST IDF specifies 30%). The HARES project documentation is internally consistent and cites the IDF directly. The 30% figure is credible but cannot be independently quoted from the primary ASHRAE 140-2017 document.
- **Verdict**: **Partially confirmed** — the 30% radiant fraction is well-supported by the project's own root-cause analysis and BESTEST IDF references, but direct web access to ASHRAE 140-2017 §5.2.4.3 was not possible. The ticket correctly identifies the need to track the split.

**Citation 3**: ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.2.2 "Convective and Radiant Components"

- **Source found**: ASHRAE HoF 2021 Table of Contents (https://www.ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals)
- **Quoted passage**: Chapter 18 is titled *"Nonresidential Cooling and Heating Load Calculations"*. The TOC does not list internal section numbers. The chapter does contain tables on radiant/convective fractions for internal loads (referenced in secondary sources as Table 3 and Table 14 in Ch. 18).
- **Verdict**: **Partially correct** — Chapter 18 covers nonresidential cooling/heating loads (not residential, as a reader might assume from BESTEST context). The chapter does address convective/radiant splits for internal gains. The specific section §18.2.2 heading "Convective and Radiant Components" could not be verified against the paywalled primary source. The conceptual claim — that ASHRAE HoF documents radiant/convective fractions for internal gains — is accurate.

### Legitimacy

- **Verdict**: **Legitimate**
- **Rationale**: The bug is definitively real and verified by code inspection and a failing regression test. `zone_sensible_breakdown_debug` at `mod.rs:220–256` calls only `apply_port_sensible_inputs` (line 246) and omits `apply_port_radiant_inputs`, while the production path `prepare_inputs_inner` calls both (lines 497–498). The `apply_port_radiant_inputs` function at `ports.rs:34–76` distributes radiant gains to surface RC nodes using TMULT area×emissivity weighting, with a `(1−radiation_frac)` residual returned to the zone air node. Omitting this call in the debug path means `breakdown[5]` understates the zone-air contribution by the air-node radiant residual. For a typical `radiation_frac = 0.5` on a 300 W radiant gain, the shortfall is 150 W — approximately 18% of total gains. EnergyPlus documentation and OCHRE both confirm that radiant gains produce a convective residual at the air node (via surface heat exchange), and the production code correctly models this. The BESTEST 900FF test at `tests/bestest/mod.rs:478` consumes `zone_sensible_breakdown_debug` for diagnostics, meaning the BESTEST analysis is biased. The numerical claim of ~21% understatement (for 30% radiant fraction with specific `radiation_frac` values) is plausible but depends on the per-surface `radiation_frac` configuration. The citations (EnergyPlus, ASHRAE 140-2017) are directionally correct though section numbers are approximate.

### Proposed Fix Summary

In `zone_sensible_breakdown_debug` (`mod.rs:220–256`), after the existing `apply_port_sensible_inputs` call at line 246, add a call to `apply_port_radiant_inputs(&mut u, ports)`. This aligns the debug function with the production call sequence. Optionally, change the return type from `[f64; 6]` to a new `ZoneSensibleBreakdown` struct with named fields (`convective_direct_w`, `radiant_to_air_residual_w`, `radiant_to_surfaces_w`) to make attribution explicit, as proposed in the ticket's Approach section.

### Test Written

- **File**: `crates/hares-envelope/tests/thermal_pathway_physics.rs` (function `zone_sensible_breakdown_debug_must_include_radiant_air_residual`, appended at end of file)
- **What it tests**: A 1-state zone model with one interior surface (`radiation_frac=0.5`) receives a port with 700 W convective + 300 W radiant. Asserts that `breakdown[5]` equals 850 W (700 convective + 150 W air residual from 300 × 0.5 radiant), not 700 W. Currently **FAILS** (produces 700 W), demonstrating the bug. Will pass once `apply_port_radiant_inputs` is added to the debug function.
