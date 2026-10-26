//! The not-applicable output cell is null, never a substituted 0.0
//! (`09-numeric-fixes.md` B-6): the producers write `None` for a quantity
//! undefined in the current state, the row plan's validity bitset carries
//! the absence into every batch, and the consumers' statistics (the
//! reducer's valid-step counts, the fleet's fold, polars) read nulls. The
//! not-nullable quantities (the physically-zero-when-inactive ones) stay
//! 0.0, and a declared channel's missing value is the wiring defect the
//! core-output contract fails the run with (OR-5).

use arrow::array::{Array, Float64Array, RecordBatch};
use chrono::{DateTime, Duration, FixedOffset, TimeZone};
use hares_control::DispatchRequest;
use hares_core::actor::Actor;
use hares_core::{Dwelling, DwellingConfig, DwellingSettings};
use hares_io::{
    DefaultsSource, Destination, Frequency, InputSource, ReportSpec, Selection, SimulationConfig,
    WeatherInput,
};
use hares_types::{
    ControlCapabilities, CoreCapabilities, CoreOutput, EndUse, EnvironmentState,
    EquipmentDescriptor, EquipmentId, ExecutionStage, FuelType, HaresError, OperatingMode,
    PortSlots, Telemetry, TelemetryField, Unit,
};
use std::path::PathBuf;
use std::time::Duration as StdDuration;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sim_config(
    start: DateTime<FixedOffset>,
    duration_s: i64,
    step_s: i64,
    verbosity: u8,
) -> SimulationConfig {
    SimulationConfig {
        scenario_seed: None,
        start_time: start,
        duration: Duration::seconds(duration_s),
        time_res: Duration::seconds(step_s),
        reports: vec![ReportSpec {
            name: "frame".to_string(),
            selection: Selection::Verbosity(verbosity),
            frequency: Frequency::default(),
            destination: Destination::Memory,
        }],
        diagnostics: false,
        metrics: false,
        output_chunk_size: 4096,
        setpoint_deadband_c: None,
        master_seed: 0,
        civil_timezone: None,
        site_location: hares_io::SiteLocationOverride::default(),
        max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
        health_policy: hares_io::HealthPolicy::default(),
        first_hour_interpolation: Default::default(),
        weather_year: hares_io::WeatherYear::default(),
        warmup: hares_io::Warmup::Disabled,
    }
}

