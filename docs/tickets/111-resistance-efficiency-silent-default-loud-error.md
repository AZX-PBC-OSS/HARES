# `resistance_efficiency_from_params` Silent 1.0 Default

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-io/hpxml/resolve_hvac

## Problem

`crates/hares-io/src/hpxml/resolve_hvac.rs:366-371` `resistance_efficiency_from_params` silently returns `1.0` when efficiency is absent in the params map. Electric resistance heating is conventionally modelled as 100% efficient, but using a silent default obscures whether the input data actually supplied an efficiency value. A misspelled key or absent field produces the same result as a correctly-supplied 1.0.

## Current Behavior

`crates/hares-io/src/hpxml/resolve_hvac.rs:366-371`:
```rust
fn resistance_efficiency_from_params(params: &Params) -> f64 {
    params.get("efficiency").copied().unwrap_or(1.0)
}
```

A typo (`"effciency"`) silently returns 1.0; an HPXML file missing the efficiency element silently returns 1.0; a correctly-supplied 1.0 returns 1.0. The three cases are indistinguishable.

## Required Behavior

Choose one:

A. **Cite the 1.0 default** — keep the default value but add an inline citation (ASHRAE HoF 2021 Ch. 33 "Furnaces" — electric resistance heating is by definition 100% efficient at the appliance) and emit a `tracing::debug!` when the default is taken so the omission is at least logged.

B. **Error loudly** — return `Result<f64, HpxmlError>`; missing efficiency for a resistance unit returns `HpxmlError::MissingField { field: "AnnualHeatingEfficiency", expected: "1.0 for electric resistance" }`.

Recommended path: B for HPXML strictness; the 1.0 value is so universally the right answer that requiring the input file to state it explicitly is a reasonable consistency check. If A is chosen instead, the citation must be inline and the debug log must include the equipment ID.

## Approach

1. Change the function signature to `fn resistance_efficiency_from_params(params: &Params) -> Result<f64, HpxmlError>`.
2. Return `Err(HpxmlError::MissingField { ... })` when the key is absent.
3. If a non-1.0 value is supplied, validate `0 < value <= 1.0` and return error otherwise.
4. Update callsites to propagate the error.
5. Update fixtures: ensure all electric resistance heaters in test HPXML files supply an explicit efficiency.

## Definition of Done

- [ ] `resistance_efficiency_from_params` returns `Result<f64, HpxmlError>`
- [ ] Missing key returns `HpxmlError::MissingField`
- [ ] Out-of-range value returns `HpxmlError::InvalidField`
- [ ] Callsites propagate the error
- [ ] HPXML fixtures supply explicit efficiency
- [ ] Inline comment cites ASHRAE HoF 2021 Ch. 33 for the conventional 1.0 value

## Verification

```bash
cargo test -p hares-io resolve_hvac resistance
cargo test -p hares-io hpxml_parity
```

## References

- ASHRAE Handbook of Fundamentals 2021 Ch. 33 "Furnaces" — electric resistance heating is by definition 100% efficient at the appliance (all electrical input becomes heat).
- HPXML Specification v4.x §8.4 "Heating Systems" — `AnnualHeatingEfficiency` element with `Units` and `Value` children; required for electric resistance.
- Project policy `feedback_no_silent_defaults.md`.

## Related Tickets

- 015-er-on-off-modeling
- 077-hpxml-backup-efficiency-units-ignored

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match (corrected: ticket quotes simplified pseudo-code; actual function is at lines 365–372, not 366–371 — one-line shift due to doc-comment. Core logic is identical.)
- [x] Described logic matches current implementation — `unwrap_or(1.0)` silent default is present at line 371.
- [x] OCHRE cross-check result: **matches** — `vendors/OCHRE/ochre/Equipment/HVAC.py:1202` uses `kwargs.get("Backup EIR (-)", 1)` (same silent-default pattern). OCHRE's HPXML parser (`ochre/utils/hpxml.py:963`) does require the HPXML value when setting the backup EIR: `"Backup EIR (-)": 1 / backup_cop`, but the equipment class itself falls back to EIR=1 if the key is absent from kwargs. HARES's behavior is consistent with OCHRE — both silently default.
- [x] EnergyPlus cross-check result: **matches** — EnergyPlus Engineering Reference (v9.3 and v24.2, via bigladdersoftware.com) states for `ZoneHVAC:Baseboard:Convective:Electric`: *"the efficiency is a required input that defaults to unity"* and the energy consumption formula is `Energy_electric = Heating_baseboard / Efficiency`. EnergyPlus treats efficiency=1.0 as the correct physical value but does require the field to be specified (defaulting it internally). The physics is correct; the silent-default pattern is what's at issue.

### Web-Verified Citations

**Citation 1**: ASHRAE HoF 2021 Ch. 33 "Furnaces" — electric resistance heating is by definition 100% efficient at the appliance.

- **Source found**: ASHRAE official table of contents — https://resourcecenter.ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals
- **Quoted passage**: Chapter 33 of the 2021 ASHRAE **Handbook of Fundamentals** is titled **"Physical Properties of Materials"** — not "Furnaces".
- **Verdict**: **Incorrect** — The ticket misidentifies the volume. "Furnaces" appears in Chapter 33 of the **ASHRAE Handbook of HVAC Systems and Equipment (2020)**, not the Handbook of Fundamentals. The ticket citation "ASHRAE HoF 2021 Ch. 33" is wrong on two counts: (a) "HoF" conventionally denotes Handbook of Fundamentals, not Systems and Equipment; (b) the 2021 Fundamentals Ch. 33 is "Physical Properties of Materials". The correct citation is: **ASHRAE Handbook — HVAC Systems and Equipment 2020, Chapter 33 "Furnaces"** (confirmed via ashraepyramids.org/page.php?id=232). The underlying physical claim — that electric resistance heating is 100% efficient at the appliance — is scientifically correct and universally accepted; only the bibliographic citation is wrong.

