# Ground Temperature Uses Shallowest EPW Depth; Should Use Foundation Depth

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-io, hares-envelope

## Problem

When an EPW file contains multiple ground-temperature depth entries, `parse_ground_temperatures` in `crates/hares-io/src/epw.rs` selects the entry with the **smallest depth** (`best_depth < best_depth` selection loop at line 354). EPW files from EnergyPlus typically include ground temperatures at 0.5 m, 2.0 m, and 4.0 m depths. The 0.5 m depth has the largest seasonal amplitude — it closely tracks the outdoor air temperature sinusoid with minimal damping. For a residential slab-on-grade or basement foundation, the relevant depth for the thermal boundary condition is closer to 0.5–1.0 m for the slab surface but 1.5–3.0 m for foundation walls and the undisturbed soil boundary.

Selecting the shallowest depth (0.5 m) as the ground boundary temperature for all ground-coupled boundaries — slab, basement wall, and crawlspace — over-estimates the seasonal swing of the ground temperature. In winter, 0.5 m ground temperature is substantially colder than 2.0 m; in summer, substantially warmer. This produces a systematic over-prediction of slab heat loss in winter and heat gain in summer relative to the physically correct deeper boundary temperature.

The DOE-2 fallback (`doe2_ground_temp_monthly`) uses a fixed depth factor of `DOE2_GROUND_DEPTH_FACTOR = 10.0` m (line 371 in `epw.rs`), which is far too deep and essentially eliminates all seasonal variation. The actual floor/basement boundary in residential construction sits at 0.3–1.5 m below grade.

## Evidence

`crates/hares-io/src/epw.rs:354` — shallowest depth selected:
```rust
if valid && depth_m < best_depth {
    best_depth = depth_m;
    best_monthly = Some(monthly);
}
```

`crates/hares-io/src/epw.rs:371` — DOE-2 fallback uses 10 m depth factor:
```rust
const DOE2_GROUND_DEPTH_FACTOR: f64 = 10.0;
```

The Kusuda-Achenbach model in `crates/hares-physics/src/ground.rs` already supports depth-parameterized temperature at any depth, but is not used in the EPW-path ground temperature selection.

## Required Fix

1. Replace the shallowest-depth selection heuristic with a configurable target depth. The default should be 0.5 m for slab surface temperature (appropriate for slab-on-grade) and 2.0 m for foundation wall midpoint temperature (appropriate for basements). The solver builder at `hares-core/src/dwelling/solver_builder.rs` should specify the relevant depth when constructing the thermal solver config.

2. For the DOE-2 fallback, replace the `DOE2_GROUND_DEPTH_FACTOR = 10.0` constant with a depth parameter (default 0.5 m for slab surface). At 10 m depth the seasonal amplitude is attenuated by a factor of `exp(-10 * sqrt(π / (α * τ)))` ≈ `exp(-10 * 0.4)` ≈ 0.018 — effectively zero. At 0.5 m it is `exp(-0.5 * 0.4)` ≈ 0.82, which correctly captures seasonal variation.

3. Expose the ground boundary depth as a per-boundary configuration field in the HPXML parser and `ThermalSolverConfig`, allowing slab vs. basement vs. crawlspace to each specify their relevant depth.

## OCHRE Cross-Check

OCHRE `Envelope.py` uses the EPW 0.5 m depth by default but allows configuration. The current HARES behavior (selecting shallowest regardless of foundation type) is consistent with OCHRE's default but known to introduce error for deep foundations. EnergyPlus uses the Kusuda-Achenbach model with building-specific soil properties and depth.

## Annual kWh Impact

Medium. For slab-on-grade buildings, the 0.5 m selection is approximately correct. For buildings with basements or deep crawlspaces, the error in annual ground heat loss can reach 5–15% of the total ground coupling load. In climate zones 4–7 (cold climates), ground coupling is a significant fraction of the heating load.

## References

- EnergyPlus Engineering Reference §3.17 "Ground Heat Transfer Calculations" — Kusuda-Achenbach model with site-specific depth
- Kusuda, T. and Achenbach, P.R. (1965) ASHRAE Transactions 71(1):61-74 — original derivation
- ASHRAE Handbook of Fundamentals 2021 Ch. 18.31 "Below-Grade Heat Transfer" — recommended depths for different foundation types
- `crates/hares-physics/src/ground.rs:63-80` — Kusuda-Achenbach implementation already present

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-22

