//! Equipment capability flags and introspection.

use hares_types::ControlSignal;

pub use hares_types::ControlCapabilities;

/// Returns true when the equipment capability set accepts the provided signal type.
pub fn can_accept(caps: ControlCapabilities, signal: &ControlSignal) -> bool {
    caps.contains(signal.required_capability())
}

#[cfg(test)]
mod tests {
    use super::{ControlCapabilities, can_accept};
    use crate::ControlSignalConstructors;
    use hares_types::{ControlSignal, DRLevel, OperatingMode, ProtocolId};

    fn signals_with_caps() -> Vec<(ControlSignal, ControlCapabilities)> {
        vec![
            (
                ControlSignal::thermal_setpoint(Some(20.0), Some(24.0), Some(1.0)),
                ControlCapabilities::THERMAL_SETPOINT,
            ),
            (
                ControlSignal::power_setpoint(5.0, None),
                ControlCapabilities::POWER_SETPOINT,
            ),
            (
                ControlSignal::power_limit(8.0, None),
                ControlCapabilities::POWER_LIMIT,
            ),
            (
                ControlSignal::soc_target(0.6, None, None),
                ControlCapabilities::SOC_TARGET,
            ),
            (
                ControlSignal::mode_override(OperatingMode::Heating),
                ControlCapabilities::MODE_OVERRIDE,
            ),
            (
                ControlSignal::duty_cycle(0.5, Some(900.0), None),
                ControlCapabilities::DUTY_CYCLE,
            ),
            (
                ControlSignal::load_fraction(0.7),
                ControlCapabilities::LOAD_FRACTION,
            ),
            (
                ControlSignal::grid_connect(true),
                ControlCapabilities::GRID_CONNECT,
            ),
            (
                ControlSignal::self_consumption(true, false),
                ControlCapabilities::SELF_CONSUMPTION,
            ),
            (
                ControlSignal::demand_response(DRLevel::High, Some(600.0)),
                ControlCapabilities::DEMAND_RESPONSE,
            ),
            (
                ControlSignal::humidity_setpoint(0.45, Some(0.30), Some(0.60)),
                ControlCapabilities::HUMIDITY_SETPOINT,
            ),
            (
                ControlSignal::protocol_native(ProtocolId(42), vec![7, 8]),
                ControlCapabilities::PROTOCOL_NATIVE,
            ),
        ]
    }

    #[test]
    fn matching_capability_is_accepted() {
        for (signal, cap) in signals_with_caps() {
            assert!(can_accept(cap, &signal));
        }
    }

    #[test]
    fn missing_capability_is_rejected_for_all_signal_types() {
        for (signal, _cap) in signals_with_caps() {
            let non_matching_caps = ControlCapabilities::empty();
            assert!(!can_accept(non_matching_caps, &signal));
        }
    }
}
