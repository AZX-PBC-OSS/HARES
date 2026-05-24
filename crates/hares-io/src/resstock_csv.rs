//! ResStock simplified CSV weather file parser.
//!
//! ResStock weather files (AMY 2018, etc.) contain 8 columns:
//! `date_time`, dry bulb, RH, wind speed, wind direction, GHI, DNI, DHI.
//!
//! Several fields required by HARES are **missing** and must be estimated:
//!
//! | Field               | Source                                                  |
//! |---------------------|---------------------------------------------------------|
//! | Dry bulb [°C]       | **Measured** -- CSV column                               |
//! | Relative humidity [%]| **Measured** -- CSV column                              |
//! | Wind speed [m/s]    | **Measured** -- CSV column                               |
//! | Wind direction [°]  | **Measured** -- CSV column                               |
//! | GHI [W/m²]          | **Measured** -- CSV column                               |
//! | DNI [W/m²]          | **Measured** -- CSV column                               |
//! | DHI [W/m²]          | **Measured** -- CSV column                               |
//! | Pressure [kPa]      | **Estimated** -- ISA standard atmosphere from elevation  |
//! | Dew point [°C]      | **Estimated** -- Magnus formula from dry bulb + RH       |
//! | Horizontal IR [W/m²]| **Placeholder** -- set to 0.0 (triggers Clark-Allen)     |
//! | Sky temperature [°C]| **Estimated** -- Clark-Allen from dry bulb + dew point   |
//! | Opaque sky cover    | **Placeholder** -- set to 0.0 (unavailable)              |
//! | Precipitation [m]   | **Placeholder** -- set to 0.0 (unavailable)              |
//! | Ground temp [°C]    | **Estimated** -- DOE-2 model from monthly dry-bulb avg   |

use std::path::Path;

use chrono::{Datelike, NaiveDateTime, Timelike};

use crate::epw::{
    compute_sky_temp_c, doe2_ground_temp_from_monthly_avg, interpolate_ground_temp_c,
    monthly_average_dry_bulb,
};
use crate::weather::{WeatherError, WeatherMeta, WeatherTimeSeries};

/// ISA standard atmosphere: pressure [kPa] at a given elevation [m].
///
/// Formula: `P = 101.325 * (1 - 2.25577e-5 * h)^5.25588`
///
/// Valid for elevations 0–11 000 m (troposphere). Returns NaN-safe minimum
/// of 1.0 kPa for elevations beyond the formula's validity range.
fn isa_pressure_kpa(elevation_m: f64) -> f64 {
    let base = 1.0 - 2.25577e-5 * elevation_m;
    if base <= 0.0 {
        return 1.0; // Above ~44 km -- return a safe floor
    }
    101.325 * base.powf(5.25588)
}

/// Magnus formula dew point [°C] from dry-bulb [°C] and relative humidity [%].
///
/// `alpha = ln(RH/100) + (17.67 * T_db) / (243.5 + T_db)`
/// `T_dp  = (243.5 * alpha) / (17.67 - alpha)`
///
/// August-Roche-Magnus approximation. Coefficients b=17.67, c=243.5°C are from
/// Alduchov & Eskridge (1996) but widely attributed to the Magnus/Tetens family.
/// Valid range: -40°C to +50°C dry bulb.
fn magnus_dew_point_c(dry_bulb_c: f64, rh_pct: f64) -> f64 {
    let rh_frac = (rh_pct / 100.0).clamp(0.001, 1.0);
    let alpha = rh_frac.ln() + (17.67 * dry_bulb_c) / (243.5 + dry_bulb_c);
    (243.5 * alpha) / (17.67 - alpha)
}

