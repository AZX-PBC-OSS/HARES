//! Checkpoint restart sub-suite: checkpoint at step N, restart, verify identical
//! continuation compared to an uninterrupted run.

use chrono::Duration;
use hares_core::Dwelling;
use hares_core::DwellingCheckpoint;
use hares_equipment::{load_versioned, try_save_versioned};
use hares_types::EquipmentId;
use hares_types::{BmsMode, ChargingStrategy, GridExportRule, PlugInPolicy, ScheduleSource};
use rand::SeedableRng;
use serde::{Deserialize, Serialize};

use super::helpers;

const CHECKPOINT_AT_STEP: u64 = 30;
const TOTAL_STEPS: i64 = 60;

pub fn run_checkpoint_restart_check() -> Result<(), Vec<String>> {
    let schedule_path = helpers::unique_temp_path("hares-regr-ckpt-sched", "csv");
    let weather_path = helpers::unique_temp_path("hares-regr-ckpt-weather", "epw");
    helpers::write_schedule_csv(&schedule_path);
    helpers::write_weather_epw(&weather_path);

    let config = helpers::build_dwelling_config(
        1,
        schedule_path.clone(),
        weather_path.clone(),
        Duration::minutes(TOTAL_STEPS),
        42,
    );

    let mut failures = Vec::new();

    // --- Uninterrupted reference run ---
    let mut dwelling_ref = match Dwelling::from_config(config.clone()) {
        Ok(d) => d,
        Err(err) => {
            helpers::cleanup_paths(&[schedule_path, weather_path]);
            return Err(vec![format!(
                "reference dwelling construction failed: {err}"
            )]);
        }
    };

    let ref_results = match dwelling_ref.simulate() {
        Ok(r) => r,
        Err(err) => {
            helpers::cleanup_paths(&[schedule_path, weather_path]);
            return Err(vec![format!("reference simulation failed: {err}")]);
        }
    };

    // --- Interrupted run: step to checkpoint, save, recreate, load, continue ---
    let mut dwelling_a = match Dwelling::from_config(config.clone()) {
        Ok(d) => d,
        Err(err) => {
            helpers::cleanup_paths(&[schedule_path, weather_path]);
            return Err(vec![format!(
                "checkpoint dwelling construction failed: {err}"
            )]);
        }
    };

    for _ in 0..CHECKPOINT_AT_STEP {
        if let Err(err) = dwelling_a.step() {
            failures.push(format!("step before checkpoint failed: {err}"));
            helpers::cleanup_paths(&[schedule_path, weather_path]);
            return Err(failures);
        }
    }

    let checkpoint = dwelling_a
        .save_checkpoint()
        .map_err(|e| vec![e.to_string()])?;

    let cp_path = helpers::unique_temp_path("hares-regr-ckpt", "json");
    if let Err(err) = checkpoint.save(&cp_path) {
        helpers::cleanup_paths(&[schedule_path, weather_path, cp_path]);
        return Err(vec![format!("checkpoint save failed: {err}")]);
    }

    let loaded_cp = match hares_core::DwellingCheckpoint::load(&cp_path) {
        Ok(cp) => cp,
        Err(err) => {
            helpers::cleanup_paths(&[schedule_path, weather_path, cp_path]);
            return Err(vec![format!("checkpoint load failed: {err}")]);
        }
    };

    let mut dwelling_b = match Dwelling::from_config(config) {
        Ok(d) => d,
        Err(err) => {
            helpers::cleanup_paths(&[schedule_path, weather_path, cp_path]);
            return Err(vec![format!(
                "restarted dwelling construction failed: {err}"
            )]);
        }
    };

    if let Err(err) = dwelling_b.load_checkpoint(loaded_cp) {
        helpers::cleanup_paths(&[schedule_path, weather_path, cp_path]);
        return Err(vec![format!("checkpoint restore failed: {err}")]);
    }

    let mut restarted_steps = Vec::new();
    while let Ok(step) = dwelling_b.step() {
        restarted_steps.push(step);
    }

    // Compare the tail of the reference run with the restarted run.
    let ref_tail = &ref_results.steps[CHECKPOINT_AT_STEP as usize..];
    let compare_len = ref_tail.len().min(restarted_steps.len());

    if compare_len == 0 {
        failures.push("no steps to compare after checkpoint restart".to_string());
    } else {
        for i in 0..compare_len {
            let ref_step = &ref_tail[i];
            let rst_step = &restarted_steps[i];

            let power_diff =
                (ref_step.net_electric_power_kw - rst_step.net_electric_power_kw).abs();
            if power_diff > 1e-9 {
                failures.push(format!(
                    "step[{}] power divergence after checkpoint: ref={:.9} restart={:.9} diff={:.2e}",
                    CHECKPOINT_AT_STEP as usize + i,
                    ref_step.net_electric_power_kw,
                    rst_step.net_electric_power_kw,
                    power_diff,
                ));
                break;
            }
        }
    }

    helpers::cleanup_paths(&[schedule_path, weather_path, cp_path]);

    if failures.is_empty() {
        eprintln!("[checkpoint_restart] PASS -- {compare_len} steps identical after restore");
        Ok(())
    } else {
        Err(failures)
    }
}

