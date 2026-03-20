//! Schedule CSV parser and time-series interpolation.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use chrono::{DateTime, Duration, FixedOffset, NaiveDateTime, TimeZone};
use thiserror::Error;

use crate::weather::WeatherMeta;

/// Error type for schedule CSV parsing and access operations.
#[derive(Debug, Error)]
pub enum ScheduleError {
    #[error("io error reading `{path}`: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("schedule parse error: {0}")]
    Parse(String),
    #[error("schedule validation error: {0}")]
    Validation(String),
    #[error("required columns missing: {missing:?}")]
    MissingRequiredColumns { missing: Vec<String> },
    #[error("column not found: `{column}`")]
    ColumnNotFound { column: String },
    #[error("index out of range for `{column}`: index={index}, len={len}")]
    IndexOutOfRange {
        column: String,
        index: usize,
        len: usize,
    },
    #[error("schedule resampling error: {0}")]
    Resample(String),
    #[error("schedule coverage error: {0}")]
    Coverage(String),
}

/// How to aggregate values when downsampling a schedule column.
///
/// `Mean` is the safe default for rates, temperatures, and setpoints.
/// `Sum` should be used for energy or power-hour columns where the integral
/// must be preserved across the resampling boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColumnAggregation {
    Mean,
    Sum,
}

/// Column-major schedule time series.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleTimeSeries {
    pub timestamps: Vec<DateTime<FixedOffset>>,
    pub column_names: Vec<String>,
    pub columns: Vec<Vec<f64>>, // outer index = column, inner index = timestep
    pub column_index: HashMap<String, usize>,
    pub source_step_secs: u32,
    /// Per-column aggregation strategy used when downsampling.
    /// Parallel to `column_names`; defaults to `Mean` for all columns.
    pub column_aggregations: Vec<ColumnAggregation>,
}

impl ScheduleTimeSeries {
    /// Returns the number of timesteps.
    #[must_use]
    pub fn len(&self) -> usize {
        self.timestamps.len()
    }

