//! Parse counters for the parsed-input sharing proof: the number of input
//! documents each parse layer has read, process-wide. The many-home
//! construction (the fleet's shared parses) reads each unique input once;
//! these counters are the proof instrument, and a `doc(hidden)` diagnostic
//! surface, not an API commitment.

use std::sync::atomic::{AtomicU64, Ordering};

static HPXML_PARSES: AtomicU64 = AtomicU64::new(0);
static WEATHER_PARSES: AtomicU64 = AtomicU64::new(0);
static SCHEDULE_PARSES: AtomicU64 = AtomicU64::new(0);
static DEFAULTS_LOADS: AtomicU64 = AtomicU64::new(0);
static PROFILE_LOADS: AtomicU64 = AtomicU64::new(0);

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
    /// Defaults tree loads (`DefaultsStore::load`).
    pub defaults: u64,
    /// Default schedule profile loads (`load_default_profiles`).
    pub profiles: u64,
}

#[doc(hidden)]
pub fn bump_hpxml() {
    HPXML_PARSES.fetch_add(1, Ordering::Relaxed);
}

#[doc(hidden)]
pub fn bump_weather() {
    WEATHER_PARSES.fetch_add(1, Ordering::Relaxed);
}

#[doc(hidden)]
pub fn bump_schedule() {
    SCHEDULE_PARSES.fetch_add(1, Ordering::Relaxed);
}

#[doc(hidden)]
pub fn bump_defaults() {
    DEFAULTS_LOADS.fetch_add(1, Ordering::Relaxed);
}

#[doc(hidden)]
pub fn bump_profiles() {
    PROFILE_LOADS.fetch_add(1, Ordering::Relaxed);
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
