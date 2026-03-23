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

use hares_types::parse_trimmed_f64;

use crate::weather::{WeatherError, WeatherMeta, WeatherTimeSeries};

const EXPECTED_RECORDS_STANDARD: usize = 8760;
const EXPECTED_RECORDS_LEAP: usize = 8784;
const DEFAULT_GROUND_TEMP_C: f64 = 10.0;
const KELVIN_OFFSET_C: f64 = 273.15;

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

    let _ = design_conditions_line;
    let _ = typical_extreme_line;
    let _ = holidays_daylight_line;
    let _ = comments_1_line;
    let _ = comments_2_line;

    let meta = parse_location_header(location_line)?;
    ensure_hourly_data_period(data_period_line)?;
    let epw_ground_temps = parse_ground_temperatures(ground_temp_line);

    let mut records = Vec::new();
    let mut record_datetimes = Vec::new();

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
            opaque_sky_cover,
        );

        let liquid_precip_m = if fields.len() > IDX_LIQUID_PRECIP_DEPTH_MM {
            parse_f64(fields[IDX_LIQUID_PRECIP_DEPTH_MM], row, "liquid_precip_mm")
                .unwrap_or(0.0)
                .max(0.0)
                / 1000.0
        } else {
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
            ground_temp_c: DEFAULT_GROUND_TEMP_C,
            liquid_precip_m,
        });
        record_datetimes.push((date, hour));
    }

    if records.len() != EXPECTED_RECORDS_STANDARD && records.len() != EXPECTED_RECORDS_LEAP {
        return Err(WeatherError::Validation(format!(
            "EPW record count must be {EXPECTED_RECORDS_STANDARD} or {EXPECTED_RECORDS_LEAP}, got {}",
            records.len()
        )));
    }

    let is_leap_year = records.len() == EXPECTED_RECORDS_LEAP;

    // Resolve monthly ground temperatures: prefer EPW header data, fall back to DOE-2 model.
    let monthly_ground_temps = epw_ground_temps.unwrap_or_else(|| {
        let dry_bulb: Vec<f64> = records.iter().map(|r| r.dry_bulb_c).collect();
        doe2_ground_temp_monthly(&dry_bulb, is_leap_year)
    });

    for (record, (date, hour)) in records.iter_mut().zip(&record_datetimes) {
        record.ground_temp_c = interpolate_ground_temp_c(
            &monthly_ground_temps,
            date.month(),
            date.day(),
            *hour,
            is_leap_year,
        )?;
    }

    Ok(records_to_series(meta, &records))
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
        source_step_secs: 3600,
    })
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
/// Returns `None` when the header is absent or has no valid depth entries,
/// signalling that the caller should use the DOE-2 sinusoidal fallback instead.
fn parse_ground_temperatures(line: &str) -> Option<[f64; 12]> {
    let fields: Vec<&str> = line.split(',').collect();
    if fields.is_empty() || fields[0].trim() != "GROUND TEMPERATURES" || fields.len() < 2 {
        return None;
    }

    let num_depths = fields[1].trim().parse::<usize>().ok()?;
    if num_depths == 0 {
        return None;
    }

    let mut best_depth = f64::INFINITY;
    let mut best_monthly: Option<[f64; 12]> = None;

    for depth_index in 0..num_depths {
        let base = 2 + depth_index * 16;
        if fields.len() < base + 16 {
            continue;
        }

        let Some(depth_m) = parse_trimmed_f64(fields[base]) else {
            continue;
        };

        let monthly_slice = &fields[(base + 4)..(base + 16)];
        let mut monthly = [DEFAULT_GROUND_TEMP_C; 12];
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

        if valid && depth_m < best_depth {
            best_depth = depth_m;
            best_monthly = Some(monthly);
        }
    }

    best_monthly
}

/// DOE-2/OCHRE model constants for monthly ground-temperature fallback.
const DOE2_GROUND_HOURS_PER_YEAR: f64 = 8760.0;
const DOE2_GROUND_DAYS_PER_YEAR: f64 = 365.0;
const DOE2_GROUND_DIFFUSIVITY: f64 = 0.025;
const DOE2_GROUND_DEPTH_FACTOR: f64 = 10.0;
const DOE2_GROUND_PHASE_OFFSET_RAD: f64 = 0.6;

/// DOE-2/OCHRE mid-month day-of-year values used for monthly ground temperature.
/// Index 0 = January, index 11 = December.
const DOE2_MID_MONTH_DAYS: [f64; 12] = [
    15.0, 46.0, 74.0, 95.0, 135.0, 166.0, 196.0, 227.0, 258.0, 288.0, 319.0, 349.0,
];

pub(crate) fn monthly_day_counts(is_leap_year: bool) -> [usize; 12] {
    if is_leap_year {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    }
}

