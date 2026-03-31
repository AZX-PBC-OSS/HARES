//! Typed configuration for PV equipment.

use serde::{Deserialize, Serialize};

use crate::config::EquipmentTypedConfig;

/// Typed configuration for a photovoltaic system.
///
/// Supports a single array (the common residential case). Multi-array
/// systems continue to use the raw config path via `array_count`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PvConfig {
    // Identity
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,

    // DC capacity of the array
    pub capacity_kw: f64,

    // Array geometry
    pub tilt_deg: Option<f64>,
    pub azimuth_deg: Option<f64>,

    // Module characteristics
    pub module_type: Option<String>,
    pub noct_c: Option<f64>,
    pub system_losses_fraction: Option<f64>,

    // Inverter
    pub inverter_efficiency: Option<f64>,
    pub inverter_capacity_kw: Option<f64>,

    // AC output
    pub power_factor: Option<f64>,

    // Surface resolution for irradiance lookup
    pub surface_resolution_deg: Option<f64>,
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
        if !self.capacity_kw.is_finite() || self.capacity_kw <= 0.0 {
            return Err(HaresError::Equipment(
                "PV capacity_kw must be finite and > 0".to_string(),
            ));
        }
        if let Some(tilt) = self.tilt_deg {
            if !tilt.is_finite() || !(0.0..=180.0).contains(&tilt) {
                return Err(HaresError::Equipment(
                    "PV tilt_deg must be finite and within [0, 180]".to_string(),
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
            system_losses_fraction: None,
            inverter_efficiency: None,
            inverter_capacity_kw: None,
            power_factor: None,
            surface_resolution_deg: None,
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
        let ec = EquipmentConfig {
            name: "test".to_string(),
            ochre_class: "PV".to_string(),
            payload: ConfigPayload::Typed {
                type_name: "PV".to_string(),
                version: 1,
                data: json,
            },
        };
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
    fn pv_config_validate_passes_for_valid_config() {
        let cfg = minimal_pv_config();
        assert!(cfg.validate().is_ok());
    }
}
