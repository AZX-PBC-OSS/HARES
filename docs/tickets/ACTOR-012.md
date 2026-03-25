---
id: ACTOR-012
title: Occupant actor — lights, appliances, EV behavioral control
kind: implement
depends_on:
  - ACTOR-003
  - ACTOR-001
files_to_touch:
  - crates/hares-core/src/actors/occupant.rs
  - crates/hares-core/src/actors/mod.rs
  - crates/hares-core/src/dwelling/mod.rs
references:
  - crates/hares-equipment/src/scheduled_load.rs
  - crates/hares-equipment/src/event_load.rs
  - crates/hares-equipment/src/ev/mod.rs
  - crates/hares-types/src/control_signal.rs
  - docs/tickets/ACTOR-INDEX.md
verification:
  - cargo build --workspace
  - cargo test --workspace
---

## Background/Context

Equipment is self-contained: lighting has its own power schedule, appliances have their own cycle profiles, HVAC has its thermostat schedule. These internal schedules are part of the equipment and stay there.

The Occupant actor models *external human behavior* that acts on equipment from outside:
- Turning lights off when leaving the room (override equipment's default schedule)
- Starting a washing machine cycle (trigger an event on the appliance)
- Plugging/unplugging an EV (connect/disconnect)
- Changing EV charge mode (eco/fast)
- Overriding thermostat setpoint (covered by ACTOR-009)

This is foundational for DR RL simulation where the occupant must decide "do I comply with this DR request?" — which requires modeling presence, preferences, and decision-making.

Equipment already handles these signals via `apply_control_unchecked` (ScheduledLoad supports LOAD_FRACTION, MODE_OVERRIDE; EV supports POWER_SETPOINT, SOC_TARGET). The actor just needs to push them.

## Work to Do

- [ ] Create `crates/hares-core/src/actors/occupant.rs` implementing `Actor` trait
- [ ] Occupant fields: name, presence schedule (home/away/sleeping), behavioral preferences
- [ ] `decide(env, out)` dispatches ControlSignals only — never mutates environment or equipment state directly. Equipment receives signals via `apply_control` and adjusts its own internal operational state. Examples:
  - `ModeOverride { mode: Off }` → lighting equipment sets internal state to off → `step()` produces 0W
  - `ModeOverride { mode: On }` → washing machine sets internal state to running → `step()` produces rated power draw + thermal gain
  - EV: use existing `ModeOverride { mode: Off }` for disconnect, `ModeOverride { mode: On }` for connect → EV equipment sets plugged/unplugged state → `step()` computes charge power or 0W. Use `PowerLimit` to change charge rate. No new signal types needed — existing ControlSignal variants cover EV use cases.
  - When occupant is home and not intervening, emit nothing — equipment runs on its internal schedule
- [ ] `decide(env, out)` pushes into pre-allocated buffer (ACTOR-008 pattern), zero per-step allocation
- [ ] Occupant presence derived from schedule or occupancy column — stored as pre-computed array, not cloned each step
- [ ] Pass `&EnvironmentState` by immutable reference — actors never mutate environment
- [ ] Constructor: `from_schedule(schedule: &ScheduleTimeSeries, building: &Building) -> Self`

## Files to Touch

- `crates/hares-core/src/actors/occupant.rs`: **New** — Occupant actor
- `crates/hares-core/src/actors/mod.rs`: Export occupant module
- `crates/hares-core/src/dwelling/mod.rs`: Optionally create Occupant actor in from_config()

## Measures of Success

- [ ] Occupant actor emits control signals that equipment responds to
- [ ] Without actor: equipment runs on internal schedules (baseline behavior unchanged)
- [ ] With actor: external overrides modify equipment behavior (lights off when away, etc.)
- [ ] Actor can be subclassed/replaced for DR compliance modeling
- [ ] EV plug/unplug events flow through actor→equipment control signals

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
