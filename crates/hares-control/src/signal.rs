//! Control signal enum and associated types.

use hares_types::{ControlSignal, DRLevel, DutyCycleComponent, OperatingMode, ProtocolId};

/// Ergonomic constructor helpers for `ControlSignal`.
///
/// Import this trait to enable `ControlSignal::thermal_setpoint(...)` style calls:
///
/// ```no_run
/// use hares_control::ControlSignalConstructors;
/// use hares_types::ControlSignal;
/// let signal = ControlSignal::power_setpoint(5.0, None);
/// ```
pub trait ControlSignalConstructors {
    fn thermal_setpoint(
        heating_setpoint_c: Option<f64>,
        cooling_setpoint_c: Option<f64>,
        deadband_c: Option<f64>,
    ) -> ControlSignal;

    fn power_setpoint(active_power_kw: f64, reactive_power_kvar: Option<f64>) -> ControlSignal;

    fn power_limit(max_power_kw: f64, ramp_rate_kw_per_s: Option<f64>) -> ControlSignal;

    fn soc_target(target_soc: f64, min_soc: Option<f64>, max_soc: Option<f64>) -> ControlSignal;

    fn mode_override(mode: OperatingMode) -> ControlSignal;

    fn duty_cycle(
        on_fraction: f64,
        period_s: Option<f64>,
        component: Option<DutyCycleComponent>,
    ) -> ControlSignal;

    fn load_fraction(fraction: f64) -> ControlSignal;

    fn grid_connect(connected: bool) -> ControlSignal;

    fn self_consumption(enabled: bool, solar_only_charging: bool) -> ControlSignal;

    fn demand_response(level: DRLevel, duration_s: Option<f64>) -> ControlSignal;

    fn humidity_setpoint(target_rh: f64, min_rh: Option<f64>, max_rh: Option<f64>)
    -> ControlSignal;

    fn protocol_native(protocol: ProtocolId, payload: Vec<u8>) -> ControlSignal;
}

impl ControlSignalConstructors for ControlSignal {
    fn thermal_setpoint(
        heating_setpoint_c: Option<f64>,
        cooling_setpoint_c: Option<f64>,
        deadband_c: Option<f64>,
    ) -> ControlSignal {
        if let (Some(heat), Some(cool)) = (heating_setpoint_c, cooling_setpoint_c) {
            debug_assert!(
                heat < cool,
                "thermal_setpoint: heating_setpoint_c ({heat}) must be < cooling_setpoint_c ({cool})"
            );
        }
        ControlSignal::ThermalSetpoint {
            heating_setpoint_c,
            cooling_setpoint_c,
            deadband_c,
        }
    }

    fn power_setpoint(active_power_kw: f64, reactive_power_kvar: Option<f64>) -> ControlSignal {
        debug_assert!(
            active_power_kw.is_finite(),
            "active_power_kw must be finite"
        );
        if let Some(q) = reactive_power_kvar {
            debug_assert!(q.is_finite(), "reactive_power_kvar must be finite");
        }
        ControlSignal::PowerSetpoint {
            active_power_kw,
            reactive_power_kvar,
            min_soc: None,
            max_soc: None,
        }
    }

    fn power_limit(max_power_kw: f64, ramp_rate_kw_per_s: Option<f64>) -> ControlSignal {
        debug_assert!(
            max_power_kw >= 0.0 && max_power_kw.is_finite(),
            "max_power_kw must be >= 0.0 and finite"
        );
        if let Some(r) = ramp_rate_kw_per_s {
            debug_assert!(
                r >= 0.0 && r.is_finite(),
                "ramp_rate_kw_per_s must be >= 0.0 and finite"
            );
        }
        ControlSignal::PowerLimit {
            max_power_kw,
            ramp_rate_kw_per_s,
        }
    }

