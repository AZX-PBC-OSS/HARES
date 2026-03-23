//! PSM3/NSRDB CSV file parser (SAM CSV format).
//!
//! PSM3 files are produced by NREL's National Solar Radiation Database (NSRDB)
//! Physical Solar Model v3. Format: 2-line header (metadata + column names),
//! then data rows at 5/15/30/60-minute intervals.
//!
//! Reference: <https://developer.nrel.gov/docs/solar/nsrdb/psm3-download/>

use std::fs;
use std::path::Path;

use chrono::{Datelike, NaiveDate};

use crate::epw::{
    clark_allen_sky_temp_c, doe2_ground_temp_monthly, interpolate_ground_temp_c,
};
#[cfg(test)]
use crate::epw::monthly_day_counts;
use crate::weather::{WeatherError, WeatherMeta, WeatherTimeSeries};

/// Valid PSM3 timestep intervals in seconds.
const VALID_STEP_SECS: [u32; 4] = [300, 900, 1800, 3600];

/// Expected record counts for non-leap year at each supported interval.
const RECORDS_PER_YEAR_STANDARD: [usize; 4] = [105_120, 35_040, 17_520, 8_760];
/// Expected record counts for leap year at each supported interval.
const RECORDS_PER_YEAR_LEAP: [usize; 4] = [105_408, 35_136, 17_568, 8_784];

/// PSM3 metadata header field indices (comma-separated line 1).
const META_IDX_LATITUDE: usize = 5;
const META_IDX_LONGITUDE: usize = 6;
const META_IDX_TIMEZONE: usize = 7;
const META_IDX_ELEVATION: usize = 8;

/// Parse a PSM3/NSRDB CSV file (SAM CSV format) into a [`WeatherTimeSeries`].
///
/// PSM3 files are produced by NREL's National Solar Radiation Database (NSRDB)
/// Physical Solar Model v3. Format: 2-line header (metadata + column names),
/// then data rows at 5/15/30/60-minute intervals.
///
/// Unit conversions applied:
/// - Pressure: millibar (mbar) to kilopascal (kPa), divided by 10.
/// - Sky temperature: computed via Clark-Allen empirical correlation (no IR data in PSM3).
/// - Ground temperature: DOE-2 sinusoidal model with monthly dry-bulb averages.
///
/// Reference: <https://developer.nrel.gov/docs/solar/nsrdb/psm3-download/>
pub fn parse_psm3(path: impl AsRef<Path>) -> Result<WeatherTimeSeries, WeatherError> {
    let path_ref = path.as_ref();
    let contents = fs::read_to_string(path_ref).map_err(|source| WeatherError::Io {
        path: path_ref.display().to_string(),
        source,
    })?;

    parse_psm3_str(&contents)
}

