//! Typed configuration structs for heat pump HVAC equipment.

use serde::{Deserialize, Serialize};

use super::heating_config::DuctConfig;
use crate::config::EquipmentTypedConfig;

fn default_one() -> u8 {
    1
}

/// Typed configuration for heat-pump heaters (ASHP and MSHP heating side).
///
/// For mini-splits, `number_of_speeds` is forced to 4 in the equipment init path,
/// not in this struct — the struct records user intent; the equipment enforces the rule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeatPumpHeaterConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    // --- Heating parameters ---
    /// Rated heating capacity in watts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heating_capacity_w: Option<f64>,
    /// Heating Seasonal Performance Factor (BTU/Wh).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hspf: Option<f64>,
    /// Per-stage heating capacities [W].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_heating_capacities_w: Option<Vec<f64>>,
    /// Per-stage heating EIRs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_heating_eirs: Option<Vec<f64>>,
    /// Backup heating fuel type (e.g., "electric", "gas").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_fuel: Option<String>,
    /// Backup heating capacity in watts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_capacity_w: Option<f64>,
    /// Backup heating EIR.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_eir: Option<f64>,
    /// Fraction of heating load served.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction_heating_load_served: Option<f64>,
    // --- Cooling parameters ---
    /// Rated cooling capacity in watts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooling_capacity_w: Option<f64>,
    /// Seasonal Energy Efficiency Ratio (BTU/Wh).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seer: Option<f64>,
    /// Per-stage cooling capacities [W].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_cooling_capacities_w: Option<Vec<f64>>,
    /// Per-stage cooling EIRs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_cooling_eirs: Option<Vec<f64>>,
    /// Per-stage sensible heat ratios.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_shrs: Option<Vec<f64>>,
    /// Fraction of cooling load served.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction_cooling_load_served: Option<f64>,
    // --- Common parameters ---
    /// Number of compressor speeds.
    /// When `is_mini_split` is true, the equipment init path forces this to 4.
    #[serde(default = "default_one")]
    pub number_of_speeds: u8,
    /// Whether this is a mini-split heat pump.
    /// When true, the equipment init path forces `number_of_speeds` to 4.
    #[serde(default)]
    pub is_mini_split: bool,
    /// Sensible heat ratio at rated conditions (for single-speed or default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shr: Option<f64>,
    /// Fan power in watts (constant).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w: Option<f64>,
    /// Fan power per CFM of airflow (W/CFM).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w_per_cfm: Option<f64>,
    /// Duct configuration.
    #[serde(flatten)]
    pub duct: DuctConfig,
    /// Biquadratic curve x1 (wet-bulb) lower bound [C].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biquadratic_x1_min: Option<f64>,
    /// Biquadratic curve x1 (wet-bulb) upper bound [C].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biquadratic_x1_max: Option<f64>,
    /// Biquadratic curve x2 (outdoor dry-bulb) lower bound [C].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biquadratic_x2_min: Option<f64>,
    /// Biquadratic curve x2 (outdoor dry-bulb) upper bound [C].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biquadratic_x2_max: Option<f64>,
    /// Flow-fraction lower clamp bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ff_min: Option<f64>,
    /// Flow-fraction upper clamp bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ff_max: Option<f64>,
    /// Part-load fraction (PLF) lower clamp bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plf_min: Option<f64>,
    /// Part-load fraction (PLF) upper clamp bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plf_max: Option<f64>,
}

impl Default for HeatPumpHeaterConfig {
    fn default() -> Self {
        Self {
            equipment_id: None,
            zone_id: None,
            heating_capacity_w: None,
            hspf: None,
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            backup_fuel: None,
            backup_capacity_w: None,
            backup_eir: None,
            fraction_heating_load_served: None,
            cooling_capacity_w: None,
            seer: None,
            stage_cooling_capacities_w: None,
            stage_cooling_eirs: None,
            stage_shrs: None,
            fraction_cooling_load_served: None,
            number_of_speeds: 1,
            is_mini_split: false,
            shr: None,
            fan_power_w: None,
            fan_power_w_per_cfm: None,
            duct: DuctConfig::default(),
            biquadratic_x1_min: None,
            biquadratic_x1_max: None,
            biquadratic_x2_min: None,
            biquadratic_x2_max: None,
            ff_min: None,
            ff_max: None,
            plf_min: None,
            plf_max: None,
        }
    }
}

impl EquipmentTypedConfig for HeatPumpHeaterConfig {
    fn equipment_type_name() -> &'static str {
        "ASHP Heater"
    }
}

