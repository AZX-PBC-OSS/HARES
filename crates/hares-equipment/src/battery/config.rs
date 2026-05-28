//! Typed configuration for battery storage equipment.

use serde::{Deserialize, Serialize};

use crate::config::EquipmentTypedConfig;

/// Typed configuration for a residential battery storage system.
///
/// `inverter_efficiency` is the one-way charge/discharge efficiency as a
/// fraction in (0, 1]. HPXML resolvers convert round-trip efficiency to
/// one-way by applying `sqrt()` once before constructing this struct.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatteryConfig {
    // Identity
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,

    // Core energy / power
    pub capacity_kwh: f64,
    pub max_charge_kw: f64,
    pub max_discharge_kw: f64,

    // Pack topology
    pub n_series: Option<u32>,
    pub n_parallel: Option<u32>,
    pub ah_cell: Option<f64>,
    pub v_cell: Option<f64>,
    pub cell_resistance_ohm: Option<f64>,
    /// Target pack voltage (V). Overrides the hardcoded 350 V used for
    /// ah_cell/v_cell topology derivation.
    pub pack_voltage_v: Option<f64>,

    // Chemistry
    pub chemistry: Option<String>,

    // Standby / self-discharge
    pub standby_power_w: Option<f64>,
    pub self_discharge_pct_per_day: Option<f64>,

    // SOC bounds and initial state
    pub min_soc: Option<f64>,
    pub max_soc: Option<f64>,
    pub initial_soc: Option<f64>,
    /// Initial cell temperature (°C). When `None`, the temperature is derived
    /// from the zone or ambient environment. When `Some`, overrides the
    /// ambient derivation to allow warm-start or cold-start scenarios.
    pub initial_cell_temp_c: Option<f64>,

    // Limits
    pub import_limit_w: Option<f64>,
    pub export_limit_w: Option<f64>,

    // Heater
    pub heater_power_w: Option<f64>,
    pub heater_threshold_c: Option<f64>,
    pub heater_on_discharge: Option<bool>,

    // Thermal model
    pub min_discharge_temp_c: Option<f64>,
    pub full_power_temp_c: Option<f64>,
    pub min_charge_temp_c: Option<f64>,
    pub cell_thermal_mass_j_per_k: Option<f64>,
    pub cell_ua_w_per_k: Option<f64>,

    // Efficiency -- one-way charge/discharge efficiency.
    // Overridden per-direction by charge_efficiency / discharge_efficiency.
    pub inverter_efficiency: Option<f64>,
    pub charge_efficiency: Option<f64>,
    pub discharge_efficiency: Option<f64>,

    // BMS / grid rules (serialised as string, parsed at init)
    pub bms_mode: Option<String>,
    pub grid_export_rule: Option<String>,
}

impl EquipmentTypedConfig for BatteryConfig {
    fn equipment_type_name() -> &'static str {
        "Battery"
    }
}

