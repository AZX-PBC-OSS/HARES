---
id: HARES-054
title: "OCHRE Output Parity Harness"
kind: test
depends_on: [HARES-043, HARES-044]
files_to_touch:
  - tests/parity/mod.rs
  - tests/parity/corpus.rs
  - tests/parity/tolerance.rs
  - tests/fixtures/parity/README.md
references:
  - docs/architecture/07-testing-and-verification.md
verification:
  - cargo test --test parity -- --ignored
---

## Background/Context
The architecture's primary validation is OCHRE output parity: same HPXML + schedule + weather + params → compare ochre_next output against OCHRE reference output at 1-minute resolution within defined tolerance bands. This is the CI gate for Phase 1 and the release gate for Phase 2. The harness must be established early so every subsequent equipment ticket can add its parity cases.

## Work to Do
- [ ] Create `tests/parity/` test infrastructure
  - [ ] `corpus.rs`: discover and iterate reference building fixtures under `tests/fixtures/parity/`
  - [ ] Each fixture is a directory containing: `building.xml` (HPXML), `schedule.csv`, `weather.epw`, `reference_output.parquet` (OCHRE's output), `config.toml` (sim params)
  - [ ] `tolerance.rs`: define tolerance band checker per metric:
    - [ ] Zone temperature (conditioned): ±0.1°C MAE
    - [ ] Zone temperature (unconditioned): ±0.5°C MAE
    - [ ] Annual HVAC energy: ±1.0% relative
    - [ ] Annual water heater energy: ±0.5% relative
    - [ ] Annual total site energy: ±1.0% relative
    - [ ] Peak HVAC power: ±2.0% relative
    - [ ] Battery SOC trajectory: ±1% MAE absolute
    - [ ] Equipment mode cycle count: ±5% relative
  - [ ] `mod.rs`: test runner that loads each fixture, runs ochre_next, loads reference, compares within tolerance bands. Report per-metric pass/fail with actual vs allowed deviation.
- [ ] Create initial fixture set (10 reference buildings):
  - [ ] CZ 2A hot-humid: gas furnace + AC, resistance WH
  - [ ] CZ 4A mixed: ASHP, HPWH
  - [ ] CZ 5A cold: mini-split, gas WH
  - [ ] CZ 6B cold-dry: electric resistance, resistance WH
  - [ ] + 6 more spanning DER combinations (PV, battery, EV)
  - [ ] Each fixture: 30-day summer + 30-day winter run
  - [ ] Generate reference outputs by running OCHRE on same inputs
- [ ] Property parity sub-test: parse each HPXML through both OCHRE and ochre_next, assert equipment sets, zone configs, and key parameters match
- [ ] Full-year parity for at least 3 buildings (release gate): CZ 2A, CZ 4A, CZ 5A

## Measures of Success
- [ ] All 10 reference buildings pass within tolerance bands for 30-day runs
- [ ] At least 3 buildings pass full-year parity
- [ ] Property parity: equipment names, counts, and key parameters match OCHRE for all 10 HPXMLs
- [ ] A tolerance violation produces a clear report: building ID, metric, expected tolerance, actual deviation
- [ ] Tests are `#[ignore]` by default (long-running) but run in CI nightly

## Verification
- [ ] `cargo test --test parity -- --ignored` passes
