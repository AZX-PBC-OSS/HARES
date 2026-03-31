//! Typed configuration for battery storage equipment.

use serde::{Deserialize, Serialize};

use crate::config::EquipmentTypedConfig;

/// Typed configuration for a residential battery storage system.
///
/// `inverter_efficiency` is the symmetric round-trip efficiency (RTE) as a
/// fraction in (0, 1].  The equipment splits it as `sqrt(rte)` per direction —
/// applying `sqrt()` exactly once.  Do NOT pre-apply `sqrt()` when constructing
/// this struct; pass the raw RTE value directly.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatteryConfig {
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

    // Chemistry
    pub chemistry: Option<String>,

    // Standby / self-discharge
    pub standby_power_w: Option<f64>,
    pub self_discharge_pct_per_day: Option<f64>,

    // SOC bounds and initial state
    pub min_soc: Option<f64>,
    pub max_soc: Option<f64>,
    pub initial_soc: Option<f64>,

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

    // Efficiency — symmetric RTE; split as sqrt(rte) per direction.
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
