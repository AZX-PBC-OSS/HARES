# EER2 Not Converted to EER — Treated as Identical, Overstating Room AC Efficiency

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-io

## Problem

`normalize_efficiency_units` at line 1772 of `resolve_hvac.rs` maps `"EER2"` to
`"EER"` with no numeric correction:

```
"SEER" | "EER" | "EER2" | "HSPF" | "AFUE" | "PERCENT" | "COP" => {
    (units.trim().to_ascii_uppercase(), value)
}
```

EER2 is measured under DOE's revised test procedure (AHRI 340/360-2022 for
commercial units; residential room ACs follow a similar update under 10 CFR Part 430
Appendix F). EER2 ratings are approximately 8% lower than EER under the old test
procedure, meaning EER = EER2 / 0.92. Using EER2 as if it were EER overstates
efficiency by approximately 8%.

SEER2 is correctly converted using `SEER2_TO_SEER_FACTOR = 1.0 / 0.95`. EER2
has a distinct conversion factor that is not applied.

Note: EER2 for room air conditioners is governed by DOE's 2022 rulemaking
(AHRI 310/380-2022); the factor varies slightly by unit type (window vs. through-
wall, single vs. reverse-cycle). Approximately 1/0.92 is the residential central
estimate.

## Evidence

`resolve_hvac.rs:1772`:

```
"SEER" | "EER" | "EER2" | "HSPF" | "AFUE" | "PERCENT" | "COP" => {
    (units.trim().to_ascii_uppercase(), value)
```

`resolve_hvac.rs:1770`:

```
"SEER2" => ("SEER".to_string(), value * SEER2_TO_SEER_FACTOR),
```

`SEER2` is normalized; `EER2` is not.

## OCHRE Cross-check

OCHRE receives pre-normalised EER from ResStock and does not encounter EER2 values
in practice. OCHRE provides no reference for this conversion.

## Required Behavior

Add a conversion constant at `resolve_hvac.rs:27–28` alongside the existing SEER2/HSPF2 constants:

```
const EER2_TO_EER_FACTOR: f64 = 1.0 / 0.92;
```

In `normalize_efficiency_units` at `resolve_hvac.rs:1772`, change EER2 from the
pass-through arm to a conversion arm:

```
"EER2" => ("EER".to_string(), value * EER2_TO_EER_FACTOR),
```

Also verify that `eer_from_params` at `resolve_hvac.rs:400` treats the stored
`"EER2"` tag consistently: after `normalize_efficiency_units` runs, the stored units
key will be `"EER"`, so line 400's `eq_ignore_ascii_case("EER2")` branch is exercised
only if the normalization has not yet run. Confirm the normalization always runs before
`eer_from_params` is called, or update line 400 accordingly.

## Approach

1. Add `const EER2_TO_EER_FACTOR: f64 = 1.0 / 0.92;` at `resolve_hvac.rs:29` (after HSPF2 constant).
2. In `normalize_efficiency_units` at line 1772, split EER2 out of the pass-through arm into its own conversion arm (before the pass-through arm, like SEER2).
3. Audit `eer_from_params` at line 400: if EER2 has already been normalized to EER by `normalize_efficiency_units`, remove the redundant `eq_ignore_ascii_case("EER2")` condition. If normalization may not have run yet, keep it but note the dependency.

## Citation

- DOE Final Rule, 87 FR 74364 (December 2022): defines EER2 test conditions under
  the revised DOE test procedure; central residential estimate EER ≈ EER2 / 0.92
- DOE 10 CFR Part 430, Subpart B, Appendix F (2022 revision): room air conditioner
  EER2 measurement methodology
- AHRI 310/380-2022: packaged terminal and room AC test procedure alignment with
  DOE 2022 revision

## Annual kWh Impact Rank

**Low.** EER2-rated equipment is uncommon in residential HPXML files today (most
files carry EER under the old procedure). Impact when present: ~8% understatement
of room AC electricity consumption.

## Definition of Done

- [ ] `EER2_TO_EER_FACTOR: f64 = 1.0 / 0.92` defined at `resolve_hvac.rs:29`
- [ ] `normalize_efficiency_units` at `resolve_hvac.rs:1772` converts EER2 → EER using the factor (not a pass-through)
- [ ] `eer_from_params` at `resolve_hvac.rs:400` audited for consistency; EER2 branch removed or documented
- [ ] Test: `normalize_efficiency_units("EER2", 10.0)` → `("EER", 10.0/0.92)` ≈ `("EER", 10.870)` (within 0.01)
- [ ] Test: `normalize_efficiency_units("EER", 10.0)` → `("EER", 10.0)` (unchanged)
- [ ] Test: HPXML room AC with `<Units>EER2</Units><Value>10.0</Value>` → EIR ≈ 0.3138 (not 0.3412)

## Verification

