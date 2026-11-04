// Each bench binary compiles this module and uses a different subset of it.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Duration;
use hares_core::Dwelling;
use hares_core::{DwellingConfig, SimulationConfig};

use hares_core::DwellingSettings;
use hares_io::{DefaultsSource, InputSource, WeatherInput};
use hares_types::{
    CoreCapabilities, CoreOutput, EndUse, EquipmentDescriptor, EquipmentId, ExecutionStage,
    FuelType, PortDeclaration, Telemetry,
};

#[path = "../tests/support/fixture_start.rs"]
mod fixture_start;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// `dir/<prefix>-<n>.<ext>` with a process-wide counter `n`, so every call
/// names a new file. `dir` is the benchmark's own directory, which each
/// benchmark function creates with `tempfile::tempdir()` before any timed
/// region and which is removed with everything in it when dropped.
pub fn numbered_path(dir: &Path, prefix: &str, ext: &str) -> PathBuf {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    dir.join(format!("{prefix}-{id}.{ext}"))
}

pub fn fixture_hpxml_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/hpxml/ochre_samples/base.xml")
}

/// The shipped defaults directory: the benchmark schedule has no lighting or
/// plug-load columns, so those loads take their default profiles from here.
pub fn repo_defaults_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../python/hares/defaults")
}

pub fn write_schedule_csv(path: &PathBuf) {
    // The canonical schedule column names the resolver's mapping table
    // reads: every event appliance the HPXML fixture declares needs its
    // own fraction column (the constant-window fallback for a missing
    // column is a construction error), and the file carries no setpoint
    // columns, so the HVAC setpoints come from the HPXML's own profiles.
    // The shipped defaults directory supplies the lighting and plug-load
    // profiles the file omits.
    let rows = [
        "Time,clothes_washer,clothes_dryer,dishwasher,cooking_range,hot_water_clothes_washer,hot_water_dishwasher",
        "2021-01-01T00:00:00-07:00,0.10,0.0,0.0,0.0,0.1,0.0",
        "2021-01-01T00:15:00-07:00,0.20,0.5,0.0,0.0,0.2,0.0",
        "2021-01-01T00:30:00-07:00,0.15,0.5,0.3,0.0,0.15,0.3",
        "2021-01-01T00:45:00-07:00,0.12,0.0,0.0,0.6,0.12,0.0",
    ]
    .join("\n");
    fs::write(path, rows).expect("failed to write schedule fixture");
}

pub fn write_weather_epw(path: &PathBuf) {
    use chrono::{Datelike, Duration as ChronoDuration, NaiveDate, Timelike};

    let mut lines = vec![
        "LOCATION,Benchmark,CZ4A,USA,TMY3,999999,39.74,-104.99,-7.0,1609.3".to_string(),
        "DESIGN CONDITIONS,0".to_string(),
        "GROUND TEMPERATURES,0".to_string(),
        "TYPICAL/EXTREME PERIODS,0".to_string(),
        "HOLIDAYS/DAYLIGHT SAVINGS,No,0,0,0".to_string(),
        "COMMENTS 1,synthetic benchmark weather".to_string(),
        "COMMENTS 2,synthetic benchmark weather".to_string(),
        "DATA PERIODS,1,1,Data,Sunday, 1/ 1,12/31".to_string(),
    ];

    let start_date = NaiveDate::from_ymd_opt(2021, 1, 1).expect("valid date");
    let start_time = start_date.and_hms_opt(0, 0, 0).expect("valid time");
    for i in 0..8760 {
        let timestamp = start_time + ChronoDuration::hours(i as i64);
        let year = timestamp.year();
        let month = timestamp.month();
        let day = timestamp.day();
        let hour = timestamp.hour() + 1;

        let row = [
            year.to_string(),
            month.to_string(),
            day.to_string(),
            hour.to_string(),
            "0".to_string(),
            "A0A0A0A0*0*0*0*0*0*0*0*0*0*0".to_string(),
            "19.0".to_string(),
            "10.0".to_string(),
            "50".to_string(),
            "101325".to_string(),
            "0".to_string(),
            "0".to_string(),
            "300".to_string(),
            "100".to_string(),
            "200".to_string(),
            "50".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "180".to_string(),
            "3.5".to_string(),
            "4".to_string(),
            "4".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
        ];
        lines.push(row.join(","));
    }

    fs::write(path, lines.join("\n")).expect("failed to write weather fixture");
}

