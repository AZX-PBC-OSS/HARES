//! Row 34: the config validation runs on the Rust construction path too.

use std::path::{Path, PathBuf};

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::dwelling::DwellingBlueprint;
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};

fn sim_config(duration_secs: i64, time_res_secs: i64) -> SimulationConfig {
    SimulationConfig {
        start_time: FixedOffset::east_opt(0)
            .expect("UTC offset is valid")
            .with_ymd_and_hms(2018, 1, 1, 0, 0, 0)
            .unwrap(),
        duration: Duration::seconds(duration_secs),
        time_res: Duration::seconds(time_res_secs),
        reports: Vec::new(),
        diagnostics: false,
        output_chunk_size: 1024,
        setpoint_deadband_c: None,
        master_seed: 0,
        civil_timezone: None,
        site_location: hares_io::SiteLocationOverride::default(),
        max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
    }
}

fn dwelling_config(root: &Path, sim_config: SimulationConfig) -> DwellingConfig {
    DwellingConfig {
        hpxml_path: root.join("tests/fixtures/resstock/2025.1/bldg0000007/home.xml"),
        schedule_path: Some(
            root.join("tests/fixtures/resstock/2025.1/bldg0000007/in.schedules.csv"),
        ),
        weather_path: root.join("tests/fixtures/resstock/2025.1/weather/G1500030_2018.csv"),
        defaults_path: Some(root.join("defaults")),
        sim_config,
        overrides: None,
        bldg_id: 100,
        initialization_duration: None,
        patches: None,
    }
}

/// `Dwelling::from_config` with a directly constructed config whose
/// `duration` is not a multiple of `time_res` fails with the same message
/// `from_toml` gives.
#[test]
fn validate_runs_for_rust_built_config() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let Err(err) = Dwelling::from_config(dwelling_config(&root, sim_config(3601, 900))) else {
        panic!("a dwelling whose duration is not a multiple of time_res must not build");
    };
    let message = err.to_string();
    let toml_err = SimulationConfig::from_toml(
        "\nstart_time = \"2018-01-01T00:00:00Z\"\nduration = 3601\ntime_res = 900\n",
    )
    .expect_err("the same misaligned config parsed from TOML fails from_toml's validate");
    assert!(
        message.contains(&toml_err.to_string()),
        "one validation, one message, on every construction path; from_toml gives \
         {toml_err}, the dwelling path gives {message}"
    );
    assert!(
        message.contains("divisible"),
        "the message is the divisibility check's, got: {message}"
    );
}

/// One representative per `validate()` check class: a directly constructed
/// config failing only that check fails `DwellingBlueprint::from_config`
/// with the same message `from_toml` gives for the same values.
#[test]
fn validate_fires_for_rust_built_blueprint_per_check_class() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");

    let assert_fails_like_from_toml = |case: &str, config: SimulationConfig, toml: String| {
        let Err(err) = DwellingBlueprint::from_config(dwelling_config(&root, config)) else {
            panic!("{case}: the blueprint must refuse an invalid config");
        };
        let toml_err = SimulationConfig::from_toml(&toml)
            .expect_err(&format!("{case}: from_toml refuses the same values"));
        assert!(
            err.to_string().contains(&toml_err.to_string()),
            "{case}: one validation, one message; from_toml gives {toml_err}, \
             the blueprint path gives {err}"
        );
    };

    // duration must be greater than zero.
    let mut config = sim_config(0, 60);
    config.duration = Duration::seconds(0);
    assert_fails_like_from_toml(
        "zero duration",
        config,
        "\nstart_time = \"2018-01-01T00:00:00Z\"\nduration = 0\ntime_res = 60\n".into(),
    );

    // time_res must be greater than zero.
    let mut config = sim_config(3600, 0);
    config.time_res = Duration::seconds(0);
    assert_fails_like_from_toml(
        "zero time_res",
        config,
        "\nstart_time = \"2018-01-01T00:00:00Z\"\nduration = 3600\ntime_res = 0\n".into(),
    );

    // A verbosity report above 8 must be rejected.
    let toml = "\nstart_time = \"2018-01-01T00:00:00Z\"\nduration = 3600\ntime_res = 60\n\n[[reports]]\nname = \"frame\"\nselection = { verbosity = 9 }\ndestination = \"memory\"\n";
    let config = hares_io::SimulationConfig::from_toml(toml);
    assert!(
        config.is_err(),
        "a verbosity report above 8 must be rejected"
    );

    // duration must be evenly divisible by time_res.
    let config = sim_config(3601, 900);
    assert_fails_like_from_toml(
        "misaligned duration",
        config,
        "\nstart_time = \"2018-01-01T00:00:00Z\"\nduration = 3601\ntime_res = 900\n".into(),
    );

    // setpoint_deadband_c must be finite and >= 0.
    let mut config = sim_config(3600, 60);
    config.setpoint_deadband_c = Some(-1.0);
    assert_fails_like_from_toml(
        "negative deadband",
        config,
        "\nstart_time = \"2018-01-01T00:00:00Z\"\nduration = 3600\ntime_res = 60\nsetpoint_deadband_c = -1.0\n"
            .into(),
    );

    // time_res must fit the u32 seconds range the metrics calculators
    // require (a duration that is a multiple keeps the earlier checks
    // silent, so the u32 check's message is the one asserted).
    let config = sim_config(2 * (1i64 << 32), 1 << 32);
    assert_fails_like_from_toml(
        "time_res above u32 seconds",
        config,
        "\nstart_time = \"2018-01-01T00:00:00Z\"\nduration = 8589934592\ntime_res = 4294967296\n"
            .into(),
    );

    // The site-location check: one representative field, latitude.
    let mut config = sim_config(3600, 60);
    config.site_location = hares_io::SiteLocationOverride {
        latitude_deg: Some(95.0),
        ..hares_io::SiteLocationOverride::default()
    };
    assert_fails_like_from_toml(
        "out-of-range latitude",
        config,
        "\nstart_time = \"2018-01-01T00:00:00Z\"\nduration = 3600\ntime_res = 60\n\n[site_location]\nlatitude_deg = 95.0\n"
            .into(),
    );
}
