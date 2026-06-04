//! EnergyPlus Weather (EPW) file parser.
//!
//! ## Leap year handling
//!
//! HARES supports both 8760-row (standard year) and 8784-row (leap year) EPW
//! files. Unlike OCHRE, which strips February 29 data, HARES preserves the full
//! leap year data. This means annual simulations from a leap-year EPW will have
//! 366 days of weather, and schedules/weather indexing use modular wrap-around
//! to handle multi-year or cross-year simulations correctly.

use std::fs;
use std::path::Path;

use chrono::{Datelike, NaiveDate};

use hares_physics::constants::{
    CELSIUS_TO_KELVIN as KELVIN_OFFSET_C, HOURS_PER_YEAR, STEFAN_BOLTZMANN,
};
use hares_types::parse_trimmed_f64;
use tracing::{debug, warn};

use crate::weather::{WeatherError, WeatherMeta, WeatherTimeSeries};

const EXPECTED_RECORDS_STANDARD: usize = 8760;
const EXPECTED_RECORDS_LEAP: usize = 8784;
const LOCATION_MIN_FIELDS: usize = 10;
const DATA_PERIOD_MIN_FIELDS: usize = 3;
const EPW_RECORD_MIN_FIELDS: usize = 24;

const IDX_YEAR: usize = 0;
const IDX_MONTH: usize = 1;
const IDX_DAY: usize = 2;
const IDX_HOUR: usize = 3;

const IDX_DRY_BULB_C: usize = 6;
const IDX_DEW_POINT_C: usize = 7;
const IDX_REL_HUMIDITY_PCT: usize = 8;
const IDX_PRESSURE_PA: usize = 9;
const IDX_HORIZONTAL_INFRARED: usize = 12;
const IDX_GHI_W_M2: usize = 13;
const IDX_DNI_W_M2: usize = 14;
const IDX_DHI_W_M2: usize = 15;
const IDX_WIND_DIR_DEG: usize = 20;
const IDX_WIND_SPEED_M_S: usize = 21;
const IDX_OPAQUE_SKY_COVER: usize = 23;
const IDX_LIQUID_PRECIP_DEPTH_MM: usize = 33;

/// Parsed design conditions from EPW header line 2.
///
/// Extracted from the "Extremes" section at the end of the design conditions line.
/// `heating_design_db_c` is the minimum of all extreme low dry-bulb temperatures;
/// `cooling_design_db_c` is the maximum of all extreme high dry-bulb temperatures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DesignConditions {
    pub heating_design_db_c: f64,
    pub cooling_design_db_c: f64,
}

/// One parsed EPW weather row.
#[derive(Debug, Clone, PartialEq)]
pub struct EpwRecord {
    pub dry_bulb_c: f64,
    pub dew_point_c: f64,
    pub rel_humidity_pct: f64,
    pub pressure_kpa: f64,
    pub ghi_w_m2: f64,
    pub dni_w_m2: f64,
    pub dhi_w_m2: f64,
    pub wind_speed_m_s: f64,
    pub wind_dir_deg: f64,
    pub opaque_sky_cover: f64,
    pub horizontal_infrared_w_m2: f64,
    pub sky_temp_c: f64,
    pub ground_temp_c: f64,
    /// Liquid precipitation depth [m]. Zero when EPW field 33 is absent or invalid.
    pub liquid_precip_m: f64,
}

/// Parse an EPW file into an hourly weather time series.
pub fn parse_epw<P: AsRef<Path>>(path: P) -> Result<WeatherTimeSeries, WeatherError> {
    let path_ref = path.as_ref();
    let contents = fs::read_to_string(path_ref).map_err(|source| WeatherError::Io {
        path: path_ref.display().to_string(),
        source,
    })?;

    parse_epw_str(&contents)
}

fn parse_epw_str(contents: &str) -> Result<WeatherTimeSeries, WeatherError> {
    let mut lines = contents.lines();

    let location_line = lines
        .next()
        .ok_or_else(|| WeatherError::Parse("missing EPW location header".to_string()))?;
    let design_conditions_line = lines
        .next()
        .ok_or_else(|| WeatherError::Parse("missing EPW design-conditions header".to_string()))?;
    let ground_temp_line = lines
        .next()
        .ok_or_else(|| WeatherError::Parse("missing EPW ground-temperature header".to_string()))?;
    let typical_extreme_line = lines
        .next()
        .ok_or_else(|| WeatherError::Parse("missing EPW typical/extreme header".to_string()))?;
    let holidays_daylight_line = lines
        .next()
        .ok_or_else(|| WeatherError::Parse("missing EPW holidays/daylight header".to_string()))?;
    let comments_1_line = lines
        .next()
        .ok_or_else(|| WeatherError::Parse("missing EPW comments 1 header".to_string()))?;
    let comments_2_line = lines
        .next()
        .ok_or_else(|| WeatherError::Parse("missing EPW comments 2 header".to_string()))?;
    let data_period_line = lines
        .next()
        .ok_or_else(|| WeatherError::Parse("missing EPW data-period header".to_string()))?;

    let design_conditions = parse_design_conditions(design_conditions_line);
    let _ = typical_extreme_line;

    // Parse the HOLIDAYS/DAYLIGHT SAVINGS header to extract field A1
    // (Leap Year Observed) in order to gate Feb 29 data processing.
    // EnergyPlus stores this in `WFAllowsLeapYears` (WeatherManager.cc:7889).
    let wf_allows_leap_years = parse_holidays_daylight_header(holidays_daylight_line)?;
    let _ = comments_1_line;
    let _ = comments_2_line;

    let mut meta = parse_location_header(location_line)?;
    ensure_hourly_data_period(data_period_line)?;
    let epw_ground_temps = parse_ground_temperatures(ground_temp_line);

    let mut records = Vec::new();
    let mut record_datetimes = Vec::new();
    let mut precip_field_absent = false;
    let mut precip_sentinel_seen = false;

    for (data_index, line) in lines.filter(|l| !l.trim().is_empty()).enumerate() {
        let row = data_index + 1;
        let fields: Vec<&str> = line.split(',').collect();
        if fields.len() < EPW_RECORD_MIN_FIELDS {
            return Err(WeatherError::Parse(format!(
                "row {row}: expected at least {EPW_RECORD_MIN_FIELDS} fields, got {}",
                fields.len()
            )));
        }

        let year = parse_i32(fields[IDX_YEAR], row, "year")?;
        let month = parse_u32(fields[IDX_MONTH], row, "month")?;
        let day = parse_u32(fields[IDX_DAY], row, "day")?;
        let hour = parse_u32(fields[IDX_HOUR], row, "hour")?;
        if !(1..=24).contains(&hour) {
            return Err(WeatherError::Validation(format!(
                "row {row}: hour must be in 1..=24, got {hour}"
            )));
        }
        let date = NaiveDate::from_ymd_opt(year, month, day).ok_or_else(|| {
            WeatherError::Validation(format!(
                "row {row}: invalid date year={year}, month={month}, day={day}"
            ))
        })?;

        let dry_bulb_c = parse_f64(fields[IDX_DRY_BULB_C], row, "dry_bulb_c")?;
        if !(-60.0..=55.0).contains(&dry_bulb_c) {
            return Err(WeatherError::Validation(format!(
                "row {row}: dry bulb out of range [-60, 55] C: {dry_bulb_c}"
            )));
        }

        let dew_point_c = parse_f64(fields[IDX_DEW_POINT_C], row, "dew_point_c")?;
        if dew_point_c > dry_bulb_c {
            return Err(WeatherError::Validation(format!(
                "row {row}: dew point ({dew_point_c}) exceeds dry bulb ({dry_bulb_c})"
            )));
        }

        let rel_humidity_pct = parse_f64(fields[IDX_REL_HUMIDITY_PCT], row, "rel_humidity_pct")?;

        let pressure_kpa = parse_f64(fields[IDX_PRESSURE_PA], row, "pressure_pa")? / 1000.0;
        if !(60.0..=110.0).contains(&pressure_kpa) {
            return Err(WeatherError::Validation(format!(
                "row {row}: pressure out of range [60, 110] kPa: {pressure_kpa}"
            )));
        }

        let ghi_w_m2 = parse_f64(fields[IDX_GHI_W_M2], row, "ghi_w_m2")?;
        if !(0.0..=1500.0).contains(&ghi_w_m2) {
            return Err(WeatherError::Validation(format!(
                "row {row}: GHI out of range [0, 1500] W/m^2: {ghi_w_m2}"
            )));
        }

        let dni_w_m2 = parse_f64(fields[IDX_DNI_W_M2], row, "dni_w_m2")?;
        let dhi_w_m2 = parse_f64(fields[IDX_DHI_W_M2], row, "dhi_w_m2")?;

        let wind_dir_deg = parse_f64(fields[IDX_WIND_DIR_DEG], row, "wind_dir_deg")?;
        let wind_speed_m_s = parse_f64(fields[IDX_WIND_SPEED_M_S], row, "wind_speed_m_s")?;
        if !(0.0..=60.0).contains(&wind_speed_m_s) {
            return Err(WeatherError::Validation(format!(
                "row {row}: wind speed out of range [0, 60] m/s: {wind_speed_m_s}"
            )));
        }

        let opaque_sky_cover = parse_f64(fields[IDX_OPAQUE_SKY_COVER], row, "opaque_sky_cover")?;

        let horizontal_infrared_w_m2 =
            parse_f64(fields[IDX_HORIZONTAL_INFRARED], row, "horizontal_infrared")?;
        if !(0.0..=700.0).contains(&horizontal_infrared_w_m2) {
            return Err(WeatherError::Validation(format!(
                "row {row}: horizontal infrared out of range [0, 700] W/m²: {horizontal_infrared_w_m2}"
            )));
        }

        let sky_temp_c = compute_sky_temp_c(
            horizontal_infrared_w_m2,
            dry_bulb_c,
            dew_point_c,
            rel_humidity_pct,
            opaque_sky_cover,
            SkyTempModel::default(),
        );

        let liquid_precip_m = if fields.len() > IDX_LIQUID_PRECIP_DEPTH_MM {
            let raw_field = fields[IDX_LIQUID_PRECIP_DEPTH_MM];
            match parse_f64(raw_field, row, "liquid_precip_mm") {
                Ok(value) if value >= 900.0 => {
                    // EPW sentinel for field 33 (Liquid Precipitation Depth) is 999
                    // per EnergyPlus IDD: N33, \missing 999.
                    // Threshold 900.0 catches 999 and common sentinel variants.
                    precip_sentinel_seen = true;
                    0.0
                }
                Ok(value) => value.max(0.0) / 1000.0,
                Err(_) => {
                    warn!(
                        row = row,
                        raw = raw_field.trim(),
                        "field 33 (liquid_precip_mm) parse error; substituting 0.0"
                    );
                    0.0
                }
            }
        } else {
            precip_field_absent = true;
            0.0
        };

        records.push(EpwRecord {
            dry_bulb_c,
            dew_point_c,
            rel_humidity_pct,
            pressure_kpa,
            ghi_w_m2,
            dni_w_m2,
            dhi_w_m2,
            wind_speed_m_s,
            wind_dir_deg,
            opaque_sky_cover,
            horizontal_infrared_w_m2,
            sky_temp_c,
            ground_temp_c: f64::NAN,
            liquid_precip_m,
        });
        record_datetimes.push((date, hour));
    }

    if precip_field_absent {
        debug!(
            "EPW file has no precipitation data (field 33 absent); liquid_precip_m set to 0.0 for all rows"
        );
    }
    if precip_sentinel_seen {
        debug!(
            "EPW file contains missing-data sentinel in field 33 (Liquid Precipitation Depth >= 900 mm); treated as 0.0"
        );
    }

    if records.len() != EXPECTED_RECORDS_STANDARD && records.len() != EXPECTED_RECORDS_LEAP {
        return Err(WeatherError::Validation(format!(
            "EPW record count must be {EXPECTED_RECORDS_STANDARD} or {EXPECTED_RECORDS_LEAP}, got {}",
            records.len()
        )));
    }

    let is_leap_year = records.len() == EXPECTED_RECORDS_LEAP;

    // EnergyPlus WeatherManager.cc:2816-2827: if WFAllowsLeapYears is false,
    // Feb 29 data is discarded and February is treated as a 28-day month.
    if is_leap_year && !wf_allows_leap_years {
        warn!(
            "EPW header declares Leap Year Observed = No, but file contains 8784 rows. Discarding Feb 29 data."
        );
        // Retain only records that are NOT Feb 29.
        let mut retained = Vec::with_capacity(EXPECTED_RECORDS_STANDARD);
        let mut retained_dt = Vec::with_capacity(EXPECTED_RECORDS_STANDARD);
        for (rec, dt) in records.into_iter().zip(record_datetimes) {
            if !(dt.0.month() == 2 && dt.0.day() == 29) {
                retained.push(rec);
                retained_dt.push(dt);
            }
        }
        records = retained;
        record_datetimes = retained_dt;
    }

    let is_leap_year = records.len() == EXPECTED_RECORDS_LEAP;

    // Invariant: if the file has 8784 records but the header says "No" for
    // leap year observation, Feb 29 must have been stripped.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        if records.len() == EXPECTED_RECORDS_LEAP && !wf_allows_leap_years {
            panic!(
                "EPW invariant violation: 8784 records present but wf_allows_leap_years is false. \
                 Feb 29 data should have been stripped."
            );
        }
        // Also assert: if records.len() == 8784 and wf_allows_leap_years is true,
        // there must be at least one Feb 29 record (the file is a leap year).
        if records.len() == EXPECTED_RECORDS_LEAP && wf_allows_leap_years {
            let has_feb29 = record_datetimes
                .iter()
                .any(|(date, _)| date.month() == 2 && date.day() == 29);
            assert!(
                has_feb29,
                "EPW invariant violation: 8784 records with wf_allows_leap_years=true \
                 but no Feb 29 record found."
            );
        }
    }

    // Apply the parsed flag to the weather meta so downstream consumers
    // (observer, validation tooling) can flag mismatches.
    meta.wf_allows_leap_years = wf_allows_leap_years;

    // Resolve monthly ground temperatures: prefer EPW header data, fall back to DOE-2 model.
    let monthly_ground_temps = match epw_ground_temps {
        Some(gt) => gt,
        None => {
            let dry_bulb: Vec<f64> = records.iter().map(|r| r.dry_bulb_c).collect();
            doe2_ground_temp_monthly(&dry_bulb, is_leap_year)?
        }
    };

    for (record, (date, hour)) in records.iter_mut().zip(&record_datetimes) {
        record.ground_temp_c = interpolate_ground_temp_c(
            &monthly_ground_temps,
            date.month(),
            date.day(),
            *hour,
            is_leap_year,
        )?;
    }

    Ok(records_to_series(meta, design_conditions, &records))
}