pub fn build_dwelling_config(
    dir: &Path,
    bldg_id: i64,
    schedule_path: PathBuf,
    weather_path: PathBuf,
    defaults_path: PathBuf,
    duration: Duration,
) -> DwellingConfig {
    DwellingConfig {
        hpxml: InputSource::Path(fixture_hpxml_path()),
        schedule: Some(InputSource::Path(schedule_path)),
        weather: WeatherInput::Path(weather_path),
        defaults: DefaultsSource::directory(defaults_path),
        settings: DwellingSettings::new(
            SimulationConfig {
                max_memory_rows: None,
                scenario_seed: None,
                schedule_diversity: Default::default(),
                start_time: fixture_start::fixture_start(),
                duration,
                time_res: Duration::minutes(1),
                reports: vec![hares_io::ReportSpec {
                    name: "file".to_string(),
                    selection: hares_io::Selection::Verbosity(0),
                    frequency: hares_io::Frequency::default(),
                    destination: hares_io::Destination::Csv {
                        path: numbered_path(dir, "hares-bench-output", "csv"),
                        overwrite: true,
                    },
                }],
                diagnostics: false,
                metrics: false,
                output_chunk_size: 1024,
                setpoint_deadband_c: None,
                master_seed: 0,
                civil_timezone: None,
                site_location: hares_io::SiteLocationOverride::default(),
                max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                health_policy: hares_io::HealthPolicy::default(),
                first_hour_interpolation: hares_io::FirstHour::default(),
                weather_year: hares_io::WeatherYear::default(),
                warmup: hares_io::Warmup::Disabled,
            },
            None,
            bldg_id,
            None,
        ),
    }
}

pub fn synthetic_toml(duration_s: i64, time_res_s: i64) -> String {
    synthetic_toml_with_start(duration_s, time_res_s, "2024-01-01T00:00:00Z")
}

/// The same fixture at a caller's start: the year-episode witness's
/// non-leap case starts 2023-01-01, where the run year carries no Feb 29
/// for the typical-weather rule to skip.
pub fn synthetic_toml_with_start(duration_s: i64, time_res_s: i64, start: &str) -> String {
    format!(
        r#"building_id = 1

[simulation]
start_time = "{start}"
time_res = {time_res_s}
duration = {duration_s}
output_chunk_size = 1000
master_seed = 0
[geometry]
number_of_bedrooms = 3.0
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 25.0

[weather]
outdoor_temp_c = 8.0
dew_point_c = 4.0
rel_humidity_pct = 55.0
pressure_kpa = 101.325

# The infiltration measurement the airflow assembly requires (OS-HPXML
# errors on a dwelling with neither a measurement nor a constant rate):
# 3 ACH, a conventional continuous rate for the leaky tier this
# synthetic single-zone dwelling models (R-2 walls, no controlled
# ventilation). The bench's workload is the stepping cost, unchanged
# from what the fixture intended before the assembly hardened.
[infiltration]
ach = 3.0

[schedule]
occupancy = 1.0
"#
    )
}

pub fn synthetic_toml_case(path: &PathBuf, duration_s: i64) {
    let toml = synthetic_toml(duration_s, 60);
    fs::write(path, toml).expect("failed to write synthetic TOML fixture");
}

/// The checkpoint bench's small roster: the synthetic TOML dwelling (one
/// zone, one electric furnace, no event loads, no actors), the checkpoint
/// floor, the same fixture the rl_step bench builds. The run's 26-hour
/// duration covers the bench's 24-hour prefix and its continuation with
/// margin, so the assertion's steps never meet the run's end.
pub const CHECKPOINT_SMALL_PREFIX_STEPS: u64 = 1_440; // 24 h at 60 s
pub const CHECKPOINT_SMALL_CONTINUATION_STEPS: u64 = 60; // 1 h

