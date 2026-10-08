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
use tracing::warn;

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

impl std::str::FromStr for ResStockVersion {
    type Err = String;

    /// Parses the release spellings [`Display`](Self::fmt) writes: the
    /// error names every accepted spelling.
    fn from_str(text: &str) -> std::result::Result<Self, Self::Err> {
        match text {
            "2024.1" => Ok(Self::V2024_1),
            "2024.2" => Ok(Self::V2024_2),
            "2025.1" => Ok(Self::V2025_1),
            other => Err(format!(
                "unknown ResStock version {other:?}: known versions are \"2024.1\", \"2024.2\", \"2025.1\""
            )),
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
    #[error("building {bldg_id}: missing required characteristic `{characteristic}`")]
    MissingCharacteristic {
        bldg_id: i64,
        characteristic: String,
    },
    #[error("invalid sample_weight {value} for building {bldg_id}")]
    InvalidSampleWeight { bldg_id: i64, value: f64 },
    #[error(
        "building {bldg_id}: missing dataset layout element {element}: {path} (pass the \
         unpacked buildings directory, where each building is extracted to a bldgNNNNNNN \
         folder holding home.xml and in.schedules.csv, not the dataset zip archive)"
    )]
    MissingLayout {
        bldg_id: i64,
        element: &'static str,
        path: PathBuf,
    },
    #[error("building {bldg_id}: cannot read the weather station FIPS from {path}: {source}")]
    WeatherStation {
        bldg_id: i64,
        path: PathBuf,
        source: WeatherStationError,
    },
    #[error("parquet error: {0}")]
    ParquetError(String),
    #[error("arrow error: {0}")]
    ArrowError(String),
    #[error("io error: {0}")]
    IoError(String),
}

/// Errors raised when a building's HPXML weather station cannot yield a
/// county FIPS code.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum WeatherStationError {
    #[error("the HPXML file could not be read or parsed: {0}")]
    Unreadable(String),
    #[error("the HPXML has no non-empty WeatherStation/Name element")]
    MissingName,
    #[error(
        "the WeatherStation/Name value `{value}` is not a county FIPS code (G followed by digits)"
    )]
    NotAFips { value: String },
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

/// The directory of one unpacked building under `buildings_dir`.
///
/// The extracted layout is version independent: `hares.data.resstock`
/// unpacks every downloaded building zip to `bldg<bldg_id:07>/` holding
/// `home.xml` and `in.schedules.csv`, whatever the release, and the
/// committed `tests/fixtures/resstock/<version>` buildings carry the same
/// shape.
fn extracted_building_dir(buildings_dir: &Path, bldg_id: i64) -> PathBuf {
    buildings_dir.join(format!("bldg{bldg_id:07}"))
}

