//! DR compliance actor -- demand response decision modeling.
//!
//! The `DrCompliance` actor models occupant compliance decisions when a DR
//! (demand response) event arrives: does the occupant comply? Do they shed load,
//! adjust thermostat setpoint, turn off equipment?
//!
//! This actor receives DR signals (from schedule data or external control) and
//! decides what equipment actions to take based on occupant preferences, comfort
//! constraints, and behavioral models.
//!
//! # Design
//!
//! Equipment is self-contained with internal schedules. The actor only pushes
//! control signals -- never mutates environment or equipment state directly.
//!
//! # Compliance Models
//!
//! Built-in rule-based models:
//! - `AlwaysComply`: Always participates in DR events
//! - `NeverComply`: Ignores all DR events
//! - `Probabilistic`: Randomly decides compliance with configurable rate
//!
//! Users can plug in richer models (e.g. RL agents) via the `ComplianceModel`
//! trait.
//!
//! # Example
//!
//! ```ignore
//! use hares_core::actors::{DrCompliance, ComplianceModel, AlwaysComply, DrAction};
//! use hares_control::DispatchTarget;
//!
//! // Create actor that always complies with DR events
//! let mut actor = DrCompliance::new("DRResponder")
//!     .with_compliance_model(AlwaysComply)
//!     .with_hvac_target(DispatchTarget::ByName("HVAC".into()))
//!     .with_hvac_action(DrAction::SetpointAdjust { delta_c: 2.0 });
//!
//! // In the dwelling loop, the actor will dispatch signals when DR is active
//! let mut requests = Vec::new();
//! actor.decide(&env, &mut requests);
//! ```

use std::sync::Arc;

use hares_control::{DispatchRequest, DispatchTarget, PriorityTier};
use hares_types::{ControlSignal, DRLevel, EnvironmentState, OperatingMode, Telemetry};

#[cfg(test)]
use hares_types::EndUse;

use crate::Actor;

use super::constants::DEFAULT_FREEZE_THRESHOLD_C;

/// Trait for DR compliance decision models.
///
/// Implementations decide whether an occupant complies with a DR event.
/// The trait is designed for extensibility -- future RL agents implement
/// this interface to replace rule-based models.
pub trait ComplianceModel: Send + Sync {
    /// Returns true if the occupant should comply with the DR event.
    ///
    /// # Arguments
    ///
    /// * `dr_level` - Severity of the DR event (Normal, Moderate, High, Critical, GridEmergency)
    /// * `env` - Current environment state (weather, zone temps, time, etc.)
    fn should_comply(&self, dr_level: DRLevel, env: &EnvironmentState) -> bool;
}

/// Compliance model that always participates in DR events.
#[derive(Clone, Copy, Debug, Default)]
pub struct AlwaysComply;

impl ComplianceModel for AlwaysComply {
    fn should_comply(&self, _dr_level: DRLevel, _env: &EnvironmentState) -> bool {
        true
    }
}

/// Compliance model that never participates in DR events.
#[derive(Clone, Copy, Debug, Default)]
pub struct NeverComply;

impl ComplianceModel for NeverComply {
    fn should_comply(&self, _dr_level: DRLevel, _env: &EnvironmentState) -> bool {
        false
    }
}

/// Compliance model that randomly decides with a configurable probability.
///
/// Uses a fixed seed for reproducibility in simulation.
#[derive(Clone, Debug)]
pub struct Probabilistic {
    /// Probability of compliance in [0.0, 1.0].
    pub compliance_rate: f64,
    /// Seed for deterministic random decisions.
    pub seed: u64,
}

impl Probabilistic {
    /// Creates a new probabilistic model with the given compliance rate.
    ///
    /// # Panics (debug only)
    ///
    /// Panics if `compliance_rate` is outside `[0.0, 1.0]`.
    pub fn new(compliance_rate: f64) -> Self {
        debug_assert!(
            (0.0..=1.0).contains(&compliance_rate),
            "compliance_rate must be in [0.0, 1.0], got {compliance_rate}"
        );
        Self {
            compliance_rate,
            seed: 42,
        }
    }

    /// Sets a custom seed for reproducibility.
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }
}

impl Default for Probabilistic {
    fn default() -> Self {
        Self::new(0.5)
    }
}

impl ComplianceModel for Probabilistic {
    fn should_comply(&self, dr_level: DRLevel, env: &EnvironmentState) -> bool {
        let hash = self.hash_inputs(dr_level, env);
        let normalized = (hash % 10_000) as f64 / 10_000.0;
        normalized < self.compliance_rate
    }
}

impl Probabilistic {
    /// Stable splitmix64 mixing -- deterministic across Rust versions.
    fn hash_inputs(&self, dr_level: DRLevel, env: &EnvironmentState) -> u64 {
        let mut x = self
            .seed
            .wrapping_add(dr_level as u64)
            .wrapping_add(env.current_time.timestamp() as u64);
        x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
        x ^ (x >> 31)
    }
}