/// The checkpoint bench's large roster: the HPXML base.xml fixture at the
/// fleet shape (900 s steps, no own reports), the eight-equipment roster
/// the fleet and single_building benches run (heating, cooling, water
/// heating, clothes washer, clothes dryer, dishwasher, cooking range,
/// refrigerator; four of them RNG-carrying event appliances). The run's
/// 48-hour duration covers the 24-hour prefix and the continuation with
/// margin.
pub const CHECKPOINT_LARGE_PREFIX_STEPS: u64 = 96; // 24 h at 900 s
pub const CHECKPOINT_LARGE_CONTINUATION_STEPS: u64 = 48; // 12 h

/// The rl_episode bench's year-episode shape, shared with the
/// bench_construction suite's year witnesses: 366 days at 3600 s steps.
/// The leap-year start's Feb 29 is skipped under the typical weather
/// rule, so the duration holds exactly 8,760 steps with the skip and
/// 8,784 without it (the non-leap start).
pub const YEAR_EPISODE_DURATION_S: i64 = 31_622_400;
pub const YEAR_EPISODE_TIME_RES_S: i64 = 3_600;
pub const YEAR_EPISODE_STEPS: usize = 8_760;

/// The checkpoint bench's small-roster dwelling: the shared synthetic
/// TOML through `from_toml_config_with_reports`, the bench's own
/// construction call, with no reports (the restore refuses non-memory
/// report destinations, and the bench measures the checkpoint, not the
/// report row streams).
pub fn checkpoint_small_dwelling(dir: &Path) -> Dwelling {
    let path = numbered_path(dir, "hares-bench-checkpoint-small", "toml");
    synthetic_toml_case(&path, 93_600);
    Dwelling::from_toml_config_with_reports(&path, Some(Vec::new()))
        .expect("the checkpoint bench's small dwelling builds")
}

/// The checkpoint bench's large-roster dwelling config: the shared HPXML
/// fixture through `build_dwelling_config` at the fleet shape.
pub fn checkpoint_large_config(
    dir: &Path,
    bldg_id: i64,
    schedule_path: &Path,
    weather_path: &Path,
) -> DwellingConfig {
    let mut config = build_dwelling_config(
        dir,
        bldg_id,
        schedule_path.to_path_buf(),
        weather_path.to_path_buf(),
        repo_defaults_path(),
        Duration::hours(48),
    );
    config.settings.sim_config.reports.clear();
    config.settings.sim_config.time_res = Duration::seconds(900);
    config
}

/// The checkpoint bench's large-roster dwelling: the config above through
/// `Dwelling::from_config`, the construction the restore target repeats.
pub fn checkpoint_large_dwelling(
    dir: &Path,
    bldg_id: i64,
    schedule_path: &Path,
    weather_path: &Path,
) -> Dwelling {
    let config = checkpoint_large_config(dir, bldg_id, schedule_path, weather_path);
    Dwelling::from_config(config).expect("the checkpoint bench's large dwelling builds")
}

/// The invariant-cost bench's run lengths: the checkpoint bench's own
/// roster shapes (the synthetic TOML's 26 hours, the HPXML fleet shape's
/// 48 hours), each holding the marching arm's steps with margin.
pub const INVARIANT_COST_SMALL_DURATION_S: i64 = 93_600;
pub const INVARIANT_COST_LARGE_DURATION_S: i64 = 172_800;

/// The invariant-cost bench's march lengths per timed iteration: eight
/// simulated hours of the small roster's 60 s steps, one simulated day
/// of the large roster's 900 s steps.
pub const INVARIANT_COST_SMALL_ARM_STEPS: u64 = 480;
pub const INVARIANT_COST_LARGE_ARM_STEPS: u64 = 96;

/// The invariant-cost bench's small-roster dwelling: the shared synthetic
/// TOML through `from_toml_config_with_reports`, no reports, at the
/// bench's 26-hour run length.
pub fn invariant_cost_small_dwelling(dir: &Path) -> Dwelling {
    let path = numbered_path(dir, "hares-bench-invariant-small", "toml");
    synthetic_toml_case(&path, INVARIANT_COST_SMALL_DURATION_S);
    Dwelling::from_toml_config_with_reports(&path, Some(Vec::new()))
        .expect("the invariant-cost bench's small dwelling builds")
}

