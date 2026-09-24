# Re-derive Per-Surface UA Expectations From ASHRAE/E+ First Principles, Drop Tolerance Widening

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-envelope, tests/parity, tests/structural_envelope_oracle

## Problem

Two parity tolerances were widened to mask a divergence between HARES and OCHRE that the review identified as HARES being more correct, not less:

1. `tests/parity/tolerance.rs:8,30`: MAE was widened from 0.1°C → 0.6°C and peak HVAC power from 2% → 80%.
2. `tests/structural_envelope_oracle.rs`: `beopt_ua_parity` compares HARES UA per surface against hardcoded OCHRE constants (e.g. `OCHRE_TOTAL_UA: f64 = 558.74`).

Both treat OCHRE as the truth. The underlying physics: HARES uses a convection-only film coefficient (R_film_conv ~0.447 m²·K/W at ΔT=5°C, computed via TARP) plus an explicit linearised radiation conductance (h_rad branch) in parallel. OCHRE uses a single combined R_film (~0.12 m²·K/W per ISO 6946) that bundles convection and radiation. The HARES decomposition is closer to ASHRAE/EnergyPlus practice; the per-surface UA naturally differs because the radiation path is now explicit rather than baked into a combined film.

Widening the tolerances normalizes OCHRE divergence as if OCHRE were correct. The correct response is to re-derive expected per-surface UA from first principles using the same convection-only + explicit h_rad network HARES actually solves, then validate against BESTEST ASHRAE 140 reference bands rather than OCHRE constants.

## Current Behavior

`tests/parity/tolerance.rs:8`:
```rust
pub const PARITY_MAE_C: f64 = 0.6;  // widened from 0.1
```

`tests/parity/tolerance.rs:30`:
```rust
pub const PARITY_PEAK_HVAC_PCT: f64 = 80.0;  // widened from 2.0
```

`tests/structural_envelope_oracle.rs`: hardcoded `OCHRE_TOTAL_UA: f64 = 558.74` and per-surface OCHRE constants used as the comparison baseline.

`tests/bestest/mod.rs:77,99,120,143,167`: BESTEST tests `#[ignore]`d — see ticket 094.

## Required Behavior

1. Re-derive expected per-surface UA values from first principles:
   - Per-surface UA = 1 / (R_film_exterior_conv + R_layers + R_film_interior_conv) for the conduction path
   - Per-surface radiation path: explicit linearised h_rad conductance to other interior surfaces (StarMesh)
   - The two paths are parallel branches in the assembled network, not a series film resistance
2. Replace hardcoded `OCHRE_TOTAL_UA` and per-surface OCHRE constants in `tests/structural_envelope_oracle.rs` with values derived from ASHRAE HoF 2021 Ch. 26 / EnergyPlus Engineering Reference §3.5 first principles. Cite the derivation inline.
3. Drop the tolerance widening in `tests/parity/tolerance.rs`: restore PARITY_MAE_C to 0.1 (or document the tightest tolerance achievable while passing BESTEST 140 reference bands), restore PARITY_PEAK_HVAC_PCT to a defensible value (no looser than 5%).
4. Adopt BESTEST ASHRAE 140 reference bands as the validation gate. BESTEST defines pass/fail bands for annual heating, annual cooling, and peak loads. These are the primary acceptance criteria; OCHRE parity is a useful cross-check at best.

## Approach

1. Compute reference per-surface UA values using TARP convection coefficients at design conditions (winter ΔT=20°C indoor-outdoor for heating-design, summer ΔT=11°C for cooling-design — ASHRAE HoF 2021 Ch. 14) and explicit linearised h_rad at the design mean surface temperature.
2. Document the derivation in a comment block at the top of `tests/structural_envelope_oracle.rs` with citations.
3. Replace the OCHRE comparison with first-principles comparison; allow ±2% tolerance to absorb residual modelling differences between TARP and the ASHRAE HoF tabulated value.
4. Tighten `tests/parity/tolerance.rs` to the minimum that passes the existing parity suite once the BESTEST gate is in place.
5. Cross-validate by running BESTEST cases 600, 610, 620, 900, and 940 (or whichever are in `tests/bestest/`) and checking annual heating/cooling totals fall within ASHRAE 140 reference bands.

