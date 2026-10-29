//! The island's grid-interchange publication (the co-simulation
//! contract's island semantics).
//!
//! During an island the service connection carries no power: the breaker
//! is open, so the physically correct published grid interchange is
//! exactly 0.0. The island's internal imbalance (the load the island
//! sources fail to cover, the surplus they cannot absorb) surfaces as
//! the island's internal response (`island_unserved_w` /
//! `island_excess_w` and the premise balance, `total_power_w`), never as
//! a phantom grid load the feeder federate would read as demand exactly
//! during the disturbances the study exists to examine.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use hares_core::Dwelling;
use hares_equipment::battery::Battery;
use hares_equipment::config::ConfigValue;
use hares_equipment::scheduled_load::ScheduledLoad;
use hares_equipment::{BatteryConfig, Equipment, EquipmentConfig};
use hares_types::EndUse;

fn write_minimal_toml(path: &Path) {
    let content = r#"building_id = 9403

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res = 60
duration = 1200
output_chunk_size = 1000
master_seed = 0
[geometry]
number_of_bedrooms = 3.0
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "none"

[weather]
outdoor_temp_c = 20.0
dew_point_c = 10.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[infiltration]
ach = 0.0

[schedule]
occupancy = 0.0
"#;
    fs::write(path, content).expect("failed to write synthetic TOML");
}

/// Dwelling with a 1.5 kW constant-power scheduled load (the island's
/// load) and a battery whose discharge limit cannot cover it, so the
/// island runs a real internal imbalance.
fn build_islanded_dwelling(tag: &str) -> Dwelling {
    let dir = tempfile::tempdir().expect("temp dir");
    let toml_path = dir.path().join(format!("{tag}.toml"));
    write_minimal_toml(&toml_path);
    let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build dwelling");

    let env = dwelling.latest_env().clone();
    let mut raw: HashMap<String, ConfigValue> = HashMap::new();
    raw.insert("power_schedule_source".to_string(), "constant".into());
    raw.insert("power_constant_w".to_string(), 1500.0.into());
    raw.insert("sensible_gain_fraction".to_string(), 0.5.into());
    raw.insert("zone_id".to_string(), 1.0.into());
    let config = EquipmentConfig::raw("BaseLoad".to_string(), "ScheduledLoad".to_string(), raw);
    let mut eq = ScheduledLoad::new(config.clone(), EndUse::LIGHTING, "Lighting");
    eq.init(&config, &env).expect("init ScheduledLoad");
    dwelling
        .add_equipment(Box::new(eq))
        .expect("add_equipment must succeed");

    let cfg = BatteryConfig {
        equipment_id: None,
        zone_id: None,
        capacity_j: 10.0 * 3.6e6,
        max_charge_w: 5000.0,
        max_discharge_w: 500.0,
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
        initial_soc: Some(0.6),
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
        der_1547: None,
    };
    let battery_config =
        EquipmentConfig::from_typed("Battery".to_string(), "Battery".to_string(), cfg).unwrap();
    let mut bat = Battery::new(battery_config.clone());
    bat.init(&battery_config, &env).expect("init Battery");
    dwelling
        .add_equipment(Box::new(bat))
        .expect("add_equipment must succeed");
    dwelling
}

/// The island interchange witness: from island formation the published
/// grid interchange reads exactly 0.0 while the premise balance continues
/// and the imbalance surfaces as the island's internal unserved load.
#[test]
fn islanded_publication_reads_zero_interchange_with_the_internal_response() {
    let mut dwelling = build_islanded_dwelling("island-interchange");

    // Grid-connected: the interchange is the meter's net flow.
    dwelling.step().expect("nominal step");
    let connected = dwelling.telemetry().expect("telemetry");
    assert!(!connected.islanded);
    assert_eq!(connected.bus_voltage_pu, 1.0);
    assert_eq!(connected.grid_interchange_w, connected.total_power_w);

    // Island formation: the utility out, the battery holding the bus —
    // one step after the outage itself (the formation condition is
    // end-of-previous-step state, the documented lag).
    dwelling
        .set_grid_voltage(0.0)
        .expect("the outage override is in contract");
    dwelling.step().expect("the outage's dead-bus step");
    assert!(
        !dwelling.telemetry().expect("telemetry").islanded,
        "the outage's first step is a dead-bus step"
    );
    assert_eq!(
        dwelling.telemetry().expect("telemetry").grid_interchange_w,
        0.0,
        "the breaker is open on the dead-bus step too"
    );
    dwelling.step().expect("islanded step");
    let islanded = dwelling.telemetry().expect("telemetry");
    assert!(islanded.islanded, "the step islands the dwelling");
    assert_eq!(islanded.bus_voltage_pu, 1.0, "the island bus is nominal");
    assert_eq!(
        islanded.grid_interchange_w, 0.0,
        "the published grid interchange during an island is exactly zero: \
         the breaker is open"
    );
    assert!(
        islanded.total_power_w != 0.0,
        "the premise balance continues: the imbalance is the island's own, \
         not a phantom grid load"
    );
    assert_eq!(
        islanded.island_unserved_w, islanded.total_power_w,
        "the uncovered share of the imbalance is the island's unserved load"
    );
    assert!(
        islanded.island_unserved_w > 0.0,
        "the scenario's imbalance is real (the battery's discharge limit \
         cannot cover the load), so the witness is non-vacuous"
    );

    // The interchange stays exactly zero every islanded step.
    dwelling.step().expect("second islanded step");
    let still_islanded = dwelling.telemetry().expect("telemetry");
    assert!(still_islanded.islanded);
    assert_eq!(still_islanded.grid_interchange_w, 0.0);

    // Restoration: the interchange is the meter's net flow again.
    dwelling
        .set_grid_voltage(1.0)
        .expect("the restoration override is in contract");
    dwelling.step().expect("restored step");
    let restored = dwelling.telemetry().expect("telemetry");
    assert!(!restored.islanded);
    assert_eq!(restored.grid_interchange_w, restored.total_power_w);
}