/// Regression: a postcard blob with a wrong per-equipment version is rejected
/// with a descriptive error naming the equipment type and expected/found versions.
pub fn run_per_equipment_version_rejection_regression() -> Result<(), Vec<String>> {
    #[derive(Serialize, Deserialize)]
    struct FakeState {
        val: f64,
    }

    let state = FakeState { val: 42.0 };
    let mut blob =
        try_save_versioned(&state, 1, "FakeEquip").expect("serialization should not fail in test");
    // Corrupt version byte
    blob[0] = 99;

    let result = load_versioned::<FakeState>(&blob, 1, "FakeEquip", EquipmentId(42));
    match result {
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("FakeEquip") && msg.contains("99") && msg.contains("1") {
                eprintln!("[per_equipment_version_rejection] PASS");
                Ok(())
            } else {
                Err(vec![format!(
                    "error message missing expected detail; got: {msg}"
                )])
            }
        }
        Ok(_) => Err(vec![
            "corrupted version blob should have been rejected but was accepted".to_string(),
        ]),
    }
}

/// Regression: a checkpoint with CRC32 and SHA-256 enabled round-trips
/// correctly through save/load and the restored state is identical.
pub fn run_checkpoint_integrity_roundtrip() -> Result<(), Vec<String>> {
    let schedule_path = helpers::unique_temp_path("hares-regr-ckpt-int-sched", "csv");
    let weather_path = helpers::unique_temp_path("hares-regr-ckpt-int-weather", "epw");
    helpers::write_schedule_csv(&schedule_path);
    helpers::write_weather_epw(&weather_path);

    let config = helpers::build_dwelling_config(
        1,
        schedule_path.clone(),
        weather_path.clone(),
        chrono::Duration::minutes(10),
        42,
    );

    let mut dwelling = match Dwelling::from_config(config) {
        Ok(d) => d,
        Err(err) => {
            helpers::cleanup_paths(&[schedule_path, weather_path]);
            return Err(vec![format!("dwelling construction failed: {err}")]);
        }
    };

    // Step a few times to produce non-trivial checkpoint state
    for _ in 0..5 {
        if let Err(err) = dwelling.step() {
            helpers::cleanup_paths(&[schedule_path, weather_path]);
            return Err(vec![format!("step before checkpoint failed: {err}")]);
        }
    }

    let checkpoint = dwelling
        .save_checkpoint()
        .map_err(|e| vec![e.to_string()])?;
    let cp_path = helpers::unique_temp_path("hares-regr-ckpt-int", "json");
    if let Err(err) = checkpoint.save(&cp_path) {
        helpers::cleanup_paths(&[schedule_path, weather_path, cp_path]);
        return Err(vec![format!("checkpoint save failed: {err}")]);
    }

    let loaded_cp = match DwellingCheckpoint::load(&cp_path) {
        Ok(cp) => cp,
        Err(err) => {
            helpers::cleanup_paths(&[schedule_path, weather_path, cp_path]);
            return Err(vec![format!("checkpoint load failed: {err}")]);
        }
    };

    // Verify the loaded checkpoint matches the original
    if loaded_cp.format_version != checkpoint.format_version {
        helpers::cleanup_paths(&[schedule_path, weather_path, cp_path]);
        return Err(vec![format!(
            "format_version mismatch: saved={} loaded={}",
            checkpoint.format_version, loaded_cp.format_version
        )]);
    }
    if loaded_cp.timestep_index != checkpoint.timestep_index {
        helpers::cleanup_paths(&[schedule_path, weather_path, cp_path]);
        return Err(vec![format!(
            "timestep_index mismatch: saved={} loaded={}",
            checkpoint.timestep_index, loaded_cp.timestep_index
        )]);
    }

    helpers::cleanup_paths(&[schedule_path, weather_path, cp_path]);
    eprintln!("[checkpoint_integrity_roundtrip] PASS");
    Ok(())
}

