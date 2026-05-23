//! Regression tests for occupant count silently returning 0.0 when
//! the schedule domain payload is absent at step time.
//!
//! ## What the bug is
//!
//! `apply_occupancy_gains()` in `dwelling/mod.rs` calls `.unwrap_or(0.0)` on
//! the schedule-domain payload lookup.  When no SCHEDULE_DOMAIN_ID update is
//! present in `latest_env`, the dwelling computes zero occupants and deposits
//! zero internal gains — indistinguishable from "the schedule says 0 occupants
//! this hour".  No warning, no error, no diagnostic.
//!
//! ## Why private-field tests live inside `mod.rs`
//!
//! The private fields (`occupancy_column_idx`, `occupancy_scale`,
//! `latest_env`, `apply_occupancy_gains`) are inaccessible from outside the
//! crate.  The unit test that exercises the private path already lives inside
//! `dwelling/mod.rs` (`occupancy_gains_scaled_by_number_of_occupants`).
//!
//! The private-path regression (silent zero when domain update is absent mid-
//! simulation) is tracked in the `#[cfg(test)]` module inside `mod.rs`.
//!
//! ## What this file tests
//!
//! Construction-time behaviour observable through the public API: a synthetic
//! TOML fixture with an explicitly-zero occupancy schedule builds and steps
//! without error.  Once fixed, legitimately unoccupied dwellings
//! must declare `occupants_present: false`, and dwellings with an Occupancy
//! spec but no schedule column must error at construction.

use std::path::PathBuf;

use hares_core::Dwelling;

/// A fixture with `occupancy = 0.0` in the schedule must build successfully
/// and simulate without error.  This is the "legitimately zero occupancy" path.
///
/// Once fixed, this fixture must additionally declare
/// `occupants_present: false` in the TOML for the construction to succeed;
/// without it the builder should reject the configuration with
/// `MissingScheduleDomain { domain: "occupants" }`.
#[test]
fn zero_occupancy_schedule_does_not_error_at_construction() {
    let base_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
    let result = Dwelling::from_toml_config_with_write_output(&base_path, Some(false));
    assert!(
        result.is_ok(),
        "Dwelling construction must succeed for BESTEST 600 (zero-occupancy fixture): {:?}",
        result.err()
    );
}

/// Stepping a zero-occupancy BESTEST dwelling for one step must succeed.
/// This documents the current silent-zero path through the public API; once
/// fixed the step must only succeed when `occupants_present: false` is declared.
#[test]
fn zero_occupancy_step_succeeds() {
    let base_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
    let mut dwelling = Dwelling::from_toml_config_with_write_output(&base_path, Some(false))
        .expect("build dwelling");

    let result = dwelling.step();
    assert!(
        result.is_ok(),
        "zero-occupancy fixture must step without error: {:?}",
        result.err()
    );
}
