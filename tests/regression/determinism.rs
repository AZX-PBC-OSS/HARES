//! Determinism sub-suite: same seed must produce identical trajectories
//! regardless of thread count.
//!
//! Uses the real BEopt_example + ResStock buildings from the OCHRE vendor
//! directory so the simulation exercises realistic equipment and envelope paths.

use chrono::Duration;
use hares_core::SimulationEngine;

use super::helpers;

const THREAD_COUNTS: [usize; 3] = [1, 4, 8];
const SIM_DURATION_HOURS: i64 = 24;
const SEED: u64 = 42;

pub fn run_determinism_checks() -> Result<(), Vec<String>> {
    helpers::assert_vendor_fixtures_exist();

    let engine = SimulationEngine::new();
    let mut failures = Vec::new();
    let duration = Duration::hours(SIM_DURATION_HOURS);

    let buildings: Vec<(&str, Box<dyn Fn(i64, Duration, u64) -> hares_core::DwellingConfig>)> = vec![
        ("beopt_1", Box::new(|id, dur, s| helpers::build_beopt_dwelling_config(id, dur, s))),
        ("beopt_2", Box::new(|id, dur, s| helpers::build_beopt_dwelling_config(id, dur, s))),
        ("resstock", Box::new(|id, dur, s| helpers::build_resstock_dwelling_config(id, dur, s))),
    ];

    for (label, builder) in &buildings {
        let mut trajectories: Vec<(usize, Vec<f64>)> = Vec::new();

        for &n_threads in &THREAD_COUNTS {
            let config = builder(1, duration, SEED);

            std::env::set_var("RAYON_NUM_THREADS", n_threads.to_string());

            match engine.run(config) {
                Ok(result) => {
                    let mut trajectory: Vec<f64> = result
                        .metrics
                        .annual_energy_kwh
                        .per_end_use
                        .values()
                        .copied()
                        .collect();
                    trajectory.sort_by(|a, b| a.total_cmp(b));
                    trajectories.push((n_threads, trajectory));
                }
                Err(err) => {
                    failures.push(format!(
                        "building={label} threads={n_threads} failed: {err}"
                    ));
                }
            }
        }

        if trajectories.len() >= 2 {
            let (ref_threads, ref_traj) = &trajectories[0];
            for (other_threads, other_traj) in &trajectories[1..] {
                if ref_traj.len() != other_traj.len() {
                    failures.push(format!(
                        "building={label}: metric count mismatch threads={ref_threads} ({}) vs threads={other_threads} ({})",
                        ref_traj.len(), other_traj.len()
                    ));
                    continue;
                }
                for (idx, (a, b)) in ref_traj.iter().zip(other_traj.iter()).enumerate() {
                    if (a - b).abs() > f64::EPSILON {
                        failures.push(format!(
                            "building={label}: metric[{idx}] differs -- threads={ref_threads} → {a}, threads={other_threads} → {b}"
                        ));
                        break;
                    }
                }
            }
        }
    }

    if failures.is_empty() {
        eprintln!(
            "[determinism] PASS -- 3 buildings × {} thread counts all identical",
            THREAD_COUNTS.len()
        );
        Ok(())
    } else {
        Err(failures)
    }
}
