//! Parse counters for the parsed-input sharing proof: the number of input
//! documents each parse layer has read, process-wide, plus the calling
//! thread's own mirror of the same counts. The many-home construction
//! (the fleet's shared parses) reads each unique input once; these
//! counters are the proof instrument, and a `doc(hidden)` diagnostic
//! surface, not an API commitment.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

static HPXML_PARSES: AtomicU64 = AtomicU64::new(0);
static WEATHER_PARSES: AtomicU64 = AtomicU64::new(0);
static SCHEDULE_PARSES: AtomicU64 = AtomicU64::new(0);
static DEFAULTS_LOADS: AtomicU64 = AtomicU64::new(0);
static PROFILE_LOADS: AtomicU64 = AtomicU64::new(0);

/// The calling thread's own mirror of the five counters: the snapshot the
/// in-process probes read, because a sibling test on another thread parses
/// concurrently and the process-global counters move under its samples.
thread_local! {
    static THREAD_PARSE_COUNTS: ParseCountsCell = ParseCountsCell::default();
}

#[derive(Default)]
struct ParseCountsCell {
    hpxml: Cell<u64>,
    weather: Cell<u64>,
    schedule: Cell<u64>,
    defaults: Cell<u64>,
    profiles: Cell<u64>,
}

/// The parse counts, read as one snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ParseCounts {
    /// Full HPXML document parses (`parse_hpxml`, path or content).
    pub hpxml: u64,
    /// Weather file parses (`parse_weather` and the format parsers, path
    /// or content).
    pub weather: u64,
    /// Schedule CSV parses (path or content; a generated schedule counts
    /// none: it is computed, not parsed).
    pub schedule: u64,
    /// Defaults tree loads (`DefaultsStore::load`) and the per-file
    /// replacement's group re-parses (`ParsedDefaults::with_file`).
    pub defaults: u64,
    /// Default schedule profile loads (`load_default_profiles`).
    pub profiles: u64,
}

#[doc(hidden)]
pub fn bump_hpxml() {
    HPXML_PARSES.fetch_add(1, Ordering::Relaxed);
    THREAD_PARSE_COUNTS.with(|c| c.hpxml.set(c.hpxml.get() + 1));
}

#[doc(hidden)]
pub fn bump_weather() {
    WEATHER_PARSES.fetch_add(1, Ordering::Relaxed);
    THREAD_PARSE_COUNTS.with(|c| c.weather.set(c.weather.get() + 1));
}

#[doc(hidden)]
pub fn bump_schedule() {
    SCHEDULE_PARSES.fetch_add(1, Ordering::Relaxed);
    THREAD_PARSE_COUNTS.with(|c| c.schedule.set(c.schedule.get() + 1));
}

#[doc(hidden)]
pub fn bump_defaults() {
    DEFAULTS_LOADS.fetch_add(1, Ordering::Relaxed);
    THREAD_PARSE_COUNTS.with(|c| c.defaults.set(c.defaults.get() + 1));
}

#[doc(hidden)]
pub fn bump_profiles() {
    PROFILE_LOADS.fetch_add(1, Ordering::Relaxed);
    THREAD_PARSE_COUNTS.with(|c| c.profiles.set(c.profiles.get() + 1));
}

/// The calling thread's own parse counts: the snapshot for tests that prove
/// a construction path parsed each unique input once while the runner runs
/// sibling tests on other threads of the same process.
#[doc(hidden)]
pub fn parse_counts_thread() -> ParseCounts {
    THREAD_PARSE_COUNTS.with(|c| ParseCounts {
        hpxml: c.hpxml.get(),
        weather: c.weather.get(),
        schedule: c.schedule.get(),
        defaults: c.defaults.get(),
        profiles: c.profiles.get(),
    })
}

/// A snapshot of every parse counter, for tests that prove a construction
/// path parsed each unique input once.
#[doc(hidden)]
pub fn parse_counts() -> ParseCounts {
    ParseCounts {
        hpxml: HPXML_PARSES.load(Ordering::Relaxed),
        weather: WEATHER_PARSES.load(Ordering::Relaxed),
        schedule: SCHEDULE_PARSES.load(Ordering::Relaxed),
        defaults: DEFAULTS_LOADS.load(Ordering::Relaxed),
        profiles: PROFILE_LOADS.load(Ordering::Relaxed),
    }
}
