//! Combustion loss destination tests.
//!
//! A combustion appliance's unexplained residual (fuel input minus useful
//! output) leaves the cabinet through exactly two paths: the flue (outdoors)
//! and the jacket or skin (the surrounding space). These tests pin where each
//! share goes.

use std::time::Duration;

use chrono::{FixedOffset, TimeZone};
use hares_equipment::{EquipmentConfig, EquipmentRegistry, GasBoilerConfig, HvacSetpointConfig};
use hares_types::{
    EnvironmentState, FluidAccumulator, FluidType, GridState, LoopId, PortSlots,
    ThermalAccumulator, ThermalCategory, WeatherState, ZoneId, ZoneState, telemetry_keys as tk,
};

fn boiler_env() -> EnvironmentState {
    EnvironmentState {
        ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
        zones: vec![ZoneState {
            id: ZoneId(1),
            temperature_c: 18.0,
            humidity_ratio: 0.005,
            volume_m3: 200.0,
        }],
        weather: WeatherState {
            outdoor_temp_c: 0.0,
            outdoor_humidity_ratio: 0.003,
            outdoor_wet_bulb_c: -0.7,
            outdoor_enthalpy_j_kg: 20_000.0,
            wind_speed_m_s: 2.0,
            wind_dir_deg: 0.0,
            ground_temp_c: 8.0,
            sky_temp_c: -8.0,
            pressure_kpa: 101.325,
            solar_irradiance: vec![],
            ghi_w_m2: 0.0,
            dni_w_m2: 0.0,
            dhi_w_m2: 0.0,
            solar_altitude_deg: 0.0,
            solar_azimuth_deg: 180.0,
            mains_temp_c: 15.0,
            rainfall_m: 0.0,
            ground_albedo: 0.2,
            ground_t_mean_c: 10.0,
            ground_t_amplitude_c: 0.0,
            ground_phase_day: 35.0,
            day_of_year: 15.0,
        },
        grid: GridState {
            voltage_pu: 1.0,
            frequency_hz: 60.0,
            island_bus_voltage_pu: None,
        },
        schedule_row: None,
        unavailable_periods: hares_types::UnavailablePeriodState::default(),
        domains: hares_types::DomainSlots::default(),
        equipment_telemetry: std::collections::HashMap::new(),
        equipment_core: Default::default(),
        current_time: FixedOffset::east_opt(0)
            .expect("UTC offset")
            .with_ymd_and_hms(2026, 1, 15, 0, 0, 0)
            .single()
            .expect("valid"),
        time_res: chrono::Duration::seconds(60),
        price_signal: Default::default(),
        electrical: Default::default(),
    }
}

fn boiler_ports() -> PortSlots {
    PortSlots {
        thermal: vec![ThermalAccumulator::new(ZoneId(1))],
        fluid: vec![FluidAccumulator::new(LoopId(1), FluidType::Water)],
        ..PortSlots::default()
    }
}

/// Non-condensing gas boiler (AFUE 0.8, so EIR 1.25): 20 percent of the fuel
/// input never becomes useful heat.
fn boiler_typed_config() -> GasBoilerConfig {
    GasBoilerConfig {
        equipment_id: None,
        zone_id: Some(1),
        loop_id: Some(1),
        capacity_w: 10_000.0,
        afue: 0.8,
        flow_rate_kg_s: 0.5,
        return_temp_c: 70.0,
        fluid_type: FluidType::Water,
        fan_power_w: None,
        number_of_speeds: 1,
        setpoint: HvacSetpointConfig::default(),
        condensing: false,
        ..Default::default()
    }
}

fn boiler_config() -> EquipmentConfig {
    EquipmentConfig::from_typed(
        "boiler".to_string(),
        "Gas Boiler".to_string(),
        boiler_typed_config(),
    )
    .expect("typed gas boiler config")
}

