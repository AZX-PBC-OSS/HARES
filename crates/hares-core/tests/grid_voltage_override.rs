//! The grid-voltage override's contract at the dwelling's setter.
//!
//! `Dwelling::set_grid_voltage` is the documented outage setter (a
//! co-simulation or scenario caller computing voltage from a model that
//! produces one non-finite value would otherwise poison the entire outage
//! machinery silently: a NaN stored raw reads as energized by every
//! per-unit comparison downstream and un-gates every load during an
//! intended outage). The setter therefore accepts exactly `0.0` (the
//! utility-outage signal) or a finite per-unit voltage in `[0.5, 1.5]`,
//! the band the HELICS voltage contract admits, and rejects everything
//! else with a typed error, leaving the stored grid state untouched.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use hares_core::Dwelling;
use hares_equipment::config::ConfigValue;
use hares_equipment::scheduled_load::ScheduledLoad;
use hares_equipment::{Equipment, EquipmentConfig};
use hares_types::EndUse;

fn write_minimal_toml(path: &Path) {
    let content = r#"building_id = 9401

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

fn build_dwelling(tag: &str) -> Dwelling {
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
    dwelling
}

/// Every in-contract value is accepted: the outage signal, the band's
/// edges, and a nominal value.
#[test]
fn in_contract_overrides_are_accepted() {
    let mut dwelling = build_dwelling("voltage-accept");
    dwelling.set_grid_voltage(1.0).expect("1.0 pu is in band");
    dwelling
        .set_grid_voltage(0.0)
        .expect("0.0 is the outage signal");
    dwelling
        .set_grid_voltage(0.5)
        .expect("the band's lower edge");
    dwelling
        .set_grid_voltage(1.5)
        .expect("the band's upper edge");
    dwelling.set_grid_voltage(0.97).expect("a sagged nominal");
}

/// NaN, the infinities, negatives and out-of-band values are rejected
/// with a typed error naming the value and the band, and the stored grid
/// state is untouched by every rejection.
#[test]
fn out_of_contract_overrides_are_rejected() {
    let mut dwelling = build_dwelling("voltage-reject");
    dwelling.set_grid_voltage(1.0).expect("the control value");

    let before_grid = dwelling.latest_env().grid.clone();
    for invalid in [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        -1.0,
        0.2,
        1.8,
        240.0,
    ] {
        let err = dwelling
            .set_grid_voltage(invalid)
            .expect_err("an out-of-contract override must be rejected");
        let message = err.to_string();
        assert!(
            message.contains("grid voltage override"),
            "the error names the override: {message}"
        );
        assert_eq!(
            dwelling.latest_env().grid,
            before_grid,
            "a rejected override must leave the stored grid state untouched"
        );
    }
}
