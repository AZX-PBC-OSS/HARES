//! Typed configuration structs for heat pump HVAC equipment.

use serde::{Deserialize, Serialize};

use super::core_config::default_one;
use super::heating_config::DuctConfig;
use super::speed_control::SpeedControlMode;
use crate::config::EquipmentTypedConfig;
use hares_types::{FuelType, ScheduleSourceConfig};

/// Fields shared between `HeatPumpHeaterConfig` and `HeatPumpCoolerConfig`.
///
/// `deny_unknown_fields` is intentionally omitted: serde `flatten` is
/// incompatible with `deny_unknown_fields` on both the inner (flattened) struct
/// and the outer struct that flattens it. Neither struct uses it.
/// See <https://serde.rs/attr-flatten.html> and <https://github.com/serde-rs/serde/issues/2384>.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HeatPumpCommonConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heating_capacity_w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heating_eir: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_heating_capacities_w: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_heating_eirs: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_fuel: Option<FuelType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_capacity_w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_eir: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction_heating_load_served: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooling_capacity_w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooling_eir: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_cooling_capacities_w: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_cooling_eirs: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction_cooling_load_served: Option<f64>,
    #[serde(default = "default_one")]
    pub number_of_speeds: u8,
    #[serde(default)]
    pub is_mini_split: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shr: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w_per_cfm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub airflow_m3_s_per_w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heating_setpoint_c: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooling_setpoint_c: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hysteresis_c: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heating_setpoint_source: Option<ScheduleSourceConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooling_setpoint_source: Option<ScheduleSourceConfig>,
    #[serde(flatten)]
    pub duct: DuctConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biquadratic_x1_min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biquadratic_x1_max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biquadratic_x2_min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biquadratic_x2_max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ff_min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ff_max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plf_min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plf_max: Option<f64>,
}

