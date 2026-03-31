//! Generic equipment lifecycle integration tests.
//!
//! Tests the cross-equipment contract: every Equipment implementor must
//! satisfy the full lifecycle — init, apply_control, step, telemetry,
//! save_state, load_state — through the Equipment trait boundary, not via
//! type-specific internals.
//!
//! Phase 2 gate: grows as each equipment type lands. Each new equipment type
//! must add a `test_equipment_lifecycle` call here.
//!
//! NOTE: Type-specific physics assertions (setpoint tracking, efficiency curves,
//! etc.) live in the per-equipment test files. This harness only tests the
//! trait contract and the cross-layer port accumulation pipeline.

mod common;

use std::collections::HashMap;
use std::time::Duration;

use hares_equipment::{
    CentralAirConditionerConfig, DehumidifierConfig, DuctConfig, ElectricBaseboardConfig,
    ElectricBoilerConfig, ElectricFurnaceConfig, Equipment, EquipmentConfig, EquipmentRegistry,
    EquipmentTypedConfig, EvConfig, GasBoilerConfig, GasFurnaceConfig, GeneratorConfig,
    HeatPumpHeaterConfig, IdealHvacConfig, RoomAcConfig, VentilationConfig,
    config::ConfigValue,
    water_heater::water_heater_config::{
        ElectricResistanceWaterHeaterConfig, GasWaterHeaterConfig, HeatPumpWaterHeaterConfig,
        TanklessWaterHeaterConfig,
    },
};
use hares_types::{
    ControlSignal, CoreCapabilities, EnvironmentState, FuelType, OperatingMode, PortSlots,
    SurfaceIrradiance, ZoneId, validate_core_contract,
};

use common::{default_env, env_with_zone_temp};

// ---------------------------------------------------------------------------
// Generic lifecycle harness
// ---------------------------------------------------------------------------

/// Builds a `PortSlots` wired exactly for `equipment.ports()`.
///
/// Called after `init()` so that ports() reflects the post-init declaration.
fn ports_for(equipment: &dyn Equipment) -> PortSlots {
    PortSlots::from_declarations(equipment.ports())
}

/// Verifies the full Equipment lifecycle contract for any implementor.
///
/// Arguments:
/// - `equipment`: initialized and ready (init() has been called)
/// - `env`: environment state to use for step/control calls
/// - `valid_signal`: a signal the equipment must accept via apply_control
/// - `invalid_signal`: a signal the equipment must reject via apply_control
fn assert_equipment_lifecycle(
    equipment: &mut dyn Equipment,
    env: &EnvironmentState,
    valid_signal: ControlSignal,
    invalid_signal: ControlSignal,
) {
    let dt = Duration::from_secs(60);

    // 1. Descriptor must be populated after init.
    let desc = equipment.descriptor();
    assert!(
        !desc.name.is_empty(),
        "descriptor.name must be non-empty after init"
    );
    assert!(
        !desc.telemetry_fields.is_empty(),
        "descriptor.telemetry_fields must be non-empty after init for '{}'",
        desc.name
    );

    // 2. apply_control with valid signal must succeed.
    equipment
        .apply_control(&valid_signal)
        .expect("apply_control with valid signal must return Ok");

    // 3. apply_control with invalid signal must fail.
    let rejection = equipment.apply_control(&invalid_signal);
    assert!(
        rejection.is_err(),
        "apply_control with invalid signal must return Err for '{}'",
        equipment.descriptor().name
    );

    // 4. step must succeed and write at least one port contribution.
    let mut ports = ports_for(equipment);
    equipment.update_control(env);
    equipment
        .step(env, dt, &mut ports)
        .expect("step must return Ok");

    // Verify at least one port domain has a non-zero accumulation.
    // Equipment may contribute to thermal, electrical, or fuel — at least one must be non-zero.
    let has_thermal = ports
        .thermal
        .iter()
        .any(|a| a.sensible_gain_w.abs() > 0.0 || a.latent_gain_w.abs() > 0.0);
    let has_electrical = ports.electrical.load_power_kw.abs() > 0.0
        || ports.electrical.generation_power_kw.abs() > 0.0;
    let gas_w = ports.fuel.get(FuelType::Gas);
    let has_fuel = gas_w.abs() > 0.0 || ports.fuel.get(FuelType::Propane).abs() > 0.0;

    assert!(
        has_thermal || has_electrical || has_fuel,
        "step() for '{}' must write at least one port contribution; \
         thermal={}, electrical_load={}, electrical_gen={}, gas={gas_w}",
        equipment.descriptor().name,
        ports.thermal.iter().map(|a| a.sensible_gain_w).sum::<f64>(),
        ports.electrical.load_power_kw,
        ports.electrical.generation_power_kw,
    );

    // 5. telemetry() field count must match descriptor.telemetry_fields.len().
    let expected_field_count = equipment.descriptor().telemetry_fields.len();
    let actual_field_count = equipment.telemetry().len();
    assert_eq!(
        actual_field_count,
        expected_field_count,
        "telemetry field count must match descriptor for '{}': got {actual_field_count}, expected {expected_field_count}",
        equipment.descriptor().name
    );

    // 6. save_state must produce non-empty bytes.
    let saved = equipment.save_state();
    assert!(
        !saved.is_empty(),
        "save_state must return non-empty bytes for '{}'",
        equipment.descriptor().name
    );

    // 7. Capture port output from first step (reference), then step again (mutate).
    let port_output_step1 = ports.clone();

    let mut ports2 = ports_for(equipment);
    equipment.update_control(env);
    equipment
        .step(env, dt, &mut ports2)
        .expect("second step must return Ok");

    // 8. Restore from snapshot and step again. Output must match step 1.
    equipment
        .load_state(&saved)
        .expect("load_state must return Ok for valid snapshot bytes");

    let mut ports_after_restore = ports_for(equipment);
    equipment.update_control(env);
    equipment
        .step(env, dt, &mut ports_after_restore)
        .expect("step after restore must return Ok");

    // Same state + same env → same deterministic port output.
    let orig_net = port_output_step1.electrical.net_active_kw();
    let rest_net = ports_after_restore.electrical.net_active_kw();
    assert!(
        (orig_net - rest_net).abs() < 1e-3,
        "electrical net_active_kw after restore must match step-1 output within 1 W for '{}': \
         step1={orig_net}, after_restore={rest_net}",
        equipment.descriptor().name,
    );
    for (orig_acc, rest_acc) in port_output_step1
        .thermal
        .iter()
        .zip(ports_after_restore.thermal.iter())
    {
        assert!(
            (orig_acc.sensible_gain_w - rest_acc.sensible_gain_w).abs() < 0.05,
            "sensible_gain_w for zone {:?} after restore must match step-1 for '{}': \
             step1={}, after_restore={}",
            orig_acc.zone,
            equipment.descriptor().name,
            orig_acc.sensible_gain_w,
            rest_acc.sensible_gain_w,
        );
    }
}