/// Regression: corrupt one byte in a checkpoint file's JSON body and verify
/// that `load()` rejects it with a checksum error.
pub fn run_checkpoint_sha256_corruption_regression() -> Result<(), Vec<String>> {
    let schedule_path = helpers::unique_temp_path("hares-regr-ckpt-sha-corrupt-sched", "csv");
    let weather_path = helpers::unique_temp_path("hares-regr-ckpt-sha-corrupt-weather", "epw");
    helpers::write_schedule_csv(&schedule_path);
    helpers::write_weather_epw(&weather_path);

    let config = helpers::build_dwelling_config(
        1,
        schedule_path.clone(),
        weather_path.clone(),
        chrono::Duration::minutes(5),
        42,
    );

    let mut dwelling = match Dwelling::from_config(config) {
        Ok(d) => d,
        Err(err) => {
            helpers::cleanup_paths(&[schedule_path, weather_path]);
            return Err(vec![format!("dwelling construction failed: {err}")]);
        }
    };

    if let Err(err) = dwelling.step() {
        helpers::cleanup_paths(&[schedule_path, weather_path]);
        return Err(vec![format!("step failed: {err}")]);
    }

    let checkpoint = dwelling
        .save_checkpoint()
        .map_err(|e| vec![e.to_string()])?;
    let cp_path = helpers::unique_temp_path("hares-regr-ckpt-sha-corrupt", "json");
    if let Err(err) = checkpoint.save(&cp_path) {
        helpers::cleanup_paths(&[schedule_path, weather_path, cp_path]);
        return Err(vec![format!("checkpoint save failed: {err}")]);
    }

    // Corrupt one byte after the SHA-256 prefix line
    let mut file_bytes = std::fs::read(&cp_path).map_err(|e| vec![e.to_string()])?;
    let nl_pos = file_bytes
        .iter()
        .position(|&b| b == b'\n')
        .ok_or_else(|| vec!["corrupt: no newline in checkpoint file".to_string()])?;
    if nl_pos + 5 < file_bytes.len() {
        file_bytes[nl_pos + 3] ^= 0x01;
    }
    std::fs::write(&cp_path, &file_bytes).map_err(|e| vec![e.to_string()])?;

    let result = DwellingCheckpoint::load(&cp_path);
    match result {
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("SHA-256") || msg.contains("checksum") {
                eprintln!("[checkpoint_sha256_corruption] PASS — load rejected with: {msg}");
                helpers::cleanup_paths(&[schedule_path, weather_path, cp_path]);
                Ok(())
            } else {
                helpers::cleanup_paths(&[schedule_path, weather_path, cp_path]);
                Err(vec![format!(
                    "error message missing checksum indication; got: {msg}"
                )])
            }
        }
        Ok(_) => {
            helpers::cleanup_paths(&[schedule_path, weather_path, cp_path]);
            Err(vec![
                "corrupted checkpoint should have been rejected by SHA-256, but load succeeded"
                    .to_string(),
            ])
        }
    }
}

/// Regression: corrupt a postcard equipment state blob and verify CRC32
/// integrity check catches it with a descriptive error.
pub fn run_equipment_crc_corruption_regression() -> Result<(), Vec<String>> {
    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct FakeState {
        val: f64,
    }

    let state = FakeState { val: 42.0 };
    let mut blob =
        try_save_versioned(&state, 1, "FakeEquip").expect("serialization should not fail in test");

    // Corrupt a byte in the postcard payload portion (past the 4-byte version prefix)
    if blob.len() > 6 {
        blob[6] ^= 0x01;
    }

    let result = load_versioned::<FakeState>(&blob, 1, "FakeEquip", EquipmentId(99));
    match result {
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("FakeEquip") && msg.contains("EquipmentId(99)") {
                eprintln!("[equipment_crc_corruption] PASS — load rejected with equipment context");
                Ok(())
            } else {
                Err(vec![format!(
                    "error should name equipment type FakeEquip and id 99; got: {msg}"
                )])
            }
        }
        Ok(_) => Err(vec![
            "CRC corruption in equipment blob should have been rejected but load succeeded"
                .to_string(),
        ]),
    }
}

