---
reproduction_outcome: confirmed
failing_tests:
  - command: cargo test -p hares-core --lib actors::ev_driver::tests::idle_nightly_window_does_not_charge_equipment
    path: crates/hares-core/src/actors/ev_driver/mod.rs
    name: idle_nightly_window_does_not_charge_equipment
  - command: cargo test -p hares-core --lib actors::ev_driver::tests::idle_low_soc_gate_does_not_charge_equipment
    path: crates/hares-core/src/actors/ev_driver/mod.rs
    name: idle_low_soc_gate_does_not_charge_equipment
  - command: cargo test -p hares-core --lib actors::ev_driver::tests::range_anxiety_override_does_not_preempt_low_soc_gate
    path: crates/hares-core/src/actors/ev_driver/mod.rs
    name: range_anxiety_override_does_not_preempt_low_soc_gate
  - command: cargo test -p hares-core --lib actors::ev_driver::tests::range_anxiety_override_does_not_preempt_nightly_window
    path: crates/hares-core/src/actors/ev_driver/mod.rs
    name: range_anxiety_override_does_not_preempt_nightly_window
---

# Reproduction: actor silence is filled by the equipment's charge-to-full default

This document supersedes the prior revision-1 reproduction for this
initiative. The RCA (revision 2) re-scoped the root cause: the primary
defect is not the range-anxiety override but the undefined actor/equipment
"silence" contract — when a `ChargingStrategy` decides not to charge, the
actor dispatches nothing, and a plugged-in EV receiving no instruction
charges itself toward 100% at rated power (`compute_charging_power_kw`
falls through to `ready_soc`, default `soc_max` = 1.0). The two new tests
below target that primary defect at the equipment level, which is where
the ticket's symptom actually lives.

## Cause (from the RCA, restated)

An HPXML-declared EV with an `Equipment["EV"].charging_strategy` override
sends the strategy to the actor but never sets the equipment's `ready_soc`.
Whenever the strategy idles (`Nightly` outside its window, `LowSoc` above
its gate), the composer's idle vote carries `target_soc: None,
power_kw: None` and `ChargingComposer::emit_vote` pushes no dispatch at
all — there is no "hold" signal. The equipment, silent-stepped, applies
its BMS default: charge to `ready_soc` (= `soc_max` = 1.0). Every strategy
therefore converges on charge-on-plug-in, producing the ticket's identical
profiles.

## Tests written

Two new tests added to the existing `#[cfg(test)]` module in
`crates/hares-core/src/actors/ev_driver/mod.rs`. Each wires a real
`EvDriverActor` (via the module's existing `make_plugged_in_actor` helper,
SOC 0.5 — deliberately above the ~0.25 range-anxiety band, so the override
cannot be the mechanism) to a real `hares_equipment::ev::Ev` built the way
the ticket's override leaves it (60 kWh, 7.2 kW L2, `initial_soc` 0.5,
**no `ready_soc`**). The pair is stepped together from 18:00 to 22:00 at
5-minute resolution, applying whatever the actor dispatches to the
equipment each step. The assertion is on observable equipment state, not
on dispatch contents: SOC at 22:00 must still be the plug-in 0.5, because
the strategy said "wait" for the entire window.

- `idle_nightly_window_does_not_charge_equipment`: `Nightly
  22:00–06:00` — the whole 18:00–22:00 span is outside the window.
- `idle_low_soc_gate_does_not_charge_equipment`: `LowSoc{0.15}` — SOC 0.5
  is above the gate, so the gate holds for the whole span.

The pair brackets the two policy classes in the ticket's completion bar
(time-window gating, SOC-threshold gating) and targets the broken rule —
"an idling strategy must produce no equipment charging" — not the single
reported path: any strategy variant whose composer idles
(`QuickThenWait`, `TouAware`, `SolarSurplus`, …) fails this same
invariant through the same mechanism.

The two revision-1 tests (`range_anxiety_override_does_not_preempt_*`)
remain in the file and still fail; they cover the secondary,
independently-confirmed defect the RCA says must also land for the
ticket's completion bar (the urgency override preempting strategy gating
in the 0.15–0.28 SOC band). They are listed in `failing_tests` alongside
the new pair. Note the interplay the RCA flags: a fix that makes the
composer emit a latching hold (e.g. `PowerSetpoint{0.0}`) without a
clearing rule could make these tests pass while producing the opposite
failure mode (never charges) — the fix phase must weigh fact (c) of the
RCA.

## Confirmation — observed failures

Ran:

```
cargo test -p hares-core --lib actors::ev_driver::tests::idle_ -- --nocapture
```

Result: **both tests FAILED** with the reproduced mechanism in the panic
messages — in each case the EV charged from 0.5 to 0.932 during the
4-hour window in which its configured strategy said "wait" and the actor
dispatched nothing:

```
Nightly 22:00-06:00 is idle from 18:00 to 22:00, so the EV must hold its
plug-in SOC (0.5) through that window — instead it charged to 0.932,
meaning the actor's idle decision was silently overridden by the
equipment's charge-to-full BMS default
```

```
SOC 0.5 is above LowSoc's configured 0.15 gate, so the EV must hold its
plug-in SOC (0.5) while the gate holds — instead it charged to 0.932,
meaning the actor's idle decision was silently overridden by the
equipment's charge-to-full BMS default
```

The two revision-1 tests still fail as before — re-run this session via
`cargo test -p hares-core --lib actors::ev_driver::tests::range_anxiety_override`:
both `range_anxiety_override_does_not_preempt_*` tests FAILED with the
same `SOCTarget Some(1.0)` preemption messages as in the prior flow step.

`cargo fmt --check -p hares-core` and
`cargo clippy -p hares-core --lib --tests -- -D warnings` both pass with
all four tests in place. The tests assert post-fix behaviour (equipment
respects an idling strategy), so they must pass once the fix lands —
whether the fix adds an explicit hold signal to the composer's idle path,
changes the equipment's actor-controlled default, or both. They are
deliberately not marked `#[should_panic]`: they are the gate commands for
the fix phase and must fail until then.
