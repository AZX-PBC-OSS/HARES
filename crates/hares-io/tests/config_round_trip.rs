use std::collections::HashMap;

use chrono::{FixedOffset, TimeZone};
use hares_equipment::hvac::cooling_config::{
    CentralAirConditionerConfig, DehumidifierConfig, RoomAcConfig,
};
use hares_equipment::hvac::heat_pump_config::{HeatPumpCommonConfig, HeatPumpConfig};
use hares_equipment::hvac::heating_config::{
    ElectricBaseboardConfig, ElectricBoilerConfig, ElectricFurnaceConfig, GasBoilerConfig,
    GasFurnaceConfig, IdealHvacConfig,
};
use hares_equipment::water_heater::wh_config::{
    ElectricResistanceWaterHeaterConfig, GasWaterHeaterConfig, HeatPumpWaterHeaterConfig,
    TanklessWaterHeaterConfig,
};
use hares_equipment::{
    BatteryConfig, DefrostConfig, DuctConfig, EquipmentConfig, EquipmentRegistry,
    EquipmentTypedConfig, EvConfig, GeneratorConfig, HvacSetpointConfig, PvConfig,
    VentilationConfig,
};
use hares_types::{
    BoundaryPolicy, EnvironmentState, FuelType, GridState, ScheduleSourceConfig, SurfaceIrradiance,
    WeatherState, ZoneId, ZoneState,
};

fn sample_duct_config() -> DuctConfig {
    DuctConfig {
        dse_heat: Some(0.92),
        dse_cool: Some(0.89),
        airflow_m3_s_per_w: Some(5.0e-5),
        duct_zone_id: Some(2),
        duct_house_volume_m3: Some(425.0),
        duct_supply_leakage_frac: Some(0.08),
        duct_supply_area_m2: Some(12.0),
        duct_supply_r_m2_k_w: Some(2.5),
        duct_return_leakage_frac: Some(0.05),
        duct_return_area_m2: Some(9.0),
        duct_return_r_m2_k_w: Some(1.8),
        duct_zone_type: Some("attic_vented".to_string()),
    }
}

fn sample_gas_furnace_config() -> GasFurnaceConfig {
    GasFurnaceConfig {
        equipment_id: Some(1),
        zone_id: Some(1),
        capacity_w: 15_000.0,
        afue: 0.92,
        fan_power_w: Some(450.0),
        number_of_speeds: 2,
        ducts: sample_duct_config(),
        stage_heating_capacities_w: None,
        stage_heating_eirs: None,
        setpoint: HvacSetpointConfig::default(),
    }
}

fn sample_electric_furnace_config() -> ElectricFurnaceConfig {
    ElectricFurnaceConfig {
        equipment_id: Some(2),
        zone_id: Some(1),
        capacity_w: 12_000.0,
        eir: 1.0,
        fan_power_w: Some(350.0),
        number_of_speeds: 1,
        ducts: sample_duct_config(),
        setpoint: HvacSetpointConfig::default(),
    }
}

fn sample_gas_boiler_config() -> GasBoilerConfig {
    GasBoilerConfig {
        equipment_id: Some(3),
        zone_id: Some(1),
        loop_id: Some(1),
        capacity_w: 18_000.0,
        afue: 0.88,
        flow_rate_kg_s: 0.6,
        return_temp_c: 45.0,
        fluid_type: hares_types::FluidType::Water,
        fan_power_w: Some(80.0),
        number_of_speeds: 1,
        setpoint: HvacSetpointConfig::default(),
        condensing: false,
    }
}

fn sample_electric_boiler_config() -> ElectricBoilerConfig {
    ElectricBoilerConfig {
        equipment_id: Some(4),
        zone_id: Some(1),
        loop_id: Some(1),
        capacity_w: 18_000.0,
        eir: 1.0,
        flow_rate_kg_s: 0.6,
        return_temp_c: 45.0,
        fluid_type: hares_types::FluidType::Water,
        fan_power_w: Some(80.0),
        number_of_speeds: 1,
        setpoint: HvacSetpointConfig::default(),
    }
}