fn parse_psm3_str(contents: &str) -> Result<WeatherTimeSeries, WeatherError> {
    let mut lines = contents.lines();

    // --- Line 1: metadata ---
    let meta_line = lines
        .next()
        .ok_or_else(|| WeatherError::Parse("missing PSM3 metadata header (line 1)".into()))?;
    let meta_fields: Vec<&str> = meta_line.split(',').collect();
    let min_meta = META_IDX_ELEVATION + 1;
    if meta_fields.len() < min_meta {
        return Err(WeatherError::Parse(format!(
            "PSM3 metadata header must have at least {min_meta} fields, got {}",
            meta_fields.len()
        )));
    }

    let latitude = parse_meta_f64(meta_fields[META_IDX_LATITUDE], "Latitude")?;
    let longitude = parse_meta_f64(meta_fields[META_IDX_LONGITUDE], "Longitude")?;
    let timezone_offset_h = parse_meta_f64(meta_fields[META_IDX_TIMEZONE], "Time Zone")?;
    let elevation_m = parse_meta_f64(meta_fields[META_IDX_ELEVATION], "Elevation")?;

    // Location label: use City (index 2) if available, else "PSM3".
    let location = if meta_fields.len() > 2 && !meta_fields[2].trim().is_empty() {
        meta_fields[2].trim().to_string()
    } else {
        "PSM3".to_string()
    };

    // --- Line 2: column names ---
    let header_line = lines
        .next()
        .ok_or_else(|| WeatherError::Parse("missing PSM3 column header (line 2)".into()))?;
    let col_names: Vec<&str> = header_line.split(',').map(str::trim).collect();
    let col_map = build_column_map(&col_names)?;

    // --- Data rows ---
    let data_lines: Vec<&str> = lines.filter(|l| !l.trim().is_empty()).collect();
    if data_lines.len() < 2 {
        return Err(WeatherError::Parse(
            "PSM3 file must have at least 2 data rows".into(),
        ));
    }

    // Auto-detect timestep from first two rows.
    let step_secs = detect_timestep(&data_lines[0], &data_lines[1], &col_map)?;

    let n = data_lines.len();

    // Pre-allocate column vectors.
    let mut dry_bulb_c = Vec::with_capacity(n);
    let mut dew_point_c = Vec::with_capacity(n);
    let mut rel_humidity_pct = Vec::with_capacity(n);
    let mut pressure_kpa = Vec::with_capacity(n);
    let mut ghi_w_m2 = Vec::with_capacity(n);
    let mut dni_w_m2 = Vec::with_capacity(n);
    let mut dhi_w_m2 = Vec::with_capacity(n);
    let mut wind_speed_m_s = Vec::with_capacity(n);
    let mut wind_dir_deg = Vec::with_capacity(n);
    let mut timestamps: Vec<(u32, u32, u32)> = Vec::with_capacity(n); // (month, day, hour)

    for (data_idx, line) in data_lines.iter().enumerate() {
        let row = data_idx + 1;
        let fields: Vec<&str> = line.split(',').collect();

        let db = parse_data_f64(&fields, col_map.temperature, row, "Temperature")?;
        if !(-60.0..=55.0).contains(&db) {
            return Err(WeatherError::Validation(format!(
                "row {row}: temperature out of range [-60, 55] C: {db}"
            )));
        }

        let dp = parse_data_f64(&fields, col_map.dew_point, row, "Dew Point")?;
        if dp > db {
            return Err(WeatherError::Validation(format!(
                "row {row}: dew point ({dp}) exceeds dry bulb ({db})"
            )));
        }

        let rh = parse_data_f64(&fields, col_map.relative_humidity, row, "Relative Humidity")?;

        // PSM3 pressure is in millibar (mbar); convert to kPa by dividing by 10.
        let pres_mbar = parse_data_f64(&fields, col_map.pressure, row, "Pressure")?;
        let pres_kpa = pres_mbar / 10.0;
        if !(60.0..=110.0).contains(&pres_kpa) {
            return Err(WeatherError::Validation(format!(
                "row {row}: pressure out of range [60, 110] kPa: {pres_kpa}"
            )));
        }

        let ghi = parse_data_f64(&fields, col_map.ghi, row, "GHI")?;
        if !(0.0..=1500.0).contains(&ghi) {
            return Err(WeatherError::Validation(format!(
                "row {row}: GHI out of range [0, 1500] W/m^2: {ghi}"
            )));
        }

        let dni = parse_data_f64(&fields, col_map.dni, row, "DNI")?;
        let dhi = parse_data_f64(&fields, col_map.dhi, row, "DHI")?;

        let ws = parse_data_f64(&fields, col_map.wind_speed, row, "Wind Speed")?;
        if !(0.0..=60.0).contains(&ws) {
            return Err(WeatherError::Validation(format!(
                "row {row}: wind speed out of range [0, 60] m/s: {ws}"
            )));
        }

        let wd = parse_data_f64(&fields, col_map.wind_direction, row, "Wind Direction")?;

        let month = parse_data_u32(&fields, col_map.month, row, "Month")?;
        let day = parse_data_u32(&fields, col_map.day, row, "Day")?;
        let hour = parse_data_u32(&fields, col_map.hour, row, "Hour")?;

        dry_bulb_c.push(db);
        dew_point_c.push(dp);
        rel_humidity_pct.push(rh);
        pressure_kpa.push(pres_kpa);
        ghi_w_m2.push(ghi);
        dni_w_m2.push(dni);
        dhi_w_m2.push(dhi);
        wind_speed_m_s.push(ws);
        wind_dir_deg.push(wd);
        timestamps.push((month, day, hour));
    }

    // Validate record count against expected year length.
    validate_record_count(n, step_secs)?;

    // Determine if leap year from record count.
    let step_idx = VALID_STEP_SECS.iter().position(|&s| s == step_secs).expect("validated step");
    let is_leap_year = n == RECORDS_PER_YEAR_LEAP[step_idx];

    // Compute sky temperature via Clark-Allen (PSM3 has no horizontal IR data).
    let sky_temp_c: Vec<f64> = dry_bulb_c
        .iter()
        .zip(dew_point_c.iter())
        .map(|(&db, &dp)| clark_allen_sky_temp_c(db, dp))
        .collect();

    // Compute ground temperature via DOE-2 model.
    // monthly_average_dry_bulb expects hourly data, so we compute monthly means
    // directly from the sub-hourly data using the known timestep.
    let monthly_ground_temps =
        compute_monthly_means_sub_hourly(&dry_bulb_c, &timestamps, is_leap_year)
            .map(|monthly_avg| {
                // Use the monthly averages to drive DOE-2 ground temp model directly.
                doe2_ground_temp_from_monthly_avg(&monthly_avg)
            })
            .unwrap_or_else(|| doe2_ground_temp_monthly(&dry_bulb_c, is_leap_year));

    let mut ground_temp_c = Vec::with_capacity(n);
    for &(month, day, hour) in &timestamps {
        ground_temp_c.push(interpolate_ground_temp_c(
            &monthly_ground_temps,
            month,
            day,
            // PSM3 uses 0-23 hours; interpolate_ground_temp_c expects 1-24 (EPW convention).
            // Use hour + 1 to convert, but cap at 24 so hour=23 becomes 24.
            (hour + 1).min(24).max(1),
            is_leap_year,
        )?);
    }

    // PSM3 has no horizontal infrared or opaque sky cover data; fill with zeros.
    let horizontal_infrared_w_m2 = vec![0.0; n];
    let opaque_sky_cover = vec![0.0; n];
    let liquid_precip_m = vec![0.0; n];

    let meta = WeatherMeta {
        location,
        latitude,
        longitude,
        timezone_offset_h,
        elevation_m,
        source_step_secs: step_secs,
    };

    Ok(WeatherTimeSeries {
        meta,
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
    })
}

