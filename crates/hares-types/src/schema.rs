//! The input schema's field vocabulary, shared by every crate that
//! describes a config or override surface: the value types, the bounds,
//! the field schema, and the `DescribeFields` trait every config and
//! override-accepting struct implements beside its definition.

use serde::{Deserialize, Serialize};

/// The type of a field's value, as the schema publishes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueType {
    /// A boolean.
    Boolean,
    /// An integer.
    Integer,
    /// A finite float.
    Number,
    /// A string.
    String,
    /// A local wall-clock timestamp (RFC 3339 with offset).
    Timestamp,
    /// A whole number of seconds.
    DurationSeconds,
    /// An array of values.
    Array,
    /// A nested table described by its own type.
    Object,
}

/// A field's physical bounds, as the schema publishes them: an inclusive
/// or exclusive end, `None` when that end is unbounded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub min_exclusive: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub max_exclusive: bool,
}

/// One field of a config or override surface: its name, value type, unit,
/// default, bounds and description.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldSchema {
    pub name: String,
    pub value_type: ValueType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub default: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<Bounds>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// The description a struct's fields carry: one row per field, the
/// schema's per-field metadata beside the serde-derived shape.
#[derive(Debug, Clone, Copy, Default)]
pub struct FieldMeta {
    pub unit: Option<&'static str>,
    pub bounds: Option<Bounds>,
    pub description: Option<&'static str>,
}

/// A config or override-accepting struct that publishes its fields: the
/// trait every such struct implements beside its definition, so the
/// schema cannot drift from the type. The implementor names every field
/// with its metadata; the field's DEFAULT is the value the impl's sample
/// instance serialises.
pub trait DescribeFields {
    /// The struct's fields, in declaration order.
    fn fields() -> Vec<FieldSchema>;
}

/// Builds one field's schema from its name, the sample instance's
/// serialised value and the field's metadata.
#[must_use]
pub fn field_schema(name: &str, sample: &serde_json::Value, meta: FieldMeta) -> FieldSchema {
    let value_type = match sample {
        serde_json::Value::Bool(_) => ValueType::Boolean,
        serde_json::Value::Number(number) => {
            if number.is_i64() || number.is_u64() {
                ValueType::Integer
            } else {
                ValueType::Number
            }
        }
        serde_json::Value::String(_) => ValueType::String,
        serde_json::Value::Array(_) => ValueType::Array,
        serde_json::Value::Object(_) => ValueType::Object,
        serde_json::Value::Null => ValueType::String,
    };
    FieldSchema {
        name: name.to_string(),
        value_type,
        unit: meta.unit.map(str::to_string),
        default: sample.clone(),
        bounds: meta.bounds,
        description: meta.description.map(str::to_string),
    }
}

/// Builds a struct's field schemas: the sample instance serialises (its
/// fields in declaration order, every field set so an `Option`'s key never
/// skips), and the metadata table attaches each field's unit, bounds and
/// description. A metadata row naming no serialised field, or a serialised
/// field no row names, is a schema bug and panics: the list and the type
/// must move together.
#[must_use]
pub fn describe<S: Serialize>(sample: &S, metas: &[(&str, FieldMeta)]) -> Vec<FieldSchema> {
    let value = serde_json::to_value(sample).expect("a config's sample serialises");
    let serde_json::Value::Object(fields) = value else {
        panic!("a described struct's sample serialises to an object");
    };
    let mut out = Vec::new();
    for (name, sample) in &fields {
        let (_, meta) = metas
            .iter()
            .find(|(meta_name, _)| meta_name == name)
            .unwrap_or_else(|| {
                panic!(
                    "the described struct's field '{name}' has no metadata row: \
                     add it to the DescribeFields impl beside the definition"
                )
            });
        out.push(field_schema(name, sample, *meta));
    }
    for (name, _) in metas {
        assert!(
            fields.contains_key(*name),
            "the metadata row '{name}' names no field the struct serialises: \
             the impl and the struct must move together"
        );
    }
    out
}