    /// Returns true if there are no rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.timestamps.is_empty()
    }

    /// Get a scalar schedule value by column name and timestep index.
    pub fn get_value(
        &self,
        column_name: &str,
        timestep_index: usize,
    ) -> Result<f64, ScheduleError> {
        let normalized = normalize_column_name(column_name);
        let &col_idx =
            self.column_index
                .get(&normalized)
                .ok_or_else(|| ScheduleError::ColumnNotFound {
                    column: normalized.clone(),
                })?;

        let col = &self.columns[col_idx];
        if timestep_index >= col.len() {
            return Err(ScheduleError::IndexOutOfRange {
                column: normalized,
                index: timestep_index,
                len: col.len(),
            });
        }

        Ok(col[timestep_index])
    }

    /// Append a new column to the schedule. The data length must match `self.len()`.
    pub fn add_column(
        &mut self,
        name: &str,
        data: Vec<f64>,
        aggregation: ColumnAggregation,
    ) -> Result<(), ScheduleError> {
        if data.len() != self.timestamps.len() {
            return Err(ScheduleError::Coverage(format!(
                "column '{}' has {} values but schedule has {} timesteps",
                name,
                data.len(),
                self.timestamps.len()
            )));
        }
        let normalized = normalize_column_name(name);
        if self.column_index.contains_key(&normalized) {
            return Err(ScheduleError::Validation(format!(
                "column '{}' already exists",
                normalized
            )));
        }
        let idx = self.columns.len();
        self.columns.push(data);
        self.column_names.push(normalized.clone());
        self.column_index.insert(normalized, idx);
        self.column_aggregations.push(aggregation);
        Ok(())
    }

    /// Append a derived column and return its column index.
    ///
    /// If `name` already exists with identical values and aggregation, the existing
    /// index is returned. If the name exists with different data, a suffixed name is used.
    pub fn append_derived_column(
        &mut self,
        name: &str,
        data: Vec<f64>,
        aggregation: ColumnAggregation,
    ) -> Result<usize, ScheduleError> {
        if data.len() != self.timestamps.len() {
            return Err(ScheduleError::Coverage(format!(
                "column '{}' has {} values but schedule has {} timesteps",
                name,
                data.len(),
                self.timestamps.len()
            )));
        }

        let normalized = normalize_column_name(name);
        if let Some(&existing_idx) = self.column_index.get(&normalized) {
            if self.columns.get(existing_idx) == Some(&data)
                && self.column_aggregations.get(existing_idx) == Some(&aggregation)
            {
                return Ok(existing_idx);
            }
        }

        let mut candidate = normalized.clone();
        let mut suffix = 1usize;
        while self.column_index.contains_key(&candidate) {
            candidate = format!("{normalized}__derived_{suffix}");
            suffix = suffix.saturating_add(1);
        }

        let idx = self.columns.len();
        self.columns.push(data);
        self.column_names.push(candidate.clone());
        self.column_index.insert(candidate, idx);
        self.column_aggregations.push(aggregation);
        Ok(idx)
    }

    /// Resample to `target_step_secs`.
    ///
    /// - **Upsampling** (`target_step_secs < source_step_secs`): zero-order hold — each source
    ///   value is repeated for the sub-steps it covers.
    /// - **Downsampling** (`target_step_secs > source_step_secs`): groups of consecutive source
    ///   values are reduced per [`ColumnAggregation`]: averaged for `Mean` columns, summed for
    ///   `Sum` columns.
    /// - **No-op** (`target_step_secs == source_step_secs`): returns a clone.
    ///
    /// In both cases `target_step_secs` must be an integer multiple (or divisor) of
    /// `source_step_secs`.
    pub fn resample(&self, target_step_secs: u32) -> Result<ScheduleTimeSeries, ScheduleError> {
        if target_step_secs == 0 {
            return Err(ScheduleError::Resample(
                "target_step_secs must be > 0".to_string(),
            ));
        }

        if target_step_secs == self.source_step_secs {
            return Ok(self.clone());
        }

        if target_step_secs < self.source_step_secs {
            // Upsampling: target is finer than source — zero-order hold.
            if !self.source_step_secs.is_multiple_of(target_step_secs) {
                return Err(ScheduleError::Resample(format!(
                    "incompatible timestep: source_step_secs ({}) is not an integer multiple of target_step_secs ({target_step_secs})",
                    self.source_step_secs
                )));
            }

            let repeat = (self.source_step_secs / target_step_secs) as usize;
            let step = Duration::seconds(i64::from(target_step_secs));

            let mut timestamps = Vec::with_capacity(self.timestamps.len() * repeat);
            for &ts in &self.timestamps {
                for i in 0..repeat {
                    timestamps.push(ts + step * i as i32);
                }
            }

            let mut columns = Vec::with_capacity(self.columns.len());
            for col in &self.columns {
                let mut out = Vec::with_capacity(col.len() * repeat);
                for &value in col {
                    out.extend(std::iter::repeat_n(value, repeat));
                }
                columns.push(out);
            }

            Ok(ScheduleTimeSeries {
                timestamps,
                column_names: self.column_names.clone(),
                columns,
                column_index: self.column_index.clone(),
                source_step_secs: target_step_secs,
                column_aggregations: self.column_aggregations.clone(),
            })
        } else {
            // Downsampling: target is coarser than source — aggregate groups.
            if !target_step_secs.is_multiple_of(self.source_step_secs) {
                return Err(ScheduleError::Resample(format!(
                    "incompatible timestep: target_step_secs ({target_step_secs}) is not an integer multiple of source_step_secs ({})",
                    self.source_step_secs
                )));
            }

            let factor = (target_step_secs / self.source_step_secs) as usize;
            let out_len = self.timestamps.len() / factor;

            if out_len == 0 {
                return Err(ScheduleError::Resample(format!(
                    "downsampling by factor {factor} would produce an empty series (source length {})",
                    self.timestamps.len()
                )));
            }

            // Take every `factor`-th timestamp (the first of each group).
            let timestamps: Vec<DateTime<FixedOffset>> = self
                .timestamps
                .chunks(factor)
                .take(out_len)
                .map(|chunk| chunk[0])
                .collect();

            let mut columns = Vec::with_capacity(self.columns.len());
            for (col, &agg) in self.columns.iter().zip(self.column_aggregations.iter()) {
                let out: Vec<f64> = col
                    .chunks(factor)
                    .take(out_len)
                    .map(|chunk| match agg {
                        ColumnAggregation::Mean => chunk.iter().sum::<f64>() / chunk.len() as f64,
                        ColumnAggregation::Sum => chunk.iter().sum::<f64>(),
                    })
                    .collect();
                columns.push(out);
            }

            Ok(ScheduleTimeSeries {
                timestamps,
                column_names: self.column_names.clone(),
                columns,
                column_index: self.column_index.clone(),
                source_step_secs: target_step_secs,
                column_aggregations: self.column_aggregations.clone(),
            })
        }
    }
}

