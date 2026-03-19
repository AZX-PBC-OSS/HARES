---
id: HARES-055
title: "BESTEST / ASHRAE Standard 140 Test Suite"
kind: test
depends_on: [HARES-043, HARES-044, HARES-014]
files_to_touch:
  - tests/bestest/mod.rs
  - tests/bestest/cases.rs
  - tests/bestest/reference_bands.rs
  - tests/fixtures/bestest/
references:
  - docs/architecture/07-testing-and-verification.md
verification:
  - cargo test --test bestest -- --ignored
---

## Background/Context
ASHRAE Standard 140-2023 cases validate envelope and HVAC physics against reference programs (EnergyPlus, DOE-2, TRNSYS). This is the industry standard for simulation engine credentialing. Core cases (600FF, 900FF, 640) are firm Phase 2 deliverables. BESTEST buildings are not representable in HPXML — use synthetic building configs (TOML input).

## Work to Do
- [ ] Create `tests/bestest/` test infrastructure
  - [ ] `cases.rs`: define BESTEST building configs as TOML (not HPXML) — geometry, construction, schedules, HVAC
  - [ ] `reference_bands.rs`: define min/max reference bands per case from ASHRAE 140-2023 Table B.x results (EnergyPlus, DOE-2, TRNSYS, ESP-r ranges)
  - [ ] `mod.rs`: test runner that builds Dwelling from TOML config, runs simulation, checks results fall within reference bands
- [ ] Implement core BESTEST cases (Phase 2 firm deliverables):
  - [ ] **600FF**: Free-float lightweight envelope — annual peak/min zone temp, annual heating/cooling load
  - [ ] **900FF**: Heavyweight free-float (thermal mass) — same metrics
  - [ ] **640**: Setback thermostat — annual heating energy
- [ ] Implement extended BESTEST cases (stretch goals):
  - [ ] **610**: South shading overhang
  - [ ] **620**: East/west windows
  - [ ] **CE100–CE200**: DX cooling mechanical equipment performance
  - [ ] **§5.4**: Heat pump heating performance
- [ ] Each case fixture: TOML config, expected reference band (min, max), metric name
- [ ] BESTEST cases use `Dwelling::from_toml_config()` (defined in HARES-044) since BESTEST buildings are not representable in HPXML; do not define a new TOML constructor in this ticket

## Measures of Success
- [ ] Cases 600FF, 900FF, 640 all fall within ASHRAE 140 reference program bands
- [ ] Test report shows: case ID, metric, ochre_next value, reference min, reference max, pass/fail
- [ ] A result outside the reference band produces a clear failure with the deviation
- [ ] Extended cases (610, 620, CE100–CE200) tracked but not gating for v1 release

## Verification
- [ ] `cargo test --test bestest -- --ignored` passes for core cases
