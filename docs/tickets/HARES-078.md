---
id: HARES-078
title: Dwelling orchestration and aggregation correctness tests
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-core/tests/orchestration_parity.rs
  - tests/fixtures/parity/orchestration/
references:
  - vendors/OCHRE/ochre/Dwelling.py
  - crates/hares-core/src/dwelling.rs
  - crates/hares-core/src/engine.rs
  - crates/hares-types/src/ports.rs
verification:
  - cargo test -p hares-core orchestration_parity
  - cargo clippy -p hares-core
---

## Background/Context

Orchestration audit found: HARES batches all non-thermal equipment, then runs thermal equipment with stale zone temps. OCHRE updates zone state between each equipment step so HVAC sees current gains. This causes thermostat decisions to be 1-timestep delayed. Tests must verify execution ordering, zone temp feedback, and power aggregation.

## Work to Do

- [ ] Create `crates/hares-core/tests/orchestration_parity.rs`
- [ ] **Test: execution order** — Create a minimal dwelling with 1 ScheduledLoad + 1 Furnace + 1 Envelope. Instrument step to record execution order. Verify: ScheduledLoad runs before Furnace, Furnace runs after internal gains from ScheduledLoad are available
- [ ] **Test: zone temp feedback to HVAC** — ScheduledLoad contributes 2kW sensible gain. Verify furnace's thermostat decision reflects this gain (zone temp should be higher than without the load). Compare to scenario with 0kW load — HVAC duty cycle should differ
- [ ] **Test: total electric aggregation** — 3 equipment: ScheduledLoad@1.5kW, AC@2.0kW, fan@0.4kW. Verify total_electric_kw = 3.9kW
- [ ] **Test: total gas aggregation** — Furnace@15kW gas + WH@5kW gas. Verify total_gas_w = 20kW (or equivalent therms/hr)
- [ ] **Test: internal heat gain summation** — ScheduledLoad (sensible=0.7, latent=0.03) at 1kW + second load at 0.5kW. Verify total sensible and latent gains fed to envelope match expected sums
- [ ] **Test: port value flow end-to-end** — Step a minimal dwelling 10 steps. At each step verify: equipment deposits to ports → ports feed envelope → envelope updates zone temps → zone temps available to next step's equipment
- [ ] **Test: no stale zone temp** — This is the key regression test. Add a 5kW internal gain at step 5. Verify the HVAC response at step 5 (not step 6) reflects the new gain. If HARES currently fails this, mark as expected failure with clear documentation

## Measures of Success

- [ ] Execution ordering validated
- [ ] Aggregation arithmetic proven correct
- [ ] Zone temp feedback latency documented (passes or expected-failure with explanation)
- [ ] Port flow verified across multiple steps

## Verification

- [ ] `cargo test -p hares-core orchestration_parity` passes
- [ ] `cargo clippy -p hares-core` passes
