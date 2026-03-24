//! PV array configuration: parsing, validation, and surface-ID mapping.

use hares_types::HaresError;
use serde::{Deserialize, Serialize};

use crate::EquipmentConfig;

use super::{
    DEFAULT_NOCT_C, KEY_ARRAY_AZIMUTH_DEG, KEY_ARRAY_COUNT, KEY_ARRAY_TILT_DEG, KEY_AZIMUTH_DEG,
    KEY_CAPACITY_KW, KEY_MODULE_TYPE, KEY_NOCT_C, KEY_SAM_LUT_PATH, KEY_SYSTEM_SIZE_KW,
    KEY_TILT_DEG,
};

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

    pub(crate) fn from_config(config: &EquipmentConfig, key: &str) -> Self {
        config
            .get_str(key)
            .map(Self::from_str)
            .unwrap_or(Self::Standard)
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
}

impl PvArray {
    pub(crate) fn from_single_config(config: &EquipmentConfig) -> Result<Self, HaresError> {
        let capacity_kw = config
            .get_f64(KEY_CAPACITY_KW)
            .or_else(|| config.get_f64(KEY_SYSTEM_SIZE_KW))
            .ok_or_else(|| {
                HaresError::Equipment(
                    "PV array requires capacity_kw or system_size_kw (must be > 0)".into(),
                )
            })?;
        let tilt_deg = config
            .get_f64(KEY_TILT_DEG)
            .or_else(|| config.get_f64(KEY_ARRAY_TILT_DEG))
            .unwrap_or(30.0);
        let azimuth_deg = config
            .get_f64(KEY_AZIMUTH_DEG)
            .or_else(|| config.get_f64(KEY_ARRAY_AZIMUTH_DEG))
            .unwrap_or(180.0);
        let module_type = ModuleType::from_config(config, KEY_MODULE_TYPE);
        let noct_c = config.get_f64(KEY_NOCT_C).unwrap_or(DEFAULT_NOCT_C);
        let sam_lut_path = config.get_str(KEY_SAM_LUT_PATH).map(ToOwned::to_owned);
        Self::validate(capacity_kw, tilt_deg, azimuth_deg, noct_c)?;
        Ok(Self {
            tilt_deg,
            azimuth_deg,
            capacity_kw,
            noct_c,
            module_type,
            surface_id: None,
            sam_lut_path,
        })
    }

    pub(crate) fn from_indexed_config(
        config: &EquipmentConfig,
        idx: usize,
    ) -> Result<Self, HaresError> {
        let key = |base: &str| format!("array_{idx}_{base}");
        let capacity_kw = config
            .get_f64(&key(KEY_CAPACITY_KW))
            .or_else(|| config.get_f64(&key(KEY_SYSTEM_SIZE_KW)))
            .ok_or_else(|| {
                HaresError::Equipment(format!(
                    "PV array {idx} requires capacity_kw or system_size_kw (must be > 0)"
                ))
            })?;
        let tilt_deg = config
            .get_f64(&key(KEY_TILT_DEG))
            .or_else(|| config.get_f64(&key(KEY_ARRAY_TILT_DEG)))
            .unwrap_or(30.0);
        let azimuth_deg = config
            .get_f64(&key(KEY_AZIMUTH_DEG))
            .or_else(|| config.get_f64(&key(KEY_ARRAY_AZIMUTH_DEG)))
            .unwrap_or(180.0);
        let module_type = config
            .get_str(&key(KEY_MODULE_TYPE))
            .map(ModuleType::from_str)
            .unwrap_or(ModuleType::Standard);
        let noct_c = config
            .get_f64(&key(KEY_NOCT_C))
            .or_else(|| config.get_f64(KEY_NOCT_C))
            .unwrap_or(DEFAULT_NOCT_C);
        let sam_lut_path = config
            .get_str(&key(KEY_SAM_LUT_PATH))
            .or_else(|| config.get_str(KEY_SAM_LUT_PATH))
            .map(ToOwned::to_owned);
        Self::validate(capacity_kw, tilt_deg, azimuth_deg, noct_c)?;
        Ok(Self {
            tilt_deg,
            azimuth_deg,
            capacity_kw,
            noct_c,
            module_type,
            surface_id: None,
            sam_lut_path,
        })
    }

    fn validate(
        capacity_kw: f64,
        tilt_deg: f64,
        azimuth_deg: f64,
        noct_c: f64,
    ) -> Result<(), HaresError> {
        if !capacity_kw.is_finite() || capacity_kw <= 0.0 {
            return Err(HaresError::Equipment(
                "PV capacity_kw must be finite and > 0".to_string(),
            ));
        }
        if !tilt_deg.is_finite() || !(0.0..=180.0).contains(&tilt_deg) {
            return Err(HaresError::Equipment(
                "PV tilt_deg must be finite and within [0, 180]".to_string(),
            ));
        }
        if !azimuth_deg.is_finite() {
            return Err(HaresError::Equipment(
                "PV azimuth_deg must be finite".to_string(),
            ));
        }
        if !noct_c.is_finite() {
            return Err(HaresError::Equipment(
                "PV noct_c must be finite".to_string(),
            ));
        }
        Ok(())
    }
}

pub(crate) fn parse_arrays_from_config(
    config: &EquipmentConfig,
) -> Result<Vec<PvArray>, HaresError> {
    let count = parse_usize_from_f64(config.get_f64(KEY_ARRAY_COUNT))?.unwrap_or(0);
    if count == 0 {
        return PvArray::from_single_config(config).map(|arr| vec![arr]);
    }

    let mut arrays = Vec::with_capacity(count);
    for idx in 0..count {
        arrays.push(PvArray::from_indexed_config(config, idx)?);
    }
    Ok(arrays)
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

fn parse_usize_from_f64(v: Option<f64>) -> Result<Option<usize>, HaresError> {
    let Some(raw) = v else {
        return Ok(None);
    };
    if !raw.is_finite() || raw < 0.0 {
        return Err(HaresError::Equipment(
            "PV array_count must be finite and non-negative".to_string(),
        ));
    }
    let rounded = raw.round();
    if (rounded - raw).abs() > 1e-6 {
        return Err(HaresError::Equipment(
            "PV array_count must be an integer value".to_string(),
        ));
    }
    if rounded > (usize::MAX as f64) {
        return Err(HaresError::Equipment(
            "PV array_count exceeds supported range".to_string(),
        ));
    }
    Ok(Some(rounded as usize))
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