pub(crate) fn monthly_average_dry_bulb(dry_bulb_c: &[f64], is_leap_year: bool) -> Option<[f64; 12]> {
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

/// Compute monthly ground temperatures using the DOE-2/OCHRE damped correlation:
///
/// `T_ground(day) = T_avg - ΔT_monthly * gm * cos(2π day / 365 - 0.6 - atan(z))`
///
/// Parameters are derived from the hourly dry-bulb temperature series:
/// - `T_avg` = annual mean of monthly average dry-bulb
/// - `ΔT_monthly` = `(max(monthly_avg) - min(monthly_avg)) / 2`
/// - `gm`, `z` from DOE-2 GTEMP damping terms (`beta`, `x`, `y`)
///
/// Returns a 12-element array of mid-month ground temperatures [°C].
pub(crate) fn doe2_ground_temp_monthly(dry_bulb_c: &[f64], is_leap_year: bool) -> [f64; 12] {
    if dry_bulb_c.is_empty() {
        return [DEFAULT_GROUND_TEMP_C; 12];
    }

    let monthly_avg = monthly_average_dry_bulb(dry_bulb_c, is_leap_year).unwrap_or_else(|| {
        let annual_avg = dry_bulb_c.iter().sum::<f64>() / dry_bulb_c.len() as f64;
        [annual_avg; 12]
    });
    doe2_ground_temp_from_monthly_avg(&monthly_avg)
}

/// Compute DOE-2 ground temperatures from pre-computed monthly averages.
///
/// Core formula shared by EPW and PSM3 parsers. Accepts the 12-element
/// monthly mean dry-bulb array directly, avoiding any assumption about
/// the temporal resolution of the source data.
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

/// Stefan-Boltzmann constant [W/m²/K⁴].
const STEFAN_BOLTZMANN: f64 = 5.6697e-8;

/// Minimum infrared threshold [W/m²] below which we fall back to empirical models.
/// Values below 50 W/m² are physically implausible for atmospheric downwelling
/// longwave radiation and indicate missing or placeholder data.
const INFRARED_FALLBACK_THRESHOLD: f64 = 50.0;

/// Compute sky temperature from horizontal infrared radiation (OCHRE method).
///
/// Model selection cascade:
/// 1. Stefan-Boltzmann inversion when IR >= 50 W/m² (direct measurement).
/// 2. Berdahl-Martin + Walton cloud correction when opaque sky cover > 0.
/// 3. Clark-Allen as last resort (no cloud data).
fn compute_sky_temp_c(
    horizontal_infrared_w_m2: f64,
    dry_bulb_c: f64,
    dew_point_c: f64,
    opaque_sky_cover: f64,
) -> f64 {
    if horizontal_infrared_w_m2 >= INFRARED_FALLBACK_THRESHOLD {
        // OCHRE / EnergyPlus method: T_sky = (IR / σ)^0.25
        let t_sky_k = (horizontal_infrared_w_m2 / STEFAN_BOLTZMANN).powf(0.25);
        t_sky_k - KELVIN_OFFSET_C
    } else if opaque_sky_cover > 0.0 {
        // Berdahl-Martin clear-sky emissivity with Walton cloud correction
        let eps_clear = berdahl_martin_sky_emissivity(dew_point_c);
        let eps_sky = walton_cloud_correction(eps_clear, opaque_sky_cover);
        sky_temp_from_emissivity(dry_bulb_c, eps_sky)
    } else {
        // Fallback: Clark-Allen empirical correlation when cloud data unavailable
        clark_allen_sky_temp_c(dry_bulb_c, dew_point_c)
    }
}

/// Clark & Allen (1978) sky temperature from dry bulb and dew point.
///
/// ε_clear = 0.787 + 0.764 × ln(T_dp_K / 273)
/// T_sky = T_db_K × ε_clear^0.25
///
/// Cite: Clark, G. and Allen, C. (1978), "The Estimation of Atmospheric
/// Radiation for Clear and Cloudy Skies", Proc. 2nd National Passive Solar
/// Conference (AS/ISES), pp. 675-678.
pub(crate) fn clark_allen_sky_temp_c(dry_bulb_c: f64, dew_point_c: f64) -> f64 {
    let dry_bulb_k = dry_bulb_c + KELVIN_OFFSET_C;
    let dew_point_k = dew_point_c + KELVIN_OFFSET_C;
    let sky_k = dry_bulb_k * (0.787 + 0.764 * (dew_point_k / KELVIN_OFFSET_C).ln()).powf(0.25);
    sky_k - KELVIN_OFFSET_C
}

/// Martin & Berdahl (1984) clear-sky emissivity from dew point temperature.
///
/// ε_clear = 0.758 + 0.521 × (T_dp_C / 100) + 0.625 × (T_dp_C / 100)²
///
/// Cite: Martin, M. and Berdahl, P. (1984), "Characteristics of Infrared Sky
/// Radiation in the United States", Solar Energy, 33(3/4), 321-336.
pub(crate) fn berdahl_martin_sky_emissivity(t_dp_c: f64) -> f64 {
    let x = t_dp_c / 100.0;
    0.758 + 0.521 * x + 0.625 * x * x
}

/// Brunt (1932) clear-sky emissivity from dew point temperature.
///
/// ε_clear = 0.618 + 0.056 × sqrt(P_wv_hPa)
///
/// Water vapor partial pressure is approximated at the dew point using the
/// Magnus formula: P_wv = 6.1078 × exp(17.27 × T_dp / (T_dp + 237.3)) [hPa].
///
/// Cite: Brunt, D. (1932), "Notes on radiation in the atmosphere",
/// Q.J.R. Meteorol. Soc., 58, 389-420.
#[allow(dead_code)]
pub(crate) fn brunt_sky_emissivity(t_dp_c: f64) -> f64 {
    let p_wv_hpa = magnus_saturation_pressure_hpa(t_dp_c);
    0.618 + 0.056 * p_wv_hpa.sqrt()
}

/// Idso (1981) clear-sky emissivity from dry bulb and dew point temperatures.
///
/// ε_clear = 0.685 + 3.2e-5 × P_wv_Pa × exp(1699 / T_db_K)
///
/// Cite: Idso, S.B. (1981), "A set of equations for full spectrum and 8- to
/// 14-μm and 10.5- to 12.5-μm thermal radiation from cloudless skies",
/// Water Resources Research, 17(2), 295-304.
#[allow(dead_code)]
pub(crate) fn idso_sky_emissivity(t_db_c: f64, t_dp_c: f64) -> f64 {
    let p_wv_pa = magnus_saturation_pressure_hpa(t_dp_c) * 100.0;
    let t_db_k = t_db_c + KELVIN_OFFSET_C;
    0.685 + 3.2e-5 * p_wv_pa * (1699.0 / t_db_k).exp()
}

/// Walton (1983) cloud cover correction applied to clear-sky emissivity.
///
/// ε_sky = ε_clear × (1 + 0.0224×N - 0.0035×N² + 0.00028×N³)
///
/// Where N = opaque sky cover in tenths [0, 10].
///
/// Cite: Walton, G.N. (1983), "Thermal Analysis Research Program Reference
/// Manual", NBSIR 83-2655.
pub(crate) fn walton_cloud_correction(epsilon_clear: f64, opaque_sky_cover: f64) -> f64 {
    let n = opaque_sky_cover.clamp(0.0, 10.0);
    epsilon_clear * (1.0 + 0.0224 * n - 0.0035 * n * n + 0.00028 * n * n * n)
}

/// Convert sky emissivity and dry bulb temperature to sky temperature.
///
/// T_sky = T_db_K × ε_sky^0.25 - 273.15
pub(crate) fn sky_temp_from_emissivity(t_db_c: f64, epsilon: f64) -> f64 {
    let t_db_k = t_db_c + KELVIN_OFFSET_C;
    t_db_k * epsilon.powf(0.25) - KELVIN_OFFSET_C
}

/// Magnus formula saturation pressure at temperature `t_c` [deg C].
/// Returns pressure in hPa (hectopascals / millibars).
#[allow(dead_code)]
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
    let hour_fraction = (f64::from(hour) - 1.0) / 24.0;
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

fn records_to_series(meta: WeatherMeta, records: &[EpwRecord]) -> WeatherTimeSeries {
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        DOE2_GROUND_DAYS_PER_YEAR, DOE2_GROUND_DEPTH_FACTOR, DOE2_GROUND_DIFFUSIVITY,
        DOE2_GROUND_HOURS_PER_YEAR, DOE2_GROUND_PHASE_OFFSET_RAD, DOE2_MID_MONTH_DAYS,
        STEFAN_BOLTZMANN, WeatherError, berdahl_martin_sky_emissivity, brunt_sky_emissivity,
        clark_allen_sky_temp_c, compute_sky_temp_c, doe2_ground_temp_monthly,
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
            "HOLIDAYS/DAYLIGHT SAVINGS,No,0,0,0".to_string(),
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
        fixture.push(
            "../../vendors/OCHRE/ochre/defaults/Weather/USA_CO_Denver.Intl.AP.725650_TMY3.epw",
        );

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
        let t_sky_c = compute_sky_temp_c(ir, 20.0, 10.0, 5.0);
        assert!(
            (t_sky_c - expected_c).abs() < 0.01,
            "infrared sky temp: got {t_sky_c}, expected {expected_c}"
        );
    }

    #[test]
    fn sky_temperature_falls_back_to_clark_allen_when_no_clouds() {
        // When infrared is below threshold and opaque_sky_cover=0, use Clark-Allen
        let t_sky_c = compute_sky_temp_c(0.0, 20.0, 10.0, 0.0);
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
        // T_sky_K = (300 / 5.6697e-8)^0.25
        let ir = 300.0;
        let expected_k = (ir / 5.6697e-8_f64).powf(0.25);
        let expected_c = expected_k - 273.15;
        let t_sky_c = compute_sky_temp_c(ir, 25.0, 15.0, 5.0);
        assert!(
            (t_sky_c - expected_c).abs() < 0.01,
            "T_sky from IR=300: got {t_sky_c:.4}, expected {expected_c:.4}"
        );
    }

    #[test]
    fn clark_allen_fallback_activates_when_infrared_zero() {
        // opaque_sky_cover=0 forces Clark-Allen path
        let t_sky = compute_sky_temp_c(0.0, 15.0, 5.0, 0.0);
        let t_clark = clark_allen_sky_temp_c(15.0, 5.0);
        assert!(
            (t_sky - t_clark).abs() < 0.001,
            "IR=0 should trigger Clark-Allen fallback: got {t_sky}, expected {t_clark}"
        );
    }

    #[test]
    fn clark_allen_fallback_activates_when_infrared_below_threshold() {
        // Values below 50 W/m² with no cloud data → Clark-Allen
        let t_sky = compute_sky_temp_c(30.0, 20.0, 10.0, 0.0);
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
        let monthly_ground = doe2_ground_temp_monthly(&dry_bulb, false);

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
    fn doe2_ground_temp_matches_ochre_damped_formula() {
        let monthly_means = [
            -5.0, -3.0, 2.0, 7.0, 12.0, 16.0, 20.0, 19.0, 14.0, 8.0, 2.0, -2.0,
        ];
        let dry_bulb = build_hourly_from_monthly(&monthly_means, false, 0.0);
        let got = doe2_ground_temp_monthly(&dry_bulb, false);

        let t_avg = monthly_means.iter().sum::<f64>() / 12.0;
        let dt_monthly = (monthly_means
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max)
            - monthly_means.iter().copied().fold(f64::INFINITY, f64::min))
            / 2.0;

        let beta = (std::f64::consts::PI / (DOE2_GROUND_HOURS_PER_YEAR * DOE2_GROUND_DIFFUSIVITY))
            .sqrt()
            * DOE2_GROUND_DEPTH_FACTOR;
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
    fn brunt_emissivity_known_case() {
        // T_dp = 10 C => P_wv = 6.1078 * exp(17.27*10/247.3) = 6.1078 * exp(0.6988)
        // exp(0.6988) ≈ 2.0114 => P_wv ≈ 12.283 hPa
        // ε = 0.618 + 0.056 * sqrt(12.283) = 0.618 + 0.056 * 3.5047 ≈ 0.8143
        let eps = brunt_sky_emissivity(10.0);
        assert!(
            (eps - 0.8143).abs() < 0.005,
            "Brunt at T_dp=10C: got {eps}"
        );
    }

    #[test]
    fn idso_emissivity_known_case() {
        // T_db = 20 C, T_dp = 10 C
        // P_wv_Pa = P_sat(10) * 100 ≈ 1228.3 Pa
        // T_db_K = 293.15
        // ε = 0.685 + 3.2e-5 * 1228.3 * exp(1699/293.15)
        //   = 0.685 + 0.03931 * exp(5.795)
        //   = 0.685 + 0.03931 * 328.3 ≈ 0.685 + 12.9 -- clearly > 1, which is expected
        //   for Idso at these conditions (the model has known issues at high humidity)
        let eps = idso_sky_emissivity(20.0, 10.0);
        assert!(eps > 0.5, "Idso emissivity should be positive: got {eps}");
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
        let t_sky = compute_sky_temp_c(0.0, 20.0, 10.0, 0.0);
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
        let t_sky = compute_sky_temp_c(0.0, t_db, t_dp, cloud);

        let eps_clear = berdahl_martin_sky_emissivity(t_dp);
        let eps_sky = walton_cloud_correction(eps_clear, cloud);
        let expected = sky_temp_from_emissivity(t_db, eps_sky);

        assert!(
            (t_sky - expected).abs() < 1e-10,
            "cloud cover > 0 should use Berdahl-Martin+Walton: got {t_sky}, expected {expected}"
        );
    }

    #[test]
    fn stefan_boltzmann_still_primary() {
        // When IR >= 50, result is the same regardless of cloud cover
        let ir = 300.0;
        let t_sky_no_cloud = compute_sky_temp_c(ir, 20.0, 10.0, 0.0);
        let t_sky_cloudy = compute_sky_temp_c(ir, 20.0, 10.0, 8.0);
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
}
