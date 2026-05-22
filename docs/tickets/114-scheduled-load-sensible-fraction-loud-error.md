# `ScheduledLoad.sensible_gain_fraction` Silent 0.5 Default

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-equipment/scheduled_load

## Problem

`crates/hares-equipment/src/scheduled_load.rs:262-267` `sensible_gain_fraction` falls back to `0.5` when the field is absent in the config, with only a `tracing::warn!`. The 50/50 sensible/latent split is an arbitrary pick — the actual fraction depends on the load type (lighting is ~100% sensible, dishwasher exhaust is ~30% sensible, etc.). Substituting 0.5 silently for any load type biases the latent/sensible balance and downstream HVAC sizing.

## Current Behavior

`crates/hares-equipment/src/scheduled_load.rs:262-267`:
```rust
let sensible_fraction = config.sensible_gain_fraction.unwrap_or_else(|| {
    tracing::warn!("sensible_gain_fraction not specified; defaulting to 0.5");
    0.5
});
```

Warning logs, simulation continues with a wrong split.

## Required Behavior

1. `sensible_gain_fraction` must be required at the config level. Return `Err(EquipmentError::MissingField { field: "sensible_gain_fraction" })` when absent.
2. Validate `0.0 <= value <= 1.0`; return `Err` otherwise.
3. The accompanying `latent_gain_fraction` (if present) must satisfy `sensible + latent <= 1.0`; the residual is convective-only loss to surroundings.
4. Update all `ScheduledLoad` fixtures to supply an explicit value with a citation comment.

## Approach

1. Open `crates/hares-equipment/src/scheduled_load.rs:262-267` and replace the fallback with an explicit error.
2. Add validation for the 0–1 range.
3. Audit all `ScheduledLoad` configs in fixtures and test code; add explicit `sensible_gain_fraction` values with inline citations to the source for the load type (e.g. ASHRAE HoF 2021 Ch. 18 Table 5 for residential appliance heat gains).
4. Add a unit test asserting the error fires when the field is absent.
5. Add a unit test asserting the validation fires for out-of-range values.

## Definition of Done

- [ ] Silent 0.5 default removed from `scheduled_load.rs:262-267`
- [ ] Missing field returns `EquipmentError::MissingField`
- [ ] Out-of-range value returns `EquipmentError::InvalidField`
- [ ] All fixtures supply explicit `sensible_gain_fraction` with inline source citations
- [ ] Unit tests cover missing field and out-of-range value cases
- [ ] No `ScheduledLoad` callsite produces an implicit 0.5 split

## Verification

```bash
cargo test -p hares-equipment scheduled_load
cargo test -p hares-core dwelling
```

## References

- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.2 "Internal Heat Gains" Tables 1-5 — sensible and latent fractions for residential appliances and lighting.
- EnergyPlus Engineering Reference §3.6.3 "ZoneHVAC:EquipmentList" — sensible and latent fractions are explicit inputs per equipment.
- Project policy `feedback_no_silent_defaults.md`.

## Related Tickets

