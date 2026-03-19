//! Checkpoint restart sub-suite: checkpoint at step N, restart, verify identical
//! continuation compared to an uninterrupted run.

use chrono::Duration;
use hares_core::Dwelling;

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
    let mut dwelling_ref = match Dwelling::new(config.clone()) {
        Ok(d) => d,
        Err(err) => {
            helpers::cleanup_paths(&[schedule_path, weather_path]);
            return Err(vec![format!("reference dwelling construction failed: {err}")]);
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
    let mut dwelling_a = match Dwelling::new(config.clone()) {
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

    let checkpoint = dwelling_a.save_checkpoint();

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

    let mut dwelling_b = match Dwelling::new(config) {
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
    loop {
        match dwelling_b.step() {
            Ok(step) => restarted_steps.push(step),
            Err(_) => break,
        }
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

            let power_diff = (ref_step.net_electric_power_kw - rst_step.net_electric_power_kw).abs();
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
        eprintln!("[checkpoint_restart] PASS — {compare_len} steps identical after restore");
        Ok(())
    } else {
        Err(failures)
    }
}
