# ResStock CSV Midpoint Offset Should Be 1800 Seconds, Not Zero

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-io/resstock_csv

## Problem

`crates/hares-io/src/resstock_csv.rs:337` sets `midpoint_offset_secs = 0` for ResStock weather records. ResStock CSV timestamps are end-of-interval (per the ResStock weather format documentation), the same convention as TMY3. The TMY3 path was corrected by the B4 fix to `1800` (30 minutes); the ResStock path was not. The bug is structurally identical to the pre-B4 TMY3 defect.

Effect: solar position calculations, solar-time alignment, and any sub-hourly resampling that consumes the midpoint timestamp are off by 30 minutes when the dwelling is configured with a ResStock weather CSV. The systematic offset corrupts solar load magnitudes at sunrise and sunset and biases peak-load timing.

## Current Behavior

`crates/hares-io/src/resstock_csv.rs:337`:
```rust
midpoint_offset_secs: 0,  // wrong; ResStock timestamps are end-of-interval
```

No test guards this value. The simulation runs and produces plausible-looking results biased by 30 minutes in solar-time alignment.

## Required Behavior

1. Set `midpoint_offset_secs = 1800` for ResStock CSV records, matching the TMY3 fix.
2. Add a regression test pinning the value (analogous to ticket 097 for TMY3).
3. Add an integration test confirming solar position computed from a ResStock CSV at 12:00 timestamp produces the same solar zenith as a TMY3 fixture at 12:00, when both are interpreted with the correct midpoint offset.

## Approach

1. Open `crates/hares-io/src/resstock_csv.rs:337` and change the literal to `1800`.
2. Add inline comment citing the ResStock weather format (end-of-interval) and the parallel TMY3 fix.
3. Add a unit test in `crates/hares-io/tests/` asserting `meta.midpoint_offset_secs == 1800` for a representative ResStock fixture.
4. Cross-validate solar position calculations between a ResStock CSV and a TMY3 fixture for the same site/date — they should agree to within solar-position numerical precision once both midpoint offsets are correct.

## Definition of Done

- [ ] `crates/hares-io/src/resstock_csv.rs:337` sets `midpoint_offset_secs = 1800`
- [ ] Regression test pins the value
- [ ] Cross-validation test: solar zenith from ResStock CSV at noon-timestamp matches TMY3 reference for same site/date within 0.01°
- [ ] Inline comment cites the ResStock format and the TMY3 parallel fix

## Verification

```bash
cargo test -p hares-io resstock
cargo test -p hares-io solar_position
```

## References

- ResStock Weather Format documentation (NREL — see `reference_resstock_weather_format` project memory) — ResStock CSV timestamps are end-of-interval.
- NSRDB TMY3 User's Manual (NREL/TP-581-43156, 2007) §3 — analogous end-of-interval convention.
- HARES B4 fix at `crates/hares-io/src/tmy3.rs:417` — the parallel fix already in place for TMY3.

## Related Tickets

- 097-tmy3-midpoint-offset-regression-test (analogous TMY3 regression test)
- 029-resstock-csv-constant-pressure (related ResStock CSV defect)
