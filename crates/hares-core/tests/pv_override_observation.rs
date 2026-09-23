//! Behavioral replication of the coverage-triage PV scenario: a solar
//! override whose rows lack the PV arrays' synthetic orientation surfaces
//! makes every PV `step()` fail (tolerated, warned); the question under
//! test is whether the dwelling's per-step observable contract survives —
//! run to completion with every equipment individually observable, and the
//! start-of-step core-entry invariant never firing.

use std::path::PathBuf;

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
use hares_io::OutputFormat;
use hares_types::SurfaceIrradiance;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sample_dir() -> PathBuf {
    project_root().join("tests/fixtures/hpxml/ochre_samples")
}

/// The override the triage exercised: rows carrying only the envelope
/// surfaces (0..=13), omitting the PV arrays' synthetic orientation
/// surfaces (the large ids like 200018000) — the exact shape the Python
/// DataFrame conversion produces for a pvlib CSV that has no PV-surface
/// columns.
fn envelope_only_override() -> Vec<Vec<SurfaceIrradiance>> {
    (0..24)
        .map(|_| {
            (0..=13)
                .map(|sid| SurfaceIrradiance {
                    surface_id: sid,
                    direct_w_m2: 0.0,
                    diffuse_w_m2: 0.0,
                    reflected_w_m2: 0.0,
                    angle_of_incidence_rad: 0.0,
                })
                .collect()
        })
        .collect()
}

#[test]
fn tolerated_pv_step_failure_under_surface_less_override_keeps_equipment_observable() {
    let config = DwellingConfig {
        hpxml_path: sample_dir().join("base-pv.xml"),
        schedule_path: project_root().join("data/examples/BEopt_example_schedule.csv"),
        weather_path: project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: SimulationConfig {
            start_time: FixedOffset::east_opt(0)
                .expect("UTC")
                .with_ymd_and_hms(2019, 7, 15, 0, 0, 0)
                .unwrap(),
            duration: Duration::hours(6),
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
        },
        overrides: None,
        bldg_id: 42,
        initialization_duration: None,
        resample_overrides: None,
        patches: None,
    };
    let mut dwelling = Dwelling::from_config(config).expect("dwelling builds");
    assert!(
        dwelling
            .equipment()
            .iter()
            .any(|eq| eq.descriptor().name.starts_with("PV")),
        "precondition: the base-pv fixture must assemble PV equipment"
    );
    dwelling
        .environment
        .set_solar_override(envelope_only_override());

    // Six steps, mirroring the Python test. Behavioral contract: every step
    // succeeds (the PV failure is tolerated by design), and — the face the
    // triage found — the dwelling does not panic on its start-of-step
    // core-entry invariant, because a tolerated step failure must not make
    // an equipment permanently unobservable.
    for step in 0..6 {
        dwelling
            .step()
            .unwrap_or_else(|e| panic!("step {step} must tolerate the PV failure: {e}"));
    }

    // The observable: every equipment in the vector is individually
    // addressable in the environment snapshot after the run — a tolerated
    // failure retains the equipment's last committed core output instead of
    // dropping it from observation.
    let missing: Vec<String> = dwelling
        .equipment()
        .iter()
        .map(|eq| eq.descriptor().name.clone())
        .zip(dwelling.equipment().iter().map(|eq| {
            dwelling
                .latest_env()
                .equipment_core
                .contains_key(&eq.descriptor().id)
        }))
        .filter(|(_, present)| !present)
        .map(|(name, _)| name)
        .collect();
    assert!(
        missing.is_empty(),
        "a tolerated step failure must not leave equipment unobservable in \
         equipment_core (missing: {missing:?}) — actors reading those ids get \
         the no-observation sentinel for the rest of the run"
    );
}
