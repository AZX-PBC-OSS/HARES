//! ChargingPreference trait and supporting types for composable EV charging decisions.

use hares_types::EnvironmentState;

use super::efficiency::temp_efficiency_multiplier;

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

/// Minutes from `current_minute` until `target_minute`, wrapping past
/// midnight (a departure minute earlier in the day than the current time is
/// the *next* day's departure). Returns a full 1440 for a target equal to
/// the current minute: "departing now" leaves no charging time before the
/// next departure window.
///
/// Single home for the wraparound arithmetic: `DepartureDeadline`'s urgency
/// computation and the actor's range-anxiety urgency gate both measure time
/// to departure, so they must not drift apart.
pub(super) fn minutes_until(current_minute: u16, target_minute: u32) -> f64 {
    let diff = i64::from(target_minute) - i64::from(current_minute);
    if diff > 0 {
        diff as f64
    } else {
        (diff + 1440) as f64
    }
}

/// Hours needed to charge from the context's current SOC to `target_soc` at
/// the max charge rate, rounded up to the nearest timestep to avoid
/// underestimating charge time.
///
/// Single source of truth for time-to-charge estimates: `SocTarget`,
/// `SocGate`, and `DepartureDeadline` all report through here so the
/// estimates cannot drift between preferences.
///
/// Applies `temp_efficiency_multiplier` to the base charging efficiency to
/// account for temperature-dependent degradation of charging acceptance
/// (cold weather reduces BMS acceptance rate, thermal conditioning draws
/// power, and internal resistance increases). The multiplier is the same
/// piecewise-linear fleet-average curve from `efficiency.rs`, originally
/// calibrated for driving energy consumption (AAA 2019, Geotab 2020,
/// DOE/Argonne 2024, Recurrent Auto) but applicable to charging because
/// the same physical mechanisms (electrochemical kinetics, resistive
/// heating, thermal management) degrade both driving and charging efficiency
/// at low temperatures.
pub(super) fn needed_charge_hours_to_target(
    target_soc: f64,
    charging_efficiency: f64,
    ctx: &DecisionContext,
) -> f64 {
    let soc_gap = (target_soc - ctx.current_soc).max(0.0);
    let energy_kwh = soc_gap * ctx.capacity_kwh;
    if ctx.max_charge_kw <= 0.0 || charging_efficiency <= 0.0 {
        return f64::INFINITY;
    }
    let effective_efficiency =
        charging_efficiency / temp_efficiency_multiplier(ctx.env.weather.outdoor_temp_c);
    let raw_hours = energy_kwh / (ctx.max_charge_kw * effective_efficiency);
    let hours = if ctx.time_res_minutes > 0.0 {
        let step_hours = ctx.time_res_minutes / 60.0;
        (raw_hours / step_hours).ceil() * step_hours
    } else {
        raw_hours
    };

    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        debug_assert!(
            hours.is_finite() && hours >= 0.0,
            "needed_charge_hours: result is not a finite non-negative number; got hours={hours}, soc_gap={soc_gap}, energy_kwh={energy_kwh}, max_charge_kw={}, effective_efficiency={effective_efficiency}",
            ctx.max_charge_kw,
        );
    }

    hours
}