/// Column index mapping for PSM3 data fields.
struct Psm3ColumnMap {
    year: usize,
    month: usize,
    day: usize,
    hour: usize,
    minute: usize,
    temperature: usize,
    dew_point: usize,
    relative_humidity: usize,
    pressure: usize,
    ghi: usize,
    dni: usize,
    dhi: usize,
    wind_speed: usize,
    wind_direction: usize,
}

fn build_column_map(col_names: &[&str]) -> Result<Psm3ColumnMap, WeatherError> {
    let find = |name: &str| -> Result<usize, WeatherError> {
        col_names
            .iter()
            .position(|c| c.eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                WeatherError::Parse(format!("PSM3 missing required column: {name}"))
            })
    };

    Ok(Psm3ColumnMap {
        year: find("Year")?,
        month: find("Month")?,
        day: find("Day")?,
        hour: find("Hour")?,
        minute: find("Minute")?,
        temperature: find("Temperature")?,
        dew_point: find("Dew Point")?,
        relative_humidity: find("Relative Humidity")?,
        pressure: find("Pressure")?,
        ghi: find("GHI")?,
        dni: find("DNI")?,
        dhi: find("DHI")?,
        wind_speed: find("Wind Speed")?,
        wind_direction: find("Wind Direction")?,
    })
}

fn detect_timestep(
    row1: &str,
    row2: &str,
    col_map: &Psm3ColumnMap,
) -> Result<u32, WeatherError> {
    let fields1: Vec<&str> = row1.split(',').collect();
    let fields2: Vec<&str> = row2.split(',').collect();

    let ts1 = row_to_epoch_secs(&fields1, col_map, 1)?;
    let ts2 = row_to_epoch_secs(&fields2, col_map, 2)?;

    let diff = ts2
        .checked_sub(ts1)
        .ok_or_else(|| WeatherError::Parse("PSM3 timestamps not monotonically increasing".into()))?;

    let step = u32::try_from(diff).map_err(|_| {
        WeatherError::Parse(format!("PSM3 timestep overflow: {diff} seconds"))
    })?;

    if !VALID_STEP_SECS.contains(&step) {
        return Err(WeatherError::Validation(format!(
            "PSM3 detected timestep {step}s is not one of {{300, 900, 1800, 3600}}"
        )));
    }

    Ok(step)
}

