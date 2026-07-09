//! ChargingComposer -- evaluates preferences and resolves to a DispatchRequest.

use std::sync::Arc;

use hares_control::{DispatchRequest, DispatchTarget, PriorityTier};
use hares_types::ControlSignal;

use super::preference::{ChargingPreference, Constraint, DecisionContext, PreferenceVote};

/// Evaluates a set of ChargingPreference instances and resolves
/// conflicting votes into a single DispatchRequest per timestep.
pub struct ChargingComposer {
    preferences: Vec<Box<dyn ChargingPreference>>,
    scratch: Vec<PreferenceVote>,
    dispatch_target: DispatchTarget,
    last_action: String,
    last_needed_charge_hours: f64,
}

impl ChargingComposer {
    pub fn new(preferences: Vec<Box<dyn ChargingPreference>>, target_name: &str) -> Self {
        Self {
            preferences,
            scratch: Vec::with_capacity(8),
            dispatch_target: DispatchTarget::ByName(Arc::from(target_name)),
            last_action: String::new(),
            last_needed_charge_hours: f64::INFINITY,
        }
    }

    pub fn last_action(&self) -> &str {
        &self.last_action
    }

    /// Whether the SocGate preference (if present) currently allows charging.
    /// Returns `true` if no SocGate is in the preference stack.
    pub fn soc_gate_charging_allowed(&self) -> bool {
        self.preferences
            .iter()
            .filter_map(|p| p.charging_allowed())
            .next()
            .unwrap_or(true)
    }

    /// The most recent needed-charge-hours estimate from preferences
    /// (minimum across all preferences). Returns `0.0` if no preference
    /// provides an estimate (the fold starts at `f64::INFINITY` and
    /// collapses to the minimum; the getter maps non-finite results to
    /// `0.0` so the telemetry value is always finite).
    pub fn last_needed_charge_hours(&self) -> f64 {
        if self.last_needed_charge_hours.is_finite() {
            self.last_needed_charge_hours
        } else {
            0.0
        }
    }

    /// Evaluate all preferences and emit a resolved DispatchRequest.
    ///
    /// 1. Check constraints -- first Override wins.
    /// 2. Collect scored votes.
    /// 3. Resolve: highest-scored target_soc, most conservative power_kw,
    ///    max min_soc, earliest departure.
    /// 4. Translate to ControlSignal.
    pub fn evaluate(&mut self, ctx: &DecisionContext, out: &mut Vec<DispatchRequest>) {
        // Capture needed charge hours for telemetry (minimum across all preferences)
        self.last_needed_charge_hours = self
            .preferences
            .iter()
            .map(|p| p.needed_charge_hours(ctx))
            .fold(f64::INFINITY, f64::min);

        // Check constraints first
        for pref in &mut self.preferences {
            if let Constraint::Override(vote) = pref.constraint(ctx) {
                self.last_action.clear();
                self.last_action.push_str("override:");
                self.last_action.push_str(pref.name());
                self.last_action.push(':');
                self.last_action.push_str(vote.label);
                self.emit_vote(ctx, &vote, out);
                return;
            }
        }

        // Collect scored votes
        self.scratch.clear();
        for pref in &mut self.preferences {
            let vote = pref.score(ctx);
            tracing::trace!(
                preference = pref.name(),
                score = vote.score,
                label = vote.label,
                "preference scored"
            );
            self.scratch.push(vote);
        }

        if self.scratch.is_empty() {
            self.last_action.clear();
            self.last_action.push_str("idle:no_preferences");
            return;
        }

        let resolved = Self::resolve(&self.scratch);
        self.last_action.clear();
        self.last_action.push_str("resolved:");
        self.last_action.push_str(resolved.label);
        self.emit_vote(ctx, &resolved, out);
    }

