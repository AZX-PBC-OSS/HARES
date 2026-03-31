//! Typed configuration structs for cooling HVAC equipment.

use serde::{Deserialize, Serialize};

use super::heating_config::DuctConfig;
use crate::config::EquipmentTypedConfig;

fn default_one() -> u8 {
    1
}

/// Typed configuration for central air conditioners.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CentralAirConditionerConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    /// Rated cooling capacity in watts.
    pub capacity_w: f64,
    /// Seasonal Energy Efficiency Ratio (BTU/Wh). EIR = 3.412 / SEER.
    pub seer: f64,
    /// Sensible heat ratio at rated conditions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shr: Option<f64>,
    /// Number of compressor speeds (1, 2, or 4 for variable-speed).
    #[serde(default = "default_one")]
    pub number_of_speeds: u8,
    /// Per-stage cooling capacities [W].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_capacities_w: Option<Vec<f64>>,
    /// Per-stage energy input ratios (EIR = 1/COP).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_eirs: Option<Vec<f64>>,
    /// Per-stage sensible heat ratios.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_shrs: Option<Vec<f64>>,
    /// Fan power in watts (constant).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w: Option<f64>,
    /// Fan power per CFM of airflow (W/CFM).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w_per_cfm: Option<f64>,
    /// Fraction of zone load served by this equipment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction_load_served: Option<f64>,
    /// Duct configuration (distribution system efficiency).
    #[serde(flatten)]
    pub duct: DuctConfig,
    /// System type string (e.g., "split", "packaged").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_type: Option<String>,
    /// Startup capacity degradation coefficient (Cd).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_cd: Option<f64>,
}

impl EquipmentTypedConfig for CentralAirConditionerConfig {
    fn equipment_type_name() -> &'static str {
        "Central AC"
    }
}

impl CentralAirConditionerConfig {
    /// Validate that efficiency and capacity fields are finite and positive.
    pub fn validate(&self) -> crate::Result<()> {
        use hares_types::HaresError;
        if !self.capacity_w.is_finite() || self.capacity_w <= 0.0 {
            return Err(HaresError::Equipment(format!(
                "CentralAirConditionerConfig: capacity_w must be finite and positive, got {}",
                self.capacity_w
            )));
        }
        if !self.seer.is_finite() || self.seer <= 0.0 {
            return Err(HaresError::Equipment(format!(
                "CentralAirConditionerConfig: seer must be finite and positive, got {}",
                self.seer
            )));
        }
        Ok(())
    }
}

/// Typed configuration for room air conditioners (window/through-wall units).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomAcConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    /// Rated cooling capacity in watts.
    pub capacity_w: f64,
    /// Energy Efficiency Ratio (BTU/Wh). EIR = 3.412 / EER.
    pub eer: f64,
}

impl EquipmentTypedConfig for RoomAcConfig {
    fn equipment_type_name() -> &'static str {
        "Room AC"
    }
}

impl RoomAcConfig {
    /// Validate that efficiency and capacity fields are finite and positive.
    pub fn validate(&self) -> crate::Result<()> {
        use hares_types::HaresError;
        if !self.capacity_w.is_finite() || self.capacity_w <= 0.0 {
            return Err(HaresError::Equipment(format!(
                "RoomAcConfig: capacity_w must be finite and positive, got {}",
                self.capacity_w
            )));
        }
        if !self.eer.is_finite() || self.eer <= 0.0 {
            return Err(HaresError::Equipment(format!(
                "RoomAcConfig: eer must be finite and positive, got {}",
                self.eer
            )));
        }
        Ok(())
    }
}

/// Typed configuration for heat pumps (both heating and cooling).
///
/// For mini-splits, `number_of_speeds` is forced to 4 in the equipment init path,
/// not in this struct — the struct records user intent; the equipment enforces the rule.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeatPumpConfig {
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
}

impl EquipmentTypedConfig for HeatPumpConfig {
    fn equipment_type_name() -> &'static str {
        "Heat Pump"
    }
}

impl HeatPumpConfig {
    /// Returns the effective number of speeds, forcing 4 when `is_mini_split` is true.
    ///
    /// Equipment init paths use this to apply the mini-split speed rule without
    /// mutating the config struct.
    pub fn effective_number_of_speeds(&self) -> u8 {
        if self.is_mini_split { 4 } else { self.number_of_speeds }
    }
}

