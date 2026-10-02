//! Run-level health totals for a dwelling.

#[cfg(feature = "profiling")]
use std::sync::Arc;
#[cfg(feature = "profiling")]
use std::time::Duration as StdDuration;

use serde::{Deserialize, Serialize};

use hares_types::ZoneId;

/// Run-total health counters for one dwelling, recorded unconditionally in
/// every build profile and never reset per step: a finished run always
/// reports the degradation it hit without requiring a special build or log
/// capture. Per-step observe counters do not exist; the run totals replace
/// them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunHealth {
    /// Equipment steps whose port contributions were rolled back after the
    /// step failed validation.
    pub port_rollbacks: u64,
    /// Control signals rejected by equipment (capability gate, numeric
    /// bounds) or routed at a target that does not exist.
    pub rejected_control_signals: u64,
    /// Equipment actions clamped to a safe bound instead of the requested
    /// value. Recorded via [`crate::dwelling::Dwelling::record_clamped_actions`].
    pub clamped_actions: u64,
    /// Total biquadratic curve-index clamps across all HVAC equipment over
    /// the run: an out-of-bounds curve index was evaluated and clamped to
    /// the last same-type curve.
    pub curve_index_clamps: u64,
    /// Outcome of the warm-up convergence loop (Disabled until warm-up runs).
    pub warmup: WarmupOutcome,
}

impl Default for RunHealth {
    fn default() -> Self {
        Self {
            port_rollbacks: 0,
            rejected_control_signals: 0,
            clamped_actions: 0,
            curve_index_clamps: 0,
            warmup: WarmupOutcome::Disabled,
        }
    }
}

/// One registered actor's run-total scheduler-entry time, behind the
/// `profiling` feature: `total` is the actor's share of the run's `actors`
/// phase (the interest filter, `decide()` and the health check, measured by
/// the same phase-clock readings that accumulate the phase), `calls` the
/// number of `decide()` invocations the interest filter made.
#[cfg(feature = "profiling")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorTiming {
    pub name: Arc<str>,
    pub total: StdDuration,
    pub calls: u64,
}

/// Run-total per-actor timing store behind the `profiling` feature: one slot
/// per registered actor, in registration order, accumulated in place across
/// the run's steps and never reset per step (like [`RunHealth`]). Opaque by
/// contract (`plan.md` §2.9): the slots are private, `record` mutates them,
/// and the profiling summary reads them through [`Self::timings`].
#[cfg(feature = "profiling")]
#[derive(Debug, Default)]
pub struct ActorTimings {
    slots: Vec<ActorTiming>,
}

#[cfg(feature = "profiling")]
impl ActorTimings {
    /// One zeroed slot per name, in iteration order. Runs at actor
    /// registration (off the hot path), so the allocation is fine.
    pub fn new(names: impl IntoIterator<Item = Arc<str>>) -> Self {
        Self {
            slots: names
                .into_iter()
                .map(|name| ActorTiming {
                    name,
                    total: StdDuration::ZERO,
                    calls: 0,
                })
                .collect(),
        }
    }

    /// Adds `elapsed` to the slot's total in place and, when `called`,
    /// counts one `decide()` invocation. The accumulation allocates nothing:
    /// the scheduler calls this once per plan entry per step. An
    /// out-of-range slot has no actor (the actor list changed without the
    /// store being rebuilt, which the registration paths prevent) and is
    /// ignored in release builds.
    pub fn record(&mut self, slot: usize, elapsed: StdDuration, called: bool) {
        debug_assert!(
            slot < self.slots.len(),
            "actor timing slot {slot} out of range ({} slots)",
            self.slots.len()
        );
        if let Some(entry) = self.slots.get_mut(slot) {
            entry.total += elapsed;
            if called {
                entry.calls += 1;
            }
        }
    }

    /// The run's per-actor totals, one entry per registered actor in
    /// registration order.
    pub fn timings(&self) -> &[ActorTiming] {
        &self.slots
    }
}

/// How the warm-up convergence loop ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WarmupOutcome {
    /// Warm-up never ran (no initialization duration configured, or a
    /// degenerate zero-step day).
    Disabled,
    /// Warm-up ran to a stop.
    Ran {
        /// Warm-up days simulated (iterations of the 24-hour replay).
        days_run: u32,
        /// Whether the per-day temperature change fell below the
        /// convergence threshold before the iteration cap.
        converged: bool,
        /// Day-over-day residuals of the final warm-up day against the day
        /// before it; `None` when only one warm-up day ran (no
        /// predecessor to diff against).
        residuals: Option<WarmupResiduals>,
    },
}

/// Day-over-day residuals of the final warm-up day.
///
/// The load residuals describe the run's warm-up quality, not a per-zone
/// load audit: HVAC delivery is only measured in aggregate, so the
/// aggregate daily-peak heating/cooling relative changes are attributed to
/// the worst-temperature-residual zone's row. The zone scope is ALL zones
/// (unconditioned included) even though the convergence criterion is
/// conditioned zones only: a converged run can still carry an
/// unconditioned-zone residual.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WarmupResiduals {
    /// Zone with the largest day-over-day temperature residual
    /// (max of |Δ daily max|, |Δ daily min|).
    pub worst_zone: ZoneId,
    /// |Δ daily max temperature| of [`Self::worst_zone`] [°C].
    pub max_temperature_c: f64,
    /// |Δ daily min temperature| of [`Self::worst_zone`] [°C].
    pub min_temperature_c: f64,
    /// Relative change of the aggregate daily peak heating [fraction].
    pub heating_load: f64,
    /// Relative change of the aggregate daily peak cooling [fraction].
    pub cooling_load: f64,
}

#[cfg(test)]
mod tests {
    use super::{RunHealth, WarmupOutcome, WarmupResiduals};
    use hares_types::ZoneId;

    #[test]
    fn run_health_defaults_to_disabled_warmup_and_zero_counters() {
        let health = RunHealth::default();
        assert_eq!(health.port_rollbacks, 0);
        assert_eq!(health.rejected_control_signals, 0);
        assert_eq!(health.clamped_actions, 0);
        assert_eq!(health.curve_index_clamps, 0);
        assert_eq!(health.warmup, WarmupOutcome::Disabled);
    }

    #[test]
    fn run_health_round_trips_through_json() {
        let health = RunHealth {
            port_rollbacks: 1,
            rejected_control_signals: 2,
            clamped_actions: 3,
            curve_index_clamps: 4,
            warmup: WarmupOutcome::Ran {
                days_run: 3,
                converged: false,
                residuals: Some(WarmupResiduals {
                    worst_zone: ZoneId(2),
                    max_temperature_c: 0.7,
                    min_temperature_c: 0.2,
                    heating_load: 0.05,
                    cooling_load: 0.0,
                }),
            },
        };
        let json = serde_json::to_string(&health).expect("serialize RunHealth");
        let decoded: RunHealth = serde_json::from_str(&json).expect("deserialize RunHealth");
        assert_eq!(decoded, health);
    }
}