/// Convert a PSM3 data row's date/time fields to seconds since a reference epoch.
/// We use a simple day-of-year calculation to avoid pulling in full datetime machinery.
fn row_to_epoch_secs(
    fields: &[&str],
    col_map: &Psm3ColumnMap,
    row: usize,
) -> Result<u64, WeatherError> {
    let year = parse_data_u32(fields, col_map.year, row, "Year")?;
    let month = parse_data_u32(fields, col_map.month, row, "Month")?;
    let day = parse_data_u32(fields, col_map.day, row, "Day")?;
    let hour = parse_data_u32(fields, col_map.hour, row, "Hour")?;
    let minute = parse_data_u32(fields, col_map.minute, row, "Minute")?;

    let date = NaiveDate::from_ymd_opt(year as i32, month, day).ok_or_else(|| {
        WeatherError::Parse(format!(
            "row {row}: invalid date year={year}, month={month}, day={day}"
        ))
    })?;

    let day_of_year = date.ordinal() as u64;
    let year_days = if date.leap_year() { 366u64 } else { 365u64 };
    let base = u64::from(year) * year_days * 86400;

    Ok(base + (day_of_year - 1) * 86400 + u64::from(hour) * 3600 + u64::from(minute) * 60)
}

fn validate_record_count(n: usize, step_secs: u32) -> Result<(), WeatherError> {
    let step_idx = VALID_STEP_SECS.iter().position(|&s| s == step_secs).ok_or_else(|| {
        WeatherError::Validation(format!("unexpected timestep: {step_secs}"))
    })?;

    let expected_std = RECORDS_PER_YEAR_STANDARD[step_idx];
    let expected_leap = RECORDS_PER_YEAR_LEAP[step_idx];

    if n != expected_std && n != expected_leap {
        return Err(WeatherError::Validation(format!(
            "PSM3 record count {n} does not match expected {expected_std} \
             (standard year) or {expected_leap} (leap year) for {step_secs}s interval"
        )));
    }

    Ok(())
}

/// Compute monthly average dry-bulb from sub-hourly data by grouping timestamps.
fn compute_monthly_means_sub_hourly(
    dry_bulb_c: &[f64],
    timestamps: &[(u32, u32, u32)],
    _is_leap_year: bool,
) -> Option<[f64; 12]> {
    let mut sums = [0.0f64; 12];
    let mut counts = [0u32; 12];

    for (i, &(month, _, _)) in timestamps.iter().enumerate() {
        let m = (month as usize).checked_sub(1)?;
        if m >= 12 {
            return None;
        }
        sums[m] += dry_bulb_c[i];
        counts[m] += 1;
    }

    let mut result = [0.0f64; 12];
    for i in 0..12 {
        if counts[i] == 0 {
            return None;
        }
        result[i] = sums[i] / f64::from(counts[i]);
    }
    Some(result)
}

/// DOE-2 ground temperature model constants (mirrors epw.rs).
const DOE2_GROUND_HOURS_PER_YEAR: f64 = 8760.0;
const DOE2_GROUND_DAYS_PER_YEAR: f64 = 365.0;
const DOE2_GROUND_DIFFUSIVITY: f64 = 0.025;
const DOE2_GROUND_DEPTH_FACTOR: f64 = 10.0;
const DOE2_GROUND_PHASE_OFFSET_RAD: f64 = 0.6;
const DOE2_MID_MONTH_DAYS: [f64; 12] = [
    15.0, 46.0, 74.0, 95.0, 135.0, 166.0, 196.0, 227.0, 258.0, 288.0, 319.0, 349.0,
];

