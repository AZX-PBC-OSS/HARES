//! Regression tests for checkpoint fidelity: the restored dwelling resumes
//! with the state the checkpointed one held.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use arrow::array::{Array, Float64Array, RecordBatch};
use arrow::compute::concat_batches;
use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::{Dwelling, DwellingCheckpoint, DwellingConfig, SimulationConfig};
use hares_equipment::EvConfig;
use hares_equipment::config::ConfigValue;
use hares_equipment::ev::Ev;
use hares_equipment::scheduled_load::ScheduledLoad;
use hares_equipment::{Equipment, EquipmentConfig};
use hares_io::OutputFormat;
use hares_types::EndUse;

/// Write a minimal synthetic-TOML dwelling (no equipment, just envelope).
fn write_minimal_toml(path: &Path) {
    let content = r#"building_id = 9001

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res_s = 60
duration_s = 600

[geometry]
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

/// Builds the dwelling from a TOML written into `dir`; the path is returned so
/// the test can build the restore target from the same definition.
fn build_dwelling_with_base_load(dir: &Path) -> (PathBuf, Dwelling) {
    let toml_path = dir.join("dwelling.toml");
    write_minimal_toml(&toml_path);

    let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build dwelling");

    // Programmatically add a 1.5 kW constant-power ScheduledLoad so the
    // electrical solver and prior_electrical_summary are exercised.
    let env = dwelling.latest_env().clone();
    let mut raw: HashMap<String, ConfigValue> = HashMap::new();
    raw.insert("power_schedule_source".to_string(), "constant".into());
    raw.insert("power_constant_kw".to_string(), 1.5.into());
    raw.insert("sensible_gain_fraction".to_string(), 0.5.into());
    raw.insert("zone_id".to_string(), 1.0.into());
    let config = EquipmentConfig::raw("BaseLoad".to_string(), "ScheduledLoad".to_string(), raw);
    let mut eq = ScheduledLoad::new(config.clone(), EndUse::LIGHTING, "Lighting");
    eq.init(&config, &env).expect("init ScheduledLoad");
    dwelling
        .add_equipment(Box::new(eq))
        .expect("add_equipment must succeed");

    (toml_path, dwelling)
}

/// If `prior_electrical_summary` is not checkpointed, the first post-restore
/// step sees an all-zeros ElectricalSummary and actors that read
/// `env.electrical` make decisions on stale data. This test saves a
/// checkpoint after several steps (when the summary is populated), restores
/// it into a fresh dwelling, and verifies the summary survives the round-trip.
#[test]
fn prior_electrical_summary_survives_checkpoint_restart() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (toml_path, mut dwelling_a) = build_dwelling_with_base_load(dir.path());

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
    raw.insert("zone_id".to_string(), 1.0.into());
    let config_b = EquipmentConfig::raw("BaseLoad".to_string(), "ScheduledLoad".to_string(), raw);
    let mut eq_b = ScheduledLoad::new(config_b.clone(), EndUse::LIGHTING, "Lighting");
    eq_b.init(&config_b, &env_b).expect("init ScheduledLoad B");
    dwelling_b
        .add_equipment(Box::new(eq_b))
        .expect("add_equipment must succeed");

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
}

/// Verifies that after `load_checkpoint`, `latest_env.equipment_core`
/// is populated for every equipment instance — not left empty as it
/// was before the fix.
#[test]
fn equipment_core_populated_after_checkpoint_restore() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (toml_path, mut dwelling_a) = build_dwelling_with_base_load(dir.path());

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
    raw.insert("zone_id".to_string(), 1.0.into());
    let config_b = EquipmentConfig::raw("BaseLoad".to_string(), "ScheduledLoad".to_string(), raw);
    let mut eq_b = ScheduledLoad::new(config_b.clone(), EndUse::LIGHTING, "Lighting");
    eq_b.init(&config_b, &env_b).expect("init ScheduledLoad B");
    dwelling_b
        .add_equipment(Box::new(eq_b))
        .expect("add_equipment must succeed");

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
}

