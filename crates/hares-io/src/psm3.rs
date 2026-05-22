//! PSM3/NSRDB CSV file parser (SAM CSV format).
//!
//! PSM3 files are produced by NREL's National Solar Radiation Database (NSRDB)
//! Physical Solar Model v3. Format: 3-line header (field names, field values,
//! column names), then data rows at 5/15/30/60-minute intervals.
//!
//! Reference: <https://developer.nrel.gov/docs/solar/nsrdb/psm3-download/>

use std::fs;
use std::path::Path;

use chrono::{Datelike, NaiveDate};

#[cfg(test)]
use crate::epw::monthly_day_counts;
use crate::epw::{
    clark_allen_sky_temp_c, doe2_ground_temp_from_monthly_avg, doe2_ground_temp_monthly,
    interpolate_ground_temp_c,
};
use crate::weather::{WeatherError, WeatherMeta, WeatherTimeSeries};

/// Valid PSM3 timestep intervals in seconds.
const VALID_STEP_SECS: [u32; 4] = [300, 900, 1800, 3600];

/// Expected record counts for non-leap year at each supported interval.
const RECORDS_PER_YEAR_STANDARD: [usize; 4] = [105_120, 35_040, 17_520, 8_760];
/// Expected record counts for leap year at each supported interval.
const RECORDS_PER_YEAR_LEAP: [usize; 4] = [105_408, 35_136, 17_568, 8_784];

/// Parse a PSM3/NSRDB CSV file (SAM CSV format) into a [`WeatherTimeSeries`].
///
/// PSM3 files are produced by NREL's National Solar Radiation Database (NSRDB)
/// Physical Solar Model v3. Format: 3-line header (field names, field values,
/// column names), then data rows at 5/15/30/60-minute intervals.
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

    // --- Line 1: field names ---
    let names_line = lines
        .next()
        .ok_or_else(|| WeatherError::Parse("missing PSM3 field names header (line 1)".into()))?;
    // Strip UTF-8 BOM if present (Windows/Excel exports may prepend \xEF\xBB\xBF).
    let names_line = names_line.trim_start_matches('\u{FEFF}');
    let field_names: Vec<&str> = names_line.split(',').map(str::trim).collect();

    // --- Line 2: field values ---
    let values_line = lines
        .next()
        .ok_or_else(|| WeatherError::Parse("missing PSM3 field values header (line 2)".into()))?;
    let field_values: Vec<&str> = values_line.split(',').collect();

    if field_names.len() != field_values.len() {
        return Err(WeatherError::Parse(format!(
            "PSM3 header field count mismatch: {} names vs {} values",
            field_names.len(),
            field_values.len()
        )));
    }

    // Zip into name→value map for named lookup.
    let meta_map: std::collections::HashMap<&str, &str> = field_names
        .iter()
        .copied()
        .zip(field_values.iter().copied())
        .collect();

    let lookup_meta = |name: &str| -> Result<&str, WeatherError> {
        meta_map
            .get(name)
            .copied()
            .ok_or_else(|| WeatherError::Parse(format!("PSM3 metadata missing field: {name}")))
    };

    let latitude = parse_meta_f64(lookup_meta("Latitude")?, "Latitude")?;
    let longitude = parse_meta_f64(lookup_meta("Longitude")?, "Longitude")?;
    let timezone_offset_h = parse_meta_f64(lookup_meta("Time Zone")?, "Time Zone")?;
    let elevation_m = parse_meta_f64(lookup_meta("Elevation")?, "Elevation")?;

    // Location label: use City if available and non-empty/non-placeholder, else "PSM3".
    let location = lookup_meta("City")
        .ok()
        .map(str::trim)
        .filter(|c| !c.is_empty() && *c != "-")
        .map_or_else(|| "PSM3".to_string(), str::to_string);

    // --- Line 3: column names ---
    let header_line = lines
        .next()
        .ok_or_else(|| WeatherError::Parse("missing PSM3 column header (line 3)".into()))?;
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
    let step_secs = detect_timestep(data_lines[0], data_lines[1], &col_map)?;

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
    let has_albedo_col = col_map.surface_albedo.is_some();
    let mut surface_albedo = if has_albedo_col {
        Some(Vec::with_capacity(n))
    } else {
        None
    };
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
        if !(0.0..=100.0).contains(&rh) {
            return Err(WeatherError::Validation(format!(
                "row {row}: relative humidity out of range [0, 100] %: {rh}"
            )));
        }

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
        if !(0.0..=1100.0).contains(&dni) {
            return Err(WeatherError::Validation(format!(
                "row {row}: DNI out of range [0, 1100] W/m^2: {dni}"
            )));
        }
        let dhi = parse_data_f64(&fields, col_map.dhi, row, "DHI")?;
        if !(0.0..=800.0).contains(&dhi) {
            return Err(WeatherError::Validation(format!(
                "row {row}: DHI out of range [0, 800] W/m^2: {dhi}"
            )));
        }

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

        if let Some(idx) = col_map.surface_albedo {
            let val = parse_data_f64(&fields, idx, row, "Surface Albedo")?;
            if !(0.0..=1.0).contains(&val) {
                tracing::warn!("row {row}: Surface Albedo {val} outside [0, 1], clamped");
            }
            surface_albedo
                .as_mut()
                .expect("pre-allocated")
                .push(val.clamp(0.0, 1.0));
        }

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
    let step_idx = VALID_STEP_SECS
        .iter()
        .position(|&s| s == step_secs)
        .expect("validated step");
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
    let monthly_ground_temps = compute_monthly_means_sub_hourly(&dry_bulb_c, &timestamps)
        .map(|monthly_avg| doe2_ground_temp_from_monthly_avg(&monthly_avg))
        .unwrap_or_else(|| doe2_ground_temp_monthly(&dry_bulb_c, is_leap_year));

    let mut ground_temp_c = Vec::with_capacity(n);
    for &(month, day, hour) in &timestamps {
        ground_temp_c.push(interpolate_ground_temp_c(
            &monthly_ground_temps,
            month,
            day,
            // PSM3 uses 0-23 hours (start-of-interval); interpolate_ground_temp_c
            // expects 1-24 (EPW end-of-interval convention). Add 1 to convert,
            // clamped to [1, 24].
            (hour + 1).clamp(1, 24),
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
        midpoint_offset_secs: 0,
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
        surface_albedo,
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
    /// Optional: not all PSM3 files include Surface Albedo.
    surface_albedo: Option<usize>,
}

fn build_column_map(col_names: &[&str]) -> Result<Psm3ColumnMap, WeatherError> {
    let find = |name: &str| -> Result<usize, WeatherError> {
        col_names
            .iter()
            .position(|c| c.eq_ignore_ascii_case(name))
            .ok_or_else(|| WeatherError::Parse(format!("PSM3 missing required column: {name}")))
    };

    // Surface Albedo is optional -- not all PSM3 files include it.
    let surface_albedo = col_names
        .iter()
        .position(|c| c.eq_ignore_ascii_case("Surface Albedo"));

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
        surface_albedo,
    })
}

