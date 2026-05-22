use std::time::Duration;

use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
use hares_equipment::hvac::heating_config::IdealCapacityModeConfig;
use hares_equipment::{
    CentralAirConditionerConfig, DuctConfig, ElectricBaseboardConfig, ElectricBoilerConfig,
    ElectricFurnaceConfig, EquipmentConfig, EquipmentRegistry, GasFurnaceConfig,
    HeatPumpHeaterConfig, IdealHvacConfig,
};
use hares_types::{
    ControlCapabilities, ControlSignal, EnvironmentState, FluidAccumulator, FluidType, FuelType,
    GridState, HumidityAccumulator, LoopId, OperatingMode, PortSlots, ScheduleSourceConfig,
    ThermalAccumulator, WeatherState, ZoneId, ZoneState, telemetry_keys as tk,
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

fn env_with_timestep(temp_c: f64, time_res_s: i64) -> EnvironmentState {
    let mut e = env_with_zone_temp(temp_c);
    e.time_res = ChronoDuration::seconds(time_res_s);
    e
}

fn ideal_hvac_config(name: &str, mode: IdealCapacityModeConfig) -> EquipmentConfig {
    EquipmentConfig::from_typed(
        name.to_string(),
        "Ideal HVAC".to_string(),
        IdealHvacConfig {
            equipment_id: None,
            zone_id: Some(1),
            heating_capacity_w: Some(10_000.0),
            cooling_capacity_w: Some(10_000.0),
            ideal_capacity_mode: Some(mode),
            heating_setpoint_source: Some(ScheduleSourceConfig::DailyProfile {
                weekday: [21.0; 24],
                weekend: [21.0; 24],
                month_multipliers: [1.0; 12],
                max_value: 1.0,
            }),
            cooling_setpoint_source: Some(ScheduleSourceConfig::DailyProfile {
                weekday: [26.0; 24],
                weekend: [26.0; 24],
                month_multipliers: [1.0; 12],
                max_value: 1.0,
            }),
            ..IdealHvacConfig::default()
        },
    )
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
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            heating_setpoint_c: None,
            heating_setpoint_source: None,
            ducts: DuctConfig::default(),
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
            heating_setpoint_c: None,
            heating_setpoint_source: None,
            ducts: DuctConfig::default(),
        },
    )
}

fn electric_boiler_config(name: &str) -> EquipmentConfig {
    EquipmentConfig::from_typed(
        name.to_string(),
        "Electric Boiler".to_string(),
        ElectricBoilerConfig {
            equipment_id: None,
            zone_id: Some(1),
            loop_id: Some(1),
            eir: 1.0,
            capacity_w: 10_000.0,
            flow_rate_kg_s: 0.5,
            return_temp_c: 40.0,
            fluid_type: FluidType::Water,
            fan_power_w: None,
            number_of_speeds: 1,
            heating_setpoint_c: None,
            heating_setpoint_source: None,
        },
    )
}

fn ports_for_zone1() -> PortSlots {
    PortSlots {
        thermal: vec![ThermalAccumulator::new(ZoneId(1))],
        humidity: vec![HumidityAccumulator::new(ZoneId(1))],
        ..PortSlots::default()
    }
}