/// Parse schedule CSV into a column-major time series.
///
/// `required_columns` are validated after normalization.
///
/// If `coverage_period` is provided, the parsed schedule must fully cover
/// `[simulation_start, simulation_end)`.
pub fn parse_schedule_csv<P: AsRef<Path>>(
    path: P,
    required_columns: &[&str],
    weather_meta: Option<&WeatherMeta>,
    coverage_period: Option<(DateTime<FixedOffset>, DateTime<FixedOffset>)>,
) -> Result<ScheduleTimeSeries, ScheduleError> {
    let path_ref = path.as_ref();
    let contents = fs::read_to_string(path_ref).map_err(|source| ScheduleError::Io {
        path: path_ref.display().to_string(),
        source,
    })?;

    parse_schedule_csv_str(&contents, required_columns, weather_meta, coverage_period)
}

fn parse_schedule_csv_str(
    contents: &str,
    required_columns: &[&str],
    weather_meta: Option<&WeatherMeta>,
    coverage_period: Option<(DateTime<FixedOffset>, DateTime<FixedOffset>)>,
) -> Result<ScheduleTimeSeries, ScheduleError> {
    let mut lines = contents.lines().filter(|line| !line.trim().is_empty());

    let header_line = lines
        .next()
        .ok_or_else(|| ScheduleError::Parse("schedule CSV is empty".to_string()))?;
    let headers = parse_csv_line(header_line)?;
    if headers.is_empty() {
        return Err(ScheduleError::Parse(
            "schedule CSV header has no columns".to_string(),
        ));
    }

    let time_col_idx = detect_time_column(&headers);
    let is_index_based = time_col_idx.is_none();

    if !is_index_based && headers.len() < 2 {
        return Err(ScheduleError::Parse(
            "schedule CSV must contain at least one time column and one data column".to_string(),
        ));
    }

    let fallback_offset = weather_meta
        .map(weather_timezone_offset)
        .transpose()?
        .flatten();

    let mut data_column_names = Vec::new();
    let mut source_col_indices = Vec::new();
    for (idx, header) in headers.iter().enumerate() {
        if Some(idx) == time_col_idx {
            continue;
        }
        data_column_names.push(normalize_column_name(header));
        source_col_indices.push(idx);
    }

    let mut missing_required = Vec::new();
    for required in required_columns {
        let normalized = normalize_column_name(required);
        if !data_column_names.iter().any(|name| name == &normalized) {
            missing_required.push(normalized);
        }
    }
    if !missing_required.is_empty() {
        return Err(ScheduleError::MissingRequiredColumns {
            missing: missing_required,
        });
    }

    let mut timestamps = Vec::new();
    let mut columns = vec![Vec::new(); data_column_names.len()];

    for (line_no, line) in lines.enumerate() {
        let row = line_no + 2;
        let fields = parse_csv_line(line)?;
        if fields.len() != headers.len() {
            return Err(ScheduleError::Parse(format!(
                "row {row}: expected {} columns, found {}",
                headers.len(),
                fields.len()
            )));
        }

        if let Some(tc) = time_col_idx {
            let timestamp_raw = fields[tc].trim();
            let timestamp = parse_timestamp(timestamp_raw, fallback_offset).map_err(|msg| {
                ScheduleError::Parse(format!(
                    "row {row}: failed to parse timestamp `{timestamp_raw}`: {msg}"
                ))
            })?;
            timestamps.push(timestamp);
        }

        for (out_col_idx, src_col_idx) in source_col_indices.iter().enumerate() {
            let raw = fields[*src_col_idx].trim();
            let value = raw.parse::<f64>().map_err(|_| {
                ScheduleError::Parse(format!(
                    "row {row}: failed to parse value `{raw}` in column `{}` as f64",
                    data_column_names[out_col_idx]
                ))
            })?;
            columns[out_col_idx].push(value);
        }
    }

    let num_data_rows = columns.first().map_or(0, |c| c.len());
    if num_data_rows == 0 {
        return Err(ScheduleError::Parse(
            "schedule CSV contains no data rows".to_string(),
        ));
    }

    let source_step_secs;
    if is_index_based {
        // Index-based schedule (no timestamp column): generate synthetic
        // timestamps from row count, matching OCHRE's set_annual_index.
        source_step_secs = infer_index_step_secs(num_data_rows)?;
        timestamps = generate_annual_timestamps(num_data_rows, source_step_secs, weather_meta)?;
    } else {
        source_step_secs = infer_step_secs(&timestamps)?;
    }

    let mut column_index = HashMap::with_capacity(data_column_names.len());
    for (idx, name) in data_column_names.iter().enumerate() {
        column_index.insert(name.clone(), idx);
    }

    let num_columns = data_column_names.len();
    let series = ScheduleTimeSeries {
        timestamps,
        column_names: data_column_names,
        columns,
        column_index,
        source_step_secs,
        column_aggregations: vec![ColumnAggregation::Mean; num_columns],
    };

    if let Some((simulation_start, simulation_end)) = coverage_period {
        validate_coverage(&series, simulation_start, simulation_end)?;
    }

    Ok(series)
}

