//! IdealThermostat actor -- external setpoint override control.
//!
//! This actor represents *external user override* behavior -- a person walking
//! to the thermostat and changing the setpoint, putting it on hold, or a
//! smart thermostat program pushing a schedule change. It does NOT replace
//! the equipment's internal schedule; it pushes `ThermalSetpoint` overrides
//! via the control surface, exactly like a human would.
//!
//! # Use Cases
//!
//! - **User override**: "hold at 68°F" until manually cleared
//! - **Smart thermostat program**: "pre-cool before DR event"
//! - **Occupant away mode**: "setback to 60°F while away"
//!
//! # Design Principles
//!
//! 1. Equipment is self-contained with internal schedules (thermostat profiles, etc.)
//! 2. Actors only dispatch `ControlSignal`s -- they never mutate equipment directly
//! 3. When no override is active, emit nothing -- equipment uses its internal schedule
//! 4. Override signals use `PriorityTier::UserOverride` (higher than Schedule)

use hares_control::{DispatchRequest, DispatchTarget, PriorityTier};
use hares_types::{ControlSignal, EnvironmentState, Telemetry};

use crate::Actor;

/// Thermostat override state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OverrideState {
    /// Override heating setpoint (°C). `None` means no heating override.
    pub heating_setpoint_c: Option<f64>,
    /// Override cooling setpoint (°C). `None` means no cooling override.
    pub cooling_setpoint_c: Option<f64>,
    /// Override deadband (°C). `None` means no deadband override.
    pub deadband_c: Option<f64>,
}

impl OverrideState {
    /// Returns `true` if any override is active.
    pub fn is_active(&self) -> bool {
        self.heating_setpoint_c.is_some()
            || self.cooling_setpoint_c.is_some()
            || self.deadband_c.is_some()
    }

    /// Creates an empty override state (no overrides active).
    pub fn none() -> Self {
        Self::default()
    }

    /// Creates a heating-only override.
    pub fn heating(setpoint_c: f64) -> Self {
        Self {
            heating_setpoint_c: Some(setpoint_c),
            cooling_setpoint_c: None,
            deadband_c: None,
        }
    }

    /// Creates a cooling-only override.
    pub fn cooling(setpoint_c: f64) -> Self {
        Self {
            heating_setpoint_c: None,
            cooling_setpoint_c: Some(setpoint_c),
            deadband_c: None,
        }
    }

    /// Creates a dual heating/cooling override.
    ///
    /// # Panics (debug only)
    ///
    /// Panics if `heating_c >= cooling_c` -- physically impossible setpoint inversion.
    pub fn dual(heating_c: f64, cooling_c: f64) -> Self {
        debug_assert!(
            heating_c < cooling_c,
            "heating setpoint ({heating_c}°C) must be below cooling setpoint ({cooling_c}°C)"
        );
        Self {
            heating_setpoint_c: Some(heating_c),
            cooling_setpoint_c: Some(cooling_c),
            deadband_c: None,
        }
    }

    /// Adds a deadband override.
    pub fn with_deadband(mut self, deadband_c: f64) -> Self {
        self.deadband_c = Some(deadband_c);
        self
    }

    /// Clears all overrides.
    pub fn clear(&mut self) {
        self.heating_setpoint_c = None;
        self.cooling_setpoint_c = None;
        self.deadband_c = None;
    }
}

/// Actor that pushes external thermostat setpoint overrides to equipment.
///
/// This actor models user-driven thermostat changes (hold mode, away mode,
/// DR pre-conditioning) by dispatching `ThermalSetpoint` signals at
/// `UserOverride` priority.
///
/// # Example
///
/// ```ignore
/// use hares_core::actors::{IdealThermostat, OverrideState};
/// use hares_core::Actor;
///
/// // Create an actor that holds heating at 20°C
/// let mut thermostat = IdealThermostat::new("HVAC")
///     .with_override(OverrideState::heating(20.0));
///
/// // In the dwelling loop, the actor will dispatch the override signal
/// let mut requests = Vec::new();
/// thermostat.decide(&env, &mut requests);
/// assert_eq!(requests.len(), 1);
/// ```
pub struct IdealThermostat {
    /// Actor name derived from the target equipment name.
    name: String,
    /// Current override state.
    override_state: OverrideState,
    /// Pre-allocated dispatch target (avoids per-step Arc construction).
    dispatch_target: DispatchTarget,
    /// Actor telemetry: observable decision state for diagnostics.
    telemetry: Telemetry,
}

