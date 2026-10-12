// Row 38 (OR-3): destinations replace the output booleans (HARES#28).
//
// These are the RED tests: they are written against the report and
// destination surface before it exists. The configuration rejects the
// `reports` key (deny_unknown_fields) until the row's change lands, so
// every test here fails against the parent commit, and the file does not
// even compile while the types are missing.

use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;

use arrow::array::Float64Array;
use arrow::record_batch::RecordBatch;
use hares_core::{Dwelling, DwellingConfig, DwellingSettings};
use hares_equipment::{Equipment, EquipmentConfig};
use hares_io::{DefaultsSource, InputSource, WeatherInput};
use hares_types::{
    ControlCapabilities, CoreCapabilities, CoreOutput, EndUse, EnvironmentState,
    EquipmentDescriptor, EquipmentId, ExecutionStage, FuelType, HaresError, OperatingMode,
    PortSlots, Telemetry, TelemetryField,
};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sim_toml(reports: &str) -> String {
    format!(
        r#"
start_time = "2023-01-01T00:00:00-07:00"
duration = 21600
time_res = 900
{reports}
"#
    )
}

fn memory_report(name: &str, verbosity: u8) -> String {
    format!(
        "\n[[reports]]\nname = \"{name}\"\nselection = {{ verbosity = {verbosity} }}\ndestination = \"memory\"\n"
    )
}

fn file_report(name: &str, verbosity: u8, format: &str, path: &Path, overwrite: bool) -> String {
    format!(
        "\n[[reports]]\nname = \"{name}\"\nselection = {{ verbosity = {verbosity} }}\n\
         destination = {{ {format} = {{ path = \"{}\", overwrite = {overwrite} }} }}\n",
        path.display()
    )
}

fn dwelling_config(sim_toml: &str) -> DwellingConfig {
    let home = repo_root().join("tests/fixtures/parity/cz4a_ashp_hpwh");
    DwellingConfig {
        hpxml: InputSource::Path(home.join("building.xml")),
        schedule: Some(InputSource::Path(home.join("schedule.csv"))),
        weather: WeatherInput::Path(home.join("weather.epw")),
        defaults: DefaultsSource::directory(repo_root().join("defaults")),
        settings: DwellingSettings::new(
            hares_io::SimulationConfig::from_toml(sim_toml).expect("sim config parses"),
            None,
            1,
            None,
            None,
        ),
    }
}

fn dir_entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("directory read")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// `memory_only_produces_full_schema_and_no_file`: a run with one Memory
/// report returns every selected column, and no file backend was
/// constructed: the run's only plausible output root (the fresh temporary
/// directory it is pointed at) is empty afterwards.
#[test]
fn memory_only_produces_full_schema_and_no_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // The process cwd is the only output root a fileless default could
    // reach (the deleted default_output_path wrote there); nextest runs
    // this test in its own process.
    let cwd = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(tmp.path()).expect("cwd is the only output root");

    let toml = sim_toml(&memory_report("frame", 2));
    let mut dwelling = Dwelling::from_config(dwelling_config(&toml)).expect("dwelling builds");
    let result = dwelling.simulate();
    std::env::set_current_dir(cwd).expect("cwd restored");
    result.expect("run completes");

    let batches = dwelling.flushed_batches().to_vec();
    assert!(
        !batches.is_empty(),
        "the memory report retains the run's batches"
    );
    let schema = batches[0].schema();
    assert!(
        schema.fields().len() > 3,
        "the memory report carries the full selected schema, got {} fields",
        schema.fields().len()
    );
    assert_eq!(
        dir_entries(tmp.path()),
        Vec::<String>::new(),
        "a memory-only run creates no file"
    );
}

