# Berdahl-Martin Sky Emissivity Coefficient Citation Wrong

**Severity**: Nit
**Priority**: P4
**Status**: Open
**Areas**: hares-io/epw

## Problem

`crates/hares-io/src/epw.rs:529-531` uses Berdahl-Martin clear-sky emissivity coefficients `0.758`, `0.521`, and `0.625` and cites them to "Martin & Berdahl (1984)". This citation is incorrect: the original 1984 paper by Berdahl & Martin gives coefficients `0.711`, `0.56`, and `0.73`. The coefficients actually used (`0.758`, `0.521`, `0.625`) are the recalibrated values used by EnergyPlus 9.6 and reported by Li et al. (2017, Solar Energy). The wrong citation misleads any future contributor who tries to verify the numbers against the original paper.

## Current Behavior

`crates/hares-io/src/epw.rs:529-531`:
```rust
// Berdahl-Martin clear-sky emissivity (Martin & Berdahl 1984)
let eps_clear = 0.758 + 0.521 * t_dp_c / 100.0 + 0.625 * (t_dp_c / 100.0).powi(2);
```

Comment cites the wrong source for the coefficients.

## Required Behavior

Update the citation to reflect the actual provenance of the coefficients:

1. Cite EnergyPlus Engineering Reference §3.7.4 "Sky Emissivity" or §3.5.6 (depending on E+ version) for the recalibrated form actually used.
2. Cite Li, Coimbra & Walsh (2017) "On the determination of atmospheric longwave irradiance under all-sky conditions" *Solar Energy* 144:40-48 for the recalibrated coefficient set.
3. Optionally retain a secondary reference to the original Berdahl & Martin (1984) paper, but make clear the coefficient values are the recalibrated set, not the original.

## Approach

1. Update the inline comment at `crates/hares-io/src/epw.rs:529-531` to:
```rust
// Berdahl-Martin form, recalibrated coefficients per Li et al. (2017)
// and EnergyPlus 9.6+ (see EnergyPlus Engineering Reference §3.5.6).
// Original Berdahl & Martin (1984) gave 0.711 / 0.56 / 0.73 — superseded.
let eps_clear = 0.758 + 0.521 * t_dp_c / 100.0 + 0.625 * (t_dp_c / 100.0).powi(2);
```
2. Verify no other file in the workspace cites the same coefficients with the wrong attribution.
3. Cross-check the EnergyPlus 9.6 source to confirm the coefficient set matches before committing the new citation.

## Definition of Done

- [ ] Citation comment at `crates/hares-io/src/epw.rs:529-531` updated to cite Li et al. (2017) and EnergyPlus Engineering Reference
- [ ] Comment notes the original Berdahl & Martin (1984) values for historical clarity
- [ ] No other workspace file cites these coefficients with the wrong attribution

## Verification

```bash
cargo test -p hares-io epw
rg '0\.758.*0\.521.*0\.625' crates/
```

## References

- Li, M., Coimbra, C.F.M. & Walsh, P. (2017). "On the determination of atmospheric longwave irradiance under all-sky conditions." *Solar Energy* 144:40-48. DOI: 10.1016/j.solener.2017.01.006 — recalibrated coefficient set actually used.
- EnergyPlus Engineering Reference (DOE), §3.5.6 "Sky Emissivity Calculations" — documents the recalibrated form used by EnergyPlus 9.6+.
- Berdahl, P. & Martin, M. (1984). "Emissivity of clear skies." *Solar Energy* 32(5):663-664 — original 1984 paper with `0.711 / 0.56 / 0.73` coefficients (superseded).

## Related Tickets

- 025-psm3-sky-temp-clark-allen-only (related sky temperature model)

---

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers no longer match — the ticket cites `epw.rs:529-531` but the code has been refactored. The actual function is now at **`crates/hares-io/src/epw.rs:523-538`** (`berdahl_martin_sky_emissivity`). The ticket was written against an older one-liner form; the function now has a multi-line docstring.
- [x] Described logic matches current implementation — the coefficients `0.758 / 0.521 / 0.625` and the quadratic form are present and correct.
- [x] **Partially fixed since ticket was filed** — the current comment at lines 523–533 already distinguishes between the two 1984 papers (Solar Energy 32 vs 33) and notes the original `0.711/0.56/0.73` values. However it **does not yet cite Li et al. (2017)**, which is what EnergyPlus itself credits for the recalibrated set.
- [x] A second citation site exists at **`crates/hares-core/src/dwelling/synthetic.rs:744-754`** with the same incomplete attribution (Martin & Berdahl 1984 only, no Li et al. 2017 reference).
- [x] OCHRE cross-check result: **N/A** — OCHRE does not implement the Berdahl-Martin formula at all. OCHRE uses direct Stefan-Boltzmann inversion from the `ghi_infrared` field (`vendors/OCHRE/ochre/utils/schedule.py:179-182`): `df["sky_temperature"] = convert((df["ghi_infrared"].values / 5.6697e-8) ** 0.25, "K", "degC")`. No OCHRE sky emissivity coefficients to cross-check against.
- [x] EnergyPlus cross-check result: **matches** — confirmed below.