fn parse_location_header(line: &str) -> Result<WeatherMeta, WeatherError> {
    let fields: Vec<&str> = line.split(',').collect();
    if fields.len() < LOCATION_MIN_FIELDS {
        return Err(WeatherError::Parse(format!(
            "location header must contain at least {LOCATION_MIN_FIELDS} fields"
        )));
    }
    if fields[0].trim() != "LOCATION" {
        return Err(WeatherError::Parse(
            "first EPW line must start with LOCATION".to_string(),
        ));
    }

    let location = fields[1].trim().to_string();
    let latitude = parse_f64_field(fields[6], "location latitude")?;
    let longitude = parse_f64_field(fields[7], "location longitude")?;
    let timezone_offset_h = parse_f64_field(fields[8], "location timezone")?;
    let elevation_m = parse_f64_field(fields[9], "location elevation")?;

    Ok(WeatherMeta {
        location,
        latitude,
        longitude,
        timezone_offset_h,
        elevation_m,
        wf_allows_leap_years: true,
        source_step_secs: 3600,
        // EPW uses hour-ending convention: row "12" covers 11:00–12:00.
        // Subtract half-period (30 min) from sim time to read the correct period.
        midpoint_offset_secs: 1800,
        has_embedded_location: true,
    })
}

/// Parse the EPW HOLIDAYS/DAYLIGHT SAVINGS header (line 5).
///
/// Extracts field A1 (`Leap Year Observed`) which controls whether Feb 29
/// weather data should be honoured. EnergyPlus stores this in
/// `WFAllowsLeapYears` (WeatherManager.cc:7889) and uses it to gate
/// Feb 29 processing at WeatherManager.cc:2816-2827.
///
/// EPW Data Dictionary values for field A1 are "Yes" or "No".
/// Returns `Ok(true)` for "Yes", `Ok(false)` for "No".
/// Returns an error if the header is malformed or the value is unrecognised.
fn parse_holidays_daylight_header(line: &str) -> Result<bool, WeatherError> {
    let mut fields = line.split(',');
    let header_name = fields
        .next()
        .ok_or_else(|| WeatherError::Parse("empty HOLIDAYS/DAYLIGHT SAVINGS header".to_string()))?;
    if header_name.trim() != "HOLIDAYS/DAYLIGHT SAVINGS" {
        return Err(WeatherError::Parse(format!(
            "expected HOLIDAYS/DAYLIGHT SAVINGS header, got `{header_name}`"
        )));
    }
    let a1 = fields.next().ok_or_else(|| {
        WeatherError::Parse(
            "HOLIDAYS/DAYLIGHT SAVINGS header missing field A1 (Leap Year Observed)".to_string(),
        )
    })?;
    match a1.trim() {
        "Yes" => Ok(true),
        "No" => Ok(false),
        unrecognised => Err(WeatherError::Parse(format!(
            "HOLIDAYS/DAYLIGHT SAVINGS field A1 must be 'Yes' or 'No', got `{unrecognised}`"
        ))),
    }
}

fn ensure_hourly_data_period(line: &str) -> Result<(), WeatherError> {
    let fields: Vec<&str> = line.split(',').collect();
    if fields.len() < DATA_PERIOD_MIN_FIELDS {
        return Err(WeatherError::Parse(
            "invalid DATA PERIODS header; expected at least 3 fields".to_string(),
        ));
    }
    if fields[0].trim() != "DATA PERIODS" {
        return Err(WeatherError::Parse(
            "header line 8 must be DATA PERIODS".to_string(),
        ));
    }

    let records_per_hour = parse_u32_field(fields[2], "records per hour")?;
    if records_per_hour != 1 {
        return Err(WeatherError::Validation(
            "EPW downsampling not supported; source must be hourly".to_string(),
        ));
    }

    Ok(())
}

/// Parses the monthly ground temperatures from the GROUND TEMPERATURES header line.
///
/// Returns `None` when the header is absent, has no valid depth entries, or any
/// monthly value fails to parse — signalling that the caller should use the
/// DOE-2 sinusoidal fallback instead.
///
/// Selects the depth entry closest to 0.5 m per EPW Data Dictionary v9.6 §3
/// (GROUND TEMPERATURES field): 0.5 m is the reference depth for surface
/// boundary conditions used by GroundTemperatures:Surface.
fn parse_ground_temperatures(line: &str) -> Option<[f64; 12]> {
    const TARGET_DEPTH_M: f64 = 0.5;

    let fields: Vec<&str> = line.split(',').collect();
    if fields.is_empty() || fields[0].trim() != "GROUND TEMPERATURES" || fields.len() < 2 {
        return None;
    }

    let num_depths = fields[1].trim().parse::<usize>().ok()?;
    if num_depths == 0 {
        return None;
    }

    let mut best_dist = f64::INFINITY;
    let mut best_monthly: Option<[f64; 12]> = None;

    for depth_index in 0..num_depths {
        let base = 2 + depth_index * 16;
        if fields.len() < base + 16 {
            continue;
        }

        let Some(depth_m) = parse_trimmed_f64(fields[base]) else {
            continue;
        };

        // Select the entry whose depth is closest to 0.5 m.
        let dist = (depth_m - TARGET_DEPTH_M).abs();

        let monthly_slice = &fields[(base + 4)..(base + 16)];
        let mut monthly = [f64::NAN; 12];
        let mut valid = true;
        for (i, value) in monthly_slice.iter().enumerate() {
            match value.trim().parse::<f64>() {
                Ok(v) => monthly[i] = v,
                Err(_) => {
                    valid = false;
                    break;
                }
            }
        }

        if !valid {
            // A malformed monthly value makes this depth entry unusable.
            // Continue searching other depth entries — don't silently substitute.
            continue;
        }

        // All 12 values parsed successfully. Check if this is the best depth match.
        if dist < best_dist {
            best_dist = dist;
            best_monthly = Some(monthly);
        }
    }

    best_monthly
}

/// DOE-2/OCHRE model constants for monthly ground-temperature fallback.
/// Hours in a standard (non-leap) year [h/year]. Alias of [`hares_physics::constants::HOURS_PER_YEAR`].
const DOE2_GROUND_HOURS_PER_YEAR: f64 = HOURS_PER_YEAR;
/// Days in a standard year used in the phase-angle formula [days].
const DOE2_GROUND_DAYS_PER_YEAR: f64 = 365.0;
/// Soil thermal diffusivity [m²/hr] for the DOE-2 GTEMP correlation.
///
/// OEM DOE-2 default for average soil: 0.025 ft²/hr. Converted to SI
/// via NIST exact factor 1 ft² = 0.09290304 m²: 0.025 × 0.09290304 = 0.002_322_576.
/// Cross-validated against OCHRE `vendors/OCHRE/ochre/utils/schedule.py:248`
/// which computes `beta = sqrt(π/(8760×0.025)) × 10` in imperial units (10 ft depth);
/// HARES β matches OCHRE β via `sqrt(π/(8760×0.002_322_576)) × 3.048` ≈ 1.198.
const DOE2_GROUND_DIFFUSIVITY: f64 = 0.002_322_576;
/// Reference depth [m] for the DOE-2 GTEMP ground-temperature correlation.
///
/// DOE-2 GTEMP subroutine uses 10 ft (3.048 m) — verified against OCHRE
/// `vendors/OCHRE/ochre/utils/schedule.py:248` —
/// `beta = (np.pi / (8760 * 0.025)) ** 0.5 * 10`.
const DOE2_GROUND_REFERENCE_DEPTH_M: f64 = 3.048;
/// Phase offset [rad] aligning the ground-temperature sinusoid to peak in late summer.
const DOE2_GROUND_PHASE_OFFSET_RAD: f64 = 0.6;

/// DOE-2/OCHRE mid-month day-of-year values used for monthly ground temperature.
/// Index 0 = January, index 11 = December.
const DOE2_MID_MONTH_DAYS: [f64; 12] = [
    15.0, 46.0, 74.0, 95.0, 135.0, 166.0, 196.0, 227.0, 258.0, 288.0, 319.0, 349.0,
];

pub fn monthly_day_counts(is_leap_year: bool) -> [usize; 12] {
    if is_leap_year {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    }
}

pub(crate) fn monthly_average_dry_bulb(
    dry_bulb_c: &[f64],
    is_leap_year: bool,
) -> Option<[f64; 12]> {
    let day_counts = monthly_day_counts(is_leap_year);
    let expected_hours = day_counts.iter().sum::<usize>() * 24;
    if dry_bulb_c.len() != expected_hours {
        return None;
    }

    let mut monthly = [0.0_f64; 12];
    let mut offset = 0usize;
    for (month_idx, day_count) in day_counts.iter().enumerate() {
        let hours = day_count * 24;
        let month_slice = &dry_bulb_c[offset..offset + hours];
        let sum = month_slice.iter().sum::<f64>();
        monthly[month_idx] = sum / hours as f64;
        offset += hours;
    }

    Some(monthly)
}

/// Compute monthly ground temperatures using the DOE-2 damped correlation:
///
/// `T_ground(day) = T_avg - ΔT_monthly * gm * cos(2π day / 365 - 0.6 - atan(z))`
///
/// Parameters are derived from the hourly dry-bulb temperature series:
/// - `T_avg` = annual mean of monthly average dry-bulb
/// - `ΔT_monthly` = `(max(monthly_avg) - min(monthly_avg)) / 2`
/// - `gm`, `z` from DOE-2 GTEMP damping terms (`beta`, `x`, `y`)
///
/// Returns a 12-element array of mid-month ground temperatures [°C],
/// or an error if the dry-bulb series is empty.
pub(crate) fn doe2_ground_temp_monthly(
    dry_bulb_c: &[f64],
    is_leap_year: bool,
) -> Result<[f64; 12], WeatherError> {
    if dry_bulb_c.is_empty() {
        return Err(WeatherError::Parse(
            "cannot compute DOE-2 ground temperature from empty dry-bulb series".to_string(),
        ));
    }

    let monthly_avg = monthly_average_dry_bulb(dry_bulb_c, is_leap_year).unwrap_or_else(|| {
        let annual_avg = dry_bulb_c.iter().sum::<f64>() / dry_bulb_c.len() as f64;
        [annual_avg; 12]
    });
    Ok(doe2_ground_temp_from_monthly_avg(&monthly_avg))
}