/// Typed configuration for standalone dehumidifiers.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DehumidifierConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    /// Rated water removal capacity in pints per day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_pints_per_day: Option<f64>,
    /// Energy factor in L/kWh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub energy_factor: Option<f64>,
    /// Integrated energy factor in L/kWh (newer rating).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrated_energy_factor: Option<f64>,
    /// Fraction of dehumidification load served.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction_served: Option<f64>,
    /// Target relative humidity setpoint (fraction 0–1 or percent 0–100).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_rh: Option<f64>,
}

impl EquipmentTypedConfig for DehumidifierConfig {
    fn equipment_type_name() -> &'static str {
        "Dehumidifier"
    }
}

impl DehumidifierConfig {
    /// Validate that capacity and efficiency fields are finite and positive when present.
    pub fn validate(&self) -> crate::Result<()> {
        use hares_types::HaresError;
        if let Some(cap) = self.capacity_pints_per_day {
            if !cap.is_finite() || cap <= 0.0 {
                return Err(HaresError::Equipment(format!(
                    "DehumidifierConfig: capacity_pints_per_day must be finite and positive, got {cap}"
                )));
            }
        }
        if let Some(ef) = self.energy_factor {
            if !ef.is_finite() || ef <= 0.0 {
                return Err(HaresError::Equipment(format!(
                    "DehumidifierConfig: energy_factor must be finite and positive, got {ef}"
                )));
            }
        }
        if let Some(ief) = self.integrated_energy_factor {
            if !ief.is_finite() || ief <= 0.0 {
                return Err(HaresError::Equipment(format!(
                    "DehumidifierConfig: integrated_energy_factor must be finite and positive, got {ief}"
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

    fn typed_config<T: EquipmentTypedConfig>(config: T) -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "test".to_string(),
            T::equipment_type_name().to_string(),
            config,
        )
        .unwrap()
    }

    #[test]
    fn central_ac_config_round_trips() {
        let cfg = CentralAirConditionerConfig {
            equipment_id: Some(1),
            zone_id: Some(1),
            capacity_w: 12_000.0,
            seer: 16.0,
            shr: Some(0.75),
            number_of_speeds: 2,
            stage_capacities_w: Some(vec![6_000.0, 12_000.0]),
            stage_eirs: Some(vec![0.25, 0.22]),
            stage_shrs: Some(vec![0.78, 0.72]),
            fan_power_w: Some(300.0),
            fan_power_w_per_cfm: None,
            fraction_load_served: Some(1.0),
            duct: DuctConfig { dse_heat: Some(0.8), dse_cool: Some(0.85) },
            system_type: Some("split".to_string()),
            startup_cd: None,
        };
        let ec = typed_config(cfg.clone());
        assert!(ec.is_typed());
        let recovered: CentralAirConditionerConfig = ec.typed().unwrap();
        assert!((recovered.seer - cfg.seer).abs() < 1e-12);
        assert!((recovered.capacity_w - cfg.capacity_w).abs() < 1e-12);
        assert_eq!(recovered.stage_shrs, cfg.stage_shrs);
    }

    #[test]
    fn central_ac_rejects_unknown_fields() {
        let data = serde_json::json!({
            "capacity_w": 12000.0,
            "seer": 16.0,
            "unknown_key": true,
        });
        let ec = EquipmentConfig {
            name: "test".to_string(),
            ochre_class: "Central AC".to_string(),
            payload: ConfigPayload::Typed {
                type_name: "Central AC".to_string(),
                version: 1,
                data,
            },
        };
        let result: crate::Result<CentralAirConditionerConfig> = ec.typed();
        assert!(result.is_err());
    }

    #[test]
    fn central_ac_rejects_zero_capacity() {
        let cfg = CentralAirConditionerConfig {
            equipment_id: None,
            zone_id: None,
            capacity_w: 0.0,
            seer: 16.0,
            shr: None,
            number_of_speeds: 1,
            stage_capacities_w: None,
            stage_eirs: None,
            stage_shrs: None,
            fan_power_w: None,
            fan_power_w_per_cfm: None,
            fraction_load_served: None,
            duct: DuctConfig::default(),
            system_type: None,
            startup_cd: None,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn central_ac_rejects_negative_seer() {
        let cfg = CentralAirConditionerConfig {
            equipment_id: None,
            zone_id: None,
            capacity_w: 12_000.0,
            seer: -1.0,
            shr: None,
            number_of_speeds: 1,
            stage_capacities_w: None,
            stage_eirs: None,
            stage_shrs: None,
            fan_power_w: None,
            fan_power_w_per_cfm: None,
            fraction_load_served: None,
            duct: DuctConfig::default(),
            system_type: None,
            startup_cd: None,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn room_ac_config_round_trips() {
        let cfg = RoomAcConfig {
            equipment_id: Some(2),
            zone_id: Some(1),
            capacity_w: 3_500.0,
            eer: 10.0,
        };
        let ec = typed_config(cfg.clone());
        let recovered: RoomAcConfig = ec.typed().unwrap();
        assert!((recovered.eer - cfg.eer).abs() < 1e-12);
        assert!((recovered.capacity_w - cfg.capacity_w).abs() < 1e-12);
    }

    #[test]
    fn room_ac_rejects_zero_eer() {
        let cfg = RoomAcConfig {
            equipment_id: None,
            zone_id: None,
            capacity_w: 3_500.0,
            eer: 0.0,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn room_ac_rejects_nan_capacity() {
        let cfg = RoomAcConfig {
            equipment_id: None,
            zone_id: None,
            capacity_w: f64::NAN,
            eer: 10.0,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn heat_pump_config_round_trips() {
        let cfg = HeatPumpConfig {
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
        };
        let ec = typed_config(cfg.clone());
        let recovered: HeatPumpConfig = ec.typed().unwrap();
        assert!((recovered.seer.unwrap() - cfg.seer.unwrap()).abs() < 1e-12);
        assert!(recovered.is_mini_split);
        assert_eq!(recovered.stage_shrs, cfg.stage_shrs);
    }

    #[test]
    fn heat_pump_mini_split_effective_speeds() {
        let cfg = HeatPumpConfig {
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
            cooling_capacity_w: Some(12_000.0),
            seer: Some(16.0),
            stage_cooling_capacities_w: None,
            stage_cooling_eirs: None,
            stage_shrs: None,
            fraction_cooling_load_served: None,
            number_of_speeds: 1,
            is_mini_split: true,
            shr: None,
            fan_power_w: None,
            fan_power_w_per_cfm: None,
            duct: DuctConfig::default(),
        };
        assert_eq!(cfg.effective_number_of_speeds(), 4);
    }

    #[test]
    fn heat_pump_number_of_speeds_defaults_to_one() {
        let json = serde_json::json!({
            "cooling_capacity_w": 12000.0,
            "seer": 16.0,
        });
        let cfg: HeatPumpConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.number_of_speeds, 1);
    }

    #[test]
    fn heat_pump_rejects_unknown_fields() {
        let data = serde_json::json!({
            "cooling_capacity_w": 12000.0,
            "seer": 16.0,
            "unknown_key": true,
        });
        let ec = EquipmentConfig {
            name: "test".to_string(),
            ochre_class: "Heat Pump".to_string(),
            payload: ConfigPayload::Typed {
                type_name: "Heat Pump".to_string(),
                version: 1,
                data,
            },
        };
        let result: crate::Result<HeatPumpConfig> = ec.typed();
        assert!(result.is_err());
    }

    #[test]
    fn dehumidifier_config_round_trips() {
        let cfg = DehumidifierConfig {
            equipment_id: Some(1),
            zone_id: Some(1),
            capacity_pints_per_day: Some(70.0),
            energy_factor: Some(2.0),
            integrated_energy_factor: None,
            fraction_served: Some(1.0),
            target_rh: Some(0.50),
        };
        let ec = typed_config(cfg.clone());
        let recovered: DehumidifierConfig = ec.typed().unwrap();
        assert!(
            (recovered.capacity_pints_per_day.unwrap() - cfg.capacity_pints_per_day.unwrap()).abs()
                < 1e-12
        );
    }

    #[test]
    fn dehumidifier_rejects_zero_capacity() {
        let cfg = DehumidifierConfig {
            equipment_id: None,
            zone_id: None,
            capacity_pints_per_day: Some(0.0),
            energy_factor: None,
            integrated_energy_factor: None,
            fraction_served: None,
            target_rh: None,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn dehumidifier_rejects_infinite_energy_factor() {
        let cfg = DehumidifierConfig {
            equipment_id: None,
            zone_id: None,
            capacity_pints_per_day: Some(30.0),
            energy_factor: Some(f64::INFINITY),
            integrated_energy_factor: None,
            fraction_served: None,
            target_rh: None,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn dehumidifier_rejects_unknown_fields() {
        let data = serde_json::json!({
            "capacity_pints_per_day": 70.0,
            "unknown_field": true,
        });
        let ec = EquipmentConfig {
            name: "test".to_string(),
            ochre_class: "Dehumidifier".to_string(),
            payload: ConfigPayload::Typed {
                type_name: "Dehumidifier".to_string(),
                version: 1,
                data,
            },
        };
        let result: crate::Result<DehumidifierConfig> = ec.typed();
        assert!(result.is_err());
    }
}
