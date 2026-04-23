//! Weather data processing and time-series handling.

use std::io::BufRead;
use std::path::Path;

use thiserror::Error;

// Re-export from hares-types for use in this crate's fallback logic.
use hares_types::DEFAULT_GROUND_ALBEDO;

use crate::epw::compute_sky_temp_c;

/// Supported weather file formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
pub fn parse_weather_with_location(
    path: impl AsRef<Path>,
    elevation_m: f64,
    latitude: f64,
    longitude: f64,
    timezone_offset_h: f64,
) -> Result<WeatherTimeSeries, WeatherError> {
    let path = path.as_ref();
    match detect_weather_format(path)? {
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
    }
}

/// Metadata extracted from an EPW header.
#[derive(Debug, Clone, PartialEq)]
pub struct WeatherMeta {
    pub location: String,
    pub latitude: f64,
    pub longitude: f64,
    pub timezone_offset_h: f64,
    pub elevation_m: f64,
    /// Native timestep of the source file in seconds (e.g. 3600 for EPW, 300 for 5-min PSM3).
    pub source_step_secs: u32,
    /// Seconds to subtract from simulation time when indexing into the
    /// resampled weather array. For EPW (hour-ending convention), this is
    /// `source_step_secs / 2` (1800s for hourly) so that PCHIP knot positions
    /// align with period midpoints, matching OCHRE/pvlib's +30min convention.
    /// For hour-beginning formats (PSM3), this is 0.
    pub midpoint_offset_secs: u32,
}

/// Upsampling interpolation strategy for continuous weather fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResampleMethod {
    /// Piecewise Cubic Hermite Interpolating Polynomial -- smooth, monotone,
    /// physically superior for continuous fields like temperature. Default.
    #[default]
    Pchip,
    /// Zero-order hold (forward fill) -- each sub-step gets the previous
    /// hourly value. Matches OCHRE's pandas `resample().ffill()` convention.
    /// Use for parity testing against OCHRE.
    Zoh,
    /// Linear interpolation between hourly knots.
    Linear,
    /// Circular linear interpolation for angular quantities (e.g. wind direction).
    ///
    /// Interpolates along the shortest arc between two angles, correctly
    /// handling wrap-around at 0°/360°. For example, 350° → 10° interpolates
    /// through 0° (incrementing), not through 180° (the long way).
    ///
    /// Reference: EnergyPlus WeatherManager.cc:3183-3197 (`interpolateWindDirection`).
    ///
    /// Missing/sentinel values (NaN or outside [0, 360)) are NOT interpolated
    /// circularly -- intervals touching a sentinel use ZOH from the left value.
    CircularLinear,
}

/// Per-column override for weather resampling strategy.
///
/// All fields default to `None` (use the category default: PCHIP for
/// continuous, ZOH for energy/wind). Set a field to override.
///
/// `sky_temp_c` is not overridable because it is always recomputed from
/// the interpolated inputs after resampling — see `compute_sky_temp_c`.
#[derive(Debug, Clone, Default)]
pub struct ResampleOverrides {
    pub dry_bulb: Option<ResampleMethod>,
    pub dew_point: Option<ResampleMethod>,
    pub rel_humidity: Option<ResampleMethod>,
    pub pressure: Option<ResampleMethod>,
    pub infrared: Option<ResampleMethod>,
    pub ground_temp: Option<ResampleMethod>,
    pub opaque_sky_cover: Option<ResampleMethod>,
    pub ghi: Option<ResampleMethod>,
    pub dni: Option<ResampleMethod>,
    pub dhi: Option<ResampleMethod>,
    pub wind_speed: Option<ResampleMethod>,
    pub wind_dir: Option<ResampleMethod>,
}

impl ResampleOverrides {
    /// All continuous fields set to ZOH -- matches OCHRE's resampling.
    ///
    /// Note: sky_temp_c is not listed because it is always recomputed from
    /// the interpolated inputs, never directly interpolated.
    pub fn ochre_compat() -> Self {
        Self {
            dry_bulb: Some(ResampleMethod::Zoh),
            dew_point: Some(ResampleMethod::Zoh),
            rel_humidity: Some(ResampleMethod::Zoh),
            pressure: Some(ResampleMethod::Zoh),
            infrared: Some(ResampleMethod::Zoh),
            ground_temp: Some(ResampleMethod::Zoh),
            opaque_sky_cover: Some(ResampleMethod::Zoh),
            ..Default::default()
        }
    }
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
    /// Liquid precipitation depth per timestep [m].
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

    /// Resample weather data to a different timestep.
    ///
    /// Supports three cases based on `self.meta.source_step_secs` vs `target_step_secs`:
    /// - Same resolution: return clone (no-op).
    /// - Upsampling (source coarser than target): PCHIP for continuous fields,
    ///   ZOH for solar/wind, distribute for accumulated fields.
    /// - Downsampling (source finer than target): mean for instantaneous and solar
    ///   fields, sum for accumulated fields.
    ///
    /// Requires that one step divides evenly into the other.
    /// Resample with default strategies (PCHIP for continuous, ZOH for energy/wind).
    pub fn resample(&self, target_step_secs: u32) -> Result<Self, WeatherError> {
        self.resample_with(target_step_secs, &ResampleOverrides::default())
    }

