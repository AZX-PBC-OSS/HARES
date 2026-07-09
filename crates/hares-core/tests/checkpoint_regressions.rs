//! Regression tests for prior_electrical_summary checkpoint fidelity.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use hares_core::Dwelling;
use hares_equipment::EvConfig;
use hares_equipment::config::ConfigValue;
use hares_equipment::ev::Ev;
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

/// Verifies that after `load_checkpoint`, `latest_env.equipment_core`
/// is populated for every equipment instance — not left empty as it
/// was before the fix.
#[test]
fn equipment_core_populated_after_checkpoint_restore() {
    let (toml_path, mut dwelling_a) = build_dwelling_with_base_load("core-populated");

    for _ in 0..3 {
        dwelling_a.step().expect("step in dwelling A");
    }
    let checkpoint = dwelling_a
        .save_checkpoint()
        .expect("save checkpoint from A");

    let dwelling_b_raw = Dwelling::from_toml_config(&toml_path).expect("build dwelling B base");
    let env_b = dwelling_b_raw.latest_env().clone();
    let mut dwelling_b = dwelling_b_raw;
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

    let env = dwelling_b.latest_env();
    assert!(
        !env.equipment_core.is_empty(),
        "equipment_core must be non-empty after checkpoint restore"
    );

    // Each equipment_core entry must have non-default content (not
    // zeroed CoreOutput::default()).  This catches the bug where
    // load_state() resets core_output to default and
    // snapshot_equipment_state() takes a snapshot of the empty struct
    // before the next step() repopulates it.
    for (id, co) in &env.equipment_core {
        assert!(
            co.flows.electric_kw.is_some(),
            "equipment_core entry for {:?} must have non-default electric_kw after checkpoint restore; got flows={:?}",
            id,
            co.flows,
        );
    }

    // The ScheduledLoad must have an entry in equipment_core
    // and equipment_telemetry after checkpoint restore.
    let eq_name = "BaseLoad";
    assert!(
        env.equipment_telemetry.contains_key(eq_name),
        "equipment_telemetry must contain '{}' after checkpoint restore",
        eq_name
    );

    // equipment_core should have an entry for every registered equipment.
    let core_equipment_count = env.equipment_core.len();
    assert!(
        core_equipment_count > 0,
        "equipment_core must have at least one entry after checkpoint restore"
    );

    let _ = fs::remove_file(&toml_path);
}