- feedback_equipment_self_contained
- 001-unify-hfg-add-humidity-port (related humidity port handling)

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match — the `unwrap_or_else` fallback that emits `tracing::warn!` and returns `0.5` is at lines **262–268** of `crates/hares-equipment/src/scheduled_load.rs`, exactly as cited.
- [x] Described logic matches current implementation — the fallback chain (explicit key → `frac_sensible` alias → convective+radiative sum → 0.5 default with warn) is present verbatim.
- [x] OCHRE cross-check result: **diverges** — OCHRE `Equipment.py:83-86` computes `sensible_gain_fraction = convective_gain + radiative_gain` and defaults to **0.0** when neither fraction is provided (no `warn`, no 0.5). File: `vendors/OCHRE/ochre/Equipment/Equipment.py` lines 83–86. HARES's 0.5 fallback is an intentional "safer" default than OCHRE's 0.0 (which would silently drop all heat gain to the zone), but the ticket is correct that 0.5 is still an arbitrary choice that should be rejected.
- [x] EnergyPlus cross-check result: **diverges (by design)** — EnergyPlus `ElectricEquipment` and `OtherEquipment` objects do not have an explicit `sensible_fraction` input. Instead they take `Fraction Latent`, `Fraction Radiant`, and `Fraction Lost`; the convective-sensible fraction is the residual (1 − latent − radiant − lost). Default values per the Energy+.idd are `Fraction Latent = 0`, `Fraction Radiant = 0` (source: Unmet Hours community thread quoting the IDD: https://unmethours.com/question/44022/default-latent-and-radiant-fraction-for-equipment-and-lights/), implying a default of 100% sensible — NOT 50%. EnergyPlus Engineering Reference (latest, v25.2) Zone Internal Gains page (https://bigladdersoftware.com/epx/docs/25-2/engineering-reference/zone-internal-gains.html) confirms sensible/latent handling per-equipment object but contains no §3.6.3 section number and no reference to `ZoneHVAC:EquipmentList` sensible/latent fraction inputs (see citation verdict below).

### Web-Verified Citations

**Citation 1**
- **Citation**: "ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.2 'Internal Heat Gains' Tables 1-5 — sensible and latent fractions for residential appliances and lighting."
- **Source found**: ASHRAE Table of Contents 2021 (https://www.ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals) and official ASHRAE Chapter 17 online (https://handbook.ashrae.org/Handbooks/F21/IP/F21_Ch17/F21_Ch17_ip.aspx).
- **Quoted passage**: ASHRAE ToC entry: **"18. Nonresidential Cooling and Heating Load Calculations"**. The residential chapter is **Chapter 17** ("Residential Cooling and Heating Load Calculations"). Chapter 17 does not tabulate appliance-level sensible/latent fractions; it uses aggregate coefficients: `qig,s = 464 + 0.7·Acf + 75·Noc` and `qig,l = 68 + 0.07·Acf + 41·Noc`. The chapter "does not provide itemized tables breaking down sensible/latent contributions by specific appliances, lighting types, or equipment categories." Chapter 18 covers **commercial** appliances (Table 5 lists commercial cooking equipment such as ranges, ovens, steamers — not residential dishwashers or plug loads).
- **Verdict**: **Incorrect**. The ticket's ASHRAE citation has the wrong chapter number for residential loads (should be Ch. 17, not Ch. 18), and the correct chapter does not contain the per-appliance Tables 1–5 the ticket implies. The underlying physics principle (lighting is ~100% sensible, dishwasher is ~30% sensible) is correct — it is sourced from OCHRE's `hpxml.py` (which itself cites NREL/RESNET), not from an ASHRAE per-appliance table in Ch. 18. The correct citation for residential appliance gain fractions is the OCHRE/NREL HPXML parser defaults documented in `vendors/OCHRE/ochre/utils/hpxml.py`.

**Citation 2**
- **Citation**: "EnergyPlus Engineering Reference §3.6.3 'ZoneHVAC:EquipmentList' — sensible and latent fractions are explicit inputs per equipment."
- **Source found**: EnergyPlus Engineering Reference Zone Internal Gains (v25.2): https://bigladdersoftware.com/epx/docs/25-2/engineering-reference/zone-internal-gains.html; EnergyPlus I/O Reference Internal Gains (v8.7): https://bigladdersoftware.com/epx/docs/8-7/input-output-reference/group-internal-gains-people-lights-other.html.
- **Quoted passage**: The EnergyPlus Engineering Reference (v25.2) does **not use numerical section identifiers** (no §3.6.3 exists). `ZoneHVAC:EquipmentList` controls zone HVAC equipment sequencing; it does not define per-equipment sensible/latent fraction fields. Per-equipment fractions are inputs on `ElectricEquipment` / `OtherEquipment` objects as `Fraction Latent`, `Fraction Radiant`, `Fraction Lost`. The sensible (convective) fraction is the residual: `fconvected = 1.0 – (Fraction Latent + Fraction Radiant + Fraction Lost)`. Defaults: `Fraction Latent = 0`, `Fraction Radiant = 0` (IDD default, confirmed via https://unmethours.com/question/44022/default-latent-and-radiant-fraction-for-equipment-and-lights/).
- **Verdict**: **Incorrect**. Section §3.6.3 does not exist in the EnergyPlus Engineering Reference with that number or title. The claim that `ZoneHVAC:EquipmentList` carries per-equipment sensible/latent fractions is wrong — those fields belong to the individual equipment load objects. The underlying point (EnergyPlus requires explicit fraction inputs per equipment type) is **correct in spirit**: EnergyPlus's I/O Reference shows these as explicit fields with a default of 0, meaning the user must know that omitting them yields 100% sensible (not 50%). This actually strengthens the ticket's core argument.

**Citation 3**
- **Citation**: "Project policy `feedback_no_silent_defaults.md`."
- **Source found**: Memory index at `/Users/rich/.claude/projects/-Users-rich-source-HARES/memory/MEMORY.md` — not checked as a web citation; this is an internal project policy reference.
- **Quoted passage**: N/A (internal project file).
- **Verdict**: **Cannot independently verify via web** — internal policy document, not a published standard. No impact on the technical legitimacy of the ticket.

### Legitimacy

- **Verdict**: **Partially Legitimate**

- **Rationale**: The core bug is real and confirmed: `crates/hares-equipment/src/scheduled_load.rs:262–268` silently falls back to `0.5` when no sensible fraction is supplied by the caller, emitting only a `tracing::warn!`. The ticket's physics examples are correct (lighting ~100% sensible, dishwashers ~30%). The OCHRE cross-check confirms that OCHRE itself defaults to 0.0 (not 0.5), making 0.5 an unanchored HARES-specific invention. However, both ASHRAE and EnergyPlus citations are **inaccurate**: Ch. 18 is nonresidential (Ch. 17 is residential, and it uses aggregate equations rather than per-appliance tables), and §3.6.3 of the EnergyPlus Engineering Reference does not exist. The out-of-range validation issue (ticket item 2) is **partially already implemented** — values >1.0 are already rejected by the `sensible + latent > 1.0` check at line 293; what is missing is an explicit `sensible_gain_fraction > 1.0` guard and a direct `MissingField` error for the absent-value case.

### Proposed Fix Summary

Replace the `unwrap_or_else` at lines 262–268 of `scheduled_load.rs` with a hard error: if none of the three resolution strategies (explicit key, HPXML alias, convective+radiative sum) produce a value, return `Err(HaresError::Equipment("sensible_gain_fraction missing for '<name>'; must be specified explicitly".to_string()))`. Add a direct `sensible_gain_fraction > 1.0` guard after the existing `< 0.0` check. Audit all `ScheduledLoad` configs in `hares-io/src/hpxml/` (which already compute and inject explicit fractions via `default_gain_fractions`) and in `crates/hares-equipment/tests/lifecycle.rs` (tests already supply explicit values). The existing `missing_sensible_gain_fraction_uses_conservative_half` test must be updated to assert `is_err()` instead of `is_ok()`.

### Test Written

- **File**: `crates/hares-equipment/src/scheduled_load.rs` (inline `#[cfg(test)]` module, appended after `missing_sensible_gain_fraction_uses_conservative_half`)
- **What it tests**:
  1. `missing_sensible_gain_fraction_returns_err` — asserts `init()` returns `Err` when `sensible_gain_fraction` is absent from config. **Currently FAILS** (init returns Ok with 0.5). Will pass once the fallback is replaced with `MissingField`.
  2. `out_of_range_sensible_gain_fraction_returns_err` — asserts `init()` returns `Err` for `sensible_gain_fraction = 1.5`. **Currently PASSES** because the existing `sensible + latent > 1.0` check at line 293 already catches this. Added as documentation of expected behavior.
