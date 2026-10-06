//! A home whose ACH50 infiltration needs its storey count fails without it.
//!
//! AIM-2's infiltration from ACH50 reads the number of conditioned floors
//! above grade, which OS-HPXML requires. A home without it is an error
//! naming the element, not a home assumed to be one storey.

use std::path::PathBuf;

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
use hares_io::OutputFormat;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn ach50_infiltration_without_floors_above_grade_is_an_error() {
    let xml = std::fs::read_to_string(project_root().join("data/examples/BEopt_example.xml"))
        .expect("BEopt example readable");
    let element = "<NumberofConditionedFloorsAboveGrade>1.0</NumberofConditionedFloorsAboveGrade>";
    assert!(xml.contains(element), "BEopt names its storeys");
    let dir = tempfile::tempdir().expect("temp dir");
    let hpxml_path = dir.path().join("home.xml");
    std::fs::write(&hpxml_path, xml.replacen(element, "", 1)).expect("write HPXML");

    let start_time = FixedOffset::west_opt(7 * 3600)
        .expect("UTC-7")
        .with_ymd_and_hms(2023, 1, 15, 0, 0, 0)
        .single()
        .expect("start time");
    let config = DwellingConfig {
        hpxml_path,
        schedule_path: None,
        weather_path: project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: SimulationConfig {
            start_time,
            duration: Duration::hours(1),
            time_res: Duration::seconds(3600),
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
    };
    let err = match Dwelling::from_config(config) {
        Err(err) => err,
        Ok(_) => panic!("a home without its storey count must not build"),
    };
    assert!(
        err.to_string()
            .contains("NumberofConditionedFloorsAboveGrade"),
        "the error must name the missing element, got: {err}"
    );
}
