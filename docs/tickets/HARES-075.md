---
id: HARES-075
title: HVAC equipment step correctness tests against OCHRE
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-equipment/tests/hvac_parity.rs
  - tests/fixtures/parity/hvac/
references:
  - vendors/OCHRE/ochre/Equipment/HVAC.py
  - crates/hares-equipment/src/hvac/
verification:
  - cargo test -p hares-equipment hvac_parity
  - cargo clippy -p hares-equipment
---

## Background/Context

HVAC audit found: PLF bounds not clamped (OCHRE min 0.7), COP definition mismatch (fan in/out of denominator), multi-speed selection untested, furnace COP not reported. Tests must exercise the step function with known inputs and compare intermediate values.

## Work to Do

- [ ] Create `crates/hares-equipment/tests/hvac_parity.rs`
- [ ] **Test: biquadratic evaluation with PLF clamping** — Evaluate EIR-f(PLR) biquadratic at PLR=0.2; verify HARES clamps PLF at min 0.7 (if fix applied) or document current behavior vs OCHRE
- [ ] **Test: biquadratic evaluation temperature bounds** — Evaluate capacity-f(Twb,Tdb) at out-of-range temps, verify clamping matches OCHRE bounds
- [ ] **Test: single-speed AC step** — Given outdoor_temp=35°C, indoor_wb=19.4°C, rated_capacity=10kW, EIR=0.25, SHR=0.75, fan_power=400W: compute one step, verify electric_kw, sensible_cooling_w, latent_cooling_w, COP against OCHRE hand-calculation
- [ ] **Test: two-speed AC at low speed** — Same conditions, 2-speed with capacity_ratios=[0.5, 1.0], verify low-speed operation selects correct capacity/EIR
- [ ] **Test: gas furnace step** — Given setpoint=21°C, zone_temp=19°C, AFUE=0.80, capacity=15kW: compute one step, verify gas_therms_hr, thermal_output_w, fan_power_w, COP
- [ ] **Test: thermostat deadband transitions** — Start at 21°C setpoint with 1°C deadband in heating mode. Step with zone temps: 20.0→20.5→21.0→21.5→22.0→21.5→21.0→20.5→20.0. Verify mode transitions at correct temps
- [ ] **Test: AC SHR split** — Verify sensible = total × SHR, latent = total × (1 - SHR) for known operating point
- [ ] **Test: heat pump defrost** — At outdoor_temp=0°C, verify capacity degradation factor matches OCHRE formula (0.875 × (1 - defrost_time_frac))
- [ ] Create reference values by running OCHRE's HVAC.py step functions offline, store as constants in test
- [ ] Each test should print both HARES and expected values on failure for easy debugging

## Measures of Success

- [ ] Per-step power and heat values match OCHRE within 1%
- [ ] Thermostat mode transitions match exactly
- [ ] At least 6 distinct test scenarios covering furnace, AC, HP heating, HP cooling, 2-speed, defrost

## Verification

- [ ] `cargo test -p hares-equipment hvac_parity` passes
- [ ] `cargo clippy -p hares-equipment` passes
