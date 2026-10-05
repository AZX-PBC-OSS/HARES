//! A solar override on a dwelling with PV: one that lacks the PV arrays'
//! orientation surfaces is a configuration error caught at the call, and
//! one that carries them lets the PV step, producing nothing at night.

use std::path::PathBuf;

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
use hares_io::OutputFormat;
use hares_types::{HaresError, SurfaceIrradiance};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn pv_dwelling() -> Dwelling {
    let config = DwellingConfig {
        hpxml_path: project_root().join("tests/fixtures/hpxml/ochre_samples/base-pv.xml"),
        schedule_path: Some(project_root().join("data/examples/BEopt_example_schedule.csv")),
        weather_path: project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: SimulationConfig {
            start_time: FixedOffset::east_opt(0)
                .expect("UTC")
                .with_ymd_and_hms(2019, 7, 15, 0, 0, 0)
                .unwrap(),
            duration: Duration::hours(4),
            time_res: Duration::hours(1),
            output_verbosity: 0,
            write_output: false,
            output_path: None,
            output_format: OutputFormat::Csv,
            output_chunk_size: 1024,
            setpoint_deadband_c: None,
            master_seed: 0,
            civil_timezone: None,
            site_location: hares_io::SiteLocationOverride::default(),
            retain_batches: false,
            rotation: hares_io::RotationPolicy::None,
            max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
        },
        overrides: None,
        bldg_id: 42,
        initialization_duration: None,
        resample_overrides: None,
        patches: None,
    };
    Dwelling::from_config(config).expect("dwelling builds")
}

fn pv_names(dwelling: &Dwelling) -> Vec<String> {
    let names: Vec<String> = dwelling
        .equipment()
        .iter()
        .map(|eq| eq.descriptor().name.clone())
        .filter(|name| name.starts_with("PV"))
        .collect();
    assert!(!names.is_empty(), "the base-pv fixture assembles PV");
    names
}

/// A dark override over the surfaces the predicate keeps.
fn dark_override(dwelling: &Dwelling, keep: impl Fn(u32) -> bool) -> Vec<Vec<SurfaceIrradiance>> {
    let row: Vec<SurfaceIrradiance> = dwelling
        .environment
        .surface_geometry()
        .iter()
        .map(|s| s.surface_id)
        .filter(|&id| keep(id))
        .map(|surface_id| SurfaceIrradiance {
            surface_id,
            direct_w_m2: 0.0,
            diffuse_w_m2: 0.0,
            reflected_w_m2: 0.0,
            angle_of_incidence_rad: 0.0,
        })
        .collect();
    vec![row; 24]
}

/// The coverage-triage shape: rows carrying only the envelope surfaces
/// (0..=13), the shape the Python DataFrame conversion produces for a pvlib
/// CSV without PV-surface columns.
#[test]
fn an_override_without_the_pv_surfaces_is_rejected_at_the_call() {
    let mut dwelling = pv_dwelling();
    pv_names(&dwelling);
    let err = dwelling
        .environment
        .set_solar_override(dark_override(&dwelling, |id| id <= 13))
        .expect_err("the PV surfaces are missing");
    assert!(
        matches!(
            err,
            HaresError::SolarOverrideMissingSurface { timestep: 0, .. }
        ),
        "{err}"
    );
    assert!(!dwelling.environment.has_solar_override());
    dwelling
        .step()
        .expect("the dwelling steps without the override");
}

#[test]
fn pv_produces_nothing_at_night_under_an_override_carrying_its_surfaces() {
    let mut dwelling = pv_dwelling();
    let pvs = pv_names(&dwelling);
    dwelling
        .environment
        .set_solar_override(dark_override(&dwelling, |_| true))
        .expect("the override carries every surface");
    for step in 0..4 {
        dwelling
            .step()
            .unwrap_or_else(|e| panic!("step {step}: {e}"));
        for eq in dwelling.equipment() {
            if pvs.contains(&eq.descriptor().name) {
                let kw = eq
                    .core_output()
                    .flows
                    .electric_kw
                    .map_or(0.0, |e| e.signed_kw());
                assert_eq!(kw, 0.0, "step {step}: {} at night", eq.descriptor().name);
            }
        }
    }
    assert_eq!(dwelling.health().port_rollbacks, 0);
}