/// Compute DOE-2 ground temperatures from pre-computed monthly averages.
///
/// This avoids the hourly-data assumption in `doe2_ground_temp_monthly` by
/// accepting monthly means directly.
fn doe2_ground_temp_from_monthly_avg(monthly_avg: &[f64; 12]) -> [f64; 12] {
    let t_avg = monthly_avg.iter().sum::<f64>() / 12.0;
    let t_min = monthly_avg.iter().copied().fold(f64::INFINITY, f64::min);
    let t_max = monthly_avg
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    let dt_monthly = (t_max - t_min) / 2.0;

    let beta = (std::f64::consts::PI / (DOE2_GROUND_HOURS_PER_YEAR * DOE2_GROUND_DIFFUSIVITY))
        .sqrt()
        * DOE2_GROUND_DEPTH_FACTOR;
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
    result
}

fn parse_meta_f64(raw: &str, name: &str) -> Result<f64, WeatherError> {
    raw.trim()
        .parse::<f64>()
        .map_err(|_| WeatherError::Parse(format!("PSM3 metadata: failed to parse `{name}`: `{}`", raw.trim())))
}

fn parse_data_f64(
    fields: &[&str],
    idx: usize,
    row: usize,
    name: &str,
) -> Result<f64, WeatherError> {
    let raw = fields.get(idx).ok_or_else(|| {
        WeatherError::Parse(format!("row {row}: missing field `{name}` at index {idx}"))
    })?;
    raw.trim()
        .parse::<f64>()
        .map_err(|_| WeatherError::Parse(format!("row {row}: failed to parse `{name}`: `{}`", raw.trim())))
}

