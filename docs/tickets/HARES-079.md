---
id: HARES-079
title: End-to-end OCHRE parity test harness
kind: implement
depends_on:
  - HARES-072
  - HARES-073
  - HARES-074
  - HARES-075
  - HARES-076
  - HARES-077
  - HARES-078
files_to_touch:
  - tests/parity/mod.rs
  - tests/parity/harness.rs
  - tests/parity/ochre_runner.py
  - tests/parity/compare.rs
  - tests/fixtures/parity/e2e/
references:
  - vendors/OCHRE/ochre/
  - vendors/OCHRE/test/
  - crates/hares-core/src/dwelling.rs
  - crates/hares-io/src/hpxml/
verification:
  - cargo test --test parity
  - uv run pytest tests/parity/ -v
---

## Background/Context

This is the integration capstone: load a real HPXML + EPW, run N steps in BOTH OCHRE (Python) and HARES (Rust), and compare per-timestep outputs. This catches any regressions that unit tests miss and proves end-to-end correctness.

## Work to Do

- [ ] Create `tests/parity/ochre_runner.py` — Python script that loads an HPXML + EPW via OCHRE, runs 168 steps (1 week at 1-hour), writes per-step CSV with columns: timestamp, total_electric_kw, total_gas_therms_hr, zone_temp_c, per-equipment electric_kw, HVAC COP, WH outlet_temp, battery SOC
- [ ] Create `tests/parity/harness.rs` — Rust test that loads same HPXML + EPW via HARES, runs same 168 steps, writes equivalent CSV
- [ ] Create `tests/parity/compare.rs` — Comparison logic that reads both CSVs and checks:
  - `total_electric_kw`: within 5% or 0.1 kW (whichever is larger)
  - `total_gas_therms_hr`: within 5%
  - `zone_temp_c`: within 0.5°C
  - Per-equipment power: within 10% (looser for individual equipment)
  - Documents every column that exceeds tolerance with actual vs expected and % error
- [ ] Use existing OCHRE test fixtures (check `vendors/OCHRE/test/` for HPXML + EPW pairs)
- [ ] Create at least 2 test scenarios:
  - Scenario A: Simple gas furnace + AC home (heating-dominant week in January)
  - Scenario B: Heat pump home with water heater (cooling-dominant week in July)
- [ ] Test should produce a human-readable parity report showing pass/fail per metric per timestep
- [ ] Mark known-divergent metrics as expected failures with issue references (e.g., "COP differs due to HARES-075 fan power definition")

## Measures of Success

- [ ] Harness runs both OCHRE and HARES on same inputs
- [ ] Per-timestep CSV comparison with configurable tolerances
- [ ] Clear parity report output
- [ ] Known divergences documented, not hidden
- [ ] At least 2 scenarios tested

## Verification

- [ ] `cargo test --test parity` passes (or expected failures documented)
- [ ] `uv run pytest tests/parity/ -v` passes
- [ ] Parity report generated in tests/parity/output/
