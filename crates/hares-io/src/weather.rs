//! Weather data processing and time-series handling.

use std::io::BufRead;
use std::path::Path;

use chrono::Datelike;
use serde::{Deserialize, Serialize};
use thiserror::Error;

// Re-export from hares-types for use in this crate's fallback logic.
use hares_types::DEFAULT_GROUND_ALBEDO;

use crate::epw::DesignConditions;
use crate::epw::{SkyTempModel, compute_sky_temp_c};

/// Supported weather file formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeatherFormat {
    /// EnergyPlus Weather file (.epw), hourly data.
    Epw,
    /// NREL NSRDB PSM3 file (.csv), SAM CSV format at 5/15/30/60-min resolution.
    /// Reference: <https://developer.nrel.gov/docs/solar/nsrdb/psm3-download/>
    Psm3,
    /// ResStock simplified 8-column CSV (AMY 2018, etc.).
    /// Contains only dry bulb, RH, wind, and solar -- missing pressure, dew point,
    /// infrared, sky cover, and precipitation, which are estimated at parse time.
    ResStockCsv,
    /// NREL TMY3 CSV file, hourly data (8760 rows for standard year).
    /// Reference: Wilcox & Marion (2008), NREL/TP-581-43156.
    /// <https://docs.nrel.gov/docs/fy08osti/43156.pdf>
    Tmy3,
}

/// Whether the run treats the weather file as a typical (TMY-style) year
/// or an actual one: EnergyPlus's RunPeriod "Treat Weather as Actual",
/// which defaults to No (`idd/Energy+.idd.in:1480-1484`).
///
/// `Typical`: the file's month, day and hour address the records
/// whatever the stamps' years; a run past Dec 31 wraps to Jan 1 of the
/// file (wrap by day of year); a run year's Feb 29 uses the file's Feb
/// 29 records when it has them and is not simulated when it does not
/// (`WeatherManager.cc:2816-2828`).
///
/// `Actual`: the run's first and last steps must lie inside the file's
/// own dated records, and a run that includes a Feb 29 the file has no
/// records for is fatal (`WeatherManager.cc:888-899, 2832-2836,
/// 904-926, 990`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeatherYear {
    /// The file is a typical (TMY-style) year: month, day and hour
    /// address the records whatever their years, runs wrap by day of
    /// year, and a Feb 29 the file lacks is not simulated. The default
    /// (EnergyPlus's "Treat Weather as Actual" = No).
    #[default]
    Typical,
    /// The file is an actual year: the run must lie inside the file's
    /// own dated records.
    Actual,
}

impl WeatherYear {
    /// Whether the run skips Feb 29 days over this file: the typical
    /// rule with a file that carries no Feb 29 records. EnergyPlus
    /// simulates such a leap year as a 365-day year, skipping every Feb
    /// 29 record met in a non-leap run year and the missing one in a
    /// leap one (`WeatherManager.cc:2816-2828`, the condition at
    /// `:2817`; HARES follows it for weather and schedules, with
    /// calendar day types).
    #[must_use]
    pub fn skips_feb29(self, file: &WeatherTimeSeries) -> bool {
        matches!(self, Self::Typical) && !file.has_feb29()
    }
}

/// A weather file's declared data period (the EPW DATA PERIODS header's
/// start and end day/month; the count-derived period for formats without
/// the header).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataPeriod {
    pub start_month: u32,
    pub start_day: u32,
    pub end_month: u32,
    pub end_day: u32,
}

impl DataPeriod {
    /// A complete year: Jan 1 to Dec 31, the period every weather file a
    /// HARES run reads must declare.
    #[must_use]
    pub const fn complete_year() -> Self {
        Self {
            start_month: 1,
            start_day: 1,
            end_month: 12,
            end_day: 31,
        }
    }

    /// Whether the period covers the complete year, Jan 1 to Dec 31.
    #[must_use]
    pub const fn is_complete_year(&self) -> bool {
        self.start_month == 1 && self.start_day == 1 && self.end_month == 12 && self.end_day == 31
    }
}

impl std::fmt::Display for DataPeriod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}/{} to {}/{}",
            self.start_month, self.start_day, self.end_month, self.end_day
        )
    }
}

/// One weather record's dated stamp, as the file states it: the year,
/// month and day of the record's date, the file's own hour field, and
/// the file's own minute field. The stamps are the file's own dates:
/// an actual-year run is checked against them.
///
/// The hour field keeps the file's convention: EPW, TMY3 and the
/// ResStock CSV state 1..=24 (hour-ending; 24 is the day's last hour),
/// PSM3 states 0..=23 (hour-beginning).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordStamp {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
}

impl RecordStamp {
    /// The stamp's date.
    #[must_use]
    pub fn date(&self) -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(self.year, u32::from(self.month), u32::from(self.day))
            .unwrap_or_else(|| {
                chrono::NaiveDate::from_ymd_opt(2000, 1, 1).expect("a valid fallback date")
            })
    }

    /// Whether the stamp is a Feb 29.
    #[must_use]
    pub fn is_feb29(&self) -> bool {
        self.month == 2 && self.day == 29
    }

    /// The stamp as a naive datetime: the date at midnight plus the
    /// hour's end (an hour field of 24 is the day's last hour, ending at
    /// the next midnight) and the minute field.
    #[must_use]
    pub fn stamp_datetime(&self) -> Option<chrono::NaiveDateTime> {
        self.date()
            .and_hms_opt(0, 0, 0)?
            .checked_add_signed(chrono::Duration::hours(i64::from(self.hour)))?
            .checked_add_signed(chrono::Duration::minutes(i64::from(self.minute)))
    }
}

/// Detect the weather file format from its path extension and (for CSV) header content.
///
/// - `.epw` extension maps to [`WeatherFormat::Epw`].
/// - `.csv` extension triggers header sniffing:
///   - **PSM3**: line 1 starts with `Source` and has 10+ fields; line 3 contains
///     `Year`, `Month`, `Day`, `Hour`, `Minute` and at least one of `GHI`/`DNI`/`DHI`.
///   - **TMY3**: line 2 contains `Date (MM/DD/YYYY)`, `Time (HH:MM)`, and at least
///     one of `GHI (W/m^2)` / `DNI (W/m^2)` / `DHI (W/m^2)`.
///   - **ResStock CSV**: line 1 contains both `Dry Bulb Temperature` and
///     `Global Horizontal Radiation` (case-insensitive).
/// - Other extensions produce an error listing supported formats.
pub fn detect_weather_format(path: impl AsRef<Path>) -> Result<WeatherFormat, WeatherError> {
    let path = path.as_ref();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match ext.as_str() {
        "epw" => Ok(WeatherFormat::Epw),
        "csv" => sniff_csv_header(path),
        _ => {
            let ext_display = if ext.is_empty() {
                "(none)".to_string()
            } else {
                format!(".{ext}")
            };
            Err(WeatherError::Parse(format!(
                "unsupported weather file extension {ext_display}; supported formats: .epw, .csv (PSM3/TMY3/ResStock)"
            )))
        }
    }
}

