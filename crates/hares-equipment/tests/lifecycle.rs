//! Generic equipment lifecycle integration tests.
//!
//! Tests the cross-equipment contract: every Equipment implementor must
//! satisfy the full lifecycle — init, apply_control, step, telemetry,
//! save_state, load_state — through the Equipment trait boundary, not via
//! type-specific internals.
//!
//! Phase 2 gate: grows as each equipment type lands. Per HARES-061, each new
//! equipment ticket must add a `test_equipment_lifecycle` call here.
//!
//! NOTE: Type-specific physics assertions (setpoint tracking, efficiency curves,
//! etc.) live in the per-equipment test files. This harness only tests the
//! trait contract and the cross-layer port accumulation pipeline.

mod common;

use std::collections::HashMap;
use std::time::Duration;

use hares_equipment::{Equipment, EquipmentConfig, EquipmentRegistry, config::ConfigValue};
use hares_types::{ControlSignal, EnvironmentState, FuelType, PortSlots, ZoneId};

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
    assert!(!desc.name.is_empty(), "descriptor.name must be non-empty after init");
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
    equipment
        .update_control(env);
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
        actual_field_count, expected_field_count,
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

    // 7. Capture pre-mutation telemetry, step again to mutate state.
    let telemetry_before = equipment.telemetry().clone();
    let mut ports2 = ports_for(equipment);
    equipment.update_control(env);
    equipment.step(env, dt, &mut ports2).expect("second step must return Ok");

    // 8. load_state from snapshot must restore telemetry to pre-mutation values.
    equipment
        .load_state(&saved)
        .expect("load_state must return Ok for valid snapshot bytes");

    let telemetry_restored = equipment.telemetry();
    for field in &equipment.descriptor().telemetry_fields {
        let before_val = telemetry_before.get(&field.name);
        let after_val = telemetry_restored.get(&field.name);
        assert_eq!(
            before_val, after_val,
            "telemetry field '{}' must match pre-mutation value after load_state for '{}': \
             before={:?}, after={:?}",
            field.name,
            equipment.descriptor().name,
            before_val,
            after_val,
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
    EquipmentConfig { name: name.to_string(), ochre_class: class.to_string(), raw_config: raw }
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
    EquipmentConfig { name: name.to_string(), ochre_class: class.to_string(), raw_config: raw }
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
    let cfg = config_with_floats(
        "Gas Furnace",
        "Gas Furnace",
        &[
            ("zone_id", 1.0),
            ("capacity_w", 10_000.0),
            ("heating_setpoint_c", 21.0),
            ("cooling_setpoint_c", 27.0),
        ],
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
    eq.step(&env, Duration::from_secs(60), &mut ports).expect("step");

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
