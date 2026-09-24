//! Regression tests: Actor trait must expose a `telemetry()` method.
//!
//! These tests verify that every concrete actor that holds observable internal
//! state returns `Some(&Telemetry)` from `telemetry()`, and that the telemetry
//! keys reflect the actor's decision after a `decide()` call.
//!
//! The only actor exempt from the requirement is `SpyActor` / `SolverFeedbackActor`,
//! which have no observable internal state beyond the `DispatchRequest` they emit.

use hares_control::DispatchRequest;
use hares_core::ChaCha8Rng;
use hares_core::actor::{Actor, testing::test_env};
use hares_core::actors::{
    BatteryManagementActor, DrCompliance, EvDriverActor, IdealThermostat, Occupant,
};
use hares_types::{
    BmsMode, ChargingStrategy, DRLevel, EnvironmentState, GridExportRule, PlugInPolicy,
    ScheduleSource,
};
use rand::SeedableRng;

// ---------------------------------------------------------------------------
// Helper: minimal spy actor for default-behavior validation
// ---------------------------------------------------------------------------

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
// Default behavior: actors without observable state return None
// ---------------------------------------------------------------------------

#[test]
fn spy_actor_telemetry_returns_none() {
    let actor = SpyActor {
        name: "spy".to_string(),
    };
    assert!(
        actor.telemetry().is_none(),
        "SpyActor has no internal state"
    );
}

// ---------------------------------------------------------------------------
// Occupant telemetry
// ---------------------------------------------------------------------------

#[test]
fn occupant_telemetry_reports_presence_after_decision() {
    use hares_core::actors::Presence;

    let schedule = vec![Presence::Away];
    let mut occupant = Occupant::new("Resident").with_presence_schedule(schedule);

    let env = test_env().build();
    let mut out = Vec::new();
    occupant.decide(&env, &mut out);

    let tel = occupant
        .telemetry()
        .expect("Occupant must expose telemetry");
    assert!(
        tel.get("away").is_some(),
        "telemetry must contain 'away' key"
    );
    assert!(
        tel.get("transition").is_some(),
        "telemetry must contain 'transition' key"
    );
    assert!(
        tel.get("signals_count").is_some(),
        "telemetry must contain 'signals_count' key"
    );
}

// ---------------------------------------------------------------------------
// IdealThermostat telemetry
// ---------------------------------------------------------------------------

#[test]
fn ideal_thermostat_telemetry_reports_setpoints() {
    use hares_core::actors::OverrideState;

    let mut thermostat = IdealThermostat::new("HVAC").with_override(OverrideState {
        heating_setpoint_c: Some(20.0),
        cooling_setpoint_c: None,
        deadband_c: None,
    });

    let env = test_env().build();
    let mut out = Vec::new();
    thermostat.decide(&env, &mut out);

    let tel = thermostat
        .telemetry()
        .expect("IdealThermostat must expose telemetry");
    assert_eq!(tel.get("heating_setpoint_c"), Some(20.0));
    assert_eq!(
        tel.get("cooling_setpoint_c"),
        Some(0.0),
        "cooling setpoint unset → 0.0"
    );
    assert_eq!(tel.get("deadband_c"), Some(0.0), "deadband unset → 0.0");
}

#[test]
fn ideal_thermostat_cleared_override_reports_default() {
    let mut thermostat = IdealThermostat::new("HVAC").with_heating_setpoint(20.0);
    thermostat.clear_override();

    let env = test_env().build();
    let mut out = Vec::new();
    thermostat.decide(&env, &mut out);

    let tel = thermostat
        .telemetry()
        .expect("IdealThermostat must expose telemetry");
    assert_eq!(
        tel.get("heating_setpoint_c"),
        Some(0.0),
        "cleared → 0.0 (default)"
    );
}

// ---------------------------------------------------------------------------
// BatteryManagementActor telemetry
// ---------------------------------------------------------------------------

#[test]
fn bms_telemetry_reports_action_and_state() {
    let mut actor = BatteryManagementActor::new(
        "Battery",
        BmsMode::Manual,
        GridExportRule::Unrestricted,
        5.0,
        5.0,
        None,
        1440,
        0,
    );

    let env = test_env().build();
    let mut out = Vec::new();
    actor.decide(&env, &mut out);

    let tel = actor.telemetry().expect("BMS must expose telemetry");
    assert!(
        tel.get("bms_action").is_some(),
        "telemetry must contain 'bms_action'"
    );
    assert!(tel.get("soc").is_some(), "telemetry must contain 'soc'");
    assert_eq!(tel.get("pv_kw"), Some(0.0));
    assert_eq!(tel.get("load_kw"), Some(0.0));
}

