---
id: PARITY-021
title: "BESTEST validation harness"
kind: implement
depends_on: [PARITY-014, PARITY-015, PARITY-016, PARITY-018, PARITY-020]
files_to_touch:
  - tests/bestest/cases.rs
  - tests/bestest/reference_bands.rs
  - tests/bestest/mod.rs
references:
  - docs/equipment/ochre-parity-gaps.md (Gap 25)
  - ASHRAE Standard 140 (BESTEST)
  - EnergyPlus BESTEST results
  - NREL BESTEST reference buildings
verification:
  - cargo build --workspace
  - cargo test --workspace -- bestest
---

## Background/Context

HARES has a `tests/bestest/` directory with `cases.rs`, `reference_bands.rs`, and `mod.rs` stubs. Systematic BESTEST validation is needed to verify envelope physics against ASHRAE 140 reference buildings and ensure results fall within EnergyPlus/DOE-2/BLAST acceptable bands.

**Target**: Implement BESTEST cases 600-960 (low-mass and high-mass) with automated pass/fail against reference bands.

## Work to Do

- [ ] Define BESTEST cases 600, 610, 620, 630, 640, 650, 900, 910, 920, 930, 940, 950, 960 as test fixtures:
  - TOML or JSON building definitions matching ASHRAE 140 specifications
  - Each case has specific geometry, construction, internal gains, setpoints, infiltration
- [ ] Populate `reference_bands.rs` with acceptable annual heating/cooling load ranges from ASHRAE 140 Table B8-1:
  - Min/max MWh for each case from EnergyPlus, DOE-2, BLAST, TRNSYS, ESP, SERIRES
- [ ] In `cases.rs`, implement test functions:
  - For each case: construct building from TOML, run annual simulation, extract heating/cooling loads
  - Assert loads fall within reference bands
- [ ] Run cases and identify any failures — create follow-up tickets for physics fixes
- [ ] Add CI integration: `cargo test --workspace -- bestest` runs all BESTEST cases

## Files to Touch

- `tests/bestest/cases.rs`: BESTEST case definitions and test functions
- `tests/bestest/reference_bands.rs`: ASHRAE 140 acceptable ranges
- `tests/bestest/mod.rs`: Module organization

## Measures of Success

- [ ] At least cases 600, 900 (base low-mass and high-mass) pass within reference bands
- [ ] Failures are documented with specific physics gaps (actionable follow-up tickets)
- [ ] Test suite runs in <60s (annual simulations at 1-hour timestep)

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace -- bestest` runs and reports results
