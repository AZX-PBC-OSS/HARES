//! `assign_equipment_ids` contract — the assembly identity pre-pass.
//!
//! The pass's documented contract distinguishes an absent `equipment_id`
//! (the pass assigns) from a *present but malformed* one (the pass leaves it
//! untouched so the dwelling's assembly validation rejects the build loudly
//! instead of silently overwriting it — the constitution's loud-errors rule
//! for unrecognised values).

use serde_json::{Map, Value, json};

use hares_equipment::{ConfigPayload, EquipmentConfig};
use hares_io::EquipmentSpec;
use hares_io::hpxml::equipment::assign_equipment_ids;
use hares_types::FuelType;

fn spec_with_raw_equipment_id(id: Value) -> EquipmentSpec {
    let mut parameters = Map::new();
    parameters.insert("equipment_id".to_string(), id);
    EquipmentSpec {
        name: "EV".to_string(),
        instance_name: None,
        fuel_type: FuelType::Electric,
        parameters,
        zip_params: None,
        typed_config: None,
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
    }
}

/// A malformed present `equipment_id` must survive the pass untouched — the
/// constructor's tri-state reader (or assembly validation) is the boundary
/// that must reject it loudly. The pass silently overwriting it would turn a
/// config error into a silently auto-assigned id.
#[test]
fn assign_equipment_ids_preserves_a_malformed_explicit_id_for_loud_rejection() {
    for (label, malformed) in [
        ("negative", json!(-1)),
        ("fractional", json!(1.5)),
        ("non-numeric", json!("3")),
    ] {
        let mut spec = spec_with_raw_equipment_id(malformed.clone());
        assign_equipment_ids(std::slice::from_mut(&mut spec));
        assert_eq!(
            spec.parameters["equipment_id"], malformed,
            "{label} equipment_id must be left untouched by the assignment pass \
             so the dwelling's validation rejects the build loudly instead of \
             the pass silently substituting an assigned id"
        );
    }
}

/// A spec whose typed payload carries the malformed id — the channel the
/// pass injects through for typed specs — must likewise be left untouched:
/// the classifier reads both channels, and a malformed value on either is
/// the loud-rejection contract, never a silent overwrite.
#[test]
fn malformed_equipment_id_in_the_typed_payload_is_left_untouched() {
    let typed = EquipmentConfig::with_payload(
        "Dehumidifier".to_string(),
        "Dehumidifier".to_string(),
        ConfigPayload::Typed {
            type_name: "Dehumidifier".to_string(),
            version: 1,
            data: json!({ "equipment_id": -1 }),
        },
    );
    let mut spec = EquipmentSpec {
        name: "Dehumidifier".to_string(),
        instance_name: None,
        fuel_type: FuelType::Electric,
        parameters: Map::new(),
        zip_params: None,
        typed_config: Some(typed),
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
    };
    assign_equipment_ids(std::slice::from_mut(&mut spec));
    let ConfigPayload::Typed { data, .. } = &spec.typed_config.expect("typed config").payload
    else {
        panic!("payload must stay typed");
    };
    assert_eq!(
        data["equipment_id"],
        json!(-1),
        "a malformed equipment_id in the typed payload must be left untouched \
         by the assignment pass so the dwelling's validation rejects the build \
         loudly instead of the pass silently substituting an assigned id"
    );
    assert!(
        spec.parameters.get("equipment_id").is_none(),
        "the pass must not inject an id into the raw channel either when the \
         typed payload carries a malformed one"
    );
}

/// `null` is how a typed config serializes `Option<u32>::None` — the unset
/// field — and how it lands in the spec's parameters mirror. It must be
/// classified absent (the pass assigns); classifying it malformed would
/// leave every unset typed spec unassigned and fail every dwelling build.
#[test]
fn null_equipment_id_is_classified_absent_and_gets_an_assigned_id() {
    for (label, spec) in [
        (
            "raw channel (the typed mirror's landing spot)",
            spec_with_raw_equipment_id(Value::Null),
        ),
        ("typed payload", {
            let typed = EquipmentConfig::with_payload(
                "Dehumidifier".to_string(),
                "Dehumidifier".to_string(),
                ConfigPayload::Typed {
                    type_name: "Dehumidifier".to_string(),
                    version: 1,
                    data: json!({ "equipment_id": null }),
                },
            );
            EquipmentSpec {
                name: "Dehumidifier".to_string(),
                instance_name: None,
                fuel_type: FuelType::Electric,
                parameters: Map::new(),
                zip_params: None,
                typed_config: Some(typed),
                system_id: None,
                related_hvac_idref: None,
                primary_role: None,
            }
        }),
    ] {
        let mut spec = spec;
        assign_equipment_ids(std::slice::from_mut(&mut spec));
        let assigned = spec
            .parameters
            .get("equipment_id")
            .and_then(|v| v.as_u64())
            .unwrap_or_else(|| {
                panic!(
                    "{label}: the pass must assign a real id for a null (unset) \
                     equipment_id, got {:?}",
                    spec.parameters.get("equipment_id")
                )
            });
        assert!(
            assigned >= 1,
            "{label}: the assigned id must be a real id (>= 1), got {assigned}"
        );
    }
}

/// Repeated invocation must be idempotent: on a second pass every spec
/// already carries a valid id, so the pass preserves each one and re-derives
/// the same counter — no spec's id may move, and a malformed id must stay
/// untouched across repeated passes. Assembly invokes the pass once, but the
/// function is public: a future caller that runs it twice (or a retry around
/// assembly) must not re-assign over identities already delivered.
#[test]
fn assign_equipment_ids_is_idempotent_on_repeated_invocation() {
    let mut specs = vec![
        // Absent → assigned on the first pass, preserved on the second.
        spec_with_raw_equipment_id(Value::Null),
        // Explicit valid → preserved on both passes.
        spec_with_raw_equipment_id(json!(7)),
        // Malformed → untouched on both passes.
        spec_with_raw_equipment_id(json!(1.5)),
    ];

    assign_equipment_ids(&mut specs);
    let first: Vec<Value> = specs
        .iter()
        .map(|s| s.parameters["equipment_id"].clone())
        .collect();

    assign_equipment_ids(&mut specs);
    let second: Vec<Value> = specs
        .iter()
        .map(|s| s.parameters["equipment_id"].clone())
        .collect();

    assert_eq!(
        first, second,
        "a repeated assignment pass must not move any equipment_id — the pass \
         preserves valid ids, so its second invocation is a no-op"
    );
    assert_eq!(
        first[1],
        json!(7),
        "the explicit id must be preserved across repeated passes, not re-assigned"
    );
    assert_eq!(
        first[2],
        json!(1.5),
        "the malformed id must be left untouched across repeated passes"
    );
}
