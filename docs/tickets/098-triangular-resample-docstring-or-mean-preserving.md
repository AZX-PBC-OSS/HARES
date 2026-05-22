# `Triangular` Resample Docstring Misclaims Hourly Mean Preservation

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-io/weather

## Problem

The `Triangular` resample method docstrings at `crates/hares-io/src/weather.rs:292` and `crates/hares-io/src/weather.rs:1055` claim "hourly mean preservation". The actual hourly mean of the triangular interpolant is:

```
mean_hour_i = 0.125 * value[i-1] + 0.75 * value[i] + 0.125 * value[i+1]
```

This is a weighted blend with neighbours, not a mean-preserving filter. At a sunrise transition with `prev=0, cur=800, next=800` W/m², the resulting hourly mean is 700 W/m² — a 12.5% shortfall against the original 800 W/m². The docstring is materially wrong; users relying on the documented "hourly mean preservation" property will get incorrect annual solar gains.

Two valid resolutions:

A. **Correct the docstring** — state the actual `0.125 * prev + 0.75 * cur + 0.125 * next` formula and quantify the boundary error, point users to `Zoh` or a true mean-preserving filter when integral conservation matters.

B. **Replace the implementation** with a true mean-preserving (energy-conservative) upsampling filter — e.g. piecewise-constant ZOH, or a midpoint-anchored filter that integrates back to the cell-mean.

## Current Behavior

`crates/hares-io/src/weather.rs:292` (and:1055): docstring claims hourly mean preservation. Implementation does not preserve the mean.

Numerical example: GHI series `[0, 800, 800]` W/m² resampled to 15-minute steps via the triangular tent centred at each hour midpoint produces sub-hourly values that, when averaged back over the second hour, yield 700 W/m² rather than 800 W/m². At seasonal aggregation this introduces a ~5% bias in solar-driven loads at sunrise/sunset transitions.

## Required Behavior

Either:

A. The docstring is corrected to the exact formula `mean_hour_i = 0.125 * value[i-1] + 0.75 * value[i] + 0.125 * value[i+1]`, with a worked example showing the 12.5% sunrise-hour shortfall and a recommendation to use `Zoh` (or a future mean-preserving method) for integrals.

OR

B. The implementation is replaced with a genuinely mean-preserving filter. The simplest option is `Zoh` (already the recommendation in ticket 030 for solar channels). For users who explicitly want smooth interpolation, document the trade-off and provide a method named `TriangularLossy` or similar.

The two-pronged choice exists because some users want smoothness and accept the boundary error; the docstring lie is the violation, not the smoothness method itself.

## Approach

Recommended path: implement A (docstring correction), since `Triangular` is already used as an opt-in option and `Zoh` is the documented default for solar channels in ticket 030. The replacement-implementation path (B) is the more invasive change.

1. Open `crates/hares-io/src/weather.rs:292` and `:1055` and replace the docstring with the corrected formula and a worked example.
2. Add a `WARNING:` block in the docstring stating that integral conservation is not guaranteed and recommending `Zoh` for energy-conservative use.
3. Add a unit test asserting the mean blend formula, so any future implementation change either preserves the formula or updates the docstring.
4. Cross-link this docstring to the existing ticket 030 which addresses the default for solar channels.

## Definition of Done

- [ ] `Triangular` docstring at `crates/hares-io/src/weather.rs:292` corrected to the actual blend formula
- [ ] `Triangular` docstring at `crates/hares-io/src/weather.rs:1055` corrected likewise
- [ ] Worked example in docstring shows the sunrise-hour 12.5% shortfall
- [ ] Recommendation to use `Zoh` for integral conservation included
- [ ] Unit test asserts `mean(triangular_resample([0, 800, 800])) == [0, 700, 800]` (the documented behaviour)
- [ ] No code claims "preserves hourly mean" for the triangular method anywhere

## Verification

```bash
cargo test -p hares-io resample
cargo test -p hares-io weather
cargo doc -p hares-io --no-deps   # confirm corrected docstring renders
```

## References