/// Read the first lines of a CSV file and detect whether it is PSM3, TMY3, or ResStock format.
fn sniff_csv_header(path: &Path) -> Result<WeatherFormat, WeatherError> {
    let file = std::fs::File::open(path).map_err(|source| WeatherError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let reader = std::io::BufReader::new(file);
    let mut lines_iter = reader.lines();

    // Line 1.
    let line1 = next_line(&mut lines_iter, path, 1)?;
    let fields: Vec<&str> = line1.split(',').collect();

    // PSM3: line 1 starts with "Source" and has 10+ fields.
    let looks_like_psm3_line1 = fields.first().is_some_and(|f| {
        f.trim()
            .trim_start_matches('\u{FEFF}')
            .eq_ignore_ascii_case("Source")
    }) && fields.len() >= 10;

    if looks_like_psm3_line1 {
        // Line 2: field values -- skip (we only need lines 1 and 3 for detection).
        let _line2 = next_line(&mut lines_iter, path, 2)?;

        // Line 3: column names -- must contain temporal and at least one solar column.
        let line3 = next_line(&mut lines_iter, path, 3)?;
        let cols: Vec<&str> = line3.split(',').map(str::trim).collect();
        let has = |name: &str| cols.iter().any(|c| c.eq_ignore_ascii_case(name));

        let has_temporal =
            has("Year") && has("Month") && has("Day") && has("Hour") && has("Minute");
        let has_solar = has("GHI") || has("DNI") || has("DHI");

        if has_temporal && has_solar {
            return Ok(WeatherFormat::Psm3);
        }
    }

    // ResStock: detectable from line 1 alone.
    if crate::resstock_csv::is_resstock_csv_header(&line1) {
        return Ok(WeatherFormat::ResStockCsv);
    }

    // TMY3: line 1 is station metadata; line 2 is the column header.
    // The column header contains "Date (MM/DD/YYYY)" and solar columns.
    let line2 = next_line(&mut lines_iter, path, 2)?;
    if crate::tmy3::is_tmy3_column_header(&line2) {
        return Ok(WeatherFormat::Tmy3);
    }

    Err(WeatherError::Parse(
        "CSV file does not appear to be PSM3/SAM, TMY3, or ResStock format; \
         expected NSRDB header, TMY3 column header, or ResStock columns"
            .to_string(),
    ))
}

/// Read the next line from a buffered reader, mapping I/O errors.
fn next_line(
    lines: &mut std::io::Lines<std::io::BufReader<std::fs::File>>,
    path: &Path,
    line_num: usize,
) -> Result<String, WeatherError> {
    lines
        .next()
        .ok_or_else(|| {
            WeatherError::Parse(format!(
                "CSV file has fewer than {line_num} lines; cannot detect format"
            ))
        })?
        .map_err(|source| WeatherError::Io {
            path: path.display().to_string(),
            source,
        })
}

/// Reads a weather file's header metadata with no data row read: the
/// format's own header (EPW's LOCATION line, the PSM3 field names and
/// values, the TMY3 station header), the same header functions the full
/// parsers call. A ResStock CSV has no embedded location, so its metadata
/// is the no-location placeholder its full parser starts from. The
/// fleet's alignment pre-pass reads a home's weather metadata through
/// this, before any home is parsed; the site-location resolution reads
/// only the location fields, which every header carries.
///
/// # Errors
///
/// [`WeatherError`] when the format is not supported, the file is
/// unreadable, or the header is malformed.
pub fn read_weather_meta(path: impl AsRef<Path>) -> Result<WeatherMeta, WeatherError> {
    let path = path.as_ref();
    match detect_weather_format(path)? {
        WeatherFormat::Epw => crate::epw::read_meta(path),
        WeatherFormat::Psm3 => crate::psm3::read_meta(path),
        WeatherFormat::Tmy3 => crate::tmy3::read_meta(path),
        WeatherFormat::ResStockCsv => Ok(WeatherMeta {
            location: String::new(),
            latitude: 0.0,
            longitude: 0.0,
            timezone_offset_h: 0.0,
            elevation_m: 0.0,
            wf_allows_leap_years: true,
            source_step_secs: 3600,
            record_label: RecordLabel::IntervalEnd,
            data_period: DataPeriod::complete_year(),
            has_embedded_location: false,
            station_wmo: None,
        }),
    }
}

/// Parse a weather file, auto-detecting the format from its extension and header.
///
/// Supported formats:
/// - **EPW** (`.epw`): EnergyPlus Weather files, hourly data.
/// - **PSM3** (`.csv`): NREL NSRDB SAM CSV files at 5/15/30/60-min resolution.
/// - **TMY3** (`.csv`): NREL TMY3 CSV files, hourly data.
/// - **ResStock CSV** (`.csv`): ResStock simplified 8-column CSV files.
///
/// Format detection is performed by [`detect_weather_format`], then the file is
/// dispatched to the appropriate parser.
pub fn parse_weather(path: impl AsRef<Path>) -> Result<WeatherTimeSeries, WeatherError> {
    let path = path.as_ref();
    match detect_weather_format(path)? {
        WeatherFormat::Epw => crate::epw::parse_epw(path),
        WeatherFormat::Psm3 => crate::psm3::parse_psm3(path),
        WeatherFormat::Tmy3 => crate::tmy3::parse_tmy3(path),
        WeatherFormat::ResStockCsv => {
            // Default to sea-level elevation and equator when called through the
            // generic interface. Callers who know the site location should use
            // `parse_resstock_csv` or `parse_weather_with_location` directly.
            tracing::warn!(
                "ResStock CSV parsed with default sea-level elevation and equator location; \
                 use parse_weather_with_location for correct pressure and solar calculations"
            );
            crate::resstock_csv::parse_resstock_csv(path, 0.0, 0.0, 0.0, 0.0)
        }
    }
}

/// Parse a weather file with a known site elevation.
///
/// Behaves identically to [`parse_weather`] for EPW and PSM3 formats
/// (which carry their own elevation metadata). For ResStock CSV files,
/// the provided `elevation_m` is used to estimate atmospheric pressure
/// via the ISA standard atmosphere model, and lat/lon/tz default to 0.0.
pub fn parse_weather_with_elevation(
    path: impl AsRef<Path>,
    elevation_m: f64,
) -> Result<WeatherTimeSeries, WeatherError> {
    parse_weather_with_location(path, elevation_m, 0.0, 0.0, 0.0)
}

/// Parse a weather file with full site location metadata.
///
/// Behaves identically to [`parse_weather`] for EPW and PSM3 formats
/// (which carry their own location metadata). For ResStock CSV files,
/// `elevation_m`, `latitude`, `longitude`, and `timezone_offset_h` are used
/// to populate [`WeatherMeta`] for downstream solar and pressure calculations.
///
/// When caller-provided coordinates are non-zero and differ from the file's
/// embedded location by more than 1.0°, a [`tracing::warn!`] is emitted
/// but the file's embedded location remains authoritative. Use
/// [`parse_weather_override_location`] when the file's metadata is known
/// wrong and must be replaced.
///
/// The 1.0° threshold follows EnergyPlus precedent — EnergyPlus's
/// WeatherManager.cc emits a warning for any coordinate difference between
/// the IDF `Site:Location` and the weather file's embedded coordinates.
pub fn parse_weather_with_location(
    path: impl AsRef<Path>,
    elevation_m: f64,
    latitude: f64,
    longitude: f64,
    timezone_offset_h: f64,
) -> Result<WeatherTimeSeries, WeatherError> {
    let path = path.as_ref();
    let result = match detect_weather_format(path)? {
        WeatherFormat::Epw => crate::epw::parse_epw(path),
        WeatherFormat::Psm3 => crate::psm3::parse_psm3(path),
        WeatherFormat::Tmy3 => crate::tmy3::parse_tmy3(path),
        WeatherFormat::ResStockCsv => crate::resstock_csv::parse_resstock_csv(
            path,
            elevation_m,
            latitude,
            longitude,
            timezone_offset_h,
        ),
    }?;

    // Coordinate mismatch warning for formats with embedded location metadata.
    // EnergyPlus WeatherManager.cc emits a warning when the IDF Site:Location
    // coordinates differ from the EPW file's embedded location; HARES mirrors
    // this. When caller coordinates are all zero (no HPXML site data provided),
    // the check is skipped — the file is the sole source of location.
    let has_caller_coords = latitude.abs() + longitude.abs() > 0.0;
    if has_caller_coords {
        let lat_diff = (result.meta.latitude - latitude).abs();
        let lon_diff = (result.meta.longitude - longitude).abs();
        if lat_diff > 1.0 || lon_diff > 1.0 {
            tracing::warn!(
                file_location = %result.meta.location,
                file_latitude = result.meta.latitude,
                file_longitude = result.meta.longitude,
                caller_latitude = latitude,
                caller_longitude = longitude,
                "weather file location ({:.4}°, {:.4}°) differs from caller-provided site \
                 location ({:.4}°, {:.4}°) by more than 1.0°; \
                 the file's embedded location will be used",
                result.meta.latitude,
                result.meta.longitude,
                latitude,
                longitude,
            );
        }
    }

    Ok(result)
}

/// Parse a weather file and override its embedded location metadata with
/// caller-supplied values.
///
/// Use when the file's embedded latitude, longitude, elevation, or timezone
/// are known to be incorrect and must be replaced with the HPXML/IDF site
/// data. For the standard path (emit a warning on mismatch but keep the
/// file's location authoritative), use [`parse_weather_with_location`].
///
/// Parses the file via [`parse_weather`] (auto-detecting format), then
/// replaces `meta.latitude`, `meta.longitude`, `meta.elevation_m`, and
/// `meta.timezone_offset_h` with the caller-provided values.
pub fn parse_weather_override_location(
    path: impl AsRef<Path>,
    latitude: f64,
    longitude: f64,
    elevation_m: f64,
    timezone_offset_h: f64,
) -> Result<WeatherTimeSeries, WeatherError> {
    let mut weather = parse_weather(path)?;
    weather.meta.latitude = latitude;
    weather.meta.longitude = longitude;
    weather.meta.elevation_m = elevation_m;
    weather.meta.timezone_offset_h = timezone_offset_h;
    Ok(weather)
}

/// The timestamp convention of a weather file's records: which end of the
/// interval a record's stamp labels.
///
/// EPW, TMY3 and ResStock CSV stamp a record at its interval's end: a
/// record stamped `h` covers `(h - 1 h, h]`. PSM3 stamps a record at its
/// interval's start: a record stamped `t` covers `[t, t + interval)`, so
/// an hourly PSM3 record stamped `t` is the record of the hour ending
/// `t + 1 h`.
///
/// EnergyPlus reads every format's records as the hours they cover: the
/// hourly file's 24 records are hours 1 to 24 of the day, and a record's
/// value is reached at the end of its hour (Input Output Reference,
/// "Weather Data Hourly Interpolation",
/// `doc/input-output-reference/src/weather-data/weather-data-hourly-interpolation.tex`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordLabel {
    /// A record's stamp labels its interval's end: the record covers
    /// `(stamp - interval, stamp]`.
    IntervalEnd,
    /// A record's stamp labels its interval's start: the record covers
    /// `[stamp, stamp + interval)`.
    IntervalStart,
}

/// Which hour's record the run's first hour interpolates from, when the
/// weather file is hourly and the run's step is shorter than an hour.
///
/// EnergyPlus Input Output Reference, RunPeriod, "First Hour Interpolation
/// Starting Values"
/// (`doc/input-output-reference/src/overview/group-location-climate-weather-file-access.tex:717-721`):
/// `Hour24` (the default) interpolates the first hour from hour 24 of the
/// first day; `Hour1` from hour 1. The option only affects the first hour
/// of the first day in the entire run period; the code sets the previous
/// hour's record from it once per environment
/// (`WeatherManager.cc:3050-3056`), for every field, solar included. The
/// same interpolation is used for repeated warm-up days
/// (`idd/Energy+.idd.in:1488`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FirstHour {
    /// Interpolate the run's first hour from hour 24 of the run's first
    /// day (EnergyPlus's default).
    #[default]
    Hour24,
    /// Interpolate the run's first hour from hour 1 of the run's first day.
    Hour1,
}

/// One weather field's interpolation method over an hourly file at a step
/// shorter than an hour, as EnergyPlus's interpolation loop computes it
/// (`WeatherManager.cc:3093-3121`). One exhaustive match over
/// [`WeatherField`] chooses the method; see
/// `docs/PHYSICS_DECISIONS.md`'s weather section for the field table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InterpMethod {
    /// Linear last-hour interpolation between the previous hour's record
    /// and this hour's, evaluated for the step's end: dry bulb, dew point,
    /// relative humidity, pressure, wind speed, opaque sky cover,
    /// horizontal infrared (`WeatherManager.cc:3093-3105`).
    LastHour,
    /// The last-hour interpolation on the circle, shortest arc
    /// (`interpolateWindDirection`, `WeatherManager.cc:3174-3188`): wind
    /// direction.
    LastHourCircular,
    /// The last-hour interpolated depth divided by the steps per hour:
    /// liquid precipitation (`WeatherManager.cc:3119-3120`).
    LastHourDivided,
    /// The hour's record held across its steps: ground temperature (a
    /// monthly site series, not a weather-file hourly field in EnergyPlus)
    /// and surface albedo (EnergyPlus drops the file's albedo; holding the
    /// hour's record is a HARES choice).
    HourHold,
    /// EnergyPlus's solar midpoint interpolation: GHI, DNI and DHI
    /// (`WeatherManager.cc:8343-8375`, `:3107-3117`).
    SolarMidpoint,
    /// Recomputed from the step's interpolated inputs, never interpolated
    /// itself: sky temperature (`calcSky`, `WeatherManager.cc:3106-3113`).
    Recompute,
}

/// Every field's one method, chosen by one exhaustive match: the test
/// `every_weather_field_has_one_declared_method` checks the match against
/// the field table in `docs/PHYSICS_DECISIONS.md`'s weather section.
pub(crate) fn field_method(field: WeatherField) -> InterpMethod {
    match field {
        WeatherField::DryBulbC
        | WeatherField::DewPointC
        | WeatherField::RelHumidityPct
        | WeatherField::PressureKpa
        | WeatherField::WindSpeedMS
        | WeatherField::OpaqueSkyCover
        | WeatherField::HorizontalInfrared => InterpMethod::LastHour,
        WeatherField::WindDirDeg => InterpMethod::LastHourCircular,
        WeatherField::LiquidPrecipM => InterpMethod::LastHourDivided,
        WeatherField::GhiWM2 | WeatherField::DniWM2 | WeatherField::DhiWM2 => {
            InterpMethod::SolarMidpoint
        }
        WeatherField::SkyTempC => InterpMethod::Recompute,
        WeatherField::GroundTempC | WeatherField::SurfaceAlbedo => InterpMethod::HourHold,
    }
}

