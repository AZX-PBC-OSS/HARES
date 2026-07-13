//! Equivalent Battery Model (EBM) for HVAC equipment.

use super::hvac_core::HvacEquipment;
use super::thermostat::ThermostatMode;
use hares_physics::units::power_w_to_kw;

/// OCHRE reference temperature for heating mode: temperature at which Energy=0.
/// Matches OCHRE `HVAC.make_equivalent_battery_model()` (HVAC.py:626).
const REF_TEMP_HEATING_C: f64 = 10.0;

/// OCHRE reference temperature for cooling mode: temperature at which Energy=0.
/// Matches OCHRE `HVAC.make_equivalent_battery_model()` (HVAC.py:626).
const REF_TEMP_COOLING_C: f64 = 30.0;

/// Equivalent Battery Model (EBM) parameters for HVAC equipment.
///
/// Matches OCHRE `HVAC.make_equivalent_battery_model()` (HVAC.py:620-641).
/// Model: `E_dot = eta * P - P_b`, with `E_min <= E <= E_max`, `0 <= P <= P_max`.
///
/// Energy state (`energy_kwh`, `min_energy_kwh`, `max_energy_kwh`) tracks absolute
/// thermal energy stored in the building relative to a fixed reference temperature
/// (10°C for heating, 30°C for cooling). This follows OCHRE's convention where
/// `energy = total_capacitance * (zone.temperature - ref_temp) * hvac_mult`
/// (HVAC.py:635), making the EBM dimensionally consistent and traceable to the
/// envelope solver's state variables. State-of-charge is `(energy - min) / (max - min)`.
#[derive(Debug, Clone, PartialEq)]
pub struct EquivalentBatteryModel {
    /// Current energy state [kWh]. Computed as absolute thermal energy relative to
    /// reference temperature. `None` when equipment is in Deadband mode.
    pub energy_kwh: Option<f64>,
    /// Minimum energy state [kWh]. Energy at the thermostat turn-on threshold,
    /// computed as `capacitance * (t_on - ref_temp) * hvac_direction`.
    pub min_energy_kwh: f64,
    /// Maximum energy state [kWh]. Energy at the thermostat turn-off threshold,
    /// computed as `capacitance * (t_off - ref_temp) * hvac_direction`.
    /// `None` when rated capacity is unavailable.
    pub max_energy_kwh: Option<f64>,
    /// Maximum power consumption [kW]. `None` when rated capacity is unavailable.
    pub max_power_kw: Option<f64>,
    /// Charging efficiency [-]. Coefficient of performance `COP = 1 / EIR`: the
    /// thermal energy delivered to (or removed from) the building per unit of
    /// electrical energy consumed. Matches OCHRE `1 / self.eir` (HVAC.py:639).
    pub efficiency: f64,
    /// Baseline disturbance power [kW]: the steady-state electrical power the
    /// system must draw to hold the current setpoint against envelope losses.
    /// Computed as `capacity_ideal_w * eir / 1000` — the ideal thermal capacity
    /// needed to maintain setpoint, divided by the COP to convert thermal load
    /// to electrical draw. OCHRE reports `self.capacity_ideal` directly
    /// (HVAC.py:640), but that is a thermal quantity mislabeled "kW"; converting
    /// through the EIR yields the electrical power the EBM's `eta * P - P_b`
    /// balance actually requires.
    pub baseline_power_kw: f64,
}