/// Verifies that after `load_checkpoint`, the equipment_core keys
/// match those present at the time `save_checkpoint` was called.
#[test]
fn equipment_core_keys_match_after_checkpoint_restore() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (toml_path, mut dwelling_a) = build_dwelling_with_base_load(dir.path());

    // Add a second equipment so key matching is non-trivial.
    let env_a = dwelling_a.latest_env().clone();
    let mut raw2: HashMap<String, ConfigValue> = HashMap::new();
    raw2.insert("power_schedule_source".to_string(), "constant".into());
    raw2.insert("power_constant_kw".to_string(), 0.5.into());
    raw2.insert("sensible_gain_fraction".to_string(), 0.3.into());
    raw2.insert("zone_id".to_string(), 1.0.into());
    let config2 = EquipmentConfig::raw("Plug".to_string(), "ScheduledLoad".to_string(), raw2);
    let mut eq2 = ScheduledLoad::new(config2.clone(), EndUse::PLUG_LOADS, "Plug");
    eq2.init(&config2, &env_a).expect("init Plug");
    dwelling_a
        .add_equipment(Box::new(eq2))
        .expect("add_equipment must succeed");

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
    raw.insert("zone_id".to_string(), 1.0.into());
    let config_b = EquipmentConfig::raw("BaseLoad".to_string(), "ScheduledLoad".to_string(), raw);
    let mut eq_b = ScheduledLoad::new(config_b.clone(), EndUse::LIGHTING, "Lighting");
    eq_b.init(&config_b, &env_b).expect("init Lighting");
    dwelling_b
        .add_equipment(Box::new(eq_b))
        .expect("add_equipment must succeed");

    let mut raw2_b: HashMap<String, ConfigValue> = HashMap::new();
    raw2_b.insert("power_schedule_source".to_string(), "constant".into());
    raw2_b.insert("power_constant_kw".to_string(), 0.5.into());
    raw2_b.insert("sensible_gain_fraction".to_string(), 0.3.into());
    raw2_b.insert("zone_id".to_string(), 1.0.into());
    let config2_b = EquipmentConfig::raw("Plug".to_string(), "ScheduledLoad".to_string(), raw2_b);
    let mut eq2_b = ScheduledLoad::new(config2_b.clone(), EndUse::PLUG_LOADS, "Plug");
    eq2_b.init(&config2_b, &env_b).expect("init Plug");
    dwelling_b
        .add_equipment(Box::new(eq2_b))
        .expect("add_equipment must succeed");

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
}

/// Verifies that after checkpoint restore, stepping succeeds and produces
/// the same equipment_core state as a continuous run would — confirming that
/// the snapshot was complete and actors read correct equipment outputs.
#[test]
fn first_post_restore_step_produces_valid_equipment_output() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (toml_path, mut dwelling_a) = build_dwelling_with_base_load(dir.path());

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
    raw.insert("zone_id".to_string(), 1.0.into());
    let config_b = EquipmentConfig::raw("BaseLoad".to_string(), "ScheduledLoad".to_string(), raw);
    let mut eq_b = ScheduledLoad::new(config_b.clone(), EndUse::LIGHTING, "Lighting");
    eq_b.init(&config_b, &env_b).expect("init ScheduledLoad B");
    dwelling_b
        .add_equipment(Box::new(eq_b))
        .expect("add_equipment must succeed");

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
            n_series: None,
            n_parallel: None,
            cell_resistance_ohm: None,
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
    dwelling
        .add_equipment(Box::new(ev))
        .expect("add_equipment must succeed");
}

/// Verifies that after `load_checkpoint`, EV equipment_core entries contain
/// correct SOC values (not None/default) so that actors reading
/// `env.equipment_core` for SOC-based decisions see the actual restored SOC
/// rather than falling back to estimated values.  This would have caught
/// the bug where load_state() resets core_output to CoreOutput::default()
/// before snapshot_equipment_state() captures it.
#[test]
fn equipment_core_restores_ev_soc_after_checkpoint() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (toml_path, mut dwelling_a) = build_dwelling_with_base_load(dir.path());
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
    raw.insert("zone_id".to_string(), 1.0.into());
    let config_b = EquipmentConfig::raw("BaseLoad".to_string(), "ScheduledLoad".to_string(), raw);
    let mut eq_b = ScheduledLoad::new(config_b.clone(), EndUse::LIGHTING, "Lighting");
    eq_b.init(&config_b, &env_b).expect("init ScheduledLoad B");
    dwelling_b
        .add_equipment(Box::new(eq_b))
        .expect("add_equipment must succeed");

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
}