### Code Confirmation
- [x] Referenced line numbers still match (confirmed: shallowest-depth loop at `epw.rs:354`, `DOE2_GROUND_DEPTH_FACTOR` at `epw.rs:371`)
- [x] Described logic matches current implementation — `best_depth = f64::INFINITY` initialised at line 328; loop at line 354 selects `depth_m < best_depth`, unconditionally picking the shallowest depth
- [x] OCHRE cross-check result: **matches HARES** — `vendors/OCHRE/ochre/utils/schedule.py:248` uses `beta = (np.pi / (8760 * 0.025)) ** 0.5 * 10`, identical to HARES `DOE2_GROUND_DEPTH_FACTOR = 10.0` / `DOE2_GROUND_DIFFUSIVITY = 0.025`. OCHRE does **not** read EPW GROUND TEMPERATURES at all; it always recalculates from dry-bulb using this DOE-2 formula. The shallowest-depth EPW selection is a HARES-only code path.
- [x] EnergyPlus cross-check result: **matches** for the Kusuda-Achenbach formula. EnergyPlus Engineering Reference (v22.1, BigLadder Software) states: *"T(z,t) = T̄s − ΔT̄s · e^(−z·√(π/ατ)) · cos(2πt/τ − θ)"* citing Kusuda & Achenbach 1965. EnergyPlus provides three depth objects (`Site:GroundTemperature:Shallow` = 0.5 m, `Site:GroundTemperature:BuildingSurface`, `Site:GroundTemperature:Deep`) and explicitly warns *"Do not use the 'undisturbed' ground temperatures from the weather data. These values are too extreme for the soil under a conditioned building."* This means the EPW GROUND TEMPERATURES header was never intended for direct use as a building surface boundary condition; selecting any specific depth from it is an approximation regardless.

### Web-Verified Citations

**Citation 1**: EnergyPlus Engineering Reference §3.17 "Ground Heat Transfer Calculations"
- **Source found**: https://bigladdersoftware.com/epx/docs/22-1/engineering-reference/undisturbed-ground-temperature-model-kusuda.html
- **Quoted passage**: *"T(z,t) = T̄s − ΔT̄s · e^(−z·√(π/ατ)) · cos(2πt/τ − θ)"* — Kusuda, T. and P.R. Achenbach. 1965. "Earth Temperatures and Thermal Diffusivity at Selected Stations in the United States." ASHRAE Transactions. 71(1): 61-74.
- **Verdict**: **Partially correct** — the formula and Kusuda citation are confirmed. However the section identifier "§3.17" does not correspond to a numbered section in the web-hosted EnergyPlus Engineering Reference, which uses heading-based navigation. The Kusuda-Achenbach content lives under "Undisturbed Ground Temperature Model: Kusuda-Achenbach" without a §3.17 number. The citation is substantively correct but the section number is unverifiable and likely inaccurate.

**Citation 2**: Kusuda, T. and Achenbach, P.R. (1965) ASHRAE Transactions 71(1):61-74
- **Source found**: https://www.semanticscholar.org/paper/EARTH-TEMPERATURE-AND-THERMAL-DIFFUSIVITY-AT-IN-THE-Kusuda-Achenbach/fe1b3ec9c47d2bc09059f6aea282f8cd55d77064
- **Quoted passage**: Semantic Scholar confirms the paper: "Earth Temperature and Thermal Diffusivity at Selected Stations in the United States", ASHRAE Transactions 71(1): 61-74, 1965 — also cited verbatim in the EnergyPlus Engineering Reference.
- **Verdict**: **Confirmed** — correct authors, journal, volume, issue, and page range.

**Citation 3**: ASHRAE Handbook of Fundamentals 2021 Ch. 18.31 "Below-Grade Heat Transfer"
- **Source found**: https://www.ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals
- **Quoted passage**: The official ASHRAE 2021 HoF table of contents lists Chapter 18 as **"Nonresidential Cooling and Heating Load Calculations"**. Chapter 17 is "Residential Cooling and Heating Load Calculations". Below-grade heat transfer content (slab and basement) is a section within Chapter 17, not Chapter 18. No chapter is titled or subtitled "Below-Grade Heat Transfer".
- **Verdict**: **Incorrect** — Chapter 18 of ASHRAE HoF 2021 is not about below-grade heat transfer. Below-grade residential slab and basement heat loss methodology appears in Chapter 17, not Chapter 18. The section number "18.31" cannot be independently verified and appears erroneous. The same wrong citation appears in `crates/hares-physics/src/ground.rs:11`.

