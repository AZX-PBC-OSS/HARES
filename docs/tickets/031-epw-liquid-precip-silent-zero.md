# EPW Liquid Precipitation: Silent Zero Default for Missing Field 33

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-io/epw
**Weather-cluster note**: This ticket belongs adjacent to the 024–034 weather pipeline cluster. Do not move — renumber when the cluster is sequenced.

## Problem

The EPW parser at `hares-io/src/epw.rs:202–209` reads liquid precipitation from EPW field index 33 (zero-indexed; the 34th field). Two distinct silent-default violations exist, both prohibited by project policy `feedback_no_silent_defaults.md`:

1. **Field absent (row has fewer than 34 fields)**: Substitutes 0.0 silently. Many EPW files (older TMY3-derived EPWs, some CWEC files) have 24 or 30 fields; field 33 (liquid precipitation depth) was added in EPW 2.0. When absent, 0.0 is physically plausible but cannot be distinguished from missing data. No diagnostic is emitted.

2. **Parse error or sentinel on present field**: `parse_f64(...).unwrap_or(0.0)` silently maps parse failures and the EPW missing-data sentinel (9999, per EnergyPlus Input-Output Reference §"EPW Data Dictionary") to 0.0. Substituting 0.0 for a 9999 sentinel incorrectly implies no precipitation when the record is simply absent.

```rust
let liquid_precip_m = if fields.len() > IDX_LIQUID_PRECIP_DEPTH_MM {
    parse_f64(fields[IDX_LIQUID_PRECIP_DEPTH_MM], row, "liquid_precip_mm")
        .unwrap_or(0.0)
        .max(0.0)
        / 1000.0
} else {
    0.0
};
```

Both branches emit nothing. Callers have no telemetry signal about what they are receiving.

## Current Behavior

`hares-io/src/epw.rs:202–209`: silent 0.0 substitution for absent field, parse errors, and 9999 sentinel. No diagnostic output in any case.

## Required Behavior

1. **Field absent (row < 34 fields)**: Emit a single `tracing::debug!` at file-parse time — not per-row — stating: "EPW file has no precipitation data (field 33 absent); liquid_precip_m set to 0.0 for all rows". Track absence across the loop; emit once after parsing completes.

2. **Field present, value ≥ 9 000 (EPW missing-data sentinel)**: Emit a single `tracing::debug!` per file stating sentinel was detected; treat as 0.0. Use `value >= 9000.0` to cover 9999 and common EPW sentinel variants (per EnergyPlus Input-Output Reference §"EPW Data Dictionary" — sentinel = 9999 for precipitation depth).

3. **Field present, parse error**: Emit `tracing::warn!` with row number and raw field value; substitute 0.0. Do not error-out — 0.0 is the only defensible physical substitute, and precipitation is not consumed by the thermal solver.

Do not emit per-row diagnostics for absence or sentinel (file-level once is sufficient). Do not add a silent fallback without the diagnostic.

Reference: EnergyPlus Input-Output Reference §"EPW Data Dictionary" — field 33 (Liquid Precipitation Depth, mm), missing-value indicator = 9999; EPW format specification v2.0 (EnergyPlus 9.6 release) — field count and sentinel encoding.

## Approach

In the row-parse closure inside `parse_epw`:
- Replace `unwrap_or(0.0)` with explicit match arms.
- Track file-level flags (`precip_field_absent: bool`, `precip_sentinel_seen: bool`) during the parsing loop.
- After the loop, emit the two file-level `tracing::debug!` messages if the flags are set.
- Inside the loop, emit `tracing::warn!` per-row only for parse errors.

Named functions to modify: the row-parse closure in `parse_epw` that builds `EpwRecord`, and the post-loop summary path.

## Definition of Done

- [ ] EPW field 33 absent across all rows → single `tracing::debug!` at file level after parsing, not per-row
- [ ] EPW field 33 present with value ≥ 9 000 → treated as 0.0 with single `tracing::debug!` at file level
- [ ] EPW field 33 present with parse error → `tracing::warn!` with row number and raw value, then 0.0
- [ ] `cargo test -p hares-io epw` passes with tests for each of the three cases

## Verification

```bash
cargo test -p hares-io epw
```

## References

