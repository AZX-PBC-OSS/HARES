use std::collections::HashMap;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
use hares_equipment::{
    CentralAirConditionerConfig, DuctConfig, ElectricBaseboardConfig, ElectricFurnaceConfig,
    EquipmentConfig, EquipmentRegistry, GasFurnaceConfig, HeatPumpHeaterConfig,
};
use hares_types::{
    ControlSignal, EnvironmentState, FuelType, GridState, OperatingMode, PortSlots,
    ThermalAccumulator, WeatherState, ZoneId, ZoneState,
};

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

fn env_with_zone_temp(temp_c: f64) -> EnvironmentState {
    EnvironmentState {
        zones: vec![ZoneState {
            id: ZoneId(1),
            temperature_c: temp_c,
            humidity_ratio: 0.008,
            relative_humidity: 0.45,
            wet_bulb_c: 14.0,
            volume_m3: 200.0,
        }],
        weather: WeatherState {
            outdoor_temp_c: -5.0,
            outdoor_humidity_ratio: 0.003,
            outdoor_wet_bulb_c: -6.0,
            outdoor_enthalpy_j_kg: 0.0,
            wind_speed_m_s: 2.0,
            wind_dir_deg: 0.0,
            ground_temp_c: 5.0,
            sky_temp_c: -10.0,
            pressure_kpa: 101.325,
            solar_irradiance: vec![],
            ghi_w_m2: 0.0,
            dni_w_m2: 0.0,
            dhi_w_m2: 0.0,
            solar_altitude_deg: 0.0,
            solar_azimuth_deg: 180.0,
            mains_temp_c: 10.0,
            rainfall_m: 0.0,
            ground_albedo: 0.2,
        },
        grid: GridState {
            voltage_pu: 1.0,
            frequency_hz: 60.0,
        },
        custom_domains: vec![],
        equipment_telemetry: std::collections::HashMap::new(),
        equipment_core: Default::default(),
        current_time: FixedOffset::east_opt(0)
            .expect("UTC offset")
            .with_ymd_and_hms(2026, 1, 15, 12, 0, 0)
            .single()
            .expect("valid"),
        time_res: ChronoDuration::seconds(60),
        price_signal: Default::default(),
        electrical: Default::default(),
    }
}

fn env_with_zone_temp_hot(temp_c: f64) -> EnvironmentState {
    let mut e = env_with_zone_temp(temp_c);
    e.weather.outdoor_temp_c = 35.0;
    e.weather.outdoor_wet_bulb_c = 24.0;
    e
}

fn gas_furnace_config(name: &str) -> EquipmentConfig {
    EquipmentConfig::from_typed(
        name.to_string(),
        "Gas Furnace".to_string(),
        GasFurnaceConfig {
            equipment_id: None,
            zone_id: Some(1),
            afue: 0.80,
            capacity_w: 10_000.0,
            number_of_speeds: 1,
            fan_power_w: Some(0.0),
            ducts: DuctConfig::default(),
            ..GasFurnaceConfig::default()
        },
    )
}

fn electric_furnace_config(name: &str) -> EquipmentConfig {
    EquipmentConfig::from_typed(
        name.to_string(),
        "Electric Furnace".to_string(),
        ElectricFurnaceConfig {
            equipment_id: None,
            zone_id: Some(1),
            eir: 1.0,
            capacity_w: 10_000.0,
            number_of_speeds: 1,
            fan_power_w: Some(0.0),
            ducts: DuctConfig::default(),
            ..ElectricFurnaceConfig::default()
        },
    )
}

fn ports_for_zone1() -> PortSlots {
    PortSlots {
        thermal: vec![ThermalAccumulator::new(ZoneId(1))],
        ..PortSlots::default()
    }
}

// ---------------------------------------------------------------------------
// 1. furnace_heats_when_below_setpoint
//    Zone at 18°C, setpoint 21°C, deadband 1°C → thermal gain > 0
// ---------------------------------------------------------------------------