/// EnergyPlus's `SolarInterpolation` weight for step `ts` of `n` steps
/// per hour (1 to n), one function of `(n, ts)` reproducing the table
/// build at `WeatherManager.cc:8343-8375` for every n: for an even n the
/// step ending at the half hour takes 1.0 on its own hour and the weight
/// falls by 1/n per step each side; n = 3 takes 5/6, 5/6, 0.5; for an
/// odd n >= 5 the two middle steps take `1 - 1/(2n)`.
pub(crate) fn solar_interpolation_weight(n: usize, ts: usize) -> f64 {
    if n == 1 {
        return 0.5;
    }
    if n.is_multiple_of(2) {
        let halfpoint = n / 2;
        if ts == halfpoint {
            return 1.0;
        }
        let hpoint = halfpoint.abs_diff(ts);
        return 1.0 - hpoint as f64 * (1.0 / n as f64);
    }
    if n == 3 {
        return [5.0 / 6.0, 5.0 / 6.0, 0.5][ts - 1];
    }
    let tweight = 1.0 / n as f64;
    let halfpoint = n / 2;
    let tweight1 = 1.0 - tweight / 2.0;
    if ts == halfpoint || ts == halfpoint + 1 {
        return tweight1;
    }
    let hpoint = if ts < halfpoint {
        halfpoint - ts
    } else {
        ts - halfpoint - 1
    };
    tweight1 - hpoint as f64 * tweight
}
/// Metadata extracted from an EPW header.
#[derive(Debug, Clone, PartialEq)]
pub struct WeatherMeta {
    pub location: String,
    pub latitude: f64,
    pub longitude: f64,
    pub timezone_offset_h: f64,
    pub elevation_m: f64,
    /// Whether the EPW HOLIDAYS/DAYLIGHT SAVINGS header field A1 declares
    /// "Yes" for leap year observation. When `false`, Feb 29 data rows are
    /// discarded even if the file contains 8784 records. Non-EPW weather
    /// sources (TMY3, PSM3, ResStock CSV) default to `true` since they lack
    /// an equivalent header and their record counts determine leap behaviour.
    ///
    /// EnergyPlus: `WFAllowsLeapYears` (WeatherManager.cc:7889).
    /// Field A1 values as per EPW Data Dictionary: "Yes" or "No".
    pub wf_allows_leap_years: bool,
    /// Native timestep of the source file in seconds (e.g. 3600 for EPW, 300 for 5-min PSM3).
    pub source_step_secs: u32,
    /// Which end of the interval a record's stamp labels, per format: EPW,
    /// TMY3 and ResStock CSV are hour-ending (`IntervalEnd`); PSM3 is
    /// hour-beginning (`IntervalStart`). See [`RecordLabel`].
    pub record_label: RecordLabel,
    /// The file's declared data period (the EPW DATA PERIODS header's
    /// start and end day/month; the count-derived period for the formats
    /// without the header). Every file a run reads must declare a
    /// complete year: the water-mains annual mean and monthly range and
    /// the ground temperature the parsers derive from twelve monthly
    /// dry-bulb means need twelve complete months (the binding "Weather"
    /// section of the numeric-fixes ledger; EnergyPlus runs a partial
    /// typical period when the run lies inside it,
    /// `WeatherManager.cc:941-991`, but its weather-file water-mains
    /// correlation otherwise falls back to a fixed 10.0 C, and HARES
    /// substitutes no value it was not given).
    pub data_period: DataPeriod,
    /// Whether this weather format embeds its own site location (latitude,
    /// longitude, timezone) in its header. EPW, PSM3, and TMY3 do (`true`);
    /// ResStock CSV does not (`false`) and is parsed with `0.0` placeholders
    /// that callers must supply from HPXML or an explicit override.
    ///
    /// The site-location resolver uses this to distinguish a genuine
    /// equator/prime-meridian/UTC site from an absent value: a `0.0` from a
    /// format with `has_embedded_location == false` means "unknown", not
    /// "the prime meridian".
    pub has_embedded_location: bool,
    /// The weather station's WMO number, from the EPW LOCATION header
    /// (field N4) or the TMY3 header's station ID; `None` for formats that
    /// carry none (PSM3, ResStock CSV). OS-HPXML derives an HPXML's missing
    /// IECC climate zone from it.
    pub station_wmo: Option<String>,
}

/// Addressable weather columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeatherField {
    DryBulbC,
    DewPointC,
    RelHumidityPct,
    PressureKpa,
    GhiWM2,
    DniWM2,
    DhiWM2,
    WindSpeedMS,
    WindDirDeg,
    OpaqueSkyCover,
    HorizontalInfrared,
    SkyTempC,
    GroundTempC,
    LiquidPrecipM,
    SurfaceAlbedo,
}

/// Error type for weather I/O and time-series operations.
#[derive(Debug, Error)]
pub enum WeatherError {
    #[error("io error reading `{path}`: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parse error: {0}")]
    Parse(String),
    #[error("validation error: {0}")]
    Validation(String),
    #[error("weather resampling error: {0}")]
    Resample(String),
}

/// Column-oriented weather time series.
#[derive(Debug, Clone, PartialEq)]
pub struct WeatherTimeSeries {
    pub meta: WeatherMeta,
    /// Design conditions parsed from the EPW header line 2, or `None`
    /// for formats that lack them (PSM3, TMY3, ResStock CSV).
    pub design_conditions: Option<DesignConditions>,
    /// Each record's dated stamp as the file states it (year, month,
    /// day, hour and minute), one per record, so an actual-year run is
    /// checked against the file's own dates. Empty for a series built in
    /// code or derived by resampling (whose rows are no longer the
    /// file's records).
    pub stamps: Vec<RecordStamp>,
    pub dry_bulb_c: Vec<f64>,
    pub dew_point_c: Vec<f64>,
    pub rel_humidity_pct: Vec<f64>,
    pub pressure_kpa: Vec<f64>,
    pub ghi_w_m2: Vec<f64>,
    pub dni_w_m2: Vec<f64>,
    pub dhi_w_m2: Vec<f64>,
    pub wind_speed_m_s: Vec<f64>,
    pub wind_dir_deg: Vec<f64>,
    pub opaque_sky_cover: Vec<f64>,
    pub horizontal_infrared_w_m2: Vec<f64>,
    pub sky_temp_c: Vec<f64>,
    pub ground_temp_c: Vec<f64>,
    /// Liquid precipitation depth per timestep (m).
    /// Parsed from EPW field 33; zero when data is unavailable.
    pub liquid_precip_m: Vec<f64>,
    /// Surface albedo (ground reflectance) [dimensionless, 0–1].
    /// `Some(vec)` when the source file provides per-timestep albedo (e.g. PSM3
    /// `Surface Albedo` column); `None` for formats that lack it (EPW, ResStock CSV).
    /// Consumers should fall back to `DEFAULT_GROUND_ALBEDO` (0.2) when `None`.
    pub surface_albedo: Option<Vec<f64>>,
}

impl WeatherTimeSeries {
    /// Number of timesteps in this series.
    #[must_use]
    pub fn len(&self) -> usize {
        self.dry_bulb_c.len()
    }

    /// Returns true when no samples are present.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether the file carries Feb 29 records: any stamp dated Feb 29.
    /// A series without stamps (built in code) carries none.
    #[must_use]
    pub fn has_feb29(&self) -> bool {
        self.stamps.iter().any(RecordStamp::is_feb29)
    }

    /// The position of the record stamped at `t` (the stamp's own date,
    /// hour and minute fields as a time of day: an hour field of 24 is
    /// the day's last hour, ending at the next midnight). `None` when no
    /// record carries that stamp.
    #[must_use]
    pub fn find_stamp(&self, t: chrono::NaiveDateTime) -> Option<usize> {
        self.stamps
            .iter()
            .position(|stamp| stamp.stamp_datetime() == Some(t))
    }

    /// The file's own year: the calendar year whose hours the file's
    /// records cover. The covered interval runs from the first record's
    /// interval start to the last record's interval end (an
    /// [`RecordLabel::IntervalEnd`] file's covered interval starts one
    /// interval before its first stamp; an [`RecordLabel::IntervalStart`]
    /// file's ends one interval after its last stamp), and the year that
    /// owns most of those hours is the file's own year: for an
    /// `IntervalEnd` AMY 2018 file stamped 2018-01-01 01:00 to
    /// 2019-01-01 00:00 the covered hours are the hours of 2018, not the
    /// year of the last stamp. `None` when the series carries no dated
    /// stamps (a series built in code).
    #[must_use]
    pub fn file_year(&self) -> Option<i32> {
        use chrono::Datelike;

        let covered_start = self.stamps.first()?.stamp_datetime()?;
        let covered_end = self.stamps.last()?.stamp_datetime()?;
        let interval = i64::from(self.meta.source_step_secs.max(1));
        let (covered_start, covered_end) = match self.meta.record_label {
            RecordLabel::IntervalEnd => (
                covered_start - chrono::Duration::seconds(interval),
                covered_end,
            ),
            RecordLabel::IntervalStart => (
                covered_start,
                covered_end + chrono::Duration::seconds(interval),
            ),
        };
        // The covered interval's seconds per calendar year; the majority
        // year is the file's own year. A complete-year file covers one
        // calendar year exactly, so this loop is two iterations there.
        let mut best_year = covered_start.year();
        let mut best_secs: i64 = -1;
        let mut year = covered_start.year();
        while year <= covered_end.year() {
            let year_start = chrono::NaiveDate::from_ymd_opt(year, 1, 1)
                .expect("Jan 1 is a real date")
                .and_hms_opt(0, 0, 0)
                .expect("midnight is a real time");
            let next_start = chrono::NaiveDate::from_ymd_opt(year + 1, 1, 1)
                .expect("Jan 1 is a real date")
                .and_hms_opt(0, 0, 0)
                .expect("midnight is a real time");
            let lo = covered_start.max(year_start);
            let hi = covered_end.min(next_start);
            let secs = (hi - lo).num_seconds().max(0);
            if secs > best_secs {
                best_secs = secs;
                best_year = year;
            }
            year += 1;
        }
        Some(best_year)
    }

    /// Indexed access into a specific weather column.
    ///
    /// # Panics
    /// Panics if `timestep_index` is out of bounds.
    #[must_use]
    pub fn get(&self, field: WeatherField, timestep_index: usize) -> f64 {
        match field {
            WeatherField::DryBulbC => self.dry_bulb_c[timestep_index],
            WeatherField::DewPointC => self.dew_point_c[timestep_index],
            WeatherField::RelHumidityPct => self.rel_humidity_pct[timestep_index],
            WeatherField::PressureKpa => self.pressure_kpa[timestep_index],
            WeatherField::GhiWM2 => self.ghi_w_m2[timestep_index],
            WeatherField::DniWM2 => self.dni_w_m2[timestep_index],
            WeatherField::DhiWM2 => self.dhi_w_m2[timestep_index],
            WeatherField::WindSpeedMS => self.wind_speed_m_s[timestep_index],
            WeatherField::WindDirDeg => self.wind_dir_deg[timestep_index],
            WeatherField::OpaqueSkyCover => self.opaque_sky_cover[timestep_index],
            WeatherField::HorizontalInfrared => self.horizontal_infrared_w_m2[timestep_index],
            WeatherField::SkyTempC => self.sky_temp_c[timestep_index],
            WeatherField::GroundTempC => self.ground_temp_c[timestep_index],
            WeatherField::LiquidPrecipM => self.liquid_precip_m[timestep_index],
            WeatherField::SurfaceAlbedo => self
                .surface_albedo
                .as_ref()
                .map_or(DEFAULT_GROUND_ALBEDO, |v| v[timestep_index]),
        }
    }

