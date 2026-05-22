# `compressor_type_to_mode` Wildcard Must Warn and Fail Closed

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-io/hpxml/resolve_hvac

## Problem

`crates/hares-io/src/hpxml/resolve_hvac.rs:1783-1785` `compressor_type_to_mode` uses a `_ => "single_speed"` catch-all to map unknown HPXML `CompressorType` values. Project policy explicitly bans `_ =>` catch-alls (see project memory) and the silent default produces wrong speed-mode inference for any new HPXML CompressorType variant added in a future spec revision.

## Current Behavior

`crates/hares-io/src/hpxml/resolve_hvac.rs:1783-1785`:
```rust
match compressor_type {
    "single stage" | "SingleStage" => "single_speed",
    "two stage" | "TwoStage" => "two_speed_setpoint",
    "variable speed" | "VariableSpeed" => "variable_speed_ideal",
    _ => "single_speed",  // wildcard — silent default
}
```

A future HPXML CompressorType (e.g. "DualStage") silently maps to `"single_speed"` with no diagnostic.

## Required Behavior

1. Replace the wildcard with explicit handling: emit `tracing::warn!` listing the unknown compressor type and the equipment ID, then return `Err(HpxmlError::InvalidField { field: "CompressorType", value })`.
2. Match arms must be exhaustive over the known HPXML CompressorType enum values; any unknown value is an error.
3. If a future HPXML revision adds a new variant, the resolver fails closed (errors at parse time) rather than silently miscategorising the equipment.

## Approach

1. Define an `HpxmlCompressorType` enum mirroring the HPXML 4.x specification values.
2. Parse the raw string into the enum; an unparseable string returns `HpxmlError::InvalidField`.
3. Match the enum exhaustively in `compressor_type_to_mode`. Each arm maps to the appropriate HARES `SpeedControlMode`.
4. No `_ =>` arm. If the HPXML enum is extended in a future spec, the compiler will flag the missing match arm.
5. Add a fixture exercising the unknown-string case and assert the error.

## Definition of Done

- [ ] `_ =>` wildcard removed from `compressor_type_to_mode`
- [ ] `HpxmlCompressorType` enum defined with all HPXML 4.x variants
- [ ] String parsing returns `Err(HpxmlError::InvalidField)` for unknown values
- [ ] Match in `compressor_type_to_mode` is exhaustive over the enum
- [ ] Fixture exercises the unknown-string case
- [ ] Unit test asserts error variant and message
- [ ] No other `_ =>` catch-all in `resolve_hvac.rs` (audit complete)

## Verification

```bash
cargo test -p hares-io resolve_hvac compressor_type
cargo test -p hares-io hpxml_parity
rg "_ =>" crates/hares-io/src/hpxml/   # audit
```

## References

- HPXML Specification v4.x §8.4 "Cooling Systems" and "Heat Pumps" — `CompressorType` enumeration: SingleStage, TwoStage, VariableSpeed.
- Project policy: no `_ =>` catch-alls in match arms (per project memory).
- Project policy `feedback_no_silent_defaults.md`.

## Related Tickets

- 007-unify-variable-speed-selection
- 093-seer-silent-zero-fallback-loud-error
- 095-backup-switchover-temp-er-lockout-only

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] **Line numbers**: Ticket cites `1783-1785`; the function `compressor_type_to_mode` is actually at lines **1779-1786** (function starts at 1779, wildcard arm is at 1784, closing brace at 1786). The line range is slightly off but the correct function is found at the cited location.
- [x] **Logic matches**: The wildcard `_ => "single_speed"` is present and active at line 1784. Confirmed by reading `crates/hares-io/src/hpxml/resolve_hvac.rs:1779-1786`.
- [x] **Bug confirmed by test**: Running the regression test with `--include-ignored` shows `'DualStage'` silently resolves to `speed_control_mode=Some(String("single_speed"))` rather than returning an error. The bug is not yet fixed.

**Actual current code (lines 1779–1786):**
```rust
fn compressor_type_to_mode(compressor_type: &str) -> &'static str {
    match compressor_type.trim().to_ascii_lowercase().as_str() {
        "single stage" => "single_speed",
        "two stage" => "two_speed",
        "variable speed" => "variable_speed",
        _ => "single_speed",  // wildcard — silent default  ← BUG
    }
}
```

**Discrepancy from ticket's code snippet**: The ticket shows `"SingleStage"`, `"TwoStage"`, `"VariableSpeed"` as separate arms; the actual code does not have those PascalCase variants — it normalizes with `.to_ascii_lowercase()` before matching and only has the lowercase-with-space forms. The ticket's snippet is illustrative, not verbatim. The bug (`_ => "single_speed"`) is real regardless.

**OCHRE cross-check**: `vendors/OCHRE/ochre/utils/hpxml.py:862-870` — OCHRE uses a `speed_options` dict `{"single stage": 1, "two stage": 2, "variable speed": 4}` with an `elif hvac.get("CompressorType") in speed_options:` guard. If the CompressorType is absent from the dict (i.e. unknown), OCHRE falls through to a SEER-based heuristic (`elif cop <= 15: 1 … elif cop <= 21: 2 … else: 4`). HARES diverges: it does not fall through to the SEER heuristic for unknown strings; it maps them silently to `single_speed`. OCHRE's handling of unknown CompressorType values is also a silent default (different default, same symptom). Neither implementation fails closed; HARES is slightly worse because the SEER fallback at least uses physical data.

