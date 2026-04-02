---
id: BESTEST-002
title: Review layer ordering convention change
kind: review
depends_on: [BESTEST-001]
files_to_touch:
  - crates/hares-envelope/src/boundary_rc.rs
  - crates/hares-core/src/dwelling/conversions.rs
  - crates/hares-core/src/dwelling/synthetic.rs
  - tests/fixtures/bestest/600.toml
  - tests/fixtures/bestest/600ff.toml
  - tests/fixtures/bestest/640.toml
  - tests/fixtures/bestest/900.toml
  - tests/fixtures/bestest/900ff.toml
references:
  - /home/rich/.claude/plans/dapper-hatching-gosling.md
  - docs/tickets/BESTEST-001.md
verification:
  - cargo test -p hares-envelope
  - cargo test -p hares-core --test bestest -- --nocapture
  - cargo test --workspace --exclude hares-python --lib
---

## Background/Context

Code review of the layer ordering convention change from BESTEST-001. This is a high-risk change touching the RC network construction path that affects every simulation. Must verify correctness of the wiring swap and that no index errors were introduced.

## Work to Do

- [ ] Verify `build_layered_boundary` wiring: interior film R connects to `layer_nodes[n-1]`, exterior film R connects to `layer_nodes[0]`
- [ ] Verify `build_precomputed_boundary` wiring: same swap applied consistently
- [ ] Verify same-zone truncation keeps the correct half (interior layers = end of list)
- [ ] Verify `inner_node` diagnostics point to the correct (zone-facing) RC node in both paths
- [ ] Verify `r_zone_to_inner` uses the interior-facing layer (last in list) in both paths
- [ ] Verify TOML fixture layers are correctly reversed (walls/roofs only, not floors)
- [ ] Verify no off-by-one errors in node ID calculations
- [ ] Verify return value semantics: first = inner (zone-facing), second = outer (exterior-facing)
- [ ] Check HPXML path (`building.rs:1420`) still produces correct ordering without changes
- [ ] Verify Envelope Materials CSV / LUT path produces correct results for a real HPXML fixture (not just BESTEST TOML) — run parity tests as proxy
- [ ] Verify the new `radiation_frac` assertion test covers the expected value range
- [ ] Confirm BESTEST test results improved (record exact values)

## Measures of Success

- [ ] No correctness issues found in the wiring swap
- [ ] All verification commands pass
- [ ] BESTEST results show improvement in the expected direction

## Verification

- [ ] `cargo test -p hares-envelope` passes
- [ ] `cargo test -p hares-core --test bestest -- --nocapture` passes or shows improvement
- [ ] `cargo test --workspace --exclude hares-python --lib` passes
