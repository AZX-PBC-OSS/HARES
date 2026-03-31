//! Typed configuration structs for all water heater equipment types.

use serde::{Deserialize, Serialize};

use crate::config::EquipmentTypedConfig;

/// Typed configuration for a gas storage water heater.
///
/// All power fields are in watts; all volume fields are in cubic metres;
/// all temperature fields are in degrees Celsius.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GasWaterHeaterConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,

    /// Tank volume [m³].
    pub tank_volume_m3: Option<f64>,
    /// Tank height [m].
    pub tank_height_m: Option<f64>,
    /// Tank diameter [m].
    pub tank_diameter_m: Option<f64>,
    /// Tank heat-loss conductance [W/K].
    pub ua_w_per_k: Option<f64>,
    /// Insulation jacket R-value [m²·K/W].
    pub jacket_r_value_m2_k_w: Option<f64>,
    /// Number of tank stratification nodes.
    pub tank_nodes: Option<u8>,
    /// Index of the burner node (0-based).
    pub burner_node: Option<u8>,

    /// Thermostat setpoint [°C].
    pub setpoint_c: Option<f64>,
    /// Thermostat deadband width [°C].
    pub deadband_c: Option<f64>,
    /// Maximum safe tank temperature [°C]; burner locks out above this.
    pub max_tank_temp_c: Option<f64>,

    /// Burner rated heat-input power [W].
    pub heating_capacity_w: Option<f64>,
    /// Burner thermal efficiency (fraction in (0, 1]).
    pub burner_efficiency: Option<f64>,
    /// Fraction of heat input lost up the flue [0, 1].
    pub flue_loss_fraction: Option<f64>,
    /// Ignition type string (e.g. "standing pilot", "electronic ignition").
    pub ignition_type: Option<String>,
    /// Standing-pilot flame power [W]. Only meaningful when ignition_type is "standing pilot".
    pub pilot_power_w: Option<f64>,
    /// Induced-draft fan electric power [W].
    pub fan_power_w: Option<f64>,
    /// Fraction of standby losses delivered to the zone as sensible heat [0, 1].
    pub skin_loss_fraction: Option<f64>,

    /// Fuel type string (e.g. "Gas", "Propane").
    pub fuel_type: Option<String>,

    /// Fallback mains water temperature [°C].
    pub mains_temp_c: Option<f64>,
    /// Average daily hot-water draw [L/day]; used when no schedule column is present.
    pub avg_water_draw_l_per_day: Option<f64>,
    /// Steady-state draw flow rate [kg/s].
    pub draw_flow_rate_kg_s: Option<f64>,
    /// Schedule column index for draw flow rate (L/min).
    pub draw_flow_rate_schedule_col: Option<u32>,
    /// Schedule column index for mains temperature (°C).
    pub mains_temp_schedule_col: Option<u32>,

    // ZIP voltage model
    pub zip_z: Option<f64>,
    pub zip_i: Option<f64>,
    pub zip_p: Option<f64>,
    pub zip_zq: Option<f64>,
    pub zip_iq: Option<f64>,
    pub zip_pq: Option<f64>,
    pub zip_pf: Option<f64>,
    pub zip_v0: Option<f64>,
}

impl EquipmentTypedConfig for GasWaterHeaterConfig {
    fn equipment_type_name() -> &'static str {
        "Gas Water Heater"
    }
}

impl GasWaterHeaterConfig {
    pub fn validate(&self) -> crate::Result<()> {
        use hares_types::HaresError;
        if let Some(v) = self.tank_volume_m3 {
            if !v.is_finite() || v <= 0.0 {
                return Err(HaresError::Equipment(
                    "gas_wh: tank_volume_m3 must be finite and > 0".to_string(),
                ));
            }
        }
        if let Some(v) = self.heating_capacity_w {
            if !v.is_finite() || v < 0.0 {
                return Err(HaresError::Equipment(
                    "gas_wh: heating_capacity_w must be finite and >= 0".to_string(),
                ));
            }
        }
        if let Some(v) = self.burner_efficiency {
            if !v.is_finite() || v <= 0.0 || v > 1.0 {
                return Err(HaresError::Equipment(
                    "gas_wh: burner_efficiency must be finite and within (0, 1]".to_string(),
                ));
            }
        }
        if let Some(v) = self.flue_loss_fraction {
            if !v.is_finite() || !(0.0..=1.0).contains(&v) {
                return Err(HaresError::Equipment(
                    "gas_wh: flue_loss_fraction must be finite and within [0, 1]".to_string(),
                ));
            }
        }
        if let Some(v) = self.ua_w_per_k {
            if !v.is_finite() || v <= 0.0 {
                return Err(HaresError::Equipment(
                    "gas_wh: ua_w_per_k must be finite and > 0".to_string(),
                ));
            }
        }
        Ok(())
    }
}