fn detect_timestep(row1: &str, row2: &str, col_map: &Psm3ColumnMap) -> Result<u32, WeatherError> {
    let fields1: Vec<&str> = row1.split(',').collect();
    let fields2: Vec<&str> = row2.split(',').collect();

    let ts1 = row_to_epoch_secs(&fields1, col_map, 1)?;
    let ts2 = row_to_epoch_secs(&fields2, col_map, 2)?;

    let diff = ts2.checked_sub(ts1).ok_or_else(|| {
        WeatherError::Parse("PSM3 timestamps not monotonically increasing".into())
    })?;

    let step = u32::try_from(diff)
        .map_err(|_| WeatherError::Parse(format!("PSM3 timestep overflow: {diff} seconds")))?;

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

    let day_of_year = u64::from(date.ordinal());

    Ok((day_of_year - 1) * 86400 + u64::from(hour) * 3600 + u64::from(minute) * 60)
}

fn validate_record_count(n: usize, step_secs: u32) -> Result<(), WeatherError> {
    let step_idx = VALID_STEP_SECS
        .iter()
        .position(|&s| s == step_secs)
        .ok_or_else(|| WeatherError::Validation(format!("unexpected timestep: {step_secs}")))?;

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

fn parse_meta_f64(raw: &str, name: &str) -> Result<f64, WeatherError> {
    raw.trim().parse::<f64>().map_err(|_| {
        WeatherError::Parse(format!(
            "PSM3 metadata: failed to parse `{name}`: `{}`",
            raw.trim()
        ))
    })
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
    raw.trim().parse::<f64>().map_err(|_| {
        WeatherError::Parse(format!(
            "row {row}: failed to parse `{name}`: `{}`",
            raw.trim()
        ))
    })
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
    raw.trim().parse::<u32>().map_err(|_| {
        WeatherError::Parse(format!(
            "row {row}: failed to parse `{name}` as u32: `{}`",
            raw.trim()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal PSM3 CSV string for testing.
    fn make_psm3_csv(step_minutes: u32, is_leap: bool) -> String {
        let day_counts = monthly_day_counts(is_leap);
        let total_records: usize =
            day_counts.iter().sum::<usize>() * 24 * (60 / step_minutes as usize);

        let mut lines = Vec::with_capacity(total_records + 3);

        // Line 1: field names
        lines.push(
            "Source,Location ID,City,State,Country,Latitude,Longitude,Time Zone,Elevation,Local Time Zone"
                .to_string(),
        );

        // Line 2: field values
        lines.push("NSRDB,12345,TestCity,-,-,39.74,-104.99,-7,1609.0,-7".to_string());

        // Line 3: column names
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

        assert_eq!(lines.len() - 3, total_records);
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
        let csv = "Source,Location ID,City,State,Country,Latitude,Longitude,Time Zone,Elevation,Local Time Zone\n\
                   NSRDB,1,City,-,-,39.74,-104.99,-7,1609.0,-7\n\
                   Year,Month,Day,Hour,Minute,GHI,DNI,DHI,Temperature,Pressure,Dew Point,Relative Humidity,Wind Speed,Wind Direction\n\
                   2021,1,1,0,0,100,200,50,20.0,10,10.0,50.0,3.0,180\n\
                   2021,1,1,1,0,100,200,50,20.0,10,10.0,50.0,3.0,180";
        let err = parse_psm3_str(csv).expect_err("should reject bad pressure");
        assert!(err.to_string().contains("pressure out of range"));
    }

    #[test]
    fn psm3_rejects_dew_exceeding_dry() {
        let csv = "Source,Location ID,City,State,Country,Latitude,Longitude,Time Zone,Elevation,Local Time Zone\n\
                   NSRDB,1,City,-,-,39.74,-104.99,-7,1609.0,-7\n\
                   Year,Month,Day,Hour,Minute,GHI,DNI,DHI,Temperature,Pressure,Dew Point,Relative Humidity,Wind Speed,Wind Direction\n\
                   2021,1,1,0,0,100,200,50,20.0,1013.25,25.0,50.0,3.0,180\n\
                   2021,1,1,1,0,100,200,50,20.0,1013.25,25.0,50.0,3.0,180";
        let err = parse_psm3_str(csv).expect_err("should reject dew > dry");
        assert!(err.to_string().contains("dew point"));
    }

    #[test]
    fn psm3_rejects_invalid_timestep() {
        // Two rows 7 minutes apart.
        let csv = "Source,Location ID,City,State,Country,Latitude,Longitude,Time Zone,Elevation,Local Time Zone\n\
                   NSRDB,1,City,-,-,39.74,-104.99,-7,1609.0,-7\n\
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

    #[test]
    fn psm3_default_albedo_when_column_absent() {
        let csv = make_psm3_csv(60, false);
        let ts = parse_psm3_str(&csv).expect("should parse");
        assert!(
            ts.surface_albedo.is_none(),
            "surface_albedo should be None when Surface Albedo column is absent"
        );
    }

    #[test]
    fn psm3_parses_surface_albedo_column() {
        // Build a minimal PSM3 CSV with a Surface Albedo column.
        let day_counts = monthly_day_counts(false);
        let total_records: usize = day_counts.iter().sum::<usize>() * 24;

        let mut lines = Vec::with_capacity(total_records + 3);
        lines.push(
            "Source,Location ID,City,State,Country,Latitude,Longitude,Time Zone,Elevation,Local Time Zone"
                .to_string(),
        );
        lines.push("NSRDB,12345,TestCity,-,-,39.74,-104.99,-7,1609.0,-7".to_string());
        lines.push(
            "Year,Month,Day,Hour,Minute,GHI,DNI,DHI,Temperature,Pressure,Dew Point,Relative Humidity,Wind Speed,Wind Direction,Surface Albedo"
                .to_string(),
        );

        for (mi, &days) in day_counts.iter().enumerate() {
            let month = mi as u32 + 1;
            let albedo = if month <= 3 || month >= 11 { 0.7 } else { 0.15 };
            for day in 1..=days as u32 {
                for hour in 0..24u32 {
                    lines.push(format!(
                        "2021,{month},{day},{hour},0,\
                         100,200,50,20.0,1013.25,10.0,50.0,3.0,180,{albedo}"
                    ));
                }
            }
        }

        let csv = lines.join("\n");
        let ts = parse_psm3_str(&csv).expect("should parse PSM3 with albedo");
        let albedo = ts
            .surface_albedo
            .as_ref()
            .expect("should be Some when column present");
        assert_eq!(albedo.len(), ts.len());

        // January row should have snow albedo (0.7).
        assert!(
            (albedo[0] - 0.7).abs() < 1e-12,
            "January albedo should be 0.7, got {}",
            albedo[0]
        );
        // June row (hour 0 of June 1 = 151 * 24 = row 3624 for non-leap).
        let june_idx = (31 + 28 + 31 + 30 + 31) * 24; // May end
        assert!(
            (albedo[june_idx] - 0.15).abs() < 1e-12,
            "June albedo should be 0.15, got {}",
            albedo[june_idx]
        );
    }

    // ── Regression tests for ticket 025 ─────────────────────────────────────

    /// When IR = 0.0 (no column present), sky temp from compute_sky_temp_c must
    /// be numerically identical to the current clark_allen_sky_temp_c result.
    ///
    /// This test currently PASSES because both code-paths produce the same
    /// number.  It is here to guard that the fix (routing through
    /// compute_sky_temp_c) does not change the value when IR is absent.
    #[test]
    fn psm3_sky_temp_zero_ir_matches_clark_allen() {
        use crate::epw::compute_sky_temp_c;
        let csv = make_psm3_csv(60, false);
        let ts = parse_psm3_str(&csv).expect("should parse");
        let via_clark_allen = clark_allen_sky_temp_c(20.0, 10.0);
        let via_compute = compute_sky_temp_c(0.0, 20.0, 10.0, 0.0);
        assert!(
            (via_clark_allen - via_compute).abs() < 1e-12,
            "clark_allen and compute_sky_temp_c(0.0,…,0.0) must be identical: \
             clark_allen={via_clark_allen}, compute={via_compute}"
        );
        // The parsed sky_temp_c should equal clark_allen today; after the fix
        // it must still equal this value.
        assert!(
            (ts.sky_temp_c[0] - via_clark_allen).abs() < 1e-6,
            "psm3 sky_temp_c should equal clark_allen result: \
             got {}, expected {via_clark_allen}",
            ts.sky_temp_c[0]
        );
    }

    /// A PSM3 file with a synthetic `Lwdown` column must produce sky
    /// temperatures derived from the Stefan-Boltzmann inversion, not
    /// Clark-Allen.
    ///
    /// This test FAILS on current code (the parser ignores the Lwdown column
    /// and always calls clark_allen_sky_temp_c).  It will pass once the fix
    /// from ticket 025 is applied.
    #[test]
    fn psm3_lwdown_column_activates_stefan_boltzmann_path() {
        use crate::epw::compute_sky_temp_c;

        let day_counts = monthly_day_counts(false);
        let total_records: usize = day_counts.iter().sum::<usize>() * 24;
        let mut lines = Vec::with_capacity(total_records + 3);

        lines.push(
            "Source,Location ID,City,State,Country,Latitude,Longitude,Time Zone,Elevation,Local Time Zone"
                .to_string(),
        );
        lines.push("NSRDB,12345,TestCity,-,-,39.74,-104.99,-7,1609.0,-7".to_string());
        // Include the optional Lwdown column.
        lines.push(
            "Year,Month,Day,Hour,Minute,GHI,DNI,DHI,Temperature,Pressure,Dew Point,Relative Humidity,Wind Speed,Wind Direction,Lwdown"
                .to_string(),
        );

        // Use a plausible downwelling IR value (300 W/m²) that exceeds the
        // 50 W/m² Stefan-Boltzmann threshold.
        let lwdown = 300.0_f64;
        for (mi, &days) in day_counts.iter().enumerate() {
            let month = mi as u32 + 1;
            for day in 1..=days as u32 {
                for hour in 0..24u32 {
                    lines.push(format!(
                        "2021,{month},{day},{hour},0,\
                         100,200,50,20.0,1013.25,10.0,50.0,3.0,180,{lwdown}"
                    ));
                }
            }
        }

        let csv = lines.join("\n");
        // Currently this will succeed as a parse (no format error), but sky
        // temp will equal clark_allen result, not the Stefan-Boltzmann value.
        let ts = parse_psm3_str(&csv).expect("should parse PSM3 with Lwdown column");

        let stefan_boltzmann_expected = compute_sky_temp_c(lwdown, 20.0, 10.0, 0.0);
        let clark_allen_fallback = clark_allen_sky_temp_c(20.0, 10.0);

        // After the fix: sky_temp_c must equal the Stefan-Boltzmann inversion.
        assert!(
            (ts.sky_temp_c[0] - stefan_boltzmann_expected).abs() < 1e-6,
            "ticket-025: PSM3 with Lwdown column must use Stefan-Boltzmann inversion \
             (expected {stefan_boltzmann_expected:.4} °C) but got {:.4} °C \
             (Clark-Allen fallback would give {clark_allen_fallback:.4} °C)",
            ts.sky_temp_c[0]
        );

        // After the fix: horizontal_infrared_w_m2 must be populated from the column.
        assert!(
            (ts.horizontal_infrared_w_m2[0] - lwdown).abs() < 1e-6,
            "ticket-025: PSM3 Lwdown column must populate horizontal_infrared_w_m2 \
             (expected {lwdown}) but got {}",
            ts.horizontal_infrared_w_m2[0]
        );
    }
}