/// Parse a ResStock simplified 8-column CSV weather file from a path.
///
/// The `elevation_m` parameter is needed to estimate atmospheric pressure
/// via the ISA standard atmosphere model since ResStock CSVs do not include
/// pressure data.
///
/// `latitude`, `longitude`, and `timezone_offset_h` are passed through to
/// [`WeatherMeta`] for downstream solar calculations. When unknown, pass 0.0
/// (but note this will place the site on the equator/prime meridian).
///
/// See module-level docs for which fields are measured vs estimated.
pub fn parse_resstock_csv(
    path: impl AsRef<Path>,
    elevation_m: f64,
    latitude: f64,
    longitude: f64,
    timezone_offset_h: f64,
) -> Result<WeatherTimeSeries, WeatherError> {
    let path_ref = path.as_ref();
    let contents = std::fs::read_to_string(path_ref).map_err(|source| WeatherError::Io {
        path: path_ref.display().to_string(),
        source,
    })?;
    parse_resstock_csv_str(
        &contents,
        elevation_m,
        latitude,
        longitude,
        timezone_offset_h,
    )
}

/// Parse a ResStock CSV from an in-memory string (useful for testing).
pub fn parse_resstock_csv_str(
    contents: &str,
    elevation_m: f64,
    latitude: f64,
    longitude: f64,
    timezone_offset_h: f64,
) -> Result<WeatherTimeSeries, WeatherError> {
    let mut lines = contents.lines();

    // Parse header row to find column indices.
    let header_line = lines
        .next()
        .ok_or_else(|| WeatherError::Parse("ResStock CSV is empty".to_string()))?;

    let header_line = header_line.trim_start_matches('\u{FEFF}');
    let columns: Vec<&str> = header_line.split(',').map(str::trim).collect();
    let col_indices = resolve_columns(&columns)?;

    // Constant pressure from elevation.
    let pressure_kpa = isa_pressure_kpa(elevation_m);

    // First pass: parse all data rows.
    struct RawRow {
        month: u32,
        day: u32,
        hour: u32,
        dry_bulb_c: f64,
        rh_pct: f64,
        wind_speed_m_s: f64,
        wind_dir_deg: f64,
        ghi_w_m2: f64,
        dni_w_m2: f64,
        dhi_w_m2: f64,
    }

    let mut raw_rows: Vec<RawRow> = Vec::new();

    for (line_idx, line) in lines.enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let row_num = line_idx + 2; // 1-indexed, header is row 1
        let fields: Vec<&str> = line.split(',').collect();

        let max_idx = *[
            col_indices.date_time,
            col_indices.dry_bulb,
            col_indices.rh,
            col_indices.wind_speed,
            col_indices.wind_dir,
            col_indices.ghi,
            col_indices.dni,
            col_indices.dhi,
        ]
        .iter()
        .max()
        .expect("non-empty array");

        if fields.len() <= max_idx {
            return Err(WeatherError::Parse(format!(
                "row {row_num}: expected at least {} columns, got {}",
                max_idx + 1,
                fields.len()
            )));
        }

        // Parse datetime to extract month/day/hour.
        let dt_str = fields[col_indices.date_time].trim();
        let dt = NaiveDateTime::parse_from_str(dt_str, "%Y-%m-%d %H:%M:%S").map_err(|e| {
            WeatherError::Parse(format!(
                "row {row_num}: failed to parse date_time `{dt_str}`: {e}"
            ))
        })?;

        // ResStock uses end-of-interval timestamps (01:00 = hour 1).
        // EPW convention: hour 1..=24. ResStock 00:00 → hour 24 of prev day.
        let hour_0based = dt.hour();
        let (month, day, hour) = if hour_0based == 0 {
            let prev = dt.date() - chrono::Days::new(1);
            (prev.month(), prev.day(), 24u32)
        } else {
            (dt.month(), dt.day(), hour_0based)
        };

        let dry_bulb_c = parse_field(fields[col_indices.dry_bulb], row_num, "dry_bulb")?;
        if !(-60.0..=55.0).contains(&dry_bulb_c) {
            return Err(WeatherError::Validation(format!(
                "row {row_num}: dry bulb out of range [-60, 55] °C: {dry_bulb_c}"
            )));
        }

        let rh_pct = parse_field(fields[col_indices.rh], row_num, "relative_humidity")?;
        if !(0.0..=100.0).contains(&rh_pct) {
            return Err(WeatherError::Validation(format!(
                "row {row_num}: relative humidity out of range [0, 100] %: {rh_pct}"
            )));
        }

        let wind_speed_m_s = parse_field(fields[col_indices.wind_speed], row_num, "wind_speed")?;
        if !(0.0..=60.0).contains(&wind_speed_m_s) {
            return Err(WeatherError::Validation(format!(
                "row {row_num}: wind speed out of range [0, 60] m/s: {wind_speed_m_s}"
            )));
        }

        let wind_dir_deg = parse_field(fields[col_indices.wind_dir], row_num, "wind_direction")?;
        if !(0.0..=360.0).contains(&wind_dir_deg) {
            return Err(WeatherError::Validation(format!(
                "row {row_num}: wind direction out of range [0, 360] deg: {wind_dir_deg}"
            )));
        }

        let ghi_w_m2 = parse_field(fields[col_indices.ghi], row_num, "ghi")?;
        if ghi_w_m2 < 0.0 {
            return Err(WeatherError::Validation(format!(
                "row {row_num}: GHI must be >= 0, got {ghi_w_m2}"
            )));
        }

        let dni_w_m2 = parse_field(fields[col_indices.dni], row_num, "dni")?;
        if !(0.0..=1100.0).contains(&dni_w_m2) {
            return Err(WeatherError::Validation(format!(
                "row {row_num}: DNI out of range [0, 1100] W/m^2: {dni_w_m2}"
            )));
        }

        let dhi_w_m2 = parse_field(fields[col_indices.dhi], row_num, "dhi")?;
        if !(0.0..=800.0).contains(&dhi_w_m2) {
            return Err(WeatherError::Validation(format!(
                "row {row_num}: DHI out of range [0, 800] W/m^2: {dhi_w_m2}"
            )));
        }

        raw_rows.push(RawRow {
            month,
            day,
            hour,
            dry_bulb_c,
            rh_pct,
            wind_speed_m_s,
            wind_dir_deg,
            ghi_w_m2,
            dni_w_m2,
            dhi_w_m2,
        });
    }

    if raw_rows.is_empty() {
        return Err(WeatherError::Parse(
            "ResStock CSV contains no data rows".to_string(),
        ));
    }

    // Auto-detect timestep from first two rows.
    let source_step_secs = if raw_rows.len() >= 2 {
        // Infer from row count assuming a full year.
        let n = raw_rows.len();
        let total_seconds_in_year = 8760 * 3600;
        if total_seconds_in_year % n == 0 {
            (total_seconds_in_year / n) as u32
        } else {
            3600 // default to hourly
        }
    } else {
        3600
    };

    // Determine if this is a leap year from the first timestamp's year.
    let first_dt_str = contents
        .lines()
        .nth(1)
        .and_then(|line| line.split(',').next())
        .unwrap_or("");
    let is_leap_year = NaiveDateTime::parse_from_str(first_dt_str.trim(), "%Y-%m-%d %H:%M:%S")
        .map(|dt| {
            let y = dt.year();
            y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
        })
        .unwrap_or(false);

    // Compute monthly dry-bulb averages for ground temperature.
    let dry_bulb_all: Vec<f64> = raw_rows.iter().map(|r| r.dry_bulb_c).collect();
    let monthly_avg = monthly_average_dry_bulb(&dry_bulb_all, is_leap_year).unwrap_or_else(|| {
        let annual_avg = dry_bulb_all.iter().sum::<f64>() / dry_bulb_all.len() as f64;
        [annual_avg; 12]
    });
    let monthly_ground_temps = doe2_ground_temp_from_monthly_avg(&monthly_avg);

    // Build output vectors.
    let n = raw_rows.len();
    let mut dry_bulb_c = Vec::with_capacity(n);
    let mut dew_point_c = Vec::with_capacity(n);
    let mut rel_humidity_pct = Vec::with_capacity(n);
    let mut pressure_kpa_vec = Vec::with_capacity(n);
    let mut ghi_w_m2 = Vec::with_capacity(n);
    let mut dni_w_m2 = Vec::with_capacity(n);
    let mut dhi_w_m2 = Vec::with_capacity(n);
    let mut wind_speed_m_s = Vec::with_capacity(n);
    let mut wind_dir_deg = Vec::with_capacity(n);
    let mut opaque_sky_cover = Vec::with_capacity(n);
    let mut horizontal_infrared_w_m2 = Vec::with_capacity(n);
    let mut sky_temp_c = Vec::with_capacity(n);
    let mut ground_temp_c = Vec::with_capacity(n);
    let mut liquid_precip_m = Vec::with_capacity(n);

    for row in &raw_rows {
        let t_dp = magnus_dew_point_c(row.dry_bulb_c, row.rh_pct);
        let t_sky = compute_sky_temp_c(0.0, row.dry_bulb_c, t_dp, 0.0);
        let t_ground = interpolate_ground_temp_c(
            &monthly_ground_temps,
            row.month,
            row.day,
            row.hour,
            is_leap_year,
        )?;

        dry_bulb_c.push(row.dry_bulb_c);
        dew_point_c.push(t_dp);
        rel_humidity_pct.push(row.rh_pct);
        pressure_kpa_vec.push(pressure_kpa);
        ghi_w_m2.push(row.ghi_w_m2);
        dni_w_m2.push(row.dni_w_m2);
        dhi_w_m2.push(row.dhi_w_m2);
        wind_speed_m_s.push(row.wind_speed_m_s);
        wind_dir_deg.push(row.wind_dir_deg);
        opaque_sky_cover.push(0.0);
        horizontal_infrared_w_m2.push(0.0);
        sky_temp_c.push(t_sky);
        ground_temp_c.push(t_ground);
        liquid_precip_m.push(0.0);
    }

    let meta = WeatherMeta {
        location: String::new(),
        latitude,
        longitude,
        timezone_offset_h,
        elevation_m,
        source_step_secs,
        midpoint_offset_secs: 0,
    };

    Ok(WeatherTimeSeries {
        meta,
        dry_bulb_c,
        dew_point_c,
        rel_humidity_pct,
        pressure_kpa: pressure_kpa_vec,
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
    })
}

