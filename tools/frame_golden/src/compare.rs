//! Comparison logic: `compare`, `delta` and `diff`.
//!
//! `compare` checks a fresh run against the committed golden digests;
//! `delta` and `diff` check two full-frame sets against each other at the
//! value level. All difference reports are structured values the CLI
//! renders, so tests assert on structure instead of output text.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, BooleanArray, StringArray};
use arrow::datatypes::DataType;
use arrow::record_batch::RecordBatch;

use crate::digest::{
    FrameDigests, digest_frame, first_digest_difference, format_column_value, numeric_value,
    value_bytes,
};
use crate::error::{FrameGoldenError, FrameGoldenResult};
use crate::frames;
use crate::golden::{GoldenDoc, first_metrics_differences};

/// Column selection for `compare` and `delta`: when set, only the listed
/// columns are compared, and each must exist on both sides.
pub type ColumnSelection = Option<Vec<String>>;

/// One difference found by `compare`.
#[derive(Debug, Clone, PartialEq)]
pub enum Difference {
    /// The committed golden names a product the fresh run does not produce.
    MissingProduct {
        product: String,
    },
    /// The fresh run produces a product the committed golden does not hold.
    ExtraProduct {
        product: String,
    },
    ProductSchema {
        product: String,
        detail: String,
    },
    RowCount {
        product: String,
        expected: usize,
        actual: usize,
    },
    ColumnDigest {
        product: String,
        column: String,
        block: usize,
    },
    MetricsRowCount {
        expected: usize,
        actual: usize,
    },
    MetricsField {
        field: String,
        expected: String,
        actual: String,
    },
    Health {
        expected: String,
        actual: String,
    },
}

/// The first differing row, reported only when full frames whose digests
/// equal the committed golden are available.
#[derive(Debug, Clone, PartialEq)]
pub struct RowMismatch {
    pub column: String,
    pub row: usize,
    pub expected: String,
    pub actual: String,
    /// Differing cell count per column, in schema order.
    pub per_column_cells: Vec<(String, usize)>,
}

/// The result of one `compare`.
#[derive(Debug, Clone, PartialEq)]
pub struct CompareReport {
    pub name: String,
    pub compared: Vec<String>,
    pub differences: Vec<Difference>,
    pub row_mismatch: Option<RowMismatch>,
    pub notes: Vec<String>,
}

impl CompareReport {
    pub fn is_identical(&self) -> bool {
        self.differences.is_empty()
    }
}

/// Per-column statistics over one differing column.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnDelta {
    pub column: String,
    pub differing_cells: usize,
    pub validity_changes: usize,
    /// Maximum absolute difference; null for non-numeric columns and for
    /// columns with no finite-value pair to compare.
    pub max_abs_difference: Option<f64>,
    /// Maximum |b - a| / max(|a|, |b|) over differing cells; 0.0 when both
    /// values are zero, null for non-numeric columns.
    pub max_relative_difference: Option<f64>,
}

/// The result of one `delta` or `diff`.
#[derive(Debug, Clone, PartialEq)]
pub struct DeltaReport {
    pub products_compared: Vec<String>,
    pub columns: Vec<ColumnDelta>,
    /// Metrics fields that changed: field, reference value, actual value.
    pub metrics_changes: Vec<(String, String, String)>,
    pub notes: Vec<String>,
}

impl DeltaReport {
    pub fn is_identical(&self) -> bool {
        self.columns.is_empty() && self.metrics_changes.is_empty()
    }
}

fn schema_detail(expected: &FrameDigests, actual: &FrameDigests) -> Option<String> {
    if expected.schema != actual.schema {
        return Some(format!(
            "schema differs:\n  expected: {:?}\n  actual:   {:?}",
            expected.schema, actual.schema
        ));
    }
    if expected.num_rows != actual.num_rows {
        return Some(format!(
            "row count differs: expected {}, actual {}",
            expected.num_rows, actual.num_rows
        ));
    }
    None
}

/// The columns to digest-compare for one product: all of them, or the
/// selected subset. Every selected column must exist on both sides.
fn selected_columns(
    product: &str,
    expected: &FrameDigests,
    actual: &FrameDigests,
    columns: &ColumnSelection,
) -> FrameGoldenResult<Vec<String>> {
    let Some(selection) = columns else {
        return Ok(expected.columns.iter().map(|c| c.name.clone()).collect());
    };
    for name in selection {
        let expected_has = expected.columns.iter().any(|c| &c.name == name);
        let actual_has = actual.columns.iter().any(|c| &c.name == name);
        if !expected_has || !actual_has {
            return Err(FrameGoldenError::Usage(format!(
                "product {product}: selected column {name:?} must exist on both sides \
                 (expected: {}, actual: {})",
                expected_has, actual_has
            )));
        }
    }
    Ok(selection.clone())
}

