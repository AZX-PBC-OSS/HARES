//! Generic equipment lifecycle integration tests.
//!
//! Tests the cross-equipment contract: every Equipment implementor must
//! satisfy the full lifecycle -- init, apply_control, step, telemetry,
//! save_state, load_state -- through the Equipment trait boundary, not via
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
    BatteryConfig, CentralAirConditionerConfig, DefrostConfig, DehumidifierConfig, DuctConfig,
    ElectricBaseboardConfig, ElectricBoilerConfig, ElectricFurnaceConfig, Equipment,
    EquipmentConfig, EquipmentRegistry, EquipmentTypedConfig, EvConfig, GasBoilerConfig,
    GasFurnaceConfig, GeneratorConfig, HeatPumpCommonConfig, HeatPumpCoolerConfig,
    HeatPumpHeaterConfig, HvacSetpointConfig, IdealHvacConfig, IndirectTankConfig,
    ProtocolBridgeConfig, PvConfig, RoomAcConfig, VentilationConfig,
    config::ConfigValue,
    water_heater::wh_config::{
        ElectricResistanceWaterHeaterConfig, GasWaterHeaterConfig, HeatPumpWaterHeaterConfig,
        TanklessWaterHeaterConfig,
    },
};
use hares_types::{
    ControlSignal, CoreCapabilities, CoreOutput, EnvironmentState, EquipmentDescriptor, FuelType,
    OperatingMode, PortSlots, SurfaceIrradiance, ZoneId, telemetry_keys as tk,
    validate_core_contract,
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
    assert_core_output_contract(equipment.descriptor(), equipment.core_output());

    // Verify at least one port domain has a non-zero accumulation.
    // Equipment may contribute to thermal, electrical, or fuel -- at least one must be non-zero.
    let has_thermal = ports
        .thermal
        .iter()
        .any(|a| a.sensible_gain_w.abs() > 0.0 || a.latent_gain_w.abs() > 0.0);
    let has_electrical = ports.electrical.load_power_w.abs() > 0.0
        || ports.electrical.generation_power_w.abs() > 0.0;
    let gas_w = ports.fuel.get(FuelType::Gas);
    let has_fuel = gas_w.abs() > 0.0 || ports.fuel.get(FuelType::Propane).abs() > 0.0;

    assert!(
        has_thermal || has_electrical || has_fuel,
        "step() for '{}' must write at least one port contribution; \
         thermal={}, electrical_load={}, electrical_gen={}, gas={gas_w}",
        equipment.descriptor().name,
        ports.thermal.iter().map(|a| a.sensible_gain_w).sum::<f64>(),
        ports.electrical.load_power_w,
        ports.electrical.generation_power_w,
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
    let saved = equipment.save_state().unwrap();
    assert!(
        !saved.is_empty(),
        "save_state must return non-empty bytes for '{}'",
        equipment.descriptor().name
    );

    // 7. Capture the reference outputs from the second step, which should be
    //    reproduced exactly after restoring the first-step snapshot and stepping again.

    let mut ports2 = ports_for(equipment);
    equipment.update_control(env);
    equipment
        .step(env, dt, &mut ports2)
        .expect("second step must return Ok");
    let port_output_step2 = ports2.clone();
    let core_output_step2 = equipment.core_output().clone();

    // 8. Restore from snapshot and step again. Output must match step 1.
    equipment
        .load_state(&saved)
        .expect("load_state must return Ok for valid snapshot bytes");
    // After load_state(), core_output is reconstructed from restored internal
    // state (not reset to default).  Equipment types that carry dynamic state
    // (EV, Battery, ScheduledLoad) populate core_output from checkpointed
    // fields so that snapshot_equipment_state() captures correct values for
    // actor reads on the first post-restore step.

    let mut ports_after_restore = ports_for(equipment);
    equipment.update_control(env);
    equipment
        .step(env, dt, &mut ports_after_restore)
        .expect("step after restore must return Ok");
    assert_core_output_contract(equipment.descriptor(), equipment.core_output());
    assert_eq!(
        equipment.core_output(),
        &core_output_step2,
        "step after restore must deterministically recompute cached core_output for '{}'",
        equipment.descriptor().name
    );

    // Same restored state + same env → same deterministic port output.
    let orig_net = port_output_step2.electrical.net_active_w();
    let rest_net = ports_after_restore.electrical.net_active_w();
    assert!(
        (orig_net - rest_net).abs() < 1.0,
        "electrical net_active_w after restore must match step-2 output within 1 W for '{}': \
         step2={orig_net}, after_restore={rest_net}",
        equipment.descriptor().name,
    );
    for (orig_acc, rest_acc) in port_output_step2
        .thermal
        .iter()
        .zip(ports_after_restore.thermal.iter())
    {
        assert!(
            (orig_acc.sensible_gain_w - rest_acc.sensible_gain_w).abs() < 0.05,
            "sensible_gain_w for zone {:?} after restore must match step-2 for '{}': \
             step2={}, after_restore={}",
            orig_acc.zone,
            equipment.descriptor().name,
            orig_acc.sensible_gain_w,
            rest_acc.sensible_gain_w,
        );
    }
}

fn assert_core_output_contract(desc: &EquipmentDescriptor, co: &CoreOutput) {
    validate_core_contract(desc, co)
        .unwrap_or_else(|e| panic!("core contract failed for '{}': {e}", desc.name));

    let caps = desc.core_capabilities;
    assert_eq!(
        co.flows.electric_kw.is_some(),
        caps.contains(CoreCapabilities::ELECTRIC),
        "electric core output presence must match capabilities for '{}'",
        desc.name
    );
    assert_eq!(
        co.flows.reactive_power_kvar.is_some(),
        caps.contains(CoreCapabilities::REACTIVE),
        "reactive core output presence must match capabilities for '{}'",
        desc.name
    );
    assert_eq!(
        co.flows.fuel_w.is_some(),
        caps.contains(CoreCapabilities::FUEL),
        "fuel core output presence must match capabilities for '{}'",
        desc.name
    );
    assert_eq!(
        co.state.soc.is_some(),
        caps.contains(CoreCapabilities::HAS_SOC),
        "soc core output presence must match capabilities for '{}'",
        desc.name
    );
    assert_eq!(
        co.state.operating_mode.is_some(),
        caps.contains(CoreCapabilities::HAS_MODE),
        "operating_mode core output presence must match capabilities for '{}'",
        desc.name
    );
    assert_eq!(
        co.flows.thermal_output_w.is_some(),
        caps.contains(CoreCapabilities::THERMAL),
        "thermal_output_w core output presence must match capabilities for '{}'",
        desc.name
    );
    assert_eq!(
        co.state.speed_index.is_some(),
        caps.contains(CoreCapabilities::HAS_SPEED),
        "speed_index core output presence must match capabilities for '{}'",
        desc.name
    );
    assert_eq!(
        co.state.setpoint_c.is_some(),
        caps.contains(CoreCapabilities::HAS_SETPOINT),
        "setpoint_c core output presence must match capabilities for '{}'",
        desc.name
    );
    assert_eq!(
        co.performance.cop.is_some(),
        caps.contains(CoreCapabilities::HAS_COP),
        "cop core output presence must match capabilities for '{}'",
        desc.name
    );
}

// ---------------------------------------------------------------------------
// Configuration helpers
// ---------------------------------------------------------------------------

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
    EquipmentConfig::raw(name.to_string(), class.to_string(), raw)
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
    let cfg = typed_alias_config(
        "Battery",
        BatteryConfig {
            equipment_id: None,
            zone_id: Some(1),
            capacity_kwh: 13.5,
            max_charge_kw: 5.0,
            max_discharge_kw: 5.0,
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
            heater_on_discharge: None,
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
            min_dwell_steps: 0,
        },
    );

    let mut eq = registry
        .create("Battery", cfg.clone())
        .expect("registry must create Battery");
    let env = default_env();
    eq.init(&cfg, &env).expect("init must succeed");

    let valid = ControlSignal::PowerSetpoint {
        active_power_kw: 2.0,
        reactive_power_kvar: None,
        min_soc: None,
        max_soc: None,
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
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            setpoint: HvacSetpointConfig::default(),
        },
    )
    .unwrap();

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
        min_soc: None,
        max_soc: None,
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
        ports.electrical.load_power_w > 0.0,
        "electrical load must be positive; got {}",
        ports.electrical.load_power_w
    );
    assert!(
        (ports.electrical.load_power_w - 1_500.0).abs() < 1.0,
        "electrical load must be ~1_500 W; got {}",
        ports.electrical.load_power_w
    );
}

