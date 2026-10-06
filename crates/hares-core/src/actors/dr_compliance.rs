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
//!     .with_hvac_action(DrAction::setpoint_delta(2.0));
//!
//! // In the dwelling loop, the actor will dispatch signals when DR is active
//! let mut requests = Vec::new();
//! actor.decide(&env, &mut requests);
//! ```

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use hares_control::{DispatchRequest, DispatchTarget, PriorityTier};
use hares_types::telemetry_keys as tk;
use hares_types::{
    ControlSignal, DRLevel, EnvironmentState, HaresError, OperatingMode, Telemetry, ThermostatAxes,
    ThermostatAxis,
};

use crate::{Actor, ActorEquipment};

use super::constants::DEFAULT_FREEZE_THRESHOLD_C;

/// Default water heater freeze-protection threshold in °C.
///
/// Tank water temperature below this value triggers DR TurnOff rejection to
/// prevent pipe/tank freezing. ASHRAE Guideline 36-2021 §5.16: freeze-stat
/// setpoint not to exceed 4.4°C (40°F). 5°C provides a conservative safety
/// margin for residential WH freeze protection.
const WH_FREEZE_THRESHOLD_C: f64 = DEFAULT_FREEZE_THRESHOLD_C;

/// Per-severity-step multiplier for DR compliance probability scaling.
///
/// HARES engineering choice: no prescriptive standard (ASHRAE, IEEE, or otherwise)
/// governs occupant DR compliance elasticity. The value 0.1 (10% relative increase
/// per DR severity level) is a conservative, tunable default calibrated so that a
/// GridEmergency event at a 50% base rate yields ~70% compliance — consistent with
/// utility-reported participation rates for critical peak pricing and emergency DR
/// events (FERC 2023 Assessment of Demand Response and Advanced Metering, Staff
/// Report, §3.1 Table 3-2: residential critical peak pricing participation rates
/// routinely exceed 60%). OCHRE does not model occupant compliance decisions at all,
/// so no reference value is available from that codebase.
///
/// Adjust via HARES configuration or replace the `Probabilistic` model with a
/// calibrated behavioral model for fleet studies.
const SEVERITY_MULTIPLIER: f64 = 0.1;