- ASHRAE Handbook of Fundamentals 2021 Ch. 14 §14.6 "Solar Radiation" — hourly solar values are period averages; sub-hourly upsampling must preserve the hourly integral if used for energy calculations.
- EnergyPlus Engineering Reference §2.5.5 "Solar Radiation Calculations" — energy-conservative upsampling.
- Press et al. *Numerical Recipes* 3rd ed. §3.3 "Cubic Spline Interpolation" — smooth interpolation methods do not generally preserve cell averages.

## Related Tickets

- 030-solar-upsampling-triangular-mean-error (default for solar channels — switch to Zoh)
- 099-resstock-csv-midpoint-offset (related weather-pipeline correction)

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match (or note corrected location)
  - Line 292: `Triangular` variant docstring in `ResampleMethod` enum — confirmed at `weather.rs:289-292`. The exact offending phrase is: *"midpoint interpolation which achieves the same goals (smooth transitions, **hourly mean preservation**) with a different algorithm."*
  - Line 1055: Same claim repeated in the `triangular_resample` function doc at `weather.rs:1056-1059`. Identical wording.
- [x] Described logic matches current implementation — confirmed. The piecewise-linear formulas at `weather.rs:1089-1093` exactly match what the ticket describes:
  ```
  frac < 0.5: values[prev]*(0.5-frac) + values[i]*(0.5+frac)
  frac ≥ 0.5: values[i]*(1.5-frac) + values[next]*(frac-0.5)
  ```
