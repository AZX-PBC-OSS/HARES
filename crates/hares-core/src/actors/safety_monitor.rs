//! Safety monitor actor that dispatches Safety-tier control signals when
//! zone temperatures breach configurable thresholds.
//!
//! Two safety conditions are monitored every timestep:
//!
//! - **Freeze protection**: any zone temp < freeze threshold triggers
//!   `ModeOverride { mode: Heating }` at `PriorityTier::Safety`. Default
//!   threshold is 5°C — an engineering default aligned with freeze-stat
//!   setpoints used in building simulation (EnergyPlus pipe freeze protection
//!   and ASHRAE Guideline 36 freeze-stat guidance for air-handling units).
//!   See [`DEFAULT_FREEZE_THRESHOLD_C`](super::constants) for the full
//!   source citation.
//! - **Over-temperature lockout**: any zone temp > over-temp threshold
//!   triggers `ModeOverride { mode: Off }` at `PriorityTier::Safety`.
//!   Default threshold is 50°C — a conservative engineering default chosen
//!   above the typical 35–40°C maximum indoor temperature seen in overheating
//!   studies to avoid tripping on hot-but-safe days while protecting against
//!   runaway heating equipment failure.

use std::sync::Arc;

use hares_control::{DispatchRequest, DispatchTarget, PriorityTier};
use hares_types::{ControlSignal, EndUse, EnvironmentState, OperatingMode, Telemetry};

use crate::actor::Actor;

use super::constants::DEFAULT_FREEZE_THRESHOLD_C;

/// Default over-temperature lockout threshold in °C.
pub const DEFAULT_OVER_TEMP_THRESHOLD_C: f64 = 50.0;

/// Safety monitor actor that guards against temperature extremes.
///
/// Registered on the dwelling via `dwelling.add_actor()`, this actor runs
/// every timestep and checks zone temperatures against configurable
/// thresholds. When a breach is detected, it dispatches a Safety-tier
/// control signal that overrides all lower-priority signals (Schedule,
/// UserOverride, Grid).
///
/// # Example
///
/// ```ignore
/// let monitor = SafetyMonitor::new("safety")
///     .with_target(DispatchTarget::ByEndUse(EndUse::HVAC_HEATING))
///     .with_freeze_protection_threshold(3.0);
/// dwelling.add_actor(Box::new(monitor));
/// ```
pub struct SafetyMonitor {
    name: Arc<str>,
    target: DispatchTarget,
    freeze_protection_threshold_c: f64,
    over_temperature_threshold_c: f64,
    telemetry: Telemetry,
}

impl SafetyMonitor {
    /// Creates a new safety monitor with default thresholds.
    ///
    /// Default target is `DispatchTarget::ByEndUse(EndUse::HVAC_HEATING)`.
    /// Default freeze protection threshold is 5°C.
    /// Default over-temperature threshold is 50°C.
    pub fn new(name: &str) -> Self {
        let mut telemetry = Telemetry::with_capacity(4);
        telemetry.insert("freeze_alarm", 0.0);
        telemetry.insert("over_temp_alarm", 0.0);
        telemetry.insert("safety_signals_count", 0.0);
        telemetry.insert("min_zone_temp_c", 0.0);
        Self {
            name: Arc::from(name),
            target: DispatchTarget::ByEndUse(EndUse::HVAC_HEATING),
            freeze_protection_threshold_c: DEFAULT_FREEZE_THRESHOLD_C,
            over_temperature_threshold_c: DEFAULT_OVER_TEMP_THRESHOLD_C,
            telemetry,
        }
    }

    /// Sets the dispatch target for safety signals.
    pub fn with_target(mut self, target: DispatchTarget) -> Self {
        self.target = target;
        self
    }

    /// Sets the freeze protection threshold in °C.
    ///
    /// When any zone temperature drops below this value, a Safety-tier
    /// `ModeOverride { mode: Heating }` is dispatched.
    pub fn with_freeze_protection_threshold(mut self, threshold_c: f64) -> Self {
        self.freeze_protection_threshold_c = threshold_c;
        self
    }

    /// Sets the over-temperature lockout threshold in °C.
    ///
    /// When any zone temperature exceeds this value, a Safety-tier
    /// `ModeOverride { mode: Off }` is dispatched.
    pub fn with_over_temperature_threshold(mut self, threshold_c: f64) -> Self {
        self.over_temperature_threshold_c = threshold_c;
        self
    }

    /// Disables over-temperature lockout by setting the threshold to
    /// an unreachable value.
    pub fn without_over_temperature(mut self) -> Self {
        self.over_temperature_threshold_c = f64::MAX;
        self
    }

