//! Type conversions between Rust and Python.

use std::sync::Arc;

use arrow::datatypes::Schema;
use arrow::record_batch::RecordBatch;
use chrono::NaiveDateTime;
use pyo3::prelude::*;
use pyo3::types::PyAny;
use pyo3_arrow::PyTable;

/// The batches as pyo3-arrow's `PyTable`: the Arrow PyCapsule surface.
/// The buffers stay the batches' own (Arc-shared), so every consumer that
/// imports the stream (polars, pyarrow, pandas through pyarrow) reads
/// them zero-copy; no IPC serialize-and-parse round trip.
pub fn batches_to_py_table(batches: Vec<RecordBatch>) -> PyResult<PyTable> {
    let schema = batches
        .first()
        .map(|batch| batch.schema())
        .unwrap_or_else(|| Arc::new(Schema::empty()));
    PyTable::try_new(batches, schema)
}

/// The table as a polars `DataFrame`: the frame reads the table's stream
/// through the capsule, over the same shared buffers.
pub fn py_table_to_polars_df(py: Python<'_>, table: &Bound<'_, PyTable>) -> PyResult<Py<PyAny>> {
    let polars = py.import("polars")?;
    let df = polars.getattr("DataFrame")?.call1((table,))?;
    Ok(df.unbind())
}

/// Convert a naive local wall-clock time into a naive Python `datetime`:
/// the representation the step dict's timestamp and the environment's
/// `current_time` carry, the value the frame's Time column spells.
pub fn naive_chrono_to_py_datetime(py: Python<'_>, dt: NaiveDateTime) -> PyResult<Py<PyAny>> {
    let datetime = py.import("datetime")?.getattr("datetime")?;
    let obj = datetime.call_method1(
        "fromisoformat",
        (dt.format("%Y-%m-%dT%H:%M:%S%.f").to_string(),),
    )?;
    Ok(obj.unbind())
}

#[cfg(test)]
pub(crate) fn assert_batches_bitwise_equal(
    label: &str,
    expected: &[RecordBatch],
    actual: &[RecordBatch],
) {
    assert_eq!(
        expected.len(),
        actual.len(),
        "{label}: the batch count differs"
    );
    for (index, (expected_batch, actual_batch)) in expected.iter().zip(actual.iter()).enumerate() {
        assert_eq!(
            expected_batch.num_columns(),
            actual_batch.num_columns(),
            "{label}: batch {index} column count"
        );
        assert_eq!(
            expected_batch.num_rows(),
            actual_batch.num_rows(),
            "{label}: batch {index} row count"
        );
        for (position, (expected_col, actual_col)) in expected_batch
            .columns()
            .iter()
            .zip(actual_batch.columns().iter())
            .enumerate()
        {
            let expected_schema = expected_batch.schema();
            let actual_schema = actual_batch.schema();
            let expected_field = expected_schema.field(position);
            let actual_field = actual_schema.field(position);
            assert_eq!(
                expected_field.name(),
                actual_field.name(),
                "{label}: batch {index} column {position} name"
            );
            assert_eq!(
                expected_field.data_type(),
                actual_field.data_type(),
                "{label}: batch {index} column {position} type"
            );
            assert!(
                column_bits_match(expected_col, actual_col),
                "{label}: batch {index} column {position} ('{}') differs by bits",
                expected_field.name()
            );
        }
    }
}

/// Every value equal, floats by bit pattern: NaN payloads and signed
/// zeros compare exactly, nulls align.
#[cfg(test)]
fn column_bits_match(expected: &dyn arrow::array::Array, actual: &dyn arrow::array::Array) -> bool {
    use arrow::array::AsArray;
    use arrow::datatypes::{
        DataType, Float32Type, Float64Type, Int8Type, Int16Type, Int32Type, Int64Type,
        TimestampMicrosecondType, TimestampMillisecondType, TimestampNanosecondType,
        TimestampSecondType, UInt8Type, UInt16Type, UInt32Type, UInt64Type,
    };

    macro_rules! primitive_bits {
        ($ty:ty) => {
            expected
                .as_primitive::<$ty>()
                .iter()
                .zip(actual.as_primitive::<$ty>().iter())
                .all(|(a, b)| match (a, b) {
                    (None, None) => true,
                    (Some(a), Some(b)) => a == b,
                    _ => false,
                })
        };
    }

    macro_rules! float_bits {
        ($ty:ty) => {
            expected
                .as_primitive::<$ty>()
                .iter()
                .zip(actual.as_primitive::<$ty>().iter())
                .all(|(a, b)| match (a, b) {
                    (None, None) => true,
                    (Some(a), Some(b)) => a.to_bits() == b.to_bits(),
                    _ => false,
                })
        };
    }

    match expected.data_type() {
        DataType::Float64 => float_bits!(Float64Type),
        DataType::Float32 => float_bits!(Float32Type),
        DataType::Timestamp(unit, _) => match unit {
            arrow::datatypes::TimeUnit::Second => primitive_bits!(TimestampSecondType),
            arrow::datatypes::TimeUnit::Millisecond => primitive_bits!(TimestampMillisecondType),
            arrow::datatypes::TimeUnit::Microsecond => primitive_bits!(TimestampMicrosecondType),
            arrow::datatypes::TimeUnit::Nanosecond => primitive_bits!(TimestampNanosecondType),
        },
        DataType::Int64 => primitive_bits!(Int64Type),
        DataType::Int32 => primitive_bits!(Int32Type),
        DataType::Int16 => primitive_bits!(Int16Type),
        DataType::Int8 => primitive_bits!(Int8Type),
        DataType::UInt64 => primitive_bits!(UInt64Type),
        DataType::UInt32 => primitive_bits!(UInt32Type),
        DataType::UInt16 => primitive_bits!(UInt16Type),
        DataType::UInt8 => primitive_bits!(UInt8Type),
        DataType::Utf8 => expected
            .as_string::<i32>()
            .iter()
            .zip(actual.as_string::<i32>().iter())
            .all(|(a, b)| a == b),
        DataType::Utf8View => expected
            .as_string_view()
            .iter()
            .zip(actual.as_string_view().iter())
            .all(|(a, b)| a == b),
        DataType::Boolean => expected
            .as_boolean()
            .iter()
            .zip(actual.as_boolean().iter())
            .all(|(a, b)| a == b),
        other => panic!("the bitwise comparison has no arm for {other}"),
    }
}