fn cz4a_dwelling(
    start: DateTime<FixedOffset>,
    duration_s: i64,
    step_s: i64,
    verbosity: u8,
) -> Dwelling {
    let fixture = project_root().join("tests/fixtures/parity/cz4a_ashp_hpwh");
    let config = DwellingConfig {
        hpxml: InputSource::Path(fixture.join("building.xml")),
        schedule: Some(InputSource::Path(fixture.join("schedule.csv"))),
        weather: WeatherInput::Path(fixture.join("weather.epw")),
        defaults: DefaultsSource::directory(project_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(
            sim_config(start, duration_s, step_s, verbosity),
            None,
            1,
            None,
        ),
    };
    Dwelling::from_config(config).expect("cz4a dwelling builds")
}

/// The named column of a run's frame: every row's (value, validity) in
/// row order.
fn column(dwelling: &Dwelling, name: &str) -> Vec<(f64, bool)> {
    let batches: &[RecordBatch] = dwelling.flushed_batches();
    assert!(!batches.is_empty(), "the run recorded a frame");
    let mut cells = Vec::new();
    for batch in batches {
        let values = batch
            .column_by_name(name)
            .unwrap_or_else(|| panic!("the frame carries '{name}'"))
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("a Float64 column")
            .clone();
        for row in 0..values.len() {
            cells.push((values.value(row), values.is_valid(row)));
        }
    }
    cells
}

const MODE_OFF: f64 = OperatingMode::Off as u8 as f64;

/// The heating-variant mode codes: `OperatingMode`'s heating side.
fn is_heating_mode(code: f64) -> bool {
    matches!(
        code as u64,
        1 | 7 | 8 | 9 | 10 | 11 // Heating, HeatingHP, HeatingER, HeatingHPAndER, HeatPumpWH, BackupElement
    )
}

#[test]
fn undefined_quantities_are_null() {
    // The `tests/fixtures/parity/cz4a_ashp_hpwh` inputs (an air-source heat
    // pump and a heat-pump water heater), two January days in Denver at
    // 900 s, warm-up disabled. Verbosity 7 carries the COP and SHR columns.
    let start = FixedOffset::west_opt(7 * 3600)
        .expect("offset")
        .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
        .single()
        .expect("start");
    let mut dwelling = cz4a_dwelling(start, 2 * 86_400, 900, 7);
    dwelling.simulate().expect("the run completes");

    let mode = column(&dwelling, "ASHP Heater Mode (-)");
    let cop = column(&dwelling, "ASHP Heater COP (-)");
    let shr = column(&dwelling, "ASHP Cooler SHR (-)");

    // The run has at least one Off step and at least one heating step (two
    // January days in Denver).
    let off_steps: Vec<usize> = mode
        .iter()
        .enumerate()
        .filter(|(_, (code, _))| *code == MODE_OFF)
        .map(|(row, _)| row)
        .collect();
    let heating_steps: Vec<usize> = mode
        .iter()
        .enumerate()
        .filter(|(_, (code, _))| is_heating_mode(*code))
        .map(|(row, _)| row)
        .collect();
    assert!(
        !off_steps.is_empty(),
        "two January days leave the ASHP Heater off in some step"
    );
    assert!(
        !heating_steps.is_empty(),
        "two January days leave the ASHP Heater heating in some step"
    );

    // The ASHP Cooler's sensible heat ratio is null in every step in which
    // it is not cooling (every step of the run): the ratio is undefined
    // without cooling, never 0.0 or a 1.0 sentinel.
    for (row, (value, valid)) in shr.iter().enumerate() {
        let _ = value;
        assert!(!valid, "ASHP Cooler SHR row {row} is null, not a sentinel");
    }

    // The ASHP Heater's COP is null in every Off step and non-null in
    // every heating step.
    for &row in &off_steps {
        assert!(
            !cop[row].1,
            "ASHP Heater COP row {row} (mode off) is null, not 0.0"
        );
    }
    for &row in &heating_steps {
        assert!(
            cop[row].1,
            "ASHP Heater COP row {row} (heating) is non-null"
        );
        assert!(cop[row].0 > 0.0, "a heating step's COP is positive");
    }
}

#[test]
fn inactive_power_is_zero_not_null() {
    let start = FixedOffset::west_opt(7 * 3600)
        .expect("offset")
        .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
        .single()
        .expect("start");
    let mut dwelling = cz4a_dwelling(start, 2 * 86_400, 900, 7);
    dwelling.simulate().expect("the run completes");

    let mode = column(&dwelling, "ASHP Heater Mode (-)");
    let electric = column(&dwelling, "ASHP Heater Electric Power (kW)");
    let rtf = column(&dwelling, "ASHP Heater Runtime Fraction (-)");
    let off_rows = mode.iter().filter(|(code, _)| *code == MODE_OFF).count();
    assert!(off_rows > 0, "the run has Off steps");

    // The heat pump's electric power and runtime fraction are 0.0 in every
    // off step: the quantities are physically zero when the equipment is
    // inactive, never null.
    for (row, ((elec_value, elec_valid), (rtf_value, rtf_valid))) in
        electric.iter().zip(&rtf).enumerate()
    {
        if mode[row].0 == MODE_OFF {
            assert!(
                *elec_valid && *rtf_valid,
                "row {row}: the off step's power cells are valid"
            );
            assert_eq!(elec_value, &0.0, "row {row}: the off step's electric power");
            assert_eq!(
                rtf_value, &0.0,
                "row {row}: the off step's runtime fraction"
            );
        }
    }
}

/// A test equipment that declares the `Electric Power` channel
/// non-nullable (the ELECTRIC capability behind the catalogue's
/// declaration) and never writes it. The core-output contract (OR-5) is
/// the wiring-defect check: a declared channel with no value fails the
/// run naming the missing variable's channel.
struct NeverWritesElectric {
    descriptor: EquipmentDescriptor,
    telemetry: Telemetry,
    core_output: CoreOutput,
}

impl NeverWritesElectric {
    fn new(name: &str) -> Self {
        Self {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(0),
                name: name.to_string(),
                end_use: EndUse::OTHER,
                equipment_type: std::borrow::Cow::Borrowed("NeverWritesElectric"),
                zone: None,
                fuel: FuelType::Electric,
                stage: ExecutionStage::Independent,
                control_capabilities: ControlCapabilities::empty(),
                core_capabilities: CoreCapabilities::ELECTRIC,
                telemetry_fields: vec![],
                zone_type: None,
            },
            telemetry: Telemetry::new(),
            core_output: CoreOutput::default(),
        }
    }
}

