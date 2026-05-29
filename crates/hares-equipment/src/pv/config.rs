//! Typed configuration for PV equipment.

use serde::{Deserialize, Serialize};

use crate::config::EquipmentTypedConfig;
use crate::pv::array_config::PvArraySpec;

/// Typed configuration for a photovoltaic system.
///
/// Supports both single-array (via the top-level `capacity_kw`, `tilt_deg`,
/// `azimuth_deg` fields) and multi-array configurations (via the optional
/// `arrays` field). When `arrays` is present and non-empty, `capacity_kw`
/// must equal the sum of per-array capacities; other top-level singular
/// fields are ignored in favour of the per-array specs.
///
/// ## System losses and soiling interaction
///
/// The default `system_losses_fraction` (0.14) matches the PVWatts v5 default
/// and includes a 2% static soiling component (multiplier 0.98). When the
/// Kimber dynamic soiling model is active (via `soiling_config`), the PV
/// equipment automatically subtracts the static soiling component from
/// `system_losses_fraction`, preventing double-counting of soiling losses.
/// The effective loss fraction used in power computation is then 0.12 for
/// the remaining non-soiling components (mismatch, wiring, connections, LID,
/// nameplate, age, availability). A warning is emitted when a custom
/// `system_losses_fraction` is provided alongside a dynamic soiling model
/// so that users can verify the effective value matches their intent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PvConfig {
    // Identity
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,

    // DC capacity of the array (single-array path)
    pub capacity_kw: f64,

    // Array geometry (single-array path)
    pub tilt_deg: Option<f64>,
    pub azimuth_deg: Option<f64>,

    // Module characteristics
    pub module_type: Option<String>,
    pub noct_c: Option<f64>,
    pub array_type: Option<String>,
    pub system_losses_fraction: Option<f64>,

    // Inverter
    pub inverter_efficiency: Option<f64>,
    pub inverter_capacity_kw: Option<f64>,

    // AC output
    pub power_factor: Option<f64>,

    // Surface resolution for irradiance lookup
    pub surface_resolution_deg: Option<f64>,
    // Optional SAM LUT path (CSV/Parquet ingestion handled at init boundary)
    pub sam_lut_path: Option<String>,

    /// Per-array configuration for multi-array systems.
    ///
    /// When present and non-empty, each entry maps to one `PvArray`.
    /// The top-level `capacity_kw` must equal the sum of per-array
    /// capacities; other top-level singular fields (`tilt_deg`,
    /// `azimuth_deg`, `module_type`, `noct_c`, `sam_lut_path`) are
    /// ignored in favour of the per-array specs.
    #[serde(default)]
    pub arrays: Option<Vec<PvArraySpec>>,
}

impl EquipmentTypedConfig for PvConfig {
    fn equipment_type_name() -> &'static str {
        "PV"
    }
}