/// The invariant-cost bench's large-roster dwelling: the shared HPXML
/// fixture through `build_dwelling_config` at the fleet shape (no own
/// reports, 900 s steps) at the bench's 48-hour run length.
pub fn invariant_cost_large_dwelling(dir: &Path, bldg_id: i64) -> Dwelling {
    let schedule_path = numbered_path(dir, "hares-bench-invariant-schedule", "csv");
    let weather_path = numbered_path(dir, "hares-bench-invariant-weather", "epw");
    write_schedule_csv(&schedule_path);
    write_weather_epw(&weather_path);
    let mut config = build_dwelling_config(
        dir,
        bldg_id,
        schedule_path,
        weather_path,
        repo_defaults_path(),
        Duration::seconds(INVARIANT_COST_LARGE_DURATION_S),
    );
    config.settings.sim_config.reports.clear();
    config.settings.sim_config.time_res = Duration::seconds(900);
    Dwelling::from_config(config).expect("the invariant-cost bench's large dwelling builds")
}

/// The validation bench's witness equipment: an electric
/// Independent-stage appliance that can break the per-equipment contract
/// the enforcement block checks, deposit a matching flow, fail its own
/// step, or any combination the witness needs.
/// `missing_declared_field` publishes no electric flow although the
/// descriptor declares the ELECTRIC capability (the core-output
/// contract's missing-field arm) and deposits nothing at its port, so no
/// downstream check sees a divergence the block itself does not name;
/// `reactive_divergent` deposits active power its core output matches
/// plus reactive power its core output does not report, so only the
/// port/core electrical consistency check's reactive arm fires;
/// `depositing` and `failing_after_deposit` carry the capture witness:
/// the first leaves a 1 kW contribution in the shared electrical bus, the
/// second deposits 500 W and returns an error, so the step's rollback
/// either restores the deposit (the enforced state) or swaps in the
/// snapshot the skipped capture never wrote (the off state), and the
/// step's always-on consistency check separates the two. The
/// dwelling's real rosters never violate the contract; this equipment
/// exists so the witness can prove which way the enforcement switch
/// points.
struct WitnessEquipment {
    descriptor: EquipmentDescriptor,
    telemetry: Telemetry,
    core_output: CoreOutput,
    deposited_active_w: f64,
    deposited_reactive_var: f64,
    step_error: Option<&'static str>,
    ports: Vec<PortDeclaration>,
}

impl WitnessEquipment {
    /// The missing-declared-field flavor: `validate_core_contract` fails
    /// the step because the declared `flows.electric_w` is absent. The
    /// port sees nothing, so the off state's step carries no divergence
    /// for any other check to catch.
    fn missing_declared_field(name: &str) -> Self {
        Self {
            descriptor: witness_descriptor(name),
            telemetry: Telemetry::default(),
            core_output: CoreOutput::default(),
            deposited_active_w: 0.0,
            deposited_reactive_var: 0.0,
            step_error: None,
            ports: vec![PortDeclaration::electrical()],
        }
    }

    /// The reactive-divergent flavor: the core output's active flow
    /// matches the 3 kW the step deposits, and the step deposits 500 VAR
    /// the core output does not report, so
    /// `validate_port_core_electrical_consistency` fails the step on the
    /// reactive divergence alone.
    fn reactive_divergent(name: &str) -> Self {
        let mut core_output = CoreOutput::default();
        core_output.flows.electric_w =
            Some(hares_types::ElectricPower::consumption(3_000.0).unwrap());
        Self {
            descriptor: witness_descriptor(name),
            telemetry: Telemetry::default(),
            core_output,
            deposited_active_w: 3_000.0,
            deposited_reactive_var: 500.0,
            step_error: None,
            ports: vec![PortDeclaration::electrical()],
        }
    }

