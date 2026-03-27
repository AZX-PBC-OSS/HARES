//! Occupant actor — behavioral control over equipment based on presence.
//!
//! The Occupant actor models *external human behavior* that acts on equipment from outside:
//! - Turning lights off when leaving the room (override equipment's default schedule)
//! - Starting a washing machine cycle (trigger an event on the appliance)
//! - Plugging/unplugging an EV (connect/disconnect)
//! - Changing EV charge mode (eco/fast)
//!
//! Equipment is self-contained with internal schedules. The occupant only pushes
//! control signals — never mutates environment or equipment state directly.
//!
//! # Example
//!
//! ```ignore
//! use hares_core::actors::Occupant;
//! use hares_core::Actor;
//!
//! // Create an occupant with home/away schedule
//! let mut occupant = Occupant::new("Resident1")
//!     .with_lighting_target("Indoor Lighting")
//!     .with_ev_target("EV")
//!     .with_presence_schedule(presence_array);
//!
//! // In the dwelling loop, the actor will dispatch signals based on presence
//! let mut requests = Vec::new();
//! occupant.decide(&env, &mut requests);
//! ```

use std::sync::Arc;

use hares_control::{DispatchRequest, DispatchTarget, PriorityTier};
use hares_types::{ControlSignal, EndUse, EnvironmentState, EvConnectionState, OperatingMode};

use crate::Actor;

/// Occupant presence state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Presence {
    /// Occupant is home and active.
    #[default]
    Home,
    /// Occupant is away from home.
    Away,
    /// Occupant is home but sleeping.
    Sleeping,
}

impl Presence {
    /// Returns true if occupant is present (home or sleeping).
    pub fn is_present(&self) -> bool {
        matches!(self, Presence::Home | Presence::Sleeping)
    }

    /// Returns true if occupant is away.
    pub fn is_away(&self) -> bool {
        matches!(self, Presence::Away)
    }
}

/// Configuration for equipment control behavior.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EquipmentBehavior {
    /// Turn off equipment when away.
    pub off_when_away: bool,
    /// Turn on equipment when returning home.
    pub on_when_home: bool,
    /// Override power setpoint when equipment is active.
    pub power_setpoint_kw: Option<f64>,
    /// Apply load fraction when equipment is active.
    pub load_fraction: Option<f64>,
}

impl EquipmentBehavior {
    /// Creates default behavior with no overrides.
    pub fn none() -> Self {
        Self::default()
    }

    /// Turn off equipment when away.
    pub fn off_when_away(mut self) -> Self {
        self.off_when_away = true;
        self
    }

    /// Turn on equipment when home.
    pub fn on_when_home(mut self) -> Self {
        self.on_when_home = true;
        self
    }

    /// Set power setpoint for equipment.
    ///
    /// # Panics (debug only)
    ///
    /// Panics if `kw` is negative.
    pub fn with_power_setpoint(mut self, kw: f64) -> Self {
        debug_assert!(kw >= 0.0, "power setpoint must be non-negative, got {kw}");
        self.power_setpoint_kw = Some(kw);
        self
    }

    /// Set load fraction for equipment.
    ///
    /// # Panics (debug only)
    ///
    /// Panics if `fraction` is outside `[0.0, 1.0]`.
    pub fn with_load_fraction(mut self, fraction: f64) -> Self {
        debug_assert!(
            (0.0..=1.0).contains(&fraction),
            "load fraction must be in [0.0, 1.0], got {fraction}"
        );
        self.load_fraction = Some(fraction);
        self
    }
}