/// `existing_target_without_overwrite_fails_before_first_step`.
#[test]
fn existing_target_without_overwrite_fails_before_first_step() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let target = tmp.path().join("out.parquet");
    std::fs::write(&target, b"existing").expect("target written");
    let toml = sim_toml(&file_report("frame", 2, "parquet", &target, false));
    let err = Dwelling::from_config(dwelling_config(&toml))
        .err()
        .expect("an existing target without overwrite fails at freeze, before the first step");
    let message = err.to_string();
    assert!(
        message.contains("out.parquet"),
        "the error names the path, got: {message}"
    );
}

/// `overwrite_true_replaces_target`.
#[test]
fn overwrite_true_replaces_target() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let target = tmp.path().join("out.parquet");
    std::fs::write(&target, b"existing").expect("target written");
    let toml = sim_toml(&file_report("frame", 2, "parquet", &target, true));
    let mut dwelling = Dwelling::from_config(dwelling_config(&toml)).expect("dwelling builds");
    dwelling.simulate().expect("run completes");
    assert!(target.exists(), "overwrite: true publishes the target");
    let bytes = std::fs::read(&target).expect("target read");
    assert_ne!(
        &bytes[..3],
        b"exi",
        "the existing file was replaced by the run's output"
    );
}

/// `missing_parent_dir_is_io_error_naming_path`.
#[test]
fn missing_parent_dir_is_io_error_naming_path() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let target = tmp.path().join("missing_dir").join("out.parquet");
    let toml = sim_toml(&file_report("frame", 2, "parquet", &target, false));
    let err = Dwelling::from_config(dwelling_config(&toml))
        .err()
        .expect("a missing parent directory fails at freeze, before the first step");
    let message = err.to_string();
    assert!(
        message.contains("out.parquet"),
        "the error names the path, got: {message}"
    );
}

/// A minimal in-test equipment whose `step()` always fails: the run-level
/// failure a destination must clean up after (the FailingPortEquipment
/// pattern).
struct FailingStepEquipment {
    descriptor: EquipmentDescriptor,
    telemetry: Telemetry,
    core_output: CoreOutput,
}

impl FailingStepEquipment {
    fn new(name: &str, id: EquipmentId) -> Self {
        Self {
            descriptor: EquipmentDescriptor {
                id,
                name: name.to_string(),
                end_use: EndUse::OTHER,
                equipment_type: std::borrow::Cow::Borrowed("FailingStepEquipment"),
                zone: None,
                fuel: FuelType::Electric,
                stage: ExecutionStage::Independent,
                control_capabilities: ControlCapabilities::empty(),
                core_capabilities: CoreCapabilities::empty(),
                telemetry_fields: vec![TelemetryField::ratio("probe", "failing step probe")],
                zone_type: None,
            },
            telemetry: Telemetry::with_capacity(1),
            core_output: CoreOutput::default(),
        }
    }
}

impl Equipment for FailingStepEquipment {
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
        self.telemetry.insert("probe", 0.0);
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
        Err(HaresError::Simulation("injected step failure".to_string()))
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

/// `failed_run_leaves_no_partial_file`: a run made to fail at a configured
/// step by an equipment that returns `Err` from `step()` removes its
/// partial files. The failure budget is the default: the first failure is
/// tolerated (the row is still recorded, the partial file is created at
/// its flush), the second ends the run.
#[test]
fn failed_run_leaves_no_partial_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let target = tmp.path().join("out.parquet");
    let toml = sim_toml(&format!(
        "output_chunk_size = 1\n{}",
        file_report("frame", 2, "parquet", &target, false)
    ));
    let mut dwelling = Dwelling::from_config(dwelling_config(&toml)).expect("dwelling builds");
    dwelling
        .add_equipment(Box::new(FailingStepEquipment::new(
            "Failing Probe",
            EquipmentId(0),
        )))
        .expect("the failing equipment joins");
    let result = dwelling.simulate();
    assert!(result.is_err(), "the injected failure ends the run");
    assert_eq!(
        dir_entries(tmp.path()),
        Vec::<String>::new(),
        "a failed run leaves no partial file"
    );
}

