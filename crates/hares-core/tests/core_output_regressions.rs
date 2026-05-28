use std::path::PathBuf;

use hares_core::actor::testing::TestEnvBuilder;
use hares_core::actors::BatteryManagementActor;
use hares_core::{Actor, Dwelling, DwellingConfig, StepResult};
use hares_io::SimulationConfig;
use hares_types::{
    BmsMode, ControlSignal, CoreOutput, CoreState, ElectricalSummary, EndUse, EquipmentId,
    GridExportRule, Soc, telemetry_keys::FUEL_INPUT_W,
};

fn approx_eq(left: f64, right: f64, context: &str) {
    assert!(
        (left - right).abs() < 1e-9,
        "{context}: expected {right}, got {left} (diff {})",
        (left - right).abs()
    );
}

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
    let sim_config = SimulationConfig::from_toml(&sim_toml).expect("simulation config must parse");
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

#[test]
fn energy_balance_residual_has_entry_per_zone_and_becomes_nonzero() {
    let mut dwelling = load_fixture("cz2a_gas_furnace_ac_res_wh");
    let telemetry = dwelling.telemetry();
    assert_eq!(
        telemetry.energy_balance_residuals.len(),
        telemetry.zone_names.len(),
        "energy_balance_residuals must have one entry per zone"
    );

    let mut saw_nonzero = false;
    for _ in 0..12 {
        dwelling.step().expect("fixture step must succeed");
        let tel = dwelling.telemetry();
        assert_eq!(
            tel.energy_balance_residuals.len(),
            tel.zone_names.len(),
            "energy_balance_residuals length must stay consistent across steps"
        );
        if tel.energy_balance_residuals.iter().any(|&r| r > 0.0) {
            saw_nonzero = true;
            break;
        }
    }
    assert!(
        saw_nonzero,
        "energy balance residual must be nonzero in at least one zone after a few steps"
    );
}

fn battery_equipment(dwelling: &Dwelling) -> String {
    let battery = dwelling
        .equipment()
        .iter()
        .find(|eq| eq.descriptor().end_use == EndUse::BATTERY)
        .expect("fixture must contain battery equipment");
    battery.descriptor().name.clone()
}

fn sum_signed_electric_kw(dwelling: &Dwelling) -> f64 {
    dwelling
        .equipment()
        .iter()
        .map(|eq| {
            eq.core_output()
                .flows
                .electric_kw
                .map_or(0.0, |power| power.signed_kw())
        })
        .sum()
}

fn sum_fuel_consumption_w(dwelling: &Dwelling) -> f64 {
    dwelling
        .equipment()
        .iter()
        .map(|eq| {
            eq.core_output()
                .flows
                .fuel_w
                .map_or(0.0, |fuel| fuel.consumption_w)
        })
        .sum()
}

fn assert_snapshot_aligned(dwelling: &Dwelling, step: &StepResult) {
    let telemetry = dwelling.telemetry();
    let equipment = dwelling.equipment();

    assert_eq!(
        telemetry.equipment_names.len(),
        equipment.len(),
        "telemetry equipment count must match equipment registry"
    );
    assert_eq!(
        telemetry.equipment_power_kw.len(),
        equipment.len(),
        "telemetry power vector length must match equipment registry"
    );
    assert_eq!(
        telemetry.equipment_soc.len(),
        equipment.len(),
        "telemetry soc vector length must match equipment registry"
    );
    assert_eq!(
        telemetry.equipment_modes.len(),
        equipment.len(),
        "telemetry mode vector length must match equipment registry"
    );
    let mut core_electric_sum = 0.0;
    let mut core_fuel_sum = 0.0;

    for (idx, eq) in equipment.iter().enumerate() {
        let desc = eq.descriptor();
        assert_eq!(
            telemetry.equipment_names[idx], desc.name,
            "telemetry name ordering must remain stable"
        );

        let co = eq.core_output();
        let electric_kw = co.flows.electric_kw.map_or(0.0, |power| power.signed_kw());
        let soc = co.state.soc.map_or(0.0, |s| s.get());
        let operating_mode = co.state.operating_mode.map_or(0.0, |mode| mode.as_code());
        let fuel_w = co.flows.fuel_w.map_or(0.0, |fuel| fuel.consumption_w);

        approx_eq(
            telemetry.equipment_power_kw[idx],
            electric_kw,
            "equipment power telemetry must mirror CoreOutput",
        );
        approx_eq(
            telemetry.equipment_soc[idx],
            soc,
            "equipment soc telemetry must mirror CoreOutput",
        );
        approx_eq(
            telemetry.equipment_modes[idx],
            operating_mode,
            "equipment mode telemetry must mirror CoreOutput",
        );

        core_electric_sum += electric_kw;
        core_fuel_sum += fuel_w;
    }

    approx_eq(
        step.net_electric_power_kw,
        core_electric_sum,
        "step net electric power must equal the sum of equipment CoreOutput electric power",
    );
    approx_eq(
        telemetry.total_power_kw,
        core_electric_sum,
        "dwelling telemetry total_power_kw must equal the sum of equipment CoreOutput electric power",
    );
    approx_eq(
        sum_signed_electric_kw(dwelling),
        core_electric_sum,
        "equipment CoreOutput electric sum must remain self-consistent",
    );
    approx_eq(
        sum_fuel_consumption_w(dwelling),
        core_fuel_sum,
        "equipment CoreOutput fuel sum must remain self-consistent",
    );
}