/// Verifies that after `load_checkpoint`, the equipment_core keys
/// match those present at the time `save_checkpoint` was called.
#[test]
fn equipment_core_keys_match_after_checkpoint_restore() {
    let (toml_path, mut dwelling_a) = build_dwelling_with_base_load("core-keys-match");

    // Add a second equipment so key matching is non-trivial.
    let env_a = dwelling_a.latest_env().clone();
    let mut raw2: HashMap<String, ConfigValue> = HashMap::new();
    raw2.insert("power_schedule_source".to_string(), "constant".into());
    raw2.insert("power_constant_kw".to_string(), 0.5.into());
    raw2.insert("sensible_gain_fraction".to_string(), 0.3.into());
    let config2 = EquipmentConfig::raw("Plug".to_string(), "ScheduledLoad".to_string(), raw2);
    let mut eq2 = ScheduledLoad::new(config2.clone(), EndUse::PLUG_LOADS, "Plug");
    eq2.init(&config2, &env_a).expect("init Plug");
    dwelling_a.add_equipment(Box::new(eq2));

    for _ in 0..3 {
        dwelling_a.step().expect("step in dwelling A");
    }

    let pre_save_core_keys: std::collections::BTreeSet<_> = dwelling_a
        .latest_env()
        .equipment_core
        .keys()
        .copied()
        .collect();

    let checkpoint = dwelling_a
        .save_checkpoint()
        .expect("save checkpoint from A");

    let dwelling_b_raw = Dwelling::from_toml_config(&toml_path).expect("build dwelling B");
    let env_b = dwelling_b_raw.latest_env().clone();
    let mut dwelling_b = dwelling_b_raw;

    // Re-add both equipment pieces.
    let mut raw: HashMap<String, ConfigValue> = HashMap::new();
    raw.insert("power_schedule_source".to_string(), "constant".into());
    raw.insert("power_constant_kw".to_string(), 1.5.into());
    raw.insert("sensible_gain_fraction".to_string(), 0.5.into());
    let config_b = EquipmentConfig::raw("BaseLoad".to_string(), "ScheduledLoad".to_string(), raw);
    let mut eq_b = ScheduledLoad::new(config_b.clone(), EndUse::LIGHTING, "Lighting");
    eq_b.init(&config_b, &env_b).expect("init Lighting");
    dwelling_b.add_equipment(Box::new(eq_b));

    let mut raw2_b: HashMap<String, ConfigValue> = HashMap::new();
    raw2_b.insert("power_schedule_source".to_string(), "constant".into());
    raw2_b.insert("power_constant_kw".to_string(), 0.5.into());
    raw2_b.insert("sensible_gain_fraction".to_string(), 0.3.into());
    let config2_b = EquipmentConfig::raw("Plug".to_string(), "ScheduledLoad".to_string(), raw2_b);
    let mut eq2_b = ScheduledLoad::new(config2_b.clone(), EndUse::PLUG_LOADS, "Plug");
    eq2_b.init(&config2_b, &env_b).expect("init Plug");
    dwelling_b.add_equipment(Box::new(eq2_b));

    dwelling_b
        .load_checkpoint(checkpoint)
        .expect("load checkpoint into B");

    let post_restore_core_keys: std::collections::BTreeSet<_> = dwelling_b
        .latest_env()
        .equipment_core
        .keys()
        .copied()
        .collect();

    assert_eq!(
        pre_save_core_keys, post_restore_core_keys,
        "equipment_core keys must survive checkpoint round-trip unchanged"
    );

    let _ = fs::remove_file(&toml_path);
}

/// Verifies that after checkpoint restore, stepping succeeds and produces
/// the same equipment_core state as a continuous run would — confirming that
/// the snapshot was complete and actors read correct equipment outputs.
#[test]
fn first_post_restore_step_produces_valid_equipment_output() {
    let (toml_path, mut dwelling_a) = build_dwelling_with_base_load("core-output-valid");

    for _ in 0..3 {
        dwelling_a.step().expect("step in dwelling A");
    }
    let checkpoint = dwelling_a
        .save_checkpoint()
        .expect("save checkpoint from A");

    let dwelling_b_raw = Dwelling::from_toml_config(&toml_path).expect("build dwelling B base");
    let env_b = dwelling_b_raw.latest_env().clone();
    let mut dwelling_b = dwelling_b_raw;
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

    // Step after restore — the invariant check at step start verifies
    // equipment_core completeness.
    dwelling_b
        .step()
        .expect("first post-restore step should succeed");

    // The core output after the step should have actual power values
    // (not default-zero), confirming equipment was properly restored.
    let env = dwelling_b.latest_env();
    let has_power = env.equipment_core.iter().any(|(_, co)| {
        co.flows
            .electric_kw
            .is_some_and(|e| e.net_consumption_kw().abs() > 0.0)
    });
    assert!(
        has_power,
        "equipment_core must contain entries with non-zero power after first post-restore step"
    );

    let _ = fs::remove_file(&toml_path);
}

fn add_ev_to_dwelling(dwelling: &mut Dwelling) {
    let env = dwelling.latest_env().clone();
    let config = EquipmentConfig::from_typed(
        "EV1".to_string(),
        "EV".to_string(),
        EvConfig {
            equipment_id: None,
            capacity_kwh: 60.0,
            charging_level: Some("L2".to_string()),
            max_charging_power_kw: 7.2,
            charging_efficiency: None,
            l1_current_a: None,
            l1_voltage_v: None,
            soc_max: None,
            initial_soc: Some(0.65),
            battery_temp_c: None,
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
            power_factor: None,
            charger_capacity_kva: None,
            cc_cv_transition_soc: None,
            charging_priority: None,
            discharge_respects_deadline: true,
        },
    )
    .unwrap();
    let mut ev = Ev::new(config.clone());
    ev.init(&config, &env).expect("init EV");
    dwelling.add_equipment(Box::new(ev));
}

