//! A control update's error reaches the caller: the thermostat's failing
//! setpoint-source read is recorded by `update_control` and the following
//! step fails with it, the equipment's failure budget ends the run, and the
//! error names the equipment. The `let _ =` around the step loop's
//! `update_control` discards only the operating mode, never the error.

use hares_core::DwellingSettings;
use hares_io::{DefaultsSource, InputSource, WeatherInput};
use std::path::{Path, PathBuf};

use chrono::{Duration, TimeZone};
use hares_core::{Dwelling, DwellingConfig};
use serde_json::json;

#[path = "../../../tests/support/denver_offset.rs"]
mod denver_offset;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(name: &str) -> PathBuf {
    project_root()
        .join("tests/fixtures/hpxml/ochre_samples")
        .join(name)
}

fn config(hpxml: &Path, overrides: Option<serde_json::Value>) -> DwellingConfig {
    let start_time = denver_offset::denver_offset()
        .with_ymd_and_hms(2023, 1, 15, 10, 0, 0)
        .single()
        .expect("valid start time");
    DwellingConfig {
        hpxml: InputSource::Path(hpxml.to_path_buf()),
        schedule: None,
        weather: WeatherInput::Path(
            project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        ),
        defaults: DefaultsSource::directory(project_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(
            hares_io::SimulationConfig {
                scenario_seed: None,
                schedule_diversity: Default::default(),
                start_time,
                duration: Duration::hours(24),
                time_res: Duration::seconds(3600),
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
                first_hour_interpolation: Default::default(),
                weather_year: hares_io::WeatherYear::default(),
                warmup: hares_io::Warmup::Disabled,
            },
            overrides,
            1,
            None,
        ),
    }
}

/// A furnace whose heating setpoint source cannot be read (a column index
/// beyond the schedule payload) fails the step naming the equipment, and
/// the run ends under the failure budget instead of heating on static
/// setpoints. Before the fix the read's error was swallowed and the unit
/// ran on its static setpoints with only a tracing warning.
#[test]
fn a_control_update_error_fails_the_step_and_ends_the_run() {
    let broken_source = json!({"Gas Furnace": {"setpoint": {"heating_setpoint_source": {
        "kind": "column_ref",
        "col_idx": 9999,
        "boundary": "Error",
    }}}});
    let mut dwelling = Dwelling::from_config(config(&fixture("base.xml"), Some(broken_source)))
        .expect("the dwelling builds (the corrupt source is a runtime read failure)");

    let mut first_failure = None;
    for step in 0..4 {
        match dwelling.step() {
            Ok(_) => {}
            Err(err) => {
                first_failure = Some((step, err.to_string()));
                break;
            }
        }
    }
    let (step, shown) =
        first_failure.expect("the run must fail: a failing control update cannot step forever");
    assert!(
        shown.contains("Gas Furnace") || shown.contains("failed"),
        "the error names the equipment or the failure budget: {shown}"
    );
    assert!(
        step <= 2,
        "the failure budget (default 1) ends the run by the second failed \
         step; the failure surfaced at step {step}: {shown}"
    );

    // The run stays ended.
    for _ in 0..2 {
        let again = dwelling
            .step()
            .expect_err("the run has ended under the failure budget");
        assert!(
            again.to_string().contains("ended"),
            "the terminal error persists: {again}"
        );
    }
}

/// The same dwelling without the broken source steps through the window.
#[test]
fn the_unbroken_setpoint_source_steps_through_the_window() {
    let mut dwelling =
        Dwelling::from_config(config(&fixture("base.xml"), None)).expect("the dwelling builds");
    for _ in 0..4 {
        dwelling
            .step()
            .expect("the unbroken source steps without error");
    }
}
