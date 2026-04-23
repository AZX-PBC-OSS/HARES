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