**EnergyPlus cross-check**: EnergyPlus Engineering Reference does not define a `CompressorType` field in the same schema sense — EnergyPlus uses equipment-type enumerations internally. The HPXML CompressorType field is part of the OS-HPXML/ResStock layer that maps to EnergyPlus inputs. No EnergyPlus source citation is relevant here; this is an HPXML parsing concern.

### Web-Verified Citations

**Citation 1**: "HPXML Specification v4.x §8.4 "Cooling Systems" and "Heat Pumps" — `CompressorType` enumeration: SingleStage, TwoStage, VariableSpeed."

- **Source found**: HPXML Data Dictionary v4.0.0 at `https://hpxml.nlr.gov/datadictionary/4.0.0/Building/BuildingDetails/Systems/HVAC/HVACPlant/CoolingSystem/CompressorType` and v4.2.0 at the same path structure.
- **Quoted passage**: "CompressorType element has three allowed enumeration values: 1. variable speed, 2. two stage, 3. single stage" (from fetched HPXML Data Dictionary v4.0.0 and v4.2.0, both versions consistent).
- **Verdict**: **Partially correct**. The enumeration values are real and confirmed (`single stage`, `two stage`, `variable speed`). However the ticket's names are wrong: the HPXML spec uses lowercase-with-space strings (`"single stage"`, `"two stage"`, `"variable speed"`), **not** PascalCase identifiers (`SingleStage`, `TwoStage`, `VariableSpeed`). The three-value enumeration is stable across v4.0.0 and v4.2.0 — no new variants have been added in recent spec revisions. The "future HPXML revision adds DualStage" scenario is speculative but structurally valid as a risk.

**Citation 2**: "Project policy: no `_ =>` catch-alls in match arms (per project memory). Project policy `feedback_no_silent_defaults.md`."

- **Source found**: The `silent_default_regressions.rs` test file (`crates/hares-io/tests/silent_default_regressions.rs:1-8`) documents this policy inline: *"Per project rule: no silent substitution of engineering defaults."* The pattern is also embodied across multiple `HpxmlError::MissingField` usages in the codebase.
- **Quoted passage**: `"Regression tests: every field that used to be silently defaulted must now error loudly with a HpxmlError::MissingField carrying the HPXML path, system kind, and system id."` — `silent_default_regressions.rs:1-3`
- **Verdict**: **Confirmed**. The project policy against silent defaults is consistently enforced in the test suite.

### Legitimacy

- **Verdict**: **Partially Legitimate**

- **Rationale**: The core bug is real and confirmed by live test execution: `compressor_type_to_mode` at `resolve_hvac.rs:1784` silently maps any unknown `CompressorType` string to `"single_speed"` via a `_ =>` wildcard. This violates the project's explicit no-silent-defaults policy and the HPXML spec (which defines exactly three valid values). The HPXML v4.x enumeration is confirmed to have exactly three values (`single stage`, `two stage`, `variable speed`) via the NREL HPXML Data Dictionary v4.0.0 and v4.2.0. The ticket is deducted from "Legitimate" to "Partially Legitimate" for two inaccuracies: (1) the line numbers are slightly off (the wildcard is at line 1784, not 1783; the function spans 1779-1786); (2) the code snippet shows PascalCase variants (`"SingleStage"`, `"TwoStage"`, `"VariableSpeed"`) that do not exist in the actual code — the implementation uses `.to_ascii_lowercase()` normalization and lowercase-with-space arms only; and (3) `HpxmlError::InvalidField` does not exist — the correct variant to add or use would be a new variant or `HpxmlError::Parse(...)`.

### Proposed Fix Summary

1. Add a new `HpxmlError::InvalidField { field: &'static str, value: String, system_id: String }` variant to `hpxml/mod.rs` (or reuse `Parse` if a new variant is undesired).
2. Change `compressor_type_to_mode` to return `Result<&'static str, HpxmlError>` and replace `_ => "single_speed"` with a `tracing::warn!` call followed by `Err(HpxmlError::InvalidField { … })`.
3. Propagate the `?` up through `insert_mode_and_speed_metadata` (change its signature to return `Result<(), HpxmlError>`) and all callers in `resolve_hvac.rs`.
4. No PascalCase re-parsing needed — the lowercase normalization already handles case variation; the only gap is the catch-all.

### Test Written

- **File**: `crates/hares-io/tests/silent_default_regressions.rs` (appended at line 771)
- **What it tests**:
  - `unknown_compressor_type_errors_not_silently_defaults` — `#[ignore]` (fails until fix applied): confirms that `CompressorType=DualStage` currently silently resolves to `speed_control_mode="single_speed"` and will need to return `Err` after the fix. Verified to **FAIL** when run with `--include-ignored`.
  - `known_compressor_types_resolve_cleanly` — **passing now**: confirms all three HPXML-spec values (`single stage`, `two stage`, `variable speed`) map correctly to `single_speed`/1, `two_speed`/2, `variable_speed`/4 respectively. Must continue to pass after the fix.