// ---------------------------------------------------------------------------
// Configuration helpers
// ---------------------------------------------------------------------------

fn config_with_floats(name: &str, class: &str, entries: &[(&str, f64)]) -> EquipmentConfig {
    let mut raw: HashMap<String, ConfigValue> = HashMap::new();
    for &(k, v) in entries {
        raw.insert(k.to_string(), ConfigValue::Float(v));
    }
    EquipmentConfig {
        name: name.to_string(),
        ochre_class: class.to_string(),
        payload: hares_equipment::ConfigPayload::Raw { data: raw },
    }
}

fn config_mixed(
    name: &str,
    class: &str,
    floats: &[(&str, f64)],
    strings: &[(&str, &str)],
) -> EquipmentConfig {
    let mut raw: HashMap<String, ConfigValue> = HashMap::new();
    for &(k, v) in floats {
        raw.insert(k.to_string(), ConfigValue::Float(v));
    }
    for &(k, v) in strings {
        raw.insert(k.to_string(), ConfigValue::Text(v.to_string()));
    }
    EquipmentConfig {
        name: name.to_string(),
        ochre_class: class.to_string(),
        payload: hares_equipment::ConfigPayload::Raw { data: raw },
    }
}

// ---------------------------------------------------------------------------
// Stage 1: ScheduledLoad
// ---------------------------------------------------------------------------

/// ScheduledLoad with a constant 1.5 kW power schedule (plug load).
/// Valid signal: LoadFraction (reduces output by fraction).
/// Invalid signal: ThermalSetpoint (not in capabilities).
#[test]
fn lifecycle_scheduled_load() {
    let registry = EquipmentRegistry::new();
    let cfg = config_mixed(
        "Plug Loads",
        "Plug Loads",
        &[
            ("zone_id", 1.0),
            ("sensible_gain_fraction", 0.5),
            ("latent_gain_fraction", 0.1),
            ("power_constant_kw", 1.5),
        ],
        &[("power_schedule_source", "constant")],
    );

    let mut eq = registry
        .create("Plug Loads", cfg.clone())
        .expect("registry must create Plug Loads");
    let env = env_with_zone_temp(21.0);
    eq.init(&cfg, &env).expect("init must succeed");

    let valid = ControlSignal::LoadFraction { fraction: 0.8 };
    let invalid = ControlSignal::ThermalSetpoint {
        heating_setpoint_c: Some(21.0),
        cooling_setpoint_c: None,
        deadband_c: None,
    };

    assert_equipment_lifecycle(eq.as_mut(), &env, valid, invalid);
}

