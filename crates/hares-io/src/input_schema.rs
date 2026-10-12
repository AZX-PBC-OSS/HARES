//! The input schema's `hares-io` half: the section types and the
//! derivations from the tables this crate owns (the removed keys, the
//! schedule column table, the defaults read list, the HPXML element
//! lists). `hares_core::input_schema()` assembles the whole schema from
//! these and the `DescribeFields` impls next to the config definitions.

use serde::{Deserialize, Serialize};

pub use hares_types::{Bounds, DescribeFields, FieldMeta, FieldSchema, ValueType};

use crate::config::REMOVED_KEYS;
use crate::hpxml::elements::{HPXML_ELEMENTS, HPXML_REJECTED_ELEMENTS};

/// One override target: an equipment class and the fields its config
/// accepts. The class name is the override map's key; a field name not
/// listed is rejected by the same strict deserialisation the override
/// merge runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverrideClassSchema {
    pub name: String,
    pub description: String,
    pub fields: Vec<FieldSchema>,
}

/// One removed key: its scope, its name and the replacement the surfaces
/// raise for it (the `REMOVED_KEYS` row itself, never a parallel list).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovedKeySchema {
    pub scope: String,
    pub key: String,
    pub replacement: String,
}

/// One schedule column the engine reads: its name, the unit the parser
/// reads its values in when that is fixed, and what the engine maps it to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleColumnSchema {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    pub description: String,
}

/// One schedule column the engine accepts without reading, with the reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnusedScheduleColumnSchema {
    pub name: String,
    pub reason: String,
}

/// One HPXML element the parser reads, as the schema publishes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HpxmlElementSchema {
    pub name: String,
    pub context: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    pub description: String,
}

/// One HPXML element whose presence fails the parse, as the schema
/// publishes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedHpxmlElementSchema {
    pub name: String,
    pub context: String,
    pub reason: String,
}

/// The machine-readable schema of every input the engine reads: the
/// config keys, the dwelling keys, the override targets, the schedule
/// columns, the defaults files and the HPXML elements, each with its
/// types, units, bounds and defaults. `hares_core::input_schema()`
/// assembles it from the typed definitions; serialises to JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputSchema {
    /// The `SimulationConfig` fields: the `[simulation]` table's keys.
    pub config_fields: Vec<FieldSchema>,
    /// The dwelling keys: the fields `DwellingConfig` carries beside its
    /// settings.
    pub dwelling_fields: Vec<FieldSchema>,
    /// The `DwellingSettings` fields: the non-input settings a dwelling
    /// is built with.
    pub settings_fields: Vec<FieldSchema>,
    /// The override targets: every equipment class and the fields its
    /// config accepts, so an override key not listed here is rejected.
    pub override_classes: Vec<OverrideClassSchema>,
    /// Every removed key with its scope, from the `REMOVED_KEYS` table.
    pub removed_keys: Vec<RemovedKeySchema>,
    /// The schedule columns the engine reads.
    pub schedule_columns: Vec<ScheduleColumnSchema>,
    /// The schedule columns the engine accepts without reading, with why.
    pub unused_schedule_columns: Vec<UnusedScheduleColumnSchema>,
    /// The defaults files a load of the canonical tree reads, in load
    /// order.
    pub defaults_files: Vec<String>,
    /// Every HPXML element the parser reads.
    pub hpxml_elements: Vec<HpxmlElementSchema>,
    /// Every HPXML element whose presence fails the parse.
    pub rejected_hpxml_elements: Vec<RejectedHpxmlElementSchema>,
}

/// The removed keys as the schema publishes them: the `REMOVED_KEYS`
/// table itself, the scope as the snake_case name the JSON carries.
#[must_use]
pub fn removed_key_schemas() -> Vec<RemovedKeySchema> {
    REMOVED_KEYS
        .iter()
        .map(|removed| RemovedKeySchema {
            scope: serde_json::to_value(removed.scope)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default(),
            key: removed.key.to_string(),
            replacement: removed.replacement.to_string(),
        })
        .collect()
}

/// The schedule columns as the schema publishes them: the mapping
/// table's reads and the `NotUsed` entries with their reasons.
#[must_use]
pub fn schedule_column_schemas() -> (Vec<ScheduleColumnSchema>, Vec<UnusedScheduleColumnSchema>) {
    crate::schedule_resolve::schedule_column_schemas()
}