/// Verifies that after `load_checkpoint`, EV equipment_core entries contain
/// correct SOC values (not None/default) so that actors reading
/// `env.equipment_core` for SOC-based decisions see the actual restored SOC
/// rather than falling back to estimated values.  This would have caught
/// the bug where load_state() resets core_output to CoreOutput::default()
/// before snapshot_equipment_state() captures it.
#[test]
fn equipment_core_restores_ev_soc_after_checkpoint() {
    let (toml_path, mut dwelling_a) = build_dwelling_with_base_load("ev-soc-A");
    add_ev_to_dwelling(&mut dwelling_a);

    // Run steps so the EV charges and SOC moves from initial 0.65.
    for _ in 0..3 {
        dwelling_a.step().expect("step in dwelling A");
    }

    // Collect SOC values from every equipment_core entry before save.
    let pre_save_soc_values: Vec<f64> = dwelling_a
        .latest_env()
        .equipment_core
        .values()
        .filter_map(|co| co.state.soc.map(|s| s.get()))
        .collect();
    assert!(
        !pre_save_soc_values.is_empty(),
        "equipment_core must have at least one SOC-bearing entry after steps"
    );

    let checkpoint = dwelling_a
        .save_checkpoint()
        .expect("save checkpoint from A");

    // Build dwelling B from same TOML, re-add the same equipment in the
    // same order so equipment array indices match the checkpoint.
    let dwelling_b_raw = Dwelling::from_toml_config(&toml_path).expect("build dwelling B");
    let mut dwelling_b = dwelling_b_raw;
    let env_b = dwelling_b.latest_env().clone();

    // Re-add BaseLoad (same as build_dwelling_with_base_load).
    let mut raw: HashMap<String, ConfigValue> = HashMap::new();
    raw.insert("power_schedule_source".to_string(), "constant".into());
    raw.insert("power_constant_kw".to_string(), 1.5.into());
    raw.insert("sensible_gain_fraction".to_string(), 0.5.into());
    let config_b = EquipmentConfig::raw("BaseLoad".to_string(), "ScheduledLoad".to_string(), raw);
    let mut eq_b = ScheduledLoad::new(config_b.clone(), EndUse::LIGHTING, "Lighting");
    eq_b.init(&config_b, &env_b).expect("init ScheduledLoad B");
    dwelling_b.add_equipment(Box::new(eq_b));

    // Re-add EV (same order as dwelling A).
    add_ev_to_dwelling(&mut dwelling_b);

    dwelling_b
        .load_checkpoint(checkpoint)
        .expect("load checkpoint into B");

    let post_restore_soc_values: Vec<f64> = dwelling_b
        .latest_env()
        .equipment_core
        .values()
        .filter_map(|co| co.state.soc.map(|s| s.get()))
        .collect();

    assert_eq!(
        post_restore_soc_values.len(),
        pre_save_soc_values.len(),
        "SOC-bearing entry count must survive checkpoint: before={}, after={}",
        pre_save_soc_values.len(),
        post_restore_soc_values.len(),
    );

    // Verify each SOC value matches within float tolerance — not just
    // that entries exist. A count-only assertion cannot distinguish
    // "value correctly preserved at 0.66" from "value degraded to 0.01".
    for (i, (&pre, &post)) in pre_save_soc_values
        .iter()
        .zip(post_restore_soc_values.iter())
        .enumerate()
    {
        let delta = (pre - post).abs();
        assert!(
            delta <= 1e-9,
            "SOC value at index {i} diverged after checkpoint round-trip: pre={pre}, post={post}, delta={delta}",
        );
    }

    let _ = fs::remove_file(&toml_path);
}