/// Action to take when complying with a DR event.
#[derive(Clone, Debug, PartialEq)]
pub enum DrAction {
    /// Reduce load by a fraction (0.0 = full curtailment, 1.0 = no change).
    LoadCurtailment { fraction: f64 },
    /// Adjust thermostat setpoint by a delta relative to current effective
    /// setpoints. Positive `delta_c` raises heating and lowers cooling.
    SetpointAdjust { delta_c: f64 },
    /// Override thermostat to absolute setpoints.
    AbsoluteSetpoint {
        heating_c: Option<f64>,
        cooling_c: Option<f64>,
    },
    /// Turn equipment off.
    TurnOff,
    /// Limit power draw to a maximum.
    PowerLimit { max_kw: f64 },
    /// No action (used for testing).
    None,
}

impl DrAction {
    /// Creates a load curtailment action.
    ///
    /// # Panics (debug only)
    ///
    /// Panics if `fraction` is outside `[0.0, 1.0]`.
    pub fn curtail(fraction: f64) -> Self {
        debug_assert!(
            (0.0..=1.0).contains(&fraction),
            "curtailment fraction must be in [0.0, 1.0], got {fraction}"
        );
        Self::LoadCurtailment { fraction }
    }

    /// Creates a setpoint adjustment action (delta from current setpoints).
    ///
    /// Positive delta raises heating setpoint / lowers cooling setpoint.
    pub fn setpoint_delta(delta_c: f64) -> Self {
        Self::SetpointAdjust { delta_c }
    }

    /// Creates an absolute setpoint override action.
    pub fn absolute_setpoint(heating_c: f64, cooling_c: f64) -> Self {
        debug_assert!(
            heating_c < cooling_c,
            "heating setpoint ({heating_c}°C) must be below cooling setpoint ({cooling_c}°C)"
        );
        Self::AbsoluteSetpoint {
            heating_c: Some(heating_c),
            cooling_c: Some(cooling_c),
        }
    }

    /// Creates a turn-off action.
    pub fn off() -> Self {
        Self::TurnOff
    }

    /// Creates a power limit action.
    ///
    /// # Panics (debug only)
    ///
    /// Panics if `max_kw` is negative.
    pub fn limit_power(max_kw: f64) -> Self {
        debug_assert!(
            max_kw >= 0.0,
            "power limit must be non-negative, got {max_kw}"
        );
        Self::PowerLimit { max_kw }
    }
}

/// Actor that models DR compliance decisions.
///
/// When a DR event is active and the compliance model decides to comply,
/// this actor dispatches control signals at `PriorityTier::Grid` priority
/// (higher than UserOverride and Schedule).
pub struct DrCompliance {
    /// Actor name for diagnostics.
    name: Arc<str>,
    /// Compliance decision model.
    model: Box<dyn ComplianceModel>,
    /// HVAC equipment target for thermal setpoint adjustments.
    hvac_target: Option<DispatchTarget>,
    /// Action to take on HVAC when complying.
    hvac_action: DrAction,
    /// Additional equipment targets for load curtailment.
    load_targets: Vec<(DispatchTarget, DrAction)>,
    /// Current DR level. `Normal` means no DR event is active.
    current_dr_level: DRLevel,
    /// Freeze-risk threshold in °C. When any zone temperature is below this
    /// value and the HVAC action is `TurnOff`, the action is downgraded to a
    /// minimum-heating `ThermalSetpoint` instead.
    ///
    /// Defaults to [`DEFAULT_FREEZE_THRESHOLD_C`] (5°C). See the
    /// [`constants`](super::constants) module for the source citation.
    freeze_risk_threshold_c: f64,
    /// Actor telemetry: observable decision state for diagnostics.
    telemetry: Telemetry,
}

impl DrCompliance {
    /// Creates a new DR compliance actor with the default compliance model.
    pub fn new(name: &str) -> Self {
        let mut telemetry = Telemetry::with_capacity(5);
        telemetry.insert("dr_level", 0.0);
        telemetry.insert("dr_active", 0.0);
        telemetry.insert("dr_complied", 0.0);
        telemetry.insert("signals_count", 0.0);
        telemetry.insert("dr_freeze_guard", 0.0);
        Self {
            name: Arc::from(name),
            model: Box::new(AlwaysComply),
            hvac_target: None,
            hvac_action: DrAction::None,
            load_targets: Vec::new(),
            current_dr_level: DRLevel::Normal,
            freeze_risk_threshold_c: DEFAULT_FREEZE_THRESHOLD_C,
            telemetry,
        }
    }