/// Actor that models occupant behavior and dispatches control signals to equipment.
///
/// The occupant acts on equipment based on their presence state (home/away/sleeping)
/// and configured behavioral preferences. All control flows through the dispatch
/// pipeline — the occupant never directly mutates equipment state.
pub struct Occupant {
    /// Actor name for diagnostics.
    name: Arc<str>,
    /// Pre-computed presence schedule (one entry per timestep).
    presence_schedule: Vec<Presence>,
    /// Current timestep index into presence_schedule.
    current_step: usize,
    /// Previous presence state for transition detection.
    previous_presence: Presence,
    /// Pre-cached dispatch target + behavior for lighting.
    lighting: Option<(DispatchTarget, EquipmentBehavior)>,
    /// Pre-cached dispatch target + behavior for appliances.
    appliance: Option<(DispatchTarget, EquipmentBehavior)>,
    /// Pre-cached dispatch target + behavior for EV.
    ev: Option<(DispatchTarget, EquipmentBehavior)>,
    /// Pre-cached dispatch target + behavior for plug loads.
    plug_loads: Option<(DispatchTarget, EquipmentBehavior)>,
}

impl Occupant {
    /// Creates a new Occupant actor with the given name.
    pub fn new(name: &str) -> Self {
        Self {
            name: Arc::from(name),
            presence_schedule: vec![Presence::Home],
            current_step: 0,
            previous_presence: Presence::Home,
            lighting: None,
            appliance: None,
            ev: None,
            plug_loads: None,
        }
    }

    /// Sets the presence schedule for the occupant.
    ///
    /// The schedule is a pre-computed array of presence states, one per timestep.
    /// This avoids per-step computation and enables deterministic behavior.
    pub fn with_presence_schedule(mut self, schedule: Vec<Presence>) -> Self {
        self.presence_schedule = schedule;
        self
    }

    /// Configures lighting equipment target and behavior.
    pub fn with_lighting(mut self, target_name: &str, behavior: EquipmentBehavior) -> Self {
        self.lighting = Some((DispatchTarget::ByName(target_name.into()), behavior));
        self
    }

    /// Configures appliance equipment target and behavior.
    pub fn with_appliance(mut self, target_name: &str, behavior: EquipmentBehavior) -> Self {
        self.appliance = Some((DispatchTarget::ByName(target_name.into()), behavior));
        self
    }

    /// Configures EV equipment target and behavior.
    pub fn with_ev(mut self, target_name: &str, behavior: EquipmentBehavior) -> Self {
        self.ev = Some((DispatchTarget::ByName(target_name.into()), behavior));
        self
    }

    /// Configures plug loads by end-use category.
    pub fn with_plug_loads(mut self, end_use: EndUse, behavior: EquipmentBehavior) -> Self {
        self.plug_loads = Some((DispatchTarget::ByEndUse(end_use), behavior));
        self
    }

    /// Returns the current presence state.
    ///
    /// # Panics (debug only)
    ///
    /// Panics if `current_step` exceeds the schedule length — the schedule
    /// must cover the full simulation duration.
    pub fn current_presence(&self) -> Presence {
        debug_assert!(
            self.current_step < self.presence_schedule.len(),
            "presence schedule exhausted at step {} (schedule length {})",
            self.current_step,
            self.presence_schedule.len(),
        );
        self.presence_schedule
            .get(self.current_step)
            .copied()
            .unwrap_or(Presence::Home)
    }

    /// Returns true if the occupant is currently present.
    pub fn is_present(&self) -> bool {
        self.current_presence().is_present()
    }

    /// Advances the timestep counter.
    ///
    /// Called automatically by `decide()` — no need to call manually.
    fn advance_step(&mut self) {
        self.previous_presence = self.current_presence();
        self.current_step = self.current_step.saturating_add(1);
    }

