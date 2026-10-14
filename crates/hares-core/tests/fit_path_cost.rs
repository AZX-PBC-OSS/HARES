//! The fit path's per-candidate cost: the HPXML parse plus the build from
//! shared handles (the calibration batch's shape after this row) against
//! a full parse of every input (the parent's shape), on the
//! consumer_shape_900s inputs. Ignored: run by hand in the canonical
//! release build, the numbers recorded in the measurement log.

use std::path::PathBuf;
use std::time::Instant;

use chrono::{FixedOffset, TimeZone};
use hares_core::{Dwelling, DwellingBlueprintBuilder, DwellingConfig, DwellingSettings};
use hares_io::{
    DefaultsSource, InputSource, ParsedDefaults, ParsedHpxml, ParsedInputs, ParsedSchedule,
    ParsedWeather, SimulationConfig, WeatherInput,
};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture() -> PathBuf {
    project_root().join("tests/fixtures/resstock/2025.1/bldg0176227")
}

fn sim_config() -> SimulationConfig {
    let start = FixedOffset::east_opt(-8 * 3600)
        .expect("offset")
        .with_ymd_and_hms(2018, 1, 1, 0, 0, 0)
        .single()
        .expect("start");
    SimulationConfig {
        start_time: start,
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
    }
}

/// The per-candidate settings the fit path edits: the candidate's own
/// override (the free factor's trial value).
fn settings(ua_w_per_k: f64, master_seed: u64) -> DwellingSettings {
    DwellingSettings::new(
        sim_config(),
        Some(serde_json::json!({ "Gas Water Heater": { "ua_w_per_k": ua_w_per_k } })),
        176_227,
        None,
        None,
    )
    .pipe(|mut settings| {
        settings.sim_config.master_seed = master_seed;
        settings
    })
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}
impl<T> Pipe for T {}

#[test]
#[ignore = "the fit path's per-candidate cost, run by hand in the release build"]
fn fit_path_per_candidate_cost() {
    let weather = project_root().join("tests/fixtures/resstock/2025.1/weather/G0600770_2018.csv");
    let defaults = DefaultsSource::directory(project_root().join("defaults"));
    let candidates: Vec<f64> = (0..50).map(|idx| 2.0 + idx as f64 * 0.1).collect();

    // The shared handles, parsed once: the batch's weather, schedule and
    // defaults (the schedule generated from the profiles; the fixture's
    // own one it carries is the same source every candidate reads).
    let started = Instant::now();
    let shared_weather = std::sync::Arc::new(
        ParsedWeather::parse(&WeatherInput::Path(weather.clone())).expect("the weather parses"),
    );
    let shared_defaults =
        std::sync::Arc::new(ParsedDefaults::parse(&defaults).expect("the defaults load"));
    let shared_schedule = std::sync::Arc::new(
        ParsedSchedule::generate(
            sim_config().start_time,
            sim_config().duration,
            std::time::Duration::from_secs(900),
            &shared_defaults.profiles(),
        )
        .expect("the schedule generates"),
    );
    let shared_parse = started.elapsed();

    // The per-candidate cost from the shared handles: the candidate's own
    // HPXML parse plus the build.
    let started = Instant::now();
    for (idx, ua) in candidates.iter().enumerate() {
        let inputs = ParsedInputs::from_handles(
            std::sync::Arc::new(
                ParsedHpxml::parse(&InputSource::Path(fixture().join("home.xml")))
                    .expect("the hpxml parses"),
            ),
            Some(std::sync::Arc::clone(&shared_schedule)),
            std::sync::Arc::clone(&shared_weather),
            std::sync::Arc::clone(&shared_defaults),
            None,
        )
        .expect("the shared inputs assemble");
        let builder = DwellingBlueprintBuilder::new(inputs, settings(*ua, idx as u64))
            .expect("the builder builds");
        let blueprint = builder.freeze().expect("the blueprint freezes");
        let _dwelling = blueprint.build().expect("the candidate builds");
    }
    let from_shared = started.elapsed();

    // The per-candidate cost with a full parse of every input (the
    // parent's shape).
    let started = Instant::now();
    for (idx, ua) in candidates.iter().enumerate() {
        let config = DwellingConfig {
            hpxml: InputSource::Path(fixture().join("home.xml")),
            schedule: None,
            weather: WeatherInput::Path(weather.clone()),
            defaults: defaults.clone(),
            settings: settings(*ua, idx as u64),
        };
        let _dwelling = Dwelling::from_config(config).expect("the candidate builds");
    }
    let full_parse = started.elapsed();

    eprintln!(
        "the fit path per candidate (50 candidates, consumer_shape_900s inputs): \
         the shared parses {shared_parse:?} once, then HPXML parse plus build from the shared \
         handles {:?} per candidate; a full parse of every input {:?} per candidate",
        from_shared / candidates.len() as u32,
        full_parse / candidates.len() as u32,
    );
}