    /// Resample with per-column strategy overrides.
    pub fn resample_with(
        &self,
        target_step_secs: u32,
        overrides: &ResampleOverrides,
    ) -> Result<Self, WeatherError> {
        if target_step_secs == 0 {
            return Err(WeatherError::Resample(
                "target_step_secs must be > 0".to_string(),
            ));
        }

        let source = self.meta.source_step_secs;

        if source == target_step_secs {
            return Ok(self.clone());
        }

        if source > target_step_secs {
            // Upsampling: source is coarser → interpolate to finer resolution.
            if !source.is_multiple_of(target_step_secs) {
                return Err(WeatherError::Resample(format!(
                    "incompatible timestep: {source} % {target_step_secs} != 0"
                )));
            }
            let factor = (source / target_step_secs) as usize;

            let mut rel_humidity_pct = pchip_resample(&self.rel_humidity_pct, factor);
            for v in &mut rel_humidity_pct {
                *v = v.clamp(0.0, 100.0);
            }
            let mut opaque_sky_cover = pchip_resample(&self.opaque_sky_cover, factor);
            for v in &mut opaque_sky_cover {
                *v = v.clamp(0.0, 10.0);
            }

            let dry_bulb_c = resample_field(
                &self.dry_bulb_c,
                factor,
                overrides.dry_bulb.unwrap_or(ResampleMethod::Pchip),
            );
            let dew_point_c = resample_field(
                &self.dew_point_c,
                factor,
                overrides.dew_point.unwrap_or(ResampleMethod::Pchip),
            );
            let pressure_kpa = resample_field(
                &self.pressure_kpa,
                factor,
                overrides.pressure.unwrap_or(ResampleMethod::Pchip),
            );
            let horizontal_infrared_w_m2 = resample_field(
                &self.horizontal_infrared_w_m2,
                factor,
                overrides.infrared.unwrap_or(ResampleMethod::Pchip),
            );
            let ground_temp_c = resample_field(
                &self.ground_temp_c,
                factor,
                overrides.ground_temp.unwrap_or(ResampleMethod::Pchip),
            );

            // Sky temperature is recomputed from interpolated inputs rather than
            // interpolated directly.  T_sky is a non-linear function of IR, dry-bulb,
            // dew-point, and sky cover; directly interpolating it violates the chain
            // rule and produces values inconsistent with the other interpolated fields.
            // EnergyPlus does the same at WeatherManager.cc:3113.
            let sky_temp_c: Vec<f64> = dry_bulb_c
                .iter()
                .zip(&dew_point_c)
                .zip(&horizontal_infrared_w_m2)
                .zip(&opaque_sky_cover)
                .map(|(((&db, &dp), &ir), &osc)| compute_sky_temp_c(ir, db, dp, osc))
                .collect();

            Ok(Self {
                meta: WeatherMeta {
                    source_step_secs: target_step_secs,
                    ..self.meta.clone()
                },
                // Continuous instantaneous fields default to PCHIP (smooth, monotone).
                // Override to ZOH for OCHRE parity or Linear for simpler interpolation.
                dry_bulb_c,
                dew_point_c,
                rel_humidity_pct,
                pressure_kpa,
                horizontal_infrared_w_m2,
                sky_temp_c,
                ground_temp_c,
                opaque_sky_cover,
                // Period-average energy flux defaults to ZOH.
                ghi_w_m2: resample_field(
                    &self.ghi_w_m2,
                    factor,
                    overrides.ghi.unwrap_or(ResampleMethod::Zoh),
                ),
                dni_w_m2: resample_field(
                    &self.dni_w_m2,
                    factor,
                    overrides.dni.unwrap_or(ResampleMethod::Zoh),
                ),
                dhi_w_m2: resample_field(
                    &self.dhi_w_m2,
                    factor,
                    overrides.dhi.unwrap_or(ResampleMethod::Zoh),
                ),
                // Turbulent/stochastic → ZOH.
                wind_speed_m_s: resample_field(
                    &self.wind_speed_m_s,
                    factor,
                    overrides.wind_speed.unwrap_or(ResampleMethod::Zoh),
                ),
                wind_dir_deg: resample_field(
                    &self.wind_dir_deg,
                    factor,
                    overrides.wind_dir.unwrap_or(ResampleMethod::CircularLinear),
                ),
                // Accumulated depth → distribute evenly so downstream sums are preserved.
                liquid_precip_m: distribute_accumulated(&self.liquid_precip_m, factor),
                // Surface property → ZOH (not interpolatable).
                surface_albedo: self
                    .surface_albedo
                    .as_ref()
                    .map(|v| replicate_zoh(v, factor)),
            })
        } else {
            // Downsampling: source is finer → aggregate to coarser resolution.
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
            let horizontal_infrared_w_m2 =
                mean_downsample(&self.horizontal_infrared_w_m2, ratio);
            let ground_temp_c = mean_downsample(&self.ground_temp_c, ratio);
            let opaque_sky_cover = mean_downsample(&self.opaque_sky_cover, ratio);

            // Recompute sky temperature from downsampled inputs (same rationale as
            // upsampling: T_sky is non-linear in its inputs, so averaging T_sky is
            // inconsistent with averaging the input fields).
            let sky_temp_c: Vec<f64> = dry_bulb_c
                .iter()
                .zip(&dew_point_c)
                .zip(&horizontal_infrared_w_m2)
                .zip(&opaque_sky_cover)
                .map(|(((&db, &dp), &ir), &osc)| compute_sky_temp_c(ir, db, dp, osc))
                .collect();

            Ok(Self {
                meta: WeatherMeta {
                    source_step_secs: target_step_secs,
                    ..self.meta.clone()
                },
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
            })
        }
    }
}

