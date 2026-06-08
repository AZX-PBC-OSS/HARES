//! Unified regression runner.
//!
//! Aggregates all validation sub-suites into one entry point so that
//! `cargo test --test regression -- --ignored` exercises every release gate.

#[path = "../parity/mod.rs"]
mod parity;

#[path = "../bestest/mod.rs"]
mod bestest;

mod aggregation_check;
mod checkpoint_restart;
mod determinism;
mod fleet_scale;
mod helpers;
mod multi_instance;
use std::fmt;

#[derive(Debug)]
struct RegressionFailure {
    suite: &'static str,
    detail: String,
}

impl fmt::Display for RegressionFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.suite, self.detail)
    }
}

#[test]
fn regression_full_suite() {
    let mut failures: Vec<RegressionFailure> = Vec::new();

    // --- Determinism ---
    if let Err(errs) = determinism::run_determinism_checks() {
        for detail in errs {
            failures.push(RegressionFailure {
                suite: "determinism",
                detail,
            });
        }
    }

    // --- Fleet scale ---
    if let Err(errs) = fleet_scale::run_fleet_scale_check() {
        for detail in errs {
            failures.push(RegressionFailure {
                suite: "fleet_scale",
                detail,
            });
        }
    }

    // --- Checkpoint restart ---
    if let Err(errs) = checkpoint_restart::run_checkpoint_restart_check() {
        for detail in errs {
            failures.push(RegressionFailure {
                suite: "checkpoint_restart",
                detail,
            });
        }
    }

    // --- Per-equipment checkpoint version rejection ---
    if let Err(errs) = checkpoint_restart::run_per_equipment_version_rejection_regression() {
        for detail in errs {
            failures.push(RegressionFailure {
                suite: "checkpoint_version_rejection",
                detail,
            });
        }
    }

    // --- Checkpoint integrity round-trip (CRC32 + SHA-256) ---
    if let Err(errs) = checkpoint_restart::run_checkpoint_integrity_roundtrip() {
        for detail in errs {
            failures.push(RegressionFailure {
                suite: "checkpoint_integrity",
                detail,
            });
        }
    }

    // --- Checkpoint SHA-256 corruption detection ---
    if let Err(errs) = checkpoint_restart::run_checkpoint_sha256_corruption_regression() {
        for detail in errs {
            failures.push(RegressionFailure {
                suite: "checkpoint_sha256_corruption",
                detail,
            });
        }
    }

    // --- Equipment CRC32 corruption detection ---
    if let Err(errs) = checkpoint_restart::run_equipment_crc_corruption_regression() {
        for detail in errs {
            failures.push(RegressionFailure {
                suite: "equipment_crc_corruption",
                detail,
            });
        }
    }

    // --- Actor state checkpoint round-trip ---
    if let Err(errs) = checkpoint_restart::run_actor_state_checkpoint_roundtrip() {
        for detail in errs {
            failures.push(RegressionFailure {
                suite: "actor_state_checkpoint",
                detail,
            });
        }
    }

    // --- Multi-instance independence ---
    if let Err(errs) = multi_instance::run_multi_instance_check() {
        for detail in errs {
            failures.push(RegressionFailure {
                suite: "multi_instance",
                detail,
            });
        }
    }

    // --- Weighted aggregation ---
    if let Err(errs) = aggregation_check::run_aggregation_check() {
        for detail in errs {
            failures.push(RegressionFailure {
                suite: "aggregation_check",
                detail,
            });
        }
    }

    if !failures.is_empty() {
        let report = failures
            .iter()
            .map(|f| f.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        panic!(
            "regression suite had {} failure(s):\n{}",
            failures.len(),
            report
        );
    }
}