/// Typed configuration for an electric resistance storage water heater.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElectricResistanceWaterHeaterConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,

    /// Tank volume [m³].
    pub tank_volume_m3: Option<f64>,
    /// Tank height [m].
    pub tank_height_m: Option<f64>,
    /// Tank diameter [m].
    pub tank_diameter_m: Option<f64>,
    /// Tank heat-loss conductance [W/K].
    pub ua_w_per_k: Option<f64>,
    /// Insulation jacket R-value [m²·K/W].
    pub jacket_r_value_m2_k_w: Option<f64>,
    /// Number of tank stratification nodes.
    pub tank_nodes: Option<u8>,
    /// Index of the upper heating element node (0-based).
    pub upper_element_node: Option<u8>,
    /// Index of the lower heating element node (0-based).
    pub lower_element_node: Option<u8>,

    /// Thermostat setpoint [°C].
    pub setpoint_c: Option<f64>,
    /// Thermostat deadband width [°C].
    pub deadband_c: Option<f64>,
    /// Maximum safe tank temperature [°C].
    pub max_tank_temp_c: Option<f64>,

    /// Rated element power [W]. Applies to both elements when per-element power is absent.
    pub heating_capacity_w: Option<f64>,
    /// Upper element power [W].
    pub upper_element_power_w: Option<f64>,
    /// Lower element power [W].
    pub lower_element_power_w: Option<f64>,
    /// Element priority mode: "MasterSlave" (default) or "Simultaneous".
    pub element_priority_mode: Option<String>,
    /// Setpoint ramp rate [°C/min]; limits how fast the setpoint can change.
    pub max_setpoint_ramp_rate_c_per_min: Option<f64>,

    /// Fallback mains water temperature [°C].
    pub mains_temp_c: Option<f64>,
    /// Average daily hot-water draw [L/day].
    pub avg_water_draw_l_per_day: Option<f64>,
    /// Steady-state draw flow rate [kg/s].
    pub draw_flow_rate_kg_s: Option<f64>,
    /// Schedule column index for draw flow rate (L/min).
    pub draw_flow_rate_schedule_col: Option<u32>,
    /// Schedule column index for mains temperature (°C).
    pub mains_temp_schedule_col: Option<u32>,

    // ZIP voltage model
    pub zip_z: Option<f64>,
    pub zip_i: Option<f64>,
    pub zip_p: Option<f64>,
    pub zip_zq: Option<f64>,
    pub zip_iq: Option<f64>,
    pub zip_pq: Option<f64>,
    pub zip_pf: Option<f64>,
    pub zip_v0: Option<f64>,
}

impl EquipmentTypedConfig for ElectricResistanceWaterHeaterConfig {
    fn equipment_type_name() -> &'static str {
        "Electric Resistance Water Heater"
    }
}

