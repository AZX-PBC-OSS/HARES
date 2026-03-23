---
id: THERMAL-008
title: 7-day performance + energy parity benchmark with tightened tolerances
kind: implement
depends_on: [THERMAL-007]
files_to_touch:
  - tests/python/test_ochre_parity.py
references:
  - "ASHRAE Standard 140-2023, Section 5.2: acceptance range ±15% for annual heating energy"
verification:
  - uv run maturin develop -m crates/hares-python/Cargo.toml --release
  - uv run pytest tests/python/test_ochre_parity.py -v -s
---

## Background/Context

After all thermal fixes, update the 7-day benchmark with tightened tolerances
and validate that performance hasn't regressed. The implicit solver should
maintain ~50k steps/sec (pre-factored LU is O(n²) per step, same as explicit).

The HVAC heating divergence should drop from +123% to within 15%.

### Tolerance rationale

ASHRAE Standard 140-2023 Section 5.2 uses statistical analysis of reference
program results. The acceptance range for annual heating energy is typically
±15% of the reference program mean. Our 7-day tolerance of 15% for HVAC
heating aligns with this standard. Schedule-driven loads (lighting, MELs) are
expected to match within 2% since they don't interact with the thermal solver.

### Performance baseline

Before changes: 50k steps/sec (release, 7-day BEopt).
After changes: should remain >30k steps/sec. LU factorization adds O(n³) at
construction but each step remains O(n²). For n=10-30, construction overhead
is <1ms.

## Work to Do

- [ ] Record current performance baseline before changes:
      Run `test_7day_benchmark` in release mode, note steps/sec.
- [ ] Update `PARITY_CHECKS` tolerances in `test_ochre_parity.py`:
      - Total Electric: 10% (allow for unimplemented event-driven equipment)
      - HVAC Heating: 15% (implicit solver + LWR fix; ASHRAE 140 acceptance)
      - HVAC Cooling: 5% (maintain)
      - Schedule-driven loads: 2% (maintain)
- [ ] Update `test_7day_benchmark` assertions:
      - HVAC Heating diff < 15% (was 123%)
      - Total diff < 15%
      - Performance: HARES sim steps/sec > 30k (regression guard)
- [ ] Add `test_1h_parity_all_pass`: run the parametrized parity checks and
      assert ALL pass (currently some are skipped).
- [ ] Update hardcoded OCHRE reference in Rust `smoke_test.rs` with values
      generated from the live OCHRE parity test.

## Files to Touch

- `tests/python/test_ochre_parity.py`: Update tolerances + add performance guard

## Measures of Success

- [ ] 1-hour parity: all equipment within 5%.
- [ ] 7-day parity: HVAC heating within 15%, total within 15%.
- [ ] Performance: > 30k steps/sec in release build (no regression from baseline).

## Verification

- [ ] `uv run pytest tests/python/test_ochre_parity.py -v -s` all pass