#[test]
fn furnace_heats_when_below_setpoint() {
    let cfg = gas_furnace_config("furnace");
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Gas Furnace", cfg.clone()).unwrap();
    let env = env_with_zone_temp(18.0);
    eq.init(&cfg, &env).unwrap();

    let mut ports = ports_for_zone1();
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    assert!(
        ports.thermal[0].sensible_gain_w > 1e-6,
        "expected positive thermal gain when zone ({:.1}°C) is below heating setpoint (20°C), got {:.3} W",
        18.0,
        ports.thermal[0].sensible_gain_w,
    );
}

// ---------------------------------------------------------------------------
// 2. furnace_off_when_above_setpoint
//    Zone at 23°C, setpoint 21°C → thermal gain = 0
// ---------------------------------------------------------------------------

#[test]
fn furnace_off_when_above_setpoint() {
    let cfg = gas_furnace_config("furnace");
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Gas Furnace", cfg.clone()).unwrap();
    let env = env_with_zone_temp(23.0);
    eq.init(&cfg, &env).unwrap();

    let mut ports = ports_for_zone1();
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    assert!(
        ports.thermal[0].sensible_gain_w.abs() < 1e-6,
        "expected zero thermal gain when zone ({:.1}°C) is above heating setpoint (20°C), got {:.3} W",
        23.0,
        ports.thermal[0].sensible_gain_w,
    );
}

// ---------------------------------------------------------------------------
// 3. gas_furnace_consumes_gas_fuel
//    Gas furnace heating → fuel.get(Gas) > 0, electrical port = fan power only
// ---------------------------------------------------------------------------

#[test]
fn gas_furnace_consumes_gas_fuel() {
    let cfg = EquipmentConfig::from_typed(
        "GF".to_string(),
        "Gas Furnace".to_string(),
        GasFurnaceConfig {
            equipment_id: None,
            zone_id: Some(1),
            afue: 0.8,
            capacity_w: 10_000.0,
            number_of_speeds: 1,
            fan_power_w: Some(400.0),
            ducts: DuctConfig::default(),
            ..GasFurnaceConfig::default()
        },
    );

    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Gas Furnace", cfg.clone()).unwrap();
    let env = env_with_zone_temp(18.0);
    eq.init(&cfg, &env).unwrap();

    let mut ports = ports_for_zone1();
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    // Gas furnace burns gas; fuel consumption must be positive.
    assert!(
        ports.fuel.get(FuelType::Gas) > 1e-6,
        "expected gas consumption > 0, got {:.3} W",
        ports.fuel.get(FuelType::Gas),
    );

    // Verify AFUE relationship: fuel_input = capacity / efficiency = 10000 / 0.8 = 12500 W.
    let expected_fuel_w = 10_000.0 / 0.8;
    assert!(
        (ports.fuel.get(FuelType::Gas) - expected_fuel_w).abs() < expected_fuel_w * 0.02,
        "gas consumption must equal capacity/efficiency = {expected_fuel_w:.0} W ±2%; \
         got {:.0} W",
        ports.fuel.get(FuelType::Gas),
    );

    // Electric port is fan-only: configured at 400 W = 0.4 kW.
    let electric_kw = ports.electrical.net_active_kw();
    let fan_w = electric_kw * 1_000.0;
    assert!(
        fan_w > 0.0,
        "expected fan-only electric draw to be positive, got {fan_w:.1} W",
    );
    assert!(
        (fan_w - 400.0).abs() < 50.0,
        "fan power must equal configured 400 W ±50 W; got {fan_w:.1} W",
    );
}

// ---------------------------------------------------------------------------
// 4. electric_furnace_consumes_electricity
//    Electric furnace heating → electrical > 0, gas = 0
// ---------------------------------------------------------------------------