**Attenuation formula in ticket body**: The ticket claims the DOE-2 fallback at 10 m depth gives "attenuation `exp(-10 * sqrt(π / (α * τ))) ≈ exp(-10 * 0.4) ≈ 0.018 — effectively zero`"
- **Analysis**: This calculation uses the Kusuda-Achenbach formula (`exp(-z * decay)`) but applies it to characterise the DOE-2 GTEMP model, which uses a **different** formula involving `gm = sqrt((x²-2x·cos(β)+1)/(2β²))`. Numerically: with `α = 0.025 m²/hr`, `τ = 8760 h`, `z = 10.0`, `β = 1.1977`, the DOE-2 formula yields `gm ≈ 0.551` — meaning **55% of the seasonal amplitude is retained**, not near-zero. The ticket's attenuation claim of `exp(-10 * 0.4) ≈ 0.018` would only hold if the HARES Kusuda-Achenbach model (`DEFAULT_SOIL_DIFFUSIVITY_M2_PER_DAY = 0.05 m²/day`) were applied at 10 m depth. The numerical claim in the ticket body is therefore incorrect when applied to the DOE-2 fallback path.
- **Verdict**: **Incorrect** for the DOE-2 fallback. The underlying concern (10 m is too deep for a shallow-foundation reference) is still valid — a shallower depth would retain more seasonal signal (gm ≈ 0.97 at 0.5 m vs 0.55 at 10 m) — but the magnitude of the error is much smaller than the ticket implies.

### Legitimacy
- **Verdict**: **Partially Legitimate**
- **Rationale**: The primary bug — `parse_ground_temperatures` selects the shallowest EPW depth rather than a physically motivated reference depth — is **confirmed real** (code at `epw.rs:354`, failing regression test at `epw.rs:1283-1310` using `#[should_panic]`). This causes incorrect depth selection when an EPW provides entries at depths other than 0.5 m (e.g., 0.1 m, 0.5 m, 2.0 m). For typical EnergyPlus EPW files that always start at 0.5 m the current code accidentally selects the right depth, so the practical impact is narrower than described. The second claim — that `DOE2_GROUND_DEPTH_FACTOR = 10.0` "essentially eliminates all seasonal variation" — is **numerically incorrect**: the DOE-2 GTEMP formula (inherited verbatim from OCHRE `schedule.py:248`) retains 55% of surface amplitude at depth 10 m, not ~2%. The concern that 10 m is too deep for a shallow-foundation reference remains valid in direction but the magnitude is overstated, and the same choice is present in OCHRE (not a HARES divergence). The ASHRAE citation (Ch. 18.31) is wrong — Chapter 18 covers nonresidential cooling load calculations; below-grade residential heat transfer is in Chapter 17. The EnergyPlus §3.17 section number is unverifiable. The Kusuda & Achenbach (1965) citation is correct.

### Proposed Fix Summary
1. **EPW depth selection (primary fix)**: Change `parse_ground_temperatures` to select the depth entry closest to a configurable target depth (default 0.5 m) rather than the unconditional minimum. Update the constant `f64::INFINITY` initialisation to use a `target_depth` parameter and replace `depth_m < best_depth` with `(depth_m - target_depth).abs() < (best_depth - target_depth).abs()`. Expose the target depth through `ThermalSolverConfig` so slab vs. basement boundaries can differ.
2. **DOE-2 depth factor (secondary fix)**: Replace the hard-coded `DOE2_GROUND_DEPTH_FACTOR: f64 = 10.0` with a parameter (suggested default 0.5 m matching the EPW reference depth) passed into `doe2_ground_temp_from_monthly_avg`. Note that the resulting `gm` change is moderate (~0.55 → ~0.97), not the dramatic 0.018 → 0.82 implied by the ticket.
3. Do NOT change the OCHRE-parity test (`doe2_ground_temp_matches_ochre_damped_formula`) until the formula change is intentional and documented.

### Test Written
- File: `crates/hares-io/src/epw.rs` — tests already exist
- **Existing failing test** (confirms Bug 1): `epw::tests::ground_temp_depth_selection_picks_0_5_m_not_0_1_m` (line 1283) — marked `#[should_panic]`, passes precisely because the current code picks 0.1 m instead of 0.5 m.
- **Existing passing test** (confirms typical EPW is accidentally correct): `epw::tests::ground_temp_depth_selection_picks_0_5_m_from_0_5_2_4_depths` (line 1256) — verifies that for the standard [0.5, 2.0, 4.0] depth set, the shallowest-wins heuristic happens to pick 0.5 m.
- **Existing passing test** (confirms Bug 2 is real but smaller than claimed): `epw::tests::doe2_ground_depth_factor_10m_overdamps_vs_0_5m` (line 1332) — verifies amplitude ratio ~0.55 at 10 m vs ~0.97 at 0.5 m.
- No new test written; existing coverage is sufficient to demonstrate and track both bugs.