impl hares_equipment::Equipment for NeverWritesElectric {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn resolved_parameters(&self) -> Vec<hares_types::ResolvedParameter> {
        Vec::new()
    }

    fn rename(&mut self, name: String) {
        self.descriptor.name = name;
    }

    fn set_equipment_id(&mut self, id: EquipmentId) -> Result<(), HaresError> {
        hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
    }

    fn ports(&self) -> &[hares_types::PortDeclaration] {
        &[]
    }

    fn init(&mut self, _: &EquipmentConfig, _: &EnvironmentState) -> Result<(), HaresError> {
        Ok(())
    }

    fn update_control(&mut self, _: &EnvironmentState) -> OperatingMode {
        OperatingMode::Off
    }

    fn step(
        &mut self,
        _: &EnvironmentState,
        _: StdDuration,
        _: &mut PortSlots,
    ) -> Result<(), HaresError> {
        // Declares the Electric Power channel and never writes it.
        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core_output
    }

    fn save_state(&self) -> Result<Vec<u8>, HaresError> {
        Ok(vec![])
    }

    fn load_state(&mut self, _: &[u8]) -> Result<(), HaresError> {
        Ok(())
    }

    fn apply_signal(&mut self, _: &hares_types::ControlSignal) -> Result<(), HaresError> {
        Ok(())
    }
}

use hares_equipment::EquipmentConfig;

#[test]
fn missing_non_nullable_value_fails_the_run() {
    // The no-HVAC TOML dwelling (the same shape the failed-step tests
    // build), with the probe added.
    let dir = tempfile::tempdir().expect("temp dir");
    let toml_path = dir.path().join("missing_non_nullable.toml");
    let csv_path = dir.path().join("missing_non_nullable.csv");
    std::fs::write(
        &toml_path,
        format!(
            r#"building_id = 9001

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res = 60
duration = 600
output_chunk_size = 1
master_seed = 0

[[simulation.reports]]
name = "frame"
selection = {{ verbosity = 2 }}
destination = {{ csv = {{ path = "{}" }} }}
[geometry]
number_of_bedrooms = 3.0
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "none"

[weather]
outdoor_temp_c = 20.0
dew_point_c = 10.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[infiltration]
ach = 0.0

[schedule]
occupancy = 0.0
"#,
            csv_path.display()
        ),
    )
    .expect("write TOML");
    let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build dwelling");
    dwelling
        .add_equipment(Box::new(NeverWritesElectric::new("Never Writes Electric")))
        .expect("the probe registers");
    let err = dwelling
        .step()
        .expect_err("a declared channel with no value fails the run");
    let message = err.to_string();
    assert!(
        message.contains("Never Writes Electric") && message.contains("electric"),
        "the wiring-defect error names the equipment and the missing \
         variable's channel: {message}"
    );
}

/// A test actor declaring telemetry key `x` and emitting it only on even
/// steps.
struct EvenStepEmitter {
    name: String,
    telemetry: Telemetry,
    step: u64,
}

/// The actor's declared field table (the telemetry was built and sealed
/// from it) and the even-step value.
fn even_emitter_fields() -> &'static [TelemetryField] {
    static FIELDS: std::sync::OnceLock<[TelemetryField; 1]> = std::sync::OnceLock::new();
    FIELDS.get_or_init(|| {
        [TelemetryField {
            name: std::borrow::Cow::Borrowed("x"),
            unit: Unit::Dimensionless,
            kind: hares_types::VariableKind::Quantity {
                across_homes: hares_types::AcrossHomes::Mean,
            },
            aggregation: hares_types::Aggregation::Mean,
            sampling: hares_types::Sampling::InstantAtEnd,
            nullable: true,
            description: std::borrow::Cow::Borrowed("the even-step probe key"),
        }]
    })
}

