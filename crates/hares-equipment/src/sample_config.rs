//! A valid sample config per registry class: the schema's override
//! acceptance and any surface that must construct a class's config
//! without an HPXML builds from these. The samples' required fields carry
//! resolver-shaped values; every optional field is set or None as the
//! resolver would leave it.

use std::collections::HashMap;

use hares_types::FuelType;

use crate::config::{ConfigValue, RejectUnknownKeys};

use crate::GeneratorEfficiencyCurvePoint;
use crate::hvac::cooling_config::{CentralAirConditionerConfig, DehumidifierConfig, RoomAcConfig};
use crate::hvac::core_config::DuctConfig;
use crate::hvac::heat_pump::defrost::DefrostConfig;
use crate::hvac::heat_pump_config::{
    HeatPumpCommonConfig, HeatPumpCoolerConfig, HeatPumpHeaterConfig,
};
use crate::hvac::heating_config::{
    ElectricBaseboardConfig, ElectricBoilerConfig, ElectricFurnaceConfig, GasBoilerConfig,
    GasFurnaceConfig, HvacSetpointConfig, IdealHvacConfig,
};
use crate::protocol_bridge::config::ProtocolBridgeConfig;
use crate::water_heater::wh_config::{
    ElectricResistanceWaterHeaterConfig, GasWaterHeaterConfig, HeatPumpWaterHeaterConfig,
    IndirectTankConfig, TanklessWaterHeaterConfig,
};
use crate::{EquipmentConfig, EvConfig, GeneratorConfig, PvConfig, VentilationConfig};

fn typed_alias_config<T: crate::EquipmentTypedConfig>(name: &str, cfg: T) -> EquipmentConfig {
    let mut out =
        EquipmentConfig::from_typed(name.to_string(), T::equipment_type_name().to_string(), cfg)
            .expect("the sample config builds");
    out.ochre_class = name.to_string();
    out
}

fn raw_config(class: &str, floats: &[(&str, f64)], strings: &[(&str, &str)]) -> EquipmentConfig {
    let mut raw: HashMap<String, ConfigValue> = HashMap::new();
    for &(k, v) in floats {
        raw.insert(k.to_string(), ConfigValue::Float(v));
    }
    for &(k, v) in strings {
        raw.insert(k.to_string(), ConfigValue::Text(v.to_string()));
    }
    EquipmentConfig::raw(class.to_string(), class.to_string(), raw)
}