## Definition of Done

- [ ] `tests/structural_envelope_oracle.rs` no longer references `OCHRE_TOTAL_UA` or any other hardcoded OCHRE constant as the baseline
- [ ] Per-surface UA expectations derived from first principles with inline citations
- [ ] `PARITY_MAE_C` returned to 0.1°C (or to a lower value that passes the suite — never 0.6)
- [ ] `PARITY_PEAK_HVAC_PCT` returned to ≤5% (never 80%)
- [ ] BESTEST cases 600/900 (and others currently `#[ignore]`d) passing within ASHRAE 140 reference bands — see ticket 094
- [ ] Comment block in `tests/structural_envelope_oracle.rs` documents the derivation and explicitly states "OCHRE differs because it uses a combined R_film; HARES is correct"

## Verification

```bash
cargo test -p hares-envelope structural_envelope_oracle
cargo test --test parity
cargo test --test bestest
```

Annual heating and cooling for BESTEST 600/900 must fall within the ASHRAE Standard 140 published reference bands.

## References

- ASHRAE Standard 140-2020 *Standard Method of Test for the Evaluation of Building Energy Analysis Computer Programs* — published reference band data for cases 600, 610, 620, 630, 640, 650, 900, 910, 920, 930, 940, 950, 960.
- ASHRAE Handbook of Fundamentals 2021 Ch. 26 "Heat, Air, and Moisture Control in Building Assemblies — Material Properties" — surface conductance values.
- ASHRAE Handbook of Fundamentals 2021 Ch. 14 "Climatic Design Information" — winter/summer design conditions.
- EnergyPlus Engineering Reference §3.5 "Inside Surface Heat Balance" — TARP convection model and explicit longwave network.
- ISO 6946:2017 — combined surface resistance values (the OCHRE baseline being departed from).

## Related Tickets

- 089-radiation-frac-starmesh-rederivation (related StarMesh derivation work)
- 094-bestest-tests-still-ignored (BESTEST is the validation gate)
- feedback_ashrae_not_ochre — project policy: target ASHRAE/E+, never regress to match OCHRE bugs

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match (with corrections noted below)
- [x] Described logic matches current implementation
- [x] OCHRE cross-check result: **partially matches** — see details below
- [x] EnergyPlus cross-check result: **matches** — HARES interior film is convection-only, consistent with E+ `CalcASHRAESimpleIntConvCoeff`

#### Line-number corrections

The ticket references `tests/parity/tolerance.rs:8,30` with constant names `PARITY_MAE_C` and `PARITY_PEAK_HVAC_PCT`. The actual constant names differ:

| Ticket name | Actual name (tolerance.rs) | Line | Value |
|---|---|---|---|
| `PARITY_MAE_C` | `ZONE_TEMP_CONDITIONED_C_MAE_MAX` | 8 | `0.6` |
| `PARITY_PEAK_HVAC_PCT` | `PEAK_HVAC_POWER_REL_PCT_MAX` | 30 | `80.0` |

The values (0.6°C MAE, 80% peak HVAC) and their location (lines 8 and 30) are correct. The constant names are wrong in the ticket — a minor labelling error that does not affect the substance of the issue.

`tests/structural_envelope_oracle.rs`: `OCHRE_TOTAL_UA: f64 = 558.74` confirmed at **line 87**. All per-surface OCHRE constants confirmed at lines 69–85. However, the ticket's description of how these constants are used is **partially incorrect** — see the OCHRE cross-check section.

`tests/bestest/mod.rs`: All five `#[ignore]` attributes confirmed at lines 77, 99, 120, 143, 167.

#### OCHRE cross-check

The ticket claims OCHRE uses "a single combined R_film (~0.12 m²·K/W per ISO 6946) that bundles convection and radiation," and that the OCHRE constants `OCHRE_TOTAL_UA = 558.74` etc. are derived from this combined film.

