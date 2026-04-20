//! Empirical diagnostic comparing BESTEST 900FF and 600FF free-float behavior.
//!
//! Runs both cases for a full annual simulation (8760 steps), tracks zone and
//! outdoor temperatures at every timestep, and prints detailed diagnostics
//! around the annual minimum temperature step including component heat flows
//! and the 900FF-vs-600FF temperature delta.

use hares_core::Dwelling;

use super::cases::core_cases;

/// Per-step snapshot used during the first (data-collection) pass.
struct StepSnapshot {
    zone_temp_c: f64,
    outdoor_temp_c: f64,
}

/// Run a full 8760-step annual simulation, recording zone and outdoor temps
/// at every timestep. Returns the complete time-series.
fn run_annual(case_id: &str) -> Vec<StepSnapshot> {
    let case = core_cases()
        .into_iter()
        .find(|c| c.id == case_id)
        .unwrap_or_else(|| panic!("unknown BESTEST case: {case_id}"));

    let mut dwelling = Dwelling::from_toml_config_with_write_output(&case.fixture_path(), Some(false))
        .unwrap_or_else(|err| panic!("failed to load BESTEST case {case_id}: {err}"));

    let total_steps = 8760usize;
    let mut snapshots = Vec::with_capacity(total_steps);

    for step in 0..total_steps {
        let result = dwelling
            .step()
            .unwrap_or_else(|err| panic!("step {step} failed for {case_id}: {err}"));

        let zone_temp: f64 = result
            .zone_temperatures_c
            .iter()
            .map(|(_, t)| *t)
            .fold(f64::NEG_INFINITY, f64::max);

        let outdoor_temp = dwelling.latest_env().weather.outdoor_temp_c;

        snapshots.push(StepSnapshot {
            zone_temp_c: zone_temp,
            outdoor_temp_c: outdoor_temp,
        });
    }

    snapshots
}

/// Reload a case, re-run to (and including) `target_step`, then print the
/// component gains that were active during that step.
fn print_component_gains_at(case_id: &str, target_step: usize) {
    let case = core_cases()
        .into_iter()
        .find(|c| c.id == case_id)
        .unwrap_or_else(|| panic!("unknown BESTEST case: {case_id}"));

    let mut dwelling = Dwelling::from_toml_config_with_write_output(&case.fixture_path(), Some(false))
        .unwrap_or_else(|err| panic!("failed to reload BESTEST case {case_id}: {err}"));

    // Run up to and including target_step (0-indexed).
    for _ in 0..=target_step {
        let _ = dwelling.step();
    }

    let gains = dwelling.thermal_solver.component_gains();

    eprintln!(
        "[diagnostic] {case_id} component gains at step {} (hour {}):",
        target_step,
        target_step + 1
    );
    eprintln!("  window_solar_w          = {:>10.1} W", gains.window_solar_w);
    eprintln!(
        "  opaque_solar_lwr_w      = {:>10.1} W",
        gains.opaque_solar_lwr_w
    );
    eprintln!("  opaque_solar_w          = {:>10.1} W", gains.opaque_solar_w);
    eprintln!("  exterior_lwr_w          = {:>10.1} W", gains.exterior_lwr_w);
    eprintln!("  interior_lwr_w          = {:>10.1} W", gains.interior_lwr_w);
    eprintln!("  infiltration_w          = {:>10.1} W", gains.infiltration_w);
    eprintln!("  internal_gain_w         = {:>10.1} W", gains.internal_gain_w);
    eprintln!(
        "  driving_outdoor_temp_c  = {:>10.1} °C",
        gains.driving_outdoor_temp_c
    );
}

#[test]
fn bestest_diagnostic_900ff_600ff_annual() {
    // --- 1. Run both cases for the full annual simulation ---
    eprintln!("[diagnostic] Running 900FF annual simulation (8760 steps)...");
    let data_900ff = run_annual("900FF");
    eprintln!("[diagnostic] 900FF complete ({} steps).", data_900ff.len());

    eprintln!("[diagnostic] Running 600FF annual simulation (8760 steps)...");
    let data_600ff = run_annual("600FF");
    eprintln!("[diagnostic] 600FF complete ({} steps).", data_600ff.len());

    assert_eq!(data_900ff.len(), 8760, "900FF should have 8760 steps");
    assert_eq!(data_600ff.len(), 8760, "600FF should have 8760 steps");

    // --- 2. Find the annual minimum temperature and the step where it occurs ---
    let (min_step_900ff, min_temp_900ff) = data_900ff
        .iter()
        .enumerate()
        .map(|(i, s)| (i, s.zone_temp_c))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .expect("900FF data is non-empty");

    let (min_step_600ff, min_temp_600ff) = data_600ff
        .iter()
        .enumerate()
        .map(|(i, s)| (i, s.zone_temp_c))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .expect("600FF data is non-empty");

    eprintln!(
        "[diagnostic] 900FF min temp = {:.4}°C at step {} (hour {})",
        min_temp_900ff, min_step_900ff, min_step_900ff + 1
    );
    eprintln!(
        "[diagnostic] 600FF min temp = {:.4}°C at step {} (hour {})",
        min_temp_600ff, min_step_600ff, min_step_600ff + 1
    );

    // Use the 900FF minimum as the reference window — 900FF is the heavyweight
    // case whose minimum is known to overshoot the ASHRAE band.
    let ref_step = min_step_900ff;

    // --- 3 & 4. 48-hour window around the minimum ---
    let win_start = ref_step.saturating_sub(24);
    let win_end = (ref_step + 24).min(data_900ff.len());
    eprintln!(
        "[diagnostic] === 48-hour window around 900FF min (hours {}..{}) ===",
        win_start + 1,
        win_end
    );
    eprintln!(
        "[diagnostic] {:>5}  {:>12}  {:>12}  {:>12}  {:>12}",
        "hour", "900FF_zone", "900FF_outdoor", "600FF_zone", "600FF_outdoor"
    );
    for h in win_start..win_end {
        let d9 = &data_900ff[h];
        let d6 = &data_600ff[h];
        eprintln!(
            "[diagnostic] {:>5}  {:>12.3}  {:>12.3}  {:>12.3}  {:>12.3}",
            h + 1, d9.zone_temp_c, d9.outdoor_temp_c, d6.zone_temp_c, d6.outdoor_temp_c
        );
    }

    // --- 5. Component heat flows at the minimum temperature step ---
    print_component_gains_at("900FF", min_step_900ff);
    print_component_gains_at("600FF", min_step_600ff);

    // --- 6. Compare 900FF vs 600FF: zone temperature delta at each hour of the minimum day ---
    // "Minimum day" = 24-hour window centred on the 900FF minimum step.
    let day_start = ref_step.saturating_sub(12);
    let day_end = (ref_step + 12)
        .min(data_900ff.len())
        .min(data_600ff.len());
    eprintln!(
        "[diagnostic] === Minimum day 900FF vs 600FF delta (hours {}..{}) ===",
        day_start + 1,
        day_end
    );
    eprintln!(
        "[diagnostic] {:>5}  {:>12}  {:>12}  {:>12}",
        "hour", "900FF_zone", "600FF_zone", "delta_°C"
    );
    for h in day_start..day_end {
        let t9 = data_900ff[h].zone_temp_c;
        let t6 = data_600ff[h].zone_temp_c;
        let delta = t9 - t6;
        eprintln!(
            "[diagnostic] {:>5}  {:>12.3}  {:>12.3}  {:>12.3}",
            h + 1, t9, t6, delta
        );
    }
}