fn sample_electric_baseboard_config() -> ElectricBaseboardConfig {
    ElectricBaseboardConfig {
        equipment_id: Some(5),
        zone_id: Some(1),
        capacity_w: 4_500.0,
        eir: 1.0,
        setpoint: HvacSetpointConfig::default(),
    }
}

fn sample_ideal_hvac_config() -> IdealHvacConfig {
    IdealHvacConfig {
        equipment_id: Some(6),
        zone_id: Some(1),
        setpoint: HvacSetpointConfig {
            heating_setpoint_c: Some(21.0),
            cooling_setpoint_c: Some(26.0),
            ..Default::default()
        },
        deadband_c: Some(1.0),
        n_speeds: Some(1),
        ideal_capacity_mode: None,
        heating_capacity_w: Some(12_000.0),
        cooling_capacity_w: Some(12_000.0),
        shr: Some(0.75),
        fraction_heating_load_served: Some(1.0),
        fraction_cooling_load_served: Some(1.0),
        rated_fan_power_w: None,
        rated_eir: Some(1.0),
        capacity_min_w: None,
        fuel_type: None,
        capacity_biquadratic_coeffs: None,
        eir_biquadratic_coeffs: None,
    }
}

fn sample_central_ac_config() -> CentralAirConditionerConfig {
    let mut duct = sample_duct_config();
    duct.airflow_m3_s_per_w = None;
    CentralAirConditionerConfig {
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
        duct,
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
    }
}

fn sample_room_ac_config() -> RoomAcConfig {
    RoomAcConfig {
        equipment_id: Some(8),
        zone_id: Some(1),
        capacity_w: 3_500.0,
        eir: 3.412_141_633 / 10.0,
        setpoint: HvacSetpointConfig {
            cooling_setpoint_c: Some(26.0),
            heating_setpoint_c: Some(18.0),
            ..Default::default()
        },
        hysteresis_c: Some(1.0),
        airflow_m3_s_per_w: Some(4.294270780782807e-5),
        biquadratic_x1_min: Some(10.0),
        biquadratic_x1_max: Some(25.0),
        biquadratic_x2_min: Some(18.0),
        biquadratic_x2_max: Some(46.0),
        ff_min: Some(0.6),
        ff_max: Some(1.2),
        plf_min: Some(0.7),
        plf_max: Some(1.0),
        shr: None,
        startup_cd: None,
        crankcase_heater_kw: None,
        crankcase_heater_threshold_c: None,
        crankcase_capacity_curve_coeffs: None,
    }
}

fn sample_heat_pump_config() -> HeatPumpConfig {
    let mut duct = sample_duct_config();
    duct.airflow_m3_s_per_w = None;
    HeatPumpConfig {
        common: HeatPumpCommonConfig {
            equipment_id: Some(9),
            zone_id: Some(1),
            heating_capacity_w: Some(12_000.0),
            heating_eir: Some(3.412_141_633 / 9.5),
            stage_heating_capacities_w: Some(vec![6_000.0, 12_000.0]),
            stage_heating_eirs: Some(vec![0.34, 0.28]),
            backup_fuel: Some(FuelType::Electric),
            backup_capacity_w: Some(5_000.0),
            backup_eir: Some(1.0),
            fraction_heating_load_served: Some(1.0),
            cooling_capacity_w: Some(12_000.0),
            cooling_eir: Some(3.412_141_633 / 16.0),
            stage_cooling_capacities_w: Some(vec![6_000.0, 12_000.0]),
            stage_cooling_eirs: Some(vec![0.28, 0.24]),
            fraction_cooling_load_served: Some(1.0),
            number_of_speeds: 2,
            is_mini_split: false,
            shr: Some(0.75),
            fan_power_w: Some(320.0),
            fan_power_w_per_cfm: None,
            airflow_m3_s_per_w: Some(5.3678384759785085e-5),
            setpoint: HvacSetpointConfig {
                heating_setpoint_c: Some(21.0),
                cooling_setpoint_c: Some(26.0),
                ..Default::default()
            },
            hysteresis_c: Some(1.0),
            duct,
            biquadratic_x1_min: Some(12.0),
            biquadratic_x1_max: Some(24.0),
            biquadratic_x2_min: Some(18.0),
            biquadratic_x2_max: Some(46.0),
            ff_min: Some(0.6),
            ff_max: Some(1.2),
            plf_min: Some(0.7),
            plf_max: Some(1.0),
            min_compressor_fraction: 0.25,
            eir_part_load_benefit: None,
            er_stages: 1,
            charge_defect_ratio: None,
            ..Default::default()
        },
        hp_lockout_temp_c: None,
        er_lockout_temp_c: None,
        max_oat_supplemental_c: None,
        er_setpoint_offset_c: None,
        er_hard_lockout_time_s: None,
        heating_shr: None,
        capacity_ratio_at_17f: None,
        defrost: DefrostConfig::default(),
    }
}