    /// Generates control signals for equipment based on behavior and presence.
    ///
    /// Mode signals (`off_when_away`, `on_when_home`) are event-driven — they fire
    /// on transitions or while the triggering condition holds (away → Off each step).
    /// Continuous controls (`power_setpoint_kw`, `load_fraction`) are re-sent every
    /// step while present, keeping the equipment override active for the duration.
    fn dispatch_for_behavior(
        target: &DispatchTarget,
        behavior: &EquipmentBehavior,
        presence: Presence,
        is_transition: bool,
        out: &mut Vec<DispatchRequest>,
    ) {
        let has_continuous_control =
            behavior.load_fraction.is_some() || behavior.power_setpoint_kw.is_some();
        let has_mode_control = behavior.off_when_away || behavior.on_when_home;
        if !is_transition && !has_mode_control && !has_continuous_control {
            return;
        }

        // Handle away mode: turn off equipment
        if presence.is_away() && behavior.off_when_away {
            out.push(DispatchRequest {
                target: target.clone(),
                signal: ControlSignal::ModeOverride {
                    mode: OperatingMode::Off,
                },
                priority: PriorityTier::UserOverride,
            });
            return;
        }

        // Handle returning home: turn on equipment if configured
        if presence.is_present() && is_transition && behavior.on_when_home {
            out.push(DispatchRequest {
                target: target.clone(),
                signal: ControlSignal::ModeOverride {
                    mode: OperatingMode::Standby,
                },
                priority: PriorityTier::UserOverride,
            });
        }

        if presence.is_present() {
            push_power_setpoint_if(target, behavior, out);
            push_load_fraction_if(target, behavior, out);
        }
    }

    /// Generates EV-specific control signals.
    ///
    /// EV mode changes (plug/unplug) are transition-only — dispatched once when
    /// presence changes, not every step. Non-EV equipment re-sends `Off` each step
    /// while away (idempotent) because the equipment has no "plugged in" concept.
    fn dispatch_for_ev(
        target: &DispatchTarget,
        behavior: &EquipmentBehavior,
        presence: Presence,
        is_transition: bool,
        out: &mut Vec<DispatchRequest>,
    ) {
        if is_transition {
            if presence.is_present() && behavior.on_when_home {
                out.push(DispatchRequest {
                    target: target.clone(),
                    signal: ControlSignal::EvPlugIn {
                        state: EvConnectionState::HomePluggedIn,
                    },
                    priority: PriorityTier::UserOverride,
                });
            } else if presence.is_away() && behavior.off_when_away {
                out.push(DispatchRequest {
                    target: target.clone(),
                    signal: ControlSignal::EvPlugIn {
                        state: EvConnectionState::Disconnected,
                    },
                    priority: PriorityTier::UserOverride,
                });
            }
        }

        // EV power limit (charge rate control) — only when plugged in
        if presence.is_present() {
            push_power_setpoint_if(target, behavior, out);
        }
    }
}

/// Pushes a `PowerSetpoint` if the behavior has one configured.
fn push_power_setpoint_if(
    target: &DispatchTarget,
    behavior: &EquipmentBehavior,
    out: &mut Vec<DispatchRequest>,
) {
    if let Some(power_kw) = behavior.power_setpoint_kw {
        out.push(DispatchRequest {
            target: target.clone(),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: power_kw,
                reactive_power_kvar: None,
            },
            priority: PriorityTier::UserOverride,
        });
    }
}

/// Pushes a `LoadFraction` if the behavior has one configured.
fn push_load_fraction_if(
    target: &DispatchTarget,
    behavior: &EquipmentBehavior,
    out: &mut Vec<DispatchRequest>,
) {
    if let Some(fraction) = behavior.load_fraction {
        out.push(DispatchRequest {
            target: target.clone(),
            signal: ControlSignal::LoadFraction { fraction },
            priority: PriorityTier::UserOverride,
        });
    }
}

impl Actor for Occupant {
    fn name(&self) -> &str {
        &self.name
    }

