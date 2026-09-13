//! PV array configuration: parsing, validation, and surface-ID mapping.

use hares_types::HaresError;
use serde::{Deserialize, Serialize};

/// PV array mounting type matching SAM's `array_type` parameter.
///
/// SAM PVWatts v8 (`cmod_pvwattsv5.cpp:195-197`) selects the nominal
/// operating cell temperature (NOCT) from the array type:
///
/// - OpenRack (array_type=0) → 45°C
/// - RoofMounted (array_type=1) → 49°C
/// - InsulatedBack (array_type=2) → 49°C
///
/// References:
///   - PVWatts v8 Technical Reference, NREL/TP-7A40-80694
///   - `vendors/EnergyPlus/third_party/ssc/ssc/cmod_pvwattsv5.cpp:195-197`
///   - `vendors/EnergyPlus/third_party/ssc/shared/lib_pvwatts.h:26`
///   - PVPMC: <https://pvpmc.sandia.gov/modeling-guide/2-dc-module-iv/cell-temperature/noct-cell-temperature/>
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArrayType {
    /// Open rack / ground mount. NOCT = 45°C.
    OpenRack,
    /// Roof mounted. NOCT = 49°C.
    RoofMounted,
    /// Insulated back / building-integrated. NOCT = 49°C.
    InsulatedBack,
}

impl ArrayType {
    pub(crate) fn from_str(s: &str) -> Result<Self, HaresError> {
        match crate::config::normalize_config_name(s).as_str() {
            "openrack" => Ok(Self::OpenRack),
            "roofmounted" => Ok(Self::RoofMounted),
            "insulatedback" => Ok(Self::InsulatedBack),
            unrecognised => Err(HaresError::Equipment(format!(
                "Unrecognised array_type '{s}'. Normalised to '{unrecognised}', but expected one of: \
                 OpenRack, RoofMounted, InsulatedBack (case/whitespace/underscore-insensitive)"
            ))),
        }
    }

    /// Default NOCT (°C) for this array type per SAM PVWatts v8.
    pub(crate) fn noct_c(self) -> f64 {
        match self {
            Self::OpenRack => 45.0,
            Self::RoofMounted => 49.0,
            Self::InsulatedBack => 49.0,
        }
    }

    /// SAM `array_type` integer index (0=OpenRack, 1=RoofMounted, 2=InsulatedBack).
    /// Used by the Python LUT adapter when writing `harvest_lut_sam_array_type`
    /// Parquet metadata; without this adapter, the method is dormant but serves
    /// as the canonical mapping definition for the crate.
    // Why: the Python LUT adapter (python/ochre_next/adapters/sam_pv.py) is the
    // intended consumer; until it is updated to write harvest_lut_sam_array_type,
    // this method is unused on the Rust side.
    #[allow(dead_code)]
    pub(crate) fn to_sam_index(self) -> u8 {
        match self {
            Self::OpenRack => 0,
            Self::RoofMounted => 1,
            Self::InsulatedBack => 2,
        }
    }