#[test]
fn scheduled_load_grid_outage_keeps_reactive_core_output_present_when_configured() {
    let registry = EquipmentRegistry::new();
    let cfg = config_mixed(
        "Reactive Plug Loads",
        "Plug Loads",
        &[
            ("zone_id", 1.0),
            ("sensible_gain_fraction", 0.0),
            ("power_constant_kw", 1.5),
            ("zip_zq", 0.2),
            ("zip_iq", 0.3),
            ("zip_pq", 0.5),
            ("zip_pf", 0.9),
        ],
        &[("power_schedule_source", "constant")],
    );

    let mut eq = registry
        .create("Plug Loads", cfg.clone())
        .expect("registry must create Plug Loads");
    let mut env = env_with_zone_temp(21.0);
    env.grid.voltage_pu = 0.0;
    eq.init(&cfg, &env).expect("init must succeed");

    let mut ports = ports_for(eq.as_ref());
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports)
        .expect("step must succeed under outage");

    let co = eq.core_output();
    assert_core_output_contract(eq.descriptor(), co);
    assert_eq!(
        co.flows.reactive_power_kvar,
        Some(0.0),
        "reactive core output must remain present and zeroed when configured"
    );
}

fn typed_alias_config<T: EquipmentTypedConfig>(name: &str, cfg: T) -> EquipmentConfig {
    let mut out =
        EquipmentConfig::from_typed(name.to_string(), T::equipment_type_name().to_string(), cfg)
            .unwrap();
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
                stage_heating_capacities_w: None,
                stage_heating_eirs: None,
                setpoint: HvacSetpointConfig::default(),
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
                setpoint: HvacSetpointConfig::default(),
            },
        ),
        "Electric Baseboard" => typed_alias_config(
            class,
            ElectricBaseboardConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 2_000.0,
                eir: 1.0,
                setpoint: HvacSetpointConfig::default(),
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
                return_temp_c: 70.0,
                fluid_type: hares_types::FluidType::Water,
                setpoint: HvacSetpointConfig::default(),
                condensing: false,
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
                return_temp_c: 70.0,
                fluid_type: hares_types::FluidType::Water,
                setpoint: HvacSetpointConfig::default(),
            },
        ),
        "Air Conditioner" => typed_alias_config(
            class,
            CentralAirConditionerConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 8_000.0,
                eir: 3.412_141_633 / 14.0,
                shr: Some(0.75),
                number_of_speeds: 1,
                stage_capacities_w: None,
                stage_eirs: None,
                stage_shrs: None,
                fan_power_w: Some(0.0),
                fan_power_w_per_cfm: None,
                setpoint: HvacSetpointConfig::default(),
                hysteresis_c: None,
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
                charge_defect_ratio: None,
                min_oat_compressor_cooling_c: None,
            },
        ),
        "Room AC" => typed_alias_config(
            class,
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
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
                crankcase_heater_kw: None,
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
            },
        ),
        "Heat Pump Heater" | "ASHP Heater" | "MSHP Heater" | "GSHP Heater" | "WSHP Heater" => {
            typed_alias_config(
                class,
                HeatPumpHeaterConfig {
                    common: HeatPumpCommonConfig {
                        equipment_id: None,
                        zone_id: Some(1),
                        heating_capacity_w: Some(8_000.0),
                        heating_eir: Some(3.412_141_633 / 9.0),
                        stage_heating_capacities_w: None,
                        stage_heating_eirs: None,
                        backup_fuel: None,
                        backup_capacity_w: Some(5_000.0),
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
                        setpoint: HvacSetpointConfig::default(),
                        hysteresis_c: None,
                        duct: DuctConfig::default(),
                        biquadratic_x1_min: None,
                        biquadratic_x1_max: None,
                        biquadratic_x2_min: None,
                        biquadratic_x2_max: None,
                        ff_min: None,
                        ff_max: None,
                        plf_min: None,
                        plf_max: None,
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
                },
            )
        }
        "ASHP Cooler" | "MSHP Cooler" | "GSHP Cooler" | "WSHP Cooler" => typed_alias_config(
            class,
            HeatPumpCoolerConfig {
                common: HeatPumpCommonConfig {
                    equipment_id: None,
                    zone_id: Some(1),
                    heating_capacity_w: Some(8_000.0),
                    heating_eir: Some(3.412_141_633 / 9.0),
                    stage_heating_capacities_w: None,
                    stage_heating_eirs: None,
                    backup_fuel: None,
                    backup_capacity_w: None,
                    backup_eir: None,
                    fraction_heating_load_served: Some(1.0),
                    cooling_capacity_w: Some(8_000.0),
                    cooling_eir: Some(3.412_141_633 / 14.0),
                    stage_cooling_capacities_w: None,
                    stage_cooling_eirs: None,
                    fraction_cooling_load_served: Some(1.0),
                    number_of_speeds: 1,
                    is_mini_split: class == "MSHP Cooler",
                    shr: Some(0.75),
                    fan_power_w: Some(0.0),
                    fan_power_w_per_cfm: None,
                    airflow_m3_s_per_w: None,
                    setpoint: HvacSetpointConfig::default(),
                    hysteresis_c: None,
                    duct: DuctConfig::default(),
                    biquadratic_x1_min: None,
                    biquadratic_x1_max: None,
                    biquadratic_x2_min: None,
                    biquadratic_x2_max: None,
                    ff_min: None,
                    ff_max: None,
                    plf_min: None,
                    plf_max: None,
                    min_compressor_fraction: 0.25,
                    eir_part_load_benefit: None,
                    er_stages: 1,
                    charge_defect_ratio: None,
                    ..Default::default()
                },
                stage_shrs: None,
                crankcase_heater_kw: None,
                crankcase_heater_threshold_c: None,
                min_oat_cooling_c: 10.0,
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
                loop_id: None,
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
                loop_id: None,
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
        "Tankless Water Heater" => typed_alias_config(
            class,
            TanklessWaterHeaterConfig {
                equipment_id: None,
                zone_id: Some(1),
                loop_id: None,
                fuel_type: FuelType::Electric,
                energy_factor: Some(0.95),
                uniform_energy_factor: None,
                heating_capacity_w: Some(12_000.0),
                setpoint_c: Some(50.0),
                parasitic_power_w: Some(5.0),
                performance_adjustment: Some(0.92),
                inlet_temp_c: None,
                draw_flow_rate_kg_s: Some(0.2),
                draw_flow_rate_source: None,
                mains_temp_c_source: None,
                avg_water_draw_l_per_day: None,
                zone_type: None,
            },
        ),
        "Gas Tankless Water Heater" => typed_alias_config(
            class,
            TanklessWaterHeaterConfig {
                equipment_id: None,
                zone_id: Some(1),
                loop_id: None,
                fuel_type: FuelType::Gas,
                energy_factor: Some(0.82),
                uniform_energy_factor: None,
                heating_capacity_w: Some(20_000.0),
                setpoint_c: Some(50.0),
                parasitic_power_w: Some(5.0),
                performance_adjustment: Some(0.92),
                inlet_temp_c: None,
                draw_flow_rate_kg_s: Some(0.2),
                draw_flow_rate_source: None,
                mains_temp_c_source: None,
                avg_water_draw_l_per_day: None,
                zone_type: None,
            },
        ),
        "Heat Pump Water Heater" | "HPWH" => typed_alias_config(
            class,
            HeatPumpWaterHeaterConfig {
                equipment_id: None,
                zone_id: Some(1),
                loop_id: None,
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
                performance_adjustment: None,
                zone_type: None,
                first_hour_rating_m3: None,
                jacket_r_value_m2_k_w: None,
                fixture_delivery_temp_c: None,
            },
        ),
        "Indirect Tank" => typed_alias_config(
            class,
            IndirectTankConfig {
                equipment_id: None,
                zone_id: Some(1),
                boiler_loop_id: None,
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
            BatteryConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_kwh: 13.5,
                max_charge_kw: 5.0,
                max_discharge_kw: 5.0,
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
                heater_on_discharge: None,
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
                min_dwell_steps: 0,
            },
        ),
        "PV" => typed_alias_config(
            class,
            PvConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_kw: 5.0,
                tilt_deg: Some(0.0),
                azimuth_deg: Some(0.0),
                module_type: None,
                noct_c: None,
                array_type: None,
                system_losses_fraction: None,
                inverter_efficiency: Some(0.96),
                inverter_capacity_kw: Some(5.0),
                power_factor: Some(1.0),
                surface_resolution_deg: Some(360.0),
                sam_lut_path: None,
                arrays: None,
            },
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
                zone_id: None,
                fuel_type: None,
                rated_power_kw: 6.0,
                eta_electric: Some(0.3),
                eta_thermal: Some(0.0),
                eta_jacket_water: None,
                eta_lube_oil: None,
                eta_exhaust: None,
                efficiency_curve_points: None,
                efficiency_type: None,
                delta_kw_per_s: Some(1.0),
                capacity_min_kw: Some(0.0),
                grid_import_limit_kw: Some(0.0),
                export_limit_kw: Some(0.0),
                loop_id: None,
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
            },
        ),
        "HRV" => typed_alias_config(
            class,
            VentilationConfig {
                equipment_id: None,
                zone_id: None,
                flow_rate_m3_s: 0.03,
                fan_power_w: Some(40.0),
                supply_fan_power_w: None,
                exhaust_fan_power_w: None,
                sensible_effectiveness: Some(0.7),
                latent_effectiveness: Some(0.0),
                bypass_temp_min_c: None,
                bypass_temp_max_c: None,
                defrost_temp_c: None,
                defrost_effectiveness_fraction: None,
                ventilation_type: Some("hrv".to_string()),
                balanced: None,
                hours_in_operation: None,
            },
        ),
        "ERV" => typed_alias_config(
            class,
            VentilationConfig {
                equipment_id: None,
                zone_id: None,
                flow_rate_m3_s: 0.03,
                fan_power_w: Some(40.0),
                supply_fan_power_w: None,
                exhaust_fan_power_w: None,
                sensible_effectiveness: Some(0.7),
                latent_effectiveness: Some(0.5),
                bypass_temp_min_c: None,
                bypass_temp_max_c: None,
                defrost_temp_c: None,
                defrost_effectiveness_fraction: None,
                ventilation_type: Some("erv".to_string()),
                balanced: None,
                hours_in_operation: None,
            },
        ),
        "Ventilation Fan" => typed_alias_config(
            class,
            VentilationConfig {
                equipment_id: None,
                zone_id: None,
                flow_rate_m3_s: 0.03,
                fan_power_w: Some(40.0),
                supply_fan_power_w: None,
                exhaust_fan_power_w: None,
                sensible_effectiveness: Some(0.0),
                latent_effectiveness: Some(0.0),
                bypass_temp_min_c: None,
                bypass_temp_max_c: None,
                defrost_temp_c: None,
                defrost_effectiveness_fraction: None,
                ventilation_type: Some("exhaust_fan".to_string()),
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
        "EventBasedLoad" | "Clothes Washer" | "Dishwasher" | "Clothes Dryer" | "Cooking Range" => {
            config_mixed(
                class,
                class,
                &[
                    ("zone_id", 1.0),
                    ("active_power_kw", 1.0),
                    ("sensible_gain_fraction", 0.0),
                ],
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
            min_soc: None,
            max_soc: None,
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
            min_soc: None,
            max_soc: None,
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
            min_soc: None,
            max_soc: None,
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
            min_soc: None,
            max_soc: None,
        },
    );
}

#[test]
fn lifecycle_gas_tankless_water_heater() {
    let registry = EquipmentRegistry::new();
    let cfg = config_for_class("Gas Tankless Water Heater");
    let mut eq = registry
        .create("Gas Tankless Water Heater", cfg.clone())
        .expect("create gas tankless");
    let env = env_for_class("Gas Tankless Water Heater");
    eq.init(&cfg, &env).expect("init gas tankless");
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
            min_soc: None,
            max_soc: None,
        },
    );
}

