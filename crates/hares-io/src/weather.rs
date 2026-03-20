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
    #[error("EPW parse error: {0}")]
    Parse(String),
    #[error("EPW validation error: {0}")]
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

    /// Zero-order-hold resampling from an hourly source series.
    pub fn resample(&self, target_step_secs: u32) -> Result<Self, WeatherError> {
        if target_step_secs == 0 {
            return Err(WeatherError::Resample(
                "target_step_secs must be > 0".to_string(),
            ));
        }
        if 3600 % target_step_secs != 0 {
            return Err(WeatherError::Resample(format!(
                "incompatible timestep: 3600 % {target_step_secs} != 0"
            )));
        }
        let factor = (3600 / target_step_secs) as usize;

        Ok(Self {
            meta: self.meta.clone(),
            dry_bulb_c: replicate_zoh(&self.dry_bulb_c, factor),
            dew_point_c: replicate_zoh(&self.dew_point_c, factor),
            rel_humidity_pct: replicate_zoh(&self.rel_humidity_pct, factor),
            pressure_kpa: replicate_zoh(&self.pressure_kpa, factor),
            ghi_w_m2: replicate_zoh(&self.ghi_w_m2, factor),
            dni_w_m2: replicate_zoh(&self.dni_w_m2, factor),
            dhi_w_m2: replicate_zoh(&self.dhi_w_m2, factor),
            wind_speed_m_s: replicate_zoh(&self.wind_speed_m_s, factor),
            wind_dir_deg: replicate_zoh(&self.wind_dir_deg, factor),
            opaque_sky_cover: replicate_zoh(&self.opaque_sky_cover, factor),
            horizontal_infrared_w_m2: replicate_zoh(&self.horizontal_infrared_w_m2, factor),
            sky_temp_c: replicate_zoh(&self.sky_temp_c, factor),
            ground_temp_c: replicate_zoh(&self.ground_temp_c, factor),
            liquid_precip_m: replicate_zoh(&self.liquid_precip_m, factor),
        })
    }
}

fn replicate_zoh(values: &[f64], factor: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(values.len().saturating_mul(factor));
    for &value in values {
        out.extend(std::iter::repeat_n(value, factor));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{WeatherError, WeatherField, WeatherMeta, WeatherTimeSeries};

    fn sample_series() -> WeatherTimeSeries {
        WeatherTimeSeries {
            meta: WeatherMeta {
                location: "Test Site".to_string(),
                latitude: 39.74,
                longitude: -104.99,
                timezone_offset_h: -7.0,
                elevation_m: 1600.0,
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

    #[test]
    fn get_returns_column_value() {
        let series = sample_series();
        assert_eq!(series.get(WeatherField::WindDirDeg, 1), 190.0);
        assert_eq!(series.get(WeatherField::PressureKpa, 0), 90.0);
    }

    #[test]
    fn zoh_resample_repeats_each_hourly_value() {
        let series = sample_series();
        let resampled = series.resample(60).expect("resample should succeed");
        assert_eq!(resampled.len(), 120);
        assert!(resampled.dry_bulb_c.iter().take(60).all(|x| *x == 10.0));
        assert!(resampled.dry_bulb_c.iter().skip(60).all(|x| *x == 11.0));
        assert!(resampled.wind_dir_deg.iter().take(60).all(|x| *x == 180.0));
        assert!(resampled.wind_dir_deg.iter().skip(60).all(|x| *x == 190.0));
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
}