    fn soc_target(target_soc: f64, min_soc: Option<f64>, max_soc: Option<f64>) -> ControlSignal {
        debug_assert!(
            (0.0..=1.0).contains(&target_soc) && target_soc.is_finite(),
            "target_soc must be in [0,1] and finite"
        );
        if let (Some(min), Some(max)) = (min_soc, max_soc) {
            debug_assert!(
                min <= max,
                "soc_target: min_soc ({min}) must be <= max_soc ({max})"
            );
            debug_assert!(
                (min..=max).contains(&target_soc),
                "soc_target: target_soc ({target_soc}) must be in [min_soc ({min}), max_soc ({max})]"
            );
        }
        ControlSignal::SOCTarget {
            target_soc,
            min_soc,
            max_soc,
        }
    }

    fn mode_override(mode: OperatingMode) -> ControlSignal {
        ControlSignal::ModeOverride { mode }
    }

    fn duty_cycle(
        on_fraction: f64,
        period_s: Option<f64>,
        component: Option<DutyCycleComponent>,
    ) -> ControlSignal {
        debug_assert!(
            (0.0..=1.0).contains(&on_fraction) && on_fraction.is_finite(),
            "on_fraction must be in [0,1] and finite"
        );
        if let Some(p) = period_s {
            debug_assert!(
                p > 0.0 && p.is_finite(),
                "period_s must be > 0.0 and finite"
            );
        }
        ControlSignal::DutyCycle {
            on_fraction,
            period_s,
            component,
        }
    }

    fn load_fraction(fraction: f64) -> ControlSignal {
        debug_assert!(
            (0.0..=1.0).contains(&fraction) && fraction.is_finite(),
            "fraction must be in [0,1] and finite"
        );
        ControlSignal::LoadFraction { fraction }
    }

    fn grid_connect(connected: bool) -> ControlSignal {
        ControlSignal::GridConnect { connected }
    }

    fn self_consumption(enabled: bool, solar_only_charging: bool) -> ControlSignal {
        ControlSignal::SelfConsumption {
            enabled,
            solar_only_charging,
        }
    }

    fn demand_response(level: DRLevel, duration_s: Option<f64>) -> ControlSignal {
        if let Some(d) = duration_s {
            debug_assert!(
                d >= 0.0 && d.is_finite(),
                "duration_s must be >= 0.0 and finite"
            );
        }
        ControlSignal::DemandResponse { level, duration_s }
    }

    fn humidity_setpoint(
        target_rh: f64,
        min_rh: Option<f64>,
        max_rh: Option<f64>,
    ) -> ControlSignal {
        debug_assert!(
            (0.0..=1.0).contains(&target_rh) && target_rh.is_finite(),
            "target_rh must be in [0,1] and finite"
        );
        if let (Some(min), Some(max)) = (min_rh, max_rh) {
            debug_assert!(
                min <= max,
                "humidity_setpoint: min_rh ({min}) must be <= max_rh ({max})"
            );
            debug_assert!(
                (min..=max).contains(&target_rh),
                "humidity_setpoint: target_rh ({target_rh}) must be in [min_rh ({min}), max_rh ({max})]"
            );
        }
        ControlSignal::HumiditySetpoint {
            target_rh,
            min_rh,
            max_rh,
        }
    }

    fn protocol_native(protocol: ProtocolId, payload: Vec<u8>) -> ControlSignal {
        ControlSignal::ProtocolNative { protocol, payload }
    }
}

#[cfg(test)]
mod tests {
    use super::ControlSignalConstructors;
    use hares_types::{ControlSignal, DRLevel, DutyCycleComponent, OperatingMode, ProtocolId};

    #[test]
    fn constructors_set_expected_fields() {
        assert_eq!(
            ControlSignal::thermal_setpoint(Some(20.0), Some(24.0), Some(1.0)),
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(20.0),
                cooling_setpoint_c: Some(24.0),
                deadband_c: Some(1.0),
            }
        );

        assert_eq!(
            ControlSignal::power_setpoint(4.5, Some(0.3)),
            ControlSignal::PowerSetpoint {
                active_power_kw: 4.5,
                reactive_power_kvar: Some(0.3),
                min_soc: None,
                max_soc: None,
            }
        );

        assert_eq!(
            ControlSignal::power_limit(7.0, Some(0.5)),
            ControlSignal::PowerLimit {
                max_power_kw: 7.0,
                ramp_rate_kw_per_s: Some(0.5),
            }
        );

