//! Equipment identity config reads — the payload-agnostic contract of
//! `hares_equipment::config::equipment_id_from_config` at the real
//! construction boundary.
//!
//! Five constructors (EV, Battery, Ventilation, Protocol Bridge, PV) used to
//! read `equipment_id` through `EquipmentConfig::get_f64` — the `Raw`-payload
//! accessor only — so a typed payload carrying the key was invisible to them
//! and every instance collapsed onto the unassigned sentinel. Generator had
//! the mirrored defect (typed-only read, blind to raw payloads). These tests
//! pin the aligned contract: every constructor resolves its id from either
//! payload kind through the one shared reader, and an absent key stamps the
//! unassigned sentinel. The payloads mirror exactly what the dwelling
//! assembly injects: a top-level `equipment_id` key in the typed payload's
//! JSON object (where `#[serde(flatten)]`-ed heat-pump configs expose
//! `common.equipment_id` at the top level).

mod common;

use std::collections::HashMap;

use hares_equipment::config::ConfigValue;
use hares_equipment::{
    CANONICAL_EQUIPMENT_NAMES, ConfigPayload, Equipment, EquipmentConfig, EquipmentRegistry,
};
use hares_types::HaresError;
use hares_types::{EnvironmentState, EquipmentId};

use common::{default_env, env_with_zone_temp};

/// Every constructor whose id read used to be one-sided, plus the
/// heat-pump newtypes whose configs carry the id through
/// `#[serde(flatten)]`'d `HeatPumpCommonConfig`.
const CLASSES: &[&str] = &[
    "EV",
    "Battery",
    "Ventilation Fan",
    "Protocol Bridge",
    "PV",
    "Gas Generator",
    "ASHP Heater",
    "MSHP Heater",
    "GSHP Heater",
    "WSHP Heater",
    "ASHP Cooler",
    "GSHP Cooler",
    "WSHP Cooler",
];

/// A `Typed` payload carrying `equipment_id` at the top level — the exact
/// shape the dwelling assembly's id-injection pass writes.
fn typed_id_config(class: &str, id: u32) -> EquipmentConfig {
    EquipmentConfig::with_payload(
        class.to_string(),
        class.to_string(),
        ConfigPayload::Typed {
            type_name: class.to_string(),
            version: 1,
            data: serde_json::json!({ "equipment_id": id }),
        },
    )
}

/// A `Raw` payload carrying `equipment_id` — custom-equipment adapter path.
fn raw_id_config(class: &str, id: f64) -> EquipmentConfig {
    let mut data = HashMap::new();
    data.insert("equipment_id".to_string(), ConfigValue::Float(id));
    EquipmentConfig::raw(class.to_string(), class.to_string(), data)
}

/// A `Raw` payload with no id key.
fn raw_absent_config(class: &str) -> EquipmentConfig {
    EquipmentConfig::raw(class.to_string(), class.to_string(), HashMap::new())
}

fn create(class: &str, config: EquipmentConfig) -> Box<dyn Equipment> {
    EquipmentRegistry::new()
        .create(class, config)
        .unwrap_or_else(|e| panic!("registry must create '{class}': {e}"))
}

#[test]
fn equipment_id_reads_from_typed_payload_for_every_constructor() {
    for &class in CLASSES {
        let eq = create(class, typed_id_config(class, 7));
        assert_eq!(
            eq.descriptor().id,
            EquipmentId(7),
            "'{class}' must read equipment_id from a Typed payload's top-level key \
             (the channel the dwelling assembly injects through)"
        );
    }
}

#[test]
fn equipment_id_reads_from_raw_payload_for_every_constructor() {
    for &class in CLASSES {
        let eq = create(class, raw_id_config(class, 7.0));
        assert_eq!(
            eq.descriptor().id,
            EquipmentId(7),
            "'{class}' must read equipment_id from a Raw payload (custom-equipment \
             adapter path)"
        );
    }
}

#[test]
fn absent_equipment_id_stamps_the_unassigned_sentinel() {
    for &class in CLASSES {
        let eq = create(class, raw_absent_config(class));
        assert_eq!(
            eq.descriptor().id,
            EquipmentId(0),
            "'{class}' must stamp the unassigned sentinel when the config carries no id"
        );
    }
}

