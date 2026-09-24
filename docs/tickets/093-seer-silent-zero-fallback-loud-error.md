# SEER Silent Zero Fallback Misclassifies EER-Only Cooling Systems

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-io/hpxml/resolve_hvac

## Problem

`apply_default_hvac_speed_fallback` in `crates/hares-io/src/hpxml/resolve_hvac.rs:1836` reads SEER with `.unwrap_or(0.0)` and then uses the resulting 0.0 to drive single-speed inference. An HPXML `CoolingSystem` element that provides `EnergyEfficiencyRatio` (EER) but not `SeasonalEnergyEfficiencyRatio` (SEER) — a valid configuration for room ACs and some legacy central units — silently has its speed inference forced to single-speed regardless of what the EER and CompressorType fields actually imply.

This violates `feedback_no_silent_defaults`: a missing field is silently substituted with a sentinel that then drives downstream inference. The user's data is misinterpreted with no diagnostic.

## Current Behavior

`crates/hares-io/src/hpxml/resolve_hvac.rs:1836`:
```rust
let seer = extract_seer(...).unwrap_or(0.0);
// ... downstream code uses seer == 0.0 as a single-speed signal
```

When SEER is absent and EER is present, the resolver assumes single-speed without checking EER. It also uses 0.0 — a non-physical efficiency value — in any subsequent computation that consumes `seer`.

## Required Behavior

1. If SEER is absent, attempt to derive a single-speed inference signal from EER (single-speed central ACs and room ACs are normally rated by EER alone; SEER is a seasonal aggregate that requires multi-condition test data).
2. If neither SEER nor EER is present, return a loud `HpxmlError::MissingField` identifying both candidate field paths. Do not substitute 0.0.
3. Never use 0.0 as a SEER value anywhere downstream. The variable type should be `Option<f64>` or the code path should return early on absence.

The general policy: missing efficiency data is a data-quality error to be surfaced to the caller, not a silent substitution that produces a wrong simulation.

## Approach

1. Replace the `.unwrap_or(0.0)` at `resolve_hvac.rs:1836` with explicit handling:
   - On `Some(seer)`, proceed as today.
   - On `None`, attempt EER lookup. If EER is present, derive the inference signal from EER (per AHRI 210/240, single-speed equipment is EER-rated).
   - If neither is present, return `HpxmlError::MissingField { field: "CoolingSystem/AnnualCoolingEfficiency", expected: "SEER or EER" }`.
2. Audit the rest of `apply_default_hvac_speed_fallback` and downstream resolver code for any other use of the SEER variable; ensure 0.0 cannot be propagated.
3. Add a fixture `tests/data/hpxml/cooling_system_eer_only.xml` exercising the EER-only path and assert correct inference.
4. Add a fixture missing both SEER and EER and assert the resolver errors with the expected field path.

## Definition of Done

- [ ] `unwrap_or(0.0)` on SEER removed from `resolve_hvac.rs:1836`
- [ ] EER-only `CoolingSystem` resolves to single-speed via EER (not via silent 0.0)
- [ ] Missing-both case returns `HpxmlError::MissingField` with both candidate paths
- [ ] No path in `resolve_hvac.rs` propagates SEER == 0.0 as a valid value
- [ ] Tests: EER-only fixture, missing-both fixture, SEER-present fixture (regression)
- [ ] `cargo test -p hares-io resolve_hvac` passes

## Verification

```bash
cargo test -p hares-io resolve_hvac
cargo test -p hares-io hpxml_parity
```

## References

- AHRI Standard 210/240-2023 *Performance Rating of Unitary Air-conditioning and Air-source Heat Pump Equipment* — single-speed central ACs are rated at the A test condition (EER); SEER aggregates A, B test points and requires multi-condition data.
- HPXML Specification v4.x §8.4 "Cooling Systems" — `EnergyEfficiencyRatio` (EER) and `SeasonalEnergyEfficiencyRatio` (SEER) are defined as alternative efficiency expressions; a `CoolingSystem` may provide one or both.
- Project policy `feedback_no_silent_defaults.md` — never substitute fallback values for missing/invalid input.

## Related Tickets

