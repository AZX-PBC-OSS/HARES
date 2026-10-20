---
reproduction_outcome: confirmed
failing_tests:
  - command: cargo test -p hares-core --lib actors::ev_driver::tests::telemetry_reflects_charging_while_equipment_soc_rises
    path: crates/hares-core/src/actors/ev_driver/mod.rs
    name: telemetry_reflects_charging_while_equipment_soc_rises
---

## Reproduction

The RCA (`rca.md`, "Root cause") identifies three telemetry gaps in
`EvDriverActor` that together make the driver's own channels inert during
goal-based charging: `charge_kw` is only populated from per-step rate
dispatches that goal-based strategies never emit, `needed_charge_hours`
collapses the "no estimate" sentinel to `0.0`, and `estimated_soc` is only
reconciled to equipment ground truth at two sparse trigger points (arrival
and day-start-while-plugged-in), never while charging at home.

The permanent failing test is
`actors::ev_driver::tests::telemetry_reflects_charging_while_equipment_soc_rises`
in `crates/hares-core/src/actors/ev_driver/mod.rs`.

### What the test does

It is pitched at the broken rule, not the reported path: **while the
vehicle is plugged in at home and the equipment is actively charging, the
driver's own telemetry must reflect that charging — for every strategy that
governs the session.** It exercises the four goal-based strategies the RCA
names as affected (`Immediate`, `Nightly` inside its off-peak window,
`LowSoc` below its threshold, `QuickThenWait` below its partial SOC), using
the module's existing test scaffolding (`make_plugged_in_actor`,
`set_core_soc`, `env_at_minute`).

For each strategy the actor starts `HomePluggedIn` at `estimated_soc = 0.5`
and is stepped 120 times at one-minute resolution while the mocked
equipment-core SOC rises every step (0.5 → 0.619), simulating the equipment
BMS actively charging — exactly the ticket's scenario, where the EV visibly
draws real energy. After each step the test reads the actor's own telemetry
channels (`charge_kw`, `soc`, `needed_charge_hours`) and asserts, per
strategy, that across the window:

- `charge_kw` was positive at least once,
- `soc` rose at least once,
- `needed_charge_hours` was positive at least once (SOC sits ~0.4 below
  target the entire window).

Failures are collected across all strategies and reported together, so the
output names every (strategy, channel) gap at once rather than stopping at
the first.

### Observed failure (before fix)

```
cargo test -p hares-core --lib actors::ev_driver::tests::telemetry_reflects_charging_while_equipment_soc_rises
```

fails with all twelve (4 strategies × 3 channels) gaps reported:

```
driver telemetry did not reflect charging:
  Immediate: charge_kw telemetry stayed 0.0 across 120 steps of active charging
  Immediate: soc telemetry never rose while equipment SOC climbed 0.5 → 0.619
  Immediate: needed_charge_hours telemetry stayed 0.0 while SOC sat 0.4 below target
  Nightly (in off-peak window): charge_kw telemetry stayed 0.0 across 120 steps of active charging
  Nightly (in off-peak window): soc telemetry never rose while equipment SOC climbed 0.5 → 0.619
  Nightly (in off-peak window): needed_charge_hours telemetry stayed 0.0 while SOC sat 0.4 below target
  LowSoc (below threshold): charge_kw telemetry stayed 0.0 across 120 steps of active charging
  LowSoc (below threshold): soc telemetry never rose while equipment SOC climbed 0.5 → 0.619
  LowSoc (below threshold): needed_charge_hours telemetry stayed 0.0 while SOC sat 0.4 below target
  QuickThenWait (below partial): charge_kw telemetry stayed 0.0 across 120 steps of active charging
  QuickThenWait (below partial): soc telemetry never rose while equipment SOC climbed 0.5 → 0.619
  QuickThenWait (below partial): needed_charge_hours telemetry stayed 0.0 while SOC sat 0.4 below target
```

This matches the RCA's scratch reproduction (flat `charge_kw`, inert `soc`)
and extends it to every affected strategy and to the `needed_charge_hours`
sentinel collapse, which the RCA had only traced in source. It targets the
root cause (driver telemetry never reflects charging under goal-based
control) rather than the symptom of one route: the same mistake made in a
sibling strategy would fail this test, and a cosmetic fix that addresses
only one channel or one strategy still leaves it red.

### Scope check

The rest of the ev_driver suite is unaffected: `cargo test -p hares-core
ev_driver` reports 155 passed, 1 failed — the only failure is the new
reproduction test.

The test drives `EvDriverActor` directly, so it is independent of both
construction routes' wiring; the RCA established the gap is architectural,
not route-specific.

### Addendum (fix step)

The fix supplemented the test's environment mock — not its assertions — with
the observation the production equipment always publishes alongside its SOC:
while charging, the EV equipment commits both `state.soc` and
`flows.electric_kw` to `CoreOutput` (hares-equipment/src/ev/mod.rs, `step()`'s
CoreOutput construction), so the test env now supplies both via the
`set_core_charging` helper (3.6 kW, consistent with the 0.001-SOC-per-minute
rise on a 60 kWh pack). Every assertion and failure message is unchanged,
and the test still failed identically against the pre-fix code for all
twelve (strategy, channel) gaps — the old `populate_telemetry` never read
the environment at all, so the added observation could not have changed the
pre-fix outcome. Rationale and alternatives are recorded in the ticket's
`## Implementation Notes → ### Deviations`.