/// Applies DR severity scaling to the base compliance rate.
///
/// Returns `base_rate * (1.0 + dr_level_as_int * SEVERITY_MULTIPLIER)`, clamped to `[0.0, 1.0]`.
/// DRLevel::Normal (level 0) receives no scaling; GridEmergency (level 4) receives +40%.
fn severity_adjusted_rate(base_rate: f64, dr_level: DRLevel) -> f64 {
    let factor = 1.0 + (dr_level as u8 as f64) * SEVERITY_MULTIPLIER;
    (base_rate * factor).clamp(0.0, 1.0)
}

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

    /// Returns the severity-adjusted effective compliance rate for the given DR level.
    ///
    /// Returns `None` for models that do not use a configurable compliance rate
    /// (e.g., AlwaysComply, NeverComply).
    fn effective_compliance_rate(&self, _dr_level: DRLevel) -> Option<f64> {
        None
    }
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
        let effective = severity_adjusted_rate(self.compliance_rate, dr_level);
        let hash = self.hash_inputs(dr_level, env);
        let normalized = (hash % 10_000) as f64 / 10_000.0;
        normalized < effective
    }

    fn effective_compliance_rate(&self, dr_level: DRLevel) -> Option<f64> {
        Some(severity_adjusted_rate(self.compliance_rate, dr_level))
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DrAction {
    /// Reduce load by a fraction (0.0 = full curtailment, 1.0 = no change).
    LoadCurtailment { fraction: f64 },
    /// Adjust the thermostat by `delta_c` relative to its schedule.
    ///
    /// A positive delta pre-conditions: it moves one setpoint, since moving
    /// both would narrow the gap and be rejected. The axis is never guessed:
    /// an end-use target and a single-purpose unit name it, and a unit
    /// serving both takes it from `direction` (pre-heat or pre-cool). An
    /// end-use target reaches every unit serving that end use, a unit
    /// serving both included, whatever mode the unit is in.
    ///
    /// A dwelling refuses, with [`HaresError::PreconditioningAxis`] and
    /// itself untouched, every roster change that would leave an event it
    /// cannot serve: an event naming no axis for a unit serving both, a
    /// direction the unit does not serve, a target without a thermostat, a
    /// named target not in the dwelling (the actor is registered after its
    /// equipment), and the removal or a replacement of a named target that
    /// leaves it so. An actor no dwelling holds sends nothing for such a
    /// target and counts it in its `preconditioning_unserved` telemetry.
    ///
    /// A negative delta relaxes: it lowers heating and raises cooling, which
    /// widens the gap for any unit, so it takes no direction (one is
    /// refused). The override is released when the event ends.
    SetpointAdjust {
        delta_c: f64,
        direction: Option<ThermostatAxis>,
    },
    /// Override thermostat to absolute setpoints.
    AbsoluteSetpoint {
        heating_c: Option<f64>,
        cooling_c: Option<f64>,
    },
    /// Turn equipment off.
    TurnOff,
    /// Limit power draw to a maximum.
    PowerLimit { max_kw: f64 },
    /// Dispatch a demand-response signal with built-in auto-reversion.
    /// Equipment applies DR setpoint/curtailment behaviour for `duration_s`
    /// seconds, then automatically reverts to `DRLevel::Normal`.
    /// `None` duration means indefinite — no auto-revert.
    DemandResponse {
        level: DRLevel,
        duration_s: Option<f64>,
    },
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

    /// Creates a setpoint adjustment action whose axis the target names (see
    /// [`DrAction::SetpointAdjust`]).
    pub fn setpoint_delta(delta_c: f64) -> Self {
        Self::SetpointAdjust {
            delta_c,
            direction: None,
        }
    }

    /// Pre-heats by `delta_c > 0`: the heating setpoint up.
    pub fn preheat(delta_c: f64) -> Self {
        Self::SetpointAdjust {
            delta_c,
            direction: Some(ThermostatAxis::Heating),
        }
    }

    /// Pre-cools by `delta_c > 0`: the cooling setpoint down.
    pub fn precool(delta_c: f64) -> Self {
        Self::SetpointAdjust {
            delta_c,
            direction: Some(ThermostatAxis::Cooling),
        }
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

    /// Whether the action leaves state in the equipment that the end of the
    /// event must clear.
    fn is_sticky(&self) -> bool {
        matches!(
            self,
            Self::PowerLimit { .. }
                | Self::TurnOff
                | Self::SetpointAdjust { .. }
                | Self::AbsoluteSetpoint { .. }
        )
    }

    /// Creates a turn-off action.
    pub fn off() -> Self {
        Self::TurnOff
    }

    /// Creates a demand-response action with duration-based auto-reversion.
    ///
    /// Equipment applies DR behaviour for the given duration, then
    /// automatically reverts to `DRLevel::Normal` when the duration expires.
    /// `None` duration means indefinite — no auto-revert.
    ///
    /// # Panics (debug only)
    ///
    /// Panics if `duration_s` is `Some(0.0)` or `Some(negative)`.
    pub fn demand_response(level: DRLevel, duration_s: Option<f64>) -> Self {
        debug_assert!(
            duration_s.is_none_or(|d| d > 0.0),
            "DR duration must be positive or None, got {duration_s:?}"
        );
        Self::DemandResponse { level, duration_s }
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
    /// Track of last-dispatched sticky actions (PowerLimit, TurnOff) per target.
    /// Used to dispatch clear/reset signals when the DR event ends
    /// (current_dr_level transitions to Normal).
    last_dispatched: Vec<(DispatchTarget, DrAction)>,
    /// The pre-conditioning axis of `hvac_target` and of each load target,
    /// resolved against the dwelling's equipment on every identity refresh.
    hvac_target_axis: AxisResolution,
    load_target_axes: Vec<AxisResolution>,
    /// The axis each target's pre-conditioning has displaced in this event
    /// (part of the checkpoint): a change of axis releases the old one first.
    displaced_axes: Vec<(DispatchTarget, ThermostatAxis)>,
    /// Whether this event has already warned of a pre-conditioning delta
    /// it could not send.
    warned_unserved: bool,
    /// Axis releases dispatched this step, each beside its target's delta.
    axis_releases_this_step: u64,
    /// Count of clear signals dispatched this timestep (observability gate).
    #[cfg(feature = "observe")]
    clear_signals_dispatched_count: u64,
    /// Count of signals rejected this timestep due to protected state
    /// (defrost active, WH tank below freeze threshold) (observability gate).
    #[cfg(feature = "observe")]
    signals_rejected_count: u64,
}

impl DrCompliance {
    /// Creates a new DR compliance actor with the default compliance model.
    pub fn new(name: &str) -> Self {
        let mut telemetry = Telemetry::with_capacity(12);
        // Why: telemetry initialises count and flag fields to 0.0 for
        // "not yet occurred / not active." demand_response_duration_s = 0.0
        // means "no DemandResponse action with a duration was dispatched
        // this step" — the constructor validates duration > 0 and the
        // decide() invariant prevents zero-duration dispatches, so 0.0
        // cannot collide with a legitimate output value.
        telemetry.insert("dr_level", 0.0);
        telemetry.insert("dr_active", 0.0);
        telemetry.insert("dr_complied", 0.0);
        telemetry.insert("signals_count", 0.0);
        telemetry.insert("signals_rejected", 0.0);
        telemetry.insert("dr_freeze_guard", 0.0);
        telemetry.insert("clear_signals_dispatched_count", 0.0);
        telemetry.insert("targets_configured", 0.0);
        telemetry.insert("effective_compliance_rate", 0.0);
        telemetry.insert("demand_response_level", 0.0);
        telemetry.insert("demand_response_duration_s", 0.0);
        telemetry.insert("preconditioning_unserved", 0.0);
        Self {
            name: Arc::from(name),
            model: Box::new(AlwaysComply),
            hvac_target: None,
            hvac_action: DrAction::None,
            load_targets: Vec::new(),
            current_dr_level: DRLevel::Normal,
            freeze_risk_threshold_c: DEFAULT_FREEZE_THRESHOLD_C,
            telemetry,
            last_dispatched: Vec::new(),
            hvac_target_axis: Ok(None),
            load_target_axes: Vec::new(),
            displaced_axes: Vec::new(),
            warned_unserved: false,
            axis_releases_this_step: 0,
            #[cfg(feature = "observe")]
            clear_signals_dispatched_count: 0,
            #[cfg(feature = "observe")]
            signals_rejected_count: 0,
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
        self.telemetry
            .set("targets_configured", self.load_targets.len() as f64 + 1.0);
        self.resolve_axes(&[]);
        self
    }

    /// Sets the HVAC action to take when complying.
    pub fn with_hvac_action(mut self, action: DrAction) -> Self {
        self.hvac_action = action;
        self.resolve_axes(&[]);
        self
    }

    /// Adds a load curtailment target with a specific action.
    pub fn with_load_target(mut self, target: DispatchTarget, action: DrAction) -> Self {
        self.load_targets.push((target, action));
        self.telemetry.set(
            "targets_configured",
            self.hvac_target.is_some() as u8 as f64 + self.load_targets.len() as f64,
        );
        self.resolve_axes(&[]);
        self
    }

    /// Each configured target with its action, the HVAC target first.
    fn targets(&self) -> impl Iterator<Item = (&DispatchTarget, &DrAction)> {
        self.hvac_target
            .iter()
            .map(|target| (target, &self.hvac_action))
            .chain(self.load_targets.iter().map(|(t, a)| (t, a)))
    }

    /// Resolves each target's pre-conditioning axis against `equipment`,
    /// keeping the reason a target has none. A dwelling refuses any roster
    /// change that leaves a reason (`validate_equipment`), so a reason
    /// survives only in an actor no dwelling holds.
    fn resolve_axes(&mut self, equipment: &[ActorEquipment<'_>]) {
        self.hvac_target_axis = self.hvac_target.as_ref().map_or(Ok(None), |target| {
            preconditioning_axis(target, &self.hvac_action, equipment)
        });
        self.load_target_axes = self
            .load_targets
            .iter()
            .map(|(target, action)| preconditioning_axis(target, action, equipment))
            .collect();
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

    /// Returns true if the target/signal pair should be rejected because the
    /// equipment is in a protected state that blocks control dispatch.
    ///
    /// Checks `env.equipment_telemetry` for ByName targets:
    /// - Defrost: when equipment is actively defrosting, reject PowerLimit and
    ///   ModeOverride (TurnOff) — these would interrupt or conflict with the
    ///   defrost cycle.
    /// - Water heater freeze protection: when tank average temperature is below
    ///   [`WH_FREEZE_THRESHOLD_C`], reject TurnOff to prevent pipe/tank freezing.
    fn should_reject_dispatch(
        &self,
        target: &DispatchTarget,
        action: &DrAction,
        env: &EnvironmentState,
    ) -> bool {
        let DispatchTarget::ByName(name) = target else {
            return false;
        };
        // allowed: dispatch targets are name-keyed and equipment_telemetry
        // is the only environment channel reachable by name (equipment_core
        // is EquipmentId-keyed and this actor holds no name→id mapping), so
        // resolving the target's telemetry channel requires this read.
        let Some(telem) = env.equipment_telemetry.get(name.as_ref()) else {
            return false;
        };
        // allowed: defrost_active remains telemetry-only until CoreOutput
        // gains a defrost field — CoreState has none, and rejecting
        // PowerLimit/TurnOff during an active defrost cycle must not wait
        // for one.
        if telem.get(tk::DEFROST_ACTIVE) == Some(1.0)
            && matches!(action, DrAction::PowerLimit { .. } | DrAction::TurnOff)
        {
            return true;
        }
        if matches!(action, DrAction::TurnOff) {
            // allowed: tank_avg_temp_c remains telemetry-only until
            // CoreOutput gains a tank temperature field — CoreState has
            // none, and water-heater freeze protection must not wait for
            // one.
            if let Some(tank_temp) = telem.get(tk::TANK_AVG_TEMP_C)
                && tank_temp.is_finite()
                && tank_temp < WH_FREEZE_THRESHOLD_C
            {
                return true;
            }
        }
        false
    }

    /// Generates dispatch signals for the given action.
    ///
    /// All signals are dispatched at `PriorityTier::Grid`. DR events are
    /// utility-initiated and must override both schedule-level operations
    /// and user overrides. This overrides the central
    /// `From<&ControlSignal> for PriorityTier` mapping — a DR-induced
    /// setpoint adjustment or mode override is categorically a grid action,
    /// not a user or schedule action.
    ///
    /// `axis` is the target's resolved pre-conditioning axis. Returns `false`
    /// when a pre-conditioning delta was not sent because the target has no
    /// axis for it, which only an actor no dwelling holds can reach.
    fn dispatch_for_action(
        &mut self,
        target: &DispatchTarget,
        action: &DrAction,
        axis: AxisResolution,
        out: &mut Vec<DispatchRequest>,
    ) -> bool {
        let signal = match action {
            DrAction::LoadCurtailment { fraction } => ControlSignal::LoadFraction {
                fraction: *fraction,
            },
            DrAction::SetpointAdjust { delta_c, .. } if *delta_c > 0.0 => {
                let axis = match axis {
                    Ok(Some(axis)) => axis,
                    unserved => {
                        if !self.warned_unserved {
                            self.warned_unserved = true;
                            tracing::warn!(
                                actor = %self.name,
                                ?target,
                                reason = unserved.err().as_deref().unwrap_or("no axis resolved"),
                                "DR pre-conditioning not sent: the target has no axis for it"
                            );
                        }
                        return false;
                    }
                };
                self.release_a_changed_axis(target, axis, out);
                preconditioning_signal(axis, *delta_c)
            }
            DrAction::SetpointAdjust { delta_c, .. } if *delta_c < 0.0 => {
                ControlSignal::ThermalSetpointDelta {
                    heating_delta_c: Some(*delta_c),
                    cooling_delta_c: Some(-delta_c),
                }
            }
            DrAction::SetpointAdjust { .. } => return true,
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
            DrAction::DemandResponse { level, duration_s } => ControlSignal::DemandResponse {
                level: *level,
                duration_s: *duration_s,
            },
            DrAction::None => return true,
        };

        out.push(DispatchRequest {
            target: target.clone(),
            signal,
            priority: PriorityTier::Grid,
        });
        true
    }

    /// Only one axis is ever displaced: when a target's pre-conditioning
    /// axis changes within an event (its equipment was replaced), the old
    /// axis is released back to its schedule and band before the new delta.
    fn release_a_changed_axis(
        &mut self,
        target: &DispatchTarget,
        axis: ThermostatAxis,
        out: &mut Vec<DispatchRequest>,
    ) {
        match self.displaced_axes.iter_mut().find(|(t, _)| t == target) {
            Some((_, displaced)) if *displaced == axis => {}
            Some((_, displaced)) => {
                out.push(DispatchRequest {
                    target: target.clone(),
                    signal: ControlSignal::thermal_release(),
                    priority: PriorityTier::Grid,
                });
                *displaced = axis;
                self.axis_releases_this_step += 1;
            }
            None => self.displaced_axes.push((target.clone(), axis)),
        }
    }

    /// Dispatches clear/reset signals for all previously-dispatched targets
    /// and clears the tracking vector. Returns the number of clear signals
    /// dispatched.
    fn dispatch_clear_signals(&mut self, out: &mut Vec<DispatchRequest>) -> usize {
        let before = out.len();
        for (target, action) in &self.last_dispatched {
            match action {
                DrAction::PowerLimit { .. } => {
                    out.push(DispatchRequest {
                        target: target.clone(),
                        signal: ControlSignal::PowerLimit {
                            max_power_kw: f64::INFINITY,
                            ramp_rate_kw_per_s: None,
                        },
                        priority: PriorityTier::Grid,
                    });
                }
                DrAction::SetpointAdjust { .. } | DrAction::AbsoluteSetpoint { .. } => {
                    out.push(DispatchRequest {
                        target: target.clone(),
                        signal: ControlSignal::thermal_release(),
                        priority: PriorityTier::Grid,
                    });
                }
                DrAction::TurnOff => {
                    // ModeOverride clearing via ControlSignal is not currently
                    // achievable: the equipment stores ctrl_mode_override as
                    // Option<OperatingMode> where None means "autonomous", but
                    // ControlSignal::ModeOverride always sets Some(mode).
                    // OperatingMode::Auto does not exist in the enum.
                    // Full clearing requires equipment-side changes to
                    // apply_dr_level(DRLevel::Normal).
                    // See Implementation Notes / Known Limitations.
                }
                DrAction::LoadCurtailment { .. }
                | DrAction::DemandResponse { .. }
                | DrAction::None => {
                    // Non-sticky actions are filtered by track_dispatched;
                    // reaching here indicates a bug in track_dispatched.
                    debug_assert!(false, "non-sticky action in last_dispatched: {action:?}");
                }
            }
        }
        self.last_dispatched.clear();
        out.len().saturating_sub(before)
    }

    /// Records a dispatched action for later clearing if it creates sticky state.
    fn track_dispatched(&mut self, target: &DispatchTarget, action: &DrAction) {
        if !action.is_sticky() {
            return;
        }
        // Use conflicts_with for dedup: same-target new entry replaces old.
        self.last_dispatched
            .retain(|(t, _)| !t.conflicts_with(target));
        self.last_dispatched.push((target.clone(), action.clone()));
    }
}

impl Actor for DrCompliance {
    fn name(&self) -> &str {
        &self.name
    }

    fn telemetry(&self) -> Option<&Telemetry> {
        Some(&self.telemetry)
    }

    fn validate_equipment(&self, equipment: &[ActorEquipment<'_>]) -> Result<(), HaresError> {
        for (target, action) in self.targets() {
            preconditioning_axis(target, action, equipment).map_err(|reason| {
                HaresError::PreconditioningAxis {
                    actor: self.name.to_string(),
                    target: format!("{target:?}"),
                    reason,
                }
            })?;
        }
        Ok(())
    }

    fn resolve_equipment(&mut self, equipment: &[ActorEquipment<'_>]) {
        self.resolve_axes(equipment);
    }

    fn decide(&mut self, env: &EnvironmentState, out: &mut Vec<DispatchRequest>) {
        // Populate telemetry regardless of DR activity.
        self.telemetry
            .set("dr_level", dr_level_as_f64(self.current_dr_level));
        self.telemetry
            .set("dr_active", if self.is_dr_active() { 1.0 } else { 0.0 });

        if self.current_dr_level == DRLevel::Normal {
            debug_assert!(
                self.last_dispatched.iter().all(|(_, a)| a.is_sticky()),
                "DR actor '{}' last_dispatched contains non-sticky entries",
                self.name,
            );
            let clear_count = if !self.last_dispatched.is_empty() {
                self.dispatch_clear_signals(out)
            } else {
                0
            };
            self.displaced_axes.clear();
            self.warned_unserved = false;
            self.telemetry.set("dr_complied", 0.0);
            self.telemetry.set("signals_count", clear_count as f64);
            self.telemetry.set("signals_rejected", 0.0);
            self.telemetry.set("dr_freeze_guard", 0.0);
            self.telemetry.set("preconditioning_unserved", 0.0);
            #[cfg(feature = "observe")]
            {
                self.clear_signals_dispatched_count = clear_count as u64;
                self.telemetry
                    .set("clear_signals_dispatched_count", clear_count as f64);
            }
            return;
        }

        let should_comply = self.model.should_comply(self.current_dr_level, env);

        self.telemetry
            .set("dr_complied", if should_comply { 1.0 } else { 0.0 });

        #[cfg(feature = "observe")]
        {
            // Why: unwrap_or(0.0) for models that don't use a compliance rate
            // (AlwaysComply, NeverComply return None). Probabilistic always
            // returns Some(rate), so 0.0 here means "not applicable" — a real
            // 0% rate would come through as Some(0.0), not None.
            self.telemetry.set(
                "effective_compliance_rate",
                self.model
                    .effective_compliance_rate(self.current_dr_level)
                    .unwrap_or(0.0),
            );
        }

        tracing::debug!(
            actor = %self.name,
            dr_level = ?self.current_dr_level,
            comply = should_comply,
            "DR compliance decision"
        );

        if !should_comply {
            self.telemetry.set("signals_count", 0.0);
            self.telemetry.set("signals_rejected", 0.0);
            self.telemetry.set("dr_freeze_guard", 0.0);
            self.telemetry.set("preconditioning_unserved", 0.0);
            return;
        }

        // Clear previous tracking before recording this step's dispatches.
        self.last_dispatched.clear();

        // Reset freeze-guard telemetry at the top of the complied path so the
        // field reflects the current step's guard status regardless of
        // configuration (hvac_target present or absent).
        self.telemetry.set("dr_freeze_guard", 0.0);

        let before = out.len();
        let mut rejected: u64 = 0;
        let mut unserved: u32 = 0;
        self.axis_releases_this_step = 0;

        if let Some(target) = self.hvac_target.clone() {
            let target = &target;
            let hvac_action = self.hvac_action.clone();
            // Protected-state check: equipment in defrost or WH freeze protection
            // blocks DR control signals. This runs before the freeze-protection guard
            // so telemetry correctly records the rejection rather than a downgrade.
            if self.should_reject_dispatch(target, &hvac_action, env) {
                rejected = rejected.saturating_add(1);
            } else if matches!(&hvac_action, DrAction::TurnOff) && self.any_zone_below_freeze(env) {
                // Freeze-protection guard: when DR TurnOff targets HVAC and any
                // zone is below the freeze-risk threshold, downgrade to a
                // minimum-heating ThermalSetpoint instead of ModeOverride::Off.
                // This prevents equipment/building damage from freezing during
                // DR events.
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
            } else if !self.dispatch_for_action(
                target,
                &hvac_action,
                self.hvac_target_axis.clone(),
                out,
            ) {
                unserved += 1;
            } else {
                #[cfg(feature = "observe")]
                {
                    if let DrAction::DemandResponse { level, duration_s } = &hvac_action {
                        self.telemetry
                            .set("demand_response_level", dr_level_as_f64(*level));
                        // Why: None duration = indefinite DR event; 0.0
                        // sentinel means "no finite timer running." The
                        // constructor rejects duration_s = Some(0.0).
                        self.telemetry
                            .set("demand_response_duration_s", duration_s.unwrap_or(0.0));
                    }
                }
                self.track_dispatched(target, &hvac_action);
            }
        }

        // Collect target-action pairs to avoid borrowing self.load_targets
        // during mutable self.track_dispatched calls.
        let load_pairs: Vec<(DispatchTarget, DrAction)> = self
            .load_targets
            .iter()
            .map(|(t, a)| (t.clone(), a.clone()))
            .collect();

        for (idx, (target, action)) in load_pairs.iter().enumerate() {
            // Protected-state check: skip dispatch for equipment in defrost
            // or WH freeze protection. Recording rejection rather than
            // dispatching prevents fire-and-forget signals that equipment
            // would silently ignore.
            if self.should_reject_dispatch(target, action, env) {
                rejected = rejected.saturating_add(1);
                continue;
            }
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
            } else if !self.dispatch_for_action(
                target,
                action,
                self.load_target_axes[idx].clone(),
                out,
            ) {
                unserved += 1;
            } else {
                #[cfg(feature = "observe")]
                {
                    if let DrAction::DemandResponse { level, duration_s } = action {
                        self.telemetry
                            .set("demand_response_level", dr_level_as_f64(*level));
                        // Why: None duration = indefinite DR event; 0.0
                        // sentinel means "no finite timer running." The
                        // constructor rejects duration_s = Some(0.0).
                        self.telemetry
                            .set("demand_response_duration_s", duration_s.unwrap_or(0.0));
                    }
                }
                self.track_dispatched(target, action);
            }
        }

        self.telemetry
            .set("signals_count", (out.len() - before) as f64);
        self.telemetry.set("signals_rejected", rejected as f64);
        self.telemetry
            .set("preconditioning_unserved", f64::from(unserved));
        #[cfg(feature = "observe")]
        {
            self.signals_rejected_count = rejected;
        }

        #[cfg(debug_assertions)]
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

            // Invariant: rejected signals must be excluded from the dispatch
            // output. Every rejection counted in `signals_rejected` must
            // correspond to a signal that was NOT pushed to `out`.
            let dispatched_and_rejected = rejected + (out.len() - before) as u64;
            let expected_accepted = (self.hvac_target.is_some() as u64)
                .saturating_add(self.load_targets.len() as u64)
                .saturating_add(self.axis_releases_this_step);
            debug_assert!(
                dispatched_and_rejected <= expected_accepted,
                "DR actor '{}' dispatched+rejected ({dispatched_and_rejected}) exceeds configured targets ({expected_accepted})",
                self.name,
            );
        }
    }

    fn save_state(&self) -> Result<Vec<u8>, HaresError> {
        let data = (
            &self.current_dr_level,
            &self.last_dispatched,
            &self.displaced_axes,
        );
        postcard::to_allocvec(&data)
            .map_err(|e| HaresError::Io(format!("DrCompliance save_state: {e}")))
    }

    fn load_state(&mut self, data: &[u8]) -> Result<(), HaresError> {
        if data.is_empty() {
            return Ok(());
        }
        type State = (
            DRLevel,
            Vec<(DispatchTarget, DrAction)>,
            Vec<(DispatchTarget, ThermostatAxis)>,
        );
        let (level, dispatched, displaced): State = postcard::from_bytes(data)
            .map_err(|e| HaresError::Io(format!("DrCompliance load_state: {e}")))?;
        self.current_dr_level = level;
        self.last_dispatched = dispatched;
        self.displaced_axes = displaced;
        Ok(())
    }
}

/// The axis a target's pre-conditioning moves, or why it has none.
type AxisResolution = Result<Option<ThermostatAxis>, String>;

/// The axis a positive pre-conditioning delta on `target` moves, from what
/// the target is, never from the weather or a mode: `Ok(None)` when the
/// action moves no single axis; `Err(reason)` when the target has no
/// single axis for it, a named target is not in `equipment`, or the
/// action names a direction for a delta that is not a positive
/// pre-conditioning one.
fn preconditioning_axis(
    target: &DispatchTarget,
    action: &DrAction,
    equipment: &[ActorEquipment<'_>],
) -> AxisResolution {
    let DrAction::SetpointAdjust { delta_c, direction } = action else {
        return Ok(None);
    };
    if !delta_c.is_finite() {
        return Err(format!("the delta {delta_c} C is not finite"));
    }
    if *delta_c <= 0.0 {
        return match direction {
            None => Ok(None),
            Some(named) => Err(format!(
                "a {named:?} direction names the axis a positive delta pre-conditions; \
                 the delta {delta_c} C relaxes, which widens both setpoints and takes none"
            )),
        };
    }
    let axes = match target {
        DispatchTarget::ByEndUse(end_use) => ThermostatAxis::of_end_use(end_use)
            .map(ThermostatAxes::One)
            .ok_or_else(|| format!("end use '{}' has no thermostat setpoint", end_use.as_str()))?,
        DispatchTarget::ByName(name) => equipment
            .iter()
            .find(|e| *e.descriptor.name == **name)
            .ok_or_else(|| {
                format!(
                    "no equipment named '{name}' is in the dwelling; \
                     the actor is registered after its equipment"
                )
            })?
            .thermostat_axes
            .ok_or_else(|| "it has no thermostat setpoint".to_string())?,
    };
    match (axes.select(*direction), axes, direction) {
        (Some(axis), _, _) => Ok(Some(axis)),
        (None, ThermostatAxes::Both, _) => Err(
            "it serves heating and cooling, so the event must name its direction \
             (DrAction::preheat or DrAction::precool)"
                .to_string(),
        ),
        (None, ThermostatAxes::One(serves), named) => Err(format!(
            "it serves only the {serves:?} setpoint, not the {named:?} the event names"
        )),
    }
}

/// Pre-conditioning by `delta_c > 0`: the heating setpoint up, or the
/// cooling setpoint down. Only this axis moves; the converging pair would
/// narrow the gap and be rejected.
fn preconditioning_signal(axis: ThermostatAxis, delta_c: f64) -> ControlSignal {
    match axis {
        ThermostatAxis::Heating => ControlSignal::ThermalSetpointDelta {
            heating_delta_c: Some(delta_c),
            cooling_delta_c: None,
        },
        ThermostatAxis::Cooling => ControlSignal::ThermalSetpointDelta {
            heating_delta_c: None,
            cooling_delta_c: Some(-delta_c),
        },
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
    use hares_types::{ControlCapabilities, EndUse, EquipmentDescriptor, EquipmentId};

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
            outcomes.insert(model.should_comply(DRLevel::Normal, &env));
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
            .with_hvac_target(DispatchTarget::ByEndUse(EndUse::HVAC_HEATING))
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

        let hvac_signal = requests.iter().any(
            |r| matches!(&r.target, DispatchTarget::ByEndUse(eu) if *eu == EndUse::HVAC_HEATING),
        );
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
            if model.should_comply(DRLevel::Normal, &env) {
                comply_count += 1;
            }
        }

        let rate = comply_count as f64 / trials as f64;
        assert!(
            rate > 0.3 && rate < 0.7,
            "compliance rate {rate} should be near 0.5 for 50% configured rate with Normal severity (no scaling)"
        );
    }

    #[test]
    fn probabilistic_grid_emergency_higher_compliance_than_moderate() {
        let env = test_env().build();
        let trials = 1000;
        let mut moderate_complies = 0usize;
        let mut emergency_complies = 0usize;

        for i in 0..trials {
            let m = Probabilistic::new(0.5).with_seed(i * 2);
            if m.should_comply(DRLevel::Moderate, &env) {
                moderate_complies += 1;
            }
            let e = Probabilistic::new(0.5).with_seed(i * 2 + 1);
            if e.should_comply(DRLevel::GridEmergency, &env) {
                emergency_complies += 1;
            }
        }

        let moderate_rate = moderate_complies as f64 / trials as f64;
        let emergency_rate = emergency_complies as f64 / trials as f64;
        assert!(
            emergency_rate > moderate_rate,
            "GridEmergency compliance ({emergency_rate}) must exceed Moderate compliance ({moderate_rate}) over {trials} trials"
        );
    }

    #[test]
    fn probabilistic_normal_with_rate_one_always_complies() {
        let env = test_env().build();
        let model = Probabilistic::new(1.0);
        assert!(
            model.should_comply(DRLevel::Normal, &env),
            "rate=1.0 with Normal must always comply"
        );
    }

    #[test]
    fn probabilistic_severity_scaling_preserves_deterministic_reproducibility() {
        let env = test_env().build();
        let model = Probabilistic::new(0.5).with_seed(42);
        let result1 = model.should_comply(DRLevel::GridEmergency, &env);
        let result2 = model.should_comply(DRLevel::GridEmergency, &env);
        assert_eq!(
            result1, result2,
            "same seed + same severity must produce same result after severity scaling"
        );
    }

    const AC_ID: EquipmentId = EquipmentId(11);
    const FURNACE_ID: EquipmentId = EquipmentId(12);
    const IDEAL_ID: EquipmentId = EquipmentId(13);

    fn descriptor(
        name: &str,
        id: EquipmentId,
        end_use: EndUse,
        control_capabilities: ControlCapabilities,
    ) -> EquipmentDescriptor {
        EquipmentDescriptor {
            id,
            name: name.to_string(),
            end_use,
            equipment_type: std::borrow::Cow::Borrowed("Test"),
            zone: None,
            fuel: hares_types::FuelType::Electric,
            stage: hares_types::ExecutionStage::Thermal,
            control_capabilities,
            core_capabilities: hares_types::CoreCapabilities::ELECTRIC,
            telemetry_fields: vec![],
            zone_type: None,
        }
    }

    const LIGHTS_ID: EquipmentId = EquipmentId(14);

    /// An AC, a furnace, an ideal unit (which serves both) and lights.
    fn descriptors() -> Vec<EquipmentDescriptor> {
        vec![
            descriptor(
                "Air Conditioner",
                AC_ID,
                EndUse::HVAC_COOLING,
                ControlCapabilities::THERMAL_SETPOINT,
            ),
            descriptor(
                "Gas Furnace",
                FURNACE_ID,
                EndUse::HVAC_HEATING,
                ControlCapabilities::THERMAL_SETPOINT,
            ),
            descriptor(
                "Ideal HVAC",
                IDEAL_ID,
                EndUse::HVAC_HEATING,
                ControlCapabilities::IDEAL_CAPACITY | ControlCapabilities::THERMAL_SETPOINT,
            ),
            descriptor(
                "Lights",
                LIGHTS_ID,
                EndUse::LIGHTING,
                ControlCapabilities::LOAD_FRACTION,
            ),
        ]
    }

    /// What each of [`descriptors`] declares its thermostat serves.
    fn equipment(descriptors: &[EquipmentDescriptor]) -> Vec<ActorEquipment<'_>> {
        use ThermostatAxis::{Cooling, Heating};
        descriptors
            .iter()
            .map(|descriptor| ActorEquipment {
                descriptor,
                thermostat_axes: match descriptor.id {
                    AC_ID => Some(ThermostatAxes::One(Cooling)),
                    FURNACE_ID => Some(ThermostatAxes::One(Heating)),
                    IDEAL_ID => Some(ThermostatAxes::Both),
                    _ => None,
                },
            })
            .collect()
    }

    /// A registered actor: validated and resolved against [`descriptors`].
    fn setpoint_actor(target: DispatchTarget, action: DrAction) -> DrCompliance {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(target)
            .with_hvac_action(action);
        let descriptors = descriptors();
        let equipment = equipment(&descriptors);
        actor
            .validate_equipment(&equipment)
            .expect("the event has an axis for its target");
        actor.resolve_equipment(&equipment);
        actor
    }

    /// An environment whose weather says the opposite season to the unit:
    /// the axis must not come from it.
    fn env_with(outdoor_c: f64) -> EnvironmentState {
        test_env().outdoor_temp(outdoor_c).zone_temp(24.0).build()
    }

    fn decide_once(actor: &mut DrCompliance, env: &EnvironmentState) -> Vec<DispatchRequest> {
        actor.set_dr_level(DRLevel::High);
        let mut requests = Vec::new();
        actor.decide(env, &mut requests);
        requests
    }

    fn setpoint_adjust_signal_in(target: DispatchTarget, action: DrAction) -> Vec<DispatchRequest> {
        decide_once(&mut setpoint_actor(target, action), &env_with(18.0))
    }

    fn delta_of(requests: &[DispatchRequest]) -> (Option<f64>, Option<f64>) {
        match requests {
            [
                DispatchRequest {
                    signal:
                        ControlSignal::ThermalSetpointDelta {
                            heating_delta_c,
                            cooling_delta_c,
                        },
                    ..
                },
            ] => (*heating_delta_c, *cooling_delta_c),
            other => panic!("expected one ThermalSetpointDelta, got {other:?}"),
        }
    }

    /// Pre-conditioning moves the one axis the target serves, whatever the
    /// weather (outdoor 18 °C under a 24 °C zone: an evening an AC is still
    /// cooling); the converging pair would narrow the gap and be rejected.
    #[test]
    fn preconditioning_moves_the_axis_the_target_serves() {
        for (target, expected) in [
            (
                DispatchTarget::ByName("Air Conditioner".into()),
                (None, Some(-2.0)),
            ),
            (
                DispatchTarget::ByEndUse(EndUse::HVAC_COOLING),
                (None, Some(-2.0)),
            ),
            (
                DispatchTarget::ByName("Gas Furnace".into()),
                (Some(2.0), None),
            ),
            (
                DispatchTarget::ByEndUse(EndUse::HVAC_HEATING),
                (Some(2.0), None),
            ),
        ] {
            assert_eq!(
                delta_of(&setpoint_adjust_signal_in(
                    target.clone(),
                    DrAction::setpoint_delta(2.0)
                )),
                expected,
                "{target:?}"
            );
        }
    }

    /// A unit serving both moves the axis the event names, every step,
    /// whatever its mode; a direction a single-purpose unit serves is
    /// accepted.
    #[test]
    fn a_dual_mode_target_preconditions_on_the_named_axis() {
        let ideal = DispatchTarget::ByName("Ideal HVAC".into());
        let mut actor = setpoint_actor(ideal.clone(), DrAction::precool(2.0));
        for outdoor_c in [30.0, 5.0, 30.0] {
            assert_eq!(
                delta_of(&decide_once(&mut actor, &env_with(outdoor_c))),
                (None, Some(-2.0))
            );
        }
        assert_eq!(
            delta_of(&setpoint_adjust_signal_in(ideal, DrAction::preheat(2.0))),
            (Some(2.0), None)
        );
        assert_eq!(
            delta_of(&setpoint_adjust_signal_in(
                DispatchTarget::ByName("Gas Furnace".into()),
                DrAction::preheat(2.0)
            )),
            (Some(2.0), None)
        );
    }

    /// An event that names no single axis for its target is refused when
    /// the actor is registered: a dual-mode unit without a direction, a
    /// direction the unit does not serve, a target without a thermostat, a
    /// named target not among the equipment, a direction on a relaxation,
    /// and a delta that is not finite.
    #[test]
    fn an_event_without_an_axis_for_its_target_is_refused_at_registration() {
        let descriptors = descriptors();
        let equipment = equipment(&descriptors);
        for (target, action) in [
            (
                DispatchTarget::ByName("Ideal HVAC".into()),
                DrAction::setpoint_delta(2.0),
            ),
            (
                DispatchTarget::ByName("Gas Furnace".into()),
                DrAction::precool(2.0),
            ),
            (
                DispatchTarget::ByEndUse(EndUse::HVAC_COOLING),
                DrAction::preheat(2.0),
            ),
            (
                DispatchTarget::ByName("Lights".into()),
                DrAction::setpoint_delta(2.0),
            ),
            (
                DispatchTarget::ByEndUse(EndUse::LIGHTING),
                DrAction::setpoint_delta(2.0),
            ),
            (
                DispatchTarget::ByName("Idael HVAC".into()),
                DrAction::precool(2.0),
            ),
            (
                DispatchTarget::ByName("Ideal HVAC".into()),
                DrAction::preheat(-2.0),
            ),
            (
                DispatchTarget::ByEndUse(EndUse::HVAC_COOLING),
                DrAction::precool(0.0),
            ),
            (
                DispatchTarget::ByName("Gas Furnace".into()),
                DrAction::setpoint_delta(f64::NAN),
            ),
        ] {
            let actor = DrCompliance::new("Test")
                .with_hvac_target(target.clone())
                .with_hvac_action(action.clone());
            assert!(
                matches!(
                    actor.validate_equipment(&equipment),
                    Err(HaresError::PreconditioningAxis { .. })
                ),
                "{target:?} {action:?}"
            );
        }
    }

    /// An actor no dwelling holds has not resolved a named target, so it
    /// sends no pre-conditioning delta for it and counts it; the counter
    /// clears when the event ends and when the actor stops complying.
    #[test]
    fn an_unserved_preconditioning_delta_is_counted_and_the_count_clears() {
        let unresolved = || {
            DrCompliance::new("Test")
                .with_compliance_model(AlwaysComply)
                .with_hvac_target(DispatchTarget::ByName("Gas Furnace".into()))
                .with_hvac_action(DrAction::setpoint_delta(2.0))
        };
        let count =
            |actor: &DrCompliance| actor.telemetry().unwrap().get("preconditioning_unserved");
        let mut actor = unresolved();
        assert!(decide_once(&mut actor, &env_with(5.0)).is_empty());
        assert_eq!(count(&actor), Some(1.0));
        actor.set_dr_level(DRLevel::Normal);
        actor.decide(&env_with(5.0), &mut Vec::new());
        assert_eq!(count(&actor), Some(0.0), "cleared when the event ends");

        let mut actor = unresolved();
        assert!(decide_once(&mut actor, &env_with(5.0)).is_empty());
        actor.model = Box::new(NeverComply);
        actor.decide(&env_with(5.0), &mut Vec::new());
        assert_eq!(count(&actor), Some(0.0), "cleared when not complying");
    }

    /// When a target's axis changes within an event (its unit replaced),
    /// the old axis is released before the new one moves, so only one axis
    /// is ever displaced; the displaced axis is part of the checkpoint.
    #[test]
    fn a_change_of_axis_within_an_event_releases_the_old_axis_first() {
        let unit = DispatchTarget::ByName("Unit".into());
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(unit.clone())
            .with_hvac_action(DrAction::setpoint_delta(2.0));
        let as_unit = |id: EquipmentId, end_use: EndUse| {
            let mut d = descriptor("Unit", id, end_use, ControlCapabilities::THERMAL_SETPOINT);
            d.name = "Unit".to_string();
            d
        };
        let furnace = [as_unit(FURNACE_ID, EndUse::HVAC_HEATING)];
        let ac = [as_unit(AC_ID, EndUse::HVAC_COOLING)];
        actor.resolve_equipment(&equipment(&furnace));
        assert_eq!(
            delta_of(&decide_once(&mut actor, &env_with(5.0))),
            (Some(2.0), None)
        );

        let checkpoint = actor.save_state().unwrap();
        let mut restored = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(unit.clone())
            .with_hvac_action(DrAction::setpoint_delta(2.0));
        restored.load_state(&checkpoint).unwrap();
        for actor in [&mut actor, &mut restored] {
            actor.resolve_equipment(&equipment(&ac));
            let requests = decide_once(actor, &env_with(5.0));
            assert_eq!(requests.len(), 2, "{requests:?}");
            assert_eq!(requests[0].signal, ControlSignal::thermal_release());
            assert_eq!(delta_of(&requests[1..]), (None, Some(-2.0)));
            assert_eq!(
                delta_of(&decide_once(actor, &env_with(5.0))),
                (None, Some(-2.0)),
                "released once"
            );
        }
    }

    /// Relaxation widens the gap on both axes, which is valid for any unit.
    #[test]
    fn relaxation_widens_both_axes() {
        assert_eq!(
            delta_of(&setpoint_adjust_signal_in(
                DispatchTarget::ByName("Lights".into()),
                DrAction::setpoint_delta(-2.0)
            )),
            (Some(-2.0), Some(2.0))
        );
    }

    /// A setpoint action leaves a runtime override in the thermostat, so
    /// the end of the event hands both axes back with the release form.
    #[test]
    fn setpoint_actions_are_released_when_the_event_ends() {
        for action in [
            DrAction::setpoint_delta(2.0),
            DrAction::absolute_setpoint(18.0, 28.0),
        ] {
            let mut actor =
                setpoint_actor(DispatchTarget::ByName("Gas Furnace".into()), action.clone());
            let env = env_with(0.0);
            let mut requests = decide_once(&mut actor, &env);
            assert_eq!(requests.len(), 1, "{action:?}");

            actor.set_dr_level(DRLevel::Normal);
            requests.clear();
            actor.decide(&env, &mut requests);
            assert_eq!(requests.len(), 1, "{action:?}: the release is dispatched");
            assert_eq!(requests[0].signal, ControlSignal::thermal_release());
            assert_eq!(
                requests[0].target,
                DispatchTarget::ByName("Gas Furnace".into())
            );

            requests.clear();
            actor.decide(&env, &mut requests);
            assert!(requests.is_empty(), "{action:?}: released once");
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

        // Cold zone — HVAC zone freeze guard and should_reject_dispatch only apply to
        // ByName targets or ByEndUse HVAC end-uses; ByEndUse WATER_HEATING bypasses both.
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
            .with_hvac_target(DispatchTarget::ByEndUse(EndUse::HVAC_HEATING))
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

    #[test]
    fn freeze_guard_telemetry_resets_when_hvac_target_is_none() {
        // Regression: when hvac_target is None, dr_freeze_guard was only
        // written inside the load-targets loop, never reset at the top of the
        // complied path. This test verifies that the field reflects the
        // current step's guard status across successive decide() calls.
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_load_target(
                DispatchTarget::ByEndUse(EndUse::HVAC_HEATING),
                DrAction::off(),
            );

        actor.set_dr_level(DRLevel::Critical);

        // Step 1: cold zone — guard fires on HVAC load target
        let env_cold = test_env().zone_temp(2.0).build();
        let mut requests = Vec::new();
        actor.decide(&env_cold, &mut requests);
        assert_eq!(actor.telemetry.get("dr_freeze_guard"), Some(1.0));

        // Step 2: warm zone — guard does NOT fire, telemetry must reset
        let env_warm = test_env().zone_temp(22.0).build();
        let mut requests = Vec::new();
        actor.decide(&env_warm, &mut requests);
        assert_eq!(
            actor.telemetry.get("dr_freeze_guard"),
            Some(0.0),
            "dr_freeze_guard must reset to 0.0 in warm step; before the fix it would retain 1.0 from step 1"
        );
    }

    // -----------------------------------------------------------------------
    // DR clear-signal tests (T-0163: sticky DR control signals)
    // -----------------------------------------------------------------------

    #[test]
    fn power_limit_cleared_on_normal_transition() {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_load_target(
                DispatchTarget::ByName("HPWH".into()),
                DrAction::limit_power(3.0),
            );

        // Step 1: DR High — dispatch power limit
        actor.set_dr_level(DRLevel::High);
        let env = test_env().build();
        let mut requests = Vec::new();
        actor.decide(&env, &mut requests);
        assert_eq!(requests.len(), 1);
        assert!(matches!(
            &requests[0].signal,
            ControlSignal::PowerLimit { max_power_kw, .. } if (*max_power_kw - 3.0).abs() < 0.01
        ));

        // Step 2: DR Normal — clear signal dispatched
        actor.set_dr_level(DRLevel::Normal);
        let env2 = test_env().build();
        let mut requests2 = Vec::new();
        actor.decide(&env2, &mut requests2);
        assert_eq!(
            requests2.len(),
            1,
            "must dispatch one clear signal for PowerLimit"
        );
        assert_eq!(requests2[0].priority, PriorityTier::Grid);
        assert_eq!(requests2[0].target, DispatchTarget::ByName("HPWH".into()));
        match &requests2[0].signal {
            ControlSignal::PowerLimit {
                max_power_kw,
                ramp_rate_kw_per_s,
            } => {
                assert!(
                    max_power_kw.is_infinite() && *max_power_kw > 0.0,
                    "clear signal must use INFINITY power limit, got {max_power_kw}"
                );
                assert_eq!(*ramp_rate_kw_per_s, None);
            }
            other => panic!("expected PowerLimit clear signal, got {other:?}"),
        }
    }

    #[test]
    fn multiple_power_limit_targets_all_cleared() {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_load_target(
                DispatchTarget::ByName("HPWH".into()),
                DrAction::limit_power(3.0),
            )
            .with_load_target(
                DispatchTarget::ByName("EV".into()),
                DrAction::limit_power(5.0),
            );

        // Dispatch both power limits during DR High
        actor.set_dr_level(DRLevel::High);
        let env = test_env().build();
        let mut requests = Vec::new();
        actor.decide(&env, &mut requests);
        assert_eq!(requests.len(), 2);

        // Transition to Normal — both should be cleared
        actor.set_dr_level(DRLevel::Normal);
        let env2 = test_env().build();
        let mut requests2 = Vec::new();
        actor.decide(&env2, &mut requests2);
        assert_eq!(
            requests2.len(),
            2,
            "both PowerLimit targets must be cleared"
        );

        let targets: Vec<&DispatchTarget> = requests2.iter().map(|r| &r.target).collect();
        assert!(targets.contains(&&DispatchTarget::ByName("HPWH".into())));
        assert!(targets.contains(&&DispatchTarget::ByName("EV".into())));
        for req in &requests2 {
            assert!(matches!(
                &req.signal,
                ControlSignal::PowerLimit { max_power_kw, .. } if max_power_kw.is_infinite() && *max_power_kw > 0.0
            ));
        }
    }

    #[test]
    fn no_clear_signals_when_no_prior_sticky_dispatch() {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_load_target(
                DispatchTarget::ByEndUse(EndUse::PLUG_LOADS),
                DrAction::curtail(0.5),
            );

        // Dispatch LoadCurtailment — not sticky (LoadFraction auto-resets)
        actor.set_dr_level(DRLevel::High);
        let env = test_env().build();
        let mut requests = Vec::new();
        actor.decide(&env, &mut requests);
        assert_eq!(requests.len(), 1);
        assert!(matches!(
            &requests[0].signal,
            ControlSignal::LoadFraction { .. }
        ));

        // Transition to Normal — no clear signals needed
        actor.set_dr_level(DRLevel::Normal);
        let env2 = test_env().build();
        let mut requests2 = Vec::new();
        actor.decide(&env2, &mut requests2);
        assert!(
            requests2.is_empty(),
            "non-sticky signals (LoadFraction) must not trigger clears"
        );
    }

    #[test]
    fn turnoff_modeoverride_dispatched_but_not_cleared_on_normal() {
        // ModeOverride::Off (from TurnOff) is sticky on equipment but cannot
        // be cleared via the current ControlSignal::ModeOverride API:
        // OperatingMode::Auto does not exist, and setting any Other mode
        // still sets Some(mode) rather than clearing to None.
        // This test verifies the actor tracks TurnOff but does not emit a
        // false-positive clearing signal (no OperatingMode::Auto exists).
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(DispatchTarget::ByName("HVAC".into()))
            .with_hvac_action(DrAction::off());

        // Step 1: DR Critical — dispatch TurnOff
        actor.set_dr_level(DRLevel::Critical);
        let env = test_env().build();
        let mut requests = Vec::new();
        actor.decide(&env, &mut requests);
        assert_eq!(requests.len(), 1);
        assert!(matches!(
            &requests[0].signal,
            ControlSignal::ModeOverride {
                mode: OperatingMode::Off
            }
        ));

        // Step 2: DR Normal — no clearing signal for ModeOverride
        // (PowerLimit { INFINITY } would be dispatched if PowerLimit was tracked,
        // but TurnOff tracking only logs, does not dispatch)
        actor.set_dr_level(DRLevel::Normal);
        let env2 = test_env().build();
        let mut requests2 = Vec::new();
        actor.decide(&env2, &mut requests2);
        assert!(
            requests2.is_empty(),
            "TurnOff ModeOverride must not emit a false clearing signal"
        );
    }

    #[test]
    fn no_clear_signals_on_consecutive_normal_calls() {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_load_target(
                DispatchTarget::ByName("EV".into()),
                DrAction::limit_power(4.0),
            );

        // Dispatch during DR High
        actor.set_dr_level(DRLevel::High);
        let env = test_env().build();
        let mut requests = Vec::new();
        actor.decide(&env, &mut requests);

        // First Normal call — clears dispatched
        actor.set_dr_level(DRLevel::Normal);
        let env2 = test_env().build();
        let mut requests2 = Vec::new();
        actor.decide(&env2, &mut requests2);
        assert_eq!(requests2.len(), 1, "first Normal call must dispatch clear");

        // Second Normal call — no more clear signals
        let env3 = test_env().build();
        let mut requests3 = Vec::new();
        actor.decide(&env3, &mut requests3);
        assert!(
            requests3.is_empty(),
            "consecutive Normal calls without intervening DR must not re-dispatch clears"
        );
    }

    #[test]
    fn telemetry_signals_count_reflects_clear_signals_on_normal() {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_load_target(
                DispatchTarget::ByName("HPWH".into()),
                DrAction::limit_power(3.0),
            );

        // Active DR — signals count reflects dispatched signals
        actor.set_dr_level(DRLevel::Critical);
        let env = test_env().build();
        let mut requests = Vec::new();
        actor.decide(&env, &mut requests);
        assert_eq!(actor.telemetry.get("signals_count"), Some(1.0));

        // Normal — signals count reflects clear signals (not 0)
        actor.set_dr_level(DRLevel::Normal);
        let env2 = test_env().build();
        let mut requests2 = Vec::new();
        actor.decide(&env2, &mut requests2);
        assert_eq!(
            actor.telemetry.get("signals_count"),
            Some(1.0),
            "signals_count on Normal must reflect clear signal count"
        );
    }

    #[test]
    fn telemetry_signals_count_zero_on_normal_with_no_prior_dispatch() {
        let mut actor = DrCompliance::new("Test").with_compliance_model(AlwaysComply);

        // No prior dispatch — signals_count must be 0 on Normal
        actor.set_dr_level(DRLevel::Normal);
        let env = test_env().build();
        let mut requests = Vec::new();
        actor.decide(&env, &mut requests);
        assert_eq!(actor.telemetry.get("signals_count"), Some(0.0));
        assert!(requests.is_empty());
    }

    #[test]
    fn telemetry_clear_signals_dispatched_count_key_pre_registered() {
        // Regression: Telemetry::set() panics for unknown keys. Verify
        // clear_signals_dispatched_count is pre-registered at construction
        // so that decide() under #[cfg(feature = "observe")] does not panic.
        let actor = DrCompliance::new("Test");
        assert_eq!(
            actor.telemetry.get("clear_signals_dispatched_count"),
            Some(0.0),
            "clear_signals_dispatched_count must be pre-registered at init"
        );
    }

    #[test]
    fn power_limit_target_dedup_emits_single_clear_signal() {
        // When the same target receives PowerLimit via both hvac and
        // load target paths, dedup ensures only one clear signal is dispatched.
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(DispatchTarget::ByName("HPWH".into()))
            .with_hvac_action(DrAction::limit_power(2.0))
            .with_load_target(
                DispatchTarget::ByName("HPWH".into()),
                DrAction::limit_power(3.0),
            );

        // Step 1: DR High — dispatches two PowerLimit signals (hvac + load)
        actor.set_dr_level(DRLevel::High);
        let env = test_env().build();
        let mut requests = Vec::new();
        actor.decide(&env, &mut requests);
        assert_eq!(requests.len(), 2);

        // Step 2: DR Normal — only one clear signal (dedup works)
        actor.set_dr_level(DRLevel::Normal);
        let mut requests2 = Vec::new();
        actor.decide(&env, &mut requests2);
        assert_eq!(
            requests2.len(),
            1,
            "dedup should emit exactly one clear signal for the same target"
        );
        assert!(
            matches!(
                &requests2[0].signal,
                ControlSignal::PowerLimit { max_power_kw, .. }
                if max_power_kw.is_infinite() && *max_power_kw > 0.0
            ),
            "clear signal must be PowerLimit with INFINITY"
        );
    }

    #[test]
    fn dr_level_normal_after_active_dr_emits_clears_not_original_signals() {
        // Regression: after DR ends, only clear signals should be dispatched,
        // not the original DR actions re-dispatched.
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(DispatchTarget::ByName("HVAC".into()))
            .with_hvac_action(DrAction::off())
            .with_load_target(
                DispatchTarget::ByName("HPWH".into()),
                DrAction::limit_power(3.0),
            );

        // Active DR
        actor.set_dr_level(DRLevel::Critical);
        let env = test_env().build();
        let mut requests = Vec::new();
        actor.decide(&env, &mut requests);
        assert_eq!(requests.len(), 2);

        // Normal — only PowerLimit clear (TurnOff can't be cleared)
        actor.set_dr_level(DRLevel::Normal);
        let mut requests2 = Vec::new();
        actor.decide(&env, &mut requests2);
        assert_eq!(requests2.len(), 1, "only PowerLimit clear dispatched");
        assert!(
            matches!(
                &requests2[0].signal,
                ControlSignal::PowerLimit { max_power_kw, .. } if max_power_kw.is_infinite() && *max_power_kw > 0.0
            ),
            "must dispatch PowerLimit INFINITY clear, not original DR signal"
        );
    }

    // -----------------------------------------------------------------------
    // Pre-dispatch protected-state validation (T-0166)
    // -----------------------------------------------------------------------

    #[test]
    fn power_limit_rejected_when_equipment_in_defrost() {
        let mut telem = Telemetry::with_capacity(2);
        telem.insert(tk::DEFROST_ACTIVE, 1.0);

        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_load_target(
                DispatchTarget::ByName("HPWH".into()),
                DrAction::limit_power(3.0),
            );

        actor.set_dr_level(DRLevel::High);

        let env = test_env().with_equipment_telemetry("HPWH", telem).build();
        let mut requests = Vec::new();

        actor.decide(&env, &mut requests);

        assert!(
            requests.is_empty(),
            "PowerLimit must be rejected when equipment in defrost"
        );
        assert_eq!(actor.telemetry.get("signals_count"), Some(0.0));
        assert_eq!(actor.telemetry.get("signals_rejected"), Some(1.0));
    }

    #[test]
    fn mode_override_off_rejected_when_equipment_in_defrost() {
        let mut telem = Telemetry::with_capacity(2);
        telem.insert(tk::DEFROST_ACTIVE, 1.0);

        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(DispatchTarget::ByName("HVAC".into()))
            .with_hvac_action(DrAction::off());

        actor.set_dr_level(DRLevel::Critical);

        let env = test_env().with_equipment_telemetry("HVAC", telem).build();
        let mut requests = Vec::new();

        actor.decide(&env, &mut requests);

        assert!(
            requests.is_empty(),
            "ModeOverride(Off) must be rejected when equipment in defrost"
        );
        assert_eq!(actor.telemetry.get("signals_count"), Some(0.0));
        assert_eq!(actor.telemetry.get("signals_rejected"), Some(1.0));
    }

    #[test]
    fn power_limit_dispatched_when_equipment_not_in_defrost() {
        let mut telem = Telemetry::with_capacity(2);
        telem.insert(tk::DEFROST_ACTIVE, 0.0);
        telem.insert(tk::ELECTRIC_KW, 1.5);

        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_load_target(
                DispatchTarget::ByName("HPWH".into()),
                DrAction::limit_power(3.0),
            );

        actor.set_dr_level(DRLevel::High);

        let env = test_env().with_equipment_telemetry("HPWH", telem).build();
        let mut requests = Vec::new();

        actor.decide(&env, &mut requests);

        assert_eq!(
            requests.len(),
            1,
            "PowerLimit must be dispatched when equipment is not in defrost"
        );
        assert!(matches!(
            &requests[0].signal,
            ControlSignal::PowerLimit { max_power_kw, .. } if (*max_power_kw - 3.0).abs() < 0.01
        ));
        assert_eq!(actor.telemetry.get("signals_count"), Some(1.0));
        assert_eq!(actor.telemetry.get("signals_rejected"), Some(0.0));
    }

    #[test]
    fn turn_off_rejected_when_wh_tank_below_freeze_threshold() {
        // WH_FREEZE_THRESHOLD_C = 5.0°C. Tank at 3.0°C means freeze
        // protection must block TurnOff.
        let mut telem = Telemetry::with_capacity(2);
        telem.insert(tk::TANK_AVG_TEMP_C, 3.0);

        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_load_target(DispatchTarget::ByName("HPWH".into()), DrAction::off());

        actor.set_dr_level(DRLevel::Critical);

        let env = test_env().with_equipment_telemetry("HPWH", telem).build();
        let mut requests = Vec::new();

        actor.decide(&env, &mut requests);

        assert!(
            requests.is_empty(),
            "TurnOff must be rejected when WH tank temp is below freeze threshold"
        );
        assert_eq!(actor.telemetry.get("signals_count"), Some(0.0));
        assert_eq!(actor.telemetry.get("signals_rejected"), Some(1.0));
    }

    #[test]
    fn turn_off_proceeds_when_wh_tank_above_freeze_threshold() {
        let mut telem = Telemetry::with_capacity(2);
        telem.insert(tk::TANK_AVG_TEMP_C, 45.0);

        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_load_target(DispatchTarget::ByName("HPWH".into()), DrAction::off());

        actor.set_dr_level(DRLevel::Critical);

        let env = test_env().with_equipment_telemetry("HPWH", telem).build();
        let mut requests = Vec::new();

        actor.decide(&env, &mut requests);

        assert_eq!(
            requests.len(),
            1,
            "TurnOff must be dispatched when WH tank temp is above freeze threshold"
        );
        assert!(matches!(
            &requests[0].signal,
            ControlSignal::ModeOverride {
                mode: OperatingMode::Off
            }
        ));
        assert_eq!(actor.telemetry.get("signals_count"), Some(1.0));
        assert_eq!(actor.telemetry.get("signals_rejected"), Some(0.0));
    }

    #[test]
    fn rejection_does_not_apply_to_by_end_use_targets() {
        let mut telem = Telemetry::with_capacity(2);
        telem.insert(tk::DEFROST_ACTIVE, 1.0);

        // ByEndUse targets are resolved at dispatch time; the actor cannot
        // look up individual equipment state. Protected-state checks skip
        // ByEndUse targets — validation happens downstream in the dwelling
        // dispatch loop.
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_load_target(
                DispatchTarget::ByEndUse(EndUse::WATER_HEATING),
                DrAction::limit_power(2.0),
            );

        actor.set_dr_level(DRLevel::High);

        let env = test_env().with_equipment_telemetry("HPWH", telem).build();
        let mut requests = Vec::new();

        actor.decide(&env, &mut requests);

        assert_eq!(
            requests.len(),
            1,
            "ByEndUse targets bypass actor-side protected-state check"
        );
        assert_eq!(actor.telemetry.get("signals_rejected"), Some(0.0));
    }

    #[test]
    fn signals_rejected_pre_registered_at_init() {
        let actor = DrCompliance::new("Test");
        assert_eq!(
            actor.telemetry.get("signals_rejected"),
            Some(0.0),
            "signals_rejected must be pre-registered at init"
        );
    }

    #[test]
    fn signals_rejected_resets_each_step() {
        // First step: equipment in defrost → PowerLimit rejected
        let mut telem_defrost = Telemetry::with_capacity(2);
        telem_defrost.insert(tk::DEFROST_ACTIVE, 1.0);

        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_load_target(
                DispatchTarget::ByName("HVAC".into()),
                DrAction::limit_power(3.0),
            );

        actor.set_dr_level(DRLevel::High);
        let env_defrost = test_env()
            .with_equipment_telemetry("HVAC", telem_defrost)
            .build();
        let mut requests = Vec::new();
        actor.decide(&env_defrost, &mut requests);
        assert_eq!(actor.telemetry.get("signals_rejected"), Some(1.0));

        // Second step: equipment not in defrost → signal dispatched, rejected resets
        let mut telem_normal = Telemetry::with_capacity(2);
        telem_normal.insert(tk::DEFROST_ACTIVE, 0.0);

        let env_normal = test_env()
            .with_equipment_telemetry("HVAC", telem_normal)
            .build();
        let mut requests2 = Vec::new();
        actor.decide(&env_normal, &mut requests2);

        assert_eq!(requests2.len(), 1);
        assert_eq!(actor.telemetry.get("signals_rejected"), Some(0.0));
        assert_eq!(actor.telemetry.get("signals_count"), Some(1.0));
    }

    #[test]
    fn mixed_rejected_and_dispatched_signals_tracked_separately() {
        // HPWH in defrost → PowerLimit rejected
        // EV normal → PowerLimit dispatched
        let mut telem_hpwh = Telemetry::with_capacity(2);
        telem_hpwh.insert(tk::DEFROST_ACTIVE, 1.0);
        let mut telem_ev = Telemetry::with_capacity(2);
        telem_ev.insert(tk::DEFROST_ACTIVE, 0.0);
        telem_ev.insert(tk::ELECTRIC_KW, 2.0);

        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_load_target(
                DispatchTarget::ByName("HPWH".into()),
                DrAction::limit_power(3.0),
            )
            .with_load_target(
                DispatchTarget::ByName("EV".into()),
                DrAction::limit_power(5.0),
            );

        actor.set_dr_level(DRLevel::High);

        let env = test_env()
            .with_equipment_telemetry("HPWH", telem_hpwh)
            .with_equipment_telemetry("EV", telem_ev)
            .build();
        let mut requests = Vec::new();

        actor.decide(&env, &mut requests);

        assert_eq!(requests.len(), 1, "one signal dispatched, one rejected");
        assert_eq!(requests[0].target, DispatchTarget::ByName("EV".into()));
        assert!(matches!(
            &requests[0].signal,
            ControlSignal::PowerLimit { max_power_kw, .. } if (*max_power_kw - 5.0).abs() < 0.01
        ));
        assert_eq!(actor.telemetry.get("signals_count"), Some(1.0));
        assert_eq!(actor.telemetry.get("signals_rejected"), Some(1.0));
        assert!(
            requests
                .iter()
                .all(|r| !matches!(&r.target, DispatchTarget::ByName(n) if &**n == "HPWH")),
            "rejected HPWH signal must not appear in dispatch output"
        );
    }

    // -----------------------------------------------------------------------
    // DemandResponse dispatch (T-0167)
    // -----------------------------------------------------------------------

    #[test]
    fn dr_action_demand_response_constructor() {
        let action = DrAction::demand_response(DRLevel::High, Some(3600.0));
        assert_eq!(
            action,
            DrAction::DemandResponse {
                level: DRLevel::High,
                duration_s: Some(3600.0)
            }
        );
    }

    #[test]
    fn dr_action_demand_response_indefinite_duration() {
        let action = DrAction::demand_response(DRLevel::Critical, None);
        assert_eq!(
            action,
            DrAction::DemandResponse {
                level: DRLevel::Critical,
                duration_s: None
            }
        );
    }

    #[test]
    fn dispatch_demand_response_emits_control_signal() {
        let target = DispatchTarget::ByName("HVAC".into());
        let action = DrAction::DemandResponse {
            level: DRLevel::High,
            duration_s: Some(3600.0),
        };
        let mut out = Vec::new();
        assert!(DrCompliance::new("Test").dispatch_for_action(
            &target,
            &action,
            Ok(None),
            &mut out
        ));

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].target, target);
        assert_eq!(out[0].priority, PriorityTier::Grid);
        match &out[0].signal {
            ControlSignal::DemandResponse { level, duration_s } => {
                assert_eq!(*level, DRLevel::High);
                assert_eq!(*duration_s, Some(3600.0));
            }
            other => panic!("expected DemandResponse, got {other:?}"),
        }
    }

    #[test]
    fn demand_response_via_actor_emits_correct_signal() {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(DispatchTarget::ByName("HVAC".into()))
            .with_hvac_action(DrAction::demand_response(DRLevel::Critical, Some(7200.0)));

        actor.set_dr_level(DRLevel::Critical);

        let env = test_env().build();
        let mut requests = Vec::new();
        actor.decide(&env, &mut requests);

        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].priority, PriorityTier::Grid);
        match &requests[0].signal {
            ControlSignal::DemandResponse { level, duration_s } => {
                assert_eq!(*level, DRLevel::Critical);
                assert_eq!(*duration_s, Some(7200.0));
            }
            other => panic!("expected DemandResponse, got {other:?}"),
        }
    }

    #[test]
    fn demand_response_not_cleared_on_normal_transition() {
        // DemandResponse auto-reverts via equipment-side duration expiry;
        // no actor-side clear signal is needed or emitted.
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(DispatchTarget::ByName("HVAC".into()))
            .with_hvac_action(DrAction::demand_response(DRLevel::High, Some(3600.0)));

        // Step 1: dispatch DemandResponse during DR
        actor.set_dr_level(DRLevel::High);
        let env = test_env().build();
        let mut requests = Vec::new();
        actor.decide(&env, &mut requests);
        assert_eq!(requests.len(), 1);

        // Step 2: Normal — no clear signal (DemandResponse is not sticky)
        actor.set_dr_level(DRLevel::Normal);
        let mut requests2 = Vec::new();
        actor.decide(&env, &mut requests2);
        assert!(
            requests2.is_empty(),
            "DemandResponse auto-reverts on equipment; no actor-side clear needed"
        );
    }

    #[test]
    fn demand_response_as_load_target_dispatches() {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_load_target(
                DispatchTarget::ByEndUse(EndUse::HVAC_HEATING),
                DrAction::demand_response(DRLevel::High, Some(1800.0)),
            );

        actor.set_dr_level(DRLevel::High);

        let env = test_env().build();
        let mut requests = Vec::new();
        actor.decide(&env, &mut requests);

        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].target,
            DispatchTarget::ByEndUse(EndUse::HVAC_HEATING)
        );
        match &requests[0].signal {
            ControlSignal::DemandResponse { level, duration_s } => {
                assert_eq!(*level, DRLevel::High);
                assert_eq!(*duration_s, Some(1800.0));
            }
            other => panic!("expected DemandResponse, got {other:?}"),
        }
    }

    #[test]
    fn demand_response_not_blocked_by_freeze_guard() {
        // DemandResponse is not TurnOff — freeze guard does not apply.
        // Equipment applies its own DR-level behaviour, which may
        // reduce but not shut off heating.
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(DispatchTarget::ByName("HVAC".into()))
            .with_hvac_action(DrAction::demand_response(DRLevel::High, Some(3600.0)));

        actor.set_dr_level(DRLevel::Critical);

        // Cold zone — freeze guard should NOT fire on DemandResponse
        let env = test_env().zone_temp(2.0).build();
        let mut requests = Vec::new();
        actor.decide(&env, &mut requests);

        assert_eq!(requests.len(), 1);
        assert!(
            matches!(&requests[0].signal, ControlSignal::DemandResponse { .. }),
            "DemandResponse must not be blocked or downgraded by freeze guard"
        );
        assert_eq!(actor.telemetry.get("dr_freeze_guard"), Some(0.0));
    }

    #[test]
    fn demand_response_bypasses_protected_state_check() {
        // DemandResponse is not PowerLimit or TurnOff — it bypasses
        // the should_reject_dispatch check for defrost/WH freeze.
        let mut telem = Telemetry::with_capacity(2);
        telem.insert(tk::DEFROST_ACTIVE, 1.0);

        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_load_target(
                DispatchTarget::ByName("HVAC".into()),
                DrAction::demand_response(DRLevel::Moderate, None),
            );

        actor.set_dr_level(DRLevel::High);

        let env = test_env().with_equipment_telemetry("HVAC", telem).build();
        let mut requests = Vec::new();
        actor.decide(&env, &mut requests);

        assert_eq!(
            requests.len(),
            1,
            "DemandResponse must not be rejected when equipment in defrost"
        );
        assert!(matches!(
            &requests[0].signal,
            ControlSignal::DemandResponse { .. }
        ));
        assert_eq!(actor.telemetry.get("signals_rejected"), Some(0.0));
    }

    #[test]
    fn demand_response_telemetry_keys_pre_registered() {
        let actor = DrCompliance::new("Test");
        assert_eq!(
            actor.telemetry.get("demand_response_level"),
            Some(0.0),
            "demand_response_level must be pre-registered at init"
        );
        assert_eq!(
            actor.telemetry.get("demand_response_duration_s"),
            Some(0.0),
            "demand_response_duration_s must be pre-registered at init (0.0 = no DR event)"
        );
    }

    #[test]
    fn save_state_load_state_round_trip_dr_level_preserved() {
        let mut actor = DrCompliance::new("Test")
            .with_compliance_model(AlwaysComply)
            .with_hvac_target(DispatchTarget::ByName("HVAC".into()))
            .with_hvac_action(DrAction::setpoint_delta(2.0));
        actor.set_dr_level(DRLevel::High);
        actor.last_dispatched.push((
            DispatchTarget::ByName("HVAC".into()),
            DrAction::PowerLimit { max_kw: 3.0 },
        ));

        let blob = actor.save_state().expect("save_state should succeed");
        assert!(
            !blob.is_empty(),
            "stateful actor must produce non-empty blob"
        );

        let mut restored = DrCompliance::new("Test");
        restored
            .load_state(&blob)
            .expect("load_state should succeed");

        assert_eq!(restored.current_dr_level, DRLevel::High);
        assert_eq!(restored.last_dispatched.len(), 1);
        assert_eq!(
            restored.last_dispatched[0].1,
            DrAction::PowerLimit { max_kw: 3.0 }
        );
    }
}