fn detect_time_column(headers: &[String]) -> Option<usize> {
    headers.iter().position(|h| {
        let trimmed = h.trim();
        trimmed.eq_ignore_ascii_case("timestamp") || trimmed.eq_ignore_ascii_case("time")
    })
}

/// Infer step size from an index-based (no-timestamp) annual schedule.
///
/// Mirrors OCHRE's `set_annual_index`: the row count must evenly divide
/// 525600 minutes (365 days). Returns the step size in seconds.
fn infer_index_step_secs(num_rows: usize) -> Result<u32, ScheduleError> {
    const MINUTES_PER_YEAR: u64 = 525_600; // 365 * 24 * 60

    if num_rows == 0 {
        return Err(ScheduleError::Parse(
            "index-based schedule has no data rows".to_string(),
        ));
    }

    let n = num_rows as u64;

    // Check if n divides 8760 evenly and 525600 divides by n evenly
    // (matching OCHRE: `n % 8760 == 0 and 525600 % n == 0`)
    if n.is_multiple_of(8760) && MINUTES_PER_YEAR.is_multiple_of(n) {
        let step_minutes = MINUTES_PER_YEAR / n;
        return Ok((step_minutes * 60) as u32);
    }

    // OCHRE also allows n == rows+1 (inclusive end): `n % 8760 == 1`
    let n_minus_1 = n.saturating_sub(1);
    if n_minus_1 > 0 && n_minus_1.is_multiple_of(8760) && MINUTES_PER_YEAR.is_multiple_of(n_minus_1)
    {
        let step_minutes = MINUTES_PER_YEAR / n_minus_1;
        return Ok((step_minutes * 60) as u32);
    }

    Err(ScheduleError::Parse(format!(
        "index-based schedule has {num_rows} rows which is not compatible with annual data \
         (must evenly divide 525600 minutes)"
    )))
}