    /// Sets the compliance model.
    pub fn with_compliance_model(mut self, model: impl ComplianceModel + 'static) -> Self {
        self.model = Box::new(model);
        self
    }

    /// Sets the HVAC equipment target.
    pub fn with_hvac_target(mut self, target: DispatchTarget) -> Self {
        self.hvac_target = Some(target);
        self
    }

    /// Sets the HVAC action to take when complying.
    pub fn with_hvac_action(mut self, action: DrAction) -> Self {
        self.hvac_action = action;
        self
    }

    /// Adds a load curtailment target with a specific action.
    pub fn with_load_target(mut self, target: DispatchTarget, action: DrAction) -> Self {
        self.load_targets.push((target, action));
        self
    }

    /// Sets the DR level. `DRLevel::Normal` clears the DR event.
    pub fn set_dr_level(&mut self, level: DRLevel) {
        self.current_dr_level = level;
    }

    /// Sets the freeze-risk threshold for TurnOff safety guard in °C.
    ///
    /// When any zone temperature is below this threshold and the HVAC action
    /// is `TurnOff`, the action is downgraded to a minimum-heating
    /// `ThermalSetpoint` at this temperature instead.
    pub fn with_freeze_risk_threshold(mut self, threshold_c: f64) -> Self {
        self.freeze_risk_threshold_c = threshold_c;
        self
    }

    /// Returns the current DR level.
    pub fn current_dr_level(&self) -> DRLevel {
        self.current_dr_level
    }

    /// Returns true if a DR event is currently active (level above Normal).
    pub fn is_dr_active(&self) -> bool {
        self.current_dr_level != DRLevel::Normal
    }

    /// Returns true if any zone temperature is below the freeze-risk threshold.
    fn any_zone_below_freeze(&self, env: &EnvironmentState) -> bool {
        env.zones
            .iter()
            .any(|z| z.temperature_c.is_finite() && z.temperature_c < self.freeze_risk_threshold_c)
    }

    /// Generates dispatch signals for the given action.
    ///
    /// All signals are dispatched at `PriorityTier::Grid`. DR events are
    /// utility-initiated and must override both schedule-level operations
    /// and user overrides. This overrides the central
    /// `From<&ControlSignal> for PriorityTier` mapping — a DR-induced
    /// setpoint adjustment or mode override is categorically a grid action,
    /// not a user or schedule action.
    fn dispatch_for_action(
        target: &DispatchTarget,
        action: &DrAction,
        out: &mut Vec<DispatchRequest>,
    ) {
        let signal = match action {
            DrAction::LoadCurtailment { fraction } => ControlSignal::LoadFraction {
                fraction: *fraction,
            },
            DrAction::SetpointAdjust { delta_c } => ControlSignal::ThermalSetpointDelta {
                heating_delta_c: Some(*delta_c),
                cooling_delta_c: Some(-delta_c),
            },
            DrAction::AbsoluteSetpoint {
                heating_c,
                cooling_c,
            } => ControlSignal::ThermalSetpoint {
                heating_setpoint_c: *heating_c,
                cooling_setpoint_c: *cooling_c,
                deadband_c: None,
            },
            DrAction::TurnOff => ControlSignal::ModeOverride {
                mode: OperatingMode::Off,
            },
            DrAction::PowerLimit { max_kw } => ControlSignal::PowerLimit {
                max_power_kw: *max_kw,
                ramp_rate_kw_per_s: None,
            },
            DrAction::None => return,
        };

        out.push(DispatchRequest {
            target: target.clone(),
            signal,
            priority: PriorityTier::Grid,
        });
    }
}

impl Actor for DrCompliance {
    fn name(&self) -> &str {
        &self.name
    }

    fn telemetry(&self) -> Option<&Telemetry> {
        Some(&self.telemetry)
    }