- [x] Bug confirmed: the phrase "hourly mean preservation" appears at both cited locations and is mathematically false (see numerical verification below).
- [x] OCHRE cross-check result: **N/A with a note.** OCHRE does not implement a triangular resampler at all. `vendors/OCHRE/ochre/utils/schedule.py:550-553` shows OCHRE uses `df.resample().interpolate()` (pandas linear interpolation, with `interpolate=True`) or `df.resample().ffill()` (ZOH, the default). HARES's `ResampleOverrides::ochre_compat()` at `weather.rs:324-338` reflects this correctly (ZOH for all fields). The `Triangular` method is a HARES-specific addition not present in OCHRE, so there is no OCHRE reference implementation to diverge from.
- [x] EnergyPlus cross-check result: **Partially matches in spirit, but the claim of "same goals" is overstated.** The EnergyPlus docstring reference (`SetupInterpolationValues`, `WeatherManager.cc:8328-8380`) is cited in the HARES docstring as implementing "a weighted blend of current/previous hours." The EnergyPlus Engineering Reference (multiple versions, confirmed via WebFetch of `bigladdersoftware.com/epx/docs/23-2/…`) states:

  > *"The solar values on the weather file are average values over the hour. For interpolation of hourly weather data (i.e., when the specified timestep is greater than 1), the average value is assumed to be the value at the midpoint of the hour. The reported values in the output are totals for each reporting period. So, hourly reported values will not match the original values in the weather file, but the **total solar for a day should agree**."*

  EnergyPlus uses the midpoint-assumption approach (same conceptual starting point as HARES's `Triangular`) but **explicitly disclaims hourly mean preservation** — only daily totals are conserved. The HARES docstring is therefore worse than EnergyPlus's own documentation in the claim it makes.

### Numerical Verification of the Bug

The continuous-limit mean over hour *i* of the triangular interpolant is derived analytically by integrating over [0, 1):

- ∫₀^0.5 [prev·(0.5−x) + cur·(0.5+x)] dx = **prev·1/8 + cur·3/8**
- ∫₀.₅^1 [cur·(1.5−x) + next·(x−0.5)] dx = **cur·3/8 + next·1/8**

**Total: 0.125·prev + 0.75·cur + 0.125·next** — confirming the ticket's formula exactly.

For the ticket's sunrise example `[0, 800, 800]`, hour 1 (prev=0, cur=800, next=800):
- Formula: 0.125·0 + 0.75·800 + 0.125·800 = **700 W/m²** (not 800)
- Shortfall: (800 − 700)/800 = **12.5%** — confirming the ticket's claim.

For finite resampling factors the discrete error is even larger:
| Factor | Step size | Hour-1 mean | Shortfall |
|--------|-----------|-------------|-----------|
| 4 | 15 min | 650.0 W/m² | **18.8%** |
| 12 | 5 min | 683.3 W/m² | 14.6% |
| 60 | 1 min | 696.7 W/m² | 12.9% |
| ∞ | continuous | 700.0 W/m² | 12.5% |

The standard `factor=4` (15-minute) case produces an **18.8% shortfall**, making the docstring claim even more misleading than the ticket's conservative 12.5% figure.

**Note on the existing test `triangular_hourly_mean_preservation` (line 2453):** The test comment *already explicitly documents* the correct formula (`0.125·values[prev] + 0.75·values[i] + 0.125·values[next]`), and uses a 2% relative-error tolerance for daytime hours with a smooth sinusoidal profile. The test does not contradict the ticket — it merely uses forgiving tolerances over a slowly-varying profile, hiding the sharp-transition case. The docstring lie persists despite the test's accurate comment.

### Web-Verified Citations

**Citation 1:**
- **Citation**: "ASHRAE Handbook of Fundamentals 2021 Ch. 14 §14.6 'Solar Radiation' — hourly solar values are period averages; sub-hourly upsampling must preserve the hourly integral if used for energy calculations."
- **Source found**: ASHRAE Handbook of Fundamentals 2021 Table of Contents (`ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals`), confirmed via WebFetch.
- **Quoted passage**: Chapter 14 of the 2021 ASHRAE Handbook—Fundamentals is titled **"Climatic Design Information"** — not "Solar Radiation". The full chapter list confirms: Ch. 14 = Climatic Design Information, Ch. 15 = Fenestration, Ch. 16 = Ventilation and Infiltration.
- **Verdict**: **Incorrect chapter title.** There is no chapter or section called "Solar Radiation" in ASHRAE HoF 2021. The nearest relevant content on solar radiation in the context of building load calculations is in Ch. 15 (Fenestration) and Ch. 18 (Nonresidential Cooling and Heating Load Calculations). The underlying claim — that hourly EPW solar values are period averages — is a well-known fact corroborated by EnergyPlus documentation, but the ASHRAE citation as written (Ch. 14 §14.6 "Solar Radiation") does not exist. The section number §14.6 cannot be verified.

**Citation 2:**
- **Citation**: "EnergyPlus Engineering Reference §2.5.5 'Solar Radiation Calculations' — energy-conservative upsampling."
- **Source found**: EnergyPlus Engineering Reference, "Climate Calculations" chapter, confirmed via WebFetch of `bigladdersoftware.com/epx/docs/23-2/engineering-reference/climate-calculations.html` and `bigladdersoftware.com/epx/docs/8-8/engineering-reference/climate-calculations.html`.
- **Quoted passage** (verbatim from EnergyPlus Engineering Reference, multiple versions):
  > *"The solar values on the weather file are average values over the hour. For interpolation of hourly weather data (i.e., when the specified timestep is greater than 1), the average value is assumed to be the value at the midpoint of the hour. The reported values in the output are totals for each reporting period. So, hourly reported values will not match the original values in the weather file, but the total solar for a day should agree."*
  — Section heading: **"Weather File Solar Interpolation"** (not §2.5.5 "Solar Radiation Calculations")
- **Verdict**: **Partially correct.** The underlying engineering fact is documented by EnergyPlus: hourly solar values are period averages treated as midpoint values. However, the section reference (§2.5.5) and heading ("Solar Radiation Calculations") do not match what was found in any version of the EnergyPlus Engineering Reference. The correct section heading is "Weather File Solar Interpolation" within the "Climate Calculations" chapter. Crucially, EnergyPlus explicitly states only **daily totals** are preserved — not hourly means — contradicting the "energy-conservative upsampling" label in the citation.

**Citation 3:**
- **Citation**: "Press et al. *Numerical Recipes* 3rd ed. §3.3 'Cubic Spline Interpolation' — smooth interpolation methods do not generally preserve cell averages."
- **Source found**: Multiple sources confirm Numerical Recipes 3rd ed., Chapter 3 "Interpolation and Extrapolation", §3.3 covers cubic spline interpolation. Confirmed via web search (`numerical.recipes/forumarchive`, `foo.be/docs-free/Numerical_Recipe_In_C/c3-3.pdf` title confirmed as "3.3 Cubic Spline Interpolation 113"). Also confirmed by peer-reviewed literature: "Mean-preserving interpolation with splines for solar radiation modeling" (ScienceDirect, DOI 10.1016/j.solener.2022…) and "A Fast Mean-Preserving Spline for Interpolating Interval Data" (JTECH 2022) both confirm that standard splines (including cubic) do not preserve interval means and that dedicated mean-preserving methods are needed.
- **Quoted passage**: Section heading "3.3 Cubic Spline Interpolation" confirmed. The specific claim ("smooth interpolation methods do not generally preserve cell averages") is not a verbatim quote from §3.3 itself — it is an inference that the ticket presents as a paraphrase. The claim is **mathematically correct** as a general statement (cubic splines pass through knot values, not through cell-average constraints) and is corroborated by the mean-preserving-spline literature.
- **Verdict**: **Confirmed in substance.** The section reference (§3.3) and title are correct. The cited claim (smooth interpolation ≠ mean-preserving) is mathematically accurate, though the exact phrase does not appear verbatim in §3.3. The citation is a valid supporting reference for the claim.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: The core bug is real and confirmed: the docstrings at `weather.rs:289-292` and `weather.rs:1056-1059` both claim "hourly mean preservation" for the `Triangular` resampler, but the implementation provably does not preserve hourly means at sharp transitions. The mean formula `0.125·prev + 0.75·cur + 0.125·next` is analytically verified, and the 12.5% shortfall at a sunrise step is correct in the continuous limit (18.8% at the common 15-minute resolution). The EnergyPlus cross-reference strengthens this — E+ itself only promises daily totals, not hourly means. The ticket is "partially" rather than "fully" legitimate because: (a) the ASHRAE citation (Ch. 14 §14.6 "Solar Radiation") is incorrect — no such chapter/section exists in HoF 2021; and (b) the EnergyPlus section reference (§2.5.5 "Solar Radiation Calculations") does not match the actual section heading found in the Engineering Reference. The cited claims are directionally right but the specific bibliographic details are wrong and should be corrected when resolving the ticket.

### Proposed Fix Summary

Implement **Resolution A** (docstring correction):

1. In the `Triangular` variant docstring (`weather.rs:268-292`): replace the phrase "hourly mean preservation" with the actual blend formula `mean = 0.125·prev + 0.75·cur + 0.125·next`; add a `WARNING:` block noting integral non-conservation and recommending `Zoh` for energy-conservative use; include the sunrise worked example (prev=0, cur=800, next=800 → mean=700, 12.5% shortfall at continuous limit, 18.8% at 15-min resolution).
2. Apply the same correction to the `triangular_resample` function docstring (`weather.rs:1034-1059`).
3. Correct the ASHRAE citation to the actual chapter title ("Climatic Design Information", Ch. 14) and remove the non-existent §14.6 "Solar Radiation" sub-section.
4. Correct the EnergyPlus citation to the actual section heading ("Weather File Solar Interpolation", "Climate Calculations" chapter).
5. Do **not** change production code logic — the implementation is correct; only the claim about it is wrong.

### Test Written

- **File**: `crates/hares-io/src/weather.rs` (within existing `#[cfg(test)]` block at end of file)
- **Test name**: `triangular_mean_is_not_preserved_at_sharp_transitions`
- **What it tests**: Given the sunrise step sequence `[0.0, 800.0, 800.0]` resampled with a large factor (600 steps ≈ continuous limit), the test asserts that the per-hour means match the blend formula `0.125·prev + 0.75·cur + 0.125·next` (tolerance < 1 W/m²) and differ significantly from the original hourly values — confirming the non-mean-preserving behaviour and locking it in so any future implementation change that accidentally achieves or breaks true mean preservation forces a docstring sync.
