//! The step counter for the proofs that a construction path runs no step:
//! the number of completed timesteps the process has run, process-wide.
//! Like the parse counters these are a `doc(hidden)` diagnostic surface, a
//! test instrument and not an API commitment.

use std::sync::atomic::{AtomicU64, Ordering};

static STEPS_RUN: AtomicU64 = AtomicU64::new(0);

/// One completed timestep was run.
#[doc(hidden)]
pub fn bump() {
    STEPS_RUN.fetch_add(1, Ordering::Relaxed);
}

/// The steps the process has run so far.
#[doc(hidden)]
pub fn steps_run() -> u64 {
    STEPS_RUN.load(Ordering::Relaxed)
}

/// Zero the counter, for a proof that reads a construction path's delta.
#[doc(hidden)]
pub fn reset() {
    STEPS_RUN.store(0, Ordering::Relaxed);
}