/// Generate synthetic annual timestamps for an index-based schedule.
fn generate_annual_timestamps(
    num_rows: usize,
    step_secs: u32,
    weather_meta: Option<&WeatherMeta>,
) -> Result<Vec<DateTime<FixedOffset>>, ScheduleError> {
    let offset = weather_meta
        .map(weather_timezone_offset)
        .transpose()?
        .flatten()
        .unwrap_or(FixedOffset::east_opt(0).expect("UTC offset is valid"));

    // Index-based schedules are cyclic annual data — the exact year does not
    // affect simulation results.  We use 2007 (a non-leap year consistent with
    // the BEopt default CalendarYear) so generated timestamps never land on
    // Feb 29.
    let year: i32 = 2007;
    let start = offset
        .with_ymd_and_hms(year, 1, 1, 0, 0, 0)
        .single()
        .ok_or_else(|| {
            ScheduleError::Validation(format!(
                "cannot construct start timestamp for year {year} with offset {offset}"
            ))
        })?;

    let step = Duration::seconds(i64::from(step_secs));
    let timestamps: Vec<_> = (0..num_rows).map(|i| start + step * i as i32).collect();

    Ok(timestamps)
}

fn weather_timezone_offset(meta: &WeatherMeta) -> Result<Option<FixedOffset>, ScheduleError> {
    let secs = (meta.timezone_offset_h * 3600.0).round() as i32;
    if secs.unsigned_abs() > 86_399 {
        return Err(ScheduleError::Validation(format!(
            "WeatherMeta.timezone_offset_h out of range: {}",
            meta.timezone_offset_h
        )));
    }
    Ok(FixedOffset::east_opt(secs))
}

fn infer_step_secs(timestamps: &[DateTime<FixedOffset>]) -> Result<u32, ScheduleError> {
    if timestamps.len() < 2 {
        return Err(ScheduleError::Validation(
            "schedule must contain at least two rows to infer timestep".to_string(),
        ));
    }

    let first_delta = timestamps[1] - timestamps[0];
    let step_secs = first_delta.num_seconds();
    if step_secs <= 0 {
        return Err(ScheduleError::Validation(format!(
            "non-positive timestep detected: {step_secs}s"
        )));
    }

    for i in 1..(timestamps.len() - 1) {
        let delta = (timestamps[i + 1] - timestamps[i]).num_seconds();
        if delta != step_secs {
            return Err(ScheduleError::Validation(format!(
                "non-uniform timestep at row {}: expected {step_secs}s, found {delta}s",
                i + 2
            )));
        }
    }

    u32::try_from(step_secs).map_err(|_| {
        ScheduleError::Parse(format!("schedule step size {step_secs}s exceeds u32 range"))
    })
}

fn validate_coverage(
    series: &ScheduleTimeSeries,
    simulation_start: DateTime<FixedOffset>,
    simulation_end: DateTime<FixedOffset>,
) -> Result<(), ScheduleError> {
    if simulation_end <= simulation_start {
        return Err(ScheduleError::Coverage(
            "simulation_end must be after simulation_start".to_string(),
        ));
    }

    let schedule_start = series.timestamps[0];
    let schedule_end_exclusive = *series
        .timestamps
        .last()
        .expect("series timestamps are non-empty")
        + Duration::seconds(i64::from(series.source_step_secs));

    let mut gaps = Vec::new();
    if simulation_start < schedule_start {
        gaps.push(format!(
            "missing start coverage: [{simulation_start}, {schedule_start})"
        ));
    }
    if simulation_end > schedule_end_exclusive {
        gaps.push(format!(
            "missing end coverage: [{schedule_end_exclusive}, {simulation_end})"
        ));
    }

    if gaps.is_empty() {
        Ok(())
    } else {
        Err(ScheduleError::Coverage(gaps.join("; ")))
    }
}