impl HvacEquipment {
    /// Compute Equivalent Battery Model parameters from current equipment state.
    ///
    /// Energy state tracks absolute thermal energy stored in the building relative
    /// to a fixed reference temperature, following OCHRE (HVAC.py:620-641):
    ///   `energy = zone_capacitance * (zone_temp - ref_temp) * hvac_direction`
    ///
    /// Reference temperatures: 10°C for heating, 30°C for cooling (OCHRE convention).
    /// `hvac_direction` is +1.0 for heating (positive energy = warmer than reference),
    /// -1.0 for cooling (positive energy = cooler than reference, i.e. stored "coolth").
    ///
    /// `min_energy_kwh` and `max_energy_kwh` mark the thermostat turn-on and turn-off
    /// thresholds, using the thermostat FSM's asymmetric deadband boundaries so the
    /// EBM state-of-charge aligns with actual equipment switching behaviour
    /// (thermostat.rs:403-409). State-of-charge = `(energy - min) / (max - min)`
    /// ranges from 0 at turn-on to 1 at turn-off.
    ///
    /// `rated_eir` is the equipment's energy input ratio (electrical input per unit
    /// thermal output); `efficiency = 1 / rated_eir` is the resulting COP
    /// (OCHRE HVAC.py:639). `capacity_ideal_w` is the ideal thermal capacity [W]
    /// needed to hold the setpoint against envelope losses (OCHRE `capacity_ideal`,
    /// HVAC.py:640); `baseline_power_kw = capacity_ideal_w * rated_eir / 1000` is the
    /// steady-state electrical draw to maintain state.
    ///
    /// Returns `None` fields in Deadband mode or when rated capacity is zero.
    ///
    /// # Panics
    ///
    /// Panics if `rated_eir <= 0.0`: a non-positive energy input ratio implies
    /// infinite or negative COP, which is physically impossible.
    #[must_use]
    pub fn make_equivalent_battery_model(
        &self,
        zone_temp_c: f64,
        zone_capacitance_kwh_per_k: f64,
        rated_eir: f64,
        capacity_ideal_w: f64,
    ) -> EquivalentBatteryModel {
        assert!(
            rated_eir > 0.0,
            "EBM requires rated_eir > 0.0 (COP = 1/EIR must be finite and positive), got {rated_eir}"
        );
        // COP = 1/EIR (OCHRE HVAC.py:639) and steady-state electrical draw to hold
        // setpoint = ideal thermal capacity * EIR (OCHRE HVAC.py:640, converted from
        // the thermal quantity to electrical draw via the EIR).
        let efficiency = 1.0 / rated_eir;
        let baseline_power_kw = power_w_to_kw(capacity_ideal_w * rated_eir);

        let setpoints = self.effective_setpoints();
        let hysteresis = self.thermostat_fsm.thermostat.hysteresis_c;
        let offset = self
            .thermostat_fsm
            .thermostat
            .deadband_offset
            .clamp(0.0, 1.0);

        let (max_power_kw, energy_kwh, min_energy_kwh, max_energy_kwh, _ref_temp_c) = match self
            .thermostat_fsm
            .mode
        {
            ThermostatMode::Heating => {
                let rated_w = self.rated_capacity_w(ThermostatMode::Heating);
                if rated_w <= 0.0 {
                    (None, None, 0.0, None, REF_TEMP_HEATING_C)
                } else {
                    let max_kw = power_w_to_kw(rated_w);
                    // Turn-on / turn-off thresholds matching thermostat FSM update_mode
                    // (thermostat.rs:403-409). OCHRE deadband_offset convention
                    // (HVAC.py:628-633): turn_on at setpoint - hysteresis * (1 - offset),
                    // turn_off at setpoint + hysteresis * offset.
                    let t_on = setpoints.heating_c - hysteresis * (1.0 - offset);
                    let t_off = setpoints.heating_c + hysteresis * offset;
                    let ref_temp = REF_TEMP_HEATING_C;
                    // hvac_direction = +1.0 for heating: positive energy = warmer than ref.
                    let hvac_dir = 1.0;
                    let energy = zone_capacitance_kwh_per_k * (zone_temp_c - ref_temp) * hvac_dir;
                    let min_e = zone_capacitance_kwh_per_k * (t_on - ref_temp) * hvac_dir;
                    let max_e = zone_capacitance_kwh_per_k * (t_off - ref_temp) * hvac_dir;
                    (Some(max_kw), Some(energy), min_e, Some(max_e), ref_temp)
                }
            }
            ThermostatMode::Cooling => {
                let rated_w = self.rated_capacity_w(ThermostatMode::Cooling);
                if rated_w <= 0.0 {
                    (None, None, 0.0, None, REF_TEMP_COOLING_C)
                } else {
                    let max_kw = power_w_to_kw(rated_w);
                    // Turn-on / turn-off thresholds matching thermostat FSM update_mode
                    // (thermostat.rs:403-409). OCHRE deadband_offset convention
                    // (HVAC.py:628-633): turn_off at setpoint - hysteresis * offset,
                    // turn_on at setpoint + hysteresis * (1 - offset).
                    let t_off = setpoints.cooling_c - hysteresis * offset;
                    let t_on = setpoints.cooling_c + hysteresis * (1.0 - offset);
                    let ref_temp = REF_TEMP_COOLING_C;
                    // hvac_direction = -1.0 for cooling: positive energy = cooler than ref
                    // (stored "coolth").
                    let hvac_dir = -1.0;
                    let energy = zone_capacitance_kwh_per_k * (zone_temp_c - ref_temp) * hvac_dir;
                    let min_e = zone_capacitance_kwh_per_k * (t_on - ref_temp) * hvac_dir;
                    let max_e = zone_capacitance_kwh_per_k * (t_off - ref_temp) * hvac_dir;
                    (Some(max_kw), Some(energy), min_e, Some(max_e), ref_temp)
                }
            }
            ThermostatMode::Deadband => (None, None, 0.0, None, f64::NAN),
        };

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            assert!(
                efficiency > 0.0,
                "EBM invariant violation: efficiency={efficiency} must be > 0 \
                 (COP = 1/EIR with EIR={rated_eir})."
            );
            assert!(
                baseline_power_kw >= 0.0,
                "EBM invariant violation: baseline_power_kw={baseline_power_kw} must be >= 0 \
                 (capacity_ideal_w={capacity_ideal_w} * eir={rated_eir})."
            );
        }

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        if zone_capacitance_kwh_per_k > 0.0 {
            if let Some(max_e) = max_energy_kwh {
                if max_e <= 0.0 {
                    panic!(
                        "EBM invariant violation: zone_capacitance_kwh_per_k={} > 0 \
                         but max_energy_kwh={} <= 0. Capacitance is positive but \
                         the computed energy capacity is non-positive — check \
                         deadband and reference temperature computation.",
                        zone_capacitance_kwh_per_k, max_e
                    );
                }
                // max_energy_kwh must exceed min_energy_kwh when deadband > 0
                let deadband_range = (max_e - min_energy_kwh).max(0.0);
                if deadband_range <= f64::EPSILON && zone_capacitance_kwh_per_k > f64::EPSILON {
                    panic!(
                        "EBM invariant violation: max_energy_kwh={} equals min_energy_kwh={}, \
                         implying zero deadband range while capacitance is positive ({zone_capacitance_kwh_per_k}). \
                         Check thermostat thresholds.",
                        max_e, min_energy_kwh
                    );
                }
            }
        }

