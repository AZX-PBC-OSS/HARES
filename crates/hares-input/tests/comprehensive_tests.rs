//! Comprehensive integration tests for hares-input: ResStock metadata
//! parsing, the defaults store, and the re-exported mapper trait.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use arrow::array::{ArrayRef, Float64Array, Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::ArrowWriter;
use tempfile::tempdir;

use hares_input::{ColumnMapper, ResStockError, ResStockVersion, parse_resstock_metadata};

fn write_parquet(path: &Path, batch: &RecordBatch) {
    let file = std::fs::File::create(path).expect("create parquet");
    let mut writer = ArrowWriter::try_new(file, batch.schema(), None).expect("writer");
    writer.write(batch).expect("write");
    writer.close().expect("close");
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
// 8. hvac_coefficients split
// ===========================================================================

/// Fixture helper: a defaults tree whose EV mapping is not the subject of
/// the test still needs a complete one, because a missing or malformed
/// vehicle mapping CSV fails the defaults load.
fn write_complete_ev_mapping(dir: &std::path::Path) {
    let ev_dir = dir.join("ev");
    std::fs::create_dir_all(&ev_dir).unwrap();
    let mut csv_content = String::from(
        "profile_column,vehicle_type,profile_file,capacity_kwh,charger_power_kw,efficiency\n",
    );
    for i in 1..=35 {
        csv_content.push_str(&format!(
            "Vehicle {i},MY2030_BEV_SUV,pdf_Veh{},117.6,10.26,0.9\n",
            ((i - 1) % 4) + 1
        ));
    }
    for i in 36..=50 {
        csv_content.push_str(&format!(
            "Vehicle {i},MY2030_PHEV_SUV,pdf_Veh{},14.8,7.2,0.9\n",
            ((i - 1) % 4) + 1
        ));
    }
    std::fs::write(ev_dir.join("vehicle_mapping.csv"), csv_content).unwrap();
}

/// The required CSVs a loadable tree carries beside the files under test:
/// the water-heating CSV and the envelope LUT's three CSVs, header-only.
fn write_required_csvs(dir: &std::path::Path) {
    let wh = dir.join("water_heating");
    std::fs::create_dir_all(&wh).unwrap();
    std::fs::write(
        wh.join("default_paramters.csv"),
        "Description,Name,Value,Units\n",
    )
    .unwrap();
    let env = dir.join("envelope");
    std::fs::create_dir_all(&env).unwrap();
    std::fs::write(
        env.join("Envelope Boundary Types.csv"),
        "Boundary Name,Boundary Type,Finish Type,Construction Type,Insulation Details,Assembly R Value\n",
    )
    .unwrap();
    std::fs::write(
        env.join("Envelope Materials.csv"),
        "Boundary Name,Boundary Type,Resistance (m^2-K/W),Capacitance (kJ/m^2-K)\n",
    )
    .unwrap();
    std::fs::write(
        env.join("Envelope Boundaries.csv"),
        "Boundary Name,Exterior Zone Label,Interior Zone Label\n",
    )
    .unwrap();
}

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
    // The generator efficiency curve is mandatory at load.
    write_complete_ev_mapping(dir.path());
    std::fs::write(
        dir.path().join("generator").join("efficiency_curve.toml"),
        "[[points]]\ncapacity_ratio = 0.0\nefficiency_ratio = 0.0\n\
         [[points]]\ncapacity_ratio = 1.0\nefficiency_ratio = 1.0\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("HVAC Multispeed Parameters.csv"),
        "HVAC Name,HVAC Efficiency,Number of Speeds,Capacity Ratio 1,Air Flow Ratio 1,COP 1,SHR 1\n",
    )
    .unwrap();
    write_required_csvs(dir.path());
    std::fs::write(
        dir.path().join("HVAC Multispeed Parameters.csv"),
        "HVAC Name,HVAC Efficiency,Number of Speeds,Capacity Ratio 1,Air Flow Ratio 1,COP 1,SHR 1\n",
    )
    .unwrap();
    write_required_csvs(dir.path());

    let store = hares_input::DefaultsStore::load(dir.path()).expect("load defaults");

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
    // The generator efficiency curve is mandatory at load.
    write_complete_ev_mapping(dir.path());
    std::fs::write(
        dir.path().join("generator").join("efficiency_curve.toml"),
        "[[points]]\ncapacity_ratio = 0.0\nefficiency_ratio = 0.0\n\
         [[points]]\ncapacity_ratio = 1.0\nefficiency_ratio = 1.0\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("HVAC Multispeed Parameters.csv"),
        "HVAC Name,HVAC Efficiency,Number of Speeds,Capacity Ratio 1,Air Flow Ratio 1,COP 1,SHR 1\n",
    )
    .unwrap();
    write_required_csvs(dir.path());

    let store = hares_input::DefaultsStore::load(dir.path()).expect("load");

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
        ResStockError::MissingCharacteristic {
            bldg_id,
            characteristic,
        } => {
            assert_eq!(bldg_id, 1);
            assert_eq!(
                characteristic, "in.state",
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
fn column_mapper_is_accessible_from_hares_input() {
    // This test simply verifies that the ColumnMapper trait is re-exported
    // from hares_input and can be used in downstream code.
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