impl Actor for EvenStepEmitter {
    fn name(&self) -> &str {
        &self.name
    }

    fn decide(&mut self, _: &EnvironmentState, _out: &mut Vec<DispatchRequest>) {
        if self.step.is_multiple_of(2) {
            self.telemetry.set("x", self.step as f64 + 1.0);
        } else {
            // The key is not emitted this step: its value is withdrawn.
            self.telemetry.remove("x");
        }
        self.step += 1;
    }

    fn output_variables(&self) -> &[TelemetryField] {
        even_emitter_fields()
    }

    fn telemetry(&self) -> Option<&Telemetry> {
        Some(&self.telemetry)
    }
}

#[test]
fn absent_actor_key_is_null() {
    // The no-HVAC TOML dwelling with the even-step emitter registered: the
    // actor:x column exists (the actor declares the field) and reads null
    // on every odd step.
    let dir = tempfile::tempdir().expect("temp dir");
    let toml_path = dir.path().join("absent_actor_key.toml");
    std::fs::write(
        &toml_path,
        r#"building_id = 9002

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res = 60
duration = 600
output_chunk_size = 1
master_seed = 0

[[simulation.reports]]
name = "frame"
selection = { verbosity = 0 }
destination = "memory"
[geometry]
number_of_bedrooms = 3.0
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "none"

[weather]
outdoor_temp_c = 20.0
dew_point_c = 10.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[infiltration]
ach = 0.0

[schedule]
occupancy = 0.0
"#,
    )
    .expect("write TOML");
    let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build dwelling");
    dwelling
        .add_actor(Box::new(EvenStepEmitter {
            name: "even_emitter".to_string(),
            telemetry: Telemetry::from_fields(even_emitter_fields()),
            step: 0,
        }))
        .expect("the actor registers");
    for _ in 0..4 {
        dwelling.step().expect("the dwelling steps");
    }
    let cells = column(&dwelling, "actor:even_emitter:x");
    assert_eq!(cells.len(), 4, "four steps recorded");
    for (row, (value, valid)) in cells.iter().enumerate() {
        if row % 2 == 0 {
            assert!(valid, "row {row}: the emitted key is valid");
            assert_eq!(value, &(row as f64 + 1.0), "row {row}: the emitted value");
        } else {
            assert!(!valid, "row {row}: the unemitted key is null, not 0.0");
        }
    }
}

/// A dwelling lacking an equipment class whose column exists: the cell is
/// null. The dwelling has no HVAC equipment, so no producer claims the
/// setpoint-chain columns (bound to the first HVAC equipment) or the duct
/// losses aggregate (no HVAC contributor); the unmet-load column has no
/// producer in any dwelling. Every one of them reads null in every step,
/// never the row's zero.
#[test]
fn lacking_equipment_class_columns_are_null() {
    let dir = tempfile::tempdir().expect("temp dir");
    let toml_path = dir.path().join("lacking_class.toml");
    std::fs::write(
        &toml_path,
        r#"building_id = 9003

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res = 900
duration = 14400
output_chunk_size = 16
master_seed = 0

[[simulation.reports]]
name = "frame"
selection = { verbosity = 7 }
destination = "memory"

[geometry]
number_of_bedrooms = 3.0
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "none"

[weather]
outdoor_temp_c = 20.0
dew_point_c = 10.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[infiltration]
ach = 0.0

[schedule]
occupancy = 0.0
"#,
    )
    .expect("write TOML");
    let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build dwelling");
    dwelling.simulate().expect("the run completes");

    for name in [
        "Scheduled Heating Setpoint (C)",
        "Scheduled Cooling Setpoint (C)",
        "HVAC Duct Losses (W)",
        "Unmet HVAC Load (C)",
    ] {
        let cells = column(&dwelling, name);
        assert!(
            !cells.is_empty(),
            "'{name}' exists in the schema of a dwelling with no HVAC"
        );
        for (row, (_, valid)) in cells.iter().enumerate() {
            assert!(
                !valid,
                "'{name}' row {row}: the dwelling lacks the equipment class, \
                 the cell is null, not the row's zero"
            );
        }
    }
}