/// The HPXML elements as the schema publishes them: the declared list.
#[must_use]
pub fn hpxml_element_schemas() -> Vec<HpxmlElementSchema> {
    HPXML_ELEMENTS
        .iter()
        .map(|element| HpxmlElementSchema {
            name: element.name.to_string(),
            context: element.context.to_string(),
            unit: element.unit.map(|unit| unit.symbol().to_string()),
            description: element.description.to_string(),
        })
        .collect()
}

/// The rejected HPXML elements as the schema publishes them: the declared
/// list.
#[must_use]
pub fn rejected_hpxml_element_schemas() -> Vec<RejectedHpxmlElementSchema> {
    HPXML_REJECTED_ELEMENTS
        .iter()
        .map(|entry| RejectedHpxmlElementSchema {
            name: entry.name.to_string(),
            context: entry.context.to_string(),
            reason: entry.reason.to_string(),
        })
        .collect()
}

/// The defaults files a load of the defaults tree at `base` reads, in
/// load order: the load's own read list (the input digest's material),
/// never a parallel list. `base: None` is the engine's working-directory
/// convention.
pub fn defaults_file_schemas(
    base: Option<&std::path::Path>,
) -> Result<Vec<String>, hares_types::HaresError> {
    let source = match base {
        Some(base) => crate::inputs::DefaultsSource::directory(base),
        None => crate::inputs::DefaultsSource::default(),
    };
    let parsed = crate::inputs::ParsedDefaults::parse(&source)?;
    Ok(parsed.read_files())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::config::REMOVED_KEYS;

    /// The removed-key schemas are the `REMOVED_KEYS` table itself: every
    /// row, in table order, with the table's own replacement text, and
    /// the scope as the snake_case name the JSON publishes.
    #[test]
    fn input_schema_removed_keys_equal_the_table() {
        let schemas = super::removed_key_schemas();
        assert_eq!(schemas.len(), REMOVED_KEYS.len());
        for (schema, removed) in schemas.iter().zip(REMOVED_KEYS.iter()) {
            assert_eq!(schema.key, removed.key);
            assert_eq!(schema.replacement, removed.replacement);
            let scope = serde_json::to_value(removed.scope).expect("a scope serialises");
            assert_eq!(schema.scope, scope.as_str().expect("a scope is a string"));
        }
    }

    /// The schedule-column schemas are the parser's column table: the
    /// mapped categories, the water-heater columns and the occupancy
    /// column as the read columns; every `NotUsed` entry with its reason
    /// as the unused columns; and no column twice. The setpoint columns
    /// carry the unit the parser reads them in.
    #[test]
    fn input_schema_schedule_columns_equal_the_column_table() {
        let (used, unused) = super::schedule_column_schemas();
        assert!(!used.is_empty(), "the column table maps columns");
        assert!(!unused.is_empty(), "the column table carries NotUsed rows");
        let mut names: Vec<&str> = used
            .iter()
            .map(|column| column.name.as_str())
            .chain(unused.iter().map(|column| column.name.as_str()))
            .collect();
        let unique = names.clone();
        names.sort_unstable();
        unique
            .iter()
            .for_each(|name| assert_eq!(names.iter().filter(|n| *n == name).count(), 1));
        for column in &used {
            if column.name.ends_with("setpoint") {
                assert_eq!(
                    column.unit.as_deref(),
                    Some("C"),
                    "the setpoint columns carry the unit the parser reads them in"
                );
            }
        }
        for column in &unused {
            assert!(!column.reason.is_empty(), "a NotUsed entry states why");
        }
        // The water heater's setpoint column is in neither list: a
        // schedule carrying it is a construction error, not an input the
        // engine accepts.
        for column in &used {
            assert_ne!(column.name, "water_heater_setpoint");
        }
        for column in &unused {
            assert_ne!(column.name, "water_heater_setpoint");
        }
    }

    /// The schema's defaults files are the files a parse of the full
    /// shipped `defaults/` tree reads, in load order.
    #[test]
    fn input_schema_defaults_files_equal_files_read() {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../defaults");
        let parsed =
            crate::inputs::ParsedDefaults::parse(&crate::inputs::DefaultsSource::directory(&base))
                .expect("the shipped defaults tree parses");
        let expected = parsed.read_files();
        assert!(!expected.is_empty());
        let schema = super::defaults_file_schemas(Some(&base)).expect("the schema's read");
        assert_eq!(schema, expected);
    }
}