#[test]
fn electric_furnace_consumes_electricity() {
    let cfg = electric_furnace_config("ef");
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Electric Furnace", cfg.clone()).unwrap();
    let env = env_with_zone_temp(18.0);
    eq.init(&cfg, &env).unwrap();

    let mut ports = ports_for_zone1();
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    assert!(
        ports.electrical.net_active_kw() > 1e-6,
        "expected electric draw > 0 for electric furnace, got {:.4} kW",
        ports.electrical.net_active_kw(),
    );
    assert!(
        ports.fuel.get(FuelType::Gas).abs() < 1e-6,
        "electric furnace must consume zero gas, got {:.4} W",
        ports.fuel.get(FuelType::Gas),
    );
}

// ---------------------------------------------------------------------------
// 5. ashp_heating_cop_above_unity
//    ASHP thermal output / electrical input > 1.0
// ---------------------------------------------------------------------------

#[test]
fn ashp_heating_cop_above_unity() {
    let cfg = EquipmentConfig::from_typed(
        "ashp".to_string(),
        "ASHP Heater".to_string(),
        HeatPumpHeaterConfig {
            equipment_id: None,
            zone_id: Some(1),
            heating_capacity_w: Some(8_000.0),
            hspf: Some(9.0),
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            backup_fuel: None,
            backup_capacity_w: Some(0.0),
            backup_eir: None,
            fraction_heating_load_served: Some(1.0),
            cooling_capacity_w: Some(8_000.0),
            seer: Some(14.0),
            stage_cooling_capacities_w: None,
            stage_cooling_eirs: None,
            stage_shrs: None,
            fraction_cooling_load_served: Some(1.0),
            number_of_speeds: 1,
            is_mini_split: false,
            shr: Some(0.75),
            fan_power_w: Some(0.0),
            fan_power_w_per_cfm: None,
            heating_setpoint_c: None,
            cooling_setpoint_c: None,
            hysteresis_c: None,
            hp_lockout_temp_c: None,
            er_lockout_temp_c: None,
            max_oat_supplemental_c: None,
            er_setpoint_offset_c: None,
            er_hard_lockout_time_s: None,
            duct: DuctConfig::default(),
            biquadratic_x1_min: None,
            biquadratic_x1_max: None,
            biquadratic_x2_min: None,
            biquadratic_x2_max: None,
            ff_min: None,
            ff_max: None,
            plf_min: None,
            plf_max: None,
        },
    );

    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("ASHP Heater", cfg.clone()).unwrap();
    // Use a mild outdoor temperature where ASHP operates efficiently.
    let mut env = env_with_zone_temp(18.0);
    env.weather.outdoor_temp_c = 7.0;
    env.weather.outdoor_wet_bulb_c = 5.0;
    eq.init(&cfg, &env).unwrap();

    let mut ports = ports_for_zone1();
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let thermal_w = ports.thermal[0].sensible_gain_w;
    let electric_kw = ports.electrical.net_active_kw();

    assert!(
        thermal_w > 1e-6,
        "ASHP must deliver positive heat, got {thermal_w:.3} W"
    );
    assert!(
        electric_kw > 1e-6,
        "ASHP must draw positive electricity, got {electric_kw:.4} kW"
    );

    let cop = thermal_w / (electric_kw * 1_000.0);
    // AHRI 210/240-2023 Tier 1 minimum COP at 47°F (8.3°C) is 2.0.
    // At 7°C outdoor with default EIR=0.35, expected COP ≈ 2.86.
    assert!(
        cop > 2.0,
        "ASHP COP must exceed 2.0 at 7°C outdoor (near AHRI 47°F rating point); \
         AHRI 210/240-2023 minimum=2.0, got {cop:.3} \
         (thermal={thermal_w:.1} W, electric={electric_kw:.4} kW)",
    );
    assert!(
        cop < 5.0,
        "ASHP COP {cop:.3} unrealistically high at 7°C outdoor",
    );
}

// ---------------------------------------------------------------------------
// 6. hvac_port_contributions_are_correct_sign
//    Heating → positive thermal; cooling → negative thermal
// ---------------------------------------------------------------------------

