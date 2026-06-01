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
    /// (minimum across all preferences). Returns `f64::INFINITY` if no
    /// preference provides an estimate.
    pub fn last_needed_charge_hours(&self) -> f64 {
        self.last_needed_charge_hours
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
                self.emit_vote(&vote, out);
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
        self.emit_vote(&resolved, out);
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
    fn emit_vote(&self, vote: &PreferenceVote, out: &mut Vec<DispatchRequest>) {
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
            return;
        }

        // If there's a power_kw, use PowerSetpoint
        if let Some(power) = vote.power_kw {
            if power.abs() < 1e-9 {
                // Zero power = idle, no dispatch needed
                return;
            }
            out.push(DispatchRequest {
                target: self.dispatch_target.clone(),
                signal: ControlSignal::PowerSetpoint {
                    active_power_kw: power,
                    reactive_power_kvar: None,
                },
                priority: PriorityTier::Schedule,
            });
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
}