- EnergyPlus Input-Output Reference §"EPW Data Dictionary" — field 33 (Liquid Precipitation Depth, mm), missing value = 9999
- EPW format specification v2.0 (EnergyPlus 9.6 release) — field count and sentinel encoding
- Project policy `feedback_no_silent_defaults.md` — never silently substitute fallback values

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-20

### Code Confirmation
- [x] Referenced line numbers still match — `IDX_LIQUID_PRECIP_DEPTH_MM: usize = 33` at line 45; the precipitation block is at lines 202–209 (ticket cites 202–209 ✓)
- [x] Described logic matches current implementation — both silent-zero branches confirmed present: `unwrap_or(0.0)` for parse errors/sentinel, and `else { 0.0 }` for absent field
- [x] OCHRE cross-check result: **N/A** — OCHRE delegates entirely to `pvlib.iotools.read_epw()` (`vendors/OCHRE/ochre/utils/schedule.py:169`) and explicitly discards all columns not in its 8-field `WEATHER_NAMES` dict (line 211). Liquid precipitation is never accessed in OCHRE; no OCHRE sentinel handling exists to compare against.
- [x] EnergyPlus cross-check result: **Partial match / Sentinel value wrong** — EnergyPlus `WeatherManager.f90` declares `LiquidPrecip` arrays for field 33. The EPW Data Dictionary IDD specification (verified across EnergyPlus 8.3, 9.6, and 24.2 — see citations below) reads `N33, \field Liquid Precipitation Depth \units mm \missing 999`. The ticket and the `>= 9000.0` threshold in §Required Behavior both reference **9999**, which is the sentinel for solar radiation fields (N10–N15), **not** the precipitation field. The correct sentinel for Liquid Precipitation Depth is **999**.

### Web-Verified Citations

**Citation 1**: "EnergyPlus Input-Output Reference §'EPW Data Dictionary' — field 33 (Liquid Precipitation Depth, mm), missing value = 9999"

- **Source found**: EnergyPlus Auxiliary Programs — EPW Data Dictionary, multiple versions verified:
  - https://bigladdersoftware.com/epx/docs/9-6/auxiliary-programs/energyplus-weather-file-epw-data-dictionary.html
  - https://bigladdersoftware.com/epx/docs/24-2/auxiliary-programs/energyplus-weather-file-epw-data-dictionary.html
  - https://bigladdersoftware.com/epx/docs/8-3/auxiliary-programs/energyplus-weather-file-epw-data-dictionary.html
- **Quoted passage** (EnergyPlus 9.6 and 24.2, identical):
  > `N33, \field Liquid Precipitation Depth`
  > `     \units mm`
  > `     \missing 999`
  >
  > For comparison, the solar radiation fields that use 9999 as sentinel:
  > `N10, \field Extraterrestrial Horizontal Radiation \units Wh/m2 \missing 9999`
  > `N13, \field Global Horizontal Radiation \units Wh/m2 \missing 9999`
  > `N34, \field Liquid Precipitation Quantity \units hr \missing 99`
- **Verdict**: **Incorrect** — The ticket's claim that the sentinel is **9999** is wrong. The correct EPW missing-value indicator for Liquid Precipitation Depth (N33) is **999**, not 9999. The 9999 sentinel applies to radiation fields (N10–N19). The ticket's proposed threshold `>= 9000.0` would correctly catch a 9999 value if it appeared, but the actual sentinel that real EPW generators write is 999, so any field value of 999 (a plausible but implausible-for-one-hour rainfall amount) would be mishandled by a `>= 9000.0` threshold. The correct threshold should be `>= 900.0` (or a precise `== 999.0` check) to catch the documented 999 sentinel.

**Citation 2**: "EPW format specification v2.0 (EnergyPlus 9.6 release) — field count and sentinel encoding"

- **Source found**: EnergyPlus 9.6 Auxiliary Programs — EPW Data Dictionary (same URL as above); also DesignBuilder EPW format reference at https://designbuilder.co.uk/cahelp/Content/EnergyPlusWeatherFileFormat.htm
- **Quoted passage** (DesignBuilder, echoing E+ spec):
  > "Missing value is 999. … If this value is not missing, then it is used and overrides the 'precipitation' flag as rainfall. Conversely, if the precipitation flag shows rain and this field is missing or zero, it is set to 1.5 (mm)."
