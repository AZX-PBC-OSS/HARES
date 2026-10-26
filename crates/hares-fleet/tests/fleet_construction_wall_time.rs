//! The many-home construction's wall-time measurement: 1000 homes
//! sharing one weather file, one schedule file and one defaults tree
//! (the ResStock shape: unique HPXML per home, everything else shared),
//! the fleet built through the shared parses. Ignored: run by hand in
//! the canonical release build, the number recorded in the measurement
//! log.

use std::path::PathBuf;
use std::time::Instant;

use chrono::{FixedOffset, TimeZone};
use hares_core::{DwellingConfig, DwellingSettings};
use hares_fleet::SteppableFleet;
use hares_io::{DefaultsSource, InputSource, SimulationConfig, WeatherInput};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
#[ignore = "the 1000-home construction's wall time, run by hand in the release build"]
fn fleet_construction_1000_homes_wall_time() {
    let homes = 1000;
    let dir = tempfile::tempdir().expect("temp dir");
    let fixture = project_root().join("tests/fixtures/resstock/2025.1/bldg0000002");
    let weather_src =
        project_root().join("tests/fixtures/resstock/2025.1/weather/G0900090_2018.csv");

    let weather_path = dir.path().join("weather.csv");
    std::fs::copy(&weather_src, &weather_path).expect("the weather copies");

    let start_time = FixedOffset::west_opt(5 * 3600)
        .expect("offset")
        .with_ymd_and_hms(2018, 1, 15, 0, 0, 0)
        .single()
        .expect("start");
    let sim_config = SimulationConfig {
        scenario_seed: None,
        start_time,
        duration: chrono::Duration::hours(24),
        time_res: chrono::Duration::seconds(900),
        reports: Vec::new(),
        diagnostics: false,
        metrics: false,
        output_chunk_size: 10_000,
        setpoint_deadband_c: None,
        master_seed: 0,
        civil_timezone: None,
        site_location: hares_io::SiteLocationOverride::default(),
        max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
        health_policy: hares_io::HealthPolicy::default(),
        first_hour_interpolation: Default::default(),
        weather_year: hares_io::WeatherYear::default(),
        warmup: hares_io::Warmup::default(),
    };

    let configs: Vec<DwellingConfig> = (0..homes)
        .map(|idx| {
            let home = dir.path().join(format!("bldg{idx:07}"));
            std::fs::create_dir_all(&home).expect("the home dir creates");
            std::fs::copy(fixture.join("home.xml"), home.join("home.xml"))
                .expect("the hpxml copies");
            std::fs::copy(
                fixture.join("in.schedules.csv"),
                home.join("in.schedules.csv"),
            )
            .expect("the schedule copies");
            DwellingConfig {
                hpxml: InputSource::Path(home.join("home.xml")),
                schedule: Some(InputSource::Path(home.join("in.schedules.csv"))),
                weather: WeatherInput::Path(weather_path.clone()),
                defaults: DefaultsSource::directory(project_root().join("python/hares/defaults")),
                settings: DwellingSettings::new(sim_config.clone(), None, idx as i64, None),
            }
        })
        .collect();

    // Warm the file cache once, untimed.
    drop(SteppableFleet::from_configs(configs[..2].to_vec(), 0).expect("the warm-up builds"));

    let started = Instant::now();
    let (fleet, errors) = SteppableFleet::from_configs(configs, 0).expect("the fleet builds");
    let wall = started.elapsed();
    assert!(errors.is_empty(), "no home errors: {errors:?}");
    assert_eq!(fleet.len(), homes);
    eprintln!("the 1000-home construction (one shared weather file, one defaults tree): {wall:?}");
}