impl HeatPumpHeaterConfig {
    /// Returns the effective number of speeds, forcing 4 when `is_mini_split` is true.
    pub fn effective_number_of_speeds(&self) -> u8 {
        if self.is_mini_split {
            4
        } else {
            self.number_of_speeds
        }
    }

    /// Validate that capacity and efficiency fields are finite and positive when present.
    pub fn validate(&self) -> crate::Result<()> {
        use hares_types::HaresError;
        let check = |v: f64, field: &str| -> crate::Result<()> {
            if !v.is_finite() || v <= 0.0 {
                Err(HaresError::Equipment(format!(
                    "HeatPumpHeaterConfig: {field} must be finite and positive, got {v}"
                )))
            } else {
                Ok(())
            }
        };
        if let Some(v) = self.heating_capacity_w {
            check(v, "heating_capacity_w")?;
        }
        if let Some(v) = self.hspf {
            check(v, "hspf")?;
        }
        if let Some(v) = self.cooling_capacity_w {
            check(v, "cooling_capacity_w")?;
        }
        if let Some(v) = self.seer {
            check(v, "seer")?;
        }
        if let Some(v) = self.backup_capacity_w {
            if v.is_nan() || (v < 0.0 && v.is_finite()) {
                return Err(HaresError::Equipment(format!(
                    "HeatPumpHeaterConfig: backup_capacity_w must be finite and non-negative, got {v}"
                )));
            }
        }
        Ok(())
    }
}

/// Typed configuration for heat-pump coolers (ASHP and MSHP cooling side).
///
/// Contains the same fields as `HeatPumpHeaterConfig` but registers under the
/// "ASHP Cooler" equipment type name so that typed configs round-trip correctly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeatPumpCoolerConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heating_capacity_w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hspf: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_heating_capacities_w: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_heating_eirs: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_fuel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_capacity_w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_eir: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction_heating_load_served: Option<f64>,
    /// Rated cooling capacity in watts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooling_capacity_w: Option<f64>,
    /// Seasonal Energy Efficiency Ratio (BTU/Wh).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seer: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_cooling_capacities_w: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_cooling_eirs: Option<Vec<f64>>,
    /// Per-stage sensible heat ratios.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_shrs: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction_cooling_load_served: Option<f64>,
    /// Number of compressor speeds.
    #[serde(default = "default_one")]
    pub number_of_speeds: u8,
    /// Whether this is a mini-split heat pump.
    #[serde(default)]
    pub is_mini_split: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shr: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w_per_cfm: Option<f64>,
    #[serde(flatten)]
    pub duct: DuctConfig,
    /// Biquadratic curve x1 (wet-bulb) lower bound [C].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biquadratic_x1_min: Option<f64>,
    /// Biquadratic curve x1 (wet-bulb) upper bound [C].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biquadratic_x1_max: Option<f64>,
    /// Biquadratic curve x2 (outdoor dry-bulb) lower bound [C].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biquadratic_x2_min: Option<f64>,
    /// Biquadratic curve x2 (outdoor dry-bulb) upper bound [C].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biquadratic_x2_max: Option<f64>,
    /// Flow-fraction lower clamp bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ff_min: Option<f64>,
    /// Flow-fraction upper clamp bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ff_max: Option<f64>,
    /// Part-load fraction (PLF) lower clamp bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plf_min: Option<f64>,
    /// Part-load fraction (PLF) upper clamp bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plf_max: Option<f64>,
}

impl EquipmentTypedConfig for HeatPumpCoolerConfig {
    fn equipment_type_name() -> &'static str {
        "ASHP Cooler"
    }
}