fn sample_dehumidifier_config() -> DehumidifierConfig {
    DehumidifierConfig {
        equipment_id: Some(10),
        zone_id: Some(1),
        capacity_liters_per_day: Some(25.0),
        energy_factor: Some(2.0),
        integrated_energy_factor: Some(2.4),
        fraction_served: Some(1.0),
        target_rh: Some(0.50),
    }
}

fn sample_gas_water_heater_config() -> GasWaterHeaterConfig {
    GasWaterHeaterConfig {
        equipment_id: Some(11),
        zone_id: Some(1),
        loop_id: Some(1),
        fuel_type: FuelType::Gas,
        tank_volume_m3: Some(0.19),
        tank_height_m: Some(1.4),
        energy_factor: Some(0.82),
        uniform_energy_factor: Some(0.84),
        heating_capacity_w: Some(11_000.0),
        ua_w_per_k: Some(3.0),
        setpoint_c: Some(51.67),
        deadband_c: None,
        max_tank_temp_c: None,
        initial_tank_temp_c: None,
        tank_nodes: None,
        avg_water_draw_l_per_day: Some(220.0),
        draw_flow_rate_kg_s: None,
        draw_flow_rate_source: Some(ScheduleSourceConfig::ColumnRef {
            col_idx: 7,
            boundary: BoundaryPolicy::Clamp,
        }),
        mains_temp_c_source: Some(ScheduleSourceConfig::ColumnRef {
            col_idx: 2,
            boundary: BoundaryPolicy::Clamp,
        }),
        pilot_power_w: Some(5.0),
        flue_loss_fraction: Some(0.12),
        skin_loss_fraction: None,
        ignition_type: None,
        performance_adjustment: Some(0.92),
        zone_type: Some("conditioned".to_string()),
        first_hour_rating_m3: Some(0.20),
        jacket_r_value_m2_k_w: None,
        conversion_efficiency: None,
        fixture_delivery_temp_c: None,
        hot_draw_temp_c: None,
    }
}

fn sample_electric_resistance_water_heater_config() -> ElectricResistanceWaterHeaterConfig {
    ElectricResistanceWaterHeaterConfig {
        equipment_id: Some(12),
        zone_id: Some(1),
        loop_id: Some(1),
        tank_volume_m3: Some(0.19),
        tank_height_m: Some(1.4),
        energy_factor: Some(0.92),
        uniform_energy_factor: Some(0.94),
        heating_capacity_w: Some(4_500.0),
        ua_w_per_k: Some(3.0),
        setpoint_c: Some(51.67),
        deadband_c: None,
        max_tank_temp_c: None,
        initial_tank_temp_c: None,
        tank_nodes: None,
        avg_water_draw_l_per_day: Some(220.0),
        draw_flow_rate_kg_s: None,
        draw_flow_rate_source: Some(ScheduleSourceConfig::ColumnRef {
            col_idx: 7,
            boundary: BoundaryPolicy::Clamp,
        }),
        mains_temp_c_source: Some(ScheduleSourceConfig::ColumnRef {
            col_idx: 2,
            boundary: BoundaryPolicy::Clamp,
        }),
        performance_adjustment: Some(0.95),
        zone_type: Some("conditioned".to_string()),
        first_hour_rating_m3: Some(0.20),
        element_power_w: Some(4_500.0),
        element_priority_mode: None,
        max_setpoint_ramp_rate_c_per_min: None,
        jacket_r_value_m2_k_w: None,
        fixture_delivery_temp_c: None,
        hot_draw_temp_c: None,
    }
}

