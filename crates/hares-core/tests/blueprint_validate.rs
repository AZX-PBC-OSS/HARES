//! The `DwellingBlueprint::validate()` acceptance: every assembly-time
//! check runs, and the resolved model is readable, without running a step.
//! `validate()` and `build()` fail identically on an infeasible input, and
//! a valid blueprint's read-back equals the built dwelling's.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Duration, FixedOffset, TimeZone};
use hares_core::{
    DwellingBlueprint, DwellingConfig, DwellingSettings, ResolvedBoundary, ResolvedClock,
    ResolvedLayer, ResolvedModel,
};
use hares_io::hpxml::BoundaryType;
use hares_io::{
    Catalogue, DefaultsSource, Destination, Frequency, InputSource, ReportSpec, Selection,
    SimulationConfig, SiteLocationOverride, WeatherInput,
};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn site_offset() -> FixedOffset {
    FixedOffset::east_opt(-7 * 3600).expect("offset")
}

fn start_at() -> DateTime<FixedOffset> {
    site_offset()
        .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
        .single()
        .expect("start")
}

fn sim_config(duration_s: i64, step_s: i64) -> SimulationConfig {
    SimulationConfig {
        start_time: start_at(),
        duration: Duration::seconds(duration_s),
        time_res: Duration::seconds(step_s),
        reports: vec![ReportSpec {
            name: "frame".to_string(),
            selection: Selection::Verbosity(2),
            frequency: Frequency::default(),
            destination: Destination::Memory,
        }],
        diagnostics: false,
        metrics: true,
        output_chunk_size: 10_000,
        setpoint_deadband_c: None,
        master_seed: 0,
        civil_timezone: None,
        site_location: SiteLocationOverride::default(),
        max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
        health_policy: hares_io::HealthPolicy::default(),
    }
}

/// The `full_year_hourly` inputs of `02-measurement.md` M-2: the gas
/// furnace fixture.
fn cz2a_config(
    duration_s: i64,
    step_s: i64,
    overrides: Option<serde_json::Value>,
) -> DwellingConfig {
    let fixture = project_root().join("tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh");
    DwellingConfig {
        hpxml: InputSource::Path(fixture.join("building.xml")),
        schedule: Some(InputSource::Path(fixture.join("schedule.csv"))),
        weather: WeatherInput::Path(fixture.join("weather.epw")),
        defaults: DefaultsSource::directory(project_root().join("defaults")),
        settings: DwellingSettings {
            sim_config: sim_config(duration_s, step_s),
            overrides,
            bldg_id: 1,
            initialization_duration: None,
            patches: None,
        },
    }
}

/// An override that sets the furnace's AFUE to 1.33, outside [0, 1].
fn afue_above_one_config() -> DwellingConfig {
    cz2a_config(
        48 * 3600,
        900,
        Some(serde_json::json!({"Gas Furnace": {"afue": 1.33}})),
    )
}

/// The `validate()` of the M-2 full-year config: the read-back a caller
/// checks a table's row count against.
fn full_year_blueprint() -> Arc<DwellingBlueprint> {
    DwellingBlueprint::from_config(cz2a_config(365 * 24 * 3600, 3600, None))
        .expect("the blueprint builds")
}

/// An infeasible input fails `validate()` with the run-time message: the
/// AFUE of 1.33 the gas furnace's typed config rejects, and `validate()`
/// and `build()` return equal errors, compared both as `PartialEq` and by
/// their `Display`.
#[test]
fn validate_rejects_afue_above_one_with_run_time_message() {
    let config = afue_above_one_config();
    let blueprint = DwellingBlueprint::from_config(config.clone()).expect("the blueprint builds");
    let validated = blueprint.validate().expect_err("the AFUE is infeasible");
    let built = DwellingBlueprint::from_config(config)
        .expect("the blueprint builds")
        .build()
        .err()
        .expect("the build fails on the same input");
    assert_eq!(validated, built, "the two errors are equal");
    assert_eq!(validated.to_string(), built.to_string());
    assert!(
        validated.to_string().contains("afue"),
        "the error names the run-time cause: {validated}"
    );
}

/// `validate()` runs no step: the process-wide step counter reads 0 after
/// it, and the same counter reads 3 after three stepped calls on the built
/// dwelling, so the pin discriminates.
#[test]
fn validate_runs_no_step() {
    let blueprint = full_year_blueprint();
    hares_core::step_counts::reset();
    blueprint.validate().expect("the blueprint validates");
    assert_eq!(hares_core::step_counts::steps_run(), 0);

    let mut dwelling = blueprint.build().expect("the dwelling builds");
    for _ in 0..3 {
        dwelling.step().expect("the step runs");
    }
    assert_eq!(hares_core::step_counts::steps_run(), 3);
}

/// `validate()` equals the built dwelling's read-back: the same assembly,
/// the same `ResolvedModel`.
#[test]
fn validate_equals_built_dwelling_resolved_model() {
    let blueprint = full_year_blueprint();
    let validated: ResolvedModel = blueprint.validate().expect("the blueprint validates");
    let dwelling = blueprint.build().expect("the dwelling builds");
    assert_eq!(&validated, dwelling.resolved_model());
}