**This is incorrect.** Reading `/vendors/OCHRE/ochre/utils/envelope.py:342–401`, OCHRE's `calculate_film_resistances()` also uses **TARP natural convection only** for the interior film (line 401: `return {"Interior Film Resistance (m^2-K/W)": 1 / h_natural}`). The interior film resistance OCHRE returns is `1 / h_natural`, exactly the same convection-only convention HARES uses. OCHRE does **not** use ISO 6946 combined film resistance for dynamic simulation.

However, OCHRE clamps `delta_t = max(12.9, |T_ext − T_int|)` (line 374), whereas HARES uses ASHRAE Simple fixed h_conv values (3.076 for vertical). At ΔT = 12.9°C, TARP gives `h = 1.31 × 12.9^(1/3) ≈ 3.076 W/(m²·K)`, so they agree for vertical surfaces. The divergence for non-vertical surfaces (attic floor, attic roof) is due to this clamped-ΔT vs. actual-ΔT difference, not a convection-vs-combined-film difference.

**The OCHRE constants** `OCHRE_EXTERIOR_WALL_UA = 34.34`, `OCHRE_TOTAL_UA = 558.74`, etc., are now marked `#[allow(dead_code)]` in the current code (lines 68–87) and are **not used as the assertion baseline** in `beopt_ua_parity`. The test at line 688–1035 compares HARES against `tests/fixtures/parity/ashrae_rc_reference.json`, which is independently derived from ASHRAE/E+ first principles. The OCHRE constants appear only as diagnostic `eprintln!` references. The ticket's framing ("both treat OCHRE as the truth") is **no longer accurate** for the UA parity test.

**The tolerance widening** (`ZONE_TEMP_CONDITIONED_C_MAE_MAX = 0.6` and `PEAK_HVAC_POWER_REL_PCT_MAX = 80.0`) is real and documented in tolerance.rs with in-code rationale pointing to the step-0 ideal-capacity back-solve in `stepping.rs:24–66`. These tolerances are genuinely wide and the underlying issues are not yet fixed, so the core concern about wide tolerances remains valid even if the diagnosis of its cause is partially wrong.

### Web-Verified Citations

**Citation 1: EnergyPlus Engineering Reference §3.5 "Inside Surface Heat Balance"**
- **Source found**: EnergyPlus Engineering Reference v9.2, Big Ladder Software mirror: https://bigladdersoftware.com/epx/docs/9-2/engineering-reference/inside-heat-balance.html
- **Quoted passage**: *"Walton derived his coefficients from the surface conductances for e = 0.90 found in the ASHRAE Handbook (1985). The radiative heat transfer component was estimated at 1.02 × 0.9 = 0.918 BTU/h-ft²-°F and then subtracted off. Finally the coefficients were converted to SI units."* Values: vertical h = 3.076; horizontal enhanced h = 4.040; horizontal reduced h = 0.948 W/(m²·K). Also: *"it permits separating the radiant and convective parts of the heat transfer at the surface, which is an important attribute of the heat balance method."*
- **Verdict**: **Confirmed.** The EnergyPlus inside heat balance does explicitly separate convection and radiation. The ASHRAE Simple interior coefficients are convection-only, with radiation subtracted. The section number in the ticket (§3.5) cannot be verified as the online docs use descriptive anchors, not section numbers, but the content is correct. TARP equations confirmed: vertical h = 1.31·ΔT^(1/3); enhanced h = 9.482·ΔT^(1/3)/(7.238−|cosΣ|); reduced h = 1.810·ΔT^(1/3)/(1.382+|cosΣ|).