```bash
cargo test -p hares-io -- normalize_efficiency_units
cargo test -p hares-io -- resolve_hvac::tests::eer2
```

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation
- [x] Referenced line numbers still match (or note corrected location)
  - `normalize_efficiency_units` is at line 1768 (ticket says 1772 — minor shift, same function).
  - `SEER2_TO_SEER_FACTOR` is at line 27; `HSPF2_TO_HSPF_FACTOR` at line 28 (ticket says "27–28" — correct).
  - `eer_from_params` is at line 392 (ticket says 400 — minor shift). The `eq_ignore_ascii_case("EER2")` condition is at line 400, inside the function body.
- [x] Described logic matches current implementation
  - `"SEER2"` has its own conversion arm (line 1770); `"HSPF2"` has its own arm (line 1771).
  - `"EER2"` falls through the pass-through arm at line 1772 alongside plain `"EER"` — no conversion applied. Bug confirmed.
  - `eer_from_params` at line 400: `units.eq_ignore_ascii_case("EER") || units.eq_ignore_ascii_case("EER2")` treats both identically. If `normalize_efficiency_units` ran first the stored key is already `"EER2"` (unchanged), so this branch is still exercised with the unconverted value.
- [x] OCHRE cross-check result: **N/A — OCHRE does not handle EER2**
  - `vendors/OCHRE/ochre/utils/hpxml.py:855` lists only `["EER", "SEER", "HSPF"]` as valid efficiency units. EER2 is not mentioned anywhere in the OCHRE codebase. The ticket's OCHRE cross-check claim is accurate: "OCHRE receives pre-normalised EER and does not encounter EER2 values in practice."
- [x] EnergyPlus cross-check result: **N/A — EER2 is a DOE/AHRI certification metric, not an EnergyPlus simulation input**
  - EnergyPlus takes EIR (Energy Input Ratio) directly; EER2 normalisation happens at the HPXML-ingestion layer, not inside EnergyPlus itself. The EnergyPlus Engineering Reference has no section on EER2 conversion since it pre-dates the AHRI 210/240-2023 metrics.

### Web-Verified Citations

#### Citation 1
- **Citation**: "DOE Final Rule, 87 FR 74364 (December 2022): defines EER2 test conditions under the revised DOE test procedure; central residential estimate EER ≈ EER2 / 0.92"
- **Source found**: Federal Register search for 87 FR 74364. Search results indicate 87 FR 75168 (December 7, 2022) covers Single Package Vertical Air Conditioners; 87 FR 77325 (December 16, 2022) covers Three-Phase Small Commercial Package Equipment. Neither covers residential room ACs.
- **Quoted passage**: Unable to retrieve full text of page 74364 — federalregister.gov redirects to an unblock page. However, from surrounding Federal Register references: the December 2022 DOE rulemakings in volume 87 address *commercial* and *single-package vertical* equipment, not residential room air conditioners (which use 10 CFR 430 Appendix F and report CEER, not EER2).
- **Verdict**: **Incorrect / Unverified**. The cited document number could not be verified as covering residential room AC EER2. Even if it covers EER2 for a commercial product class, the cited 0.92 conversion ratio is not confirmed by any authoritative source found.

#### Citation 2
- **Citation**: "DOE 10 CFR Part 430, Subpart B, Appendix F (2022 revision): room air conditioner EER2 measurement methodology"
- **Source found**: 10 CFR Part 430 Subpart B Appendix F, current eCFR text (via law.cornell.edu and eCFR search)
- **Quoted passage** (from law.cornell.edu fetch): "This Appendix F specifies a uniform test method for measuring the energy consumption of room air conditioners... The primary performance metric is CEER (Combined Energy Efficiency Ratio)." The appendix defines CEER, not EER2. The 2021 amendment (86 FR) updated the standby power measurement; no EER2 is introduced in Appendix F.
- **Verdict**: **Incorrect**. 10 CFR Part 430 Appendix F does NOT define EER2 for room air conditioners. Room ACs are rated in **CEER** under Appendix F. EER2 is defined in Appendix M1 for *central* air conditioners and heat pumps. The ticket misattributes EER2 to room AC Appendix F.

#### Citation 3
- **Citation**: "AHRI 310/380-2022: packaged terminal and room AC test procedure alignment with DOE 2022 revision"
- **Source found**: AHRI standards search (ahrinet.org). The current standard is AHRI 310/380-2017/CSA C744-17 **(R2022)** — this is the 2017 standard reaffirmed in 2022, not a new 2022 edition.
- **Quoted passage** (from GlobalSpec/ANSI search): "AHRI 310/380-2017/CSA C744:17 (R2022) — Packaged Terminal Air-Conditioners and Heat Pumps." The "(R2022)" suffix means reaffirmed, not revised.
- **Verdict**: **Incorrect**. There is no "AHRI 310/380-2022." The 2017 standard was reaffirmed (not revised) in 2022. More importantly, AHRI 310/380 covers *packaged terminal* air conditioners (PTACs/PTHPs), which are commercial hotel-style units — not residential room/window ACs. The ticket conflates PTAC and residential room AC standards.