fn sample_tankless_water_heater_config() -> TanklessWaterHeaterConfig {
    TanklessWaterHeaterConfig {
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
        inlet_temp_c: None,
        draw_flow_rate_kg_s: None,
        draw_flow_rate_source: Some(ScheduleSourceConfig::ColumnRef {
            col_idx: 7,
            boundary: BoundaryPolicy::Clamp,
        }),
        mains_temp_c_source: Some(ScheduleSourceConfig::ColumnRef {
            col_idx: 2,
            boundary: BoundaryPolicy::Clamp,
        }),
        avg_water_draw_l_per_day: Some(220.0),
    }
}

fn sample_heat_pump_water_heater_config() -> HeatPumpWaterHeaterConfig {
    HeatPumpWaterHeaterConfig {
        equipment_id: Some(14),
        zone_id: Some(1),
        loop_id: Some(1),
        tank_volume_m3: Some(0.24),
        tank_height_m: Some(1.5),
        cop: Some(3.5),
        backup_element_power_w: Some(4_500.0),
        ua_w_per_k: Some(2.5),
        setpoint_c: Some(51.67),
        deadband_c: None,
        max_tank_temp_c: None,
        initial_tank_temp_c: None,
        tank_nodes: None,
        tempering_valve_setpoint_c: Some(51.67),
        avg_water_draw_l_per_day: Some(220.0),
        draw_flow_rate_kg_s: None,
        compressor_power_w: None,
        backup_enable_offset_c: None,
        min_ambient_temp_c: None,
        max_ambient_temp_c: None,
        min_on_time_s: None,
        min_off_time_s: None,
        hp_only_mode: None,
        element_hp_control_mode: None,
        fan_power_w: None,
        parasitic_power_w: None,
        backup_efficiency: None,
        shr: None,
        lost_heat_fraction: None,
        wall_heat_fraction: None,
        capacity_biquadratic_coeffs: None,
        cop_biquadratic_coeffs: None,
        performance_adjustment: Some(0.92),
        zone_type: Some("conditioned".to_string()),
        first_hour_rating_m3: Some(0.20),
        jacket_r_value_m2_k_w: None,
        fixture_delivery_temp_c: None,
    }
}

fn sample_battery_config() -> BatteryConfig {
    BatteryConfig {
        equipment_id: Some(15),
        zone_id: Some(1),
        capacity_kwh: 13.5,
        max_charge_kw: 5.0,
        max_discharge_kw: 5.0,
        n_series: Some(96),
        n_parallel: Some(1),
        ah_cell: Some(50.0),
        v_cell: Some(3.7),
        cell_resistance_ohm: Some(0.005),
        pack_voltage_v: Some(350.0),
        chemistry: Some("NMC".to_string()),
        standby_power_w: Some(5.0),
        self_discharge_pct_per_day: Some(0.05),
        min_soc: Some(0.1),
        max_soc: Some(0.9),
        initial_soc: Some(0.5),
        initial_cell_temp_c: None,
        import_limit_w: Some(7_000.0),
        export_limit_w: Some(5_000.0),
        heater_power_w: Some(250.0),
        heater_threshold_c: Some(0.0),
        heater_on_discharge: Some(false),
        min_discharge_temp_c: Some(-10.0),
        full_power_temp_c: Some(20.0),
        min_charge_temp_c: Some(0.0),
        cell_thermal_mass_j_per_k: Some(20_000.0),
        cell_ua_w_per_k: Some(4.0),
        inverter_efficiency: Some(0.92),
        charge_efficiency: Some(0.95),
        discharge_efficiency: Some(0.94),
        bms_mode: None,
        grid_export_rule: None,
    }
}