// ---------------------------------------------------------------------------
// Stage 2: Battery
// ---------------------------------------------------------------------------

/// Battery with 13.5 kWh capacity, 50% SOC, power setpoint control.
/// Valid signal: PowerSetpoint (discharge at 2 kW).
/// Invalid signal: ThermalSetpoint (not in capabilities).
#[test]
fn lifecycle_battery() {
    let registry = EquipmentRegistry::new();
    let cfg = config_with_floats(
        "Battery",
        "Battery",
        &[
            ("capacity_kwh", 13.5),
            ("max_charge_kw", 5.0),
            ("max_discharge_kw", 5.0),
            ("initial_soc", 0.5),
            ("min_soc", 0.15),
            ("max_soc", 0.95),
        ],
    );

    let mut eq = registry
        .create("Battery", cfg.clone())
        .expect("registry must create Battery");
    let env = default_env();
    eq.init(&cfg, &env).expect("init must succeed");

    let valid = ControlSignal::PowerSetpoint {
        active_power_kw: 2.0,
        reactive_power_kvar: None,
    };
    let invalid = ControlSignal::ThermalSetpoint {
        heating_setpoint_c: Some(21.0),
        cooling_setpoint_c: None,
        deadband_c: None,
    };

    assert_equipment_lifecycle(eq.as_mut(), &env, valid, invalid);
}

// ---------------------------------------------------------------------------
// Stage 3: Gas Furnace (Thermostat + Furnace)
// ---------------------------------------------------------------------------

/// Gas Furnace with zone below heating setpoint → must produce thermal output.
/// Valid signal: ThermalSetpoint.
/// Invalid signal: PowerSetpoint (furnaces do not accept direct power setpoints).
#[test]
fn lifecycle_gas_furnace() {
    let registry = EquipmentRegistry::new();
    let cfg = EquipmentConfig::from_typed(
        "Gas Furnace".to_string(),
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
    );

    let mut eq = registry
        .create("Gas Furnace", cfg.clone())
        .expect("registry must create Gas Furnace");
    // Zone below setpoint (18°C) ensures the furnace fires during step().
    let env = env_with_zone_temp(18.0);
    eq.init(&cfg, &env).expect("init must succeed");

    let valid = ControlSignal::ThermalSetpoint {
        heating_setpoint_c: Some(21.0),
        cooling_setpoint_c: Some(27.0),
        deadband_c: Some(1.0),
    };
    let invalid = ControlSignal::PowerSetpoint {
        active_power_kw: 5.0,
        reactive_power_kvar: None,
    };

    assert_equipment_lifecycle(eq.as_mut(), &env, valid, invalid);
}

// ---------------------------------------------------------------------------
// Equipment → PortSlots pipeline (within hares-equipment boundary)
//
// NOTE: The full equipment → PortSlots → ElectricalSolver/ThermalSolver
// pipeline requires hares-envelope as a dev-dependency of hares-equipment.
// That Cargo.toml change is out of scope for this ticket. Those tests live
// in crates/hares-core/tests/ where hares-envelope is already a dependency.
// ---------------------------------------------------------------------------

/// ScheduledLoad step() must write correct sensible and latent thermal gains
/// to the PortSlots accumulator. Validates the cross-layer port writing
/// contract without requiring hares-envelope.
#[test]
fn scheduled_load_writes_positive_thermal_gain_to_port() {
    let registry = EquipmentRegistry::new();
    let cfg = config_mixed(
        "Plug Loads",
        "Plug Loads",
        &[
            ("zone_id", 1.0),
            ("sensible_gain_fraction", 0.5),
            ("latent_gain_fraction", 0.1),
            ("power_constant_kw", 1.5),
        ],
        &[("power_schedule_source", "constant")],
    );

    let mut eq = registry
        .create("Plug Loads", cfg.clone())
        .expect("registry must create Plug Loads");
    let env = env_with_zone_temp(21.0);
    eq.init(&cfg, &env).expect("init");

    // PortSlots wired from equipment.ports() includes thermal for zone 1.
    let mut ports = ports_for(eq.as_ref());
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports)
        .expect("step");

    let thermal_acc = ports
        .thermal
        .iter()
        .find(|a| a.zone == ZoneId(1))
        .expect("thermal accumulator for zone 1 must exist after step");

    // 1.5 kW × 1000 W/kW × 0.5 sensible_gain_fraction = 750 W sensible.
    let expected_sensible_w = 1.5 * 1_000.0 * 0.5;
    assert!(
        (thermal_acc.sensible_gain_w - expected_sensible_w).abs() < 1.0,
        "sensible_gain_w must be ~{expected_sensible_w} W (1.5 kW × 50%); got {}",
        thermal_acc.sensible_gain_w
    );
    assert!(
        thermal_acc.sensible_gain_w > 0.0,
        "sensible_gain_w must be positive (internal gain adds heat); got {}",
        thermal_acc.sensible_gain_w
    );
    assert!(
        thermal_acc.latent_gain_w > 0.0,
        "latent_gain_w must be positive (1.5 kW × 10%); got {}",
        thermal_acc.latent_gain_w
    );

    // Electrical port must also be populated.
    assert!(
        ports.electrical.load_power_kw > 0.0,
        "electrical load must be positive; got {}",
        ports.electrical.load_power_kw
    );
    assert!(
        (ports.electrical.load_power_kw - 1.5).abs() < 0.001,
        "electrical load must be ~1.5 kW; got {}",
        ports.electrical.load_power_kw
    );
}

