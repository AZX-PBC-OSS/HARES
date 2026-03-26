---
id: ENVELOPE-007
title: Tighten conditioned oracle tolerances
kind: implement
depends_on: [ENVELOPE-002, ENVELOPE-003, ENVELOPE-004, ENVELOPE-005]
files_to_touch:
  - tests/conditioned_oracle.rs
references:
  - tests/fixtures/parity/ochre_rc_reference.json
  - docs/tickets/ENVELOPE-004.md
  - docs/tickets/ENVELOPE-002.md
  - docs/tickets/ENVELOPE-005.md
  - docs/tickets/ENVELOPE-003.md
verification:
  - cargo test -p hares-core --features observe --test conditioned_oracle -- --nocapture
  - cargo clippy --all-targets -- -D warnings
---

## Background/Context

The conditioned oracle currently uses loose tolerances: 50% for HVAC load comparison,
and many component comparisons are informational only (logged but not asserted). The
`compare_mean` function has a silent NaN-pass rule (`hares.is_nan()` => passed) that
lets missing HARES data slip through.

Once ENVELOPE-002 through ENVELOPE-005 fix the underlying parameter discrepancies,
we should tighten the oracle to prevent regressions. The current known over-prediction
is 25% — a 25% tolerance gate would provide zero regression protection. Target 10-15%
after fixes.

## Work to Do

- [ ] Close the NaN silent-pass gap: require non-NaN HARES values for all asserted
  components. Change `hares.is_nan()` from auto-pass to auto-fail.
- [ ] Add per-component heat gain assertions (currently only logged):
  - Wall heat gain: +/-20% of OCHRE mean
  - Floor heat gain: +/-20% of OCHRE mean
  - Roof heat gain: +/-30% of OCHRE mean (higher tolerance — unconditioned zone coupling)
  - Window solar transmitted: +/-10% of OCHRE mean
  - Window heat gain (conduction): +/-20% of OCHRE mean
  - Infiltration: +/-5% of OCHRE mean (already close)
  - Internal/occupancy gains: +/-20% of OCHRE mean (once ENVELOPE-003 adds fallback)
- [ ] Tighten HVAC load comparison from 50% to **10-15%** (ideal mode). The exact
  value depends on the residual after fixes — set based on measured results, not
  the known-broken state.
- [ ] Add an RC parity gate: before running the simulation, verify that
  `envelope_diagnostics()` total UA is within 5% of OCHRE reference total UA
- [ ] Add regression bounds: if any component deviates by more than 2x its historical
  best delta, fail with a clear message about which component regressed
- [ ] Keep the diagnostic table output — it's valuable for debugging when assertions fire

## Files to Touch

- `tests/conditioned_oracle.rs`: Add component-level assertions, tighten HVAC tolerance,
  close NaN-pass gap, add RC gate

## Measures of Success

- [ ] Conditioned oracle catches regressions in individual envelope components, not just aggregate HVAC load
- [ ] HVAC load tolerance is tight enough to catch a 15% regression (not just 25%)
- [ ] NaN values in HARES data cause test failure (not silent pass)
- [ ] RC parity gate fails early if film coefficients or material data regress
- [ ] All three seasonal scenarios (spring/summer/winter) pass with tightened tolerances

## Verification

- [ ] `cargo test -p hares-core --features observe --test conditioned_oracle -- --nocapture` passes
- [ ] `cargo clippy --all-targets -- -D warnings` passes