        assert_eq!(
            ControlSignal::soc_target(0.7, Some(0.2), Some(0.9)),
            ControlSignal::SOCTarget {
                target_soc: 0.7,
                min_soc: Some(0.2),
                max_soc: Some(0.9),
            }
        );

        assert_eq!(
            ControlSignal::mode_override(OperatingMode::Defrost),
            ControlSignal::ModeOverride {
                mode: OperatingMode::Defrost,
            }
        );

        assert_eq!(
            ControlSignal::duty_cycle(0.6, Some(600.0), None),
            ControlSignal::DutyCycle {
                on_fraction: 0.6,
                period_s: Some(600.0),
                component: None,
            }
        );

        assert_eq!(
            ControlSignal::duty_cycle(0.8, Some(300.0), Some(DutyCycleComponent::Compressor)),
            ControlSignal::DutyCycle {
                on_fraction: 0.8,
                period_s: Some(300.0),
                component: Some(DutyCycleComponent::Compressor),
            }
        );

        assert_eq!(
            ControlSignal::load_fraction(0.8),
            ControlSignal::LoadFraction { fraction: 0.8 }
        );

        assert_eq!(
            ControlSignal::grid_connect(true),
            ControlSignal::GridConnect { connected: true }
        );

        assert_eq!(
            ControlSignal::self_consumption(true, false),
            ControlSignal::SelfConsumption {
                enabled: true,
                solar_only_charging: false,
            }
        );

        assert_eq!(
            ControlSignal::demand_response(DRLevel::Critical, Some(1800.0)),
            ControlSignal::DemandResponse {
                level: DRLevel::Critical,
                duration_s: Some(1800.0),
            }
        );

        assert_eq!(
            ControlSignal::humidity_setpoint(0.45, Some(0.30), Some(0.60)),
            ControlSignal::HumiditySetpoint {
                target_rh: 0.45,
                min_rh: Some(0.30),
                max_rh: Some(0.60),
            }
        );

