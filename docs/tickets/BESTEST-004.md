---
id: BESTEST-004
title: Review IdealHVAC cooling capacity fix
kind: review
depends_on: [BESTEST-003]
files_to_touch:
  - crates/hares-core/src/dwelling/synthetic.rs
  - crates/hares-equipment/src/hvac/ideal_hvac.rs
references:
  - docs/tickets/BESTEST-003.md
verification:
  - cargo test -p hares-core --test bestest -- --nocapture
  - cargo test --workspace --exclude hares-python --lib
---

## Background/Context

Review the IdealHVAC cooling capacity fix from BESTEST-003, if it was needed.

## Work to Do

- [ ] Verify the fix doesn't affect non-ideal HVAC equipment
- [ ] Verify the fix doesn't break parity tests (which use real HVAC, not IdealHVAC)
- [ ] Confirm the cooling capacity value is physically reasonable for BESTEST spec
- [ ] Check that the fix approach doesn't introduce silent defaults (per project conventions)

## Measures of Success

- [ ] Fix is targeted and minimal
- [ ] No side effects on other HVAC paths
- [ ] All verification commands pass

## Verification

- [ ] `cargo test -p hares-core --test bestest -- --nocapture` passes
- [ ] `cargo test --workspace --exclude hares-python --lib` passes
