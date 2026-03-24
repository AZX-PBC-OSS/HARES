//! Control signal enum and associated types.

use hares_types::{ControlSignal, DRLevel, OperatingMode, ProtocolId};

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

    fn duty_cycle(on_fraction: f64, period_s: Option<f64>) -> ControlSignal;

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
        ControlSignal::ThermalSetpoint {
            heating_setpoint_c,
            cooling_setpoint_c,
            deadband_c,
        }
    }

    fn power_setpoint(active_power_kw: f64, reactive_power_kvar: Option<f64>) -> ControlSignal {
        ControlSignal::PowerSetpoint {
            active_power_kw,
            reactive_power_kvar,
        }
    }

    fn power_limit(max_power_kw: f64, ramp_rate_kw_per_s: Option<f64>) -> ControlSignal {
        ControlSignal::PowerLimit {
            max_power_kw,
            ramp_rate_kw_per_s,
        }
    }

    fn soc_target(target_soc: f64, min_soc: Option<f64>, max_soc: Option<f64>) -> ControlSignal {
        ControlSignal::SOCTarget {
            target_soc,
            min_soc,
            max_soc,
        }
    }

    fn mode_override(mode: OperatingMode) -> ControlSignal {
        ControlSignal::ModeOverride { mode }
    }

    fn duty_cycle(on_fraction: f64, period_s: Option<f64>) -> ControlSignal {
        ControlSignal::DutyCycle {
            on_fraction,
            period_s,
            component: None,
        }
    }

    fn load_fraction(fraction: f64) -> ControlSignal {
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
        ControlSignal::DemandResponse { level, duration_s }
    }

    fn humidity_setpoint(
        target_rh: f64,
        min_rh: Option<f64>,
        max_rh: Option<f64>,
    ) -> ControlSignal {
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
    use hares_types::{ControlSignal, DRLevel, OperatingMode, ProtocolId};

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
            ControlSignal::duty_cycle(0.6, Some(600.0)),
            ControlSignal::DutyCycle {
                on_fraction: 0.6,
                period_s: Some(600.0),
                component: None,
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
}
