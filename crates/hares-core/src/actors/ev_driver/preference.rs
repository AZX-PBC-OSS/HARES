//! ChargingPreference trait and supporting types for composable EV charging decisions.

use hares_types::EnvironmentState;

/// Context passed to each preference for per-step evaluation.
pub struct DecisionContext<'a> {
    pub current_soc: f64,
    pub capacity_kwh: f64,
    pub max_charge_kw: f64,
    pub max_discharge_kw: f64,
    pub env: &'a EnvironmentState,
    pub current_minute: u16,
    pub next_departure_minute: Option<u16>,
    pub time_res_minutes: f64,
}

/// A preference's recommendation for this timestep.
#[derive(Clone, Debug)]
pub struct PreferenceVote {
    pub target_soc: Option<f64>,
    /// Positive = charge, negative = discharge.
    pub power_kw: Option<f64>,
    pub departure_hour: Option<f64>,
    pub min_soc: Option<f64>,
    pub max_soc: Option<f64>,
    /// 0 = neutral, higher = stronger recommendation.
    pub score: f64,
    pub label: &'static str,
}

impl PreferenceVote {
    pub fn idle(label: &'static str) -> Self {
        Self {
            target_soc: None,
            power_kw: None,
            departure_hour: None,
            min_soc: None,
            max_soc: None,
            score: 0.0,
            label,
        }
    }
}

/// Hard constraint that short-circuits the scoring pipeline.
pub enum Constraint {
    /// No constraint -- proceed to scoring.
    Inactive,
    /// Override all other preferences with this vote.
    Override(PreferenceVote),
}

/// A composable charging preference that can express hard constraints
/// and scored recommendations.
pub trait ChargingPreference: Send + Sync {
    /// Check for a hard constraint that overrides scoring.
    fn constraint(&mut self, _ctx: &DecisionContext) -> Constraint {
        Constraint::Inactive
    }

    /// Score this preference given the current context.
    fn score(&mut self, ctx: &DecisionContext) -> PreferenceVote;

    /// Human-readable name for telemetry.
    fn name(&self) -> &'static str;

    /// Whether this preference currently allows charging. Returns `None`
    /// for preferences that don't manage charge/no-charge gating.
    fn charging_allowed(&self) -> Option<bool> {
        None
    }

    /// Estimated hours needed to reach target SOC from current state.
    /// Returns `f64::INFINITY` for preferences that don't produce a
    /// time-to-charge estimate. The composer aggregates by taking the
    /// minimum across all preferences.
    fn needed_charge_hours(&self, _ctx: &DecisionContext) -> f64 {
        f64::INFINITY
    }
}