/// Parse a ResStock metadata parquet into buildings resolved to the
/// unpacked dataset layout.
///
/// # Layout policy: the unpacked buildings directory is required
///
/// `buildings_dir` must hold one directory per building, named
/// `bldg<bldg_id:07>`, containing that building's `home.xml` (HPXML) and
/// `in.schedules.csv` (the schedule). This is the layout
/// `hares.data.resstock` unpacks every dataset zip to and the committed
/// `tests/fixtures/resstock/<version>` buildings carry. The engine never
/// reads HPXML or schedules out of the dataset's zip archives, so the
/// constructor does not unpack: a building whose directory or files are
/// missing (as when the dataset root with its zip archives is passed) is
/// a [`ResStockError::MissingLayout`] naming the building, the missing
/// element and the fix.
///
/// Each building's `weather_fips` is read from its own HPXML
/// (`WeatherStation/Name`) through [`parse_weather_station_fips`]; a
/// building whose station name cannot be read is a
/// [`ResStockError::WeatherStation`] naming the building and the value
/// read.
///
/// `in.state` is a required characteristic: the fleet's zone validation
/// and the weather lookup key on it, and every released metadata schema
/// carries it.
///
/// The layout resolution covers EVERY metadata row: a fleet filter narrows
/// the returned fleet, not this parse, so a filtered fleet still requires
/// every metadata building to be unpacked under `buildings_dir`.
pub fn parse_resstock_metadata(
    path: &Path,
    version: ResStockVersion,
    buildings_dir: &Path,
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

            match crate::classify_sample_weight(sample_weight) {
                crate::SampleWeightClass::Invalid => {
                    tracing::error!(
                        bldg_id = bldg_id,
                        sample_weight = sample_weight,
                        "Building has invalid sample_weight (NaN, infinite, or negative)",
                    );
                    return Err(ResStockError::InvalidSampleWeight {
                        bldg_id,
                        value: sample_weight,
                    });
                }
                crate::SampleWeightClass::Zero => {
                    warn!(bldg_id = bldg_id, "Building has zero sample_weight",);
                }
                crate::SampleWeightClass::Positive => {}
            }

            let mut characteristics = HashMap::new();
            for (name, col) in &string_cols {
                if let Some(value) = string_value_at(col, row_idx) {
                    characteristics.insert(name.clone(), maybe_range_to_midpoint(value));
                }
            }

            if !characteristics.contains_key("in.state") {
                return Err(ResStockError::MissingCharacteristic {
                    bldg_id,
                    characteristic: "in.state".to_string(),
                });
            }

            let building_dir = extracted_building_dir(buildings_dir, bldg_id);
            let home_xml_path = building_dir.join("home.xml");
            let schedule_path = building_dir.join("in.schedules.csv");
            for (element, path, want_dir) in [
                ("the building directory", building_dir.as_path(), true),
                ("home.xml", home_xml_path.as_path(), false),
                ("in.schedules.csv", schedule_path.as_path(), false),
            ] {
                let present = if want_dir {
                    path.is_dir()
                } else {
                    path.is_file()
                };
                if !present {
                    return Err(ResStockError::MissingLayout {
                        bldg_id,
                        element,
                        path: path.to_path_buf(),
                    });
                }
            }

            let weather_fips = parse_weather_station_fips(&home_xml_path).map_err(|source| {
                ResStockError::WeatherStation {
                    bldg_id,
                    path: home_xml_path.clone(),
                    source,
                }
            })?;

            rows.push(ResStockBuilding {
                bldg_id,
                upgrade,
                sample_weight,
                hpxml_path: home_xml_path,
                schedule_path,
                weather_path: None,
                weather_fips: Some(weather_fips),
                characteristics,
            });
        }
    }

    #[cfg(feature = "observe")]
    {
        let zero_count = rows.iter().filter(|r| r.sample_weight == 0.0).count();
        let nan_count = rows.iter().filter(|r| r.sample_weight.is_nan()).count();
        let positive_weights: Vec<f64> = rows
            .iter()
            .map(|r| r.sample_weight)
            .filter(|w| w.is_finite() && *w > 0.0)
            .collect();
        if !positive_weights.is_empty() {
            let min = positive_weights
                .iter()
                .copied()
                .fold(f64::INFINITY, f64::min);
            let max = positive_weights
                .iter()
                .copied()
                .fold(f64::NEG_INFINITY, f64::max);
            let mean = positive_weights.iter().sum::<f64>() / positive_weights.len() as f64;
            tracing::info!(
                target: "observe",
                version = %version,
                total_rows = rows.len(),
                zero_weight_count = zero_count,
                nan_weight_count = nan_count,
                weight_min = min,
                weight_max = max,
                weight_mean = mean,
                "ResStock weight distribution statistics",
            );
        } else {
            tracing::info!(
                target: "observe",
                version = %version,
                total_rows = rows.len(),
                zero_weight_count = zero_count,
                nan_weight_count = nan_count,
                "ResStock weight distribution (no positive weights)",
            );
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
/// # Errors
///
/// [`WeatherStationError::Unreadable`] when the file cannot be read or
/// parsed, [`WeatherStationError::MissingName`] when no non-empty
/// `WeatherStation/Name` element is present, and
/// [`WeatherStationError::NotAFips`] when the name does not match the
/// county FIPS pattern (`G` + 2-digit state + 3-digit county + 2-digit
/// suffix), naming the value read. Names like `"USA_CO_Denver"` are
/// legacy OCHRE fixtures, not ResStock dataset entries, and are
/// [`WeatherStationError::NotAFips`].
pub fn parse_weather_station_fips(path: &Path) -> std::result::Result<String, WeatherStationError> {
    let xml =
        fs::read_to_string(path).map_err(|err| WeatherStationError::Unreadable(err.to_string()))?;
    let root = crate::hpxml::building::parse_xml_document(&xml)
        .map_err(|err| WeatherStationError::Unreadable(err.to_string()))?;

    let name_text = root
        .first_descendant("WeatherStation")
        .and_then(|ws| ws.child("Name"))
        .map(|n| n.text.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or(WeatherStationError::MissingName)?;

    // ResStock weather station names are FIPS codes like 'G0100290'.
    // Pattern: G + state_fips(2) + county_fips(3) + suffix(2) e.g. G0800130
    if !name_text.starts_with('G') || name_text.len() < 7 {
        return Err(WeatherStationError::NotAFips { value: name_text });
    }

    if name_text[1..].chars().all(|c| c.is_ascii_digit()) {
        Ok(name_text)
    } else {
        Err(WeatherStationError::NotAFips { value: name_text })
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
        write_extracted_building(tmp.path(), 12_345, "G0800130");

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
        write_extracted_building(tmp.path(), 12_345, "G0800130");

        let rows =
            parse_resstock_metadata(&pq, ResStockVersion::V2024_1, tmp.path()).expect("parse");
        let chars = &rows[0].characteristics;
        assert!(!chars.contains_key("bldg_id"));
        assert!(!chars.contains_key("upgrade"));
        assert!(!chars.contains_key("sample_weight"));
    }

    #[test]
    fn v2024_1_paths_use_the_extracted_layout() {
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("test.parquet");
        write_parquet(&pq, &sample_batch());
        write_extracted_building(tmp.path(), 12_345, "G0800130");

        let rows =
            parse_resstock_metadata(&pq, ResStockVersion::V2024_1, tmp.path()).expect("parse");
        let row = &rows[0];
        assert_eq!(row.bldg_id, 12_345);
        assert_eq!(row.upgrade, 3);
        assert_eq!(row.sample_weight, 2.345_678_f64);
        assert_eq!(
            row.hpxml_path,
            tmp.path().join("bldg0012345").join("home.xml")
        );
        assert_eq!(
            row.schedule_path,
            tmp.path().join("bldg0012345").join("in.schedules.csv")
        );
        assert!(
            row.hpxml_path.is_file(),
            "the HPXML path must be a real file"
        );
        assert!(row.weather_path.is_none());
        assert_eq!(row.weather_fips.as_deref(), Some("G0800130"));
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
    fn v2024_2_paths_use_the_extracted_layout() {
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("test.parquet");
        write_parquet(&pq, &sample_batch_v2024_2());
        write_extracted_building(tmp.path(), 12_345, "G0800130");

        let rows =
            parse_resstock_metadata(&pq, ResStockVersion::V2024_2, tmp.path()).expect("parse");
        assert_eq!(
            rows[0].hpxml_path,
            tmp.path().join("bldg0012345").join("home.xml")
        );
        assert_eq!(
            rows[0].schedule_path,
            tmp.path().join("bldg0012345").join("in.schedules.csv")
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
    fn v2025_1_paths_use_the_extracted_layout() {
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("test.parquet");
        write_parquet(&pq, &sample_batch_v2025());
        write_extracted_building(tmp.path(), 12_345, "G0800130");

        let rows =
            parse_resstock_metadata(&pq, ResStockVersion::V2025_1, tmp.path()).expect("parse");
        assert_eq!(
            rows[0].hpxml_path,
            tmp.path().join("bldg0012345").join("home.xml")
        );
        assert_eq!(
            rows[0].schedule_path,
            tmp.path().join("bldg0012345").join("in.schedules.csv")
        );
        assert_eq!(rows[0].weather_fips.as_deref(), Some("G0800130"));
    }

    #[test]
    fn resstock_missing_building_directory_is_an_error() {
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("test.parquet");
        write_parquet(&pq, &sample_batch());

        let err = parse_resstock_metadata(&pq, ResStockVersion::V2024_1, tmp.path())
            .expect_err("no building directory");
        match err {
            ResStockError::MissingLayout {
                bldg_id,
                element,
                path,
            } => {
                assert_eq!(bldg_id, 12_345);
                assert_eq!(element, "the building directory");
                assert_eq!(path, tmp.path().join("bldg0012345"));
            }
            other => panic!("expected MissingLayout, got {other}"),
        }
    }

    #[test]
    fn resstock_a_missing_row_fails_the_parse_naming_that_row() {
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("test.parquet");
        // Two rows: the first building is unpacked, the second is not.
        let schema = Arc::new(Schema::new(vec![
            Field::new("bldg_id", DataType::Int64, false),
            Field::new("upgrade", DataType::Int64, false),
            Field::new("sample_weight", DataType::Float64, false),
            Field::new("in.state", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![12_345, 67_890])) as ArrayRef,
                Arc::new(Int64Array::from(vec![0, 0])) as ArrayRef,
                Arc::new(Float64Array::from(vec![1.0, 2.0])) as ArrayRef,
                Arc::new(StringArray::from(vec!["CO", "WA"])) as ArrayRef,
            ],
        )
        .expect("batch");
        write_parquet(&pq, &batch);
        write_extracted_building(tmp.path(), 12_345, "G0800130");

        let err = parse_resstock_metadata(&pq, ResStockVersion::V2024_1, tmp.path())
            .expect_err("the second building is not unpacked");
        match err {
            ResStockError::MissingLayout { bldg_id, .. } => assert_eq!(bldg_id, 67_890),
            other => panic!("expected MissingLayout for the unpacked row, got {other}"),
        }
    }

    #[test]
    fn v2025_1_metadata_without_state_is_an_error() {
        let tmp = tempdir().expect("tmp");
        let schema = Arc::new(Schema::new(vec![
            Field::new("building_id", DataType::Int64, false),
            Field::new("upgrade_id", DataType::Int64, false),
            Field::new("weight", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![12_345])) as ArrayRef,
                Arc::new(Int64Array::from(vec![0])) as ArrayRef,
                Arc::new(Float64Array::from(vec![1.0])) as ArrayRef,
            ],
        )
        .expect("batch");
        let pq = tmp.path().join("no_state_v2025.parquet");
        write_parquet(&pq, &batch);
        write_extracted_building(tmp.path(), 12_345, "G0800130");

        let err = parse_resstock_metadata(&pq, ResStockVersion::V2025_1, tmp.path())
            .expect_err("no in.state column");
        match err {
            ResStockError::MissingCharacteristic {
                bldg_id,
                characteristic,
            } => {
                assert_eq!(bldg_id, 12_345);
                assert_eq!(characteristic, "in.state");
            }
            other => panic!("expected MissingCharacteristic for in.state, got {other}"),
        }
    }

    #[test]
    fn resstock_missing_building_file_is_an_error() {
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("test.parquet");
        write_parquet(&pq, &sample_batch());
        let dir = extracted_building_dir(tmp.path(), 12_345);
        fs::create_dir_all(&dir).expect("create building dir");
        write_hpxml_fixture(&dir.join("home.xml"), "G0800130");

        let err = parse_resstock_metadata(&pq, ResStockVersion::V2024_1, tmp.path())
            .expect_err("no schedule file");
        match err {
            ResStockError::MissingLayout {
                bldg_id, element, ..
            } => {
                assert_eq!(bldg_id, 12_345);
                assert_eq!(element, "in.schedules.csv");
            }
            other => panic!("expected MissingLayout, got {other}"),
        }
    }

    #[test]
    fn resstock_unreadable_weather_station_is_an_error() {
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("test.parquet");
        write_parquet(&pq, &sample_batch());
        let dir = extracted_building_dir(tmp.path(), 12_345);
        fs::create_dir_all(&dir).expect("create building dir");
        fs::write(dir.join("home.xml"), "<HPXML><Building/></HPXML>").expect("write hpxml");
        fs::write(dir.join("in.schedules.csv"), "occupants\n1\n").expect("write schedules");

        let err = parse_resstock_metadata(&pq, ResStockVersion::V2024_1, tmp.path())
            .expect_err("no weather station name");
        match err {
            ResStockError::WeatherStation {
                bldg_id, source, ..
            } => {
                assert_eq!(bldg_id, 12_345);
                assert!(matches!(source, WeatherStationError::MissingName));
            }
            other => panic!("expected WeatherStation, got {other}"),
        }
    }

    #[test]
    fn resstock_zip_archive_is_a_layout_error_naming_the_fix() {
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("test.parquet");
        write_parquet(&pq, &sample_batch());
        // The dataset-as-distributed shape the constructor must reject: a
        // zip archive where the extracted building directory belongs.
        fs::write(tmp.path().join("bldg0012345.zip"), "not a directory")
            .expect("write zip stand-in");

        let err = parse_resstock_metadata(&pq, ResStockVersion::V2024_1, tmp.path())
            .expect_err("zip archive instead of the unpacked building");
        let message = err.to_string();
        assert!(
            message.contains("12345"),
            "must name the building: {message}"
        );
        assert!(
            message.contains("unpacked buildings directory"),
            "must name the fix: {message}"
        );
        assert!(
            message.contains("zip"),
            "must name the zip mistake: {message}"
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
            ResStockError::MissingCharacteristic {
                bldg_id,
                characteristic,
            } => {
                assert_eq!(bldg_id, 1);
                assert_eq!(characteristic, "in.state");
            }
            other => panic!("expected MissingCharacteristic for in.state, got {other}"),
        }
    }

    #[test]
    fn characteristics_midpoint_for_range() {
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("test.parquet");
        write_parquet(&pq, &sample_batch());
        write_extracted_building(tmp.path(), 12_345, "G0800130");

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
        write_extracted_building(tmp.path(), 12_345, "G0800130");

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
        let tmp = tempdir().expect("tmp");
        let err = parse_resstock_metadata(
            &tmp.path().join("file.parquet"),
            ResStockVersion::V2024_1,
            tmp.path(),
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

    /// Write one building in the extracted layout the constructor requires:
    /// `root/bldg{bldg_id:07}/home.xml` and `in.schedules.csv`.
    fn write_extracted_building(root: &Path, bldg_id: i64, weather_station_name: &str) {
        let dir = extracted_building_dir(root, bldg_id);
        fs::create_dir_all(&dir).expect("create building dir");
        write_hpxml_fixture(&dir.join("home.xml"), weather_station_name);
        fs::write(dir.join("in.schedules.csv"), "occupants\n1\n").expect("write schedules");
    }

    #[test]
    fn parse_weather_station_fips_extracts_valid_fips() {
        let tmp = tempdir().expect("tmp");
        let hpxml_path = tmp.path().join("home.xml");
        write_hpxml_fixture(&hpxml_path, "G0800130");
        let fips = parse_weather_station_fips(&hpxml_path);
        assert_eq!(fips, Ok("G0800130".to_string()));
    }

    #[test]
    fn parse_weather_station_fips_rejects_non_fips_name() {
        let tmp = tempdir().expect("tmp");
        let hpxml_path = tmp.path().join("home.xml");
        write_hpxml_fixture(&hpxml_path, "USA_CO_Denver");
        let err = parse_weather_station_fips(&hpxml_path).expect_err("non-FIPS name");
        assert_eq!(
            err.to_string(),
            "the WeatherStation/Name value `USA_CO_Denver` is not a county FIPS code \
             (G followed by digits)"
        );
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
        let err = parse_weather_station_fips(&hpxml_path).expect_err("no weather station");
        assert!(matches!(err, WeatherStationError::MissingName));
    }

    #[test]
    fn parse_weather_station_fips_handles_missing_hpxml_file() {
        let err =
            parse_weather_station_fips(Path::new("/nonexistent/hpxml.xml")).expect_err("no file");
        assert!(matches!(err, WeatherStationError::Unreadable(_)));
    }

    fn nan_weight_batch() -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("bldg_id", DataType::Int64, false),
            Field::new("upgrade", DataType::Int64, false),
            Field::new("sample_weight", DataType::Float64, false),
            Field::new("in.state", DataType::Utf8, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![1])) as ArrayRef,
                Arc::new(Int64Array::from(vec![0])) as ArrayRef,
                Arc::new(Float64Array::from(vec![f64::NAN])) as ArrayRef,
                Arc::new(StringArray::from(vec!["CO"])) as ArrayRef,
            ],
        )
        .expect("batch")
    }

    #[test]
    fn reject_nan_sample_weight() {
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("nan_weight.parquet");
        write_parquet(&pq, &nan_weight_batch());

        let err = parse_resstock_metadata(&pq, ResStockVersion::V2024_1, tmp.path()).unwrap_err();
        match err {
            ResStockError::InvalidSampleWeight { bldg_id, value } => {
                assert_eq!(bldg_id, 1);
                assert!(value.is_nan());
            }
            other => panic!("expected InvalidSampleWeight, got {other}"),
        }
    }

    fn negative_weight_batch() -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("bldg_id", DataType::Int64, false),
            Field::new("upgrade", DataType::Int64, false),
            Field::new("sample_weight", DataType::Float64, false),
            Field::new("in.state", DataType::Utf8, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![2])) as ArrayRef,
                Arc::new(Int64Array::from(vec![0])) as ArrayRef,
                Arc::new(Float64Array::from(vec![-1.0])) as ArrayRef,
                Arc::new(StringArray::from(vec!["CO"])) as ArrayRef,
            ],
        )
        .expect("batch")
    }

    #[test]
    fn reject_negative_sample_weight() {
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("neg_weight.parquet");
        write_parquet(&pq, &negative_weight_batch());

        let err = parse_resstock_metadata(&pq, ResStockVersion::V2024_1, tmp.path()).unwrap_err();
        match err {
            ResStockError::InvalidSampleWeight { bldg_id, value } => {
                assert_eq!(bldg_id, 2);
                assert_eq!(value, -1.0);
            }
            other => panic!("expected InvalidSampleWeight, got {other}"),
        }
    }

    fn infinite_weight_batch() -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("bldg_id", DataType::Int64, false),
            Field::new("upgrade", DataType::Int64, false),
            Field::new("sample_weight", DataType::Float64, false),
            Field::new("in.state", DataType::Utf8, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![3])) as ArrayRef,
                Arc::new(Int64Array::from(vec![0])) as ArrayRef,
                Arc::new(Float64Array::from(vec![f64::INFINITY])) as ArrayRef,
                Arc::new(StringArray::from(vec!["CO"])) as ArrayRef,
            ],
        )
        .expect("batch")
    }

    #[test]
    fn reject_infinite_sample_weight() {
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("inf_weight.parquet");
        write_parquet(&pq, &infinite_weight_batch());

        let err = parse_resstock_metadata(&pq, ResStockVersion::V2024_1, tmp.path()).unwrap_err();
        match err {
            ResStockError::InvalidSampleWeight { bldg_id, value } => {
                assert_eq!(bldg_id, 3);
                assert!(value.is_infinite());
            }
            other => panic!("expected InvalidSampleWeight, got {other}"),
        }
    }

    fn zero_weight_batch() -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("bldg_id", DataType::Int64, false),
            Field::new("upgrade", DataType::Int64, false),
            Field::new("sample_weight", DataType::Float64, false),
            Field::new("in.state", DataType::Utf8, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![4])) as ArrayRef,
                Arc::new(Int64Array::from(vec![0])) as ArrayRef,
                Arc::new(Float64Array::from(vec![0.0])) as ArrayRef,
                Arc::new(StringArray::from(vec!["CO"])) as ArrayRef,
            ],
        )
        .expect("batch")
    }

    #[test]
    fn zero_weight_allowed() {
        let tmp = tempdir().expect("tmp");
        let pq = tmp.path().join("zero_weight.parquet");
        write_parquet(&pq, &zero_weight_batch());
        write_extracted_building(tmp.path(), 4, "G0800130");

        let rows =
            parse_resstock_metadata(&pq, ResStockVersion::V2024_1, tmp.path()).expect("parse");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].bldg_id, 4);
        assert_eq!(rows[0].sample_weight, 0.0);
    }
}