    fn decide(&mut self, _env: &EnvironmentState, out: &mut Vec<DispatchRequest>) {
        let current_presence = self.current_presence();
        let is_transition = current_presence != self.previous_presence;

        let before = out.len();

        if let Some((target, behavior)) = &self.lighting {
            Self::dispatch_for_behavior(target, behavior, current_presence, is_transition, out);
        }

        if let Some((target, behavior)) = &self.appliance {
            Self::dispatch_for_behavior(target, behavior, current_presence, is_transition, out);
        }

        if let Some((target, behavior)) = &self.ev {
            Self::dispatch_for_ev(target, behavior, current_presence, is_transition, out);
        }

        if let Some((target, behavior)) = &self.plug_loads {
            Self::dispatch_for_behavior(target, behavior, current_presence, is_transition, out);
        }

        let dispatched = out.len() - before;
        if dispatched > 0 || is_transition {
            tracing::debug!(
                actor = %self.name,
                presence = ?current_presence,
                transition = is_transition,
                signals = dispatched,
                "occupant decision",
            );
        }

        self.advance_step();
    }
}

#[cfg(test)]
mod tests {
    use hares_control::{DispatchTarget, PriorityTier};
    use hares_types::{ControlSignal, EndUse, EvConnectionState, OperatingMode};

    use super::{EquipmentBehavior, Occupant, Presence};
    use crate::Actor;
    use crate::actor::testing::test_env;

    #[test]
    fn presence_default_is_home() {
        assert_eq!(Presence::default(), Presence::Home);
        assert!(Presence::Home.is_present());
        assert!(!Presence::Away.is_present());
        assert!(Presence::Sleeping.is_present());
    }

    #[test]
    fn presence_is_away() {
        assert!(Presence::Away.is_away());
        assert!(!Presence::Home.is_away());
        assert!(!Presence::Sleeping.is_away());
    }

    #[test]
    fn equipment_behavior_default_is_none() {
        let behavior = EquipmentBehavior::none();
        assert!(!behavior.off_when_away);
        assert!(!behavior.on_when_home);
        assert_eq!(behavior.power_setpoint_kw, None);
        assert_eq!(behavior.load_fraction, None);
    }

    #[test]
    fn equipment_behavior_builder_methods() {
        let behavior = EquipmentBehavior::none()
            .off_when_away()
            .on_when_home()
            .with_power_setpoint(5.0)
            .with_load_fraction(0.8);

        assert!(behavior.off_when_away);
        assert!(behavior.on_when_home);
        assert_eq!(behavior.power_setpoint_kw, Some(5.0));
        assert_eq!(behavior.load_fraction, Some(0.8));
    }

    #[test]
    fn occupant_name_returns_expected_value() {
        let occupant = Occupant::new("TestOccupant");
        assert_eq!(occupant.name(), "TestOccupant");
    }

    #[test]
    fn occupant_default_presence_is_home() {
        let occupant = Occupant::new("Test");
        assert_eq!(occupant.current_presence(), Presence::Home);
        assert!(occupant.is_present());
    }

    #[test]
    fn occupant_presence_schedule_works() {
        let schedule = vec![Presence::Home, Presence::Away, Presence::Home];
        let mut occupant = Occupant::new("Test").with_presence_schedule(schedule);
        let env = test_env().build();
        let mut requests = Vec::new();

        // Step 0: Home — after decide, step advances to 1
        assert_eq!(occupant.current_presence(), Presence::Home);
        occupant.decide(&env, &mut requests);
        assert_eq!(occupant.current_presence(), Presence::Away);

        // Step 1: Away — after decide, step advances to 2
        requests.clear();
        occupant.decide(&env, &mut requests);
        assert_eq!(occupant.current_presence(), Presence::Home);

        // Step 2: Home — final step
        requests.clear();
        occupant.decide(&env, &mut requests);
    }

    #[test]
    fn occupant_no_targets_emits_nothing() {
        let mut occupant = Occupant::new("Test");
        let env = test_env().build();
        let mut requests = Vec::new();

        occupant.decide(&env, &mut requests);
        assert!(requests.is_empty());
    }

