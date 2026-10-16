//! The catalogue builder classifies equipment by its typed end use and
//! declared variables, never by its name string: the unit test over the
//! catalogue builder. The catalogue input's typed fields
//! (`CatalogueEquipment::end_use`, `::core_capabilities`) do not exist at
//! the parent commit, so this file is a compile RED there.

use std::collections::BTreeSet;

use hares_io::{CatalogueEquipment, CatalogueInput, build_catalogue};
use hares_types::{EndUse, EquipmentId, FuelType};

/// `misleadingly_named_equipment_classifies_by_its_descriptor`: one
/// descriptor named `"Television"` (end use plug load, no EV variables)
/// has no EV columns, and one named `"EV Charger"` with the EV end use
/// has them; names are never read.
#[test]
fn misleadingly_named_equipment_classifies_by_its_descriptor() {
    let catalogue = build_catalogue(&CatalogueInput {
        equipment: vec![
            CatalogueEquipment {
                name: "Television".to_string(),
                instance_name: None,
                id: EquipmentId(1),
                fuel: FuelType::Electric,
                end_use: EndUse::PLUG_LOADS,
                core_capabilities: hares_types::CoreCapabilities::ELECTRIC,
                telemetry: Vec::new(),
            },
            CatalogueEquipment {
                name: "EV Charger".to_string(),
                instance_name: None,
                id: EquipmentId(2),
                fuel: FuelType::Electric,
                end_use: EndUse::EV,
                core_capabilities: hares_types::CoreCapabilities::ELECTRIC,
                telemetry: Vec::new(),
            },
        ],
        zones: vec![(hares_types::ZoneId(1), "Indoor".to_string())],
        actors: Vec::new(),
    })
    .expect("the catalogue builds");

    let names: BTreeSet<&str> = catalogue
        .variables()
        .iter()
        .map(|v| v.name.as_ref())
        .collect();
    // The plug-load television: no EV columns, and its power rolls into
    // the Plug Loads aggregate.
    for ev_column in [
        "Television Connection State (-)",
        "Television Charging Level (-)",
    ] {
        assert!(
            !names.contains(ev_column),
            "'{ev_column}' must not exist: the name carries no classification"
        );
    }
    assert!(
        names.contains("Plug Loads End Use Electric Power (kW)"),
        "the television's power rolls into the Plug Loads aggregate"
    );
    // The EV-end-use charger: the EV columns, and the EV aggregate.
    for ev_column in [
        "EV Charger Connection State (-)",
        "EV Charger Charging Level (-)",
    ] {
        assert!(
            names.contains(ev_column),
            "'{ev_column}' must exist: the EV end use declares the EV columns"
        );
    }
    assert!(names.contains("EV End Use Electric Power (kW)"));
}
