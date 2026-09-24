//! Bounded warning log for [`Dwelling`](super::Dwelling).
//!
//! Long-running simulations (e.g. RL rollouts stepping a dwelling for years
//! of simulated time without draining warnings) must not grow the warning
//! buffer unboundedly. [`WarningLog`] keeps the first [`WarningLog::CAPACITY`]
//! messages verbatim and counts every message dropped past that point; the
//! drop count is surfaced as a final summary warning when the log is drained.

/// Bounded FIFO warning buffer: keeps the first [`Self::CAPACITY`] messages,
/// counts the rest.
///
/// Derefs to `[String]`, so callers can index, iterate, and query length
/// exactly like the `Vec<String>` it replaces. Messages beyond capacity are
/// never stored — only counted — and [`WarningLog::take`] appends one
/// summary entry reporting the dropped count.
#[derive(Debug, Default, Clone)]
pub struct WarningLog {
    entries: Vec<String>,
    dropped: u64,
}

impl WarningLog {
    /// Maximum number of warning messages stored verbatim. The earliest
    /// messages are kept because they usually describe the root cause;
    /// later repetitions are counted instead of stored.
    pub const CAPACITY: usize = 1000;

    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a warning. Once [`Self::CAPACITY`] messages are stored, further
    /// messages are counted (see [`Self::dropped`]) instead of stored, so the
    /// log cannot grow unboundedly during long runs.
    pub fn push(&mut self, msg: String) {
        if self.entries.len() < Self::CAPACITY {
            self.entries.push(msg);
        } else {
            self.dropped += 1;
        }
    }

    /// Number of warnings dropped (counted but not stored) since the last
    /// [`Self::take`].
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Drain all stored warnings, resetting the log. When any messages were
    /// dropped, a final summary entry reports the count so overflow is never
    /// silent.
    pub fn take(&mut self) -> Vec<String> {
        let mut out = std::mem::take(&mut self.entries);
        if self.dropped > 0 {
            out.push(format!(
                "warning log capacity ({}) reached; {} additional warning(s) were dropped",
                Self::CAPACITY,
                self.dropped
            ));
            self.dropped = 0;
        }
        out
    }
}

impl std::ops::Deref for WarningLog {
    type Target = [String];

    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::WarningLog;

    #[test]
    fn stores_and_drains_below_capacity() {
        let mut log = WarningLog::new();
        log.push("a".to_string());
        log.push("b".to_string());
        assert_eq!(log.len(), 2);
        assert_eq!(log[0], "a");
        assert_eq!(log.dropped(), 0);
        assert_eq!(log.take(), vec!["a".to_string(), "b".to_string()]);
        assert!(log.is_empty());
    }

    #[test]
    fn overflow_stops_growing_and_reports_dropped_count() {
        let mut log = WarningLog::new();
        for i in 0..WarningLog::CAPACITY + 5 {
            log.push(format!("warning {i}"));
        }
        // Storage is capped: the log stops growing at CAPACITY.
        assert_eq!(log.len(), WarningLog::CAPACITY);
        assert_eq!(log.dropped(), 5);
        // The first CAPACITY messages are kept verbatim.
        assert_eq!(log[0], "warning 0");
        assert_eq!(
            log[WarningLog::CAPACITY - 1],
            format!("warning {}", WarningLog::CAPACITY - 1)
        );

        // Draining appends exactly one summary entry with the dropped count.
        let drained = log.take();
        assert_eq!(drained.len(), WarningLog::CAPACITY + 1);
        let summary = drained.last().unwrap();
        assert!(
            summary.contains("5 additional warning(s) were dropped"),
            "unexpected summary: {summary}"
        );

        // The counter resets after draining.
        assert!(log.is_empty());
        assert_eq!(log.dropped(), 0);
        log.push("fresh".to_string());
        assert_eq!(log.take(), vec!["fresh".to_string()]);
    }
}
