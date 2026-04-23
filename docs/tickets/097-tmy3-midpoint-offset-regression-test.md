# TMY3 Midpoint-Offset Regression Test for B4 Fix

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-io/tmy3

## Problem

The B4 fix at `crates/hares-io/src/tmy3.rs:417` corrected the TMY3 midpoint offset to a literal `1800` seconds (30 minutes) — TMY3 timestamps are end-of-interval, so the midpoint of each hourly record sits 30 minutes earlier than the timestamp. There is no test pinning `tmy3.meta.midpoint_offset_secs == 1800`. A future refactor or a careless edit to the literal can silently reintroduce the systematic 30-minute offset that caused the original B4 bug, with no test catching it.

A 30-minute systematic offset corrupts solar position calculations, sun-time vs clock-time alignment, and any sub-hourly resampling that relies on the midpoint timestamp. The error is subtle because the simulation still runs and produces plausible-looking results.

## Current Behavior

`crates/hares-io/src/tmy3.rs:417`:
```rust
midpoint_offset_secs: 1800,  // single literal; no test guards it
```

No assertion exists in `crates/hares-io/tests/tmy3*.rs` that pins this value.

## Required Behavior

Add a regression test that:
1. Parses a representative TMY3 fixture
2. Asserts `meta.midpoint_offset_secs == 1800` exactly
3. Optionally asserts the derived midpoint timestamps for the first three records sit 1800 seconds before the corresponding TMY3 timestamps (this guards against the meta value being correct but the consumer ignoring it)

The test is a single literal assertion — cheap, fast, and prevents reintroduction of a subtle bias.

## Approach

1. Open the existing `crates/hares-io/tests/` TMY3 test module (or `crates/hares-io/src/tmy3.rs` inline tests) and add `#[test] fn tmy3_midpoint_offset_is_1800_seconds()`.
2. The test parses any TMY3 fixture (use the smallest fixture in `crates/hares-io/tests/data/`) and asserts:
   ```rust
   assert_eq!(parsed.meta.midpoint_offset_secs, 1800);
   ```
3. Add a second assertion that derives the midpoint of the first hourly record from the parsed timestamp minus the offset and confirms it equals the expected hh:30:00 wall-clock time.

## Definition of Done

- [ ] Test `tmy3_midpoint_offset_is_1800_seconds` exists in TMY3 test module
- [ ] Test asserts `meta.midpoint_offset_secs == 1800` exactly
- [ ] Test asserts the derived midpoint of the first record is 1800 seconds before its timestamp
- [ ] Test runs in `cargo test -p hares-io` without requiring a network resource

## Verification

```bash
cargo test -p hares-io tmy3
```

The test must fail if the literal at `crates/hares-io/src/tmy3.rs:417` is changed to any value other than 1800.

## References

- NSRDB TMY3 User's Manual (NREL/TP-581-43156, 2007) §3 "Data Format" — TMY3 timestamps are end-of-interval; the midpoint of each hourly record is 30 minutes earlier than the timestamp.
- ASHRAE Handbook of Fundamentals 2021 Ch. 14 §14.5 "Solar Radiation" — solar position calculation requires correct midpoint timestamp; using end-of-interval timestamps without offset shifts the apparent solar time by 30 minutes.

## Related Tickets

- 026-solar-position-spencer-accuracy (consumes the corrected midpoint timestamp)
- 029-resstock-csv-constant-pressure (related ResStock weather format)
- 099-resstock-csv-midpoint-offset (analogous fix for ResStock)