    #[test]
    fn occupant_lighting_off_when_away_emits_signal() {
        let schedule = vec![Presence::Away];
        let mut occupant = Occupant::new("Test")
            .with_presence_schedule(schedule)
            .with_lighting("Indoor Lights", EquipmentBehavior::none().off_when_away());

        let env = test_env().build();
        let mut requests = Vec::new();

        occupant.decide(&env, &mut requests);

        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].priority, PriorityTier::UserOverride);
        assert_eq!(
            requests[0].target,
            DispatchTarget::ByName("Indoor Lights".into())
        );
        assert!(
            matches!(
                requests[0].signal,
                ControlSignal::ModeOverride {
                    mode: OperatingMode::Off
                }
            ),
            "expected ModeOverride Off"
        );
    }

    #[test]
    fn occupant_lighting_no_signal_when_home() {
        let schedule = vec![Presence::Home];
        let mut occupant = Occupant::new("Test")
            .with_presence_schedule(schedule)
            .with_lighting("Indoor Lights", EquipmentBehavior::none().off_when_away());

        let env = test_env().build();
        let mut requests = Vec::new();

        occupant.decide(&env, &mut requests);

        // When home with off_when_away only, no signal emitted
        assert!(requests.is_empty());
    }

    #[test]
    fn occupant_appliance_on_when_home_emits_signal() {
        let schedule = vec![Presence::Away, Presence::Home];
        let mut occupant = Occupant::new("Test")
            .with_presence_schedule(schedule.clone())
            .with_appliance("Washer", EquipmentBehavior::none().on_when_home());

        let env = test_env().build();
        let mut requests = Vec::new();

        // Step 0: Away (no signal, no transition)
        occupant.decide(&env, &mut requests);
        assert!(requests.is_empty());

        // Step 1: Home (transition from Away, signal emitted)
        requests.clear();
        occupant.decide(&env, &mut requests);

        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].priority, PriorityTier::UserOverride);
        assert_eq!(requests[0].target, DispatchTarget::ByName("Washer".into()));
        assert!(
            matches!(
                requests[0].signal,
                ControlSignal::ModeOverride {
                    mode: OperatingMode::Standby
                }
            ),
            "expected ModeOverride Standby"
        );
    }

    #[test]
    fn occupant_ev_plug_unplug_on_transition() {
        let schedule = vec![Presence::Away, Presence::Home];
        let mut occupant = Occupant::new("Test")
            .with_presence_schedule(schedule.clone())
            .with_ev(
                "MyEV",
                EquipmentBehavior::none().on_when_home().off_when_away(),
            );

        let env = test_env().build();
        let mut requests = Vec::new();

        // Step 0: Away at start (no transition, but off_when_away applies)
        occupant.decide(&env, &mut requests);
        // First step is always transition from default(Home) to first schedule value
        // Actually: default is Home, first step goes to Away -> that's a transition
        assert!(!requests.is_empty());
        let has_unplug = requests.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::EvPlugIn {
                    state: EvConnectionState::Disconnected,
                }
            )
        });
        assert!(
            has_unplug,
            "expected EvPlugIn(Disconnected) signal when transitioning to Away"
        );

        // Step 1: Home (plug in)
        requests.clear();
        occupant.decide(&env, &mut requests);

        assert!(!requests.is_empty());
        let has_plug = requests.iter().any(|r| {
            matches!(
                r.signal,
                ControlSignal::EvPlugIn {
                    state: EvConnectionState::HomePluggedIn,
                }
            )
        });
        assert!(
            has_plug,
            "expected EvPlugIn(HomePluggedIn) signal when transitioning to Home"
        );
    }

    #[test]
    fn occupant_ev_power_setpoint_when_home() {
        let schedule = vec![Presence::Home];
        let mut occupant = Occupant::new("Test")
            .with_presence_schedule(schedule)
            .with_ev("MyEV", EquipmentBehavior::none().with_power_setpoint(7.2));

        let env = test_env().build();
        let mut requests = Vec::new();

        occupant.decide(&env, &mut requests);

        let has_power_setpoint = requests.iter().any(|r| {
            matches!(r.signal, ControlSignal::PowerSetpoint { active_power_kw, .. } if (active_power_kw - 7.2).abs() < 0.01)
        });
        assert!(has_power_setpoint, "expected PowerSetpoint signal for EV");
    }

    #[test]
    fn occupant_plug_loads_by_end_use() {
        let schedule = vec![Presence::Away];
        let mut occupant = Occupant::new("Test")
            .with_presence_schedule(schedule)
            .with_plug_loads(
                EndUse::PLUG_LOADS,
                EquipmentBehavior::none().off_when_away(),
            );

        let env = test_env().build();
        let mut requests = Vec::new();

        occupant.decide(&env, &mut requests);

        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].target,
            DispatchTarget::ByEndUse(EndUse::PLUG_LOADS)
        );
        assert!(
            matches!(
                requests[0].signal,
                ControlSignal::ModeOverride {
                    mode: OperatingMode::Off
                }
            ),
            "expected ModeOverride Off for plug loads"
        );
    }

    #[test]
    fn occupant_load_fraction_signal() {
        let schedule = vec![Presence::Home];
        let mut occupant = Occupant::new("Test")
            .with_presence_schedule(schedule)
            .with_lighting("Lights", EquipmentBehavior::none().with_load_fraction(0.5));

        let env = test_env().build();
        let mut requests = Vec::new();

        occupant.decide(&env, &mut requests);

        let has_load_fraction = requests.iter().any(|r| {
            matches!(r.signal, ControlSignal::LoadFraction { fraction } if (fraction - 0.5).abs() < 0.01)
        });
        assert!(has_load_fraction, "expected LoadFraction signal");
    }

    #[test]
    fn occupant_uses_user_override_priority() {
        let schedule = vec![Presence::Away];
        let mut occupant = Occupant::new("Test")
            .with_presence_schedule(schedule)
            .with_lighting("Lights", EquipmentBehavior::none().off_when_away());

        let env = test_env().build();
        let mut requests = Vec::new();

        occupant.decide(&env, &mut requests);

        assert_eq!(requests[0].priority, PriorityTier::UserOverride);
    }

    #[test]
    fn occupant_multiple_equipment_targets() {
        let schedule = vec![Presence::Away];
        let mut occupant = Occupant::new("Test")
            .with_presence_schedule(schedule)
            .with_lighting("Lights", EquipmentBehavior::none().off_when_away())
            .with_appliance("Washer", EquipmentBehavior::none().off_when_away())
            .with_plug_loads(
                EndUse::PLUG_LOADS,
                EquipmentBehavior::none().off_when_away(),
            );

        let env = test_env().build();
        let mut requests = Vec::new();

        occupant.decide(&env, &mut requests);

        // Should emit 3 signals: lights off, appliance off, plug loads off
        assert_eq!(requests.len(), 3);

        let lighting_signal = requests
            .iter()
            .any(|r| matches!(r.target, DispatchTarget::ByName(ref n) if &**n == "Lights"));
        let appliance_signal = requests
            .iter()
            .any(|r| matches!(r.target, DispatchTarget::ByName(ref n) if &**n == "Washer"));
        let plug_loads_signal = requests.iter().any(
            |r| matches!(r.target, DispatchTarget::ByEndUse(ref e) if *e == EndUse::PLUG_LOADS),
        );

        assert!(lighting_signal, "expected lighting signal");
        assert!(appliance_signal, "expected appliance signal");
        assert!(plug_loads_signal, "expected plug loads signal");
    }

    #[test]
    fn occupant_no_duplicate_signals_on_same_presence() {
        // When presence doesn't change, no transition signals should be emitted
        // (unless continuous control like power setpoint is configured)
        let schedule = vec![Presence::Home, Presence::Home];
        let mut occupant = Occupant::new("Test")
            .with_presence_schedule(schedule)
            .with_lighting("Lights", EquipmentBehavior::none().on_when_home());

        let env = test_env().build();
        let mut requests = Vec::new();

        // First step: transition from default Home to scheduled Home
        // This is NOT a transition (Home -> Home), so no on_when_home signal
        occupant.decide(&env, &mut requests);
        // Actually: default is Home, first step is index 0 which is Home
        // So we're at Home already, first step stays Home -> no transition
        assert!(requests.is_empty(), "no signal expected when staying home");

        // Second step: Home -> Home (no transition)
        requests.clear();
        occupant.decide(&env, &mut requests);
        assert!(
            requests.is_empty(),
            "no signal expected when staying home (second step)"
        );
    }

    #[test]
    fn occupant_sleeping_is_present() {
        let schedule = vec![Presence::Sleeping];
        let occupant = Occupant::new("Test").with_presence_schedule(schedule);

        assert!(occupant.is_present());
        assert!(!occupant.current_presence().is_away());
    }

    #[test]
    fn occupant_transition_home_to_sleeping_no_signal() {
        // Home -> Sleeping is not an away transition, so off_when_away shouldn't trigger
        let schedule = vec![Presence::Home, Presence::Sleeping];
        let mut occupant = Occupant::new("Test")
            .with_presence_schedule(schedule)
            .with_lighting("Lights", EquipmentBehavior::none().off_when_away());

        let env = test_env().build();
        let mut requests = Vec::new();

        // Step 0: Home (default is Home, so no transition)
        occupant.decide(&env, &mut requests);
        assert!(requests.is_empty(), "no signal expected when staying home");

        // Step 1: Home -> Sleeping (still present, no off signal)
        requests.clear();
        occupant.decide(&env, &mut requests);
        assert!(
            requests.is_empty(),
            "no off signal when going to sleep (still present)"
        );
    }

    #[test]
    fn lighting_power_setpoint_not_sent_when_away() {
        // Regression: power setpoint must NOT leak to non-EV equipment when away
        let schedule = vec![Presence::Away];
        let mut occupant = Occupant::new("Test")
            .with_presence_schedule(schedule)
            .with_lighting("Lights", EquipmentBehavior::none().with_power_setpoint(1.0))
            .with_ev("MyEV", EquipmentBehavior::none());

        let env = test_env().build();
        let mut requests = Vec::new();

        occupant.decide(&env, &mut requests);

        let has_power_for_lights = requests.iter().any(|r| {
            matches!(&r.target, DispatchTarget::ByName(n) if &**n == "Lights")
                && matches!(r.signal, ControlSignal::PowerSetpoint { .. })
        });
        assert!(
            !has_power_for_lights,
            "lighting must not receive PowerSetpoint when occupant is away"
        );
    }

    #[test]
    #[should_panic(expected = "presence schedule exhausted")]
    fn schedule_exhaustion_panics_in_debug() {
        let schedule = vec![Presence::Home];
        let mut occupant = Occupant::new("Test").with_presence_schedule(schedule);
        let env = test_env().build();
        let mut requests = Vec::new();

        // Step 0: consumes the only entry, advances to step 1
        occupant.decide(&env, &mut requests);
        // Step 1: schedule exhausted — debug_assert fires
        occupant.decide(&env, &mut requests);
    }

    #[test]
    fn continuous_power_setpoint_sent_every_step_while_present() {
        let schedule = vec![Presence::Home, Presence::Home];
        let mut occupant = Occupant::new("Test")
            .with_presence_schedule(schedule)
            .with_lighting("Lights", EquipmentBehavior::none().with_power_setpoint(1.0));

        let env = test_env().build();
        let mut requests = Vec::new();

        occupant.decide(&env, &mut requests);
        assert_eq!(requests.len(), 1, "power setpoint sent on first step");

        requests.clear();
        occupant.decide(&env, &mut requests);
        assert_eq!(requests.len(), 1, "power setpoint re-sent on second step");
    }
}