    /// Resolve the series to the run's step, by the EnergyPlus weather
    /// rules:
    ///
    /// - A file at the run's own step: the records are the steps, used
    ///   directly (clone).
    /// - A sub-hourly file (more than one record per hour) at any other
    ///   step: an error naming both intervals. EnergyPlus refuses the run
    ///   the same way: "Number of intervals per hour on Weather file does
    ///   not match specified number of Time Steps Per Hour"
    ///   (`WeatherManager.cc:715-722`).
    /// - An hourly file at a step longer than an hour: the interval mean
    ///   of the records the step covers (EnergyPlus has no timestep
    ///   longer than an hour, so this is a HARES convention).
    /// - An hourly file at a step shorter than an hour: EnergyPlus's
    ///   interpolation
    ///   (`WeatherManager.cc:3043-3128`): every point field is linear
    ///   last-hour interpolation evaluated for the step's end, the wind
    ///   direction interpolates on the circle, the precipitation is the
    ///   weighted depth divided by the steps per hour, the solar fields
    ///   take the midpoint interpolation (the hourly average is the
    ///   hour's midpoint value), the sky temperature is recomputed from
    ///   the interpolated inputs, and the ground temperature and surface
    ///   albedo hold the hour's record.
    ///
    /// `first_hour` and `start` drive the first-hour rule: the first hour
    /// of the run interpolates from hour 24 of the run's first day
    /// (`FirstHour::Hour24`, the default) or hour 1 (`FirstHour::Hour1`),
    /// never from a record before the run's first day.
    ///
    /// # Errors
    ///
    /// [`WeatherError::Resample`] when the intervals are incompatible or
    /// the file does not cover whole days.
    pub fn resample(
        &self,
        target_step_secs: u32,
        first_hour: FirstHour,
        start: chrono::NaiveDateTime,
        weather_year: WeatherYear,
    ) -> Result<Self, WeatherError> {
        if target_step_secs == 0 {
            return Err(WeatherError::Resample(
                "target_step_secs must be > 0".to_string(),
            ));
        }

        // The calendar-date addressing the run's reads use (the typical
        // and actual year rules): the interpolation's first-hour rule and
        // the construction checks resolve records through it.
        let addressing = WeatherAddressing::new(self, weather_year)?;

        let source = self.meta.source_step_secs;

        if source == target_step_secs {
            return Ok(self.clone());
        }

        // A sub-hourly file's records are its timesteps: the run's step
        // must equal the file's interval, or the run is an error, as
        // EnergyPlus refuses the mismatch
        // (`WeatherManager.cc:715-722`). Today's resampling of such
        // files (the mean when finer, PCHIP or hold when coarser) is
        // gone.
        if source != 3600 {
            return Err(WeatherError::Resample(format!(
                "the weather file's interval per record ({source} s) does not match the run's \
                 step ({target_step_secs} s): a sub-hourly weather file's interval must equal \
                 the run's step, as in EnergyPlus (number of intervals per hour on Weather \
                 file does not match specified number of Time Steps Per Hour)"
            )));
        }

        if target_step_secs > 3600 {
            // Downsampling: an hourly file at a step longer than an hour
            // keeps the interval mean of the records the step covers.
            if !target_step_secs.is_multiple_of(source) {
                return Err(WeatherError::Resample(format!(
                    "incompatible timestep: {target_step_secs} % {source} != 0"
                )));
            }
            let ratio = (target_step_secs / source) as usize;

            if !self.len().is_multiple_of(ratio) {
                return Err(WeatherError::Resample(format!(
                    "record count {} not divisible by downsample ratio {ratio}",
                    self.len()
                )));
            }

            // Instantaneous fields → mean.
            let dry_bulb_c = mean_downsample(&self.dry_bulb_c, ratio);
            let dew_point_c = mean_downsample(&self.dew_point_c, ratio);
            let rel_humidity_pct = mean_downsample(&self.rel_humidity_pct, ratio);
            let pressure_kpa = mean_downsample(&self.pressure_kpa, ratio);
            let horizontal_infrared_w_m2 = mean_downsample(&self.horizontal_infrared_w_m2, ratio);
            let ground_temp_c = mean_downsample(&self.ground_temp_c, ratio);
            let opaque_sky_cover = mean_downsample(&self.opaque_sky_cover, ratio);

            // Recompute sky temperature from downsampled inputs (same rationale as
            // the interpolation: T_sky is non-linear in its inputs, so averaging
            // T_sky is inconsistent with averaging the input fields).
            let sky_temp_c: Vec<f64> = dry_bulb_c
                .iter()
                .zip(&dew_point_c)
                .zip(&rel_humidity_pct)
                .zip(&horizontal_infrared_w_m2)
                .zip(&opaque_sky_cover)
                .map(|((((&db, &dp), &rh), &ir), &osc)| {
                    compute_sky_temp_c(ir, db, dp, rh, osc, SkyTempModel::default())
                })
                .collect();

            return Ok(Self {
                meta: WeatherMeta {
                    source_step_secs: target_step_secs,
                    record_label: RecordLabel::IntervalEnd,
                    ..self.meta.clone()
                },
                design_conditions: self.design_conditions,
                stamps: Vec::new(),
                dry_bulb_c,
                dew_point_c,
                rel_humidity_pct,
                pressure_kpa,
                horizontal_infrared_w_m2,
                sky_temp_c,
                ground_temp_c,
                opaque_sky_cover,
                wind_speed_m_s: mean_downsample(&self.wind_speed_m_s, ratio),
                wind_dir_deg: mean_downsample(&self.wind_dir_deg, ratio),
                // Solar fields → mean (average irradiance preserves energy).
                ghi_w_m2: mean_downsample(&self.ghi_w_m2, ratio),
                dni_w_m2: mean_downsample(&self.dni_w_m2, ratio),
                dhi_w_m2: mean_downsample(&self.dhi_w_m2, ratio),
                // Accumulated depth → sum.
                liquid_precip_m: sum_downsample(&self.liquid_precip_m, ratio),
                // Surface property → mean. Mean is acceptable for downsampling albedo
                // because reflected solar is linear in albedo: mean(albedo) × GHI equals
                // mean(albedo × GHI) when GHI is constant within the block.
                surface_albedo: self
                    .surface_albedo
                    .as_ref()
                    .map(|v| mean_downsample(v, ratio)),
            });
        }

        // An hourly file at a step shorter than an hour: the EnergyPlus
        // interpolation. The step must divide the hour (EnergyPlus allows
        // only timesteps per hour that divide 60,
        // `idd/Energy+.idd.in:925-936`).
        if !3600u32.is_multiple_of(target_step_secs) {
            return Err(WeatherError::Resample(format!(
                "the step ({target_step_secs} s) must divide the hour: EnergyPlus allows only \
                 timesteps per hour evenly divisible into 60"
            )));
        }
        self.interpolate_hourly_records(
            (3600 / target_step_secs) as usize,
            first_hour,
            start,
            &addressing,
        )
    }

    /// The EnergyPlus interpolation of an hourly file at a step shorter
    /// than an hour: one output row per step, the point fields linear
    /// last-hour interpolation evaluated for the step's end
    /// (`Value_TimeStep = LastHourValue * Weight_LastHour +
    /// ThisHourValue * Weight_ThisHour`, `Weight_ThisHour =
    /// CurrentTimeStep / NumberOfTimeStepsInHour`; Input Output Reference,
    /// "Weather Data Hourly Interpolation"), the wind direction on the
    /// circle, the precipitation divided by the steps per hour, the sky
    /// temperature recomputed from the row's interpolated inputs
    /// (`calcSky`, `WeatherManager.cc:3106-3113`), and the held fields at
    /// the hour's record.
    ///
    /// The previous hour's record of the run's first hour is the record
    /// `first_hour` names (hour 24 or hour 1 of the run's first day,
    /// `WeatherManager.cc:3050-3056`); every later hour interpolates from
    /// the previous hour's own record, day boundaries included (the
    /// previous day's hour 24), and the file's first hour carries the
    /// file's last record for runs that read past the year's end. No step
    /// reads a record before the run's first day.
    ///
    /// # Errors
    ///
    /// [`WeatherError::Resample`] when the file does not cover whole days.
    fn interpolate_hourly_records(
        &self,
        steps_per_hour: usize,
        first_hour: FirstHour,
        start: chrono::NaiveDateTime,
        addressing: &WeatherAddressing,
    ) -> Result<Self, WeatherError> {
        let n = steps_per_hour;
        let len = self.len();
        if len == 0 {
            return Err(WeatherError::Resample(
                "the weather series is empty".to_string(),
            ));
        }
        if !len.is_multiple_of(24) {
            return Err(WeatherError::Resample(format!(
                "hourly weather must cover whole days: {len} records is not a multiple of 24"
            )));
        }

        // The run's start's record, addressed by calendar date: the hour
        // containing the run's start, whose steps interpolate from the
        // record the first-hour rule names.
        let first_hour_k = addressing.record_at(start)?;
        let first_day = first_hour_k / 24;
        let first_hour_prev = match first_hour {
            FirstHour::Hour24 => first_day * 24 + 23,
            FirstHour::Hour1 => first_day * 24,
        };

        let rows = len * n;
        let mut out = Self {
            meta: WeatherMeta {
                source_step_secs: u32::try_from(3600 / n).expect("the steps per hour divide 3600"),
                record_label: RecordLabel::IntervalEnd,
                ..self.meta.clone()
            },
            design_conditions: self.design_conditions,
            stamps: Vec::new(),
            dry_bulb_c: vec![0.0; rows],
            dew_point_c: vec![0.0; rows],
            rel_humidity_pct: vec![0.0; rows],
            pressure_kpa: vec![0.0; rows],
            ghi_w_m2: vec![0.0; rows],
            dni_w_m2: vec![0.0; rows],
            dhi_w_m2: vec![0.0; rows],
            wind_speed_m_s: vec![0.0; rows],
            wind_dir_deg: vec![0.0; rows],
            opaque_sky_cover: vec![0.0; rows],
            horizontal_infrared_w_m2: vec![0.0; rows],
            sky_temp_c: vec![0.0; rows],
            ground_temp_c: vec![0.0; rows],
            liquid_precip_m: vec![0.0; rows],
            surface_albedo: self.surface_albedo.as_ref().map(|_| vec![0.0; rows]),
        };

        // The last-hour fields: `prev * wp + this * w` per row, the
        // weights EnergyPlus's (`wgt_curr = ts / N`, `wgt_prev =
        // 1 - wgt_curr`, `WeatherManager.cc:8343-8347`; the weighted sum
        // prev-first, `:3094-3105`).
        let last_hour_field = |field: WeatherField, out_column: &mut Vec<f64>| {
            for k in 0..len {
                let prev = prev_record_index(k, first_hour_k, first_hour_prev, len);
                for ts in 1..=n {
                    let w = (ts as f64 / n as f64).min(1.0);
                    let wp = 1.0 - w;
                    out_column[k * n + ts - 1] =
                        self.get(field, prev) * wp + self.get(field, k) * w;
                }
            }
        };
        for (field, column) in [
            (WeatherField::DryBulbC, &mut out.dry_bulb_c),
            (WeatherField::DewPointC, &mut out.dew_point_c),
            (WeatherField::RelHumidityPct, &mut out.rel_humidity_pct),
            (WeatherField::PressureKpa, &mut out.pressure_kpa),
            (WeatherField::WindSpeedMS, &mut out.wind_speed_m_s),
            (WeatherField::OpaqueSkyCover, &mut out.opaque_sky_cover),
            (
                WeatherField::HorizontalInfrared,
                &mut out.horizontal_infrared_w_m2,
            ),
        ] {
            debug_assert_eq!(field_method(field), InterpMethod::LastHour);
            last_hour_field(field, column);
        }

        // The wind direction: the same weight, on the circle.
        for k in 0..len {
            let prev = prev_record_index(k, first_hour_k, first_hour_prev, len);
            for ts in 1..=n {
                let w = (ts as f64 / n as f64).min(1.0);
                out.wind_dir_deg[k * n + ts - 1] =
                    circular_interp(self.wind_dir_deg[prev], self.wind_dir_deg[k], w);
            }
        }

        // The precipitation: the weighted hourly depth divided by N.
        for k in 0..len {
            let prev = prev_record_index(k, first_hour_k, first_hour_prev, len);
            for ts in 1..=n {
                let w = (ts as f64 / n as f64).min(1.0);
                let wp = 1.0 - w;
                let weighted = self.liquid_precip_m[prev] * wp + self.liquid_precip_m[k] * w;
                out.liquid_precip_m[k * n + ts - 1] = weighted / n as f64;
            }
        }

        // The solar fields: EnergyPlus's midpoint interpolation. The
        // hourly average is the value at the hour's midpoint (Engineering
        // Reference, "Weather File Solar Interpolation",
        // climate-calculations.tex:304-306), so the step's value is
        // linear interpolation between hour midpoints evaluated at the
        // step's end: before the half hour the remainder goes to the
        // previous hour's record, after it to the next hour's
        // (`WeatherManager.cc:3074-3091, 3113-3116`), at it the hour's
        // own record. The next hour of hour 24 is hour 1 of the same day
        // (`:3061-3065`); the previous hour of the run's first hour is
        // the record the first-hour rule names. The three-term sum and
        // the zero weights stay in E+'s form (`last * prev_w + this *
        // wgt + next * next_w`).
        let solar_weights: Vec<f64> = (1..=n)
            .map(|ts| solar_interpolation_weight(n, ts))
            .collect();
        tracing::debug!(steps_per_hour = n, weights = ?solar_weights, "the solar interpolation weights");
        for (field, column) in [
            (WeatherField::GhiWM2, &mut out.ghi_w_m2),
            (WeatherField::DniWM2, &mut out.dni_w_m2),
            (WeatherField::DhiWM2, &mut out.dhi_w_m2),
        ] {
            debug_assert_eq!(field_method(field), InterpMethod::SolarMidpoint);
            for k in 0..len {
                let prev = prev_record_index(k, first_hour_k, first_hour_prev, len);
                let hour_in_day = k % 24;
                // The next hour of hour 24 is hour 1 of the same day's
                // records, not the next day's hour 1.
                let next = if hour_in_day == 23 {
                    (k / 24) * 24
                } else {
                    k + 1
                };
                for ts in 1..=n {
                    let wgt = solar_weights[ts - 1];
                    let (prev_w, next_w) = if wgt == 1.0 {
                        (0.0, 0.0)
                    } else if 2 * ts < n {
                        (1.0 - wgt, 0.0)
                    } else {
                        (0.0, 1.0 - wgt)
                    };
                    column[k * n + ts - 1] = self.get(field, prev) * prev_w
                        + self.get(field, k) * wgt
                        + self.get(field, next) * next_w;
                }
            }
        }

        // The held fields: the hour's record across its steps.
        {
            let field = WeatherField::GroundTempC;
            let column = &mut out.ground_temp_c;
            debug_assert_eq!(field_method(field), InterpMethod::HourHold);
            for k in 0..len {
                for ts in 1..=n {
                    column[k * n + ts - 1] = self.get(field, k);
                }
            }
        }
        if let Some(albedo) = self.surface_albedo.as_ref() {
            let out_albedo = out
                .surface_albedo
                .as_mut()
                .expect("the albedo column exists when the source's does");
            for k in 0..len {
                for ts in 1..=n {
                    out_albedo[k * n + ts - 1] = albedo[k];
                }
            }
        }

        // The sky temperature: recomputed from each row's interpolated
        // inputs (`calcSky`, `WeatherManager.cc:3106-3113`), never
        // interpolated itself.
        for row in 0..rows {
            out.sky_temp_c[row] = compute_sky_temp_c(
                out.horizontal_infrared_w_m2[row],
                out.dry_bulb_c[row],
                out.dew_point_c[row],
                out.rel_humidity_pct[row],
                out.opaque_sky_cover[row],
                SkyTempModel::default(),
            );
        }

        // The record indices and weights of the first step of each day,
        // at debug: the interpolation's audit trail.
        for day in 0..len / 24 {
            let k = day * 24;
            let prev = prev_record_index(k, first_hour_k, first_hour_prev, len);
            tracing::debug!(
                day,
                first_step_record = k,
                previous_record = prev,
                weight_this_hour = 1.0 / n as f64,
                "weather interpolation, the day's first step"
            );
        }

        Ok(out)
    }
}