fn typed_alias_config<T: EquipmentTypedConfig>(name: &str, cfg: T) -> EquipmentConfig {
    let mut out =
        EquipmentConfig::from_typed(name.to_string(), T::equipment_type_name().to_string(), cfg);
    out.ochre_class = name.to_string();
    out
}

fn env_with_pv_surface() -> EnvironmentState {
    let mut env = default_env();
    env.weather.solar_irradiance = vec![SurfaceIrradiance {
        surface_id: 0,
        direct_w_m2: 800.0,
        diffuse_w_m2: 100.0,
        reflected_w_m2: 50.0,
        angle_of_incidence_rad: 0.0,
    }];
    env.weather.ghi_w_m2 = 950.0;
    env.weather.dni_w_m2 = 800.0;
    env.weather.dhi_w_m2 = 100.0;
    env
}

fn config_for_class(class: &str) -> EquipmentConfig {
    match class {
        "Gas Furnace" => typed_alias_config(
            class,
            GasFurnaceConfig {
                equipment_id: None,
                zone_id: Some(1),
                afue: 0.8,
                capacity_w: 8_000.0,
                number_of_speeds: 1,
                fan_power_w: Some(0.0),
                ducts: DuctConfig::default(),
                ..GasFurnaceConfig::default()
            },
        ),
        "Electric Furnace" => typed_alias_config(
            class,
            ElectricFurnaceConfig {
                equipment_id: None,
                zone_id: Some(1),
                eir: 1.0,
                capacity_w: 6_000.0,
                number_of_speeds: 1,
                fan_power_w: Some(0.0),
                ducts: DuctConfig::default(),
                ..ElectricFurnaceConfig::default()
            },
        ),
        "Electric Baseboard" => typed_alias_config(
            class,
            ElectricBaseboardConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 2_000.0,
                eir: 1.0,
            },
        ),
        "Gas Boiler" => typed_alias_config(
            class,
            GasBoilerConfig {
                equipment_id: None,
                zone_id: Some(1),
                loop_id: None,
                afue: 0.82,
                capacity_w: 7_000.0,
                number_of_speeds: 1,
                fan_power_w: Some(0.0),
                flow_rate_kg_s: 0.3,
                return_temp_c: 40.0,
                fluid_type: hares_types::FluidType::Water,
            },
        ),
        "Electric Boiler" => typed_alias_config(
            class,
            ElectricBoilerConfig {
                equipment_id: None,
                zone_id: Some(1),
                loop_id: None,
                eir: 1.0,
                capacity_w: 7_000.0,
                number_of_speeds: 1,
                fan_power_w: Some(0.0),
                flow_rate_kg_s: 0.3,
                return_temp_c: 40.0,
                fluid_type: hares_types::FluidType::Water,
            },
        ),
        "Air Conditioner" => typed_alias_config(
            class,
            CentralAirConditionerConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 8_000.0,
                seer: 14.0,
                shr: Some(0.75),
                number_of_speeds: 1,
                stage_capacities_w: None,
                stage_eirs: None,
                stage_shrs: None,
                fan_power_w: Some(0.0),
                fan_power_w_per_cfm: None,
                fraction_load_served: Some(1.0),
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
        ),
        "Room AC" => typed_alias_config(
            class,
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 4_000.0,
                eer: 10.0,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
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
            },
        ),
        "Heat Pump Heater" | "ASHP Heater" | "MSHP Heater" => typed_alias_config(
            class,
            HeatPumpHeaterConfig {
                equipment_id: None,
                zone_id: Some(1),
                heating_capacity_w: Some(8_000.0),
                hspf: Some(9.0),
                stage_heating_capacities_w: None,
                stage_heating_eirs: None,
                backup_fuel: None,
                backup_capacity_w: None,
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
        ),
        "ASHP Cooler" | "MSHP Cooler" => typed_alias_config(
            class,
            CentralAirConditionerConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 8_000.0,
                seer: 14.0,
                shr: Some(0.75),
                number_of_speeds: 1,
                stage_capacities_w: None,
                stage_eirs: None,
                stage_shrs: None,
                fan_power_w: Some(0.0),
                fan_power_w_per_cfm: None,
                fraction_load_served: Some(1.0),
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
        ),
        "Ideal HVAC" => typed_alias_config(
            class,
            IdealHvacConfig {
                equipment_id: None,
                zone_id: Some(1),
                heating_capacity_w: Some(8_000.0),
                cooling_capacity_w: Some(8_000.0),
                ..IdealHvacConfig::default()
            },
        ),
        "Gas Water Heater" => typed_alias_config(
            class,
            GasWaterHeaterConfig {
                equipment_id: None,
                zone_id: Some(1),
                tank_volume_m3: None,
                tank_height_m: None,
                tank_diameter_m: None,
                ua_w_per_k: None,
                jacket_r_value_m2_k_w: None,
                tank_nodes: None,
                burner_node: None,
                setpoint_c: Some(52.0),
                deadband_c: Some(2.0),
                max_tank_temp_c: Some(90.0),
                heating_capacity_w: Some(12_000.0),
                burner_efficiency: Some(0.8),
                flue_loss_fraction: None,
                ignition_type: None,
                pilot_power_w: Some(0.0),
                fan_power_w: Some(0.0),
                skin_loss_fraction: None,
                fuel_type: Some("Gas".to_string()),
                mains_temp_c: Some(15.0),
                avg_water_draw_l_per_day: None,
                draw_flow_rate_kg_s: Some(0.0),
                draw_flow_rate_schedule_col: None,
                mains_temp_schedule_col: None,
                zip_z: None,
                zip_i: None,
                zip_p: None,
                zip_zq: None,
                zip_iq: None,
                zip_pq: None,
                zip_pf: None,
                zip_v0: None,
            },
        ),
        "Resistance Water Heater" | "Electric Resistance Water Heater" => typed_alias_config(
            class,
            ElectricResistanceWaterHeaterConfig {
                equipment_id: None,
                zone_id: Some(1),
                tank_volume_m3: None,
                tank_height_m: None,
                tank_diameter_m: None,
                ua_w_per_k: None,
                jacket_r_value_m2_k_w: None,
                tank_nodes: None,
                upper_element_node: None,
                lower_element_node: None,
                setpoint_c: Some(52.0),
                deadband_c: Some(2.0),
                max_tank_temp_c: Some(90.0),
                heating_capacity_w: Some(4_500.0),
                upper_element_power_w: None,
                lower_element_power_w: None,
                element_priority_mode: None,
                max_setpoint_ramp_rate_c_per_min: None,
                mains_temp_c: Some(15.0),
                avg_water_draw_l_per_day: None,
                draw_flow_rate_kg_s: Some(0.0),
                draw_flow_rate_schedule_col: None,
                mains_temp_schedule_col: None,
                zip_z: None,
                zip_i: None,
                zip_p: None,
                zip_zq: None,
                zip_iq: None,
                zip_pq: None,
                zip_pf: None,
                zip_v0: None,
            },
        ),
        "Tankless Water Heater" => typed_alias_config(
            class,
            TanklessWaterHeaterConfig {
                equipment_id: None,
                zone_id: Some(1),
                fuel_type: Some("Electric".to_string()),
                setpoint_c: Some(50.0),
                efficiency_factor: Some(0.95),
                performance_adjustment: Some(0.92),
                max_thermal_power_w: Some(12_000.0),
                parasitic_power_w: Some(5.0),
                inlet_temp_c: Some(15.0),
                avg_water_draw_l_per_day: None,
                draw_flow_rate_kg_s: Some(0.02),
                zip_z: None,
                zip_i: None,
                zip_p: None,
                zip_zq: None,
                zip_iq: None,
                zip_pq: None,
                zip_pf: None,
                zip_v0: None,
            },
        ),
        "Gas Tankless Water Heater" => typed_alias_config(
            class,
            TanklessWaterHeaterConfig {
                equipment_id: None,
                zone_id: Some(1),
                fuel_type: Some("Gas".to_string()),
                setpoint_c: Some(50.0),
                efficiency_factor: Some(0.82),
                performance_adjustment: Some(0.92),
                max_thermal_power_w: Some(20_000.0),
                parasitic_power_w: Some(5.0),
                inlet_temp_c: Some(15.0),
                avg_water_draw_l_per_day: None,
                draw_flow_rate_kg_s: Some(0.02),
                zip_z: None,
                zip_i: None,
                zip_p: None,
                zip_zq: None,
                zip_iq: None,
                zip_pq: None,
                zip_pf: None,
                zip_v0: None,
            },
        ),
        "Heat Pump Water Heater" | "HPWH" => typed_alias_config(
            class,
            HeatPumpWaterHeaterConfig {
                equipment_id: None,
                zone_id: Some(1),
                tank_volume_m3: None,
                tank_height_m: None,
                tank_diameter_m: None,
                ua_w_per_k: None,
                jacket_r_value_m2_k_w: None,
                tank_nodes: None,
                thermostat_node: None,
                thermostat_upper_node: None,
                condenser_node: None,
                setpoint_c: Some(52.0),
                deadband_c: Some(2.0),
                max_tank_temp_c: Some(90.0),
                max_setpoint_ramp_rate_c_per_min: None,
                compressor_power_w: Some(500.0),
                backup_element_power_w: Some(4_500.0),
                backup_enable_offset_c: None,
                backup_efficiency: None,
                hp_only_mode: None,
                cop: Some(2.5),
                uniform_energy_factor: None,
                cop_curve_coeffs: None,
                capacity_curve_coeffs: None,
                min_ambient_temp_c: None,
                max_ambient_temp_c: None,
                low_power_hpwh: None,
                shr: None,
                lost_heat_fraction: None,
                wall_heat_fraction: None,
                fan_power_w: None,
                parasitic_power_w: None,
                min_on_time_s: None,
                min_off_time_s: None,
                tempering_valve_setpoint_c: None,
                element_hp_control_mode: None,
                mains_temp_c: Some(15.0),
                avg_water_draw_l_per_day: None,
                draw_flow_rate_kg_s: Some(0.0),
                zip_z: None,
                zip_i: None,
                zip_p: None,
                zip_zq: None,
                zip_iq: None,
                zip_pq: None,
                zip_pf: None,
                zip_v0: None,
            },
        ),
        "Battery" => config_with_floats(
            class,
            class,
            &[
                ("capacity_kwh", 13.5),
                ("max_charge_kw", 5.0),
                ("max_discharge_kw", 5.0),
                ("initial_soc", 0.5),
                ("min_soc", 0.15),
                ("max_soc", 0.95),
            ],
        ),
        "PV" => config_with_floats(
            class,
            class,
            &[
                ("capacity_kw", 5.0),
                ("tilt_deg", 0.0),
                ("azimuth_deg", 0.0),
                ("surface_resolution_deg", 360.0),
                ("power_factor", 1.0),
                ("inverter_efficiency", 0.96),
                ("inverter_capacity_kw", 5.0),
            ],
        ),
        "EV" | "Electric Vehicle" => typed_alias_config(
            class,
            EvConfig {
                equipment_id: None,
                capacity_kwh: 60.0,
                charging_level: Some("L2".to_string()),
                max_charging_power_kw: 7.2,
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
                v2l_enabled: None,
                v2l_soc_reserve: None,
                v2l_max_discharge_kw: None,
                v2g_enabled: None,
                v2g_soc_reserve: None,
                v2g_max_discharge_kw: None,
                chemistry: None,
                fuel_economy_kwh_per_mi: None,
                ready_soc: None,
                charging_strategy: None,
                plug_in_policy: None,
                power_limit_kw: None,
                initial_connection_state: None,
            },
        ),
        "Scheduled EV" => config_mixed(
            class,
            class,
            &[
                ("zone_id", 1.0),
                ("power_constant_kw", 3.6),
                ("sensible_gain_fraction", 0.0),
                ("latent_gain_fraction", 0.0),
            ],
            &[("power_schedule_source", "constant")],
        ),
        "Gas Generator" | "Gas Fuel Cell" => typed_alias_config(
            class,
            GeneratorConfig {
                equipment_id: None,
                fuel_type: None,
                rated_power_kw: 6.0,
                eta_electric: Some(0.3),
                eta_thermal: Some(0.0),
                efficiency_type: None,
                delta_kw_per_s: Some(1.0),
                capacity_min_kw: Some(0.0),
                grid_import_limit_kw: Some(0.0),
                export_limit_kw: Some(0.0),
                loop_id: None,
                flow_rate_kg_s: None,
                supply_temp_c: None,
                return_temp_c: None,
            },
        ),
        "HRV" => typed_alias_config(
            class,
            VentilationConfig {
                equipment_id: None,
                zone_id: None,
                flow_rate_m3_s: 0.03,
                fan_power_w: Some(40.0),
                sensible_effectiveness: Some(0.7),
                latent_effectiveness: Some(0.0),
                bypass_temp_min_c: None,
                bypass_temp_max_c: None,
                defrost_temp_c: None,
                defrost_effectiveness_fraction: None,
                ventilation_type: Some("hrv".to_string()),
                balanced: None,
                hours_in_operation: None,
                schedule_source: Some("constant".to_string()),
                schedule_constant: Some(1.0),
            },
        ),
        "ERV" => typed_alias_config(
            class,
            VentilationConfig {
                equipment_id: None,
                zone_id: None,
                flow_rate_m3_s: 0.03,
                fan_power_w: Some(40.0),
                sensible_effectiveness: Some(0.7),
                latent_effectiveness: Some(0.5),
                bypass_temp_min_c: None,
                bypass_temp_max_c: None,
                defrost_temp_c: None,
                defrost_effectiveness_fraction: None,
                ventilation_type: Some("erv".to_string()),
                balanced: None,
                hours_in_operation: None,
                schedule_source: Some("constant".to_string()),
                schedule_constant: Some(1.0),
            },
        ),
        "Ventilation Fan" => typed_alias_config(
            class,
            VentilationConfig {
                equipment_id: None,
                zone_id: None,
                flow_rate_m3_s: 0.03,
                fan_power_w: Some(40.0),
                sensible_effectiveness: Some(0.0),
                latent_effectiveness: Some(0.0),
                bypass_temp_min_c: None,
                bypass_temp_max_c: None,
                defrost_temp_c: None,
                defrost_effectiveness_fraction: None,
                ventilation_type: Some("exhaust_fan".to_string()),
                balanced: None,
                hours_in_operation: None,
                schedule_source: Some("constant".to_string()),
                schedule_constant: Some(1.0),
            },
        ),
        "EventBasedLoad" | "Clothes Washer" | "Dishwasher" | "Clothes Dryer" | "Cooking Range" => {
            config_mixed(
                class,
                class,
                &[("zone_id", 1.0), ("active_power_kw", 1.0)],
                &[("event_window_source", "constant")],
            )
        }
        _ => config_mixed(
            class,
            class,
            &[
                ("zone_id", 1.0),
                ("power_constant_kw", 1.0),
                ("sensible_gain_fraction", 0.3),
                ("latent_gain_fraction", 0.1),
            ],
            &[("power_schedule_source", "constant")],
        ),
    }
}