/// Column index mapping for ResStock CSV headers.
struct ColumnIndices {
    date_time: usize,
    dry_bulb: usize,
    rh: usize,
    wind_speed: usize,
    wind_dir: usize,
    ghi: usize,
    dni: usize,
    dhi: usize,
}

/// Resolve column indices by case-insensitive partial matching.
fn resolve_columns(headers: &[&str]) -> Result<ColumnIndices, WeatherError> {
    let find = |pattern: &str| -> Result<usize, WeatherError> {
        let pattern_lower = pattern.to_ascii_lowercase();
        headers
            .iter()
            .position(|h| h.to_ascii_lowercase().contains(&pattern_lower))
            .ok_or_else(|| {
                WeatherError::Parse(format!(
                    "ResStock CSV missing required column matching `{pattern}`"
                ))
            })
    };

    Ok(ColumnIndices {
        date_time: find("date_time")?,
        dry_bulb: find("dry bulb temperature")?,
        rh: find("relative humidity")?,
        wind_speed: find("wind speed")?,
        wind_dir: find("wind direction")?,
        ghi: find("global horizontal radiation")?,
        dni: find("direct normal radiation")?,
        dhi: find("diffuse horizontal radiation")?,
    })
}

fn parse_field(raw: &str, row: usize, name: &str) -> Result<f64, WeatherError> {
    raw.trim().parse::<f64>().map_err(|_| {
        WeatherError::Parse(format!(
            "row {row}: failed to parse `{name}` as number: `{}`",
            raw.trim()
        ))
    })
}