/// The previous hour's record index for the interpolation of hour `k`:
/// the run's first hour reads the record the first-hour rule names, every
/// other hour the previous hour's own record, day boundaries included;
/// the file's first hour carries the file's last record for runs that
/// read past the year's end.
fn prev_record_index(k: usize, first_hour_k: usize, first_hour_prev: usize, len: usize) -> usize {
    if k == first_hour_k {
        first_hour_prev
    } else if k == 0 {
        len - 1
    } else {
        k - 1
    }
}

/// The calendar-date addressing of a weather file's records: which file
/// record a run step at a given local standard time reads. Records are
/// addressed by the local standard time date and hour (the site's
/// standard offset), never by the civil time of a `civil_timezone` with
/// daylight saving, and never by a running index.
///
/// `Typical`: the file's month, day and hour address the records
/// whatever the stamps' years (a run past Dec 31 continues at Jan 1 of
/// the file, wrap by day of year). `Actual`: the run is checked against
/// the file's own dated stamps.
#[derive(Debug, Clone)]
pub struct WeatherAddressing {
    year: WeatherYear,
    /// The file's interval per record, in seconds.
    file_interval_secs: u32,
    records_per_hour: usize,
    /// The file's day count (its records per day × 24).
    file_days: usize,
    /// The file's record count.
    file_len: usize,
    /// `Actual` only: (record date, the record's position within its
    /// day) -> the record's row. The key's position-within-day is the
    /// record's own slot (its stamp's hour at the file's interval), so a
    /// step at local standard time `t` looks up `(t's date, t's seconds
    /// of day / interval)`.
    actual_index: Option<std::collections::HashMap<(chrono::NaiveDate, u32), usize>>,
    /// `Actual` only: the file's dated range, from its first stamp's
    /// interval start to its last stamp's interval end. An `IntervalEnd`
    /// file's range runs from its first stamp less one interval to its
    /// last stamp.
    actual_range: Option<(chrono::NaiveDateTime, chrono::NaiveDateTime)>,
}

impl WeatherAddressing {
    /// The addressing for one parsed file under one weather-year rule.
    ///
    /// # Errors
    ///
    /// [`WeatherError::Validation`] when the file's data period is not a
    /// complete year (naming the period and the inputs that need a
    /// complete year), or `Actual` is requested over a file with no
    /// dated stamps.
    pub fn new(series: &WeatherTimeSeries, year: WeatherYear) -> Result<Self, WeatherError> {
        let period = series.meta.data_period;
        if !period.is_complete_year() {
            return Err(WeatherError::Validation(format!(
                "the weather file's data period {period} is not a complete year: every weather \
                 file must cover Jan 1 to Dec 31, because the water mains annual mean and \
                 monthly range and the ground temperature derived from twelve monthly dry-bulb \
                 means need a complete year; EnergyPlus runs a partial typical period when the \
                 run lies inside it, but HARES derives the mains from the weather file and \
                 substitutes no value it was not given"
            )));
        }
        let file_interval_secs = series.meta.source_step_secs;
        let records_per_hour = usize::from((3600 / file_interval_secs.max(1)) as u16).max(1);
        let file_days = series.len() / (24 * records_per_hour);
        let file_len = series.len();
        let (actual_index, actual_range) = match year {
            WeatherYear::Typical => (None, None),
            WeatherYear::Actual => {
                if series.stamps.len() != series.len() || series.stamps.is_empty() {
                    return Err(WeatherError::Validation(
                        "an actual-year run needs a weather file with dated records; this \
                         weather series carries none"
                            .to_string(),
                    ));
                }
                let mut index = std::collections::HashMap::with_capacity(series.len());
                let mut day_start = 0usize;
                let mut current_date = series.stamps[0].date();
                for (i, stamp) in series.stamps.iter().enumerate() {
                    if stamp.date() != current_date {
                        current_date = stamp.date();
                        day_start = i;
                    }
                    index.insert((current_date, (i - day_start) as u32), i);
                }
                let interval = i64::from(file_interval_secs);
                let stamp_time = |stamp: &RecordStamp| {
                    stamp.date().and_hms_opt(0, 0, 0).expect("midnight")
                        + chrono::Duration::hours(i64::from(stamp.hour))
                };
                let range = match series.meta.record_label {
                    RecordLabel::IntervalEnd => (
                        stamp_time(&series.stamps[0]) - chrono::Duration::seconds(interval),
                        stamp_time(series.stamps.last().expect("non-empty")),
                    ),
                    RecordLabel::IntervalStart => (
                        stamp_time(&series.stamps[0]),
                        stamp_time(series.stamps.last().expect("non-empty"))
                            + chrono::Duration::seconds(interval),
                    ),
                };
                (Some(index), Some(range))
            }
        };
        Ok(Self {
            year,
            file_interval_secs,
            records_per_hour,
            file_days,
            file_len,
            actual_index,
            actual_range,
        })
    }

    /// The weather-year rule the addressing was built under.
    #[must_use]
    pub fn year(&self) -> WeatherYear {
        self.year
    }

    /// The file's day count.
    #[must_use]
    pub fn file_days(&self) -> usize {
        self.file_days
    }

    /// The file's dated range, for `Actual`: from the first stamp's
    /// interval start to the last stamp's interval end.
    #[must_use]
    pub fn actual_range(&self) -> Option<(chrono::NaiveDateTime, chrono::NaiveDateTime)> {
        self.actual_range
    }

    /// The day index (0-based) of `date` in the file's own calendar: a
    /// 366-day file is indexed by the leap calendar's ordinal, a 365-day
    /// file (and any shorter series) by the shared 365-day ordinal, whose
    /// leap-year rule maps a date after Feb 29 one slot back. The modulo
    /// wraps a day index past the file's own days at the file's Jan 1
    /// (wrap by day of year) and keeps short series addressable.
    #[must_use]
    pub fn typical_day_index(&self, date: chrono::NaiveDate) -> u32 {
        let ordinal = if self.file_days == 366 {
            // The file's own leap calendar: the month-and-day slot of a
            // 366-day file, independent of the run year.
            chrono::NaiveDate::from_ymd_opt(2020, date.month(), date.day())
                .expect("a real month and day")
                .ordinal()
        } else {
            hares_types::schedule_day_ordinal(&date)
        };
        (ordinal - 1) % self.file_days.max(1) as u32
    }

    /// The file record (0-based row) the run step starting at `t` (local
    /// standard time) reads.
    ///
    /// # Errors
    ///
    /// [`WeatherError::Validation`] when `Actual` addressing finds no
    /// record for the step's date and hour: the step lies outside the
    /// file's dated records.
    pub fn record_at(&self, t: chrono::NaiveDateTime) -> Result<usize, WeatherError> {
        use chrono::Timelike;
        let secs_of_day = t.hour() * 3600 + t.minute() * 60 + t.second();
        let slot = secs_of_day / self.file_interval_secs;
        match &self.actual_index {
            None => {
                let day = self.typical_day_index(t.date());
                // The modulo wraps a slot past the file's own records at
                // the file's Jan 1 and keeps a short (sub-year) series
                // addressable; for a complete-year file the index is
                // always inside and the modulo is a no-op.
                Ok((day as usize * 24 * self.records_per_hour + slot as usize)
                    % self.file_len.max(1))
            }
            Some(index) => index.get(&(t.date(), slot)).copied().ok_or_else(|| {
                WeatherError::Validation(format!(
                    "no weather record covers local standard time {t}: the step's date and hour \
                     are outside the file's dated records"
                ))
            }),
        }
    }