fn env_for_class(class: &str) -> EnvironmentState {
    if class == "PV" {
        env_with_pv_surface()
    } else if matches!(
        class,
        "Gas Furnace"
            | "Electric Furnace"
            | "Electric Baseboard"
            | "Gas Boiler"
            | "Electric Boiler"
            | "Heat Pump Heater"
            | "ASHP Heater"
            | "MSHP Heater"
            | "Ideal HVAC"
    ) {
        env_with_zone_temp(18.0)
    } else if matches!(
        class,
        "Air Conditioner" | "Room AC" | "ASHP Cooler" | "MSHP Cooler"
    ) {
        env_with_zone_temp(30.0)
    } else {
        default_env()
    }
}

#[test]
fn lifecycle_resistance_water_heater() {
    let registry = EquipmentRegistry::new();
    let cfg = config_for_class("Resistance Water Heater");
    let mut eq = registry
        .create("Resistance Water Heater", cfg.clone())
        .expect("create resistance water heater");
    let env = env_for_class("Resistance Water Heater");
    eq.init(&cfg, &env).expect("init resistance water heater");
    assert_equipment_lifecycle(
        eq.as_mut(),
        &env,
        ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(52.0),
            cooling_setpoint_c: None,
            deadband_c: Some(2.0),
        },
        ControlSignal::PowerSetpoint {
            active_power_kw: 1.0,
            reactive_power_kvar: None,
        },
    );
}