fn parse_timestamp(
    raw: &str,
    fallback_offset: Option<FixedOffset>,
) -> Result<DateTime<FixedOffset>, String> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Ok(dt);
    }

    for fmt in [
        "%Y-%m-%d %H:%M:%S%:z",
        "%Y-%m-%d %H:%M%:z",
        "%Y-%m-%d %H:%M:%S %:z",
        "%Y-%m-%d %H:%M %:z",
        "%Y-%m-%d %H:%M:%S%z",
        "%Y-%m-%d %H:%M%z",
    ] {
        if let Ok(dt) = DateTime::parse_from_str(raw, fmt) {
            return Ok(dt);
        }
    }

    for fmt in [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%m/%d/%Y %H:%M:%S",
        "%m/%d/%Y %H:%M",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(raw, fmt) {
            let offset = fallback_offset.ok_or_else(|| {
                "timestamp has no timezone and no WeatherMeta fallback was provided".to_string()
            })?;
            let local = offset
                .from_local_datetime(&naive)
                .single()
                .ok_or_else(|| "failed to localize timestamp with fallback timezone".to_string())?;
            return Ok(local);
        }
    }

    Err("unrecognized timestamp format".to_string())
}

pub(crate) fn normalize_column_name(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_csv_line(line: &str) -> Result<Vec<String>, ScheduleError> {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                if in_quotes && matches!(chars.peek(), Some('"')) {
                    current.push('"');
                    let _ = chars.next();
                } else {
                    in_quotes = !in_quotes;
                }
            }
            ',' if !in_quotes => {
                values.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    if in_quotes {
        return Err(ScheduleError::Parse(
            "unterminated quoted field in CSV line".to_string(),
        ));
    }

    values.push(current.trim().to_string());
    Ok(values)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use chrono::{DateTime, FixedOffset};

    use super::{
        ColumnAggregation, ScheduleError, ScheduleTimeSeries, parse_schedule_csv,
        parse_schedule_csv_str,
    };
    use crate::weather::WeatherMeta;

    fn write_temp_csv(csv_contents: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before UNIX_EPOCH")
            .as_nanos();
        path.push(format!("hares-io-schedule-test-{nanos}.csv"));
        fs::write(&path, csv_contents).expect("failed to write temporary schedule CSV");
        path
    }

    fn schedule_csv_15min_with_tz() -> String {
        [
            "Time,Clothes Washer  (kW),HVAC Heating (C)",
            "2021-01-01T00:00:00-07:00,0.1,20.0",
            "2021-01-01T00:15:00-07:00,0.2,20.5",
            "2021-01-01T00:30:00-07:00,0.3,21.0",
            "2021-01-01T00:45:00-07:00,0.4,21.5",
        ]
        .join("\n")
    }

    fn weather_meta() -> WeatherMeta {
        WeatherMeta {
            location: "Test".to_string(),
            latitude: 39.7,
            longitude: -105.0,
            timezone_offset_h: -7.0,
            elevation_m: 1600.0,
        }
    }

    fn parse_fixed(ts: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(ts).expect("valid RFC3339 timestamp")
    }

    #[test]
    fn parses_rows_and_normalizes_column_names() {
        let csv = schedule_csv_15min_with_tz();
        let series = parse_schedule_csv_str(
            &csv,
            &["Clothes Washer (kW)", "HVAC Heating (C)"],
            None,
            None,
        )
        .expect("schedule should parse");

        assert_eq!(series.len(), 4);
        assert_eq!(series.column_names[0], "Clothes Washer (kW)");
        assert_eq!(series.column_names[1], "HVAC Heating (C)");
        assert_eq!(series.source_step_secs, 900);
    }

    #[test]
    fn resample_from_15min_to_60s_is_zoh() {
        let csv = schedule_csv_15min_with_tz();
        let series = parse_schedule_csv_str(&csv, &[], None, None).expect("schedule should parse");
        let upsampled = series.resample(60).expect("upsample should work");

        assert_eq!(upsampled.len(), 60);
        assert!(
            upsampled.columns[0]
                .iter()
                .take(15)
                .all(|v| (*v - 0.1).abs() < 1e-9)
        );
        assert!(
            upsampled.columns[0]
                .iter()
                .skip(15)
                .take(15)
                .all(|v| (*v - 0.2).abs() < 1e-9)
        );
    }

    /// Build a 1-minute schedule with 10 rows.
    /// Column 0 (index 0): temperature-like values  [10.0, 20.0, 30.0, 40.0, 50.0, ...]
    /// Column 1 (index 1): energy-like values        [1.0, 2.0, 3.0, 4.0, 5.0, ...]
    fn schedule_csv_1min_10rows() -> String {
        let mut lines = vec!["Time,Temp (C),Energy (kWh)".to_string()];
        for i in 0..10_u32 {
            let mins = i;
            lines.push(format!(
                "2021-01-01T00:{:02}:00-07:00,{:.1},{:.1}",
                mins,
                (i + 1) as f64 * 10.0,
                (i + 1) as f64,
            ));
        }
        lines.join("\n")
    }

    #[test]
    fn resample_downsamples_1min_to_5min_mean() {
        let csv = schedule_csv_1min_10rows();
        let series = parse_schedule_csv_str(&csv, &[], None, None).expect("schedule should parse");
        assert_eq!(series.source_step_secs, 60);

        let downsampled = series
            .resample(300)
            .expect("5-minute downsample should work");

        assert_eq!(downsampled.source_step_secs, 300);
        assert_eq!(downsampled.len(), 2);

        // Group 0: rows with values 10, 20, 30, 40, 50 → mean = 30
        // Group 1: rows with values 60, 70, 80, 90, 100 → mean = 80
        let temp_col = &downsampled.columns[0];
        assert!(
            (temp_col[0] - 30.0).abs() < 1e-9,
            "expected mean 30.0, got {}",
            temp_col[0]
        );
        assert!(
            (temp_col[1] - 80.0).abs() < 1e-9,
            "expected mean 80.0, got {}",
            temp_col[1]
        );
    }

    #[test]
    fn resample_downsamples_1min_to_5min_sum() {
        let csv = schedule_csv_1min_10rows();
        let mut series =
            parse_schedule_csv_str(&csv, &[], None, None).expect("schedule should parse");
        // Mark column 1 (Energy) as Sum.
        series.column_aggregations[1] = ColumnAggregation::Sum;

        let downsampled = series
            .resample(300)
            .expect("5-minute downsample should work");

        // Group 0: 1 + 2 + 3 + 4 + 5 = 15
        // Group 1: 6 + 7 + 8 + 9 + 10 = 40
        let energy_col = &downsampled.columns[1];
        assert!(
            (energy_col[0] - 15.0).abs() < 1e-9,
            "expected sum 15.0, got {}",
            energy_col[0]
        );
        assert!(
            (energy_col[1] - 40.0).abs() < 1e-9,
            "expected sum 40.0, got {}",
            energy_col[1]
        );
    }

    #[test]
    fn resample_downsamples_preserves_total_energy() {
        let csv = schedule_csv_1min_10rows();
        let mut series =
            parse_schedule_csv_str(&csv, &[], None, None).expect("schedule should parse");
        series.column_aggregations[1] = ColumnAggregation::Sum;

        let original_sum: f64 = series.columns[1].iter().sum();
        let downsampled = series
            .resample(300)
            .expect("5-minute downsample should work");
        let downsampled_sum: f64 = downsampled.columns[1].iter().sum();

        assert!(
            (original_sum - downsampled_sum).abs() < 1e-9,
            "total energy should be preserved: original={original_sum}, downsampled={downsampled_sum}"
        );
    }

    #[test]
    fn resample_downsamples_preserves_mean() {
        let csv = schedule_csv_1min_10rows();
        let series = parse_schedule_csv_str(&csv, &[], None, None).expect("schedule should parse");

        let original_mean: f64 =
            series.columns[0].iter().sum::<f64>() / series.columns[0].len() as f64;
        let downsampled = series
            .resample(300)
            .expect("5-minute downsample should work");
        let downsampled_mean: f64 =
            downsampled.columns[0].iter().sum::<f64>() / downsampled.columns[0].len() as f64;

        assert!(
            (original_mean - downsampled_mean).abs() < 1e-9,
            "mean of Mean column should be preserved: original={original_mean}, downsampled={downsampled_mean}"
        );
    }

    #[test]
    fn resample_rejects_non_multiple_downsample() {
        let csv = schedule_csv_15min_with_tz();
        let series = parse_schedule_csv_str(&csv, &[], None, None).expect("schedule should parse");
        // 900s source, 1800s target is a valid multiple, but 1000s is not.
        let err = series
            .resample(1000)
            .expect_err("non-multiple downsample should be rejected");

        assert!(matches!(err, ScheduleError::Resample(_)));
    }

    #[test]
    fn missing_required_column_returns_typed_error() {
        let csv = schedule_csv_15min_with_tz();
        let err = parse_schedule_csv_str(&csv, &["Not Present (kW)"], None, None)
            .expect_err("missing required column should fail");

        match err {
            ScheduleError::MissingRequiredColumns { missing } => {
                assert_eq!(missing, vec!["Not Present (kW)"])
            }
            other => panic!("expected MissingRequiredColumns, got {other}"),
        }
    }

    #[test]
    fn coverage_gap_is_reported() {
        let csv = schedule_csv_15min_with_tz();
        let start = parse_fixed("2020-12-31T23:45:00-07:00");
        let end = parse_fixed("2021-01-01T01:15:00-07:00");

        let err = parse_schedule_csv_str(&csv, &[], None, Some((start, end)))
            .expect_err("coverage gap should fail");
        assert!(matches!(err, ScheduleError::Coverage(_)));
        assert!(err.to_string().contains("missing start coverage"));
        assert!(err.to_string().contains("missing end coverage"));
    }

    #[test]
    fn timezone_fallback_from_weather_meta() {
        let csv = [
            "timestamp,Load (kW)",
            "2021-01-01 00:00:00,1.0",
            "2021-01-01 00:15:00,2.0",
        ]
        .join("\n");

        let series = parse_schedule_csv_str(&csv, &[], Some(&weather_meta()), None)
            .expect("weather timezone fallback should parse");
        let first = series.timestamps[0];
        assert_eq!(first.offset().local_minus_utc(), -7 * 3600);
    }

    #[test]
    fn timezone_required_when_timestamp_has_no_offset() {
        let csv = [
            "timestamp,Load (kW)",
            "2021-01-01 00:00:00,1.0",
            "2021-01-01 00:15:00,2.0",
        ]
        .join("\n");

        let err = parse_schedule_csv_str(&csv, &[], None, None)
            .expect_err("missing timezone info should fail");
        assert!(
            err.to_string()
                .contains("no WeatherMeta fallback was provided")
        );
    }

    #[test]
    fn get_value_reports_missing_column_and_bad_index() {
        let csv = schedule_csv_15min_with_tz();
        let series: ScheduleTimeSeries =
            parse_schedule_csv_str(&csv, &[], None, None).expect("schedule should parse");

        let missing = series
            .get_value("Not There (kW)", 0)
            .expect_err("missing column should error");
        assert!(matches!(missing, ScheduleError::ColumnNotFound { .. }));

        let oob = series
            .get_value("Clothes Washer (kW)", 10)
            .expect_err("oob index should error");
        assert!(matches!(oob, ScheduleError::IndexOutOfRange { .. }));
    }

    #[test]
    fn parse_schedule_csv_reads_from_path() {
        let csv = schedule_csv_15min_with_tz();
        let path = write_temp_csv(&csv);

        let result = parse_schedule_csv(&path, &[], None, None);
        let _ = fs::remove_file(path);

        assert!(result.is_ok());
    }
}