/// Fritsch-Carlson monotone cubic interpolation slopes.
///
/// Algorithm: Fritsch, F.N. and Carlson, R.E. (1980)
/// "Monotone Piecewise Cubic Interpolation",
/// SIAM Journal on Numerical Analysis, 17(2), pp. 238-246.
/// doi:10.1137/0717021
///
/// Given uniformly-spaced values `y` (unit spacing h=1), returns the slope
/// (first derivative) at each knot such that the resulting piecewise cubic
/// Hermite interpolant preserves monotonicity within every interval.
pub(crate) fn fritsch_carlson_slopes(y: &[f64]) -> Vec<f64> {
    let n = y.len();
    if n == 0 {
        return vec![];
    }
    if n == 1 {
        return vec![0.0];
    }

    // Secant slopes between consecutive knots (h=1, so delta_k = y[k+1] - y[k]).
    let mut delta: Vec<f64> = Vec::with_capacity(n - 1);
    for k in 0..n - 1 {
        delta.push(y[k + 1] - y[k]);
    }

    // Initialize tangent slopes.
    let mut d = vec![0.0_f64; n];

    // Boundary slopes: non-centered three-point formula with shape-preserving caps.
    // Matches SLATEC pchim.f, SciPy PchipInterpolator, and MATLAB pchip.
    d[0] = if n > 2 {
        let s = 1.5 * delta[0] - 0.5 * delta[1];
        if s.signum() != delta[0].signum() {
            0.0
        } else if delta[0].signum() != delta[1].signum() && s.abs() > (3.0 * delta[0]).abs() {
            3.0 * delta[0]
        } else {
            s
        }
    } else {
        delta[0]
    };
    d[n - 1] = if n > 2 {
        let s = 1.5 * delta[n - 2] - 0.5 * delta[n - 3];
        if s.signum() != delta[n - 2].signum() {
            0.0
        } else if delta[n - 2].signum() != delta[n - 3].signum()
            && s.abs() > (3.0 * delta[n - 2]).abs()
        {
            3.0 * delta[n - 2]
        } else {
            s
        }
    } else {
        delta[n - 2]
    };

    // Interior slopes: average of adjacent secants, zeroed when signs differ.
    for k in 1..n - 1 {
        if delta[k - 1].signum() != delta[k].signum() {
            d[k] = 0.0;
        } else {
            d[k] = (delta[k - 1] + delta[k]) / 2.0;
        }
    }

    // Fritsch-Carlson monotonicity correction (§3 of the paper).
    // Uses SLATEC pchim.f form: tau * d[k] instead of tau * alpha * delta[k]
    // to avoid 0 × ∞ = NaN when delta[k] is subnormal.
    for k in 0..n - 1 {
        if delta[k] == 0.0 {
            // Flat segment: both endpoint slopes must be zero.
            d[k] = 0.0;
            d[k + 1] = 0.0;
        } else {
            let alpha = d[k] / delta[k];
            let beta = d[k + 1] / delta[k];
            let r2 = alpha * alpha + beta * beta;
            if r2 > 9.0 {
                let tau = 3.0 / r2.sqrt();
                d[k] *= tau;
                d[k + 1] *= tau;
            }
        }
    }

    d
}

/// Resample data to a finer resolution using PCHIP interpolation.
///
/// Uses Piecewise Cubic Hermite Interpolating Polynomials (PCHIP)
/// with Fritsch-Carlson monotonicity-preserving slopes.
/// Guarantees: C1 continuity, monotonicity preservation,
/// interpolating (passes through original data points).
/// Hermite basis functions per Press et al., Numerical Recipes, 3rd ed., §3.3.
///
/// Reference: Fritsch & Carlson (1980), SIAM J. Numer. Anal. 17(2).
///
/// Edge cases:
/// - Single element: replicate (no neighbor to interpolate toward).
/// - Two elements: linear interpolation.
/// - Year boundary: flat extrapolation (no cyclic wrap).
/// - NaN in source data: propagated through interpolation.
///
/// Exposed as `pub` for integration tests. Not a stable public API;
/// callers outside `hares-io` should use [`WeatherTimeSeries::resample`].
#[doc(hidden)]
pub fn pchip_resample(values: &[f64], factor: usize) -> Vec<f64> {
    let n = values.len();
    if n == 0 {
        return vec![];
    }
    if factor <= 1 {
        return values.to_vec();
    }
    if n == 1 {
        return vec![values[0]; factor];
    }

    let total = n * factor;
    let mut out = Vec::with_capacity(total);

    if n == 2 {
        // Linear interpolation fallback for two-element input.
        // Clamp frac to [0, 1]: beyond the last knot, frac saturates at 1.0
        // producing flat-hold at values[n-1].
        for i in 0..total {
            let t = i as f64 / factor as f64;
            let k = (t as usize).min(n - 2);
            let frac = (t - k as f64).clamp(0.0, 1.0);
            out.push(values[k] + frac * (values[k + 1] - values[k]));
        }
        return out;
    }

    let d = fritsch_carlson_slopes(values);

    for i in 0..total {
        let t_global = i as f64 / factor as f64;
        let k = (t_global as usize).min(n - 2);
        // Clamp to [0, 1] for flat extrapolation at year boundary (no cyclic wrap).
        // h = 1.0 for uniform spacing, so t = t_global - k.
        let t = (t_global - k as f64).clamp(0.0, 1.0);

        // Hermite basis functions (h = 1.0, so slope terms are just d[k]).
        let t2 = t * t;
        let t3 = t2 * t;
        let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
        let h10 = t3 - 2.0 * t2 + t;
        let h01 = -2.0 * t3 + 3.0 * t2;
        let h11 = t3 - t2;

        out.push(h00 * values[k] + h10 * d[k] + h01 * values[k + 1] + h11 * d[k + 1]);
    }

    out
}

fn replicate_zoh(values: &[f64], factor: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(values.len().saturating_mul(factor));
    for &value in values {
        out.extend(std::iter::repeat_n(value, factor));
    }
    out
}

/// Linear interpolation between hourly knots (simpler than PCHIP, no overshoot).
fn linear_resample(values: &[f64], factor: usize) -> Vec<f64> {
    let n = values.len();
    if n == 0 {
        return vec![];
    }
    if factor <= 1 {
        return values.to_vec();
    }
    if n == 1 {
        return vec![values[0]; factor];
    }

    let total = n * factor;
    let mut out = Vec::with_capacity(total);
    for i in 0..total {
        let t = i as f64 / factor as f64;
        let k = (t as usize).min(n - 2);
        let frac = (t - k as f64).clamp(0.0, 1.0);
        out.push(values[k] + frac * (values[k + 1] - values[k]));
    }
    out
}

