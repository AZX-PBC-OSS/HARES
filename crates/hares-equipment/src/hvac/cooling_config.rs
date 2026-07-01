//! Typed configuration structs for cooling HVAC equipment.

use serde::{Deserialize, Serialize};

use hares_physics::constants::BTU_PER_HR_PER_W;

use super::core_config::default_one;
use super::core_config::equipment_type_name;
use super::heating_config::DuctConfig;
use super::speed_control::SpeedControlMode;
use crate::config::EquipmentTypedConfig;
use crate::hvac::heating_config::HvacSetpointConfig;

/// Typed configuration for central air conditioners.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CentralAirConditionerConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    /// Required; resolver errors if absent.
    pub capacity_w: f64,
    /// Required; resolver errors if absent.
    pub eir: f64,
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
    #[serde(default)]
    pub setpoint: HvacSetpointConfig,
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
    #[serde(default)]
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
    /// Refrigerant charge defect ratio `(InstalledCharge - DesignCharge) / DesignCharge`.
    /// Applied multiplicatively to all rated capacity and EIR values during equipment init.
    /// Correction factors: ANSI/RESNET/ACCA 310-2020.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charge_defect_ratio: Option<f64>,
    /// Minimum outdoor air temperature for DX cooling compressor operation [°C].
    /// Below this temperature the compressor is locked out to prevent liquid slugging
    /// and oil foaming. EnergyPlus `DXCoils.cc:731`: `minOATCompDXCooling = -25.0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_oat_compressor_cooling_c: Option<f64>,
}

impl EquipmentTypedConfig for CentralAirConditionerConfig {
    fn equipment_type_name() -> &'static str {
        equipment_type_name::CENTRAL_AC
    }
}

impl CentralAirConditionerConfig {
    pub fn cooling_speed_control_mode(&self) -> SpeedControlMode {
        match self.number_of_speeds {
            1 => SpeedControlMode::SingleSpeed,
            2 => SpeedControlMode::TwoSpeedSetpoint,
            4 => SpeedControlMode::VariableSpeedIdeal,
            _ => SpeedControlMode::SingleSpeed,
        }
    }

    pub fn derived_cooling_startup_cd(&self) -> Option<f64> {
        self.startup_cd.or(match self.cooling_speed_control_mode() {
            SpeedControlMode::VariableSpeedIdeal => Some(0.0),
            SpeedControlMode::TwoSpeedSetpoint
            | SpeedControlMode::TwoSpeedTime
            | SpeedControlMode::TwoSpeedAlternating => Some(0.11),
            SpeedControlMode::SingleSpeed | SpeedControlMode::MultiSpeedInterpolated => {
                let seer = BTU_PER_HR_PER_W / self.eir.max(1e-6);
                Some(if seer < 13.0 { 0.20 } else { 0.07 })
            }
        })
    }

