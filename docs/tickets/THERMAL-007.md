---
id: THERMAL-007
title: 48-hour OCHRE thermal trace comparison
kind: implement
depends_on: [THERMAL-001, THERMAL-002, THERMAL-005]
files_to_touch:
  - tests/python/test_thermal_trace.py
references:
  - crates/hares-core/src/observer.rs
  - crates/hares-core/src/observer_capture.rs
  - tests/python/test_ochre_parity.py
  - "OCHRE v0.9.2 (vendored at vendors/OCHRE, pinned by git submodule)"
verification:
  - uv run maturin develop -m crates/hares-python/Cargo.toml --features observe --release
  - uv run pytest tests/python/test_thermal_trace.py -v -s
---

## Background/Context

After all thermal fixes (implicit solver, 4-component LWR, dry air density),
validate multi-day thermal behavior against OCHRE with per-timestep comparison.
This is the definitive parity test that exercises the full envelope over
day/night cycles, solar peaks, and nighttime losses.

OCHRE version: pinned by the `vendors/OCHRE` git submodule. Tests should
assert `ochre.__version__` matches expected to catch accidental drift.

## Work to Do

- [ ] Create `tests/python/test_thermal_trace.py`

- [ ] `test_48h_thermal_trace`:
      - Run OCHRE for 48h (BEopt example, May 5, 1-min timestep, verbosity=9)
      - Run HARES for 48h with observer enabled
      - Capture per-step: outdoor_temp_c, zone_indoor_temp_c, window_solar_w,
        infiltration_w, hvac_heating_w, hvac_cooling_w, port_sensible_w,
        internal_gain_w
      - Print comparison table: first 10 steps + every 60th step
      - Assert per-step zone temp divergence < 0.1°C for first 6 hours
      - Assert cumulative zone temp divergence < 0.5°C at 48h
      - Assert cumulative HVAC heating kWh within 10% at 48h

- [ ] `test_overnight_losses`:
      - Start at 18:00 local (post-sunset), run 14h to 08:00 next morning
      - Isolates conduction + infiltration + LWR (no solar)
      - Assert zone temp trajectory within 0.3°C at every step

- [ ] `test_solar_peak`:
      - Start at 10:00 local, run 6h through solar noon
      - Compare window_solar_w between OCHRE and HARES
      - Assert within 5% at each step

- [ ] `test_divergence_detector` (helper):
      - Finds first step where any component diverges by > 5%
      - Prints breakdown at that step with diagnostic guidance:
        "If LWR diverges → check β / view factor. If HVAC diverges →
        check solve_for_output_input. If infiltration diverges → check
        density or zone coupling."
      - Called by 48h test on failure

## Files to Touch

- `tests/python/test_thermal_trace.py`: New test file

## Measures of Success

- [ ] Zone temp < 0.1°C divergence per step for first 6 hours.
- [ ] Zone temp < 0.5°C cumulative at 48 hours.
- [ ] HVAC energy within 10% at 48 hours.
- [ ] Nighttime decay within 0.3°C.
- [ ] Solar gain within 5%.

## Verification

- [ ] `uv run pytest tests/python/test_thermal_trace.py -v -s` passes
