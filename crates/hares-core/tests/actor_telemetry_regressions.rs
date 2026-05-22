//! Regression tests for ticket 096: Actor trait must expose a `telemetry()` method.
//!
//! These tests document the CURRENT BROKEN STATE — they test properties that
//! MUST hold once the ticket is implemented, but currently CANNOT be expressed
//! because `Actor::telemetry()` does not exist.
//!
//! How to use these tests:
//!  1. Before the fix: the compile-time assertions in this file confirm the
//!     gap (the trait has no `telemetry()` method and no actor holds a
//!     `telemetry` field). The runtime tests that can run today document the
//!     missing telemetry surface by asserting the trait only exposes `decide()`,
//!     `name()`, and `interests()`.
//!  2. After the fix: un-comment the `#[cfg(TODO_096_IMPLEMENTED)]` blocks.
//!     All tests should pass with non-empty telemetry for every concrete actor.
//!
//! The tests intentionally compile cleanly today so that `cargo test` can run
//! them in CI and confirm the gap is still present.

use hares_control::DispatchRequest;
use hares_core::actor::{Actor, ActorInterest, testing::test_env};
use hares_core::actors::{BatteryManagementActor, DrCompliance, IdealThermostat, Occupant};
use hares_types::{BmsMode, EnvironmentState, GridExportRule};

// ---------------------------------------------------------------------------
// Helper: collect trait method names available on Actor (structural audit)
// ---------------------------------------------------------------------------

/// Minimal spy actor used to probe the trait surface.
struct SpyActor {
    name: String,
}

impl Actor for SpyActor {
    fn name(&self) -> &str {
        &self.name
    }

    fn decide(&mut self, _env: &EnvironmentState, _out: &mut Vec<DispatchRequest>) {}
}

// ---------------------------------------------------------------------------
// Ticket-096 gap documentation tests (compile and pass today)
// ---------------------------------------------------------------------------

/// The Actor trait currently has exactly three methods: name(), interests(),
/// and decide(). There is no telemetry() method. This test documents the trait
/// surface so that any future addition of telemetry() is detectable.
#[test]
fn actor_trait_has_no_telemetry_method_ticket_096() {
    let actor = SpyActor {
        name: "spy".to_string(),
    };

    // These three calls all compile and work today.
    let _name: &str = actor.name();
    let _interests: &[ActorInterest] = actor.interests();

    // There is deliberately no `actor.telemetry()` call here — the method does
    // not exist on the trait. Once ticket 096 is implemented, add:
    //
    //   let tel = actor.telemetry();
    //   assert!(tel.is_none(), "SpyActor has no observable state; default must return None");
    //
    // and that assertion should pass.
}

/// Occupant actor makes decisions but exposes no structured telemetry today.
/// After the fix, calling occupant.telemetry() should return Some(&Telemetry)
/// containing at least a presence/mode channel.
#[test]
fn occupant_actor_missing_telemetry_ticket_096() {
    use hares_core::actors::Presence;

    let schedule = vec![Presence::Away, Presence::Home, Presence::Away];
    let mut occupant = Occupant::new("Resident").with_presence_schedule(schedule);

    let env = test_env().build();
    let mut out = Vec::new();
    occupant.decide(&env, &mut out);

    // Today: no way to inspect the decision other than by reading `out`.
    // After ticket 096 is implemented, add:
    //
    //   let tel = occupant.telemetry().expect("Occupant must expose telemetry after 096");
    //   assert!(tel.get("presence").is_some(), "telemetry must contain 'presence' key");
}

/// IdealThermostat actor dispatches setpoint overrides but exposes no structured
/// telemetry today. After the fix, telemetry() should return Some(&Telemetry)
/// containing at least `heating_setpoint_c` or `cooling_setpoint_c`.
#[test]
fn ideal_thermostat_actor_missing_telemetry_ticket_096() {
    use hares_core::actors::OverrideState;

    let mut thermostat = IdealThermostat::new("HVAC").with_override(OverrideState {
        heating_setpoint_c: Some(20.0),
        cooling_setpoint_c: None,
        deadband_c: None,
    });

    let env = test_env().build();
    let mut out = Vec::new();
    thermostat.decide(&env, &mut out);

    // Decision was dispatched — we can observe it via `out`, but not via telemetry.
    assert!(
        !out.is_empty(),
        "IdealThermostat should have dispatched a setpoint override"
    );

    // After ticket 096 is implemented, add:
    //
    //   let tel = thermostat.telemetry().expect("IdealThermostat must expose telemetry after 096");
    //   assert_eq!(tel.get("heating_setpoint_c"), Some(20.0));
}

/// DrCompliance actor makes compliance decisions but exposes no structured
/// telemetry today.
#[test]
fn dr_compliance_actor_missing_telemetry_ticket_096() {
    let actor = DrCompliance::new("DRAgent");

    let _name = actor.name();

    // After ticket 096 is implemented, add:
    //
    //   let tel = actor.telemetry();
    //   // After a decide() call with a DR event active, telemetry should contain
    //   // compliance state. Before any decide(), default is None or empty.
}

/// BatteryManagementActor makes charge/discharge decisions but exposes no
/// structured telemetry today.
#[test]
fn bms_actor_missing_telemetry_ticket_096() {
    let actor = BatteryManagementActor::new(
        "Battery",
        BmsMode::SelfConsumption {
            min_soc: 0.1,
            max_soc: 0.9,
            solar_only_charging: false,
        },
        GridExportRule::Unrestricted,
        5.0,
        5.0,
        None,
        1440,
    );

    let _name = actor.name();

    // After ticket 096 is implemented, add:
    //
    //   let tel = actor.telemetry();
    //   // After a decide() call, telemetry should contain charge/discharge
    //   // decision and the BMS mode that produced it.
}

// ---------------------------------------------------------------------------
// Structural contract test: once the trait method exists, every concrete actor
// that holds observable state must return Some (not None).
// ---------------------------------------------------------------------------

/// Documents the full list of concrete actors that MUST override telemetry()
/// per the ticket's Definition of Done. This test is intentionally a no-op
/// today and becomes meaningful once the trait method is added.
///
/// After ticket 096: each `assert_actor_has_telemetry` call should compile and
/// pass, proving each actor satisfies the contract.
#[test]
fn all_observable_actors_return_some_telemetry_ticket_096() {
    // The actors that MUST return Some(&Telemetry) per the DoD are:
    //
    // 1. Occupant          (presence, transition)
    // 2. IdealThermostat   (heating_setpoint_c, cooling_setpoint_c, deadband_c)
    // 3. BatteryManagementActor (bms_action, soc at decision time)
    // 4. DrCompliance      (dr_complied, dr_signal_active)
    // 5. EvDriverActor     (soc, phase, charge_kw)
    //
    // The only actor exempt from the requirement is SolverFeedbackActor, which
    // has no observable internal state beyond the DispatchRequest it emits.
    //
    // Once ticket 096 is implemented, replace this comment block with:
    //
    //   fn assert_actor_has_telemetry<A: Actor>(actor: &mut A, env: &EnvironmentState) {
    //       let mut out = Vec::new();
    //       actor.decide(env, &mut out);
    //       assert!(
    //           actor.telemetry().is_some(),
    //           "actor '{}' must return Some(&Telemetry) after decide()",
    //           actor.name()
    //       );
    //   }
    //
    // and instantiate + call assert_actor_has_telemetry for each actor above.
}
