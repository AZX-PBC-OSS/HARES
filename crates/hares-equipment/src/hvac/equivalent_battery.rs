//! Equivalent Battery Model (EBM) for HVAC equipment.

use super::hvac_core::HvacEquipment;
use super::thermostat::ThermostatMode;

/// Equivalent Battery Model (EBM) parameters for HVAC equipment.
///
/// Matches OCHRE `HVAC.make_equivalent_battery_model()` (HVAC.py:620-641).
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
    /// filled. `max_energy_kwh = zone_capacitance_kwh_per_k * deadband_range_c`,
    /// following OCHRE's `total_capacitance * (max_temp - min_temp)` pattern
    /// (HVAC.py:627-637). The fill fraction is derived from how far the zone
    /// temperature sits within the current deadband, using the thermostat FSM's
    /// asymmetric turn-on / turn-off thresholds so the EBM state-of-charge aligns
    /// with actual equipment switching behaviour (thermostat.rs:403-409).
    ///
    /// Returns `None` fields in Deadband mode or when rated capacity is zero.
    #[must_use]
    pub fn make_equivalent_battery_model(
        &self,
        zone_temp_c: f64,
        zone_capacitance_kwh_per_k: f64,
    ) -> EquivalentBatteryModel {
        let setpoints = self.effective_setpoints();
        let hysteresis = self.thermostat_fsm.thermostat.hysteresis_c;
        let offset = self
            .thermostat_fsm
            .thermostat
            .deadband_offset
            .clamp(0.0, 1.0);

        let (max_power_kw, energy_kwh, max_energy_kwh) = match self.thermostat_fsm.mode {
            ThermostatMode::Heating => {
                let rated_w = self.rated_capacity_w(ThermostatMode::Heating);
                if rated_w <= 0.0 {
                    (None, None, None)
                } else {
                    let max_kw = rated_w / 1_000.0;
                    // Turn-on / turn-off thresholds matching thermostat FSM update_mode
                    // (thermostat.rs:403-409). OCHRE deadband_offset convention
                    // (HVAC.py:628-633): turn_on at setpoint - hysteresis * (1 - offset),
                    // turn_off at setpoint + hysteresis * offset.
                    let t_on = setpoints.heating_c - hysteresis * (1.0 - offset);
                    let t_off = setpoints.heating_c + hysteresis * offset;
                    let deadband_range = (t_off - t_on).max(f64::EPSILON);
                    let max_kwh = zone_capacitance_kwh_per_k * deadband_range;
                    // fill = 0 at t_on (cold, empty battery), 1 at t_off (warm, full)
                    let fill = ((zone_temp_c - t_on) / deadband_range).clamp(0.0, 1.0);
                    (Some(max_kw), Some(fill * max_kwh), Some(max_kwh))
                }
            }
            ThermostatMode::Cooling => {
                let rated_w = self.rated_capacity_w(ThermostatMode::Cooling);
                if rated_w <= 0.0 {
                    (None, None, None)
                } else {
                    let max_kw = rated_w / 1_000.0;
                    // Turn-on / turn-off thresholds matching thermostat FSM update_mode
                    // (thermostat.rs:403-409). OCHRE deadband_offset convention
                    // (HVAC.py:628-633): turn_off at setpoint - hysteresis * offset,
                    // turn_on at setpoint + hysteresis * (1 - offset).
                    let t_off = setpoints.cooling_c - hysteresis * offset;
                    let t_on = setpoints.cooling_c + hysteresis * (1.0 - offset);
                    let deadband_range = (t_on - t_off).max(f64::EPSILON);
                    let max_kwh = zone_capacitance_kwh_per_k * deadband_range;
                    // fill = 1 at t_off (cold, full battery), 0 at t_on (warm, empty)
                    let fill = ((t_on - zone_temp_c) / deadband_range).clamp(0.0, 1.0);
                    (Some(max_kw), Some(fill * max_kwh), Some(max_kwh))
                }
            }
            ThermostatMode::Deadband => (None, None, None),
        };

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        if zone_capacitance_kwh_per_k > 0.0 {
            if let Some(max_e) = max_energy_kwh {
                if max_e <= 0.0 {
                    panic!(
                        "EBM invariant violation: zone_capacitance_kwh_per_k={} > 0 \
                         but max_energy_kwh={} <= 0. Capacitance is positive but \
                         the computed energy capacity is non-positive — check \
                         deadband_range computation.",
                        zone_capacitance_kwh_per_k, max_e
                    );
                }
            }
        }

        #[cfg(feature = "observe")]
        if let Some(max_e) = max_energy_kwh {
            let deadband_c = if zone_capacitance_kwh_per_k > f64::EPSILON {
                max_e / zone_capacitance_kwh_per_k
            } else {
                0.0
            };
            tracing::debug!(
                max_energy_kwh = max_e,
                zone_capacitance_kwh_per_k,
                deadband_range_c = deadband_c,
                "EquivalentBatteryModel parameters computed"
            );
        }

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

#[cfg(test)]
mod tests {
    use super::super::hvac_core::{HvacEquipment, HvacEquipmentType};
    use super::super::thermostat::{
        ThermalSetpoints, ThermostatConfig, ThermostatFsm, ThermostatMode,
    };

    /// Creates an HvacEquipment in Heating mode with a configured thermostat.
    fn heating_equipment(
        setpoint_c: f64,
        rated_w: f64,
        hysteresis_c: f64,
        offset: f64,
    ) -> HvacEquipment {
        let mut eq = HvacEquipment::new(HvacEquipmentType::GasFurnace, hares_types::ZoneId(1));
        eq.thermostat_fsm = ThermostatFsm::new(ThermalSetpoints {
            heating_c: setpoint_c,
            cooling_c: setpoint_c + 5.0,
        });
        eq.thermostat_fsm.thermostat = ThermostatConfig {
            hysteresis_c,
            deadband_offset: offset,
            ..ThermostatConfig::default()
        };
        eq.thermostat_fsm.mode = ThermostatMode::Heating;
        eq.config.heating_capacities_w = vec![rated_w];
        eq
    }

    /// Creates an HvacEquipment in Cooling mode with a configured thermostat.
    fn cooling_equipment(
        setpoint_c: f64,
        rated_w: f64,
        hysteresis_c: f64,
        offset: f64,
    ) -> HvacEquipment {
        let mut eq = HvacEquipment::new(HvacEquipmentType::AcCooler, hares_types::ZoneId(1));
        eq.thermostat_fsm = ThermostatFsm::new(ThermalSetpoints {
            heating_c: setpoint_c - 5.0,
            cooling_c: setpoint_c,
        });
        eq.thermostat_fsm.thermostat = ThermostatConfig {
            hysteresis_c,
            deadband_offset: offset,
            ..ThermostatConfig::default()
        };
        eq.thermostat_fsm.mode = ThermostatMode::Cooling;
        eq.config.cooling_capacities_w = vec![rated_w];
        eq
    }

    #[test]
    fn max_energy_kwh_equals_capacitance_times_deadband_heating() {
        let setpoint = 21.0;
        let hysteresis = 1.0;
        let offset = 0.2;
        let eq = heating_equipment(setpoint, 10_000.0, hysteresis, offset);

        // With offset=0.2, hysteresis=1.0:
        //   t_on  = 21.0 - 1.0 * (1.0 - 0.2) = 20.2
        //   t_off = 21.0 + 1.0 * 0.2 = 21.2
        //   deadband = 21.2 - 20.2 = 1.0
        let expected_deadband = hysteresis;
        let capacitance = 2.5; // kWh/K

        let ebm = eq.make_equivalent_battery_model(20.5, capacitance);
        let max_e = ebm.max_energy_kwh.unwrap();
        let expected = capacitance * expected_deadband;
        assert!(
            (max_e - expected).abs() < 1e-10,
            "max_energy_kwh={} != capacitance*deadband={expected}",
            max_e
        );
    }

    #[test]
    fn max_energy_kwh_equals_capacitance_times_deadband_cooling() {
        let setpoint = 24.0;
        let hysteresis = 1.0;
        let offset = 0.2;
        let eq = cooling_equipment(setpoint, 10_000.0, hysteresis, offset);

        // With offset=0.2, hysteresis=1.0:
        //   t_off = 24.0 - 1.0 * 0.2 = 23.8
        //   t_on  = 24.0 + 1.0 * (1.0 - 0.2) = 24.8
        //   deadband = 24.8 - 23.8 = 1.0
        let expected_deadband = hysteresis;
        let capacitance = 3.5; // kWh/K

        let ebm = eq.make_equivalent_battery_model(24.5, capacitance);
        let max_e = ebm.max_energy_kwh.unwrap();
        let expected = capacitance * expected_deadband;
        assert!(
            (max_e - expected).abs() < 1e-10,
            "max_energy_kwh={} != capacitance*deadband={expected}",
            max_e
        );
    }

    #[test]
    fn max_energy_kwh_scales_linearly_with_capacitance() {
        let setpoint = 21.0;
        let hysteresis = 1.0;
        let offset = 0.2;
        let eq = heating_equipment(setpoint, 10_000.0, hysteresis, offset);

        let cap_small = 0.5; // kWh/K — light apartment
        let cap_large = 100.0; // kWh/K — heavy warehouse

        let ebm_small = eq.make_equivalent_battery_model(20.5, cap_small);
        let ebm_large = eq.make_equivalent_battery_model(20.5, cap_large);

        let max_small = ebm_small.max_energy_kwh.unwrap();
        let max_large = ebm_large.max_energy_kwh.unwrap();

        let ratio = max_large / max_small;
        let expected_ratio = cap_large / cap_small;
        assert!(
            (ratio - expected_ratio).abs() < 1e-10,
            "max_energy ratio={ratio} should equal capacitance ratio={expected_ratio}"
        );
    }

    #[test]
    fn max_energy_kwh_scales_with_deadband_offset() {
        let setpoint = 21.0;
        let hysteresis = 1.0;
        let capacitance = 2.0;
        let eq_zero = heating_equipment(setpoint, 10_000.0, hysteresis, 0.0);
        let eq_default = heating_equipment(setpoint, 10_000.0, hysteresis, 0.2);

        // Both should have the same deadband range (hysteresis)
        let ebm_zero = eq_zero.make_equivalent_battery_model(20.5, capacitance);
        let ebm_default = eq_default.make_equivalent_battery_model(20.5, capacitance);

        let max_zero = ebm_zero.max_energy_kwh.unwrap();
        let max_default = ebm_default.max_energy_kwh.unwrap();

        // Deadband width is the same regardless of offset
        assert!(
            (max_zero - max_default).abs() < 1e-10,
            "max_energy_kwh should be identical regardless of deadband_offset (same deadband width)"
        );
    }

    #[test]
    fn fill_fraction_uses_correct_deadband_boundaries_heating() {
        let setpoint = 21.0;
        let hysteresis = 1.0;
        let offset = 0.2;
        let capacitance = 2.0;
        let eq = heating_equipment(setpoint, 10_000.0, hysteresis, offset);

        // With offset=0.2, hysteris=1.0:
        //   t_on  = 21.0 - 0.8 = 20.2 (empty battery)
        //   t_off = 21.0 + 0.2 = 21.2 (full battery)
        let t_on = setpoint - hysteresis * (1.0 - offset); // 20.2
        let t_off = setpoint + hysteresis * offset; // 21.2

        // At t_on: fill should be 0 (empty)
        let ebm_empty = eq.make_equivalent_battery_model(t_on, capacitance);
        assert!(
            ebm_empty.energy_kwh.unwrap() < 1e-10,
            "fill should be 0 at t_on (empty battery)"
        );

        // At t_off: fill should be 1 (full)
        let ebm_full = eq.make_equivalent_battery_model(t_off, capacitance);
        assert!(
            (ebm_full.energy_kwh.unwrap() - ebm_full.max_energy_kwh.unwrap()).abs() < 1e-10,
            "fill should be 1 at t_off (full battery)"
        );
    }

    #[test]
    fn fill_fraction_uses_correct_deadband_boundaries_cooling() {
        let setpoint = 24.0;
        let hysteresis = 1.0;
        let offset = 0.2;
        let capacitance = 2.0;
        let eq = cooling_equipment(setpoint, 10_000.0, hysteresis, offset);

        // With offset=0.2, hysteris=1.0:
        //   t_off = 24.0 - 0.2 = 23.8 (full battery — coldest)
        //   t_on  = 24.0 + 0.8 = 24.8 (empty battery — warmest)
        let t_off = setpoint - hysteresis * offset; // 23.8
        let t_on = setpoint + hysteresis * (1.0 - offset); // 24.8

        // At t_on: fill should be 0 (empty)
        let ebm_empty = eq.make_equivalent_battery_model(t_on, capacitance);
        assert!(
            ebm_empty.energy_kwh.unwrap() < 1e-10,
            "fill should be 0 at t_on (empty battery)"
        );

        // At t_off: fill should be 1 (full)
        let ebm_full = eq.make_equivalent_battery_model(t_off, capacitance);
        assert!(
            (ebm_full.energy_kwh.unwrap() - ebm_full.max_energy_kwh.unwrap()).abs() < 1e-10,
            "fill should be 1 at t_off (full battery)"
        );
    }

    #[test]
    fn zero_capacitance_produces_zero_max_energy() {
        let eq = heating_equipment(21.0, 10_000.0, 1.0, 0.2);
        let ebm = eq.make_equivalent_battery_model(20.5, 0.0);
        assert!(
            ebm.max_energy_kwh.unwrap() < 1e-10,
            "zero capacitance should produce zero max_energy_kwh"
        );
    }

    #[test]
    fn energy_state_midband_heating() {
        let setpoint = 21.0;
        let hysteresis = 1.0;
        let offset = 0.2;
        let capacitance = 2.0;
        let eq = heating_equipment(setpoint, 10_000.0, hysteresis, offset);

        // t_on=20.2, t_off=21.2, deadband=1.0
        // At zone_temp=20.7 (midpoint), fill = (20.7-20.2)/1.0 = 0.5
        let ebm = eq.make_equivalent_battery_model(20.7, capacitance);
        let expected_energy = capacitance * 1.0 * 0.5; // C * deadband * fill
        let actual = ebm.energy_kwh.unwrap();
        assert!(
            (actual - expected_energy).abs() < 1e-10,
            "energy={actual} should be {expected_energy} (50% fill)"
        );
    }
}