/// Compares a fresh run against the committed golden document.
///
/// When `frames_dir` holds full products whose digests equal the committed
/// golden, the report also carries the first differing row with both
/// values and bit patterns, plus per-column differing-cell counts.
pub fn compare_products(
    name: &str,
    golden: &GoldenDoc,
    fresh_frames: &BTreeMap<String, crate::adapter::FrameProducts>,
    fresh_metrics: &[crate::golden::MetricsRow],
    fresh_health: Option<&serde_json::Value>,
    columns: &ColumnSelection,
    frames_dir: Option<&Path>,
) -> FrameGoldenResult<CompareReport> {
    let mut report = CompareReport {
        name: name.to_string(),
        compared: Vec::new(),
        differences: Vec::new(),
        row_mismatch: None,
        notes: Vec::new(),
    };

    let mut fresh_digests = BTreeMap::new();
    for (product, frame) in fresh_frames {
        fresh_digests.insert(product.clone(), digest_frame(&frame.batches)?);
    }

    for (product, expected) in &golden.products {
        let Some(actual) = fresh_digests.get(product) else {
            report.differences.push(Difference::MissingProduct {
                product: product.clone(),
            });
            continue;
        };
        report.compared.push(product.clone());
        // Without a column selection the whole schema (types, nullability,
        // order) and row count are checked; with one, only the listed
        // columns are.
        if columns.is_none()
            && let Some(detail) = schema_detail(expected, actual)
        {
            report.differences.push(Difference::ProductSchema {
                product: product.clone(),
                detail,
            });
            continue;
        }
        if expected.num_rows != actual.num_rows {
            report.differences.push(Difference::RowCount {
                product: product.clone(),
                expected: expected.num_rows,
                actual: actual.num_rows,
            });
            continue;
        }
        let selection = selected_columns(product, expected, actual, columns)?;
        for column_name in &selection {
            let expected_column = expected
                .columns
                .iter()
                .find(|c| &c.name == column_name)
                .expect("selected_columns validates existence");
            let actual_column = actual
                .columns
                .iter()
                .find(|c| &c.name == column_name)
                .expect("selected_columns validates existence");
            // The selected column's schema entry must match too: same
            // Arrow type and nullability on both sides.
            let expected_field = expected
                .schema
                .iter()
                .find(|f| &f.name == column_name)
                .expect("schema lists every digested column");
            let actual_field = actual
                .schema
                .iter()
                .find(|f| &f.name == column_name)
                .expect("schema lists every digested column");
            if expected_field != actual_field {
                report
                    .differences
                    .push(Difference::ProductSchema {
                        product: product.clone(),
                        detail: format!(
                            "column {column_name:?} differs: expected {expected_field:?}, actual {actual_field:?}"
                        ),
                    });
                continue;
            }
            if expected_column.digest != actual_column.digest {
                let block = first_digest_difference(
                    &single_column(expected, column_name),
                    &single_column(actual, column_name),
                )
                .map(|location| location.block)
                .unwrap_or(0);
                report.differences.push(Difference::ColumnDigest {
                    product: product.clone(),
                    column: column_name.clone(),
                    block,
                });
            }
        }
    }
    for product in fresh_digests.keys() {
        if !golden.products.contains_key(product) {
            report.differences.push(Difference::ExtraProduct {
                product: product.clone(),
            });
        }
    }

    if golden.metrics.len() != fresh_metrics.len() {
        report.differences.push(Difference::MetricsRowCount {
            expected: golden.metrics.len(),
            actual: fresh_metrics.len(),
        });
    } else {
        for (index, (expected_row, actual_row)) in
            golden.metrics.iter().zip(fresh_metrics).enumerate()
        {
            for (field, expected_value, actual_value) in
                first_metrics_differences(expected_row, actual_row)
            {
                report.differences.push(Difference::MetricsField {
                    field: format!("home {index}: {field}"),
                    expected: expected_value,
                    actual: actual_value,
                });
            }
        }
    }

    match (&golden.health, fresh_health) {
        (Some(expected), Some(actual)) => {
            if expected != actual {
                report.differences.push(Difference::Health {
                    expected: serde_json::to_string_pretty(expected)?,
                    actual: serde_json::to_string_pretty(actual)?,
                });
            }
        }
        (Some(_), None) => report.differences.push(Difference::Health {
            expected: "present".to_string(),
            actual: "absent".to_string(),
        }),
        (None, Some(_)) => report.differences.push(Difference::Health {
            expected: "absent".to_string(),
            actual: "present".to_string(),
        }),
        (None, None) => {}
    }

    if let Some(dir) = frames_dir {
        // The row-level report needs the materialized reference frames of
        // the committed capture; when they are absent or have drifted, the
        // digest-level report above still stands.
        match read_reference_frames(dir, golden) {
            Ok(reference) => {
                if let Some(mismatch) = first_row_mismatch(fresh_frames, &reference, columns)? {
                    report.row_mismatch = Some(mismatch);
                }
            }
            Err(reason) => {
                report.notes.push(format!("no row-level report: {reason}"));
            }
        }
    }

    Ok(report)
}

