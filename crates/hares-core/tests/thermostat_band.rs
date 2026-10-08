//! The thermostat band a `ThermalSetpoint` carries, at dwelling level: a
//! band below a thermostat's resolution never reaches the equipment, and a
//! band set by a named signal for an event is part of the resumable state
//! until the release restores the configured one.

use std::path::PathBuf;

use hares_core::Dwelling;
use hares_types::{ControlSignal, EndUse};

const STEPS_BEFORE_CHECKPOINT: usize = 30;
const STEPS_AFTER_CHECKPOINT: usize = 240;

fn s54_dwelling() -> Dwelling {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/bestest/s54_heat_pump.toml");
    Dwelling::from_toml_config_with_write_output(&path, Some(false)).expect("build s54 dwelling")
}

fn heating_equipment_name(dwelling: &Dwelling) -> String {
    dwelling
        .equipment()
        .iter()
        .map(|eq| eq.descriptor())
        .find(|d| d.end_use == EndUse::HVAC_HEATING)
        .expect("s54 carries a heating unit")
        .name
        .clone()
}

/// A sub-band deadband is rejected at dispatch, counted and warned, before
/// it reaches the furnace, so the furnace keeps heating and the zone holds
/// its setpoint over twelve hours of -5 C weather.
#[test]
fn a_sub_band_deadband_is_rejected_and_the_furnace_keeps_heating() {
    let mut dwelling = s54_dwelling();
    let heater = heating_equipment_name(&dwelling);
    for _ in 0..STEPS_BEFORE_CHECKPOINT {
        dwelling.step().expect("step before the signal");
    }
    for deadband_c in [1e-17, 5e-324] {
        dwelling
            .apply_control_validated(
                &heater,
                ControlSignal::ThermalSetpoint {
                    heating_setpoint_c: Some(21.0),
                    cooling_setpoint_c: None,
                    deadband_c: Some(deadband_c),
                },
                None,
            )
            .expect("the signal is queued; its bounds are checked at dispatch");
    }
    let mut lowest_zone_c = f64::INFINITY;
    for _ in 0..720 {
        let step = dwelling.step().expect("the furnace keeps stepping");
        for &(_, zone_c) in &step.zone_temperatures_c {
            lowest_zone_c = lowest_zone_c.min(zone_c);
        }
    }
    assert_eq!(dwelling.health().rejected_control_signals, 2);
    assert_eq!(dwelling.health().port_rollbacks, 0);
    assert!(
        lowest_zone_c > 15.0,
        "the zone must stay heated, fell to {lowest_zone_c} C"
    );
}

#[test]
fn restored_run_during_a_named_band_event_matches_the_continuous_run() {
    let mut continuous = s54_dwelling();
    let heater = heating_equipment_name(&continuous);
    for _ in 0..STEPS_BEFORE_CHECKPOINT {
        continuous.step().expect("step before the event");
    }
    continuous
        .apply_control_validated(
            &heater,
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(21.0),
                cooling_setpoint_c: None,
                deadband_c: Some(0.25),
            },
            None,
        )
        .expect("the named event is valid");
    continuous.step().expect("step with the event");
    let checkpoint = continuous.save_checkpoint().expect("save checkpoint");

    let mut restored = s54_dwelling();
    restored
        .load_checkpoint(checkpoint)
        .expect("restore checkpoint");

    for i in 0..STEPS_AFTER_CHECKPOINT {
        if i == STEPS_AFTER_CHECKPOINT / 2 {
            for dwelling in [&mut continuous, &mut restored] {
                dwelling
                    .apply_control_validated(&heater, ControlSignal::thermal_release(), None)
                    .expect("the release is valid");
            }
        }
        let a = continuous.step().expect("continuous step");
        let b = restored.step().expect("restored step");
        assert_eq!(
            a.zone_temperatures_c, b.zone_temperatures_c,
            "step {i}: zone temperatures diverge"
        );
        assert_eq!(
            a.hvac_heating_w.to_bits(),
            b.hvac_heating_w.to_bits(),
            "step {i}: hvac_heating_w diverges"
        );
    }
}