/// A valid sample config for `class` (the registry's class names), or
/// `None` for a name no registry builds.
#[must_use]
pub fn sample_config_for_class(class: &str) -> Option<EquipmentConfig> {
    let config = match class {
        "Gas Furnace" => typed_alias_config(
            class,
            GasFurnaceConfig {
                equipment_id: None,
                zone_id: None,
                afue: 0.8,
                capacity_w: 8_000.0,
                number_of_speeds: 1,
                fan_power_w: Some(0.0),
                ducts: DuctConfig::default(),
                stage_heating_capacities_w: None,
                stage_heating_eirs: None,
                setpoint: HvacSetpointConfig::default(),
            },
        ),
        "Electric Furnace" => typed_alias_config(
            class,
            ElectricFurnaceConfig {
                equipment_id: None,
                zone_id: None,
                eir: 1.0,
                capacity_w: 6_000.0,
                number_of_speeds: 1,
                fan_power_w: Some(0.0),
                ducts: DuctConfig::default(),
                setpoint: HvacSetpointConfig::default(),
            },
        ),
        "Electric Baseboard" => typed_alias_config(
            class,
            ElectricBaseboardConfig {
                equipment_id: None,
                zone_id: None,
                capacity_w: 2_000.0,
                eir: 1.0,
                setpoint: HvacSetpointConfig::default(),
            },
        ),
        "Gas Boiler" => typed_alias_config(
            class,
            GasBoilerConfig {
                equipment_id: None,
                zone_id: None,
                loop_id: Some(1),
                afue: 0.82,
                capacity_w: 7_000.0,
                number_of_speeds: 1,
                fan_power_w: Some(0.0),
                flow_rate_kg_s: 0.3,
                return_temp_c: 70.0,
                fluid_type: hares_types::FluidType::Water,
                setpoint: HvacSetpointConfig::default(),
                condensing: false,
                ..Default::default()
            },
        ),
        "Electric Boiler" => typed_alias_config(
            class,
            ElectricBoilerConfig {
                equipment_id: None,
                zone_id: None,
                loop_id: Some(1),
                eir: 1.0,
                capacity_w: 7_000.0,
                number_of_speeds: 1,
                fan_power_w: Some(0.0),
                flow_rate_kg_s: 0.3,
                return_temp_c: 70.0,
                fluid_type: hares_types::FluidType::Water,
                setpoint: HvacSetpointConfig::default(),
            },
        ),
        "Air Conditioner" => typed_alias_config(
            class,
            CentralAirConditionerConfig {
                equipment_id: None,
                zone_id: None,
                capacity_w: 8_000.0,
                eir: 3.412_141_633 / 14.0,
                shr: Some(0.75),
                number_of_speeds: 1,
                stage_capacities_w: None,
                stage_eirs: None,
                stage_shrs: None,
                fan_power_w: Some(0.0),
                // si-guard-ignore: the config field's own name
                fan_power_w_per_cfm: None,
                setpoint: HvacSetpointConfig::default(),
                hysteresis_c: None,
                airflow_m3_s_per_w: None,
                fraction_load_served: Some(1.0),
                crankcase_heater_w: None,
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
                charge_defect_ratio: None,
                min_oat_compressor_cooling_c: None,
            },
        ),
        "Room AC" => typed_alias_config(
            class,
            RoomAcConfig {
                equipment_id: None,
                zone_id: None,
                capacity_w: 4_000.0,
                eir: 3.412_141_633 / 10.0,
                setpoint: HvacSetpointConfig::default(),
                hysteresis_c: None,
                airflow_m3_s_per_w: None,
                shr: None,
                startup_cd: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                crankcase_heater_w: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                min_oat_compressor_cooling_c: None,
            },
        ),
        "Dehumidifier" => typed_alias_config(
            class,
            DehumidifierConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_liters_per_day: Some(20.0),
                energy_factor: Some(2.0),
                integrated_energy_factor: None,
                fraction_served: Some(1.0),
                target_rh: Some(0.5),
                part_load_curve_coeffs: None,
                plf_min: None,
                off_cycle_parasitic_load_w: None,
                min_operating_temp_c: None,
                max_operating_temp_c: None,
            },
        ),
        "Heat Pump Heater" | "ASHP Heater" | "MSHP Heater" | "GSHP Heater" | "WSHP Heater" => {
            typed_alias_config(
                class,
                HeatPumpHeaterConfig {
                    reject_unknown_keys: RejectUnknownKeys,
                    common: HeatPumpCommonConfig {
                        heating_capacity_w: Some(8_000.0),
                        heating_eir: Some(3.412_141_633 / 9.0),
                        backup_capacity_w: Some(5_000.0),
                        fraction_heating_load_served: Some(1.0),
                        cooling_capacity_w: Some(8_000.0),
                        cooling_eir: Some(3.412_141_633 / 14.0),
                        fraction_cooling_load_served: Some(1.0),
                        number_of_speeds: 1,
                        shr: Some(0.75),
                        er_stages: 1,
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
                },
            )
        }
        "ASHP Cooler" | "MSHP Cooler" | "GSHP Cooler" | "WSHP Cooler" => typed_alias_config(
            class,
            HeatPumpCoolerConfig {
                reject_unknown_keys: RejectUnknownKeys,
                common: HeatPumpCommonConfig {
                    heating_capacity_w: Some(8_000.0),
                    heating_eir: Some(3.412_141_633 / 9.0),
                    cooling_capacity_w: Some(8_000.0),
                    cooling_eir: Some(3.412_141_633 / 14.0),
                    fraction_cooling_load_served: Some(1.0),
                    number_of_speeds: 1,
                    is_mini_split: class == "MSHP Cooler",
                    shr: Some(0.75),
                    er_stages: 1,
                    ..Default::default()
                },
                stage_shrs: None,
                crankcase_heater_w: None,
                crankcase_heater_threshold_c: None,
                min_oat_cooling_c: 10.0,
            },
        ),
        "Ideal HVAC" => typed_alias_config(
            class,
            IdealHvacConfig {
                equipment_id: None,
                zone_id: None,
                setpoint: HvacSetpointConfig::default(),
                deadband_c: None,
                n_speeds: None,
                ideal_capacity_mode: None,
                heating_capacity_w: Some(8_000.0),
                cooling_capacity_w: Some(8_000.0),
                shr: None,
                fraction_heating_load_served: None,
                fraction_cooling_load_served: None,
                rated_fan_power_w: None,
                rated_eir: None,
                capacity_min_w: None,
                fuel_type: None,
                capacity_biquadratic_coeffs: None,
                eir_biquadratic_coeffs: None,
            },
        ),
        "Gas Water Heater" => typed_alias_config(
            class,
            GasWaterHeaterConfig {
                fan_power_w: None,
                equipment_id: None,
                zone_id: Some(1),
                loop_id: Some(1),
                fuel_type: FuelType::Gas,
                tank_volume_m3: None,
                tank_height_m: None,
                energy_factor: Some(0.8),
                uniform_energy_factor: None,
                setpoint_c: Some(52.0),
                deadband_c: None,
                max_tank_temp_c: None,
                initial_tank_temp_c: None,
                tank_nodes: None,
                heating_capacity_w: Some(12_000.0),
                ua_w_per_k: None,
                avg_water_draw_l_per_day: None,
                draw_flow_rate_kg_s: None,
                draw_flow_rate_source: None,
                mains_temp_c_source: None,
                appliance_draw_flow_rate_source: None,
                pilot_power_w: Some(0.0),
                flue_loss_fraction: None,
                skin_loss_fraction: None,
                ignition_type: None,
                performance_adjustment: None,
                zone_type: None,
                first_hour_rating_m3: None,
                jacket_r_value_m2_k_w: None,
                conversion_efficiency: None,
                fixture_delivery_temp_c: None,
                hot_draw_temp_c: None,
                pilot_fraction_to_tank: None,
            },
        ),
        "Resistance Water Heater" | "Electric Resistance Water Heater" => typed_alias_config(
            class,
            ElectricResistanceWaterHeaterConfig {
                equipment_id: None,
                zone_id: Some(1),
                loop_id: Some(1),
                tank_volume_m3: None,
                tank_height_m: None,
                energy_factor: None,
                uniform_energy_factor: None,
                setpoint_c: Some(52.0),
                deadband_c: None,
                max_tank_temp_c: None,
                initial_tank_temp_c: None,
                tank_nodes: None,
                heating_capacity_w: Some(4_500.0),
                ua_w_per_k: None,
                avg_water_draw_l_per_day: None,
                draw_flow_rate_kg_s: None,
                draw_flow_rate_source: None,
                mains_temp_c_source: None,
                appliance_draw_flow_rate_source: None,
                performance_adjustment: None,
                zone_type: None,
                first_hour_rating_m3: None,
                element_power_w: None,
                max_setpoint_ramp_rate_c_per_min: None,
                element_priority_mode: None,
                jacket_r_value_m2_k_w: None,
                max_combined_power_w: None,
                fixture_delivery_temp_c: None,
                hot_draw_temp_c: None,
            },
        ),
        "Tankless Water Heater" | "Gas Tankless Water Heater" => typed_alias_config(
            class,
            TanklessWaterHeaterConfig {
                equipment_id: None,
                zone_id: Some(1),
                loop_id: Some(1),
                fuel_type: if class == "Gas Tankless Water Heater" {
                    FuelType::Gas
                } else {
                    FuelType::Electric
                },
                energy_factor: Some(if class == "Gas Tankless Water Heater" {
                    0.82
                } else {
                    0.95
                }),
                uniform_energy_factor: None,
                heating_capacity_w: Some(if class == "Gas Tankless Water Heater" {
                    20_000.0
                } else {
                    12_000.0
                }),
                setpoint_c: Some(50.0),
                parasitic_power_w: Some(5.0),
                performance_adjustment: Some(0.92),
                inlet_temp_c: None,
                draw_flow_rate_kg_s: Some(0.2),
                draw_flow_rate_source: None,
                mains_temp_c_source: None,
                appliance_draw_flow_rate_source: None,
                avg_water_draw_l_per_day: None,
                zone_type: None,
                min_flow_kg_s: None,
                min_flow_gpm: None,
            },
        ),
        "Heat Pump Water Heater" | "HPWH" => typed_alias_config(
            class,
            HeatPumpWaterHeaterConfig {
                equipment_id: None,
                zone_id: Some(1),
                loop_id: Some(1),
                tank_volume_m3: None,
                tank_height_m: None,
                cop: Some(2.5),
                backup_element_power_w: Some(4_500.0),
                ua_w_per_k: None,
                setpoint_c: Some(52.0),
                deadband_c: None,
                max_tank_temp_c: None,
                initial_tank_temp_c: None,
                tank_nodes: None,
                tempering_valve_setpoint_c: None,
                avg_water_draw_l_per_day: None,
                draw_flow_rate_kg_s: None,
                appliance_draw_flow_rate_source: None,
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
                cop_curve_is_normalized: None,
                performance_adjustment: None,
                zone_type: None,
                first_hour_rating_m3: None,
                jacket_r_value_m2_k_w: None,
                fixture_delivery_temp_c: None,
                low_power_hpwh: None,
                uniform_energy_factor: None,
            },
        ),
        "Indirect Tank" => typed_alias_config(
            class,
            IndirectTankConfig {
                equipment_id: None,
                zone_id: Some(1),
                boiler_loop_id: Some(1),
                tank_volume_m3: None,
                tank_height_m: None,
                ua_w_per_k: None,
                hx_ua_w_per_k: Some(150.0),
                setpoint_c: Some(52.0),
                deadband_c: None,
                max_tank_temp_c: None,
                initial_tank_temp_c: None,
                tank_nodes: None,
                draw_flow_rate_kg_s: None,
                avg_water_draw_l_per_day: None,
                draw_flow_rate_source: None,
                mains_temp_c_source: None,
                appliance_draw_flow_rate_source: None,
                performance_adjustment: None,
                zone_type: None,
                first_hour_rating_m3: None,
                jacket_r_value_m2_k_w: None,
                fixture_delivery_temp_c: None,
                hot_draw_temp_c: None,
                boiler_loop_flow_rate_kg_s: None,
            },
        ),
        "Battery" => typed_alias_config(
            class,
            crate::battery::BatteryConfig {
                equipment_id: None,
                zone_id: None,
                capacity_j: 13.5 * 3.6e6,
                max_charge_w: 5000.0,
                max_discharge_w: 5000.0,
                n_series: None,
                n_parallel: None,
                ah_cell: None,
                v_cell: None,
                cell_resistance_ohm: None,
                pack_voltage_v: None,
                chemistry: None,
                standby_power_w: None,
                self_discharge_pct_per_day: None,
                min_soc: Some(0.15),
                max_soc: Some(0.95),
                initial_soc: Some(0.5),
                initial_cell_temp_c: None,
                import_limit_w: None,
                export_limit_w: None,
                heater_power_w: None,
                heater_threshold_c: None,
                min_discharge_temp_c: None,
                full_power_temp_c: None,
                min_charge_temp_c: None,
                cell_thermal_mass_j_per_k: None,
                cell_ua_w_per_k: None,
                inverter_efficiency: None,
                charge_efficiency: None,
                discharge_efficiency: None,
                bms_mode: None,
                grid_export_rule: None,
                power_factor: None,
                inverter_capacity_va: None,
                grid_forming: None,
                min_dwell_steps: 0,
            },
        ),
        "PV" => typed_alias_config(
            class,
            PvConfig {
                equipment_id: None,
                zone_id: None,
                capacity_w: 5000.0,
                tilt_deg: Some(0.0),
                azimuth_deg: Some(0.0),
                attached_boundary_id: None,
                module_type: None,
                noct_c: None,
                array_type: None,
                system_losses_fraction: None,
                inverter_efficiency: Some(0.96),
                inverter_capacity_w: Some(5000.0),
                power_factor: Some(1.0),
                surface_resolution_deg: Some(360.0),
                sam_lut_path: None,
                soiling: None,
                arrays: None,
            },
        ),
        "EV" | "Electric Vehicle" => typed_alias_config(
            class,
            EvConfig {
                equipment_id: None,
                capacity_j: 60.0 * 3.6e6,
                charging_level: Some("L2".to_string()),
                max_charging_power_w: 7200.0,
                charging_efficiency: Some(0.9),
                l1_current_a: None,
                l1_voltage_v: None,
                soc_max: Some(1.0),
                initial_soc: Some(0.5),
                battery_temp_c: Some(20.0),
                min_charge_temp_c: None,
                full_power_temp_c: None,
                heater_power_w: None,
                heater_threshold_c: None,
                thermal_mass_j_per_k: None,
                ua_w_per_k: None,
                n_series: None,
                n_parallel: None,
                cell_resistance_ohm: None,
                v2l_enabled: None,
                v2l_soc_reserve: None,
                v2l_max_discharge_w: None,
                v2g_enabled: None,
                v2g_soc_reserve: None,
                v2g_max_discharge_w: None,
                chemistry: None,
                fuel_economy_j_per_m: None,
                ready_soc: None,
                charging_strategy: None,
                plug_in_policy: None,
                power_limit_w: None,
                initial_connection_state: None,
                power_factor: None,
                charger_capacity_va: None,
                cc_cv_transition_soc: None,
                charging_priority: None,
                discharge_respects_deadline: true,
            },
        ),
        "Scheduled EV" | "EventBasedLoad" | "Clothes Washer" | "Dishwasher" | "Clothes Dryer"
        | "Cooking Range" | "Microwave" => raw_config(
            class,
            &[
                ("zone_id", 1.0),
                ("active_power_kw", 1000.0),
                ("power_constant_w", 3600.0),
                ("sensible_gain_fraction", 0.0),
                ("latent_gain_fraction", 0.0),
            ],
            &[
                ("event_window_source", "constant"),
                ("power_schedule_source", "constant"),
            ],
        ),
        "Gas Generator" | "Gas Fuel Cell" => typed_alias_config(
            class,
            GeneratorConfig {
                equipment_id: None,
                zone_id: None,
                fuel_type: Some(FuelType::Gas),
                rated_power_w: 6000.0,
                eta_electric: Some(0.3),
                eta_thermal: Some(0.0),
                eta_jacket_water: None,
                eta_lube_oil: None,
                eta_exhaust: None,
                efficiency_curve_points: Some(vec![
                    GeneratorEfficiencyCurvePoint {
                        capacity_ratio: 0.0,
                        efficiency_ratio: 0.0,
                    },
                    GeneratorEfficiencyCurvePoint {
                        capacity_ratio: 0.1,
                        efficiency_ratio: 0.47,
                    },
                    GeneratorEfficiencyCurvePoint {
                        capacity_ratio: 0.333,
                        efficiency_ratio: 0.78,
                    },
                    GeneratorEfficiencyCurvePoint {
                        capacity_ratio: 1.0,
                        efficiency_ratio: 1.0,
                    },
                ]),
                efficiency_type: None,
                delta_w_per_s: Some(1000.0),
                capacity_min_w: Some(0.0),
                grid_import_limit_w: Some(0.0),
                export_limit_w: Some(0.0),
                loop_id: Some(1),
                flow_rate_kg_s: None,
                supply_temp_c: None,
                return_temp_c: None,
                inverter_efficiency: None,
                stack_temp_c: None,
                stack_cooler_r0: None,
                stack_cooler_r1: None,
                stack_cooler_r2: None,
                stack_cooler_r3: None,
                stack_nominal_temp_c: None,
                heat_rec_max_temp_c: None,
                no_load_fuel_fraction: None,
                grid_forming: None,
                standby_mode: None,
                parasitic_fraction: None,
            },
        ),
        "HRV" | "ERV" | "Ventilation Fan" => typed_alias_config(
            class,
            VentilationConfig {
                equipment_id: None,
                zone_id: None,
                flow_rate_m3_s: 0.03,
                fan_power_w: Some(40.0),
                supply_fan_power_w: None,
                exhaust_fan_power_w: None,
                sensible_effectiveness: Some(0.7),
                latent_effectiveness: Some(if class == "ERV" { 0.5 } else { 0.0 }),
                bypass_temp_min_c: None,
                bypass_temp_max_c: None,
                defrost_temp_c: None,
                defrost_initial_time_fraction: None,
                defrost_time_increase_rate_per_k: None,
                ventilation_type: Some(
                    if class == "ERV" {
                        "erv"
                    } else if class == "HRV" {
                        "hrv"
                    } else {
                        "exhaust_fan"
                    }
                    .to_string(),
                ),
                balanced: None,
                hours_in_operation: None,
            },
        ),
        "Protocol Bridge" => typed_alias_config(
            class,
            ProtocolBridgeConfig {
                equipment_id: None,
                registered_protocols: vec![],
                handlers: vec![],
            },
        ),
        // The scheduled loads: lighting, plug loads, refrigeration, pumps,
        // heaters, gas loads and fans, the generic raw classes. A class
        // the raw parameter lists name is a raw-parameter load.
        _ => {
            return if crate::raw_params::raw_params_for_class(class).is_some() {
                Some(raw_config(
                    class,
                    &[
                        ("zone_id", 1.0),
                        ("active_power_kw", 1000.0),
                        ("power_constant_w", 1000.0),
                        ("sensible_gain_fraction", 0.0),
                        ("latent_gain_fraction", 0.0),
                    ],
                    &[
                        ("event_window_source", "constant"),
                        ("power_schedule_source", "constant"),
                    ],
                ))
            } else {
                None
            };
        }
    };
    Some(config)
}