#[test]
fn hvac_port_contributions_are_correct_sign() {
    // Heating equipment: furnace at 18°C with setpoint 20°C (default).
    let heat_cfg = gas_furnace_config("furnace");
    let registry = EquipmentRegistry::new();
    let mut heater = registry.create("Gas Furnace", heat_cfg.clone()).unwrap();
    let heat_env = env_with_zone_temp(18.0);
    heater.init(&heat_cfg, &heat_env).unwrap();

    let mut heat_ports = ports_for_zone1();
    heater.update_control(&heat_env);
    heater
        .step(&heat_env, Duration::from_secs(60), &mut heat_ports)
        .unwrap();

    assert!(
        heat_ports.thermal[0].sensible_gain_w > 1e-6,
        "heating equipment must produce positive thermal contribution, got {:.3} W",
        heat_ports.thermal[0].sensible_gain_w,
    );

    // Cooling equipment: AC at 30°C with cooling setpoint 26°C.
    let cool_cfg = EquipmentConfig::from_typed(
        "ac".to_string(),
        "Air Conditioner".to_string(),
        CentralAirConditionerConfig {
            equipment_id: None,
            zone_id: Some(1),
            capacity_w: 10_000.0,
            seer: 14.0,
            shr: Some(0.75),
            number_of_speeds: 1,
            stage_capacities_w: None,
            stage_eirs: None,
            stage_shrs: None,
            fan_power_w: Some(0.0),
            fan_power_w_per_cfm: None,
            cooling_setpoint_c: None,
            heating_setpoint_c: None,
            hysteresis_c: None,
            airflow_cfm_per_ton: None,
            fraction_load_served: Some(1.0),
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            duct: DuctConfig::default(),
            system_type: None,
            startup_cd: None,
            biquadratic_x1_min: None,
            biquadratic_x1_max: None,
            biquadratic_x2_min: None,
            biquadratic_x2_max: None,
            ff_min: None,
            ff_max: None,
            plf_min: None,
            plf_max: None,
        },
    );
    let mut cooler = registry
        .create("Air Conditioner", cool_cfg.clone())
        .unwrap();
    // Use a hot environment to ensure cooling is demanded.
    let cool_env = env_with_zone_temp_hot(30.0);
    cooler.init(&cool_cfg, &cool_env).unwrap();

    let mut cool_ports = PortSlots {
        thermal: vec![ThermalAccumulator::new(ZoneId(1))],
        ..PortSlots::default()
    };
    cooler.update_control(&cool_env);
    cooler
        .step(&cool_env, Duration::from_secs(60), &mut cool_ports)
        .unwrap();

    assert!(
        cool_ports.thermal[0].sensible_gain_w < -1e-6,
        "cooling equipment must produce negative thermal contribution, got {:.3} W",
        cool_ports.thermal[0].sensible_gain_w,
    );
}

// ---------------------------------------------------------------------------
// 7. baseboard_electric_resistance_cop_unity
//    COP ≈ 1.0 for electric resistance baseboard (thermal = electrical * 1000)
// ---------------------------------------------------------------------------

#[test]
fn baseboard_electric_resistance_cop_unity() {
    let cfg = EquipmentConfig::from_typed(
        "bb".to_string(),
        "Electric Baseboard".to_string(),
        ElectricBaseboardConfig {
            equipment_id: None,
            zone_id: Some(1),
            capacity_w: 3_000.0,
            eir: 1.0,
        },
    );

    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Electric Baseboard", cfg.clone()).unwrap();
    let env = env_with_zone_temp(18.0);
    eq.init(&cfg, &env).unwrap();

    let mut ports = ports_for_zone1();
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let thermal_w = ports.thermal[0].sensible_gain_w;
    let electric_w = ports.electrical.net_active_kw() * 1_000.0;

    assert!(
        thermal_w > 1e-6,
        "baseboard must deliver positive heat, got {thermal_w:.3} W"
    );
    assert!(
        electric_w > 1e-6,
        "baseboard must draw electricity, got {electric_w:.3} W"
    );

    // COP = thermal_output / electrical_input; for resistive heating this is 1.0.
    // The space_fraction default is 1.0, so the ratio should be unity.
    let cop = thermal_w / electric_w;
    assert!(
        (cop - 1.0).abs() < 0.01,
        "electric baseboard COP must be ~1.0, got {cop:.4} (thermal={thermal_w:.1} W, electric={electric_w:.1} W)",
    );
}

