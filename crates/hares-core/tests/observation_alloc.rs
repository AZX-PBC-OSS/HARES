//! The RL observation plan's zero-allocation and bitwise contracts.
//!
//! The plan is built once per episode from the environment's field list
//! against a built dwelling; every step reads through it into a
//! caller-owned buffer. Two contracts are pinned here:
//!
//! - `observe_into_allocates_nothing_after_the_first_step`: after the
//!   executor's first pass, 96 `observe_into` calls allocate nothing
//!   (the counting allocator installed on this test binary);
//! - `observation_plan_matches_to_observation_vec`: the plan's values
//!   equal `to_observation_vec`'s on bits, on the dwelling-direct and the
//!   telemetry-snapshot executors, including the NaN row before the first
//!   step.

use std::path::PathBuf;

use chrono::{DateTime, Duration};
use hares_core::{Dwelling, DwellingConfig, DwellingSettings};
use hares_io::{DefaultsSource, InputSource, WeatherInput};
use hares_types::alloc_count::{CountingAllocator, thread_allocations};

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn base_xml_config() -> DwellingConfig {
    DwellingConfig {
        hpxml: InputSource::Path(
            project_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"),
        ),
        schedule: Some(InputSource::Path(
            project_root().join("data/examples/BEopt_example_schedule.csv"),
        )),
        weather: WeatherInput::Path(
            project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        ),
        defaults: DefaultsSource::directory(project_root().join("defaults")),
        settings: DwellingSettings::new(
            hares_io::SimulationConfig {
                start_time: DateTime::parse_from_rfc3339("2019-01-01T00:00:00-07:00")
                    .expect("valid start time"),
                duration: Duration::hours(1),
                time_res: Duration::seconds(60),
                reports: Vec::new(),
                diagnostics: false,
                metrics: false,
                output_chunk_size: 1024,
                setpoint_deadband_c: None,
                master_seed: 0,
                civil_timezone: None,
                site_location: hares_io::SiteLocationOverride::default(),
                max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                health_policy: hares_io::HealthPolicy::default(),
            },
            None,
            1,
            None,
            None,
        ),
    }
}

/// Every observation field kind the base.xml dwelling can answer: the
/// scalar keys, the zone and equipment brackets, the temporal encoding
/// and the DR actor's channels (dot and bare name).
fn base_observation_fields(dwelling: &Dwelling) -> Vec<String> {
    let mut fields: Vec<String> = [
        "outdoor_temp",
        "outdoor_humidity_ratio",
        "total_power_kw",
        "total_electric_kw",
        "reactive_power_kvar",
        "time_sin",
        "time_cos",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let zone_names = dwelling.telemetry().expect("telemetry builds").zone_names;
    for name in &zone_names {
        for prefix in [
            "zone_temp",
            "setpoint_heat",
            "setpoint_cool",
            "zone_energy_balance",
        ] {
            fields.push(format!("{prefix}[{name}]"));
        }
    }
    let equipment_names = dwelling
        .telemetry()
        .expect("telemetry builds")
        .equipment_names;
    for name in &equipment_names {
        fields.push(format!("equipment_soc[{name}]"));
        fields.push(format!("equipment_power[{name}]"));
    }
    fields
}

/// After the executor's first pass, 96 `observe_into` calls allocate
/// nothing on the counting allocator.
#[test]
fn observe_into_allocates_nothing_after_the_first_step() {
    let dwelling = Dwelling::from_config(base_xml_config()).expect("base.xml must load");
    let fields = base_observation_fields(&dwelling);

    let plan = dwelling
        .observation_plan(&fields)
        .expect("every field kind resolves");

    let mut out = vec![0.0_f64; fields.len()];
    // The first pass warms the executor (lazy statics, the throttled
    // resolution warnings); it may allocate.
    dwelling
        .observe_into(&plan, &mut out)
        .expect("the first observation read succeeds");

    let before = thread_allocations().expect("the counting allocator is installed");
    for _ in 0..96 {
        dwelling
            .observe_into(&plan, &mut out)
            .expect("every observation read succeeds");
    }
    let after = thread_allocations().expect("the counting allocator is installed");
    assert_eq!(after, before, "96 observe_into calls must allocate nothing");
}

/// The plan's values equal `to_observation_vec`'s on bits: the
/// dwelling-direct executor against the telemetry-snapshot executor,
/// before the first step (all NaN) and after one.
#[test]
fn observation_plan_matches_to_observation_vec() {
    let mut dwelling = Dwelling::from_config(base_xml_config()).expect("base.xml must load");
    let fields = base_observation_fields(&dwelling);

    // Before the first step: every value NaN, both executors.
    let plan = dwelling
        .observation_plan(&fields)
        .expect("every field kind resolves");
    let mut direct = vec![0.0_f64; fields.len()];
    dwelling
        .observe_into(&plan, &mut direct)
        .expect("the uninitialized read succeeds");
    let snapshot = dwelling
        .telemetry()
        .expect("telemetry builds")
        .to_observation_vec(&fields.iter().map(String::as_str).collect::<Vec<_>>())
        .expect("the snapshot read succeeds");
    assert_eq!(direct.len(), snapshot.len());
    for (a, b) in direct.iter().zip(&snapshot) {
        assert_eq!(
            a.to_bits(),
            b.to_bits(),
            "uninitialized rows must match on bits"
        );
        assert!(a.is_nan(), "uninitialized observation must be NaN");
    }

    // After one step: the values match on bits.
    dwelling.step().expect("the step succeeds");
    let mut direct = vec![0.0_f64; fields.len()];
    dwelling
        .observe_into(&plan, &mut direct)
        .expect("the initialized read succeeds");
    let snapshot = dwelling
        .telemetry()
        .expect("telemetry builds")
        .to_observation_vec(&fields.iter().map(String::as_str).collect::<Vec<_>>())
        .expect("the snapshot read succeeds");
    assert_eq!(direct.len(), snapshot.len());
    for (a, b) in direct.iter().zip(&snapshot) {
        assert_eq!(
            a.to_bits(),
            b.to_bits(),
            "initialized values must match on bits"
        );
    }
}