fn single_column(digests: &FrameDigests, column: &str) -> FrameDigests {
    FrameDigests {
        num_rows: digests.num_rows,
        schema: digests
            .schema
            .iter()
            .filter(|f| f.name == column)
            .cloned()
            .collect(),
        columns: digests
            .columns
            .iter()
            .filter(|c| c.name == column)
            .cloned()
            .collect(),
    }
}

/// Reads the materialized frames of one capture and verifies their digests
/// equal the committed golden before they are used as references.
fn read_reference_frames(
    dir: &Path,
    golden: &GoldenDoc,
) -> FrameGoldenResult<BTreeMap<String, Vec<RecordBatch>>> {
    let mut reference = BTreeMap::new();
    for product in golden.products.keys() {
        let path = dir.join(format!("{product}.parquet"));
        let (_, batches) = frames::read_frame(&path)?;
        reference.insert(product.clone(), batches);
    }
    for (product, batches) in &reference {
        let digests = digest_frame(batches)?;
        let committed = golden
            .products
            .get(product)
            .expect("reference read iterates golden products");
        if &digests != committed {
            return Err(FrameGoldenError::MaterializedFrames(format!(
                "product {product}: materialized digests differ from the committed golden; \
                 re-run materialize to refresh the full frames"
            )));
        }
    }
    Ok(reference)
}

/// Value-level comparison of one column across two frames: differing-cell
/// count, validity changes, max absolute and relative difference, and the
/// first differing row with both rendered values.
#[derive(Debug, Clone, PartialEq)]
struct ColumnValueCompare {
    delta: ColumnDelta,
    first_mismatch: Option<(usize, String, String)>,
}

fn compare_column_values(
    expected: &ArrayRef,
    actual: &ArrayRef,
    expected_schema_field: &arrow::datatypes::Field,
) -> FrameGoldenResult<ColumnValueCompare> {
    if expected.len() != actual.len() {
        return Err(FrameGoldenError::Digest(format!(
            "column {}: row counts differ ({} vs {})",
            expected_schema_field.name(),
            expected.len(),
            actual.len()
        )));
    }
    let mut differing_cells = 0usize;
    let mut validity_changes = 0usize;
    let mut max_abs: Option<f64> = None;
    let mut max_rel: Option<f64> = None;
    let mut first_mismatch: Option<(usize, String, String)> = None;

    let numeric = matches!(
        expected.data_type(),
        DataType::Float64
            | DataType::Int64
            | DataType::Int32
            | DataType::UInt32
            | DataType::UInt64
            | DataType::Timestamp(_, _)
            | DataType::Duration(_)
    );

    for row in 0..expected.len() {
        let expected_valid = !expected.is_null(row);
        let actual_valid = !actual.is_null(row);
        if expected_valid != actual_valid {
            validity_changes += 1;
            differing_cells += 1;
            if first_mismatch.is_none() {
                first_mismatch = Some((
                    row,
                    format_column_value(expected, row),
                    format_column_value(actual, row),
                ));
            }
            continue;
        }
        if !expected_valid {
            continue;
        }
        let equal = values_bit_equal(expected, actual, row)?;
        if !equal {
            differing_cells += 1;
            if numeric {
                let (a, b) = numeric_pair(expected, actual, row)?;
                let abs = (b - a).abs();
                let rel = if a == 0.0 && b == 0.0 {
                    0.0
                } else {
                    (b - a).abs() / a.abs().max(b.abs())
                };
                // A NaN on either side is incomparably different from any
                // finite value; represent it as an infinite difference.
                let abs = if abs.is_nan() { f64::INFINITY } else { abs };
                let rel = if rel.is_nan() { f64::INFINITY } else { rel };
                max_abs = Some(max_abs.map_or(abs, |m: f64| m.max(abs)));
                max_rel = Some(max_rel.map_or(rel, |m: f64| m.max(rel)));
            }
            if first_mismatch.is_none() {
                first_mismatch = Some((
                    row,
                    format_column_value(expected, row),
                    format_column_value(actual, row),
                ));
            }
        }
    }

    Ok(ColumnValueCompare {
        delta: ColumnDelta {
            column: expected_schema_field.name().clone(),
            differing_cells,
            validity_changes,
            max_abs_difference: max_abs,
            max_relative_difference: max_rel,
        },
        first_mismatch,
    })
}

