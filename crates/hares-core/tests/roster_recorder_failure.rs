//! The output recorder is created as part of a roster plan, so a recorder
//! that cannot be created fails construction, and rejects a later roster
//! change without touching the dwelling.

use std::path::{Path, PathBuf};

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
use hares_io::OutputFormat;
use hares_types::HaresError;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn config(output_path: &Path) -> DwellingConfig {
    DwellingConfig {
        hpxml_path: project_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"),
        schedule_path: Some(project_root().join("data/examples/BEopt_example_schedule.csv")),
        weather_path: project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: SimulationConfig {
            start_time: FixedOffset::west_opt(7 * 3600)
                .expect("UTC-7 offset is valid")
                .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
                .unwrap(),
            duration: Duration::hours(1),
            time_res: Duration::seconds(900),
            output_verbosity: 3,
            write_output: true,
            output_path: Some(output_path.to_path_buf()),
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
        bldg_id: 100,
        initialization_duration: None,
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    }
}

fn equipment_names(dwelling: &Dwelling) -> Vec<String> {
    dwelling
        .equipment()
        .iter()
        .map(|e| e.descriptor().name.clone())
        .collect()
}

#[test]
fn construction_fails_when_the_output_file_cannot_be_created() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output_path = dir.path().join("missing").join("out.csv");

    let err = match Dwelling::from_config(config(&output_path)) {
        Err(err) => err,
        Ok(_) => panic!("an output file in a missing directory must fail construction"),
    };

    assert!(matches!(err, HaresError::Io(_)), "got: {err:?}");
}

/// Before the first step every roster change rebuilds the schema and so
/// recreates the recorder. When the recorder cannot be recreated the change
/// is rejected, the dwelling keeps its roster and its working recorder, and
/// it still steps.
#[test]
fn a_pre_step_add_is_rejected_when_the_recorder_cannot_be_recreated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output_dir = dir.path().join("output");
    std::fs::create_dir(&output_dir).expect("create output dir");
    let mut dwelling =
        Dwelling::from_config(config(&output_dir.join("out.csv"))).expect("build the dwelling");
    let name = equipment_names(&dwelling)
        .into_iter()
        .next()
        .expect("base.xml assembles equipment");
    let removed = dwelling
        .remove_equipment(&name)
        .expect("remove one equipment while the output directory exists");

    std::fs::remove_dir_all(&output_dir).expect("remove the output directory");
    let names_before = equipment_names(&dwelling);
    let columns_before = dwelling
        .recorder
        .as_ref()
        .expect("write_output is true")
        .schema()
        .fields()
        .len();
    let warnings_before = dwelling.warnings.to_vec();

    let err = dwelling
        .add_equipment(removed)
        .expect_err("a recorder that cannot be created rejects the add");

    assert!(err.to_string().contains("output recorder"), "got: {err}");
    assert_eq!(equipment_names(&dwelling), names_before);
    assert_eq!(
        dwelling
            .recorder
            .as_ref()
            .expect("the working recorder is kept")
            .schema()
            .fields()
            .len(),
        columns_before
    );
    assert_eq!(dwelling.warnings.to_vec(), warnings_before);
    dwelling
        .step()
        .expect("the dwelling still steps with its working recorder");
}
