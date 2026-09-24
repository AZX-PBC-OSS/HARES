//! Regression tests for occupant count silently returning 0.0 when
//! the schedule domain payload is absent at step time.
//!
//! ## What the bug was
//!
//! `apply_occupancy_gains()` in `dwelling/mod.rs` called `.unwrap_or(0.0)` on
//! the schedule-domain payload lookup.  When no SCHEDULE_DOMAIN_ID update was
//! present in `latest_env`, the dwelling computed zero occupants and deposited
//! zero internal gains — indistinguishable from "the schedule says 0 occupants
//! this hour".  No warning, no error, no diagnostic.
//!
//! ## What the fix does
//!
//! 1. Construction-time validation rejects dwellings that configure an Occupancy
//!    spec but have no occupancy column in the schedule.
//! 2. The `occupants_present: false` flag (default true) on synthetic TOML
//!    schedule configs tells the builder to skip the occupancy schedule
//!    column entirely for legitimately unoccupied dwellings (e.g. BESTEST).
//! 3. The hot-path `.unwrap_or(0.0)` is replaced by `.expect()` — absence at
//!    step time is a programming error because construction validated the
//!    schedule domain exists.

use std::path::PathBuf;

use hares_core::Dwelling;

/// A fixture with `occupants_present = false` must build successfully and
/// step without error.  This is the "legitimately unoccupied" path for
/// BESTEST base cases.
#[test]
fn unoccupied_dwelling_builds_and_steps() {
    let base_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
    let mut dwelling = Dwelling::from_toml_config_with_write_output(&base_path, Some(false))
        .expect("BESTEST 600 (occupants_present = false) must build");
    let result = dwelling.step();
    assert!(
        result.is_ok(),
        "unoccupied dwelling must step without error: {:?}",
        result.err()
    );
}

/// BESTEST 600 with `occupants_present = false` has no occupancy schedule
/// column and no Occupancy equipment spec.  Construction must succeed
/// without error — this exercises the path where neither an occupancy
/// schedule column nor an Occupancy spec is configured.
#[test]
fn no_occupancy_spec_and_no_column_builds() {
    let base_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
    let result = Dwelling::from_toml_config_with_write_output(&base_path, Some(false));
    assert!(
        result.is_ok(),
        "BESTEST 600 (no spec, no column) must build: {:?}",
        result.err()
    );
}