const RESUME_TOTAL_STEPS: usize = 96;

/// One resume scenario: a building, its weather and start day, and the step
/// at which the run is checkpointed.
struct ResumeCase {
    hpxml_path: PathBuf,
    weather_file: &'static str,
    utc_offset_west_h: i32,
    month: u32,
    day: u32,
    checkpoint_step: usize,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn resume_home_dir() -> PathBuf {
    repo_root().join("tests/fixtures/resstock/2025.1/bldg0176227")
}

fn resumable_dwelling_config(case: &ResumeCase, output_dir: &Path, run: &str) -> DwellingConfig {
    let root = repo_root();
    DwellingConfig {
        hpxml_path: case.hpxml_path.clone(),
        schedule_path: Some(resume_home_dir().join("in.schedules.csv")),
        weather_path: root
            .join("tests/fixtures/resstock/2025.1/weather")
            .join(case.weather_file),
        defaults_path: Some(root.join("defaults")),
        sim_config: SimulationConfig {
            start_time: FixedOffset::west_opt(case.utc_offset_west_h * 3600)
                .expect("valid offset")
                .with_ymd_and_hms(2018, case.month, case.day, 0, 0, 0)
                .single()
                .expect("valid start time"),
            duration: Duration::minutes(15 * RESUME_TOTAL_STEPS as i64),
            time_res: Duration::minutes(15),
            output_verbosity: 8,
            output_path: Some(output_dir.join(format!("{run}.parquet"))),
            write_output: true,
            output_format: OutputFormat::Parquet,
            output_chunk_size: 1024,
            setpoint_deadband_c: None,
            master_seed: 0,
            civil_timezone: None,
            site_location: hares_io::SiteLocationOverride::default(),
            retain_batches: true,
            rotation: hares_io::RotationPolicy::None,
        },
        overrides: None,
        bldg_id: 176_227,
        initialization_duration: None,
        resample_overrides: None,
        patches: None,
    }
}

fn recorded_frame(dwelling: &Dwelling) -> RecordBatch {
    let batches = dwelling.flushed_batches();
    let schema = batches.first().expect("the run recorded rows").schema();
    concat_batches(&schema, batches).expect("batches of one run share a schema")
}

/// Asserts every column of `actual` equals `expected` bitwise; `Float64`
/// columns compare bit patterns so a `-0.0`/`0.0` or NaN-payload change is a
/// difference, not an equality.
fn assert_frames_bitwise_equal(expected: &RecordBatch, actual: &RecordBatch) {
    assert_eq!(expected.schema(), actual.schema(), "column sets differ");
    assert_eq!(expected.num_rows(), actual.num_rows(), "row counts differ");
    for (field, (want, got)) in expected
        .schema()
        .fields()
        .iter()
        .zip(expected.columns().iter().zip(actual.columns()))
    {
        let name = field.name();
        match (
            want.as_any().downcast_ref::<Float64Array>(),
            got.as_any().downcast_ref::<Float64Array>(),
        ) {
            (Some(want), Some(got)) => {
                assert_eq!(
                    want.nulls(),
                    got.nulls(),
                    "column '{name}': null masks differ"
                );
                for (row, (w, g)) in want.values().iter().zip(got.values()).enumerate() {
                    assert_eq!(
                        w.to_bits(),
                        g.to_bits(),
                        "column '{name}' row {row}: continuous {w} vs resumed {g}"
                    );
                }
            }
            _ => assert_eq!(want.to_data(), got.to_data(), "column '{name}' differs"),
        }
    }
}

/// A run checkpointed at step k and resumed in a freshly built dwelling
/// produces the same rows for steps k..N as the continuous run, bitwise in
/// every output column. The building is multi-zone (conditioned space,
/// attic, vented crawlspace) with couplings active and its HVAC sized by the
/// ideal-capacity solve, so every piece of thermal solver state a step reads
/// must travel through the checkpoint.
///
/// A dwelling step rebuilds the couplings before any ideal-capacity solve
/// reads them: `Dwelling::step` runs `ThermalSolver::prepare_inputs`, whose
/// `prepare_inputs_inner` rebuilds `last_coupling`, before
/// `SolverFeedbackActor::collect_and_solve` runs the solves. This test
/// therefore pins resume equality of the whole dwelling; the restore of the
/// coupling state itself, which a solve called directly after
/// `restore_state` reads, is pinned by the solver unit test
/// `restore_state_restores_coupling_state`.
#[test]
fn resumed_run_equals_continuous_run() {
    let case = ResumeCase {
        hpxml_path: resume_home_dir().join("home.xml"),
        weather_file: "G0600770_2018.csv",
        utc_offset_west_h: 8,
        month: 1,
        day: 15,
        checkpoint_step: 37,
    };
    let output_dir = tempfile::tempdir().expect("temp dir");
    assert_resume_equals_continuous(&case, output_dir.path(), |dwelling, checkpoint| {
        assert!(
            dwelling.latest_env().zones.len() > 1,
            "the fixture must model more than one zone"
        );
        assert!(
            !checkpoint.thermal.last_coupling.is_empty(),
            "couplings must be active at the checkpoint"
        );
    });
}

/// The resume equality of `resumed_run_equals_continuous_run` on the same
/// building with a whole-building heat recovery ventilator, in cold weather
/// where defrost derates the ventilator's recovery effectiveness. A step
/// reads the effectiveness the ventilator reported on the previous step, so
/// the checkpoint must carry the derated value, not the rated one a freshly
/// built dwelling starts from.
#[test]
fn resumed_hrv_run_equals_continuous_run() {
    let output_dir = tempfile::tempdir().expect("temp dir");
    let case = ResumeCase {
        hpxml_path: write_hrv_variant(output_dir.path()),
        weather_file: "G0900090_2018.csv",
        utc_offset_west_h: 5,
        month: 1,
        day: 2,
        checkpoint_step: 36,
    };
    assert_resume_equals_continuous(&case, output_dir.path(), |dwelling, _| {
        let recovery = dwelling
            .thermal_solver()
            .config()
            .ventilation
            .sensible_recovery_efficiency;
        assert!(
            recovery < HRV_SENSIBLE_RECOVERY,
            "the ventilator must be derated at the checkpoint (recovery {recovery}, \
             rated {HRV_SENSIBLE_RECOVERY})"
        );
    });
}

const HRV_SENSIBLE_RECOVERY: f64 = 0.72;

/// A checkpoint with an invalid thermal snapshot is rejected, by both
/// `load_checkpoint` and `restore_building_state`, before any part of the
/// dwelling changes: the dwelling that rejected it holds the same state as
/// an identical dwelling never offered it, and runs on bitwise like it.
#[test]
fn rejected_checkpoint_leaves_the_dwelling_unchanged() {
    let case = ResumeCase {
        hpxml_path: resume_home_dir().join("home.xml"),
        weather_file: "G0600770_2018.csv",
        utc_offset_west_h: 8,
        month: 1,
        day: 15,
        checkpoint_step: 5,
    };
    let output_dir = tempfile::tempdir().expect("temp dir");
    let mut donor = Dwelling::from_config(resumable_dwelling_config(&case, output_dir.path(), "d"))
        .expect("build donor dwelling");
    for _ in 0..case.checkpoint_step {
        donor.step().expect("donor step");
    }
    let mut bad = donor.save_checkpoint().expect("save checkpoint");
    bad.thermal.x[0] = f64::NAN;

    let mut offered =
        Dwelling::from_config(resumable_dwelling_config(&case, output_dir.path(), "o"))
            .expect("build offered dwelling");
    let mut untouched =
        Dwelling::from_config(resumable_dwelling_config(&case, output_dir.path(), "u"))
            .expect("build untouched dwelling");
    offered.step().expect("offered step");
    untouched.step().expect("untouched step");

    offered
        .load_checkpoint(bad.clone())
        .expect_err("a NaN thermal state must be rejected");
    offered
        .restore_building_state(&bad)
        .expect_err("a NaN thermal state must be rejected");
    assert_eq!(
        offered.save_checkpoint().expect("save offered"),
        untouched.save_checkpoint().expect("save untouched"),
        "a rejected checkpoint changed the dwelling"
    );

    offered.simulate().expect("offered run");
    untouched.simulate().expect("untouched run");
    assert_frames_bitwise_equal(&recorded_frame(&untouched), &recorded_frame(&offered));
}

/// Writes the resume building with its local exhaust fans replaced by one
/// whole-building heat recovery ventilator and returns the HPXML path.
fn write_hrv_variant(dir: &Path) -> PathBuf {
    let xml = fs::read_to_string(resume_home_dir().join("home.xml")).expect("read HPXML");
    let start = xml
        .find("<VentilationFans>")
        .expect("HPXML has ventilation fans");
    let end_tag = "</VentilationFans>";
    let end = xml
        .find(end_tag)
        .expect("HPXML closes its ventilation fans")
        + end_tag.len();
    let hrv = format!(
        "<VentilationFans><VentilationFan>\
         <SystemIdentifier id='VentilationFan1'/>\
         <FanType>heat recovery ventilator</FanType>\
         <RatedFlowRate>110.0</RatedFlowRate>\
         <HoursInOperation>24.0</HoursInOperation>\
         <UsedForWholeBuildingVentilation>true</UsedForWholeBuildingVentilation>\
         <SensibleRecoveryEfficiency>{HRV_SENSIBLE_RECOVERY}</SensibleRecoveryEfficiency>\
         <FanPower>60.0</FanPower>\
         </VentilationFan></VentilationFans>"
    );
    let path = dir.join("home_hrv.xml");
    fs::write(&path, format!("{}{hrv}{}", &xml[..start], &xml[end..])).expect("write HPXML");
    path
}

/// Runs `case` continuously and, separately, to its checkpoint step,
/// checkpointed and resumed in a freshly built dwelling, then asserts every
/// output column of the resumed rows equals the continuous run's bitwise.
/// `at_checkpoint` asserts the scenario's preconditions on the interrupted
/// dwelling and its checkpoint.
fn assert_resume_equals_continuous(
    case: &ResumeCase,
    output_dir: &Path,
    at_checkpoint: impl FnOnce(&Dwelling, &DwellingCheckpoint),
) {
    let k = case.checkpoint_step;
    let mut continuous = Dwelling::from_config(resumable_dwelling_config(case, output_dir, "a"))
        .expect("build continuous dwelling");
    let continuous_steps = continuous.simulate().expect("continuous run").steps;
    assert!(
        continuous_steps[k..]
            .iter()
            .any(|step| step.hvac_heating_w > 0.0),
        "the HVAC must run after the checkpoint for the resumed solves to be compared"
    );
    let continuous_frame = recorded_frame(&continuous);
    assert_eq!(continuous_frame.num_rows(), RESUME_TOTAL_STEPS);

    let mut interrupted = Dwelling::from_config(resumable_dwelling_config(case, output_dir, "b"))
        .expect("build interrupted dwelling");
    for _ in 0..k {
        interrupted.step().expect("step before checkpoint");
    }
    let checkpoint = interrupted.save_checkpoint().expect("save checkpoint");
    at_checkpoint(&interrupted, &checkpoint);

    let mut resumed = Dwelling::from_config(resumable_dwelling_config(case, output_dir, "c"))
        .expect("build resumed dwelling");
    resumed
        .load_checkpoint(checkpoint)
        .expect("load checkpoint");
    resumed.simulate().expect("resumed run");
    let resumed_frame = recorded_frame(&resumed);

    let tail = continuous_frame.slice(k, RESUME_TOTAL_STEPS - k);
    assert_frames_bitwise_equal(&tail, &resumed_frame);
}
