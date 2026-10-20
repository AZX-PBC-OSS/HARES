---
coverage_outcome: adequate
new_test_paths:
  - crates/hares-core/src/actors/ev_driver/mod.rs
  - crates/hares-core/src/actors/ev_driver/soc_target.rs
failing_tests: []
---

## Triage of I-04 coverage rounding

### Artifacts reviewed

Read in full: `rca.md` (three-channel root cause: charge_kw rate-only
scan, needed_charge_hours sentinel collapse, soc reconciled only at two
sparse triggers), `reproduction.md` (permanent 4-strategies × 3-channels
test, green post-fix), `coverage-note.md` (this pass), and the
constitution. Every load-bearing claim was checked against the current
tree, not taken from summaries.

### Independent verification (this run)

The tree continues to move as fix work lands — `ev_driver` now reports
**177 passed, 0 failed** (the coverage note recorded 171). Re-verified
against the current tree:

- All four tests the coverage pass added still exist and pass:
  `charge_kw_is_zero_while_driving`,
  `charge_kw_reports_negative_observed_export_flow` (mod.rs),
  `needed_charge_hours_infinite_without_charge_rate`,
  `needed_charge_hours_unrounded_when_timestep_zero` (soc_target.rs).
- `cargo test -p hares-core --lib ev_driver` → 177 passed, 0 failed.
- `cargo clippy -p hares-core --lib --tests -- -D warnings` → clean.
- `cargo fmt -p hares-core -- --check` → clean.
- The note's "deliberately not covered" claims hold by direct inspection:
  the Away present/absent cases are pinned by
  `charge_kw_reports_away_charging_on_non_dispatch_steps` (asserts both
  the observed 6.6 kW path and the 0.0 degradation when the observation
  is absent), and the enum-exhaustive guard
  `every_strategy_variant_telemetry_reflects_active_charging` covers all
  9 `ChargingStrategy` variants — satisfying the constitution's
  enum-exemption class-risk rule.
- The fix's paths beyond the note's scope (composer
  `refresh_needed_charge_hours`/`set_needed_charge_hours`, soc_gate
  override, non-driving-day refresh, checkpoint restore, multi-day
  reconciliation) each carry their own passing tests, confirmed by name
  in this run's test output. No untested path introduced.

### Value judgment per added test

Applied "what goes wrong if this isn't covered?" honestly:

- `charge_kw_is_zero_while_driving` — pins the Driving arm reading 0.0
  despite an observable equipment flow; without it, double-counting
  observed flow while driving would silently corrupt the truthful-face
  telemetry contract. Real value.
- `charge_kw_reports_negative_observed_export_flow` — pins signed
  pass-through (no clamping of export to 0.0), the constitution's
  "truthful about the face it is measured at" rule on exactly the
  observation path the fix introduced; clamping is the plausible future
  "cleanup" mistake. Real value.
- `needed_charge_hours_infinite_without_charge_rate` — pins the INFINITY
  guard with inputs chosen so its removal panics or yields NaN; the test
  cannot survive deletion of what it covers. Real value.
- `needed_charge_hours_unrounded_when_timestep_zero` — thinnest of the
  four: a broken unrounded branch means rounding-level drift on a
  degenerate zero-timestep configuration normal runs never hit. Below
  ticket bar on its own, but cheap and pins a real branch.

No padding, no tests asserting internals, no mock-the-world setups — each
test asserts observable telemetry or helper outputs.

### Gaps check

No path near the change lacks coverage that would catch a real
regression. The RCA's most extreme scenario (HPXML route,
`estimated_soc` stuck at 0.0 across days by missing both reconciliation
triggers) is pinned by the multi-day reconciliation tests, observed
passing in this run. The note's test count (171) is stale relative to the
current 177 — the fix suite keeps growing; that is a historical-record
inaccuracy, not a coverage gap, and is named here so it is not silent.

### Reassessment (challenge pass)

**Overreach.** None. This run read artifacts, ran the existing suite,
clippy, and fmt, and wrote only this triage document. No fix applied, no
test written, no production or test file touched.

**Every assessed item and why no action is warranted:**

1. The four coverage-pass tests — per-test reasons in the value-judgment
   section. The thinnest (`needed_charge_hours_unrounded_when_timestep_zero`)
   guards a degenerate-config rounding branch: if wrong, cosmetic telemetry
   drift on a configuration normal runs never hit. Below ticket bar.
2. The note's "deliberately not covered" claims — verified by direct
   inspection (Away present/absent test; 9-variant enum-exhaustive guard).
3. The fix's paths beyond the note's scope — each observed with its own
   passing test by name in this run's output.
4. The note's stale test count (171 vs current 177) — historical-record
   inaccuracy in a moving tree; the note's coverage claims remain true.
   Named here so it is not silent; not a coverage gap.
5. The driver↔equipment seam (new this pass): the driver's
   `observed_charge_kw` reads `CoreOutput.flows.electric_kw`
   (mod.rs:427) and the unit tests construct that boundary struct
   directly. Checked whether this is the constitution's
   mock-stands-in-for-the-integration case. It is not: `CoreOutput` is a
   true boundary type, the publish side is pinned by equipment-level
   tests (`lifecycle.rs:205` asserts `flows.electric_kw.is_some()` after
   step), and dwelling-level regression tests read real equipment
   CoreOutputs (`core_output_regressions.rs:157`). The only unpinned
   sliver is the generic dwelling-loop plumbing routing CoreOutput into
   the actor's `EnvironmentState` — pre-existing, shared by all actors,
   not introduced by this fix. Below ticket bar.

**Strongest case for action.** Item 5 is the strongest candidate: if the
equipment ever published `None` for `electric_kw` while charging, the
driver's new observation path would silently degrade to dispatched-rate
or 0.0 and the driver-level unit tests would stay green (they supply the
field themselves). It still fails the routing test: the publish side is
pinned at equipment level, a struct-field removal is a compile error at
the read site, and the failure mode requires the equipment to start
omitting a field its own tests assert is present — a regression those
tests catch first.

**Riskiest unaddressed finding.** None unaddressed rises to actionable.
The riskiest *covered* item remains the signed-export pass-through
(negative `charge_kw`): a future "clamp to non-negative" cleanup would
silently violate the truthful-face contract — held by
`charge_kw_reports_negative_observed_export_flow`, not left open.

### Verdict

Coverage is **adequate**. The four added tests pin real, previously
unpinned branches adjacent to the fix; every claim in the note that still
matters reproduces against the current tree; no defect surfaced. No
follow-up work routed.