/// Compute DOE-2 ground temperatures from pre-computed monthly averages.
///
/// Core formula shared by EPW and PSM3 parsers. Accepts the 12-element
/// monthly mean dry-bulb array directly, avoiding any assumption about
/// the temporal resolution of the source data.
#[must_use]
pub(crate) fn doe2_ground_temp_from_monthly_avg(monthly_avg: &[f64; 12]) -> [f64; 12] {
    let t_avg = monthly_avg.iter().sum::<f64>() / 12.0;
    let t_min = monthly_avg.iter().copied().fold(f64::INFINITY, f64::min);
    let t_max = monthly_avg
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    let dt_monthly = (t_max - t_min) / 2.0;

    let beta = (std::f64::consts::PI / (DOE2_GROUND_HOURS_PER_YEAR * DOE2_GROUND_DIFFUSIVITY))
        .sqrt()
        * DOE2_GROUND_REFERENCE_DEPTH_M;
    let x = (-beta).exp();
    let cos_beta = beta.cos();
    let sin_beta = beta.sin();
    let y = (x * x - 2.0 * x * cos_beta + 1.0) / (2.0 * beta * beta);
    let gm = y.sqrt();
    let z = (1.0 - x * (cos_beta + sin_beta)) / (1.0 - x * (cos_beta - sin_beta));
    let phase = DOE2_GROUND_PHASE_OFFSET_RAD + z.atan();

    let mut result = [0.0_f64; 12];
    for (i, &day) in DOE2_MID_MONTH_DAYS.iter().enumerate() {
        let argument = 2.0 * std::f64::consts::PI / DOE2_GROUND_DAYS_PER_YEAR * day - phase;
        result[i] = t_avg - dt_monthly * gm * argument.cos();
    }

    // Invariant: the DOE-2 damped ground temperature must have strictly less
    // seasonal amplitude than the outdoor air temperature. If ground amplitude
    // equals or exceeds the air amplitude, the depth is too shallow or soil
    // diffusivity is implausibly low.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        let result_min = result.iter().copied().fold(f64::INFINITY, f64::min);
        let result_max = result.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let ground_pp = result_max - result_min;
        let air_pp = t_max - t_min;
        if ground_pp >= air_pp {
            tracing::error!(
                ground_peak_to_peak_c = ground_pp,
                air_peak_to_peak_c = air_pp,
                beta = beta,
                depth_m = DOE2_GROUND_REFERENCE_DEPTH_M,
                "DOE-2 ground temperature amplitude ({ground_pp_c:.2}°C) is not \
                 strictly less than outdoor air temperature amplitude ({air_pp_c:.2}°C); \
                 the DOE-2 fallback depth ({depth_m} m) may be too shallow or soil \
                 diffusivity is implausibly low",
                ground_pp_c = ground_pp,
                air_pp_c = air_pp,
                depth_m = DOE2_GROUND_REFERENCE_DEPTH_M,
            );
        }
    }

    // Observer capture: record the DOE-2 fallback configuration and resulting
    // seasonal amplitude for diagnostic analysis.
    #[cfg(feature = "observe")]
    {
        let result_min = result.iter().copied().fold(f64::INFINITY, f64::min);
        let result_max = result.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let ground_pp = result_max - result_min;
        tracing::debug!(
            target: "observe",
            doe2_fallback_depth_m = DOE2_GROUND_REFERENCE_DEPTH_M,
            doe2_beta = beta,
            doe2_ground_peak_to_peak_c = ground_pp,
        );
    }

    result
}

/// Minimum infrared threshold [W/m²] below which we fall back to empirical models.
/// Values below 50 W/m² are physically implausible for atmospheric downwelling
/// longwave radiation and indicate missing or placeholder data.
const INFRARED_FALLBACK_THRESHOLD: f64 = 50.0;

/// Sky emissivity model selection.
///
/// EnergyPlus exposes all four models as user-selectable options via the
/// `WeatherProperty:SkyTemperature` input object with default `ClarkAllen`.
/// HARES mirrors this enum so that the clear-sky emissivity formula can be
/// selected independently of the IR-vs-model cascade.
///
/// Cite: EnergyPlus WeatherManager.cc:121–130 (SkyTempModel enum), 3191–3217
/// (CalcSkyEmissivity), 6699–6892 (WeatherProperty:SkyTemperature parsing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SkyTempModel {
    /// Clark & Allen (1978) — EnergyPlus default.
    /// ε_clear = 0.787 + 0.764 × ln(T_dp_K / 273.15)
    #[default]
    ClarkAllen,
    /// Berdahl-Martin (1984) recalibrated coefficients per Li, Jiang & Coimbra (2017).
    /// ε_clear = 0.758 + 0.521 × (T_dp_C / 100) + 0.625 × (T_dp_C / 100)²
    BerdahlMartin,
    /// Brunt (1932) — uses dry-bulb saturation × RH/100 for vapor pressure.
    /// ε_clear = 0.618 + 0.056 × sqrt(P_wv_hPa)
    Brunt,
    /// Idso (1981) — uses dry-bulb saturation × RH/100 for vapor pressure.
    /// ε_clear = 0.685 + 3.2e-5 × P_wv_hPa × exp(1699 / T_db_K)
    Idso,
}

/// Compute sky temperature from horizontal infrared radiation (OCHRE method).
///
/// Model selection cascade:
/// 1. Stefan-Boltzmann inversion when IR >= 50 W/m² (direct measurement).
/// 2. Selected clear-sky emissivity model + Walton cloud correction when
///    opaque sky cover > 0.
/// 3. Selected clear-sky emissivity model (no cloud correction) as fallback.
///
/// Recompute sky temperature from the (possibly interpolated) weather inputs.
///
/// Called both during EPW parsing and after resampling so that T_sky stays
/// consistent with its non-linear inputs.  Directly interpolating T_sky
/// violates the chain rule — EnergyPlus WeatherManager.cc:3113 recomputes
/// after interpolating all input fields, and so must we.
///
/// Cite: EnergyPlus WeatherManager.cc:3130–3217 (sky temperature dispatch +
/// CalcSkyEmissivity model selection).
pub fn compute_sky_temp_c(
    horizontal_infrared_w_m2: f64,
    dry_bulb_c: f64,
    dew_point_c: f64,
    rel_humidity_pct: f64,
    opaque_sky_cover: f64,
    model: SkyTempModel,
) -> f64 {
    if horizontal_infrared_w_m2 >= INFRARED_FALLBACK_THRESHOLD {
        // Stefan-Boltzmann inversion — model-independent (direct measurement).
        let t_sky_k = (horizontal_infrared_w_m2 / STEFAN_BOLTZMANN).powf(0.25);
        t_sky_k - KELVIN_OFFSET_C
    } else {
        let eps_clear = match model {
            SkyTempModel::ClarkAllen => clark_allen_sky_emissivity(dew_point_c),
            SkyTempModel::BerdahlMartin => berdahl_martin_sky_emissivity(dew_point_c),
            SkyTempModel::Brunt => brunt_sky_emissivity(dry_bulb_c, rel_humidity_pct),
            SkyTempModel::Idso => idso_sky_emissivity(dry_bulb_c, rel_humidity_pct),
        };
        if opaque_sky_cover > 0.0 {
            let eps_sky = walton_cloud_correction(eps_clear, opaque_sky_cover);
            sky_temp_from_emissivity(dry_bulb_c, eps_sky)
        } else {
            sky_temp_from_emissivity(dry_bulb_c, eps_clear)
        }
    }
}

/// Clark & Allen (1978) clear-sky emissivity from dew point temperature.
///
/// ε_clear = 0.787 + 0.764 × ln(T_dp_K / 273.15)
///
/// EnergyPlus applies `min(DryBulb, DewPoint)` in the numerator; HARES
/// uses dew point directly since the EPW parser already validates
/// dew_point_c <= dry_bulb_c.
///
/// Cite: Clark, G. and Allen, C. (1978), "The Estimation of Atmospheric
/// Radiation for Clear and Cloudy Skies", Proc. 2nd National Passive Solar
/// Conference (AS/ISES), pp. 675-678.
/// Cite: EnergyPlus WeatherManager.cc:3214 (CalcSkyEmissivity, ClarkAllen case).
#[must_use]
pub fn clark_allen_sky_emissivity(dew_point_c: f64) -> f64 {
    let dew_point_k = dew_point_c + KELVIN_OFFSET_C;
    0.787 + 0.764 * (dew_point_k / KELVIN_OFFSET_C).ln()
}

/// Clark & Allen (1978) sky temperature from dry bulb and dew point.
///
/// Convenience wrapper: computes ε_clear via [`clark_allen_sky_emissivity`]
/// then converts to sky temperature via [`sky_temp_from_emissivity`].
///
/// T_sky = T_db_K × ε_clear^0.25 − 273.15
///
/// Cite: Clark, G. and Allen, C. (1978), "The Estimation of Atmospheric
/// Radiation for Clear and Cloudy Skies", Proc. 2nd National Passive Solar
/// Conference (AS/ISES), pp. 675-678.
// Why: used by test code in epw.rs, tmy3.rs, psm3.rs, weather.rs for
// direct Clark-Allen comparison against the compute_sky_temp_c cascade.
#[allow(dead_code)]
#[must_use]
pub(crate) fn clark_allen_sky_temp_c(dry_bulb_c: f64, dew_point_c: f64) -> f64 {
    let emissivity = clark_allen_sky_emissivity(dew_point_c);
    sky_temp_from_emissivity(dry_bulb_c, emissivity)
}

/// Berdahl-Martin clear-sky emissivity from dew point temperature.
///
/// ε_clear = 0.758 + 0.521 × (T_dp_C / 100) + 0.625 × (T_dp_C / 100)²
///
/// The quadratic functional form was introduced by Martin & Berdahl (1984),
/// "Characteristics of Infrared Sky Radiation in the United States," Solar Energy
/// 33(3/4):321-336. The original companion paper Berdahl & Martin (1984) Solar
/// Energy 32(5):663-664 used a linear approximation with coefficients
/// 0.711 / 0.56 / 0.73.
///
/// The coefficient values 0.758 / 0.521 / 0.625 are the recalibrated set from
/// Li, M., Jiang, Y. & Coimbra, C.F.M. (2017), "On the determination of
/// atmospheric longwave irradiance under all-sky conditions," Solar Energy
/// 144:40-48. This recalibrated form is used by EnergyPlus under the
/// "Martin & Berdahl" model label (EnergyPlus Engineering Reference,
/// Sky Radiation Modeling section).
#[must_use]
pub fn berdahl_martin_sky_emissivity(t_dp_c: f64) -> f64 {
    let x = t_dp_c / 100.0;
    0.758 + 0.521 * x + 0.625 * x * x
}

/// Brunt (1932) clear-sky emissivity from dry-bulb temperature and
/// relative humidity.
///
/// ε_clear = 0.618 + 0.056 × sqrt(P_wv_hPa)
///
/// Water vapor partial pressure is computed via the EnergyPlus method:
/// saturation pressure at dry-bulb temperature × (RH / 100), i.e.
/// actual vapor pressure using T_db as the saturation reference.
///
/// Cite: Brunt, D. (1932), "Notes on radiation in the atmosphere",
/// Q.J.R. Meteorol. Soc., 58, 389-420.
/// Cite: EnergyPlus WeatherManager.cc:3204–3206 (CalcSkyEmissivity, Brunt case).
#[must_use]
pub(crate) fn brunt_sky_emissivity(t_db_c: f64, rel_humidity_pct: f64) -> f64 {
    let p_wv_hpa = magnus_saturation_pressure_hpa(t_db_c) * (rel_humidity_pct / 100.0);
    0.618 + 0.056 * p_wv_hpa.sqrt()
}

/// Idso (1981) clear-sky emissivity from dry-bulb temperature and
/// relative humidity.
///
/// ε_clear = 0.685 + 3.2e-5 × P_wv_hPa × exp(1699 / T_db_K)
///
/// Water vapor partial pressure is computed via the EnergyPlus method:
/// saturation pressure at dry-bulb temperature × (RH / 100).
/// The coefficient 3.2e-5 is calibrated for water vapour pressure in hPa
/// (matching EnergyPlus). Do not convert to Pa before applying.
///
/// Cite: Idso, S.B. (1981), "A set of equations for full spectrum and 8- to
/// 14-μm and 10.5- to 12.5-μm thermal radiation from cloudless skies",
/// Water Resources Research, 17(2), 295-304.
/// Cite: EnergyPlus WeatherManager.cc:3207–3209 (CalcSkyEmissivity, Idso case).
#[must_use]
pub(crate) fn idso_sky_emissivity(t_db_c: f64, rel_humidity_pct: f64) -> f64 {
    let p_wv_hpa = magnus_saturation_pressure_hpa(t_db_c) * (rel_humidity_pct / 100.0);
    let t_db_k = t_db_c + KELVIN_OFFSET_C;
    0.685 + 3.2e-5 * p_wv_hpa * (1699.0 / t_db_k).exp()
}