        #[cfg(feature = "observe")]
        if let Some(max_e) = max_energy_kwh {
            if let Some(e) = energy_kwh {
                let fill_ratio = if max_e > f64::EPSILON {
                    (e - min_energy_kwh) / (max_e - min_energy_kwh).max(f64::EPSILON)
                } else {
                    0.0
                };
                let deadband_range_c = if zone_capacitance_kwh_per_k > f64::EPSILON {
                    (max_e - min_energy_kwh) / zone_capacitance_kwh_per_k
                } else {
                    0.0
                };
                tracing::debug!(
                    energy_kwh = e,
                    zone_temp_c,
                    ref_temp_c = _ref_temp_c,
                    fill_ratio,
                    min_energy_kwh,
                    max_energy_kwh = max_e,
                    zone_capacitance_kwh_per_k,
                    deadband_range_c,
                    efficiency,
                    baseline_power_kw,
                    rated_eir,
                    capacity_ideal_w,
                    "EquivalentBatteryModel parameters computed"
                );
            }
        }

        EquivalentBatteryModel {
            energy_kwh,
            min_energy_kwh,
            max_energy_kwh,
            max_power_kw,
            efficiency,
            baseline_power_kw,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::hvac_core::{HvacEquipment, HvacEquipmentType};
    use super::super::thermostat::{
        ThermalSetpoints, ThermostatConfig, ThermostatFsm, ThermostatMode,
    };
    use super::{REF_TEMP_COOLING_C, REF_TEMP_HEATING_C};

    /// Arbitrary valid EIR (COP = 4) used by state-focused tests that do not
    /// assert on efficiency or baseline power. Efficiency/baseline behaviour is
    /// covered by the dedicated tests below.
    const TEST_EIR: f64 = 0.25;
    /// Arbitrary steady-state hold load [W] for state-focused tests.
    const TEST_CAP_IDEAL_W: f64 = 3_000.0;

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

    /// Turn-on threshold for heating: thermostat FSM formula (thermostat.rs:403).
    fn heating_t_on(setpoint: f64, hysteresis: f64, offset: f64) -> f64 {
        setpoint - hysteresis * (1.0 - offset)
    }

    /// Turn-off threshold for heating: thermostat FSM formula (thermostat.rs:408).
    fn heating_t_off(setpoint: f64, hysteresis: f64, offset: f64) -> f64 {
        setpoint + hysteresis * offset
    }

    /// Turn-off threshold for cooling: thermostat FSM formula (thermostat.rs:416).
    fn cooling_t_off(setpoint: f64, hysteresis: f64, offset: f64) -> f64 {
        setpoint - hysteresis * offset
    }

    /// Turn-on threshold for cooling: thermostat FSM formula (thermostat.rs:404).
    fn cooling_t_on(setpoint: f64, hysteresis: f64, offset: f64) -> f64 {
        setpoint + hysteresis * (1.0 - offset)
    }

    // ---------------------------------------------------------------------------
    // Existing tests updated for absolute energy tracking
    // ---------------------------------------------------------------------------

    #[test]
    fn max_min_energy_range_equals_capacitance_times_deadband_heating() {
        let setpoint = 21.0;
        let hysteresis = 1.0;
        let offset = 0.2;
        let eq = heating_equipment(setpoint, 10_000.0, hysteresis, offset);

        let t_on = heating_t_on(setpoint, hysteresis, offset);
        let t_off = heating_t_off(setpoint, hysteresis, offset);
        let deadband = t_off - t_on; // 1.0
        let capacitance = 2.5; // kWh/K

        let ebm = eq.make_equivalent_battery_model(20.5, capacitance, TEST_EIR, TEST_CAP_IDEAL_W);
        let max_e = ebm.max_energy_kwh.unwrap();
        let min_e = ebm.min_energy_kwh;
        let range = max_e - min_e;
        let expected_range = capacitance * deadband;
        assert!(
            (range - expected_range).abs() < 1e-10,
            "max-min range={} != capacitance*deadband={expected_range}",
            range
        );
        // Absolute values: max = C * (t_off - ref), min = C * (t_on - ref)
        let expected_max = capacitance * (t_off - REF_TEMP_HEATING_C);
        let expected_min = capacitance * (t_on - REF_TEMP_HEATING_C);
        assert!((max_e - expected_max).abs() < 1e-10);
        assert!((min_e - expected_min).abs() < 1e-10);
    }

    #[test]
    fn max_min_energy_range_equals_capacitance_times_deadband_cooling() {
        let setpoint = 24.0;
        let hysteresis = 1.0;
        let offset = 0.2;
        let eq = cooling_equipment(setpoint, 10_000.0, hysteresis, offset);

        let t_on = cooling_t_on(setpoint, hysteresis, offset);
        let t_off = cooling_t_off(setpoint, hysteresis, offset);
        let deadband = t_on - t_off; // 1.0
        let capacitance = 3.5; // kWh/K
        let hvac_dir = -1.0;

        let ebm = eq.make_equivalent_battery_model(24.5, capacitance, TEST_EIR, TEST_CAP_IDEAL_W);
        let max_e = ebm.max_energy_kwh.unwrap();
        let min_e = ebm.min_energy_kwh;
        let range = max_e - min_e;
        let expected_range = capacitance * deadband;
        assert!(
            (range - expected_range).abs() < 1e-10,
            "max-min range={} != capacitance*deadband={expected_range}",
            range
        );
        // Absolute values: max = C * (t_off - ref) * dir, min = C * (t_on - ref) * dir
        let expected_max = capacitance * (t_off - REF_TEMP_COOLING_C) * hvac_dir;
        let expected_min = capacitance * (t_on - REF_TEMP_COOLING_C) * hvac_dir;
        assert!((max_e - expected_max).abs() < 1e-10);
        assert!((min_e - expected_min).abs() < 1e-10);
    }

    #[test]
    fn max_energy_kwh_scales_linearly_with_capacitance() {
        let setpoint = 21.0;
        let hysteresis = 1.0;
        let offset = 0.2;
        let eq = heating_equipment(setpoint, 10_000.0, hysteresis, offset);

        let cap_small = 0.5; // kWh/K — light apartment
        let cap_large = 100.0; // kWh/K — heavy warehouse

        let ebm_small =
            eq.make_equivalent_battery_model(20.5, cap_small, TEST_EIR, TEST_CAP_IDEAL_W);
        let ebm_large =
            eq.make_equivalent_battery_model(20.5, cap_large, TEST_EIR, TEST_CAP_IDEAL_W);

        let range_small = ebm_small.max_energy_kwh.unwrap() - ebm_small.min_energy_kwh;
        let range_large = ebm_large.max_energy_kwh.unwrap() - ebm_large.min_energy_kwh;

        let ratio = range_large / range_small;
        let expected_ratio = cap_large / cap_small;
        assert!(
            (ratio - expected_ratio).abs() < 1e-10,
            "energy range ratio={ratio} should equal capacitance ratio={expected_ratio}"
        );
    }

    #[test]
    fn deadband_range_independent_of_offset() {
        let setpoint = 21.0;
        let hysteresis = 1.0;
        let capacitance = 2.0;
        let eq_zero = heating_equipment(setpoint, 10_000.0, hysteresis, 0.0);
        let eq_default = heating_equipment(setpoint, 10_000.0, hysteresis, 0.2);

        let ebm_zero =
            eq_zero.make_equivalent_battery_model(20.5, capacitance, TEST_EIR, TEST_CAP_IDEAL_W);
        let ebm_default =
            eq_default.make_equivalent_battery_model(20.5, capacitance, TEST_EIR, TEST_CAP_IDEAL_W);

        let range_zero = ebm_zero.max_energy_kwh.unwrap() - ebm_zero.min_energy_kwh;
        let range_default = ebm_default.max_energy_kwh.unwrap() - ebm_default.min_energy_kwh;

        // Deadband width is the same regardless of offset
        assert!(
            (range_zero - range_default).abs() < 1e-10,
            "energy range should be identical regardless of deadband_offset (same deadband width)"
        );
    }

    #[test]
    fn energy_at_turn_on_equals_min_energy_heating() {
        let setpoint = 21.0;
        let hysteresis = 1.0;
        let offset = 0.2;
        let capacitance = 2.0;
        let eq = heating_equipment(setpoint, 10_000.0, hysteresis, offset);

        let t_on = heating_t_on(setpoint, hysteresis, offset);

        let ebm = eq.make_equivalent_battery_model(t_on, capacitance, TEST_EIR, TEST_CAP_IDEAL_W);
        assert!(
            (ebm.energy_kwh.unwrap() - ebm.min_energy_kwh).abs() < 1e-10,
            "energy at t_on should equal min_energy_kwh"
        );
    }

    #[test]
    fn energy_at_turn_off_equals_max_energy_heating() {
        let setpoint = 21.0;
        let hysteresis = 1.0;
        let offset = 0.2;
        let capacitance = 2.0;
        let eq = heating_equipment(setpoint, 10_000.0, hysteresis, offset);

        let t_off = heating_t_off(setpoint, hysteresis, offset);

        let ebm = eq.make_equivalent_battery_model(t_off, capacitance, TEST_EIR, TEST_CAP_IDEAL_W);
        assert!(
            (ebm.energy_kwh.unwrap() - ebm.max_energy_kwh.unwrap()).abs() < 1e-10,
            "energy at t_off should equal max_energy_kwh"
        );
    }

    #[test]
    fn energy_at_turn_on_equals_min_energy_cooling() {
        let setpoint = 24.0;
        let hysteresis = 1.0;
        let offset = 0.2;
        let capacitance = 2.0;
        let eq = cooling_equipment(setpoint, 10_000.0, hysteresis, offset);

        let t_on = cooling_t_on(setpoint, hysteresis, offset);

        let ebm = eq.make_equivalent_battery_model(t_on, capacitance, TEST_EIR, TEST_CAP_IDEAL_W);
        assert!(
            (ebm.energy_kwh.unwrap() - ebm.min_energy_kwh).abs() < 1e-10,
            "energy at t_on should equal min_energy_kwh"
        );
    }

    #[test]
    fn energy_at_turn_off_equals_max_energy_cooling() {
        let setpoint = 24.0;
        let hysteresis = 1.0;
        let offset = 0.2;
        let capacitance = 2.0;
        let eq = cooling_equipment(setpoint, 10_000.0, hysteresis, offset);

        let t_off = cooling_t_off(setpoint, hysteresis, offset);

        let ebm = eq.make_equivalent_battery_model(t_off, capacitance, TEST_EIR, TEST_CAP_IDEAL_W);
        assert!(
            (ebm.energy_kwh.unwrap() - ebm.max_energy_kwh.unwrap()).abs() < 1e-10,
            "energy at t_off should equal max_energy_kwh"
        );
    }

    #[test]
    fn zero_capacitance_produces_zero_max_energy() {
        let eq = heating_equipment(21.0, 10_000.0, 1.0, 0.2);
        let ebm = eq.make_equivalent_battery_model(20.5, 0.0, TEST_EIR, TEST_CAP_IDEAL_W);
        let range = ebm.max_energy_kwh.unwrap() - ebm.min_energy_kwh;
        assert!(
            range < 1e-10,
            "zero capacitance should produce zero energy range"
        );
    }

    // ---------------------------------------------------------------------------
    // New tests: absolute energy tracking (T-0440)
    // ---------------------------------------------------------------------------

    #[test]
    fn energy_kwh_tracks_absolute_temperature_deviation_cooling() {
        // Ticket: cap=10, zone=25, ref=30, dir=-1 → energy = 10*(25-30)*(-1) = 50
        let eq = cooling_equipment(24.0, 10_000.0, 1.0, 0.2);
        let capacitance = 10.0;
        let zone_temp = 25.0;

        let ebm =
            eq.make_equivalent_battery_model(zone_temp, capacitance, TEST_EIR, TEST_CAP_IDEAL_W);
        let expected = -capacitance * (zone_temp - REF_TEMP_COOLING_C);
        assert!(
            (ebm.energy_kwh.unwrap() - expected).abs() < 1e-10,
            "energy_kwh={} should be {expected} = C*(zone-ref)*dir",
            ebm.energy_kwh.unwrap()
        );
    }

    #[test]
    fn energy_kwh_increases_with_precooling() {
        // Ticket: pre-cool by 2°C from 25 to 23 → energy increases by 20 kWh
        let eq = cooling_equipment(24.0, 10_000.0, 1.0, 0.2);
        let capacitance = 10.0;
        let hvac_dir = -1.0;

        let ebm_warm =
            eq.make_equivalent_battery_model(25.0, capacitance, TEST_EIR, TEST_CAP_IDEAL_W);
        let ebm_cool =
            eq.make_equivalent_battery_model(23.0, capacitance, TEST_EIR, TEST_CAP_IDEAL_W);

        let energy_warm = capacitance * (25.0 - REF_TEMP_COOLING_C) * hvac_dir;
        let energy_cool = capacitance * (23.0 - REF_TEMP_COOLING_C) * hvac_dir;

        assert!(
            (ebm_warm.energy_kwh.unwrap() - energy_warm).abs() < 1e-10,
            "energy at 25°C should be {energy_warm}"
        );
        assert!(
            (ebm_cool.energy_kwh.unwrap() - energy_cool).abs() < 1e-10,
            "energy at 23°C should be {energy_cool}"
        );

        let delta = ebm_cool.energy_kwh.unwrap() - ebm_warm.energy_kwh.unwrap();
        let expected_delta = capacitance * 2.0;
        assert!(
            (delta - expected_delta).abs() < 1e-10,
            "pre-cool by 2°C should increase energy by {expected_delta}, got {delta}"
        );
    }

    #[test]
    fn energy_kwh_tracks_absolute_temperature_deviation_heating() {
        let eq = heating_equipment(21.0, 10_000.0, 1.0, 0.2);
        let capacitance = 10.0;
        let zone_temp = 20.5;

        let ebm =
            eq.make_equivalent_battery_model(zone_temp, capacitance, TEST_EIR, TEST_CAP_IDEAL_W);
        let expected = capacitance * (zone_temp - REF_TEMP_HEATING_C) * 1.0;
        assert!(
            (ebm.energy_kwh.unwrap() - expected).abs() < 1e-10,
            "energy_kwh={} should be {expected}",
            ebm.energy_kwh.unwrap()
        );
    }

    #[test]
    fn energy_kwh_changes_with_heating_temperature_change() {
        // Verify absolute tracking in heating: 2°C change → energy changes by C * 2
        let eq = heating_equipment(21.0, 10_000.0, 1.0, 0.2);
        let capacitance = 10.0;

        let ebm_cooler =
            eq.make_equivalent_battery_model(20.0, capacitance, TEST_EIR, TEST_CAP_IDEAL_W);
        let ebm_warmer =
            eq.make_equivalent_battery_model(22.0, capacitance, TEST_EIR, TEST_CAP_IDEAL_W);

        let delta = ebm_warmer.energy_kwh.unwrap() - ebm_cooler.energy_kwh.unwrap();
        let expected_delta = capacitance * 2.0;
        assert!(
            (delta - expected_delta).abs() < 1e-10,
            "2°C warmer should increase energy by {expected_delta}, got {delta}"
        );
    }

    #[test]
    fn energy_kwh_independent_of_timestep() {
        // Ticket: same zone temp & capacitance → same energy regardless of time_res_s.
        // time_res_s is not a parameter to make_equivalent_battery_model, so this
        // is trivially satisfied. Verify that repeated calls produce identical results.
        let eq = cooling_equipment(24.0, 10_000.0, 1.0, 0.2);
        let capacitance = 10.0;

        let ebm_a = eq.make_equivalent_battery_model(25.0, capacitance, TEST_EIR, TEST_CAP_IDEAL_W);
        let ebm_b = eq.make_equivalent_battery_model(25.0, capacitance, TEST_EIR, TEST_CAP_IDEAL_W);

        assert!(
            (ebm_a.energy_kwh.unwrap() - ebm_b.energy_kwh.unwrap()).abs() < 1e-10,
            "energy_kwh should be invariant to repeated calls (no timestep dependency)"
        );
        assert!(
            (ebm_a.max_energy_kwh.unwrap() - ebm_b.max_energy_kwh.unwrap()).abs() < 1e-10,
            "max_energy_kwh should be invariant"
        );
    }

    #[test]
    fn state_of_charge_zero_at_turn_on_one_at_turn_off_heating() {
        let setpoint = 21.0;
        let hysteresis = 1.0;
        let offset = 0.2;
        let capacitance = 5.0;
        let eq = heating_equipment(setpoint, 10_000.0, hysteresis, offset);

        let t_on = heating_t_on(setpoint, hysteresis, offset);
        let t_off = heating_t_off(setpoint, hysteresis, offset);

        let ebm_on =
            eq.make_equivalent_battery_model(t_on, capacitance, TEST_EIR, TEST_CAP_IDEAL_W);
        let ebm_off =
            eq.make_equivalent_battery_model(t_off, capacitance, TEST_EIR, TEST_CAP_IDEAL_W);

        let range = ebm_on.max_energy_kwh.unwrap() - ebm_on.min_energy_kwh;
        let soc_on = (ebm_on.energy_kwh.unwrap() - ebm_on.min_energy_kwh) / range;
        let soc_off = (ebm_off.energy_kwh.unwrap() - ebm_off.min_energy_kwh) / range;

        assert!(
            soc_on.abs() < 1e-10,
            "SOC at t_on should be 0, got {soc_on}"
        );
        assert!(
            (soc_off - 1.0).abs() < 1e-10,
            "SOC at t_off should be 1, got {soc_off}"
        );
    }

    #[test]
    fn state_of_charge_zero_at_turn_on_one_at_turn_off_cooling() {
        let setpoint = 24.0;
        let hysteresis = 1.0;
        let offset = 0.2;
        let capacitance = 5.0;
        let eq = cooling_equipment(setpoint, 10_000.0, hysteresis, offset);

        let t_on = cooling_t_on(setpoint, hysteresis, offset);
        let t_off = cooling_t_off(setpoint, hysteresis, offset);

        let ebm_on =
            eq.make_equivalent_battery_model(t_on, capacitance, TEST_EIR, TEST_CAP_IDEAL_W);
        let ebm_off =
            eq.make_equivalent_battery_model(t_off, capacitance, TEST_EIR, TEST_CAP_IDEAL_W);

        let range = ebm_on.max_energy_kwh.unwrap() - ebm_on.min_energy_kwh;
        let soc_on = (ebm_on.energy_kwh.unwrap() - ebm_on.min_energy_kwh) / range;
        let soc_off = (ebm_off.energy_kwh.unwrap() - ebm_off.min_energy_kwh) / range;

        assert!(
            soc_on.abs() < 1e-10,
            "SOC at t_on should be 0, got {soc_on}"
        );
        assert!(
            (soc_off - 1.0).abs() < 1e-10,
            "SOC at t_off should be 1, got {soc_off}"
        );
    }

    #[test]
    fn deadband_mode_returns_none_fields() {
        let mut eq = HvacEquipment::new(HvacEquipmentType::GasFurnace, hares_types::ZoneId(1));
        eq.thermostat_fsm = ThermostatFsm::new(ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 25.0,
        });
        eq.thermostat_fsm.mode = ThermostatMode::Deadband;

        let ebm = eq.make_equivalent_battery_model(22.0, 5.0, TEST_EIR, TEST_CAP_IDEAL_W);
        assert!(ebm.energy_kwh.is_none());
        assert!(ebm.max_energy_kwh.is_none());
        assert!(ebm.max_power_kw.is_none());
    }