#[test]
fn lifecycle_gas_water_heater() {
    let registry = EquipmentRegistry::new();
    let cfg = config_for_class("Gas Water Heater");
    let mut eq = registry
        .create("Gas Water Heater", cfg.clone())
        .expect("create gas water heater");
    let env = env_for_class("Gas Water Heater");
    eq.init(&cfg, &env).expect("init gas water heater");
    assert_equipment_lifecycle(
        eq.as_mut(),
        &env,
        ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(52.0),
            cooling_setpoint_c: None,
            deadband_c: Some(2.0),
        },
        ControlSignal::PowerSetpoint {
            active_power_kw: 1.0,
            reactive_power_kvar: None,
        },
    );
}

#[test]
fn lifecycle_heat_pump_water_heater() {
    let registry = EquipmentRegistry::new();
    let cfg = config_for_class("Heat Pump Water Heater");
    let mut eq = registry
        .create("Heat Pump Water Heater", cfg.clone())
        .expect("create hpwh");
    let env = env_for_class("Heat Pump Water Heater");
    eq.init(&cfg, &env).expect("init hpwh");
    assert_equipment_lifecycle(
        eq.as_mut(),
        &env,
        ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(52.0),
            cooling_setpoint_c: None,
            deadband_c: Some(2.0),
        },
        ControlSignal::PowerSetpoint {
            active_power_kw: 1.0,
            reactive_power_kvar: None,
        },
    );
}

