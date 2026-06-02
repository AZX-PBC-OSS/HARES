use std::path::PathBuf;

use hares_core::{Dwelling, DwellingConfig, StepResult};
use hares_io::SimulationConfig;
use hares_types::{ControlCapabilities, ControlSignal, EndUse, OperatingMode};

fn load_fixture(fixture_name: &str) -> Dwelling {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.push("../../tests/fixtures/parity");
    root.push(fixture_name);

    let config_path = root.join("config.toml");
    let config_contents =
        std::fs::read_to_string(&config_path).expect("fixture config.toml must be readable");
    let config_value: toml::Value =
        toml::from_str(&config_contents).expect("fixture config.toml must parse");
    let sim_table = config_value
        .get("simulation")
        .and_then(|value| value.as_table())
        .expect("fixture config.toml must contain [simulation]");
    let sim_toml = toml::to_string(sim_table).expect("simulation table must serialize");
    let mut sim_config =
        SimulationConfig::from_toml(&sim_toml).expect("simulation config must parse");
    sim_config.write_output = false;
    let bldg_id = config_value
        .get("bldg_id")
        .and_then(|value| value.as_integer())
        .unwrap_or(1);

    let config = DwellingConfig {
        hpxml_path: root.join("building.xml"),
        schedule_path: root.join("schedule.csv"),
        weather_path: root.join("weather.epw"),
        defaults_path: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../defaults")),
        sim_config,
        overrides: None,
        bldg_id,
        initialization_duration: None,
        resample_overrides: None,
        patches: None,
    };

    Dwelling::from_config(config).expect("fixture dwelling must load")
}

fn first_heating_equipment_name(dwelling: &Dwelling) -> String {
    dwelling
        .equipment()
        .iter()
        .find(|eq| eq.descriptor().end_use == EndUse::HVAC_HEATING)
        .expect("fixture must contain a heating equipment")
        .descriptor()
        .name
        .clone()
}

fn first_heating_equipment_name_with_capability(
    dwelling: &Dwelling,
    capability: ControlCapabilities,
) -> String {
    dwelling
        .equipment()
        .iter()
        .find(|eq| {
            eq.descriptor().end_use == EndUse::HVAC_HEATING
                && eq.descriptor().control_capabilities.contains(capability)
        })
        .expect("fixture must contain a heating equipment with the requested capability")
        .descriptor()
        .name
        .clone()
}

fn simulate_until_first_active_heating(
    fixture_name: &str,
    equipment_name: &str,
) -> (Vec<StepResult>, usize) {
    let mut dwelling = load_fixture(fixture_name);
    let mut steps = Vec::new();

    for step_idx in 0..96 {
        let step = dwelling.step().expect("fixture step must succeed");
        let active = step.hvac_heating_w > 1e-6;
        steps.push(step);
        if active {
            return (steps, step_idx);
        }
    }

    panic!("fixture must produce an active heating step within 96 minutes for {equipment_name}");
}

fn simulate_with_named_control(
    fixture_name: &str,
    target_step: usize,
    equipment_name: &str,
    signal: ControlSignal,
) -> Vec<StepResult> {
    let mut dwelling = load_fixture(fixture_name);
    let mut steps = Vec::new();

    for step_idx in 0..=target_step {
        if step_idx == target_step {
            dwelling
                .apply_control_validated(equipment_name, signal.clone())
                .expect("control should validate");
        }
        steps.push(dwelling.step().expect("fixture step must succeed"));
    }

    steps
}

#[test]
fn named_mode_override_reaches_hvac_on_same_step() {
    let fixture = "cz4a_ashp_hpwh";
    let baseline_dwelling = load_fixture(fixture);
    let equipment_name = first_heating_equipment_name_with_capability(
        &baseline_dwelling,
        ControlCapabilities::MODE_OVERRIDE,
    );
    let (baseline, target_step) = simulate_until_first_active_heating(fixture, &equipment_name);
    let controlled = simulate_with_named_control(
        fixture,
        target_step,
        &equipment_name,
        ControlSignal::ModeOverride {
            mode: OperatingMode::Off,
        },
    );

    let baseline_heat = baseline[target_step].hvac_heating_w;
    let controlled_heat = controlled[target_step].hvac_heating_w;
    assert!(
        baseline_heat > 0.0,
        "baseline step must include active heating to validate same-step control wiring"
    );
    assert!(
        controlled_heat < baseline_heat * 0.2,
        "ModeOverride must suppress HVAC heating on the same step: baseline={baseline_heat:.6} W controlled={controlled_heat:.6} W step={target_step}"
    );
}

#[test]
fn named_thermal_setpoint_reaches_hvac_on_same_step() {
    let fixture = "cz2a_gas_furnace_ac_res_wh";
    let equipment_name = first_heating_equipment_name(&load_fixture(fixture));
    let (baseline, target_step) = simulate_until_first_active_heating(fixture, &equipment_name);
    let controlled = simulate_with_named_control(
        fixture,
        target_step,
        &equipment_name,
        // Set heating to a value well below any achievable indoor temperature
        // to guarantee suppression regardless of cold-start conditions.
        ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(-50.0),
            cooling_setpoint_c: None,
            deadband_c: None,
        },
    );

    let baseline_heat = baseline[target_step].hvac_heating_w;
    let controlled_heat = controlled[target_step].hvac_heating_w;
    assert!(
        baseline_heat > 0.0,
        "baseline step must include active heating to validate same-step control wiring"
    );
    assert!(
        controlled_heat < baseline_heat * 0.2,
        "ThermalSetpoint must suppress HVAC heating on the same step: baseline={baseline_heat:.6} W controlled={controlled_heat:.6} W step={target_step}"
    );
}