fn sample_ev_config() -> EvConfig {
    EvConfig {
        equipment_id: Some(16),
        capacity_kwh: 75.0,
        charging_level: Some("L2".to_string()),
        max_charging_power_kw: 11.5,
        charging_efficiency: Some(0.9),
        l1_current_a: Some(12.0),
        l1_voltage_v: Some(120.0),
        soc_max: Some(0.9),
        initial_soc: Some(0.5),
        battery_temp_c: Some(20.0),
        min_charge_temp_c: Some(0.0),
        full_power_temp_c: Some(10.0),
        heater_power_w: Some(0.0),
        heater_threshold_c: Some(0.0),
        thermal_mass_j_per_k: Some(20_000.0),
        ua_w_per_k: Some(4.0),
        v2l_enabled: Some(false),
        v2l_soc_reserve: Some(0.2),
        v2l_max_discharge_kw: Some(3.0),
        v2g_enabled: Some(false),
        v2g_soc_reserve: Some(0.3),
        v2g_max_discharge_kw: Some(5.0),
        chemistry: Some("NMC".to_string()),
        fuel_economy_kwh_per_mi: Some(0.325),
        ready_soc: Some(0.8),
        charging_strategy: None,
        plug_in_policy: None,
        power_limit_kw: Some(11.5),
        initial_connection_state: None,
    }
}

fn sample_pv_config() -> PvConfig {
    PvConfig {
        equipment_id: Some(17),
        zone_id: Some(1),
        capacity_kw: 5.0,
        tilt_deg: Some(30.0),
        azimuth_deg: Some(180.0),
        module_type: Some("mono-si".to_string()),
        noct_c: Some(45.0),
        system_losses_fraction: Some(0.14),
        inverter_efficiency: Some(0.96),
        inverter_capacity_kw: Some(5.5),
        power_factor: Some(1.0),
        surface_resolution_deg: Some(5.0),
        sam_lut_path: None,
        arrays: None,
    }
}

fn sample_generator_config() -> GeneratorConfig {
    GeneratorConfig {
        equipment_id: Some(18),
        zone_id: Some(1),
        fuel_type: Some(FuelType::Gas),
        rated_power_kw: 10.0,
        eta_electric: Some(0.32),
        eta_thermal: Some(0.45),
        efficiency_curve_points: None,
        efficiency_type: None,
        delta_kw_per_s: Some(1.0),
        capacity_min_kw: Some(2.0),
        grid_import_limit_kw: Some(0.0),
        export_limit_kw: Some(10.0),
        loop_id: Some(1),
        flow_rate_kg_s: Some(0.5),
        supply_temp_c: Some(60.0),
        return_temp_c: Some(40.0),
        inverter_efficiency: None,
        stack_temp_c: None,
        stack_cooler_r0: None,
        stack_cooler_r1: None,
        stack_cooler_r2: None,
        stack_cooler_r3: None,
        stack_nominal_temp_c: None,
    }
}

fn sample_ventilation_config() -> VentilationConfig {
    VentilationConfig {
        equipment_id: Some(19),
        zone_id: Some(1),
        flow_rate_m3_s: 0.05,
        fan_power_w: Some(60.0),
        sensible_effectiveness: Some(0.75),
        latent_effectiveness: Some(0.20),
        bypass_temp_min_c: Some(18.0),
        bypass_temp_max_c: Some(24.0),
        defrost_temp_c: Some(-5.0),
        defrost_effectiveness_fraction: Some(0.5),
        ventilation_type: Some("hrv".to_string()),
        balanced: Some(true),
        hours_in_operation: Some(12.0),
    }
}