    /// Disables freeze protection by setting the threshold to
    /// an unreachable value.
    pub fn without_freeze_protection(mut self) -> Self {
        self.freeze_protection_threshold_c = f64::NEG_INFINITY;
        self
    }

    /// Returns the current freeze protection threshold.
    pub fn freeze_protection_threshold(&self) -> f64 {
        self.freeze_protection_threshold_c
    }

    /// Returns the current over-temperature threshold.
    pub fn over_temperature_threshold(&self) -> f64 {
        self.over_temperature_threshold_c
    }

    fn has_freeze_breach(&self, zone_temp_c: f64) -> bool {
        zone_temp_c.is_finite() && zone_temp_c < self.freeze_protection_threshold_c
    }

    fn has_over_temp_breach(&self, zone_temp_c: f64) -> bool {
        zone_temp_c.is_finite() && zone_temp_c > self.over_temperature_threshold_c
    }
}

impl Actor for SafetyMonitor {
    fn name(&self) -> &str {
        &self.name
    }

    fn telemetry(&self) -> Option<&Telemetry> {
        Some(&self.telemetry)
    }

    fn decide(&mut self, env: &EnvironmentState, out: &mut Vec<DispatchRequest>) {
        let mut freeze_breach = false;
        let mut over_temp_breach = false;
        let mut min_temp = f64::INFINITY;
        let mut max_temp = f64::NEG_INFINITY;

        for zone in &env.zones {
            let temp = zone.temperature_c;
            if !temp.is_finite() {
                continue;
            }
            if temp < min_temp {
                min_temp = temp;
            }
            if temp > max_temp {
                max_temp = temp;
            }
            if self.has_freeze_breach(temp) {
                freeze_breach = true;
            }
            if self.has_over_temp_breach(temp) {
                over_temp_breach = true;
            }
        }

        let before = out.len();

        if freeze_breach {
            tracing::warn!(
                actor = %self.name,
                min_zone_temp_c = min_temp,
                threshold_c = self.freeze_protection_threshold_c,
                "freeze protection: dispatching Safety-tier Heating override"
            );
            out.push(DispatchRequest {
                target: self.target.clone(),
                signal: ControlSignal::ModeOverride {
                    mode: OperatingMode::Heating,
                },
                priority: PriorityTier::Safety,
            });
        }

        if over_temp_breach {
            tracing::warn!(
                actor = %self.name,
                max_zone_temp_c = max_temp,
                threshold_c = self.over_temperature_threshold_c,
                "over-temperature lockout: dispatching Safety-tier Off override"
            );
            out.push(DispatchRequest {
                target: self.target.clone(),
                signal: ControlSignal::ModeOverride {
                    mode: OperatingMode::Off,
                },
                priority: PriorityTier::Safety,
            });
        }

        self.telemetry
            .set("freeze_alarm", if freeze_breach { 1.0 } else { 0.0 });
        self.telemetry
            .set("over_temp_alarm", if over_temp_breach { 1.0 } else { 0.0 });
        self.telemetry
            .set("safety_signals_count", (out.len() - before) as f64);
        if env.zones.iter().any(|z| z.temperature_c.is_finite()) {
            self.telemetry.set("min_zone_temp_c", min_temp);
        }

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            // Invariant: no Safety-tier signal is emitted without an actual
            // threshold breach detected during this step.
            if !freeze_breach {
                let has_heating = out.iter().any(|req| {
                    req.priority == PriorityTier::Safety
                        && matches!(
                            &req.signal,
                            ControlSignal::ModeOverride {
                                mode: OperatingMode::Heating
                            }
                        )
                });
                debug_assert!(
                    !has_heating,
                    "Safety-tier Heating signal emitted without freeze breach"
                );
            }
            if !over_temp_breach {
                let has_off = out.iter().any(|req| {
                    req.priority == PriorityTier::Safety
                        && matches!(
                            &req.signal,
                            ControlSignal::ModeOverride {
                                mode: OperatingMode::Off
                            }
                        )
                });
                debug_assert!(
                    !has_off,
                    "Safety-tier Off signal emitted without over-temperature breach"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use hares_types::EndUse;

    use super::*;
    use crate::actor::testing::test_env;

    // -----------------------------------------------------------------------
    // Freeze protection
    // -----------------------------------------------------------------------

    #[test]
    fn emits_heating_on_freeze_protection() {
        let mut monitor =
            SafetyMonitor::new("test").with_target(DispatchTarget::ByEndUse(EndUse::HVAC_HEATING));
        let env = test_env().zone_temp(3.0).build();
        let mut out = Vec::new();
        monitor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].priority, PriorityTier::Safety);
        assert!(matches!(
            &out[0].signal,
            ControlSignal::ModeOverride {
                mode: OperatingMode::Heating
            }
        ));
        assert_eq!(monitor.telemetry.get("freeze_alarm"), Some(1.0));
        assert_eq!(monitor.telemetry.get("over_temp_alarm"), Some(0.0));
        assert_eq!(monitor.telemetry.get("safety_signals_count"), Some(1.0));
    }

    #[test]
    fn no_dispatch_above_freeze_threshold() {
        let mut monitor =
            SafetyMonitor::new("test").with_target(DispatchTarget::ByEndUse(EndUse::HVAC_HEATING));
        let env = test_env().zone_temp(22.0).build();
        let mut out = Vec::new();
        monitor.decide(&env, &mut out);

        assert!(out.is_empty());
        assert_eq!(monitor.telemetry.get("freeze_alarm"), Some(0.0));
        assert_eq!(monitor.telemetry.get("over_temp_alarm"), Some(0.0));
        assert_eq!(monitor.telemetry.get("safety_signals_count"), Some(0.0));
    }

    #[test]
    fn no_dispatch_at_exact_freeze_threshold() {
        let mut monitor =
            SafetyMonitor::new("test").with_target(DispatchTarget::ByEndUse(EndUse::HVAC_HEATING));
        let env = test_env().zone_temp(DEFAULT_FREEZE_THRESHOLD_C).build();
        let mut out = Vec::new();
        monitor.decide(&env, &mut out);

        assert!(out.is_empty());
    }

    #[test]
    fn emits_when_below_custom_freeze_threshold() {
        let mut monitor = SafetyMonitor::new("test")
            .with_target(DispatchTarget::ByEndUse(EndUse::HVAC_HEATING))
            .with_freeze_protection_threshold(10.0);
        let env = test_env().zone_temp(8.0).build();
        let mut out = Vec::new();
        monitor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        assert!(matches!(
            &out[0].signal,
            ControlSignal::ModeOverride {
                mode: OperatingMode::Heating
            }
        ));
    }

    // -----------------------------------------------------------------------
    // Over-temperature lockout
    // -----------------------------------------------------------------------

    #[test]
    fn emits_off_on_over_temperature() {
        let mut monitor = SafetyMonitor::new("test")
            .with_target(DispatchTarget::ByEndUse(EndUse::HVAC_HEATING))
            .with_over_temperature_threshold(45.0);
        let env = test_env().zone_temp(46.0).build();
        let mut out = Vec::new();
        monitor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].priority, PriorityTier::Safety);
        assert!(matches!(
            &out[0].signal,
            ControlSignal::ModeOverride {
                mode: OperatingMode::Off
            }
        ));
        assert_eq!(monitor.telemetry.get("over_temp_alarm"), Some(1.0));
        assert_eq!(monitor.telemetry.get("freeze_alarm"), Some(0.0));
    }

    #[test]
    fn no_dispatch_below_over_temp_threshold() {
        let mut monitor =
            SafetyMonitor::new("test").with_target(DispatchTarget::ByEndUse(EndUse::HVAC_HEATING));
        let env = test_env().zone_temp(22.0).build();
        let mut out = Vec::new();
        monitor.decide(&env, &mut out);

        assert!(out.is_empty());
        assert_eq!(monitor.telemetry.get("over_temp_alarm"), Some(0.0));
    }

    #[test]
    fn no_dispatch_at_exact_over_temp_threshold() {
        let threshold = 45.0;
        let mut monitor = SafetyMonitor::new("test")
            .with_target(DispatchTarget::ByEndUse(EndUse::HVAC_HEATING))
            .with_over_temperature_threshold(threshold);
        let env = test_env().zone_temp(threshold).build();
        let mut out = Vec::new();
        monitor.decide(&env, &mut out);

        assert!(out.is_empty());
    }

    // -----------------------------------------------------------------------
    // Multiple conditions
    // -----------------------------------------------------------------------

    #[test]
    fn emits_both_heating_and_off_with_multi_zone() {
        let mut monitor = SafetyMonitor::new("test")
            .with_target(DispatchTarget::ByEndUse(EndUse::HVAC_HEATING))
            .with_freeze_protection_threshold(5.0)
            .with_over_temperature_threshold(50.0);

        let mut env = test_env().build();
        env.zones = vec![
            hares_types::ZoneState {
                id: hares_types::ZoneId(1),
                temperature_c: 3.0,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: 14.0,
                volume_m3: 200.0,
            },
            hares_types::ZoneState {
                id: hares_types::ZoneId(2),
                temperature_c: 55.0,
                humidity_ratio: 0.010,
                relative_humidity: 0.15,
                wet_bulb_c: 25.0,
                volume_m3: 200.0,
            },
        ];

        let mut out = Vec::new();
        monitor.decide(&env, &mut out);

        assert_eq!(
            out.len(),
            2,
            "expected 2 dispatches (heating + off), got {:?}",
            out
        );
        assert_eq!(monitor.telemetry.get("freeze_alarm"), Some(1.0));
        assert_eq!(monitor.telemetry.get("over_temp_alarm"), Some(1.0));
        assert_eq!(monitor.telemetry.get("safety_signals_count"), Some(2.0));

        let has_heating = out.iter().any(|req| {
            matches!(
                &req.signal,
                ControlSignal::ModeOverride {
                    mode: OperatingMode::Heating
                }
            )
        });
        let has_off = out.iter().any(|req| {
            matches!(
                &req.signal,
                ControlSignal::ModeOverride {
                    mode: OperatingMode::Off
                }
            )
        });
        assert!(has_heating, "missing heating dispatch");
        assert!(has_off, "missing off dispatch");
    }

    #[test]
    fn multiple_zones_single_breach_emits_one_dispatch() {
        let mut monitor =
            SafetyMonitor::new("test").with_target(DispatchTarget::ByEndUse(EndUse::HVAC_HEATING));
        let mut env = test_env().build();
        env.zones = vec![
            hares_types::ZoneState {
                id: hares_types::ZoneId(1),
                temperature_c: 3.0,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: 14.0,
                volume_m3: 200.0,
            },
            hares_types::ZoneState {
                id: hares_types::ZoneId(2),
                temperature_c: 3.5,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: 14.0,
                volume_m3: 150.0,
            },
        ];

        let mut out = Vec::new();
        monitor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        assert_eq!(monitor.telemetry.get("safety_signals_count"), Some(1.0));
    }

    // -----------------------------------------------------------------------
    // Edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn no_zones_produces_no_dispatch() {
        let mut monitor =
            SafetyMonitor::new("test").with_target(DispatchTarget::ByEndUse(EndUse::HVAC_HEATING));
        let mut env = test_env().build();
        env.zones.clear();

        let mut out = Vec::new();
        monitor.decide(&env, &mut out);

        assert!(out.is_empty());
    }

    #[test]
    fn nan_zone_temp_is_ignored() {
        let mut monitor =
            SafetyMonitor::new("test").with_target(DispatchTarget::ByEndUse(EndUse::HVAC_HEATING));
        let env = test_env().zone_temp(f64::NAN).build();
        let mut out = Vec::new();
        monitor.decide(&env, &mut out);

        assert!(out.is_empty());
        assert_eq!(monitor.telemetry.get("freeze_alarm"), Some(0.0));
    }

    #[test]
    fn infinite_zone_temp_is_ignored() {
        let mut monitor =
            SafetyMonitor::new("test").with_target(DispatchTarget::ByEndUse(EndUse::HVAC_HEATING));
        // Negative infinity shouldn't trigger freeze protection because
        // is_finite() check rejects it.
        let env = test_env().zone_temp(f64::NEG_INFINITY).build();
        let mut out = Vec::new();
        monitor.decide(&env, &mut out);

        assert!(out.is_empty());
    }

    // -----------------------------------------------------------------------
    // Configuration
    // -----------------------------------------------------------------------

    #[test]
    fn without_freeze_protection_disables_check() {
        let mut monitor = SafetyMonitor::new("test")
            .with_target(DispatchTarget::ByEndUse(EndUse::HVAC_HEATING))
            .without_freeze_protection();
        let env = test_env().zone_temp(-999.0).build();
        let mut out = Vec::new();
        monitor.decide(&env, &mut out);

        assert!(out.is_empty());
    }

    #[test]
    fn without_over_temperature_disables_check() {
        let mut monitor = SafetyMonitor::new("test")
            .with_target(DispatchTarget::ByEndUse(EndUse::HVAC_HEATING))
            .without_over_temperature();
        let env = test_env().zone_temp(999.0).build();
        let mut out = Vec::new();
        monitor.decide(&env, &mut out);

        assert!(out.is_empty());
    }

    #[test]
    fn custom_target_is_respected() {
        let mut monitor =
            SafetyMonitor::new("test").with_target(DispatchTarget::ByEndUse(EndUse::WATER_HEATING));
        let env = test_env().zone_temp(3.0).build();
        let mut out = Vec::new();
        monitor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].target,
            DispatchTarget::ByEndUse(EndUse::WATER_HEATING)
        );
    }
}
