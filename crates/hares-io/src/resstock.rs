//! ResStock building stock sampling and metadata.

use std::{
    collections::HashMap,
    fs,
    fs::File,
    path::{Path, PathBuf},
};

use arrow::{
    array::{Array, Float64Array, Int64Array, LargeStringArray, StringArray},
    datatypes::{DataType, Schema},
    record_batch::RecordBatch,
};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResStockVersion {
    V2024_1,
    V2024_2,
    V2025_1,
}

impl std::fmt::Display for ResStockVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::V2024_1 => write!(f, "2024.1"),
            Self::V2024_2 => write!(f, "2024.2"),
            Self::V2025_1 => write!(f, "2025.1"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResStockBuilding {
    pub bldg_id: i64,
    pub upgrade: i64,
    pub sample_weight: f64,
    pub hpxml_path: PathBuf,
    pub schedule_path: PathBuf,
    pub weather_path: Option<PathBuf>,
    pub weather_fips: Option<String>,
    pub characteristics: HashMap<String, String>,
}

#[derive(Debug, Error)]
pub enum ResStockError {
    #[error("version mismatch: expected {expected}, missing column `{missing_column}`")]
    VersionMismatch {
        expected: String,
        missing_column: String,
    },
    #[error("null value in required column `{column}` at row {row}")]
    NullRequired { column: String, row: usize },
    #[error("missing required characteristic: {0}")]
    MissingCharacteristic(String),
    #[error("parquet error: {0}")]
    ParquetError(String),
    #[error("arrow error: {0}")]
    ArrowError(String),
    #[error("io error: {0}")]
    IoError(String),
}

impl From<parquet::errors::ParquetError> for ResStockError {
    fn from(e: parquet::errors::ParquetError) -> Self {
        Self::ParquetError(e.to_string())
    }
}

impl From<std::io::Error> for ResStockError {
    fn from(e: std::io::Error) -> Self {
        Self::IoError(e.to_string())
    }
}

impl From<arrow::error::ArrowError> for ResStockError {
    fn from(e: arrow::error::ArrowError) -> Self {
        Self::ArrowError(e.to_string())
    }
}

type Result<T> = std::result::Result<T, ResStockError>;

pub trait ColumnMapper: Send + Sync {
    fn bldg_id_col(&self) -> &str;
    fn sample_weight_col(&self) -> &str;
    fn upgrade_col(&self) -> &str;
}

#[derive(Debug, Clone, Copy)]
struct V2024_1Mapper;
#[derive(Debug, Clone, Copy)]
struct V2024_2Mapper;
#[derive(Debug, Clone, Copy)]
struct V2025Mapper;

impl ColumnMapper for V2024_1Mapper {
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

impl ColumnMapper for V2024_2Mapper {
    fn bldg_id_col(&self) -> &str {
        // ResStock 2024.2 renamed bldg_id to building_id.
        "building_id"
    }
    fn sample_weight_col(&self) -> &str {
        "sample_weight"
    }
    fn upgrade_col(&self) -> &str {
        "upgrade"
    }
}

impl ColumnMapper for V2025Mapper {
    fn bldg_id_col(&self) -> &str {
        "building_id"
    }
    fn sample_weight_col(&self) -> &str {
        "weight"
    }
    fn upgrade_col(&self) -> &str {
        "upgrade_id"
    }
}

fn mapper_for(version: ResStockVersion) -> &'static dyn ColumnMapper {
    match version {
        ResStockVersion::V2024_1 => &V2024_1Mapper,
        ResStockVersion::V2024_2 => &V2024_2Mapper,
        ResStockVersion::V2025_1 => &V2025Mapper,
    }
}

enum StringCol<'a> {
    Utf8(&'a StringArray),
    LargeUtf8(&'a LargeStringArray),
}

fn string_value_at<'a>(col: &'a StringCol<'a>, row_idx: usize) -> Option<&'a str> {
    match col {
        StringCol::Utf8(arr) => {
            if arr.is_null(row_idx) {
                None
            } else {
                Some(arr.value(row_idx))
            }
        }
        StringCol::LargeUtf8(arr) => {
            if arr.is_null(row_idx) {
                None
            } else {
                Some(arr.value(row_idx))
            }
        }
    }
}

fn collect_string_columns<'a>(
    batch: &'a RecordBatch,
    skip: &[&str],
) -> Result<Vec<(String, StringCol<'a>)>> {
    let mut cols = Vec::new();
    for (col_idx, field) in batch.schema().fields().iter().enumerate() {
        let name = field.name();
        if name.starts_with("out.") || skip.contains(&name.as_str()) {
            continue;
        }
        let arr = batch.column(col_idx);
        match field.data_type() {
            DataType::Utf8 => {
                let typed = arr.as_any().downcast_ref::<StringArray>().ok_or_else(|| {
                    ResStockError::ArrowError(format!("column `{name}` is not Utf8"))
                })?;
                cols.push((name.clone(), StringCol::Utf8(typed)));
            }
            DataType::LargeUtf8 => {
                let typed = arr
                    .as_any()
                    .downcast_ref::<LargeStringArray>()
                    .ok_or_else(|| {
                        ResStockError::ArrowError(format!("column `{name}` is not LargeUtf8"))
                    })?;
                cols.push((name.clone(), StringCol::LargeUtf8(typed)));
            }
            _ => {}
        }
    }
    Ok(cols)
}

fn build_zip_path(
    version: ResStockVersion,
    dataset_root: &Path,
    bldg_id: i64,
    upgrade: i64,
    characteristics: &HashMap<String, String>,
) -> Result<PathBuf> {
    match version {
        ResStockVersion::V2024_1 | ResStockVersion::V2024_2 => {
            let state = characteristics
                .get("in.state")
                .ok_or_else(|| ResStockError::MissingCharacteristic("in.state".to_string()))?;
            Ok(dataset_root
                .join("building_energy_models")
                .join(state.as_str())
                .join(format!("up{upgrade:02}-baseline"))
                .join(format!("bldg{bldg_id}.zip")))
        }
        ResStockVersion::V2025_1 => Ok(dataset_root
            .join("building_energy_models")
            .join(format!("upgrade={upgrade}"))
            .join(format!("bldg{bldg_id:07}-up{upgrade:02}.zip"))),
    }
}

pub fn parse_resstock_metadata(
    path: &Path,
    version: ResStockVersion,
    dataset_root: &Path,
) -> Result<Vec<ResStockBuilding>> {
    let mapper = mapper_for(version);
    let file = File::open(path)?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
    let schema = builder.schema().clone();

    let bldg_id_idx = required_col_idx(&schema, mapper.bldg_id_col(), version)?;
    let weight_idx = required_col_idx(&schema, mapper.sample_weight_col(), version)?;
    let upgrade_idx = required_col_idx(&schema, mapper.upgrade_col(), version)?;

    let reader = builder.build()?;
    let mut rows = Vec::new();

    for maybe_batch in reader {
        let batch = maybe_batch?;
        let bldg_arr = col_as_i64(&batch, bldg_id_idx, mapper.bldg_id_col())?;
        let weight_arr = col_as_f64(&batch, weight_idx, mapper.sample_weight_col())?;
        let upgrade_arr = col_as_i64(&batch, upgrade_idx, mapper.upgrade_col())?;

        let skip = [
            mapper.bldg_id_col(),
            mapper.sample_weight_col(),
            mapper.upgrade_col(),
        ];
        let string_cols = collect_string_columns(&batch, &skip)?;

        for row_idx in 0..batch.num_rows() {
            if bldg_arr.is_null(row_idx) {
                return Err(ResStockError::NullRequired {
                    column: mapper.bldg_id_col().to_string(),
                    row: row_idx,
                });
            }
            if weight_arr.is_null(row_idx) {
                return Err(ResStockError::NullRequired {
                    column: mapper.sample_weight_col().to_string(),
                    row: row_idx,
                });
            }
            if upgrade_arr.is_null(row_idx) {
                return Err(ResStockError::NullRequired {
                    column: mapper.upgrade_col().to_string(),
                    row: row_idx,
                });
            }
            let bldg_id = bldg_arr.value(row_idx);
            let sample_weight = weight_arr.value(row_idx);
            let upgrade = upgrade_arr.value(row_idx);

            let mut characteristics = HashMap::new();
            for (name, col) in &string_cols {
                if let Some(value) = string_value_at(col, row_idx) {
                    characteristics.insert(name.clone(), maybe_range_to_midpoint(value));
                }
            }

            let zip_path =
                build_zip_path(version, dataset_root, bldg_id, upgrade, &characteristics)?;

            rows.push(ResStockBuilding {
                bldg_id,
                upgrade,
                sample_weight,
                hpxml_path: zip_path.clone(),
                schedule_path: zip_path.clone(),
                weather_path: None,
                weather_fips: None,
                characteristics,
            });
        }
    }

    Ok(rows)
}

fn required_col_idx(schema: &Schema, name: &str, version: ResStockVersion) -> Result<usize> {
    schema
        .index_of(name)
        .map_err(|_| ResStockError::VersionMismatch {
            expected: version.to_string(),
            missing_column: name.to_string(),
        })
}

fn col_as_i64<'a>(batch: &'a RecordBatch, idx: usize, col_name: &str) -> Result<&'a Int64Array> {
    batch
        .column(idx)
        .as_any()
        .downcast_ref::<Int64Array>()
        .ok_or_else(|| ResStockError::ArrowError(format!("column `{col_name}` is not Int64")))
}