/// Bit-level equality of one cell through the canonical value bytes the
/// digests hash: equality here agrees with `digest_frame` by construction
/// and covers every type the digest supports, the fleet products'
/// Int64 and Boolean columns included.
fn values_bit_equal(expected: &ArrayRef, actual: &ArrayRef, row: usize) -> FrameGoldenResult<bool> {
    Ok(value_bytes(expected, row)? == value_bytes(actual, row)?)
}

fn numeric_pair(
    expected: &ArrayRef,
    actual: &ArrayRef,
    row: usize,
) -> FrameGoldenResult<(f64, f64)> {
    Ok((numeric_value(expected, row)?, numeric_value(actual, row)?))
}

fn type_error(array: &ArrayRef) -> FrameGoldenError {
    FrameGoldenError::Digest(format!(
        "column data {:?} does not match its declared type",
        array.data_type()
    ))
}

/// Value-level comparison of two full-frame product sets, restricted to
/// `columns` when set.
pub fn compare_frames_value_level(
    expected: &BTreeMap<String, Vec<RecordBatch>>,
    actual: &BTreeMap<String, Vec<RecordBatch>>,
    columns: &ColumnSelection,
) -> FrameGoldenResult<DeltaReport> {
    let mut report = DeltaReport {
        products_compared: Vec::new(),
        columns: Vec::new(),
        metrics_changes: Vec::new(),
        notes: Vec::new(),
    };
    for (product, expected_batches) in expected {
        let Some(actual_batches) = actual.get(product) else {
            report
                .notes
                .push(format!("product {product}: absent from the second side"));
            continue;
        };
        report.products_compared.push(product.clone());
        let expected_schema = crate::digest::schema_of(expected_batches)?;
        if crate::digest::schema_fields_of(expected_batches)?
            != crate::digest::schema_fields_of(actual_batches)?
        {
            report.notes.push(format!(
                "product {product}: schema differs, per-column statistics skipped"
            ));
            continue;
        }
        for index in 0..expected_schema.fields().len() {
            let field = expected_schema.field(index).clone();
            if let Some(selection) = columns
                && !selection.contains(field.name())
            {
                continue;
            }
            let expected_column: ArrayRef = concat_batches(expected_batches, index)?;
            let actual_column: ArrayRef = concat_batches(actual_batches, index)?;
            let compare = compare_column_values(&expected_column, &actual_column, &field)?;
            if compare.delta.differing_cells > 0 {
                report.columns.push(compare.delta);
            }
            if let Some((row, e, a)) = compare.first_mismatch {
                report.notes.push(format!(
                    "product {product} column {:?}: first differing row {row}: {e} vs {a}",
                    field.name()
                ));
            }
        }
    }
    for product in actual.keys() {
        if !expected.contains_key(product) {
            report
                .notes
                .push(format!("product {product}: absent from the first side"));
        }
    }
    Ok(report)
}

/// Materializes one column of a batch list into a single contiguous array
/// so value-level statistics can walk it by row.
fn concat_batches(batches: &[RecordBatch], index: usize) -> FrameGoldenResult<ArrayRef> {
    let mut arrays: Vec<ArrayRef> = Vec::with_capacity(batches.len());
    for batch in batches {
        arrays.push(batch.column(index).clone());
    }
    // A single batch needs no copy.
    if arrays.len() == 1 {
        return Ok(arrays.remove(0));
    }
    let mut out: Option<ArrayRef> = None;
    for array in arrays {
        out = Some(match out {
            None => array,
            Some(existing) => concat_two(&existing, &array)?,
        });
    }
    Ok(out.expect("at least one batch"))
}

