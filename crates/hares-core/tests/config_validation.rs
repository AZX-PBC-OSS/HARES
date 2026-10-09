//! Row 34: the config validation runs on the Rust construction path too.

use std::path::PathBuf;

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};

fn sim_config(duration_secs: i64, time_res_secs: i64) -> SimulationConfig {
    SimulationConfig {
        start_time: FixedOffset::east_opt(0)
            .expect("UTC offset is valid")
            .with_ymd_and_hms(2018, 1, 1, 0, 0, 0)
            .unwrap(),
        duration: Duration::seconds(duration_secs),
        time_res: Duration::seconds(time_res_secs),
        output_verbosity: 0,
        output_path: None,
        write_output: false,
        output_format: hares_io::OutputFormat::Csv,
        output_chunk_size: 1024,
        setpoint_deadband_c: None,
        master_seed: 0,
        civil_timezone: None,
        site_location: hares_io::SiteLocationOverride::default(),
        retain_batches: false,
        rotation: hares_io::RotationPolicy::None,
        max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
    }
}

/// `Dwelling::from_config` with a directly constructed config whose
/// `duration` is not a multiple of `time_res` fails with the same message
/// `from_toml` gives.
#[test]
fn validate_runs_for_rust_built_config() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config = DwellingConfig {
        hpxml_path: root.join("tests/fixtures/resstock/2025.1/bldg0000007/home.xml"),
        schedule_path: Some(
            root.join("tests/fixtures/resstock/2025.1/bldg0000007/in.schedules.csv"),
        ),
        weather_path: root.join("tests/fixtures/resstock/2025.1/weather/G1500030_2018.csv"),
        defaults_path: Some(root.join("defaults")),
        sim_config: sim_config(3601, 900),
        overrides: None,
        bldg_id: 100,
        initialization_duration: None,
        patches: None,
    };
    let Err(err) = Dwelling::from_config(config) else {
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
