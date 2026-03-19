---
id: HARES-057
title: "Regression Corpus and Release Exit Criteria"
kind: test
depends_on: [HARES-054, HARES-055, HARES-056, HARES-062]
files_to_touch:
  - tests/regression/mod.rs
  - docs/RELEASE_CRITERIA.md
references:
  - docs/architecture/07-testing-and-verification.md
  - docs/architecture/08-operations.md
  - docs/architecture/09-roadmap.md
verification:
  - cargo test --test regression -- --ignored
---

## Background/Context
Phase 4 validation requires a regression corpus, release gates, and documented exit criteria. This ticket aggregates the individual validation pieces (parity, BESTEST, benchmarks) into a single release-gating check and establishes the regression corpus that prevents future changes from breaking validated behavior.

## Work to Do
- [ ] Create `tests/regression/mod.rs`: unified regression runner
  - [ ] Runs OCHRE parity tests (HARES-054)
  - [ ] Runs core BESTEST cases (HARES-055)
  - [ ] Runs determinism test: same seed → identical trajectory for 3 reference buildings
  - [ ] Runs fleet scale test: 1000 dwellings on 32 GB without OOM
  - [ ] Runs checkpoint restart test: checkpoint at step N, restart, verify identical continuation
  - [ ] Runs RL determinism test: `DwellingGymEnv.reset(seed=42)` → identical trajectory
  - [ ] Runs multi-instance test: 2 batteries + 2 PV → independent behavior
  - [ ] Runs weighted aggregation test: fleet aggregate matches manual weighted sum
- [ ] Create `docs/RELEASE_CRITERIA.md`:
  - [ ] **Must pass** (hard gate):
    - [ ] All 10 OCHRE parity buildings within tolerance bands (30-day)
    - [ ] 3 full-year parity buildings within tolerance
    - [ ] BESTEST 600FF, 900FF, 640 within ASHRAE 140 reference bands
    - [ ] Determinism: identical output across thread counts (1, 4, 8)
    - [ ] Fleet 1000 dwellings completes without OOM
    - [ ] Zero hot-path allocations per timestep
    - [ ] `docs/PHYSICS_DECISIONS.md` documents all OCHRE divergences
  - [ ] **Should pass** (tracked, not blocking):
    - [ ] Extended BESTEST (610, 620, CE100-CE200, §5.4)
    - [ ] Full-year parity for all 10 buildings
    - [ ] RL step latency < 1ms
  - [ ] **Reported** (informational):
    - [ ] Benchmark wall times vs OCHRE
    - [ ] Memory per dwelling under fleet
    - [ ] Profiling kernel breakdown
- [ ] Create `docs/PHYSICS_DECISIONS.md`:
  - [ ] Template with sections: decision, OCHRE behavior, ochre_next behavior, rationale, expected output impact
  - [ ] Populate with known divergences from earlier tickets (air density correction, thermostat deadband FSM, supply air temps, terrain/wind, dehumidifier EF/IEF, standby power)

## Measures of Success
- [ ] `cargo test --test regression -- --ignored` runs all sub-suites and produces pass/fail summary
- [ ] `docs/RELEASE_CRITERIA.md` is complete and actionable
- [ ] `docs/PHYSICS_DECISIONS.md` has at least 6 entries (the known divergences)
- [ ] A failing regression test produces a clear report: test name, expected, actual, tolerance

## Verification
- [ ] `cargo test --test regression -- --ignored` passes