/// Check if a CSV header line looks like a ResStock weather file.
///
/// Returns true if the first line contains both `Dry Bulb Temperature`
/// and `Global Horizontal Radiation` (case-insensitive).
pub(crate) fn is_resstock_csv_header(first_line: &str) -> bool {
    let lower = first_line.to_ascii_lowercase();
    lower.contains("dry bulb temperature") && lower.contains("global horizontal radiation")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epw::clark_allen_sky_temp_c;

    /// Build a synthetic ResStock CSV with the given number of hourly rows.
    ///
    /// Uses end-of-interval timestamps starting at `2005-01-01 01:00:00`,
    /// matching real ResStock CSV format.
    fn build_test_csv(rows: usize) -> String {
        use chrono::{NaiveDate, TimeDelta};

        let mut lines = Vec::with_capacity(rows + 1);
        lines.push(
            "date_time,Dry Bulb Temperature [°C],Relative Humidity [%],\
             Wind Speed [m/s],Wind Direction [Deg],\
             Global Horizontal Radiation [W/m2],\
             Direct Normal Radiation [W/m2],\
             Diffuse Horizontal Radiation [W/m2]"
                .to_string(),
        );

        let base = NaiveDate::from_ymd_opt(2005, 1, 1)
            .unwrap()
            .and_hms_opt(1, 0, 0)
            .unwrap();

        for i in 0..rows {
            let dt = base + TimeDelta::hours(i as i64);
            let hour_of_day = dt.format("%H").to_string().parse::<u32>().unwrap();
            let temp = 10.0 + 5.0 * (2.0 * std::f64::consts::PI * i as f64 / 24.0).sin();
            let rh = 60.0;
            let wind = 3.0;
            let wind_dir = 180.0;
            let ghi = if (7..=18).contains(&hour_of_day) {
                400.0
            } else {
                0.0
            };
            let dni = if (7..=18).contains(&hour_of_day) {
                600.0
            } else {
                0.0
            };
            let dhi = if (7..=18).contains(&hour_of_day) {
                100.0
            } else {
                0.0
            };

            lines.push(format!(
                "{},{temp:.1},{rh:.1},{wind:.1},{wind_dir:.1},\
                 {ghi:.1},{dni:.1},{dhi:.1}",
                dt.format("%Y-%m-%d %H:%M:%S")
            ));
        }

        lines.join("\n")
    }

    #[test]
    fn parse_basic_resstock_csv() {
        let csv = build_test_csv(8760);
        let result = parse_resstock_csv_str(&csv, 0.0, 39.7, -105.0, -7.0)
            .expect("should parse 8760-row CSV");

        assert_eq!(result.len(), 8760);
        assert_eq!(result.meta.source_step_secs, 3600);
        assert_eq!(result.meta.elevation_m, 0.0);

        // Pressure should be sea-level ISA.
        let p = result.pressure_kpa[0];
        assert!(
            (p - 101.325).abs() < 0.01,
            "sea-level pressure should be ~101.325 kPa, got {p}"
        );

        // All horizontal infrared should be 0.0 (placeholder).
        assert!(
            result.horizontal_infrared_w_m2.iter().all(|&v| v == 0.0),
            "horizontal IR should be 0.0 placeholder"
        );

        // All opaque sky cover should be 0.0 (placeholder).
        assert!(
            result.opaque_sky_cover.iter().all(|&v| v == 0.0),
            "opaque sky cover should be 0.0 placeholder"
        );

        // All precipitation should be 0.0 (placeholder).
        assert!(
            result.liquid_precip_m.iter().all(|&v| v == 0.0),
            "precipitation should be 0.0 placeholder"
        );

        // Dew point should be <= dry bulb for all rows.
        for (i, (&dp, &db)) in result
            .dew_point_c
            .iter()
            .zip(result.dry_bulb_c.iter())
            .enumerate()
        {
            assert!(
                dp <= db + 0.01,
                "row {i}: dew point {dp} must be <= dry bulb {db}"
            );
        }

        // Sky temp should be finite and below dry bulb.
        for (i, &t_sky) in result.sky_temp_c.iter().enumerate() {
            assert!(t_sky.is_finite(), "row {i}: sky temp must be finite");
        }

        // Ground temp should be finite.
        for (i, &t_g) in result.ground_temp_c.iter().enumerate() {
            assert!(t_g.is_finite(), "row {i}: ground temp must be finite");
        }
    }

    #[test]
    fn pressure_from_elevation() {
        // Sea level: 101.325 kPa
        let p_sea = isa_pressure_kpa(0.0);
        assert!(
            (p_sea - 101.325).abs() < 0.001,
            "sea level: got {p_sea}, expected 101.325"
        );

        // Denver, CO ~1609 m: ~83.4 kPa
        let p_denver = isa_pressure_kpa(1609.0);
        assert!(
            (p_denver - 83.4).abs() < 0.5,
            "Denver 1609m: got {p_denver}, expected ~83.4"
        );

        // Mexico City ~2250 m: ~77.0 kPa
        let p_mexico = isa_pressure_kpa(2250.0);
        assert!(
            (p_mexico - 77.0).abs() < 1.0,
            "Mexico City 2250m: got {p_mexico}, expected ~77.0"
        );
    }

    #[test]
    fn dew_point_estimation() {
        // At 100% RH, dew point should equal dry bulb.
        let dp_100 = magnus_dew_point_c(20.0, 100.0);
        assert!(
            (dp_100 - 20.0).abs() < 0.1,
            "at 100% RH, dew point should ≈ dry bulb: got {dp_100}"
        );

        // At 20°C, 50% RH: dew point ≈ 9.3°C (standard psychrometric tables).
        let dp_50 = magnus_dew_point_c(20.0, 50.0);
        assert!(
            (dp_50 - 9.3).abs() < 0.5,
            "at 20°C/50% RH, dew point ≈ 9.3°C: got {dp_50}"
        );

        // At 30°C, 30% RH: dew point ≈ 10.5°C.
        let dp_30 = magnus_dew_point_c(30.0, 30.0);
        assert!(
            (dp_30 - 10.5).abs() < 1.0,
            "at 30°C/30% RH, dew point ≈ 10.5°C: got {dp_30}"
        );

        // Dew point should always be <= dry bulb.
        for t in [-10.0, 0.0, 10.0, 20.0, 30.0, 40.0] {
            for rh in [10.0, 30.0, 50.0, 70.0, 90.0, 100.0] {
                let dp = magnus_dew_point_c(t, rh);
                assert!(
                    dp <= t + 0.1,
                    "dew point {dp} must be <= dry bulb {t} at RH={rh}%"
                );
            }
        }
    }

    #[test]
    fn sky_temp_uses_clark_allen() {
        // Verify the parser produces sky temps consistent with Clark-Allen.
        let csv = build_test_csv(8760);
        let result = parse_resstock_csv_str(&csv, 0.0, 39.7, -105.0, -7.0).expect("should parse");

        for i in 0..result.len() {
            let t_db = result.dry_bulb_c[i];
            let t_dp = result.dew_point_c[i];
            let expected = clark_allen_sky_temp_c(t_db, t_dp);
            assert!(
                (result.sky_temp_c[i] - expected).abs() < 0.01,
                "row {i}: sky temp {}, expected Clark-Allen {}",
                result.sky_temp_c[i],
                expected
            );
        }
    }

    #[test]
    fn rejects_bad_temperature() {
        let mut csv = build_test_csv(24);
        // Replace the second data row with an out-of-range temperature.
        let lines: Vec<&str> = csv.lines().collect();
        let mut new_lines: Vec<String> = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if i == 2 {
                // Row 2 (0-indexed line 2, data row 2) -- inject 60°C.
                let fields: Vec<&str> = line.split(',').collect();
                let mut fields: Vec<String> = fields.iter().map(|s| s.to_string()).collect();
                fields[1] = "60.0".to_string();
                new_lines.push(fields.join(","));
            } else {
                new_lines.push(line.to_string());
            }
        }
        csv = new_lines.join("\n");

        let err = parse_resstock_csv_str(&csv, 0.0, 39.7, -105.0, -7.0)
            .expect_err("should reject bad temperature");
        assert!(
            matches!(err, WeatherError::Validation(_)),
            "expected Validation error, got {err:?}"
        );
        assert!(
            err.to_string().contains("dry bulb"),
            "error should mention dry bulb: {err}"
        );
    }

    #[test]
    fn detect_resstock_csv_format() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("weather.csv");
        let csv = build_test_csv(24);
        std::fs::write(&path, &csv).expect("write temp file");

        let fmt = crate::weather::detect_weather_format(&path).expect("should detect ResStock CSV");
        assert_eq!(fmt, crate::weather::WeatherFormat::ResStockCsv);
    }

    #[test]
    fn is_resstock_header_positive() {
        let header = "date_time,Dry Bulb Temperature [°C],Relative Humidity [%],\
                      Wind Speed [m/s],Wind Direction [Deg],\
                      Global Horizontal Radiation [W/m2],\
                      Direct Normal Radiation [W/m2],\
                      Diffuse Horizontal Radiation [W/m2]";
        assert!(is_resstock_csv_header(header));
    }

    #[test]
    fn is_resstock_header_negative() {
        let header = "Year,Month,Day,Hour,Minute,DHI,DNI,GHI";
        assert!(!is_resstock_csv_header(header));
    }

    #[test]
    fn midnight_timestamp_uses_previous_day() {
        // A single row at 00:00:00 should map to hour 24 of the previous day.
        let csv = "\
date_time,Dry Bulb Temperature [°C],Relative Humidity [%],\
Wind Speed [m/s],Wind Direction [Deg],\
Global Horizontal Radiation [W/m2],\
Direct Normal Radiation [W/m2],\
Diffuse Horizontal Radiation [W/m2]
2005-01-02 00:00:00,5.0,60.0,3.0,180.0,0.0,0.0,0.0
2005-01-02 01:00:00,5.0,60.0,3.0,180.0,0.0,0.0,0.0
2005-01-02 02:00:00,5.0,60.0,3.0,180.0,0.0,0.0,0.0";

        // The test just verifies parsing succeeds (midnight row doesn't
        // incorrectly reference month/day from the midnight timestamp).
        let result = parse_resstock_csv_str(csv, 0.0, 39.7, -105.0, -7.0)
            .expect("should parse CSV with midnight row");
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn dew_point_sub_zero_temperatures() {
        // Magnus formula should produce valid dew points at sub-zero temperatures.
        for t in [-40.0, -20.0, -10.0, -5.0] {
            for rh in [20.0, 50.0, 80.0, 100.0] {
                let dp = magnus_dew_point_c(t, rh);
                assert!(
                    dp.is_finite(),
                    "dew point should be finite at t={t}, rh={rh}: got {dp}"
                );
                assert!(
                    dp <= t + 0.1,
                    "dew point {dp} must be <= dry bulb {t} at RH={rh}%"
                );
            }
        }
    }

    #[test]
    fn isa_extreme_elevation_safe() {
        // Beyond troposphere (~44 km), formula base goes negative.
        // Should return the safety floor, not NaN or negative.
        let p = isa_pressure_kpa(50_000.0);
        assert!(
            p > 0.0,
            "extreme elevation should return positive pressure: {p}"
        );
        assert!(
            p.is_finite(),
            "extreme elevation should return finite pressure: {p}"
        );
    }

    #[test]
    fn parse_from_file_path() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("weather.csv");
        let csv = build_test_csv(8760);
        std::fs::write(&path, &csv).expect("write temp file");

        let result = parse_resstock_csv(&path, 0.0, 39.7, -105.0, -7.0)
            .expect("should parse from file path");
        assert_eq!(result.len(), 8760);
        assert_eq!(result.meta.source_step_secs, 3600);
    }

    #[test]
    fn meta_carries_lat_lon_tz() {
        let csv = build_test_csv(8760);
        let result =
            parse_resstock_csv_str(&csv, 1609.0, 39.7, -105.0, -7.0).expect("should parse");
        assert!((result.meta.latitude - 39.7).abs() < 1e-9);
        assert!((result.meta.longitude - (-105.0)).abs() < 1e-9);
        assert!((result.meta.timezone_offset_h - (-7.0)).abs() < 1e-9);
        assert!((result.meta.elevation_m - 1609.0).abs() < 1e-9);
    }

    #[test]
    fn resstock_csv_surface_albedo_is_none() {
        let csv = build_test_csv(8760);
        let result = parse_resstock_csv_str(&csv, 0.0, 39.7, -105.0, -7.0).expect("should parse");
        assert!(
            result.surface_albedo.is_none(),
            "ResStock CSV has no albedo column; surface_albedo should be None"
        );
    }

    /// Build a synthetic ResStock CSV with a given interval (minutes) starting
    /// from a leap-year base date.  Used to expose the leap-year step-inference bug.
    fn build_test_csv_leap_year(interval_minutes: u32) -> String {
        use chrono::{NaiveDate, TimeDelta};

        // 2004 is a leap year: 366 days × (60/interval_minutes) rows/day
        let rows_per_day = (60 * 24 / interval_minutes) as usize;
        let total_rows = 366 * rows_per_day;

        let mut lines = Vec::with_capacity(total_rows + 1);
        lines.push(
            "date_time,Dry Bulb Temperature [°C],Relative Humidity [%],\
             Wind Speed [m/s],Wind Direction [Deg],\
             Global Horizontal Radiation [W/m2],\
             Direct Normal Radiation [W/m2],\
             Diffuse Horizontal Radiation [W/m2]"
                .to_string(),
        );

        let base = NaiveDate::from_ymd_opt(2004, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();

        for i in 0..total_rows {
            // End-of-interval: first row is at base + 1 interval.
            let dt = base + TimeDelta::minutes((i as i64 + 1) * interval_minutes as i64);
            lines.push(format!(
                "{},10.0,60.0,3.0,180.0,0.0,0.0,0.0",
                dt.format("%Y-%m-%d %H:%M:%S")
            ));
        }

        lines.join("\n")
    }

    // Regression test for the leap-year timestep inference bug (ticket #029).
    // A 30-minute sub-hourly leap-year file has 17568 rows.
    // The buggy code uses `8760 * 3600` as the year length, so
    // `31536000 % 17568 == 1440 != 0` and it falls back to 3600 s.
    // The correct answer is 8784 * 3600 / 17568 = 1800 s.
    // This test FAILS until the fix in ticket #029 is applied.
    /// Fix pending on ticket 029 — will stop panicking when leap-year 30-min timestep is inferred correctly
    #[should_panic(expected = "should infer 1800 s step")]
    #[test]
    fn leap_year_30min_step_inferred_correctly() {
        let csv = build_test_csv_leap_year(30);
        let result = parse_resstock_csv_str(&csv, 0.0, 39.7, -105.0, -7.0)
            .expect("should parse 17568-row leap-year CSV");
        assert_eq!(result.len(), 17568);
        assert_eq!(
            result.meta.source_step_secs, 1800,
            "30-min sub-hourly leap-year file should infer 1800 s step, \
             got {} s (leap-year fix not applied)",
            result.meta.source_step_secs
        );
    }

    // Regression test: hourly leap-year file (8784 rows) should still
    // infer 3600 s even though the fix changes how the year length is computed.
    // With the fix applied this must also pass.
    #[test]
    fn leap_year_hourly_step_inferred_correctly() {
        let csv = build_test_csv_leap_year(60);
        let result = parse_resstock_csv_str(&csv, 0.0, 39.7, -105.0, -7.0)
            .expect("should parse 8784-row leap-year CSV");
        assert_eq!(result.len(), 8784);
        assert_eq!(
            result.meta.source_step_secs, 3600,
            "hourly leap-year file should infer 3600 s step, got {} s",
            result.meta.source_step_secs
        );
    }

    /// Regression test — ticket 099: ResStock CSV timestamps are end-of-interval
    /// (first row = 01:00:00, representing the period 00:00–01:00), identical to
    /// the TMY3 and EPW convention. The midpoint of each hourly record therefore
    /// lies 1800 s (30 min) before the timestamp, so `midpoint_offset_secs` must
    /// be 1800, not 0.
    ///
    /// Evidence: real NREL ResStock AMY 2018 CSV files
    /// (e.g. G0100630_2018.csv) start at `2018-01-01 01:00:00`, confirming the
    /// end-of-interval convention. The TMY3 path was corrected to 1800 (B4 fix at
    /// `tmy3.rs`); this test guards the equivalent fix for the ResStock CSV path.
    ///
    /// This test FAILS until `midpoint_offset_secs: 0` is changed to
    /// `midpoint_offset_secs: 1800` in `parse_resstock_csv_str`.
    /// Fix pending on ticket 099 — will stop panicking when ResStock CSV midpoint_offset_secs is 1800
    #[should_panic(expected = "midpoint_offset_secs must be 1800 s")]
    #[test]
    fn resstock_midpoint_offset_is_1800_seconds() {
        let csv = build_test_csv(8760);
        let result = parse_resstock_csv_str(&csv, 0.0, 39.7, -105.0, -7.0)
            .expect("should parse 8760-row CSV");
        assert_eq!(
            result.meta.midpoint_offset_secs, 1800,
            "ResStock CSV uses end-of-interval timestamps: midpoint_offset_secs \
             must be 1800 s (30 min), not {}",
            result.meta.midpoint_offset_secs
        );
    }
}
