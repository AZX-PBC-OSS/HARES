//! Equivalent Battery Model (EBM) for HVAC equipment.

use super::hvac_core::HvacEquipment;
use super::thermostat::ThermostatMode;

/// Equivalent Battery Model (EBM) parameters for HVAC equipment.
///
/// Matches OCHRE `Equipment.make_equivalent_battery_model()` (Equipment.py:260-286).
/// Model: `E_dot = eta * P - P_b`, with `E_min <= E <= E_max`, `0 <= P <= P_max`.
#[derive(Debug, Clone, PartialEq)]
pub struct EquivalentBatteryModel {
    /// Current energy state [kWh]. `None` when equipment is in Deadband mode.
    pub energy_kwh: Option<f64>,
    /// Minimum allowed energy state [kWh]. Always 0.0 for HVAC.
    pub min_energy_kwh: f64,
    /// Maximum energy state [kWh]. `None` when rated capacity is unavailable.
    pub max_energy_kwh: Option<f64>,
    /// Maximum power consumption [kW]. `None` when rated capacity is unavailable.
    pub max_power_kw: Option<f64>,
    /// Charging efficiency [-]. 1.0 for HVAC equipment.
    pub efficiency: f64,
    /// Baseline disturbance power [kW] to maintain current state. 0.0 for HVAC.
    pub baseline_power_kw: f64,
}

impl HvacEquipment {
    /// Compute Equivalent Battery Model parameters from current equipment state.
    ///
    /// The thermal energy state represents how much of the thermostat deadband has been
    /// filled. `max_energy_kwh = max_power_kw * time_res_s / 3600` (energy deliverable
    /// in one timestep). The fill fraction is derived from how far the zone temperature
    /// sits within the current deadband.
    ///
    /// Returns `None` fields in Deadband mode or when rated capacity is zero.
    #[must_use]
    pub fn make_equivalent_battery_model(
        &self,
        zone_temp_c: f64,
        time_res_s: f64,
    ) -> EquivalentBatteryModel {
        let setpoints = self.effective_setpoints();
        let hysteresis = self.thermostat_fsm.thermostat.hysteresis_c;

        let (max_power_kw, energy_kwh, max_energy_kwh) = match self.thermostat_fsm.mode {
            ThermostatMode::Heating => {
                let rated_w = self.rated_capacity_w(ThermostatMode::Heating);
                if rated_w <= 0.0 {
                    (None, None, None)
                } else {
                    let max_kw = rated_w / 1_000.0;
                    let t_min = setpoints.heating_c - hysteresis;
                    let t_max = setpoints.heating_c;
                    let deadband_range = (t_max - t_min).max(f64::EPSILON);
                    let max_kwh = max_kw * time_res_s / 3_600.0;
                    let fill = ((zone_temp_c - t_min) / deadband_range).clamp(0.0, 1.0);
                    (Some(max_kw), Some(fill * max_kwh), Some(max_kwh))
                }
            }
            ThermostatMode::Cooling => {
                let rated_w = self.rated_capacity_w(ThermostatMode::Cooling);
                if rated_w <= 0.0 {
                    (None, None, None)
                } else {
                    let max_kw = rated_w / 1_000.0;
                    let t_min = setpoints.cooling_c;
                    let t_max = setpoints.cooling_c + hysteresis;
                    let deadband_range = (t_max - t_min).max(f64::EPSILON);
                    let max_kwh = max_kw * time_res_s / 3_600.0;
                    let fill = ((t_max - zone_temp_c) / deadband_range).clamp(0.0, 1.0);
                    (Some(max_kw), Some(fill * max_kwh), Some(max_kwh))
                }
            }
            ThermostatMode::Deadband => (None, None, None),
        };

        EquivalentBatteryModel {
            energy_kwh,
            min_energy_kwh: 0.0,
            max_energy_kwh,
            max_power_kw,
            efficiency: 1.0,
            baseline_power_kw: 0.0,
        }
    }
}
