use chrono::{FixedOffset, TimeZone};
use hares_equipment::hvac::cooling_config::CentralAirConditionerConfig;
use hares_equipment::water_heater::wh_config::TanklessWaterHeaterConfig;
use hares_equipment::{DuctConfig, EquipmentConfig, EquipmentRegistry, HvacSetpointConfig};
use hares_types::{
    BoundaryPolicy, EnvironmentState, FuelType, GridState, HumidityAccumulator, PortSlots,
    ScheduleSourceConfig, ThermalAccumulator, WeatherState, ZoneId, ZoneState,
};

fn sample_env(zone_temp_c: f64, outdoor_temp_c: f64) -> EnvironmentState {
    EnvironmentState {
        zones: vec![
            ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            },
            ZoneState {
                id: ZoneId(2),
                temperature_c: outdoor_temp_c - 2.0,
                humidity_ratio: 0.007,
                volume_m3: 80.0,
            },
        ],
        weather: WeatherState {
            outdoor_temp_c,
            outdoor_humidity_ratio: 0.005,
            outdoor_wet_bulb_c: outdoor_temp_c - 4.0,
            outdoor_enthalpy_j_kg: 22_800.0,
            wind_speed_m_s: 2.0,
            wind_dir_deg: 0.0,
            ground_temp_c: 12.0,
            sky_temp_c: outdoor_temp_c - 3.0,
            pressure_kpa: 101.325,
            solar_irradiance: vec![],
            ghi_w_m2: 0.0,
            dni_w_m2: 0.0,
            dhi_w_m2: 0.0,
            solar_altitude_deg: 0.0,
            solar_azimuth_deg: 180.0,
            mains_temp_c: 12.0,
            rainfall_m: 0.0,
            ground_albedo: 0.2,
            ground_t_mean_c: 10.0,
            ground_t_amplitude_c: 0.0,
            ground_phase_day: 35.0,
            day_of_year: 1.0,
        },
        grid: GridState {
            voltage_pu: 1.0,
            frequency_hz: 60.0,
            island_bus_voltage_pu: None,
        },
        custom_domains: vec![],
        equipment_telemetry: std::collections::HashMap::new(),
        equipment_core: std::collections::HashMap::new(),
        current_time: FixedOffset::east_opt(0)
            .expect("UTC offset")
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .expect("valid timestamp"),
        time_res: chrono::Duration::minutes(1),
        price_signal: Default::default(),
        electrical: Default::default(),
    }
}

#[test]
fn tankless_schedule_sources_round_trip_and_init() {
    let cfg = TanklessWaterHeaterConfig {
        equipment_id: Some(13),
        zone_id: Some(1),
        loop_id: Some(1),
        fuel_type: FuelType::Gas,
        energy_factor: Some(0.82),
        uniform_energy_factor: Some(0.84),
        heating_capacity_w: Some(20_000.0),
        setpoint_c: Some(51.67),
        parasitic_power_w: Some(5.0),
        performance_adjustment: Some(0.92),
        inlet_temp_c: Some(12.0),
        draw_flow_rate_kg_s: Some(0.05),
        draw_flow_rate_source: Some(ScheduleSourceConfig::ColumnRef {
            col_idx: 3,
            boundary: BoundaryPolicy::Clamp,
        }),
        mains_temp_c_source: Some(ScheduleSourceConfig::ColumnRef {
            col_idx: 7,
            boundary: BoundaryPolicy::Clamp,
        }),
        avg_water_draw_l_per_day: Some(220.0),
        zone_type: None,
        min_flow_kg_s: None,
        min_flow_gpm: None,
    };

    let serialized = serde_json::to_value(&cfg).expect("serialize tankless config");
    let round_trip: TanklessWaterHeaterConfig =
        serde_json::from_value(serialized).expect("deserialize tankless config");
    assert_eq!(
        round_trip.draw_flow_rate_source, cfg.draw_flow_rate_source,
        "draw_flow_rate_source must survive typed round-trip"
    );
    assert_eq!(
        round_trip.mains_temp_c_source, cfg.mains_temp_c_source,
        "mains_temp_c_source must survive typed round-trip"
    );

    let registry = EquipmentRegistry::new();
    let ec = EquipmentConfig::from_typed(
        "twh".to_string(),
        "Gas Tankless Water Heater".to_string(),
        round_trip,
    )
    .unwrap();
    let env = sample_env(21.0, 10.0);
    let mut eq = registry
        .create("Gas Tankless Water Heater", ec.clone())
        .expect("tankless water heater should create");
    eq.init(&ec, &env)
        .expect("tankless water heater should init");
}

