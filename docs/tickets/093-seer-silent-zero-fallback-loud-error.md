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