/// The `Equipment::set_equipment_id` contract across the declared equipment
/// population: every registered type must accept the identity write before
/// registration and land it in its descriptor. Membership derives from
/// `CANONICAL_EQUIPMENT_NAMES`, so every future equipment type is covered
/// automatically.
///
/// Constructors that cannot build from a minimal payload panic in `new()`
/// instead of deferring to `init()`; those are the explicitly listed
/// exceptions below — every gap visible rather than silent.
#[test]
fn set_equipment_id_lands_in_descriptor_across_the_equipment_population() {
    const PANICKY_MINIMAL_CONSTRUCTORS: &[&str] = &[
        "Tankless Water Heater",
        "Gas Tankless Water Heater",
        "Heat Pump Water Heater",
        "Indirect Tank",
    ];

    let registry = EquipmentRegistry::new();
    let mut unconstructible = Vec::new();
    for &name in CANONICAL_EQUIPMENT_NAMES {
        let config = EquipmentConfig::with_payload(
            name.to_string(),
            name.to_string(),
            ConfigPayload::default(),
        );
        let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            registry.create(name, config)
        }));
        let mut eq = match attempt {
            Err(_) => {
                assert!(
                    PANICKY_MINIMAL_CONSTRUCTORS.contains(&name),
                    "'{name}' panics on a minimal config — add it to the explicit \
                     exception list or fix its constructor to defer config errors to init()"
                );
                continue;
            }
            Ok(Err(_)) => {
                unconstructible.push(name);
                continue;
            }
            Ok(Ok(eq)) => eq,
        };
        eq.set_equipment_id(EquipmentId(1234)).unwrap_or_else(|e| {
            panic!("'{name}' set_equipment_id must succeed pre-registration: {e}")
        });
        assert_eq!(
            eq.descriptor().id,
            EquipmentId(1234),
            "'{name}' must land the identity write in its descriptor"
        );
    }
    // Constructors that return Err (rather than panic) on a minimal payload:
    // also an explicit, visible list.
    const ERR_ON_MINIMAL: &[&str] = &["Water Heating"];
    for name in &unconstructible {
        assert!(
            ERR_ON_MINIMAL.contains(name),
            "'{name}' returned Err from a minimal config — add it to the explicit \
             exception list or fix its constructor"
        );
    }
}

/// The initialization guard on the identity write: once registered
/// (`mark_initialized`), a type that tracks the guard flag must reject the
/// write with the typed initialized error. Battery is one of the two
/// production guard-tracking types (with EV).
#[test]
fn set_equipment_id_rejects_after_initialization_on_guard_tracking_type() {
    let mut eq = create("Battery", raw_absent_config("Battery"));
    assert!(
        !eq.is_initialized(),
        "precondition: freshly constructed Battery must not be initialized"
    );
    eq.mark_initialized();
    let err = eq
        .set_equipment_id(EquipmentId(3))
        .expect_err("identity write must be rejected after initialization");
    assert!(
        matches!(err, HaresError::InvalidState(_)),
        "the rejection must be the typed InvalidState guard error, got: {err:?}"
    );
    assert!(
        err.to_string().contains("already initialized"),
        "the guard error must name the initialized state, got: {err}"
    );
    // The rejected write must not have landed.
    assert_eq!(eq.descriptor().id, EquipmentId(0));
}

/// Init re-assignment must preserve an existing (assembly-injected) id when
/// the config key is absent — never clobber it back to the sentinel. The
/// dehumidifier used to clobber unconditionally; the water heaters and
/// generator already preserved via their typed reads, and this pins the
/// unified payload-agnostic contract on the dehumidifier's corrected site
/// (its config fields are all optional, so a minimal typed payload inits
/// cleanly).
#[test]
fn init_reassignment_preserves_id_when_config_key_absent() {
    let env: EnvironmentState = env_with_zone_temp(21.0);
    let registry = EquipmentRegistry::new();

    // Stamp the assembly-injected identity by construction...
    let mut eq = registry
        .create("Dehumidifier", typed_id_config("Dehumidifier", 5))
        .expect("create Dehumidifier");
    assert_eq!(eq.descriptor().id, EquipmentId(5));
    // ...then init with a config whose key is absent: the id must survive.
    let absent_cfg = EquipmentConfig::with_payload(
        "Dehumidifier".to_string(),
        "Dehumidifier".to_string(),
        ConfigPayload::Typed {
            type_name: "Dehumidifier".to_string(),
            version: 1,
            data: serde_json::json!({}),
        },
    );
    eq.init(&absent_cfg, &env)
        .unwrap_or_else(|e| panic!("Dehumidifier init with minimal typed config: {e}"));
    assert_eq!(
        eq.descriptor().id,
        EquipmentId(5),
        "an absent equipment_id key at init must preserve the descriptor's id, not clobber it to 0"
    );

    // And a *present* id at init re-assigns (explicit beats stale).
    let env: EnvironmentState = default_env();
    let mut eq = registry
        .create("Dehumidifier", typed_id_config("Dehumidifier", 5))
        .expect("create Dehumidifier");
    eq.init(&typed_id_config("Dehumidifier", 6), &env)
        .unwrap_or_else(|e| panic!("Dehumidifier init: {e}"));
    assert_eq!(
        eq.descriptor().id,
        EquipmentId(6),
        "a present equipment_id key at init must re-assign the descriptor id"
    );
}