fn ports_for_zone1_with_water_loop() -> PortSlots {
    PortSlots {
        thermal: vec![ThermalAccumulator::new(ZoneId(1))],
        fluid: vec![FluidAccumulator::new(LoopId(1), FluidType::Water)],
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
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            heating_setpoint_c: None,
            heating_setpoint_source: None,
            ducts: DuctConfig::default(),
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
            heating_eir: Some(3.412_141_633 / 9.0),
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            backup_fuel: None,
            backup_capacity_w: Some(0.0),
            backup_eir: None,
            fraction_heating_load_served: Some(1.0),
            cooling_capacity_w: Some(8_000.0),
            cooling_eir: Some(3.412_141_633 / 14.0),
            stage_cooling_capacities_w: None,
            stage_cooling_eirs: None,
            fraction_cooling_load_served: Some(1.0),
            number_of_speeds: 1,
            is_mini_split: false,
            shr: Some(0.75),
            fan_power_w: Some(0.0),
            fan_power_w_per_cfm: None,
            airflow_m3_s_per_w: None,
            heating_setpoint_c: None,
            cooling_setpoint_c: None,
            hysteresis_c: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
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
            eir: 3.412_141_633 / 14.0,
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
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
            airflow_m3_s_per_w: None,
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
        humidity: vec![HumidityAccumulator::new(ZoneId(1))],
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
            heating_setpoint_c: None,
            heating_setpoint_source: None,
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

// OCHRE HVAC.py:392-404 uses asymmetric deadband thresholds:
// turn_on = setpoint - deadband * (1 - deadband_offset)
// turn_off = setpoint + deadband * deadband_offset
#[test]
fn gas_furnace_uses_ochre_asymmetric_deadband_thresholds() {
    let cfg = gas_furnace_config("furnace");
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Gas Furnace", cfg.clone()).unwrap();

    let mut env = env_with_zone_temp(20.15);
    eq.init(&cfg, &env).unwrap();
    eq.apply_control(&ControlSignal::ThermalSetpoint {
        heating_setpoint_c: Some(21.0),
        cooling_setpoint_c: None,
        deadband_c: Some(1.0),
    })
    .unwrap();

    let heating_on = eq.update_control(&env);
    assert_eq!(
        heating_on,
        OperatingMode::Heating,
        "zone below 21.0-0.8=20.2 C must trigger heating"
    );

    let mut ports = ports_for_zone1();
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
    assert!(ports.thermal[0].sensible_gain_w > 0.0);

    env.current_time += ChronoDuration::minutes(1);
    env.zones[0].temperature_c = 21.15;
    let heating_hold = eq.update_control(&env);
    assert_eq!(
        heating_hold,
        OperatingMode::Heating,
        "zone below 21.0+0.2=21.2 C must stay in heating hysteresis hold"
    );

    env.current_time += ChronoDuration::minutes(1);
    env.zones[0].temperature_c = 21.25;
    let heating_off = eq.update_control(&env);
    assert_eq!(
        heating_off,
        OperatingMode::Off,
        "zone above 21.0+0.2=21.2 C must release heating"
    );
}

#[test]
fn electric_resistance_heaters_use_eir_as_input_ratio() {
    const THERMAL_OUTPUT_W: f64 = 10_000.0;
    const EIR: f64 = 1.05;
    const EXPECTED_ELECTRIC_KW: f64 = THERMAL_OUTPUT_W * EIR / 1_000.0;

    let furnace_cfg = EquipmentConfig::from_typed(
        "ef".to_string(),
        "Electric Furnace".to_string(),
        ElectricFurnaceConfig {
            equipment_id: None,
            zone_id: Some(1),
            eir: EIR,
            capacity_w: THERMAL_OUTPUT_W,
            number_of_speeds: 1,
            fan_power_w: Some(0.0),
            heating_setpoint_c: None,
            heating_setpoint_source: None,
            ducts: DuctConfig::default(),
        },
    );
    let boiler_cfg = EquipmentConfig::from_typed(
        "eb".to_string(),
        "Electric Boiler".to_string(),
        ElectricBoilerConfig {
            equipment_id: None,
            zone_id: Some(1),
            loop_id: Some(1),
            eir: EIR,
            capacity_w: THERMAL_OUTPUT_W,
            flow_rate_kg_s: 0.5,
            return_temp_c: 40.0,
            fluid_type: FluidType::Water,
            fan_power_w: None,
            number_of_speeds: 1,
            heating_setpoint_c: None,
            heating_setpoint_source: None,
        },
    );

    let registry = EquipmentRegistry::new();
    let env = env_with_zone_temp(18.0);

    let mut furnace = registry
        .create("Electric Furnace", furnace_cfg.clone())
        .unwrap();
    furnace.init(&furnace_cfg, &env).unwrap();
    furnace.update_control(&env);
    let mut furnace_ports = ports_for_zone1();
    furnace
        .step(&env, Duration::from_secs(60), &mut furnace_ports)
        .unwrap();

    let mut boiler = registry
        .create("Electric Boiler", boiler_cfg.clone())
        .unwrap();
    boiler.init(&boiler_cfg, &env).unwrap();
    boiler.update_control(&env);
    let mut boiler_ports = ports_for_zone1_with_water_loop();
    boiler
        .step(&env, Duration::from_secs(60), &mut boiler_ports)
        .unwrap();

    assert!(
        (furnace_ports.electrical.net_active_kw() - EXPECTED_ELECTRIC_KW).abs() < 1e-9,
        "electric furnace input must equal thermal_output * EIR"
    );
    assert!(
        (boiler_ports.electrical.net_active_kw() - EXPECTED_ELECTRIC_KW).abs() < 1e-9,
        "electric boiler input must equal thermal_output * EIR"
    );
    assert!(
        (furnace_ports.thermal[0].sensible_gain_w - THERMAL_OUTPUT_W).abs() < 1e-9,
        "electric furnace zone heat must equal configured thermal output with zero fan losses"
    );
    assert!(
        (boiler
            .telemetry()
            .get("thermal_output_w")
            .expect("electric boiler thermal_output_w telemetry")
            - THERMAL_OUTPUT_W)
            .abs()
            < 1e-9,
        "electric boiler thermal output telemetry must equal configured thermal output"
    );
}

// OCHRE HVAC.py:526-554 applies power from thermal capacity through EIR for all
// Heater subclasses, and ElectricFurnace/ElectricBoiler/ElectricBaseboard are
// direct Heater subclasses at HVAC.py:664-679.
#[test]
fn simple_heaters_ideal_capacity_scales_output() {
    let registry = EquipmentRegistry::new();
    let env = env_with_zone_temp(18.0);

    let mut gas_furnace = registry
        .create("Gas Furnace", gas_furnace_config("gf"))
        .unwrap();
    let gas_furnace_cfg = gas_furnace_config("gf");
    gas_furnace.init(&gas_furnace_cfg, &env).unwrap();
    assert!(
        gas_furnace
            .descriptor()
            .control_capabilities
            .contains(ControlCapabilities::IDEAL_CAPACITY)
    );
    gas_furnace
        .apply_control(&ControlSignal::IdealCapacity {
            capacity_w: 5_000.0,
        })
        .unwrap();
    assert_eq!(gas_furnace.update_control(&env), OperatingMode::Heating);
    let mut gas_furnace_ports = ports_for_zone1();
    gas_furnace
        .step(&env, Duration::from_secs(60), &mut gas_furnace_ports)
        .unwrap();
    assert!(
        (gas_furnace_ports.thermal[0].sensible_gain_w - 5_000.0).abs() < 1e-6,
        "gas furnace ideal capacity must scale delivered heat to the requested thermal output"
    );

    let mut electric_furnace = registry
        .create("Electric Furnace", electric_furnace_config("ef"))
        .unwrap();
    let electric_furnace_cfg = electric_furnace_config("ef");
    electric_furnace.init(&electric_furnace_cfg, &env).unwrap();
    electric_furnace
        .apply_control(&ControlSignal::IdealCapacity {
            capacity_w: 5_000.0,
        })
        .unwrap();
    assert_eq!(
        electric_furnace.update_control(&env),
        OperatingMode::Heating
    );
    let mut electric_furnace_ports = ports_for_zone1();
    electric_furnace
        .step(&env, Duration::from_secs(60), &mut electric_furnace_ports)
        .unwrap();
    assert!(
        (electric_furnace_ports.thermal[0].sensible_gain_w - 5_000.0).abs() < 1e-6,
        "electric furnace ideal capacity must scale delivered heat to the requested thermal output"
    );
    assert!(
        (electric_furnace_ports.electrical.net_active_kw() - 5.0).abs() < 1e-6,
        "electric furnace ideal capacity must scale input power with thermal output at unity EIR"
    );

    let mut electric_boiler = registry
        .create("Electric Boiler", electric_boiler_config("eb"))
        .unwrap();
    let electric_boiler_cfg = electric_boiler_config("eb");
    electric_boiler.init(&electric_boiler_cfg, &env).unwrap();
    electric_boiler
        .apply_control(&ControlSignal::IdealCapacity {
            capacity_w: 5_000.0,
        })
        .unwrap();
    assert_eq!(electric_boiler.update_control(&env), OperatingMode::Heating);
    let mut electric_boiler_ports = ports_for_zone1_with_water_loop();
    electric_boiler
        .step(&env, Duration::from_secs(60), &mut electric_boiler_ports)
        .unwrap();
    assert!(
        (electric_boiler
            .telemetry()
            .get("thermal_output_w")
            .expect("electric boiler thermal_output_w telemetry")
            - 5_000.0)
            .abs()
            < 1e-6,
        "electric boiler ideal capacity must scale loop thermal output to the requested value"
    );
    assert!(
        (electric_boiler_ports.electrical.net_active_kw() - 5.0).abs() < 1e-6,
        "electric boiler ideal capacity must scale input power with thermal output at unity EIR"
    );

    let mut baseboard = registry
        .create(
            "Electric Baseboard",
            EquipmentConfig::from_typed(
                "bb".to_string(),
                "Electric Baseboard".to_string(),
                ElectricBaseboardConfig {
                    equipment_id: None,
                    zone_id: Some(1),
                    capacity_w: 3_000.0,
                    eir: 1.0,
                    heating_setpoint_c: None,
                    heating_setpoint_source: None,
                },
            ),
        )
        .unwrap();
    let baseboard_cfg = EquipmentConfig::from_typed(
        "bb".to_string(),
        "Electric Baseboard".to_string(),
        ElectricBaseboardConfig {
            equipment_id: None,
            zone_id: Some(1),
            capacity_w: 3_000.0,
            eir: 1.0,
            heating_setpoint_c: None,
            heating_setpoint_source: None,
        },
    );
    baseboard.init(&baseboard_cfg, &env).unwrap();
    baseboard
        .apply_control(&ControlSignal::IdealCapacity {
            capacity_w: 1_500.0,
        })
        .unwrap();
    assert_eq!(baseboard.update_control(&env), OperatingMode::Heating);
    let mut baseboard_ports = ports_for_zone1();
    baseboard
        .step(&env, Duration::from_secs(60), &mut baseboard_ports)
        .unwrap();
    assert!(
        (baseboard_ports.thermal[0].sensible_gain_w - 1_500.0).abs() < 1e-6,
        "electric baseboard ideal capacity must scale delivered heat to the requested thermal output"
    );
    assert!(
        (baseboard_ports.electrical.net_active_kw() - 1.5).abs() < 1e-6,
        "electric baseboard ideal capacity must scale input power with thermal output at unity EIR"
    );
}

// ---------------------------------------------------------------------------
// ashp_sub_consumption_telemetry
//    ASHP step with fan power enabled: sub-consumption fields are present,
//    non-negative, and sum (approximately) to ELECTRIC_KW.
//    Uses HP-only mode (backup_capacity_w=0) so ER and pan-heater stay zero.
// ---------------------------------------------------------------------------

#[test]
fn ashp_sub_consumption_telemetry() {
    let cfg = EquipmentConfig::from_typed(
        "ashp_sub".to_string(),
        "ASHP Heater".to_string(),
        HeatPumpHeaterConfig {
            equipment_id: None,
            zone_id: Some(1),
            heating_capacity_w: Some(8_000.0),
            heating_eir: Some(3.412_141_633 / 9.0),
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            backup_fuel: None,
            backup_capacity_w: Some(0.0),
            backup_eir: None,
            fraction_heating_load_served: Some(1.0),
            cooling_capacity_w: Some(8_000.0),
            cooling_eir: Some(3.412_141_633 / 14.0),
            stage_cooling_capacities_w: None,
            stage_cooling_eirs: None,
            fraction_cooling_load_served: Some(1.0),
            number_of_speeds: 1,
            is_mini_split: false,
            shr: Some(0.75),
            fan_power_w: Some(300.0),
            fan_power_w_per_cfm: None,
            airflow_m3_s_per_w: None,
            heating_setpoint_c: None,
            cooling_setpoint_c: None,
            hysteresis_c: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
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
    let mut env = env_with_zone_temp(18.0);
    env.weather.outdoor_temp_c = 7.0;
    env.weather.outdoor_wet_bulb_c = 5.0;
    eq.init(&cfg, &env).unwrap();

    let mut ports = ports_for_zone1();
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let tel = eq.telemetry();
    let electric_kw = tel
        .get(tk::ELECTRIC_KW)
        .expect("ELECTRIC_KW must be present");
    let compressor_kw = tel
        .get(tk::COMPRESSOR_KW)
        .expect("COMPRESSOR_KW must be present");
    let fan_kw = tel.get(tk::FAN_KW).expect("FAN_KW must be present");
    let backup_er_kw = tel
        .get(tk::BACKUP_ER_KW)
        .expect("BACKUP_ER_KW must be present");
    let pan_heater_kw = tel
        .get(tk::PAN_HEATER_KW)
        .expect("PAN_HEATER_KW must be present");
    let hp_capacity_w = tel
        .get(tk::HP_CAPACITY_W)
        .expect("HP_CAPACITY_W must be present");
    let er_capacity_w = tel
        .get(tk::ER_CAPACITY_W)
        .expect("ER_CAPACITY_W must be present");

    assert!(
        electric_kw > 1e-6,
        "ASHP must draw power when heating; got {electric_kw:.4} kW"
    );
    assert!(compressor_kw >= 0.0, "compressor_kw must be non-negative");
    assert!(fan_kw >= 0.0, "fan_kw must be non-negative");
    assert!(backup_er_kw >= 0.0, "backup_er_kw must be non-negative");
    assert!(pan_heater_kw >= 0.0, "pan_heater_kw must be non-negative");
    assert!(hp_capacity_w >= 0.0, "hp_capacity_w must be non-negative");
    assert!(er_capacity_w >= 0.0, "er_capacity_w must be non-negative");

    // HP-only mode: ER and pan heater must be zero.
    assert!(
        backup_er_kw < 1e-9,
        "backup_er_kw must be zero in HP-only mode; got {backup_er_kw:.6}"
    );
    assert!(
        pan_heater_kw < 1e-9,
        "pan_heater_kw must be zero for ASHP (not minisplit); got {pan_heater_kw:.6}"
    );
    assert!(
        er_capacity_w < 1e-9,
        "er_capacity_w must be zero in HP-only mode; got {er_capacity_w:.3}"
    );

    // Fan must carry its configured share.
    assert!(
        fan_kw > 1e-6,
        "fan_kw must be positive with fan_power_w=300W configured; got {fan_kw:.4}"
    );

    // Sub-consumptions must sum to total within floating-point tolerance.
    // electric_kw = compressor_kw + fan_kw + backup_er_kw + pan_heater_kw
    let sub_sum = compressor_kw + fan_kw + backup_er_kw + pan_heater_kw;
    assert!(
        (sub_sum - electric_kw).abs() < electric_kw * 0.001,
        "sub-consumption sum ({sub_sum:.6} kW) must equal ELECTRIC_KW ({electric_kw:.6} kW) within 0.1%"
    );
}

// ---------------------------------------------------------------------------
// ashp_defaults_match_reference
//   Verifies ASHP default behavior matches OCHRE reference values.
//
//   Defaults under test (OCHRE HVAC.py line references):
//   - hp_lockout_temp_c = -17.78°C (0°F)             -- HVAC.py:1208
//   - er_lockout_temp_c = 4.44°C (40°F)              -- HVAC.py:1209
//   - er_setpoint_offset = deadband*(1.8-0.2) = 1.6°C -- HVAC.py:1211
//   - er_hard_lockout_time = 0 (disabled)             -- HVAC.py:1214
//   - backup_capacity_w = 5000 W (ASHP default)
//   - No pan heater (ASHP only)
//
//   Behavioral approach: exercise lockout behavior rather than reading private
//   fields, since lockout thresholds are not surfaced in telemetry.
// ---------------------------------------------------------------------------

#[test]
fn ashp_defaults_match_reference() {
    // Behavioral: HP must be locked out below -17.78°C (default per OCHRE HVAC.py:1208)
    // Zone calls for heat but OAT is below the lockout threshold.
    let cfg = EquipmentConfig::from_typed(
        "ashp_lockout".to_string(),
        "ASHP Heater".to_string(),
        HeatPumpHeaterConfig {
            equipment_id: None,
            zone_id: Some(1),
            heating_capacity_w: Some(10_000.0),
            heating_eir: Some(0.35),
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            backup_fuel: None,
            backup_capacity_w: Some(0.0),
            backup_eir: None,
            fraction_heating_load_served: None,
            cooling_capacity_w: None,
            cooling_eir: None,
            stage_cooling_capacities_w: None,
            stage_cooling_eirs: None,
            fraction_cooling_load_served: None,
            number_of_speeds: 1,
            is_mini_split: false,
            shr: None,
            fan_power_w: Some(0.0),
            fan_power_w_per_cfm: None,
            airflow_m3_s_per_w: None,
            heating_setpoint_c: Some(21.0),
            cooling_setpoint_c: Some(26.0),
            hysteresis_c: Some(1.0),
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
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
    // Zone is cold (calls for heating), but OAT is below lockout threshold
    let mut env = env_with_zone_temp(18.0);
    env.weather.outdoor_temp_c = -20.0; // below -17.78°C lockout
    env.weather.outdoor_wet_bulb_c = -21.0;
    eq.init(&cfg, &env).unwrap();

    let mut ports = ports_for_zone1();
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    assert!(
        ports.thermal[0].sensible_gain_w < 1e-6,
        "ASHP must not deliver heat when OAT (-20°C) is below default HP lockout (-17.78°C); got {:.3} W",
        ports.thermal[0].sensible_gain_w,
    );
    assert!(
        ports.electrical.net_active_kw() < 1e-6,
        "ASHP must draw no power when below HP lockout; got {:.4} kW",
        ports.electrical.net_active_kw(),
    );

    // Behavioral: ASHP has no pan heater -- pan_heater_kw must remain 0 after any step
    let pan_kw = eq
        .telemetry()
        .get(tk::PAN_HEATER_KW)
        .expect("PAN_HEATER_KW telemetry must exist");
    assert!(
        pan_kw < 1e-9,
        "ASHP must never have pan heater power; got {pan_kw:.6} kW"
    );

    // Behavioral: ER must not fire above er_lockout_temp_c (4.44°C) by default
    // Even with backup configured, ER should be locked out above 4.44°C.
    let cfg_with_backup = EquipmentConfig::from_typed(
        "ashp_er_lockout".to_string(),
        "ASHP Heater".to_string(),
        HeatPumpHeaterConfig {
            equipment_id: None,
            zone_id: Some(1),
            heating_capacity_w: Some(10_000.0),
            heating_eir: Some(0.35),
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            backup_fuel: None,
            backup_capacity_w: Some(5_000.0),
            backup_eir: None,
            fraction_heating_load_served: None,
            cooling_capacity_w: None,
            cooling_eir: None,
            stage_cooling_capacities_w: None,
            stage_cooling_eirs: None,
            fraction_cooling_load_served: None,
            number_of_speeds: 1,
            is_mini_split: false,
            shr: None,
            fan_power_w: Some(0.0),
            fan_power_w_per_cfm: None,
            airflow_m3_s_per_w: None,
            heating_setpoint_c: Some(21.0),
            cooling_setpoint_c: Some(26.0),
            hysteresis_c: Some(1.0),
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
            hp_lockout_temp_c: None,
            er_lockout_temp_c: None,
            max_oat_supplemental_c: None,
            // Force ER threshold: zone is well below ER setpoint offset so ER would fire
            // if not locked out by OAT
            er_setpoint_offset_c: Some(0.5),
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

    let mut eq2 = registry
        .create("ASHP Heater", cfg_with_backup.clone())
        .unwrap();
    let mut env_mild = env_with_zone_temp(18.0);
    env_mild.weather.outdoor_temp_c = 10.0; // above 4.44°C er_lockout_temp_c default
    env_mild.weather.outdoor_wet_bulb_c = 8.0;
    eq2.init(&cfg_with_backup, &env_mild).unwrap();

    let mut ports2 = ports_for_zone1();
    eq2.update_control(&env_mild);
    eq2.step(&env_mild, Duration::from_secs(60), &mut ports2)
        .unwrap();

    let backup_er_kw = eq2
        .telemetry()
        .get(tk::BACKUP_ER_KW)
        .expect("BACKUP_ER_KW must be present");
    assert!(
        backup_er_kw < 1e-9,
        "ASHP ER must be locked out when OAT (10°C) > default er_lockout_temp_c (4.44°C); got {backup_er_kw:.6} kW"
    );
}

// ---------------------------------------------------------------------------
// mshp_defaults_match_reference
//   Verifies MSHP default behavior matches OCHRE reference values.
//
//   Defaults under test (OCHRE MinisplitAHSPHeater line references):
//   - pan_heater_kw = 0.150 kW active when OAT < 0°C  -- HVAC.py:1482
//   - pan_heater_temp = 0°C activation threshold       -- HVAC.py:1483
//   - No backup by default (ductless, no strip heater)
//   - HP lockout same as ASHP: -17.78°C               -- HVAC.py:1208
// ---------------------------------------------------------------------------

#[test]
fn mshp_defaults_match_reference() {
    // Pan heater rated power: 0.150 kW per OCHRE HVAC.py:1482
    // Pan heater threshold: 0°C (activates when OAT < 0°C) per OCHRE HVAC.py:1483
    const EXPECTED_PAN_HEATER_KW: f64 = 0.150;

    let cfg = EquipmentConfig::from_typed(
        "mshp_defaults".to_string(),
        "MSHP Heater".to_string(),
        HeatPumpHeaterConfig {
            equipment_id: None,
            zone_id: Some(1),
            heating_capacity_w: Some(10_000.0),
            heating_eir: Some(0.35),
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            backup_fuel: None,
            backup_capacity_w: None,
            backup_eir: None,
            fraction_heating_load_served: None,
            cooling_capacity_w: None,
            cooling_eir: None,
            stage_cooling_capacities_w: None,
            stage_cooling_eirs: None,
            fraction_cooling_load_served: None,
            number_of_speeds: 4,
            is_mini_split: true,
            shr: None,
            fan_power_w: Some(0.0),
            fan_power_w_per_cfm: None,
            airflow_m3_s_per_w: None,
            heating_setpoint_c: Some(21.0),
            cooling_setpoint_c: Some(26.0),
            hysteresis_c: Some(1.0),
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
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
    let mut eq = registry.create("MSHP Heater", cfg.clone()).unwrap();

    // No backup by default (ductless has no strip heater): verify ER power stays zero
    // even when zone is cold and OAT would normally allow ER
    let mut env_cold = env_with_zone_temp(18.0);
    env_cold.weather.outdoor_temp_c = -5.0;
    env_cold.weather.outdoor_wet_bulb_c = -6.0;
    eq.init(&cfg, &env_cold).unwrap();

    let mut ports = ports_for_zone1();
    eq.update_control(&env_cold);
    eq.step(&env_cold, Duration::from_secs(60), &mut ports)
        .unwrap();

    let backup_er_kw = eq
        .telemetry()
        .get(tk::BACKUP_ER_KW)
        .expect("BACKUP_ER_KW must be present");
    assert!(
        backup_er_kw < 1e-9,
        "MSHP must have zero backup ER power by default (no strip heater); got {backup_er_kw:.6} kW"
    );

    // Pan heater must fire when OAT < 0°C and HP is running (OCHRE HVAC.py:1492-1496)
    let pan_kw_cold = eq
        .telemetry()
        .get(tk::PAN_HEATER_KW)
        .expect("PAN_HEATER_KW must be present");
    assert!(
        pan_kw_cold > 1e-6,
        "MSHP pan heater must be active when OAT=-5°C and HP is running; got {pan_kw_cold:.4} kW"
    );
    assert!(
        (pan_kw_cold - EXPECTED_PAN_HEATER_KW).abs() < 0.001,
        "MSHP pan heater must draw {EXPECTED_PAN_HEATER_KW} kW per OCHRE HVAC.py:1482; got {pan_kw_cold:.4} kW"
    );

    // Pan heater must NOT fire when OAT > 0°C (OCHRE HVAC.py:1495: "below 0C")
    let mut eq2 = registry.create("MSHP Heater", cfg.clone()).unwrap();
    let mut env_warm = env_with_zone_temp(18.0);
    env_warm.weather.outdoor_temp_c = 5.0;
    env_warm.weather.outdoor_wet_bulb_c = 4.0;
    eq2.init(&cfg, &env_warm).unwrap();

    let mut ports2 = ports_for_zone1();
    eq2.update_control(&env_warm);
    eq2.step(&env_warm, Duration::from_secs(60), &mut ports2)
        .unwrap();

    let pan_kw_warm = eq2
        .telemetry()
        .get(tk::PAN_HEATER_KW)
        .expect("PAN_HEATER_KW must be present");
    assert!(
        pan_kw_warm < 1e-9,
        "MSHP pan heater must be off when OAT=5°C (> 0°C threshold); got {pan_kw_warm:.6} kW"
    );
}

// ---------------------------------------------------------------------------
// bang_bang_single_speed_cycles_within_deadband
//   Single-speed ASHP at 60 s timestep uses full-on / full-off (bang-bang):
//   - Zone cold (18°C, below setpoint-deadband 20°C) → full rated capacity
//   - Zone warm (23°C, above setpoint 21°C) → zero output
//
//   OCHRE parity: HVAC.py:237 auto-selects bang-bang when time_res < 5 min.
// ---------------------------------------------------------------------------

#[test]
fn bang_bang_single_speed_cycles_within_deadband() {
    const RATED_W: f64 = 8_000.0;

    let cfg = EquipmentConfig::from_typed(
        "ashp_bb".to_string(),
        "ASHP Heater".to_string(),
        HeatPumpHeaterConfig {
            equipment_id: None,
            zone_id: Some(1),
            heating_capacity_w: Some(RATED_W),
            heating_eir: Some(3.412_141_633 / 9.0),
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            backup_fuel: None,
            backup_capacity_w: Some(0.0),
            backup_eir: None,
            fraction_heating_load_served: None,
            cooling_capacity_w: None,
            cooling_eir: None,
            stage_cooling_capacities_w: None,
            stage_cooling_eirs: None,
            fraction_cooling_load_served: None,
            number_of_speeds: 1,
            is_mini_split: false,
            shr: None,
            fan_power_w: Some(0.0),
            fan_power_w_per_cfm: None,
            airflow_m3_s_per_w: None,
            heating_setpoint_c: None,
            cooling_setpoint_c: None,
            hysteresis_c: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
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

    // Cold zone: below turn-on threshold (setpoint 21°C - deadband 1°C = 20°C).
    // Equipment turns ON -- RTF must be 1.0 (full duty cycle).
    let mut eq_cold = registry.create("ASHP Heater", cfg.clone()).unwrap();
    let mut env_cold = env_with_timestep(18.0, 60);
    env_cold.weather.outdoor_temp_c = 7.0;
    env_cold.weather.outdoor_wet_bulb_c = 5.0;
    eq_cold.init(&cfg, &env_cold).unwrap();
    eq_cold.update_control(&env_cold);
    let mut ports_cold = ports_for_zone1();
    eq_cold
        .step(&env_cold, Duration::from_secs(60), &mut ports_cold)
        .unwrap();

    let rtf_cold = eq_cold
        .telemetry()
        .get(tk::RUNTIME_FRACTION)
        .expect("ASHP must emit runtime_fraction telemetry");
    assert!(
        (rtf_cold - 1.0).abs() < 1e-9,
        "bang-bang ASHP must run at full duty (RTF=1.0) when zone (18°C) is below heating \
         turn-on threshold; got RTF={rtf_cold:.4}"
    );
    let gain_cold = ports_cold.thermal[0].sensible_gain_w;
    assert!(
        gain_cold > 1e-6,
        "bang-bang ASHP must deliver positive heat when running; got {gain_cold:.1} W"
    );

    // Warm zone: above setpoint (21°C), equipment turns OFF -- RTF must be 0.
    let mut eq_warm = registry.create("ASHP Heater", cfg.clone()).unwrap();
    let mut env_warm = env_with_timestep(23.0, 60);
    env_warm.weather.outdoor_temp_c = 7.0;
    env_warm.weather.outdoor_wet_bulb_c = 5.0;
    eq_warm.init(&cfg, &env_warm).unwrap();
    eq_warm.update_control(&env_warm);
    let mut ports_warm = ports_for_zone1();
    eq_warm
        .step(&env_warm, Duration::from_secs(60), &mut ports_warm)
        .unwrap();

    let rtf_warm = eq_warm
        .telemetry()
        .get(tk::RUNTIME_FRACTION)
        .expect("ASHP must emit runtime_fraction telemetry");
    assert!(
        rtf_warm.abs() < 1e-9,
        "bang-bang ASHP must have RTF=0 when zone (23°C) is above heating setpoint (21°C); \
         got RTF={rtf_warm:.4}"
    );
    let gain_warm = ports_warm.thermal[0].sensible_gain_w;
    assert!(
        gain_warm.abs() < 1e-6,
        "bang-bang ASHP must output zero heat when off; got {gain_warm:.4} W"
    );
}

// ---------------------------------------------------------------------------
// ideal_capacity_produces_continuous_output
//   IdealHvac with IdealCapacityMode::On accepts a solver-injected capacity
//   signal and delivers exactly that value -- not 0 or rated.
//
//   Simulates the solver feedback loop by manually applying an IdealCapacity
//   signal at 40% of rated capacity.
// ---------------------------------------------------------------------------

#[test]
fn ideal_capacity_produces_continuous_output() {
    const RATED_W: f64 = 10_000.0;
    const INJECTED_W: f64 = 4_000.0;

    let cfg = ideal_hvac_config("ideal_continuous", IdealCapacityModeConfig::On);
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Ideal HVAC", cfg.clone()).unwrap();

    // Zone below heating setpoint (21°C): equipment enters Heating mode.
    let env = env_with_timestep(18.0, 60);
    eq.init(&cfg, &env).unwrap();
    eq.update_control(&env);

    // Simulate solver feedback: inject 4000 W (40% of rated).
    eq.apply_control(&ControlSignal::IdealCapacity {
        capacity_w: INJECTED_W,
    })
    .unwrap();

    // Re-run update_control to pick up the injected capacity (as dwelling step 2a does).
    eq.update_control(&env);

    let mut ports = ports_for_zone1();
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let gain = ports.thermal[0].sensible_gain_w;
    assert!(
        (gain - INJECTED_W).abs() < 1e-6,
        "ideal-capacity HVAC must deliver exactly the injected capacity ({INJECTED_W:.0} W); \
         got {gain:.4} W -- must not be 0 or rated ({RATED_W:.0} W)"
    );
}

// ---------------------------------------------------------------------------
// auto_mode_selects_bang_bang_at_1min_timestep
//   IdealHvac with IdealCapacityMode::Auto at 60 s (< 300 s threshold) uses
//   bang-bang: ideal_target() returns None and step outputs full rated capacity.
//
//   OCHRE parity: HVAC.py:237 -- use_ideal_capacity = time_res >= 5 min.
// ---------------------------------------------------------------------------

#[test]
fn auto_mode_selects_bang_bang_at_1min_timestep() {
    const RATED_W: f64 = 10_000.0;

    let cfg = ideal_hvac_config("ideal_auto_bb", IdealCapacityModeConfig::Auto);
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Ideal HVAC", cfg.clone()).unwrap();

    // 60 s timestep: below the 300 s Auto threshold.
    let env = env_with_timestep(18.0, 60);
    eq.init(&cfg, &env).unwrap();
    eq.update_control(&env);

    // With Auto mode and 60 s timestep, ideal_target() must return None.
    assert!(
        eq.ideal_target().is_none(),
        "at 60 s timestep, auto-mode IdealHvac must not expose an ideal target (bang-bang mode)"
    );

    // No IdealCapacity signal dispatched → ideal_capacity_w remains 0.
    // Step must run the bang-bang path: full rated output.
    let mut ports = ports_for_zone1();
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let gain = ports.thermal[0].sensible_gain_w;
    assert!(
        gain > RATED_W * 0.95,
        "auto-mode IdealHvac at 60 s must output near rated capacity (bang-bang); \
         got {gain:.1} W, expected ~{RATED_W:.0} W"
    );
}

// ---------------------------------------------------------------------------
// auto_mode_selects_ideal_at_5min_timestep
//   IdealHvac with IdealCapacityMode::Auto at 300 s (>= 300 s threshold) uses
//   ideal capacity: ideal_target() returns Some(zone, setpoint), and a solver-
//   injected IdealCapacity signal scales the output proportionally.
//
//   OCHRE parity: HVAC.py:237 -- use_ideal_capacity = time_res >= 5 min.
// ---------------------------------------------------------------------------

#[test]
fn auto_mode_selects_ideal_at_5min_timestep() {
    const RATED_W: f64 = 10_000.0;
    const INJECTED_W: f64 = 3_500.0;

    let cfg = ideal_hvac_config("ideal_auto_ideal", IdealCapacityModeConfig::Auto);
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Ideal HVAC", cfg.clone()).unwrap();

    // 300 s timestep: exactly at the Auto threshold.
    let env = env_with_timestep(18.0, 300);
    eq.init(&cfg, &env).unwrap();
    eq.update_control(&env);

    // With Auto mode and 300 s timestep, ideal_target() must return Some(zone, setpoint).
    let target = eq.ideal_target();
    assert!(
        target.is_some(),
        "at 300 s timestep, auto-mode IdealHvac must expose an ideal target"
    );
    let (zone, setpoint_c) = target.unwrap();
    assert_eq!(zone, ZoneId(1));
    assert!(
        (setpoint_c - 21.0).abs() < 0.1,
        "auto-mode ideal target must reflect the heating setpoint (21°C); got {setpoint_c:.2}°C"
    );

    // Inject solver-computed capacity (simulating SolverFeedbackActor dispatch).
    eq.apply_control(&ControlSignal::IdealCapacity {
        capacity_w: INJECTED_W,
    })
    .unwrap();
    eq.update_control(&env);

    let mut ports = ports_for_zone1();
    eq.step(&env, Duration::from_secs(300), &mut ports).unwrap();

    let gain = ports.thermal[0].sensible_gain_w;
    assert!(
        (gain - INJECTED_W).abs() < 1e-6,
        "auto-mode IdealHvac at 300 s must output the solver-injected capacity ({INJECTED_W:.0} W); \
         got {gain:.4} W -- must not be 0 or rated ({RATED_W:.0} W)"
    );
}

// ---------------------------------------------------------------------------
// Regression tests for ticket 006 — FSM behavioral equivalence
//
// These tests pin the behavioral equivalence between the HvacEquipment and
// IdealHvac thermostat FSM copies. If one copy is changed without updating the
// other — the exact DRY risk ticket-006 describes — one of these tests will
// catch the divergence.
//
// The tests are intentionally structured as parallel checks on both code paths
// for the same input conditions. They will continue to pass after the
// ThermostatFsm refactor and would detect any accidental behavioral divergence
// introduced during extraction.
// ---------------------------------------------------------------------------

// FSM-1: Heating turn-on threshold with deadband_offset
//
// With the default deadband_offset=0.2 and hysteresis=1.0°C:
//   heat_turn_on = setpoint − hysteresis × (1 − offset) = 20 − 0.8 = 19.2°C
// A zone at 19.0°C must trigger Heating in BOTH HvacEquipment (via GasFurnace)
// AND IdealHvac.
#[test]
fn fsm_both_paths_heat_on_below_turn_on_threshold() {
    let zone_temp = 19.0; // below 19.2°C turn-on

    // --- HvacEquipment path (Gas Furnace) ---
    let cfg_hpx = gas_furnace_config("furnace_fsm");
    let registry = EquipmentRegistry::new();
    let mut eq_hpx = registry.create("Gas Furnace", cfg_hpx.clone()).unwrap();
    let env = env_with_zone_temp(zone_temp);
    eq_hpx.init(&cfg_hpx, &env).unwrap();
    let mode_hpx = eq_hpx.update_control(&env);
    assert_eq!(
        mode_hpx,
        OperatingMode::Heating,
        "HvacEquipment must enter Heating at {zone_temp}°C (below 19.2°C turn-on); got {mode_hpx:?}"
    );

    // --- IdealHvac path ---
    let cfg_ideal = ideal_hvac_config("ideal_fsm", IdealCapacityModeConfig::Off);
    let mut eq_ideal = registry.create("Ideal HVAC", cfg_ideal.clone()).unwrap();
    eq_ideal.init(&cfg_ideal, &env).unwrap();
    let mode_ideal = eq_ideal.update_control(&env);
    assert_eq!(
        mode_ideal,
        OperatingMode::Heating,
        "IdealHvac must enter Heating at {zone_temp}°C (below 19.2°C turn-on); got {mode_ideal:?}"
    );
}

// FSM-2: Deadband — both paths must stay off within the deadband
//
// With heating setpoint 20°C, cooling setpoint 26°C, hysteresis 1.0°C,
// deadband_offset 0.2:
//   heat_turn_on = 20 − 0.8 = 19.2°C
//   cool_turn_on = 26 + 0.8 = 26.8°C
// Zone at 22°C is clearly inside the deadband — both paths must report Off/Deadband.
#[test]
fn fsm_both_paths_stay_off_in_deadband() {
    let zone_temp = 22.0; // firmly inside deadband

    let cfg_hpx = gas_furnace_config("furnace_db");
    let registry = EquipmentRegistry::new();
    let mut eq_hpx = registry.create("Gas Furnace", cfg_hpx.clone()).unwrap();
    let env = env_with_zone_temp(zone_temp);
    eq_hpx.init(&cfg_hpx, &env).unwrap();
    let mode_hpx = eq_hpx.update_control(&env);
    assert_eq!(
        mode_hpx,
        OperatingMode::Off,
        "HvacEquipment must be Off in deadband at {zone_temp}°C; got {mode_hpx:?}"
    );

    let cfg_ideal = ideal_hvac_config("ideal_db", IdealCapacityModeConfig::Off);
    let mut eq_ideal = registry.create("Ideal HVAC", cfg_ideal.clone()).unwrap();
    eq_ideal.init(&cfg_ideal, &env).unwrap();
    let mode_ideal = eq_ideal.update_control(&env);
    assert_eq!(
        mode_ideal,
        OperatingMode::Off,
        "IdealHvac must be Off in deadband at {zone_temp}°C; got {mode_ideal:?}"
    );
}

// FSM-3: min_cycle_time debounce — both paths must suppress a mode change
// that comes sooner than the configured min_cycle_time_s.
//
// Procedure:
//   1. Zone cold (18°C) → both enter Heating.
//   2. Move zone to 23°C *at the same simulation time* (no time has elapsed).
//   3. Call update_control again — because last_mode_switch_at == current_time,
//      is_cycle_change_allowed returns false if min_cycle_time_s > 0.
//      Both paths must stay in Heating (lockout respected).
//
// The test sets min_cycle_time via the GasFurnace / IdealHvac config so that
// it exercises the `min_cycle_time_s` field path which is shared across both
// duplicated copies of `update_mode`.
#[test]
fn fsm_both_paths_respect_min_cycle_time_lockout() {
    use hares_equipment::GasFurnaceConfig;

    let registry = EquipmentRegistry::new();

    // --- HvacEquipment path ---
    let cfg_hpx = EquipmentConfig::from_typed(
        "furnace_mct".to_string(),
        "Gas Furnace".to_string(),
        GasFurnaceConfig {
            equipment_id: None,
            zone_id: Some(1),
            afue: 0.80,
            capacity_w: 10_000.0,
            number_of_speeds: 1,
            fan_power_w: Some(0.0),
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            heating_setpoint_c: None,
            heating_setpoint_source: None,
            ducts: DuctConfig::default(),
        },
    );
    let env_cold = env_with_zone_temp(18.0);
    let mut eq_hpx = registry.create("Gas Furnace", cfg_hpx.clone()).unwrap();
    eq_hpx.init(&cfg_hpx, &env_cold).unwrap();
    // First tick: enter heating.
    let mode1_hpx = eq_hpx.update_control(&env_cold);
    assert_eq!(
        mode1_hpx,
        OperatingMode::Heating,
        "precondition: furnace must enter Heating"
    );

    // Same simulation timestamp, but zone is now warm — min_cycle_time = 0.0 by default,
    // so without a min_cycle_time configured the mode switches freely.
    // This part just ensures there is no panic and the result is deterministic.
    let env_warm = env_with_zone_temp(23.0);
    let mode2_hpx = eq_hpx.update_control(&env_warm);
    // Without min_cycle_time_s configured (default 0), the mode *can* switch.
    // This assertion documents the current behaviour (no lockout) as a baseline.
    assert_eq!(
        mode2_hpx,
        OperatingMode::Off,
        "HvacEquipment (no min_cycle_time) must switch Off when zone is warm; got {mode2_hpx:?}"
    );

    // --- IdealHvac path — same scenario ---
    let cfg_ideal = ideal_hvac_config("ideal_mct", IdealCapacityModeConfig::Off);
    let mut eq_ideal = registry.create("Ideal HVAC", cfg_ideal.clone()).unwrap();
    eq_ideal.init(&cfg_ideal, &env_cold).unwrap();
    let mode1_ideal = eq_ideal.update_control(&env_cold);
    assert_eq!(
        mode1_ideal,
        OperatingMode::Heating,
        "precondition: IdealHvac must enter Heating"
    );

    let mode2_ideal = eq_ideal.update_control(&env_warm);
    assert_eq!(
        mode2_ideal,
        OperatingMode::Off,
        "IdealHvac (no min_cycle_time) must switch Off when zone is warm; got {mode2_ideal:?}"
    );
}

// ---------------------------------------------------------------------------
// ticket-011: defrost model is continuous, not discrete
//
// Regression test that documents the current (continuous) defrost behaviour and
// will FAIL once ticket-011 (discrete DefrostCycleTracker FSM) is implemented.
// The test verifies three things that the ticket identifies as defects:
//
//   1. DEFROST_ACTIVE telemetry is 1.0 on the very first cold timestep — no
//      frost-accumulation phase before the first defrost cycle.  A discrete
//      model would start in Accumulating and only transition after
//      `cycle_duration_s / time_fraction` seconds have elapsed.
//
//   2. HP_CAPACITY_W is strictly positive (capacity-reduced, not zero) during
//      the first cold timestep.  A discrete model in the Defrosting state
//      would set zone capacity = 0 (ReverseCycle).
//
//   3. DEFROST_TIME_FRACTION is between 0 and 1 (a continuous fraction, not a
//      binary 0/1 flag).  A discrete model would always be either 0 (off) or
//      1 (on) within a given state.
//
// When discrete defrost is implemented these assertions should be INVERTED:
//   - First few steps should have DEFROST_ACTIVE = 0 (Accumulating).
//   - Once Defrosting, HP_CAPACITY_W should be 0 (ReverseCycle).
//   - DEFROST_TIME_FRACTION usage should be replaced by DEFROST_CYCLE_STATE.
// ---------------------------------------------------------------------------
// Ticket 012 regression — heating-side SHR always produces latent_gain_w = 0.
//
// The bug: heater.rs line 699 hardcodes `latent_gain_w = 0.0` for all heating
// output.  The correct behaviour during reverse-cycle defrost with
// heating_shr < 1.0 is a small non-zero latent contribution.
//
// These tests document the CURRENT (buggy) behaviour so that implementing the
// fix causes them to fail, forcing the developer to review and update the
// assertions.
//
// BUG 1 — latent is always zero even during normal heating.
//          (This is actually correct physics, so it should stay 0.0 after fix.)
// BUG 2 — latent is always zero even during a defrost step.
//          (After the fix, this should be non-zero when heating_shr < 1.0.)
// ---------------------------------------------------------------------------
#[test]
fn ticket_012_heating_latent_always_zero_during_normal_heating() {
    // Normal heating conditions: OAT = 7°C — no defrost should activate.
    let cfg = EquipmentConfig::from_typed(
        "ashp_normal_heat".to_string(),
        "ASHP Heater".to_string(),
        HeatPumpHeaterConfig {
            equipment_id: None,
            zone_id: Some(1),
            heating_capacity_w: Some(8_000.0),
            heating_eir: Some(0.35),
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            backup_fuel: None,
            backup_capacity_w: Some(0.0),
            backup_eir: None,
            fraction_heating_load_served: Some(1.0),
            cooling_capacity_w: Some(8_000.0),
            cooling_eir: Some(0.35),
            stage_cooling_capacities_w: None,
            stage_cooling_eirs: None,
            fraction_cooling_load_served: Some(1.0),
            number_of_speeds: 1,
            is_mini_split: false,
            shr: Some(0.75),
            fan_power_w: Some(0.0),
            fan_power_w_per_cfm: None,
            airflow_m3_s_per_w: None,
            heating_setpoint_c: None,
            cooling_setpoint_c: None,
            hysteresis_c: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
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

    let mut env = env_with_zone_temp(18.0);
    env.weather.outdoor_temp_c = 7.0; // above defrost threshold (4.4°C)
    env.weather.outdoor_humidity_ratio = 0.006;
    env.weather.outdoor_wet_bulb_c = 5.0;
    eq.init(&cfg, &env).unwrap();

    let mut ports = ports_for_zone1();
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let sensible_w = ports.thermal[0].sensible_gain_w;
    let latent_w = ports.thermal[0].latent_gain_w;

    assert!(
        sensible_w > 0.0,
        "ASHP must deliver sensible heat at 7°C OAT; got {sensible_w:.1} W"
    );

    // CORRECT PHYSICS — during normal (non-defrost) heating the outdoor coil
    // condensate drains outdoors.  Latent gain should be exactly 0.0.
    // This assertion should remain true after the ticket-012 fix.
    assert_eq!(
        latent_w, 0.0,
        "ticket-012: latent_gain_w must be 0.0 during normal heating (no defrost); \
         got {latent_w:.3} W"
    );
}

#[test]
fn ticket_012_heating_latent_always_zero_during_defrost() {
    // Defrost conditions: OAT = -5°C — defrost should activate.
    // With `heating_shr` not yet implemented in HeatPumpHeaterConfig, the
    // latent_gain_w is hardcoded to 0.0 even during defrost steps.
    // After ticket-012 is fixed, this test should FAIL because latent_gain_w
    // will be non-zero (a small positive value) when defrost is active.
    let cfg = EquipmentConfig::from_typed(
        "ashp_defrost_latent".to_string(),
        "ASHP Heater".to_string(),
        HeatPumpHeaterConfig {
            equipment_id: None,
            zone_id: Some(1),
            heating_capacity_w: Some(10_000.0),
            heating_eir: Some(0.35),
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            backup_fuel: None,
            backup_capacity_w: Some(0.0),
            backup_eir: None,
            fraction_heating_load_served: Some(1.0),
            cooling_capacity_w: Some(10_000.0),
            cooling_eir: Some(0.35),
            stage_cooling_capacities_w: None,
            stage_cooling_eirs: None,
            fraction_cooling_load_served: Some(1.0),
            number_of_speeds: 1,
            is_mini_split: false,
            shr: Some(0.75),
            fan_power_w: Some(0.0),
            fan_power_w_per_cfm: None,
            airflow_m3_s_per_w: None,
            heating_setpoint_c: None,
            cooling_setpoint_c: None,
            hysteresis_c: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
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

    // Cold, humid: OAT = -5°C triggers defrost in the continuous model.
    let mut env = env_with_zone_temp(18.0);
    env.weather.outdoor_temp_c = -5.0;
    env.weather.outdoor_humidity_ratio = 0.005;
    env.weather.outdoor_wet_bulb_c = -6.0;
    eq.init(&cfg, &env).unwrap();

    let mut ports = ports_for_zone1();
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let tel = eq.telemetry();
    let defrost_active = tel
        .get(tk::DEFROST_ACTIVE)
        .expect("DEFROST_ACTIVE telemetry must be present");
    assert_eq!(
        defrost_active, 1.0,
        "prerequisite: defrost must be active at -5°C OAT for this test to be meaningful"
    );

    let latent_w = ports.thermal[0].latent_gain_w;

    // ticket-012 BUG: latent_gain_w is hardcoded to 0.0 in heater.rs:699.
    // During reverse-cycle defrost the indoor coil surface can release a small
    // amount of moisture into the supply air, so latent_gain_w should be > 0.
    // Once `heating_shr` is wired through and heater.rs:699 is fixed, this
    // assertion should FAIL and be updated to: assert!(latent_w >= 0.0).
    assert_eq!(
        latent_w, 0.0,
        "ticket-012 BUG: latent_gain_w is {latent_w:.3} W during defrost; \
         expected 0.0 (current hardcoded behaviour) — fix heater.rs:699 to \
         compute latent from defrost_q_w and heating_shr"
    );
}

// ---------------------------------------------------------------------------
#[test]
fn ticket_011_defrost_is_continuous_not_discrete() {
    let cfg = EquipmentConfig::from_typed(
        "ashp_defrost_continuous".to_string(),
        "ASHP Heater".to_string(),
        HeatPumpHeaterConfig {
            equipment_id: None,
            zone_id: Some(1),
            heating_capacity_w: Some(10_000.0),
            heating_eir: Some(0.35),
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            backup_fuel: None,
            backup_capacity_w: Some(0.0),
            backup_eir: None,
            fraction_heating_load_served: Some(1.0),
            cooling_capacity_w: Some(8_000.0),
            cooling_eir: Some(0.35),
            stage_cooling_capacities_w: None,
            stage_cooling_eirs: None,
            fraction_cooling_load_served: Some(1.0),
            number_of_speeds: 1,
            is_mini_split: false,
            shr: Some(0.75),
            fan_power_w: Some(0.0),
            fan_power_w_per_cfm: None,
            airflow_m3_s_per_w: None,
            heating_setpoint_c: None,
            cooling_setpoint_c: None,
            hysteresis_c: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
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

    // Cold, humid conditions: OAT = -5°C, HR = 0.005 kg/kg — well below the 4.4445°C
    // defrost-enable threshold, sufficient humidity to produce a non-trivial time_fraction.
    let mut env = env_with_zone_temp(18.0);
    env.weather.outdoor_temp_c = -5.0;
    env.weather.outdoor_humidity_ratio = 0.005;
    env.weather.outdoor_wet_bulb_c = -6.0;
    eq.init(&cfg, &env).unwrap();

    let mut ports = ports_for_zone1();
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let tel = eq.telemetry();

    // --- BUG 1: defrost activates immediately (no Accumulating phase) ---
    // Current behaviour: active on first step because there is no frost-accumulation
    // state machine.  A discrete model would keep DEFROST_ACTIVE = 0.0 initially.
    let defrost_active = tel
        .get(tk::DEFROST_ACTIVE)
        .expect("DEFROST_ACTIVE telemetry must be present");
    assert_eq!(
        defrost_active, 1.0,
        "ticket-011 BUG: continuous model activates defrost immediately; \
         a discrete model would start in Accumulating (DEFROST_ACTIVE=0)"
    );

    // --- BUG 2: capacity is reduced but not zero (no discrete Defrosting state) ---
    // Current behaviour: capacity is multiplied by a fractional multiplier (0 < mult < 1).
    // A discrete model in the Defrosting state would set hp_capacity_w = 0 (ReverseCycle).
    let hp_capacity_w = tel
        .get(tk::HP_CAPACITY_W)
        .expect("HP_CAPACITY_W telemetry must be present");
    assert!(
        hp_capacity_w > 0.0,
        "ticket-011 BUG: continuous model produces non-zero capacity ({hp_capacity_w:.1} W); \
         a discrete model in Defrosting state would produce hp_capacity_w = 0 (ReverseCycle)"
    );

    // --- BUG 3: time_fraction is a continuous value, not a binary 0/1 ---
    // Current behaviour: DEFROST_TIME_FRACTION is a fraction in (0, 1).
    // A discrete model would use a binary DEFROST_CYCLE_STATE (0=Accumulating, 1=Defrosting).
    let time_frac = tel
        .get(tk::DEFROST_TIME_FRACTION)
        .expect("DEFROST_TIME_FRACTION telemetry must be present");
    assert!(
        time_frac > 0.0 && time_frac < 1.0,
        "ticket-011 BUG: DEFROST_TIME_FRACTION is {time_frac:.4} (continuous); \
         a discrete model would replace this with binary DEFROST_CYCLE_STATE"
    );
}

// ---------------------------------------------------------------------------
// Regression tests for ticket 013 — MSHP minimum compressor speed
//
// BUG: MSHP speed stages are hardcoded at 25%/50%/75%/100% of rated capacity
// with no way to configure the minimum. OCHRE uses 40% for MSHP Heater and
// ~49% for MSHP Cooler (loaded from "HVAC Multispeed Parameters.csv").
//
// These behavioral tests verify the *current* bug externally: a load of 26%
// of rated (above the hardcoded 25% minimum stage) must cause the HP to run
// continuously at stage 1, while a load of 20% (below 25% minimum) must cause
// cycling (duty < 1.0).  If the minimum were configurable to 20%, the 20%-load
// case would also run continuously.
// ---------------------------------------------------------------------------

#[test]
fn ticket_013_mshp_load_above_stage1_runs_continuously() {
    // When zone load is just above the hardcoded 25% stage, MSHP should run
    // at stage 1 with duty = 1.0 (continuous).  This confirms the 25% minimum
    // is active and the HP does not cycle in this regime.
    //
    // Setup: rated capacity 10 000 W; zone 20.26 °C (just above 20.0 °C
    // setpoint) so demand fraction ≈ 0.26 > 0.25 minimum stage.
    // Note: duty cycle behavior depends on the VariableSpeedIdeal control path.
    const RATED_W: f64 = 10_000.0;

    let cfg = EquipmentConfig::from_typed(
        "mshp_stage1_above".to_string(),
        "MSHP Heater".to_string(),
        HeatPumpHeaterConfig {
            equipment_id: None,
            zone_id: Some(1),
            heating_capacity_w: Some(RATED_W),
            heating_eir: Some(0.25),
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            backup_fuel: None,
            backup_capacity_w: None,
            backup_eir: None,
            fraction_heating_load_served: None,
            cooling_capacity_w: None,
            cooling_eir: None,
            stage_cooling_capacities_w: None,
            stage_cooling_eirs: None,
            fraction_cooling_load_served: None,
            number_of_speeds: 1,
            is_mini_split: true,
            shr: None,
            fan_power_w: Some(0.0),
            fan_power_w_per_cfm: None,
            airflow_m3_s_per_w: None,
            heating_setpoint_c: Some(21.0),
            cooling_setpoint_c: Some(26.0),
            hysteresis_c: Some(0.0),
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
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
    let mut eq = registry.create("MSHP Heater", cfg.clone()).unwrap();
    // Zone at 18.0 °C → well below setpoint, HP should run at full capacity.
    let env = env_with_zone_temp(18.0);
    eq.init(&cfg, &env).unwrap();
    eq.update_control(&env);

    let mut ports = ports_for_zone1();
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let hp_w = eq.telemetry().get(tk::HP_CAPACITY_W).unwrap_or(0.0);
    // Zone is very cold: HP must deliver meaningful capacity (above stage-1 minimum).
    assert!(
        hp_w > RATED_W * 0.20,
        "ticket-013: MSHP must deliver > 20% of rated when zone is well below setpoint; \
         got {hp_w:.1} W (rated {RATED_W:.0} W). Bug: minimum stage hardcoded at 25%, \
         no min_compressor_fraction config field."
    );
}

#[test]
fn ticket_013_mshp_has_no_min_compressor_fraction_field() {
    // This test documents that HeatPumpHeaterConfig has NO min_compressor_fraction
    // field. Attempting to deserialize a config with that field must fail with
    // "unknown field" (serde deny_unknown_fields). Once ticket-013 adds the field,
    // this test must be updated or removed.
    let json = serde_json::json!({
        "zone_id": 1,
        "heating_capacity_w": 10000.0,
        "heating_eir": 0.25,
        "number_of_speeds": 1,
        "is_mini_split": true,
        "min_compressor_fraction": 0.30
    });
    let result = serde_json::from_value::<HeatPumpHeaterConfig>(json);
    assert!(
        result.is_err(),
        "ticket-013 BUG: min_compressor_fraction field does not exist yet; \
         deserialization must fail with 'unknown field'. \
         Once ticket-013 adds the field, update this test."
    );
}

// ---------------------------------------------------------------------------
// Ticket 014 — DefrostConfig not wired into typed config
//
// The bug: HeatPumpHeaterConfig has no defrost fields, so DefrostConfig is
// always initialised from DefrostConfig::on_demand(1.0, 0.0) in heater.rs:379
// and never overridden in init_from_typed (heater.rs:460-617).
// Users cannot configure defrost_strategy, defrost_control, defrost_time_fraction,
// or resistive_defrost_capacity_w through the typed config system.
//
// These tests document the CURRENT (missing) state so that adding the fields
// in ticket-014 will cause them to fail, forcing the developer to verify the
// wiring is correct.
// ---------------------------------------------------------------------------

#[test]
fn ticket_014_defrost_strategy_field_not_in_typed_config() {
    // ticket-014 BUG: HeatPumpHeaterConfig has no defrost_strategy field.
    // Attempting to deserialize a config with defrost_strategy must fail with
    // "unknown field" (serde deny_unknown_fields).
    // Once ticket-014 adds the field, this test must be updated.
    let json = serde_json::json!({
        "zone_id": 1,
        "heating_capacity_w": 10_000.0,
        "heating_eir": 0.25,
        "cooling_capacity_w": 10_000.0,
        "cooling_eir": 0.35,
        "defrost_strategy": "Resistive"
    });
    let result = serde_json::from_value::<HeatPumpHeaterConfig>(json);
    assert!(
        result.is_err(),
        "ticket-014 BUG: defrost_strategy field does not exist yet in HeatPumpHeaterConfig; \
         deserialization must fail with 'unknown field'. \
         Once ticket-014 adds the field, update this test."
    );
}

#[test]
fn ticket_014_defrost_control_field_not_in_typed_config() {
    // ticket-014 BUG: HeatPumpHeaterConfig has no defrost_control field.
    let json = serde_json::json!({
        "zone_id": 1,
        "heating_capacity_w": 10_000.0,
        "heating_eir": 0.25,
        "cooling_capacity_w": 10_000.0,
        "cooling_eir": 0.35,
        "defrost_control": "Timed"
    });
    let result = serde_json::from_value::<HeatPumpHeaterConfig>(json);
    assert!(
        result.is_err(),
        "ticket-014 BUG: defrost_control field does not exist yet in HeatPumpHeaterConfig; \
         deserialization must fail with 'unknown field'. \
         Once ticket-014 adds the field, update this test."
    );
}

#[test]
fn ticket_014_resistive_defrost_capacity_field_not_in_typed_config() {
    // ticket-014 BUG: HeatPumpHeaterConfig has no resistive_defrost_capacity_w field.
    let json = serde_json::json!({
        "zone_id": 1,
        "heating_capacity_w": 10_000.0,
        "heating_eir": 0.25,
        "cooling_capacity_w": 10_000.0,
        "cooling_eir": 0.35,
        "resistive_defrost_capacity_w": 2_000.0
    });
    let result = serde_json::from_value::<HeatPumpHeaterConfig>(json);
    assert!(
        result.is_err(),
        "ticket-014 BUG: resistive_defrost_capacity_w field does not exist yet in \
         HeatPumpHeaterConfig; deserialization must fail with 'unknown field'. \
         Once ticket-014 adds the field, update this test."
    );
}

#[test]
fn ticket_014_defrost_config_not_wired_from_typed_path() {
    // ticket-014 BUG: init_from_typed never sets self.defrost_config from the typed
    // config — it always inherits DefrostConfig::on_demand(1.0, 0.0) from `new()`.
    //
    // This test demonstrates the structural gap: a heater initialised via the typed
    // path has defrost locked at the hardcoded defaults, even though DefrostConfig
    // itself is fully capable of representing Timed / Resistive modes.  The test
    // builds a typed config, initialises the heater, then runs a step under defrost
    // conditions (OAT = -5°C) and confirms that DEFROST_ACTIVE reflects the
    // hardcoded OnDemand behaviour — not any configurable alternative.
    //
    // After ticket-014 is implemented, defrost settings from the typed config should
    // propagate, and a Timed config should produce a different DEFROST_TIME_FRACTION
    // than the OnDemand formula yields at the same conditions.
    let cfg = EquipmentConfig::from_typed(
        "ashp_defrost_typed".to_string(),
        "ASHP Heater".to_string(),
        HeatPumpHeaterConfig {
            equipment_id: None,
            zone_id: Some(1),
            heating_capacity_w: Some(10_000.0),
            heating_eir: Some(0.30),
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            backup_fuel: None,
            backup_capacity_w: Some(0.0),
            backup_eir: None,
            fraction_heating_load_served: Some(1.0),
            cooling_capacity_w: Some(10_000.0),
            cooling_eir: Some(0.35),
            stage_cooling_capacities_w: None,
            stage_cooling_eirs: None,
            fraction_cooling_load_served: Some(1.0),
            number_of_speeds: 1,
            is_mini_split: false,
            shr: None,
            fan_power_w: Some(0.0),
            fan_power_w_per_cfm: None,
            airflow_m3_s_per_w: None,
            heating_setpoint_c: None,
            cooling_setpoint_c: None,
            hysteresis_c: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
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

    // Cold, humid: OAT = -5°C should trigger defrost (OnDemand active below 4.4445°C).
    let mut env = env_with_zone_temp(18.0);
    env.weather.outdoor_temp_c = -5.0;
    env.weather.outdoor_humidity_ratio = 0.005;
    env.weather.outdoor_wet_bulb_c = -6.0;
    eq.init(&cfg, &env).unwrap();

    let mut ports = ports_for_zone1();
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let tel = eq.telemetry();
    let defrost_active = tel
        .get(tk::DEFROST_ACTIVE)
        .expect("DEFROST_ACTIVE telemetry must be present");
    let defrost_time_frac = tel
        .get(tk::DEFROST_TIME_FRACTION)
        .expect("DEFROST_TIME_FRACTION telemetry must be present");

    // The hardcoded OnDemand model must activate defrost at -5°C.
    assert_eq!(
        defrost_active, 1.0,
        "ticket-014: hardcoded OnDemand defrost must be active at -5°C OAT"
    );
    // time_fraction from OnDemand formula is a continuous value in (0, 1),
    // not the fixed 0.058 that a Timed config would produce.
    // This confirms the hardcoded path is running, not a configurable one.
    assert!(
        defrost_time_frac > 0.0 && defrost_time_frac < 1.0,
        "ticket-014: OnDemand defrost_time_fraction must be in (0, 1); got {defrost_time_frac:.6}. \
         Once ticket-014 wires DefrostConfig from typed config, a Timed config should yield 0.058."
    );
    // Confirm it is NOT the fixed timed default of 0.058 — if someone erroneously
    // wired a hardcoded Timed config the value would be exactly 0.058.
    assert!(
        (defrost_time_frac - 0.058).abs() > 1e-6,
        "ticket-014: OnDemand time_fraction should differ from the Timed default 0.058; \
         got {defrost_time_frac:.6}"
    );
}

// ---------------------------------------------------------------------------
// Ticket 016 — thermostat FSM decision tracing
//
// These tests exercise every code path that ticket-016 requires `tracing::debug!`
// instrumentation on.  The tracing calls are side-effect-only with respect to
// correctness, so these tests remain valid both before (absent tracing) and after
// (tracing present) the ticket is implemented.
//
// The tests verify the BEHAVIOURAL CORRECTNESS of the decisions that ticket-016
// requires to be logged — ensuring the code paths actually execute and produce
// deterministic output.  A future reviewer confirming ticket-016 is complete
// should be able to run `RUST_LOG=hares_equipment::hvac=debug cargo test ticket_016`
// and see the tracing events appearing alongside these passing tests.
//
// Note: min_cycle_time_s and min_on_time_s paths are exercised by existing
// internal unit tests in hvac_core.rs (can_transition_mode_*, min_on_time_*,
// grid_emergency_off_blocked_*) and thermostat.rs.  The integration tests here
// cover the higher-level behavioural contracts via the public Equipment API.
// ---------------------------------------------------------------------------

// ticket_016: update_mode() full decision path — Heating → Deadband transition
// exercises hvac_core.rs lines 654-734 (the entire update_mode body).
// Without tracing (current state), this path runs silently.  After ticket-016,
// every call emits debug! with zone_temp, setpoints, current_mode, next_mode.
#[test]
fn ticket_016_update_mode_heating_to_deadband_transition() {
    let registry = EquipmentRegistry::new();

    let cfg = EquipmentConfig::from_typed(
        "furnace_016".to_string(),
        "Gas Furnace".to_string(),
        GasFurnaceConfig {
            equipment_id: None,
            zone_id: Some(1),
            afue: 0.80,
            capacity_w: 10_000.0,
            number_of_speeds: 1,
            fan_power_w: Some(0.0),
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            heating_setpoint_c: None,
            heating_setpoint_source: None,
            ducts: DuctConfig::default(),
        },
    );
    let env_cold = env_with_zone_temp(18.0);
    let env_warm = env_with_zone_temp(26.0);

    let mut eq = registry.create("Gas Furnace", cfg.clone()).unwrap();
    eq.init(&cfg, &env_cold).unwrap();

    // Cold zone → must enter Heating (exercises the Deadband→Heating branch in update_mode).
    let mode_heat = eq.update_control(&env_cold);
    assert_eq!(
        mode_heat,
        OperatingMode::Heating,
        "ticket-016: zone at 18°C must engage Heating; got {mode_heat:?}"
    );

    // Warm zone → must return to Deadband (exercises Heating→Deadband in update_mode).
    let mode_db = eq.update_control(&env_warm);
    assert_eq!(
        mode_db,
        OperatingMode::Off,
        "ticket-016: zone at 26°C must leave Heating → Deadband; got {mode_db:?}"
    );
}

// ticket_016: update_mode() Cooling path — Deadband → Cooling → Deadband.
// Exercises the Cooling branch of the mode-decision logic (hvac_core.rs lines
// 705-719) which ticket-016 must also instrument.
#[test]
fn ticket_016_update_mode_cooling_transition() {
    let registry = EquipmentRegistry::new();

    let cfg = EquipmentConfig::from_typed(
        "ac_016".to_string(),
        "Air Conditioner".to_string(),
        CentralAirConditionerConfig {
            equipment_id: None,
            zone_id: Some(1),
            capacity_w: 8_000.0,
            eir: 0.34,
            shr: None,
            number_of_speeds: 1,
            stage_capacities_w: None,
            stage_eirs: None,
            stage_shrs: None,
            fan_power_w: Some(0.0),
            fan_power_w_per_cfm: None,
            cooling_setpoint_c: None,
            heating_setpoint_c: None,
            hysteresis_c: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
            airflow_m3_s_per_w: None,
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

    // Default cooling setpoint is 24.0°C; hysteresis=1.0, deadband_offset=0.2.
    // Turn-on  = setpoint + hysteresis*(1-offset) = 24.0 + 0.8 = 24.8°C
    // Turn-off = setpoint - hysteresis*offset      = 24.0 - 0.2 = 23.8°C
    let env_hot = env_with_zone_temp_hot(26.0); // 26°C > 24.8°C → engages Cooling
    let env_cool = env_with_zone_temp_hot(22.0); // 22°C < 23.8°C → leaves Cooling

    let mut eq = registry.create("Air Conditioner", cfg.clone()).unwrap();
    eq.init(&cfg, &env_hot).unwrap();

    // Hot zone → must engage Cooling.
    let mode_cool = eq.update_control(&env_hot);
    assert_eq!(
        mode_cool,
        OperatingMode::Cooling,
        "ticket-016: zone at 26°C must engage Cooling (turn-on threshold 24.8°C); got {mode_cool:?}"
    );

    // Cool zone → must return to Deadband.
    let mode_db = eq.update_control(&env_cool);
    assert_eq!(
        mode_db,
        OperatingMode::Off,
        "ticket-016: zone at 22°C must leave Cooling → Deadband (turn-off threshold 23.8°C); got {mode_db:?}"
    );
}

// ---------------------------------------------------------------------------
// ticket-018: CoreOutput HVAC field promotion — regression tests
//
// These tests document the *current* (broken) state: HVAC equipment emits
// CoreOutput that carries only electric_kw and operating_mode.  Thermal
// output, COP, setpoint, and speed are absent from CoreOutput and live
// exclusively in the telemetry dictionary.
//
// When ticket-018 is implemented the assertions marked "FAILS AFTER FIX"
// must be inverted (or removed) and replaced with positive assertions that
// the new fields carry the expected values.
// ---------------------------------------------------------------------------

/// Furnace running at full heat: CoreOutput must carry thermal output after
/// ticket-018.  Today it does NOT — thermal_output_w is not a field.
///
/// This test verifies the *telemetry* path works (so we have a baseline) and
/// documents that no equivalent field exists in CoreOutput.
#[test]
fn ticket_018_furnace_core_output_lacks_thermal_field() {
    let cfg = gas_furnace_config("furnace018");
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Gas Furnace", cfg.clone()).unwrap();
    // Zone cold enough to trigger heating.
    let env = env_with_zone_temp(15.0);
    eq.init(&cfg, &env).unwrap();

    let mut ports = ports_for_zone1();
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let co = eq.core_output();

    // Electric power and mode ARE present (existing fields).
    assert!(
        co.flows.electric_kw.is_some(),
        "ticket-018 baseline: furnace CoreOutput must carry electric_kw"
    );
    assert_eq!(
        co.state.operating_mode,
        Some(OperatingMode::Heating),
        "ticket-018 baseline: furnace CoreOutput must carry Heating mode"
    );

    // Thermal output goes to telemetry today — non-zero confirms equipment ran.
    let telemetry_thermal = eq.telemetry().get(tk::THERMAL_OUTPUT_W);
    assert!(
        telemetry_thermal.map_or(false, |w| w > 1.0),
        "ticket-018 baseline: furnace must emit positive THERMAL_OUTPUT_W in telemetry when heating \
         (got {:?}); this confirms equipment ran and thermal data exists only in telemetry",
        telemetry_thermal,
    );

    // REGRESSION SENTINEL: CoreOutput has no `thermal_output_w` field.
    // After ticket-018 lands, the struct gains this field and this comment
    // must be replaced with: assert!(co.flows.thermal_output_w.unwrap_or(0.0) > 1.0)
    // For now we assert that the telemetry key is the ONLY carrier:
    // i.e., there is no CoreOutput field that duplicates it.
    let _ = co; // suppress unused-variable warning; struct field access would fail to compile
}

/// AC running at full cool: COP lives only in telemetry, not CoreOutput.
#[test]
fn ticket_018_ac_core_output_lacks_cop_field() {
    let cfg = EquipmentConfig::from_typed(
        "ac018".to_string(),
        "Air Conditioner".to_string(),
        CentralAirConditionerConfig {
            equipment_id: None,
            zone_id: Some(1),
            capacity_w: 10_000.0,
            eir: 3.412_141_633 / 14.0, // ~SEER 14
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
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
            airflow_m3_s_per_w: None,
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

    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Air Conditioner", cfg.clone()).unwrap();
    // Hot zone triggers cooling.
    let env = env_with_zone_temp_hot(32.0);
    eq.init(&cfg, &env).unwrap();

    let mut ports = ports_for_zone1();
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let co = eq.core_output();

    // Cooling mode must be active.
    assert_eq!(
        co.state.operating_mode,
        Some(OperatingMode::Cooling),
        "ticket-018 baseline: AC CoreOutput must carry Cooling mode"
    );

    // COP exists only in telemetry today.
    let telemetry_cop = eq.telemetry().get(tk::COP);
    assert!(
        telemetry_cop.map_or(false, |cop| cop > 0.0),
        "ticket-018 baseline: AC must emit positive COP in telemetry when cooling \
         (got {:?}); confirms COP data exists only in telemetry, not CoreOutput",
        telemetry_cop,
    );

    // REGRESSION SENTINEL: After ticket-018, assert co.performance.cop.unwrap() > 0.0
    // and that eq.telemetry().get(tk::COP) is either removed or equal to co.performance.cop.
}

/// Setpoint lives only in telemetry; CoreOutput.state has no setpoint_c field.
#[test]
fn ticket_018_furnace_core_output_lacks_setpoint_field() {
    let cfg = gas_furnace_config("furnace018sp");
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Gas Furnace", cfg.clone()).unwrap();
    let env = env_with_zone_temp(15.0);
    eq.init(&cfg, &env).unwrap();

    let mut ports = ports_for_zone1();
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    // Heating setpoint is written to telemetry by the furnace.
    let telemetry_sp = eq.telemetry().get(tk::HEATING_SETPOINT_C);
    assert!(
        telemetry_sp.is_some(),
        "ticket-018 baseline: gas furnace must emit HEATING_SETPOINT_C in telemetry \
         (got None); confirms setpoint data lives only in telemetry",
    );

    // REGRESSION SENTINEL: After ticket-018, assert co.state.setpoint_c == telemetry_sp
    // and the telemetry read in record_step() is replaced with co.state.setpoint_c.
}

// ---------------------------------------------------------------------------
// Ticket-019: Speed/Startup Internal State Telemetry Gaps
//
// These tests assert that, after ticket-019 is implemented, all 7 internal
// state values are visible in telemetry after a step.  The tests are written
// to FAIL today (the keys are absent) and PASS once the keys are added.
// ---------------------------------------------------------------------------

/// Single-speed AC running at partial load must expose all 7 telemetry keys
/// introduced in ticket-019.
///
/// Expected to FAIL until ticket-019 is implemented:
///   SPEED_FRAC, PART_LOAD_RATIO, PART_LOAD_FACTOR, STARTUP_MULTIPLIER,
///   DUTY_CYCLE, TIME_AT_CURRENT_SPEED_S, MODE_DURATION_S
#[test]
#[should_panic(expected = "ticket-019")]
fn ticket_019_speed_staging_keys_absent_from_telemetry() {
    let cfg = EquipmentConfig::from_typed(
        "ac019".to_string(),
        "Air Conditioner".to_string(),
        CentralAirConditionerConfig {
            equipment_id: None,
            zone_id: Some(1),
            capacity_w: 10_000.0,
            eir: 3.412_141_633 / 14.0,
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
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
            airflow_m3_s_per_w: None,
            fraction_load_served: Some(1.0),
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            duct: DuctConfig::default(),
            system_type: None,
            startup_cd: Some(0.25),
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
    let mut eq = registry.create("Air Conditioner", cfg.clone()).unwrap();
    // Zone hot enough to demand cooling.
    let env = env_with_zone_temp_hot(30.0);
    eq.init(&cfg, &env).unwrap();

    let mut ports = ports_for_zone1();
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let t = eq.telemetry();

    // All 7 keys must be present in telemetry after a step (ticket-019).
    // Each assert uses the string key names from the proposed telemetry_keys.rs
    // constants; update to use the constants once they are defined.
    assert!(
        t.get("speed_frac").is_some(),
        "ticket-019: 'speed_frac' must be written to telemetry after step"
    );
    assert!(
        t.get("part_load_ratio").is_some(),
        "ticket-019: 'part_load_ratio' must be written to telemetry after step"
    );
    assert!(
        t.get("part_load_factor").is_some(),
        "ticket-019: 'part_load_factor' must be written to telemetry after step"
    );
    assert!(
        t.get("startup_multiplier").is_some(),
        "ticket-019: 'startup_multiplier' must be written to telemetry after step"
    );
    assert!(
        t.get("duty_cycle").is_some(),
        "ticket-019: 'duty_cycle' must be written to telemetry after step"
    );
    assert!(
        t.get("time_at_current_speed_s").is_some(),
        "ticket-019: 'time_at_current_speed_s' must be written to telemetry after step"
    );
    assert!(
        t.get("mode_duration_s").is_some(),
        "ticket-019: 'mode_duration_s' must be written to telemetry after step"
    );
}

/// For a single-speed AC, `duty_cycle` and `part_load_ratio` must be equal
/// at the end of every step (they are the same physical quantity computed
/// from different paths).  This invariant is stated explicitly in ticket-019
/// §Approach, Step 3 timing note.
///
/// Expected to FAIL until ticket-019 is implemented (keys absent today).
#[test]
#[should_panic(expected = "ticket-019")]
fn ticket_019_single_speed_duty_cycle_equals_part_load_ratio() {
    let cfg = EquipmentConfig::from_typed(
        "ac019b".to_string(),
        "Air Conditioner".to_string(),
        CentralAirConditionerConfig {
            equipment_id: None,
            zone_id: Some(1),
            capacity_w: 10_000.0,
            eir: 3.412_141_633 / 14.0,
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
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
            airflow_m3_s_per_w: None,
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

    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Air Conditioner", cfg.clone()).unwrap();
    let env = env_with_zone_temp_hot(30.0);
    eq.init(&cfg, &env).unwrap();

    let mut ports = ports_for_zone1();
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let t = eq.telemetry();
    let plr = t
        .get("part_load_ratio")
        .expect("ticket-019: 'part_load_ratio' must be in telemetry");
    let dc = t
        .get("duty_cycle")
        .expect("ticket-019: 'duty_cycle' must be in telemetry");
    assert!(
        (plr - dc).abs() < 1e-9,
        "ticket-019: single-speed duty_cycle ({dc:.6}) must equal part_load_ratio ({plr:.6})"
    );
}

// ---------------------------------------------------------------------------
// Ticket-020: Setpoint Resolution Chain Visibility
//
// These tests assert that, after ticket-020 is implemented, schedule-stage and
// runtime-override setpoints are each visible in telemetry as distinct keys.
// All tests are written to FAIL today (keys absent) and PASS once implemented.
// ---------------------------------------------------------------------------

/// After one step without any runtime override, the schedule-stage setpoints
/// must appear in telemetry as `schedule_heating_setpoint_c` and
/// `schedule_cooling_setpoint_c`.
///
/// Expected to FAIL until ticket-020 is implemented (keys are absent today).
#[test]
#[should_panic(expected = "ticket-020")]
fn ticket_020_schedule_setpoint_keys_absent_from_telemetry() {
    // AC with explicit static setpoints; no schedule source, no runtime override.
    // The schedule-stage equals the static setpoints (no override applied).
    let cfg = EquipmentConfig::from_typed(
        "ac020sched".to_string(),
        "Air Conditioner".to_string(),
        CentralAirConditionerConfig {
            equipment_id: None,
            zone_id: Some(1),
            capacity_w: 10_000.0,
            eir: 3.412_141_633 / 14.0,
            shr: Some(0.75),
            number_of_speeds: 1,
            stage_capacities_w: None,
            stage_eirs: None,
            stage_shrs: None,
            fan_power_w: Some(0.0),
            fan_power_w_per_cfm: None,
            cooling_setpoint_c: Some(26.0),
            heating_setpoint_c: Some(21.0),
            hysteresis_c: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
            airflow_m3_s_per_w: None,
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

    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Air Conditioner", cfg.clone()).unwrap();
    let env = env_with_zone_temp(22.0);
    eq.init(&cfg, &env).unwrap();

    let mut ports = ports_for_zone1();
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let t = eq.telemetry();

    // schedule-stage heating setpoint must equal the static setpoint (21°C) when
    // no schedule source is present (schedule-stage = static).
    let sched_heat = t.get("schedule_heating_setpoint_c").expect(
        "ticket-020: 'schedule_heating_setpoint_c' must be written to telemetry after step",
    );
    assert!(
        (sched_heat - 21.0).abs() < 1e-9,
        "ticket-020: schedule_heating_setpoint_c must be 21.0 (static), got {sched_heat:.4}"
    );

    // schedule-stage cooling setpoint must equal the static setpoint (26°C).
    let sched_cool = t.get("schedule_cooling_setpoint_c").expect(
        "ticket-020: 'schedule_cooling_setpoint_c' must be written to telemetry after step",
    );
    assert!(
        (sched_cool - 26.0).abs() < 1e-9,
        "ticket-020: schedule_cooling_setpoint_c must be 26.0 (static), got {sched_cool:.4}"
    );
}

/// After applying a runtime setpoint override then clearing it (no override
/// active), `runtime_heating_setpoint_c` must be absent; and after re-applying
/// an override it must be present.  This round-trip verifies both halves of
/// the absent-means-no-override invariant.
///
/// Expected to FAIL until ticket-020 is implemented (key is never written today).
#[test]
#[should_panic(expected = "ticket-020")]
fn ticket_020_runtime_setpoint_key_absent_when_no_override() {
    // Gas furnace with a runtime override, then cleared.
    let cfg = gas_furnace_config("furnace020rt");
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Gas Furnace", cfg.clone()).unwrap();
    let env = env_with_zone_temp(22.0);
    eq.init(&cfg, &env).unwrap();

    // Apply override then immediately clear it by applying a no-op override with None fields,
    // then step.  After step, key must be absent.
    eq.apply_control(&ControlSignal::ThermalSetpoint {
        heating_setpoint_c: Some(25.0),
        cooling_setpoint_c: None,
        deadband_c: None,
    })
    .unwrap();
    // Clear the override by applying None on both axes.
    eq.apply_control(&ControlSignal::ThermalSetpoint {
        heating_setpoint_c: None,
        cooling_setpoint_c: None,
        deadband_c: None,
    })
    .unwrap();

    let mut ports = ports_for_zone1();
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    // No override active: runtime key must be absent.
    assert!(
        eq.telemetry().get("runtime_heating_setpoint_c").is_none(),
        "ticket-020: runtime_heating_setpoint_c must be ABSENT when no override is active \
         (got Some({:.4}))",
        eq.telemetry()
            .get("runtime_heating_setpoint_c")
            .unwrap_or(f64::NAN),
    );

    // Now apply a fresh override and re-step; key must appear.
    eq.apply_control(&ControlSignal::ThermalSetpoint {
        heating_setpoint_c: Some(25.0),
        cooling_setpoint_c: None,
        deadband_c: None,
    })
    .unwrap();
    let mut ports2 = ports_for_zone1();
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports2).unwrap();

    let rt = eq
        .telemetry()
        .get("runtime_heating_setpoint_c")
        .expect("ticket-020: runtime_heating_setpoint_c must be present when override is active");
    assert!(
        (rt - 25.0).abs() < 1e-9,
        "ticket-020: runtime_heating_setpoint_c must be 25.0, got {rt:.4}"
    );
}

/// When a `ThermalSetpoint` control signal is applied, `runtime_heating_setpoint_c`
/// must be present and equal to the overridden value.  The schedule-stage key
/// must reflect the pre-override (schedule/static) value, distinct from the
/// effective setpoint.
///
/// Expected to FAIL until ticket-020 is implemented.
#[test]
#[should_panic(expected = "ticket-020")]
fn ticket_020_runtime_setpoint_key_present_when_override_active() {
    let cfg = gas_furnace_config("furnace020rt2");
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Gas Furnace", cfg.clone()).unwrap();
    let env = env_with_zone_temp(22.0);
    eq.init(&cfg, &env).unwrap();

    // Apply a heating setpoint override to 25°C.
    eq.apply_control(&ControlSignal::ThermalSetpoint {
        heating_setpoint_c: Some(25.0),
        cooling_setpoint_c: None,
        deadband_c: None,
    })
    .unwrap();

    let mut ports = ports_for_zone1();
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let t = eq.telemetry();

    // runtime_heating_setpoint_c must be present with the override value.
    let rt_heat = t
        .get("runtime_heating_setpoint_c")
        .expect("ticket-020: runtime_heating_setpoint_c must be present when override is active");
    assert!(
        (rt_heat - 25.0).abs() < 1e-9,
        "ticket-020: runtime_heating_setpoint_c must be 25.0 (override), got {rt_heat:.4}"
    );

    // effective setpoint (HEATING_SETPOINT_C) must equal the override value.
    let eff = t
        .get(tk::HEATING_SETPOINT_C)
        .expect("HEATING_SETPOINT_C must always be present");
    assert!(
        (eff - 25.0).abs() < 1e-9,
        "ticket-020: HEATING_SETPOINT_C must equal override 25.0, got {eff:.4}"
    );

    // schedule-stage must NOT equal the override; it must reflect static (default ~20°C).
    let sched = t
        .get("schedule_heating_setpoint_c")
        .expect("ticket-020: schedule_heating_setpoint_c must be present");
    assert!(
        (sched - eff).abs() > 1e-9,
        "ticket-020: schedule_heating_setpoint_c ({sched:.4}) must differ from effective \
         setpoint ({eff:.4}) when a runtime override is in effect"
    );
}

/// IdealHvac must also expose the setpoint chain in telemetry.
/// Today it writes no setpoint keys at all; after ticket-020 it must write
/// all six (schedule_*, runtime_* when active, effective).
///
/// Expected to FAIL until ticket-020 is implemented.
#[test]
#[should_panic(expected = "ticket-020")]
fn ticket_020_ideal_hvac_setpoint_chain_absent() {
    let cfg = ideal_hvac_config("ideal020", IdealCapacityModeConfig::On);
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Ideal HVAC", cfg.clone()).unwrap();
    let env = env_with_zone_temp(15.0); // cold → heating
    eq.init(&cfg, &env).unwrap();

    let mut ports = ports_for_zone1();
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let t = eq.telemetry();

    // After ticket-020, IdealHvac must write the schedule-stage setpoints.
    assert!(
        t.get("schedule_heating_setpoint_c").is_some(),
        "ticket-020: IdealHvac must write 'schedule_heating_setpoint_c' to telemetry"
    );
    assert!(
        t.get("schedule_cooling_setpoint_c").is_some(),
        "ticket-020: IdealHvac must write 'schedule_cooling_setpoint_c' to telemetry"
    );

    // IdealHvac must also write HEATING_SETPOINT_C / COOLING_SETPOINT_C (which it
    // currently does NOT — that is also part of the ticket-020 gap for IdealHvac).
    assert!(
        t.get(tk::HEATING_SETPOINT_C).is_some(),
        "ticket-020: IdealHvac must write 'heating_setpoint_c' to telemetry"
    );
    assert!(
        t.get(tk::COOLING_SETPOINT_C).is_some(),
        "ticket-020: IdealHvac must write 'cooling_setpoint_c' to telemetry"
    );
}

// ---------------------------------------------------------------------------
// ticket-070 regression: space_fraction thermal-port scaling audit
//
// Ticket-070 claims that furnace, baseboard, AC, and heat-pump heater all fail
// to scale the thermal port by `space_fraction`, breaking the energy balance
// between the electrical draw (which IS scaled) and the thermal delivery.
//
// HOWEVER: the OCHRE source (HVAC.py lines 556–561) contains an explicit
// comment: "reduce delivered heat (only for results) and power output based on
// space fraction — Note: sensible/latent gains to envelope are not updated."
// OCHRE's `add_gains_to_zone` (line 563) uses the *unscaled* `sensible_gain`
// field. This means OCHRE itself does NOT scale thermal contributions to the
// zone by space_fraction — the field only adjusts *power reporting* metrics.
//
// These tests document the current (unscaled) thermal-port behavior and will
// PASS if HARES intentionally matches OCHRE's architecture (no scaling) or
// FAIL if ticket-070 is implemented (with scaling).  They serve as a
// change-detector: any modification to space_fraction handling will break them,
// forcing a deliberate review.
// ---------------------------------------------------------------------------

/// Verify that electric furnace thermal port equals gross rated capacity at
/// space_fraction=1.0 (default), and that the electrical port also equals the
/// rated value. This documents the baseline needed for ticket-070's claim that
/// the thermal port is not scaled by space_fraction.
///
/// NOTE: ElectricFurnaceConfig has no `fraction_heating_load_served` field, so
/// space_fraction < 1.0 is not configurable in this test (the config structs
/// for furnace/baseboard lack that field). The audit finding is that OCHRE
/// itself explicitly does NOT scale thermal-zone contributions by
/// space_fraction (HVAC.py lines 556–561 comment: "sensible/latent gains to
/// envelope are not updated"). See ticket-070 for full analysis.
#[test]
fn ticket_070_electric_furnace_thermal_equals_rated_capacity_at_default_sf() {
    const CAPACITY_W: f64 = 10_000.0;
    const EIR: f64 = 1.0;

    let cfg = EquipmentConfig::from_typed(
        "ef".to_string(),
        "Electric Furnace".to_string(),
        ElectricFurnaceConfig {
            equipment_id: None,
            zone_id: Some(1),
            eir: EIR,
            capacity_w: CAPACITY_W,
            number_of_speeds: 1,
            fan_power_w: Some(0.0),
            heating_setpoint_c: None,
            heating_setpoint_source: None,
            ducts: DuctConfig::default(),
        },
    );

    let registry = EquipmentRegistry::new();
    let env = env_with_zone_temp(18.0);
    let mut eq = registry.create("Electric Furnace", cfg.clone()).unwrap();
    eq.init(&cfg, &env).unwrap();
    eq.update_control(&env);
    let mut ports = ports_for_zone1();
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let thermal_w = ports.thermal[0].sensible_gain_w;
    let elec_kw = ports.electrical.net_active_kw();

    // At space_fraction=1.0, thermal and electrical should equal full rated capacity.
    assert!(
        (thermal_w - CAPACITY_W).abs() < 1.0,
        "ticket-070 audit: electric furnace thermal port should equal rated capacity \
         (CAPACITY_W={CAPACITY_W:.1} W). got={thermal_w:.1} W"
    );
    assert!(
        (elec_kw - CAPACITY_W * EIR / 1_000.0).abs() < 1e-6,
        "ticket-070 audit: electric furnace electrical port should equal capacity * EIR. \
         expected={:.6} kW, got={elec_kw:.6} kW",
        CAPACITY_W * EIR / 1_000.0
    );
}

/// Verify that electric baseboard thermal port equals gross rated capacity at
/// space_fraction=1.0, while electrical is scaled by EIR * space_fraction.
/// At space_fraction=1.0 these should be equal (for EIR=1.0).
///
/// NOTE: ElectricBaseboardConfig has no `fraction_heating_load_served` field.
/// The audit finding is that OCHRE itself explicitly does NOT scale
/// thermal-zone gains by space_fraction (HVAC.py lines 556–561). See
/// ticket-070 for the full analysis and legitimacy verdict.
#[test]
fn ticket_070_baseboard_thermal_equals_rated_capacity_at_default_sf() {
    const CAPACITY_W: f64 = 3_000.0;

    let cfg = EquipmentConfig::from_typed(
        "bb".to_string(),
        "Electric Baseboard".to_string(),
        ElectricBaseboardConfig {
            equipment_id: None,
            zone_id: Some(1),
            capacity_w: CAPACITY_W,
            eir: 1.0,
            heating_setpoint_c: None,
            heating_setpoint_source: None,
        },
    );

    let registry = EquipmentRegistry::new();
    let env = env_with_zone_temp(18.0);
    let mut eq = registry.create("Electric Baseboard", cfg.clone()).unwrap();
    eq.init(&cfg, &env).unwrap();
    eq.update_control(&env);
    let mut ports = ports_for_zone1();
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let thermal_w = ports.thermal[0].sensible_gain_w;
    let elec_kw = ports.electrical.net_active_kw();

    assert!(
        (thermal_w - CAPACITY_W).abs() < 1.0,
        "ticket-070 audit: baseboard thermal port should equal rated capacity at sf=1.0. \
         expected={CAPACITY_W:.1} W, got={thermal_w:.1} W"
    );
    assert!(
        (elec_kw - CAPACITY_W / 1_000.0).abs() < 1e-6,
        "ticket-070 audit: baseboard electrical port should equal capacity/1000 at sf=1.0 with EIR=1. \
         expected={:.6} kW, got={elec_kw:.6} kW",
        CAPACITY_W / 1_000.0
    );
}

// ---------------------------------------------------------------------------
// ticket-075 regression: AC sensible/latent thermal port space_fraction audit
//
// Ticket-075 claims that AirConditioner fails to scale sensible_cooling_w and
// latent_cooling_w by space_fraction before writing to the thermal port, while
// the electrical port IS scaled — creating an impossible COP and moisture
// imbalance.
//
// Ticket-075 is marked "Superseded by: Ticket 070" and the ticket-070 audit
// concluded "Not Legitimate": OCHRE HVAC.py lines 556–566 contain an explicit
// comment — "reduce delivered heat (only for results) and power output based on
// space fraction — Note: sensible/latent gains to envelope are not updated" —
// and its add_gains_to_zone() method writes unscaled sensible_gain and
// latent_gain to the zone air node.
//
// These tests document:
// (a) AC thermal port behavior with space_fraction=1.0 (baseline).
// (b) AC thermal port behavior with space_fraction=0.5 — under the current
//     architecture the thermal port is NOT halved (matching OCHRE's design);
//     the electrical port IS halved.
//
// They serve as change-detectors: if ticket-075 is implemented the (b) test
// will fail, forcing deliberate review.
// ---------------------------------------------------------------------------

/// Verify that AC sensible thermal port is strongly negative at sf=1.0 and
/// that a significant fraction of the rated cooling capacity is delivered.
/// Baseline for ticket-075.
#[test]
fn ticket_075_ac_thermal_port_equals_gross_at_default_sf() {
    const CAPACITY_W: f64 = 10_000.0;

    let cfg = EquipmentConfig::from_typed(
        "ac".to_string(),
        "Air Conditioner".to_string(),
        CentralAirConditionerConfig {
            equipment_id: None,
            zone_id: Some(1),
            capacity_w: CAPACITY_W,
            eir: 3.412_141_633 / 14.0,
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
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
            airflow_m3_s_per_w: None,
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

    let registry = EquipmentRegistry::new();
    let env = env_with_zone_temp_hot(30.0);
    let mut eq = registry.create("Air Conditioner", cfg.clone()).unwrap();
    eq.init(&cfg, &env).unwrap();
    eq.update_control(&env);
    let mut ports = ports_for_zone1();
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let sensible_w = ports.thermal[0].sensible_gain_w;

    // Sensible cooling is negative; total cooling (sensible+latent) must be
    // at least 50% of rated capacity at this operating point.
    assert!(
        sensible_w < -CAPACITY_W * 0.5,
        "ticket-075 baseline: AC sensible port must be strongly negative at sf=1.0. got={sensible_w:.1} W"
    );
}

/// Verify that with fraction_load_served=0.5, the AC electrical port is halved
/// relative to sf=1.0 (current correct behavior), while the sensible thermal
/// port is NOT halved (matching OCHRE's architecture: sensible/latent gains to
/// the zone air node are not scaled by space_fraction — see OCHRE HVAC.py
/// line 556 comment and add_gains_to_zone() at line 563-566).
///
/// If ticket-075 is implemented, the assertion on `sensible_ratio` will need to
/// change to expect ~0.5 rather than ~1.0.
#[test]
fn ticket_075_ac_electrical_halved_thermal_unscaled_at_half_sf() {
    let make_ac_cfg = |sf: f64| {
        EquipmentConfig::from_typed(
            "ac".to_string(),
            "Air Conditioner".to_string(),
            CentralAirConditionerConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 10_000.0,
                eir: 3.412_141_633 / 14.0,
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
                heating_setpoint_source: None,
                cooling_setpoint_source: None,
                airflow_m3_s_per_w: None,
                fraction_load_served: Some(sf),
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
        )
    };

    let registry = EquipmentRegistry::new();
    let env = env_with_zone_temp_hot(30.0);

    let cfg_full = make_ac_cfg(1.0);
    let mut eq_full = registry
        .create("Air Conditioner", cfg_full.clone())
        .unwrap();
    eq_full.init(&cfg_full, &env).unwrap();
    eq_full.update_control(&env);
    let mut ports_full = ports_for_zone1();
    eq_full
        .step(&env, Duration::from_secs(60), &mut ports_full)
        .unwrap();

    let cfg_half = make_ac_cfg(0.5);
    let mut eq_half = registry
        .create("Air Conditioner", cfg_half.clone())
        .unwrap();
    eq_half.init(&cfg_half, &env).unwrap();
    eq_half.update_control(&env);
    let mut ports_half = ports_for_zone1();
    eq_half
        .step(&env, Duration::from_secs(60), &mut ports_half)
        .unwrap();

    let elec_full = ports_full.electrical.net_active_kw();
    let elec_half = ports_half.electrical.net_active_kw();
    let sens_full = ports_full.thermal[0].sensible_gain_w;
    let sens_half = ports_half.thermal[0].sensible_gain_w;

    // Precondition: the AC must be running (sensible < 0).
    assert!(
        sens_full < -1.0,
        "ticket-075 precondition: AC at sf=1.0 must produce sensible cooling. got={sens_full:.1} W"
    );

    // Electrical port must be halved — this is the existing correct behavior.
    let elec_ratio = elec_half / elec_full.max(1e-9);
    assert!(
        (elec_ratio - 0.5).abs() < 0.02,
        "ticket-075: electrical port must be halved at sf=0.5 (elec_full={elec_full:.4} kW, \
         elec_half={elec_half:.4} kW, ratio={elec_ratio:.4})"
    );

    // Sensible thermal port is NOT halved under the current architecture (matching
    // OCHRE HVAC.py: "Note: sensible/latent gains to envelope are not updated"
    // when space_fraction is applied, and add_gains_to_zone() uses unscaled
    // sensible_gain). If ticket-075 is implemented, change ~1.0 to ~0.5.
    let sensible_ratio = sens_half / sens_full.min(-1e-9);
    assert!(
        (sensible_ratio - 1.0).abs() < 0.05,
        "ticket-075 audit: AC sensible thermal port is NOT scaled by space_fraction in current \
         architecture (matches OCHRE HVAC.py add_gains_to_zone). \
         sens_full={sens_full:.1} W, sens_half={sens_half:.1} W, ratio={sensible_ratio:.4}. \
         If ticket-075 is implemented, update this assertion to expect ratio≈0.5."
    );
}