**Citation 2: ASHRAE Handbook of Fundamentals 2021 Ch. 26 — surface conductance values**
- **Source found**: EnergyPlus Engineering Reference (which quotes ASHRAE HoF 1985 Table 1 as the basis for the Simple algorithm). Direct ASHRAE HoF 2021 text is paywalled. Secondary source confirms interior film R for vertical walls: 0.68 hr·ft²·°F/BTU combined = ~0.120 m²·K/W combined. The convection-only component after subtracting radiative h_rad ≈ 0.918 BTU/h-ft²-°F yields h_conv ≈ 3.076 W/(m²·K), R_conv ≈ 0.325 m²·K/W.
- **Quoted passage** (from ASHRAE 90.1 secondary source, unmethours.com): "R-0.12 for interior air film (wall)" as the combined value. The EnergyPlus derivation confirms the split: combined R ≈ 0.120 m²·K/W, convection-only R ≈ 0.325 m²·K/W.
- **Verdict**: **Confirmed.** The combined film resistance ~0.12 m²·K/W for vertical walls aligns with ASHRAE tabulated values. The ticket's attribution of this value to "Ch. 26" is directionally correct (HoF covers surface properties) though Ch. 26 specifically covers material properties; surface conductances are in HoF Ch. 25 or Ch. 26 Table 1 depending on edition. The substance — combined 0.12 m²·K/W vs. convection-only 0.325 m²·K/W — is verified.