impl ElectricResistanceWaterHeaterConfig {
    pub fn validate(&self) -> crate::Result<()> {
        use hares_types::HaresError;
        if let Some(v) = self.tank_volume_m3 {
            if !v.is_finite() || v <= 0.0 {
                return Err(HaresError::Equipment(
                    "resistance_wh: tank_volume_m3 must be finite and > 0".to_string(),
                ));
            }
        }
        if let Some(v) = self.ua_w_per_k {
            if !v.is_finite() || v <= 0.0 {
                return Err(HaresError::Equipment(
                    "resistance_wh: ua_w_per_k must be finite and > 0".to_string(),
                ));
            }
        }
        for (name, val) in [
            ("heating_capacity_w", self.heating_capacity_w),
            ("upper_element_power_w", self.upper_element_power_w),
            ("lower_element_power_w", self.lower_element_power_w),
        ] {
            if let Some(v) = val {
                if !v.is_finite() || v < 0.0 {
                    return Err(HaresError::Equipment(format!(
                        "resistance_wh: {name} must be finite and >= 0"
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Typed configuration for a tankless (instantaneous) water heater.
///
/// Used for both gas and electric variants; the `fuel_type` field distinguishes them.
/// The registry uses two keys: "Tankless Water Heater" (electric) and
/// "Gas Tankless Water Heater" (gas).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TanklessWaterHeaterConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,

    /// Fuel type string: "Electric" or "Gas".
    pub fuel_type: Option<String>,

    /// Thermostat setpoint [°C].
    pub setpoint_c: Option<f64>,
    /// Thermal efficiency / energy factor (fraction).
    pub efficiency_factor: Option<f64>,
    /// ANSI/RESNET 301 performance adjustment multiplier (typical: 0.92).
    pub performance_adjustment: Option<f64>,
    /// Rated maximum thermal output power [W].
    pub max_thermal_power_w: Option<f64>,
    /// Continuous standby parasitic electric power [W].
    pub parasitic_power_w: Option<f64>,

    /// Fallback inlet (mains) temperature [°C].
    pub inlet_temp_c: Option<f64>,
    /// Average daily hot-water draw [L/day].
    pub avg_water_draw_l_per_day: Option<f64>,
    /// Steady-state draw flow rate [kg/s].
    pub draw_flow_rate_kg_s: Option<f64>,

    // ZIP voltage model
    pub zip_z: Option<f64>,
    pub zip_i: Option<f64>,
    pub zip_p: Option<f64>,
    pub zip_zq: Option<f64>,
    pub zip_iq: Option<f64>,
    pub zip_pq: Option<f64>,
    pub zip_pf: Option<f64>,
    pub zip_v0: Option<f64>,
}

impl EquipmentTypedConfig for TanklessWaterHeaterConfig {
    fn equipment_type_name() -> &'static str {
        "Tankless Water Heater"
    }
}

impl TanklessWaterHeaterConfig {
    pub fn validate(&self) -> crate::Result<()> {
        use hares_types::HaresError;
        if let Some(v) = self.max_thermal_power_w {
            if !v.is_finite() || v < 0.0 {
                return Err(HaresError::Equipment(
                    "tankless_wh: max_thermal_power_w must be finite and >= 0".to_string(),
                ));
            }
        }
        if let Some(v) = self.efficiency_factor {
            if !v.is_finite() || v <= 0.0 {
                return Err(HaresError::Equipment(
                    "tankless_wh: efficiency_factor must be finite and > 0".to_string(),
                ));
            }
        }
        if let Some(v) = self.performance_adjustment {
            if !v.is_finite() || !(0.0..=1.0).contains(&v) {
                return Err(HaresError::Equipment(
                    "tankless_wh: performance_adjustment must be finite and within [0, 1]"
                        .to_string(),
                ));
            }
        }
        Ok(())
    }
}

/// Typed configuration for a heat pump water heater (HPWH).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeatPumpWaterHeaterConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,

    /// Tank volume [m³].
    pub tank_volume_m3: Option<f64>,
    /// Tank height [m].
    pub tank_height_m: Option<f64>,
    /// Tank diameter [m].
    pub tank_diameter_m: Option<f64>,
    /// Tank heat-loss conductance [W/K].
    pub ua_w_per_k: Option<f64>,
    /// Insulation jacket R-value [m²·K/W].
    pub jacket_r_value_m2_k_w: Option<f64>,
    /// Number of tank stratification nodes.
    pub tank_nodes: Option<u8>,
    /// Index of the lower thermostat node (0-based).
    pub thermostat_node: Option<u8>,
    /// Index of the upper thermostat node for composite control temperature.
    pub thermostat_upper_node: Option<u8>,
    /// Index of the condenser node (0-based).
    pub condenser_node: Option<u8>,

    /// Thermostat setpoint [°C].
    pub setpoint_c: Option<f64>,
    /// Thermostat deadband width [°C].
    pub deadband_c: Option<f64>,
    /// Maximum safe tank temperature [°C].
    pub max_tank_temp_c: Option<f64>,
    /// Setpoint ramp rate [°C/min].
    pub max_setpoint_ramp_rate_c_per_min: Option<f64>,

    /// Compressor input power at rated conditions [W].
    pub compressor_power_w: Option<f64>,
    /// Backup resistance element power [W].
    pub backup_element_power_w: Option<f64>,
    /// Temperature offset below setpoint at which backup element is enabled [°C].
    pub backup_enable_offset_c: Option<f64>,
    /// Backup element efficiency (fraction).
    pub backup_efficiency: Option<f64>,
    /// When true, the backup resistance element is permanently disabled.
    pub hp_only_mode: Option<bool>,

    /// Rated absolute COP (not normalised).  When absent, derived from uniform_energy_factor.
    pub cop: Option<f64>,
    /// Uniform Energy Factor; used to derive COP when `cop` is absent.
    pub uniform_energy_factor: Option<f64>,
    /// COP biquadratic curve coefficients [a0..a5].
    pub cop_curve_coeffs: Option<Vec<f64>>,
    /// Capacity biquadratic curve coefficients [a0..a5].
    pub capacity_curve_coeffs: Option<Vec<f64>>,
    /// Minimum zone-air temperature for compressor operation [°C].
    pub min_ambient_temp_c: Option<f64>,
    /// Maximum zone-air temperature for compressor operation [°C].
    pub max_ambient_temp_c: Option<f64>,
    /// Set to "true" or "1" for low-power HPWH variant (UEF ≈ 4.9).
    pub low_power_hpwh: Option<bool>,

    /// Evaporator sensible heat ratio [0, 1].
    pub shr: Option<f64>,
    /// Fraction of waste heat that exits the building [0, 1].
    pub lost_heat_fraction: Option<f64>,
    /// Fraction of sensible zone heat that goes to interior wall surfaces [0, 1].
    pub wall_heat_fraction: Option<f64>,
    /// Evaporator fan power [W].
    pub fan_power_w: Option<f64>,
    /// Standby parasitic power [W].
    pub parasitic_power_w: Option<f64>,

    /// Minimum compressor on-time before an off-transition is allowed [s].
    pub min_on_time_s: Option<f64>,
    /// Minimum compressor off-time before a restart is allowed [s].
    pub min_off_time_s: Option<f64>,

    /// Tempering valve outlet temperature [°C]. `None` = no valve.
    pub tempering_valve_setpoint_c: Option<f64>,
    /// Element/HP control mode: "MutuallyExclusive" (default) or "Simultaneous".
    pub element_hp_control_mode: Option<String>,

    /// Fallback mains water temperature [°C].
    pub mains_temp_c: Option<f64>,
    /// Average daily hot-water draw [L/day].
    pub avg_water_draw_l_per_day: Option<f64>,
    /// Steady-state draw flow rate [kg/s].
    pub draw_flow_rate_kg_s: Option<f64>,

    // ZIP voltage model
    pub zip_z: Option<f64>,
    pub zip_i: Option<f64>,
    pub zip_p: Option<f64>,
    pub zip_zq: Option<f64>,
    pub zip_iq: Option<f64>,
    pub zip_pq: Option<f64>,
    pub zip_pf: Option<f64>,
    pub zip_v0: Option<f64>,
}

impl EquipmentTypedConfig for HeatPumpWaterHeaterConfig {
    fn equipment_type_name() -> &'static str {
        "Heat Pump Water Heater"
    }
}

impl HeatPumpWaterHeaterConfig {
    pub fn validate(&self) -> crate::Result<()> {
        use hares_types::HaresError;
        if let Some(v) = self.tank_volume_m3 {
            if !v.is_finite() || v <= 0.0 {
                return Err(HaresError::Equipment(
                    "hpwh: tank_volume_m3 must be finite and > 0".to_string(),
                ));
            }
        }
        if let Some(v) = self.ua_w_per_k {
            if !v.is_finite() || v <= 0.0 {
                return Err(HaresError::Equipment(
                    "hpwh: ua_w_per_k must be finite and > 0".to_string(),
                ));
            }
        }
        for (name, val) in [
            ("compressor_power_w", self.compressor_power_w),
            ("backup_element_power_w", self.backup_element_power_w),
            ("fan_power_w", self.fan_power_w),
            ("parasitic_power_w", self.parasitic_power_w),
        ] {
            if let Some(v) = val {
                if !v.is_finite() || v < 0.0 {
                    return Err(HaresError::Equipment(format!(
                        "hpwh: {name} must be finite and >= 0"
                    )));
                }
            }
        }
        if let Some(v) = self.cop {
            if !v.is_finite() || v <= 0.0 {
                return Err(HaresError::Equipment(
                    "hpwh: cop must be finite and > 0".to_string(),
                ));
            }
        }
        if let Some(v) = self.uniform_energy_factor {
            if !v.is_finite() || v <= 0.0 {
                return Err(HaresError::Equipment(
                    "hpwh: uniform_energy_factor must be finite and > 0".to_string(),
                ));
            }
        }
        if let Some(v) = self.shr {
            if !v.is_finite() || !(0.0..=1.0).contains(&v) {
                return Err(HaresError::Equipment(
                    "hpwh: shr must be finite and within [0, 1]".to_string(),
                ));
            }
        }
        if let Some(v) = self.lost_heat_fraction {
            if !v.is_finite() || !(0.0..=1.0).contains(&v) {
                return Err(HaresError::Equipment(
                    "hpwh: lost_heat_fraction must be finite and within [0, 1]".to_string(),
                ));
            }
        }
        if let Some(coeffs) = &self.cop_curve_coeffs {
            if coeffs.len() != 6 {
                return Err(HaresError::Equipment(
                    "hpwh: cop_curve_coeffs must have exactly 6 elements".to_string(),
                ));
            }
        }
        if let Some(coeffs) = &self.capacity_curve_coeffs {
            if coeffs.len() != 6 {
                return Err(HaresError::Equipment(
                    "hpwh: capacity_curve_coeffs must have exactly 6 elements".to_string(),
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

    fn minimal_gas_wh() -> GasWaterHeaterConfig {
        GasWaterHeaterConfig {
            equipment_id: None,
            zone_id: None,
            tank_volume_m3: Some(0.189),
            tank_height_m: None,
            tank_diameter_m: None,
            ua_w_per_k: Some(2.0),
            jacket_r_value_m2_k_w: None,
            tank_nodes: None,
            burner_node: None,
            setpoint_c: Some(51.67),
            deadband_c: None,
            max_tank_temp_c: None,
            heating_capacity_w: Some(11_000.0),
            burner_efficiency: Some(0.78),
            flue_loss_fraction: None,
            ignition_type: None,
            pilot_power_w: None,
            fan_power_w: None,
            skin_loss_fraction: None,
            fuel_type: None,
            mains_temp_c: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: None,
            draw_flow_rate_schedule_col: None,
            mains_temp_schedule_col: None,
            zip_z: None,
            zip_i: None,
            zip_p: None,
            zip_zq: None,
            zip_iq: None,
            zip_pq: None,
            zip_pf: None,
            zip_v0: None,
        }
    }

    fn minimal_resistance_wh() -> ElectricResistanceWaterHeaterConfig {
        ElectricResistanceWaterHeaterConfig {
            equipment_id: None,
            zone_id: None,
            tank_volume_m3: Some(0.189),
            tank_height_m: None,
            tank_diameter_m: None,
            ua_w_per_k: Some(2.0),
            jacket_r_value_m2_k_w: None,
            tank_nodes: None,
            upper_element_node: None,
            lower_element_node: None,
            setpoint_c: Some(51.67),
            deadband_c: None,
            max_tank_temp_c: None,
            heating_capacity_w: Some(4_500.0),
            upper_element_power_w: None,
            lower_element_power_w: None,
            element_priority_mode: None,
            max_setpoint_ramp_rate_c_per_min: None,
            mains_temp_c: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: None,
            draw_flow_rate_schedule_col: None,
            mains_temp_schedule_col: None,
            zip_z: None,
            zip_i: None,
            zip_p: None,
            zip_zq: None,
            zip_iq: None,
            zip_pq: None,
            zip_pf: None,
            zip_v0: None,
        }
    }

    fn minimal_tankless_wh() -> TanklessWaterHeaterConfig {
        TanklessWaterHeaterConfig {
            equipment_id: None,
            zone_id: None,
            fuel_type: Some("Gas".to_string()),
            setpoint_c: Some(51.67),
            efficiency_factor: Some(0.9),
            performance_adjustment: Some(0.92),
            max_thermal_power_w: Some(20_000.0),
            parasitic_power_w: None,
            inlet_temp_c: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: None,
            zip_z: None,
            zip_i: None,
            zip_p: None,
            zip_zq: None,
            zip_iq: None,
            zip_pq: None,
            zip_pf: None,
            zip_v0: None,
        }
    }

    fn minimal_hpwh() -> HeatPumpWaterHeaterConfig {
        HeatPumpWaterHeaterConfig {
            equipment_id: None,
            zone_id: None,
            tank_volume_m3: Some(0.189),
            tank_height_m: None,
            tank_diameter_m: None,
            ua_w_per_k: Some(2.0),
            jacket_r_value_m2_k_w: None,
            tank_nodes: None,
            thermostat_node: None,
            thermostat_upper_node: None,
            condenser_node: None,
            setpoint_c: Some(51.67),
            deadband_c: None,
            max_tank_temp_c: None,
            max_setpoint_ramp_rate_c_per_min: None,
            compressor_power_w: Some(500.0),
            backup_element_power_w: Some(4_500.0),
            backup_enable_offset_c: None,
            backup_efficiency: None,
            hp_only_mode: None,
            cop: Some(3.5),
            uniform_energy_factor: None,
            cop_curve_coeffs: None,
            capacity_curve_coeffs: None,
            min_ambient_temp_c: None,
            max_ambient_temp_c: None,
            low_power_hpwh: None,
            shr: None,
            lost_heat_fraction: None,
            wall_heat_fraction: None,
            fan_power_w: None,
            parasitic_power_w: None,
            min_on_time_s: None,
            min_off_time_s: None,
            tempering_valve_setpoint_c: None,
            element_hp_control_mode: None,
            mains_temp_c: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: None,
            zip_z: None,
            zip_i: None,
            zip_p: None,
            zip_zq: None,
            zip_iq: None,
            zip_pq: None,
            zip_pf: None,
            zip_v0: None,
        }
    }

    #[test]
    fn gas_wh_config_round_trips() {
        let cfg = minimal_gas_wh();
        let ec = EquipmentConfig::from_typed(
            "gas_wh".to_string(),
            "Gas Water Heater".to_string(),
            cfg.clone(),
        );
        assert!(ec.is_typed());
        let recovered: GasWaterHeaterConfig = ec.typed().unwrap();
        assert_eq!(recovered.tank_volume_m3, cfg.tank_volume_m3);
        assert_eq!(recovered.heating_capacity_w, cfg.heating_capacity_w);
    }

    #[test]
    fn gas_wh_config_rejects_unknown_fields() {
        let json = serde_json::json!({
            "tank_volume_m3": 0.189,
            "unknown_field": 99.0
        });
        let ec = EquipmentConfig {
            name: "test".to_string(),
            ochre_class: "Gas Water Heater".to_string(),
            payload: ConfigPayload::Typed {
                type_name: "Gas Water Heater".to_string(),
                version: 1,
                data: json,
            },
        };
        let result: crate::Result<GasWaterHeaterConfig> = ec.typed();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown field"));
    }

    #[test]
    fn gas_wh_validate_rejects_zero_volume() {
        let mut cfg = minimal_gas_wh();
        cfg.tank_volume_m3 = Some(0.0);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn gas_wh_validate_rejects_out_of_range_efficiency() {
        let mut cfg = minimal_gas_wh();
        cfg.burner_efficiency = Some(1.5);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn gas_wh_validate_passes_for_valid_config() {
        let cfg = minimal_gas_wh();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn resistance_wh_config_round_trips() {
        let cfg = minimal_resistance_wh();
        let ec = EquipmentConfig::from_typed(
            "res_wh".to_string(),
            "Electric Resistance Water Heater".to_string(),
            cfg.clone(),
        );
        assert!(ec.is_typed());
        let recovered: ElectricResistanceWaterHeaterConfig = ec.typed().unwrap();
        assert_eq!(recovered.tank_volume_m3, cfg.tank_volume_m3);
        assert_eq!(recovered.heating_capacity_w, cfg.heating_capacity_w);
    }

    #[test]
    fn resistance_wh_config_rejects_unknown_fields() {
        let json = serde_json::json!({
            "tank_volume_m3": 0.189,
            "mystery": true
        });
        let ec = EquipmentConfig {
            name: "test".to_string(),
            ochre_class: "Electric Resistance Water Heater".to_string(),
            payload: ConfigPayload::Typed {
                type_name: "Electric Resistance Water Heater".to_string(),
                version: 1,
                data: json,
            },
        };
        let result: crate::Result<ElectricResistanceWaterHeaterConfig> = ec.typed();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown field"));
    }

    #[test]
    fn resistance_wh_validate_passes_for_valid_config() {
        let cfg = minimal_resistance_wh();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn tankless_wh_config_round_trips() {
        let cfg = minimal_tankless_wh();
        let ec = EquipmentConfig::from_typed(
            "tankless".to_string(),
            "Tankless Water Heater".to_string(),
            cfg.clone(),
        );
        assert!(ec.is_typed());
        let recovered: TanklessWaterHeaterConfig = ec.typed().unwrap();
        assert_eq!(recovered.max_thermal_power_w, cfg.max_thermal_power_w);
        assert_eq!(recovered.fuel_type, cfg.fuel_type);
    }

    #[test]
    fn tankless_wh_config_rejects_unknown_fields() {
        let json = serde_json::json!({
            "fuel_type": "Gas",
            "bogus_param": 0.5
        });
        let ec = EquipmentConfig {
            name: "test".to_string(),
            ochre_class: "Gas Tankless Water Heater".to_string(),
            payload: ConfigPayload::Typed {
                type_name: "Tankless Water Heater".to_string(),
                version: 1,
                data: json,
            },
        };
        let result: crate::Result<TanklessWaterHeaterConfig> = ec.typed();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown field"));
    }

    #[test]
    fn tankless_wh_validate_rejects_negative_power() {
        let mut cfg = minimal_tankless_wh();
        cfg.max_thermal_power_w = Some(-1.0);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn tankless_wh_validate_passes_for_valid_config() {
        let cfg = minimal_tankless_wh();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn hpwh_config_round_trips() {
        let cfg = minimal_hpwh();
        let ec = EquipmentConfig::from_typed(
            "hpwh".to_string(),
            "Heat Pump Water Heater".to_string(),
            cfg.clone(),
        );
        assert!(ec.is_typed());
        let recovered: HeatPumpWaterHeaterConfig = ec.typed().unwrap();
        assert_eq!(recovered.tank_volume_m3, cfg.tank_volume_m3);
        assert_eq!(recovered.cop, cfg.cop);
    }

    #[test]
    fn hpwh_config_rejects_unknown_fields() {
        let json = serde_json::json!({
            "cop": 3.5,
            "unknown_param": "bad"
        });
        let ec = EquipmentConfig {
            name: "test".to_string(),
            ochre_class: "Heat Pump Water Heater".to_string(),
            payload: ConfigPayload::Typed {
                type_name: "Heat Pump Water Heater".to_string(),
                version: 1,
                data: json,
            },
        };
        let result: crate::Result<HeatPumpWaterHeaterConfig> = ec.typed();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown field"));
    }

    #[test]
    fn hpwh_validate_rejects_negative_cop() {
        let mut cfg = minimal_hpwh();
        cfg.cop = Some(-1.0);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn hpwh_validate_rejects_wrong_curve_length() {
        let mut cfg = minimal_hpwh();
        cfg.cop_curve_coeffs = Some(vec![1.0, 2.0, 3.0]); // needs 6
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn hpwh_validate_passes_for_valid_config() {
        let cfg = minimal_hpwh();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn hpwh_validate_rejects_zero_uef() {
        let mut cfg = minimal_hpwh();
        cfg.uniform_energy_factor = Some(0.0);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn hpwh_validate_rejects_negative_uef() {
        let mut cfg = minimal_hpwh();
        cfg.uniform_energy_factor = Some(-1.5);
        assert!(cfg.validate().is_err());
    }
}
