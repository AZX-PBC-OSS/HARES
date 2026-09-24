//! OCHRE control API compatibility mapping.

use std::collections::HashMap;

use hares_types::ControlSignal;

use crate::ControlSignalConstructors;

const KEY_SETPOINT_TEMPERATURE_C: &str = "Setpoint Temperature (C)";
const KEY_POWER_SETPOINT_KW: &str = "P Setpoint";
const KEY_DUTY_CYCLE: &str = "Duty Cycle";
const KEY_LOAD_FRACTION: &str = "Load Fraction";
const KEY_SOC: &str = "SOC";
const KEY_MIN_SOC: &str = "Min SOC";
const KEY_MAX_SOC: &str = "Max SOC";
const KEY_SELF_CONSUMPTION_MODE: &str = "Self Consumption Mode";

/// Maps OCHRE key-value controls for one equipment instance into typed control signals.
///
/// Boolean convention: OCHRE booleans are encoded as `f64`, where `1.0` means `true`
/// and `0.0` means `false`.
///
/// Unknown keys are silently skipped.
pub fn ochre_signal_to_control(
    equipment_type: &str,
    signals: &HashMap<String, f64>,
) -> Vec<ControlSignal> {
    let mut out = Vec::new();

    let soc_target = signals.get(KEY_SOC).copied();
    let min_soc = signals.get(KEY_MIN_SOC).copied();
    let max_soc = signals.get(KEY_MAX_SOC).copied();

    // Merge SOC / Min SOC / Max SOC into one SOCTarget when present.
    // - SOC alone: target, min, and max all set to the SOC value.
    // - Min + Max only: target defaults to min_soc (conservative -- don't discharge below minimum).
    // - Any combination: target_soc = SOC if present, else min_soc, else max_soc.
    if soc_target.is_some() || min_soc.is_some() || max_soc.is_some() {
        let effective_target = soc_target
            .or(min_soc)
            .or(max_soc)
            .expect("at least one SOC key must be Some");
        let effective_min = min_soc.or(soc_target);
        let effective_max = max_soc.or(soc_target);
        out.push(ControlSignal::soc_target(
            effective_target,
            effective_min,
            effective_max,
        ));
    }

    for (key, value) in signals {
        match key.as_str() {
            KEY_SETPOINT_TEMPERATURE_C => {
                let equipment = equipment_type.to_ascii_lowercase();
                let (heat, cool) = if equipment.contains("cool") {
                    (None, Some(*value))
                } else {
                    (Some(*value), None)
                };
                out.push(ControlSignal::thermal_setpoint(heat, cool, None));
            }
            KEY_POWER_SETPOINT_KW => {
                out.push(ControlSignal::power_setpoint(*value, None));
            }
            KEY_DUTY_CYCLE => {
                out.push(ControlSignal::duty_cycle(*value, None, None));
            }
            KEY_LOAD_FRACTION => {
                out.push(ControlSignal::load_fraction(*value));
            }
            KEY_SELF_CONSUMPTION_MODE => {
                out.push(ControlSignal::self_consumption(*value == 1.0, false));
            }
            // SOC keys are already handled in grouped form above.
            KEY_SOC | KEY_MIN_SOC | KEY_MAX_SOC => {}
            unknown => {
                tracing::warn!(
                    key = unknown,
                    ?equipment_type,
                    "ochre_signal_to_control: unrecognized key",
                );
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use hares_types::ControlSignal;

    use super::*;

    fn map(entries: &[(&str, f64)]) -> HashMap<String, f64> {
        entries
            .iter()
            .map(|(k, v)| ((*k).to_string(), *v))
            .collect::<HashMap<_, _>>()
    }

    #[test]
    fn maps_each_documented_ochre_key() {
        let s = map(&[
            (KEY_SETPOINT_TEMPERATURE_C, 21.0),
            (KEY_POWER_SETPOINT_KW, 3.5),
            (KEY_DUTY_CYCLE, 0.4),
            (KEY_LOAD_FRACTION, 0.8),
            (KEY_SOC, 0.6),
            (KEY_SELF_CONSUMPTION_MODE, 1.0),
        ]);

        let out = ochre_signal_to_control("HVAC Heating", &s);

        assert!(out.iter().any(|x| matches!(
            x,
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(21.0),
                cooling_setpoint_c: None,
                deadband_c: None
            }
        )));
        assert!(out.iter().any(|x| matches!(
            x,
            ControlSignal::PowerSetpoint {
                active_power_kw: 3.5,
                reactive_power_kvar: None,
                ..
            }
        )));
        assert!(out.iter().any(|x| matches!(
            x,
            ControlSignal::DutyCycle {
                on_fraction: 0.4,
                period_s: None,
                component: None
            }
        )));
        assert!(
            out.iter()
                .any(|x| matches!(x, ControlSignal::LoadFraction { fraction: 0.8 }))
        );
        assert!(out.iter().any(|x| matches!(
            x,
            ControlSignal::SOCTarget {
                target_soc: 0.6,
                min_soc: Some(0.6),
                max_soc: Some(0.6)
            }
        )));
        assert!(out.iter().any(|x| matches!(
            x,
            ControlSignal::SelfConsumption {
                enabled: true,
                solar_only_charging: false
            }
        )));
    }

    #[test]
    fn soc_bounds_are_grouped_into_one_signal() {
        let s = map(&[(KEY_MIN_SOC, 0.2), (KEY_MAX_SOC, 0.9)]);
        let out = ochre_signal_to_control("Battery", &s);

        let soc_signals: Vec<_> = out
            .iter()
            .filter(|x| matches!(x, ControlSignal::SOCTarget { .. }))
            .collect();
        assert_eq!(soc_signals.len(), 1);
        assert!(matches!(
            soc_signals[0],
            ControlSignal::SOCTarget {
                target_soc: 0.2,
                min_soc: Some(0.2),
                max_soc: Some(0.9)
            }
        ));
    }

    #[test]
    fn boolean_self_consumption_mapping_uses_0_1_convention() {
        let enabled = ochre_signal_to_control("Battery", &map(&[(KEY_SELF_CONSUMPTION_MODE, 1.0)]));
        let disabled =
            ochre_signal_to_control("Battery", &map(&[(KEY_SELF_CONSUMPTION_MODE, 0.0)]));

        assert!(matches!(
            enabled.as_slice(),
            [ControlSignal::SelfConsumption {
                enabled: true,
                solar_only_charging: false
            }]
        ));
        assert!(matches!(
            disabled.as_slice(),
            [ControlSignal::SelfConsumption {
                enabled: false,
                solar_only_charging: false
            }]
        ));
    }

    #[test]
    fn only_max_soc_uses_max_as_target() {
        let s = map(&[(KEY_MAX_SOC, 0.95)]);
        let out = ochre_signal_to_control("Battery", &s);

        let soc_signals: Vec<_> = out
            .iter()
            .filter(|x| matches!(x, ControlSignal::SOCTarget { .. }))
            .collect();
        assert_eq!(soc_signals.len(), 1);
        // No min_soc available, so target falls back to max_soc (only bound present).
        assert!(matches!(
            soc_signals[0],
            ControlSignal::SOCTarget {
                target_soc: 0.95,
                min_soc: None,
                max_soc: Some(0.95)
            }
        ));
    }

    #[test]
    fn only_min_soc_uses_min_as_target() {
        let s = map(&[(KEY_MIN_SOC, 0.2)]);
        let out = ochre_signal_to_control("Battery", &s);

        let soc_signals: Vec<_> = out
            .iter()
            .filter(|x| matches!(x, ControlSignal::SOCTarget { .. }))
            .collect();
        assert_eq!(soc_signals.len(), 1);
        assert!(matches!(
            soc_signals[0],
            ControlSignal::SOCTarget {
                target_soc: 0.2,
                min_soc: Some(0.2),
                max_soc: None
            }
        ));
    }

    #[test]
    fn cooling_equipment_routes_setpoint_to_cooling() {
        let s = map(&[(KEY_SETPOINT_TEMPERATURE_C, 24.0)]);
        let out = ochre_signal_to_control("Air Conditioner (Cooling)", &s);
        assert!(out.iter().any(|x| matches!(
            x,
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c: None,
                cooling_setpoint_c: Some(24.0),
                deadband_c: None
            }
        )));
    }

    #[test]
    fn soc_all_three_keys_grouped() {
        // SOC + Min SOC + Max SOC -> single SOCTarget
        let mut signals = HashMap::new();
        signals.insert("SOC".to_string(), 0.6);
        signals.insert("Min SOC".to_string(), 0.2);
        signals.insert("Max SOC".to_string(), 0.9);
        let result = ochre_signal_to_control("Battery", &signals);
        assert_eq!(result.len(), 1);
        match &result[0] {
            ControlSignal::SOCTarget {
                target_soc,
                min_soc,
                max_soc,
            } => {
                assert!((target_soc - 0.6).abs() < 1e-10, "target_soc should be 0.6");
                assert_eq!(*min_soc, Some(0.2));
                assert_eq!(*max_soc, Some(0.9));
            }
            other => panic!("expected SOCTarget, got {other:?}"),
        }
    }

    #[test]
    fn unknown_key_is_silently_skipped() {
        let out = ochre_signal_to_control("Battery", &map(&[("Not A Real Key", 5.0)]));
        assert!(out.is_empty());
    }
}