    /// Whether the run's calendar interval `[start, end)` can be addressed
    /// by this file under the `Actual` rule: every Feb 29 the interval
    /// includes must have records (a run that includes a leap year with a
    /// file that has no Feb 29 fails naming the year), and the interval's
    /// first and last steps must lie inside the file's dated records.
    ///
    /// # Errors
    ///
    /// [`WeatherError::Validation`] naming the year or both ranges.
    pub fn check_actual_run(
        &self,
        start: chrono::NaiveDateTime,
        end: chrono::NaiveDateTime,
        step_secs: u32,
    ) -> Result<(), WeatherError> {
        let Some(range) = self.actual_range else {
            return Ok(());
        };
        // Every Feb 29 the run includes must have records.
        for year in start.year()..=end.year() {
            if !is_leap(year) {
                continue;
            }
            let Some(feb29) = chrono::NaiveDate::from_ymd_opt(year, 2, 29) else {
                continue;
            };
            let feb29_start = feb29.and_hms_opt(0, 0, 0).expect("midnight");
            let feb29_end = feb29
                .succ_opt()
                .expect("a date after Feb 29")
                .and_hms_opt(0, 0, 0)
                .expect("midnight");
            if feb29_start < end && feb29_end > start {
                let covered = self
                    .actual_index
                    .as_ref()
                    .map(|index| index.keys().any(|(date, _)| *date == feb29))
                    .unwrap_or(false);
                if !covered {
                    return Err(WeatherError::Validation(format!(
                        "the actual-year run includes {year}, a leap year, but the weather file \
                         has no Feb 29 records: an actual-year run over a file that does not \
                         cover Feb 29 of a leap year cannot simulate that day"
                    )));
                }
            }
        }
        // The interval's first and last steps must lie inside the file's
        // dated records.
        let last_step = end - chrono::Duration::seconds(i64::from(step_secs));
        for t in [start, last_step] {
            if self.record_at(t).is_err() {
                return Err(WeatherError::Validation(format!(
                    "the actual-year run {} to {} lies outside the weather file's dated records \
                     {} to {}: the run's first and last steps must lie inside the file's own \
                     dates",
                    start.date(),
                    end,
                    range.0,
                    range.1
                )));
            }
        }
        Ok(())
    }
}

fn is_leap(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// The circular interpolation of one direction pair at one weight: the
/// shortest arc, as the engine's circular interpolation has always
/// computed it (EnergyPlus's `interpolateWindDirection`,
/// `WeatherManager.cc:3174-3188`). An endpoint outside `[0, 360)` (a
/// sentinel or NaN) falls back to holding the previous hour's value.
fn circular_interp(prev: f64, cur: f64, weight: f64) -> f64 {
    fn is_valid_angle(v: f64) -> bool {
        v.is_finite() && (0.0..360.0).contains(&v)
    }
    if !is_valid_angle(prev) || !is_valid_angle(cur) {
        return prev;
    }
    let delta = ((cur - prev + 180.0).rem_euclid(360.0)) - 180.0;
    (prev + weight * delta).rem_euclid(360.0)
}

impl std::fmt::Display for RecordLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::IntervalEnd => "interval_end",
            Self::IntervalStart => "interval_start",
        })
    }
}

/// Downsample by computing the mean of consecutive `ratio`-sized blocks.
///
/// Used for instantaneous fields (temperature, pressure, humidity, wind) and
/// period-average solar irradiance when aggregating sub-hourly data to coarser
/// resolution. For solar, mean of sub-interval averages equals the average over
/// the coarser interval, preserving total energy (energy = mean_irradiance x duration).
fn mean_downsample(values: &[f64], ratio: usize) -> Vec<f64> {
    let inv = 1.0 / ratio as f64;
    values
        .chunks_exact(ratio)
        .map(|chunk| chunk.iter().sum::<f64>() * inv)
        .collect()
}

