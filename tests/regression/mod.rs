//! Unified regression runner (HARES-057).
//!
//! Aggregates all validation sub-suites into one entry point so that
//! `cargo test --test regression -- --ignored` exercises every release gate.

#[path = "../parity/mod.rs"]
mod parity;

#[path = "../bestest/mod.rs"]
mod bestest;

mod determinism;
mod fleet_scale;
mod checkpoint_restart;
mod multi_instance;
mod aggregation_check;

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
#[ignore = "long-running regression corpus — runs all validation sub-suites"]
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
                suite: "aggregation",
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