    #[test]
    fn energy_kwh_negative_when_zone_warmer_than_cooling_ref_temp() {
        // Regression: the old invariant panicked when energy_kwh < 0 in cooling
        // mode (zone > 30°C ref), but this is valid physics — OCHRE has no lower
        // bound either. The building lost its stored "coolth" while floating warm.
        let eq = cooling_equipment(24.0, 10_000.0, 1.0, 0.2);
        let capacitance = 10.0;
        let zone_temp = 31.0; // above REF_TEMP_COOLING_C (30.0)
        let ref_temp = REF_TEMP_COOLING_C;
        let hvac_dir = -1.0;

        let ebm =
            eq.make_equivalent_battery_model(zone_temp, capacitance, TEST_EIR, TEST_CAP_IDEAL_W);
        let expected = capacitance * (zone_temp - ref_temp) * hvac_dir;
        assert!(
            expected < 0.0,
            "energy should be negative when zone > ref in cooling mode"
        );
        assert!(
            (ebm.energy_kwh.unwrap() - expected).abs() < 1e-10,
            "energy_kwh={} should be {expected}",
            ebm.energy_kwh.unwrap()
        );
    }

    #[test]
    fn energy_kwh_negative_when_zone_colder_than_heating_ref_temp() {
        // Regression: the old invariant panicked when energy_kwh < 0 in heating
        // mode (zone < 10°C ref), but this is valid physics — OCHRE has no lower
        // bound either. The building is colder than the reference during a cold snap.
        let eq = heating_equipment(21.0, 10_000.0, 1.0, 0.2);
        let capacitance = 10.0;
        let zone_temp = 5.0; // below REF_TEMP_HEATING_C (10.0)
        let ref_temp = REF_TEMP_HEATING_C;
        let hvac_dir = 1.0;

        let ebm =
            eq.make_equivalent_battery_model(zone_temp, capacitance, TEST_EIR, TEST_CAP_IDEAL_W);
        let expected = capacitance * (zone_temp - ref_temp) * hvac_dir;
        assert!(
            expected < 0.0,
            "energy should be negative when zone < ref in heating mode"
        );
        assert!(
            (ebm.energy_kwh.unwrap() - expected).abs() < 1e-10,
            "energy_kwh={} should be {expected}",
            ebm.energy_kwh.unwrap()
        );
    }