    /// Resolve multiple votes into a single action.
    fn resolve(votes: &[PreferenceVote]) -> PreferenceVote {
        let mut best_score = f64::NEG_INFINITY;
        let mut best_target_soc: Option<f64> = None;
        let mut best_label: &str = "idle";
        let mut min_power_kw: Option<f64> = None;
        let mut max_min_soc: Option<f64> = None;
        let mut max_max_soc: Option<f64> = None;
        let mut earliest_departure: Option<f64> = None;

        for vote in votes {
            // target_soc from highest-scored vote
            if vote.score > best_score {
                best_score = vote.score;
                if vote.target_soc.is_some() {
                    best_target_soc = vote.target_soc;
                }
                best_label = vote.label;
            }

            // power_kw: most conservative (smallest absolute value, preserving sign)
            if let Some(p) = vote.power_kw {
                min_power_kw = Some(match min_power_kw {
                    Some(existing) => {
                        if p.abs() < existing.abs() {
                            p
                        } else {
                            existing
                        }
                    }
                    None => p,
                });
            }

            // min_soc: take max (most restrictive lower bound)
            if let Some(m) = vote.min_soc {
                max_min_soc = Some(match max_min_soc {
                    Some(existing) => existing.max(m),
                    None => m,
                });
            }

            // max_soc: take min (most restrictive upper bound)
            if let Some(m) = vote.max_soc {
                max_max_soc = Some(match max_max_soc {
                    Some(existing) => existing.min(m),
                    None => m,
                });
            }

            // departure: take earliest
            if let Some(d) = vote.departure_hour {
                earliest_departure = Some(match earliest_departure {
                    Some(existing) => existing.min(d),
                    None => d,
                });
            }
        }

        PreferenceVote {
            target_soc: best_target_soc,
            power_kw: min_power_kw,
            departure_hour: earliest_departure,
            min_soc: max_min_soc,
            max_soc: max_max_soc,
            score: best_score,
            label: best_label,
        }
    }

