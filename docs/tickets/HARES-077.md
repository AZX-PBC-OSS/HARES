---
id: HARES-077
title: DER equipment step correctness tests against OCHRE
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-equipment/tests/der_parity.rs
  - tests/fixtures/parity/der/
references:
  - vendors/OCHRE/ochre/Equipment/Battery.py
  - vendors/OCHRE/ochre/Equipment/PV.py
  - vendors/OCHRE/ochre/Equipment/Generator.py
  - vendors/OCHRE/ochre/Equipment/EV.py
  - crates/hares-equipment/src/battery.rs
  - crates/hares-equipment/src/pv.rs
  - crates/hares-equipment/src/generator.rs
  - crates/hares-equipment/src/ev/
verification:
  - cargo test -p hares-equipment der_parity
  - cargo clippy -p hares-equipment
---

## Background/Context

DER audit found: external control signals are stubs (silently do nothing), battery degradation formula differs from OCHRE's Smith 2017 variant, generator ramp rate units are kW/s vs OCHRE's kW/min (10-60x off), generator missing capacity_min. Tests must verify each DER's step computation and catch the control signal stub issue.

## Work to Do

- [ ] Create `crates/hares-equipment/tests/der_parity.rs`
- [ ] **Test: battery charge cycle** — Charge 10kWh battery from SOC=0.2 at 3kW for 1 hour. Verify final SOC, inverter losses, and net power match OCHRE's inverter efficiency model
- [ ] **Test: battery discharge cycle** — Discharge from SOC=0.8 at 2kW. Verify SOC accounting and round-trip efficiency
- [ ] **Test: battery SOC limits** — Attempt to charge beyond SOC_max=0.95, verify clamping
- [ ] **Test: battery degradation** — Run 100 cycles, verify capacity fade formula. Document whether HARES uses `dq_li1 = b1_eff / sqrt(day_age)` vs OCHRE's `dq_li1 = 0.5 * b1² / q_li1`
- [ ] **Test: PV cell temperature** — Given GHI=800 W/m², ambient=30°C, wind=2 m/s, verify cell temp matches OCHRE NOCT model
- [ ] **Test: PV DC power** — Given POA=900 W/m², cell_temp=45°C, verify DC power output with temperature derating
- [ ] **Test: generator efficiency** — At 50% load (5kW of 10kW rated), verify fuel consumption from efficiency curve matches OCHRE
- [ ] **Test: generator ramp rate** — Verify ramp rate units. If HARES uses kW/s, document and test. Compare behavior to OCHRE (kW/min) for same scenario
- [ ] **Test: generator capacity_min** — Verify generator enforces minimum operating power (if implemented) or document gap
- [ ] **Test: EV charging profile** — Start EV at SOC=0.3, charge at Level 2 (7.2kW). Verify SOC progression over 4 hours
- [ ] **Test: control signal NOT a stub** — Send a power setpoint to battery, verify output power ACTUALLY changes (not ignored). This catches the stub issue. If it fails, document clearly
- [ ] Each test prints actual vs expected on failure

## Measures of Success

- [ ] Battery inverter model validated
- [ ] PV cell temp and power validated
- [ ] Generator fuel consumption validated
- [ ] Control signal stub issue caught and documented
- [ ] Ramp rate units documented

## Verification

- [ ] `cargo test -p hares-equipment der_parity` passes
- [ ] `cargo clippy -p hares-equipment` passes