    #[test]
    fn zero_rated_capacity_returns_none_fields() {
        let eq = heating_equipment(21.0, 0.0, 1.0, 0.2);
        let ebm = eq.make_equivalent_battery_model(20.5, 5.0, TEST_EIR, TEST_CAP_IDEAL_W);
        assert!(ebm.energy_kwh.is_none());
        assert!(ebm.max_energy_kwh.is_none());
        assert!(ebm.max_power_kw.is_none());
    }

    // ---------------------------------------------------------------------------
    // Efficiency and baseline power from EIR / ideal capacity (T-0441)
    // ---------------------------------------------------------------------------

    #[test]
    fn efficiency_is_cop_and_baseline_is_ideal_capacity_times_eir() {
        // eir=0.2 → COP=5.0; capacity_ideal=5000 W → baseline = 5000*0.2/1000 = 1.0 kW.
        let eq = heating_equipment(21.0, 10_000.0, 1.0, 0.2);
        let ebm = eq.make_equivalent_battery_model(20.5, 2.5, 0.2, 5_000.0);

        assert!(
            (ebm.efficiency - 5.0).abs() < 1e-10,
            "efficiency should be 1/eir = 5.0, got {}",
            ebm.efficiency
        );
        assert!(
            (ebm.baseline_power_kw - 1.0).abs() < 1e-10,
            "baseline should be capacity_ideal_w * eir / 1000 = 1.0 kW, got {}",
            ebm.baseline_power_kw
        );
    }