#[test]
fn lifecycle_tankless_water_heater() {
    let registry = EquipmentRegistry::new();
    let cfg = config_for_class("Tankless Water Heater");
    let mut eq = registry
        .create("Tankless Water Heater", cfg.clone())
        .expect("create tankless");
    let env = env_for_class("Tankless Water Heater");
    eq.init(&cfg, &env).expect("init tankless");
    assert_equipment_lifecycle(
        eq.as_mut(),
        &env,
        ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(50.0),
            cooling_setpoint_c: None,
            deadband_c: Some(2.0),
        },
        ControlSignal::PowerSetpoint {
            active_power_kw: 1.0,
            reactive_power_kvar: None,
        },
    );
}

#[test]
fn all_registered_equipment_core_output_matches_capabilities_and_ports() {
    let registry = EquipmentRegistry::new();
    for class in registry.known_names() {
        let cfg = config_for_class(class);
        let env = env_for_class(class);
        let mut eq = registry
            .create(class, cfg.clone())
            .unwrap_or_else(|e| panic!("create failed for '{class}': {e}"));
        eq.init(&cfg, &env)
            .unwrap_or_else(|e| panic!("init failed for '{class}': {e}"));

        let mut ports = ports_for(eq.as_ref());
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports)
            .unwrap_or_else(|e| panic!("step failed for '{class}': {e}"));

        let desc = eq.descriptor();
        let co = eq.core_output();
        validate_core_contract(desc, co)
            .unwrap_or_else(|e| panic!("core contract failed for '{class}': {e}"));

        if desc.core_capabilities.contains(CoreCapabilities::ELECTRIC) {
            let core_kw = co
                .flows
                .electric_kw
                .expect("validated electric capability must have core electric output")
                .net_consumption_kw();
            let port_kw = ports.electrical.net_active_kw();
            assert!(
                (core_kw - port_kw).abs() < 1e-6,
                "core_output vs electrical ports mismatch for '{class}': core={core_kw}, ports={port_kw}"
            );
        }
    }
}