/// Downsample by summing consecutive `ratio`-sized blocks.
///
/// Used for accumulated fields (e.g. precipitation depth) where the
/// coarser-resolution value must equal the total over the sub-intervals.
fn sum_downsample(values: &[f64], ratio: usize) -> Vec<f64> {
    values
        .chunks_exact(ratio)
        .map(|chunk| chunk.iter().sum::<f64>())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        DataPeriod, FirstHour, RecordLabel, WeatherError, WeatherField, WeatherMeta,
        WeatherTimeSeries, WeatherYear, solar_interpolation_weight,
    };
    use chrono::NaiveDate;

    /// The start every test's resample reads from: the file's own first
    /// midnight, unless a test's first-hour rule needs another day.
    fn start_midnight(day: u32) -> chrono::NaiveDateTime {
        NaiveDate::from_ymd_opt(2019, 1, day)
            .expect("a valid date")
            .and_hms_opt(0, 0, 0)
            .expect("a valid midnight")
    }

    fn meta() -> WeatherMeta {
        WeatherMeta {
            location: "Test Site".to_string(),
            latitude: 39.74,
            longitude: -104.99,
            timezone_offset_h: -7.0,
            elevation_m: 1600.0,
            wf_allows_leap_years: true,
            source_step_secs: 3600,
            record_label: RecordLabel::IntervalEnd,
            data_period: DataPeriod::complete_year(),
            has_embedded_location: true,
            station_wmo: None,
        }
    }

    /// A one-day hourly series (24 records) with the hour values the
    /// field-wise `hour_values` closures set, every other field 0.0.
    fn hourly_series(hour_values: &[f64]) -> WeatherTimeSeries {
        assert_eq!(hour_values.len(), 24, "the test series covers one day");
        WeatherTimeSeries {
            meta: meta(),
            design_conditions: None,
            stamps: Vec::new(),
            dry_bulb_c: hour_values.to_vec(),
            dew_point_c: vec![0.0; 24],
            rel_humidity_pct: vec![0.0; 24],
            pressure_kpa: vec![0.0; 24],
            ghi_w_m2: vec![0.0; 24],
            dni_w_m2: vec![0.0; 24],
            dhi_w_m2: vec![0.0; 24],
            wind_speed_m_s: vec![0.0; 24],
            wind_dir_deg: vec![0.0; 24],
            opaque_sky_cover: vec![0.0; 24],
            horizontal_infrared_w_m2: vec![0.0; 24],
            sky_temp_c: vec![0.0; 24],
            ground_temp_c: vec![0.0; 24],
            liquid_precip_m: vec![0.0; 24],
            surface_albedo: None,
        }
    }

    /// A two-day hourly series (48 records): day 1's values then day 2's.
    fn two_day_series(day1: &[f64], day2: &[f64]) -> WeatherTimeSeries {
        let mut series = hourly_series(day1);
        series.dry_bulb_c.extend_from_slice(day2);
        for column in [
            &mut series.dew_point_c,
            &mut series.rel_humidity_pct,
            &mut series.pressure_kpa,
            &mut series.ghi_w_m2,
            &mut series.dni_w_m2,
            &mut series.dhi_w_m2,
            &mut series.wind_speed_m_s,
            &mut series.wind_dir_deg,
            &mut series.opaque_sky_cover,
            &mut series.horizontal_infrared_w_m2,
            &mut series.sky_temp_c,
            &mut series.ground_temp_c,
            &mut series.liquid_precip_m,
        ] {
            column.extend(std::iter::repeat_n(0.0, 24));
        }
        series
    }

    #[test]
    fn get_returns_column_value() {
        let mut series = hourly_series(&[10.0; 24]);
        series.wind_dir_deg[1] = 190.0;
        assert_eq!(series.get(WeatherField::WindDirDeg, 1), 190.0);
        assert_eq!(series.get(WeatherField::PressureKpa, 0), 0.0);
    }

    /// The point fields' published weights: four steps per hour read
    /// `last * (1 - ts/N) + this * ts/N` for ts 1 to 4, bitwise.
    #[test]
    fn point_weights_match_io_reference_four_steps() {
        // Day 1 hour 24 at 0.0, day 2 hour 1 at 4.0; the run starts on
        // day 1, so the day-2 hour-1 steps are no first hour: they read
        // the previous day's hour-24 record as last hour.
        let mut day1 = vec![0.0; 24];
        let mut day2 = vec![0.0; 24];
        day1[23] = 0.0;
        day2[0] = 4.0;
        let series = two_day_series(&day1, &day2);
        let out = series
            .resample(
                900,
                FirstHour::default(),
                start_midnight(1),
                WeatherYear::Typical,
            )
            .expect("resample should succeed");
        assert_eq!(out.len(), 48 * 4);
        let n = 4.0_f64;
        for ts in 1..=4_usize {
            let w = (ts as f64 / n).min(1.0);
            let expected = 0.0 * (1.0 - w) + 4.0 * w;
            assert_eq!(
                out.dry_bulb_c[24 * 4 + ts - 1],
                expected,
                "the day-2 hour-1 step {ts} reads the published weight"
            );
        }
    }

    /// Six steps per hour read the published 1/6-to-6/6 weights.
    #[test]
    fn point_weights_match_io_reference_six_steps() {
        let day1 = vec![0.0; 24];
        let mut day2 = vec![0.0; 24];
        day2[0] = 6.0;
        let series = two_day_series(&day1, &day2);
        let out = series
            .resample(
                600,
                FirstHour::default(),
                start_midnight(1),
                WeatherYear::Typical,
            )
            .expect("resample should succeed");
        let n = 6.0_f64;
        for ts in 1..=6_usize {
            let w = (ts as f64 / n).min(1.0);
            let expected = 0.0 * (1.0 - w) + 6.0 * w;
            assert_eq!(
                out.dry_bulb_c[24 * 6 + ts - 1],
                expected,
                "the day-2 hour-1 step {ts} reads the published weight"
            );
        }
    }

    /// The wind direction interpolates on the circle with the same
    /// weights: 350 degrees to 10 degrees over four steps reads 355, 0,
    /// 5 and 10.
    #[test]
    fn wind_direction_interpolates_on_the_circle() {
        let mut series = hourly_series(&[0.0; 24]);
        series.wind_dir_deg = vec![0.0; 24];
        series.wind_dir_deg[0] = 350.0;
        series.wind_dir_deg[1] = 10.0;
        // The run's first hour is hour 1; read hour 2's rows, whose
        // previous hour is hour 1: 350 to 10 over four steps.
        let out = series
            .resample(
                900,
                FirstHour::default(),
                start_midnight(1),
                WeatherYear::Typical,
            )
            .expect("resample should succeed");
        let expected = [355.0, 0.0, 5.0, 10.0];
        for (ts, expected_value) in expected.iter().enumerate() {
            assert_eq!(
                out.wind_dir_deg[4 + ts],
                *expected_value,
                "the hour-2 step {} interpolates on the circle",
                ts + 1
            );
        }
    }

    /// The precipitation is the weighted hourly depth divided by the
    /// steps per hour: hourly depths 2.0 mm then 6.0 mm at 900 s read
    /// (2.0*0.75 + 6.0*0.25)/4 and so on to 6.0/4.
    #[test]
    fn precipitation_is_interpolated_then_divided_by_steps_per_hour() {
        let mut series = hourly_series(&[0.0; 24]);
        series.liquid_precip_m = vec![0.0; 24];
        series.liquid_precip_m[0] = 2.0;
        series.liquid_precip_m[1] = 6.0;
        let out = series
            .resample(
                900,
                FirstHour::default(),
                start_midnight(1),
                WeatherYear::Typical,
            )
            .expect("resample should succeed");
        let n = 4.0_f64;
        for (ts, (w_prev, w_this)) in [(0.75, 0.25), (0.5, 0.5), (0.25, 0.75), (0.0, 1.0)]
            .iter()
            .enumerate()
        {
            let expected_value = (2.0 * w_prev + 6.0 * w_this) / n;
            assert_eq!(
                out.liquid_precip_m[4 + ts],
                expected_value,
                "the hour-2 step {} divides the weighted depth by the steps per hour",
                ts + 1
            );
        }
    }

    /// A sub-hourly file whose interval differs from the step is a
    /// construction error naming both intervals.
    #[test]
    fn subhourly_file_interval_must_equal_the_step() {
        let mut series = hourly_series(&[0.0; 24]);
        series.meta.source_step_secs = 900;
        for step in [300, 1800] {
            let err = series
                .resample(
                    step,
                    FirstHour::default(),
                    start_midnight(1),
                    WeatherYear::Typical,
                )
                .expect_err("the interval mismatch is an error");
            assert!(err.to_string().contains("900"), "{err}");
            assert!(err.to_string().contains(&step.to_string()), "{err}");
        }
    }

    /// A step that does not divide the hour is an error.
    #[test]
    fn resample_rejects_non_divisor_step() {
        let series = hourly_series(&[0.0; 24]);
        let err = series
            .resample(
                7,
                FirstHour::default(),
                start_midnight(1),
                WeatherYear::Typical,
            )
            .expect_err("non-divisor timestep should fail");
        assert!(matches!(err, WeatherError::Resample(_)));
        assert!(err.to_string().contains("divide the hour"), "{err}");
    }

    #[test]
    fn resample_same_resolution_returns_clone() {
        let series = hourly_series(&[0.0; 24]);
        let resampled = series
            .resample(
                3600,
                FirstHour::default(),
                start_midnight(1),
                WeatherYear::Typical,
            )
            .expect("same-res should succeed");
        assert_eq!(resampled, series);
    }

    /// An hourly file at a step longer than an hour keeps the interval
    /// mean of the records the step covers.
    #[test]
    fn resample_downsample_mean() {
        let mut day1 = vec![0.0; 24];
        day1[0] = 10.0;
        day1[1] = 12.0;
        day1[2] = 14.0;
        day1[3] = 16.0;
        let series = two_day_series(&day1, &[0.0; 24]);
        let down = series
            .resample(
                7200,
                FirstHour::default(),
                start_midnight(1),
                WeatherYear::Typical,
            )
            .expect("downsample should succeed");
        assert_eq!(down.len(), 24);
        // The two-hour step's mean covers records 0 and 1: 10 and 12.
        assert!((down.dry_bulb_c[0] - 11.0).abs() < 1e-12);
        assert_eq!(down.meta.source_step_secs, 7200);
    }

    /// The ground temperature and surface albedo hold the hour's record.
    #[test]
    fn ground_and_albedo_hold_the_hours_record() {
        let mut series = hourly_series(&[0.0; 24]);
        series.ground_temp_c = (0..24).map(|h| 10.0 + h as f64 * 0.1).collect();
        series.surface_albedo = Some((0..24).map(|h| 0.2 + h as f64 * 0.01).collect());
        let out = series
            .resample(
                900,
                FirstHour::default(),
                start_midnight(1),
                WeatherYear::Typical,
            )
            .expect("resample should succeed");
        for h in 0..24 {
            for ts in 1..=4 {
                let row = h * 4 + ts - 1;
                assert_eq!(out.ground_temp_c[row], 10.0 + h as f64 * 0.1);
                assert_eq!(
                    out.surface_albedo.as_ref().expect("albedo")[row],
                    0.2 + h as f64 * 0.01
                );
            }
        }
    }

    /// The solar weights reproduce EnergyPlus's table for four and six
    /// steps: records 400 (previous hour), 800 (this hour), 0 (next
    /// hour); the four 900 s steps of the this-hour read 700, 800, 600,
    /// 400; the six 600 s steps read the N = 6 weights' values.
    #[test]
    fn solar_weights_match_energyplus_four_steps() {
        let mut series = hourly_series(&[0.0; 24]);
        series.ghi_w_m2 = vec![0.0; 24];
        series.ghi_w_m2[0] = 400.0;
        series.ghi_w_m2[1] = 800.0;
        // The run starts at hour 2: hour 2's rows read prev = hour 1
        // (400), this = hour 2 (800), next = hour 3 (0).
        let out = series
            .resample(
                900,
                FirstHour::default(),
                start_midnight(1),
                WeatherYear::Typical,
            )
            .expect("resample should succeed");
        let expected = [700.0, 800.0, 600.0, 400.0];
        for (ts, expected_value) in expected.iter().enumerate() {
            assert_eq!(
                out.ghi_w_m2[4 + ts],
                *expected_value,
                "the hour-2 step {} reads the published solar weight",
                ts + 1
            );
        }
    }

    /// Six steps per hour: the N = 6 weights (4/6, 5/6, 1.0, 5/6, 4/6,
    /// 3/6) on records 400 previous, 800 this, 0 next.
    #[test]
    fn solar_weights_match_energyplus_six_steps() {
        let mut series = hourly_series(&[0.0; 24]);
        series.ghi_w_m2 = vec![0.0; 24];
        series.ghi_w_m2[0] = 400.0;
        series.ghi_w_m2[1] = 800.0;
        let out = series
            .resample(
                600,
                FirstHour::default(),
                start_midnight(1),
                WeatherYear::Typical,
            )
            .expect("resample should succeed");
        let weights: Vec<f64> = (1..=6)
            .map(|ts| solar_interpolation_weight(6, ts))
            .collect();
        for ts in 1..=6_usize {
            let wgt = weights[ts - 1];
            let (prev_w, next_w) = if wgt == 1.0 {
                (0.0, 0.0)
            } else if 2 * ts < 6 {
                (1.0 - wgt, 0.0)
            } else {
                (0.0, 1.0 - wgt)
            };
            let expected = 400.0 * prev_w + 800.0 * wgt + 0.0 * next_w;
            assert_eq!(
                out.ghi_w_m2[6 + ts - 1],
                expected,
                "the hour-2 step {ts} reads the published solar weight"
            );
        }
    }

    /// The weight function reproduces EnergyPlus's odd-N tables: 5/6,
    /// 5/6, 0.5 for N = 3, and `1 - 1/10` for the two middle steps of
    /// N = 5.
    #[test]
    fn solar_weights_match_energyplus_odd_steps() {
        let n3: Vec<f64> = (1..=3)
            .map(|ts| solar_interpolation_weight(3, ts))
            .collect();
        assert_eq!(n3, vec![5.0 / 6.0, 5.0 / 6.0, 0.5]);
        let n5: Vec<f64> = (1..=5)
            .map(|ts| solar_interpolation_weight(5, ts))
            .collect();
        assert_eq!(n5[1], 1.0 - 1.0 / 10.0);
        assert_eq!(n5[2], 1.0 - 1.0 / 10.0);
        // The even and odd tables' boundary steps: N = 1 takes 0.5.
        assert_eq!(solar_interpolation_weight(1, 1), 0.5);
    }

    /// After the half hour of hour 24, the next-hour value is hour 1 of
    /// the same day's records, never the next day's hour 1.
    #[test]
    fn solar_hour24_takes_hour1_of_the_same_day_after_the_half_hour() {
        let mut day1 = vec![0.0; 24];
        let mut day2 = vec![0.0; 24];
        day1[0] = 100.0;
        day1[22] = 300.0;
        day1[23] = 500.0;
        day2[0] = 900.0;
        let mut series = two_day_series(&day1, &day2);
        // The GHI records: day 1's hours 1, 23 and 24, and the next
        // day's hour 1, carry the test's values.
        series.ghi_w_m2 = vec![0.0; 48];
        series.ghi_w_m2[0] = 100.0;
        series.ghi_w_m2[22] = 300.0;
        series.ghi_w_m2[23] = 500.0;
        series.ghi_w_m2[24] = 900.0;
        let out = series
            .resample(
                900,
                FirstHour::default(),
                start_midnight(1),
                WeatherYear::Typical,
            )
            .expect("resample should succeed");
        let expected = [
            500.0 * 0.75 + 300.0 * 0.25,
            500.0,
            500.0 * 0.75 + 100.0 * 0.25,
            500.0 * 0.5 + 100.0 * 0.5,
        ];
        for (ts, expected_value) in expected.iter().enumerate() {
            assert_eq!(
                out.ghi_w_m2[23 * 4 + ts],
                *expected_value,
                "the hour-24 step {} blends with the same day's hour 1, never the next day's",
                ts + 1
            );
        }
    }

    /// The run's first hour's solar previous-hour record is the record
    /// the first-hour rule names: hour 24 of the first day by default,
    /// hour 1 under `Hour1`.
    #[test]
    fn solar_first_hour_uses_hour24_of_the_first_day() {
        let mut series = hourly_series(&[0.0; 24]);
        series.ghi_w_m2[0] = 100.0;
        series.ghi_w_m2[1] = 200.0;
        series.ghi_w_m2[23] = 500.0;
        let out = series
            .resample(
                900,
                FirstHour::default(),
                start_midnight(1),
                WeatherYear::Typical,
            )
            .expect("resample should succeed");
        // Under Hour24: the first hour's previous-hour record is hour 24
        // (500). The four steps read (100 * 0.75 + 500 * 0.25), 100,
        // (100 * 0.75 + 200 * 0.25) and (100 * 0.5 + 200 * 0.5).
        let expected = [
            100.0 * 0.75 + 500.0 * 0.25,
            100.0,
            100.0 * 0.75 + 200.0 * 0.25,
            100.0 * 0.5 + 200.0 * 0.5,
        ];
        for (ts, expected_value) in expected.iter().enumerate() {
            assert_eq!(
                out.ghi_w_m2[ts],
                *expected_value,
                "the run's first hour's step {} reads the first-day rule's record",
                ts + 1
            );
        }
        // Under Hour1: the previous-hour record is hour 1 itself, so the
        // first step reads 100 throughout and the rest are unchanged.
        let out_hour1 = series
            .resample(
                900,
                FirstHour::Hour1,
                start_midnight(1),
                WeatherYear::Typical,
            )
            .expect("resample should succeed");
        assert_eq!(out_hour1.ghi_w_m2[0], 100.0);
        assert_eq!(out_hour1.ghi_w_m2[1], 100.0);
        assert_eq!(out_hour1.ghi_w_m2[2], 100.0 * 0.75 + 200.0 * 0.25);
        assert_eq!(out_hour1.ghi_w_m2[3], 100.0 * 0.5 + 200.0 * 0.5);
    }

    /// The warm-up days repeat the run's first day: the per-step weather
    /// evaluation is a function of the step index, and a warm-up day
    /// replays the run's first day's step indices (the loop resets the
    /// clock's step counter), so the first hour of warm-up day 1, of
    /// warm-up day 2 and of the run's first day each read the same four
    /// solar values. The rows the second day's indices would carry if the
    /// clock did not reset are different (they read the previous day's
    /// hour 24), so a loop that forgot the reset would fail here.
    #[test]
    fn warmup_days_repeat_the_first_hour_rule() {
        let mut series = hourly_series(&[0.0; 24]);
        series.ghi_w_m2[0] = 100.0;
        series.ghi_w_m2[1] = 200.0;
        series.ghi_w_m2[23] = 500.0;
        let out = series
            .resample(
                900,
                FirstHour::default(),
                start_midnight(1),
                WeatherYear::Typical,
            )
            .expect("resample should succeed");
        // The first hour's four steps, the rows steps 0 to 3 read on every
        // warm-up day and on the run's first day alike: (200, 100, 125,
        // 150) under Hour24.
        let first_hour: Vec<f64> = out.ghi_w_m2[..4].to_vec();
        let expected = [
            100.0 * 0.75 + 500.0 * 0.25,
            100.0,
            100.0 * 0.75 + 200.0 * 0.25,
            100.0 * 0.5 + 200.0 * 0.5,
        ];
        assert_eq!(
            first_hour, expected,
            "the first hour's rows carry the first-hour rule"
        );
        // Every warm-up day replays the run's first day's step indices (the
        // loop resets the clock's step counter), so the first hour of
        // warm-up day 1, of warm-up day 2 and of the run's first day each
        // read these same four rows: the evaluation is a function of the
        // step index, and the rows are the table's own 0 to 3.
        assert_eq!(
            out.ghi_w_m2[..4],
            expected,
            "the rows the warm-up days and the run's first day read are the table's first four"
        );
    }

    #[test]
    fn detect_epw_by_extension() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("test.epw");
        std::fs::write(&path, "dummy").expect("write temp file");
        let fmt = super::detect_weather_format(&path).expect("should detect EPW");
        assert_eq!(fmt, super::WeatherFormat::Epw);
    }

    #[test]
    fn detect_psm3_csv() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("weather.csv");
        let header = "\
Source,Location ID,City,State,Country,Latitude,Longitude,Time Zone,Elevation,Local Time Zone\n\
NSRDB,155561,-,-,-,40.53,-105.06,-7,1525,-7\n\
Year,Month,Day,Hour,Minute,DHI,DNI,GHI,Temperature,Pressure,Dew Point,Relative Humidity,Wind Speed,Wind Direction\n";
        std::fs::write(&path, header).expect("write temp file");
        let fmt = super::detect_weather_format(&path).expect("should detect PSM3");
        assert_eq!(fmt, super::WeatherFormat::Psm3);
    }

    #[test]
    fn reject_unknown_csv() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("data.csv");
        std::fs::write(&path, "col1,col2,col3\n1,2,3\n").expect("write temp file");
        let err = super::detect_weather_format(&path).expect_err("should reject non-PSM3 CSV");
        assert!(
            err.to_string().contains("PSM3"),
            "error should mention PSM3: {err}"
        );
    }

    #[test]
    fn reject_unknown_extension() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("data.json");
        std::fs::write(&path, "{}").expect("write temp file");
        let err = super::detect_weather_format(&path).expect_err("should reject .json");
        assert!(
            err.to_string().contains("supported formats"),
            "error should list supported formats: {err}"
        );
    }

    /// The sky temperature is recomputed from each row's interpolated
    /// inputs, never interpolated itself: T_sky is a non-linear function
    /// of its inputs, so direct interpolation violates the chain rule
    /// (E+ WeatherManager.cc:3106-3113).
    #[test]
    fn sky_temp_recomputed_from_interpolated_inputs() {
        use crate::epw::{SkyTempModel, compute_sky_temp_c};

        let mut series = hourly_series(&[0.0; 24]);
        series.horizontal_infrared_w_m2 = (0..24).map(|h| 200.0 + f64::from(h) * 5.0).collect();
        series.dry_bulb_c = (0..24).map(|h| 10.0 + f64::from(h)).collect();
        series.dew_point_c = (0..24).map(|h| 5.0 + f64::from(h) * 0.2).collect();
        series.opaque_sky_cover = vec![4.0; 24];
        series.rel_humidity_pct = vec![50.0; 24];

        let resampled = series
            .resample(
                900,
                FirstHour::default(),
                start_midnight(1),
                WeatherYear::Typical,
            )
            .expect("resample should succeed");
        assert_eq!(resampled.len(), 96);

        for i in 0..resampled.len() {
            let expected = compute_sky_temp_c(
                resampled.horizontal_infrared_w_m2[i],
                resampled.dry_bulb_c[i],
                resampled.dew_point_c[i],
                resampled.rel_humidity_pct[i],
                resampled.opaque_sky_cover[i],
                SkyTempModel::default(),
            );
            assert_eq!(
                resampled.sky_temp_c[i], expected,
                "row {i}: recomputed sky_temp_c must equal compute_sky_temp_c \
                 from interpolated inputs"
            );
        }
    }

    /// The first hour of the run interpolates from hour 24 of the run's
    /// first day, by default: at 900 s the four steps of hour 1 read
    /// `h24 * 0.75 + h1 * 0.25` and so on, never a record from before the
    /// run's first day.
    #[test]
    fn first_hour_uses_hour24_of_the_first_day_by_default() {
        let mut series = hourly_series(&[0.0; 24]);
        series.dry_bulb_c[0] = 4.0;
        series.dry_bulb_c[23] = 40.0;
        let out = series
            .resample(
                900,
                FirstHour::default(),
                start_midnight(1),
                WeatherYear::Typical,
            )
            .expect("resample should succeed");
        let n = 4.0_f64;
        for ts in 1..=4_usize {
            let w = (ts as f64 / n).min(1.0);
            let expected = 40.0 * (1.0 - w) + 4.0 * w;
            assert_eq!(
                out.dry_bulb_c[ts - 1],
                expected,
                "the run's first hour's step {ts} interpolates from the first day's hour 24"
            );
        }
    }

    /// With `Hour1` selected, the first hour's previous-hour record is the
    /// hour-1 record itself: the first hour's steps read the hour-1
    /// record's value throughout.
    #[test]
    fn first_hour_uses_hour1_when_selected() {
        let mut series = hourly_series(&[0.0; 24]);
        series.dry_bulb_c[0] = 4.0;
        series.dry_bulb_c[23] = 40.0;
        let out = series
            .resample(
                900,
                FirstHour::Hour1,
                start_midnight(1),
                WeatherYear::Typical,
            )
            .expect("resample should succeed");
        for ts in 1..=4_usize {
            assert_eq!(
                out.dry_bulb_c[ts - 1],
                4.0,
                "the run's first hour's step {ts} reads the hour-1 record"
            );
        }
        // The second hour interpolates from hour 1 normally.
        assert_eq!(out.dry_bulb_c[4], 4.0 * 0.75 + 0.0 * 0.25);
    }

    /// A run whose first day is not the file's first day applies the
    /// first-hour rule to the run's own first day.
    #[test]
    fn first_hour_rule_uses_the_runs_first_day() {
        let mut day1 = vec![0.0; 24];
        let mut day2 = vec![0.0; 24];
        day1[23] = 111.0;
        day2[0] = 4.0;
        day2[23] = 222.0;
        let series = two_day_series(&day1, &day2);
        let out = series
            .resample(
                900,
                FirstHour::default(),
                start_midnight(2),
                WeatherYear::Typical,
            )
            .expect("resample should succeed");
        let n = 4.0_f64;
        for ts in 1..=4_usize {
            let w = (ts as f64 / n).min(1.0);
            let expected = 222.0 * (1.0 - w) + 4.0 * w;
            assert_eq!(
                out.dry_bulb_c[24 * 4 + ts - 1],
                expected,
                "the run's first hour (day 2's hour 1) interpolates from day 2's hour 24"
            );
        }
    }

    /// Every weather field has one declared method, and the match reads
    /// the field table: linear last-hour for the point fields, the circle
    /// for the wind direction, the divided depth for precipitation, the
    /// hour's record for ground temperature and albedo, the recomputation
    /// for the sky temperature.
    #[test]
    fn every_weather_field_has_one_declared_method() {
        use super::{InterpMethod, field_method};
        let expected: &[(WeatherField, InterpMethod)] = &[
            (WeatherField::DryBulbC, InterpMethod::LastHour),
            (WeatherField::DewPointC, InterpMethod::LastHour),
            (WeatherField::RelHumidityPct, InterpMethod::LastHour),
            (WeatherField::PressureKpa, InterpMethod::LastHour),
            (WeatherField::WindSpeedMS, InterpMethod::LastHour),
            (WeatherField::OpaqueSkyCover, InterpMethod::LastHour),
            (WeatherField::HorizontalInfrared, InterpMethod::LastHour),
            (WeatherField::WindDirDeg, InterpMethod::LastHourCircular),
            (WeatherField::LiquidPrecipM, InterpMethod::LastHourDivided),
            (WeatherField::GhiWM2, InterpMethod::SolarMidpoint),
            (WeatherField::DniWM2, InterpMethod::SolarMidpoint),
            (WeatherField::DhiWM2, InterpMethod::SolarMidpoint),
            (WeatherField::SkyTempC, InterpMethod::Recompute),
            (WeatherField::GroundTempC, InterpMethod::HourHold),
            (WeatherField::SurfaceAlbedo, InterpMethod::HourHold),
        ];
        for (field, method) in expected {
            assert_eq!(&field_method(*field), method, "{field:?}");
        }
    }

    /// The circular interpolation's shortest arc: 350 to 10 degrees
    /// crosses 0, and an endpoint outside [0, 360) holds the previous
    /// hour's value.
    #[test]
    fn circular_interp_shortest_arc_and_sentinels() {
        use super::circular_interp;
        assert_eq!(circular_interp(350.0, 10.0, 0.5), 0.0);
        assert_eq!(circular_interp(350.0, 10.0, 1.0), 10.0);
        assert_eq!(circular_interp(10.0, 350.0, 0.5), 0.0);
        assert_eq!(circular_interp(180.0, 190.0, 0.5), 185.0);
        // A sentinel endpoint holds the previous hour's value.
        assert_eq!(circular_interp(180.0, f64::NAN, 0.5), 180.0);
        assert_eq!(circular_interp(180.0, -9999.0, 0.5), 180.0);
        assert_eq!(circular_interp(180.0, 360.0, 0.5), 180.0);
    }

    /// A stamped hourly series: `count` records whose stamps run from
    /// the stated first stamp (year, month, day, hour 1) forward one
    /// hour per record, under the stated record label.
    fn stamped_series(
        year: i32,
        month: u32,
        day: u32,
        count: usize,
        label: RecordLabel,
    ) -> WeatherTimeSeries {
        use super::RecordStamp;
        use chrono::Datelike;
        use chrono::Timelike;

        let start = NaiveDate::from_ymd_opt(year, month, day)
            .expect("a valid start date")
            .and_hms_opt(0, 0, 0)
            .expect("midnight");
        let stamps = (0..count)
            .map(|k| {
                let t = start + chrono::Duration::hours(k as i64 + 1);
                RecordStamp {
                    year: t.year(),
                    month: t.month() as u8,
                    day: t.day() as u8,
                    hour: t.hour() as u8,
                    minute: 0,
                }
            })
            .collect();
        let mut series = hourly_series(&[0.0; 24]);
        series.stamps = stamps;
        series.meta.record_label = label;
        // Pad every column from the helper's one day to the record count
        // so the series is self-consistent.
        for column in [
            &mut series.dry_bulb_c,
            &mut series.dew_point_c,
            &mut series.rel_humidity_pct,
            &mut series.pressure_kpa,
            &mut series.ghi_w_m2,
            &mut series.dni_w_m2,
            &mut series.dhi_w_m2,
            &mut series.wind_speed_m_s,
            &mut series.wind_dir_deg,
            &mut series.opaque_sky_cover,
            &mut series.horizontal_infrared_w_m2,
            &mut series.sky_temp_c,
            &mut series.ground_temp_c,
            &mut series.liquid_precip_m,
        ] {
            column.extend(core::iter::repeat_n(0.0, count.saturating_sub(24)));
        }
        series
    }

    /// The file's own year of the B-9 evidence file's shape: an
    /// `IntervalEnd` AMY 2018 file stamped 2018-01-01 01:00 to
    /// 2019-01-01 00:00 covers the hours of 2018, not the year of its
    /// last stamp.
    #[test]
    fn file_year_of_an_interval_end_amy_file_is_the_year_its_hours_cover() {
        use chrono::Datelike;

        let series = stamped_series(2018, 1, 1, 8760, RecordLabel::IntervalEnd);
        let first = series.stamps.first().expect("stamps").date();
        let last = series.stamps.last().expect("stamps").date();
        assert_eq!((first.year(), first.month(), first.day()), (2018, 1, 1));
        assert_eq!(
            (last.year(), last.month(), last.day()),
            (2019, 1, 1),
            "the last stamp is the next Jan 1 midnight, the hour-ending convention"
        );
        assert_eq!(
            series.file_year(),
            Some(2018),
            "the covered hours are the hours of 2018, not the last stamp's year"
        );
    }

    /// An `IntervalStart` file's covered interval ends one interval after
    /// its last stamp: a 2019 file stamped Jan 1 00:00 to Dec 31 23:00
    /// covers the hours of 2019.
    #[test]
    fn file_year_of_an_interval_start_file_is_the_year_its_hours_cover() {
        let series = stamped_series(2019, 1, 1, 8760, RecordLabel::IntervalStart);
        assert_eq!(series.file_year(), Some(2019));
    }

    /// A series with no dated stamps (built in code) carries no file
    /// year.
    #[test]
    fn file_year_of_a_series_without_stamps_is_none() {
        let series = hourly_series(&[0.0; 24]);
        assert_eq!(series.file_year(), None);
    }
}
