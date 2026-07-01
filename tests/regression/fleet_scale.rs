//! Fleet scale sub-suite: 1000 dwellings must complete without OOM on 32 GB.

use chrono::Duration;
use hares_fleet::Fleet;

use super::helpers;

const FLEET_SIZE: usize = 1000;
const SIM_DURATION_HOURS: i64 = 24;

pub fn run_fleet_scale_check() -> Result<(), Vec<String>> {
    let schedule_path = helpers::unique_temp_path("hares-regr-fleet-sched", "csv");
    let weather_path = helpers::unique_temp_path("hares-regr-fleet-weather", "epw");
    helpers::write_schedule_csv(&schedule_path);
    helpers::write_weather_epw(&weather_path);

    let configs: Vec<_> = (0..FLEET_SIZE)
        .map(|idx| {
            helpers::build_dwelling_config(
                idx as i64 + 1,
                schedule_path.clone(),
                weather_path.clone(),
                Duration::hours(SIM_DURATION_HOURS),
                0,
            )
        })
        .collect();

    let fleet = Fleet::from_buildings(configs);
    let outcomes = fleet.simulate(0);

    let mut failures = Vec::new();

    if outcomes.len() != FLEET_SIZE {
        failures.push(format!(
            "expected {FLEET_SIZE} outcomes, got {}",
            outcomes.len()
        ));
    }

    let mut ok_count = 0usize;
    let mut fail_count = 0usize;
    let mut inspected = 0usize;
    for (idx, outcome) in outcomes.iter().enumerate() {
        match outcome {
            Ok(dwelling_outcome) => {
                ok_count += 1;
                if inspected < 5 {
                    let total = dwelling_outcome
                        .result
                        .metrics
                        .total_energy_kwh
                        .net_energy_kwh;
                    if total <= 0.0 || !total.is_finite() {
                        failures.push(format!(
                            "dwelling[{idx}] Ok but net_energy_kwh={total} (expected > 0.0)"
                        ));
                    }
                    inspected += 1;
                }
            }
            Err(err) => {
                fail_count += 1;
                if fail_count <= 5 {
                    failures.push(format!("dwelling[{idx}] failed: {err}"));
                }
            }
        }
    }

    if fail_count > 5 {
        failures.push(format!("... and {} more failures", fail_count - 5));
    }

    eprintln!("[fleet_scale] {FLEET_SIZE} dwellings: {ok_count} ok, {fail_count} failed");

    helpers::cleanup_paths(&[schedule_path, weather_path]);

    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures)
    }
}
