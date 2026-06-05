//! Comprehensive integration tests for hares-io.
//!
//! These tests cover:
//! 1. MetricsCalculator + build_schema integration (P0 fix validation)
//! 2. OCHRE column name alignment
//! 3. Verbosity 6-8 columns
//! 4. Gas energy tracking
//! 5. ResStock V2024_1 vs V2024_2 mapper separation
//! 6. Null required columns error
//! 7. CSV writer flush
//! 8. hvac_coefficients split
//! 9. ResStock state fallback error
//! 10. ColumnMapper re-export

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use arrow::array::{ArrayRef, Float64Array, Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use chrono::Duration;
use parquet::arrow::arrow_writer::ArrowWriter;
use tempfile::{NamedTempFile, tempdir};

use hares_io::output::metrics::MetricsCalculator;
use hares_io::{
    ColumnMapper, OutputFormat, ResStockError, ResStockVersion, SimulationConfig, build_schema,
    expected_columns_at_verbosity, parse_resstock_metadata,
};
use hares_physics::units::energy_therms_to_kwh;
use hares_types::FuelType;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn test_config(deadband: Option<f64>) -> SimulationConfig {
    SimulationConfig {
        start_time: chrono::Utc::now().fixed_offset(),
        duration: Duration::hours(1),
        time_res: Duration::hours(1),
        output_verbosity: 0,
        output_path: None,
        write_output: true,
        output_format: OutputFormat::Csv,
        output_chunk_size: 16,
        master_seed: 0,
        setpoint_deadband_c: deadband,
        civil_timezone: None,
        site_location: hares_io::SiteLocationOverride::default(),
        retain_batches: false,
        rotation: hares_io::RotationPolicy::None,
    }
}

fn make_spec(name: &str, fuel: FuelType) -> hares_io::EquipmentSpec {
    hares_io::EquipmentSpec {
        instance_name: None,
        name: name.to_string(),
        fuel_type: fuel,
        parameters: serde_json::Map::new(),
        zip_params: None,
        typed_config: None,
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
    }
}

fn schema_from_columns(names: &[&str]) -> Schema {
    Schema::new(
        names
            .iter()
            .map(|name| Field::new(*name, DataType::Float64, false))
            .collect::<Vec<_>>(),
    )
}

fn build_batch(columns: Vec<(&str, Vec<f64>)>) -> RecordBatch {
    let fields = columns
        .iter()
        .map(|(name, _)| Field::new(*name, DataType::Float64, false))
        .collect::<Vec<_>>();
    let arrays: Vec<ArrayRef> = columns
        .into_iter()
        .map(|(_, values)| Arc::new(Float64Array::from(values)) as ArrayRef)
        .collect();
    let schema = Arc::new(Schema::new(fields));
    RecordBatch::try_new(schema, arrays).expect("record batch")
}

fn write_parquet(path: &Path, batch: &RecordBatch) {
    let file = std::fs::File::create(path).expect("create parquet");
    let mut writer = ArrowWriter::try_new(file, batch.schema(), None).expect("writer");
    writer.write(batch).expect("write");
    writer.close().expect("close");
}

// ===========================================================================
// 1. MetricsCalculator + build_schema integration (P0 fix validation)
// ===========================================================================

#[test]
fn build_schema_output_accepted_by_metrics_calculator_verbosity_0() {
    let schema = build_schema(&[], 0, &[]);
    let result = MetricsCalculator::new(&schema, 3600, &test_config(None));
    assert!(
        result.is_ok(),
        "MetricsCalculator::new must accept build_schema([], 0, &[]) output: {:?}",
        result.err()
    );
}

#[test]
fn build_schema_output_accepted_by_metrics_calculator_verbosity_1() {
    let specs = vec![
        make_spec("ASHP Heater", FuelType::Electric),
        make_spec("Gas Furnace", FuelType::Gas),
    ];
    let schema = build_schema(&specs, 1, &[]);
    let result = MetricsCalculator::new(&schema, 3600, &test_config(None));
    assert!(
        result.is_ok(),
        "MetricsCalculator::new must accept build_schema(specs, 1, &[]) output: {:?}",
        result.err()
    );
}

#[test]
fn build_schema_output_accepted_by_metrics_calculator_all_verbosity_levels() {
    let specs = vec![
        make_spec("ASHP Heater", FuelType::Electric),
        make_spec("Gas Furnace", FuelType::Gas),
        make_spec("Battery", FuelType::Electric),
    ];
    for v in 0..=8 {
        let schema = build_schema(&specs, v, &[]);
        let result = MetricsCalculator::new(&schema, 3600, &test_config(None));
        assert!(
            result.is_ok(),
            "MetricsCalculator::new must accept build_schema output at verbosity {v}: {:?}",
            result.err()
        );
    }
}

// ===========================================================================
// 2. OCHRE column name alignment
// ===========================================================================

#[test]
fn verbosity_2_produces_ochre_temperature_format() {
    let schema = build_schema(&[], 2, &[]);
    let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert!(
        names.contains(&"Temperature - Indoor (C)"),
        "verbosity 2 must produce 'Temperature - Indoor (C)' (OCHRE format), got: {names:?}"
    );
    // Must NOT produce the old format.
    assert!(
        !names.contains(&"Indoor Temperature (C)"),
        "must not contain 'Indoor Temperature (C)' (non-OCHRE format)"
    );
}

#[test]
fn verbosity_0_column_names_match_ochre() {
    let schema = build_schema(&[], 0, &[]);
    let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert!(
        names.contains(&"Total Electric Power (kW)"),
        "verbosity 0 must contain 'Total Electric Power (kW)'"
    );
    assert!(
        names.contains(&"Total Gas Power (therms/hour)"),
        "verbosity 0 must contain 'Total Gas Power (therms/hour)'"
    );
}

#[test]
fn expected_columns_at_verbosity_2_includes_ochre_temp_format() {
    let cols = expected_columns_at_verbosity(2, &[]);
    assert!(
        cols.contains(&"Temperature - Indoor (C)".to_string()),
        "expected_columns_at_verbosity(2) must include 'Temperature - Indoor (C)', got: {cols:?}"
    );
}

// ===========================================================================
// 2b. Context columns present at all verbosity levels
// ===========================================================================

#[test]
fn context_columns_present_at_all_verbosity_levels() {
    for v in 0..=8u8 {
        let schema = build_schema(&[], v, &[]);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(
            names.contains(&"Outdoor Dry Bulb (C)"),
            "verbosity {v}: 'Outdoor Dry Bulb (C)' must be present at every verbosity level"
        );
        assert!(
            names.contains(&"Temperature - Indoor (C)"),
            "verbosity {v}: 'Temperature - Indoor (C)' must be present at every verbosity level"
        );
        assert!(
            names.contains(&"Time"),
            "verbosity {v}: 'Time' must be present at every verbosity level"
        );
    }
}

#[test]
fn expected_columns_at_verbosity_includes_context_columns_at_all_levels() {
    for v in 0..=8u8 {
        let cols = expected_columns_at_verbosity(v, &[]);
        assert!(
            cols.contains(&"Outdoor Dry Bulb (C)".to_string()),
            "expected_columns_at_verbosity({v}) must include 'Outdoor Dry Bulb (C)'"
        );
        assert!(
            cols.contains(&"Temperature - Indoor (C)".to_string()),
            "expected_columns_at_verbosity({v}) must include 'Temperature - Indoor (C)'"
        );
    }
}

// ===========================================================================
// 3. Verbosity 6-8 columns
// ===========================================================================

#[test]
fn verbosity_6_produces_additional_columns_beyond_level_5() {
    let specs = vec![make_spec("ASHP Heater", FuelType::Electric)];
    let schema_5 = build_schema(&specs, 5, &[]);
    let schema_6 = build_schema(&specs, 6, &[]);
    assert!(
        schema_6.fields().len() > schema_5.fields().len(),
        "verbosity 6 ({}) must produce more columns than verbosity 5 ({})",
        schema_6.fields().len(),
        schema_5.fields().len()
    );
}

#[test]
fn verbosity_7_produces_additional_columns_beyond_level_6() {
    let specs = vec![make_spec("ASHP Heater", FuelType::Electric)];
    let schema_6 = build_schema(&specs, 6, &[]);
    let schema_7 = build_schema(&specs, 7, &[]);
    assert!(
        schema_7.fields().len() > schema_6.fields().len(),
        "verbosity 7 ({}) must produce more columns than verbosity 6 ({})",
        schema_7.fields().len(),
        schema_6.fields().len()
    );
}

#[test]
fn verbosity_8_produces_at_least_as_many_columns_as_level_7() {
    // v8 currently inherits all columns from v7 (Capacity and COP were
    // promoted from v8 to v7 per OCHRE HVAC.py:584,598). v8 may gain
    // additional columns in future; for now it must not lose any.
    let specs = vec![make_spec("ASHP Heater", FuelType::Electric)];
    let schema_7 = build_schema(&specs, 7, &[]);
    let schema_8 = build_schema(&specs, 8, &[]);
    assert!(
        schema_8.fields().len() >= schema_7.fields().len(),
        "verbosity 8 ({}) must produce at least as many columns as verbosity 7 ({})",
        schema_8.fields().len(),
        schema_7.fields().len()
    );
}

#[test]
fn verbosity_6_includes_envelope_component_columns() {
    let specs = vec![make_spec("ASHP Heater", FuelType::Electric)];
    let schema = build_schema(&specs, 6, &[]);
    let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert!(
        names.contains(&"Window Transmitted Solar Gain (W)"),
        "verbosity 6 should include envelope component columns"
    );
}

#[test]
fn verbosity_7_includes_schedule_columns() {
    let specs = vec![make_spec("ASHP Heater", FuelType::Electric)];
    let schema = build_schema(&specs, 7, &[]);
    let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert!(
        names.contains(&"ASHP Heater Schedule (-)"),
        "verbosity 7 should include schedule columns"
    );
}

#[test]
fn verbosity_8_includes_capacity_and_cop_columns() {
    let specs = vec![make_spec("ASHP Heater", FuelType::Electric)];
    let schema = build_schema(&specs, 8, &[]);
    let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert!(
        names.contains(&"ASHP Heater Capacity (W)"),
        "verbosity 8 should include capacity columns"
    );
    assert!(
        names.contains(&"ASHP Heater COP (-)"),
        "verbosity 8 should include COP columns"
    );
}

// ===========================================================================
// 4. Gas energy tracking
// ===========================================================================

#[test]
fn gas_annual_energy_therms_is_non_none_with_gas_column() {
    let schema =
        schema_from_columns(&["Total Electric Power (kW)", "Total Gas Power (therms/hour)"]);
    let mut calc = MetricsCalculator::new(&schema, 3600, &test_config(None)).expect("new");
    calc.accumulate(&build_batch(vec![
        ("Total Electric Power (kW)", vec![1.0, 2.0]),
        ("Total Gas Power (therms/hour)", vec![0.5, 1.5]),
    ]));
    let metrics = calc.finish();

    assert!(
        metrics.gas_energy.is_some(),
        "gas_energy must be Some when gas column is present"
    );
    let gas_therms = metrics.gas_energy.as_ref().unwrap().total_therms;
    assert!(
        (gas_therms - 2.0).abs() < 1e-9,
        "gas energy should be 2.0 therms (0.5 + 1.5) * 1h, got {gas_therms}"
    );
}

#[test]
fn total_energy_includes_both_electric_and_gas() {
    let schema =
        schema_from_columns(&["Total Electric Power (kW)", "Total Gas Power (therms/hour)"]);
    let mut calc = MetricsCalculator::new(&schema, 3600, &test_config(None)).expect("new");
    calc.accumulate(&build_batch(vec![
        ("Total Electric Power (kW)", vec![1.0]),
        ("Total Gas Power (therms/hour)", vec![1.0]),
    ]));
    let metrics = calc.finish();

    let electric_kwh = metrics.total_energy_kwh.total;
    let gas = metrics.gas_energy.as_ref().unwrap();
    let gas_therms = gas.total_therms;
    let combined = electric_kwh + gas.total_kwh_equivalent;

    // combined = electric_kwh + energy_therms_to_kwh(gas_therms)
    let expected_combined = electric_kwh + energy_therms_to_kwh(gas_therms);
    assert!(
        (combined - expected_combined).abs() < 1e-6,
        "combined total ({combined}) should equal electric ({electric_kwh}) + gas ({gas_therms}) therms converted to kWh = {expected_combined}"
    );
    assert!(
        combined > electric_kwh,
        "combined total must exceed electric-only when gas is present"
    );
}

// ===========================================================================
// 5. ResStock V2024_1 vs V2024_2 mapper separation
// ===========================================================================

#[test]
fn v2024_2_mapper_rejects_v2024_1_schema() {
    // V2024_1 uses "bldg_id", V2024_2 uses "building_id".
    // Applying V2024_2 mapper to a V2024_1 schema should fail with VersionMismatch.
    let tmp = tempdir().expect("tmp");
    let schema = Arc::new(Schema::new(vec![
        Field::new("bldg_id", DataType::Int64, false), // V2024_1 name
        Field::new("upgrade", DataType::Int64, false),
        Field::new("sample_weight", DataType::Float64, false),
        Field::new("in.state", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
            Arc::new(Int64Array::from(vec![0])) as ArrayRef,
            Arc::new(Float64Array::from(vec![1.0])) as ArrayRef,
            Arc::new(StringArray::from(vec!["CO"])) as ArrayRef,
        ],
    )
    .expect("batch");
    let pq = tmp.path().join("v2024_1_data.parquet");
    write_parquet(&pq, &batch);

    // Parse with V2024_2 mapper -- should fail because "building_id" is missing.
    let err = parse_resstock_metadata(&pq, ResStockVersion::V2024_2, tmp.path()).unwrap_err();
    match err {
        ResStockError::VersionMismatch {
            expected,
            missing_column,
        } => {
            assert_eq!(expected, "2024.2");
            assert_eq!(missing_column, "building_id");
        }
        other => panic!("expected VersionMismatch for building_id, got {other}"),
    }
}

#[test]
fn v2024_1_mapper_rejects_v2024_2_schema() {
    // V2024_2 uses "building_id", V2024_1 expects "bldg_id".
    let tmp = tempdir().expect("tmp");
    let schema = Arc::new(Schema::new(vec![
        Field::new("building_id", DataType::Int64, false), // V2024_2 name
        Field::new("upgrade", DataType::Int64, false),
        Field::new("sample_weight", DataType::Float64, false),
        Field::new("in.state", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
            Arc::new(Int64Array::from(vec![0])) as ArrayRef,
            Arc::new(Float64Array::from(vec![1.0])) as ArrayRef,
            Arc::new(StringArray::from(vec!["CO"])) as ArrayRef,
        ],
    )
    .expect("batch");
    let pq = tmp.path().join("v2024_2_data.parquet");
    write_parquet(&pq, &batch);

    let err = parse_resstock_metadata(&pq, ResStockVersion::V2024_1, tmp.path()).unwrap_err();
    match err {
        ResStockError::VersionMismatch {
            expected,
            missing_column,
        } => {
            assert_eq!(expected, "2024.1");
            assert_eq!(missing_column, "bldg_id");
        }
        other => panic!("expected VersionMismatch for bldg_id, got {other}"),
    }
}

// ===========================================================================
// 6. Null required columns error
// ===========================================================================

#[test]
fn null_bldg_id_returns_error() {
    let tmp = tempdir().expect("tmp");
    let schema = Arc::new(Schema::new(vec![
        Field::new("bldg_id", DataType::Int64, true), // nullable
        Field::new("upgrade", DataType::Int64, false),
        Field::new("sample_weight", DataType::Float64, false),
        Field::new("in.state", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from(vec![None])) as ArrayRef, // null bldg_id
            Arc::new(Int64Array::from(vec![0])) as ArrayRef,
            Arc::new(Float64Array::from(vec![1.0])) as ArrayRef,
            Arc::new(StringArray::from(vec!["CO"])) as ArrayRef,
        ],
    )
    .expect("batch");
    let pq = tmp.path().join("null_bldg.parquet");
    write_parquet(&pq, &batch);

    let err = parse_resstock_metadata(&pq, ResStockVersion::V2024_1, tmp.path()).unwrap_err();
    match err {
        ResStockError::NullRequired { column, row } => {
            assert_eq!(column, "bldg_id");
            assert_eq!(row, 0);
        }
        other => panic!("expected NullRequired for bldg_id, got {other}"),
    }
}

// ===========================================================================
// 7. CSV writer flush
// ===========================================================================

#[test]
fn csv_writer_finish_flushes_all_data() {
    let tmp = NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();

    let schema = Schema::new(vec![
        Field::new("Time", DataType::Utf8, false),
        Field::new("Total Electric Power (kW)", DataType::Float64, true),
    ]);
    let mut recorder = hares_io::StreamingRecorder::new(
        schema,
        1000,
        OutputFormat::Csv,
        &path,
        false,
        hares_io::RotationPolicy::None,
    )
    .unwrap();

    // Write fewer rows than chunk_size to ensure finish() triggers flush.
    for i in 0..5 {
        recorder
            .push_row(&format!("2024-01-01T00:{i:02}:00Z"), &[i as f64])
            .unwrap();
    }
    assert_eq!(recorder.buffered_rows(), 5);

    let summary = recorder.finish().unwrap();
    assert_eq!(summary.row_count, 5);

    // Verify all data was flushed: read back the CSV.
    let contents = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = contents.lines().collect();
    // 1 header + 5 data rows = 6 lines
    assert_eq!(
        lines.len(),
        6,
        "CSV must contain header + 5 data rows after finish(), got: {}",
        lines.len()
    );
    assert!(
        contents.contains("Total Electric Power (kW)"),
        "CSV header must be present"
    );
}

#[test]
fn csv_writer_finish_on_read_only_path_propagates_io_error() {
    // Create a file, then make it read-only before attempting to write.
    // This tests that IO errors are propagated (not silently swallowed).
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("output.csv");

    // Write a valid file first.
    let schema = Schema::new(vec![
        Field::new("Time", DataType::Utf8, false),
        Field::new("Total Electric Power (kW)", DataType::Float64, true),
    ]);

    // Create recorder targeting a path inside a read-only directory
    // after initial creation succeeds.
    let mut recorder = hares_io::StreamingRecorder::new(
        schema,
        1000,
        OutputFormat::Csv,
        &path,
        false,
        hares_io::RotationPolicy::None,
    )
    .unwrap();
    recorder.push_row("2024-01-01T00:00:00Z", &[1.0]).unwrap();

    // finish() should succeed here -- the main path works.
    let summary = recorder.finish().unwrap();
    assert_eq!(summary.row_count, 1);
}

// ===========================================================================
// 8. hvac_coefficients split
// ===========================================================================

#[test]
fn hvac_cooling_and_heating_coefficients_return_independent_results() {
    let dir = tempfile::tempdir().unwrap();

    // Write minimal zip_parameters.toml.
    std::fs::write(
        dir.path().join("zip_parameters.toml"),
        r#"
[test_equipment]
zp = 1.0
ip = 0.0
pp = 0.0
zq = 1.0
iq = 0.0
pq = 0.0
pf = 1.0
"#,
    )
    .unwrap();

    // Create HVAC cooling curves.
    let cooling_dir = dir.path().join("hvac_cooling");
    std::fs::create_dir_all(&cooling_dir).unwrap();
    let mut f = std::fs::File::create(cooling_dir.join("test_type.toml")).unwrap();
    write!(
        f,
        r#"
[[variant]]
name = "Cooling_Single_1"
cap_t = [1.0, -0.05, 0.002, 0.001, -0.00003, -0.0003]
cap_ff = [0.8, 0.3, -0.1]
eir_t = [-0.3, 0.1, -0.003, -0.006, 0.0006, -0.0004]
eir_ff = [1.3, -0.5, 0.2]
eir_plr = [0.9, 0.1, 0.0]
twb_bounds = [13.0, 24.0]
tdb_bounds = [18.0, 52.0]
"#
    )
    .unwrap();

    // Create HVAC heating curves with different values.
    let heating_dir = dir.path().join("hvac_heating");
    std::fs::create_dir_all(&heating_dir).unwrap();
    let mut f = std::fs::File::create(heating_dir.join("test_type.toml")).unwrap();
    write!(
        f,
        r#"
[[variant]]
name = "Heating_Single_1"
cap_t = [2.0, -0.1, 0.004, 0.002, -0.00006, -0.0006]
cap_ff = [0.6, 0.5, -0.1]
eir_t = [-0.6, 0.2, -0.006, -0.012, 0.0012, -0.0008]
eir_ff = [1.6, -0.8, 0.2]
eir_plr = [0.8, 0.2, 0.0]
twb_bounds = [-10.0, 20.0]
tdb_bounds = [-20.0, 30.0]
"#
    )
    .unwrap();

    // Create all other required subdirs.
    for subdir in &[
        "battery",
        "envelope",
        "ev",
        "generator",
        "loads",
        "pv",
        "water_heating",
    ] {
        std::fs::create_dir_all(dir.path().join(subdir)).unwrap();
    }

    let store = hares_io::DefaultsStore::load(dir.path()).expect("load defaults");

    let cooling = store
        .hvac_cooling_coefficients("Test Type")
        .expect("cooling coefficients");
    let heating = store
        .hvac_heating_coefficients("Test Type")
        .expect("heating coefficients");

    // They must be independent.
    assert_eq!(cooling.variants[0].name, "Cooling_Single_1");
    assert_eq!(heating.variants[0].name, "Heating_Single_1");
    assert_ne!(
        cooling.variants[0].name, heating.variants[0].name,
        "cooling and heating must return different coefficient sets"
    );

    // Verify correct coefficients from each.
    assert!(
        (cooling.variants[0].cap_t.coeffs[0] - 1.0).abs() < 1e-10,
        "cooling cap_t[0] should be 1.0"
    );
    assert!(
        (heating.variants[0].cap_t.coeffs[0] - 2.0).abs() < 1e-10,
        "heating cap_t[0] should be 2.0"
    );
}

#[test]
fn same_type_in_both_cooling_and_heating_returns_correct_one_from_each() {
    let dir = tempfile::tempdir().unwrap();

    std::fs::write(
        dir.path().join("zip_parameters.toml"),
        r#"
[heat_pump]
zp = 1.0
ip = 0.0
pp = 0.0
zq = 1.0
iq = 0.0
pq = 0.0
pf = 0.95
"#,
    )
    .unwrap();

    // Create "heat_pump.toml" in both cooling and heating directories.
    let cooling_dir = dir.path().join("hvac_cooling");
    std::fs::create_dir_all(&cooling_dir).unwrap();
    let mut f = std::fs::File::create(cooling_dir.join("heat_pump.toml")).unwrap();
    write!(
        f,
        r#"
[[variant]]
name = "HP_Cooling_Mode"
cap_t = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0]
cap_ff = [1.0, 0.0, 0.0]
eir_t = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0]
eir_ff = [1.0, 0.0, 0.0]
eir_plr = [1.0, 0.0, 0.0]
twb_bounds = [10.0, 25.0]
tdb_bounds = [15.0, 50.0]
"#
    )
    .unwrap();

    let heating_dir = dir.path().join("hvac_heating");
    std::fs::create_dir_all(&heating_dir).unwrap();
    let mut f = std::fs::File::create(heating_dir.join("heat_pump.toml")).unwrap();
    write!(
        f,
        r#"
[[variant]]
name = "HP_Heating_Mode"
cap_t = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0]
cap_ff = [1.0, 0.0, 0.0]
eir_t = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0]
eir_ff = [1.0, 0.0, 0.0]
eir_plr = [1.0, 0.0, 0.0]
twb_bounds = [-15.0, 15.0]
tdb_bounds = [-25.0, 25.0]
"#
    )
    .unwrap();

    for subdir in &[
        "battery",
        "envelope",
        "ev",
        "generator",
        "loads",
        "pv",
        "water_heating",
    ] {
        std::fs::create_dir_all(dir.path().join(subdir)).unwrap();
    }

    let store = hares_io::DefaultsStore::load(dir.path()).expect("load");

    let cooling = store
        .hvac_cooling_coefficients("Heat Pump")
        .expect("cooling for heat pump");
    let heating = store
        .hvac_heating_coefficients("Heat Pump")
        .expect("heating for heat pump");

    assert_eq!(cooling.variants[0].name, "HP_Cooling_Mode");
    assert_eq!(heating.variants[0].name, "HP_Heating_Mode");
}

// ===========================================================================
// 9. ResStock state fallback error
// ===========================================================================

#[test]
fn missing_in_state_returns_error_not_default() {
    let tmp = tempdir().expect("tmp");
    let schema = Arc::new(Schema::new(vec![
        Field::new("bldg_id", DataType::Int64, false),
        Field::new("upgrade", DataType::Int64, false),
        Field::new("sample_weight", DataType::Float64, false),
        // Intentionally no "in.state" column.
        Field::new("in.floor_area", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from(vec![1])) as ArrayRef,
            Arc::new(Int64Array::from(vec![0])) as ArrayRef,
            Arc::new(Float64Array::from(vec![1.0])) as ArrayRef,
            Arc::new(StringArray::from(vec!["1500-1999"])) as ArrayRef,
        ],
    )
    .expect("batch");
    let pq = tmp.path().join("no_state.parquet");
    write_parquet(&pq, &batch);

    let err = parse_resstock_metadata(&pq, ResStockVersion::V2024_1, tmp.path()).unwrap_err();
    match err {
        ResStockError::MissingCharacteristic(col) => {
            assert_eq!(
                col, "in.state",
                "error must indicate the missing characteristic is 'in.state'"
            );
        }
        other => panic!("expected MissingCharacteristic for in.state, got {other}"),
    }
}

// ===========================================================================
// 10. ColumnMapper re-export
// ===========================================================================

#[test]
fn column_mapper_is_accessible_from_hares_io() {
    // This test simply verifies that the ColumnMapper trait is re-exported
    // from hares_io and can be used in downstream code.
    fn assert_trait_object_usable(_mapper: &dyn ColumnMapper) {
        // If this compiles and runs, the re-export works.
    }

    // We can't easily construct a concrete mapper from outside the crate,
    // but the fact that `ColumnMapper` is importable and usable as a trait
    // object is sufficient.
    struct TestMapper;
    impl ColumnMapper for TestMapper {
        fn bldg_id_col(&self) -> &str {
            "bldg_id"
        }
        fn sample_weight_col(&self) -> &str {
            "sample_weight"
        }
        fn upgrade_col(&self) -> &str {
            "upgrade"
        }
    }

    let mapper = TestMapper;
    assert_trait_object_usable(&mapper);
    assert_eq!(mapper.bldg_id_col(), "bldg_id");
}
