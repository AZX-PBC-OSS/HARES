---
id: HARES-072
title: Schedule loading correctness tests against OCHRE
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-io/src/schedule_resolve.rs
  - crates/hares-io/tests/schedule_parity.rs
  - tests/fixtures/parity/schedules/
references:
  - vendors/OCHRE/ochre/utils/schedule.py
  - vendors/OCHRE/ochre/Equipment/ScheduledLoad.py
verification:
  - cargo test -p hares-io schedule_parity
  - cargo clippy -p hares-io
---

## Background/Context

Audit found that `schedule_resolve.rs:228-237` silently falls back to constant average power (`annual_kwh / 8760`) when a schedule CSV column isn't found, instead of producing a time-varying kW series. This is the root cause of the lighting 100% power bug. Additionally, OCHRE checks for `Max Electric Power (W)` before falling back to annual energy; HARES only checks `annual_electric_kwh`.

We need tests that prove schedule loading produces correct time-varying output and catches silent failures.

## Work to Do

- [ ] Create `crates/hares-io/tests/schedule_parity.rs` integration test module
- [ ] **Test: time-varying lighting schedule** — Given a schedule CSV with a `lighting_interior` column and `annual_electric_kwh=1200`, verify the resolved kW series is NOT constant, has correct peak, correct mean matching annual energy, and varies by hour-of-day
- [ ] **Test: Max Electric Power path** — Given config with `max_electric_power_w=500` (no annual energy), verify resolved series uses 500W as peak and schedule fraction as multiplier
- [ ] **Test: missing column errors** — Given a schedule CSV that does NOT contain the requested column name, verify the resolver returns an error (not a silent constant fallback)
- [ ] **Test: duty_cycle_fraction propagation** — Given `duty_cycle_fraction=0.5` in config, verify the resolved kW series is scaled by 0.5 vs the same config without it
- [ ] **Test: OCHRE parity values** — Run OCHRE's `schedule.py:resolve_schedule()` on a test fixture, capture 24 hourly values, hardcode as expected in Rust test, verify HARES matches within 0.1%
- [ ] Create minimal test fixture CSV in `tests/fixtures/parity/schedules/` with known column values
- [ ] **Test: annual energy round-trip** — Sum resolved kW series × dt over 8760 hours, verify it equals input `annual_electric_kwh` within 0.01%

## Measures of Success

- [ ] All schedule loading tests pass
- [ ] No silent fallback to constant power — missing columns produce errors
- [ ] Round-trip energy conservation proven
- [ ] At least one test with OCHRE-derived reference values

## Verification

- [ ] `cargo test -p hares-io schedule_parity` passes
- [ ] `cargo clippy -p hares-io` passes
