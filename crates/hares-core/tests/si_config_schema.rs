//! The input schema's override classes are SI: every field of every class
//! of `hares_core::input_schema().overrides` carries a unit whose symbol
//! is none of the non-SI spellings the unit move renamed away, and no
//! field name matches the unit-segment pattern, so a config type added
//! later without its own case in the config walk still fails. A
//! `SyntheticEventLoadConfig` with every field set, serialised to JSON,
//! has no key that matches either.

use hares_types::{FieldSchema, Unit};

const NON_SI_UNIT_SYMBOLS: [&str; 9] = [
    "kW", "kWh", "kVA", "kVAR", "kW/s", "kWh/mi", "kWh/K", "mi", "mph",
];

/// Whether a field name carries a non-SI unit segment: a whole `kw`,
/// `kwh`, `kva`, `mi`, `miles` or `mph` token, at the name's edges or
/// alone (case-insensitive).
fn name_has_non_si_segment(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let units = ["kw", "kwh", "kva", "mi", "miles", "mph"];
    units.iter().any(|unit| {
        lower == *unit
            || lower.ends_with(&format!("_{unit}"))
            || lower.starts_with(&format!("{unit}_"))
            || lower.contains(&format!("_{unit}_"))
    })
}

fn walk_fields(class: &str, fields: &[FieldSchema], failures: &mut Vec<String>) {
    for field in fields {
        if let Some(unit) = &field.unit
            && let Some(symbol) = Unit::from_symbol(unit).map(|u| u.symbol())
            && NON_SI_UNIT_SYMBOLS.contains(&symbol)
        {
            failures.push(format!(
                "class {class}: field {} declares unit {symbol}",
                field.name
            ));
        }
        if name_has_non_si_segment(&field.name) {
            failures.push(format!(
                "class {class}: field {} carries a non-SI unit segment",
                field.name
            ));
        }
    }
}

#[test]
fn input_schema_override_classes_are_si() {
    let schema = hares_core::input_schema();
    let mut failures = Vec::new();
    for class in &schema.override_classes {
        walk_fields(&class.name, &class.fields, &mut failures);
    }
    assert!(
        failures.is_empty(),
        "the override classes' fields must be SI:\n{}",
        failures.join("\n")
    );
}

#[test]
fn synthetic_event_load_config_fields_are_si() {
    // The synthetic TOML event-load table's config, every field set:
    // serialised to JSON, no key matches the unit-segment pattern.
    let text = r#"
        [event_load]
        active_power_w = 1500.0
        active_duration_s = 300.0
        cooldown_duration_s = 120.0
        event_probability = 0.5
        sensible_gain_fraction = 0.5
        latent_gain_fraction = 0.2
    "#;
    let table: toml::Table = toml::from_str(text).expect("the event_load table parses");
    assert!(!table.contains_key("active_power_kw"));
    for key in table.keys() {
        assert!(
            !name_has_non_si_segment(key),
            "the synthetic event-load table's key {key} carries a non-SI unit segment"
        );
    }
}