    fn decide(&mut self, env: &EnvironmentState, out: &mut Vec<DispatchRequest>) {
        // Populate telemetry regardless of DR activity.
        self.telemetry
            .set("dr_level", dr_level_as_f64(self.current_dr_level));
        self.telemetry
            .set("dr_active", if self.is_dr_active() { 1.0 } else { 0.0 });

        if self.current_dr_level == DRLevel::Normal {
            self.telemetry.set("dr_complied", 0.0);
            self.telemetry.set("signals_count", 0.0);
            self.telemetry.set("dr_freeze_guard", 0.0);
            return;
        }

        let should_comply = self.model.should_comply(self.current_dr_level, env);

        self.telemetry
            .set("dr_complied", if should_comply { 1.0 } else { 0.0 });

        tracing::debug!(
            actor = %self.name,
            dr_level = ?self.current_dr_level,
            comply = should_comply,
            "DR compliance decision"
        );

        if !should_comply {
            self.telemetry.set("signals_count", 0.0);
            self.telemetry.set("dr_freeze_guard", 0.0);
            return;
        }

        let before = out.len();

        if let Some(target) = &self.hvac_target {
            // Freeze-protection guard: when DR TurnOff targets HVAC and any
            // zone is below the freeze-risk threshold, downgrade to a
            // minimum-heating ThermalSetpoint instead of ModeOverride::Off.
            // This prevents equipment/building damage from freezing during
            // DR events. Long-term: the Safety actor (T-0052) will provide
            // an independent freeze-protection layer at Safety tier, at
            // which point this guard can be relaxed.
            if matches!(&self.hvac_action, DrAction::TurnOff) && self.any_zone_below_freeze(env) {
                out.push(DispatchRequest {
                    target: target.clone(),
                    signal: ControlSignal::ThermalSetpoint {
                        heating_setpoint_c: Some(self.freeze_risk_threshold_c),
                        cooling_setpoint_c: None,
                        deadband_c: None,
                    },
                    priority: PriorityTier::Grid,
                });

                self.telemetry.set("dr_freeze_guard", 1.0);

                tracing::warn!(
                    actor = %self.name,
                    threshold_c = self.freeze_risk_threshold_c,
                    "DR TurnOff downgraded to minimum-heating setpoint: zone temp below freeze-risk threshold"
                );
            } else {
                Self::dispatch_for_action(target, &self.hvac_action, out);
                self.telemetry.set("dr_freeze_guard", 0.0);
            }
        }

        for (target, action) in &self.load_targets {
            let is_turn_off = matches!(action, DrAction::TurnOff);
            let is_hvac_by_end_use = matches!(target, DispatchTarget::ByEndUse(eu) if eu.is_hvac());
            if is_turn_off && is_hvac_by_end_use && self.any_zone_below_freeze(env) {
                out.push(DispatchRequest {
                    target: target.clone(),
                    signal: ControlSignal::ThermalSetpoint {
                        heating_setpoint_c: Some(self.freeze_risk_threshold_c),
                        cooling_setpoint_c: None,
                        deadband_c: None,
                    },
                    priority: PriorityTier::Grid,
                });
                self.telemetry.set("dr_freeze_guard", 1.0);

                tracing::warn!(
                    actor = %self.name,
                    threshold_c = self.freeze_risk_threshold_c,
                    "DR load-target TurnOff downgraded to minimum-heating setpoint: zone temp below freeze-risk threshold and target is HVAC"
                );
            } else {
                Self::dispatch_for_action(target, action, out);
            }
        }

        self.telemetry
            .set("signals_count", (out.len() - before) as f64);

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            // Invariant: when any TurnOff action targets HVAC equipment and zone
            // temperature is below the freeze-risk threshold, no TurnOff should be
            // dispatched. The downgrade guards above prevent this; this assertion
            // catches a bypassed guard (regression safety net).
            if self.any_zone_below_freeze(env) {
                let hvac_turnoff_dispatched = matches!(&self.hvac_action, DrAction::TurnOff)
                    || self.load_targets.iter().any(|(t, a)| {
                        matches!(a, DrAction::TurnOff)
                            && matches!(t, DispatchTarget::ByEndUse(eu) if eu.is_hvac())
                    });
                if hvac_turnoff_dispatched {
                    let has_turn_off = out[before..].iter().any(|req| {
                        matches!(
                            &req.signal,
                            ControlSignal::ModeOverride {
                                mode: OperatingMode::Off
                            }
                        )
                    });
                    debug_assert!(
                        !has_turn_off,
                        "DR TurnOff dispatched when zone temp < freeze-risk threshold ({:.1}°C)",
                        self.freeze_risk_threshold_c
                    );
                }
            }
        }
    }
}

fn dr_level_as_f64(level: DRLevel) -> f64 {
    match level {
        DRLevel::Normal => 0.0,
        DRLevel::Moderate => 1.0,
        DRLevel::High => 2.0,
        DRLevel::Critical => 3.0,
        DRLevel::GridEmergency => 4.0,
    }
}

#[cfg(test)]
mod tests {
    use hares_types::EndUse;

    use super::*;
    use crate::actor::testing::test_env;

    #[test]
    fn always_comply_returns_true() {
        let env = test_env().build();
        assert!(AlwaysComply.should_comply(DRLevel::Normal, &env));
        assert!(AlwaysComply.should_comply(DRLevel::Critical, &env));
    }

    #[test]
    fn never_comply_returns_false() {
        let env = test_env().build();
        assert!(!NeverComply.should_comply(DRLevel::Normal, &env));
        assert!(!NeverComply.should_comply(DRLevel::Critical, &env));
    }

    #[test]
    fn probabilistic_compliance_rate_zero() {
        let model = Probabilistic::new(0.0);
        let env = test_env().build();
        assert!(!model.should_comply(DRLevel::High, &env));
    }