    /// The capture witness's depositing step: 1 kW into the shared ports,
    /// reported in the core output so the enforced state's per-equipment
    /// checks pass it. Its contribution is what the next equipment's
    /// failed step either restores from the pre-step capture or loses.
    fn depositing(name: &str) -> Self {
        let mut core_output = CoreOutput::default();
        core_output.flows.electric_w =
            Some(hares_types::ElectricPower::consumption(1_000.0).unwrap());
        Self {
            descriptor: witness_descriptor(name),
            telemetry: Telemetry::default(),
            core_output,
            deposited_active_w: 1_000.0,
            deposited_reactive_var: 0.0,
            step_error: None,
            ports: vec![PortDeclaration::electrical()],
        }
    }

    /// The capture witness's failing step: 500 W deposited, then an error,
    /// so `record_equipment_step_failure` runs the rollback over the ports
    /// the capture did or did not save.
    fn failing_after_deposit(name: &str) -> Self {
        Self {
            descriptor: witness_descriptor(name),
            telemetry: Telemetry::default(),
            core_output: CoreOutput::default(),
            deposited_active_w: 500.0,
            deposited_reactive_var: 0.0,
            step_error: Some("the witness equipment's step fails"),
            ports: vec![PortDeclaration::electrical()],
        }
    }
}

/// The witness equipment's descriptor: a minimal unzoned electric
/// appliance in the Independent stage, no control surface, no telemetry
/// fields, the ELECTRIC core capability declared.
fn witness_descriptor(name: &str) -> EquipmentDescriptor {
    EquipmentDescriptor {
        id: EquipmentId(0),
        name: name.to_string(),
        end_use: EndUse::OTHER,
        equipment_type: std::borrow::Cow::Borrowed("WitnessEquipment"),
        zone: None,
        fuel: FuelType::Electric,
        stage: ExecutionStage::Independent,
        control_capabilities: hares_types::ControlCapabilities::empty(),
        core_capabilities: CoreCapabilities::ELECTRIC,
        telemetry_fields: vec![],
        zone_type: None,
    }
}

impl hares_equipment::Equipment for WitnessEquipment {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn resolved_parameters(&self) -> Vec<hares_types::ResolvedParameter> {
        Vec::new()
    }

    fn rename(&mut self, name: String) {
        self.descriptor.name = name;
    }