- 002-ideal-hvac-biquadratic-fallback (EIR/CAP curve evaluation; speed inference feeds into curve selection)
- 088-eer2-not-converted-to-eer-treated-as-identical (related EER conversion fix)
- 087-room-ac-shr-always-none-from-hpxml (related Room AC HPXML wiring)

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match — `apply_default_hvac_speed_fallback` is defined at `resolve_hvac.rs:1816` and the `.unwrap_or(0.0)` is at **line 1836** (exact match with ticket)
- [x] Described logic matches current implementation — confirmed. The function reads SEER via two paths (bare `efficiency_seer` key, or `cooling_efficiency` when `cooling_efficiency_units == "SEER"`) then calls `.unwrap_or(0.0)`. When SEER is absent, `seer = 0.0` drives `n_speeds = 1`. The function never consults `eer_from_params` (which exists at `resolve_hvac.rs:392–406`) even though EER data may be present in the map.
- [x] Bug is not already fixed — confirmed by `cargo test` results: `cooling_system_missing_both_seer_and_eer_produces_error` and `high_eer_central_ac_without_seer_resolves_via_eer_not_zero_sentinel` both fail against current code.
- [x] OCHRE cross-check: **diverges**. OCHRE `vendors/OCHRE/ochre/utils/hpxml.py:849` uses direct dict access `efficiency = hvac[f"Annual{hvac_type}Efficiency"]` — no fallback at all. If `AnnualCoolingEfficiency` is absent, OCHRE raises `KeyError` (loud failure). OCHRE `hpxml.py:861–876` infers speed from the converted `cop` value regardless of whether the original metric was EER or SEER, because both are converted to COP via the same `convert()` call before the speed thresholds are applied. HARES diverges by using a sentinel 0.0 instead of converting EER to the same basis.
- [x] EnergyPlus cross-check: **N/A** — EnergyPlus `ZoneHVAC:IdealLoadsAirSystem` does not perform HPXML parsing or SEER→n_speeds inference; that logic is specific to HARES/OCHRE's HPXML front-end. No EnergyPlus Engineering Reference passage is applicable to this parsing-layer decision.

### Web-Verified Citations

