---
id: HARES-073
title: HPXML config extraction correctness tests against OCHRE
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-io/tests/hpxml_parity.rs
  - tests/fixtures/hpxml/ochre_samples/
references:
  - vendors/OCHRE/ochre/Dwelling.py
  - crates/hares-io/src/hpxml/building.rs
  - crates/hares-io/src/hpxml/equipment.rs
  - crates/hares-io/src/hpxml/validation.rs
verification:
  - cargo test -p hares-io hpxml_parity
  - cargo clippy -p hares-io
---

## Background/Context

Audit found multiple HPXML extraction gaps: HVAC number_of_speeds not derived, startup_capacity_degradation missing, water heater UA not including jacket R-value, heat pump backup lockout not extracted, fan power not scaled by capacity, duct surface area missing. These cause equipment to run with wrong configurations.

Tests must verify HARES extracts the same config values OCHRE does from identical HPXML inputs.

## Work to Do

- [ ] Create `crates/hares-io/tests/hpxml_parity.rs` integration test module
- [ ] **Test: HVAC number_of_speeds** — Parse HPXML with `CompressorType=variable speed` and SEER 15, verify config produces `number_of_speeds=4` (matching OCHRE's derivation logic)
- [ ] **Test: HVAC number_of_speeds single** — Parse HPXML with `CompressorType=single stage`, verify `number_of_speeds=1`
- [ ] **Test: startup_capacity_degradation** — Parse HPXML with AC, verify config includes `startup_capacity_degradation` field (OCHRE default 0.0 for AC, varies for HP)
- [ ] **Test: water heater UA with jacket** — Parse HPXML with `WaterHeater` including `Jacket R-Value=10`, verify total UA incorporates jacket insulation (compare against OCHRE's `calculate_ua()`)
- [ ] **Test: heat pump backup lockout** — Parse HPXML with ASHP, verify backup heating lockout temperature is extracted or defaulted correctly
- [ ] **Test: fan power scaling** — Parse HPXML with AC rated 3-ton (36000 BTU/h), verify auxiliary fan power = rated_cfm_per_ton × tons × watts_per_cfm (not just watts_per_cfm)
- [ ] **Test: duct parameters** — Parse HPXML with duct info, verify `duct_surface_area`, `duct_r_value`, `duct_leakage_fraction` all extracted
- [ ] Use existing fixtures in `tests/fixtures/hpxml/ochre_samples/` or create minimal ones
- [ ] For each test, include the OCHRE-computed expected value as a comment with the Python expression that produces it

## Measures of Success

- [ ] Config extraction matches OCHRE for all tested fields
- [ ] Tests document the OCHRE derivation logic in comments
- [ ] At least one HPXML fixture per equipment type (HVAC, WH, HP)

## Verification

- [ ] `cargo test -p hares-io hpxml_parity` passes
- [ ] `cargo clippy -p hares-io` passes