/// Concatenates two arrays of the same type using builders for the types
/// frame products carry. Fixed-width columns read their physical value
/// bytes: the array types sharing one storage (Timestamp, Duration,
/// Date64 and Time64 over i64; Date32 over i32) do not downcast to each
/// other's builders, so the rebuilt array is retyped to the column's
/// declared type.
fn concat_two(a: &ArrayRef, b: &ArrayRef) -> FrameGoldenResult<ArrayRef> {
    use arrow::array::{
        BooleanBuilder, Float64Builder, Int32Builder, Int64Builder, StringBuilder, UInt32Builder,
        UInt64Builder,
    };

    macro_rules! concat_fixed_width {
        ($native:ty, $builder:ty, $read:ident) => {{
            let mut builder = <$builder>::new();
            for array in [a, b] {
                let data = array.to_data();
                for row in 0..array.len() {
                    if array.is_null(row) {
                        builder.append_null();
                    } else {
                        let raw = crate::digest::$read(&data, row)?;
                        builder.append_value(<$native>::from_ne_bytes(raw));
                    }
                }
            }
            retyped(builder.finish().into_data(), a.data_type().clone())?
        }};
    }

    Ok(match a.data_type() {
        DataType::Float64 => concat_fixed_width!(f64, Float64Builder, physical_value_8),
        DataType::Int64
        | DataType::Timestamp(_, _)
        | DataType::Duration(_)
        | DataType::Date64
        | DataType::Time64(_) => concat_fixed_width!(i64, Int64Builder, physical_value_8),
        DataType::Int32 | DataType::Date32 => {
            concat_fixed_width!(i32, Int32Builder, physical_value_4)
        }
        DataType::UInt32 => concat_fixed_width!(u32, UInt32Builder, physical_value_4),
        DataType::UInt64 => concat_fixed_width!(u64, UInt64Builder, physical_value_8),
        DataType::Boolean => {
            let mut builder = BooleanBuilder::new();
            for array in [a, b] {
                let values = array
                    .as_any()
                    .downcast_ref::<BooleanArray>()
                    .ok_or_else(|| type_error(array))?;
                for row in 0..values.len() {
                    if values.is_null(row) {
                        builder.append_null();
                    } else {
                        builder.append_value(values.value(row));
                    }
                }
            }
            Arc::new(builder.finish()) as ArrayRef
        }
        DataType::Utf8 => {
            let mut builder = StringBuilder::new();
            for array in [a, b] {
                let values = array
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .ok_or_else(|| type_error(array))?;
                for row in 0..values.len() {
                    if values.is_null(row) {
                        builder.append_null();
                    } else {
                        builder.append_value(values.value(row));
                    }
                }
            }
            Arc::new(builder.finish())
        }
        other => {
            return Err(FrameGoldenError::Digest(format!(
                "unsupported column type {other} for concatenation"
            )));
        }
    })
}

/// The builder's finished data retyped to the column's declared type: the
/// fixed-width families sharing one physical storage build through the
/// physical type's builder, and the output must keep the original data
/// type.
fn retyped(data: arrow::array::ArrayData, data_type: DataType) -> FrameGoldenResult<ArrayRef> {
    Ok(arrow::array::make_array(
        data.into_builder().data_type(data_type).build()?,
    ))
}

/// The first differing row across the compared products, reported per
/// column with both rendered values and differing-cell counts.
fn first_row_mismatch(
    fresh: &BTreeMap<String, crate::adapter::FrameProducts>,
    reference: &BTreeMap<String, Vec<RecordBatch>>,
    columns: &ColumnSelection,
) -> FrameGoldenResult<Option<RowMismatch>> {
    let mut per_column_cells: Vec<(String, usize)> = Vec::new();
    let mut first: Option<RowMismatch> = None;

    for (product, fresh_frame) in fresh {
        let Some(reference_batches) = reference.get(product) else {
            continue;
        };
        if crate::digest::schema_fields_of(&fresh_frame.batches)?
            != crate::digest::schema_fields_of(reference_batches)?
        {
            continue;
        }
        for index in 0..fresh_frame.schema.fields().len() {
            let field = fresh_frame.schema.field(index).clone();
            if let Some(selection) = columns
                && !selection.contains(field.name())
            {
                continue;
            }
            let fresh_column: ArrayRef = concat_batches(&fresh_frame.batches, index)?;
            let reference_column: ArrayRef = concat_batches(reference_batches, index)?;
            let compare = compare_column_values(&reference_column, &fresh_column, &field)?;
            per_column_cells.push((field.name().clone(), compare.delta.differing_cells));
            if let Some((row, expected, actual)) = compare.first_mismatch
                && first.is_none()
            {
                first = Some(RowMismatch {
                    column: format!("{product}.{}", field.name()),
                    row,
                    expected,
                    actual,
                    per_column_cells: Vec::new(),
                });
            }
        }
    }

    if let Some(first) = &mut first {
        first.per_column_cells = per_column_cells;
    }
    Ok(first)
}

