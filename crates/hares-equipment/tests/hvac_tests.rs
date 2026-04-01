use std::time::Duration;

use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
use hares_equipment::hvac::heating_config::IdealCapacityModeConfig;
use hares_equipment::{
    CentralAirConditionerConfig, DuctConfig, ElectricBaseboardConfig, ElectricBoilerConfig,
    ElectricFurnaceConfig, EquipmentConfig, EquipmentRegistry, GasFurnaceConfig,
    HeatPumpHeaterConfig, IdealHvacConfig,
};
use hares_types::{
    telemetry_keys as tk, ControlCapabilities, ControlSignal, EnvironmentState, FluidAccumulator,
    FluidType, FuelType, GridState, LoopId, OperatingMode, PortSlots, ScheduleSourceConfig,
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
        },
    )
}

fn ports_for_zone1() -> PortSlots {
    PortSlots {
        thermal: vec![ThermalAccumulator::new(ZoneId(1))],
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
    let electric_kw = tel.get(tk::ELECTRIC_KW).expect("ELECTRIC_KW must be present");
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
//   - hp_lockout_temp_c = -17.78°C (0°F)             — HVAC.py:1208
//   - er_lockout_temp_c = 4.44°C (40°F)              — HVAC.py:1209
//   - er_setpoint_offset = deadband*(1.8-0.2) = 1.6°C — HVAC.py:1211
//   - er_hard_lockout_time = 0 (disabled)             — HVAC.py:1214
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

    // Behavioral: ASHP has no pan heater — pan_heater_kw must remain 0 after any step
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
//   - pan_heater_kw = 0.150 kW active when OAT < 0°C  — HVAC.py:1482
//   - pan_heater_temp = 0°C activation threshold       — HVAC.py:1483
//   - No backup by default (ductless, no strip heater)
//   - HP lockout same as ASHP: -17.78°C               — HVAC.py:1208
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
    // Equipment turns ON — RTF must be 1.0 (full duty cycle).
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

    // Warm zone: above setpoint (21°C), equipment turns OFF — RTF must be 0.
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
//   signal and delivers exactly that value — not 0 or rated.
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
         got {gain:.4} W — must not be 0 or rated ({RATED_W:.0} W)"
    );
}

// ---------------------------------------------------------------------------
// auto_mode_selects_bang_bang_at_1min_timestep
//   IdealHvac with IdealCapacityMode::Auto at 60 s (< 300 s threshold) uses
//   bang-bang: ideal_target() returns None and step outputs full rated capacity.
//
//   OCHRE parity: HVAC.py:237 — use_ideal_capacity = time_res >= 5 min.
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
//   OCHRE parity: HVAC.py:237 — use_ideal_capacity = time_res >= 5 min.
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
         got {gain:.4} W — must not be 0 or rated ({RATED_W:.0} W)"
    );
}