### Web-Verified Citations

#### Citation A: Berdahl & Martin (1984) Solar Energy 32(5):663-664 — original coefficients 0.711/0.56/0.73

- **Citation**: Ticket claims this paper uses `0.711 / 0.56 / 0.73` as coefficients.
- **Sources found**:
  - [OSTI/ETDEWEB record 6431813](https://www.osti.gov/etdeweb/biblio/6431813) — confirms paper title "Emissivity of clear skies", authors Berdahl & Martin, Solar Energy 32(5).
  - [Semantic Scholar](https://www.semanticscholar.org/paper/Emissivity-of-clear-skies-Berdahl-Martin/f46f220679db7010b680f7d6117019d4ef83c95e) — confirms publication details; 254 citations.
  - Multiple independent secondary sources (survey papers, review articles) consistently reproduce: `ε_clear = 0.711 + 0.56(T_dp/100) + 0.73(T_dp/100)²` attributed to this paper.
- **Quoted passage**: From web search aggregate of secondary literature: *"Martin and Berdahl proposed a quadratic relation between the monthly average clear sky emissivity and the monthly average dew point temperature: εm = 0.711 + 0.56(Tdp/100) + 0.73(Tdp/100)²"* (Sky Temperature Estimation and Measurement, IBPSA 2017 proceedings).
- **Verdict**: **Confirmed** — paper exists, `0.711/0.56/0.73` is universally attributed to this reference in secondary literature.

#### Citation B: Martin & Berdahl (1984) Solar Energy 33(3/4):321-336 — the companion paper

- **Citation**: HARES code (current state) attributes `0.758/0.521/0.625` to this companion paper.
- **Sources found**:
  - [Semantic Scholar](https://www.semanticscholar.org/paper/Characteristics-of-infrared-sky-radiation-in-the-Martin-Berdahl/2732ac39769a8bc709fd66536c2d418064cbb6c3) — confirms title "Characteristics of Infrared Sky Radiation in the United States", authors Martin & Berdahl, Solar Energy 33:321-336.
  - [ScienceDirect abstract](https://www.sciencedirect.com/science/article/abs/pii/0038092X84901622) — confirms DOI 0038-092X/84/90162-2.
- **Quoted passage**: No search result or accessible source attributes `0.758/0.521/0.625` to the Solar Energy 33 paper. All sources associating a coefficient set with the 1984 Martin & Berdahl paper use `0.711/0.56/0.73`.
- **Verdict**: **Incorrect** — it is not established that `0.758/0.521/0.625` appeared in the Solar Energy 33 paper. The EnergyPlus Engineering Reference labels the model "Martin & Berdahl" but cites **only Li et al. 2017** as its source. The claim in the current HARES docstring that `0.758/0.521/0.625` is "the quadratic form from Martin & Berdahl (1984) Solar Energy 33(3/4)" is unverified and likely wrong.

#### Citation C: Li, Coimbra & Walsh (2017) Solar Energy 144:40-48 — recalibrated coefficients

- **Citation**: Ticket attributes recalibrated coefficients to this paper; ticket gives authors as "Li, Coimbra & Walsh".
- **Sources found**:
  - [Renewable Energy Advancement Lab (first author's page)](https://www.li-realab.info/publication/radiative-model/li-2017/) — paper confirmed. **Authors are Li, Jiang & Coimbra — not Walsh.**
  - [EnergyPlus 9.3 Engineering Reference](https://bigladdersoftware.com/epx/docs/9-3/engineering-reference/climate-calculations.html) — reference list states: *"Li, M., Jiang, Y. and Coimbra, C. F. M. 2017. On the determination of atmospheric longwave irradiance under all-sky conditions. Solar Energy 144, 40-48"*
  - EnergyPlus source code (confirmed via sub-agent from WeatherManager.cc) cites **only** Li et al. 2017 in the `CalcSkyEmissivity` function comment for the BerdahlMartin model.
- **Quoted passage from EnergyPlus 9.3 Engineering Reference**: *"Li, M., Jiang, Y. and Coimbra, C. F. M. 2017. On the determination of atmospheric longwave irradiance under all-sky conditions. Solar Energy 144, 40-48"* (in the references list of the Sky Radiation Modeling section, together with Walton 1983, Clark & Allen 1978).
- **Verdict**: **Partially correct** — paper exists with correct journal, volume, and pages. **The third author is Jiang, not Walsh** — this is an error in the ticket. The formula and coefficient set `0.758/0.521/0.625` are attributed to Li et al. by EnergyPlus. Full-text access to Li et al. 2017 is paywalled; the specific table in which the recalibrated coefficients appear could not be directly quoted, but EnergyPlus's exclusive citation of Li et al. for these coefficients is strong circumstantial evidence.

#### Citation D: EnergyPlus Engineering Reference §3.5.6 / §3.7.4 "Sky Emissivity"

- **Citation**: Ticket claims the recalibrated form is documented in EnergyPlus Engineering Reference §3.5.6 or §3.7.4.
- **Source found**: [EnergyPlus 9.6 Engineering Reference — Climate Calculations](https://bigladdersoftware.com/epx/docs/9-6/engineering-reference/climate-calculations.html) and [9.3 version](https://bigladdersoftware.com/epx/docs/9-3/engineering-reference/climate-calculations.html).
- **Quoted passage**: *"ϵsky,clear = 0.758 + 0.521(Tdp/100) + 0.625(Tdp/100)²"* — exact formula confirmed in both E+ 9.3 and 9.6 under the section titled "Sky Radiation Modeling". The referenced citations in that section are: *(Walton, 1983) (Clark & Allen, 1978), (Li et al, 2017).*
- **Verdict**: **Confirmed** that EnergyPlus uses these exact coefficients and labels the model "Martin & Berdahl". **Section numbers §3.5.6/§3.7.4 cannot be verified** — the online HTML version uses anchor headings ("Sky Radiation Modeling"), not numbered subsections. The section number claim in the ticket is unverifiable from the online documentation.

### Legitimacy

- **Verdict**: **Partially Legitimate**

- **Rationale**: The core claim — that the comment in HARES mislabels the provenance of `0.758/0.521/0.625` — has merit, but the situation is more nuanced than the ticket describes. At the time of this audit, the code at `epw.rs:523-533` has already been partially corrected: it now distinguishes the two 1984 papers and mentions the original `0.711/0.56/0.73` values. However it still fails to cite Li et al. (2017), which is the source EnergyPlus exclusively credits for this coefficient set. The same gap exists in `synthetic.rs:753-754`. Additionally, the ticket itself contains an error: it names the third author of Li et al. as "Walsh" when the correct author is Jiang. The ticket's claim that the EnergyPlus documentation is at "§3.5.6 or §3.7.4" cannot be verified. The claim that `0.758/0.521/0.625` appear specifically in *Martin & Berdahl (1984) Solar Energy 33* — as distinct from Li et al. 2017 — is unestablished by any accessible source; it may be that both papers share these values, but the consensus provenance in EnergyPlus and secondary literature points to Li et al. 2017.

### Proposed Fix Summary

Update two comment sites (do NOT touch production logic):

1. **`crates/hares-io/src/epw.rs:523-533`** — Replace the current `Cite:` line with a dual citation: Li, Jiang & Coimbra (2017) Solar Energy 144:40-48 as the primary provenance of the recalibrated coefficients, and Martin & Berdahl (1984) Solar Energy 33(3/4):321-336 as the functional form origin. The original Berdahl & Martin (1984) Solar Energy 32(5):663-664 mention can remain for historical context.

2. **`crates/hares-core/src/dwelling/synthetic.rs:753-754`** — Same correction: add Li, Jiang & Coimbra (2017) citation alongside the current Martin & Berdahl (1984) reference.

The correct author list for Li et al. is: **Li, M., Jiang, Y. and Coimbra, C.F.M.** (not Walsh). Do not use "Li, Coimbra & Walsh" anywhere in the codebase.

### Test Written

- **File**: `crates/hares-io/src/epw.rs` — added inside the existing `#[cfg(test)]` module at the "Sky emissivity models" section
- **Test name**: `epw::tests::berdahl_martin_coefficients_match_energyplus_recalibrated_set`
- **What it tests**: Pins the three coefficient values (`0.758`, `0.521`, `0.625`) at three dew-point temperatures (0 °C, 10 °C, 20 °C) against exact expected values that match the EnergyPlus/Li et al. 2017 recalibrated set. Assertions are written with explicit messages contrasting against the original Berdahl & Martin 1984 values (`0.711/0.56/0.73`). If anyone "corrects" the coefficients back to the original 1984 paper values the test fails with a clear explanation.
- **Test result**: All 4 berdahl-related tests pass (`cargo test -p hares-io --lib berdahl`).
