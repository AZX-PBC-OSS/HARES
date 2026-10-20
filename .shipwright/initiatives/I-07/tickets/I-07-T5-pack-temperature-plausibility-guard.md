---
id: I-07-T5
title: "Pack/cell temperature has no plausibility bound at any validation choke point"
status: open
created_at: 2026-09-24
created_by: i07-retrospective
---

# Pack/cell temperature has no plausibility bound at any validation choke point

## Problem

I-07's root cause shipped silently because no layer asked "is this
temperature physical?": a misattributed `(1−η)·P` heating term drove EV
pack temperatures to 165–281 °C on routine L2 sessions, in every
climate, behind green test suites.

The fix closed the *instance* and left two mechanisms:

- the degradation model's cell-temperature domain guard
  (`crates/hares-equipment/src/battery/degradation.rs`, error bounds
  [−100, +130] °C) — a hard error, but only at the degradation
  accumulation point, and only for equipment that uses the shared
  degradation model;
- the regression tests in
  `crates/hares-core/tests/ev_thermal_regressions.rs` — EV-specific.

What does not exist: a *general* bound at a validation choke point.
`hares-types` already has the right pattern —
`validate_core_contract`'s physical-range layer
(`crates/hares-types/src/equipment.rs`, reject/warn split with cited
bounds for COP, main power, cooling sign conventions, setpoints,
thermal output, plus the `RangeViolationEvent` observability ring) —
but `CoreState` (`equipment.rs:1189-1198`) carries no pack/cell
temperature field, so the layer cannot bound the quantity this bug
class corrupts. A future equipment family with thermal state (or a
future EV thermal regression that stops short of the degradation
guard's ±100 °C-class envelope) has no net.

## Proposed mechanism

Carry pack/cell temperature (or a general `state.temp_c` channel)
through `CoreState` for equipment that declares a thermal-state
capability, and bound it in `validate_core_contract`'s range layer:
reject outside a physical envelope (the degradation guard's
[−100, +130] °C is the existing precedent), warn outside a plausible
operating band. This converts the constitution's "assert physical
plausibility" principle from test-discipline into a runtime guarantee
for every equipment type at one site.

## Boundaries and alternatives

- **Deliberately not done in the retrospective pass:** adding a field
  to `CoreState` is a cross-cutting API change (every equipment's
  CoreOutput construction, the mode-flow guard's consumers, Python
  bindings, telemetry columns). It is a design decision for the team,
  not a drive-by.
- **Alternative:** an equipment-level `validate_thermal_state` hook on
  the `Equipment` trait, called from the same validation site, avoiding
  the telemetry-channel change. Weaker (opt-in per equipment) but no
  API churn.
- **Related survivor, recorded for context:** the pack *thermal*
  models (heater gating + ambient relaxation) remain per-family —
  `battery/mod.rs:1346-1367` and `ev/mod.rs:1644-1709` carry the same
  equation shape in two bodies. The I-07 fix plan deliberately chose
  behavioral alignment over structural merge, and the electrical half
  is shared (`pack_electrical.rs`). If a third pack-thermal consumer
  ever appears, extract a shared thermal module at that point rather
  than writing a third body (constitution, "One model per physical
  phenomenon").