fn sample_env() -> EnvironmentState {
    EnvironmentState {
        zones: vec![ZoneState {
            id: ZoneId(1),
            temperature_c: 21.0,
            humidity_ratio: 0.008,
            relative_humidity: 0.45,
            wet_bulb_c: 16.0,
            volume_m3: 200.0,
        }],
        weather: WeatherState {
            outdoor_temp_c: 10.0,
            outdoor_humidity_ratio: 0.005,
            outdoor_wet_bulb_c: 6.0,
            outdoor_enthalpy_j_kg: 22_800.0,
            wind_speed_m_s: 2.0,
            wind_dir_deg: 0.0,
            ground_temp_c: 12.0,
            sky_temp_c: 8.0,
            pressure_kpa: 101.325,
            solar_irradiance: vec![SurfaceIrradiance {
                surface_id: 300_018_000,
                direct_w_m2: 0.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
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
            day_of_year: 1.0,
        },
        grid: GridState {
            voltage_pu: 1.0,
            frequency_hz: 60.0,
        },
        custom_domains: vec![],
        equipment_telemetry: HashMap::new(),
        equipment_core: HashMap::new(),
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

fn assert_round_trip<T>(cfg: T)
where
    T: EquipmentTypedConfig + PartialEq + Clone + std::fmt::Debug,
{
    let serialized = serde_json::to_value(&cfg).unwrap();
    let deserialized: T = serde_json::from_value(serialized).unwrap();
    assert_eq!(cfg, deserialized);
}

fn assert_deny_unknown<T>(cfg: T)
where
    T: EquipmentTypedConfig,
{
    let mut value = serde_json::to_value(&cfg).unwrap();
    value["nonexistent_key"] = serde_json::json!(42.0);
    assert!(serde_json::from_value::<T>(value).is_err());
}

fn smoke_case<T>(registry: &EquipmentRegistry, name: &str, cfg: T, env: &EnvironmentState)
where
    T: EquipmentTypedConfig,
{
    let ec = EquipmentConfig::from_typed(name.to_string(), name.to_string(), cfg);
    let mut equipment = registry.create(name, ec.clone()).unwrap();
    equipment.init(&ec, env).unwrap();
}

macro_rules! typed_config_tests {
    ($round_trip:ident, $deny_unknown:ident, $ty:ty, $sample_fn:ident) => {
        #[test]
        fn $round_trip() {
            assert_round_trip($sample_fn());
        }

        #[test]
        fn $deny_unknown() {
            assert_deny_unknown($sample_fn());
        }
    };
}

typed_config_tests!(
    round_trip_gas_furnace_config,
    deny_unknown_field_gas_furnace_config,
    GasFurnaceConfig,
    sample_gas_furnace_config
);
typed_config_tests!(
    round_trip_electric_furnace_config,
    deny_unknown_field_electric_furnace_config,
    ElectricFurnaceConfig,
    sample_electric_furnace_config
);
typed_config_tests!(
    round_trip_gas_boiler_config,
    deny_unknown_field_gas_boiler_config,
    GasBoilerConfig,
    sample_gas_boiler_config
);
typed_config_tests!(
    round_trip_electric_boiler_config,
    deny_unknown_field_electric_boiler_config,
    ElectricBoilerConfig,
    sample_electric_boiler_config
);
typed_config_tests!(
    round_trip_electric_baseboard_config,
    deny_unknown_field_electric_baseboard_config,
    ElectricBaseboardConfig,
    sample_electric_baseboard_config
);
typed_config_tests!(
    round_trip_ideal_hvac_config,
    deny_unknown_field_ideal_hvac_config,
    IdealHvacConfig,
    sample_ideal_hvac_config
);
typed_config_tests!(
    round_trip_central_air_conditioner_config,
    deny_unknown_field_central_air_conditioner_config,
    CentralAirConditionerConfig,
    sample_central_ac_config
);
typed_config_tests!(
    round_trip_room_ac_config,
    deny_unknown_field_room_ac_config,
    RoomAcConfig,
    sample_room_ac_config
);

#[test]
fn round_trip_heat_pump_config() {
    assert_round_trip(sample_heat_pump_config());
}

#[test]
fn heat_pump_config_ignores_unknown_fields() {
    let mut value = serde_json::to_value(sample_heat_pump_config()).unwrap();
    value["unknown_key"] = serde_json::json!(42.0);
    let result: Result<HeatPumpConfig, _> = serde_json::from_value(value);
    assert!(
        result.is_ok(),
        "serde flatten + no deny_unknown_fields: unknown keys are silently ignored"
    );
}

typed_config_tests!(
    round_trip_dehumidifier_config,
    deny_unknown_field_dehumidifier_config,
    DehumidifierConfig,
    sample_dehumidifier_config
);
typed_config_tests!(
    round_trip_gas_water_heater_config,
    deny_unknown_field_gas_water_heater_config,
    GasWaterHeaterConfig,
    sample_gas_water_heater_config
);
typed_config_tests!(
    round_trip_electric_resistance_water_heater_config,
    deny_unknown_field_electric_resistance_water_heater_config,
    ElectricResistanceWaterHeaterConfig,
    sample_electric_resistance_water_heater_config
);
typed_config_tests!(
    round_trip_tankless_water_heater_config,
    deny_unknown_field_tankless_water_heater_config,
    TanklessWaterHeaterConfig,
    sample_tankless_water_heater_config
);
typed_config_tests!(
    round_trip_heat_pump_water_heater_config,
    deny_unknown_field_heat_pump_water_heater_config,
    HeatPumpWaterHeaterConfig,
    sample_heat_pump_water_heater_config
);
typed_config_tests!(
    round_trip_battery_config,
    deny_unknown_field_battery_config,
    BatteryConfig,
    sample_battery_config
);
typed_config_tests!(
    round_trip_ev_config,
    deny_unknown_field_ev_config,
    EvConfig,
    sample_ev_config
);
typed_config_tests!(
    round_trip_pv_config,
    deny_unknown_field_pv_config,
    PvConfig,
    sample_pv_config
);
typed_config_tests!(
    round_trip_generator_config,
    deny_unknown_field_generator_config,
    GeneratorConfig,
    sample_generator_config
);
typed_config_tests!(
    round_trip_ventilation_config,
    deny_unknown_field_ventilation_config,
    VentilationConfig,
    sample_ventilation_config
);

#[test]
fn all_equipment_types_init_without_error() {
    let registry = EquipmentRegistry::new();
    let env = sample_env();

    smoke_case(&registry, "Gas Furnace", sample_gas_furnace_config(), &env);
    smoke_case(
        &registry,
        "Electric Furnace",
        sample_electric_furnace_config(),
        &env,
    );
    smoke_case(&registry, "Gas Boiler", sample_gas_boiler_config(), &env);
    smoke_case(
        &registry,
        "Electric Boiler",
        sample_electric_boiler_config(),
        &env,
    );
    smoke_case(
        &registry,
        "Electric Baseboard",
        sample_electric_baseboard_config(),
        &env,
    );
    smoke_case(&registry, "Ideal HVAC", sample_ideal_hvac_config(), &env);
    smoke_case(
        &registry,
        "Air Conditioner",
        sample_central_ac_config(),
        &env,
    );
    smoke_case(&registry, "Room AC", sample_room_ac_config(), &env);
    smoke_case(&registry, "ASHP Heater", sample_heat_pump_config(), &env);
    smoke_case(
        &registry,
        "Dehumidifier",
        sample_dehumidifier_config(),
        &env,
    );
    smoke_case(
        &registry,
        "Gas Water Heater",
        sample_gas_water_heater_config(),
        &env,
    );
    smoke_case(
        &registry,
        "Electric Resistance Water Heater",
        sample_electric_resistance_water_heater_config(),
        &env,
    );
    smoke_case(
        &registry,
        "Gas Tankless Water Heater",
        sample_tankless_water_heater_config(),
        &env,
    );
    smoke_case(
        &registry,
        "Heat Pump Water Heater",
        sample_heat_pump_water_heater_config(),
        &env,
    );
    smoke_case(&registry, "Battery", sample_battery_config(), &env);
    smoke_case(&registry, "EV", sample_ev_config(), &env);
    smoke_case(&registry, "PV", sample_pv_config(), &env);
    smoke_case(&registry, "Gas Generator", sample_generator_config(), &env);
    smoke_case(&registry, "HRV", sample_ventilation_config(), &env);
}