    /// Emits dispatch requests from a preference vote.
    ///
    /// All signals use `Schedule` tier — matches the central mapping for
    /// `EvSetReadyBy`, `PowerSetpoint`, and `SOCTarget`. The charging
    /// composer operates within the EV driver's schedule-level framework.
    ///
    /// When a vote carries both `departure_hour`/`target_soc` and `power_kw`
    /// (e.g. an urgency Override from [`super::departure::DepartureDeadline`]),
    /// both `EvSetReadyBy` and `PowerSetpoint` are emitted so the equipment
    /// receives the scheduler's full intent rather than having the deadline
    /// signal silently discard the rate.
    fn emit_vote(
        &self,
        ctx: &DecisionContext,
        vote: &PreferenceVote,
        out: &mut Vec<DispatchRequest>,
    ) {
        let mut emitted_ready_by = false;

        // If there's a departure_hour + target_soc, use EvSetReadyBy
        if let (Some(departure), Some(target)) = (vote.departure_hour, vote.target_soc) {
            out.push(DispatchRequest {
                target: self.dispatch_target.clone(),
                signal: ControlSignal::EvSetReadyBy {
                    departure_hour: departure,
                    target_soc: target,
                },
                priority: PriorityTier::Schedule,
            });
            emitted_ready_by = true;
        }

        // If there's a power_kw, use PowerSetpoint
        if let Some(unclamped) = vote.power_kw {
            if unclamped.abs() < 1e-9 {
                // Zero power = idle, no dispatch needed
                return;
            }

            // Defense-in-depth: clamp power against equipment context limits.
            // Individual strategies self-clamp, but this output-boundary clamp
            // catches unclamped custom preferences and strategy bugs.
            // OCHRE reference: EV.py:296-300 centralizes power clamping at the
            // equipment level.
            let power = if unclamped > 0.0 {
                unclamped.min(ctx.max_charge_kw)
            } else {
                unclamped.max(-ctx.max_discharge_kw)
            };

            if (power - unclamped).abs() > 1e-9 {
                tracing::warn!(
                    original_kw = unclamped,
                    clamped_kw = power,
                    preference = vote.label,
                    max_charge_kw = ctx.max_charge_kw,
                    max_discharge_kw = ctx.max_discharge_kw,
                    "PowerSetpoint clamped to context limit",
                );
            }

            #[cfg(feature = "observe")]
            {
                tracing::debug!(
                    power_before_clamp = unclamped,
                    power_after_clamp = power,
                    preference = vote.label,
                    "PowerSetpoint clamp applied",
                );
            }

            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            {
                // Invariant: when a strategy sets min_soc or max_soc on a discharge
                // vote, the emitted PowerSetpoint must carry that constraint. Strategies
                // that discharge without a SOC floor (e.g. pure price-based TOU) are valid.
                // This check confirms the forwarding code below is not dropping the field.
                if power < 0.0 {
                    if let Some(ms) = vote.min_soc {
                        debug_assert!(
                            (0.0..=1.0).contains(&ms),
                            "min_soc must be in [0, 1], got {ms}"
                        );
                    }
                    if let Some(ms) = vote.max_soc {
                        debug_assert!(
                            (0.0..=1.0).contains(&ms),
                            "max_soc must be in [0, 1], got {ms}"
                        );
                    }
                }
            }
            #[cfg(feature = "observe")]
            {
                if vote.min_soc.is_some() || vote.max_soc.is_some() {
                    tracing::debug!(
                        power_kw = power,
                        min_soc = vote.min_soc,
                        max_soc = vote.max_soc,
                        "PowerSetpoint carries SOC constraint",
                    );
                }
            }
            #[cfg(feature = "observe")]
            {
                tracing::debug!(
                    ev_override_power_propagated = emitted_ready_by,
                    power_kw = power,
                    preference = vote.label,
                    "PowerSetpoint dispatch with Ready-By coexistence",
                );
            }
            out.push(DispatchRequest {
                target: self.dispatch_target.clone(),
                signal: ControlSignal::PowerSetpoint {
                    active_power_kw: power,
                    reactive_power_kvar: None,
                    min_soc: vote.min_soc,
                    max_soc: vote.max_soc,
                },
                priority: PriorityTier::Schedule,
            });
            return;
        }

        // If Ready-By was already emitted and there is no power_kw, we are done.
        if emitted_ready_by {
            return;
        }

        // If there's a target_soc, use SOCTarget
        if let Some(target) = vote.target_soc {
            out.push(DispatchRequest {
                target: self.dispatch_target.clone(),
                signal: ControlSignal::SOCTarget {
                    target_soc: target,
                    min_soc: vote.min_soc,
                    max_soc: vote.max_soc,
                },
                priority: PriorityTier::Schedule,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::testing::TestEnvBuilder;
    use hares_types::{DayFilter, DepartureConstraint};

    fn make_ctx(env: &hares_types::EnvironmentState) -> DecisionContext<'_> {
        DecisionContext {
            current_soc: 0.5,
            capacity_kwh: 60.0,
            max_charge_kw: 7.2,
            max_discharge_kw: 5.0,
            env,
            current_minute: 720,
            next_departure_minute: None,
            time_res_minutes: 1.0,
        }
    }

    /// A test preference that always returns Override.
    struct OverridePref {
        vote: PreferenceVote,
    }

    impl ChargingPreference for OverridePref {
        fn constraint(&mut self, _ctx: &DecisionContext) -> Constraint {
            Constraint::Override(self.vote.clone())
        }
        fn score(&mut self, _ctx: &DecisionContext) -> PreferenceVote {
            unreachable!("constraint should short-circuit")
        }
        fn name(&self) -> &'static str {
            "override_test"
        }
    }

    /// A test preference that scores with configurable values.
    struct ScoredPref {
        vote: PreferenceVote,
    }

    impl ChargingPreference for ScoredPref {
        fn score(&mut self, _ctx: &DecisionContext) -> PreferenceVote {
            self.vote.clone()
        }
        fn name(&self) -> &'static str {
            "scored_test"
        }
    }

    #[test]
    fn constraint_overrides_preferences() {
        let env = TestEnvBuilder::new().build();
        let ctx = make_ctx(&env);

        let override_vote = PreferenceVote {
            target_soc: Some(0.8),
            power_kw: None,
            departure_hour: None,
            min_soc: None,
            max_soc: None,
            score: 1.0,
            label: "forced",
        };

        let scored_vote = PreferenceVote {
            target_soc: Some(1.0),
            power_kw: None,
            departure_hour: None,
            min_soc: None,
            max_soc: None,
            score: 10.0,
            label: "should_not_win",
        };

        let prefs: Vec<Box<dyn ChargingPreference>> = vec![
            Box::new(OverridePref {
                vote: override_vote,
            }),
            Box::new(ScoredPref { vote: scored_vote }),
        ];

        let mut composer = ChargingComposer::new(prefs, "ev1");
        let mut out = Vec::new();
        composer.evaluate(&ctx, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::SOCTarget { target_soc, .. } => {
                assert!((target_soc - 0.8).abs() < 1e-9);
            }
            other => panic!("expected SOCTarget, got {other:?}"),
        }
        assert!(composer.last_action().starts_with("override:"));
    }

    #[test]
    fn highest_score_wins() {
        let env = TestEnvBuilder::new().build();
        let ctx = make_ctx(&env);

        let low_vote = PreferenceVote {
            target_soc: Some(0.5),
            power_kw: None,
            departure_hour: None,
            min_soc: None,
            max_soc: None,
            score: 1.0,
            label: "low",
        };
        let high_vote = PreferenceVote {
            target_soc: Some(0.9),
            power_kw: None,
            departure_hour: None,
            min_soc: None,
            max_soc: None,
            score: 5.0,
            label: "high",
        };

        let prefs: Vec<Box<dyn ChargingPreference>> = vec![
            Box::new(ScoredPref { vote: low_vote }),
            Box::new(ScoredPref { vote: high_vote }),
        ];

        let mut composer = ChargingComposer::new(prefs, "ev1");
        let mut out = Vec::new();
        composer.evaluate(&ctx, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::SOCTarget { target_soc, .. } => {
                assert!((target_soc - 0.9).abs() < 1e-9);
            }
            other => panic!("expected SOCTarget, got {other:?}"),
        }
    }

    #[test]
    fn conservative_power_resolution() {
        let env = TestEnvBuilder::new().build();
        let ctx = make_ctx(&env);

        let big = PreferenceVote {
            target_soc: None,
            power_kw: Some(7.2),
            departure_hour: None,
            min_soc: None,
            max_soc: None,
            score: 3.0,
            label: "big_power",
        };
        let small = PreferenceVote {
            target_soc: None,
            power_kw: Some(3.0),
            departure_hour: None,
            min_soc: None,
            max_soc: None,
            score: 1.0,
            label: "small_power",
        };

        let prefs: Vec<Box<dyn ChargingPreference>> = vec![
            Box::new(ScoredPref { vote: big }),
            Box::new(ScoredPref { vote: small }),
        ];

        let mut composer = ChargingComposer::new(prefs, "ev1");
        let mut out = Vec::new();
        composer.evaluate(&ctx, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                assert!((active_power_kw - 3.0).abs() < 1e-9);
            }
            other => panic!("expected PowerSetpoint, got {other:?}"),
        }
    }