#[test]
fn failed_step_does_not_update_cached_core_output() {
    let registry = EquipmentRegistry::new();
    let cfg = config_for_class("PV");
    let mut eq = registry.create("PV", cfg.clone()).expect("create pv");
    let good_env = env_with_pv_surface();
    eq.init(&cfg, &good_env).expect("init pv");

    let mut ports = ports_for(eq.as_ref());
    eq.update_control(&good_env);
    eq.step(&good_env, Duration::from_secs(60), &mut ports)
        .expect("first step must succeed");
    let before_failure = eq.core_output().clone();

    let mut bad_env = good_env.clone();
    bad_env.weather.solar_irradiance.clear();
    let step_result = eq.step(&bad_env, Duration::from_secs(60), &mut ports);
    assert!(step_result.is_err(), "invalid env step must fail");
    assert_eq!(
        *eq.core_output(),
        before_failure,
        "failed step must not update cached core_output"
    );
}

#[test]
fn operating_mode_numeric_codes_are_stable() {
    assert_eq!(OperatingMode::Off as u8, 0);
    assert_eq!(OperatingMode::Heating as u8, 1);
    assert_eq!(OperatingMode::Cooling as u8, 2);
    assert_eq!(OperatingMode::Defrost as u8, 3);
    assert_eq!(OperatingMode::Standby as u8, 4);
    assert_eq!(OperatingMode::Charging as u8, 5);
    assert_eq!(OperatingMode::Discharging as u8, 6);
    assert_eq!(OperatingMode::HeatingHP as u8, 7);
    assert_eq!(OperatingMode::HeatingER as u8, 8);
    assert_eq!(OperatingMode::HeatingHPAndER as u8, 9);
    assert_eq!(OperatingMode::HeatPumpWH as u8, 10);
    assert_eq!(OperatingMode::BackupElement as u8, 11);
}

#[test]
fn pv_capacitive_reactive_output_is_negative_in_core_output() {
    let registry = EquipmentRegistry::new();
    let cfg = config_for_class("PV");
    let mut eq = registry.create("PV", cfg.clone()).expect("create pv");
    let env = env_with_pv_surface();
    eq.init(&cfg, &env).expect("init pv");

    eq.apply_control(&ControlSignal::ReactiveSetpoint { kvar: -1.0 })
        .expect("reactive setpoint accepted");
    let mut ports = ports_for(eq.as_ref());
    eq.step(&env, Duration::from_secs(60), &mut ports)
        .expect("pv step");

    let q = eq
        .core_output()
        .flows
        .reactive_power_kvar
        .expect("pv declares reactive capability");
    assert!(
        q < 0.0,
        "capacitive setpoint must produce negative core reactive_power_kvar; got {q}"
    );
}