#[test]
fn battery_core_output_matches_dwelling_aggregation() {
    let mut dwelling = load_fixture("cz4a_battery_only");
    let battery_name = battery_equipment(&dwelling);

    let first_step = dwelling.step().expect("initial battery step must succeed");
    assert_snapshot_aligned(&dwelling, &first_step);

    dwelling
        .apply_control_validated(
            &battery_name,
            ControlSignal::PowerSetpoint {
                active_power_kw: 3.0,
                reactive_power_kvar: None,
            },
        )
        .expect("battery power setpoint must validate");

    let second_step = dwelling
        .step()
        .expect("controlled battery step must succeed");
    assert_snapshot_aligned(&dwelling, &second_step);
}

#[test]
fn gas_fuel_core_output_matches_dwelling_aggregation() {
    let mut dwelling = load_fixture("cz2a_gas_furnace_ac_res_wh");
    let mut saw_fuel = false;

    for _ in 0..12 {
        let step = dwelling.step().expect("gas fixture step must succeed");
        assert_snapshot_aligned(&dwelling, &step);

        let step_fuel_telemetry: f64 = dwelling
            .equipment()
            .iter()
            .map(|eq| eq.telemetry().get(FUEL_INPUT_W).unwrap_or(0.0))
            .sum();
        let step_fuel_core: f64 = dwelling
            .equipment()
            .iter()
            .map(|eq| {
                eq.core_output()
                    .flows
                    .fuel_w
                    .map_or(0.0, |fuel| fuel.consumption_w)
            })
            .sum();

        approx_eq(
            step_fuel_telemetry,
            step_fuel_core,
            "fuel telemetry must mirror CoreOutput",
        );

        if step_fuel_core > 0.0 {
            saw_fuel = true;
            break;
        }
    }

    assert!(
        saw_fuel,
        "gas fixture must produce nonzero fuel consumption"
    );
}

#[test]
fn actors_observe_previous_equipment_core_snapshot() {
    let mut actor = BatteryManagementActor::new(
        "Battery1",
        BmsMode::SelfConsumption {
            min_soc: 0.1,
            max_soc: 0.8,
            solar_only_charging: false,
        },
        GridExportRule::Unrestricted,
        5.0,
        5.0,
        None,
        24,
    );
    let battery_id = EquipmentId(42);
    let mut id_map = std::collections::HashMap::new();
    id_map.insert("Battery1".to_string(), battery_id);
    actor.resolve_equipment_id(&id_map);

    let mut env = TestEnvBuilder::new()
        .with_electrical(ElectricalSummary {
            pv_generation_kw: 5.0,
            base_load_kw: 2.0,
            ..Default::default()
        })
        .build();
    env.equipment_core.insert(
        battery_id,
        CoreOutput {
            state: CoreState {
                soc: Some(Soc::try_from(0.2).expect("valid SOC")),
                ..Default::default()
            },
            ..Default::default()
        },
    );

    let mut out = Vec::new();
    actor.decide(&env, &mut out);
    assert!(
        out.iter().any(|req| matches!(
            req.signal,
            ControlSignal::SelfConsumption { enabled: true, .. }
        )),
        "actor must see the low-SOC snapshot and request charging"
    );

    env.equipment_core.insert(
        battery_id,
        CoreOutput {
            state: CoreState {
                soc: Some(Soc::try_from(0.95).expect("valid SOC")),
                ..Default::default()
            },
            ..Default::default()
        },
    );

    out.clear();
    actor.decide(&env, &mut out);
    assert!(
        out.is_empty(),
        "actor must react to the updated equipment_core snapshot rather than a stale one"
    );
}