    #[test]
    #[should_panic(expected = "rated_eir > 0.0")]
    fn zero_eir_panics() {
        let eq = heating_equipment(21.0, 10_000.0, 1.0, 0.2);
        let _ = eq.make_equivalent_battery_model(20.5, 2.5, 0.0, 5_000.0);
    }

    #[test]
    #[should_panic(expected = "rated_eir > 0.0")]
    fn negative_eir_panics() {
        let eq = heating_equipment(21.0, 10_000.0, 1.0, 0.2);
        let _ = eq.make_equivalent_battery_model(20.5, 2.5, -0.3, 5_000.0);
    }

    #[test]
    fn efficiency_matches_configured_equipment_eir_end_to_end() {
        // Regression for T-0441: efficiency must be derived from the equipment's
        // configured EIR, not hardcoded to 1.0. A production caller reads the rated
        // EIR from the equipment via `eir_at_stage(0)` (staging.rs:343) and feeds it
        // to the EBM; the resulting COP must be `1 / eir_by_stage[0]`.
        let rated_eir = 0.30; // COP ≈ 3.33, typical residential central AC
        let capacity_w = 8_000.0;
        let mut eq = cooling_equipment(24.0, capacity_w, 1.0, 0.2);
        eq.config.eir_by_stage = vec![rated_eir];

        let config_eir = eq.eir_at_stage(0);
        assert!(
            (config_eir - rated_eir).abs() < 1e-9,
            "eir_at_stage(0) should equal configured eir_by_stage[0]={rated_eir}, got {config_eir}"
        );

        let ebm = eq.make_equivalent_battery_model(24.5, 5.0, config_eir, capacity_w);

        assert!(
            (ebm.efficiency - 1.0 / config_eir).abs() < 1e-10,
            "efficiency={} should equal 1/eir_by_stage[0]={}",
            ebm.efficiency,
            1.0 / config_eir
        );
        assert!(
            (ebm.baseline_power_kw - capacity_w * config_eir / 1_000.0).abs() < 1e-10,
            "baseline should be capacity_w * eir / 1000"
        );
    }
}