/// Circular linear interpolation for angular quantities (e.g. wind direction).
///
/// Interpolates along the shortest arc between consecutive angles so that
/// transitions like 350° → 10° go through 0°/360° instead of the long way
/// around (180°). This matches EnergyPlus WeatherManager.cc:3183-3197
/// (`interpolateWindDirection`).
///
/// Algorithm for each sub-sample between values[k] and values[k+1]:
/// 1. If either endpoint is NaN or outside [0, 360) (sentinel/missing),
///    fall back to ZOH from values[k].
/// 2. Compute shortest-arc delta:
///    `delta = ((values[k+1] - values[k] + 180) % 360) - 180`
/// 3. Interpolate: `result = (values[k] + t * delta + 360) % 360`
///    where t ∈ [0, 1] is the interpolation fraction.
///
/// Edge cases match [`linear_resample`]: single element replicates, two-element
/// input uses simple linear path, last segment flat-holds beyond the final knot.
fn circular_linear_resample(values: &[f64], factor: usize) -> Vec<f64> {
    let n = values.len();
    if n == 0 {
        return vec![];
    }
    if factor <= 1 {
        return values.to_vec();
    }
    if n == 1 {
        return vec![values[0]; factor];
    }

    /// Returns true for a valid wind direction in [0, 360).
    fn is_valid_angle(v: f64) -> bool {
        v.is_finite() && (0.0..360.0).contains(&v)
    }

    let total = n * factor;
    let mut out = Vec::with_capacity(total);
    for i in 0..total {
        let t = i as f64 / factor as f64;
        let k = (t as usize).min(n - 2);
        let frac = (t - k as f64).clamp(0.0, 1.0);
        let a = values[k];
        let b = values[k + 1];

        if !is_valid_angle(a) || !is_valid_angle(b) {
            // Sentinel or NaN: fall back to ZOH from left value.
            out.push(a);
        } else if frac == 0.0 {
            out.push(a);
        } else {
            // Shortest-arc delta in [-180, +180).
            // Uses Euclidean remainder to handle negative numerators correctly
            // (Rust's `%` truncates toward zero, giving negative remainders).
            let delta = ((b - a + 180.0).rem_euclid(360.0)) - 180.0;
            let result = (a + frac * delta).rem_euclid(360.0);
            out.push(result);
        }
    }
    out
}

/// Dispatch resampling based on method enum.
fn resample_field(values: &[f64], factor: usize, method: ResampleMethod) -> Vec<f64> {
    match method {
        ResampleMethod::Pchip => pchip_resample(values, factor),
        ResampleMethod::Zoh => replicate_zoh(values, factor),
        ResampleMethod::Linear => linear_resample(values, factor),
        ResampleMethod::CircularLinear => circular_linear_resample(values, factor),
    }
}

