//! Fleet-level parallel simulation with weighted aggregation.

pub mod aggregation;
pub mod fleet;
pub mod progress;

pub use aggregation::{AggregationResolution, DwellingMetrics, FleetResults};
pub use fleet::{DwellingOutcome, Fleet, SimError, SimStatus};
