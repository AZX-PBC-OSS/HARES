//! PV array configuration: parsing, validation, and surface-ID mapping.

use hares_types::HaresError;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModuleType {
    Standard,
    Premium,
    ThinFilm,
}

impl ModuleType {
    pub(crate) fn from_str(s: &str) -> Self {
        match s.trim() {
            "Premium" => Self::Premium,
            "ThinFilm" => Self::ThinFilm,
            _ => Self::Standard,
        }
    }

    /// Temperature coefficient of power (gamma) per °C, matching PVWatts v8 defaults.
    pub(crate) fn gamma_per_c(self) -> f64 {
        match self {
            Self::Standard => super::DEFAULT_GAMMA_PER_C,
            Self::Premium => -0.0035,
            Self::ThinFilm => -0.0020,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PvArray {
    pub tilt_deg: f64,
    pub azimuth_deg: f64,
    pub capacity_kw: f64,
    pub noct_c: f64,
    pub module_type: ModuleType,
    pub surface_id: Option<u32>,
    pub sam_lut_path: Option<String>,
    /// Envelope boundary index this array is attached to (for roof shading).
    pub attached_boundary_id: Option<u32>,
}

pub(super) fn parse_u32_from_f64(v: Option<f64>) -> Option<u32> {
    let raw = v?;
    if !raw.is_finite() || raw < 0.0 || raw > (u32::MAX as f64) {
        return None;
    }
    let rounded = raw.round();
    if (rounded - raw).abs() > 1e-6 {
        return None;
    }
    Some(rounded as u32)
}

fn normalize_azimuth(mut azimuth_deg: f64) -> f64 {
    azimuth_deg = azimuth_deg.rem_euclid(360.0);
    if azimuth_deg == 360.0 {
        0.0
    } else {
        azimuth_deg
    }
}

fn round_to_resolution(value: f64, resolution: f64) -> f64 {
    (value / resolution).round() * resolution
}

pub fn surface_id_for_orientation(
    tilt_deg: f64,
    azimuth_deg: f64,
    resolution_deg: f64,
) -> Result<u32, HaresError> {
    if !resolution_deg.is_finite() || resolution_deg <= 0.0 {
        return Err(HaresError::Equipment(
            "surface resolution must be finite and > 0".to_string(),
        ));
    }
    if !tilt_deg.is_finite() || !azimuth_deg.is_finite() {
        return Err(HaresError::Equipment(
            "surface orientation values must be finite".to_string(),
        ));
    }

    let rounded_tilt = round_to_resolution(tilt_deg.clamp(0.0, 180.0), resolution_deg);
    let rounded_az = round_to_resolution(normalize_azimuth(azimuth_deg), resolution_deg);
    let tilt_centideg = (rounded_tilt * 100.0).round() as u32;
    let az_centideg = (rounded_az * 100.0).round() as u32;

    Ok(tilt_centideg * 100_000 + az_centideg)
}