    #[test]
    fn empty_preferences_idles() {
        let env = TestEnvBuilder::new().build();
        let ctx = make_ctx(&env);

        let prefs: Vec<Box<dyn ChargingPreference>> = vec![];
        let mut composer = ChargingComposer::new(prefs, "ev1");
        let mut out = Vec::new();
        composer.evaluate(&ctx, &mut out);

        assert!(out.is_empty());
        assert_eq!(composer.last_action(), "idle:no_preferences");
    }

    #[test]
    fn needed_charge_hours_returns_minimum_across_preferences() {
        let env = TestEnvBuilder::new().build();
        let ctx = make_ctx(&env);

        struct FixedHours(f64);
        impl ChargingPreference for FixedHours {
            fn score(&mut self, _ctx: &DecisionContext) -> PreferenceVote {
                PreferenceVote {
                    target_soc: Some(0.8),
                    power_kw: None,
                    departure_hour: None,
                    min_soc: None,
                    max_soc: None,
                    score: 1.0,
                    label: "fixed",
                }
            }
            fn needed_charge_hours(&self, _ctx: &DecisionContext) -> f64 {
                self.0
            }
            fn name(&self) -> &'static str {
                "fixed_hours"
            }
        }

        let prefs: Vec<Box<dyn ChargingPreference>> = vec![
            Box::new(FixedHours(5.0)),
            Box::new(FixedHours(3.0)),
            Box::new(FixedHours(7.0)),
        ];
        let mut composer = ChargingComposer::new(prefs, "ev1");
        let mut out = Vec::new();
        composer.evaluate(&ctx, &mut out);