impl BatteryConfig {
    /// Validate fields for physical plausibility.
    pub fn validate(&self) -> crate::Result<()> {
        use hares_types::HaresError;
        if !self.capacity_kwh.is_finite() || self.capacity_kwh <= 0.0 {
            return Err(HaresError::Equipment(
                "battery capacity_kwh must be finite and > 0".to_string(),
            ));
        }
        if !self.max_charge_kw.is_finite() || self.max_charge_kw <= 0.0 {
            return Err(HaresError::Equipment(
                "battery max_charge_kw must be finite and > 0".to_string(),
            ));
        }
        if !self.max_discharge_kw.is_finite() || self.max_discharge_kw <= 0.0 {
            return Err(HaresError::Equipment(
                "battery max_discharge_kw must be finite and > 0".to_string(),
            ));
        }
        if let (Some(min), Some(max)) = (self.min_soc, self.max_soc) {
            if min >= max {
                return Err(HaresError::Equipment(
                    "battery min_soc must be less than max_soc".to_string(),
                ));
            }
        }
        if let Some(n) = self.n_series {
            if n == 0 {
                return Err(HaresError::Equipment(
                    "battery n_series must be > 0".to_string(),
                ));
            }
        }
        if let Some(n) = self.n_parallel {
            if n == 0 {
                return Err(HaresError::Equipment(
                    "battery n_parallel must be > 0".to_string(),
                ));
            }
        }
        for (name, val) in [
            ("import_limit_w", self.import_limit_w),
            ("export_limit_w", self.export_limit_w),
            ("heater_power_w", self.heater_power_w),
            ("standby_power_w", self.standby_power_w),
        ] {
            if let Some(v) = val {
                if !v.is_finite() || v < 0.0 {
                    return Err(HaresError::Equipment(format!(
                        "battery {name} must be finite and >= 0"
                    )));
                }
            }
        }
        for (name, val) in [
            ("inverter_efficiency", self.inverter_efficiency),
            ("charge_efficiency", self.charge_efficiency),
            ("discharge_efficiency", self.discharge_efficiency),
        ] {
            if let Some(v) = val {
                if !v.is_finite() || v <= 0.0 || v > 1.0 {
                    return Err(HaresError::Equipment(format!(
                        "battery {name} must be finite and within (0, 1]"
                    )));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigPayload, EquipmentConfig};

    fn minimal_battery_config() -> BatteryConfig {
        BatteryConfig {
            equipment_id: None,
            zone_id: None,
            capacity_kwh: 13.5,
            max_charge_kw: 5.0,
            max_discharge_kw: 5.0,
            n_series: None,
            n_parallel: None,
            ah_cell: None,
            v_cell: None,
            cell_resistance_ohm: None,
            pack_voltage_v: None,
            chemistry: None,
            standby_power_w: None,
            self_discharge_pct_per_day: None,
            min_soc: None,
            max_soc: None,
            initial_soc: None,
            initial_cell_temp_c: None,
            import_limit_w: None,
            export_limit_w: None,
            heater_power_w: None,
            heater_threshold_c: None,
            heater_on_discharge: None,
            min_discharge_temp_c: None,
            full_power_temp_c: None,
            min_charge_temp_c: None,
            cell_thermal_mass_j_per_k: None,
            cell_ua_w_per_k: None,
            inverter_efficiency: None,
            charge_efficiency: None,
            discharge_efficiency: None,
            bms_mode: None,
            grid_export_rule: None,
        }
    }

    #[test]
    fn battery_config_round_trips_via_equipment_config() {
        let cfg = minimal_battery_config();
        let ec = EquipmentConfig::from_typed(
            "test_battery".to_string(),
            "Battery".to_string(),
            cfg.clone(),
        );
        assert!(ec.is_typed());
        let recovered: BatteryConfig = ec.typed().unwrap();
        assert_eq!(recovered.capacity_kwh, cfg.capacity_kwh);
        assert_eq!(recovered.max_charge_kw, cfg.max_charge_kw);
    }

    #[test]
    fn battery_config_rejects_unknown_fields() {
        let json = serde_json::json!({
            "capacity_kwh": 13.5,
            "max_charge_kw": 5.0,
            "max_discharge_kw": 5.0,
            "unknown_field": 42.0
        });
        let ec = EquipmentConfig::with_payload(
            "test".to_string(),
            "Battery".to_string(),
            ConfigPayload::Typed {
                type_name: "Battery".to_string(),
                version: 1,
                data: json,
            },
        );
        let result: crate::Result<BatteryConfig> = ec.typed();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown field"));
    }

    #[test]
    fn battery_config_validate_rejects_zero_capacity() {
        let mut cfg = minimal_battery_config();
        cfg.capacity_kwh = 0.0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn battery_config_validate_rejects_inverted_soc_bounds() {
        let mut cfg = minimal_battery_config();
        cfg.min_soc = Some(0.9);
        cfg.max_soc = Some(0.1);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn battery_config_validate_rejects_out_of_range_efficiency() {
        let mut cfg = minimal_battery_config();
        cfg.inverter_efficiency = Some(1.5);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn battery_config_validate_passes_for_valid_config() {
        let cfg = minimal_battery_config();
        assert!(cfg.validate().is_ok());
    }
}
