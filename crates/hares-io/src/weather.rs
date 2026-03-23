//! Weather data processing and time-series handling.

use thiserror::Error;

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
    pub fn resample(&self, target_step_secs: u32) -> Result<Self, WeatherError> {
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

            Ok(Self {
                meta: WeatherMeta {
                    source_step_secs: target_step_secs,
                    ..self.meta.clone()
                },
                // Continuous instantaneous fields → PCHIP.
                dry_bulb_c: pchip_resample(&self.dry_bulb_c, factor),
                dew_point_c: pchip_resample(&self.dew_point_c, factor),
                rel_humidity_pct,
                pressure_kpa: pchip_resample(&self.pressure_kpa, factor),
                horizontal_infrared_w_m2: pchip_resample(
                    &self.horizontal_infrared_w_m2,
                    factor,
                ),
                sky_temp_c: pchip_resample(&self.sky_temp_c, factor),
                ground_temp_c: pchip_resample(&self.ground_temp_c, factor),
                opaque_sky_cover,
                // Period-average energy flux → ZOH.
                ghi_w_m2: replicate_zoh(&self.ghi_w_m2, factor),
                dni_w_m2: replicate_zoh(&self.dni_w_m2, factor),
                dhi_w_m2: replicate_zoh(&self.dhi_w_m2, factor),
                // Turbulent/stochastic → ZOH.
                wind_speed_m_s: replicate_zoh(&self.wind_speed_m_s, factor),
                wind_dir_deg: replicate_zoh(&self.wind_dir_deg, factor),
                // Accumulated depth → distribute evenly so downstream sums are preserved.
                liquid_precip_m: distribute_accumulated(&self.liquid_precip_m, factor),
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

            Ok(Self {
                meta: WeatherMeta {
                    source_step_secs: target_step_secs,
                    ..self.meta.clone()
                },
                // Instantaneous fields → mean.
                dry_bulb_c: mean_downsample(&self.dry_bulb_c, ratio),
                dew_point_c: mean_downsample(&self.dew_point_c, ratio),
                rel_humidity_pct: mean_downsample(&self.rel_humidity_pct, ratio),
                pressure_kpa: mean_downsample(&self.pressure_kpa, ratio),
                horizontal_infrared_w_m2: mean_downsample(
                    &self.horizontal_infrared_w_m2,
                    ratio,
                ),
                sky_temp_c: mean_downsample(&self.sky_temp_c, ratio),
                ground_temp_c: mean_downsample(&self.ground_temp_c, ratio),
                opaque_sky_cover: mean_downsample(&self.opaque_sky_cover, ratio),
                wind_speed_m_s: mean_downsample(&self.wind_speed_m_s, ratio),
                wind_dir_deg: mean_downsample(&self.wind_dir_deg, ratio),
                // Solar fields → mean (average irradiance preserves energy).
                ghi_w_m2: mean_downsample(&self.ghi_w_m2, ratio),
                dni_w_m2: mean_downsample(&self.dni_w_m2, ratio),
                dhi_w_m2: mean_downsample(&self.dhi_w_m2, ratio),
                // Accumulated depth → sum.
                liquid_precip_m: sum_downsample(&self.liquid_precip_m, ratio),
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
        // Clamp frac to [0, 1] for flat extrapolation beyond the last knot.
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
        }
    }

    #[test]
    fn get_returns_column_value() {
        let series = sample_series();
        assert_eq!(series.get(WeatherField::WindDirDeg, 1), 190.0);
        assert_eq!(series.get(WeatherField::PressureKpa, 0), 90.0);
    }

    #[test]
    fn resample_pchip_on_dry_bulb_and_zoh_on_wind() {
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

        // Wind fields still use ZOH.
        assert!(resampled.wind_dir_deg.iter().take(60).all(|x| *x == 180.0));
        assert!(resampled.wind_dir_deg.iter().skip(60).all(|x| *x == 190.0));
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
}
