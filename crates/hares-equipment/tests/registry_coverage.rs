use std::collections::HashSet;

use hares_equipment::{CANONICAL_EQUIPMENT_NAMES, EquipmentRegistry};

#[test]
fn all_built_in_equipment_types_are_registered() {
    let registry = EquipmentRegistry::new();
    let known: HashSet<&str> = registry.known_names().into_iter().collect();

    for &name in CANONICAL_EQUIPMENT_NAMES {
        assert!(
            known.contains(name),
            "CANONICAL_EQUIPMENT_NAMES entry '{name}' is not registered in EquipmentRegistry::new()"
        );
    }
}

#[test]
fn no_duplicate_registrations() {
    let registry = EquipmentRegistry::new();
    let names = registry.known_names();
    let unique: HashSet<&str> = names.iter().copied().collect();
    assert_eq!(
        names.len(),
        unique.len(),
        "EquipmentRegistry::new() contains duplicate registrations: {names:?}"
    );
}
