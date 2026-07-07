//! Integration tests for grid-outage and islanding semantics at the
//! dwelling level.
//!
//! `Dwelling::set_grid_voltage(0.0)` signals a **utility** outage. The
//! dwelling resolves bus energization at the start of each timestep:
//! - no island-capable source → the bus is dead
//!   (`GridState::bus_energized() == false`) and every load force-offs at
//!   the control level;
//! - an island-capable source (battery with usable charge, generator,
//!   discharging V2G EV) → the bus is held at nominal
//!   (`GridState::island_bus_voltage_pu == Some(1.0)`) and loads keep
//!   running — battery-backed homes must NOT drop their loads.
//!
//! See docs/outage-behavior.md.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use hares_core::Dwelling;
use hares_equipment::battery::Battery;
use hares_equipment::config::ConfigValue;
use hares_equipment::scheduled_load::ScheduledLoad;
use hares_equipment::{BatteryConfig, Equipment, EquipmentConfig};
use hares_types::EndUse;

fn nanos_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos()
}

fn unique_temp_toml(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("hares-outage-island-{tag}-{}.toml", nanos_suffix()));
    path
}

fn write_minimal_toml(path: &PathBuf) {
    let content = r#"building_id = 9401

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res_s = 60
duration_s = 1200

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0
wall_area_m2 = 145.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "none"

[weather]
outdoor_temp_c = 20.0
dew_point_c = 10.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[schedule]
occupancy = 0.0

[output]
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
write_output = false
master_seed = 0
"#;
    fs::write(path, content).expect("failed to write synthetic TOML");
}

/// Dwelling with a 1.5 kW constant-power scheduled load.
fn build_dwelling_with_base_load(tag: &str) -> Dwelling {
    let toml_path = unique_temp_toml(tag);
    write_minimal_toml(&toml_path);
    let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build dwelling");
    let _ = fs::remove_file(&toml_path);

    let env = dwelling.latest_env().clone();
    let mut raw: HashMap<String, ConfigValue> = HashMap::new();
    raw.insert("power_schedule_source".to_string(), "constant".into());
    raw.insert("power_constant_kw".to_string(), 1.5.into());
    raw.insert("sensible_gain_fraction".to_string(), 0.5.into());
    let config = EquipmentConfig::raw("BaseLoad".to_string(), "ScheduledLoad".to_string(), raw);
    let mut eq = ScheduledLoad::new(config.clone(), EndUse::LIGHTING, "Lighting");
    eq.init(&config, &env).expect("init ScheduledLoad");
    dwelling.add_equipment(Box::new(eq));
    dwelling
}

fn add_battery(dwelling: &mut Dwelling, initial_soc: f64) {
    let cfg = BatteryConfig {
        equipment_id: None,
        zone_id: None,
        capacity_kwh: 10.0,
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
        min_soc: Some(0.1),
        max_soc: None,
        initial_soc: Some(initial_soc),
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
        inverter_capacity_kva: None,
        min_dwell_steps: 0,
    };
    let config =
        EquipmentConfig::from_typed("Battery".to_string(), "Battery".to_string(), cfg).unwrap();
    let env = dwelling.latest_env().clone();
    let mut bat = Battery::new(config.clone());
    bat.init(&config, &env).expect("init Battery");
    dwelling.add_equipment(Box::new(bat));
}

/// Utility outage without any backup source: the bus is dead and the
/// scheduled load force-offs — the meter reads (near) zero.
#[test]
fn outage_without_backup_deenergizes_bus_and_drops_loads() {
    let mut dwelling = build_dwelling_with_base_load("no-backup");

    // Nominal step: base load flows.
    dwelling.step().expect("nominal step");
    let summary = dwelling
        .save_checkpoint()
        .expect("checkpoint")
        .prior_electrical_summary;
    assert!(
        summary.base_load_kw > 1.0,
        "baseline base load must flow, got {}",
        summary.base_load_kw
    );

    // Utility outage.
    dwelling.set_grid_voltage(0.0);
    dwelling.step().expect("outage step");

    let grid = &dwelling.latest_env().grid;
    assert!(
        grid.grid_outage(),
        "voltage_pu 0.0 signals a utility outage"
    );
    assert!(
        !grid.bus_energized(),
        "no island source → the bus must be de-energized"
    );
    assert_eq!(grid.island_bus_voltage_pu, None);

    let summary = dwelling
        .save_checkpoint()
        .expect("checkpoint")
        .prior_electrical_summary;
    assert_eq!(
        summary.base_load_kw, 0.0,
        "dead bus: the scheduled load must draw nothing"
    );
    assert_eq!(
        summary.net_grid_kw, 0.0,
        "dead bus: the meter must read zero"
    );

    // Restoration: the load returns.
    dwelling.set_grid_voltage(1.0);
    dwelling.step().expect("restored step");
    let summary = dwelling
        .save_checkpoint()
        .expect("checkpoint")
        .prior_electrical_summary;
    assert!(
        summary.base_load_kw > 1.0,
        "load must return after restoration, got {}",
        summary.base_load_kw
    );
}

/// Utility outage with a charged battery: the dwelling islands (bus held at
/// nominal), loads keep running, and the battery discharges to serve them.
#[test]
fn outage_with_battery_islands_and_keeps_loads_running() {
    let mut dwelling = build_dwelling_with_base_load("battery-backup");
    add_battery(&mut dwelling, 0.6);

    dwelling.step().expect("nominal step");

    // Utility outage: the battery islands the home.
    dwelling.set_grid_voltage(0.0);
    dwelling.step().expect("islanded step");

    let grid = &dwelling.latest_env().grid;
    assert!(grid.grid_outage());
    assert!(
        grid.islanded(),
        "charged battery must island the home (bus energized at nominal)"
    );
    assert_eq!(grid.island_bus_voltage_pu, Some(1.0));

    dwelling.step().expect("second islanded step");
    let summary = dwelling
        .save_checkpoint()
        .expect("checkpoint")
        .prior_electrical_summary;
    assert!(
        summary.base_load_kw > 1.0,
        "islanded home must NOT drop its loads, got {} kW",
        summary.base_load_kw
    );
    assert!(
        summary.battery_power_kw < 0.0,
        "battery must discharge to serve the islanded load, got {} kW",
        summary.battery_power_kw
    );
}

/// A depleted battery (SOC at its floor) cannot island the home: the bus is
/// dead and loads drop, exactly as with no battery at all.
#[test]
fn outage_with_depleted_battery_does_not_island() {
    let mut dwelling = build_dwelling_with_base_load("depleted-battery");
    add_battery(&mut dwelling, 0.1); // at min_soc floor

    dwelling.step().expect("nominal step");
    dwelling.set_grid_voltage(0.0);
    dwelling.step().expect("outage step");

    let grid = &dwelling.latest_env().grid;
    assert!(
        !grid.bus_energized(),
        "a battery at its SOC floor cannot island the home"
    );

    dwelling.step().expect("second outage step");
    let summary = dwelling
        .save_checkpoint()
        .expect("checkpoint")
        .prior_electrical_summary;
    assert_eq!(
        summary.base_load_kw, 0.0,
        "dead bus: loads must drop with a depleted battery"
    );
}