/// Delta against the materialized full products: verifies the reference
/// frames against the committed digests first, then reports per-column
/// statistics. Always succeeds even with differences.
pub fn delta_products(
    name: &str,
    golden: &GoldenDoc,
    frames_dir: &Path,
    fresh_frames: &BTreeMap<String, crate::adapter::FrameProducts>,
    fresh_metrics: &[crate::golden::MetricsRow],
    columns: &ColumnSelection,
) -> FrameGoldenResult<DeltaReport> {
    if !frames_dir.exists() {
        return Err(FrameGoldenError::MaterializedFrames(format!(
            "{} does not exist: run `frame_golden materialize {name}` at the parent commit to obtain the reference frames",
            frames_dir.display()
        )));
    }
    // An empty directory is as unusable as a missing one: either way the
    // reference frames were never materialized.
    if frames_in_dir(frames_dir)?.is_empty() {
        return Err(FrameGoldenError::MaterializedFrames(format!(
            "{} holds no frame products: run `frame_golden materialize {name}` at the parent commit to obtain the reference frames",
            frames_dir.display()
        )));
    }
    let reference = read_reference_frames(frames_dir, golden)?;

    let mut reference_map = BTreeMap::new();
    for (product, batches) in &reference {
        reference_map.insert(product.clone(), batches.clone());
    }

    let mut report =
        compare_frames_value_level(&reference_map, &fresh_frame_map(fresh_frames), columns)?;

    for (index, (golden_row, fresh_row)) in golden.metrics.iter().zip(fresh_metrics).enumerate() {
        for (field, expected, actual) in first_metrics_differences(golden_row, fresh_row) {
            report
                .metrics_changes
                .push((format!("home {index}: {field}"), expected, actual));
        }
    }
    if golden.metrics.len() != fresh_metrics.len() {
        report.notes.push(format!(
            "metrics row count changed: {} -> {}",
            golden.metrics.len(),
            fresh_metrics.len()
        ));
    }
    Ok(report)
}

fn fresh_frame_map(
    frames: &BTreeMap<String, crate::adapter::FrameProducts>,
) -> BTreeMap<String, Vec<RecordBatch>> {
    frames
        .iter()
        .map(|(name, frame)| (name.clone(), frame.batches.clone()))
        .collect()
}

/// Diff of two directories of full products, same per-column report as
/// `delta`. Purely a report: the caller decides what the output means.
pub fn diff_dirs(dir_a: &Path, dir_b: &Path) -> FrameGoldenResult<DeltaReport> {
    let mut expected = BTreeMap::new();
    for product in frames_in_dir(dir_a)? {
        let (_, batches) = frames::read_frame(&product.path)?;
        expected.insert(product.name, batches);
    }
    let mut actual = BTreeMap::new();
    for product in frames_in_dir(dir_b)? {
        let (_, batches) = frames::read_frame(&product.path)?;
        actual.insert(product.name, batches);
    }
    compare_frames_value_level(&expected, &actual, &None)
}

struct ProductFile {
    name: String,
    path: std::path::PathBuf,
}

/// Lists the parquet products in one full-frames directory. Products are
/// flat files named after the product (`frame.parquet`, `aggregate.parquet`,
/// ...).
fn frames_in_dir(dir: &Path) -> FrameGoldenResult<Vec<ProductFile>> {
    if !dir.exists() {
        return Err(FrameGoldenError::MaterializedFrames(format!(
            "{} does not exist",
            dir.display()
        )));
    }
    let mut products = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            continue;
        }
        if path.extension().is_some_and(|ext| ext == "parquet") {
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            products.push(ProductFile { name, path });
        }
    }
    products.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(products)
}
