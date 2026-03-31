//! Typed configuration structs for cooling HVAC equipment.

use serde::{Deserialize, Serialize};

use super::heating_config::DuctConfig;
use crate::config::EquipmentTypedConfig;

fn default_one() -> u8 {
    1
}

/// Typed configuration for central air conditioners.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CentralAirConditionerConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    /// Required; resolver errors if absent.
    pub capacity_w: f64,
    /// Required; resolver errors if absent.
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
    /// Cooling setpoint used by the HVAC thermostat FSM.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooling_setpoint_c: Option<f64>,
    /// Heating setpoint used by the HVAC thermostat FSM.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heating_setpoint_c: Option<f64>,
    /// Thermostat hysteresis around the setpoints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hysteresis_c: Option<f64>,
    /// Airflow in m^3/s/W for the HVAC wrapper.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub airflow_m3_s_per_w: Option<f64>,
    /// Fraction of zone load served by this equipment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction_load_served: Option<f64>,
    /// Crankcase heater rated power [kW].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crankcase_heater_kw: Option<f64>,
    /// Crankcase activation threshold [C].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crankcase_heater_threshold_c: Option<f64>,
    /// Outdoor-temperature capacity curve coefficients `[c0, c1, c2]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crankcase_capacity_curve_coeffs: Option<[f64; 3]>,
    /// Duct configuration (distribution system efficiency).
    #[serde(flatten)]
    pub duct: DuctConfig,
    /// System type string (e.g., "split", "packaged").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_type: Option<String>,
    /// Startup capacity degradation coefficient (Cd).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_cd: Option<f64>,
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
        for (name, value) in [
            ("cooling_setpoint_c", self.cooling_setpoint_c),
            ("heating_setpoint_c", self.heating_setpoint_c),
            ("hysteresis_c", self.hysteresis_c),
            ("airflow_m3_s_per_w", self.airflow_m3_s_per_w),
            (
                "crankcase_heater_threshold_c",
                self.crankcase_heater_threshold_c,
            ),
        ] {
            if let Some(v) = value
                && !v.is_finite()
            {
                return Err(HaresError::Equipment(format!(
                    "CentralAirConditionerConfig: {name} must be finite, got {v}"
                )));
            }
        }
        if let Some(v) = self.airflow_m3_s_per_w
            && v <= 0.0
        {
            return Err(HaresError::Equipment(
                "CentralAirConditionerConfig: airflow_m3_s_per_w must be > 0".to_string(),
            ));
        }
        if let Some(v) = self.crankcase_heater_kw
            && (!v.is_finite() || v < 0.0)
        {
            return Err(HaresError::Equipment(
                "CentralAirConditionerConfig: crankcase_heater_kw must be finite and >= 0"
                    .to_string(),
            ));
        }
        if let Some(coeffs) = self.crankcase_capacity_curve_coeffs
            && coeffs.iter().any(|v| !v.is_finite())
        {
            return Err(HaresError::Equipment(
                "CentralAirConditionerConfig: crankcase_capacity_curve_coeffs must be finite"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

/// Typed configuration for room air conditioners (window/through-wall units).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomAcConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    /// Required; resolver errors if absent.
    pub capacity_w: f64,
    /// Required; resolver errors if absent.
    pub eer: f64,
    /// Cooling setpoint used by the HVAC thermostat FSM.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooling_setpoint_c: Option<f64>,
    /// Heating setpoint used by the HVAC thermostat FSM.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heating_setpoint_c: Option<f64>,
    /// Thermostat hysteresis around the setpoints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hysteresis_c: Option<f64>,
    /// Airflow in m^3/s/W for the HVAC wrapper.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub airflow_m3_s_per_w: Option<f64>,
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
    /// Crankcase heater rated power [kW].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crankcase_heater_kw: Option<f64>,
    /// Crankcase activation threshold [C].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crankcase_heater_threshold_c: Option<f64>,
    /// Outdoor-temperature capacity curve coefficients `[c0, c1, c2]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crankcase_capacity_curve_coeffs: Option<[f64; 3]>,
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
        for (name, value) in [
            ("cooling_setpoint_c", self.cooling_setpoint_c),
            ("heating_setpoint_c", self.heating_setpoint_c),
            ("hysteresis_c", self.hysteresis_c),
            ("airflow_m3_s_per_w", self.airflow_m3_s_per_w),
            (
                "crankcase_heater_threshold_c",
                self.crankcase_heater_threshold_c,
            ),
        ] {
            if let Some(v) = value
                && !v.is_finite()
            {
                return Err(HaresError::Equipment(format!(
                    "RoomAcConfig: {name} must be finite, got {v}"
                )));
            }
        }
        if let Some(v) = self.airflow_m3_s_per_w
            && v <= 0.0
        {
            return Err(HaresError::Equipment(
                "RoomAcConfig: airflow_m3_s_per_w must be > 0".to_string(),
            ));
        }
        if let Some(v) = self.crankcase_heater_kw
            && (!v.is_finite() || v < 0.0)
        {
            return Err(HaresError::Equipment(
                "RoomAcConfig: crankcase_heater_kw must be finite and >= 0".to_string(),
            ));
        }
        if let Some(coeffs) = self.crankcase_capacity_curve_coeffs
            && coeffs.iter().any(|v| !v.is_finite())
        {
            return Err(HaresError::Equipment(
                "RoomAcConfig: crankcase_capacity_curve_coeffs must be finite".to_string(),
            ));
        }
        Ok(())
    }
}

/// Typed configuration for standalone dehumidifiers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DehumidifierConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    /// Rated water removal capacity in liters per day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_liters_per_day: Option<f64>,
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
        if let Some(cap) = self.capacity_liters_per_day {
            if !cap.is_finite() || cap <= 0.0 {
                return Err(HaresError::Equipment(format!(
                    "DehumidifierConfig: capacity_liters_per_day must be finite and positive, got {cap}"
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
    use hares_physics::constants::{CFM_TO_M3_S, W_PER_TON};

    fn typed_config<T: EquipmentTypedConfig>(config: T) -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "test".to_string(),
            T::equipment_type_name().to_string(),
            config,
        )
    }

    fn airflow_m3_s_per_w(cfm_per_ton: f64) -> f64 {
        cfm_per_ton * CFM_TO_M3_S / W_PER_TON
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
            cooling_setpoint_c: None,
            heating_setpoint_c: None,
            hysteresis_c: None,
            airflow_m3_s_per_w: Some(airflow_m3_s_per_w(400.0)),
            fraction_load_served: Some(1.0),
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            duct: DuctConfig {
                dse_heat: Some(0.8),
                dse_cool: Some(0.85),
                ..DuctConfig::default()
            },
            system_type: Some("split".to_string()),
            startup_cd: None,
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
        assert!(ec.is_typed());
        let recovered: CentralAirConditionerConfig = ec.typed().unwrap();
        assert!((recovered.seer - cfg.seer).abs() < 1e-12);
        assert!((recovered.capacity_w - cfg.capacity_w).abs() < 1e-12);
        assert_eq!(recovered.stage_shrs, cfg.stage_shrs);
        assert_eq!(recovered.biquadratic_x1_min, cfg.biquadratic_x1_min);
        assert_eq!(recovered.ff_min, cfg.ff_min);
        assert_eq!(recovered.plf_max, cfg.plf_max);
    }

    #[test]
    fn central_ac_rejects_unknown_fields() {
        let data = serde_json::json!({
            "capacity_w": 12000.0,
            "seer": 16.0,
            "unknown_key": true,
        });
        let ec = EquipmentConfig::with_payload(
            "test".to_string(),
            "Central AC".to_string(),
            ConfigPayload::Typed {
                type_name: "Central AC".to_string(),
                version: 1,
                data,
            },
        );
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
            cooling_setpoint_c: None,
            heating_setpoint_c: None,
            hysteresis_c: None,
            airflow_m3_s_per_w: None,
            fraction_load_served: None,
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            duct: DuctConfig::default(),
            system_type: None,
            startup_cd: None,
            biquadratic_x1_min: None,
            biquadratic_x1_max: None,
            biquadratic_x2_min: None,
            biquadratic_x2_max: None,
            ff_min: None,
            ff_max: None,
            plf_min: None,
            plf_max: None,
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
            cooling_setpoint_c: None,
            heating_setpoint_c: None,
            hysteresis_c: None,
            airflow_m3_s_per_w: None,
            fraction_load_served: None,
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            duct: DuctConfig::default(),
            system_type: None,
            startup_cd: None,
            biquadratic_x1_min: None,
            biquadratic_x1_max: None,
            biquadratic_x2_min: None,
            biquadratic_x2_max: None,
            ff_min: None,
            ff_max: None,
            plf_min: None,
            plf_max: None,
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
            cooling_setpoint_c: None,
            heating_setpoint_c: None,
            hysteresis_c: None,
            airflow_m3_s_per_w: Some(airflow_m3_s_per_w(320.0)),
            biquadratic_x1_min: Some(10.0),
            biquadratic_x1_max: Some(25.0),
            biquadratic_x2_min: None,
            biquadratic_x2_max: None,
            ff_min: None,
            ff_max: None,
            plf_min: None,
            plf_max: None,
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
        };
        let ec = typed_config(cfg.clone());
        let recovered: RoomAcConfig = ec.typed().unwrap();
        assert!((recovered.eer - cfg.eer).abs() < 1e-12);
        assert!((recovered.capacity_w - cfg.capacity_w).abs() < 1e-12);
        assert_eq!(recovered.biquadratic_x1_min, cfg.biquadratic_x1_min);
    }

    #[test]
    fn room_ac_rejects_zero_eer() {
        let cfg = RoomAcConfig {
            equipment_id: None,
            zone_id: None,
            capacity_w: 3_500.0,
            eer: 0.0,
            cooling_setpoint_c: None,
            heating_setpoint_c: None,
            hysteresis_c: None,
            airflow_m3_s_per_w: None,
            biquadratic_x1_min: None,
            biquadratic_x1_max: None,
            biquadratic_x2_min: None,
            biquadratic_x2_max: None,
            ff_min: None,
            ff_max: None,
            plf_min: None,
            plf_max: None,
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
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
            cooling_setpoint_c: None,
            heating_setpoint_c: None,
            hysteresis_c: None,
            airflow_m3_s_per_w: None,
            biquadratic_x1_min: None,
            biquadratic_x1_max: None,
            biquadratic_x2_min: None,
            biquadratic_x2_max: None,
            ff_min: None,
            ff_max: None,
            plf_min: None,
            plf_max: None,
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn dehumidifier_config_round_trips() {
        let cfg = DehumidifierConfig {
            equipment_id: Some(1),
            zone_id: Some(1),
            capacity_liters_per_day: Some(33.1223531),
            energy_factor: Some(2.0),
            integrated_energy_factor: None,
            fraction_served: Some(1.0),
            target_rh: Some(0.50),
        };
        let ec = typed_config(cfg.clone());
        let recovered: DehumidifierConfig = ec.typed().unwrap();
        assert!(
            (recovered.capacity_liters_per_day.unwrap() - cfg.capacity_liters_per_day.unwrap())
                .abs()
                < 1e-12
        );
    }

    #[test]
    fn dehumidifier_rejects_zero_capacity() {
        let cfg = DehumidifierConfig {
            equipment_id: None,
            zone_id: None,
            capacity_liters_per_day: Some(0.0),
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
            capacity_liters_per_day: Some(14.195),
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
            "capacity_liters_per_day": 33.1223531,
            "unknown_field": true,
        });
        let ec = EquipmentConfig::with_payload(
            "test".to_string(),
            "Dehumidifier".to_string(),
            ConfigPayload::Typed {
                type_name: "Dehumidifier".to_string(),
                version: 1,
                data,
            },
        );
        let result: crate::Result<DehumidifierConfig> = ec.typed();
        assert!(result.is_err());
    }
}
