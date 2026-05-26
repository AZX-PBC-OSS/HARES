# Actor trait definition, registry, and dispatch ordering correctness
**Review ID**: agents-01
**Category**: agents
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-core/src/actor.rs`
- `crates/hares-core/src/actor_registry.rs`
- `crates/hares-core/src/engine.rs`
- `crates/hares-core/src/dwelling/mod.rs` (dispatch loop)
- `crates/hares-core/tests/dispatch_ordering_regressions.rs`
- `crates/hares-control/src/dispatch.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Dwelling.py`
- `vendors/OCHRE/ochre/Simulator.py`

## Findings

### Finding 1: [Severity: high]
**Description**: The `ActorInterest` filtering mechanism is declared and documented but never enforced in the simulation loop. Every actor's `decide()` is called on every timestep regardless of its declared interests. The doc comment on `ActorInterest` claims this can yield "~60x reduction in decision calls for sparse actors at 1-min resolution," but no reduction is achieved.

**Code Location**:
- `crates/hares-core/src/actor.rs:24-36` — `ActorInterest` enum and `interests()` default
- `crates/hares-core/src/actor.rs:49-54` — documentation claiming interest-based filtering
- `crates/hares-core/src/dwelling/mod.rs:2357-2360` — the hot loop calls all actors unconditionally

**Root Cause**: The `ActorInterest` infrastructure was designed but the interest-checking logic was never wired into `run_timestep()`. The loop at line 2358-2359 iterates over all actors without consulting `actor.interests()`.

**Impact**: All actors pay the full decision cost every timestep, wasting CPU for infrequently-deciding actors (e.g., DR compliance, EV drivers updating only at departure events, time-of-day actors). At scale, this inflates simulation cost linearly with actor count when it could be near-constant for sparse actors. No physics correctness issue, but a performance regression relative to the stated design intent.

---

### Finding 2: [Severity: medium]
**Description**: Actor execution order is enforced only by insertion-order convention and code structure, with no compile-time or declarative guarantees. The `ActorRegistry` (`actor_registry.rs`) provides no execution ordering — it is solely a `HashMap`-backed factory for creating actor instances. Execution order is determined entirely by the `Dwelling.actors: Vec<Box<dyn Actor>>` field: SolverFeedbackActor runs first (via special-case code, not in the Vec), followed by the Vec in insertion order.

**Code Location**:
- `crates/hares-core/src/actor_registry.rs:83-84` — `ActorRegistry` is a `HashMap<String, ActorFactory>` with no ordering
- `crates/hares-core/src/actor_registry.rs:312-319` — `create()` returns a standalone `Box<dyn Actor>` with no ordering metadata
- `crates/hares-core/src/dwelling/mod.rs:712-713` — `actors: Vec<Box<dyn Actor>>` with comment "Actors execute in registration order"
- `crates/hares-core/src/dwelling/mod.rs:1443-1447` — `add_actor()` appends to the Vec
- `crates/hares-core/src/dwelling/mod.rs:1492-1514` — `auto_register_actors()` prepends built-in actors before user actors via `std::mem::take()`
- `crates/hares-core/src/dwelling/mod.rs:2337-2359` — execution order: SolverFeedbackActor special-case → all Vec actors

**Root Cause**: OCHRE explicitly reorders sub-simulators at init time (`Dwelling.__init__`, line 172-182 of OCHRE `Dwelling.py`) by popping and appending HVAC and Generator/Battery to the end, with comments explaining the ordering rationale. HARES uses a two-tier ordering (built-ins prepended, users appended) but provides no mechanism to validate or reorder actors by their semantic role (thermostat, BMS, DR, occupant). The ordering is fragile: a user mistakenly calling `add_actor` for a thermostat before auto-registration could violate the intended BMS→thermostat order.

**Impact**: If actor registration order deviates from the expected BMS→Thermostat→Equipment sequence, a thermostat could decide before the BMS determines the operational mode. This risks physics inconsistencies where a thermostat setpoint is computed against a BMS mode that hasn't been established yet. The current code avoids this because `auto_register_actors()` always runs first (line 1475-1523), and user actors are appended after, but this is a code-level convention, not an enforced contract.

---

### Finding 3: [Severity: medium]
**Description**: `SolverFeedbackActor` is structurally special-cased as a typed field on `Dwelling` rather than being part of the generic `actors` Vec. This ensures it always runs before other actors, but creates a maintenance hazard: any refactor that moves it into the Vec would silently break ordering without compiler error.

**Code Location**:
- `crates/hares-core/src/dwelling/mod.rs:721` — `solver_feedback_actor: SolverFeedbackActor` as a typed field
- `crates/hares-core/src/dwelling/mod.rs:2337-2338` — explicit `collect_and_solve()` call
- `crates/hares-core/src/dwelling/mod.rs:2342-2343` — explicit `decide()` call (separate from the actor Vec loop at line 2357)
- `crates/hares-core/src/actors/solver_feedback.rs:93-114` — `SolverFeedbackActor` implements `Actor` trait but is never stored in the Vec

**Root Cause**: The `SolverFeedbackActor` needs typed access to `ThermalSolver` via `collect_and_solve()`, which the generic `Actor` trait cannot express. Instead of providing a richer trait or an ordering metadata mechanism, the code stores it as a concrete type with its own call sites in the hot loop.

**Impact**: The `SolverFeedbackActor` doesn't benefit from the same code path as other actors (no profiling, no interest filtering). If actor features are added to the Vec iteration, the SolverFeedbackActor would need to be modified separately, creating a divergence risk. The ordering guarantee (solver feedback before all other actors) is implicit in the code structure, not encoded in the type system.

---

### Finding 4: [Severity: low]
**Description**: The `Actor` trait provides only shared (`&`) read access to `EnvironmentState` and a pre-allocated output buffer. It has no access to mutable Dwelling state. This is an intentional architectural choice (actors emit signals; equipment mutates state), but the trait documentation doesn't explicitly state what state is readable versus writable. The `EnvironmentState` struct passed to `decide()` includes `equipment_telemetry` and `equipment_core` maps, but these are populated from the **previous** timestep's committed state (see `dwelling/mod.rs:2653-2657`), not the current step's intermediate state.

**Code Location**:
- `crates/hares-core/src/actor.rs:65` — `decide(&mut self, env: &EnvironmentState, out: &mut Vec<DispatchRequest>)`
- `crates/hares-core/src/actor.rs:8-13` — module-level doc mentions "Actors only dispatch ControlSignals" but doesn't enumerate what's readable in `env`
- `crates/hares-core/src/dwelling/mod.rs:2653-2706` — `latest_env` snapshot is populated AFTER equipment step, for NEXT step's actors

**Root Cause**: The doc comment for the trait describes the output side (push dispatch requests) thoroughly, but doesn't document that `equipment_telemetry` and `equipment_core` in `env` are one-timestep-lagged snapshots. An actor implementer unfamiliar with this design could reasonably expect `env.equipment_core` to reflect the current step.

**Impact**: Low — this is a documentation gap only. The design is sound: all actors see the same snapshot, and dispatch writes take effect same-timestep. But the stale-equipment-state window (actors see previous-step equipment state while `update_control()` in same timestep may have changed equipment mode) should be explicitly documented in the trait.

---

### Finding 5: [Severity: low]
**Description**: The OCHRE vendor reference (`vendors/OCHRE/ochre/Dwelling.py`) uses `pop`/`append` to enforce equipment ordering at init time, with explicit comments about physics ordering (lines 171-179). HARES lacks equivalent hot-loop assert or debug hook to verify that control signals from solver feedback arrive before thermostat setpoints before BMS mode signals within a single timestep.

**Code Location**:
- OCHRE `Dwelling.py:171-182` — explicit pop/append reordering with comments
- `crates/hares-core/src/dwelling/mod.rs:2336-2363` — HARES hot loop with no ordering assertion

**Root Cause**: The HARES architecture uses two dispatch passes (pre-thermal and post-actor) with a priority ledger, which is more sophisticated than OCHRE's single-pass approach. The priority ledger handles conflicts by tier, but doesn't validate that the _sequence_ of actor decisions follows the intended solver→thermostat→BMS→equipment cascade.

**Impact**: Low — the current ordering works correctly, and `dispatch_ordering_regressions.rs` covers the critical priority-inversion scenarios. However, if actor registration order is violated (Finding 2) and multiple actors target the same equipment, the dispatch ledger only prevents _lower-tier_ signals from overwriting _higher-tier_ signals. If two actors emit signals at the same priority tier (both `PriorityTier::Schedule`) for the same target, the last-registered actor wins. This is "last write wins" semantics (documented at `dwelling/mod.rs:2340`), which is consistent but means actor _ordering_ can affect which Schedule-tier signal is applied.

---

## Summary
- Total findings: 5
- Critical: 0
- High: 1
- Medium: 2
- Low: 2

## Recommendations

1. **Wire up `ActorInterest` filtering** in `run_timestep()`. Check `actor.interests()` against state changes in the current step before calling `decide()`. Track zone temperature deltas, equipment mode changes, hour-of-day transitions, and price signal changes across steps. For actors returning `EveryStep` (or the current behavior as default), call every step. This recovers the documented ~60x savings for sparse actors.

2. **Add an `ActorStage` or ordering metadata** to the actor trait or a wrapper enum, so that execution order is declarative rather than insertion-order-based. For example:
   ```
   enum ActorRole { SolverFeedback, BmsMode, Thermostat, Occupant, DrCompliance, Custom(u8) }
   ```
   Sort the `actors` Vec by this at `auto_register_actors()` time and document the invariant.

3. **Move `SolverFeedbackActor` into the actors Vec** with a special stage marker, or extract its `collect_and_solve()` call into a trait method (`pre_decide()` or similar) so it participates in the same dispatch infrastructure as other actors.

4. **Document the one-step lag** on `EnvironmentState.equipment_telemetry` and `equipment_core` in the `Actor` trait doc comment. Add a statement: "These fields reflect the last *committed* equipment state from the end of the previous timestep. Dispatch requests take effect on the current timestep via the control dispatcher."

5. **Add a debug-assertion** in `run_timestep()` (gated on `debug_assertions` or `cfg(debug_assertions)`) that validates the actor Vec contains the expected ordering of built-in actors (BatteryManagementActor → EvDriverActor) before user-registered actors. This catches registration-order violations early.

## References / Citations

- `crates/hares-core/src/actor.rs:43-76` — `Actor` trait definition
- `crates/hares-core/src/actor.rs:24-36` — `ActorInterest` enum (unused in hot loop)
- `crates/hares-core/src/actor_registry.rs:81-94` — `ActorRegistry::new()` registers built-in types (factories only, no ordering)
- `crates/hares-core/src/dwelling/mod.rs:2209-2717` — `run_timestep()` full simulation step
- `crates/hares-core/src/dwelling/mod.rs:2336-2363` — actor decision and dispatch phase
- `crates/hares-core/src/dwelling/mod.rs:2653-2706` — end-of-step environment snapshot for next step
- `crates/hares-core/src/dwelling/mod.rs:1470-1523` — `auto_register_actors()` built-in actor prepend logic
- `crates/hares-core/src/dwelling/mod.rs:339-444` — `ControlDispatcher` with cross-pass priority ledger
- `crates/hares-core/tests/dispatch_ordering_regressions.rs` — test coverage for priority inversion and same-step dispatch
- `vendors/OCHRE/ochre/Dwelling.py:171-182` — OCHRE equipment ordering via pop/append
- `vendors/OCHRE/ochre/Simulator.py:245-257` — OCHRE `update_model()` sequential sub-simulator dispatch
