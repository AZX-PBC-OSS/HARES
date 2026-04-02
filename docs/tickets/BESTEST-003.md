---
id: BESTEST-003
title: Fix IdealHVAC cooling capacity for BESTEST 600
kind: fix
depends_on: [BESTEST-001]
files_to_touch:
  - crates/hares-core/src/dwelling/synthetic.rs
  - crates/hares-equipment/src/hvac/ideal_hvac.rs
references:
  - /home/rich/.claude/plans/dapper-hatching-gosling.md
  - docs/tickets/BESTEST-001.md
verification:
  - cargo test -p hares-core --test bestest -- bestest_case_600 --nocapture
  - cargo test -p hares-core --test parity -- --nocapture
  - cargo test --workspace --exclude hares-python --lib
---

## Background/Context

**Only proceed with this ticket if BESTEST 600 cooling still fails after BESTEST-001.**

`synthetic.rs:299` sets `has_cooling = false` when `is_ideal_hvac = true`, preventing CoolingSystem XML node creation. This means `cooling_capacity_w` is never configured and IdealHvac defaults to 10,000 W cooling capacity. For BESTEST Case 600 (ASHRAE 140), the HVAC system should be effectively ideal with sufficient capacity to maintain setpoints.

ASHRAE 140 specifies the HVAC system as "ideal" meaning unlimited capacity — the system should always maintain setpoint exactly. OCHRE caps ideal capacity to `capacity_max`, but that's an OCHRE implementation choice, not a spec requirement. For BESTEST compliance, the correct behavior is no capacity constraint. Option B (high default like f64::MAX or a very large value) is the simpler and more spec-correct approach.

## Work to Do

- [ ] After BESTEST-001, run `cargo test -p hares-core --test bestest -- bestest_case_600 --nocapture` and record cooling load result
- [ ] If cooling still fails: investigate peak cooling demand from observer data
- [ ] Choose fix approach:
  - Option A: Pass `cooling_capacity_w` through the IdealHVAC config params in `synthetic.rs` (allows TOML-level control)
  - Option B: Set a higher default cooling capacity for IdealHvac (e.g., 100 kW) since ASHRAE 140 specifies ideal unlimited capacity
- [ ] Implement chosen fix
- [ ] Verify BESTEST 600 cooling enters acceptable band

## Files to Touch

- `crates/hares-core/src/dwelling/synthetic.rs`: Either pass cooling_capacity_w through params or fix `has_cooling` logic
- `crates/hares-equipment/src/hvac/ideal_hvac.rs`: Possibly adjust default cooling capacity

## Measures of Success

- [ ] BESTEST 600 annual cooling load falls within acceptable band (6137-7964 kWh)
- [ ] No regression in other BESTEST cases or parity tests

## Verification

- [ ] `cargo test -p hares-core --test bestest -- bestest_case_600 --nocapture` — cooling in band
- [ ] `cargo test -p hares-core --test parity -- --nocapture` — no regression
- [ ] `cargo test --workspace --exclude hares-python --lib` passes
