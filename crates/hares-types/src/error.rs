//! Shared error type for cross-crate HARES domain errors.

use crate::environment::ZoneId;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Top-level error grouping by subsystem domain.
#[derive(Debug, Error, Clone, PartialEq, Serialize, Deserialize)]
pub enum HaresError {
    // DEPRECATED: No genuine physics errors exist in the codebase.
    // Use only after hares-physics crate is retrofitted to return Physics errors
    // for physically impossible input conditions.
    #[error("physics error: {0}")]
    Physics(String),
    #[error("envelope error: {0}")]
    Envelope(String),
    #[error("equipment error: {0}")]
    Equipment(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("control error: {0}")]
    Control(String),
    #[error("dwelling error: {0}")]
    Dwelling(String),
    #[error("tariff error: {0}")]
    Tariff(String),
    #[error("invariant violation in '{check_name}': value={value:.6e}, tolerance={tolerance:.6e}")]
    InvariantViolation {
        check_name: String,
        value: f64,
        tolerance: f64,
    },
    #[error("NaN detected in '{value_name}' at step {step_index} (zone {zone_id:?})")]
    NanDetected {
        step_index: u64,
        zone_id: Option<ZoneId>,
        value_name: String,
    },
}

/// Per-dwelling error wrapper for fleet-level error isolation.
#[derive(Debug, Error, Clone, PartialEq, Serialize, Deserialize)]
pub enum SimError {
    #[error("simulation error: {0}")]
    Physics(HaresError),
    #[error("panic: {0}")]
    Panic(String),
    #[error("timeout")]
    Timeout,
}

#[cfg(test)]
mod tests {
    use super::{HaresError, SimError};
    use crate::ZoneId;

    #[test]
    fn error_round_trips_through_json() {
        let err = HaresError::Physics("invalid timestep".to_string());
        let json = serde_json::to_string(&err).expect("serialize error");
        let decoded: HaresError = serde_json::from_str(&json).expect("deserialize error");
        assert_eq!(decoded, err);
    }

    #[test]
    fn all_error_variants_round_trip_through_json() {
        let errors = vec![
            HaresError::Physics("invalid timestep".to_string()),
            HaresError::Envelope("missing surface".to_string()),
            HaresError::Equipment("unknown id".to_string()),
            HaresError::Io("file not found".to_string()),
            HaresError::Control("unsupported signal".to_string()),
            HaresError::Dwelling("missing window U-factor".to_string()),
            HaresError::Tariff("invalid rate".to_string()),
            HaresError::InvariantViolation {
                check_name: "thermal_balance".to_string(),
                value: 1.23,
                tolerance: 0.001,
            },
            HaresError::NanDetected {
                step_index: 42,
                zone_id: Some(ZoneId(1)),
                value_name: "q_sum".to_string(),
            },
        ];
        for err in errors {
            let json = serde_json::to_string(&err).expect("serialize error");
            let decoded: HaresError = serde_json::from_str(&json).expect("deserialize error");
            assert_eq!(decoded, err);
        }
    }

    /// Verify the deprecated Physics variant still serializes and deserializes
    /// correctly. The variant must remain in the enum for compatibility with
    /// existing serialized data and for future hares-physics crate retrofitting
    /// (see T-0145).
    #[test]
    fn deprecated_physics_variant_round_trips_through_json() {
        let err = HaresError::Physics("physically impossible temperature".to_string());
        let json = serde_json::to_string(&err).expect("serialize deprecated physics");
        let decoded: HaresError =
            serde_json::from_str(&json).expect("deserialize deprecated physics");
        assert_eq!(decoded, err);
        assert!(json.contains("physically impossible temperature"));
    }

    #[test]
    fn sim_error_round_trips_through_json() {
        let errors = vec![
            SimError::Physics(HaresError::Physics("bad timestep".to_string())),
            SimError::Panic("thread panicked".to_string()),
            SimError::Timeout,
        ];
        for err in errors {
            let json = serde_json::to_string(&err).expect("serialize sim error");
            let decoded: SimError = serde_json::from_str(&json).expect("deserialize sim error");
            assert_eq!(decoded, err);
        }
    }
}
