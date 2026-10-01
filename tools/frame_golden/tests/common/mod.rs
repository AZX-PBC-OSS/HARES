//! Shared test helpers: small hand-built RecordBatches and golden
//! documents for the digest and compare unit tests.
//!
//! Each integration-test binary compiles this module separately, so not
//! every helper is used by every binary; the unused ones are not dead code
//! in the module itself.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::Arc;

use arrow::array::{ArrayRef, Float64Array, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema};

use frame_golden::adapter::FrameProducts;
use frame_golden::golden::{GoldenDoc, MetricsRow};

/// A logical frame row for the chunking tests.
pub struct Row {
    pub time: String,
    pub power: Option<f64>,
    pub temperature: Option<f64>,
}

/// Deterministic rows: a Time string, a power column with a null every
/// `null_every` rows, and a temperature column with the complementary
/// null pattern so both null and non-null cells exist everywhere.
pub fn rows(count: usize, null_every: usize) -> Vec<Row> {
    (0..count)
        .map(|row| Row {
            time: format!("2023-01-01T{row:02}:00:00-07:00"),
            power: if null_every > 0 && row % null_every == 0 {
                None
            } else {
                Some(row as f64 * 0.5 + 0.25)
            },
            temperature: if null_every > 0 && row % null_every == 1 {
                None
            } else {
                Some(20.0 + row as f64 * 0.01)
            },
        })
        .collect()
}

pub fn frame(rows: &[Row]) -> RecordBatch {
    let mut time = Vec::with_capacity(rows.len());
    let mut power = Vec::with_capacity(rows.len());
    let mut temperature = Vec::with_capacity(rows.len());
    for row in rows {
        time.push(Some(row.time.as_str()));
        power.push(row.power);
        temperature.push(row.temperature);
    }
    let schema = Schema::new(vec![
        Field::new("Time", DataType::Utf8, false),
        Field::new("Power (kW)", DataType::Float64, true),
        Field::new("Temperature (C)", DataType::Float64, true),
    ]);
    RecordBatch::try_new(
        Arc::new(schema),
        vec![
            utf8_column(&time),
            f64_column(&power),
            f64_column(&temperature),
        ],
    )
    .expect("hand-built batch is valid")
}

/// Splits rows into consecutive batches of `size` rows (the last batch may
/// be short), the chunking pattern the digest must be independent of.
pub fn chunked(rows: &[Row], size: usize) -> Vec<RecordBatch> {
    rows.chunks(size).map(frame).collect()
}

pub fn utf8_column(values: &[Option<&str>]) -> ArrayRef {
    Arc::new(StringArray::from(values.to_vec()))
}

pub fn utf8_column_owned(values: Vec<String>) -> ArrayRef {
    Arc::new(StringArray::from(values))
}

pub fn f64_column(values: &[Option<f64>]) -> ArrayRef {
    Arc::new(Float64Array::from(values.to_vec()))
}

pub fn batch(schema: Schema, columns: Vec<ArrayRef>) -> RecordBatch {
    RecordBatch::try_new(Arc::new(schema), columns).expect("hand-built batch is valid")
}

/// A frame product map holding one product named `frame`.
pub fn frame_map(batch: RecordBatch) -> BTreeMap<String, FrameProducts> {
    let mut products = BTreeMap::new();
    products.insert(
        "frame".to_string(),
        FrameProducts::from_batches(vec![batch]).expect("non-empty batch list"),
    );
    products
}

/// A golden document around the given product digests, with empty metrics
/// and a fixed health record so the frame logic is what the test varies.
pub fn golden_doc(products: BTreeMap<String, frame_golden::FrameDigests>) -> GoldenDoc {
    let mut metrics = MetricsRow::new();
    metrics.insert(
        "total_energy_kwh.net_energy_kwh".to_string(),
        frame_golden::MetricValue::Field(frame_golden::MetricField {
            bits: Some("0x4059000000000000".to_string()),
            value: Some(100.0),
        }),
    );
    GoldenDoc {
        kind: "dwelling".to_string(),
        features: Vec::new(),
        defaults_dir: "defaults".to_string(),
        defaults_digest: "0".repeat(64),
        git_head: "test".to_string(),
        git_dirty: Some(false),
        metrics: vec![metrics],
        products,
        health: Some(serde_json::json!({
            "port_rollbacks": 0,
            "rejected_control_signals": 0,
            "clamped_actions": 0,
            "curve_index_clamps": 0,
            "warmup": "Disabled"
        })),
    }
}

/// The fresh health value matching [`golden_doc`].
pub fn fresh_health() -> serde_json::Value {
    serde_json::json!({
        "port_rollbacks": 0,
        "rejected_control_signals": 0,
        "clamped_actions": 0,
        "curve_index_clamps": 0,
        "warmup": "Disabled"
    })
}

/// The fresh metrics row matching [`golden_doc`].
pub fn fresh_metrics() -> Vec<MetricsRow> {
    let mut row = MetricsRow::new();
    row.insert(
        "total_energy_kwh.net_energy_kwh".to_string(),
        frame_golden::MetricValue::Field(frame_golden::MetricField {
            bits: Some("0x4059000000000000".to_string()),
            value: Some(100.0),
        }),
    );
    vec![row]
}