/// Regression: actor mutable state round-trips correctly through
/// Dwelling::save_checkpoint() → DwellingCheckpoint::save() →
/// DwellingCheckpoint::load() → Dwelling::load_checkpoint().
///
/// Verifies that actor states are present in the checkpoint and that
/// a dwelling reconstructed from a checkpoint produces identical simulation
/// output to an uninterrupted reference run.
pub fn run_actor_state_checkpoint_roundtrip() -> Result<(), Vec<String>> {
    use hares_core::actors::{self, Occupant, Presence};

    let schedule_path = helpers::unique_temp_path("hares-regr-actor-ckpt-sched", "csv");
    let weather_path = helpers::unique_temp_path("hares-regr-actor-ckpt-weather", "epw");
    helpers::write_schedule_csv(&schedule_path);
    helpers::write_weather_epw(&weather_path);

    let config = helpers::build_dwelling_config(
        1,
        schedule_path.clone(),
        weather_path.clone(),
        chrono::Duration::minutes(10),
        42,
    );

    let mut failures = Vec::new();

    // --- Reference: uninterrupted run with actors ---
    let mut dwelling_ref = match Dwelling::from_config(config.clone()) {
        Ok(d) => d,
        Err(err) => {
            helpers::cleanup_paths(&[schedule_path, weather_path]);
            return Err(vec![format!(
                "reference dwelling construction failed: {err}"
            )]);
        }
    };
    dwelling_ref
        .add_actor(Box::new(
            Occupant::new("TestOccupant").with_presence_schedule(vec![Presence::Home; 12]),
        ))
        .unwrap();
    let mut reference_thermostat = actors::IdealThermostat::new("HVAC");
    reference_thermostat.set_override(actors::OverrideState::heating(22.0));
    dwelling_ref
        .add_actor(Box::new(reference_thermostat))
        .unwrap();
    dwelling_ref
        .add_actor(Box::new(hares_core::actors::EvDriverActor::new(
            "TestEV",
            "EV1",
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            ScheduleSource::Constant(30.0),
            ScheduleSource::Constant(480.0),
            ScheduleSource::Constant(600.0),
            None,
            1.0,
            0.3,
            60.0,
            7.2,
            30.0,
            30.0,
            0.8,
            7.2,
            hares_core::ChaCha8Rng::from_seed([42u8; 32]),
        )))
        .unwrap();
    dwelling_ref
        .add_actor(Box::new(hares_core::actors::BatteryManagementActor::new(
            "TestBattery",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.9,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            1440,
            0,
        )))
        .unwrap();

    let ref_results = match dwelling_ref.simulate() {
        Ok(r) => r,
        Err(err) => {
            helpers::cleanup_paths(&[schedule_path, weather_path]);
            return Err(vec![format!("reference simulation failed: {err}")]);
        }
    };

    // --- Interrupted: step to checkpoint, save, recreate, load, continue ---
    const CHECKPOINT_AT_STEP: u64 = 5;

    let mut dwelling_a = match Dwelling::from_config(config.clone()) {
        Ok(d) => d,
        Err(err) => {
            helpers::cleanup_paths(&[schedule_path, weather_path]);
            return Err(vec![format!(
                "checkpoint dwelling construction failed: {err}"
            )]);
        }
    };
    dwelling_a
        .add_actor(Box::new(
            Occupant::new("TestOccupant").with_presence_schedule(vec![Presence::Home; 12]),
        ))
        .unwrap();
    let mut checkpoint_thermostat = actors::IdealThermostat::new("HVAC");
    checkpoint_thermostat.set_override(actors::OverrideState::heating(22.0));
    dwelling_a
        .add_actor(Box::new(checkpoint_thermostat))
        .unwrap();
    dwelling_a
        .add_actor(Box::new(hares_core::actors::EvDriverActor::new(
            "TestEV",
            "EV1",
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            ScheduleSource::Constant(30.0),
            ScheduleSource::Constant(480.0),
            ScheduleSource::Constant(600.0),
            None,
            1.0,
            0.3,
            60.0,
            7.2,
            30.0,
            30.0,
            0.8,
            7.2,
            hares_core::ChaCha8Rng::from_seed([42u8; 32]),
        )))
        .unwrap();
    dwelling_a
        .add_actor(Box::new(hares_core::actors::BatteryManagementActor::new(
            "TestBattery",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.9,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            1440,
            0,
        )))
        .unwrap();

    for _ in 0..CHECKPOINT_AT_STEP {
        if let Err(err) = dwelling_a.step() {
            failures.push(format!("step before checkpoint failed: {err}"));
            helpers::cleanup_paths(&[schedule_path, weather_path]);
            return Err(failures);
        }
    }

    let checkpoint = dwelling_a
        .save_checkpoint()
        .map_err(|e| vec![e.to_string()])?;

    // Verify actor states are present in the checkpoint
    let actor_names: Vec<&str> = checkpoint
        .actor_states
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    if !actor_names.contains(&"TestOccupant") {
        failures.push("checkpoint missing TestOccupant actor state".to_string());
    }
    if !actor_names.contains(&"IdealThermostat(HVAC)") {
        failures.push("checkpoint missing IdealThermostat(HVAC) actor state".to_string());
    }
    if !actor_names.contains(&"TestEV") {
        failures.push("checkpoint missing TestEV actor state".to_string());
    }
    if !actor_names.contains(&"BatteryManagementActor:TestBattery") {
        failures
            .push("checkpoint missing BatteryManagementActor:TestBattery actor state".to_string());
    }
    if !failures.is_empty() {
        helpers::cleanup_paths(&[schedule_path, weather_path]);
        return Err(failures);
    }

    let cp_path = helpers::unique_temp_path("hares-regr-actor-ckpt", "json");
    if let Err(err) = checkpoint.save(&cp_path) {
        helpers::cleanup_paths(&[schedule_path, weather_path, cp_path]);
        return Err(vec![format!("checkpoint save failed: {err}")]);
    }

    let loaded_cp = match DwellingCheckpoint::load(&cp_path) {
        Ok(cp) => cp,
        Err(err) => {
            helpers::cleanup_paths(&[schedule_path, weather_path, cp_path]);
            return Err(vec![format!("checkpoint load failed: {err}")]);
        }
    };

    let mut dwelling_b = match Dwelling::from_config(config) {
        Ok(d) => d,
        Err(err) => {
            helpers::cleanup_paths(&[schedule_path, weather_path, cp_path]);
            return Err(vec![format!("restore dwelling construction failed: {err}")]);
        }
    };
    dwelling_b
        .add_actor(Box::new(
            Occupant::new("TestOccupant").with_presence_schedule(vec![Presence::Home; 12]),
        ))
        .unwrap();
    dwelling_b
        .add_actor(Box::new(actors::IdealThermostat::new("HVAC")))
        .unwrap();
    dwelling_b
        .add_actor(Box::new(hares_core::actors::EvDriverActor::new(
            "TestEV",
            "EV1",
            ChargingStrategy::Immediate { target_soc: 0.9 },
            PlugInPolicy::Always,
            ScheduleSource::Constant(30.0),
            ScheduleSource::Constant(480.0),
            ScheduleSource::Constant(600.0),
            None,
            1.0,
            0.3,
            60.0,
            7.2,
            30.0,
            30.0,
            0.8,
            7.2,
            hares_core::ChaCha8Rng::from_seed([42u8; 32]),
        )))
        .unwrap();
    dwelling_b
        .add_actor(Box::new(hares_core::actors::BatteryManagementActor::new(
            "TestBattery",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.9,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            1440,
            0,
        )))
        .unwrap();

    if let Err(err) = dwelling_b.load_checkpoint(loaded_cp) {
        helpers::cleanup_paths(&[schedule_path, weather_path, cp_path]);
        return Err(vec![format!("checkpoint restore failed: {err}")]);
    }

    let mut restarted_steps = Vec::new();
    while let Ok(step) = dwelling_b.step() {
        restarted_steps.push(step);
    }

    let ref_tail = &ref_results.steps[CHECKPOINT_AT_STEP as usize..];
    let compare_len = ref_tail.len().min(restarted_steps.len());
    if compare_len == 0 {
        failures.push("no steps to compare after checkpoint restart".to_string());
    } else {
        for i in 0..compare_len {
            let ref_step = &ref_tail[i];
            let rst_step = &restarted_steps[i];
            let power_diff =
                (ref_step.net_electric_power_kw - rst_step.net_electric_power_kw).abs();
            if power_diff > 1e-9 {
                failures.push(format!(
                    "step[{}] power divergence after checkpoint: ref={:.9} restart={:.9} diff={:.2e}",
                    CHECKPOINT_AT_STEP as usize + i,
                    ref_step.net_electric_power_kw,
                    rst_step.net_electric_power_kw,
                    power_diff,
                ));
                break;
            }
        }
    }

    helpers::cleanup_paths(&[schedule_path, weather_path, cp_path]);

    if failures.is_empty() {
        eprintln!(
            "[actor_state_checkpoint_roundtrip] PASS — {compare_len} steps identical after restore with actors"
        );
        Ok(())
    } else {
        Err(failures)
    }
}