impl Default for HeatPumpCommonConfig {
    fn default() -> Self {
        Self {
            equipment_id: None,
            zone_id: None,
            heating_capacity_w: None,
            heating_eir: None,
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            backup_fuel: None,
            backup_capacity_w: None,
            backup_eir: None,
            fraction_heating_load_served: None,
            cooling_capacity_w: None,
            cooling_eir: None,
            stage_cooling_capacities_w: None,
            stage_cooling_eirs: None,
            fraction_cooling_load_served: None,
            number_of_speeds: 1,
            is_mini_split: false,
            shr: None,
            fan_power_w: None,
            fan_power_w_per_cfm: None,
            airflow_m3_s_per_w: None,
            heating_setpoint_c: None,
            cooling_setpoint_c: None,
            hysteresis_c: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
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

/// Typed configuration for heat-pump heaters (ASHP and MSHP heating side).
///
/// For mini-splits, `number_of_speeds` is forced to 4 in the equipment init path,
/// not in this struct -- the struct records user intent; the equipment enforces the rule.
///
/// `deny_unknown_fields` is intentionally omitted: serde `flatten` is
/// incompatible with `deny_unknown_fields` on both the inner and outer struct.
/// See <https://serde.rs/attr-flatten.html> and <https://github.com/serde-rs/serde/issues/2384>.
/// The `heater_only_fields_missing_from_cooler_json` and
/// `cooler_only_field_missing_from_heater_json` tests guard against key leakage
/// between heater and cooler structs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HeatPumpHeaterConfig {
    #[serde(flatten)]
    pub common: HeatPumpCommonConfig,
    /// Heat-pump lockout below this outdoor temperature [C].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hp_lockout_temp_c: Option<f64>,
    /// Electric-resistance lockout below this outdoor temperature [C].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub er_lockout_temp_c: Option<f64>,
    /// EnergyPlus supplemental-ER upper OAT cap [C].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_oat_supplemental_c: Option<f64>,
    /// ER call threshold offset below the heating setpoint [C].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub er_setpoint_offset_c: Option<f64>,
    /// ER hard lockout duration after a setpoint raise [s].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub er_hard_lockout_time_s: Option<f64>,
}

impl EquipmentTypedConfig for HeatPumpHeaterConfig {
    fn equipment_type_name() -> &'static str {
        super::core_config::equipment_type_name::ASHP_HEATER
    }
}

impl HeatPumpHeaterConfig {
    /// Returns the effective number of speeds, forcing 4 when `is_mini_split` is true.
    pub fn effective_number_of_speeds(&self) -> u8 {
        if self.common.is_mini_split {
            4
        } else {
            self.common.number_of_speeds
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
        if let Some(v) = self.common.heating_capacity_w {
            check(v, "heating_capacity_w")?;
        }
        if let Some(v) = self.common.heating_eir {
            check(v, "heating_eir")?;
        }
        if let Some(v) = self.common.cooling_capacity_w {
            check(v, "cooling_capacity_w")?;
        }
        if let Some(v) = self.common.cooling_eir {
            check(v, "cooling_eir")?;
        }
        if let Some(v) = self.common.backup_capacity_w {
            if v.is_nan() || (v < 0.0 && v.is_finite()) {
                return Err(HaresError::Equipment(format!(
                    "HeatPumpHeaterConfig: backup_capacity_w must be finite and non-negative, got {v}"
                )));
            }
        }
        for (name, value) in [
            ("heating_setpoint_c", self.common.heating_setpoint_c),
            ("cooling_setpoint_c", self.common.cooling_setpoint_c),
            ("hysteresis_c", self.common.hysteresis_c),
            ("airflow_m3_s_per_w", self.common.airflow_m3_s_per_w),
            ("hp_lockout_temp_c", self.hp_lockout_temp_c),
            ("er_lockout_temp_c", self.er_lockout_temp_c),
            ("max_oat_supplemental_c", self.max_oat_supplemental_c),
            ("er_setpoint_offset_c", self.er_setpoint_offset_c),
            ("er_hard_lockout_time_s", self.er_hard_lockout_time_s),
        ] {
            if let Some(v) = value
                && !v.is_finite()
            {
                return Err(HaresError::Equipment(format!(
                    "HeatPumpHeaterConfig: {name} must be finite, got {v}"
                )));
            }
        }
        if let Some(v) = self.common.hysteresis_c
            && v < 0.0
        {
            return Err(HaresError::Equipment(
                "HeatPumpHeaterConfig: hysteresis_c must be >= 0".to_string(),
            ));
        }
        if let Some(v) = self.common.airflow_m3_s_per_w
            && v <= 0.0
        {
            return Err(HaresError::Equipment(
                "HeatPumpHeaterConfig: airflow_m3_s_per_w must be > 0".to_string(),
            ));
        }
        if let Some(v) = self.er_hard_lockout_time_s
            && v < 0.0
        {
            return Err(HaresError::Equipment(
                "HeatPumpHeaterConfig: er_hard_lockout_time_s must be >= 0".to_string(),
            ));
        }
        if !self.common.is_mini_split {
            let n_speeds = self.effective_number_of_speeds() as usize;
            if n_speeds >= 2 {
                if let Some(stages) = &self.common.stage_heating_capacities_w {
                    if stages.len() != n_speeds {
                        return Err(HaresError::Equipment(format!(
                            "HeatPumpHeaterConfig: stage_heating_capacities_w has {} elements but number_of_speeds is {}; lengths must match",
                            stages.len(),
                            n_speeds,
                        )));
                    }
                }
                if let Some(stages) = &self.common.stage_heating_eirs {
                    if stages.len() != n_speeds {
                        return Err(HaresError::Equipment(format!(
                            "HeatPumpHeaterConfig: stage_heating_eirs has {} elements but number_of_speeds is {}; lengths must match",
                            stages.len(),
                            n_speeds,
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

/// Typed configuration for heat-pump coolers (ASHP and MSHP cooling side).
///
/// Contains the same fields as `HeatPumpHeaterConfig` but registers under the
/// "ASHP Cooler" equipment type name so that typed configs round-trip correctly.
///
/// `deny_unknown_fields` is intentionally omitted: serde `flatten` is
/// incompatible with `deny_unknown_fields` on both the inner and outer struct.
/// See <https://serde.rs/attr-flatten.html> and <https://github.com/serde-rs/serde/issues/2384>.
/// The `cooler_only_field_missing_from_heater_json` and
/// `heater_only_fields_missing_from_cooler_json` tests guard against key leakage.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HeatPumpCoolerConfig {
    #[serde(flatten)]
    pub common: HeatPumpCommonConfig,
    /// Per-stage sensible heat ratios.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_shrs: Option<Vec<f64>>,
}

impl EquipmentTypedConfig for HeatPumpCoolerConfig {
    fn equipment_type_name() -> &'static str {
        super::core_config::equipment_type_name::ASHP_COOLER
    }
}

impl HeatPumpCoolerConfig {
    /// Returns the effective number of speeds, forcing 4 when `is_mini_split` is true.
    pub fn effective_number_of_speeds(&self) -> u8 {
        if self.common.is_mini_split {
            4
        } else {
            self.common.number_of_speeds
        }
    }

    /// Cooling-side control mode derived from the typed config.
    ///
    /// Mini-split coolers are modeled as continuously variable-speed equipment.
    pub fn cooling_speed_control_mode(&self) -> SpeedControlMode {
        let n = self.effective_number_of_speeds();
        match n {
            1 => SpeedControlMode::SingleSpeed,
            2 => SpeedControlMode::TwoSpeedSetpoint,
            n if n >= 4 => SpeedControlMode::VariableSpeedIdeal,
            _ => SpeedControlMode::SingleSpeed,
        }
    }

    /// Derived startup degradation coefficient for typed cooling operation.
    ///
    /// Single-speed coolers retain the generic HVAC-core default unless an explicit
    /// typed startup Cd is available upstream.
    pub fn derived_cooling_startup_cd(&self) -> Option<f64> {
        match self.cooling_speed_control_mode() {
            SpeedControlMode::VariableSpeedIdeal => Some(0.0),
            SpeedControlMode::TwoSpeedSetpoint
            | SpeedControlMode::TwoSpeedTime
            | SpeedControlMode::TwoSpeedAlternating => Some(0.11),
            SpeedControlMode::SingleSpeed | SpeedControlMode::MultiSpeedInterpolated => None,
        }
    }

    /// Validate that cooling-side fields are finite and positive when present.
    pub fn validate(&self) -> crate::Result<()> {
        use hares_types::HaresError;

        let check_positive = |value: f64, field: &str| -> crate::Result<()> {
            if !value.is_finite() || value <= 0.0 {
                Err(HaresError::Equipment(format!(
                    "HeatPumpCoolerConfig: {field} must be finite and positive, got {value}"
                )))
            } else {
                Ok(())
            }
        };

        if let Some(value) = self.common.cooling_capacity_w {
            check_positive(value, "cooling_capacity_w")?;
        }
        if let Some(value) = self.common.cooling_eir {
            check_positive(value, "cooling_eir")?;
        }
        if let Some(value) = self.common.heating_capacity_w {
            check_positive(value, "heating_capacity_w")?;
        }
        if let Some(value) = self.common.heating_eir {
            check_positive(value, "heating_eir")?;
        }
        if let Some(value) = self.common.backup_capacity_w
            && (!value.is_finite() || value < 0.0)
        {
            return Err(HaresError::Equipment(format!(
                "HeatPumpCoolerConfig: backup_capacity_w must be finite and non-negative, got {value}"
            )));
        }

        for (name, value) in [
            ("heating_setpoint_c", self.common.heating_setpoint_c),
            ("cooling_setpoint_c", self.common.cooling_setpoint_c),
            ("hysteresis_c", self.common.hysteresis_c),
            ("airflow_m3_s_per_w", self.common.airflow_m3_s_per_w),
        ] {
            if let Some(value) = value
                && !value.is_finite()
            {
                return Err(HaresError::Equipment(format!(
                    "HeatPumpCoolerConfig: {name} must be finite, got {value}"
                )));
            }
        }

        if let Some(value) = self.common.hysteresis_c
            && value < 0.0
        {
            return Err(HaresError::Equipment(
                "HeatPumpCoolerConfig: hysteresis_c must be >= 0".to_string(),
            ));
        }
        if let Some(value) = self.common.airflow_m3_s_per_w
            && value <= 0.0
        {
            return Err(HaresError::Equipment(
                "HeatPumpCoolerConfig: airflow_m3_s_per_w must be > 0".to_string(),
            ));
        }

        Ok(())
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
            common: HeatPumpCommonConfig {
                equipment_id: Some(1),
                zone_id: Some(1),
                heating_capacity_w: Some(10_000.0),
                heating_eir: Some(9.0),
                stage_heating_capacities_w: Some(vec![5_000.0, 10_000.0]),
                stage_heating_eirs: Some(vec![0.28, 0.25]),
                backup_fuel: Some(FuelType::Electric),
                backup_capacity_w: Some(5_000.0),
                backup_eir: Some(1.0),
                fraction_heating_load_served: Some(1.0),
                cooling_capacity_w: Some(12_000.0),
                cooling_eir: Some(16.0),
                stage_cooling_capacities_w: Some(vec![6_000.0, 12_000.0]),
                stage_cooling_eirs: Some(vec![0.25, 0.22]),
                fraction_cooling_load_served: Some(1.0),
                number_of_speeds: 1,
                is_mini_split: true,
                shr: Some(0.75),
                fan_power_w: Some(300.0),
                fan_power_w_per_cfm: None,
                airflow_m3_s_per_w: Some(4.0e-5),
                heating_setpoint_c: Some(21.0),
                cooling_setpoint_c: Some(26.0),
                hysteresis_c: Some(1.0),
                heating_setpoint_source: None,
                cooling_setpoint_source: None,
                duct: DuctConfig::default(),
                biquadratic_x1_min: Some(12.0),
                biquadratic_x1_max: Some(24.0),
                biquadratic_x2_min: Some(18.0),
                biquadratic_x2_max: Some(46.0),
                ff_min: Some(0.6),
                ff_max: Some(1.2),
                plf_min: Some(0.7),
                plf_max: Some(1.0),
            },
            hp_lockout_temp_c: Some(-17.8),
            er_lockout_temp_c: Some(4.4),
            max_oat_supplemental_c: Some(21.0),
            er_setpoint_offset_c: Some(1.6),
            er_hard_lockout_time_s: Some(600.0),
        };
        let ec = typed_config(cfg.clone());
        let recovered: HeatPumpHeaterConfig = ec.typed().unwrap();
        assert!(
            (recovered.common.cooling_eir.unwrap() - cfg.common.cooling_eir.unwrap()).abs() < 1e-12
        );
        assert!(recovered.common.is_mini_split);
        assert_eq!(
            recovered.common.biquadratic_x1_min,
            cfg.common.biquadratic_x1_min
        );
        assert_eq!(recovered.common.ff_min, cfg.common.ff_min);
    }

    #[test]
    fn heat_pump_cooler_effective_control_mode_tracks_typed_intent() {
        let mut cfg = HeatPumpCoolerConfig {
            common: HeatPumpCommonConfig {
                cooling_capacity_w: Some(12_000.0),
                cooling_eir: Some(0.25),
                number_of_speeds: 2,
                ..HeatPumpCommonConfig::default()
            },
            stage_shrs: None,
        };

        assert_eq!(
            cfg.cooling_speed_control_mode(),
            SpeedControlMode::TwoSpeedSetpoint
        );
        assert_eq!(cfg.derived_cooling_startup_cd(), Some(0.11));

        cfg.common.number_of_speeds = 4;
        cfg.common.is_mini_split = true;

        assert_eq!(cfg.effective_number_of_speeds(), 4);
        assert_eq!(
            cfg.cooling_speed_control_mode(),
            SpeedControlMode::VariableSpeedIdeal
        );
        assert_eq!(cfg.derived_cooling_startup_cd(), Some(0.0));
    }

    #[test]
    fn heat_pump_cooler_config_round_trips() {
        let cfg = HeatPumpCoolerConfig {
            common: HeatPumpCommonConfig {
                equipment_id: Some(2),
                zone_id: Some(1),
                heating_capacity_w: Some(9_000.0),
                heating_eir: Some(8.8),
                backup_capacity_w: Some(5_000.0),
                backup_eir: Some(1.0),
                fraction_heating_load_served: Some(1.0),
                cooling_capacity_w: Some(12_000.0),
                cooling_eir: Some(16.0),
                stage_cooling_capacities_w: Some(vec![6_000.0, 12_000.0]),
                stage_cooling_eirs: Some(vec![0.25, 0.22]),
                fraction_cooling_load_served: Some(1.0),
                number_of_speeds: 2,
                shr: Some(0.75),
                fan_power_w: Some(300.0),
                airflow_m3_s_per_w: Some(4.0e-5),
                heating_setpoint_c: Some(21.0),
                cooling_setpoint_c: Some(26.0),
                hysteresis_c: Some(1.0),
                duct: DuctConfig::default(),
                biquadratic_x1_min: Some(12.0),
                biquadratic_x1_max: Some(24.0),
                biquadratic_x2_min: Some(18.0),
                biquadratic_x2_max: Some(46.0),
                ff_min: Some(0.6),
                ff_max: Some(1.2),
                plf_min: Some(0.7),
                plf_max: Some(1.0),
                ..HeatPumpCommonConfig::default()
            },
            stage_shrs: Some(vec![0.78, 0.72]),
        };
        let ec =
            EquipmentConfig::from_typed("test".to_string(), "ASHP Cooler".to_string(), cfg.clone());
        let recovered: HeatPumpCoolerConfig = ec.typed().unwrap();
        assert!(
            (recovered.common.cooling_eir.unwrap() - cfg.common.cooling_eir.unwrap()).abs() < 1e-12
        );
        assert_eq!(recovered.common.number_of_speeds, 2);
        assert_eq!(recovered.stage_shrs, cfg.stage_shrs);
    }

    #[test]
    fn heat_pump_mini_split_effective_speeds() {
        let cfg = HeatPumpHeaterConfig {
            common: HeatPumpCommonConfig {
                cooling_capacity_w: Some(12_000.0),
                cooling_eir: Some(16.0),
                is_mini_split: true,
                ..HeatPumpCommonConfig::default()
            },
            ..Default::default()
        };
        assert_eq!(cfg.effective_number_of_speeds(), 4);
    }

    #[test]
    fn heat_pump_number_of_speeds_defaults_to_one() {
        let json = serde_json::json!({
            "cooling_capacity_w": 12000.0,
            "cooling_eir": 16.0,
        });
        let cfg: HeatPumpHeaterConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.common.number_of_speeds, 1);
    }

    /// serde flatten is incompatible with `deny_unknown_fields`, so truly
    /// unknown keys are silently ignored rather than rejected. This test
    /// documents that behavior — unknown keys do not cause an error.
    #[test]
    fn heat_pump_heater_ignores_unknown_fields() {
        let data = serde_json::json!({
            "cooling_capacity_w": 12000.0,
            "cooling_eir": 16.0,
            "unknown_key": true,
        });
        let ec = EquipmentConfig::with_payload(
            "test".to_string(),
            "ASHP Heater".to_string(),
            ConfigPayload::Typed {
                type_name: "ASHP Heater".to_string(),
                version: 1,
                data,
            },
        );
        let cfg: HeatPumpHeaterConfig = ec.typed().unwrap();
        assert_eq!(cfg.common.cooling_eir.unwrap(), 16.0);
    }

    #[test]
    fn heat_pump_heater_validate_rejects_negative_capacity() {
        let cfg = HeatPumpHeaterConfig {
            common: HeatPumpCommonConfig {
                heating_capacity_w: Some(-1.0),
                ..HeatPumpCommonConfig::default()
            },
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn heat_pump_heater_config_serde_json_round_trip() {
        let cfg = HeatPumpHeaterConfig {
            common: HeatPumpCommonConfig {
                equipment_id: Some(1),
                zone_id: Some(1),
                heating_capacity_w: Some(10_000.0),
                heating_eir: Some(9.0),
                stage_heating_capacities_w: Some(vec![5_000.0, 10_000.0]),
                stage_heating_eirs: Some(vec![0.28, 0.25]),
                backup_fuel: Some(FuelType::Electric),
                backup_capacity_w: Some(5_000.0),
                backup_eir: Some(1.0),
                fraction_heating_load_served: Some(1.0),
                cooling_capacity_w: Some(12_000.0),
                cooling_eir: Some(16.0),
                stage_cooling_capacities_w: Some(vec![6_000.0, 12_000.0]),
                stage_cooling_eirs: Some(vec![0.25, 0.22]),
                fraction_cooling_load_served: Some(1.0),
                number_of_speeds: 2,
                is_mini_split: false,
                shr: Some(0.75),
                fan_power_w: Some(300.0),
                fan_power_w_per_cfm: None,
                airflow_m3_s_per_w: Some(4.0e-5),
                heating_setpoint_c: Some(21.0),
                cooling_setpoint_c: Some(26.0),
                hysteresis_c: Some(1.0),
                heating_setpoint_source: None,
                cooling_setpoint_source: None,
                duct: DuctConfig::default(),
                biquadratic_x1_min: Some(12.0),
                biquadratic_x1_max: Some(24.0),
                biquadratic_x2_min: Some(18.0),
                biquadratic_x2_max: Some(46.0),
                ff_min: Some(0.6),
                ff_max: Some(1.2),
                plf_min: Some(0.7),
                plf_max: Some(1.0),
            },
            hp_lockout_temp_c: Some(-17.8),
            er_lockout_temp_c: Some(4.4),
            max_oat_supplemental_c: Some(21.0),
            er_setpoint_offset_c: Some(1.6),
            er_hard_lockout_time_s: Some(600.0),
        };
        let value = serde_json::to_value(&cfg).unwrap();
        let recovered: HeatPumpHeaterConfig = serde_json::from_value(value.clone()).unwrap();
        let reserialized = serde_json::to_value(&recovered).unwrap();
        assert_eq!(value, reserialized);
    }

    #[test]
    fn heat_pump_cooler_config_serde_json_round_trip() {
        let cfg = HeatPumpCoolerConfig {
            common: HeatPumpCommonConfig {
                equipment_id: Some(2),
                zone_id: Some(1),
                heating_capacity_w: Some(9_000.0),
                heating_eir: Some(8.8),
                backup_capacity_w: Some(5_000.0),
                backup_eir: Some(1.0),
                fraction_heating_load_served: Some(1.0),
                cooling_capacity_w: Some(12_000.0),
                cooling_eir: Some(16.0),
                stage_cooling_capacities_w: Some(vec![6_000.0, 12_000.0]),
                stage_cooling_eirs: Some(vec![0.25, 0.22]),
                fraction_cooling_load_served: Some(1.0),
                number_of_speeds: 2,
                shr: Some(0.75),
                fan_power_w: Some(300.0),
                airflow_m3_s_per_w: Some(4.0e-5),
                heating_setpoint_c: Some(21.0),
                cooling_setpoint_c: Some(26.0),
                hysteresis_c: Some(1.0),
                duct: DuctConfig::default(),
                biquadratic_x1_min: Some(12.0),
                biquadratic_x1_max: Some(24.0),
                biquadratic_x2_min: Some(18.0),
                biquadratic_x2_max: Some(46.0),
                ff_min: Some(0.6),
                ff_max: Some(1.2),
                plf_min: Some(0.7),
                plf_max: Some(1.0),
                ..HeatPumpCommonConfig::default()
            },
            stage_shrs: Some(vec![0.78, 0.72]),
        };
        let value = serde_json::to_value(&cfg).unwrap();
        let recovered: HeatPumpCoolerConfig = serde_json::from_value(value.clone()).unwrap();
        let reserialized = serde_json::to_value(&recovered).unwrap();
        assert_eq!(value, reserialized);
    }

    // --- Regression tests for ticket 009 ---

    #[test]
    fn number_of_speeds_defaults_to_one_heater() {
        let json = serde_json::json!({ "cooling_capacity_w": 10_000.0, "cooling_eir": 16.0 });
        let cfg: HeatPumpHeaterConfig = serde_json::from_value(json).unwrap();
        assert_eq!(
            cfg.common.number_of_speeds, 1,
            "default_one() must return 1 for number_of_speeds"
        );
    }

    #[test]
    fn number_of_speeds_defaults_to_one_cooler() {
        let json = serde_json::json!({ "cooling_capacity_w": 10_000.0, "cooling_eir": 16.0 });
        let cfg: HeatPumpCoolerConfig = serde_json::from_value(json).unwrap();
        assert_eq!(
            cfg.common.number_of_speeds, 1,
            "default_one() must return 1 for number_of_speeds (cooler)"
        );
    }

    /// serde flatten is incompatible with `deny_unknown_fields` on the outer struct,
    /// so cross-type fields like `stage_shrs` are silently ignored by
    /// `HeatPumpHeaterConfig` deserialization rather than rejected.
    /// See <https://serde.rs/attr-flatten.html>.
    #[test]
    fn heater_ignores_cooler_only_field_stage_shrs() {
        let json = serde_json::json!({
            "cooling_capacity_w": 10_000.0,
            "cooling_eir": 16.0,
            "stage_shrs": [0.75, 0.72],
        });
        let cfg: HeatPumpHeaterConfig = serde_json::from_value(json).unwrap();
        assert!(
            cfg.hp_lockout_temp_c.is_none(),
            "stage_shrs is silently dropped; heater-only fields remain None"
        );
    }

    /// Same constraint as above: heater-only field `hp_lockout_temp_c` is
    /// silently ignored by `HeatPumpCoolerConfig` deserialization.
    #[test]
    fn cooler_ignores_heater_only_field_hp_lockout() {
        let json = serde_json::json!({
            "cooling_capacity_w": 10_000.0,
            "cooling_eir": 16.0,
            "hp_lockout_temp_c": -17.8,
        });
        let cfg: HeatPumpCoolerConfig = serde_json::from_value(json).unwrap();
        assert!(
            cfg.stage_shrs.is_none(),
            "hp_lockout_temp_c is silently dropped; cooler-only fields remain None"
        );
    }

    #[test]
    fn heat_pump_equipment_type_name_literals() {
        assert_eq!(HeatPumpHeaterConfig::equipment_type_name(), "ASHP Heater",);
        assert_eq!(HeatPumpCoolerConfig::equipment_type_name(), "ASHP Cooler",);
    }

    #[test]
    fn equipment_type_names_match_constants() {
        assert_eq!(
            HeatPumpHeaterConfig::equipment_type_name(),
            super::super::core_config::equipment_type_name::ASHP_HEATER
        );
        assert_eq!(
            HeatPumpCoolerConfig::equipment_type_name(),
            super::super::core_config::equipment_type_name::ASHP_COOLER
        );
    }
}
