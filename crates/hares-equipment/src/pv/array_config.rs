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
        let normalized: String = s
            .trim()
            .chars()
            .filter(|c| !c.is_ascii_whitespace() && *c != '_')
            .flat_map(|c| c.to_lowercase())
            .collect();

        match normalized.as_str() {
            "premium" => Self::Premium,
            "thinfilm" => Self::ThinFilm,
            "standard" => Self::Standard,
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

/// Per-array configuration specification (serde-compatible).
///
/// This is the config-time representation; `PvArray` is the runtime
/// struct populated from this spec during initialisation. When `PvConfig`
/// carries an `arrays` field, each `PvArraySpec` maps to one `PvArray`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PvArraySpec {
    pub capacity_kw: f64,
    #[serde(default)]
    pub tilt_deg: Option<f64>,
    #[serde(default)]
    pub azimuth_deg: Option<f64>,
    #[serde(default)]
    pub module_type: Option<String>,
    #[serde(default)]
    pub noct_c: Option<f64>,
    #[serde(default)]
    pub sam_lut_path: Option<String>,
    #[serde(default)]
    pub attached_boundary_id: Option<u32>,
}

impl PvArraySpec {
    /// Validate per-array fields for physical plausibility.
    pub fn validate(&self) -> crate::Result<()> {
        use hares_types::HaresError;
        if !self.capacity_kw.is_finite() || self.capacity_kw <= 0.0 {
            return Err(HaresError::Equipment(
                "PvArraySpec capacity_kw must be finite and > 0".to_string(),
            ));
        }
        if let Some(tilt) = self.tilt_deg {
            // EnergyPlus PVWatts.cc:118-119 validates tilt ∈ [0, 90].
            if !tilt.is_finite() || !(0.0..=90.0).contains(&tilt) {
                return Err(HaresError::Equipment(
                    "PvArraySpec tilt_deg must be finite and within [0, 90]".to_string(),
                ));
            }
        }
        if let Some(az) = self.azimuth_deg {
            if !az.is_finite() || !(0.0..=360.0).contains(&az) {
                return Err(HaresError::Equipment(
                    "PvArraySpec azimuth_deg must be finite and within [0, 360]".to_string(),
                ));
            }
        }
        Ok(())
    }
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

#[cfg(test)]
mod tests {
    use super::ModuleType;

    #[test]
    fn module_type_parsing_is_case_insensitive_and_accepts_aliases() {
        assert_eq!(ModuleType::from_str("Premium"), ModuleType::Premium);
        assert_eq!(ModuleType::from_str("premium"), ModuleType::Premium);
        assert_eq!(ModuleType::from_str("PREMIUM"), ModuleType::Premium);

        assert_eq!(ModuleType::from_str("ThinFilm"), ModuleType::ThinFilm);
        assert_eq!(ModuleType::from_str("thin film"), ModuleType::ThinFilm);
        assert_eq!(ModuleType::from_str("thin_film"), ModuleType::ThinFilm);
        assert_eq!(ModuleType::from_str("thinfilm"), ModuleType::ThinFilm);

        assert_eq!(ModuleType::from_str("Standard"), ModuleType::Standard);
        assert_eq!(ModuleType::from_str("standard"), ModuleType::Standard);
        assert_eq!(ModuleType::from_str("unknown"), ModuleType::Standard);
    }

    #[test]
    fn module_type_gamma_matches_pvwatts_defaults() {
        assert_eq!(
            ModuleType::Standard.gamma_per_c(),
            crate::pv::DEFAULT_GAMMA_PER_C
        );
        assert_eq!(ModuleType::Premium.gamma_per_c(), -0.0035);
        assert_eq!(ModuleType::ThinFilm.gamma_per_c(), -0.0020);
    }

    // --- PvArraySpec tests ---

    use super::PvArraySpec;

    fn minimal_array_spec() -> PvArraySpec {
        PvArraySpec {
            capacity_kw: 5.0,
            tilt_deg: Some(30.0),
            azimuth_deg: Some(180.0),
            module_type: None,
            noct_c: None,
            sam_lut_path: None,
            attached_boundary_id: None,
        }
    }

    #[test]
    fn array_spec_validate_passes_for_valid_spec() {
        assert!(minimal_array_spec().validate().is_ok());
    }

    #[test]
    fn array_spec_validate_rejects_zero_capacity() {
        let mut spec = minimal_array_spec();
        spec.capacity_kw = 0.0;
        assert!(spec.validate().is_err());
    }

    #[test]
    fn array_spec_validate_rejects_negative_capacity() {
        let mut spec = minimal_array_spec();
        spec.capacity_kw = -1.0;
        assert!(spec.validate().is_err());
    }

    #[test]
    fn array_spec_validate_rejects_tilt_above_90() {
        let mut spec = minimal_array_spec();
        spec.tilt_deg = Some(91.0);
        assert!(spec.validate().is_err());
    }

    #[test]
    fn array_spec_validate_rejects_azimuth_below_0() {
        let mut spec = minimal_array_spec();
        spec.azimuth_deg = Some(-1.0);
        assert!(spec.validate().is_err());
    }

    #[test]
    fn array_spec_validate_accepts_azimuth_360_as_it_normalizes_to_0() {
        let mut spec = minimal_array_spec();
        spec.azimuth_deg = Some(360.0);
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn array_spec_validate_accepts_azimuth_0() {
        let mut spec = minimal_array_spec();
        spec.azimuth_deg = Some(0.0);
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn array_spec_validate_accepts_azimuth_359_9() {
        let mut spec = minimal_array_spec();
        spec.azimuth_deg = Some(359.9);
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn array_spec_round_trips_via_serde_json() {
        let spec = minimal_array_spec();
        let json = serde_json::to_string(&spec).unwrap();
        let recovered: PvArraySpec = serde_json::from_str(&json).unwrap();
        assert_eq!(recovered.capacity_kw, spec.capacity_kw);
        assert_eq!(recovered.tilt_deg, spec.tilt_deg);
        assert_eq!(recovered.azimuth_deg, spec.azimuth_deg);
    }

    #[test]
    fn array_spec_defaults_absent_fields_to_none() {
        let json = serde_json::json!({"capacity_kw": 3.0});
        let spec: PvArraySpec = serde_json::from_value(json).unwrap();
        assert_eq!(spec.capacity_kw, 3.0);
        assert_eq!(spec.tilt_deg, None);
        assert_eq!(spec.azimuth_deg, None);
        assert_eq!(spec.module_type, None);
    }
}
