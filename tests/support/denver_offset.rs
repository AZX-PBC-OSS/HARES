//! The standard offset of the Denver fixtures, included by path into each
//! test and benchmark target that needs it.

use chrono::FixedOffset;

/// UTC-7, the standard offset the Denver fixtures' weather and schedules are
/// written in.
pub fn denver_offset() -> FixedOffset {
    FixedOffset::west_opt(7 * 3600).expect("UTC-7 offset")
}
