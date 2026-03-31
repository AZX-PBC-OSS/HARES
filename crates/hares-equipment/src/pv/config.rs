//! Typed configuration for PV equipment.

use serde::{Deserialize, Serialize};

use crate::config::EquipmentTypedConfig;

/// Typed configuration for a photovoltaic system.
///
/// Supports a single array (the common residential case). Multi-array
/// systems continue to use the raw config path via `array_count`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PvConfig {
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
