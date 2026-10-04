//! A thermostat band set by a named `ThermalSetpoint` and kept through the
//! release is part of the resumable state: a dwelling restored from a
//! checkpoint taken after the release continues exactly as the uninterrupted
//! run does.

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

#[test]
fn restored_run_after_a_named_band_and_release_matches_the_continuous_run() {
    let mut continuous = s54_dwelling();
    let heater = heating_equipment_name(&continuous);
    for _ in 0..STEPS_BEFORE_CHECKPOINT {
        continuous.step().expect("step before the event");
    }
    for signal in [
        ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(21.0),
            cooling_setpoint_c: None,
            deadband_c: Some(0.25),
        },
        ControlSignal::ThermalSetpoint {
            heating_setpoint_c: None,
            cooling_setpoint_c: None,
            deadband_c: None,
        },
    ] {
        continuous
            .apply_control_validated(&heater, signal, None)
            .expect("the named event and the release are valid");
        continuous.step().expect("step with the signal");
    }
    let checkpoint = continuous.save_checkpoint().expect("save checkpoint");

    let mut restored = s54_dwelling();
    restored
        .load_checkpoint(checkpoint)
        .expect("restore checkpoint");

    for i in 0..STEPS_AFTER_CHECKPOINT {
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