/// The read-back's clock reports the first step's local standard time, the
/// site offset, the step and the simulated step count of the configured
/// horizon: a full year at 3600 s starting `2023-01-01T00:00:00-07:00` is
/// 8760 steps at a -25200 s offset.
#[test]
fn resolved_clock_reports_steps() {
    let blueprint = full_year_blueprint();
    let model = blueprint.validate().expect("the blueprint validates");
    let clock: ResolvedClock = model.clock;
    assert_eq!(clock.steps, 8760);
    assert_eq!(clock.time_res_s, 3600);
    assert_eq!(clock.utc_offset_s, -25200);
    assert_eq!(
        clock.start.format("%Y-%m-%dT%H:%M:%S").to_string(),
        "2023-01-01T00:00:00"
    );
}

/// The blueprint's catalogue equals the built dwelling's: the same
/// assembly, the equipment and the auto-registered actors, with no warm-up
/// and no step.
#[test]
fn blueprint_catalogue_equals_built_dwelling_catalogue() {
    let blueprint = full_year_blueprint();
    hares_core::step_counts::reset();
    let from_blueprint: Arc<Catalogue> = blueprint.catalogue().expect("the catalogue builds");
    assert_eq!(hares_core::step_counts::steps_run(), 0);
    let dwelling = blueprint.build().expect("the dwelling builds");
    let from_dwelling = dwelling.catalogue();
    assert_eq!(from_blueprint.digest(), from_dwelling.digest());
    assert_eq!(from_blueprint.len(), from_dwelling.len());
    assert!(!from_blueprint.is_empty(), "the catalogue holds variables");
}

/// Every resolved boundary carries its identity and its layer stack: the
/// read-back names each boundary once, and the opaque boundaries built
/// from the LUT report the layers the physics consumes.
#[test]
fn resolved_boundaries_carry_identity_and_layers() {
    let blueprint = full_year_blueprint();
    let model = blueprint.validate().expect("the blueprint validates");
    let boundaries: &[ResolvedBoundary] = &model.boundaries;
    assert!(!boundaries.is_empty(), "the building has boundaries");
    let ids: std::collections::HashSet<&str> = boundaries.iter().map(|b| b.id.as_str()).collect();
    assert_eq!(ids.len(), boundaries.len(), "every id appears once");
    let layered = boundaries
        .iter()
        .filter(|b| !matches!(b.kind, BoundaryType::Slab))
        .filter(|b| !b.layers.is_empty())
        .count();
    assert!(layered > 0, "the LUT serves the opaque boundaries");
    let slab = boundaries
        .iter()
        .find(|b| matches!(b.kind, BoundaryType::Slab))
        .expect("the fixture has a slab");
    assert!(
        slab.layers.is_empty(),
        "the F-factor slab carries no construction layer stack"
    );
    for boundary in boundaries {
        for layer in &boundary.layers {
            let ResolvedLayer { r_m2k_w, c_j_m2k } = *layer;
            assert!(r_m2k_w.is_finite() && r_m2k_w >= 0.0);
            assert!(c_j_m2k.is_finite() && c_j_m2k >= 0.0);
        }
        assert!(boundary.films_m2k_w.is_finite());
        assert!(boundary.r_effective_m2k_w.is_finite() && boundary.r_effective_m2k_w > 0.0);
        assert!(boundary.c_j_k.is_finite());
    }
}

/// The validate-only path's cost against the feasibility requirement's
/// 100 ms target: the cz4a fixture's `validate()` wall time and the cz2a
/// AFUE rejection's, run by hand in the release build and recorded in
/// `02-measurement.md`.
#[test]
#[ignore = "the validate-only path's wall time, run by hand in the release build"]
fn validate_wall_time() {
    let cz4a = project_root().join("tests/fixtures/parity/cz4a_ashp_hpwh");
    let config = DwellingConfig {
        hpxml: InputSource::Path(cz4a.join("building.xml")),
        schedule: Some(InputSource::Path(cz4a.join("schedule.csv"))),
        weather: WeatherInput::Path(cz4a.join("weather.epw")),
        defaults: DefaultsSource::directory(project_root().join("defaults")),
        settings: DwellingSettings {
            sim_config: sim_config(48 * 3600, 900),
            overrides: None,
            bldg_id: 1,
            initialization_duration: None,
            patches: None,
        },
    };
    let blueprint = DwellingBlueprint::from_config(config).expect("the blueprint builds");
    // One warm-up pass the timer does not read: the first call's page cache
    // and lazy statics are not the cost a caller pays twice.
    blueprint.validate().expect("the blueprint validates");
    let started = Instant::now();
    blueprint.validate().expect("the blueprint validates");
    let valid = started.elapsed();

    let infeasible =
        DwellingBlueprint::from_config(afue_above_one_config()).expect("the blueprint builds");
    let started = Instant::now();
    let rejection = infeasible.validate().expect_err("the AFUE is infeasible");
    let rejected = started.elapsed();

    println!("cz4a validate(): {valid:.1?} (target: under 100 ms)");
    println!("cz2a AFUE 1.33 rejection: {rejected:.1?} (target: under 100 ms)");
    println!("rejection message: {rejection}");
    assert!(
        valid.as_millis() < 100,
        "the valid validate() takes {valid:?}"
    );
    assert!(
        rejected.as_millis() < 100,
        "the infeasible rejection takes {rejected:?}"
    );
}