/// Distribute an accumulated quantity evenly across sub-timestep slots.
///
/// Unlike ZOH (which replicates an instantaneous value), accumulated fields
/// like precipitation depth represent a total over the source interval. Dividing
/// by `factor` preserves the integral when downstream code sums across slots.
fn distribute_accumulated(values: &[f64], factor: usize) -> Vec<f64> {
    let scale = 1.0 / factor as f64;
    let mut out = Vec::with_capacity(values.len().saturating_mul(factor));
    for &value in values {
        let distributed = value * scale;
        out.extend(std::iter::repeat_n(distributed, factor));
    }
    out
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
        WeatherError, WeatherField, WeatherMeta, WeatherTimeSeries, fritsch_carlson_slopes,
        pchip_resample,
    };

    fn sample_series() -> WeatherTimeSeries {
        WeatherTimeSeries {
            meta: WeatherMeta {
                location: "Test Site".to_string(),
                latitude: 39.74,
                longitude: -104.99,
                timezone_offset_h: -7.0,
                elevation_m: 1600.0,
                source_step_secs: 3600,
                midpoint_offset_secs: 0,
            },
            dry_bulb_c: vec![10.0, 11.0],
            dew_point_c: vec![5.0, 6.0],
            rel_humidity_pct: vec![40.0, 45.0],
            pressure_kpa: vec![90.0, 91.0],
            ghi_w_m2: vec![0.0, 500.0],
            dni_w_m2: vec![0.0, 700.0],
            dhi_w_m2: vec![0.0, 100.0],
            wind_speed_m_s: vec![2.0, 3.0],
            wind_dir_deg: vec![180.0, 190.0],
            opaque_sky_cover: vec![4.0, 5.0],
            horizontal_infrared_w_m2: vec![300.0, 310.0],
            sky_temp_c: vec![2.0, 3.0],
            ground_temp_c: vec![10.0, 10.1],
            liquid_precip_m: vec![0.0, 0.0],
            surface_albedo: None,
        }
    }

    /// 5-point series for testing PCHIP (not the linear-fallback 2-point path).
    fn sample_series_5pt() -> WeatherTimeSeries {
        WeatherTimeSeries {
            meta: WeatherMeta {
                location: "Test Site".to_string(),
                latitude: 39.74,
                longitude: -104.99,
                timezone_offset_h: -7.0,
                elevation_m: 1600.0,
                source_step_secs: 3600,
                midpoint_offset_secs: 0,
            },
            dry_bulb_c: vec![10.0, 12.0, 15.0, 13.0, 11.0],
            dew_point_c: vec![5.0, 6.0, 8.0, 7.0, 5.0],
            rel_humidity_pct: vec![40.0, 45.0, 60.0, 50.0, 42.0],
            pressure_kpa: vec![90.0, 91.0, 90.5, 91.5, 90.0],
            ghi_w_m2: vec![0.0, 200.0, 500.0, 300.0, 0.0],
            dni_w_m2: vec![0.0, 400.0, 700.0, 500.0, 0.0],
            dhi_w_m2: vec![0.0, 50.0, 100.0, 80.0, 0.0],
            wind_speed_m_s: vec![2.0, 3.0, 5.0, 4.0, 2.0],
            wind_dir_deg: vec![180.0, 190.0, 200.0, 210.0, 180.0],
            opaque_sky_cover: vec![4.0, 5.0, 6.0, 5.0, 4.0],
            horizontal_infrared_w_m2: vec![300.0, 310.0, 320.0, 310.0, 300.0],
            sky_temp_c: vec![2.0, 3.0, 4.0, 3.0, 2.0],
            ground_temp_c: vec![10.0, 10.1, 10.2, 10.1, 10.0],
            liquid_precip_m: vec![0.0, 0.0, 0.001, 0.0, 0.0],
            surface_albedo: None,
        }
    }

    #[test]
    fn get_returns_column_value() {
        let series = sample_series();
        assert_eq!(series.get(WeatherField::WindDirDeg, 1), 190.0);
        assert_eq!(series.get(WeatherField::PressureKpa, 0), 90.0);
    }

    #[test]
    fn resample_pchip_on_dry_bulb_and_circular_linear_on_wind_dir() {
        let series = sample_series();
        let resampled = series.resample(60).expect("resample should succeed");
        assert_eq!(resampled.len(), 120);

        // dry_bulb uses PCHIP (two-element → linear interpolation).
        // First sample hits the first knot exactly.
        assert!((resampled.dry_bulb_c[0] - 10.0).abs() < 1e-12);
        // Midpoint should be ~10.5 (linear for 2-element input).
        assert!((resampled.dry_bulb_c[30] - 10.5).abs() < 1e-12);
        // Values should be monotonically non-decreasing.
        for w in resampled.dry_bulb_c.windows(2) {
            assert!(w[1] >= w[0] - 1e-12, "monotonicity violated");
        }

        // Wind direction uses CircularLinear interpolation (B9 fix).
        // First sample hits the first knot exactly.
        assert!((resampled.wind_dir_deg[0] - 180.0).abs() < 1e-12);
        // Midpoint between 180 and 190 via circular linear: 185.
        assert!((resampled.wind_dir_deg[30] - 185.0).abs() < 1e-12);
        // Second hour starts at the second knot.
        assert!((resampled.wind_dir_deg[60] - 190.0).abs() < 1e-12);
    }

    #[test]
    fn resample_distributes_precipitation_across_sub_slots() {
        let mut series = sample_series();
        series.liquid_precip_m = vec![0.0, 0.006];
        let resampled = series.resample(60).expect("resample should succeed");
        let expected_per_slot = 0.006 / 60.0;
        for (i, &val) in resampled.liquid_precip_m.iter().enumerate().skip(60) {
            assert!(
                (val - expected_per_slot).abs() < 1e-15,
                "slot {i}: expected {expected_per_slot}, got {val}"
            );
        }
        let total: f64 = resampled.liquid_precip_m.iter().sum();
        assert!(
            (total - 0.006).abs() < 1e-12,
            "total rainfall must be preserved: got {total}"
        );
    }

    #[test]
    fn resample_rejects_non_divisor_step() {
        let series = sample_series();
        let err = series
            .resample(7)
            .expect_err("non-divisor timestep should fail");
        assert!(matches!(err, WeatherError::Resample(_)));
        assert!(err.to_string().contains("3600 % 7"));
    }

    #[test]
    fn resample_same_resolution_returns_clone() {
        let series = sample_series();
        let resampled = series.resample(3600).expect("same-res should succeed");
        assert_eq!(resampled, series);
    }

    #[test]
    fn resample_downsample_mean() {
        // 4-element series at 900s source, downsample to 3600s (ratio=4).
        let mut series = sample_series();
        series.meta.source_step_secs = 900;
        series.dry_bulb_c = vec![10.0, 12.0, 14.0, 16.0];
        series.dew_point_c = vec![5.0, 6.0, 7.0, 8.0];
        series.rel_humidity_pct = vec![40.0, 50.0, 60.0, 70.0];
        series.pressure_kpa = vec![90.0, 91.0, 92.0, 93.0];
        series.ghi_w_m2 = vec![100.0, 200.0, 300.0, 400.0];
        series.dni_w_m2 = vec![0.0, 0.0, 0.0, 0.0];
        series.dhi_w_m2 = vec![0.0, 0.0, 0.0, 0.0];
        series.wind_speed_m_s = vec![2.0, 3.0, 4.0, 5.0];
        series.wind_dir_deg = vec![180.0, 190.0, 200.0, 210.0];
        series.opaque_sky_cover = vec![4.0, 5.0, 6.0, 7.0];
        series.horizontal_infrared_w_m2 = vec![300.0, 310.0, 320.0, 330.0];
        series.sky_temp_c = vec![2.0, 3.0, 4.0, 5.0];
        series.ground_temp_c = vec![10.0, 10.1, 10.2, 10.3];
        series.liquid_precip_m = vec![0.001, 0.002, 0.003, 0.004];

        let down = series.resample(3600).expect("downsample should succeed");
        assert_eq!(down.len(), 1);
        assert!((down.dry_bulb_c[0] - 13.0).abs() < 1e-12);
        assert!((down.ghi_w_m2[0] - 250.0).abs() < 1e-12);
        // Precipitation is summed, not averaged.
        assert!((down.liquid_precip_m[0] - 0.01).abs() < 1e-12);
        assert_eq!(down.meta.source_step_secs, 3600);
    }

    #[test]
    fn resample_upsample_with_some_albedo() {
        let mut series = sample_series_5pt();
        series.surface_albedo = Some(vec![0.2, 0.7, 0.5, 0.3, 0.2]);
        let resampled = series.resample(600).expect("resample should succeed");
        let albedo = resampled
            .surface_albedo
            .as_ref()
            .expect("albedo should be Some");
        // ZOH: each source value replicated 6 times
        assert_eq!(albedo.len(), 5 * 6);
        assert!(albedo[..6].iter().all(|&v| (v - 0.2).abs() < 1e-12));
        assert!(albedo[6..12].iter().all(|&v| (v - 0.7).abs() < 1e-12));
    }

    #[test]
    fn resample_downsample_with_some_albedo() {
        let mut series = sample_series();
        series.meta.source_step_secs = 900;
        series.dry_bulb_c = vec![10.0, 12.0, 14.0, 16.0];
        series.dew_point_c = vec![5.0, 6.0, 7.0, 8.0];
        series.rel_humidity_pct = vec![40.0, 50.0, 60.0, 70.0];
        series.pressure_kpa = vec![90.0, 91.0, 92.0, 93.0];
        series.ghi_w_m2 = vec![100.0, 200.0, 300.0, 400.0];
        series.dni_w_m2 = vec![0.0; 4];
        series.dhi_w_m2 = vec![0.0; 4];
        series.wind_speed_m_s = vec![2.0; 4];
        series.wind_dir_deg = vec![180.0; 4];
        series.opaque_sky_cover = vec![5.0; 4];
        series.horizontal_infrared_w_m2 = vec![300.0; 4];
        series.sky_temp_c = vec![2.0; 4];
        series.ground_temp_c = vec![10.0; 4];
        series.liquid_precip_m = vec![0.0; 4];
        series.surface_albedo = Some(vec![0.2, 0.7, 0.7, 0.3]);
        let down = series.resample(3600).expect("downsample should succeed");
        let albedo = down.surface_albedo.as_ref().expect("albedo should be Some");
        assert_eq!(albedo.len(), 1);
        // Mean of [0.2, 0.7, 0.7, 0.3] = 0.475
        assert!((albedo[0] - 0.475).abs() < 1e-12);
    }

    // --- PCHIP unit tests ---

    #[test]
    fn pchip_single_element_replicates() {
        let out = pchip_resample(&[42.0], 4);
        assert_eq!(out.len(), 4);
        assert!(out.iter().all(|&v| v == 42.0));
    }

    #[test]
    fn pchip_two_elements_linear() {
        let out = pchip_resample(&[0.0, 10.0], 4);
        assert_eq!(out.len(), 8);
        // Linear interp with flat extrapolation: last 3 values hold at 10.0.
        let expected = [0.0, 2.5, 5.0, 7.5, 10.0, 10.0, 10.0, 10.0];
        for (i, (&got, &exp)) in out.iter().zip(expected.iter()).enumerate() {
            assert!(
                (got - exp).abs() < 1e-12,
                "index {i}: expected {exp}, got {got}"
            );
        }
    }

    #[test]
    fn pchip_interpolates_through_knots() {
        let values = [1.0, 4.0, 2.0, 5.0, 3.0];
        let out = pchip_resample(&values, 3);
        assert_eq!(out.len(), 15);
        // Every `factor`-th sample must equal the original knot.
        for (k, &v) in values.iter().enumerate() {
            assert!(
                (out[k * 3] - v).abs() < 1e-12,
                "knot {k}: expected {v}, got {}",
                out[k * 3]
            );
        }
    }

    #[test]
    fn pchip_preserves_monotonicity_in_monotone_run() {
        // Strictly increasing data: output should be non-decreasing.
        let values = [0.0, 1.0, 3.0, 6.0, 10.0];
        let out = pchip_resample(&values, 10);
        for w in out.windows(2) {
            assert!(
                w[1] >= w[0] - 1e-12,
                "monotonicity violated: {} > {}",
                w[0],
                w[1]
            );
        }
        // Year boundary: last output sample must equal last knot (flat extrapolation).
        assert!(
            (out[out.len() - 1] - 10.0).abs() < 1e-12,
            "last sample must equal last knot, got {}",
            out[out.len() - 1]
        );
    }

    #[test]
    fn pchip_nan_propagation() {
        let values = [1.0, f64::NAN, 3.0];
        let out = pchip_resample(&values, 2);
        // NaN at knot 1 contaminates slopes d[0] and d[1], which means:
        // - out[0] is NaN (d[0] is NaN, h10*NaN contaminates even at t=0)
        // - out[1..3] are NaN (interval touching NaN knot)
        assert!(
            out[0].is_nan(),
            "expected NaN at index 0 (slope contamination)"
        );
        assert!(out[1].is_nan(), "expected NaN at index 1");
        assert!(out[2].is_nan(), "expected NaN at index 2");
        assert!(out[3].is_nan(), "expected NaN at index 3");
    }

    #[test]
    fn pchip_factor_one_returns_original() {
        let values = [1.0, 2.0, 3.0];
        let out = pchip_resample(&values, 1);
        assert_eq!(out, values);
    }

    #[test]
    fn fritsch_carlson_flat_segment_slopes_zero() {
        let y = [5.0, 5.0, 5.0, 5.0];
        let d = fritsch_carlson_slopes(&y);
        for (i, &s) in d.iter().enumerate() {
            assert!(s.abs() < 1e-15, "slope at knot {i} should be 0, got {s}");
        }
    }

    #[test]
    fn fritsch_carlson_linear_data() {
        // For perfectly linear data, slopes should equal the constant secant.
        let y = [2.0, 4.0, 6.0, 8.0, 10.0];
        let d = fritsch_carlson_slopes(&y);
        for (i, &s) in d.iter().enumerate() {
            assert!(
                (s - 2.0).abs() < 1e-12,
                "slope at knot {i}: expected 2.0, got {s}"
            );
        }
    }

    #[test]
    fn resample_clamps_rel_humidity() {
        let mut series = sample_series_5pt();
        // Non-monotonic extrema to exercise PCHIP overshoot + clamping.
        series.rel_humidity_pct = vec![99.0, 1.0, 99.0, 1.0, 99.0];
        let resampled = series.resample(600).expect("resample should succeed");
        for &v in &resampled.rel_humidity_pct {
            assert!((0.0..=100.0).contains(&v), "RH out of bounds: {v}");
        }
    }

    #[test]
    fn resample_clamps_opaque_sky_cover() {
        let mut series = sample_series_5pt();
        // Non-monotonic extrema to exercise PCHIP overshoot + clamping.
        series.opaque_sky_cover = vec![9.5, 0.5, 9.5, 0.5, 9.5];
        let resampled = series.resample(600).expect("resample should succeed");
        for &v in &resampled.opaque_sky_cover {
            assert!((0.0..=10.0).contains(&v), "sky cover out of bounds: {v}");
        }
    }

    #[test]
    fn resample_solar_fields_still_zoh() {
        let series = sample_series();
        let resampled = series.resample(60).expect("resample should succeed");
        // GHI uses ZOH: first 60 slots are 0.0, next 60 are 500.0.
        assert!(resampled.ghi_w_m2.iter().take(60).all(|&x| x == 0.0));
        assert!(resampled.ghi_w_m2.iter().skip(60).all(|&x| x == 500.0));
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

    /// Verify that sky temperature is recomputed from interpolated inputs after
    /// upsampling, not interpolated directly.  This is the B5 fix: T_sky is a
    /// non-linear function of its inputs, so direct interpolation violates the
    /// chain rule (E+ WeatherManager.cc:3113).
    #[test]
    fn sky_temp_recomputed_after_upsampling() {
        use crate::epw::compute_sky_temp_c;

        let mut series = sample_series_5pt();
        // Set IR, dry-bulb, dew-point, and sky cover to values where
        // compute_sky_temp_c produces a specific cascade outcome.
        // Use strong IR (>= 50 W/m²) so the Stefan-Boltzmann path is taken,
        // since that's the most nonlinear (4th root).
        series.horizontal_infrared_w_m2 = vec![200.0, 250.0, 300.0, 250.0, 200.0];
        series.dry_bulb_c = vec![10.0, 12.0, 15.0, 13.0, 11.0];
        series.dew_point_c = vec![5.0, 6.0, 8.0, 7.0, 5.0];
        series.opaque_sky_cover = vec![4.0, 5.0, 6.0, 5.0, 4.0];

        // Compute the "original" sky temps to seed the series.
        series.sky_temp_c = series
            .horizontal_infrared_w_m2
            .iter()
            .zip(&series.dry_bulb_c)
            .zip(&series.dew_point_c)
            .zip(&series.opaque_sky_cover)
            .map(|(((&ir, &db), &dp), &osc)| compute_sky_temp_c(ir, db, dp, osc))
            .collect();

        // Upsample by factor 6 (3600s → 600s).
        let resampled = series.resample(600).expect("resample should succeed");
        assert_eq!(resampled.len(), 30);

        // Verify every resampled sky_temp_c matches compute_sky_temp_c applied
        // to the corresponding resampled inputs.
        for i in 0..resampled.len() {
            let expected = compute_sky_temp_c(
                resampled.horizontal_infrared_w_m2[i],
                resampled.dry_bulb_c[i],
                resampled.dew_point_c[i],
                resampled.opaque_sky_cover[i],
            );
            assert!(
                (resampled.sky_temp_c[i] - expected).abs() < 1e-9,
                "slot {i}: recomputed sky_temp_c must equal compute_sky_temp_c \
                 from interpolated inputs; got {}, expected {}",
                resampled.sky_temp_c[i],
                expected,
            );
        }
    }

    /// Same check for downsampling: sky_temp_c must be recomputed from
    /// the mean-downsampled inputs, not just averaged.
    #[test]
    fn sky_temp_recomputed_after_downsampling() {
        use crate::epw::compute_sky_temp_c;

        let mut series = sample_series_5pt();
        series.meta.source_step_secs = 600; // 10-min source
        series.horizontal_infrared_w_m2 = vec![200.0, 250.0, 300.0, 350.0, 200.0];
        series.dry_bulb_c = vec![10.0, 12.0, 15.0, 13.0, 11.0];
        series.dew_point_c = vec![5.0, 6.0, 8.0, 7.0, 5.0];
        series.opaque_sky_cover = vec![4.0, 5.0, 6.0, 5.0, 4.0];

        // Seed sky_temp_c from inputs.
        series.sky_temp_c = series
            .horizontal_infrared_w_m2
            .iter()
            .zip(&series.dry_bulb_c)
            .zip(&series.dew_point_c)
            .zip(&series.opaque_sky_cover)
            .map(|(((&ir, &db), &dp), &osc)| compute_sky_temp_c(ir, db, dp, osc))
            .collect();

        // Downsample to 3600s (factor 6 → 1 output row).
        // But 5 rows / 6 is not integer, so use 4-element series at 900s → 3600s.
        series.dry_bulb_c = vec![10.0, 12.0, 15.0, 13.0];
        series.dew_point_c = vec![5.0, 6.0, 8.0, 7.0];
        series.horizontal_infrared_w_m2 = vec![200.0, 300.0, 350.0, 250.0];
        series.opaque_sky_cover = vec![4.0, 5.0, 6.0, 5.0];
        series.rel_humidity_pct = vec![40.0, 50.0, 60.0, 50.0];
        series.pressure_kpa = vec![90.0, 91.0, 90.5, 91.5];
        series.ghi_w_m2 = vec![100.0, 200.0, 300.0, 400.0];
        series.dni_w_m2 = vec![0.0; 4];
        series.dhi_w_m2 = vec![0.0; 4];
        series.wind_speed_m_s = vec![2.0; 4];
        series.wind_dir_deg = vec![180.0; 4];
        series.ground_temp_c = vec![10.0, 10.1, 10.2, 10.3];
        series.liquid_precip_m = vec![0.0; 4];
        series.meta.source_step_secs = 900;

        // Seed sky_temp from inputs again.
        series.sky_temp_c = series
            .horizontal_infrared_w_m2
            .iter()
            .zip(&series.dry_bulb_c)
            .zip(&series.dew_point_c)
            .zip(&series.opaque_sky_cover)
            .map(|(((&ir, &db), &dp), &osc)| compute_sky_temp_c(ir, db, dp, osc))
            .collect();

        let down = series.resample(3600).expect("downsample should succeed");
        assert_eq!(down.len(), 1);

        let expected = compute_sky_temp_c(
            down.horizontal_infrared_w_m2[0],
            down.dry_bulb_c[0],
            down.dew_point_c[0],
            down.opaque_sky_cover[0],
        );
        assert!(
            (down.sky_temp_c[0] - expected).abs() < 1e-9,
            "downsampled sky_temp_c must equal compute_sky_temp_c from \
             mean-downsampled inputs; got {}, expected {}",
            down.sky_temp_c[0],
            expected,
        );
    }

    // --- CircularLinear resampling tests (B9 fix) ---

    #[test]
    fn circular_linear_normal_interpolation() {
        // 180° → 190°: no wrap-around, should behave like linear.
        let out = super::circular_linear_resample(&[180.0, 190.0], 6);
        assert_eq!(out.len(), 12);
        // First sample is exactly 180°.
        assert!((out[0] - 180.0).abs() < 1e-12);
        // Halfway between 180 and 190 = 185°.
        assert!((out[3] - 185.0).abs() < 1e-12);
        // Second knot hit.
        assert!((out[6] - 190.0).abs() < 1e-12);
        // Beyond last knot: flat hold at 190°.
        assert!((out[11] - 190.0).abs() < 1e-12);
    }

    #[test]
    fn circular_linear_wrap_350_to_10() {
        // 350° → 10°: the shortest arc goes through 0° (delta = +20°),
        // NOT through 180° (which would be delta = -340° or +20° the wrong way).
        let out = super::circular_linear_resample(&[350.0, 10.0], 6);
        assert_eq!(out.len(), 12);
        // First sample: 350°.
        assert!((out[0] - 350.0).abs() < 1e-12);
        // Midpoint: 350 + 0.5 * 20 = 360 → 360 % 360 = 0°.
        assert!((out[3] - 0.0).abs() < 1e-12, "midpoint should be 0°, got {}", out[3]);
        // Second knot: 10°.
        assert!((out[6] - 10.0).abs() < 1e-12);
        // Verify all values stay in [0, 360).
        for (i, &v) in out.iter().enumerate() {
            assert!(v >= 0.0 && v < 360.0, "out[{i}] = {v} out of [0, 360)");
        }
        // Verify monotonic increase along the short arc: 350→353→357→0→3→7→10.
        // The raw angles increase (350, 353.33, 356.67, 0, 3.33, 6.67, 10).
        // After modulo, there's a wrap at the midpoint. Check using circular distance.
        for w in out.windows(2) {
            // Use rem_euclid (not %) to handle negative numerators correctly.
            let circ_delta = ((w[1] - w[0] + 180.0).rem_euclid(360.0)) - 180.0;
            assert!(
                circ_delta >= -1e-9,
                "non-monotonic along short arc: {} → {}, circular delta = {circ_delta}",
                w[0], w[1]
            );
        }
    }

    #[test]
    fn circular_linear_reverse_wrap_10_to_350() {
        // 10° → 350°: shortest arc goes backward through 0° (delta = -20°).
        let out = super::circular_linear_resample(&[10.0, 350.0], 6);
        assert_eq!(out.len(), 12);
        // First sample: 10°.
        assert!((out[0] - 10.0).abs() < 1e-12);
        // Midpoint: 10 + 0.5 * (-20) = 0 → 0°.
        assert!((out[3] - 0.0).abs() < 1e-12, "midpoint should be 0°, got {}", out[3]);
        // Second knot: 350°.
        assert!((out[6] - 350.0).abs() < 1e-12);
        // Verify monotonic decrease along the short arc.
        for w in out.windows(2) {
            let circ_delta = ((w[1] - w[0] + 180.0).rem_euclid(360.0)) - 180.0;
            assert!(
                circ_delta <= 1e-9,
                "non-monotonic along short arc: {} → {}, circular delta = {circ_delta}",
                w[0], w[1]
            );
        }
    }

    #[test]
    fn circular_linear_constant_wind() {
        // All same value: should produce that value everywhere.
        let out = super::circular_linear_resample(&[270.0, 270.0, 270.0], 4);
        assert_eq!(out.len(), 12);
        for (i, &v) in out.iter().enumerate() {
            assert!((v - 270.0).abs() < 1e-12, "out[{i}] = {v}, expected 270.0");
        }
    }

    #[test]
    fn circular_linear_nan_propagates_as_zoh() {
        // NaN is a sentinel: interval touching NaN falls back to ZOH.
        let out = super::circular_linear_resample(&[180.0, f64::NAN, 200.0], 2);
        assert_eq!(out.len(), 6);
        // Interval [180, NaN]: ZOH from 180.
        assert!((out[0] - 180.0).abs() < 1e-12);
        assert!((out[1] - 180.0).abs() < 1e-12);
        // Interval [NaN, 200]: ZOH from NaN.
        assert!(out[2].is_nan(), "expected NaN at index 2");
        assert!(out[3].is_nan(), "expected NaN at index 3");
        // Interval [200, ...] (only 3 input values, so last interval is [NaN, 200]
        // already covered. Actually with 3 values and factor 2:
        // i=0: k=0, frac=0 → 180
        // i=1: k=0, frac=0.5 → ZOH because b=NaN → 180
        // i=2: k=1, frac=0 → NaN
        // i=3: k=1, frac=0.5 → ZOH because a=NaN → NaN
        // i=4: k=1, frac=1.0 (clamped) → ZOH because a=NaN → NaN
        // i=5: k=1, frac=1.5 (clamped to 1.0) → ZOH because a=NaN → NaN
    }

    #[test]
    fn circular_linear_sentinel_minus_9999_treated_as_zoh() {
        // -9999 sentinel (TMY3 missing): interval touching it falls back to ZOH.
        let out = super::circular_linear_resample(&[180.0, -9999.0, 200.0], 2);
        assert_eq!(out.len(), 6);
        // Interval [180, -9999]: ZOH from 180.
        assert!((out[0] - 180.0).abs() < 1e-12);
        assert!((out[1] - 180.0).abs() < 1e-12);
        // Interval [-9999, 200]: ZOH from -9999 (propagates sentinel).
        assert!((out[2] - (-9999.0)).abs() < 1e-12, "expected -9999 at index 2");
        assert!((out[3] - (-9999.0)).abs() < 1e-12, "expected -9999 at index 3");
    }

    #[test]
    fn circular_linear_360_treated_as_invalid() {
        // 360.0 is NOT in [0, 360) so it's treated as sentinel → ZOH.
        let out = super::circular_linear_resample(&[350.0, 360.0], 2);
        assert_eq!(out.len(), 4);
        // Both sub-samples in interval [350, 360]: ZOH from 350 (b is invalid).
        assert!((out[0] - 350.0).abs() < 1e-12);
        assert!((out[1] - 350.0).abs() < 1e-12);
    }

    #[test]
    fn circular_linear_single_element_replicates() {
        let out = super::circular_linear_resample(&[45.0], 4);
        assert_eq!(out.len(), 4);
        assert!(out.iter().all(|&v| (v - 45.0).abs() < 1e-12));
    }

    #[test]
    fn circular_linear_empty_returns_empty() {
        let out = super::circular_linear_resample(&[], 4);
        assert!(out.is_empty());
    }

    #[test]
    fn circular_linear_factor_one_returns_original() {
        let values = [10.0, 350.0, 180.0];
        let out = super::circular_linear_resample(&values, 1);
        assert_eq!(out, values);
    }

    #[test]
    fn circular_linear_multi_segment_with_wrap() {
        // 350 → 10 (wrap through 0), then 10 → 30 (normal).
        let out = super::circular_linear_resample(&[350.0, 10.0, 30.0], 6);
        assert_eq!(out.len(), 18);
        // First segment: 350 → 10 through 0.
        assert!((out[0] - 350.0).abs() < 1e-12);
        assert!((out[3] - 0.0).abs() < 1e-12, "midpoint of first segment: got {}", out[3]);
        assert!((out[6] - 10.0).abs() < 1e-12);
        // Second segment: 10 → 30 (normal, delta = +20).
        assert!((out[12] - 30.0).abs() < 1e-12);
        // Midpoint of second segment: 20°.
        assert!((out[9] - 20.0).abs() < 1e-12, "midpoint of second segment: got {}", out[9]);
    }
}