// ---------------------------------------------------------------------------
// DrCompliance telemetry
// ---------------------------------------------------------------------------

#[test]
fn dr_compliance_telemetry_reports_level_and_compliance() {
    let mut actor = DrCompliance::new("DRAgent");
    actor.set_dr_level(DRLevel::Critical);

    let env = test_env().build();
    let mut out = Vec::new();
    actor.decide(&env, &mut out);

    let tel = actor
        .telemetry()
        .expect("DrCompliance must expose telemetry");
    assert_eq!(tel.get("dr_active"), Some(1.0));
    assert_eq!(
        tel.get("dr_complied"),
        Some(1.0),
        "AlwaysComply is the default model"
    );
    assert!(
        tel.get("dr_level").is_some(),
        "telemetry must contain 'dr_level'"
    );
}

#[test]
fn dr_compliance_inactive_reports_zero() {
    let mut actor = DrCompliance::new("DRAgent");
    // DRLevel::Normal = no event active

    let env = test_env().build();
    let mut out = Vec::new();
    actor.decide(&env, &mut out);

    let tel = actor
        .telemetry()
        .expect("DrCompliance must expose telemetry");
    assert_eq!(tel.get("dr_active"), Some(0.0));
    assert_eq!(tel.get("dr_complied"), Some(0.0));
    assert_eq!(tel.get("signals_count"), Some(0.0));
}

// ---------------------------------------------------------------------------
// EvDriverActor telemetry
// ---------------------------------------------------------------------------

#[test]
fn ev_driver_telemetry_reports_soc_and_phase() {
    let mut actor = EvDriverActor::new(
        "TestDriver",
        "EV1",
        ChargingStrategy::Immediate { target_soc: 1.0 },
        PlugInPolicy::Always,
        ScheduleSource::Constant(30.0),
        ScheduleSource::Constant(480.0),
        ScheduleSource::Constant(600.0),
        None,
        1.0,
        0.3,
        60.0,
        7.2,
        30.0,
        20.0,
        0.0,
        0.0,
        ChaCha8Rng::from_seed([42u8; 32]),
    );

    let env = test_env().build();
    let mut out = Vec::new();
    actor.decide(&env, &mut out);

    let tel = actor
        .telemetry()
        .expect("EvDriverActor must expose telemetry");
    assert!(tel.get("soc").is_some(), "telemetry must contain 'soc'");
    assert!(tel.get("phase").is_some(), "telemetry must contain 'phase'");
    assert!(
        tel.get("charge_kw").is_some(),
        "telemetry must contain 'charge_kw'"
    );
    assert!(
        tel.get("plugged_in").is_some(),
        "telemetry must contain 'plugged_in'"
    );
}

// ---------------------------------------------------------------------------
// Structural contract: all observable actors return Some(&Telemetry)
// ---------------------------------------------------------------------------

#[test]
fn all_observable_actors_return_some_telemetry() {
    fn assert_actor_has_telemetry<A: Actor>(actor: &mut A, env: &EnvironmentState) {
        let mut out = Vec::new();
        actor.decide(env, &mut out);
        assert!(
            actor.telemetry().is_some(),
            "actor '{}' must return Some(&Telemetry) after decide()",
            actor.name()
        );
    }

    let env = test_env().build();

    // Occupant
    {
        let mut occupant = Occupant::new("TestOccupant");
        assert_actor_has_telemetry(&mut occupant, &env);
    }

    // IdealThermostat
    {
        let mut thermostat = IdealThermostat::new("HVAC").with_heating_setpoint(20.0);
        assert_actor_has_telemetry(&mut thermostat, &env);
    }

    // BatteryManagementActor (note: no equipment_core entry → will be graceful)
    {
        let mut bms = BatteryManagementActor::new(
            "Battery",
            BmsMode::Manual,
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            1440,
            0,
        );
        assert_actor_has_telemetry(&mut bms, &env);
    }

    // DrCompliance
    {
        let mut dr = DrCompliance::new("DRAgent");
        dr.set_dr_level(DRLevel::Critical);
        assert_actor_has_telemetry(&mut dr, &env);
    }

    // EvDriverActor
    {
        let mut ev = EvDriverActor::new(
            "TestDriver",
            "EV1",
            ChargingStrategy::Immediate { target_soc: 1.0 },
            PlugInPolicy::Always,
            ScheduleSource::Constant(30.0),
            ScheduleSource::Constant(480.0),
            ScheduleSource::Constant(600.0),
            None,
            1.0,
            0.3,
            60.0,
            7.2,
            30.0,
            20.0,
            0.0,
            0.0,
            ChaCha8Rng::from_seed([42u8; 32]),
        );
        assert_actor_has_telemetry(&mut ev, &env);
    }
}