        assert!(
            (composer.last_needed_charge_hours() - 3.0).abs() < 1e-9,
            "needed_charge_hours must be the minimum (3.0), got {}",
            composer.last_needed_charge_hours()
        );
    }

    #[test]
    fn needed_charge_hours_returns_zero_when_no_preferences_provide_estimate() {
        let env = TestEnvBuilder::new().build();
        let ctx = make_ctx(&env);

        let prefs: Vec<Box<dyn ChargingPreference>> = vec![];
        let mut composer = ChargingComposer::new(prefs, "ev1");
        let mut out = Vec::new();
        composer.evaluate(&ctx, &mut out);

        assert_eq!(
            composer.last_needed_charge_hours(),
            0.0,
            "no preferences → 0.0 (not f64::INFINITY)"
        );
    }

    #[test]
    fn power_setpoint_carries_soc_constraints_from_v2g_v2h() {
        let env = TestEnvBuilder::new().build();
        let ctx = make_ctx(&env);

        let discharge_vote = PreferenceVote {
            target_soc: None,
            power_kw: Some(-5.0),
            departure_hour: None,
            min_soc: Some(0.2),
            max_soc: None,
            score: 2.0,
            label: "v2h:discharging",
        };

        let prefs: Vec<Box<dyn ChargingPreference>> = vec![Box::new(ScoredPref {
            vote: discharge_vote,
        })];
        let mut composer = ChargingComposer::new(prefs, "ev1");
        let mut out = Vec::new();
        composer.evaluate(&ctx, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::PowerSetpoint {
                active_power_kw,
                min_soc,
                max_soc,
                ..
            } => {
                assert!((active_power_kw - (-5.0)).abs() < 1e-9);
                assert_eq!(*min_soc, Some(0.2));
                assert_eq!(*max_soc, None);
            }
            other => panic!("expected PowerSetpoint, got {other:?}"),
        }
    }

    #[test]
    fn power_setpoint_without_soc_constraints_passes_none() {
        let env = TestEnvBuilder::new().build();
        let ctx = make_ctx(&env);

        // Simulate a strategy like SolarTracking that produces positive power
        // (charge) without any SOC constraints.
        let charge_vote = PreferenceVote {
            target_soc: None,
            power_kw: Some(3.5),
            departure_hour: None,
            min_soc: None,
            max_soc: None,
            score: 1.0,
            label: "solar:charging",
        };

        let prefs: Vec<Box<dyn ChargingPreference>> =
            vec![Box::new(ScoredPref { vote: charge_vote })];
        let mut composer = ChargingComposer::new(prefs, "ev1");
        let mut out = Vec::new();
        composer.evaluate(&ctx, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::PowerSetpoint {
                active_power_kw,
                min_soc,
                max_soc,
                ..
            } => {
                assert!((active_power_kw - 3.5).abs() < 1e-9);
                assert_eq!(*min_soc, None);
                assert_eq!(*max_soc, None);
            }
            other => panic!("expected PowerSetpoint, got {other:?}"),
        }
    }

    #[test]
    fn charge_power_clamped_to_max_charge_kw() {
        let env = TestEnvBuilder::new().build();
        let ctx = make_ctx(&env);

        // Unclamped preference: emits 999 kW charge power, well above the
        // 7.2 kW equipment limit.
        let unclamped_vote = PreferenceVote {
            target_soc: None,
            power_kw: Some(999.0),
            departure_hour: None,
            min_soc: None,
            max_soc: None,
            score: 1.0,
            label: "unclamped:charge",
        };

        let prefs: Vec<Box<dyn ChargingPreference>> = vec![Box::new(ScoredPref {
            vote: unclamped_vote,
        })];
        let mut composer = ChargingComposer::new(prefs, "ev1");
        let mut out = Vec::new();
        composer.evaluate(&ctx, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                assert!(
                    (active_power_kw - ctx.max_charge_kw).abs() < 1e-9,
                    "expected clamped power {expected}, got {active_power_kw}",
                    expected = ctx.max_charge_kw,
                );
            }
            other => panic!("expected PowerSetpoint, got {other:?}"),
        }
    }

    #[test]
    fn discharge_power_clamped_to_max_discharge_kw() {
        let env = TestEnvBuilder::new().build();
        let ctx = make_ctx(&env);

        // Unclamped preference: emits -999 kW discharge power, well below the
        // -5.0 kW equipment limit.
        let unclamped_vote = PreferenceVote {
            target_soc: None,
            power_kw: Some(-999.0),
            departure_hour: None,
            min_soc: None,
            max_soc: None,
            score: 1.0,
            label: "unclamped:discharge",
        };

        let prefs: Vec<Box<dyn ChargingPreference>> = vec![Box::new(ScoredPref {
            vote: unclamped_vote,
        })];
        let mut composer = ChargingComposer::new(prefs, "ev1");
        let mut out = Vec::new();
        composer.evaluate(&ctx, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                assert!(
                    (active_power_kw - (-ctx.max_discharge_kw)).abs() < 1e-9,
                    "expected clamped power {expected}, got {active_power_kw}",
                    expected = -ctx.max_discharge_kw,
                );
            }
            other => panic!("expected PowerSetpoint, got {other:?}"),
        }
    }

    #[test]
    fn power_within_limits_passes_through_unchanged() {
        let env = TestEnvBuilder::new().build();
        let ctx = make_ctx(&env);

        // Charge power within equipment limit (3.0 kW < 7.2 kW max_charge).
        let charge_vote = PreferenceVote {
            target_soc: None,
            power_kw: Some(3.0),
            departure_hour: None,
            min_soc: None,
            max_soc: None,
            score: 1.0,
            label: "within_limit:charge",
        };

        let prefs: Vec<Box<dyn ChargingPreference>> =
            vec![Box::new(ScoredPref { vote: charge_vote })];
        let mut composer = ChargingComposer::new(prefs, "ev1");
        let mut out = Vec::new();
        composer.evaluate(&ctx, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                assert!((active_power_kw - 3.0).abs() < 1e-9);
            }
            other => panic!("expected PowerSetpoint, got {other:?}"),
        }

        // Discharge power within equipment limit (-3.0 kW, magnitude < 5.0 kW max_discharge).
        let discharge_vote = PreferenceVote {
            target_soc: None,
            power_kw: Some(-3.0),
            departure_hour: None,
            min_soc: None,
            max_soc: None,
            score: 1.0,
            label: "within_limit:discharge",
        };

        let prefs: Vec<Box<dyn ChargingPreference>> = vec![Box::new(ScoredPref {
            vote: discharge_vote,
        })];
        let mut composer = ChargingComposer::new(prefs, "ev1");
        let mut out = Vec::new();
        composer.evaluate(&ctx, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                assert!((active_power_kw - (-3.0)).abs() < 1e-9);
            }
            other => panic!("expected PowerSetpoint, got {other:?}"),
        }
    }

    #[test]
    fn override_with_both_departure_and_power_emits_two_signals() {
        let env = TestEnvBuilder::new().build();
        let ctx = make_ctx(&env);

        let vote = PreferenceVote {
            target_soc: Some(0.9),
            power_kw: Some(7.2),
            departure_hour: Some(7.0),
            min_soc: None,
            max_soc: None,
            score: 10.0,
            label: "departure:urgent",
        };

        let prefs: Vec<Box<dyn ChargingPreference>> = vec![Box::new(OverridePref { vote })];
        let mut composer = ChargingComposer::new(prefs, "ev1");
        let mut out = Vec::new();
        composer.evaluate(&ctx, &mut out);

        assert_eq!(
            out.len(),
            2,
            "expected 2 dispatch requests (EvSetReadyBy + PowerSetpoint), got {}",
            out.len()
        );

        let has_ready_by = out
            .iter()
            .any(|r| matches!(r.signal, ControlSignal::EvSetReadyBy { .. }));
        let has_power = out
            .iter()
            .any(|r| matches!(r.signal, ControlSignal::PowerSetpoint { .. }));

        assert!(has_ready_by, "expected EvSetReadyBy in output");
        assert!(has_power, "expected PowerSetpoint in output");

        // Verify the EvSetReadyBy signal has the correct values
        let ready_by = out
            .iter()
            .find(|r| matches!(r.signal, ControlSignal::EvSetReadyBy { .. }))
            .unwrap();
        match &ready_by.signal {
            ControlSignal::EvSetReadyBy {
                departure_hour,
                target_soc,
            } => {
                assert!((departure_hour - 7.0).abs() < 1e-9);
                assert!((target_soc - 0.9).abs() < 1e-9);
            }
            _ => unreachable!(),
        }

        // Verify the PowerSetpoint signal carries the correct power
        let power_req = out
            .iter()
            .find(|r| matches!(r.signal, ControlSignal::PowerSetpoint { .. }))
            .unwrap();
        match &power_req.signal {
            ControlSignal::PowerSetpoint {
                active_power_kw,
                min_soc,
                max_soc,
                ..
            } => {
                assert!((active_power_kw - 7.2).abs() < 1e-9);
                assert_eq!(*min_soc, None);
                assert_eq!(*max_soc, None);
            }
            _ => unreachable!(),
        }

        // Both signals should carry the same source and Schedule tier
        assert_eq!(ready_by.priority, PriorityTier::Schedule);
        assert_eq!(power_req.priority, PriorityTier::Schedule);
    }

    #[test]
    fn override_with_departure_only_emits_ev_set_ready_by() {
        let env = TestEnvBuilder::new().build();
        let ctx = make_ctx(&env);

        let vote = PreferenceVote {
            target_soc: Some(0.9),
            power_kw: None,
            departure_hour: Some(7.0),
            min_soc: None,
            max_soc: None,
            score: 10.0,
            label: "departure:urgent",
        };

        let prefs: Vec<Box<dyn ChargingPreference>> = vec![Box::new(OverridePref { vote })];
        let mut composer = ChargingComposer::new(prefs, "ev1");
        let mut out = Vec::new();
        composer.evaluate(&ctx, &mut out);

        assert_eq!(
            out.len(),
            1,
            "departure-only vote should produce a single dispatch request"
        );
        match &out[0].signal {
            ControlSignal::EvSetReadyBy {
                departure_hour,
                target_soc,
            } => {
                assert!((departure_hour - 7.0).abs() < 1e-9);
                assert!((target_soc - 0.9).abs() < 1e-9);
            }
            other => panic!("expected EvSetReadyBy, got {other:?}"),
        }
    }

    #[test]
    fn scored_votes_merge_departure_and_power_into_two_signals() {
        let env = TestEnvBuilder::new().build();
        let ctx = make_ctx(&env);

        // One preference provides departure_hour, another provides power_kw.
        // resolve() merges them: departure from the first, power from the second.
        let departure_vote = PreferenceVote {
            target_soc: Some(0.85),
            power_kw: None,
            departure_hour: Some(6.5),
            min_soc: None,
            max_soc: None,
            score: 5.0,
            label: "departure:planned",
        };
        let power_vote = PreferenceVote {
            target_soc: None,
            power_kw: Some(3.5),
            departure_hour: None,
            min_soc: None,
            max_soc: None,
            score: 2.0,
            label: "solar:charging",
        };
        // A third vote with higher score supplies the target_soc winner but
        // does NOT supply power or departure — the resolved result should
        // carry: target_soc from this vote (highest score), departure from
        // departure_vote (earliest), power from power_vote (most conservative).
        let high_score_vote = PreferenceVote {
            target_soc: Some(0.95),
            power_kw: None,
            departure_hour: None,
            min_soc: None,
            max_soc: None,
            score: 10.0,
            label: "high_score",
        };

        let prefs: Vec<Box<dyn ChargingPreference>> = vec![
            Box::new(ScoredPref {
                vote: departure_vote,
            }),
            Box::new(ScoredPref { vote: power_vote }),
            Box::new(ScoredPref {
                vote: high_score_vote,
            }),
        ];
        let mut composer = ChargingComposer::new(prefs, "ev1");
        let mut out = Vec::new();
        composer.evaluate(&ctx, &mut out);

        // Resolved vote: target_soc=0.95 (high_score wins), departure=6.5,
        // power=3.5 — should produce both EvSetReadyBy + PowerSetpoint.
        assert_eq!(
            out.len(),
            2,
            "merged departure+power should produce 2 dispatch requests, got {}",
            out.len()
        );

        let has_ready_by = out
            .iter()
            .any(|r| matches!(r.signal, ControlSignal::EvSetReadyBy { .. }));
        let has_power = out
            .iter()
            .any(|r| matches!(r.signal, ControlSignal::PowerSetpoint { .. }));

        assert!(has_ready_by, "expected EvSetReadyBy in output");
        assert!(has_power, "expected PowerSetpoint in output");

        // EvSetReadyBy should carry the winning target_soc (0.95) and departure (6.5)
        let ready_by = out
            .iter()
            .find(|r| matches!(r.signal, ControlSignal::EvSetReadyBy { .. }))
            .unwrap();
        match &ready_by.signal {
            ControlSignal::EvSetReadyBy {
                departure_hour,
                target_soc,
            } => {
                assert!(
                    (departure_hour - 6.5).abs() < 1e-9,
                    "departure should be 6.5, got {departure_hour}"
                );
                assert!(
                    (target_soc - 0.95).abs() < 1e-9,
                    "target_soc should be 0.95 (winning score), got {target_soc}"
                );
            }
            _ => unreachable!(),
        }

        // PowerSetpoint should carry the power (3.5 kW)
        let power_req = out
            .iter()
            .find(|r| matches!(r.signal, ControlSignal::PowerSetpoint { .. }))
            .unwrap();
        match &power_req.signal {
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                assert!((active_power_kw - 3.5).abs() < 1e-9);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn departure_deadline_override_emits_both_ev_set_ready_by_and_power_setpoint() {
        let env = TestEnvBuilder::new().build();
        // Departure at 7:00 AM (420 min), current time (env default) is midnight
        // 10°C default → effective_efficiency ≈ 0.81
        // SOC 0.2 → 0.9 target, needed ≈ 7.2h, 7h available
        // 7h < 7.2 * 1.2 = 8.6h → urgency fires
        // Context with max_charge_kw = 7.2
        let ctx = DecisionContext {
            current_soc: 0.2,
            capacity_kwh: 60.0,
            max_charge_kw: 7.2,
            max_discharge_kw: 5.0,
            env: &env,
            current_minute: 0,
            next_departure_minute: Some(420),
            time_res_minutes: 1.0,
        };

        let departure_pref = crate::actors::ev_driver::departure::DepartureDeadline {
            schedule: vec![DepartureConstraint {
                day_filter: DayFilter::Any,
                departure_minute: 420,
                target_soc: 0.9,
            }],
            target_soc: 0.9,
            efficiency: 0.9,
            buffer_hours: 0.0,
        };

        let prefs: Vec<Box<dyn ChargingPreference>> = vec![Box::new(departure_pref)];
        let mut composer = ChargingComposer::new(prefs, "ev1");
        let mut out = Vec::new();
        composer.evaluate(&ctx, &mut out);

        // The urgency Override sets both departure_hour and power_kw.
        // The composer must propagate both — not return early after EvSetReadyBy.
        assert_eq!(
            out.len(),
            2,
            "urgency Override with both fields must produce 2 dispatch requests, got {}",
            out.len()
        );

        let has_ready_by = out
            .iter()
            .any(|r| matches!(r.signal, ControlSignal::EvSetReadyBy { .. }));
        let has_power = out
            .iter()
            .any(|r| matches!(r.signal, ControlSignal::PowerSetpoint { .. }));

        assert!(has_ready_by, "urgency Override must emit EvSetReadyBy");
        assert!(
            has_power,
            "urgency Override must emit PowerSetpoint with max_charge_kw"
        );

        // The PowerSetpoint must carry max_charge_kw (the scheduler's urgency power)
        let power_req = out
            .iter()
            .find(|r| matches!(r.signal, ControlSignal::PowerSetpoint { .. }))
            .unwrap();
        match &power_req.signal {
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                assert!(
                    (active_power_kw - 7.2).abs() < 1e-9,
                    "urgency PowerSetpoint must be max_charge_kw (7.2), got {active_power_kw}"
                );
            }
            _ => unreachable!(),
        }

        // EvSetReadyBy must carry the departure hour (7:00 → 7.0 h) and target SOC
        let ready_by = out
            .iter()
            .find(|r| matches!(r.signal, ControlSignal::EvSetReadyBy { .. }))
            .unwrap();
        match &ready_by.signal {
            ControlSignal::EvSetReadyBy {
                departure_hour,
                target_soc,
            } => {
                assert!(
                    (departure_hour - 7.0).abs() < 1e-9,
                    "departure_hour should be 7.0, got {departure_hour}"
                );
                assert!(
                    (target_soc - 0.9).abs() < 1e-9,
                    "target_soc should be 0.9, got {target_soc}"
                );
            }
            _ => unreachable!(),
        }

        assert_eq!(ready_by.priority, PriorityTier::Schedule);
        assert_eq!(power_req.priority, PriorityTier::Schedule);
    }
}