impl IdealThermostat {
    /// Creates a new IdealThermostat actor targeting the named equipment.
    pub fn new(target_name: &str) -> Self {
        let mut telemetry = Telemetry::with_capacity(3);
        telemetry.insert("heating_setpoint_c", 0.0);
        telemetry.insert("cooling_setpoint_c", 0.0);
        telemetry.insert("deadband_c", 0.0);
        Self {
            name: format!("IdealThermostat({})", target_name),
            dispatch_target: DispatchTarget::ByName(target_name.into()),
            override_state: OverrideState::default(),
            telemetry,
        }
    }

    pub fn with_name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    /// Sets the override state.
    pub fn with_override(mut self, state: OverrideState) -> Self {
        self.override_state = state;
        self
    }

    /// Sets a heating override (convenience method).
    pub fn with_heating_setpoint(mut self, setpoint_c: f64) -> Self {
        self.override_state.heating_setpoint_c = Some(setpoint_c);
        self
    }

    /// Sets a cooling override (convenience method).
    pub fn with_cooling_setpoint(mut self, setpoint_c: f64) -> Self {
        self.override_state.cooling_setpoint_c = Some(setpoint_c);
        self
    }

    /// Sets dual heating and cooling overrides (convenience method).
    ///
    /// # Panics (debug only)
    ///
    /// Panics if `heating_c >= cooling_c` -- physically impossible setpoint inversion.
    pub fn with_setpoints(mut self, heating_c: f64, cooling_c: f64) -> Self {
        debug_assert!(
            heating_c < cooling_c,
            "heating setpoint ({heating_c}°C) must be below cooling setpoint ({cooling_c}°C)"
        );
        self.override_state.heating_setpoint_c = Some(heating_c);
        self.override_state.cooling_setpoint_c = Some(cooling_c);
        self
    }

    /// Sets a deadband override (convenience method).
    pub fn with_deadband(mut self, deadband_c: f64) -> Self {
        self.override_state.deadband_c = Some(deadband_c);
        self
    }

    /// Returns the current override state.
    pub fn override_state(&self) -> &OverrideState {
        &self.override_state
    }

    /// Updates the override state.
    pub fn set_override(&mut self, state: OverrideState) {
        self.override_state = state;
    }

    /// Clears all overrides.
    pub fn clear_override(&mut self) {
        self.override_state.clear();
    }

    /// Returns `true` if any override is currently active.
    pub fn has_override(&self) -> bool {
        self.override_state.is_active()
    }

    /// Returns the target equipment name.
    pub fn target_name(&self) -> &str {
        match &self.dispatch_target {
            DispatchTarget::ByName(n) => n,
            DispatchTarget::ByEndUse(_) => unreachable!("IdealThermostat always targets by name"),
        }
    }
}

impl Actor for IdealThermostat {
    fn name(&self) -> &str {
        &self.name
    }

    fn telemetry(&self) -> Option<&Telemetry> {
        Some(&self.telemetry)
    }