    /// Validate that efficiency and capacity fields are finite and positive.
    pub fn validate(&self) -> crate::Result<()> {
        use hares_types::HaresError;
        if !self.capacity_w.is_finite() || self.capacity_w <= 0.0 {
            return Err(HaresError::Equipment(format!(
                "CentralAirConditionerConfig: capacity_w must be finite and positive, got {}",
                self.capacity_w
            )));
        }
        if !self.eir.is_finite() || self.eir <= 0.0 {
            return Err(HaresError::Equipment(format!(
                "CentralAirConditionerConfig: eir must be finite and positive, got {}",
                self.eir
            )));
        }
        if !matches!(self.number_of_speeds, 1 | 2 | 4) {
            return Err(HaresError::Equipment(format!(
                "CentralAirConditionerConfig: number_of_speeds must be 1, 2, or 4, got {}",
                self.number_of_speeds
            )));
        }
        for (name, value) in [
            ("cooling_setpoint_c", self.setpoint.cooling_setpoint_c),
            ("heating_setpoint_c", self.setpoint.heating_setpoint_c),
            ("hysteresis_c", self.hysteresis_c),
            ("airflow_m3_s_per_w", self.airflow_m3_s_per_w),
            (
                "crankcase_heater_threshold_c",
                self.crankcase_heater_threshold_c,
            ),
            ("startup_cd", self.startup_cd),
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
        if let Some(v) = self.startup_cd
            && v < 0.0
        {
            return Err(HaresError::Equipment(
                "CentralAirConditionerConfig: startup_cd must be >= 0".to_string(),
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
        if let Some(stage_capacities_w) = &self.stage_capacities_w {
            if stage_capacities_w.len() != self.number_of_speeds as usize {
                return Err(HaresError::Equipment(format!(
                    "CentralAirConditionerConfig: stage_capacities_w length {} must match number_of_speeds {}",
                    stage_capacities_w.len(),
                    self.number_of_speeds
                )));
            }
            if stage_capacities_w
                .iter()
                .any(|value| !value.is_finite() || *value <= 0.0)
            {
                return Err(HaresError::Equipment(
                    "CentralAirConditionerConfig: stage_capacities_w must be finite and positive"
                        .to_string(),
                ));
            }
        }
        if let Some(stage_eirs) = &self.stage_eirs {
            let expected_len = self
                .stage_capacities_w
                .as_ref()
                .map_or(self.number_of_speeds as usize, Vec::len);
            if stage_eirs.len() != expected_len {
                return Err(HaresError::Equipment(format!(
                    "CentralAirConditionerConfig: stage_eirs length {} must match stage count {}",
                    stage_eirs.len(),
                    expected_len
                )));
            }
            if stage_eirs
                .iter()
                .any(|value| !value.is_finite() || *value <= 0.0)
            {
                return Err(HaresError::Equipment(
                    "CentralAirConditionerConfig: stage_eirs must be finite and positive"
                        .to_string(),
                ));
            }
        }
        if let Some(stage_shrs) = &self.stage_shrs {
            let expected_len = self
                .stage_capacities_w
                .as_ref()
                .map_or(self.number_of_speeds as usize, Vec::len);
            if stage_shrs.len() != expected_len {
                return Err(HaresError::Equipment(format!(
                    "CentralAirConditionerConfig: stage_shrs length {} must match stage count {}",
                    stage_shrs.len(),
                    expected_len
                )));
            }
            if stage_shrs
                .iter()
                .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
            {
                return Err(HaresError::Equipment(
                    "CentralAirConditionerConfig: stage_shrs must be finite and within [0, 1]"
                        .to_string(),
                ));
            }
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
    pub eir: f64,
    #[serde(default)]
    pub setpoint: HvacSetpointConfig,
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
    /// Sensible heat ratio at rated conditions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shr: Option<f64>,
    /// Startup capacity degradation coefficient (Cd).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_cd: Option<f64>,
    /// Crankcase heater rated power [kW].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crankcase_heater_kw: Option<f64>,
    /// Crankcase activation threshold [C].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crankcase_heater_threshold_c: Option<f64>,
    /// Outdoor-temperature capacity curve coefficients `[c0, c1, c2]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crankcase_capacity_curve_coeffs: Option<[f64; 3]>,
    /// Minimum outdoor air temperature for DX cooling compressor operation [°C].
    /// Below this temperature the compressor is locked out to prevent liquid slugging
    /// and oil foaming. EnergyPlus `DXCoils.cc:731`: `minOATCompDXCooling = -25.0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_oat_compressor_cooling_c: Option<f64>,
}

impl EquipmentTypedConfig for RoomAcConfig {
    fn equipment_type_name() -> &'static str {
        equipment_type_name::ROOM_AC
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
        if !self.eir.is_finite() || self.eir <= 0.0 {
            return Err(HaresError::Equipment(format!(
                "RoomAcConfig: eir must be finite and positive, got {}",
                self.eir
            )));
        }
        for (name, value) in [
            ("cooling_setpoint_c", self.setpoint.cooling_setpoint_c),
            ("heating_setpoint_c", self.setpoint.heating_setpoint_c),
            ("hysteresis_c", self.hysteresis_c),
            ("airflow_m3_s_per_w", self.airflow_m3_s_per_w),
            (
                "crankcase_heater_threshold_c",
                self.crankcase_heater_threshold_c,
            ),
            ("startup_cd", self.startup_cd),
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
        if let Some(v) = self.startup_cd
            && v < 0.0
        {
            return Err(HaresError::Equipment(
                "RoomAcConfig: startup_cd must be >= 0".to_string(),
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

    /// Derive the cycling degradation coefficient (Cd) from the unit's EIR-based
    /// SEER-equivalent rating.
    ///
    /// Returns `None` when `eir` is zero or non-finite, leaving the caller to
    /// choose a static fallback.
    ///
    /// Thresholds match EnergyPlus `StandardRatings.cc:177–180`:
    /// - SEER ≥ 13 → Cd = 0.07 (high-efficiency, lower cycling penalty)
    /// - SEER < 13  → Cd = 0.20 (standard-efficiency)
    pub fn derived_cooling_startup_cd(&self) -> Option<f64> {
        self.startup_cd.or_else(|| {
            if !self.eir.is_finite() || self.eir <= 0.0 {
                return None;
            }
            let seer = BTU_PER_HR_PER_W / self.eir;
            Some(if seer < 13.0 { 0.20 } else { 0.07 })
        })
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
    /// Cubic part-load curve coefficients [C0, C1, C2, C3] mapping PLR → PLF.
    ///
    /// When `Some`, the PLF is computed as `cubic(&coeffs, plr)` instead of the
    /// simplified `1 − Cd·(1−PLR)` formula. EnergyPlus `ZoneDehumidifier.cc`
    /// supports an optional `PartLoadCurve` (Curve:Cubic or Curve:Quadratic) on
    /// the `ZoneHVAC:Dehumidifier:DX` object.
    ///
    /// Default: EnergyPlus-informed cubic `[0.7, 1.0, -0.7, 0.0]`
    /// (PLF = 0.7 + 1.0·PLR − 0.7·PLR²), giving PLF=0.7 at PLR=0 and
    /// PLF=1.0 at PLR=1.0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part_load_curve_coeffs: Option<[f64; 4]>,
    /// Lower clamp for PLF (part-load factor). Defaults to 0.7 per
    /// EnergyPlus `ZoneDehumidifier.cc` lines 769–808.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plf_min: Option<f64>,
}

impl EquipmentTypedConfig for DehumidifierConfig {
    fn equipment_type_name() -> &'static str {
        equipment_type_name::DEHUMIDIFIER
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
        if let Some(coeffs) = self.part_load_curve_coeffs {
            if coeffs.iter().any(|v| !v.is_finite()) {
                return Err(HaresError::Equipment(
                    "DehumidifierConfig: part_load_curve_coeffs must be finite".to_string(),
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
            eir: 16.0,
            shr: Some(0.75),
            number_of_speeds: 2,
            stage_capacities_w: Some(vec![6_000.0, 12_000.0]),
            stage_eirs: Some(vec![0.25, 0.22]),
            stage_shrs: Some(vec![0.78, 0.72]),
            fan_power_w: Some(300.0),
            fan_power_w_per_cfm: None,
            setpoint: HvacSetpointConfig::default(),
            hysteresis_c: None,
            airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_CENTRAL_AC_M3_S_PER_W),
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
            charge_defect_ratio: None,
            min_oat_compressor_cooling_c: None,
        };
        let ec = typed_config(cfg.clone());
        assert!(ec.is_typed());
        let recovered: CentralAirConditionerConfig = ec.typed().unwrap();
        assert!((recovered.eir - cfg.eir).abs() < 1e-12);
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
            "eir": 16.0,
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
            eir: 16.0,
            shr: None,
            number_of_speeds: 1,
            stage_capacities_w: None,
            stage_eirs: None,
            stage_shrs: None,
            fan_power_w: None,
            fan_power_w_per_cfm: None,
            setpoint: HvacSetpointConfig::default(),
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
            charge_defect_ratio: None,
            min_oat_compressor_cooling_c: None,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn central_ac_rejects_negative_seer() {
        let cfg = CentralAirConditionerConfig {
            equipment_id: None,
            zone_id: None,
            capacity_w: 12_000.0,
            eir: -1.0,
            shr: None,
            number_of_speeds: 1,
            stage_capacities_w: None,
            stage_eirs: None,
            stage_shrs: None,
            fan_power_w: None,
            fan_power_w_per_cfm: None,
            setpoint: HvacSetpointConfig::default(),
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
            charge_defect_ratio: None,
            min_oat_compressor_cooling_c: None,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn central_ac_rejects_unsupported_speed_count() {
        let cfg = CentralAirConditionerConfig {
            equipment_id: None,
            zone_id: None,
            capacity_w: 12_000.0,
            eir: 0.25,
            shr: None,
            number_of_speeds: 3,
            stage_capacities_w: None,
            stage_eirs: None,
            stage_shrs: None,
            fan_power_w: None,
            fan_power_w_per_cfm: None,
            setpoint: HvacSetpointConfig::default(),
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
            charge_defect_ratio: None,
            min_oat_compressor_cooling_c: None,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn central_ac_rejects_stage_length_mismatch() {
        let cfg = CentralAirConditionerConfig {
            equipment_id: None,
            zone_id: None,
            capacity_w: 12_000.0,
            eir: 0.25,
            shr: None,
            number_of_speeds: 4,
            stage_capacities_w: Some(vec![3_000.0, 6_000.0, 9_000.0]),
            stage_eirs: Some(vec![0.20, 0.22, 0.24, 0.26]),
            stage_shrs: None,
            fan_power_w: None,
            fan_power_w_per_cfm: None,
            setpoint: HvacSetpointConfig::default(),
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
            charge_defect_ratio: None,
            min_oat_compressor_cooling_c: None,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn central_ac_typed_speed_mode_and_cd_follow_speed_count() {
        let mut cfg = CentralAirConditionerConfig {
            equipment_id: None,
            zone_id: None,
            capacity_w: 12_000.0,
            eir: 0.25,
            shr: None,
            number_of_speeds: 2,
            stage_capacities_w: Some(vec![6_000.0, 12_000.0]),
            stage_eirs: None,
            stage_shrs: None,
            fan_power_w: None,
            fan_power_w_per_cfm: None,
            setpoint: HvacSetpointConfig::default(),
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
            charge_defect_ratio: None,
            min_oat_compressor_cooling_c: None,
        };

        assert_eq!(
            cfg.cooling_speed_control_mode(),
            SpeedControlMode::TwoSpeedSetpoint
        );
        assert_eq!(cfg.derived_cooling_startup_cd(), Some(0.11));

        cfg.number_of_speeds = 4;
        cfg.stage_capacities_w = Some(vec![3_000.0, 6_000.0, 9_000.0, 12_000.0]);
        assert_eq!(
            cfg.cooling_speed_control_mode(),
            SpeedControlMode::VariableSpeedIdeal
        );
        assert_eq!(cfg.derived_cooling_startup_cd(), Some(0.0));
    }

    #[test]
    fn single_speed_ac_cd_derived_from_seer() {
        fn make_single_speed(eir: f64) -> CentralAirConditionerConfig {
            CentralAirConditionerConfig {
                equipment_id: None,
                zone_id: None,
                capacity_w: 10_000.0,
                eir,
                shr: None,
                number_of_speeds: 1,
                stage_capacities_w: None,
                stage_eirs: None,
                stage_shrs: None,
                fan_power_w: None,
                fan_power_w_per_cfm: None,
                setpoint: HvacSetpointConfig::default(),
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
                charge_defect_ratio: None,
                min_oat_compressor_cooling_c: None,
            }
        }

        // SEER 13 → eir = 3.412141633 / 13 ≈ 0.2624
        let eir_seer13 = 3.412_141_633 / 13.0;
        let cfg_13 = make_single_speed(eir_seer13);
        assert_eq!(cfg_13.derived_cooling_startup_cd(), Some(0.07));

        // SEER 16 (high efficiency) → c_d = 0.07
        let eir_seer16 = 3.412_141_633 / 16.0;
        let cfg_16 = make_single_speed(eir_seer16);
        assert_eq!(cfg_16.derived_cooling_startup_cd(), Some(0.07));

        // SEER 10 (older unit) → c_d = 0.20
        let eir_seer10 = 3.412_141_633 / 10.0;
        let cfg_10 = make_single_speed(eir_seer10);
        assert_eq!(cfg_10.derived_cooling_startup_cd(), Some(0.20));

        // Explicit startup_cd overrides the SEER-derived default.
        let mut cfg_override = make_single_speed(eir_seer16);
        cfg_override.startup_cd = Some(0.15);
        assert_eq!(cfg_override.derived_cooling_startup_cd(), Some(0.15));
    }

    #[test]
    fn room_ac_config_round_trips() {
        let cfg = RoomAcConfig {
            equipment_id: Some(2),
            zone_id: Some(1),
            capacity_w: 3_500.0,
            eir: 10.0,
            setpoint: HvacSetpointConfig::default(),
            hysteresis_c: None,
            airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_ROOM_AC_M3_S_PER_W),
            biquadratic_x1_min: Some(10.0),
            biquadratic_x1_max: Some(25.0),
            biquadratic_x2_min: None,
            biquadratic_x2_max: None,
            ff_min: None,
            ff_max: None,
            plf_min: None,
            plf_max: None,
            shr: None,
            startup_cd: None,
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            min_oat_compressor_cooling_c: None,
        };
        let ec = typed_config(cfg.clone());
        let recovered: RoomAcConfig = ec.typed().unwrap();
        assert!((recovered.eir - cfg.eir).abs() < 1e-12);
        assert!((recovered.capacity_w - cfg.capacity_w).abs() < 1e-12);
        assert_eq!(recovered.biquadratic_x1_min, cfg.biquadratic_x1_min);
    }

    #[test]
    fn room_ac_rejects_zero_eer() {
        let cfg = RoomAcConfig {
            equipment_id: None,
            zone_id: None,
            capacity_w: 3_500.0,
            eir: 0.0,
            setpoint: HvacSetpointConfig::default(),
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
            shr: None,
            startup_cd: None,
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            min_oat_compressor_cooling_c: None,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn room_ac_rejects_nan_capacity() {
        let cfg = RoomAcConfig {
            equipment_id: None,
            zone_id: None,
            capacity_w: f64::NAN,
            eir: 10.0,
            setpoint: HvacSetpointConfig::default(),
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
            shr: None,
            startup_cd: None,
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            min_oat_compressor_cooling_c: None,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn room_ac_derived_cd_from_high_seer_eir() {
        // SEER 14 → eir = BTU_PER_HR_PER_W / 14.0
        let eir = BTU_PER_HR_PER_W / 14.0;
        let cfg = RoomAcConfig {
            equipment_id: None,
            zone_id: None,
            capacity_w: 3_500.0,
            eir,
            setpoint: HvacSetpointConfig::default(),
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
            shr: None,
            startup_cd: None,
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            min_oat_compressor_cooling_c: None,
        };
        assert_eq!(cfg.derived_cooling_startup_cd(), Some(0.07));
    }

    #[test]
    fn room_ac_derived_cd_from_low_seer_eir() {
        // SEER 9 → eir = BTU_PER_HR_PER_W / 9.0
        let eir = BTU_PER_HR_PER_W / 9.0;
        let cfg = RoomAcConfig {
            equipment_id: None,
            zone_id: None,
            capacity_w: 3_500.0,
            eir,
            setpoint: HvacSetpointConfig::default(),
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
            shr: None,
            startup_cd: None,
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            min_oat_compressor_cooling_c: None,
        };
        assert_eq!(cfg.derived_cooling_startup_cd(), Some(0.20));
    }

    #[test]
    fn room_ac_explicit_startup_cd_overrides_derived() {
        let eir = BTU_PER_HR_PER_W / 14.0;
        let cfg = RoomAcConfig {
            equipment_id: None,
            zone_id: None,
            capacity_w: 3_500.0,
            eir,
            setpoint: HvacSetpointConfig::default(),
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
            shr: None,
            startup_cd: Some(0.15),
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            min_oat_compressor_cooling_c: None,
        };
        assert_eq!(cfg.derived_cooling_startup_cd(), Some(0.15));
    }

    #[test]
    fn room_ac_derived_cd_returns_none_for_invalid_eir() {
        let cfg = RoomAcConfig {
            equipment_id: None,
            zone_id: None,
            capacity_w: 3_500.0,
            eir: 0.0,
            setpoint: HvacSetpointConfig::default(),
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
            shr: None,
            startup_cd: None,
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            min_oat_compressor_cooling_c: None,
        };
        assert_eq!(cfg.derived_cooling_startup_cd(), None);
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
            part_load_curve_coeffs: None,
            plf_min: None,
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
            part_load_curve_coeffs: None,
            plf_min: None,
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
            part_load_curve_coeffs: None,
            plf_min: None,
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

    // --- Regression tests for ticket 009 ---

    // Issue 1: default_one() duplicated in cooling_config.rs and heat_pump_config.rs.
    #[test]
    fn central_ac_number_of_speeds_defaults_to_one() {
        let json = serde_json::json!({ "capacity_w": 10_000.0, "eir": 0.25 });
        let cfg: CentralAirConditionerConfig = serde_json::from_value(json).unwrap();
        assert_eq!(
            cfg.number_of_speeds, 1,
            "default_one() must return 1 for CentralAirConditionerConfig.number_of_speeds"
        );
    }

    // Issue 3: equipment type name string literals.
    #[test]
    fn cooling_equipment_type_name_literals() {
        assert_eq!(
            CentralAirConditionerConfig::equipment_type_name(),
            "Central AC",
            "CentralAirConditionerConfig type name must be 'Central AC'"
        );
        assert_eq!(
            RoomAcConfig::equipment_type_name(),
            "Room AC",
            "RoomAcConfig type name must be 'Room AC'"
        );
        assert_eq!(
            DehumidifierConfig::equipment_type_name(),
            "Dehumidifier",
            "DehumidifierConfig type name must be 'Dehumidifier'"
        );
    }

    #[test]
    fn room_ac_rejects_nan_startup_cd() {
        let cfg = RoomAcConfig {
            startup_cd: Some(f64::NAN),
            equipment_id: None,
            zone_id: None,
            capacity_w: 3_500.0,
            eir: 10.0,
            setpoint: HvacSetpointConfig::default(),
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
            shr: None,
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            min_oat_compressor_cooling_c: None,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn room_ac_rejects_negative_startup_cd() {
        let cfg = RoomAcConfig {
            startup_cd: Some(-0.5),
            equipment_id: None,
            zone_id: None,
            capacity_w: 3_500.0,
            eir: 10.0,
            setpoint: HvacSetpointConfig::default(),
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
            shr: None,
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            min_oat_compressor_cooling_c: None,
        };
        assert!(cfg.validate().is_err());
    }
}
