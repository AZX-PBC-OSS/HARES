//! The `DwellingConfig.schedule_path` contract.
//!
//! `None` requests a schedule generated from the HPXML. A set path must be
//! readable: construction fails naming the path when it is not, so a
//! mistyped path can never silently run on a generated schedule's different
//! occupancy and loads.

use std::path::PathBuf;

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::dwelling::DwellingBlueprint;
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
use hares_io::OutputFormat;
use sha2::{Digest, Sha256};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The schedule the generated path produced before `schedule_path` became
/// `Option<PathBuf>`, captured at the parent commit for `base.xml` over the
/// Denver TMY3 weather with the repo defaults: a 24 h run starting
/// 2023-01-15 00:00 UTC-7 at 900 s steps. The generation path must stay
/// bitwise-equal to it.
const GENERATED_SCHEDULE_DIGEST: &str =
    "ca65bbafa811a379b36c63ef52b067256612183617783159636222adc991ba62";

fn generated_schedule_config(schedule_path: Option<PathBuf>) -> DwellingConfig {
    let tz_offset = FixedOffset::west_opt(7 * 3600).expect("UTC-7 offset is valid");
    let start_time = tz_offset
        .with_ymd_and_hms(2023, 1, 15, 0, 0, 0)
        .single()
        .expect("valid start time");
    DwellingConfig {
        hpxml_path: project_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"),
        schedule_path,
        weather_path: project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: SimulationConfig {
            start_time,
            duration: Duration::hours(24),
            time_res: Duration::seconds(900),
            output_verbosity: 0,
            output_path: None,
            write_output: false,
            output_format: OutputFormat::Csv,
            output_chunk_size: 1024,
            setpoint_deadband_c: None,
            master_seed: 0,
            civil_timezone: None,
            site_location: hares_io::SiteLocationOverride::default(),
            retain_batches: false,
            rotation: hares_io::RotationPolicy::None,
        },
        overrides: None,
        bldg_id: 1,
        initialization_duration: None,
        resample_overrides: None,
        patches: None,
    }
}

/// Bit digest of a resolved schedule: timestamps, column names, and the bit
/// pattern of every value, so any change in content or ordering moves it.
fn schedule_digest(schedule: &hares_io::ScheduleTimeSeries) -> String {
    let mut hasher = Sha256::new();
    for ts in &schedule.timestamps {
        hasher.update(ts.timestamp().to_le_bytes());
    }
    for name in &schedule.column_names {
        hasher.update((name.len() as u32).to_le_bytes());
        hasher.update(name.as_bytes());
    }
    for column in &schedule.columns {
        for value in column {
            hasher.update(value.to_bits().to_le_bytes());
        }
    }
    hasher.update(schedule.source_step_secs.to_le_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A schedule path that does not exist fails construction, for the dwelling
/// and the blueprint constructors alike, and the error names the path.
#[test]
fn nonexistent_schedule_path_is_an_error() {
    let missing = project_root().join("data/examples/no-such-schedule-path.csv");
    assert!(
        !missing.exists(),
        "the probe path must not exist: {missing:?}"
    );

    let dwelling_err = match Dwelling::from_config(generated_schedule_config(Some(missing.clone())))
    {
        Err(err) => err,
        Ok(_) => panic!("a mistyped schedule path must fail dwelling construction"),
    };
    assert!(
        dwelling_err
            .to_string()
            .contains(&missing.display().to_string()),
        "the dwelling error must name the mistyped path, got: {dwelling_err}"
    );

    let blueprint_err =
        match DwellingBlueprint::from_config(generated_schedule_config(Some(missing.clone()))) {
            Err(err) => err,
            Ok(_) => panic!("a mistyped schedule path must fail blueprint construction"),
        };
    assert!(
        blueprint_err
            .to_string()
            .contains(&missing.display().to_string()),
        "the blueprint error must name the mistyped path, got: {blueprint_err}"
    );
}

/// `None` generates the schedule from the HPXML: bitwise-equal to the
/// generator called directly on the same parsed inputs, and bitwise-equal to
/// the schedule the pre-change behavior produced for a missing path.
#[test]
fn no_schedule_generates_from_hpxml() {
    let config = generated_schedule_config(None);
    let blueprint = DwellingBlueprint::from_config(config.clone())
        .expect("a None schedule requests the generated schedule");

    let building = hares_io::parse_hpxml(&config.hpxml_path).expect("the HPXML parses");
    let generated = hares_io::hpxml_schedule::generate_schedule_from_hpxml(
        &building,
        config.sim_config.start_time,
        config.sim_config.duration,
        config.sim_config.time_res,
        config.defaults_path.as_deref(),
    )
    .resample(config.sim_config.time_res.num_seconds() as u32)
    .expect("the generated schedule resamples");

    let built_digest = schedule_digest(&blueprint.schedule);
    assert_eq!(
        built_digest,
        schedule_digest(&generated),
        "None must run exactly the generator's schedule"
    );
    assert_eq!(
        built_digest, GENERATED_SCHEDULE_DIGEST,
        "the generated schedule must stay bitwise-equal to the pre-change behavior"
    );
}