        assert_eq!(
            ControlSignal::protocol_native(ProtocolId(9), vec![1, 2, 3]),
            ControlSignal::ProtocolNative {
                protocol: ProtocolId(9),
                payload: vec![1, 2, 3],
            }
        );
    }

    #[cfg(debug_assertions)]
    mod debug_assertion_tests {
        use super::ControlSignalConstructors;
        use hares_types::{ControlSignal, DRLevel};

        // ── power_setpoint ──────────────────────────────────────────

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "active_power_kw must be finite")]
        fn power_setpoint_rejects_infinite_active_power() {
            ControlSignal::power_setpoint(f64::INFINITY, None);
        }

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "reactive_power_kvar must be finite")]
        fn power_setpoint_rejects_infinite_reactive_power() {
            ControlSignal::power_setpoint(5.0, Some(f64::INFINITY));
        }

        // ── power_limit ────────────────────────────────────────────

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "max_power_kw must be >= 0.0 and finite")]
        fn power_limit_rejects_negative_max_power() {
            ControlSignal::power_limit(-5.0, None);
        }

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "max_power_kw must be >= 0.0 and finite")]
        fn power_limit_rejects_infinite_max_power() {
            ControlSignal::power_limit(f64::INFINITY, None);
        }

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "ramp_rate_kw_per_s must be >= 0.0 and finite")]
        fn power_limit_rejects_negative_ramp_rate() {
            ControlSignal::power_limit(7.0, Some(-0.5));
        }

        // ── soc_target ─────────────────────────────────────────────

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "target_soc must be in [0,1] and finite")]
        fn soc_target_rejects_target_soc_below_zero() {
            ControlSignal::soc_target(-0.5, None, None);
        }

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "target_soc must be in [0,1] and finite")]
        fn soc_target_rejects_target_soc_above_one() {
            ControlSignal::soc_target(1.5, None, None);
        }

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "target_soc must be in [0,1] and finite")]
        fn soc_target_rejects_infinite_target_soc() {
            ControlSignal::soc_target(f64::INFINITY, None, None);
        }

        // ── duty_cycle ─────────────────────────────────────────────

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "on_fraction must be in [0,1] and finite")]
        fn duty_cycle_rejects_on_fraction_below_zero() {
            ControlSignal::duty_cycle(-0.1, None, None);
        }

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "on_fraction must be in [0,1] and finite")]
        fn duty_cycle_rejects_on_fraction_above_one() {
            ControlSignal::duty_cycle(1.5, None, None);
        }

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "on_fraction must be in [0,1] and finite")]
        fn duty_cycle_rejects_infinite_on_fraction() {
            ControlSignal::duty_cycle(f64::INFINITY, None, None);
        }

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "period_s must be > 0.0 and finite")]
        fn duty_cycle_rejects_negative_period() {
            ControlSignal::duty_cycle(0.5, Some(-10.0), None);
        }

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "period_s must be > 0.0 and finite")]
        fn duty_cycle_rejects_zero_period() {
            ControlSignal::duty_cycle(0.5, Some(0.0), None);
        }

        // ── load_fraction ──────────────────────────────────────────

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "fraction must be in [0,1] and finite")]
        fn load_fraction_rejects_fraction_below_zero() {
            ControlSignal::load_fraction(-0.1);
        }

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "fraction must be in [0,1] and finite")]
        fn load_fraction_rejects_fraction_above_one() {
            ControlSignal::load_fraction(1.5);
        }

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "fraction must be in [0,1] and finite")]
        fn load_fraction_rejects_infinite_fraction() {
            ControlSignal::load_fraction(f64::INFINITY);
        }

        // ── demand_response ────────────────────────────────────────

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "duration_s must be >= 0.0 and finite")]
        fn demand_response_rejects_negative_duration() {
            ControlSignal::demand_response(DRLevel::Moderate, Some(-10.0));
        }

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "duration_s must be >= 0.0 and finite")]
        fn demand_response_rejects_infinite_duration() {
            ControlSignal::demand_response(DRLevel::Moderate, Some(f64::INFINITY));
        }

        // ── humidity_setpoint ──────────────────────────────────────

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "target_rh must be in [0,1] and finite")]
        fn humidity_setpoint_rejects_target_rh_below_zero() {
            ControlSignal::humidity_setpoint(-0.1, None, None);
        }

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "target_rh must be in [0,1] and finite")]
        fn humidity_setpoint_rejects_target_rh_above_one() {
            ControlSignal::humidity_setpoint(1.5, None, None);
        }

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "target_rh must be in [0,1] and finite")]
        fn humidity_setpoint_rejects_infinite_target_rh() {
            ControlSignal::humidity_setpoint(f64::INFINITY, None, None);
        }

        // ── soc_target cross-field ─────────────────────────────────────

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "soc_target: min_soc")]
        fn soc_target_rejects_inverted_bounds() {
            ControlSignal::soc_target(0.5, Some(0.9), Some(0.2));
        }

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "soc_target: target_soc")]
        fn soc_target_rejects_target_below_min() {
            ControlSignal::soc_target(0.3, Some(0.5), Some(0.9));
        }

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "soc_target: target_soc")]
        fn soc_target_rejects_target_above_max() {
            ControlSignal::soc_target(1.0, Some(0.2), Some(0.9));
        }

        // ── humidity_setpoint cross-field ──────────────────────────────

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "humidity_setpoint: min_rh")]
        fn humidity_setpoint_rejects_inverted_bounds() {
            ControlSignal::humidity_setpoint(0.45, Some(0.60), Some(0.30));
        }

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "humidity_setpoint: target_rh")]
        fn humidity_setpoint_rejects_target_below_min() {
            ControlSignal::humidity_setpoint(0.20, Some(0.30), Some(0.60));
        }

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "humidity_setpoint: target_rh")]
        fn humidity_setpoint_rejects_target_above_max() {
            ControlSignal::humidity_setpoint(0.70, Some(0.30), Some(0.60));
        }

        // ── thermal_setpoint cross-field ───────────────────────────────

        #[test]
        #[cfg(debug_assertions)]
        #[should_panic(expected = "thermal_setpoint: heating_setpoint_c")]
        fn thermal_setpoint_rejects_heating_above_cooling() {
            ControlSignal::thermal_setpoint(Some(24.0), Some(20.0), None);
        }
    }
}
