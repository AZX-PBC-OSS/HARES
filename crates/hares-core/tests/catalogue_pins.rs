//! The schema pins are the fixed ground truth for the catalogue: written
//! once from the pre-catalogue schema pipeline (build_schema +
//! extend_schema_with_actor_columns) before the catalogue landed, reviewed
//! alongside the catalogue's static tables. The suite that compares the
//! catalogue-derived schema against them lives in `catalogue.rs`; this file
//! only guards the pins' presence and shape.

use serde_json::Value;

#[test]
fn schema_pins_are_present_and_wellformed() {
    for verbosity in 0..=8u8 {
        let path = format!("../../tests/fixtures/schema/v{verbosity}.json");
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("schema pin missing at {path}: {err}"));
        let fields: Vec<Value> = serde_json::from_str(&raw)
            .unwrap_or_else(|err| panic!("schema pin {path} is not valid JSON: {err}"));
        assert!(
            !fields.is_empty(),
            "schema pin {path} must list at least the Time column"
        );
        for field in &fields {
            assert!(
                field.get("name").is_some()
                    && field.get("arrow_type").is_some()
                    && field.get("nullable").is_some(),
                "schema pin {path} entries carry name, arrow_type and nullable"
            );
        }
        assert_eq!(
            fields[0]["name"], "Time",
            "every schema pin starts at the Time column"
        );
    }
}