impl PvConfig {
    /// Validate fields for physical plausibility.
    pub fn validate(&self) -> crate::Result<()> {
        use hares_types::HaresError;

        // Validate per-array specs when multi-array config is present.
        let total_capacity_kw = if let Some(ref arrays) = self.arrays {
            if arrays.is_empty() {
                return Err(HaresError::Equipment(
                    "PV arrays list is empty; supply at least one array spec".to_string(),
                ));
            }
            for (i, spec) in arrays.iter().enumerate() {
                spec.validate()
                    .map_err(|e| HaresError::Equipment(format!("PV array[{i}]: {e}")))?;
            }
            let sum: f64 = arrays.iter().map(|a| a.capacity_kw).sum();
            if (self.capacity_kw - sum).abs() > 0.001 {
                return Err(HaresError::Equipment(format!(
                    "PV capacity_kw ({}) does not match sum of array capacities ({}); \
                     set capacity_kw to the total or drop the field",
                    self.capacity_kw, sum
                )));
            }
            sum
        } else {
            // Single-array path: validate the top-level singular fields.
            if !self.capacity_kw.is_finite() || self.capacity_kw <= 0.0 {
                return Err(HaresError::Equipment(
                    "PV capacity_kw must be finite and > 0".to_string(),
                ));
            }
            if let Some(tilt) = self.tilt_deg {
                // EnergyPlus PVWatts.cc:118-119 validates tilt ∈ [0, 90];
                // panels at > 90° face the ground and collect no direct beam.
                if !tilt.is_finite() || !(0.0..=90.0).contains(&tilt) {
                    return Err(HaresError::Equipment(
                        "PV tilt_deg must be finite and within [0, 90]".to_string(),
                    ));
                }
            }
            if let Some(az) = self.azimuth_deg {
                if !az.is_finite() || !(0.0..360.0).contains(&az) {
                    return Err(HaresError::Equipment(
                        "PV azimuth_deg must be finite and within [0, 360)".to_string(),
                    ));
                }
            }
            self.capacity_kw
        };

        for (name, val) in [
            ("inverter_efficiency", self.inverter_efficiency),
            ("power_factor", self.power_factor),
        ] {
            if let Some(v) = val {
                if !v.is_finite() || v <= 0.0 || v > 1.0 {
                    return Err(HaresError::Equipment(format!(
                        "PV {name} must be finite and within (0, 1]"
                    )));
                }
            }
        }
        if let Some(losses) = self.system_losses_fraction {
            if !losses.is_finite() || !(0.0..1.0).contains(&losses) {
                return Err(HaresError::Equipment(
                    "PV system_losses_fraction must be finite and within [0, 1)".to_string(),
                ));
            }
        }
        if let Some(cap) = self.inverter_capacity_kw {
            if !cap.is_finite() || cap <= 0.0 {
                return Err(HaresError::Equipment(
                    "PV inverter_capacity_kw must be finite and > 0".to_string(),
                ));
            }
        }
        if let Some(inv_cap) = self.inverter_capacity_kw {
            // Typical residential DC-to-AC ratios: 1.0–1.5 (NREL SAM documentation;
            // EnergyPlus PVWatts v8 default = 1.1, PVWatts.cc:88). [0.8, 2.0] is a
            // generous acceptance window that rejects clearly pathological configs.
            let dc_ac_ratio = total_capacity_kw / inv_cap;
            if !(0.8..=2.0).contains(&dc_ac_ratio) {
                return Err(HaresError::Equipment(format!(
                    "PV DC-to-AC ratio {dc_ac_ratio:.2} is outside reasonable range [0.8, 2.0]"
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigPayload, EquipmentConfig};

    fn minimal_pv_config() -> PvConfig {
        PvConfig {
            equipment_id: None,
            zone_id: None,
            capacity_kw: 5.0,
            tilt_deg: None,
            azimuth_deg: None,
            module_type: None,
            noct_c: None,
            array_type: None,
            system_losses_fraction: None,
            inverter_efficiency: None,
            inverter_capacity_kw: None,
            power_factor: None,
            surface_resolution_deg: None,
            sam_lut_path: None,
            arrays: None,
        }
    }

    #[test]
    fn pv_config_round_trips_via_equipment_config() {
        let cfg = minimal_pv_config();
        let ec = EquipmentConfig::from_typed("test_pv".to_string(), "PV".to_string(), cfg.clone());
        assert!(ec.is_typed());
        let recovered: PvConfig = ec.typed().unwrap();
        assert_eq!(recovered.capacity_kw, cfg.capacity_kw);
    }

    #[test]
    fn pv_config_rejects_unknown_fields() {
        let json = serde_json::json!({
            "capacity_kw": 5.0,
            "not_a_field": true
        });
        let ec = EquipmentConfig::with_payload(
            "test".to_string(),
            "PV".to_string(),
            ConfigPayload::Typed {
                type_name: "PV".to_string(),
                version: 1,
                data: json,
            },
        );
        let result: crate::Result<PvConfig> = ec.typed();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown field"));
    }

    #[test]
    fn pv_config_validate_rejects_zero_capacity() {
        let mut cfg = minimal_pv_config();
        cfg.capacity_kw = 0.0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn pv_config_validate_rejects_out_of_range_tilt() {
        let mut cfg = minimal_pv_config();
        cfg.tilt_deg = Some(181.0);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn pv_config_validate_rejects_tilt_above_90() {
        let mut cfg = minimal_pv_config();
        cfg.tilt_deg = Some(91.0);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn pv_config_validate_passes_for_valid_config() {
        let cfg = minimal_pv_config();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn pv_config_validate_rejects_high_dc_ac_ratio() {
        let mut cfg = minimal_pv_config();
        cfg.capacity_kw = 10.0;
        cfg.inverter_capacity_kw = Some(0.4);
        // ratio = 25.0, well above the 2.0 upper bound
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn pv_config_validate_rejects_low_dc_ac_ratio() {
        let mut cfg = minimal_pv_config();
        cfg.capacity_kw = 1.0;
        cfg.inverter_capacity_kw = Some(100.0);
        // ratio = 0.01, well below the 0.8 lower bound
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn pv_config_validate_accepts_reasonable_dc_ac_ratio() {
        let mut cfg = minimal_pv_config();
        cfg.capacity_kw = 5.0;
        cfg.inverter_capacity_kw = Some(4.0);
        // ratio = 1.25, within [0.8, 2.0]
        assert!(cfg.validate().is_ok());
    }

    // --- Multi-array config tests ---

    #[test]
    fn multi_array_config_validates_all_array_specs() {
        use crate::pv::array_config::PvArraySpec;
        let cfg = PvConfig {
            arrays: Some(vec![
                PvArraySpec {
                    capacity_kw: 3.0,
                    tilt_deg: Some(30.0),
                    azimuth_deg: Some(180.0),
                    module_type: None,
                    noct_c: None,
                    array_type: None,
                    sam_lut_path: None,
                    attached_boundary_id: None,
                },
                PvArraySpec {
                    capacity_kw: 2.0,
                    tilt_deg: Some(20.0),
                    azimuth_deg: Some(90.0),
                    module_type: None,
                    noct_c: None,
                    array_type: None,
                    sam_lut_path: None,
                    attached_boundary_id: None,
                },
            ]),
            ..minimal_pv_config()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn multi_array_config_rejects_empty_arrays_list() {
        let mut cfg = minimal_pv_config();
        cfg.arrays = Some(vec![]);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn multi_array_config_rejects_invalid_child_spec() {
        use crate::pv::array_config::PvArraySpec;
        let mut cfg = minimal_pv_config();
        cfg.arrays = Some(vec![PvArraySpec {
            capacity_kw: 0.0,
            tilt_deg: None,
            azimuth_deg: None,
            module_type: None,
            noct_c: None,
            array_type: None,
            sam_lut_path: None,
            attached_boundary_id: None,
        }]);
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("array[0]"));
    }

    #[test]
    fn multi_array_dc_ac_ratio_uses_total_capacity() {
        use crate::pv::array_config::PvArraySpec;
        let mut cfg = minimal_pv_config();
        cfg.capacity_kw = 6.0;
        cfg.inverter_capacity_kw = Some(4.0);
        cfg.arrays = Some(vec![
            PvArraySpec {
                capacity_kw: 3.0,
                tilt_deg: None,
                azimuth_deg: None,
                module_type: None,
                noct_c: None,
                array_type: None,
                sam_lut_path: None,
                attached_boundary_id: None,
            },
            PvArraySpec {
                capacity_kw: 3.0,
                tilt_deg: None,
                azimuth_deg: None,
                module_type: None,
                noct_c: None,
                array_type: None,
                sam_lut_path: None,
                attached_boundary_id: None,
            },
        ]);
        // Total DC = 6.0 kW, inverter = 4.0 kW → ratio = 1.5, within [0.8, 2.0]
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn multi_array_rejects_capacity_mismatch() {
        use crate::pv::array_config::PvArraySpec;
        let mut cfg = minimal_pv_config();
        cfg.capacity_kw = 10.0;
        cfg.arrays = Some(vec![
            PvArraySpec {
                capacity_kw: 3.0,
                tilt_deg: None,
                azimuth_deg: None,
                module_type: None,
                noct_c: None,
                array_type: None,
                sam_lut_path: None,
                attached_boundary_id: None,
            },
            PvArraySpec {
                capacity_kw: 2.0,
                tilt_deg: None,
                azimuth_deg: None,
                module_type: None,
                noct_c: None,
                array_type: None,
                sam_lut_path: None,
                attached_boundary_id: None,
            },
        ]);
        // capacity_kw = 10.0 but arrays sum to 5.0 → must be rejected
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string()
                .contains("does not match sum of array capacities")
        );
    }

    #[test]
    fn multi_array_round_trips_via_equipment_config() {
        use crate::pv::array_config::PvArraySpec;
        let cfg = PvConfig {
            capacity_kw: 6.0,
            arrays: Some(vec![
                PvArraySpec {
                    capacity_kw: 4.0,
                    tilt_deg: Some(30.0),
                    azimuth_deg: Some(180.0),
                    module_type: Some("standard".to_string()),
                    noct_c: Some(47.0),
                    array_type: None,
                    sam_lut_path: None,
                    attached_boundary_id: None,
                },
                PvArraySpec {
                    capacity_kw: 2.0,
                    tilt_deg: Some(20.0),
                    azimuth_deg: Some(90.0),
                    module_type: Some("premium".to_string()),
                    noct_c: Some(45.0),
                    array_type: None,
                    sam_lut_path: None,
                    attached_boundary_id: None,
                },
            ]),
            ..minimal_pv_config()
        };
        let ec =
            EquipmentConfig::from_typed("test_multi".to_string(), "PV".to_string(), cfg.clone());
        assert!(ec.is_typed());
        let recovered: PvConfig = ec.typed().unwrap();
        let arrays = recovered.arrays.unwrap();
        assert_eq!(arrays.len(), 2);
        assert_eq!(arrays[0].capacity_kw, 4.0);
        assert_eq!(arrays[1].capacity_kw, 2.0);
        assert_eq!(arrays[0].tilt_deg, Some(30.0));
        assert_eq!(arrays[1].tilt_deg, Some(20.0));
    }
}