    #[test]
    fn probabilistic_compliance_rate_one() {
        let model = Probabilistic::new(1.0);
        let env = test_env().build();
        assert!(model.should_comply(DRLevel::High, &env));
    }

    #[test]
    fn probabilistic_deterministic_with_seed() {
        let model = Probabilistic::new(0.5).with_seed(12345);
        let env = test_env().build();

        let result1 = model.should_comply(DRLevel::Moderate, &env);
        let result2 = model.should_comply(DRLevel::Moderate, &env);

        assert_eq!(result1, result2, "same inputs should produce same result");
    }

    #[test]
    fn probabilistic_different_seeds_produce_different_outcomes() {
        let env = test_env().build();
        let mut outcomes = std::collections::HashSet::new();

        // With enough distinct seeds at 50% rate, we must see both true and false
        for seed in 0..50 {
            let model = Probabilistic::new(0.5).with_seed(seed);
            outcomes.insert(model.should_comply(DRLevel::High, &env));
        }

        assert_eq!(
            outcomes.len(),
            2,
            "50 seeds at 50% rate should produce both comply and refuse"
        );
    }

    #[test]
    fn dr_action_curtail_creates_correct_variant() {
        let action = DrAction::curtail(0.3);
        assert_eq!(action, DrAction::LoadCurtailment { fraction: 0.3 });
    }

    #[test]
    fn dr_action_off_creates_correct_variant() {
        let action = DrAction::off();
        assert_eq!(action, DrAction::TurnOff);
    }

    #[test]
    fn dr_action_limit_power_creates_correct_variant() {
        let action = DrAction::limit_power(5.0);
        assert_eq!(action, DrAction::PowerLimit { max_kw: 5.0 });
    }

    #[test]
    fn dr_compliance_name_returns_expected_value() {
        let actor = DrCompliance::new("TestDR");
        assert_eq!(actor.name(), "TestDR");
    }

    #[test]
    fn dr_compliance_no_dr_active_emits_nothing() {
        let mut actor = DrCompliance::new("Test");
        let env = test_env().build();
        let mut requests = Vec::new();

        actor.decide(&env, &mut requests);

        assert!(requests.is_empty());
    }