**Citation 2**: HPXML Specification v4.x §8.4 "Heating Systems" — `AnnualHeatingEfficiency` element with `Units` and `Value` children; required for electric resistance.

- **Source found**: HPXML Data Dictionary v4.0.0 — https://hpxml.nlr.gov/datadictionary/4.0.0/Building/BuildingDetails/Systems/HVAC/HVACPlant/HeatingSystem/AnnualHeatingEfficiency
- **Quoted passage**: "Min Occurances: 0" — the element is optional, not required, in the schema.
- **Verdict**: **Partially correct** — `AnnualHeatingEfficiency` has `Units` and `Value` children as stated, but the HPXML schema marks it as optional (`Min Occurances: 0`). It is not schema-required for electric resistance systems. The OpenStudio-HPXML reference implementation supplies it explicitly in all sample files (confirmed in `vendors/OCHRE/ochre/defaults/Input Files/BEopt_example.xml:712`: `<BackupAnnualHeatingEfficiency><Units>Percent</Units><Value>1.0</Value></BackupAnnualHeatingEfficiency>` and in `tests/fixtures/parity/cz6b_resistance_res_wh/building.xml:709–712`) but does not require it per schema. This supports Option B (error loudly as a HARES strictness policy rather than schema enforcement).

**Citation 3**: Project policy `feedback_no_silent_defaults.md`

- **Source found**: Referenced in 41 files across the project (confirmed via grep), including all existing `silent_default_regressions.rs` tests.
- **Quoted passage**: N/A — the file path `feedback_no_silent_defaults.md` is a convention referenced in ticket text; the policy is evidenced by the existence of `crates/hares-io/tests/silent_default_regressions.rs` and the `MissingField` variant in `HpxmlError` (confirmed at `crates/hares-io/src/hpxml/mod.rs:51–61`).
- **Verdict**: **Confirmed** — the project policy against silent defaults is real and actively enforced. Gas Furnace and Gas Boiler already return `Err(HpxmlError::MissingField)` for missing AFUE (confirmed at `resolve_hvac.rs:623–628`). Electric resistance equipment is the remaining gap.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: The core issue is real and unfixed: `resistance_efficiency_from_params` (lines 365–372) does silently `unwrap_or(1.0)` when `AnnualHeatingEfficiency` is absent from an electric resistance heating system, while the analogous gas-combustion path (Gas Furnace, Gas Boiler) already returns `HpxmlError::MissingField`. This inconsistency is confirmed in the code and is genuinely a policy violation. The ticket's code snippet is a slightly simplified rendering (uses `"efficiency"` key and `Params` alias instead of the actual `"heating_efficiency"` key and `Map<String, Value>`), but correctly describes the behavior. The ASHRAE citation (Definition of Done item: "ASHRAE HoF 2021 Ch. 33") is bibliographically incorrect — Chapter 33 of the 2021 Fundamentals handbook is "Physical Properties of Materials", not "Furnaces"; the correct source is ASHRAE HVAC Systems and Equipment 2020 Ch. 33 "Furnaces". The underlying physics claim (electric resistance is 100% efficient at the appliance) is correct. The proposed fix (Option B — error loudly) is consistent with existing project precedent.

### Proposed Fix Summary

1. Change `fn resistance_efficiency_from_params(params: &Map<String, Value>) -> f64` to return `Result<f64, HpxmlError>`.
2. When neither `"heating_efficiency"` nor `"efficiency_cop"` is found in params, return `Err(HpxmlError::MissingField { path: "HeatingSystem/AnnualHeatingEfficiency", system_kind: <"Electric Furnace" | "Electric Boiler" | "Electric Baseboard">, ... })`.
3. Optionally validate that a supplied value is `0 < v <= 1.0` (since EIR=value here, not 1/COP), returning `HpxmlError::InvalidField` for out-of-range.
4. Update the three callsites (lines 575 and 673 for Electric Furnace and Electric Boiler; `try_build_electric_baseboard_config` at line 708 does NOT call this function — it hardcodes `eir: 1.0` directly, so it also needs addressing).
5. Update the inline ASHRAE citation to: "ASHRAE Handbook — HVAC Systems and Equipment 2020, Ch. 33 'Furnaces'".
6. Add explicit `AnnualHeatingEfficiency` to any HPXML fixtures that lack it (only `cz6b_resistance_res_wh/building.xml` was found to have it already; check others).

**Note on `try_build_electric_baseboard_config`**: Line 724 hardcodes `eir: 1.0` without calling `resistance_efficiency_from_params` at all — this is a second instance of the same silent-default pattern not mentioned in the ticket.

### Test Written

- **File**: `crates/hares-io/tests/silent_default_regressions.rs`
- **What it tests**:
  - `electric_furnace_missing_efficiency_errors` — `#[ignore]`d failing regression: Electric Furnace without `AnnualHeatingEfficiency` must produce `HpxmlError::MissingField`. Currently fails (parse succeeds silently). Remove `#[ignore]` once fixed.
  - `electric_furnace_with_explicit_efficiency_resolves` — happy path: Electric Furnace with explicit `<Units>Percent</Units><Value>1.0</Value>` must resolve to an "Electric Furnace" spec. Passes now.
  - `electric_resistance_baseboard_missing_efficiency_errors` — `#[ignore]`d failing regression: ElectricResistance baseboard without `AnnualHeatingEfficiency` must produce `HpxmlError::MissingField`. Currently fails.
  - `electric_resistance_baseboard_with_explicit_efficiency_resolves` — happy path: ElectricResistance with explicit efficiency must resolve. Passes now.