fn col_as_f64<'a>(batch: &'a RecordBatch, idx: usize, col_name: &str) -> Result<&'a Float64Array> {
    batch
        .column(idx)
        .as_any()
        .downcast_ref::<Float64Array>()
        .ok_or_else(|| ResStockError::ArrowError(format!("column `{col_name}` is not Float64")))
}

fn maybe_range_to_midpoint(value: &str) -> String {
    let Some((left, right)) = value.split_once('-') else {
        return value.to_string();
    };
    if left.is_empty()
        || right.is_empty()
        || !left.chars().all(|c| c.is_ascii_digit())
        || !right.chars().all(|c| c.is_ascii_digit())
    {
        return value.to_string();
    }
    match (left.parse::<f64>(), right.parse::<f64>()) {
        (Ok(lo), Ok(hi)) => ((lo + hi) / 2.0).to_string(),
        _ => value.to_string(),
    }
}

/// Extract the weather station FIPS code from an HPXML file.
///
/// ResStock HPXML files contain a `WeatherStation/Name` element under
/// `ClimateandRiskZones` that holds a county FIPS code (e.g. `"G0800130"`)
/// which identifies the weather station for the building.
///
/// Returns the FIPS code string if the element exists and its content
/// matches the expected FIPS pattern (`G + 2-digit state + 3-digit
/// county + 2-digit suffix`). Returns `None` if the file is missing,
/// cannot be parsed, or lacks a FIPS-format weather station name.
///
/// Weather station names that do not match the FIPS pattern (e.g.
/// `"USA_CO_Denver"`) also return `None` — these are legacy OCHRE
/// fixtures, not ResStock dataset entries.
pub fn parse_weather_station_fips(path: &Path) -> Option<String> {
    let xml = fs::read_to_string(path).ok()?;
    let root = crate::hpxml::building::parse_xml_document(&xml).ok()?;

    let name_text = root
        .first_descendant("WeatherStation")
        .and_then(|ws| ws.child("Name"))
        .map(|n| n.text.trim().to_string())
        .filter(|s| !s.is_empty())?;

    // ResStock weather station names are FIPS codes like 'G0100290'.
    // Pattern: G + state_fips(2) + county_fips(3) + suffix(2) e.g. G0800130
    if !name_text.starts_with('G') || name_text.len() < 7 {
        return None;
    }

    let fips = name_text;
    if fips[1..].chars().all(|c| c.is_ascii_digit()) {
        Some(fips)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::ArrayRef;
    use arrow::datatypes::Field;
    use parquet::arrow::arrow_writer::ArrowWriter;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn write_parquet(path: &Path, batch: &RecordBatch) {
        let file = File::create(path).expect("create parquet");
        let mut writer = ArrowWriter::try_new(file, batch.schema(), None).expect("writer");
        writer.write(batch).expect("write");
        writer.close().expect("close");
    }

    fn sample_batch() -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("bldg_id", DataType::Int64, false),
            Field::new("upgrade", DataType::Int64, false),
            Field::new("sample_weight", DataType::Float64, false),
            Field::new("in.state", DataType::Utf8, false),
            Field::new("in.floor_area", DataType::Utf8, false),
            Field::new("out.site_electricity_kwh", DataType::Utf8, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![12_345])) as ArrayRef,
                Arc::new(Int64Array::from(vec![3])) as ArrayRef,
                Arc::new(Float64Array::from(vec![2.345_678_f64])) as ArrayRef,
                Arc::new(StringArray::from(vec!["CO"])) as ArrayRef,
                Arc::new(StringArray::from(vec!["1500-1999"])) as ArrayRef,
                Arc::new(StringArray::from(vec!["1234"])) as ArrayRef,
            ],
        )
        .expect("batch")
    }

    #[test]
    fn range_string_parsed_as_midpoint() {
        assert_eq!(maybe_range_to_midpoint("1500-1999"), "1749.5");
        assert_eq!(maybe_range_to_midpoint("0-100"), "50");
        assert_eq!(maybe_range_to_midpoint("200-200"), "200");
    }

    #[test]
    fn malformed_strings_stored_verbatim() {
        assert_eq!(maybe_range_to_midpoint("N/A"), "N/A");
        assert_eq!(maybe_range_to_midpoint("hello"), "hello");
        assert_eq!(maybe_range_to_midpoint(""), "");
        assert_eq!(maybe_range_to_midpoint("-5"), "-5");
        assert_eq!(maybe_range_to_midpoint("abc-def"), "abc-def");
    }

    #[test]
    fn out_columns_excluded_from_characteristics() {
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("test.parquet");
        write_parquet(&pq, &sample_batch());

        let rows =
            parse_resstock_metadata(&pq, ResStockVersion::V2024_1, tmp.path()).expect("parse");
        let chars = &rows[0].characteristics;
        assert!(chars.contains_key("in.state"));
        assert!(chars.contains_key("in.floor_area"));
        assert!(
            !chars.contains_key("out.site_electricity_kwh"),
            "out.* columns must be excluded"
        );
    }

    #[test]
    fn core_columns_excluded_from_characteristics() {
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("test.parquet");
        write_parquet(&pq, &sample_batch());

        let rows =
            parse_resstock_metadata(&pq, ResStockVersion::V2024_1, tmp.path()).expect("parse");
        let chars = &rows[0].characteristics;
        assert!(!chars.contains_key("bldg_id"));
        assert!(!chars.contains_key("upgrade"));
        assert!(!chars.contains_key("sample_weight"));
    }

    #[test]
    fn v2024_1_path_uses_state_layout() {
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("test.parquet");
        write_parquet(&pq, &sample_batch());

        let rows =
            parse_resstock_metadata(&pq, ResStockVersion::V2024_1, tmp.path()).expect("parse");
        let row = &rows[0];
        assert_eq!(row.bldg_id, 12_345);
        assert_eq!(row.upgrade, 3);
        assert_eq!(row.sample_weight, 2.345_678_f64);
        assert_eq!(
            row.hpxml_path,
            tmp.path()
                .join("building_energy_models/CO/up03-baseline/bldg12345.zip")
        );
        assert_eq!(row.hpxml_path, row.schedule_path);
        assert!(row.weather_path.is_none());
        assert!(row.weather_fips.is_none());
    }

    fn sample_batch_v2024_2() -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("building_id", DataType::Int64, false),
            Field::new("upgrade", DataType::Int64, false),
            Field::new("sample_weight", DataType::Float64, false),
            Field::new("in.state", DataType::Utf8, false),
            Field::new("in.floor_area", DataType::Utf8, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![12_345])) as ArrayRef,
                Arc::new(Int64Array::from(vec![3])) as ArrayRef,
                Arc::new(Float64Array::from(vec![2.345_678_f64])) as ArrayRef,
                Arc::new(StringArray::from(vec!["CO"])) as ArrayRef,
                Arc::new(StringArray::from(vec!["1500-1999"])) as ArrayRef,
            ],
        )
        .expect("batch")
    }

    #[test]
    fn v2024_2_path_uses_state_layout() {
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("test.parquet");
        write_parquet(&pq, &sample_batch_v2024_2());

        let rows =
            parse_resstock_metadata(&pq, ResStockVersion::V2024_2, tmp.path()).expect("parse");
        assert_eq!(
            rows[0].hpxml_path,
            tmp.path()
                .join("building_energy_models/CO/up03-baseline/bldg12345.zip")
        );
    }

    fn sample_batch_v2025() -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("building_id", DataType::Int64, false),
            Field::new("upgrade_id", DataType::Int64, false),
            Field::new("weight", DataType::Float64, false),
            Field::new("in.state", DataType::Utf8, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![12_345])) as ArrayRef,
                Arc::new(Int64Array::from(vec![3])) as ArrayRef,
                Arc::new(Float64Array::from(vec![2.345_678_f64])) as ArrayRef,
                Arc::new(StringArray::from(vec!["CO"])) as ArrayRef,
            ],
        )
        .expect("batch")
    }

    #[test]
    fn v2025_1_path_uses_flat_layout() {
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("test.parquet");
        write_parquet(&pq, &sample_batch_v2025());

        let rows =
            parse_resstock_metadata(&pq, ResStockVersion::V2025_1, tmp.path()).expect("parse");
        assert_eq!(
            rows[0].hpxml_path,
            tmp.path()
                .join("building_energy_models/upgrade=3/bldg0012345-up03.zip")
        );
    }

    #[test]
    fn missing_required_column_returns_version_mismatch() {
        let tmp = tempdir().expect("tmp");
        let schema = Arc::new(Schema::new(vec![
            Field::new("bldg_id", DataType::Int64, false),
            Field::new("upgrade", DataType::Int64, false),
            Field::new("in.state", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![1])) as ArrayRef,
                Arc::new(Int64Array::from(vec![0])) as ArrayRef,
                Arc::new(StringArray::from(vec!["CO"])) as ArrayRef,
            ],
        )
        .expect("batch");
        let pq = tmp.path().join("missing.parquet");
        write_parquet(&pq, &batch);

        let err = parse_resstock_metadata(&pq, ResStockVersion::V2024_1, tmp.path()).unwrap_err();
        match err {
            ResStockError::VersionMismatch {
                expected,
                missing_column,
            } => {
                assert_eq!(missing_column, "sample_weight");
                assert_eq!(expected, "2024.1");
            }
            other => panic!("expected VersionMismatch, got {other}"),
        }
    }

    #[test]
    fn missing_bldg_id_returns_version_mismatch() {
        let tmp = tempdir().expect("tmp");
        let schema = Arc::new(Schema::new(vec![
            Field::new("wrong_id", DataType::Int64, false),
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
        let pq = tmp.path().join("mismatch.parquet");
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
            other => panic!("expected VersionMismatch, got {other}"),
        }
    }

    #[test]
    fn v2024_missing_state_returns_error() {
        let tmp = tempdir().expect("tmp");
        let schema = Arc::new(Schema::new(vec![
            Field::new("bldg_id", DataType::Int64, false),
            Field::new("upgrade", DataType::Int64, false),
            Field::new("sample_weight", DataType::Float64, false),
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
                assert_eq!(col, "in.state");
            }
            other => panic!("expected MissingCharacteristic for in.state, got {other}"),
        }
    }

    #[test]
    fn characteristics_midpoint_for_range() {
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("test.parquet");
        write_parquet(&pq, &sample_batch());

        let rows =
            parse_resstock_metadata(&pq, ResStockVersion::V2024_1, tmp.path()).expect("parse");
        assert_eq!(
            rows[0].characteristics.get("in.floor_area"),
            Some(&"1749.5".to_string())
        );
    }

    #[test]
    fn sample_weight_preserved_exactly() {
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("test.parquet");
        write_parquet(&pq, &sample_batch());

        let rows =
            parse_resstock_metadata(&pq, ResStockVersion::V2024_1, tmp.path()).expect("parse");
        assert_eq!(rows[0].sample_weight, 2.345_678_f64);
    }

    #[test]
    fn display_versions() {
        assert_eq!(ResStockVersion::V2024_1.to_string(), "2024.1");
        assert_eq!(ResStockVersion::V2024_2.to_string(), "2024.2");
        assert_eq!(ResStockVersion::V2025_1.to_string(), "2025.1");
    }

    #[test]
    fn io_error_variant_for_missing_file() {
        let err = parse_resstock_metadata(
            Path::new("/nonexistent/file.parquet"),
            ResStockVersion::V2024_1,
            Path::new("/tmp"),
        )
        .unwrap_err();
        assert!(matches!(err, ResStockError::IoError(_)));
    }

    fn write_hpxml_fixture(path: &Path, weather_station_name: &str) {
        let xml = format!(
            r#"<?xml version='1.0' encoding='UTF-8'?>
<HPXML xmlns='http://hpxmlonline.com/2023/09' schemaVersion='4.0'>
  <Building>
    <BuildingDetails>
      <ClimateandRiskZones>
        <ClimateZoneIECC>
          <Year>2006</Year>
          <ClimateZone>5B</ClimateZone>
        </ClimateZoneIECC>
        <WeatherStation>
          <SystemIdentifier id='WeatherStation'/>
          <Name>{weather_station_name}</Name>
        </WeatherStation>
      </ClimateandRiskZones>
    </BuildingDetails>
  </Building>
</HPXML>"#
        );
        fs::write(path, xml).expect("write hpxml fixture");
    }

    #[test]
    fn parse_weather_station_fips_extracts_valid_fips() {
        let tmp = tempdir().expect("tmp");
        let hpxml_path = tmp.path().join("home.xml");
        write_hpxml_fixture(&hpxml_path, "G0800130");
        let fips = parse_weather_station_fips(&hpxml_path);
        assert_eq!(fips, Some("G0800130".to_string()));
    }

    #[test]
    fn parse_weather_station_fips_rejects_non_fips_name() {
        let tmp = tempdir().expect("tmp");
        let hpxml_path = tmp.path().join("home.xml");
        write_hpxml_fixture(&hpxml_path, "USA_CO_Denver");
        let fips = parse_weather_station_fips(&hpxml_path);
        assert_eq!(fips, None);
    }

    #[test]
    fn parse_weather_station_fips_handles_missing_weather_station() {
        let tmp = tempdir().expect("tmp");
        let hpxml_path = tmp.path().join("home.xml");
        let xml = r#"<?xml version='1.0' encoding='UTF-8'?>
<HPXML xmlns='http://hpxmlonline.com/2023/09' schemaVersion='4.0'>
  <Building>
    <BuildingDetails>
      <ClimateandRiskZones>
        <ClimateZoneIECC>
          <Year>2006</Year>
          <ClimateZone>5B</ClimateZone>
        </ClimateZoneIECC>
      </ClimateandRiskZones>
    </BuildingDetails>
  </Building>
</HPXML>"#;
        fs::write(&hpxml_path, xml).expect("write hpxml fixture");
        let fips = parse_weather_station_fips(&hpxml_path);
        assert_eq!(fips, None);
    }

    #[test]
    fn parse_weather_station_fips_handles_missing_hpxml_file() {
        let fips = parse_weather_station_fips(Path::new("/nonexistent/hpxml.xml"));
        assert_eq!(fips, None);
    }
}