    /// Decode from SAM `array_type` integer index.
    /// Used by `PvLut::sam_noct_c()` for LUT-path NOCT derivation.
    pub(crate) fn from_sam_index(idx: u8) -> Self {
        match idx {
            0 => Self::OpenRack,
            1 => Self::RoofMounted,
            _ => Self::InsulatedBack,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModuleType {
    Standard,
    Premium,
    ThinFilm,
}

impl ModuleType {
    /// Strict parse: an unrecognised module type is a configuration error,
    /// not `Standard` — the type selects the temperature coefficient of
    /// power, so a typo would silently skew every PV output.
    pub(crate) fn from_str(s: &str) -> Result<Self, HaresError> {
        match crate::config::normalize_config_name(s).as_str() {
            "standard" => Ok(Self::Standard),
            "premium" => Ok(Self::Premium),
            "thinfilm" => Ok(Self::ThinFilm),
            unrecognised => Err(HaresError::Equipment(format!(
                "Unrecognised module_type '{s}'. Normalised to '{unrecognised}', but expected one of: \
                 Standard, Premium, ThinFilm (case/whitespace/underscore-insensitive)"
            ))),
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
    pub array_type: ArrayType,
    pub surface_id: Option<u32>,
    pub sam_lut_path: Option<String>,
    /// Envelope boundary index this array is attached to (for roof shading).
    pub attached_boundary_id: Option<u32>,
}

impl PvArray {
    /// Validate array fields for physical plausibility.
    ///
    /// Enforces:
    /// - `tilt_deg ∈ [0, 180]`, finite, not NaN
    /// - `azimuth_deg ∈ [0, 360)`, finite, not NaN
    /// - `capacity_kw > 0`, finite, not NaN
    /// - `noct_c` finite and positive (> 0)
    pub fn validate(&self) -> Result<(), HaresError> {
        if !self.tilt_deg.is_finite() || !(0.0..=180.0).contains(&self.tilt_deg) {
            return Err(HaresError::Equipment(format!(
                "PvArray tilt_deg must be finite and within [0, 180], got {}",
                self.tilt_deg
            )));
        }
        if !self.azimuth_deg.is_finite() || !(0.0..360.0).contains(&self.azimuth_deg) {
            return Err(HaresError::Equipment(format!(
                "PvArray azimuth_deg must be finite and within [0, 360), got {}",
                self.azimuth_deg
            )));
        }
        if !self.capacity_kw.is_finite() || self.capacity_kw <= 0.0 {
            return Err(HaresError::Equipment(format!(
                "PvArray capacity_kw must be finite and > 0, got {}",
                self.capacity_kw
            )));
        }
        if !self.noct_c.is_finite() || self.noct_c <= 0.0 {
            return Err(HaresError::Equipment(format!(
                "PvArray noct_c must be finite and > 0, got {}",
                self.noct_c
            )));
        }
        Ok(())
    }
}

impl Default for PvArray {
    /// Sensible defaults for PV array geometry matching existing `init_typed()`
    /// fallbacks: 30° tilt (typical residential roof pitch), 180° azimuth
    /// (south-facing in northern hemisphere).
    fn default() -> Self {
        Self {
            tilt_deg: 30.0,
            azimuth_deg: 180.0,
            capacity_kw: 1.0,
            noct_c: super::DEFAULT_NOCT_C,
            module_type: ModuleType::Standard,
            array_type: ArrayType::OpenRack,
            surface_id: None,
            sam_lut_path: None,
            attached_boundary_id: None,
        }
    }
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
    pub array_type: Option<String>,
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

pub(super) fn normalize_azimuth(mut azimuth_deg: f64) -> f64 {
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

    // Catch silent remapping in debug builds: if tilt is out of the expected
    // range, clamp(0, 180) will silently remap to a valid surface_id that
    // may not match the caller's intent. Azimuth is not asserted here because
    // normalize_azimuth correctly handles any finite value (lossless).
    debug_assert!(
        (0.0..=180.0).contains(&tilt_deg),
        "surface_id_for_orientation called with tilt_deg={tilt_deg} outside [0, 180]"
    );

    let rounded_tilt = round_to_resolution(tilt_deg.clamp(0.0, 180.0), resolution_deg);
    let rounded_az = round_to_resolution(normalize_azimuth(azimuth_deg), resolution_deg);
    let tilt_centideg = (rounded_tilt * 100.0).round() as u32;
    let az_centideg = (rounded_az * 100.0).round() as u32;

    Ok(tilt_centideg * 100_000 + az_centideg)
}

#[cfg(test)]
mod tests {
    use super::ArrayType;
    use super::ModuleType;

    #[test]
    fn module_type_parsing_is_case_insensitive_and_accepts_aliases() {
        assert_eq!(
            ModuleType::from_str("Premium").unwrap(),
            ModuleType::Premium
        );
        assert_eq!(
            ModuleType::from_str("premium").unwrap(),
            ModuleType::Premium
        );
        assert_eq!(
            ModuleType::from_str("PREMIUM").unwrap(),
            ModuleType::Premium
        );

        assert_eq!(
            ModuleType::from_str("ThinFilm").unwrap(),
            ModuleType::ThinFilm
        );
        assert_eq!(
            ModuleType::from_str("thin film").unwrap(),
            ModuleType::ThinFilm
        );
        assert_eq!(
            ModuleType::from_str("thin_film").unwrap(),
            ModuleType::ThinFilm
        );
        assert_eq!(
            ModuleType::from_str("thinfilm").unwrap(),
            ModuleType::ThinFilm
        );

        assert_eq!(
            ModuleType::from_str("Standard").unwrap(),
            ModuleType::Standard
        );
        assert_eq!(
            ModuleType::from_str("standard").unwrap(),
            ModuleType::Standard
        );
        // Unrecognised values are configuration errors, not silently Standard:
        // the module type selects the temperature coefficient of power.
        let err = ModuleType::from_str("unknown").expect_err("must reject unknown");
        assert!(
            format!("{err:?}").contains("module_type"),
            "error must name the offending key, got {err:?}"
        );
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
            array_type: None,
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

    // --- PvArray validate() and Default tests ---

    use super::PvArray;

    fn valid_array() -> PvArray {
        PvArray {
            tilt_deg: 30.0,
            azimuth_deg: 180.0,
            capacity_kw: 5.0,
            noct_c: 47.0,
            module_type: ModuleType::Standard,
            array_type: ArrayType::OpenRack,
            surface_id: None,
            sam_lut_path: None,
            attached_boundary_id: None,
        }
    }

    #[test]
    fn pv_array_validate_rejects_negative_tilt() {
        let mut array = valid_array();
        array.tilt_deg = -10.0;
        assert!(array.validate().is_err());
    }

    #[test]
    fn pv_array_validate_rejects_tilt_above_180() {
        let mut array = valid_array();
        array.tilt_deg = 181.0;
        assert!(array.validate().is_err());
    }

    #[test]
    fn pv_array_validate_rejects_azimuth_outside_range() {
        let mut array = valid_array();
        array.azimuth_deg = 400.0;
        assert!(array.validate().is_err());
    }

    #[test]
    fn pv_array_validate_rejects_azimuth_360() {
        // [0, 360) means 360.0 is excluded (maps to 0.0).
        let mut array = valid_array();
        array.azimuth_deg = 360.0;
        assert!(array.validate().is_err());
    }

    #[test]
    fn pv_array_validate_rejects_negative_azimuth() {
        let mut array = valid_array();
        array.azimuth_deg = -1.0;
        assert!(array.validate().is_err());
    }

    #[test]
    fn pv_array_validate_rejects_zero_capacity() {
        let mut array = valid_array();
        array.capacity_kw = 0.0;
        assert!(array.validate().is_err());
    }

    #[test]
    fn pv_array_validate_rejects_negative_capacity() {
        let mut array = valid_array();
        array.capacity_kw = -5.0;
        assert!(array.validate().is_err());
    }

    #[test]
    fn pv_array_validate_rejects_tilt_nan() {
        let mut array = valid_array();
        array.tilt_deg = f64::NAN;
        assert!(array.validate().is_err());
    }

    #[test]
    fn pv_array_validate_rejects_tilt_infinity() {
        let mut array = valid_array();
        array.tilt_deg = f64::INFINITY;
        assert!(array.validate().is_err());
    }

    #[test]
    fn pv_array_validate_rejects_azimuth_nan() {
        let mut array = valid_array();
        array.azimuth_deg = f64::NAN;
        assert!(array.validate().is_err());
    }

    #[test]
    fn pv_array_validate_rejects_capacity_nan() {
        let mut array = valid_array();
        array.capacity_kw = f64::NAN;
        assert!(array.validate().is_err());
    }

    #[test]
    fn pv_array_validate_rejects_noct_zero() {
        let mut array = valid_array();
        array.noct_c = 0.0;
        assert!(array.validate().is_err());
    }

    #[test]
    fn pv_array_validate_rejects_noct_negative() {
        let mut array = valid_array();
        array.noct_c = -1.0;
        assert!(array.validate().is_err());
    }

    #[test]
    fn pv_array_validate_rejects_noct_nan() {
        let mut array = valid_array();
        array.noct_c = f64::NAN;
        assert!(array.validate().is_err());
    }

    #[test]
    fn pv_array_validate_passes_for_valid_geometry() {
        assert!(valid_array().validate().is_ok());
    }

    #[test]
    fn pv_array_validate_passes_for_tilt_0() {
        let mut array = valid_array();
        array.tilt_deg = 0.0;
        assert!(array.validate().is_ok());
    }

    #[test]
    fn pv_array_validate_passes_for_tilt_180() {
        let mut array = valid_array();
        array.tilt_deg = 180.0;
        assert!(array.validate().is_ok());
    }

    #[test]
    fn pv_array_validate_passes_for_azimuth_0() {
        let mut array = valid_array();
        array.azimuth_deg = 0.0;
        assert!(array.validate().is_ok());
    }

    #[test]
    fn pv_array_validate_passes_for_azimuth_359_9() {
        let mut array = valid_array();
        array.azimuth_deg = 359.9;
        assert!(array.validate().is_ok());
    }

    #[test]
    fn pv_array_default_is_valid() {
        let array = PvArray::default();
        assert!(array.validate().is_ok());
        assert_eq!(array.tilt_deg, 30.0);
        assert_eq!(array.azimuth_deg, 180.0);
        assert_eq!(array.module_type, ModuleType::Standard);
    }
}
