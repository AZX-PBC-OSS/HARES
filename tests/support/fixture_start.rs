//! The fixed start of every run built on the synthetic fixtures, included by
//! path into each test and benchmark target that needs it.

use chrono::{DateTime, FixedOffset, TimeZone};

#[path = "denver_offset.rs"]
mod denver_offset;

/// Local midnight on 2021-01-01, the first row of the synthetic schedules, at
/// the Denver fixtures' standard offset. Fixed, so a run's weather, schedule
/// and solar position never depend on the date it runs.
pub fn fixture_start() -> DateTime<FixedOffset> {
    denver_offset::denver_offset()
        .with_ymd_and_hms(2021, 1, 1, 0, 0, 0)
        .single()
        .expect("valid local midnight")
}