    #[test]
    fn dr_compliance_always_comply_dispatches_signals() {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(DispatchTarget::ByName("HVAC".into()))
            .with_hvac_action(DrAction::off());

        actor.set_dr_level(DRLevel::Critical);

        let env = test_env().build();
        let mut requests = Vec::new();

        actor.decide(&env, &mut requests);

        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].priority, PriorityTier::Grid);
        assert_eq!(requests[0].target, DispatchTarget::ByName("HVAC".into()));
        assert!(matches!(
            requests[0].signal,
            ControlSignal::ModeOverride {
                mode: OperatingMode::Off
            }
        ));
    }

    #[test]
    fn dr_compliance_never_comply_dispatches_nothing() {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(NeverComply)
            .with_hvac_target(DispatchTarget::ByName("HVAC".into()))
            .with_hvac_action(DrAction::off());

        actor.set_dr_level(DRLevel::Critical);

        let env = test_env().build();
        let mut requests = Vec::new();

        actor.decide(&env, &mut requests);

        assert!(requests.is_empty());
    }

    #[test]
    fn dr_compliance_load_curtailment_dispatches_correct_signal() {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_load_target(
                DispatchTarget::ByEndUse(EndUse::PLUG_LOADS),
                DrAction::curtail(0.5),
            );

        actor.set_dr_level(DRLevel::Moderate);

        let env = test_env().build();
        let mut requests = Vec::new();

        actor.decide(&env, &mut requests);

        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].target,
            DispatchTarget::ByEndUse(EndUse::PLUG_LOADS)
        );
        assert!(matches!(
            requests[0].signal,
            ControlSignal::LoadFraction { fraction } if (fraction - 0.5).abs() < 0.01
        ));
    }

    #[test]
    fn dr_compliance_power_limit_dispatches_correct_signal() {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_load_target(
                DispatchTarget::ByName("EV".into()),
                DrAction::limit_power(3.3),
            );

        actor.set_dr_level(DRLevel::High);

        let env = test_env().build();
        let mut requests = Vec::new();

        actor.decide(&env, &mut requests);

        assert_eq!(requests.len(), 1);
        assert!(matches!(
            requests[0].signal,
            ControlSignal::PowerLimit { max_power_kw, .. } if (max_power_kw - 3.3).abs() < 0.01
        ));
    }

    #[test]
    fn dr_compliance_multiple_targets_dispatches_multiple_signals() {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(DispatchTarget::ByName("HVAC".into()))
            .with_hvac_action(DrAction::setpoint_delta(2.0))
            .with_load_target(
                DispatchTarget::ByName("Lights".into()),
                DrAction::curtail(0.0),
            );

        actor.set_dr_level(DRLevel::Critical);

        let env = test_env().build();
        let mut requests = Vec::new();

        actor.decide(&env, &mut requests);

        assert_eq!(requests.len(), 2);

        let hvac_signal = requests
            .iter()
            .any(|r| matches!(&r.target, DispatchTarget::ByName(n) if &**n == "HVAC"));
        let lights_signal = requests
            .iter()
            .any(|r| matches!(&r.target, DispatchTarget::ByName(n) if &**n == "Lights"));

        assert!(hvac_signal);
        assert!(lights_signal);
    }

    #[test]
    fn dr_compliance_uses_grid_priority() {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(DispatchTarget::ByName("HVAC".into()))
            .with_hvac_action(DrAction::off());

        actor.set_dr_level(DRLevel::Critical);

        let env = test_env().build();
        let mut requests = Vec::new();

        actor.decide(&env, &mut requests);

        assert_eq!(requests[0].priority, PriorityTier::Grid);
    }

    #[test]
    fn dr_compliance_none_action_dispatches_nothing() {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(DispatchTarget::ByName("HVAC".into()))
            .with_hvac_action(DrAction::None);

        actor.set_dr_level(DRLevel::Critical);

        let env = test_env().build();
        let mut requests = Vec::new();

        actor.decide(&env, &mut requests);

        assert!(requests.is_empty());
    }

    #[test]
    fn dr_compliance_current_dr_level_accessor() {
        let mut actor = DrCompliance::new("Test");

        assert_eq!(actor.current_dr_level(), DRLevel::Normal);

        actor.set_dr_level(DRLevel::High);
        assert_eq!(actor.current_dr_level(), DRLevel::High);
    }

    #[test]
    fn dr_compliance_is_dr_active_accessor() {
        let mut actor = DrCompliance::new("Test");

        assert!(!actor.is_dr_active());

        actor.set_dr_level(DRLevel::Moderate);
        assert!(actor.is_dr_active());

        actor.set_dr_level(DRLevel::Normal);
        assert!(!actor.is_dr_active());
    }

    #[test]
    fn dr_compliance_probabilistic_respects_rate() {
        let env = test_env().build();

        let mut comply_count = 0;
        let trials = 100;

        for i in 0..trials {
            let model = Probabilistic::new(0.5).with_seed(i as u64);
            if model.should_comply(DRLevel::Moderate, &env) {
                comply_count += 1;
            }
        }

        let rate = comply_count as f64 / trials as f64;
        assert!(
            rate > 0.3 && rate < 0.7,
            "compliance rate {rate} should be near 0.5 for 50% configured rate"
        );
    }

    #[test]
    fn setpoint_adjust_dispatches_delta_signal() {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(DispatchTarget::ByName("HVAC".into()))
            .with_hvac_action(DrAction::setpoint_delta(2.0));

        actor.set_dr_level(DRLevel::High);

        let env = test_env().build();
        let mut requests = Vec::new();
        actor.decide(&env, &mut requests);

        assert_eq!(requests.len(), 1);
        match &requests[0].signal {
            ControlSignal::ThermalSetpointDelta {
                heating_delta_c,
                cooling_delta_c,
            } => {
                assert_eq!(*heating_delta_c, Some(2.0));
                assert_eq!(*cooling_delta_c, Some(-2.0));
            }
            other => panic!("expected ThermalSetpointDelta, got {other:?}"),
        }
    }

    #[test]
    fn absolute_setpoint_dispatches_thermal_setpoint_signal() {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(DispatchTarget::ByName("HVAC".into()))
            .with_hvac_action(DrAction::absolute_setpoint(18.0, 28.0));

        actor.set_dr_level(DRLevel::High);

        let env = test_env().build();
        let mut requests = Vec::new();
        actor.decide(&env, &mut requests);

        assert_eq!(requests.len(), 1);
        match &requests[0].signal {
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c,
                cooling_setpoint_c,
                deadband_c,
            } => {
                assert_eq!(*heating_setpoint_c, Some(18.0));
                assert_eq!(*cooling_setpoint_c, Some(28.0));
                assert_eq!(*deadband_c, None);
            }
            other => panic!("expected ThermalSetpoint, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Freeze-protection guard
    // -----------------------------------------------------------------------

    #[test]
    fn turn_off_downgraded_to_minimum_heating_when_zone_below_freeze_threshold() {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(DispatchTarget::ByName("HVAC".into()))
            .with_hvac_action(DrAction::off());

        actor.set_dr_level(DRLevel::Critical);

        let env = test_env().zone_temp(3.0).build();
        let mut requests = Vec::new();

        actor.decide(&env, &mut requests);

        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].priority, PriorityTier::Grid);
        assert_eq!(requests[0].target, DispatchTarget::ByName("HVAC".into()));
        match &requests[0].signal {
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c,
                cooling_setpoint_c,
                deadband_c,
            } => {
                assert_eq!(*heating_setpoint_c, Some(DEFAULT_FREEZE_THRESHOLD_C));
                assert_eq!(*cooling_setpoint_c, None);
                assert_eq!(*deadband_c, None);
            }
            other => panic!("expected ThermalSetpoint downgrade, got {other:?}"),
        }
        assert_eq!(actor.telemetry.get("dr_freeze_guard"), Some(1.0));
    }

    #[test]
    fn turn_off_proceeds_normally_when_zone_above_freeze_threshold() {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(DispatchTarget::ByName("HVAC".into()))
            .with_hvac_action(DrAction::off());

        actor.set_dr_level(DRLevel::Critical);

        let env = test_env().zone_temp(22.0).build();
        let mut requests = Vec::new();

        actor.decide(&env, &mut requests);

        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].priority, PriorityTier::Grid);
        assert!(matches!(
            &requests[0].signal,
            ControlSignal::ModeOverride {
                mode: OperatingMode::Off
            }
        ));
        assert_eq!(actor.telemetry.get("dr_freeze_guard"), Some(0.0));
    }

    #[test]
    fn turn_off_proceeds_normally_at_exact_freeze_threshold() {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(DispatchTarget::ByName("HVAC".into()))
            .with_hvac_action(DrAction::off());

        actor.set_dr_level(DRLevel::Critical);

        let env = test_env().zone_temp(DEFAULT_FREEZE_THRESHOLD_C).build();
        let mut requests = Vec::new();

        actor.decide(&env, &mut requests);

        assert_eq!(requests.len(), 1);
        assert!(
            matches!(
                &requests[0].signal,
                ControlSignal::ModeOverride {
                    mode: OperatingMode::Off
                }
            ),
            "TurnOff must proceed at exact threshold (threshold is exclusive)"
        );
        assert_eq!(actor.telemetry.get("dr_freeze_guard"), Some(0.0));
    }

    #[test]
    fn load_target_turn_off_proceeds_for_non_hvac_end_use_even_in_freeze() {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_load_target(
                DispatchTarget::ByEndUse(EndUse::WATER_HEATING),
                DrAction::off(),
            );

        actor.set_dr_level(DRLevel::Critical);

        // Cold zone — guard does NOT apply to load targets
        let env = test_env().zone_temp(-10.0).build();
        let mut requests = Vec::new();

        actor.decide(&env, &mut requests);

        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].target,
            DispatchTarget::ByEndUse(EndUse::WATER_HEATING)
        );
        assert!(
            matches!(
                &requests[0].signal,
                ControlSignal::ModeOverride {
                    mode: OperatingMode::Off
                }
            ),
            "non-HVAC TurnOff must not be downgraded regardless of zone temperature"
        );
        assert_eq!(actor.telemetry.get("dr_freeze_guard"), Some(0.0));
    }

    #[test]
    fn custom_freeze_threshold_is_respected() {
        let custom_threshold = 10.0;
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(DispatchTarget::ByName("HVAC".into()))
            .with_hvac_action(DrAction::off())
            .with_freeze_risk_threshold(custom_threshold);

        actor.set_dr_level(DRLevel::Critical);

        // 7°C is below custom threshold of 10°C — guard fires
        let env_cold = test_env().zone_temp(7.0).build();
        let mut requests = Vec::new();
        actor.decide(&env_cold, &mut requests);

        assert_eq!(requests.len(), 1);
        match &requests[0].signal {
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c, ..
            } => {
                assert_eq!(
                    *heating_setpoint_c,
                    Some(custom_threshold),
                    "downgraded setpoint must match custom threshold"
                );
            }
            other => panic!("expected ThermalSetpoint, got {other:?}"),
        }

        // 12°C is above custom threshold of 10°C — guard does NOT fire
        let mut actor2 = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(DispatchTarget::ByName("HVAC".into()))
            .with_hvac_action(DrAction::off())
            .with_freeze_risk_threshold(custom_threshold);

        actor2.set_dr_level(DRLevel::Critical);
        let env_warm = test_env().zone_temp(12.0).build();
        let mut requests2 = Vec::new();
        actor2.decide(&env_warm, &mut requests2);

        assert_eq!(requests2.len(), 1);
        assert!(
            matches!(
                &requests2[0].signal,
                ControlSignal::ModeOverride {
                    mode: OperatingMode::Off
                }
            ),
            "TurnOff must proceed above custom threshold"
        );
    }

    #[test]
    fn non_turn_off_hvac_action_not_affected_by_freeze_guard() {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(DispatchTarget::ByName("HVAC".into()))
            .with_hvac_action(DrAction::setpoint_delta(2.0));

        actor.set_dr_level(DRLevel::Critical);

        // Cold zone — guard does NOT apply to SetpointAdjust actions
        let env = test_env().zone_temp(-5.0).build();
        let mut requests = Vec::new();

        actor.decide(&env, &mut requests);

        assert_eq!(requests.len(), 1);
        assert!(
            matches!(
                &requests[0].signal,
                ControlSignal::ThermalSetpointDelta { .. }
            ),
            "non-TurnOff HVAC action must not be affected by freeze guard"
        );
        assert_eq!(actor.telemetry.get("dr_freeze_guard"), Some(0.0));
    }

    #[test]
    fn nan_zone_temp_does_not_trigger_freeze_guard() {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(DispatchTarget::ByName("HVAC".into()))
            .with_hvac_action(DrAction::off());

        actor.set_dr_level(DRLevel::Critical);

        let env = test_env().zone_temp(f64::NAN).build();
        let mut requests = Vec::new();

        actor.decide(&env, &mut requests);

        assert_eq!(requests.len(), 1);
        assert!(
            matches!(
                &requests[0].signal,
                ControlSignal::ModeOverride {
                    mode: OperatingMode::Off
                }
            ),
            "NaN zone temp must not trigger freeze guard"
        );
        assert_eq!(actor.telemetry.get("dr_freeze_guard"), Some(0.0));
    }

    #[test]
    fn hvac_and_load_target_both_dispatch_with_freeze_guard() {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(DispatchTarget::ByName("HVAC".into()))
            .with_hvac_action(DrAction::off())
            .with_load_target(
                DispatchTarget::ByEndUse(EndUse::PLUG_LOADS),
                DrAction::curtail(0.5),
            );

        actor.set_dr_level(DRLevel::Critical);

        // Cold zone — HVAC TurnOff downgraded, load target proceeds normally
        let env = test_env().zone_temp(3.0).build();
        let mut requests = Vec::new();

        actor.decide(&env, &mut requests);

        assert_eq!(requests.len(), 2, "HVAC guard + load target = 2 signals");

        let load_signal = requests
            .iter()
            .find(|r| matches!(&r.target, DispatchTarget::ByEndUse(_)));
        let hvac_signal = requests
            .iter()
            .find(|r| matches!(&r.target, DispatchTarget::ByName(n) if &**n == "HVAC"));

        assert!(
            hvac_signal.is_some(),
            "HVAC signal must be present (downgraded)"
        );
        assert!(
            matches!(
                &hvac_signal.unwrap().signal,
                ControlSignal::ThermalSetpoint { .. }
            ),
            "HVAC signal must be downgraded ThermalSetpoint"
        );

        assert!(load_signal.is_some(), "load target signal must be present");
        assert!(
            matches!(
                &load_signal.unwrap().signal,
                ControlSignal::LoadFraction { .. }
            ),
            "load target signal must be unchanged"
        );

        assert_eq!(actor.telemetry.get("dr_freeze_guard"), Some(1.0));
    }

    #[test]
    fn hvac_by_end_use_load_target_turn_off_downgraded_in_freeze() {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_load_target(
                DispatchTarget::ByEndUse(EndUse::HVAC_HEATING),
                DrAction::off(),
            );

        actor.set_dr_level(DRLevel::Critical);

        let env = test_env().zone_temp(2.0).build();
        let mut requests = Vec::new();

        actor.decide(&env, &mut requests);

        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].target,
            DispatchTarget::ByEndUse(EndUse::HVAC_HEATING)
        );
        match &requests[0].signal {
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c, ..
            } => {
                assert_eq!(
                    *heating_setpoint_c,
                    Some(DEFAULT_FREEZE_THRESHOLD_C),
                    "HVAC ByEndUse load target TurnOff must be downgraded to minimum-heating setpoint"
                );
            }
            other => panic!("expected ThermalSetpoint downgrade, got {other:?}"),
        }
        assert_eq!(actor.telemetry.get("dr_freeze_guard"), Some(1.0));
    }

    #[test]
    fn by_name_load_target_turn_off_not_downgraded_in_freeze() {
        // ByName targets cannot be resolved to an end-use at actor dispatch
        // time without the equipment registry — TurnOff proceeds as normal.
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_load_target(
                DispatchTarget::ByName("SomeEquipment".into()),
                DrAction::off(),
            );

        actor.set_dr_level(DRLevel::Critical);

        let env = test_env().zone_temp(2.0).build();
        let mut requests = Vec::new();

        actor.decide(&env, &mut requests);

        assert_eq!(requests.len(), 1);
        assert!(matches!(
            &requests[0].signal,
            ControlSignal::ModeOverride {
                mode: OperatingMode::Off
            }
        ));
        assert_eq!(actor.telemetry.get("dr_freeze_guard"), Some(0.0));
    }
}
