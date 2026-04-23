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