// ---------------------------------------------------------------------------
// 8. setpoint_override_shifts_thermostat
//    Apply ThermalSetpoint → thermostat tracks new setpoint
// ---------------------------------------------------------------------------

#[test]
fn setpoint_override_shifts_thermostat() {
    // Gas furnace with zone at 22°C. Default heating setpoint 20°C → unit is off.
    let cfg = gas_furnace_config("furnace");

    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Gas Furnace", cfg.clone()).unwrap();
    let env = env_with_zone_temp(22.0);
    eq.init(&cfg, &env).unwrap();

    // Confirm off at 22°C with setpoint 21°C.
    let mode_before = eq.update_control(&env);
    assert_eq!(
        mode_before,
        OperatingMode::Off,
        "furnace should be off at 22°C with 21°C setpoint",
    );

    // Raise the heating setpoint to 25°C via control signal.
    // Now 22°C < 25°C - deadband so the thermostat should demand heating.
    eq.apply_control(&ControlSignal::ThermalSetpoint {
        heating_setpoint_c: Some(25.0),
        cooling_setpoint_c: None,
        deadband_c: None,
    })
    .unwrap();

    let mode_after = eq.update_control(&env);
    assert_eq!(
        mode_after,
        OperatingMode::Heating,
        "furnace should switch to Heating after setpoint raised to 25°C (zone is 22°C)",
    );

    let mut ports = ports_for_zone1();
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
    assert!(
        ports.thermal[0].sensible_gain_w > 1e-6,
        "furnace must deliver heat after setpoint override, got {:.3} W",
        ports.thermal[0].sensible_gain_w,
    );
}

// ---------------------------------------------------------------------------
// 9. checkpoint_round_trip_preserves_mode
//    save_state/load_state preserves operating mode and telemetry
// ---------------------------------------------------------------------------

#[test]
fn checkpoint_round_trip_preserves_mode() {
    let cfg = gas_furnace_config("furnace");
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Gas Furnace", cfg.clone()).unwrap();
    let env = env_with_zone_temp(18.0);
    eq.init(&cfg, &env).unwrap();

    // Run one step so there is non-trivial state to preserve.
    let mut ports = ports_for_zone1();
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let mode_before = eq.telemetry().get("operating_mode");
    let thermal_before = eq.telemetry().get("thermal_output_w");

    let snapshot = eq.save_state();

    // Restore into a fresh instance.
    let mut restored = registry.create("Gas Furnace", cfg.clone()).unwrap();
    restored.init(&cfg, &env).unwrap();
    restored.load_state(&snapshot).unwrap();

    let mode_after = restored.telemetry().get("operating_mode");
    let thermal_after = restored.telemetry().get("thermal_output_w");

    assert_eq!(
        mode_before, mode_after,
        "operating_mode telemetry must survive save/load round-trip",
    );
    assert_eq!(
        thermal_before, thermal_after,
        "thermal_output_w telemetry must survive save/load round-trip",
    );

    // The restored equipment must reproduce the same thermal output on the next step.
    let mut ports_restored = ports_for_zone1();
    // update_control is NOT called here; the loaded duty_cycle drives the step.
    restored
        .step(&env, Duration::from_secs(60), &mut ports_restored)
        .unwrap();
    assert!(
        (ports_restored.thermal[0].sensible_gain_w - ports.thermal[0].sensible_gain_w).abs() < 1e-6,
        "restored furnace must reproduce same thermal output: expected {:.3} W, got {:.3} W",
        ports.thermal[0].sensible_gain_w,
        ports_restored.thermal[0].sensible_gain_w,
    );
}