/// The fields an override of `class` may set: the typed config's
/// `DescribeFields` impl for a typed class, the raw parameter list's
/// schemas for a raw class; `None` for a name no registry builds. The
/// input schema's override classes derive from this, so a class's field
/// list is the deserialisation's own, never a parallel list.
#[must_use]
pub fn override_fields_for_class(class: &str) -> Option<Vec<hares_types::FieldSchema>> {
    use hares_types::DescribeFields;
    let fields = match class {
        "Gas Furnace" => <GasFurnaceConfig as DescribeFields>::fields(),
        "Electric Furnace" => <ElectricFurnaceConfig as DescribeFields>::fields(),
        "Electric Baseboard" => <ElectricBaseboardConfig as DescribeFields>::fields(),
        "Gas Boiler" => <GasBoilerConfig as DescribeFields>::fields(),
        "Electric Boiler" => <ElectricBoilerConfig as DescribeFields>::fields(),
        "Air Conditioner" => <CentralAirConditionerConfig as DescribeFields>::fields(),
        "Room AC" => <RoomAcConfig as DescribeFields>::fields(),
        "Dehumidifier" => <DehumidifierConfig as DescribeFields>::fields(),
        "Heat Pump Heater" | "ASHP Heater" | "MSHP Heater" | "GSHP Heater" | "WSHP Heater" => {
            <HeatPumpHeaterConfig as DescribeFields>::fields()
        }
        "ASHP Cooler" | "MSHP Cooler" | "GSHP Cooler" | "WSHP Cooler" => {
            <HeatPumpCoolerConfig as DescribeFields>::fields()
        }
        "Ideal HVAC" => <IdealHvacConfig as DescribeFields>::fields(),
        "Gas Water Heater" => <GasWaterHeaterConfig as DescribeFields>::fields(),
        "Resistance Water Heater" | "Electric Resistance Water Heater" => {
            <ElectricResistanceWaterHeaterConfig as DescribeFields>::fields()
        }
        "Tankless Water Heater" | "Gas Tankless Water Heater" => {
            <TanklessWaterHeaterConfig as DescribeFields>::fields()
        }
        "Indirect Tank" => <IndirectTankConfig as DescribeFields>::fields(),
        "Heat Pump Water Heater" | "HPWH" => {
            <HeatPumpWaterHeaterConfig as DescribeFields>::fields()
        }
        "Battery" => <crate::battery::BatteryConfig as DescribeFields>::fields(),
        "PV" => <PvConfig as DescribeFields>::fields(),
        "EV" | "Electric Vehicle" => <EvConfig as DescribeFields>::fields(),
        "Gas Generator" | "Gas Fuel Cell" => <GeneratorConfig as DescribeFields>::fields(),
        "HRV" | "ERV" | "Ventilation Fan" => <VentilationConfig as DescribeFields>::fields(),
        "Protocol Bridge" => <ProtocolBridgeConfig as DescribeFields>::fields(),
        _ => crate::raw_params::raw_params_for_class(class).map(|params| params.field_schemas())?,
    };
    Some(fields).map(|schemas| {
        // The reserved `equipment_id` is not an override field: the
        // dwelling's assembly assigns ids, and an override of it is
        // rejected naming the reservation. The schema publishes fields
        // the override accepts, so the reserved key is not one.
        schemas
            .into_iter()
            .filter(|field| field.name != crate::config::KEY_EQUIPMENT_ID)
            .collect()
    })
}
