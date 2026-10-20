//! The `DescribeFields` machinery for the equipment configs: the helper
//! derives a config's field schemas from its own serde implementation, so
//! the schema cannot drift from the deserialisation the override merge
//! runs. The field NAMES come from the deserialiser's own unknown-field
//! error (its "expected one of" list is the complete accepted set), the
//! defaults from the config's own defaults (the empty object's
//! deserialisation).

use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use hares_types::{FieldMeta, FieldSchema, ValueType};

/// The field names a config deserialises: the unknown-field probe's
/// "expected one of" list, which serde's derive fills with every field
/// the struct accepts. A struct without `deny_unknown_fields` (or with a
/// flattened catcher) rejects the probe without a list; its impl
/// hand-lists instead.
pub fn probe_fields<T: DeserializeOwned>() -> Option<Vec<String>> {
    let err = serde_json::from_value::<T>(serde_json::json!({"@@field_probe@@": null})).err()?;
    let text = err.to_string();
    let marker = "expected one of ";
    let position = text.find(marker)?;
    let rest = &text[position + marker.len()..];
    let end = rest.find(" at ").unwrap_or(rest.len());
    let list = &rest[..end];
    let names: Vec<String> = list
        .split(", ")
        .map(|field| field.trim_matches('`').to_string())
        .filter(|field| field != "or" && !field.is_empty())
        .collect();
    (!names.is_empty()).then_some(names)
}

/// The defaults a config applies when its keys are absent: deserialising
/// the empty object runs every field's own serde default. A config with
/// required fields (no default) rejects the empty object and publishes no
/// defaults — the schema's honest shape.
pub fn defaults_of<T: DeserializeOwned + serde::Serialize>() -> Map<String, Value> {
    match serde_json::from_value::<T>(Value::Object(Map::new())) {
        Ok(value) => match serde_json::to_value(&value) {
            Ok(Value::Object(map)) => map,
            _ => Map::new(),
        },
        Err(_) => Map::new(),
    }
}

/// The value type a field reads, for a field the defaults' serialisation
/// does not carry: the one-field-object deserialisation's error names the
/// expectation ("expected a sequence", "expected a string", "expected
/// f64"). Probes only work on a config the empty object builds; a
/// required-field config's unset fields publish the advisory `Number`.
fn probe_field_type<T: DeserializeOwned>(name: &str) -> ValueType {
    match serde_json::from_value::<T>(serde_json::json!({ name: null })) {
        // Null deserialises: the field is an `Option`'s, and any value
        // type the schema publishes for it is advisory.
        Ok(_) => ValueType::Number,
        Err(err) => {
            let text = err.to_string();
            if text.contains("expected a sequence") {
                ValueType::Array
            } else if text.contains("expected a string") || text.contains("expected a map") {
                ValueType::String
            } else if text.contains("expected a boolean") {
                ValueType::Boolean
            } else {
                ValueType::Number
            }
        }
    }
}

/// The field schemas: names from the probe, defaults from the config's
/// own defaults, the metadata rows beside the definition. A metadata row
/// naming no probed field, or a probed field no row names, is a schema
/// bug and panics.
pub fn field_schemas<T: DeserializeOwned + serde::Serialize>(
    metas: &[(&str, FieldMeta)],
) -> Vec<FieldSchema> {
    let names = probe_fields::<T>().unwrap_or_else(|| {
        panic!(
            "the config does not enumerate its fields to the probe (no \
             deny_unknown_fields list); hand-list its fields in its impl"
        )
    });
    let defaults = defaults_of::<T>();
    for (meta_name, _) in metas {
        assert!(
            names.iter().any(|name| name == meta_name),
            "the metadata row '{meta_name}' names no field the config accepts: \
             the impl and the deserialisation must move together"
        );
    }
    let mut out = Vec::new();
    for name in names {
        let (_, meta) = metas
            .iter()
            .find(|(meta_name, _)| *meta_name == name.as_str())
            .unwrap_or_else(|| {
                panic!(
                    "the config's field '{name}' has no metadata row: add it to \
                     the DescribeFields impl beside the definition"
                )
            });
        let default = defaults.get(&name).cloned().unwrap_or(Value::Null);
        let value_type = match &default {
            Value::Bool(_) => ValueType::Boolean,
            Value::Number(number) => {
                if number.is_i64() || number.is_u64() {
                    ValueType::Integer
                } else {
                    ValueType::Number
                }
            }
            Value::String(_) => ValueType::String,
            Value::Array(_) => ValueType::Array,
            Value::Object(_) => ValueType::Object,
            Value::Null => probe_field_type::<T>(&name),
        };
        out.push(FieldSchema {
            name,
            value_type,
            unit: meta.unit.map(str::to_string),
            default,
            bounds: meta.bounds,
            description: meta.description.map(str::to_string),
        });
    }
    out
}

/// The field schemas for a config the probe cannot enumerate (a
/// flattened struct's composition): the names hand-listed in the impl
/// beside the definition, the defaults from the config's own defaults,
/// the value types by the same probe. A metadata row naming no listed
/// field is a schema bug and panics.
pub fn named_field_schemas<T: DeserializeOwned + serde::Serialize>(
    names: &[(&str, FieldMeta)],
) -> Vec<FieldSchema> {
    let defaults = defaults_of::<T>();
    names
        .iter()
        .map(|(name, meta)| {
            let default = defaults.get(*name).cloned().unwrap_or(Value::Null);
            let value_type = match &default {
                Value::Bool(_) => ValueType::Boolean,
                Value::Number(number) => {
                    if number.is_i64() || number.is_u64() {
                        ValueType::Integer
                    } else {
                        ValueType::Number
                    }
                }
                Value::String(_) => ValueType::String,
                Value::Array(_) => ValueType::Array,
                Value::Object(_) => ValueType::Object,
                Value::Null => probe_field_type::<T>(name),
            };
            FieldSchema {
                name: name.to_string(),
                value_type,
                unit: meta.unit.map(str::to_string),
                default,
                bounds: meta.bounds,
                description: meta.description.map(str::to_string),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_lists_a_deny_unknown_fields_structs_fields() {
        let names = probe_fields::<crate::hvac::heating_config::GasFurnaceConfig>()
            .expect("GasFurnaceConfig enumerates its fields");
        assert!(names.contains(&"afue".to_string()));
        assert!(names.contains(&"ducts".to_string()));
        assert!(names.contains(&"stage_heating_capacities_w".to_string()));
    }

    /// A config the empty object builds publishes its defaults; one with
    /// required fields publishes none.
    #[test]
    fn the_defaults_come_from_the_empty_object() {
        let defaults = defaults_of::<crate::hvac::cooling_config::DehumidifierConfig>();
        assert!(!defaults.is_empty());
        let defaults = defaults_of::<crate::hvac::heating_config::GasFurnaceConfig>();
        assert!(defaults.is_empty(), "capacity_w is required: no defaults");
    }

    /// A metadata row naming no probed field is the schema bug the doc
    /// claims: the row panics (the fake-row case).
    #[test]
    #[should_panic(expected = "names no field")]
    fn a_metadata_row_naming_no_probed_field_panics() {
        field_schemas::<crate::hvac::heating_config::GasFurnaceConfig>(&[(
            "not_a_furnace_field",
            FieldMeta {
                unit: None,
                bounds: None,
                description: None,
            },
        )]);
    }
}
