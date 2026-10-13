//! Determinism sub-suite: one building's blueprint gives the same run
//! alone and twice concurrently on two threads, the three frames and
//! metrics compared bitwise.
//!
//! Uses the real BEopt example + ResStock buildings from the OCHRE vendor
//! directory so the simulation exercises realistic equipment and envelope
//! paths. The blueprint is the row-61 sharing model: the parsed inputs
//! are the `Arc`s every build reads, each build owns its mutable state,
//! and nothing a step writes is shared.

use std::path::Path;
use std::sync::Arc;

use arrow::array::Array;
use arrow::datatypes::DataType;
use arrow::record_batch::RecordBatch;
use chrono::Duration;
use hares_core::{DwellingBlueprint, DwellingConfig, RunMetrics};
use hares_io::Destination;

use super::helpers;

type BuildingBuilder = Box<dyn Fn(&Path, i64, Duration, u64) -> DwellingConfig>;
const SIM_DURATION_HOURS: i64 = 24;
const SEED: u64 = 42;

/// One run's comparable products: the frame batches and the finished
/// run's metrics.
struct RunRecord {
    batches: Vec<RecordBatch>,
    metrics: Option<RunMetrics>,
}

fn run_one(blueprint: &Arc<DwellingBlueprint>) -> RunRecord {
    let mut dwelling = blueprint.build().expect("the build");
    let result = dwelling.simulate().expect("the run completes");
    RunRecord {
        batches: dwelling.flushed_batches().to_vec(),
        metrics: result.metrics.clone(),
    }
}

/// Every column of every batch compared bitwise: float64 columns by their
/// bit patterns (so a sign flip or a payload difference cannot hide),
/// other columns by their buffer bytes. The mismatches collect, the
/// first per column suffices.
fn frame_mismatches(a: &[RecordBatch], b: &[RecordBatch], context: &str) -> Vec<String> {
    let mut failures = Vec::new();
    if a.len() != b.len() {
        failures.push(format!("{context}: batch count {} vs {}", a.len(), b.len()));
        return failures;
    }
    for (batch_index, (x, y)) in a.iter().zip(b.iter()).enumerate() {
        if x.num_rows() != y.num_rows() {
            failures.push(format!(
                "{context}: batch {batch_index} rows {} vs {}",
                x.num_rows(),
                y.num_rows()
            ));
            continue;
        }
        if x.num_columns() != y.num_columns() {
            failures.push(format!(
                "{context}: batch {batch_index} columns {} vs {}",
                x.num_columns(),
                y.num_columns()
            ));
            continue;
        }
        for (col, (cx, cy)) in x.columns().iter().zip(y.columns()).enumerate() {
            if cx.data_type() != cy.data_type() {
                failures.push(format!(
                    "{context}: batch {batch_index} column {col} type {} vs {}",
                    cx.data_type(),
                    cy.data_type()
                ));
                continue;
            }
            if cx.data_type() == &DataType::Float64 {
                let Some(xa) = cx.as_any().downcast_ref::<arrow::array::Float64Array>() else {
                    continue;
                };
                let Some(ya) = cy.as_any().downcast_ref::<arrow::array::Float64Array>() else {
                    continue;
                };
                for row in 0..cx.len() {
                    if xa.is_null(row) != ya.is_null(row) {
                        failures.push(format!(
                            "{context}: batch {batch_index} column {col} row {row} null"
                        ));
                        break;
                    }
                    if !xa.is_null(row) && xa.value(row).to_bits() != ya.value(row).to_bits() {
                        failures.push(format!(
                            "{context}: batch {batch_index} column {col} row {row} differs \
                             bitwise: {:x?} vs {:x?}",
                            xa.value(row).to_bits(),
                            ya.value(row).to_bits()
                        ));
                        break;
                    }
                }
            } else {
                let xb = cx.into_data();
                let yb = cy.into_data();
                for (slot, (bx, by)) in xb.buffers().iter().zip(yb.buffers()).enumerate() {
                    if bx.as_slice() != by.as_slice() {
                        failures.push(format!(
                            "{context}: batch {batch_index} column {col} buffer {slot} bytes"
                        ));
                        break;
                    }
                }
                if xb.nulls().map(|n| n.validity()).map(|s| s.to_vec())
                    != yb.nulls().map(|n| n.validity()).map(|s| s.to_vec())
                {
                    failures.push(format!(
                        "{context}: batch {batch_index} column {col} validity"
                    ));
                }
            }
        }
    }
    failures
}

/// The metrics compared bitwise: the postcard bytes, every f64 by its
/// IEEE-754 bytes (BTreeMaps inside, so the encoding is deterministic).
fn metrics_mismatch(a: &Option<RunMetrics>, b: &Option<RunMetrics>, context: &str) -> Vec<String> {
    let Ok(a_bytes) = postcard::to_allocvec(a) else {
        return vec![format!("{context}: the metrics failed to serialize")];
    };
    let Ok(b_bytes) = postcard::to_allocvec(b) else {
        return vec![format!("{context}: the metrics failed to serialize")];
    };
    if a_bytes == b_bytes {
        Vec::new()
    } else {
        vec![format!("{context}: metrics differ bitwise")]
    }
}

pub fn run_determinism_checks() -> Result<(), Vec<String>> {
    helpers::assert_vendor_fixtures_exist();

    let mut failures = Vec::new();
    let duration = Duration::hours(SIM_DURATION_HOURS);
    let output_dir = tempfile::tempdir().expect("create output directory");

    let buildings: Vec<(&str, BuildingBuilder)> = vec![
        ("beopt_1", Box::new(helpers::build_beopt_dwelling_config)),
        ("beopt_2", Box::new(helpers::build_beopt_dwelling_config)),
        (
            "resstock",
            Box::new(helpers::build_resstock_dwelling_config),
        ),
    ];

    for (label, builder) in &buildings {
        let mut config = builder(output_dir.path(), 1, duration, SEED);
        // Memory reports only: the file report would give the concurrent
        // runs one shared destination path; the compare reads the memory
        // batches.
        config
            .settings
            .sim_config
            .reports
            .retain(|report| matches!(report.destination, Destination::Memory));

        let blueprint = match DwellingBlueprint::from_config(config) {
            Ok(blueprint) => blueprint,
            Err(err) => {
                failures.push(format!(
                    "building={label}: the blueprint build failed: {err}"
                ));
                continue;
            }
        };

        // Once alone.
        let alone = run_one(&blueprint);

        // Twice concurrently on two threads, one build per thread.
        let concurrent: Vec<RunRecord> = (0..2)
            .map(|_| {
                let bp = Arc::clone(&blueprint);
                std::thread::spawn(move || run_one(&bp))
            })
            .map(|handle| handle.join().expect("the concurrent thread"))
            .collect();

        for (context, record) in concurrent.iter().enumerate() {
            failures.extend(frame_mismatches(
                &alone.batches,
                &record.batches,
                &format!("building={label} run {context}"),
            ));
            failures.extend(metrics_mismatch(
                &alone.metrics,
                &record.metrics,
                &format!("building={label} run {context}"),
            ));
        }

        if failures.is_empty() {
            eprintln!("[determinism] {label}: alone and 2x concurrent bitwise identical");
        }
    }

    if failures.is_empty() {
        eprintln!("[determinism] PASS -- 3 buildings, alone and twice concurrent, bitwise");
        Ok(())
    } else {
        Err(failures)
    }
}