#[test]
fn central_ac_airflow_and_duct_airflow_round_trip_without_collision() {
    let cfg = CentralAirConditionerConfig {
        equipment_id: Some(7),
        zone_id: Some(1),
        capacity_w: 12_000.0,
        eir: 3.412_141_633 / 16.0,
        shr: Some(0.74),
        number_of_speeds: 2,
        stage_capacities_w: Some(vec![6_000.0, 12_000.0]),
        stage_eirs: Some(vec![0.28, 0.24]),
        stage_shrs: Some(vec![0.78, 0.74]),
        fan_power_w: Some(320.0),
        fan_power_w_per_cfm: None,
        setpoint: HvacSetpointConfig {
            cooling_setpoint_c: Some(26.0),
            heating_setpoint_c: Some(18.0),
            ..Default::default()
        },
        hysteresis_c: Some(1.0),
        airflow_m3_s_per_w: Some(5.3678384759785085e-5),
        fraction_load_served: Some(1.0),
        duct: DuctConfig {
            dse_heat: Some(0.92),
            dse_cool: Some(0.89),
            airflow_m3_s_per_w: Some(4.0e-5),
            duct_zone_id: Some(2),
            duct_house_volume_m3: Some(425.0),
            duct_supply_leakage_frac: Some(0.08),
            duct_supply_area_m2: Some(12.0),
            duct_supply_r_m2_k_w: Some(2.5),
            duct_return_leakage_frac: Some(0.05),
            duct_return_area_m2: Some(9.0),
            duct_return_r_m2_k_w: Some(1.8),
            duct_zone_type: Some("attic_vented".to_string()),
        },
        system_type: Some("split".to_string()),
        startup_cd: Some(0.15),
        crankcase_heater_kw: None,
        crankcase_heater_threshold_c: None,
        crankcase_capacity_curve_coeffs: None,
        biquadratic_x1_min: Some(12.0),
        biquadratic_x1_max: Some(24.0),
        biquadratic_x2_min: Some(18.0),
        biquadratic_x2_max: Some(46.0),
        ff_min: Some(0.6),
        ff_max: Some(1.2),
        plf_min: Some(0.7),
        plf_max: Some(1.0),
        charge_defect_ratio: None,
        min_oat_compressor_cooling_c: None,
    };

    let serialized = serde_json::to_value(&cfg).expect("serialize central ac config");
    let round_trip: CentralAirConditionerConfig =
        serde_json::from_value(serialized).expect("deserialize central ac config");
    assert_eq!(
        round_trip.airflow_m3_s_per_w, cfg.airflow_m3_s_per_w,
        "coil airflow must survive typed round-trip"
    );
    assert_eq!(
        round_trip.duct.airflow_m3_s_per_w, cfg.duct.airflow_m3_s_per_w,
        "duct airflow must survive typed round-trip"
    );

    let registry = EquipmentRegistry::new();
    let ec =
        EquipmentConfig::from_typed("ac".to_string(), "Air Conditioner".to_string(), round_trip)
            .unwrap();
    let env = sample_env(27.0, 35.0);
    let mut eq = registry
        .create("Air Conditioner", ec.clone())
        .expect("air conditioner should create");
    eq.init(&ec, &env).expect("air conditioner should init");

    let mut ports = PortSlots {
        thermal: vec![
            ThermalAccumulator::new(ZoneId(1)),
            ThermalAccumulator::new(ZoneId(2)),
        ],
        humidity: vec![
            HumidityAccumulator::new(ZoneId(1)),
            HumidityAccumulator::new(ZoneId(2)),
        ],
        ..PortSlots::default()
    };
    let mode = eq.update_control(&env);
    eq.step(&env, std::time::Duration::from_secs(60), &mut ports)
        .expect("air conditioner should step");

    assert_eq!(
        mode,
        hares_types::OperatingMode::Cooling,
        "a hot zone above the cooling setpoint must call for cooling"
    );
    assert!(
        ports.thermal[0].sensible_gain_w < 0.0,
        "cooling must remove sensible heat from the zone"
    );
    assert!(
        ports.electrical.net_active_w() > 0.0,
        "cooling must draw positive electric power"
    );
}