#### Citation 4 (implicit): EER2/EER conversion factor 0.92
- **Citation**: Ticket states "EER2 ratings are approximately 8% lower than EER... EER = EER2 / 0.92"
- **Source found**: California Energy Commission conversion table (via heatpump.review citing CEC guidance); DOE Appendix M1 test condition changes (multiple sources)
- **Quoted passage** (from heatpump.review/CEC table): "Split system air conditioner < 45,000 Btu/h: EER = EER2 × 1.043" and "Packaged air conditioner: EER = EER2 × 1.038." Also: "For all air conditioners the conversion factor is 0.96 to convert EER to EER2" (implying EER = EER2 / 0.96 ≈ EER2 × 1.0417).
- **Verdict**: **Incorrect**. The empirically supported conversion for ducted split systems under AHRI 210/240 and DOE Appendix M1 is approximately **EER = EER2 × 1.043** (≈ 4.3% correction), NOT EER = EER2 / 0.92 (≈ 8.7% correction). The ticket overstates the magnitude of the correction by roughly 2×. The 8% figure may derive from HSPF2/HSPF analogies (where the reduction is 15%) or from misremembering the SEER2/SEER 5% factor.

  Note: Room air conditioners rated under Appendix F use CEER, not EER2. If an HPXML file carries `<Units>EER2</Units>` for a room AC, it is almost certainly an error in the HPXML file (central-AC metric misapplied to a room AC), rather than a standard DOE room AC rating. The ticket does not acknowledge this ambiguity.

### Legitimacy
- **Verdict**: **Partially Legitimate**
- **Rationale**: The core bug is real and confirmed: `normalize_efficiency_units` passes `EER2` through without conversion (lines 1768–1776 of `resolve_hvac.rs`), while `SEER2` and `HSPF2` each have dedicated conversion arms. The regression test added in this audit (`eer2_is_not_treated_as_eer_passthrough`) fails immediately, proving the bug is live. However, three material inaccuracies weaken the ticket: (1) the cited DOE rule number (87 FR 74364) and Appendix F citation do not establish EER2 for *room* air conditioners — room ACs use CEER under Appendix F, not EER2; EER2 belongs to central AC/heat pump Appendix M1; (2) the cited AHRI 310/380-2022 does not exist as a 2022 revision; (3) the conversion factor 1/0.92 (≈ 8% uplift) is not supported by authoritative sources — the CEC conversion table and multiple industry references consistently cite approximately 4–4.3% (EER = EER2 × 1.043 for split systems, × 1.038 for packaged). The correct constant is closer to `EER2_TO_EER_FACTOR = 1.0 / 0.96` (≈ 1.0417) for central systems, though the exact value for a hypothetical room-AC EER2 is unspecified by any standard found. The bug should be fixed but the proposed constant `1.0 / 0.92` needs revision before implementation.

### Proposed Fix Summary
1. **Confirm the correct conversion factor** before coding. The CEC table for split-system central ACs gives EER = EER2 × 1.043 (i.e., 1.0 / 0.9588 ≈ 1/0.96). For packaged units it is × 1.038. Since room ACs do not have a defined EER2 test procedure, and any EER2 value in an HPXML room-AC record likely originates from the central-AC Appendix M1 standard, the factor closest to the available evidence is `const EER2_TO_EER_FACTOR: f64 = 1.0 / 0.96` (≈ 4.2% uplift), not `1.0 / 0.92` as proposed.
2. **Split EER2 out of the pass-through arm** in `normalize_efficiency_units` (line 1772) and give it its own arm analogous to SEER2, using the verified factor.
3. **Audit `eer_from_params` at line 400**: once `normalize_efficiency_units` runs first (which `insert_annual_efficiency` at line 1736 ensures), the stored `cooling_efficiency_units` key will already be `"EER"`, so the `eq_ignore_ascii_case("EER2")` branch in `eer_from_params` is only hit for raw params not routed through `insert_annual_efficiency`. Confirm whether this path exists; if not, the EER2 branch in `eer_from_params` can be removed.
4. Do NOT use `1/0.92` as stated in the ticket — that factor is unsupported by any standard or conversion table found.

### Test Written
- **File**: `crates/hares-io/src/hpxml/resolve_hvac.rs` (within `#[cfg(test)] mod tests`)
- **Tests added**:
  - `eer2_is_not_treated_as_eer_passthrough` — asserts that `normalize_efficiency_units("EER2", 10.0)` returns label `"EER"` and a value > 10.0. Currently FAILS (label stays `"EER2"`, value unchanged), demonstrating the bug.
  - `eer_passthrough_unchanged` — asserts that `normalize_efficiency_units("EER", 10.0)` returns `("EER", 10.0)` unchanged. Currently PASSES (sanity check).
