---
id: ACTOR-003
title: Define Actor trait and wire into Dwelling timestep loop
kind: implement
depends_on:
  - ACTOR-001
files_to_touch:
  - crates/hares-core/src/actor.rs
  - crates/hares-core/src/lib.rs
  - crates/hares-core/src/dwelling/mod.rs
  - crates/hares-types/src/lib.rs
references:
  - crates/hares-control/src/dispatch.rs
  - crates/hares-types/src/control_signal.rs
  - docs/tickets/ACTOR-INDEX.md
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace
---

## Background/Context

HARES needs a clean actor model where decision-makers (occupants, thermostats, grid operators, DR programs) push commands to equipment via control channels. Currently, schedules are pull-based iterators that equipment reads from EnvironmentState each step. The actor trait inverts this: actors decide each step and emit DispatchRequests that flow through the existing ControlDispatcher.

Key principles:

1. **Equipment is self-contained** with internal schedules and control logic needed to function normally (thermostat profiles, fan curves, appliance cycles).
2. **Actors only dispatch ControlSignals** — they never directly mutate environment or equipment state. Actors are pure decision-makers that emit signals.
3. **Equipment receives signals and adjusts internal operational state** (is TV on/off, is EV plugged/unplugged, thermostat setpoint override). The equipment's `step()` then computes physical outputs (power draw, thermal gain) from that operational state during the normal equipment sim loop.
4. **Config is separate from control signals.** Config sets up equipment at init time (capacity, efficiency curves, schedules). Control signals adjust operational state at runtime (on/off, setpoint override, charge mode). Actors dispatch signals, not config.

This ticket defines the trait and wires it into the timestep loop. No actors are implemented yet — that happens in subsequent tickets.

## Work to Do

- [x] Create `crates/hares-core/src/actor.rs` with the `Actor` trait:
  ```rust
  pub trait Actor: Send + Sync {
      fn name(&self) -> &str;

      /// Declare what state changes this actor cares about.
      /// Default: empty (polled every step). When the dwelling implements
      /// interest-based filtering, actors with declared interests are only
      /// called when relevant state changes occur.
      fn interests(&self) -> &[ActorInterest] { &[] }

      /// Called once per timestep (or when interests trigger).
      /// Pushes dispatch requests into the pre-allocated buffer.
      /// MUST NOT allocate — use the provided buffer only.
      fn decide(&mut self, env: &EnvironmentState, out: &mut Vec<DispatchRequest>);
  }

  /// What state changes an actor subscribes to. Empty = polled every step.
  /// Dwelling can use these to skip actors whose interests haven't fired,
  /// reducing decision calls ~60x for sparse actors at 1-min resolution.
  pub enum ActorInterest {
      EveryStep,                                    // Default polling
      ZoneTemperatureDelta { zone: ZoneId, threshold_c: f64 },
      EquipmentModeChange { target: DispatchTarget },
      TimeOfDay { hour: u8 },
      PriceSignalChange,
  }
  ```
  The `out` buffer is owned by the Dwelling, pre-allocated once, and cleared+reused each step (swapchain pattern). Actors append to it; the dwelling drains it into the dispatcher. This avoids per-step Vec allocation in the hot loop.
  Initially the dwelling ignores `interests()` and polls every actor every step. The trait contract is set so interest-based filtering can be added later without a breaking change.
- [x] Export from `crates/hares-core/src/lib.rs`
- [x] Add `actors: Vec<Box<dyn Actor>>` field to `Dwelling` struct
- [x] Initialize as empty `Vec::new()` in `from_config()` and `from_preparsed()`
- [x] Add `pub fn add_actor(&mut self, actor: Box<dyn Actor>)` method to Dwelling
- [x] Wire into `run_timestep()` — insert actor decision step between environment update (step 1) and control dispatch (step 2):
  ```rust
  // Step 1b: actors decide and queue control signals (registration order, last write wins)
  self.actor_dispatch_buf.clear(); // pre-allocated Vec<DispatchRequest>, reused each step
  for actor in &mut self.actors {
      actor.decide(&self.latest_env, &mut self.actor_dispatch_buf);
  }
  for req in self.actor_dispatch_buf.drain(..) {
      self.control_dispatcher.queue(req);
  }
  // Step 2: dispatch (existing)
  self.control_dispatcher.dispatch_into(&mut self.equipment, &mut self.warnings);
  ```
