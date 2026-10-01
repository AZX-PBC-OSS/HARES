//! Per-equipment health-event vocabulary and run-total counters.

use serde::{Deserialize, Serialize};

/// Health events equipment records while running.
///
/// Health events are recorded unconditionally in every build profile —
/// unlike the feature-gated observe counters they replace — and are
/// returned with the run's result, so a finished run always reports the
/// degradation it hit without requiring a special build or log capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthEvent {
    /// A port write was rolled back to its pre-write value after validation
    /// rejected the write.
    PortRollback,
    /// A control signal was rejected (capability gate or numeric bounds).
    RejectedControlSignal,
    /// An equipment action was clamped to a safe bound instead of the
    /// requested value.
    ClampedAction,
    /// A biquadratic curve index was out of bounds and was clamped to the
    /// last same-type curve.
    CurveIndexClamp,
    /// Warmup did not converge before the simulation started.
    WarmupNotConverged,
}

/// Health counters for one equipment, accumulated across a run.
///
/// The counts are run totals, never per-step values: equipment hands out
/// per-step deltas and the caller accumulates them, so the counters returned
/// with the run's result answer "how often did this happen over the run"
/// without holding per-step history. Health events are recorded
/// unconditionally in every build profile — the totals exist even without
/// the `observe` feature.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EquipmentHealthCounts {
    /// Total number of biquadratic curve-index clamps: an out-of-bounds
    /// curve index was evaluated and clamped to the last same-type curve.
    pub curve_index_clamps: u64,
}

#[cfg(test)]
mod tests {
    use super::{EquipmentHealthCounts, HealthEvent};

    #[test]
    fn health_event_round_trips_through_json() {
        for event in [
            HealthEvent::PortRollback,
            HealthEvent::RejectedControlSignal,
            HealthEvent::ClampedAction,
            HealthEvent::CurveIndexClamp,
            HealthEvent::WarmupNotConverged,
        ] {
            let json = serde_json::to_string(&event).expect("serialize HealthEvent");
            let decoded: HealthEvent =
                serde_json::from_str(&json).expect("deserialize HealthEvent");
            assert_eq!(decoded, event);
        }
    }

    #[test]
    fn health_event_is_copy_and_comparable() {
        let event = HealthEvent::CurveIndexClamp;
        let copied = event;
        assert_eq!(event, copied);
    }

    #[test]
    fn default_health_counts_are_zero() {
        let counts = EquipmentHealthCounts::default();
        assert_eq!(counts.curve_index_clamps, 0);
    }

    #[test]
    fn health_counts_round_trip_through_json() {
        let counts = EquipmentHealthCounts {
            curve_index_clamps: 3,
        };
        let json = serde_json::to_string(&counts).expect("serialize EquipmentHealthCounts");
        let decoded: EquipmentHealthCounts =
            serde_json::from_str(&json).expect("deserialize EquipmentHealthCounts");
        assert_eq!(decoded, counts);
    }
}
