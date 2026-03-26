use hares_types::EvConnectionState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct EvCheckpoint {
    pub(super) soc: f64,
    pub(super) connection_state: EvConnectionState,
    pub(super) away_charger_power_kw: f64,
    pub(super) active_power_kw: f64,
    pub(super) power_limit_kw: Option<f64>,
    pub(super) power_setpoint_kw: Option<f64>,
    pub(super) soc_target: Option<f64>,
    pub(super) soc_target_min: Option<f64>,
    pub(super) soc_target_max: Option<f64>,
    pub(super) battery_temp_c: f64,
    pub(super) heater_active: bool,
    pub(super) ready_soc: f64,
    pub(super) ready_by_hour: Option<f64>,
    pub(super) ready_by_soc: Option<f64>,
    pub(super) v2l_enabled: bool,
    pub(super) v2l_soc_reserve: f64,
    pub(super) v2l_max_discharge_kw: f64,
    pub(super) v2g_enabled: bool,
    pub(super) v2g_soc_reserve: f64,
    pub(super) v2g_max_discharge_kw: f64,
    pub(super) degradation: crate::battery::degradation::DegradationState,
    pub(super) rainflow: crate::battery::degradation::RainflowCounter,
    pub(super) last_daily_update_day: i32,
}