**Citation 3: ASHRAE HoF 2021 Ch. 14 "Climatic Design Information" — ΔT=20°C / ΔT=11°C design conditions**
- **Source found**: Chapter 14 covers Climatic Design Information (confirmed via ASHRAE table of contents https://www.ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals). Specific ΔT values cannot be verified without subscription access.
- **Verdict**: **Cannot fully verify** — the chapter reference is correct but the specific design conditions (ΔT=20°C heating, ΔT=11°C cooling) are plausible but unconfirmed from public sources. This citation is used only in the "Approach" section (not in any assertion or formula), so the inability to verify it does not affect the core ticket validity.

**Citation 4: ISO 6946:2017 — combined surface resistance values**
- **Source found**: ISO 6946:2017 standard page https://www.iso.org/standard/65708.html; sampler PDF at cdn.standards.iteh.ai. Search results confirm ISO 6946:2017 Table 1 lists Rsi (interior surface resistance) values of 0.10 m²·K/W (upward heat flow), 0.13 m²·K/W (horizontal/vertical), and 0.17 m²·K/W (downward heat flow).
- **Quoted passage**: Reliable secondary source (htflux.com, citing ISO 6946) confirms these Rsi values. The canonical ISO 6946:2017 Rsi for vertical walls (horizontal heat flow) is **0.13 m²·K/W**, not 0.12 m²·K/W as stated in the ticket.
- **Verdict**: **Partially correct.** ISO 6946 does use a combined surface resistance that bundles convection and radiation. The ISO 6946:2017 Rsi for vertical walls is **0.13 m²·K/W**, not 0.12 m²·K/W. The ticket says "~0.12 m²·K/W per ISO 6946" — this is close but inexact (0.12 is the ASHRAE HoF combined value; 0.13 is the ISO 6946:2017 value). **OCHRE does not use ISO 6946 values** for its dynamic film coefficients — it uses TARP, confirmed by reading the OCHRE source directly.

**Citation 5: ASHRAE Standard 140-2020 — BESTEST reference band data**
- **Source found**: ASHRAE Std 140 reference results from LBNL Modelica Buildings Library documentation https://simulationresearch.lbl.gov/modelica/releases/v7.0.0/help/Buildings_ThermalZones_Detailed_Validation_BESTEST_Cases6xx.html; the BESTEST test file itself at `tests/bestest/mod.rs:60–70`.
- **Quoted passage**: `tests/bestest/mod.rs:69–70`: *"Metrics: annual heating load [4296, 5709] kWh, annual cooling load [6137, 7964] kWh (ASHRAE 140-2017 Table B8-2)."* The Modelica source confirms: Case 600 heating band 4.296–5.709 GJ and cooling −6.137 to −7.964 GJ (converted: heating 1193–1586 kWh... wait — GJ to kWh: 1 GJ = 277.78 kWh; 4.296 GJ = 1193 kWh). *Note: there is a unit inconsistency between sources — the LBNL Modelica reference reports GJ, while the HARES test comments report kWh. 4296 kWh ≈ 15.5 GJ, not 4.296 GJ. The LBNL documentation likely reports in MWh or the values are for a different metric.* The HARES code's band [4296, 5709] kWh is consistent with the search result stating "annual heating load reference range is 3.75 to 4.98 MWh/yr" (~3750–4980 kWh) — close but not identical, likely reflecting the 2017 vs 2020 standard revision.
- **Verdict**: **Confirmed** (with minor version uncertainty). BESTEST Case 600 annual heating band is approximately 3750–5709 kWh and cooling 5000–7964 kWh depending on the exact 2017/2020 edition. The BESTEST reference bands cited in the ticket are real and well-known.

### Legitimacy
- **Verdict**: **Partially Legitimate**
- **Rationale**: The core concern — that tolerance widening (`ZONE_TEMP_CONDITIONED_C_MAE_MAX = 0.6°C`, `PEAK_HVAC_POWER_REL_PCT_MAX = 80%`) was applied without a defensible physics baseline and should be tightened — is real and valid. The BESTEST gate is the correct validation target and is currently `#[ignore]`d. However, the ticket's primary diagnosis is incorrect in two important ways: (1) OCHRE's `calculate_film_resistances()` uses TARP convection-only film (not ISO 6946 combined), so the described "OCHRE combined R_film vs HARES convection-only" discrepancy does not exist in the film resistance layer. (2) The `beopt_ua_parity` test already compares HARES against an ASHRAE/E+-derived JSON reference (`ashrae_rc_reference.json`), not against OCHRE constants — the OCHRE constants in `structural_envelope_oracle.rs` are `#[allow(dead_code)]` and appear only as diagnostic `eprintln!` outputs. The actual UA divergences visible in the test output (Attic Floor +88% R_film, Attic Roof −18% R_film, total UA 604.68 vs 585.32 W/K = +3.3%) stem from the clamped-ΔT (OCHRE uses `max(12.9, |ΔT|)`) vs ASHRAE Simple fixed-h_conv difference for non-vertical surfaces. These are real differences but they are documented in the code and are within the 15% per-boundary tolerance set in `beopt_ua_parity`. The step-0 back-solve issue driving the 80% peak HVAC tolerance is a separate and real bug that should be fixed independently of the UA oracle question.

### Proposed Fix Summary

Do NOT implement any of the following — summary only:

1. **Tighten `PEAK_HVAC_POWER_REL_PCT_MAX`**: Fix the step-0 ideal-capacity back-solve in `crates/hares-envelope/src/thermal_solver/stepping.rs:24–66` first; once fixed, restore this constant to ≤20% for short-window cycling.
2. **Tighten `ZONE_TEMP_CONDITIONED_C_MAE_MAX`**: After BESTEST gate passes, document the tightest achievable value. The 0.6°C value may be defensible for 1-hour windows; restore to 0.1°C only if BESTEST 600/900 pass at that tolerance.
3. **`beopt_ua_parity` baseline**: No change needed to the oracle — `ashrae_rc_reference.json` is already the ASHRAE/E+ first-principles baseline. Remove or re-label the `OCHRE_TOTAL_UA` and per-surface OCHRE constants to clarify they are diagnostic, not assertions.
4. **BESTEST gate**: Un-ignore `bestest_case_600`, `bestest_case_900`, etc. once upstream physics fixes (B1 radiant fraction, S4/S5 initialization) are resolved (tracked in ticket 094).
5. **Chapter/section reference correction**: Change the ticket's "ASHRAE HoF 2021 Ch. 26" surface conductance citation to Ch. 25 (or verify the 2021 edition chapter number), and correct ISO 6946 Rsi for vertical walls from "~0.12" to "0.13 m²·K/W".

### Test Written
- **File**: `tests/structural_envelope_oracle.rs` — Test 6 (`ashrae_interior_film_resistance_regression`, lines 1061–1130) already exists and covers the convection-only film resistance regression. It passes. No new test is needed.
- **What it tests**: Asserts that `film_resistances(90°, Conditioned, Outdoor, ...)` returns `r_int ≈ 1/3.076 ≈ 0.325 m²·K/W` (convection-only), not the combined ~0.120 m²·K/W. Guards against regression to combined film by asserting `r_int_wall > 0.25`.