    fn set_equipment_id(
        &mut self,
        id: EquipmentId,
    ) -> std::result::Result<(), hares_types::HaresError> {
        hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(
        &mut self,
        _config: &hares_equipment::EquipmentConfig,
        _env: &hares_types::EnvironmentState,
    ) -> std::result::Result<(), hares_types::HaresError> {
        self.telemetry
            .insert("power_kw", self.deposited_active_w / 1000.0);
        Ok(())
    }

    fn update_control(
        &mut self,
        _env: &hares_types::EnvironmentState,
    ) -> hares_types::OperatingMode {
        hares_types::OperatingMode::Off
    }

    fn step(
        &mut self,
        _env: &hares_types::EnvironmentState,
        _dt: std::time::Duration,
        ports: &mut hares_types::ports::PortSlots,
    ) -> std::result::Result<(), hares_types::HaresError> {
        ports.accumulate(&hares_types::ports::PortContribution::Electrical {
            active_power_w: self.deposited_active_w,
            reactive_power_var: self.deposited_reactive_var,
        })?;
        match self.step_error {
            Some(message) => Err(hares_types::HaresError::Equipment(message.to_string())),
            None => Ok(()),
        }
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core_output
    }

    fn save_state(&self) -> std::result::Result<Vec<u8>, hares_types::HaresError> {
        Ok(vec![])
    }

    fn load_state(&mut self, _state: &[u8]) -> std::result::Result<(), hares_types::HaresError> {
        Ok(())
    }

    fn apply_signal(
        &mut self,
        _signal: &hares_types::ControlSignal,
    ) -> std::result::Result<(), hares_types::HaresError> {
        Ok(())
    }
}

/// The validation bench's witness, run once before anything is timed and
/// again by the bench_construction suite at suite cost: for each
/// violation flavor a dwelling steps under each enforcement state, a
/// fresh dwelling per state (a state's failed step leaves the deposits
/// the abort did not roll back, so the states share no dwelling), and
/// the assertions pin the block's behavior. The default state and the
/// re-enabled state fail the step naming the violated check (the block
/// the timed on arm prices is the enforcement block itself); the off
/// state runs the same violation clean (the comparison arm really skips
/// the block, so the paired delta is its cost, not a no-op's). Both
/// violation flavors are witnessed: the core-output contract's
/// missing-field arm and the consistency check's reactive arm. The
/// block's third site, the pre-step rollback capture, has its own case:
/// a step that fails after its predecessor deposited 1 kW restores that
/// deposit in the enforced state and loses it in the off state, so a
/// switch that gated only the checks cannot pass this witness while the
/// timed off arm still pays for the capture.
pub fn assert_validation_witness(dir: &Path) {
    const MISSING: &str = "witness missing declared field";
    const DIVERGENT: &str = "witness reactive divergent";

    let mut dwelling = witness_dwelling(dir, WitnessEquipment::missing_declared_field(MISSING));
    let err = dwelling.step().expect_err(
        "the default state enforces the core-output contract: the missing \
                     declared field fails the step",
    );
    let msg = err.to_string();
    assert!(
        msg.contains("core_output contract violation") && msg.contains(MISSING),
        "the default state's step error must name the contract violation and the \
         equipment: {msg}"
    );

    let mut dwelling = witness_dwelling(dir, WitnessEquipment::missing_declared_field(MISSING));
    dwelling.set_step_validation_enabled(false);
    dwelling
        .step()
        .expect("the off state is the comparison arm: the same violation runs clean");

    let mut dwelling = witness_dwelling(dir, WitnessEquipment::missing_declared_field(MISSING));
    dwelling.set_step_validation_enabled(true);
    let err = dwelling.step().expect_err(
        "re-enabling restores the enforcement: the switch is a state, not \
                     a one-time latch",
    );
    assert!(
        err.to_string().contains("core_output contract violation"),
        "the re-enabled state's step error must name the contract violation: {err}"
    );

    let mut dwelling = witness_dwelling(dir, WitnessEquipment::reactive_divergent(DIVERGENT));
    let err = dwelling.step().expect_err(
        "the default state enforces the port/core electrical consistency: \
                     the unreported reactive power fails the step",
    );
    let msg = err.to_string();
    assert!(
        msg.contains("port/core electrical consistency violation") && msg.contains(DIVERGENT),
        "the default state's step error must name the consistency violation and the \
         equipment: {msg}"
    );

    let mut dwelling = witness_dwelling(dir, WitnessEquipment::reactive_divergent(DIVERGENT));
    dwelling.set_step_validation_enabled(false);
    dwelling
        .step()
        .expect("the off state is the comparison arm: the same divergence runs clean");

    let mut dwelling = witness_dwelling(dir, WitnessEquipment::reactive_divergent(DIVERGENT));
    dwelling.set_step_validation_enabled(true);
    let err = dwelling
        .step()
        .expect_err("re-enabling restores the consistency enforcement");
    assert!(
        err.to_string()
            .contains("port/core electrical consistency violation"),
        "the re-enabled state's step error must name the consistency violation: {err}"
    );

    // The capture's own case: the block's pre-step rollback capture is
    // the third site the switch gates. The depositor runs first in the
    // same step and leaves 1 kW in the shared electrical bus; the failing
    // equipment then deposits 500 W and returns an error, so the rollback
    // runs over the snapshot the capture did or did not save.
    let mut dwelling = capture_witness_dwelling(dir);
    dwelling.step().expect(
        "the enforced state captures the bus before the failing equipment, so \
         the rollback restores the deposit and the step's always-on \
         electrical consistency check passes",
    );

    // The off state skipped the capture, so the rollback swaps in the
    // snapshot the skipped capture never wrote: the deposit is gone from
    // the bus while the depositor still reports it, and the step's
    // always-on electrical consistency check names the 1 kW divergence.
    // A switch that gated only the two checks would pass every case above
    // and this one, so the timed off arm would still pay for the capture
    // while the paired delta was read as the block's whole cost.
    let mut dwelling = capture_witness_dwelling(dir);
    dwelling.set_step_validation_enabled(false);
    let err = dwelling.step().expect_err(
        "the off state's skipped capture leaves the failed step's bus \
         incoherent, and the always-on check refuses it",
    );
    let msg = err.to_string();
    assert!(
        msg.contains("telemetry electrical consistency"),
        "the off state's lost deposit must surface as the always-on \
         electrical consistency violation, not another error: {msg}"
    );
}

/// The witness's stepping dwelling: the synthetic single-equipment
/// fixture carrying one witness equipment.
fn witness_dwelling(dir: &Path, eq: WitnessEquipment) -> Dwelling {
    let mut dwelling = invariant_cost_small_dwelling(dir);
    dwelling
        .add_equipment(Box::new(eq))
        .expect("the witness's equipment registers");
    dwelling
}

/// The capture witness's dwelling: the synthetic fixture with the
/// depositing equipment registered before the failing one, so the step's
/// execution order (insertion order within the Independent stage) leaves
/// the deposit in the ports before the failure's rollback runs.
fn capture_witness_dwelling(dir: &Path) -> Dwelling {
    let mut dwelling = invariant_cost_small_dwelling(dir);
    for equipment in [
        WitnessEquipment::depositing("witness capture depositor"),
        WitnessEquipment::failing_after_deposit("witness capture failure"),
    ] {
        dwelling
            .add_equipment(Box::new(equipment))
            .expect("the capture witness's equipment registers");
    }
    dwelling
}

/// Steps `n` times, failing with `what` named: the bench's prefix and
/// continuation loops, and the same loops in the suite's copy.
pub fn step_n(dwelling: &mut Dwelling, n: u64, what: &str) {
    for _ in 0..n {
        dwelling
            .step()
            .unwrap_or_else(|err| panic!("{what}: the step fails: {err}"));
    }
}

/// The checkpoint's canonical JSON bytes: the `serde_json::Value` form,
/// whose objects are BTreeMaps (serde_json without `preserve_order`), so
/// every map's keys render in sorted order whatever iteration order the
/// snapshot's own HashMap instances happened to hold. The canonical form
/// is the bench's bitwise witness: equal bytes pin equal structure and,
/// for finite values, equal float bits (the shortest round-trip form is
/// injective per value, minus-zero rendering apart from zero).
/// Non-finite values render as serde_json `null` and cannot be
/// distinguished in the bytes; the `PartialEq` leg catches the NaN cases
/// anyway (a NaN never compares equal).
pub fn canonical_checkpoint_bytes(checkpoint: &hares_core::DwellingCheckpoint) -> Vec<u8> {
    let value = serde_json::to_value(checkpoint).expect("the checkpoint converts to a JSON value");
    serde_json::to_vec(&value).expect("the canonical form serializes")
}

/// The checkpoint bench's setup assertions, run once per roster size
/// outside the timed regions (and by the bench_construction suite at
/// suite cost), failing loudly if either breaks: a bench that cannot
/// detect a deliberate regression in the path it times is a finding.
///
/// 1. State bit-identity after restore: the prefix-stepped state saves,
///    serializes, parses through the gated reader and restores into a
///    freshly built dwelling of the same fixture; the restored dwelling
///    re-saves a checkpoint equal to the saved one field for field (the
///    type derives `PartialEq`, the maps comparing order-independently)
///    and equal in canonical bytes (every finite float bit preserved;
///    the non-finite values render as `null` there, the `PartialEq` leg
///    catching the NaN cases).
/// 2. The restored run's continuation equivalence: the restored dwelling
///    and a fresh continuous dwelling step the same further steps; the
///    two end states are equal field for field and in canonical bytes,
///    the one named actor-facing divergence narrowed to its key (see the
///    compare's comment). The full output-surface acceptance stays with
///    the checkpoint_resume suite; this is the bench's own short witness.
pub fn assert_checkpoint_round_trip(
    dir: &Path,
    build: impl Fn() -> Dwelling,
    prefix_steps: u64,
    continuation_steps: u64,
    label: &str,
) {
    let mut source = build();
    step_n(&mut source, prefix_steps, &format!("{label}: the prefix"));
    let checkpoint = source
        .save_checkpoint()
        .unwrap_or_else(|err| panic!("{label}: the prefix state saves: {err}"));
    // The pinned file path: the serde_json serialization, the SHA-256
    // checksum and the atomic rename, then the checksum verify and the
    // gated parse back.
    let file = numbered_path(dir, "hares-bench-checkpoint-assert", "json");
    checkpoint
        .save(&file)
        .unwrap_or_else(|err| panic!("{label}: the checkpoint file writes: {err}"));
    let loaded = hares_core::DwellingCheckpoint::load(&file)
        .unwrap_or_else(|err| panic!("{label}: the checkpoint file loads: {err}"));

    // Bit-identity: the gated parse restores into a fresh dwelling of the
    // same fixture, and that dwelling re-saves the same state.
    let mut restored = build();
    restored
        .load_checkpoint(loaded.clone())
        .unwrap_or_else(|err| panic!("{label}: the checkpoint restores: {err}"));
    let re_saved = restored
        .save_checkpoint()
        .unwrap_or_else(|err| panic!("{label}: the restored state saves: {err}"));
    assert_eq!(
        loaded, checkpoint,
        "{label}: the file round trip changes the checkpoint"
    );
    assert_eq!(
        re_saved, loaded,
        "{label}: the restored dwelling re-saves a different checkpoint"
    );
    assert_eq!(
        canonical_checkpoint_bytes(&re_saved),
        canonical_checkpoint_bytes(&checkpoint),
        "{label}: the re-saved checkpoint's canonical bytes differ from the saved run's"
    );

    // Continuation equivalence: the restored run and the continuous run
    // step the same further steps and hold the same committed state. The
    // one named actor-facing divergence is narrowed to its exact key
    // instead of excluding the maps wholesale: the air conditioner's
    // `load_state` inserts the `shr` telemetry key at the restore (the
    // checkpoint's decoded value, air_conditioner.rs's load_state), and
    // the first post-restore step's snapshot copies it into the
    // actor-facing map; the continuous run's never-cooled air
    // conditioner's step never published the key, so the key exists in
    // the restored run's map only. Exactly that key is removed from the
    // restored side where the continuous side lacks it (at most one
    // registration: a second would be some other equipment's shr, not
    // the named divergence), and the two actor-facing maps otherwise
    // compare in full — any other key registered, any value differing,
    // fails, as does an equipment entry present on one side only. The
    // committed state the checkpoint carries (the equipment blobs, the
    // thermal, humidity and fluid state, the RNG, the electrical
    // summary, the health and warning records) compares in full.
    step_n(
        &mut restored,
        continuation_steps,
        &format!("{label}: the continuation"),
    );
    let mut continuous = build();
    step_n(
        &mut continuous,
        prefix_steps + continuation_steps,
        &format!("{label}: the continuous run"),
    );
    let continuous_end = continuous
        .save_checkpoint()
        .unwrap_or_else(|err| panic!("{label}: the continuous state saves: {err}"));
    let mut restored_end = restored
        .save_checkpoint()
        .unwrap_or_else(|err| panic!("{label}: the continued state saves: {err}"));
    let mut narrowed = 0usize;
    for (name, telemetry) in restored_end.env_equipment_telemetry.iter_mut() {
        let Some(continuous_telemetry) = continuous_end.env_equipment_telemetry.get(name) else {
            continue;
        };
        if telemetry.0.contains_key("shr") && !continuous_telemetry.0.contains_key("shr") {
            telemetry.0.remove("shr");
            narrowed += 1;
        }
    }
    assert!(
        narrowed <= 1,
        "{label}: {narrowed} equipments' `shr` keys are restorations' own; \
         the named divergence is one registration"
    );
    assert_eq!(
        restored_end, continuous_end,
        "{label}: the resumed run's committed state after {continuation_steps} further \
         steps is not the continuous run's"
    );
    assert_eq!(
        canonical_checkpoint_bytes(&restored_end),
        canonical_checkpoint_bytes(&continuous_end),
        "{label}: the resumed run's committed state after {continuation_steps} further \
         steps is not the continuous run's, byte for byte"
    );
}
