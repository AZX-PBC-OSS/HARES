//! The fixed start of every run built on the synthetic fixtures, included by
//! path into each test and benchmark target that needs it.

use chrono::{DateTime, FixedOffset, TimeZone};

/// Local midnight on 2021-01-01, the first row of the synthetic schedules, at
/// the Denver fixtures' standard offset (UTC-7). Fixed, so a run's weather,
/// schedule and solar position never depend on the date it runs.
pub fn fixture_start() -> DateTime<FixedOffset> {
    FixedOffset::west_opt(7 * 3600)
        .expect("UTC-7 offset")
        .with_ymd_and_hms(2021, 1, 1, 0, 0, 0)
        .single()
        .expect("valid local midnight")
}