/// Walton (1983) cloud cover correction applied to clear-sky emissivity.
///
/// ε_sky = ε_clear × (1 + 0.0224×N - 0.0035×N² + 0.00028×N³)
///
/// Where N = opaque sky cover in tenths [0, 10].
///
/// Cite: Walton, G.N. (1983), "Thermal Analysis Research Program Reference
/// Manual", NBSIR 83-2655.
#[must_use]
pub(crate) fn walton_cloud_correction(epsilon_clear: f64, opaque_sky_cover: f64) -> f64 {
    let n = opaque_sky_cover.clamp(0.0, 10.0);
    (epsilon_clear * (1.0 + 0.0224 * n - 0.0035 * n * n + 0.00028 * n * n * n)).clamp(0.0, 1.0)
}

/// Convert sky emissivity and dry bulb temperature to sky temperature.
///
/// T_sky = T_db_K × ε_sky^0.25 - 273.15
#[must_use]
pub fn sky_temp_from_emissivity(t_db_c: f64, epsilon: f64) -> f64 {
    let t_db_k = t_db_c + KELVIN_OFFSET_C;
    t_db_k * epsilon.powf(0.25) - KELVIN_OFFSET_C
}

/// Magnus formula saturation pressure at temperature `t_c` [deg C].
/// Returns pressure in hPa (hectopascals / millibars).
#[must_use]
fn magnus_saturation_pressure_hpa(t_c: f64) -> f64 {
    6.1078 * (17.27 * t_c / (t_c + 237.3)).exp()
}

pub(crate) fn interpolate_ground_temp_c(
    monthly_ground_temps: &[f64; 12],
    month: u32,
    day: u32,
    hour: u32,
    is_leap_year: bool,
) -> Result<f64, WeatherError> {
    let year_days = if is_leap_year { 366.0 } else { 365.0 };
    let timestamp_day = f64::from(day_of_year(month, day, is_leap_year)?);
    // EPW uses hour-ending convention: hour 12 covers 11:00–12:00.
    // Place the PCHIP knot at the midpoint of the period (11:30 for hour 12)
    // to match OCHRE/pvlib's +30min offset convention.
    let hour_fraction = (f64::from(hour) - 0.5) / 24.0;
    let x = timestamp_day + hour_fraction;

    let mut anchors = [0.0_f64; 12];
    for m in 1..=12 {
        anchors[(m - 1) as usize] = f64::from(day_of_year(m, 15, is_leap_year)?);
    }

    if x < anchors[0] {
        return Ok(linear_interp(
            anchors[11] - year_days,
            anchors[0],
            monthly_ground_temps[11],
            monthly_ground_temps[0],
            x,
        ));
    }

    if x >= anchors[11] {
        return Ok(linear_interp(
            anchors[11],
            anchors[0] + year_days,
            monthly_ground_temps[11],
            monthly_ground_temps[0],
            x,
        ));
    }

    for i in 0..11 {
        if x >= anchors[i] && x < anchors[i + 1] {
            return Ok(linear_interp(
                anchors[i],
                anchors[i + 1],
                monthly_ground_temps[i],
                monthly_ground_temps[i + 1],
                x,
            ));
        }
    }

    Err(WeatherError::Validation(
        "failed to interpolate ground temperature".to_string(),
    ))
}

fn linear_interp(x0: f64, x1: f64, y0: f64, y1: f64, x: f64) -> f64 {
    if (x1 - x0).abs() < f64::EPSILON {
        return y0;
    }
    y0 + (y1 - y0) * (x - x0) / (x1 - x0)
}

pub(crate) fn day_of_year(month: u32, day: u32, is_leap_year: bool) -> Result<u32, WeatherError> {
    let reference_year = if is_leap_year { 2020 } else { 2021 };
    let date = NaiveDate::from_ymd_opt(reference_year, month, day).ok_or_else(|| {
        WeatherError::Validation(format!(
            "invalid calendar date for interpolation: month={month}, day={day}"
        ))
    })?;
    Ok(date.ordinal())
}

fn parse_i32(raw: &str, row: usize, name: &str) -> Result<i32, WeatherError> {
    raw.trim().parse::<i32>().map_err(|_| {
        WeatherError::Parse(format!(
            "row {row}: failed to parse `{name}` as integer: `{}`",
            raw.trim()
        ))
    })
}

fn parse_u32(raw: &str, row: usize, name: &str) -> Result<u32, WeatherError> {
    raw.trim().parse::<u32>().map_err(|_| {
        WeatherError::Parse(format!(
            "row {row}: failed to parse `{name}` as unsigned integer: `{}`",
            raw.trim()
        ))
    })
}

fn parse_f64(raw: &str, row: usize, name: &str) -> Result<f64, WeatherError> {
    raw.trim().parse::<f64>().map_err(|_| {
        WeatherError::Parse(format!(
            "row {row}: failed to parse `{name}` as number: `{}`",
            raw.trim()
        ))
    })
}

fn parse_f64_field(raw: &str, name: &str) -> Result<f64, WeatherError> {
    raw.trim().parse::<f64>().map_err(|_| {
        WeatherError::Parse(format!(
            "failed to parse `{name}` as number: `{}`",
            raw.trim()
        ))
    })
}

fn parse_u32_field(raw: &str, name: &str) -> Result<u32, WeatherError> {
    raw.trim().parse::<u32>().map_err(|_| {
        WeatherError::Parse(format!(
            "failed to parse `{name}` as integer: `{}`",
            raw.trim()
        ))
    })
}

fn records_to_series(
    meta: WeatherMeta,
    design_conditions: Option<DesignConditions>,
    records: &[EpwRecord],
) -> WeatherTimeSeries {
    let len = records.len();
    let mut dry_bulb_c = Vec::with_capacity(len);
    let mut dew_point_c = Vec::with_capacity(len);
    let mut rel_humidity_pct = Vec::with_capacity(len);
    let mut pressure_kpa = Vec::with_capacity(len);
    let mut ghi_w_m2 = Vec::with_capacity(len);
    let mut dni_w_m2 = Vec::with_capacity(len);
    let mut dhi_w_m2 = Vec::with_capacity(len);
    let mut wind_speed_m_s = Vec::with_capacity(len);
    let mut wind_dir_deg = Vec::with_capacity(len);
    let mut opaque_sky_cover = Vec::with_capacity(len);
    let mut horizontal_infrared_w_m2 = Vec::with_capacity(len);
    let mut sky_temp_c = Vec::with_capacity(len);
    let mut ground_temp_c = Vec::with_capacity(len);
    let mut liquid_precip_m = Vec::with_capacity(len);

    for record in records {
        dry_bulb_c.push(record.dry_bulb_c);
        dew_point_c.push(record.dew_point_c);
        rel_humidity_pct.push(record.rel_humidity_pct);
        pressure_kpa.push(record.pressure_kpa);
        ghi_w_m2.push(record.ghi_w_m2);
        dni_w_m2.push(record.dni_w_m2);
        dhi_w_m2.push(record.dhi_w_m2);
        wind_speed_m_s.push(record.wind_speed_m_s);
        wind_dir_deg.push(record.wind_dir_deg);
        opaque_sky_cover.push(record.opaque_sky_cover);
        horizontal_infrared_w_m2.push(record.horizontal_infrared_w_m2);
        sky_temp_c.push(record.sky_temp_c);
        ground_temp_c.push(record.ground_temp_c);
        liquid_precip_m.push(record.liquid_precip_m);
    }

    WeatherTimeSeries {
        meta,
        design_conditions,
        dry_bulb_c,
        dew_point_c,
        rel_humidity_pct,
        pressure_kpa,
        ghi_w_m2,
        dni_w_m2,
        dhi_w_m2,
        wind_speed_m_s,
        wind_dir_deg,
        opaque_sky_cover,
        horizontal_infrared_w_m2,
        sky_temp_c,
        ground_temp_c,
        liquid_precip_m,
        surface_albedo: None,
    }
}

