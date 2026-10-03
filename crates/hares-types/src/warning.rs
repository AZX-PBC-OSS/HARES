//! The run-level warning type every producer shares.
//!
//! Equipment, input resolvers and the tariff parser raise warnings through
//! this one type; the dwelling's [`crate::WarningLog`] formats them into the
//! strings `take_warnings()` returns. A `tracing` log line is never the only
//! signal of a warning: every producer carries the value so the run's caller
//! sees it.

use std::fmt;
use std::sync::Arc;

/// One warning a run reports.
///
/// A producer sets `step_index: None`; the dwelling stamps the step index of
/// a warning it drains after a step. `source` names the producer (for
/// example the equipment name, `"hpxml"`, `"schedule"` or `"tariff"`).
#[derive(Debug, Clone, PartialEq)]
pub struct Warning {
    /// Simulation step the warning was raised in, when the dwelling drained
    /// it after a step; `None` for construction-time warnings.
    pub step_index: Option<u64>,
    /// Which producer raised the warning.
    pub source: Arc<str>,
    /// Human-readable message.
    pub message: String,
}

impl Warning {
    /// Builds a construction-time warning (no step index) from a source name.
    #[must_use]
    pub fn new(source: impl Into<Arc<str>>, message: impl Into<String>) -> Self {
        Self {
            step_index: None,
            source: source.into(),
            message: message.into(),
        }
    }
}

/// `"{source}: {message}"`, prefixed `"step {n}: "` when the warning carries
/// a step index: the form the run's warning log reports.
impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(step) = self.step_index {
            write!(f, "step {step}: ")?;
        }
        write!(f, "{}: {}", self.source, self.message)
    }
}
