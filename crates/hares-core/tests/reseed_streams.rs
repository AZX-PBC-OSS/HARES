//! The reseed contract: `Dwelling::reseed` re-derives every random stream
//! the dwelling holds, with the stream indices construction uses, each at
//! its start — and nothing is re-constructed. The fixture is the
//! EV-charging parity home (its dwelling holds an EV driver actor and
//! event-load equipment, every seeded component class).

use std::path::PathBuf;

use chrono::{DateTime, Duration};
use hares_core::{Dwelling, DwellingConfig, DwellingSettings};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cz5a_config(master_seed: u64, warmup: bool) -> DwellingConfig {
    let fixture = project_root().join("tests/fixtures/parity/cz5a_ev_charging");
    DwellingConfig {
        hpxml: hares_io::InputSource::Path(fixture.join("building.xml")),
        schedule: Some(hares_io::InputSource::Path(fixture.join("schedule.csv"))),
        weather: hares_io::WeatherInput::Path(fixture.join("weather.epw")),
        defaults: hares_io::DefaultsSource::directory(project_root().join("defaults")),
        settings: DwellingSettings::new(
            hares_io::SimulationConfig {
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
            },
            None,
            1,
            // Warm-up on for the stepped dwelling; None for the fresh
            // build whose construction assignment the reseeded streams
            // must equal (a warmed build's streams have already advanced
            // past their starts).
            if warmup {
                Some(std::time::Duration::from_secs(7 * 24 * 3600))
            } else {
                None
            },
            None,
        ),
    }
}

/// A dwelling stepped 96 times and then reseeded with 7 holds, for every
/// seeded component, the stream (seed, stream index, word position) a
/// fresh build with seed 7 holds at construction.
#[test]
fn reseed_resets_every_stream() {
    let mut stepped = Dwelling::from_config(cz5a_config(42, true)).expect("the dwelling builds");
    assert!(
        !stepped.seeded_streams().is_empty(),
        "the EV fixture's dwelling holds seeded components"
    );
    for _ in 0..96 {
        stepped.step().expect("the step succeeds");
    }

    stepped.reseed(7).expect("the reseed succeeds");
    let reseeded = stepped.seeded_streams();

    // The fresh build's construction assignment: built with seed 7 and no
    // warm-up, so no stream has advanced past its start.
    let fresh = Dwelling::from_config(cz5a_config(7, false)).expect("the dwelling builds");
    let fresh_streams = fresh.seeded_streams();

    assert_eq!(
        reseeded.len(),
        fresh_streams.len(),
        "the same components are seeded on both sides"
    );
    for (reseeded, fresh) in reseeded.iter().zip(&fresh_streams) {
        assert_eq!(reseeded.component, fresh.component, "component order");
        assert_eq!(
            reseeded.state, fresh.state,
            "component {}",
            reseeded.component
        );
    }

    // And the streams the record pins are the seed-7 assignment: the
    // dwelling's own stream 0 first.
    let expected_key = hares_types::rng::dwelling_seed(7, 1);
    for stream in &reseeded {
        assert_eq!(stream.state.seed, expected_key, "{}", stream.component);
        assert_eq!(stream.state.word_pos, 0, "{}", stream.component);
    }
    assert_eq!(reseeded[0].component, "dwelling");
    assert_eq!(reseeded[0].state.stream, 0);

    // Stepping after the reseed advances the streams: the record is the
    // components' own state, not a stale bookkeeping entry.
    stepped.step().expect("the step after the reseed succeeds");
    assert_ne!(
        stepped.seeded_streams()[0].state.word_pos,
        0,
        "the dwelling's own stream advanced with the step"
    );
}