- [x] Add `actor_dispatch_buf: Vec<DispatchRequest>` field to Dwelling, pre-allocated in construction
- [x] Document actor ordering contract: actors execute in registration order. Dispatch applies signals sorted by PriorityTier (ACTOR-001) — highest priority wins when multiple signals target the same equipment.
- [x] Add `tracing::debug!` in dispatch when multiple signals target the same equipment in the same step — conflict visibility without hot-path overhead (behind tracing level gate)
- [x] Feature-gated actor profiling (`#[cfg(feature = "actor_profiling")]`): per-actor `decide()` timing in the dwelling loop. Store `per_actor_timing: Vec<(String, Duration)>` on Dwelling, log summary at end of simulation.
- [x] Add actor test helpers in `crates/hares-core/src/actor.rs`:
  ```rust
  pub mod testing {
      //! Test helpers for actor development.
      //! Available for downstream actor implementations to use in their own tests.
      
      /// Builder for EnvironmentState with sensible defaults for actor testing.
      pub fn test_env() -> TestEnvBuilder { ... }
      /// Assert that dispatch requests contain a heating thermal setpoint.
      pub fn assert_is_heating(requests: &[DispatchRequest], expected_heating_c: f64) { ... }
      /// Assert that dispatch requests contain a thermal setpoint targeting a specific temperature.
      pub fn assert_target_temp(requests: &[DispatchRequest], target_c: f64) { ... }
      /// Assert that dispatch requests target a specific equipment by name.
      pub fn assert_target_by_name(requests: &[DispatchRequest], name: &str) { ... }
      /// Asserts that the dispatch request count matches the expected value.
      pub fn assert_request_count(requests: &[DispatchRequest], expected: usize) { ... }
  }
  ```
  
  Note: The `testing` module is **not** guarded by `#[cfg(test)]` so downstream crates can use these helpers in their own tests.

## Files to Touch

- `crates/hares-core/src/actor.rs`: **New** — Actor trait definition + test helpers
- `crates/hares-core/src/lib.rs`: Export actor module
- `crates/hares-core/src/dwelling/mod.rs`: Add actors field, add_actor method, wire into timestep loop, optional profiling

## Measures of Success

- [x] Actor trait compiles and is publicly accessible
- [x] Dwelling accepts actors via `add_actor()`
- [x] Empty actor list has zero overhead (no dispatch requests emitted)
- [x] Test helpers compile and are usable in downstream actor tests
- [x] Existing tests pass unchanged (no actors present = no behavior change)

## Verification

- [x] `cargo build --workspace` passes
- [x] `cargo test --workspace` passes
- [x] `cargo clippy --workspace` passes

## Tests Added

### hares-core
- `actor_trait_name_returns_value`
- `actor_trait_interests_default_empty`
- `actor_trait_decide_emits_signals`
- `test_env_builder_creates_valid_environment`
- `assert_is_heating_finds_matching_signal`
- `assert_is_heating_panics_on_mismatch`
- `assert_target_by_name_finds_match`
- `assert_request_count_validates_count`

## Post-Review Fixes (2026-03-25)

- **Removed unsafe downcast**: The `impl dyn Actor` block with `downcast_ref()` method was removed. Standard `std::any::Any` should be used if downcasting is needed.
- **Added `EndUse::as_str()`**: Added explicit string representations for all EndUse variants.

## Post-Review Fixes (2026-03-25 - Second Round)

- **Fixed `actor_profiling` feature compilation**: `Instant` import gated behind `#[cfg(any(feature = "profiling", feature = "actor_profiling"))]`.
- **Fixed `assert_target_temp` test helper**: Removed broken `IdealCapacity` branch that ignored `target_c`. Now only validates `ThermalSetpoint` signals.
- **Fixed actor_profiling Vec reallocation**: Reuse `per_actor_timing` Vec with `clear()` instead of creating a new one each step.
- **Made testing module public**: Removed `#[cfg(test)]` guard so downstream crates can use test helpers.
- **Fixed TestActor allocation violation**: Changed from `clone()` to `std::mem::take` to avoid allocation in `decide()`.
- **Updated doc comment**: Changed "backward-compatibility" to "supporting synthetic TOML inputs" to align with greenfield project guidelines.

## Post-Review Fixes (2026-03-25 - Third Round)

- **Zero-alloc conflict detection**: Replaced `DispatchTarget::conflict_key() -> String` with `DispatchTarget::conflicts_with(&Self) -> bool` — zero-allocation reference comparison. `ControlDispatcher::seen_targets` changed from `HashSet<String>` to `Vec<DispatchTarget>` with linear scan (correct for typical <16 requests/step, avoids hash overhead and per-request String allocation).
- **Fixed conflict log message**: Changed from ambiguous "lower priority signal also targeting same equipment (will be overwritten)" to "higher priority signal overwriting earlier signal for same equipment" — tiers iterate low→high so the logged signal is always the higher-priority one that wins.
- **Documented apply_control overwrite contract**: `ControlDispatcher` doc now states that all tier signals fire (no deduplication) and equipment `apply_control` must be overwrite-safe (idempotent set, not accumulate) since last write wins.