    fn decide(&mut self, _env: &EnvironmentState, out: &mut Vec<DispatchRequest>) {
        // Populate telemetry regardless of whether override is active,
        // so consumers can see cleared state after clear_override().
        self.telemetry.set(
            "heating_setpoint_c",
            self.override_state.heating_setpoint_c.unwrap_or(f64::NAN),
        );
        self.telemetry.set(
            "cooling_setpoint_c",
            self.override_state.cooling_setpoint_c.unwrap_or(f64::NAN),
        );
        self.telemetry.set(
            "deadband_c",
            self.override_state.deadband_c.unwrap_or(f64::NAN),
        );

        if !self.override_state.is_active() {
            return;
        }

        tracing::debug!(
            actor = %self.name,
            target = self.target_name(),
            heating_c = ?self.override_state.heating_setpoint_c,
            cooling_c = ?self.override_state.cooling_setpoint_c,
            deadband_c = ?self.override_state.deadband_c,
            "dispatching thermal setpoint override",
        );

        out.push(DispatchRequest {
            target: self.dispatch_target.clone(),
            signal: ControlSignal::ThermalSetpoint {
                heating_setpoint_c: self.override_state.heating_setpoint_c,
                cooling_setpoint_c: self.override_state.cooling_setpoint_c,
                deadband_c: self.override_state.deadband_c,
            },
            priority: PriorityTier::UserOverride,
        });
    }
}

#[cfg(test)]
mod tests {
    use hares_types::ControlSignal;

    use super::*;
    use crate::actor::testing::{assert_target_by_name, test_env};

    #[test]
    fn override_state_is_active_when_any_set() {
        let mut state = OverrideState::none();
        assert!(!state.is_active());

        state.heating_setpoint_c = Some(20.0);
        assert!(state.is_active());

        state.clear();
        assert!(!state.is_active());

        state.cooling_setpoint_c = Some(24.0);
        assert!(state.is_active());

        state.clear();
        assert!(!state.is_active());

        state.deadband_c = Some(1.0);
        assert!(state.is_active());
    }

    #[test]
    fn override_state_heating_factory() {
        let state = OverrideState::heating(20.0);
        assert_eq!(state.heating_setpoint_c, Some(20.0));
        assert_eq!(state.cooling_setpoint_c, None);
        assert_eq!(state.deadband_c, None);
        assert!(state.is_active());
    }

    #[test]
    fn override_state_cooling_factory() {
        let state = OverrideState::cooling(24.0);
        assert_eq!(state.heating_setpoint_c, None);
        assert_eq!(state.cooling_setpoint_c, Some(24.0));
        assert_eq!(state.deadband_c, None);
        assert!(state.is_active());
    }

    #[test]
    fn override_state_dual_factory() {
        let state = OverrideState::dual(20.0, 24.0);
        assert_eq!(state.heating_setpoint_c, Some(20.0));
        assert_eq!(state.cooling_setpoint_c, Some(24.0));
        assert_eq!(state.deadband_c, None);
        assert!(state.is_active());
    }

    #[test]
    fn override_state_with_deadband() {
        let state = OverrideState::heating(20.0).with_deadband(1.0);
        assert_eq!(state.heating_setpoint_c, Some(20.0));
        assert_eq!(state.deadband_c, Some(1.0));
    }

    #[test]
    fn ideal_thermostat_name() {
        let thermostat = IdealThermostat::new("HVAC");
        assert_eq!(thermostat.name(), "IdealThermostat(HVAC)");
    }

    #[test]
    fn ideal_thermostat_no_override_emits_nothing() {
        let mut thermostat = IdealThermostat::new("HVAC");
        let env = test_env().build();
        let mut requests = Vec::new();

        thermostat.decide(&env, &mut requests);
        assert!(requests.is_empty());
    }