/// Alias for `HeatPumpHeaterConfig`; prefer the full name for new code.
pub type HeatPumpConfig = HeatPumpHeaterConfig;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigPayload, EquipmentConfig};

    fn typed_config<T: EquipmentTypedConfig>(config: T) -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "test".to_string(),
            T::equipment_type_name().to_string(),
            config,
        )
    }

    #[test]
    fn heat_pump_heater_config_round_trips() {
        let cfg = HeatPumpHeaterConfig {
            equipment_id: Some(1),
            zone_id: Some(1),
            heating_capacity_w: Some(10_000.0),
            hspf: Some(9.0),
            stage_heating_capacities_w: Some(vec![5_000.0, 10_000.0]),
            stage_heating_eirs: Some(vec![0.28, 0.25]),
            backup_fuel: Some("electric".to_string()),
            backup_capacity_w: Some(5_000.0),
            backup_eir: Some(1.0),
            fraction_heating_load_served: Some(1.0),
            cooling_capacity_w: Some(12_000.0),
            seer: Some(16.0),
            stage_cooling_capacities_w: Some(vec![6_000.0, 12_000.0]),
            stage_cooling_eirs: Some(vec![0.25, 0.22]),
            stage_shrs: Some(vec![0.78, 0.72]),
            fraction_cooling_load_served: Some(1.0),
            number_of_speeds: 1,
            is_mini_split: true,
            shr: Some(0.75),
            fan_power_w: Some(300.0),
            fan_power_w_per_cfm: None,
            duct: DuctConfig::default(),
            biquadratic_x1_min: Some(12.0),
            biquadratic_x1_max: Some(24.0),
            biquadratic_x2_min: Some(18.0),
            biquadratic_x2_max: Some(46.0),
            ff_min: Some(0.6),
            ff_max: Some(1.2),
            plf_min: Some(0.7),
            plf_max: Some(1.0),
        };
        let ec = typed_config(cfg.clone());
        let recovered: HeatPumpHeaterConfig = ec.typed().unwrap();
        assert!((recovered.seer.unwrap() - cfg.seer.unwrap()).abs() < 1e-12);
        assert!(recovered.is_mini_split);
        assert_eq!(recovered.stage_shrs, cfg.stage_shrs);
        assert_eq!(recovered.biquadratic_x1_min, cfg.biquadratic_x1_min);
        assert_eq!(recovered.ff_min, cfg.ff_min);
    }

    #[test]
    fn heat_pump_cooler_config_round_trips() {
        let cfg = HeatPumpCoolerConfig {
            equipment_id: Some(2),
            zone_id: Some(1),
            heating_capacity_w: Some(9_000.0),
            hspf: Some(8.8),
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            backup_fuel: None,
            backup_capacity_w: Some(5_000.0),
            backup_eir: Some(1.0),
            fraction_heating_load_served: Some(1.0),
            cooling_capacity_w: Some(12_000.0),
            seer: Some(16.0),
            stage_cooling_capacities_w: Some(vec![6_000.0, 12_000.0]),
            stage_cooling_eirs: Some(vec![0.25, 0.22]),
            stage_shrs: Some(vec![0.78, 0.72]),
            fraction_cooling_load_served: Some(1.0),
            number_of_speeds: 2,
            is_mini_split: false,
            shr: Some(0.75),
            fan_power_w: Some(300.0),
            fan_power_w_per_cfm: None,
            duct: DuctConfig::default(),
            biquadratic_x1_min: Some(12.0),
            biquadratic_x1_max: Some(24.0),
            biquadratic_x2_min: Some(18.0),
            biquadratic_x2_max: Some(46.0),
            ff_min: Some(0.6),
            ff_max: Some(1.2),
            plf_min: Some(0.7),
            plf_max: Some(1.0),
        };
        let ec =
            EquipmentConfig::from_typed("test".to_string(), "ASHP Cooler".to_string(), cfg.clone());
        let recovered: HeatPumpCoolerConfig = ec.typed().unwrap();
        assert!((recovered.seer.unwrap() - cfg.seer.unwrap()).abs() < 1e-12);
        assert_eq!(recovered.number_of_speeds, 2);
        assert_eq!(recovered.stage_shrs, cfg.stage_shrs);
    }

    #[test]
    fn heat_pump_mini_split_effective_speeds() {
        let cfg = HeatPumpHeaterConfig {
            cooling_capacity_w: Some(12_000.0),
            seer: Some(16.0),
            is_mini_split: true,
            ..Default::default()
        };
        assert_eq!(cfg.effective_number_of_speeds(), 4);
    }

    #[test]
    fn heat_pump_number_of_speeds_defaults_to_one() {
        let json = serde_json::json!({
            "cooling_capacity_w": 12000.0,
            "seer": 16.0,
        });
        let cfg: HeatPumpHeaterConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.number_of_speeds, 1);
    }

    #[test]
    fn heat_pump_heater_rejects_unknown_fields() {
        let data = serde_json::json!({
            "cooling_capacity_w": 12000.0,
            "seer": 16.0,
            "unknown_key": true,
        });
        let ec = EquipmentConfig {
            name: "test".to_string(),
            ochre_class: "ASHP Heater".to_string(),
            payload: ConfigPayload::Typed {
                type_name: "ASHP Heater".to_string(),
                version: 1,
                data,
            },
        };
        let result: crate::Result<HeatPumpHeaterConfig> = ec.typed();
        assert!(result.is_err());
    }

    #[test]
    fn heat_pump_heater_validate_rejects_negative_capacity() {
        let cfg = HeatPumpHeaterConfig {
            heating_capacity_w: Some(-1.0),
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }
}
