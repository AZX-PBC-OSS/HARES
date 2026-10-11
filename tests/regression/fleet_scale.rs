//! Fleet scale sub-suite: 1000 dwellings must complete without OOM on 32 GB.

use std::num::NonZeroUsize;

use chrono::Duration;
use hares_fleet::{Fleet, FleetReport};
use hares_io::{Frequency, Selection};

use super::helpers;

const FLEET_SIZE: usize = 1000;
const SIM_DURATION_HOURS: i64 = 24;

pub fn run_fleet_scale_check() -> Result<(), Vec<String>> {
    let scratch = tempfile::tempdir().expect("create scratch directory");
    let schedule_path = scratch.path().join("schedule.csv");
    let weather_path = scratch.path().join("weather.epw");
    helpers::write_schedule_csv(&schedule_path);
    helpers::write_weather_epw(&weather_path);

    let configs: Vec<_> = (0..FLEET_SIZE)
        .map(|idx| {
            helpers::build_dwelling_config(
                scratch.path(),
                idx as i64 + 1,
                schedule_path.clone(),
                weather_path.clone(),
                Duration::hours(SIM_DURATION_HOURS),
                0,
            )
        })
        .collect();

    let fleet =
        Fleet::from_buildings(configs).map_err(|err| vec![format!("fleet construction: {err}")])?;
    let report = FleetReport {
        selection: Selection::Verbosity(0),
        frequency: Frequency::Hourly,
        health_policy: hares_io::HealthPolicy::default(),
    };
    let result = fleet
        .simulate(&report, 0, NonZeroUsize::new(64).expect("64"))
        .map_err(|err| vec![format!("fleet simulate: {err}")])?;

    let mut failures = Vec::new();

    if result.homes.len() != FLEET_SIZE {
        failures.push(format!(
            "expected {FLEET_SIZE} homes, got {}",
            result.homes.len()
        ));
    }

    let mut inspected = 0usize;
    for home in result.homes.iter() {
        match &home.status {
            hares_fleet::SimStatus::Failed(reason) => {
                failures.push(format!("home {} failed: {reason}", home.bldg_id));
            }
            status => {
                if inspected < 5 {
                    let total = home
                        .metrics
                        .as_ref()
                        .map(|m| m.total_energy_kwh.net_energy_kwh);
                    match total {
                        Some(total) if total > 0.0 && total.is_finite() => {}
                        Some(total) => failures.push(format!(
                            "home[{}] {status:?} but net_energy_kwh={total} (expected > 0.0)",
                            home.bldg_id
                        )),
                        None => {}
                    }
                    inspected += 1;
                }
            }
        }
    }

    eprintln!(
        "[fleet_scale] {FLEET_SIZE} dwellings: {} ok, {} failed",
        result.n_succeeded,
        result.homes.len() - result.n_succeeded
    );

    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures)
    }
}
