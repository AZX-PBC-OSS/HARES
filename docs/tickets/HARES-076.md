---
id: HARES-076
title: Water heater equipment step correctness tests against OCHRE
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-equipment/tests/water_heater_parity.rs
  - tests/fixtures/parity/water_heater/
references:
  - vendors/OCHRE/ochre/Equipment/WaterHeater.py
  - vendors/OCHRE/ochre/Models/Water.py
  - crates/hares-equipment/src/water_heater/
verification:
  - cargo test -p hares-equipment water_heater_parity
  - cargo clippy -p hares-equipment
---

## Background/Context

Water heater audit found: no schedule-based draw profiles (constant average flow), HPWH COP curves incomplete, deadband default mismatch (OCHRE 5.56°C vs HARES 2.0°C), tank jacket R-value not integrated into UA. Tests must verify tank physics, element cycling, and draw response.

## Work to Do

- [ ] Create `crates/hares-equipment/tests/water_heater_parity.rs`
- [ ] **Test: standby loss** — Initialize 50-gal tank at 51.7°C (125°F), ambient 20°C, UA=2.0 W/K, no draws, step 24 hours. Verify final tank temp and total standby kWh match OCHRE within 1%
- [ ] **Test: element cycling deadband** — Initialize resistance WH at setpoint=48.9°C (120°F), deadband=5.56°C. Start at 46°C (below lower threshold). Step until element turns on, then until it turns off. Verify on/off thresholds match OCHRE: on at (setpoint - deadband/2), off at setpoint
- [ ] **Test: draw response** — Draw 10 L/min for 10 minutes from top of tank, inlet at 15°C. Verify outlet temp stays at tank top temp initially, then drops as hot water depleted. Compare node temperatures after draw to OCHRE
- [ ] **Test: stratification** — Initialize 12-node tank with linear gradient (50°C top, 20°C bottom). No draws, no heating. Step 1 hour. Verify inter-node conduction and inversion mixing match OCHRE
- [ ] **Test: HPWH COP at multiple temps** — For ambient temps [10, 20, 30, 40]°C and tank temp 50°C, verify HPWH COP from curve evaluation matches OCHRE (or document that curves aren't implemented yet)
- [ ] **Test: energy conservation** — Over any test scenario, verify energy_in (element + HPWH) = energy_stored_change + standby_losses + energy_drawn, within 0.1%
- [ ] Create test fixture configs in `tests/fixtures/parity/water_heater/`

## Measures of Success

- [ ] Tank physics (conduction, mixing, draw) validated
- [ ] Element cycling thresholds match OCHRE
- [ ] Energy conservation proven for all scenarios
- [ ] HPWH gaps documented (which curves missing)

## Verification

- [ ] `cargo test -p hares-equipment water_heater_parity` passes
- [ ] `cargo clippy -p hares-equipment` passes