    #[test]
    fn ideal_thermostat_heating_override_emits_signal() {
        let mut thermostat = IdealThermostat::new("HVAC").with_heating_setpoint(20.0);
        let env = test_env().build();
        let mut requests = Vec::new();

        thermostat.decide(&env, &mut requests);
        assert_eq!(requests.len(), 1);

        let req = &requests[0];
        assert_target_by_name(&requests, "HVAC");
        assert_eq!(req.priority, PriorityTier::UserOverride);

        match &req.signal {
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c,
                cooling_setpoint_c,
                deadband_c,
            } => {
                assert_eq!(*heating_setpoint_c, Some(20.0));
                assert_eq!(*cooling_setpoint_c, None);
                assert_eq!(*deadband_c, None);
            }
            _ => panic!("expected ThermalSetpoint signal"),
        }
    }

    #[test]
    fn ideal_thermostat_cooling_override_emits_signal() {
        let mut thermostat = IdealThermostat::new("HVAC").with_cooling_setpoint(24.0);
        let env = test_env().build();
        let mut requests = Vec::new();

        thermostat.decide(&env, &mut requests);
        assert_eq!(requests.len(), 1);

        match &requests[0].signal {
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c,
                cooling_setpoint_c,
                ..
            } => {
                assert_eq!(*heating_setpoint_c, None);
                assert_eq!(*cooling_setpoint_c, Some(24.0));
            }
            _ => panic!("expected ThermalSetpoint signal"),
        }
    }

    #[test]
    fn ideal_thermostat_dual_override_emits_signal() {
        let mut thermostat = IdealThermostat::new("HVAC").with_setpoints(20.0, 24.0);
        let env = test_env().build();
        let mut requests = Vec::new();

        thermostat.decide(&env, &mut requests);
        assert_eq!(requests.len(), 1);

        match &requests[0].signal {
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c,
                cooling_setpoint_c,
                ..
            } => {
                assert_eq!(*heating_setpoint_c, Some(20.0));
                assert_eq!(*cooling_setpoint_c, Some(24.0));
            }
            _ => panic!("expected ThermalSetpoint signal"),
        }
    }

    #[test]
    fn ideal_thermostat_deadband_override_included() {
        let mut thermostat = IdealThermostat::new("HVAC")
            .with_heating_setpoint(20.0)
            .with_deadband(1.5);
        let env = test_env().build();
        let mut requests = Vec::new();

        thermostat.decide(&env, &mut requests);
        assert_eq!(requests.len(), 1);

        match &requests[0].signal {
            ControlSignal::ThermalSetpoint { deadband_c, .. } => {
                assert_eq!(*deadband_c, Some(1.5));
            }
            _ => panic!("expected ThermalSetpoint signal"),
        }
    }

    #[test]
    fn ideal_thermostat_clear_override_stops_emission() {
        let mut thermostat = IdealThermostat::new("HVAC").with_heating_setpoint(20.0);
        let env = test_env().build();

        let mut requests = Vec::new();
        thermostat.decide(&env, &mut requests);
        assert_eq!(requests.len(), 1);

        thermostat.clear_override();
        requests.clear();
        thermostat.decide(&env, &mut requests);
        assert!(requests.is_empty());
    }

    #[test]
    fn ideal_thermostat_set_override_updates_state() {
        let mut thermostat = IdealThermostat::new("HVAC");
        assert!(!thermostat.has_override());

        thermostat.set_override(OverrideState::dual(18.0, 26.0));
        assert!(thermostat.has_override());

        let state = thermostat.override_state();
        assert_eq!(state.heating_setpoint_c, Some(18.0));
        assert_eq!(state.cooling_setpoint_c, Some(26.0));
    }

    #[test]
    fn ideal_thermostat_uses_user_override_priority() {
        let mut thermostat = IdealThermostat::new("HVAC").with_heating_setpoint(20.0);
        let env = test_env().build();
        let mut requests = Vec::new();

        thermostat.decide(&env, &mut requests);
        assert_eq!(requests[0].priority, PriorityTier::UserOverride);
    }

    #[test]
    fn ideal_thermostat_target_name_accessible() {
        let thermostat = IdealThermostat::new("MainHVAC");
        assert_eq!(thermostat.target_name(), "MainHVAC");
    }

    #[test]
    fn ideal_thermostat_with_override_state() {
        let state = OverrideState::dual(19.0, 25.0).with_deadband(2.0);
        let mut thermostat = IdealThermostat::new("HVAC").with_override(state);
        let env = test_env().build();
        let mut requests = Vec::new();

        thermostat.decide(&env, &mut requests);
        assert_eq!(requests.len(), 1);

        match &requests[0].signal {
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c,
                cooling_setpoint_c,
                deadband_c,
            } => {
                assert_eq!(*heating_setpoint_c, Some(19.0));
                assert_eq!(*cooling_setpoint_c, Some(25.0));
                assert_eq!(*deadband_c, Some(2.0));
            }
            _ => panic!("expected ThermalSetpoint signal"),
        }
    }
}