**Citation 1**
- **Citation**: "AHRI Standard 210/240-2023 — single-speed central ACs are rated at the A test condition (EER); SEER aggregates A, B test points and requires multi-condition data."
- **Source found**: Wikipedia "Seasonal energy efficiency ratio" (https://en.wikipedia.org/wiki/Seasonal_energy_efficiency_ratio); LearnMetrics EER vs SEER (https://learnmetrics.com/eer-vs-seer/); AHRI 210/240-2023 page (https://ahrinet.org/search-standards/ahri-210240-2023-2020-performance-rating-unitary-air-conditioning-air-source-heat-pump-equipment)
- **Quoted passage**: Wikipedia: *"The SEER is thus calculated with the same indoor temperature, but over a range of outside temperatures from 65 °F (18 °C) to 104 °F (40 °C), with a certain specified percentage of time in each of 8 bins spanning 5 °F (2.8 °C)."* LearnMetrics: *"SEER stands for Seasonal Energy Efficiency Ratio. It is calculated as a weighted average of different EER ratings (EER25%, EER50%, EER75%, and EER100%)."* EER test condition: *"95°F outdoor temperature. 80°F indoor temperature. 50% relative humidity levels."*
- **Verdict**: **Confirmed** that SEER is a seasonal aggregate across multiple temperature bins (multi-condition), while EER is a single-point measurement. **Partially correct** on scope: the 2023 standard now expresses metrics as SEER2/EER2 for new equipment (Appendix M1 test procedure), but the conceptual claim (SEER = multi-condition aggregate, EER = single-condition) remains accurate for both the legacy and current standards.
- **Correction note**: The ticket says "single-speed central ACs are rated at the A test condition (EER)." This is not accurate for units sold after January 1, 2023, which require SEER2 under DOE/AHRI 210/240-2023. However, many installed units in HPXML files pre-date 2023 and are legitimately EER-rated. The broader point — that EER is a valid primary efficiency metric for certain cooling systems in HPXML — is confirmed.

**Citation 2**
- **Citation**: "HPXML Specification v4.x §8.4 'Cooling Systems' — `EnergyEfficiencyRatio` (EER) and `SeasonalEnergyEfficiencyRatio` (SEER) are defined as alternative efficiency expressions; a `CoolingSystem` may provide one or both."
- **Source found**: OpenStudio-ERI workflow inputs documentation (https://openstudio-eri.readthedocs.io/en/v1.6.2/workflow_inputs.html); HEScore HPXML import documentation (https://hescore-hpxml.readthedocs.io/en/v7.0.2/translation/cooling_system.html)
- **Quoted passage**: OpenStudio-ERI: *"AnnualCoolingEfficiency[Units='SEER' or Units='SEER2']/Value"* for central ACs; *"AnnualCoolingEfficiency[Units='EER' or Units='CEER']/Value"* for room air conditioners. HEScore table: split_dx → SEER; packaged_dx (room AC) → EER.
- **Verdict**: **Partially correct**. The HPXML field is `AnnualCoolingEfficiency` with a `Units` sub-element — not two separate elements named `EnergyEfficiencyRatio` and `SeasonalEnergyEfficiencyRatio` as the ticket implies. The claim that EER and SEER are "alternative efficiency expressions" in `CoolingSystem` is correct. However, EER is primarily the required metric for **room ACs**, while central ACs require SEER or SEER2. An EER-only `CoolingSystem` is valid for a room AC; for a central AC it would be non-conformant with OpenStudio-HPXML's expected schema. The ticket's general claim that EER-only is a "valid configuration" is confirmed for room ACs. The additional claim of "some legacy central units" is plausible but not directly confirmed by the sources found.
- **Correction note**: The ticket cites "§8.4" but the HPXML specification section numbering could not be confirmed from publicly accessible sources. The substance of the claim is correct.

**Citation 3**
- **Citation**: "Project policy `feedback_no_silent_defaults.md`"
- **Source found**: No file at that path exists in the repository (`find /Users/rich/source/HARES -name "feedback_no_silent_defaults*"` returns empty). The MEMORY.md references this as a memory slug, not a file.
- **Quoted passage**: N/A — file not found.
- **Verdict**: **Cannot verify** as a file citation. The project policy concept is real (multiple other tickets reference it, and `HpxmlError::MissingField` is used extensively in `resolve_hvac.rs` and other resolvers for exactly this purpose), but `feedback_no_silent_defaults.md` is a memory key, not a document on disk.

### Legitimacy

- **Verdict**: **Legitimate**

- **Rationale**: The bug is real and confirmed at the exact location cited (line 1836). The code at `apply_default_hvac_speed_fallback` uses `.unwrap_or(0.0)` on a `Option<f64>` SEER extraction that will be `None` for any `CoolingSystem` whose `AnnualCoolingEfficiency` uses `Units=EER` (including all room ACs and EER-rated central units). The 0.0 sentinel then drives `n_speeds = 1` silently for the missing-both case, and for an EER-only system it produces the correct `n_speeds` coincidentally (EER ≤ 15 → n_speeds=1) but via the wrong mechanism and with no diagnostic. The observable severity is confirmed by `high_eer_central_ac_without_seer_resolves_via_eer_not_zero_sentinel` FAILING: EER=17 → n_speeds=1 (wrong; should be 2). The AHRI 210/240 citation for SEER being multi-condition is accurate. The HPXML citation for EER as an alternative efficiency unit is accurate for room ACs. The `eer_from_params` function already exists at line 392 and correctly handles both `efficiency_eer` and `cooling_efficiency` with `Units=EER/EER2`, so the fix has a ready-made building block.

### Proposed Fix Summary

1. Change `apply_default_hvac_speed_fallback` signature from `fn(...) -> ()` to `fn(...) -> Result<(), HpxmlError>` and propagate the return value at both call sites (lines 1399 and 1528).
2. Replace `.unwrap_or(0.0)` with explicit `Option` handling: call `seer_from_params`, then on `None` call `eer_from_params`, then on `None` return `Err(HpxmlError::MissingField { path: "CoolingSystem/AnnualCoolingEfficiency", system_kind: "Cooling System", ... })`.
3. When only EER is present, use the EER value as the speed-inference input (the thresholds 15/21 were calibrated against SEER, but EER and SEER are within ~10–15% for typical equipment; using EER directly is a defensible approximation and matches OCHRE's approach of converting both to COP before thresholding).
4. Ensure the 0.0 sentinel cannot propagate downstream: the local `seer` variable must be `f64` obtained from a real measurement, not a fallback.

### Test Written

- **File**: `crates/hares-io/tests/hpxml_parsing_tests.rs` (appended to existing file)
- **Tests added**:
  1. `eer_only_cooling_system_does_not_resolve_via_seer_zero_sentinel` — verifies EER is present and non-zero in resolved params, and no synthetic SEER is added; passes on current code (coincidental correctness for EER=10) but guards mechanism post-fix.
  2. `high_eer_central_ac_without_seer_resolves_via_eer_not_zero_sentinel` — **FAILS** on current code: EER=17 → n_speeds=1 (bug) vs expected 2 (after fix). This is the primary observable regression test.
  3. `cooling_system_missing_both_seer_and_eer_produces_error` — **FAILS** on current code: missing-both case silently succeeds with seer=0.0 sentinel instead of returning an error.
  4. `seer_present_cooling_system_resolves_normally_regression` — **passes** on current and post-fix code; guards that SEER=16 → n_speeds=2 continues to work.