/// `schema_equal_across_destinations`: one run with `Memory`, `Parquet` and
/// `Csv` reports of the same selection; the Parquet file read back and the
/// CSV read back with the catalogue's Arrow types have the same column
/// names, order and types as the Memory table, and equal values (Parquet
/// on bits; CSV to its declared text precision).
#[test]
fn schema_equal_across_destinations() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let parquet = tmp.path().join("frame.parquet");
    let csv = tmp.path().join("frame.csv");
    let reports = format!(
        "{}{}{}",
        memory_report("mem", 2),
        file_report("pq", 2, "parquet", &parquet, false),
        file_report("tx", 2, "csv", &csv, false),
    );
    let toml = sim_toml(&reports);
    let mut dwelling = Dwelling::from_config(dwelling_config(&toml)).expect("dwelling builds");
    dwelling.simulate().expect("run completes");

    let memory = dwelling.flushed_batches().to_vec();
    assert!(!memory.is_empty(), "the memory report retains batches");
    let memory_schema = memory[0].schema();

    let parquet_file = std::fs::File::open(&parquet).expect("parquet file exists");
    let reader =
        parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(parquet_file)
            .expect("parquet opens")
            .build()
            .expect("parquet reader");
    let parquet_batches: Vec<_> = reader.filter_map(Result::ok).collect();
    assert!(!parquet_batches.is_empty());
    let parquet_schema = parquet_batches[0].schema();

    let memory_names: Vec<&str> = memory_schema
        .fields()
        .iter()
        .map(|f| f.name().as_str())
        .collect();
    let parquet_names: Vec<&str> = parquet_schema
        .fields()
        .iter()
        .map(|f| f.name().as_str())
        .collect();
    assert_eq!(memory_names, parquet_names, "column names and order match");
    for (m, p) in memory_schema.fields().iter().zip(parquet_schema.fields()) {
        assert_eq!(m.data_type(), p.data_type(), "types match for {}", m.name());
        assert_eq!(
            m.is_nullable(),
            p.is_nullable(),
            "nullability matches for {}",
            m.name()
        );
    }

    // Parquet values equal the memory values on bits.
    let column = "Total Electric Power (kW)";
    let memory_values = f64_column(&memory, column);
    let parquet_values = f64_column(&parquet_batches, column);
    assert_eq!(memory_values.len(), parquet_values.len());
    for (a, b) in memory_values.iter().zip(parquet_values.iter()) {
        assert_eq!(a.to_bits(), b.to_bits(), "parquet frame bitwise equal");
    }

    // CSV: same columns, values to its declared text precision.
    let csv_text = std::fs::read_to_string(&csv).expect("csv exists");
    let mut lines = csv_text.lines();
    let header = lines.next().expect("csv header");
    let csv_names: Vec<&str> = header.split(',').map(str::trim).collect();
    assert_eq!(
        csv_names, memory_names,
        "csv columns match the memory schema"
    );
    assert!(lines.next().is_some(), "csv carries the run's rows");
}

