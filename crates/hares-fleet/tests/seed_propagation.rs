//! The fleet's master-seed propagation: a home's stochastic streams under
//! the fleet are the streams the same home runs standalone with the same
//! master seed and building id, bitwise.
//!
//! The seeding contract: every home's streams derive from the master seed
//! and the home's own building id (`hares_types::rng::dwelling_seed`), so
//! the fleet's parallel stepping must not re-seed, share or advance any
//! home's streams differently than a standalone build does. The fixture is
//! the EV-charging parity home (an EV driver actor and event-load
//! equipment: every seeded component class), two homes of it, and the
//! fleet's per-step series compared bitwise against two standalone runs.
//!
//! Two seeds under one master seed must also come out different: the
//! building id mixes the derivation, so two homes that differ in nothing
//! else draw different streams.

use std::path::PathBuf;

use chrono::{DateTime, Duration};
use hares_core::{Dwelling, DwellingConfig, DwellingSettings};
use hares_fleet::SteppableFleet;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cz5a_config(master_seed: u64, bldg_id: i64) -> DwellingConfig {
    let fixture = project_root().join("tests/fixtures/parity/cz5a_ev_charging");
    DwellingConfig {
        hpxml: hares_io::InputSource::Path(fixture.join("building.xml")),
        schedule: Some(hares_io::InputSource::Path(fixture.join("schedule.csv"))),
        weather: hares_io::WeatherInput::Path(fixture.join("weather.epw")),
        defaults: hares_io::DefaultsSource::directory(project_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(
            hares_io::SimulationConfig {
                scenario_seed: None,
                schedule_diversity: Default::default(),
                start_time: DateTime::parse_from_rfc3339("2023-01-01T00:00:00-07:00")
                    .expect("valid start time"),
                duration: Duration::seconds(2 * 86_400),
                time_res: Duration::seconds(900),
                reports: Vec::new(),
                diagnostics: false,
                metrics: false,
                output_chunk_size: 1024,
                setpoint_deadband_c: None,
                master_seed,
                civil_timezone: None,
                site_location: hares_io::SiteLocationOverride::default(),
                max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                health_policy: hares_io::HealthPolicy::default(),
                first_hour_interpolation: Default::default(),
                weather_year: hares_io::WeatherYear::default(),
                warmup: hares_io::Warmup::Disabled,
            },
            None,
            bldg_id,
            None,
        ),
    }
}

/// A standalone dwelling's per-step series over its full horizon: the net
/// electric power and the telemetry's per-equipment power and SOC
/// channels, bitwise per step.
fn standalone_series(config: DwellingConfig) -> Vec<Vec<u64>> {
    let mut dwelling = Dwelling::from_config(config).expect("the dwelling builds");
    let mut bits = Vec::new();
    for _ in 0..dwelling.clock.total_steps() {
        let step = dwelling.step().expect("the step runs");
        let telem = dwelling.telemetry().expect("telemetry");
        let mut step_bits = vec![step.net_electric_power_w.to_bits()];
        for p in &telem.equipment_power_w {
            step_bits.push(p.to_bits());
        }
        for s in &telem.equipment_soc {
            step_bits.push(s.to_bits());
        }
        bits.push(step_bits);
    }
    bits
}

/// A fleet's per-home per-step series over its full horizon.
fn fleet_series(configs: Vec<DwellingConfig>) -> Vec<Vec<Vec<u64>>> {
    let n_threads = 2;
    let (mut fleet, build_errors) =
        SteppableFleet::from_configs(configs, n_threads).expect("the fleet builds");
    assert!(
        build_errors.is_empty(),
        "the fleet's homes must all build: {build_errors:?}"
    );
    let homes = fleet.len();
    let mut series: Vec<Vec<Vec<u64>>> = vec![Vec::new(); homes];
    while !fleet.is_finished() {
        let results = fleet.step().expect("the fleet steps");
        for (home, result) in results.iter().enumerate() {
            let step = result.as_ref().expect("the home's step runs");
            let telem = fleet.telemetry(home).expect("the home's telemetry");
            let mut step_bits = vec![step.net_electric_power_w.to_bits()];
            for p in &telem.equipment_power_w {
                step_bits.push(p.to_bits());
            }
            for s in &telem.equipment_soc {
                step_bits.push(s.to_bits());
            }
            series[home].push(step_bits);
        }
    }
    series
}

/// Under the fleet, each home's series is bitwise the standalone run's
/// series with the same master seed and building id.
#[test]
fn fleet_stepping_propagates_the_master_seed_bitwise() {
    let fleet = fleet_series(vec![cz5a_config(42, 1), cz5a_config(42, 2)]);
    let standalone_1 = standalone_series(cz5a_config(42, 1));
    let standalone_2 = standalone_series(cz5a_config(42, 2));

    assert_eq!(
        fleet[0], standalone_1,
        "fleet home 1's stochastic series must be bitwise the standalone \
         run's: a difference is a stream the fleet's stepping re-seeded, \
         shared or advanced differently"
    );
    assert_eq!(
        fleet[1], standalone_2,
        "fleet home 2's stochastic series must be bitwise the standalone \
         run's"
    );
}

/// Two homes that differ only in building id draw different streams under
/// the same master seed, and the same home under a different master seed
/// draws differently again: the master seed propagates through the
/// derivation to every home.
#[test]
fn the_master_seed_and_the_building_id_mix_in_the_streams() {
    let fleet = fleet_series(vec![
        cz5a_config(42, 1),
        cz5a_config(42, 2),
        cz5a_config(43, 1),
    ]);
    assert_ne!(
        fleet[0], fleet[1],
        "two homes identical but for their building id must draw different \
         streams under one master seed"
    );
    assert_ne!(
        fleet[0], fleet[2],
        "the same home under a different master seed must draw different \
         streams"
    );
}
