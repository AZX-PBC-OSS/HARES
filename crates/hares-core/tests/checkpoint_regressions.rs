//! Regression tests for prior_electrical_summary checkpoint fidelity.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use hares_core::Dwelling;
use hares_equipment::config::ConfigValue;
use hares_equipment::scheduled_load::ScheduledLoad;
use hares_equipment::{Equipment, EquipmentConfig};
use hares_types::EndUse;

fn nanos_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos()
}

fn unique_temp_toml(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("hares-checkpoint-{tag}-{}.toml", nanos_suffix()));
    path
}

/// Write a minimal synthetic-TOML dwelling (no equipment, just envelope).
fn write_minimal_toml(path: &PathBuf) {
    let content = r#"building_id = 9001

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res_s = 60
duration_s = 600

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

fn build_dwelling_with_base_load(tag: &str) -> (PathBuf, Dwelling) {
    let toml_path = unique_temp_toml(tag);
    write_minimal_toml(&toml_path);

    let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build dwelling");

    // Programmatically add a 1.5 kW constant-power ScheduledLoad so the
    // electrical solver and prior_electrical_summary are exercised.
    let env = dwelling.latest_env().clone();
    let mut raw: HashMap<String, ConfigValue> = HashMap::new();
    raw.insert("power_schedule_source".to_string(), "constant".into());
    raw.insert("power_constant_kw".to_string(), 1.5.into());
    raw.insert("sensible_gain_fraction".to_string(), 0.5.into());
    let config = EquipmentConfig::raw("BaseLoad".to_string(), "ScheduledLoad".to_string(), raw);
    let mut eq = ScheduledLoad::new(config.clone(), EndUse::LIGHTING, "Lighting");
    eq.init(&config, &env).expect("init ScheduledLoad");
    dwelling.add_equipment(Box::new(eq));

    (toml_path, dwelling)
}

/// If `prior_electrical_summary` is not checkpointed, the first post-restore
/// step sees an all-zeros ElectricalSummary and actors that read
/// `env.electrical` make decisions on stale data. This test saves a
/// checkpoint after several steps (when the summary is populated), restores
/// it into a fresh dwelling, and verifies the summary survives the round-trip.
#[test]
fn prior_electrical_summary_survives_checkpoint_restart() {
    let (toml_path, mut dwelling_a) = build_dwelling_with_base_load("elec-summary-survives");

    // Run 3 steps to populate prior_electrical_summary.
    for _ in 0..3 {
        dwelling_a.step().expect("step in dwelling A");
    }
    let checkpoint = dwelling_a
        .save_checkpoint()
        .expect("save checkpoint from A");

    // The dwelling had a 1.5 kW base load for 3 steps — summary must be non-zero.
    assert!(
        checkpoint.prior_electrical_summary.base_load_kw > 0.0,
        "after 3 steps with 1.5 kW base load, base_load_kw must be >0; got {}",
        checkpoint.prior_electrical_summary.base_load_kw,
    );

    // Restore into a fresh dwelling.
    let dwelling_b_raw = Dwelling::from_toml_config(&toml_path).expect("build dwelling B base");
    let env_b = dwelling_b_raw.latest_env().clone();
    let mut dwelling_b = dwelling_b_raw;
    // Re-add the same equipment so the equipment count matches.
    let mut raw: HashMap<String, ConfigValue> = HashMap::new();
    raw.insert("power_schedule_source".to_string(), "constant".into());
    raw.insert("power_constant_kw".to_string(), 1.5.into());
    raw.insert("sensible_gain_fraction".to_string(), 0.5.into());
    let config_b = EquipmentConfig::raw("BaseLoad".to_string(), "ScheduledLoad".to_string(), raw);
    let mut eq_b = ScheduledLoad::new(config_b.clone(), EndUse::LIGHTING, "Lighting");
    eq_b.init(&config_b, &env_b).expect("init ScheduledLoad B");
    dwelling_b.add_equipment(Box::new(eq_b));

    dwelling_b
        .load_checkpoint(checkpoint.clone())
        .expect("load checkpoint into B");

    // Save a new checkpoint — the prior_electrical_summary should match.
    let checkpoint_b = dwelling_b
        .save_checkpoint()
        .expect("save checkpoint from B");
    assert_eq!(
        checkpoint_b.prior_electrical_summary, checkpoint.prior_electrical_summary,
        "prior_electrical_summary must survive checkpoint round-trip unchanged",
    );

    let _ = fs::remove_file(&toml_path);
}