/// Two differently-shaped reports in one run: each destination's contents
/// are that report's own selection's resolution, never its neighbour's. A
/// run where report i's recorder receives report j's projection fails
/// here: the recorders' column-width check refuses the misprojected rows,
/// and the per-destination schema comparison refuses a same-width swap.
#[test]
fn multi_report_each_destination_carries_its_own_selection() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let csv = tmp.path().join("full.csv");
    let toml = sim_toml(&format!(
        "\n[[reports]]\nname = \"probe\"\nselection = {{ named = [\"Total Electric Power (kW)\", \"Outdoor Dry Bulb (C)\"] }}\ndestination = \"memory\"\n{}",
        file_report("full", 2, "csv", &csv, false),
    ));
    let mut dwelling = Dwelling::from_config(dwelling_config(&toml)).expect("dwelling builds");
    dwelling.simulate().expect("run completes");

    // Each selection resolves against the dwelling's own catalogue.
    let catalogue = dwelling.catalogue();
    let probe_selection = hares_io::Selection::Named(vec![
        "Total Electric Power (kW)".to_string(),
        "Outdoor Dry Bulb (C)".to_string(),
    ]);
    let full_selection = hares_io::Selection::Verbosity(2);
    let probe_names = hares_io::resolve_selection(&probe_selection, &catalogue)
        .expect("the probe selection resolves");
    let full_names = hares_io::resolve_selection(&full_selection, &catalogue)
        .expect("the full selection resolves");
    assert_ne!(
        probe_names.len(),
        full_names.len(),
        "the two reports are differently shaped"
    );

    // The memory destination's contents are the probe's own selection: the
    // timestamp plus exactly the resolved columns, in order.
    let memory = dwelling.flushed_batches().to_vec();
    assert!(!memory.is_empty(), "the memory report retains batches");
    let memory_names: Vec<String> = memory[0]
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    let expected_probe: Vec<String> = std::iter::once("Time".to_string())
        .chain(probe_names.iter().cloned())
        .collect();
    assert_eq!(
        memory_names, expected_probe,
        "the memory report carries its own selection, not the other report's"
    );

    // The csv destination's contents are the full report's own selection.
    let csv_text = std::fs::read_to_string(&csv).expect("csv exists");
    let mut lines = csv_text.lines();
    let header = lines.next().expect("csv header");
    let csv_names: Vec<String> = header.split(',').map(str::trim).map(String::from).collect();
    let expected_full: Vec<String> = std::iter::once("Time".to_string())
        .chain(full_names.iter().cloned())
        .collect();
    assert_eq!(
        csv_names, expected_full,
        "the csv report carries its own selection, not the other report's"
    );

    // And the columns the selections share are projections of the same
    // union row: the destinations agree value for value.
    for column in ["Total Electric Power (kW)", "Outdoor Dry Bulb (C)"] {
        let memory_values = f64_column(&memory, column);
        let csv_idx = csv_names
            .iter()
            .position(|name| name == column)
            .unwrap_or_else(|| panic!("csv carries {column}"));
        let csv_values: Vec<f64> = lines
            .clone()
            .filter_map(|line| line.split(',').nth(csv_idx))
            .map(|cell| {
                cell.trim()
                    .parse::<f64>()
                    .unwrap_or_else(|e| panic!("{column}: {cell:?} does not parse: {e}"))
            })
            .collect();
        assert_eq!(
            memory_values.len(),
            csv_values.len(),
            "{column}: both destinations carry every step"
        );
        for (a, b) in memory_values.iter().zip(csv_values.iter()) {
            assert_eq!(
                a.to_bits(),
                b.to_bits(),
                "{column}: both destinations project the same union row"
            );
        }
    }
}

/// A file-only run whose only report is a narrow selection without the
/// power column builds: the metrics are computed only when requested, and
/// this run does not request them. The refusal on the request path is
/// pinned in `metrics_optional.rs`.
#[test]
fn narrow_file_only_run_without_the_metrics_request_builds() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let target = tmp.path().join("lean.csv");
    let toml = sim_toml(&format!(
        "\n[[reports]]\nname = \"lean\"\nselection = {{ named = [\"Outdoor Dry Bulb (C)\"] }}\ndestination = {{ csv = {{ path = \"{}\", overwrite = false }} }}\n",
        target.display()
    ));
    let mut dwelling = Dwelling::from_config(dwelling_config(&toml))
        .expect("a narrow file-only run without the metrics request builds");
    dwelling.simulate().expect("the run completes");
    assert!(target.exists(), "the narrow report published its file");
}

fn f64_column(batches: &[RecordBatch], name: &str) -> Vec<f64> {
    let mut values = Vec::new();
    for batch in batches {
        let idx = batch
            .schema()
            .fields()
            .iter()
            .position(|f| f.name() == name)
            .unwrap_or_else(|| panic!("column {name} present"));
        let array = batch
            .column(idx)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap_or_else(|| panic!("column {name} is float64"));
        values.extend(array.iter().flatten());
    }
    values
}
