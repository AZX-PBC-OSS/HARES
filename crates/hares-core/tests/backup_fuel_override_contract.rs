//! The heat pump's FUEL capability declaration and its fuel flow read the
//! same config generation.
//!
//! A dwelling-level override that flips the backup fuel's class must flip
//! the declared capability with it: a combustion backup declares FUEL and
//! publishes the flow (Some(0.0) when the burner is idle), an electric
//! backup declares none and publishes none. Before the fix the declaration
//! read the pre-merge config in the constructor while the flow read the
//! merged config at init, so the flip in either direction violated the
//! core-output contract on the first step.

use std::path::PathBuf;

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::{Dwelling, DwellingConfig};
use hares_io::OutputFormat;
use hares_types::CoreCapabilities;
use serde_json::json;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Build one of the OS-HPXML sample dwellings with an optional equipment
/// override map, over the Denver TMY3 weather and a generated schedule.
fn sample_config(sample: &str, overrides: Option<serde_json::Value>) -> DwellingConfig {
    let tz_offset = FixedOffset::west_opt(7 * 3600).expect("UTC-7 offset is valid");
    let start_time = tz_offset
        .with_ymd_and_hms(2023, 1, 15, 0, 0, 0)
        .single()
        .expect("valid start time");
    DwellingConfig {
        hpxml_path: project_root()
            .join("vendors/OCHRE/test/OS-HPXML Sample Files")
            .join(sample),
        schedule_path: None,
        weather_path: project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: hares_io::SimulationConfig {
            start_time,
            duration: Duration::hours(24),
            time_res: Duration::seconds(900),
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
        overrides,
        bldg_id: 1,
        initialization_duration: None,
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    }
}

fn heater_capabilities(dwelling: &Dwelling, name: &str) -> CoreCapabilities {
    dwelling
        .equipment()
        .iter()
        .find(|eq| eq.descriptor().name == name)
        .unwrap_or_else(|| panic!("{name} must be in the assembled equipment"))
        .descriptor()
        .core_capabilities
}

fn step_a_day(mut dwelling: Dwelling, label: &str) {
    let steps = 24 * 3600 / 900;
    for step in 0..steps {
        dwelling
            .step()
            .unwrap_or_else(|err| panic!("{label}: step {step} must not error: {err}"));
    }
}

const GAS_BACKUP_SAMPLE: &str =
    "base-hvac-autosize-dual-fuel-air-to-air-heat-pump-1-speed-sizing-methodology-acca.xml";
const ELECTRIC_BACKUP_SAMPLE: &str = "base-hvac-air-to-air-heat-pump-1-speed.xml";

#[test]
fn gas_backup_sample_declares_fuel_without_an_override() {
    let dwelling = Dwelling::from_config(sample_config(GAS_BACKUP_SAMPLE, None))
        .expect("the dual-fuel sample must build");
    assert!(
        heater_capabilities(&dwelling, "ASHP Heater").contains(CoreCapabilities::FUEL),
        "the combustion backup must declare the FUEL capability"
    );
}

#[test]
fn electric_backup_sample_declares_no_fuel_without_an_override() {
    let dwelling = Dwelling::from_config(sample_config(ELECTRIC_BACKUP_SAMPLE, None))
        .expect("the electric-backup sample must build");
    assert!(
        !heater_capabilities(&dwelling, "ASHP Heater").contains(CoreCapabilities::FUEL),
        "the electric backup must not declare the FUEL capability"
    );
}

#[test]
fn override_to_electric_backup_retracts_the_fuel_declaration_and_steps_a_day() {
    let overrides = json!({"ASHP Heater": {"backup_fuel": "Electric"}});
    let dwelling = Dwelling::from_config(sample_config(GAS_BACKUP_SAMPLE, Some(overrides)))
        .expect("the dual-fuel sample must build with the backup-fuel override");
    assert!(
        !heater_capabilities(&dwelling, "ASHP Heater").contains(CoreCapabilities::FUEL),
        "flipping the backup to electric must retract the FUEL declaration, \
         the flow the step publishes follows the merged config"
    );
    step_a_day(dwelling, "gas-to-electric override");
}

#[test]
fn override_to_gas_backup_declares_fuel_and_steps_a_day() {
    let overrides = json!({"ASHP Heater": {"backup_fuel": "Gas"}});
    let dwelling = Dwelling::from_config(sample_config(ELECTRIC_BACKUP_SAMPLE, Some(overrides)))
        .expect("the electric-backup sample must build with the backup-fuel override");
    assert!(
        heater_capabilities(&dwelling, "ASHP Heater").contains(CoreCapabilities::FUEL),
        "flipping the backup to gas must declare the FUEL capability, \
         the declaration follows the same merged config the flow reads"
    );
    step_a_day(dwelling, "electric-to-gas override");
}
