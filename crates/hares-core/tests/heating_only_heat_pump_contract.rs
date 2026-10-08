//! An autosized heating-only heat pump keeps the core-output contract.
//!
//! The OS-HPXML heating-only autosize samples declare a heat pump with no
//! HeatingCapacity or CoolingCapacity (both autosized) and
//! `FractionCoolLoadServed = 0`. The cooling side's space-fraction-scaled
//! electric draw is zero on every step while its thermostat still calls for
//! cooling on a warm step, and the unit delivers real zone cooling: the
//! published operating mode must stay Cooling (the delivered thermal flow
//! counts as activity, mirroring OCHRE HVAC.py:556-561, where power scales
//! by the space fraction and "sensible/latent gains to envelope are not
//! updated"), and the core output must satisfy
//! `validate_core_contract` on every step of the day.
//!
//! The ground-to-air heating-only sample is not among these cases: it fails
//! construction because its two-speed heat pump declares one heating
//! capacity where the heater init demands one value per speed, a separate
//! defect from the cooling-side contract violation fixed here.

use std::path::PathBuf;

use chrono::{Duration, TimeZone};
use hares_core::{Dwelling, DwellingConfig};
use hares_io::OutputFormat;

#[path = "../../../tests/support/denver_offset.rs"]
mod denver_offset;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

const HEATING_ONLY_SAMPLES: [&str; 2] = [
    "base-hvac-autosize-air-to-air-heat-pump-1-speed-heating-only.xml",
    "base-hvac-autosize-mini-split-heat-pump-ducted-heating-only.xml",
];

fn build_sample(sample: &str, month: u32) -> Dwelling {
    let sample_path = project_root()
        .join("vendors/OCHRE/test/OS-HPXML Sample Files")
        .join(sample);
    // The samples declare a heating-season setpoint only; the cooling
    // setpoint comes from the defaults. Inject a 71 °F cooling setpoint so
    // the July day deterministically calls for cooling: with the conditioned
    // basement merged into the conditioned space (OS-HPXML
    // geometry.rb `create_or_get_space`, 1704-1716), the ground-coupled
    // basement holds the zone under the defaulted 78 °F on a Denver July
    // day, and the mode contract the test pins (the cooler's operating mode
    // while it delivers) would otherwise never be exercised.
    let xml = std::fs::read_to_string(&sample_path)
        .unwrap_or_else(|err| panic!("{sample} must be readable: {err}"));
    let with_cooling = xml.replace(
        "<SetpointTempHeatingSeason>68.0</SetpointTempHeatingSeason>",
        "<SetpointTempHeatingSeason>68.0</SetpointTempHeatingSeason>\n            \
         <SetpointTempCoolingSeason>71.0</SetpointTempCoolingSeason>",
    );
    assert_ne!(xml, with_cooling, "{sample} must carry a heating setpoint");
    let dir = tempfile::tempdir().expect("temp dir for the patched sample");
    let hpxml_path = dir.path().join(sample);
    std::fs::write(&hpxml_path, &with_cooling)
        .unwrap_or_else(|err| panic!("{sample} patch must be writable: {err}"));

    let start_time = denver_offset::denver_offset()
        .with_ymd_and_hms(2023, month, 15, 0, 0, 0)
        .single()
        .expect("valid start time");
    let config = DwellingConfig {
        hpxml_path,
        // No schedule file: the dwelling generates the schedule from the
        // HPXML, the sweep's generated-schedule condition.
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
            max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
        },
        overrides: None,
        bldg_id: 1,
        initialization_duration: None,
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    };
    Dwelling::from_config(config)
        .unwrap_or_else(|err| panic!("{sample} must build with autosized capacities: {err}"))
}

fn step_a_day(mut dwelling: Dwelling, sample: &str, month: u32) {
    let steps = 24 * 3600 / 900;
    let mut saw_active_cooling = false;
    for step in 0..steps {
        dwelling.step().unwrap_or_else(|err| {
            panic!("{sample} month {month} step {step} must step without an error: {err}")
        });
        // The contract is enforced by the dwelling on every step (a
        // violation fails the run), but the July assertion below keeps this
        // test meaningful if the step-path validation ever moved: the July
        // day must contain a step where the cooling side actively cools, the
        // exact state the defect broke (zone cooling delivered with a
        // space-fraction-zero electric draw).
        for eq in dwelling.equipment() {
            if eq.descriptor().name.ends_with(" Cooler") {
                let mode = eq.telemetry().get("operating_mode").unwrap_or(0.0);
                // 2 = OperatingMode::Cooling.
                if mode == 2.0 {
                    saw_active_cooling = true;
                }
            }
        }
    }
    if month == 7 {
        assert!(
            saw_active_cooling,
            "{sample}: the July day must include an active cooling step on the \
             heat pump's cooler, the state the mode-resolution defect broke"
        );
    }
}

#[test]
fn autosize_heating_only_samples_step_a_day_without_a_contract_violation() {
    for sample in HEATING_ONLY_SAMPLES {
        for month in [1, 7] {
            let dwelling = build_sample(sample, month);
            step_a_day(dwelling, sample, month);
        }
    }
}
