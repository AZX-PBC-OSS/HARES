//! Bounded warning log for [`Dwelling`](super::Dwelling).
//!
//! Long-running simulations (e.g. RL rollouts stepping a dwelling for years
//! of simulated time without draining warnings) must not grow the warning
//! buffer unboundedly. [`WarningLog`] holds two bounded buffers: the run
//! record, the first [`WarningLog::CAPACITY`] warnings of the run plus the
//! count dropped past it (the finished run's `RunResult.warnings`), and the
//! pending drain, every warning pushed since the previous
//! [`WarningLog::take`], bounded at the same capacity per drain with its
//! own dropped count. The gym's per-step drain keeps receiving every
//! warning after the run record is full.

use hares_types::Warning;

/// Bounded FIFO warning store: the run record keeps the first
/// [`Self::CAPACITY`] warnings of the run, the pending drain the first
/// [`Self::CAPACITY`] since the previous [`Self::take`].
///
/// Derefs to `[Warning]` (the pending drain), so callers can index,
/// iterate, and query length exactly like the vector it replaces.
/// Warnings beyond a buffer's capacity are never stored — only counted —
/// and a draining take appends one summary `Warning` reporting its dropped
/// count.
#[derive(Debug, Default, Clone)]
pub struct WarningLog {
    /// The run record: the first `CAPACITY` warnings of the run. The
    /// finished run's `RunResult.warnings` reads this buffer.
    run_record: Vec<Warning>,
    run_record_dropped: u64,
    /// The pending drain: every warning since the previous `take`.
    pending: Vec<Warning>,
    pending_dropped: u64,
    /// The step a push happens in, set by the step path at entry and
    /// cleared after it: a producer's warning without a step index of its
    /// own is stamped with this (the dwelling stamps the step index of a
    /// warning raised inside a step).
    now: Option<u64>,
}

impl WarningLog {
    /// Maximum number of warnings stored verbatim per buffer. The earliest
    /// warnings are kept because they usually describe the root cause;
    /// later repetitions are counted instead of stored.
    pub const CAPACITY: usize = 1000;

    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the step a push happens in (and clears it with `None`): the
    /// step path calls this at entry so every warning the step's dispatch
    /// and equipment drains raise carries the step it was raised in.
    pub fn set_step(&mut self, step: Option<u64>) {
        self.now = step;
    }

    /// Append a warning. Each buffer takes its own copy: once the run
    /// record holds [`Self::CAPACITY`] warnings, further ones are counted
    /// into `run_record_dropped` (see [`Self::run_record`]); the pending
    /// drain's bound works the same way per drain.
    pub fn push_warning(&mut self, mut warning: Warning) {
        if warning.step_index.is_none()
            && let Some(step) = self.now
        {
            warning.step_index = Some(step);
        }
        if self.run_record.len() < Self::CAPACITY {
            self.run_record.push(warning.clone());
        } else {
            self.run_record_dropped += 1;
        }
        if self.pending.len() < Self::CAPACITY {
            self.pending.push(warning);
        } else {
            self.pending_dropped += 1;
        }
    }

    /// Append a plain message as a warning from the named source: the
    /// dwelling's internal producers (control routing, warm-up, rollback)
    /// have no `Warning` of their own.
    pub fn push_from(&mut self, source: &str, message: String) {
        self.push_warning(Warning {
            step_index: None,
            source: std::sync::Arc::from(source),
            message,
        });
    }

    /// The run record: the first [`Self::CAPACITY`] warnings of the run and
    /// the count dropped past it. The finished run's `RunResult.warnings`
    /// and `RunResult.warnings_dropped` read these.
    #[must_use]
    pub fn run_record(&self) -> (&[Warning], u64) {
        (&self.run_record, self.run_record_dropped)
    }

    /// Warnings dropped from the pending drain (counted but not stored)
    /// since the last [`Self::take`].
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.pending_dropped
    }

    /// Drain the pending buffer, emptying it. When the drain overflowed
    /// since the previous take, a final summary warning naming the dropped
    /// count is appended, so overflow is never silent.
    pub fn take(&mut self) -> Vec<Warning> {
        let mut out = std::mem::take(&mut self.pending);
        if self.pending_dropped > 0 {
            out.push(Warning {
                step_index: None,
                source: std::sync::Arc::from("warnings"),
                message: format!(
                    "warning drain capacity ({}) reached; {} additional warning(s) were dropped",
                    Self::CAPACITY,
                    self.pending_dropped
                ),
            });
            self.pending_dropped = 0;
        }
        out
    }
}

impl std::ops::Deref for WarningLog {
    type Target = [Warning];

    fn deref(&self) -> &Self::Target {
        &self.pending
    }
}

#[cfg(test)]
mod tests {
    use super::WarningLog;
    use hares_types::Warning;

    fn warning(step: Option<u64>, message: &str) -> Warning {
        Warning {
            step_index: step,
            source: std::sync::Arc::from("test"),
            message: message.to_string(),
        }
    }

    #[test]
    fn stores_and_drains_below_capacity() {
        let mut log = WarningLog::new();
        log.push_warning(warning(None, "a"));
        log.push_warning(warning(None, "b"));
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].message, "a");
        assert_eq!(log.dropped(), 0);
        let drained = log.take();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].message, "a");
        assert!(log.is_empty());
        // The run record kept its copy.
        assert_eq!(log.run_record().0.len(), 2);
    }

    #[test]
    fn overflow_stops_growing_and_reports_dropped_count() {
        let mut log = WarningLog::new();
        for i in 0..WarningLog::CAPACITY + 5 {
            log.push_warning(warning(None, &format!("warning {i}")));
        }
        // Storage is capped: the buffers stop growing at CAPACITY.
        assert_eq!(log.len(), WarningLog::CAPACITY);
        assert_eq!(log.dropped(), 5);
        // The first CAPACITY warnings are kept verbatim.
        assert_eq!(log[0].message, "warning 0");
        assert_eq!(
            log[WarningLog::CAPACITY - 1].message,
            format!("warning {}", WarningLog::CAPACITY - 1)
        );

        // Draining appends exactly one summary warning with the dropped
        // count, from the `warnings` source.
        let drained = log.take();
        assert_eq!(drained.len(), WarningLog::CAPACITY + 1);
        let summary = drained.last().unwrap();
        assert_eq!(summary.source.as_ref(), "warnings");
        assert!(
            summary
                .message
                .contains("5 additional warning(s) were dropped"),
            "unexpected summary: {summary}"
        );

        // The drain resets after taking.
        assert!(log.is_empty());
        assert_eq!(log.dropped(), 0);
        log.push_warning(warning(None, "fresh"));
        assert_eq!(log.take().len(), 1);

        // The run record is the drain's: it stopped at CAPACITY and counted
        // the rest — the five overflow warnings and the post-drain "fresh"
        // one (the record is the run's, not the drain's).
        let (record, dropped) = log.run_record();
        assert_eq!(record.len(), WarningLog::CAPACITY);
        assert_eq!(dropped, 6);
    }

    #[test]
    fn the_run_record_survives_drains() {
        let mut log = WarningLog::new();
        log.push_warning(warning(Some(1), "kept"));
        assert_eq!(log.take().len(), 1);
        assert!(log.is_empty());
        let (record, _) = log.run_record();
        assert_eq!(record.len(), 1, "a drain empties the drain, not the record");
    }
}