/// The jacket's share of the combustion loss must be a fraction of the loss,
/// not the whole loss. A non-condensing boiler's flue carries most of the
/// loss outdoors (ASHRAE: jacket losses are a few percent of the fuel input,
/// while the flue carries the balance of an 80 percent AFUE unit's 20 percent
/// loss); routing the entire loss into the zone manufactures free heat.
#[test]
fn boiler_jacket_gain_is_a_share_of_the_loss_not_the_whole_loss() {
    let env = boiler_env();
    let registry = EquipmentRegistry::new();
    let cfg = boiler_config();
    let mut eq = registry.create("Gas Boiler", cfg.clone()).unwrap();
    eq.init(&cfg, &env).unwrap();

    // Run past the thermostat's start-up and average the last 5 steps.
    let n_steps = 10_usize;
    let mut fuel_w = 0.0;
    let mut useful_w = 0.0;
    let mut jacket_w = 0.0;
    let measured = 5_usize;
    for step_index in 0..n_steps {
        let mut ports = boiler_ports();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        if step_index >= n_steps - measured {
            let tel = eq.telemetry();
            fuel_w += tel.get(tk::FUEL_INPUT_W).unwrap_or(0.0);
            useful_w += tel.get(tk::THERMAL_OUTPUT_W).unwrap_or(0.0);
            jacket_w += ports.thermal[0].sensible_for_category(ThermalCategory::JacketLoss);
        }
    }
    let fuel_w = fuel_w / measured as f64;
    let useful_w = useful_w / measured as f64;
    let jacket_w = jacket_w / measured as f64;

    let loss_w = fuel_w - useful_w;
    assert!(
        loss_w > 0.0,
        "a non-condensing boiler at steady state must have a positive combustion \
         loss, got fuel {fuel_w:.1} W vs useful {useful_w:.1} W"
    );
    assert!(
        jacket_w < 0.5 * loss_w,
        "the zone's jacket gain must be a small share of the combustion loss, \
         got {jacket_w:.1} W of the {loss_w:.1} W loss: the flue share belongs \
         outdoors, not in the zone"
    );
}

/// The exact destination contract at steady state: the zone's jacket gain is
/// the configured jacket share (1 - flue_loss_fraction) of the combustion
/// loss (plus the pump's electric dissipation, zero here), and the flue
/// share leaves outdoors. ASHRAE reference pattern: jacket losses are a few
/// percent of the fuel input (the 0.9 flue / 0.1 jacket split of an 80
/// percent AFUE unit's ~20 percent residual puts the jacket at ~2 percent of
/// the input).
#[test]
fn boiler_jacket_gain_equals_the_configured_share_of_the_loss() {
    let env = boiler_env();
    let registry = EquipmentRegistry::new();
    let cfg = EquipmentConfig::from_typed(
        "boiler".to_string(),
        "Gas Boiler".to_string(),
        GasBoilerConfig {
            flue_loss_fraction: 0.9,
            ..boiler_typed_config()
        },
    )
    .expect("typed gas boiler config");
    let mut eq = registry.create("Gas Boiler", cfg.clone()).unwrap();
    eq.init(&cfg, &env).unwrap();

    let n_steps = 10_usize;
    let measured = 5_usize;
    let mut fuel_w = 0.0;
    let mut useful_w = 0.0;
    let mut jacket_w = 0.0;
    for step_index in 0..n_steps {
        let mut ports = boiler_ports();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        if step_index >= n_steps - measured {
            let tel = eq.telemetry();
            fuel_w += tel.get(tk::FUEL_INPUT_W).unwrap_or(0.0);
            useful_w += tel.get(tk::THERMAL_OUTPUT_W).unwrap_or(0.0);
            jacket_w += ports.thermal[0].sensible_for_category(ThermalCategory::JacketLoss);
        }
    }
    let fuel_w = fuel_w / measured as f64;
    let useful_w = useful_w / measured as f64;
    let jacket_w = jacket_w / measured as f64;

    let loss_w = fuel_w - useful_w;
    let expected_jacket_w = (1.0 - 0.9) * loss_w;
    assert!(
        (jacket_w - expected_jacket_w).abs() < 1e-6 * loss_w.max(1.0),
        "the zone's jacket gain must be the configured jacket share of the \
         combustion loss: expected {expected_jacket_w:.3} W, got {jacket_w:.3} W \
         (fuel {fuel_w:.1} W, useful {useful_w:.1} W)"
    );
}