fn parse_data_u32(
    fields: &[&str],
    idx: usize,
    row: usize,
    name: &str,
) -> Result<u32, WeatherError> {
    let raw = fields.get(idx).ok_or_else(|| {
        WeatherError::Parse(format!("row {row}: missing field `{name}` at index {idx}"))
    })?;
    raw.trim()
        .parse::<u32>()
        .map_err(|_| WeatherError::Parse(format!("row {row}: failed to parse `{name}` as u32: `{}`", raw.trim())))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal PSM3 CSV string for testing.
    fn make_psm3_csv(step_minutes: u32, is_leap: bool) -> String {
        let day_counts = monthly_day_counts(is_leap);
        let total_records: usize = day_counts.iter().sum::<usize>() * 24 * (60 / step_minutes as usize);

        let mut lines = Vec::with_capacity(total_records + 2);

        // Line 1: metadata header
        lines.push(
            "Source,Location ID,TestCity,State,Country,39.74,-104.99,-7,1609.0,Local TZ"
                .to_string(),
        );

        // Line 2: column names
        lines.push(
            "Year,Month,Day,Hour,Minute,GHI,DNI,DHI,Temperature,Pressure,Dew Point,Relative Humidity,Wind Speed,Wind Direction"
                .to_string(),
        );

        let year: i32 = if is_leap { 2020 } else { 2021 };
        for (mi, &days) in day_counts.iter().enumerate() {
            let month = mi as u32 + 1;
            for day in 1..=days as u32 {
                for hour in 0..24u32 {
                    for minute_slot in 0..(60 / step_minutes) {
                        let minute = minute_slot * step_minutes;
                        // Use mbar pressure (1013.25 mbar = 101.325 kPa)
                        lines.push(format!(
                            "{year},{month},{day},{hour},{minute},\
                             100,200,50,20.0,1013.25,10.0,50.0,3.0,180"
                        ));
                    }
                }
            }
        }

        assert_eq!(lines.len() - 2, total_records);
        lines.join("\n")
    }

    #[test]
    fn parse_hourly_psm3() {
        let csv = make_psm3_csv(60, false);
        let ts = parse_psm3_str(&csv).expect("should parse hourly PSM3");
        assert_eq!(ts.len(), 8760);
        assert_eq!(ts.meta.source_step_secs, 3600);
        assert!((ts.meta.latitude - 39.74).abs() < 1e-6);
        assert!((ts.meta.longitude - (-104.99)).abs() < 1e-6);
        assert_eq!(ts.meta.location, "TestCity");
        // Pressure: 1013.25 mbar / 10 = 101.325 kPa
        assert!((ts.pressure_kpa[0] - 101.325).abs() < 1e-6);
    }

    #[test]
    fn parse_5min_psm3() {
        let csv = make_psm3_csv(5, false);
        let ts = parse_psm3_str(&csv).expect("should parse 5-min PSM3");
        assert_eq!(ts.len(), 105_120);
        assert_eq!(ts.meta.source_step_secs, 300);
    }

    #[test]
    fn parse_5min_psm3_leap() {
        let csv = make_psm3_csv(5, true);
        let ts = parse_psm3_str(&csv).expect("should parse 5-min PSM3 leap year");
        assert_eq!(ts.len(), 105_408);
        assert_eq!(ts.meta.source_step_secs, 300);
    }

    #[test]
    fn psm3_sky_temp_uses_clark_allen() {
        let csv = make_psm3_csv(60, false);
        let ts = parse_psm3_str(&csv).expect("should parse");
        // Clark-Allen: sky_k = dry_k * (0.787 + 0.764 * ln(dew_k / 273.15))^0.25
        let expected = clark_allen_sky_temp_c(20.0, 10.0);
        assert!(
            (ts.sky_temp_c[0] - expected).abs() < 1e-6,
            "sky temp should match Clark-Allen: got {}, expected {expected}",
            ts.sky_temp_c[0]
        );
    }

    #[test]
    fn psm3_rejects_bad_pressure() {
        // Pressure of 10 mbar → 1 kPa, below [60, 110] range.
        let csv = "Source,ID,City,State,Country,39.74,-104.99,-7,1609.0,LTZ\n\
                   Year,Month,Day,Hour,Minute,GHI,DNI,DHI,Temperature,Pressure,Dew Point,Relative Humidity,Wind Speed,Wind Direction\n\
                   2021,1,1,0,0,100,200,50,20.0,10,10.0,50.0,3.0,180\n\
                   2021,1,1,1,0,100,200,50,20.0,10,10.0,50.0,3.0,180";
        let err = parse_psm3_str(csv).expect_err("should reject bad pressure");
        assert!(err.to_string().contains("pressure out of range"));
    }

    #[test]
    fn psm3_rejects_dew_exceeding_dry() {
        let csv = "Source,ID,City,State,Country,39.74,-104.99,-7,1609.0,LTZ\n\
                   Year,Month,Day,Hour,Minute,GHI,DNI,DHI,Temperature,Pressure,Dew Point,Relative Humidity,Wind Speed,Wind Direction\n\
                   2021,1,1,0,0,100,200,50,20.0,1013.25,25.0,50.0,3.0,180\n\
                   2021,1,1,1,0,100,200,50,20.0,1013.25,25.0,50.0,3.0,180";
        let err = parse_psm3_str(csv).expect_err("should reject dew > dry");
        assert!(err.to_string().contains("dew point"));
    }

    #[test]
    fn psm3_rejects_invalid_timestep() {
        // Two rows 7 minutes apart.
        let csv = "Source,ID,City,State,Country,39.74,-104.99,-7,1609.0,LTZ\n\
                   Year,Month,Day,Hour,Minute,GHI,DNI,DHI,Temperature,Pressure,Dew Point,Relative Humidity,Wind Speed,Wind Direction\n\
                   2021,1,1,0,0,100,200,50,20.0,1013.25,10.0,50.0,3.0,180\n\
                   2021,1,1,0,7,100,200,50,20.0,1013.25,10.0,50.0,3.0,180";
        let err = parse_psm3_str(csv).expect_err("should reject 7-min step");
        assert!(err.to_string().contains("not one of"));
    }

    #[test]
    fn psm3_downsample_to_hourly() {
        let csv = make_psm3_csv(15, false);
        let ts = parse_psm3_str(&csv).expect("should parse 15-min PSM3");
        assert_eq!(ts.len(), 35_040);
        assert_eq!(ts.meta.source_step_secs, 900);

        let hourly = ts.resample(3600).expect("should downsample to hourly");
        assert_eq!(hourly.len(), 8760);
        assert_eq!(hourly.meta.source_step_secs, 3600);

        // All temperature values are 20.0, so mean should be 20.0.
        assert!((hourly.dry_bulb_c[0] - 20.0).abs() < 1e-12);
        // GHI mean of 4x 100 = 100.
        assert!((hourly.ghi_w_m2[0] - 100.0).abs() < 1e-12);
    }
}