- **Verdict**: **Confirmed** that field 33 is the standard location for Liquid Precipitation Depth and has existed at least since EnergyPlus 8.2. The claim about "EPW 2.0" being associated with EnergyPlus 9.6 could not be verified from documentation; the field appears in all versions checked. The field count detail is confirmed: field 33 (0-indexed) is beyond the EPW_RECORD_MIN_FIELDS=24 check in HARES, so older EPW files with 24–33 fields will silently zero it.

**Citation 3**: Project policy `feedback_no_silent_defaults.md`

- **Source found**: Searched the HARES repository — the file `feedback_no_silent_defaults.md` does **not exist** in the codebase. However, the pattern is implemented: `crates/hares-io/tests/silent_default_regressions.rs` exists and its docstring reads "no silent substitution of engineering defaults." The policy is real and enforced by tests, but the specific markdown file cited does not exist.
- **Verdict**: **Partially confirmed** — the policy is real and operative; the exact filename cited is wrong (the file does not exist).

### Legitimacy
- **Verdict**: **Partially Legitimate**
- **Rationale**: Both bugs described in the ticket are real and present in the current code (`crates/hares-io/src/epw.rs:202–209`). The line numbers are correct, the two silent-zero code paths are confirmed, and the no-diagnostic policy violation is genuine. The core complaint and proposed fix structure (file-level flags, one `tracing::debug!` per condition, per-row `tracing::warn!` for parse errors) are sound. However, the ticket contains a material factual error: it repeatedly cites the EPW missing-data sentinel as **9999**, when the EnergyPlus IDD specification (verified in versions 8.3, 9.6, and 24.2) consistently shows `\missing 999` for field N33 (Liquid Precipitation Depth). The proposed guard `value >= 9000.0` would fail to catch the actual sentinel value of 999 written by real EPW generators. The correct threshold is `>= 900.0` (or equality check at 999.0). Additionally, the project policy file `feedback_no_silent_defaults.md` referenced does not exist on disk; the policy is real but the filename is wrong. Bug 2 in the regression test (`ticket_031_sentinel_999_in_field_33_yields_zero_no_diagnostic`) also reveals a subtlety: the current code does **not** map 999 to 0.0 via `.max(0.0)` — it returns 0.999 m (999 mm / 1000), which is physically wrong (not silently-zero but silently-wrong). This makes the sentinel-handling bug more impactful than the ticket implies.

### Proposed Fix Summary
In the row-parse closure of `parse_epw_str` (lines 202–209 of `crates/hares-io/src/epw.rs`):

1. Add two `bool` flags before the row loop: `precip_field_absent` and `precip_sentinel_seen`.
2. In the row loop, replace the current block with an explicit match:
   - If `fields.len() <= IDX_LIQUID_PRECIP_DEPTH_MM` → set `precip_field_absent = true`, use 0.0.
   - Else if field parses to `f64` and value `>= 900.0` (the correct threshold for sentinel 999) → set `precip_sentinel_seen = true`, use 0.0.
   - Else if field fails to parse → emit `tracing::warn!("row {row}: …")`, use 0.0.
   - Else → use `value.max(0.0) / 1000.0`.
3. After the loop, emit `tracing::debug!` once each if the flags are set.

Do NOT implement the fix — this is audit only.

### Test Written
- File: `crates/hares-io/src/epw.rs` (within the existing `#[cfg(test)]` module, lines 1542–1642)
- Three tests added:
  1. `ticket_031_absent_field_33_yields_zero_no_diagnostic` — confirms bug 1: rows with fewer than 34 fields silently produce 0.0
  2. `ticket_031_sentinel_999_in_field_33_yields_zero_no_diagnostic` — documents the actual sentinel bug: the current code produces **0.999 m** (not 0.0) for the EPW sentinel value 999, with no diagnostic; the correct fix should substitute 0.0
  3. `ticket_031_parse_error_in_field_33_silently_becomes_zero` — confirms bug 2b: a non-numeric field 33 value (e.g. "N/A") silently becomes 0.0 with no `tracing::warn!`
- All three tests pass against the current (buggy) implementation, documenting the existing behavior.
