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
use hares_equipment::hvac::ThermalSetpoints;
use hares_types::{ControlSignal, EnvironmentState, HaresError, Telemetry};
use serde::{Deserialize, Serialize};

use crate::Actor;

/// Thermostat override state.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
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
    /// # Panics
    ///
    /// Panics if `heating_c >= cooling_c` — physically impossible setpoint inversion.
    pub fn dual(heating_c: f64, cooling_c: f64) -> Self {
        assert!(
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
    /// Equipment hysteresis/deadband half-width (°C), used by `decide()` to
    /// validate setpoint ordering via `ThermalSetpoints::validate_for_deadband`.
    /// Defaults to 1.0 °C (matches `ThermostatConfig::default()`).
    hysteresis_c: f64,
    /// Count of setpoint overrides rejected because heating >= cooling.
    /// Gated on `observe` feature for diagnostic CSV output.
    #[cfg(feature = "observe")]
    #[allow(dead_code)]
    setpoint_inversion_rejected_count: u64,
}

impl IdealThermostat {
    /// Creates a new IdealThermostat actor targeting the named equipment.
    pub fn new(target_name: &str) -> Self {
        let mut telemetry = Telemetry::with_capacity(4);
        // Why: telemetry map stores f64 only (not Option<f64>). Sentinels use 0.0
        // for "unset/no-override" because OverrideState tracks actual setpoint
        // state with Option<f64> — the telemetry values are an output display
        // layer. Consumers that need to distinguish "0.0°C override" from
        // "no override" should read `override_state()` directly.
        telemetry.insert("heating_setpoint_c", 0.0);
        telemetry.insert("cooling_setpoint_c", 0.0);
        telemetry.insert("deadband_c", 0.0);
        telemetry.insert("setpoint_inversion_rejected", 0.0);
        Self {
            name: format!("IdealThermostat({})", target_name),
            dispatch_target: DispatchTarget::ByName(target_name.into()),
            override_state: OverrideState::default(),
            telemetry,
            hysteresis_c: 1.0,
            #[cfg(feature = "observe")]
            setpoint_inversion_rejected_count: 0,
        }
    }

    /// Overrides the actor display name (used in log output and telemetry).
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
    /// # Panics
    ///
    /// Panics if `heating_c >= cooling_c` — physically impossible setpoint inversion.
    pub fn with_setpoints(mut self, heating_c: f64, cooling_c: f64) -> Self {
        assert!(
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

    /// Sets the equipment hysteresis/deadband half-width (°C) used for
    /// setpoint deadband validation during `decide()`.
    ///
    /// Defaults to 1.0 °C (matching `ThermostatConfig::default`).
    pub fn with_hysteresis(mut self, hysteresis_c: f64) -> Self {
        self.hysteresis_c = hysteresis_c;
        self
    }

    /// Returns the current override state.
    pub fn override_state(&self) -> &OverrideState {
        &self.override_state
    }

    /// Updates the override state, rejecting strictly inverted setpoints.
    ///
    /// When both `heating_setpoint_c` and `cooling_setpoint_c` are set and
    /// `heating >= cooling`, the override is rejected:
    /// - In debug/check_invariants builds: panics with a diagnostic message.
    /// - In release builds: no-ops, logs a warning, and increments the
    ///   observable rejection counter.
    ///
    /// NOTE: This method only catches strict inversion (heating >= cooling).
    /// Deadband violations (cooling - heating < 2 * hysteresis_c) are not
    /// checked here — they are caught later in `decide()` which is the
    /// authoritative gate.
    pub fn set_override(&mut self, state: OverrideState) {
        if let (Some(heating_c), Some(cooling_c)) =
            (state.heating_setpoint_c, state.cooling_setpoint_c)
        {
            if heating_c >= cooling_c {
                #[cfg(any(debug_assertions, feature = "check_invariants"))]
                {
                    panic!(
                        "setpoint inversion in set_override() for IdealThermostat '{}': \
                         heating={heating_c}°C >= cooling={cooling_c}°C",
                        self.name,
                    );
                }
                #[cfg(not(any(debug_assertions, feature = "check_invariants")))]
                {
                    #[cfg(feature = "observe")]
                    {
                        self.setpoint_inversion_rejected_count =
                            self.setpoint_inversion_rejected_count.saturating_add(1);
                    }
                    tracing::warn!(
                        actor = %self.name,
                        heating_c = heating_c,
                        cooling_c = cooling_c,
                        "set_override: rejected inverted setpoints (heating >= cooling)",
                    );
                    return;
                }
            }
        }
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
        // Why: unwrap_or(0.0) for unset setpoints — the actual override
        // state is tracked by Option<f64> fields on OverrideState. Telemetry
        // is a display layer; consumers needing to distinguish "0.0°C
        // override" from "no override" should read override_state() directly.
        self.telemetry.set(
            "heating_setpoint_c",
            self.override_state.heating_setpoint_c.unwrap_or(0.0),
        );
        self.telemetry.set(
            "cooling_setpoint_c",
            self.override_state.cooling_setpoint_c.unwrap_or(0.0),
        );
        self.telemetry
            .set("deadband_c", self.override_state.deadband_c.unwrap_or(0.0));
        // Reset inversion flag each step; set to 1.0 only when rejected below.
        self.telemetry.set("setpoint_inversion_rejected", 0.0);

        if !self.override_state.is_active() {
            return;
        }

        // Validate setpoint ordering when both heating and cooling are set.
        // Uses the deadband-aware invariant: cooling_c - heating_c >= 2 * hysteresis_c.
        // This catches setpoint inversion (heating >= cooling) as well as setpoints
        // that are too close together to maintain a meaningful deadband.
        if let (Some(heat), Some(cool)) = (
            self.override_state.heating_setpoint_c,
            self.override_state.cooling_setpoint_c,
        ) {
            let setpoints = ThermalSetpoints {
                heating_c: heat,
                cooling_c: cool,
            };
            if setpoints.validate_for_deadband(self.hysteresis_c).is_err() {
                // In debug/check_invariants builds, panic with unambiguous
                // diagnostic. In release builds, reject the emission with a
                // warning and an observable counter/telemetry flag so the error
                // is auditable without crashing the fleet simulation.
                #[cfg(any(debug_assertions, feature = "check_invariants"))]
                {
                    panic!(
                        "setpoint deadband violation in IdealThermostat '{}': \
                         heating={heat}°C, cooling={cool}°C, gap={gap:.2}°C < required {required:.2}°C",
                        self.name,
                        gap = cool - heat,
                        required = 2.0 * self.hysteresis_c,
                    );
                }
                #[cfg(not(any(debug_assertions, feature = "check_invariants")))]
                {
                    #[cfg(feature = "observe")]
                    {
                        self.setpoint_inversion_rejected_count =
                            self.setpoint_inversion_rejected_count.saturating_add(1);
                    }
                    self.telemetry.set("setpoint_inversion_rejected", 1.0);
                    tracing::warn!(
                        actor = %self.name,
                        target = self.target_name(),
                        heating_c = heat,
                        cooling_c = cool,
                        hysteresis_c = self.hysteresis_c,
                        gap_c = cool - heat,
                        required_gap_c = 2.0 * self.hysteresis_c,
                        "rejected thermal setpoint override: violates deadband invariant",
                    );
                    return;
                }
            }
        }

        tracing::debug!(
            actor = %self.name,
            target = self.target_name(),
            heating_c = ?self.override_state.heating_setpoint_c,
            cooling_c = ?self.override_state.cooling_setpoint_c,
            deadband_c = ?self.override_state.deadband_c,
            "dispatching thermal setpoint override",
        );

        // `UserOverride` tier — matches the central
        // `From<&ControlSignal> for PriorityTier` mapping for
        // `ThermalSetpoint`. The thermostat override is a deliberate
        // user action (hold mode, away setback, DR pre-conditioning)
        // and must take precedence over schedule-level setpoints.
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

    fn save_state(&self) -> Result<Vec<u8>, HaresError> {
        postcard::to_allocvec(&self.override_state)
            .map_err(|e| HaresError::Io(format!("IdealThermostat save_state: {e}")))
    }

    fn load_state(&mut self, data: &[u8]) -> Result<(), HaresError> {
        if data.is_empty() {
            return Ok(());
        }
        self.override_state = postcard::from_bytes(data)
            .map_err(|e| HaresError::Io(format!("IdealThermostat load_state: {e}")))?;
        Ok(())
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

    #[test]
    fn ideal_thermostat_inverted_setpoints_block_emission() {
        let mut thermostat = IdealThermostat::new("HVAC");
        // In debug/check_invariants builds, set_override() panics on
        // inverted setpoints. In release builds it no-ops with a warning.
        // Either path must prevent the invalid state from being stored
        // and subsequently emitted.
        let set_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            thermostat.set_override(OverrideState {
                heating_setpoint_c: Some(30.0),
                cooling_setpoint_c: Some(25.0),
                deadband_c: None,
            });
        }));

        // The override must not persist — state is either never set
        // (release no-op) or set_override panicked before assignment
        // (debug/check_invariants).
        assert!(!thermostat.has_override());

        let env = test_env().build();
        let mut requests = Vec::new();
        thermostat.decide(&env, &mut requests);
        assert!(requests.is_empty());

        if set_result.is_ok() {
            // Release: set_override no-opped. decide() returned early (no active override),
            // so the inversion counter was reset to 0.0 and never incremented.
            let telemetry = thermostat.telemetry().unwrap();
            assert_eq!(telemetry.get("setpoint_inversion_rejected"), Some(0.0));
        }
    }

    #[test]
    #[should_panic(expected = "heating setpoint (30°C) must be below cooling setpoint (25°C)")]
    fn dual_inverted_setpoints_panics() {
        let _ = OverrideState::dual(30.0, 25.0);
    }

    #[test]
    #[should_panic(expected = "heating setpoint (30°C) must be below cooling setpoint (25°C)")]
    fn with_setpoints_inverted_panics() {
        let _ = IdealThermostat::new("HVAC").with_setpoints(30.0, 25.0);
    }

    #[test]
    fn set_override_rejects_inverted_state_and_retains_prior() {
        let mut thermostat = IdealThermostat::new("HVAC");

        // Set a valid override first.
        thermostat.set_override(OverrideState::dual(20.0, 26.0));
        assert!(thermostat.has_override());
        assert_eq!(thermostat.override_state().heating_setpoint_c, Some(20.0));
        assert_eq!(thermostat.override_state().cooling_setpoint_c, Some(26.0));

        // Try to set an inverted override. In debug/check_invariants builds
        // this panics in set_override(). In release builds it no-ops.
        let _set_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            thermostat.set_override(OverrideState {
                heating_setpoint_c: Some(30.0),
                cooling_setpoint_c: Some(25.0),
                deadband_c: None,
            });
        }));

        // The prior valid state must be retained.
        assert!(thermostat.has_override());
        assert_eq!(thermostat.override_state().heating_setpoint_c, Some(20.0));
        assert_eq!(thermostat.override_state().cooling_setpoint_c, Some(26.0));
    }

    #[test]
    fn decide_rejects_deadband_violation_not_just_inversion() {
        let mut thermostat = IdealThermostat::new("HVAC");
        // Default hysteresis_c=1.0 requires a gap >= 2.0°C.
        // These setpoints (20.0, 21.0) are not inverted but violate the
        // deadband constraint. set_override() accepts them (heating < cooling
        // passes the basic check), but decide() must reject emission.
        let set_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            thermostat.set_override(OverrideState {
                heating_setpoint_c: Some(20.0),
                cooling_setpoint_c: Some(21.0),
                deadband_c: None,
            });
        }));

        if set_result.is_ok() {
            // Release path: state was stored but decide() rejects emission.
            assert!(thermostat.has_override());
            let env = test_env().build();
            let mut requests = Vec::new();
            // In debug/check_invariants builds decide() panics; in release
            // it rejects the emission gracefully. Either path must block
            // signal emission.
            let decide_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                thermostat.decide(&env, &mut requests);
            }));
            assert!(requests.is_empty());
            if decide_result.is_ok() {
                let telemetry = thermostat.telemetry().unwrap();
                assert_eq!(telemetry.get("setpoint_inversion_rejected"), Some(1.0));
            }
        }
    }

    #[test]
    fn custom_hysteresis_relaxes_deadband_check() {
        let mut thermostat = IdealThermostat::new("HVAC").with_hysteresis(0.5);
        // With hysteresis_c=0.5, required gap is 1.0°C.
        // 20.0 and 21.0: gap=1.0 >= 2*0.5=1.0 — should pass.
        thermostat.set_override(OverrideState::dual(20.0, 21.0));
        assert!(thermostat.has_override());

        let env = test_env().build();
        let mut requests = Vec::new();
        thermostat.decide(&env, &mut requests);
        assert_eq!(requests.len(), 1);
    }

    #[test]
    fn save_state_load_state_round_trip_override_preserved() {
        let mut thermostat = IdealThermostat::new("HVAC");
        thermostat.set_override(OverrideState::heating(20.0));

        let blob = thermostat.save_state().expect("save_state should succeed");
        assert!(
            !blob.is_empty(),
            "stateful actor must produce non-empty blob"
        );

        let mut restored = IdealThermostat::new("HVAC");
        assert!(
            !restored.has_override(),
            "default thermostat should not have override"
        );

        restored
            .load_state(&blob)
            .expect("load_state should succeed");
        assert!(
            restored.has_override(),
            "override must be preserved after load_state"
        );

        let state = restored.override_state();
        assert_eq!(state.heating_setpoint_c, Some(20.0));
    }
}