#[test]
fn lifecycle_indirect_tank() {
    let registry = EquipmentRegistry::new();
    let cfg = config_for_class("Indirect Tank");
    let mut eq = registry
        .create("Indirect Tank", cfg.clone())
        .expect("create indirect tank");
    let env = env_for_class("Indirect Tank");
    eq.init(&cfg, &env).expect("init indirect tank");
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
            min_soc: None,
            max_soc: None,
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
        assert_core_output_contract(desc, co);

        if desc.core_capabilities.contains(CoreCapabilities::ELECTRIC) {
            let core_kw = co
                .flows
                .electric_kw
                .expect("validated electric capability must have core electric output")
                .net_consumption_kw();
            let port_w = ports.electrical.net_active_w();
            assert!(
                (core_kw - port_w / 1_000.0).abs() < 0.001,
                "core_output vs electrical ports mismatch for '{class}': core={core_kw}, ports={port_w}"
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

// ---------------------------------------------------------------------------
// Ground-source heat pump pump-power integration tests
// ---------------------------------------------------------------------------

/// GSHP Heater pump power appears in telemetry and electrical port when the
/// compressor is active (heating call with zone temp below setpoint).
#[test]
fn gshp_heater_pump_power_in_telemetry_and_ports() {
    let registry = EquipmentRegistry::new();
    let typed = HeatPumpHeaterConfig {
        common: HeatPumpCommonConfig {
            zone_id: Some(1),
            heating_capacity_w: Some(8_000.0),
            heating_eir: Some(3.412_141_633 / 9.0),
            cooling_capacity_w: Some(8_000.0),
            cooling_eir: Some(3.412_141_633 / 14.0),
            shr: Some(0.75),
            number_of_speeds: 1,
            fan_power_w: Some(0.0),
            fraction_heating_load_served: Some(1.0),
            fraction_cooling_load_served: Some(1.0),
            pump_loop_depth_m: Some(60.0),
            pump_pipe_diameter_m: Some(0.025),
            pump_flow_rate_m3_per_s: Some(0.00019),
            pump_efficiency: Some(0.35),
            pump_motor_efficiency: Some(0.40),
            pump_system_head_loss_m: Some(3.0),
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
    };
    let mut cfg = EquipmentConfig::from_typed(
        "Test GSHP Heater".to_string(),
        "GSHP Heater".to_string(),
        typed,
    )
    .unwrap();
    cfg.ochre_class = "GSHP Heater".to_string();

    let mut eq = registry
        .create("GSHP Heater", cfg.clone())
        .expect("create GSHP Heater");
    let env = env_with_zone_temp(18.0);
    eq.init(&cfg, &env).expect("init GSHP Heater");

    eq.apply_control(&ControlSignal::ThermalSetpoint {
        heating_setpoint_c: Some(21.0),
        cooling_setpoint_c: Some(27.0),
        deadband_c: Some(1.0),
    })
    .expect("apply heating setpoint");

    let mut ports = ports_for(eq.as_ref());
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports)
        .expect("step GSHP Heater");

    let pump_telemetry = eq
        .telemetry()
        .get(tk::PUMP_POWER_KW)
        .expect("PUMP_POWER_KW must be present in telemetry");
    assert!(
        (0.04..=0.12).contains(&pump_telemetry),
        "GSHP heater pump power in telemetry must be 0.04–0.12 kW for typical ~60 m borehole; got {pump_telemetry:.4}"
    );
    assert!(
        ports.electrical.load_power_w >= pump_telemetry * 1_000.0,
        "electrical port load ({:.1} W) must be at least the pump contribution ({:.1} W)",
        ports.electrical.load_power_w,
        pump_telemetry * 1_000.0
    );
}

/// GSHP Cooler pump power appears in telemetry and electrical port when the
/// compressor is active (cooling call with zone temp above setpoint).
#[test]
fn gshp_cooler_pump_power_in_telemetry_and_ports() {
    let registry = EquipmentRegistry::new();
    let typed = HeatPumpCoolerConfig {
        common: HeatPumpCommonConfig {
            zone_id: Some(1),
            heating_capacity_w: Some(8_000.0),
            heating_eir: Some(3.412_141_633 / 9.0),
            cooling_capacity_w: Some(8_000.0),
            cooling_eir: Some(3.412_141_633 / 14.0),
            shr: Some(0.75),
            number_of_speeds: 1,
            fan_power_w: Some(0.0),
            fraction_heating_load_served: Some(1.0),
            fraction_cooling_load_served: Some(1.0),
            pump_loop_depth_m: Some(60.0),
            pump_pipe_diameter_m: Some(0.025),
            pump_flow_rate_m3_per_s: Some(0.00019),
            pump_efficiency: Some(0.35),
            pump_motor_efficiency: Some(0.40),
            pump_system_head_loss_m: Some(3.0),
            ..Default::default()
        },
        stage_shrs: None,
        crankcase_heater_kw: None,
        crankcase_heater_threshold_c: None,
        min_oat_cooling_c: 10.0,
    };
    let mut cfg = EquipmentConfig::from_typed(
        "Test GSHP Cooler".to_string(),
        "GSHP Cooler".to_string(),
        typed,
    )
    .unwrap();
    cfg.ochre_class = "GSHP Cooler".to_string();

    let mut eq = registry
        .create("GSHP Cooler", cfg.clone())
        .expect("create GSHP Cooler");
    let env = env_with_zone_temp(30.0);
    eq.init(&cfg, &env).expect("init GSHP Cooler");

    eq.apply_control(&ControlSignal::ThermalSetpoint {
        heating_setpoint_c: Some(21.0),
        cooling_setpoint_c: Some(24.0),
        deadband_c: Some(1.0),
    })
    .expect("apply cooling setpoint");

    let mut ports = ports_for(eq.as_ref());
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports)
        .expect("step GSHP Cooler");

    let pump_telemetry = eq
        .telemetry()
        .get(tk::PUMP_POWER_KW)
        .expect("PUMP_POWER_KW must be present in telemetry");
    assert!(
        (0.04..=0.12).contains(&pump_telemetry),
        "GSHP cooler pump power in telemetry must be 0.04–0.12 kW for typical ~60 m borehole; got {pump_telemetry:.4}"
    );
    assert!(
        ports.electrical.load_power_w >= pump_telemetry * 1_000.0,
        "electrical port load ({:.1} W) must be at least the pump contribution ({:.1} W)",
        ports.electrical.load_power_w,
        pump_telemetry * 1_000.0
    );
}
