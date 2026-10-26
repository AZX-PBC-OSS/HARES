//! The output recorder is created as part of a roster plan, so a recorder
//! that cannot be created fails construction, and rejects a later roster
//! change without touching the dwelling.

use hares_core::DwellingSettings;
use hares_io::{DefaultsSource, InputSource, WeatherInput};
use std::path::{Path, PathBuf};

use chrono::{Duration, TimeZone};
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
use hares_equipment::{ElectricBoilerConfig, Equipment, EquipmentConfig, EquipmentRegistry};
use hares_types::{FluidType, HaresError};

#[path = "../../../tests/support/denver_offset.rs"]
mod denver_offset;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn config(csv_path: &Path) -> DwellingConfig {
    config_with_chunk(csv_path, 1024)
}

/// `chunk_size` rows per flush: the untouched-file test flushes per step
/// (the lazy partial exists), the recreation test never flushes (a stepped
/// run with its working recorder buffers only).
fn config_with_chunk(csv_path: &Path, config_chunk_size: usize) -> DwellingConfig {
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
        defaults: DefaultsSource::directory(project_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(
            SimulationConfig {
                scenario_seed: None,
                start_time: denver_offset::denver_offset()
                    .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
                    .unwrap(),
                duration: Duration::hours(1),
                time_res: Duration::seconds(900),
                reports: vec![hares_io::ReportSpec {
                    name: "frame".to_string(),
                    selection: hares_io::Selection::Verbosity(3),
                    frequency: hares_io::Frequency::default(),
                    destination: hares_io::Destination::Csv {
                        path: csv_path.to_path_buf(),
                        overwrite: false,
                    },
                }],
                diagnostics: false,
                metrics: false,
                output_chunk_size: config_chunk_size,
                setpoint_deadband_c: None,
                master_seed: 0,
                civil_timezone: None,
                site_location: hares_io::SiteLocationOverride::default(),
                max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                health_policy: hares_io::HealthPolicy::default(),
                first_hour_interpolation: Default::default(),
                weather_year: hares_io::WeatherYear::default(),
                warmup: hares_io::Warmup::Disabled,
            },
            None,
            100,
            None,
        ),
    }
}

fn equipment_names(dwelling: &Dwelling) -> Vec<String> {
    dwelling
        .equipment()
        .iter()
        .map(|e| e.descriptor().name.clone())
        .collect()
}

fn boiler_on_loop_7(name: &str, fluid_type: FluidType, dwelling: &Dwelling) -> Box<dyn Equipment> {
    let config = EquipmentConfig::from_typed(
        name.to_string(),
        "Electric Boiler".to_string(),
        ElectricBoilerConfig {
            zone_id: Some(1),
            loop_id: Some(7),
            capacity_w: 0.0,
            fluid_type,
            ..ElectricBoilerConfig::default()
        },
    )
    .expect("boiler config");
    let mut boiler = EquipmentRegistry::new()
        .create("Electric Boiler", config.clone())
        .expect("create boiler");
    boiler
        .init(&config, dwelling.latest_env())
        .expect("init boiler");
    boiler
}

/// A rejected roster change never touches the live output file: the
/// recorder for a new schema is created only once every other check of the
/// change has passed, so the partial file keeps its inode, length and
/// modification time.
#[cfg(unix)]
#[test]
fn a_rejected_roster_change_leaves_the_live_output_file_untouched() {
    use std::os::unix::fs::MetadataExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let csv_path = dir.path().join("out.csv");
    let mut dwelling =
        Dwelling::from_config(config_with_chunk(&csv_path, 1)).expect("build the dwelling");
    let water = boiler_on_loop_7("Boiler-Water", FluidType::Water, &dwelling);
    dwelling
        .add_equipment(water)
        .expect("the first declarer of loop 7 is accepted");
    // One step at chunk size 1: the lazy partial file exists (a report's
    // file is published only at simulate's end; a stepped run streams to
    // the partial).
    dwelling.step().expect("the first step records");
    let partial = std::fs::read_dir(dir.path())
        .expect("dir read")
        .filter_map(|entry| entry.ok())
        .find(|entry| entry.file_name().to_string_lossy().starts_with('.'))
        .expect("the run's partial output file exists")
        .path();
    let file_state = || {
        let metadata = std::fs::metadata(&partial).expect("the output file exists");
        (
            metadata.ino(),
            metadata.len(),
            metadata.modified().expect("modification time"),
        )
    };
    let before = file_state();
    std::thread::sleep(std::time::Duration::from_millis(20));

    let glycol = boiler_on_loop_7("Boiler-Glycol", FluidType::Glycol, &dwelling);
    dwelling
        .add_equipment(glycol)
        .expect_err("a conflicting fluid on loop 7 is rejected");

    assert_eq!(file_state(), before);
}

#[test]
fn construction_fails_when_the_output_file_cannot_be_created() {
    let dir = tempfile::tempdir().expect("tempdir");
    let csv_path = dir.path().join("missing").join("out.csv");

    let err = match Dwelling::from_config(config(&csv_path)) {
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
        .recorders
        .first()
        .expect("the report's recorder exists")
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
            .recorders
            .first()
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