/// Parse the EPW design-conditions header (line 2) to extract extreme
/// heating and cooling dry-bulb temperatures.
///
/// The EPW design-conditions line format varies by data source. This parser
/// handles the common pattern where the line ends with an "Extremes" section
/// containing pairs of (extreme low, extreme high) temperatures. When the
/// "Extremes" section is present, `heating_design_db_c` is set to the minimum
/// of all extreme low dry-bulb temperatures and `cooling_design_db_c` is set
/// to the maximum of all extreme high dry-bulb temperatures.
///
/// Returns `None` when:
/// - The line does not contain an "Extremes" token.
/// - No parseable numeric values follow the "Extremes" token.
/// - The header line has zero design conditions (common for synthetic/test EPWs).
///
/// # EPW Data Dictionary v9.6
///
/// The design-conditions line is an informational header. The authoritative
/// design-day data lives in separate DDY (Design Day) files. The "Extremes"
/// summary here provides a fallback design temperature when ASHRAE 152
/// climate station data is unavailable.
fn parse_design_conditions(line: &str) -> Option<DesignConditions> {
    let fields: Vec<&str> = line.split(',').collect();
    if fields.is_empty() {
        return None;
    }

    // Find the "Extremes" token in the comma-separated fields.
    let extremes_idx = fields
        .iter()
        .position(|f| f.trim().eq_ignore_ascii_case("Extremes"))?;

    // Parse numeric values after the "Extremes" token.
    // Format: Extremes,{n},{low1},{high1},{low2},{high2},...
    // We collect all extreme low and high values.
    let mut heating_candidates: Vec<f64> = Vec::new();
    let mut cooling_candidates: Vec<f64> = Vec::new();
    let mut is_low = true; // Toggle: low, high, low, high, ...

    for raw in fields.iter().skip(extremes_idx + 1) {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(val) = trimmed.parse::<f64>() {
            if val.is_finite() {
                if is_low {
                    heating_candidates.push(val);
                } else {
                    cooling_candidates.push(val);
                }
            }
            is_low = !is_low;
        }
    }

    let heating_design_db_c = heating_candidates
        .iter()
        .copied()
        .fold(f64::INFINITY, f64::min);
    let cooling_design_db_c = cooling_candidates
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);

    if !heating_design_db_c.is_finite() || !cooling_design_db_c.is_finite() {
        return None;
    }

    Some(DesignConditions {
        heating_design_db_c,
        cooling_design_db_c,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use tracing_subscriber;

    use super::{
        DOE2_GROUND_DAYS_PER_YEAR, DOE2_GROUND_DIFFUSIVITY, DOE2_GROUND_HOURS_PER_YEAR,
        DOE2_GROUND_PHASE_OFFSET_RAD, DOE2_GROUND_REFERENCE_DEPTH_M, DOE2_MID_MONTH_DAYS,
        STEFAN_BOLTZMANN, SkyTempModel, WeatherError, berdahl_martin_sky_emissivity,
        brunt_sky_emissivity, clark_allen_sky_temp_c, compute_sky_temp_c, doe2_ground_temp_monthly,
        idso_sky_emissivity, monthly_day_counts, parse_epw, parse_epw_str,
        sky_temp_from_emissivity, walton_cloud_correction,
    };

    fn write_temp_epw(epw_contents: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before UNIX_EPOCH")
            .as_nanos();
        path.push(format!("hares-io-epw-test-{nanos}.epw"));
        fs::write(&path, epw_contents).expect("failed to write temporary EPW");
        path
    }

    fn build_synthetic_epw(
        rows: usize,
        mut mutator: impl FnMut(usize, &mut [String; 35]),
    ) -> String {
        use chrono::{Datelike, Duration, NaiveDate, Timelike};

        let mut lines = vec![
            "LOCATION,Test Site,CO,USA,TMY3,999999,39.74,-104.99,-7.0,1609.3".to_string(),
            "DESIGN CONDITIONS,0".to_string(),
            "GROUND TEMPERATURES,0".to_string(),
            "TYPICAL/EXTREME PERIODS,0".to_string(),
            "HOLIDAYS/DAYLIGHT SAVINGS,Yes,0,0,0".to_string(),
            "COMMENTS 1,synthetic".to_string(),
            "COMMENTS 2,synthetic".to_string(),
            "DATA PERIODS,1,1,Data,Sunday, 1/ 1,12/31".to_string(),
        ];

        let start_date = if rows == 8784 {
            NaiveDate::from_ymd_opt(2020, 1, 1).expect("valid leap-year start")
        } else {
            NaiveDate::from_ymd_opt(2021, 1, 1).expect("valid non-leap-year start")
        };
        let start_time = start_date
            .and_hms_opt(0, 0, 0)
            .expect("valid midnight timestamp");

        for i in 0..rows {
            let timestamp = start_time + Duration::hours(i as i64);
            let year = timestamp.year();
            let month = timestamp.month();
            let day = timestamp.day();
            let hour = timestamp.hour() + 1;

            let mut fields = [
                year.to_string(),
                month.to_string(),
                day.to_string(),
                hour.to_string(),
                "0".to_string(),
                "A0A0A0A0*0*0*0*0*0*0*0*0*0*0".to_string(),
                "20.0".to_string(),
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

            mutator(i, &mut fields);
            lines.push(fields.join(","));
        }

        lines.join("\n")
    }

    #[test]
    fn parses_real_tmy3_fixture() {
        let mut fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        fixture.push("../../data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw");

        let weather = parse_epw(&fixture).expect("fixture should parse");
        assert_eq!(weather.len(), 8760);
        assert_eq!(weather.meta.elevation_m, 1650.0);
        assert_eq!(weather.meta.location, "Denver Intl Ap");
        assert!(
            weather
                .dry_bulb_c
                .iter()
                .all(|t| (-60.0..=55.0).contains(t))
        );
        assert_eq!(weather.wind_dir_deg.len(), weather.len());
        assert!((weather.pressure_kpa[0] - 83.7).abs() < 1e-6);
    }

    #[test]
    fn supports_amy_leap_record_count() {
        let epw = build_synthetic_epw(8784, |_row, _fields| {});
        let parsed = parse_epw_str(&epw).expect("8784 records should parse");
        assert_eq!(parsed.len(), 8784);
    }

    #[test]
    fn sky_temperature_infrared_method() {
        // OCHRE method: T_sky_K = (IR / sigma)^0.25
        let ir = 300.0; // W/m²
        let expected_k = (ir / STEFAN_BOLTZMANN).powf(0.25);
        let expected_c = expected_k - 273.15;
        let t_sky_c = compute_sky_temp_c(ir, 20.0, 10.0, 50.0, 5.0, SkyTempModel::default());
        assert!(
            (t_sky_c - expected_c).abs() < 0.01,
            "infrared sky temp: got {t_sky_c}, expected {expected_c}"
        );
    }

    #[test]
    fn sky_temperature_falls_back_to_clark_allen_when_no_clouds() {
        // When infrared is below threshold and opaque_sky_cover=0, use Clark-Allen
        let t_sky_c = compute_sky_temp_c(0.0, 20.0, 10.0, 50.0, 0.0, SkyTempModel::default());
        let t_sky_clark = clark_allen_sky_temp_c(20.0, 10.0);
        assert!(
            (t_sky_c - t_sky_clark).abs() < 0.01,
            "fallback should match Clark-Allen: got {t_sky_c}, expected {t_sky_clark}"
        );
    }

    #[test]
    fn clark_allen_matches_reference_case() {
        let t_sky_c = clark_allen_sky_temp_c(20.0, 10.0);
        let t_sky_k = t_sky_c + 273.15;
        assert!((t_sky_k - 278.5).abs() <= 1.0);
    }

    #[test]
    fn rejects_non_hourly_data_period() {
        let epw = build_synthetic_epw(8760, |_row, _fields| {});
        let epw = epw.replacen("DATA PERIODS,1,1", "DATA PERIODS,1,2", 1);
        let err = parse_epw_str(&epw).expect_err("records-per-hour != 1 should fail");
        assert!(
            err.to_string()
                .contains("EPW downsampling not supported; source must be hourly")
        );
    }

    #[test]
    fn rejects_wind_speed_above_limit() {
        let epw = build_synthetic_epw(8760, |row, fields| {
            if row == 10 {
                fields[21] = "70.0".to_string();
            }
        });
        let err = parse_epw_str(&epw).expect_err("wind speed > 60 must fail");
        assert!(matches!(err, WeatherError::Validation(_)));
        assert!(err.to_string().contains("row 11"));
    }

    #[test]
    fn rejects_ghi_above_limit() {
        let epw = build_synthetic_epw(8760, |row, fields| {
            if row == 200 {
                fields[13] = "1600.0".to_string();
            }
        });
        let err = parse_epw_str(&epw).expect_err("GHI > 1500 must fail");
        assert!(matches!(err, WeatherError::Validation(_)));
        assert!(err.to_string().contains("1600"));
    }

    #[test]
    fn rejects_pressure_out_of_range() {
        let epw = build_synthetic_epw(8760, |row, fields| {
            if row == 300 {
                fields[9] = "55000".to_string();
            }
        });
        let err = parse_epw_str(&epw).expect_err("pressure below range must fail");
        assert!(matches!(err, WeatherError::Validation(_)));
        assert!(err.to_string().contains("55"));
    }

    #[test]
    fn rejects_dew_point_above_dry_bulb() {
        let epw = build_synthetic_epw(8760, |row, fields| {
            if row == 99 {
                fields[6] = "10.0".to_string();
                fields[7] = "11.0".to_string();
            }
        });
        let err = parse_epw_str(&epw).expect_err("dew point > dry bulb must fail");
        assert!(matches!(err, WeatherError::Validation(_)));
        assert!(err.to_string().contains("row 100"));
    }

    #[test]
    fn parse_epw_reads_from_path() {
        let epw = build_synthetic_epw(8760, |_row, _fields| {});
        let path = write_temp_epw(&epw);
        let result = parse_epw(&path);
        let _ = fs::remove_file(path);
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // Sky temperature via infrared
    // -----------------------------------------------------------------------

    #[test]
    fn sky_temp_from_infrared_300() {
        // T_sky_K = (300 / σ)^0.25 where σ = STEFAN_BOLTZMANN (CODATA 2018)
        let ir = 300.0;
        let expected_k = (ir / STEFAN_BOLTZMANN).powf(0.25);
        let expected_c = expected_k - 273.15;
        let t_sky_c = compute_sky_temp_c(ir, 25.0, 15.0, 50.0, 5.0, SkyTempModel::default());
        assert!(
            (t_sky_c - expected_c).abs() < 0.01,
            "T_sky from IR=300: got {t_sky_c:.4}, expected {expected_c:.4}"
        );
    }

    #[test]
    fn clark_allen_fallback_activates_when_infrared_zero() {
        // opaque_sky_cover=0 forces Clark-Allen path
        let t_sky = compute_sky_temp_c(0.0, 15.0, 5.0, 50.0, 0.0, SkyTempModel::default());
        let t_clark = clark_allen_sky_temp_c(15.0, 5.0);
        assert!(
            (t_sky - t_clark).abs() < 0.001,
            "IR=0 should trigger Clark-Allen fallback: got {t_sky}, expected {t_clark}"
        );
    }

    #[test]
    fn clark_allen_fallback_activates_when_infrared_below_threshold() {
        // Values below 50 W/m² with no cloud data → Clark-Allen
        let t_sky = compute_sky_temp_c(30.0, 20.0, 10.0, 50.0, 0.0, SkyTempModel::default());
        let t_clark = clark_allen_sky_temp_c(20.0, 10.0);
        assert!(
            (t_sky - t_clark).abs() < 0.001,
            "IR=30 should trigger Clark-Allen fallback: got {t_sky}, expected {t_clark}"
        );
    }

    #[test]
    fn rejects_infrared_above_700() {
        let epw = build_synthetic_epw(8760, |row, fields| {
            if row == 50 {
                fields[12] = "750.0".to_string();
            }
        });
        let err = parse_epw_str(&epw).expect_err("infrared > 700 must fail validation");
        assert!(matches!(err, WeatherError::Validation(_)));
        assert!(
            err.to_string().contains("750"),
            "error should mention the offending value: {err}"
        );
    }

    #[test]
    fn rejects_negative_infrared() {
        let epw = build_synthetic_epw(8760, |row, fields| {
            if row == 50 {
                fields[12] = "-10.0".to_string();
            }
        });
        let err = parse_epw_str(&epw).expect_err("negative infrared must fail validation");
        assert!(matches!(err, WeatherError::Validation(_)));
        assert!(
            err.to_string().contains("-10"),
            "error should mention the offending value: {err}"
        );
    }

    // -----------------------------------------------------------------------
    // horizontal_infrared EPW field extraction
    // -----------------------------------------------------------------------

    #[test]
    fn epw_parsing_extracts_infrared_at_correct_column() {
        // Column index 12 in EPW data rows is horizontal infrared
        let epw = build_synthetic_epw(8760, |row, fields| {
            if row == 0 {
                fields[12] = "350.0".to_string();
            }
        });
        let parsed = parse_epw_str(&epw).expect("synthetic EPW should parse");
        assert!(
            (parsed.horizontal_infrared_w_m2[0] - 350.0).abs() < 0.01,
            "first row infrared should be 350.0, got {}",
            parsed.horizontal_infrared_w_m2[0]
        );
    }

    /// EPW headers embed site location, so `has_embedded_location` must be
    /// `true` — this tells the site-location resolver to honour the file's
    /// lat/lon/timezone rather than treating `0.0` as "unknown".
    #[test]
    fn epw_has_embedded_location_flag_set() {
        let epw = build_synthetic_epw(8760, |_row, _fields| {});
        let parsed = parse_epw_str(&epw).expect("synthetic EPW should parse");
        assert!(
            parsed.meta.has_embedded_location,
            "EPW format embeds location in its header; has_embedded_location must be true"
        );
    }

    #[test]
    fn weather_time_series_get_horizontal_infrared() {
        use crate::weather::WeatherField;
        let epw = build_synthetic_epw(8760, |row, fields| {
            if row == 5 {
                fields[12] = "280.0".to_string();
            }
        });
        let parsed = parse_epw_str(&epw).expect("synthetic EPW should parse");
        let val = parsed.get(WeatherField::HorizontalInfrared, 5);
        assert!(
            (val - 280.0).abs() < 0.01,
            "get(HorizontalInfrared, 5) should be 280.0, got {val}"
        );
    }

    #[test]
    fn epw_infrared_stored_for_all_rows() {
        // Default synthetic EPW sets infrared to 300 for all rows
        let epw = build_synthetic_epw(8760, |_row, _fields| {});
        let parsed = parse_epw_str(&epw).expect("synthetic EPW should parse");
        assert_eq!(parsed.horizontal_infrared_w_m2.len(), 8760);
        assert!(
            parsed
                .horizontal_infrared_w_m2
                .iter()
                .all(|&v| (v - 300.0).abs() < 0.01),
            "all rows should have infrared=300"
        );
    }

    fn build_hourly_from_monthly(
        monthly_values: &[f64; 12],
        is_leap_year: bool,
        diurnal_amp: f64,
    ) -> Vec<f64> {
        let mut hourly = Vec::new();
        let day_counts = monthly_day_counts(is_leap_year);
        for (month_idx, day_count) in day_counts.into_iter().enumerate() {
            for _ in 0..day_count {
                for hour in 0..24 {
                    let diurnal =
                        diurnal_amp * (2.0 * std::f64::consts::PI * hour as f64 / 24.0).sin();
                    hourly.push(monthly_values[month_idx] + diurnal);
                }
            }
        }
        hourly
    }

    #[test]
    fn doe2_ground_temp_uses_monthly_mean_amplitude_not_hourly_extremes() {
        // Large diurnal swing would dominate an hourly-extrema model. OCHRE/DOE-2 should
        // use monthly means, so output amplitude remains tied to monthly trend.
        let monthly_means = [
            -6.0, -4.0, 0.0, 5.0, 10.0, 15.0, 18.0, 17.0, 12.0, 6.0, 0.0, -4.0,
        ];
        let dry_bulb = build_hourly_from_monthly(&monthly_means, false, 12.0);
        let monthly_ground = doe2_ground_temp_monthly(&dry_bulb, false)
            .expect("DOE-2 ground temp should succeed with valid dry-bulb data");

        let min_ground = monthly_ground.iter().copied().fold(f64::INFINITY, f64::min);
        let max_ground = monthly_ground
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        let ground_amp = (max_ground - min_ground) / 2.0;
        let hourly_min = dry_bulb.iter().copied().fold(f64::INFINITY, f64::min);
        let hourly_max = dry_bulb.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let hourly_extrema_amp = (hourly_max - hourly_min) / 2.0;

        // Ground amplitude should be much smaller than an hourly-extrema amplitude model.
        assert!(
            ground_amp < hourly_extrema_amp * 0.5,
            "ground amplitude should be damped vs hourly extrema: ground_amp={ground_amp}, hourly_extrema_amp={hourly_extrema_amp}"
        );
    }

    #[test]
    fn doe2_ground_temp_formula_consistency() {
        let monthly_means = [
            -5.0, -3.0, 2.0, 7.0, 12.0, 16.0, 20.0, 19.0, 14.0, 8.0, 2.0, -2.0,
        ];
        let dry_bulb = build_hourly_from_monthly(&monthly_means, false, 0.0);
        let got = doe2_ground_temp_monthly(&dry_bulb, false)
            .expect("DOE-2 ground temp should succeed with valid dry-bulb data");

        let t_avg = monthly_means.iter().sum::<f64>() / 12.0;
        let dt_monthly = (monthly_means
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max)
            - monthly_means.iter().copied().fold(f64::INFINITY, f64::min))
            / 2.0;

        let beta = (std::f64::consts::PI / (DOE2_GROUND_HOURS_PER_YEAR * DOE2_GROUND_DIFFUSIVITY))
            .sqrt()
            * DOE2_GROUND_REFERENCE_DEPTH_M;
        let x = (-beta).exp();
        let y = (x * x - 2.0 * x * beta.cos() + 1.0) / (2.0 * beta * beta);
        let gm = y.sqrt();
        let z = (1.0 - x * (beta.cos() + beta.sin())) / (1.0 - x * (beta.cos() - beta.sin()));
        let phase = DOE2_GROUND_PHASE_OFFSET_RAD + z.atan();

        for (idx, day) in DOE2_MID_MONTH_DAYS.iter().copied().enumerate() {
            let expected = t_avg
                - dt_monthly
                    * gm
                    * (2.0 * std::f64::consts::PI / DOE2_GROUND_DAYS_PER_YEAR * day - phase).cos();
            assert!(
                (got[idx] - expected).abs() < 1e-10,
                "month {} mismatch: got {}, expected {}",
                idx + 1,
                got[idx],
                expected
            );
        }
    }

    // -----------------------------------------------------------------------
    // EPW ground-temp constant default
    // -----------------------------------------------------------------------

    /// Build a synthetic GROUND TEMPERATURES header line with the given depths
    /// and their monthly averages. Each element of `entries` is (depth_m, [12 temps]).
    fn ground_temp_header(entries: &[(f64, [f64; 12])]) -> String {
        let mut parts = vec!["GROUND TEMPERATURES".to_string(), entries.len().to_string()];
        for (depth_m, monthly) in entries {
            parts.push(depth_m.to_string());
            parts.push(String::new()); // soil conductivity (blank)
            parts.push(String::new()); // soil density (blank)
            parts.push(String::new()); // soil specific heat (blank)
            for v in monthly {
                parts.push(format!("{v}"));
            }
        }
        parts.join(",")
    }

    /// Replace the "GROUND TEMPERATURES,0" placeholder in a synthetic EPW
    /// with a custom GROUND TEMPERATURES header line.
    fn epw_with_ground_header(rows: usize, gt_line: &str) -> String {
        let base = build_synthetic_epw(rows, |_, _| {});
        // Replace "GROUND TEMPERATURES,0" (the build_synthetic_epw default)
        base.replacen("GROUND TEMPERATURES,0", gt_line, 1)
    }

    // --- Bug 1: depth selection (shallowest instead of closest to 0.5 m) ---

    /// An EPW with depths [0.5, 2.0, 4.0] must select the 0.5 m entry.
    ///
    /// The Denver TMY3 EPW has these exact three depths. The closest-to-0.5-m
    /// selection picks 0.5 m (distance 0.0) over 2.0 m (distance 1.5) and
    /// 4.0 m (distance 3.5).
    #[test]
    fn ground_temp_depth_selection_picks_0_5_m_from_0_5_2_4_depths() {
        // Depths 0.5, 2.0, 4.0 m — all-ones at 0.5 m, all-twos at 2 m, all-threes at 4 m.
        let monthly_half: [f64; 12] = [1.0; 12];
        let monthly_two: [f64; 12] = [2.0; 12];
        let monthly_four: [f64; 12] = [3.0; 12];
        let gt_line =
            ground_temp_header(&[(0.5, monthly_half), (2.0, monthly_two), (4.0, monthly_four)]);
        let epw = epw_with_ground_header(8760, &gt_line);
        let parsed = parse_epw_str(&epw).expect("EPW should parse");

        // All ground temperatures must be derived from the 0.5 m entry (value 1.0),
        // not the 2.0 m entry (value 2.0) or the 4.0 m entry (value 3.0).
        for (i, &gt) in parsed.ground_temp_c.iter().enumerate() {
            assert!(
                (gt - 1.0).abs() < 0.5,
                "hour {i}: ground_temp {gt:.4} should be near 1.0 (0.5 m entry), not 2.0 or 3.0"
            );
        }
    }

    /// An EPW with depths [0.1, 0.5, 2.0] MUST select 0.5 m (closest to 0.5),
    /// NOT 0.1 m (the shallowest). 0.5 m is the EPW reference depth for
    /// surface boundary conditions.
    #[test]
    fn ground_temp_depth_selection_picks_0_5_m_not_0_1_m() {
        // 0.1 m entry: all-tens (diurnal layer, wrong for envelope BCs)
        // 0.5 m entry: all-fives (the correct reference depth per EPW spec)
        // 2.0 m entry: all-twos
        let monthly_0_1: [f64; 12] = [10.0; 12];
        let monthly_0_5: [f64; 12] = [5.0; 12];
        let monthly_2_0: [f64; 12] = [2.0; 12];
        let gt_line =
            ground_temp_header(&[(0.1, monthly_0_1), (0.5, monthly_0_5), (2.0, monthly_2_0)]);
        let epw = epw_with_ground_header(8760, &gt_line);
        let parsed = parse_epw_str(&epw).expect("EPW should parse");

        // The fixed code selects the entry closest to 0.5 m → picks the 0.5 m entry (≈5.0),
        // not the shallowest 0.1 m entry (≈10.0).
        for (i, &gt) in parsed.ground_temp_c.iter().enumerate() {
            assert!(
                (gt - 5.0).abs() < 1.0,
                "hour {i}: ground_temp {gt:.4} should come from the 0.5 m entry (≈5.0), \
                 not the 0.1 m (shallowest) entry (≈10.0)"
            );
        }
    }

    // --- Bug 3: DOE2_GROUND_REFERENCE_DEPTH_M now 3.048 m (10 ft OEM depth) ---

    /// Verifies that with the DOE-2 OEM reference depth (3.048 m = 10 ft)
    /// and diffusivity in SI (0.002_322_576 m²/hr = 0.025 ft²/hr), the damped
    /// ground-temperature formula attenuates the seasonal swing to roughly half
    /// the outdoor air temperature amplitude, matching the OCHRE/DOE-2 GTEMP
    /// implementation.
    ///
    /// With `α = 0.002_322_576 m²/hr` and `depth = 3.048 m`:
    ///   beta = sqrt(pi / (8760 * 0.002_322_576)) * 3.048 ≈ 1.198
    ///   gm ≈ 0.55 (the theoretical attenuation factor at 10 ft).
    #[test]
    fn doe2_ground_ref_depth_3_048_m_damps_seasonal_amplitude() {
        use super::doe2_ground_temp_from_monthly_avg;

        let monthly_means: [f64; 12] = [
            -8.0, -5.0, 0.0, 6.0, 12.0, 18.0, 22.0, 20.0, 14.0, 7.0, 1.0, -4.0,
        ];
        let air_amplitude = (monthly_means
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max)
            - monthly_means.iter().copied().fold(f64::INFINITY, f64::min))
            / 2.0; // 15.0 °C

        let ground = doe2_ground_temp_from_monthly_avg(&monthly_means);
        let g_min = ground.iter().copied().fold(f64::INFINITY, f64::min);
        let g_max = ground.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let ground_amplitude = (g_max - g_min) / 2.0;

        let ratio = ground_amplitude / air_amplitude;

        // Compute the theoretical gm from the DOE-2 beta formula.
        let beta = (std::f64::consts::PI / (DOE2_GROUND_HOURS_PER_YEAR * DOE2_GROUND_DIFFUSIVITY))
            .sqrt()
            * DOE2_GROUND_REFERENCE_DEPTH_M;
        let x = (-beta).exp();
        let y = (x * x - 2.0 * x * beta.cos() + 1.0) / (2.0 * beta * beta);
        let gm = y.sqrt();

        // The 12-element output samples the mid-month days; the observed
        // amplitude ratio is gm scaled by how close the mid-month cosine
        // values get to ±1 given the phase offset. Compute this analytically:
        let z = (1.0 - x * (beta.cos() + beta.sin())) / (1.0 - x * (beta.cos() - beta.sin()));
        let phase = DOE2_GROUND_PHASE_OFFSET_RAD + z.atan();
        let cos_values: Vec<f64> = DOE2_MID_MONTH_DAYS
            .iter()
            .map(|&day| {
                (2.0 * std::f64::consts::PI / DOE2_GROUND_DAYS_PER_YEAR * day - phase).cos()
            })
            .collect();
        let cos_max = cos_values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let cos_min = cos_values.iter().copied().fold(f64::INFINITY, f64::min);
        // The expected amplitude from the 12 samples: dt_monthly * gm * (cos_max - cos_min) / 2
        // So the ratio is gm * (cos_max - cos_min) / 2.
        let expected_ratio = gm * (cos_max - cos_min) / 2.0;

        assert!(
            (ratio - expected_ratio).abs() < 1e-10,
            "amplitude ratio {ratio} should equal expected {expected_ratio} (gm = {gm:.4}, \
             cos range = {cos_min:.4} to {cos_max:.4})"
        );

        // At 3.048 m (10 ft) with α = 0.002_322_576 m²/hr (DOE-2 OEM 0.025 ft²/hr
        // converted to SI), gm ≈ 0.55 — about 45% amplitude damping, matching the
        // DOE-2 GTEMP correlation at the OEM 10 ft reference depth.
        assert!(
            (gm - 0.55).abs() < 0.05,
            "gm = {gm:.4} should be near 0.55 at 3.048 m depth"
        );
    }

    /// Verifies that at `DOE2_GROUND_REFERENCE_DEPTH_M = 3.048` (10 ft) and
    /// `DOE2_GROUND_DIFFUSIVITY = 0.002_322_576 m²/hr` (DOE-2 default in SI),
    /// the beta value matches OCHRE's imperial calculation at the same OEM depth.
    ///
    /// OCHRE: `beta = sqrt(π / (8760 × 0.025 ft²/hr)) × 10 ft ≈ 1.198`
    /// HARES: `beta = sqrt(π / (8760 × 0.002_322_576 m²/hr)) × 3.048 m ≈ 1.198`
    #[test]
    fn doe2_ground_beta_matches_ochre_10ft() {
        let beta = (std::f64::consts::PI / (DOE2_GROUND_HOURS_PER_YEAR * DOE2_GROUND_DIFFUSIVITY))
            .sqrt()
            * DOE2_GROUND_REFERENCE_DEPTH_M;
        assert!(
            (beta - 1.198).abs() < 0.001,
            "beta = {beta:.4} should be ≈ 1.198 (OCHRE: beta = sqrt(pi/(8760*0.025)) * 10 ft)"
        );
    }

    /// With the corrected DOE-2 reference depth (3.048 m), the ground temperature
    /// seasonal amplitude must be strictly less than the outdoor air temperature
    /// amplitude. This confirms the amplitude damping is working correctly — if
    /// the ground amplitude equals or exceeds the air amplitude, the depth is
    /// too shallow or soil diffusivity is implausibly low.
    #[test]
    fn doe2_ground_temp_seasonal_amplitude_less_than_air() {
        use super::doe2_ground_temp_from_monthly_avg;

        let monthly_means: [f64; 12] = [
            -10.0, -5.0, 0.0, 8.0, 16.0, 22.0, 26.0, 24.0, 18.0, 10.0, 2.0, -6.0,
        ];
        let air_min = monthly_means.iter().copied().fold(f64::INFINITY, f64::min);
        let air_max = monthly_means
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        let air_amplitude = air_max - air_min;

        let ground = doe2_ground_temp_from_monthly_avg(&monthly_means);
        let ground_min = ground.iter().copied().fold(f64::INFINITY, f64::min);
        let ground_max = ground.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let ground_amplitude = ground_max - ground_min;

        assert!(
            ground_amplitude < air_amplitude,
            "ground amplitude {ground_amplitude:.2}°C must be strictly less than air amplitude \
             {air_amplitude:.2}°C — DOE-2 damping is not working correctly at depth \
             {DOE2_GROUND_REFERENCE_DEPTH_M} m"
        );
    }

    /// An EPW with a non-numeric monthly value in the GROUND TEMPERATURES header
    /// must return `None` from `parse_ground_temperatures`, triggering the DOE-2
    /// fallback — not silently substitute a default value.
    #[test]
    fn ground_temp_malformed_monthly_triggers_doe2_fallback() {
        // Build a header with one depth (0.5 m) where one monthly value is "N/A".
        let mut parts = vec![
            "GROUND TEMPERATURES".to_string(),
            "1".to_string(),   // num_depths = 1
            "0.5".to_string(), // depth
            String::new(),     // soil conductivity (blank)
            String::new(),     // soil density (blank)
            String::new(),     // soil specific heat (blank)
        ];
        for i in 0..12 {
            if i == 5 {
                parts.push("N/A".to_string());
            } else {
                parts.push("15.0".to_string());
            }
        }
        let gt_line = parts.join(",");
        let epw = epw_with_ground_header(8760, &gt_line);
        let parsed = parse_epw_str(&epw).expect("EPW should parse");

        // When the GROUND TEMPERATURES header has malformed data, the DOE-2
        // fallback computes ground temps from the dry-bulb series. Since the
        // synthetic EPW has constant dry-bulb (20°C), the ground temps should
        // converge to ~20°C — NOT the old 10°C default and NOT the malformed
        // header data.
        for (i, &gt) in parsed.ground_temp_c.iter().enumerate() {
            assert!(
                (gt - 20.0).abs() < 1.0,
                "hour {i}: ground_temp {gt:.4} should be near the DOE-2 fallback (~20°C from \
                 constant dry-bulb), not the old 10°C default"
            );
        }
    }

    /// Calling `doe2_ground_temp_monthly` with an empty dry-bulb slice must
    /// return an error, not silently substitute a default temperature.
    #[test]
    fn doe2_ground_temp_empty_dry_bulb_returns_err() {
        let result = doe2_ground_temp_monthly(&[], false);
        assert!(
            result.is_err(),
            "empty dry-bulb should return Err, not silently substitute a default"
        );
        let err = result.unwrap_err();
        assert!(matches!(err, WeatherError::Parse(_)));
        assert!(
            err.to_string().contains("empty dry-bulb"),
            "error message should mention empty dry-bulb: {err}"
        );
    }

    #[test]
    fn epw_surface_albedo_is_none() {
        let epw = build_synthetic_epw(8760, |_, _| {});
        let ts = parse_epw_str(&epw).expect("should parse");
        assert!(
            ts.surface_albedo.is_none(),
            "EPW has no albedo column; surface_albedo should be None"
        );
    }

    // -----------------------------------------------------------------------
    // Sky emissivity models
    // -----------------------------------------------------------------------

    #[test]
    fn berdahl_martin_emissivity_known_case() {
        // T_dp = 10 C => x = 0.1
        // ε = 0.758 + 0.521 * 0.1 + 0.625 * 0.01 = 0.758 + 0.0521 + 0.00625 = 0.81635
        let eps = berdahl_martin_sky_emissivity(10.0);
        assert!(
            (eps - 0.81635).abs() < 1e-5,
            "Berdahl-Martin at T_dp=10C: got {eps}, expected 0.81635"
        );
    }

    #[test]
    fn brunt_emissivity_matches_energyplus_vapor_pressure_method() {
        // EnergyPlus method: P_wv = PsyPsatFnTemp(DryBulb) * RH * 0.01 [hPa]
        // at T_db = 20°C, RH = 50%:
        //   Magnus P_sat(20°C) = 6.1078 × exp(17.27 × 20 / 257.3) ≈ 23.39 hPa
        //   P_wv = 23.39 × 0.50 = 11.695 hPa
        //   ε = 0.618 + 0.056 × sqrt(11.695) = 0.618 + 0.056 × 3.4198 ≈ 0.8095
        //
        // Cite: EnergyPlus WeatherManager.cc:3204–3206 (CalcSkyEmissivity, Brunt case).
        let eps = brunt_sky_emissivity(20.0, 50.0);
        assert!(
            (eps - 0.8095).abs() < 0.001,
            "Brunt at T_db=20C, RH=50%: got {eps}"
        );
    }

    #[test]
    fn idso_emissivity_matches_energyplus_vapor_pressure_method() {
        // EnergyPlus method: P_wv = PsyPsatFnTemp(DryBulb) * RH * 0.01 [hPa]
        // at T_db = 20°C, RH = 50%:
        //   P_wv = 11.695 hPa (same as Brunt derivation above)
        //   T_db_K = 293.15, exp(1699 / 293.15) ≈ 328.87
        //   ε = 0.685 + 3.2e-5 × 11.695 × 328.87 ≈ 0.8081
        //
        // Cite: EnergyPlus WeatherManager.cc:3207–3209 (CalcSkyEmissivity, Idso case).
        let eps = idso_sky_emissivity(20.0, 50.0);
        assert!(
            (eps - 0.8081).abs() < 0.001,
            "Idso at T_db=20C, RH=50%: got {eps}"
        );
    }

    #[test]
    fn walton_cloud_correction_clear_sky() {
        // N = 0 => correction factor = 1.0
        let eps_clear = 0.8;
        let eps_corrected = walton_cloud_correction(eps_clear, 0.0);
        assert!(
            (eps_corrected - eps_clear).abs() < 1e-10,
            "clear sky (N=0) should not modify emissivity: got {eps_corrected}"
        );
    }

    #[test]
    fn walton_cloud_correction_overcast() {
        // N = 10 => factor = 1 + 0.0224*10 - 0.0035*100 + 0.00028*1000
        //         = 1 + 0.224 - 0.35 + 0.28 = 1.154
        let eps_clear = 0.8;
        let eps_corrected = walton_cloud_correction(eps_clear, 10.0);
        let expected = eps_clear * 1.154;
        assert!(
            (eps_corrected - expected).abs() < 1e-6,
            "overcast (N=10) correction: got {eps_corrected}, expected {expected}"
        );
        assert!(
            eps_corrected > eps_clear,
            "overcast emissivity must exceed clear-sky"
        );
    }

    #[test]
    fn sky_temp_berdahl_martin_with_clouds() {
        // Cloud-corrected sky temp should be warmer (higher) than clear-sky
        let t_db = 20.0;
        let t_dp = 10.0;
        let eps_clear = berdahl_martin_sky_emissivity(t_dp);
        let t_clear = sky_temp_from_emissivity(t_db, eps_clear);

        let eps_cloudy = walton_cloud_correction(eps_clear, 8.0);
        let t_cloudy = sky_temp_from_emissivity(t_db, eps_cloudy);

        assert!(
            t_cloudy > t_clear,
            "cloudy sky temp ({t_cloudy}) should exceed clear-sky ({t_clear})"
        );
    }

    #[test]
    fn fallback_to_clark_allen_when_no_clouds() {
        // opaque_sky_cover=0 with low IR => Clark-Allen result
        let t_sky = compute_sky_temp_c(0.0, 20.0, 10.0, 50.0, 0.0, SkyTempModel::default());
        let t_clark = clark_allen_sky_temp_c(20.0, 10.0);
        assert!(
            (t_sky - t_clark).abs() < 1e-10,
            "no cloud cover should produce Clark-Allen: got {t_sky}, expected {t_clark}"
        );
    }

    #[test]
    fn berdahl_martin_used_when_clouds_available() {
        // opaque_sky_cover > 0 with low IR => Berdahl-Martin + Walton
        let t_db = 20.0;
        let t_dp = 10.0;
        let cloud = 5.0;
        let t_sky = compute_sky_temp_c(0.0, t_db, t_dp, 50.0, cloud, SkyTempModel::BerdahlMartin);

        let eps_clear = berdahl_martin_sky_emissivity(t_dp);
        let eps_sky = walton_cloud_correction(eps_clear, cloud);
        let expected = sky_temp_from_emissivity(t_db, eps_sky);

        assert!(
            (t_sky - expected).abs() < 1e-10,
            "cloud cover > 0 should use Berdahl-Martin+Walton: got {t_sky}, expected {expected}"
        );
    }

    #[test]
    fn sky_temp_clamped_at_high_humidity_overcast() {
        // T_dp=35°C (tropical), N=10 (overcast): ε_berdahl > 1.0 before clamping
        let eps_clear = berdahl_martin_sky_emissivity(35.0);
        assert!(
            eps_clear > 0.95,
            "high dew point should give high emissivity"
        );
        let eps_cloud = walton_cloud_correction(eps_clear, 10.0);
        assert!(
            eps_cloud <= 1.0,
            "emissivity must be clamped to <= 1.0, got {eps_cloud}"
        );
        let t_sky = sky_temp_from_emissivity(38.0, eps_cloud);
        assert!(
            t_sky <= 38.0,
            "sky temp must not exceed dry bulb: got {t_sky}"
        );
    }

    #[test]
    fn stefan_boltzmann_still_primary() {
        // When IR >= 50, result is the same regardless of cloud cover
        let ir = 300.0;
        let t_sky_no_cloud = compute_sky_temp_c(ir, 20.0, 10.0, 50.0, 0.0, SkyTempModel::default());
        let t_sky_cloudy = compute_sky_temp_c(ir, 20.0, 10.0, 50.0, 8.0, SkyTempModel::default());
        assert!(
            (t_sky_no_cloud - t_sky_cloudy).abs() < 1e-10,
            "IR >= 50 ignores cloud cover: {t_sky_no_cloud} vs {t_sky_cloudy}"
        );
        let expected_k = (ir / STEFAN_BOLTZMANN).powf(0.25);
        let expected_c = expected_k - 273.15;
        assert!(
            (t_sky_no_cloud - expected_c).abs() < 0.01,
            "IR >= 50 should use Stefan-Boltzmann: got {t_sky_no_cloud}, expected {expected_c}"
        );
    }

    // -----------------------------------------------------------------------
    // Berdahl-Martin coefficient citation
    // -----------------------------------------------------------------------
    //
    // The original report asserts that coefficients 0.758/0.521/0.625 are NOT from the
    // original Berdahl & Martin (1984) Solar Energy 32(5) paper (whose values
    // are 0.711/0.56/0.73), but from the Li, Jiang & Coimbra (2017) Solar Energy
    // 144:40-48 recalibration (as used by EnergyPlus 9.3+). The test below pins
    // the coefficient values so that any future edit that "corrects" them back to
    // the original 1984 values (0.711/0.56/0.73) will be caught immediately.

    #[test]
    fn berdahl_martin_coefficients_match_energyplus_recalibrated_set() {
        // EnergyPlus 9.3+ (Engineering Reference, Sky Radiation Modeling) and
        // Li, Jiang & Coimbra (2017) Solar Energy 144:40-48 use exactly:
        //   ε_clear = 0.758 + 0.521*(T_dp/100) + 0.625*(T_dp/100)²
        //
        // The *original* Berdahl & Martin (1984) Solar Energy 32(5):663-664 used:
        //   ε_clear = 0.711 + 0.56*(T_dp/100) + 0.73*(T_dp/100)²
        //
        // At T_dp = 0 °C: both formulas reduce to the constant term alone.
        // EnergyPlus form gives 0.758; original gives 0.711.  If the code were
        // reverted to the original 1984 coefficients this assertion would fail.
        let eps_at_zero_dp = berdahl_martin_sky_emissivity(0.0);
        assert!(
            (eps_at_zero_dp - 0.758).abs() < 1e-9,
            "constant term should be 0.758 (EnergyPlus/Li et al. 2017 recalibrated); \
             got {eps_at_zero_dp:.6}. Original Berdahl & Martin 1984 value is 0.711."
        );

        // At T_dp = 10 °C: EnergyPlus form gives 0.81635; original gives 0.7784.
        let eps_at_10 = berdahl_martin_sky_emissivity(10.0);
        assert!(
            (eps_at_10 - 0.81635).abs() < 1e-5,
            "at T_dp=10°C expected EnergyPlus recalibrated 0.81635; got {eps_at_10:.6}. \
             Original 1984 form would give ≈0.7784."
        );

        // At T_dp = 20 °C: EnergyPlus form = 0.758 + 0.521*0.2 + 0.625*0.04 = 0.8872.
        // Original 1984 form = 0.711 + 0.56*0.2 + 0.73*0.04 = 0.8522.
        let eps_at_20 = berdahl_martin_sky_emissivity(20.0);
        assert!(
            (eps_at_20 - 0.8872).abs() < 1e-4,
            "at T_dp=20°C expected EnergyPlus recalibrated 0.8872; got {eps_at_20:.6}. \
             Original 1984 form would give ≈0.8522."
        );
    }

    // -----------------------------------------------------------------------
    // EPW liquid precip silent zero default
    // -----------------------------------------------------------------------

    // Helper: build a synthetic EPW where every row has exactly `field_count`
    // comma-separated fields (truncating or padding from build_synthetic_epw's 35).
    fn build_short_field_epw(field_count: usize) -> String {
        build_synthetic_epw(8760, |_row, fields| {
            // Truncate extra fields by overwriting with empty strings beyond
            // field_count. We can't actually shorten the array, but we can
            // reconstruct the row in the loop body using a mutator that leaves
            // placeholders; instead we use a different approach via post-process.
            let _ = fields; // handled by string replacement below
        })
        // Rebuild: each data row has 35 comma-separated fields; truncate to field_count.
        .lines()
        .enumerate()
        .map(|(i, line)| {
            if i < 8 {
                // Header lines – leave unchanged.
                line.to_string()
            } else {
                let mut parts: Vec<&str> = line.splitn(36, ',').collect();
                parts.truncate(field_count);
                parts.join(",")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
    }

    /// When EPW rows have fewer than 34 fields (field 33 absent),
    /// the code emits a single `tracing::debug!` at file-parse time and
    /// sets all `liquid_precip_m` values to 0.0.
    #[test]
    fn absent_field_33_yields_zero_with_debug_diagnostic() {
        // Build an EPW with only 30 fields per row — field 33 is absent.
        // EPW_RECORD_MIN_FIELDS is 24, so this passes the minimum check.
        let epw = build_short_field_epw(30);
        let parsed = parse_epw_str(&epw).expect("EPW with 30 fields should parse");

        assert!(
            parsed.liquid_precip_m.iter().all(|&v| v == 0.0),
            "absent field 33 must yield liquid_precip_m = 0.0 for every row"
        );
        assert_eq!(parsed.liquid_precip_m.len(), 8760);
    }

    /// When field 33 contains the EPW missing-data sentinel
    /// (999 per EnergyPlus EPW Data Dictionary §N33, \missing 999),
    /// the value is detected and treated as missing data (0.0), and
    /// a `tracing::debug!` is emitted at file level.
    ///
    /// Note: the original report incorrectly cites the sentinel as 9999; the correct
    /// EnergyPlus value is 999 (verified against E+ 9.6 and 24.2 docs).
    /// The threshold `>= 900.0` catches 999 and common sentinel variants.
    #[test]
    fn sentinel_999_in_field_33_treated_as_missing() {
        // Set field 33 of row 100 to the EPW missing-data sentinel for
        // Liquid Precipitation Depth: 999 mm (per E+ IDD \missing 999).
        let epw = build_synthetic_epw(8760, |row, fields| {
            if row == 100 {
                fields[33] = "999".to_string();
            }
        });
        let parsed = parse_epw_str(&epw).expect("EPW with sentinel 999 should parse");

        let sentinel_row_value = parsed.liquid_precip_m[100];
        assert!(
            sentinel_row_value == 0.0,
            "sentinel 999 in field 33 must be treated as missing; expected 0.0 m, got {sentinel_row_value} m"
        );
    }

    /// When field 33 is present with a parse error (non-numeric),
    /// the code emits a `tracing::warn!` with row number and raw field value,
    /// then substitutes 0.0.
    #[test]
    fn parse_error_in_field_33_warns_and_substitutes_zero() {
        // Inject a non-numeric string into field 33 of row 50.
        let epw = build_synthetic_epw(8760, |row, fields| {
            if row == 50 {
                fields[33] = "N/A".to_string();
            }
        });
        let parsed = parse_epw_str(&epw)
            .expect("EPW with non-numeric field 33 should not error (substitutes 0.0)");

        assert_eq!(
            parsed.liquid_precip_m[50], 0.0,
            "non-numeric field 33 must substitute 0.0 for row 51"
        );
    }

    // -----------------------------------------------------------------------
    // Caller-provided coordinates not silently discarded for EPW files.
    //
    // `parse_weather_with_location` emits a tracing::warn! when the caller
    // supplies non-zero coordinates that differ from the file's embedded
    // location by more than 1.0°. The file's location remains authoritative
    // — coordinates are NOT overridden. Use `parse_weather_override_location`
    // when the file's metadata is known wrong.
    //
    // EnergyPlus Input-Output Reference: WeatherManager.cc emits a warning
    // when the IDF Site:Location coordinates differ from the EPW file's
    // embedded coordinates.
    // -----------------------------------------------------------------------
    #[test]
    fn epw_caller_coordinates_not_silently_discarded() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();

        // Build a synthetic EPW whose LOCATION header embeds Denver, CO (39.74, -104.99).
        let epw = build_synthetic_epw(8760, |_row, _fields| {});
        let path = write_temp_epw(&epw);

        // Supply Phoenix, AZ coordinates — differ by ~6.3° lat and ~7.1° lon.
        let caller_lat = 33.45_f64;
        let caller_lon = -112.07_f64;
        let caller_tz = -7.0_f64;
        let caller_elev = 331.0_f64;

        let result = crate::weather::parse_weather_with_location(
            &path,
            caller_elev,
            caller_lat,
            caller_lon,
            caller_tz,
        );
        let _ = fs::remove_file(path);
        let weather = result.expect("EPW should parse without error");

        // File's embedded location remains authoritative — the caller coordinates
        // are NOT used to override meta. The function warns, not overrides.
        assert!(
            (weather.meta.latitude - 39.74).abs() < 0.001,
            "file latitude should remain authoritative (39.74°); got {}",
            weather.meta.latitude
        );
        assert!(
            (weather.meta.longitude - (-104.99)).abs() < 0.001,
            "file longitude should remain authoritative (-104.99°); got {}",
            weather.meta.longitude
        );
    }

    #[test]
    fn epw_zero_caller_coordinates_skip_check() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();

        let epw = build_synthetic_epw(8760, |_row, _fields| {});
        let path = write_temp_epw(&epw);

        // All-zero caller coordinates (no HPXML site data).
        let result = crate::weather::parse_weather_with_location(&path, 0.0, 0.0, 0.0, 0.0);
        let _ = fs::remove_file(path);
        let weather = result.expect("EPW should parse without error");

        // File coords are used since caller provided no location.
        assert!((weather.meta.latitude - 39.74).abs() < 0.001);
        assert!((weather.meta.longitude - (-104.99)).abs() < 0.001);
    }

    #[test]
    fn epw_override_location_replaces_all_meta_fields() {
        let epw = build_synthetic_epw(8760, |_row, _fields| {});
        let path = write_temp_epw(&epw);

        // Override with Phoenix, AZ coordinates and elevation.
        let override_lat = 33.45_f64;
        let override_lon = -112.07_f64;
        let override_elev = 331.0_f64;
        let override_tz = -7.0_f64;

        let result = crate::weather::parse_weather_override_location(
            &path,
            override_lat,
            override_lon,
            override_elev,
            override_tz,
        );
        let _ = fs::remove_file(path);
        let weather = result.expect("EPW should parse without error");

        // All four meta fields must match the caller-provided override values exactly.
        assert!(
            (weather.meta.latitude - override_lat).abs() < 0.001,
            "latitude: expected {}, got {}",
            override_lat,
            weather.meta.latitude
        );
        assert!(
            (weather.meta.longitude - override_lon).abs() < 0.001,
            "longitude: expected {}, got {}",
            override_lon,
            weather.meta.longitude
        );
        assert!(
            (weather.meta.elevation_m - override_elev).abs() < 0.001,
            "elevation_m: expected {}, got {}",
            override_elev,
            weather.meta.elevation_m
        );
        assert!(
            (weather.meta.timezone_offset_h - override_tz).abs() < 0.001,
            "timezone_offset_h: expected {}, got {}",
            override_tz,
            weather.meta.timezone_offset_h
        );
    }

    // -----------------------------------------------------------------------
    // HOLIDAYS/DAYLIGHT SAVINGS header parsing (T-0030)
    // -----------------------------------------------------------------------

    /// Build a synthetic EPW header block with a custom HOLIDAYS/DAYLIGHT SAVINGS
    /// line. This lets test code vary the `Leap Year Observed` field (A1) without
    /// replicating the entire header template.
    fn build_synthetic_epw_with_holidays(
        rows: usize,
        holidays_header: &str,
        mutator: impl FnMut(usize, &mut [String; 35]),
    ) -> String {
        let epw = build_synthetic_epw(rows, mutator);
        // Replace the default holidays line with the caller's version.
        epw.replacen("HOLIDAYS/DAYLIGHT SAVINGS,Yes,0,0,0", holidays_header, 1)
    }

    #[test]
    fn parse_holidays_daylight_header_yes() {
        let epw = build_synthetic_epw_with_holidays(
            8760,
            "HOLIDAYS/DAYLIGHT SAVINGS,Yes,0,0,0",
            |_, _| {},
        );
        let parsed = parse_epw_str(&epw).expect("valid header should parse");
        assert!(parsed.meta.wf_allows_leap_years);
    }

    #[test]
    fn parse_holidays_daylight_header_no() {
        let epw = build_synthetic_epw_with_holidays(
            8760,
            "HOLIDAYS/DAYLIGHT SAVINGS,No,0,0,0",
            |_, _| {},
        );
        let parsed = parse_epw_str(&epw).expect("valid header should parse");
        assert!(!parsed.meta.wf_allows_leap_years);
    }

    #[test]
    fn parse_holidays_daylight_header_yes_with_dst_dates() {
        // Real EPW files may include DST fields after A1; the parser should
        // correctly extract just the first value field (A1).
        let epw = build_synthetic_epw_with_holidays(
            8760,
            "HOLIDAYS/DAYLIGHT SAVINGS,Yes,3/8,11/1",
            |_, _| {},
        );
        let parsed = parse_epw_str(&epw).expect("header with DST dates should parse");
        assert!(parsed.meta.wf_allows_leap_years);
    }

    #[test]
    fn parse_holidays_daylight_header_unrecognised_value_rejected() {
        let epw = build_synthetic_epw_with_holidays(
            8760,
            "HOLIDAYS/DAYLIGHT SAVINGS,Maybe,0,0,0",
            |_, _| {},
        );
        let err = parse_epw_str(&epw).expect_err("unrecognised A1 value should fail");
        assert!(
            err.to_string().contains("Maybe"),
            "error should mention the unrecognised value: {err}"
        );
        assert!(
            err.to_string().contains("Yes") || err.to_string().contains("No"),
            "error should mention expected values: {err}"
        );
    }

    #[test]
    fn parse_holidays_daylight_header_missing_a1_rejected() {
        let epw = build_synthetic_epw_with_holidays(8760, "HOLIDAYS/DAYLIGHT SAVINGS", |_, _| {});
        let err = parse_epw_str(&epw).expect_err("missing A1 field should fail");
        assert!(
            err.to_string().contains("A1") || err.to_string().contains("Leap Year"),
            "error should mention the missing field: {err}"
        );
    }

    #[test]
    fn parse_epw_strips_feb29_when_header_says_no_and_8784_rows() {
        // 8784 rows + "No" header → Feb 29 data discarded, resulting in 8760 rows.
        let epw = build_synthetic_epw_with_holidays(
            8784,
            "HOLIDAYS/DAYLIGHT SAVINGS,No,0,0,0",
            |_, _| {},
        );
        let parsed = parse_epw_str(&epw).expect("8784+No EPW should parse");
        assert_eq!(
            parsed.len(),
            8760,
            "Feb 29 stripped: expected 8760 rows, got {}",
            parsed.len()
        );
        assert!(
            !parsed.meta.wf_allows_leap_years,
            "meta should report leap years not allowed"
        );
        // monthly_day_counts(false) should report 28 days for February.
        let day_counts = monthly_day_counts(false);
        assert_eq!(day_counts[1], 28);
    }

    #[test]
    fn parse_epw_honours_feb29_when_header_says_yes_and_8784_rows() {
        // 8784 rows + "Yes" header → Feb 29 data honoured.
        let epw = build_synthetic_epw_with_holidays(
            8784,
            "HOLIDAYS/DAYLIGHT SAVINGS,Yes,0,0,0",
            |_, _| {},
        );
        let parsed = parse_epw_str(&epw).expect("8784+Yes EPW should parse");
        assert_eq!(
            parsed.len(),
            8784,
            "Feb 29 honoured: expected 8784 rows, got {}",
            parsed.len()
        );
        assert!(parsed.meta.wf_allows_leap_years);
        let day_counts = monthly_day_counts(true);
        assert_eq!(day_counts[1], 29);
    }

    #[test]
    fn epw_8760_yes_no_change() {
        // 8760 rows + "Yes" header → normal year, no Feb 29 to strip.
        let epw = build_synthetic_epw_with_holidays(
            8760,
            "HOLIDAYS/DAYLIGHT SAVINGS,Yes,0,0,0",
            |_, _| {},
        );
        let parsed = parse_epw_str(&epw).expect("8760+Yes EPW should parse");
        assert_eq!(parsed.len(), 8760);
        assert!(parsed.meta.wf_allows_leap_years);
    }

    #[test]
    fn epw_8760_no_no_change() {
        // 8760 rows + "No" header → normal year, no stripping needed.
        let epw = build_synthetic_epw_with_holidays(
            8760,
            "HOLIDAYS/DAYLIGHT SAVINGS,No,0,0,0",
            |_, _| {},
        );
        let parsed = parse_epw_str(&epw).expect("8760+No EPW should parse");
        assert_eq!(parsed.len(), 8760);
        assert!(!parsed.meta.wf_allows_leap_years);
    }
}
