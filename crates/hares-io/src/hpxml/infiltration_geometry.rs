//! The heights above grade and the exterior share of the measured leakage
//! that OS-HPXML's infiltration of the dwelling, its garage and its vented
//! attic uses (OpenStudio-HPXML v1.12.0 `geometry.rb`, `defaults.rb` and
//! `airflow.rb`), read from a building's HPXML when a model needs them, so
//! an input lacking what one of them derives from fails only where it is
//! used.

use hares_physics::units as conv;

use super::HpxmlError;
use super::building::{Building, ValueKind, XmlNode, parse_value_with_units};
use super::xml_helpers::child_text;
use super::zone_geometry::{average_ceiling_height_m, surfaces};

const CONDITIONED_SPACE: &str = "conditioned space";

/// `location` with the HPXML 3 name of the conditioned space, "living
/// space", read as its current one.
fn canonical(location: &str) -> &str {
    if location == "living space" {
        CONDITIONED_SPACE
    } else {
        location
    }
}

/// The interior and exterior locations of `surface`, canonical.
fn sides(interior: &str, surface: &XmlNode) -> (String, Option<String>) {
    (
        canonical(interior).to_string(),
        child_text(surface, "ExteriorAdjacentTo").map(|e| canonical(&e).to_string()),
    )
}

/// Locations OS-HPXML counts as conditioned (hpxml.rb:12311-12316).
fn is_conditioned_location(location: &str) -> bool {
    matches!(
        location,
        CONDITIONED_SPACE
            | "basement - conditioned"
            | "crawlspace - conditioned"
            | "other housing unit"
    )
}

/// Whether a `Floor` is a floor rather than a ceiling of the space it
/// bounds (hpxml.rb:5079-5096, 12400-12416): its `FloorOrCeiling`, else a
/// ceiling when either side is an attic and a floor otherwise.
fn is_floor(floor: &XmlNode, interior: &str, exterior: &str) -> bool {
    if let Some(kind) = child_text(floor, "FloorOrCeiling") {
        return kind != "ceiling";
    }
    let attic = |location: &str| location.starts_with("attic");
    !(attic(interior) || attic(exterior))
}

/// The unit's height above grade, clamped at grade: `UnitHeightAboveGrade`,
/// else OS-HPXML's default (defaults.rb:933-951), 2 ft for a unit whose
/// thermal-boundary floors are all over the outside or a manufactured
/// home's belly with no slab, else zero or the depth of a conditioned
/// basement below grade, which the clamp makes zero.
fn unit_height_above_grade_m(details: &XmlNode) -> Result<f64, HpxmlError> {
    let given = parse_value_with_units(
        details.path(&[
            "BuildingSummary",
            "BuildingConstruction",
            "UnitHeightAboveGrade",
        ]),
        ValueKind::Length,
    )?;
    if let Some(height_m) = given {
        return Ok(height_m.max(0.0));
    }
    let mut boundary_floors = 0_usize;
    let mut exterior_floors = 0_usize;
    for (interior, floor) in surfaces(details, "Floors", "Floor") {
        let (interior, exterior) = sides(&interior, floor);
        let exterior = exterior.unwrap_or_default();
        let thermal_boundary =
            is_conditioned_location(&interior) && !is_conditioned_location(&exterior);
        if thermal_boundary && is_floor(floor, &interior, &exterior) {
            boundary_floors += 1;
            if matches!(
                exterior.as_str(),
                "outside" | "manufactured home underbelly"
            ) {
                exterior_floors += 1;
            }
        }
    }
    let has_slab = surfaces(details, "Slabs", "Slab").next().is_some();
    Ok(
        if boundary_floors > 0 && boundary_floors == exterior_floors && !has_slab {
            conv::length_ft_to_m(2.0)
        } else {
            0.0
        },
    )
}

fn foundation_top_in(details: &XmlNode) -> Result<f64, HpxmlError> {
    let mut top_m = unit_height_above_grade_m(details)?;
    for (location, wall) in surfaces(details, "FoundationWalls", "FoundationWall") {
        let required = |name: &str| -> Result<f64, HpxmlError> {
            parse_value_with_units(wall.child(name), ValueKind::Length)?.ok_or_else(|| {
                HpxmlError::Parse(format!("foundation wall in '{location}' has no {name}").into())
            })
        };
        top_m = top_m.max(required("Height")? - required("DepthBelowGrade")?);
    }
    Ok(top_m)
}

/// The OS-HPXML location an `Attic` or `Foundation` stands for, read in
/// OS-HPXML's order (hpxml.rb:3515-3523, 3832-3847, and `to_location` at
/// 3404-3420, 3662-3692), for the types whose location can lie inside the
/// infiltration volume.
fn space_location(space: &XmlNode, type_element: &str) -> Option<&'static str> {
    let kind = space.child(type_element)?.children.first()?;
    let flag = |name: &str| child_text(kind, name).map(|text| text == "true");
    match (kind.name.as_str(), flag("Vented"), flag("Conditioned")) {
        ("Attic", Some(false), _) => Some("attic - unvented"),
        ("Basement", _, Some(false)) => Some("basement - unconditioned"),
        ("Basement", _, Some(true)) => Some("basement - conditioned"),
        ("Crawlspace", Some(false), _) => Some("crawlspace - unvented"),
        ("Crawlspace", Some(true), _) => None,
        ("Crawlspace", _, Some(true)) => Some("crawlspace - conditioned"),
        _ => None,
    }
}

/// Locations inside the infiltration volume (defaults.rb:5872-5884): the
/// conditioned space, and each attic or foundation whose
/// `WithinInfiltrationVolume` is true or, when absent, defaults to true
/// (a conditioned basement or crawlspace, defaults.rb:1074-1113).
fn infiltration_volume_locations(details: &XmlNode) -> Vec<&'static str> {
    let mut locations = vec![CONDITIONED_SPACE];
    for (group, element, type_element) in [
        ("Attics", "Attic", "AtticType"),
        ("Foundations", "Foundation", "FoundationType"),
    ] {
        for space in details
            .path(&["Enclosure", group])
            .into_iter()
            .flat_map(|node| node.children_named(element))
        {
            let Some(location) = space_location(space, type_element) else {
                continue;
            };
            let within = child_text(space, "WithinInfiltrationVolume")
                .map_or(is_conditioned_location(location), |text| text == "true");
            if within && !locations.contains(&location) {
                locations.push(location);
            }
        }
    }
    locations
}

/// The exterior share of the compartmentalization boundary
/// (defaults.rb:5863-5909, `get_compartmentalization_boundary_areas`):
/// the area of the surfaces bounding the infiltration volume, less those
/// to a garage or another unit's or the building's common spaces and the
/// adiabatic ones, over their total, rounded to 5 places.
fn compartmentalization_exterior_fraction(details: &XmlNode) -> Result<f64, HpxmlError> {
    let within = infiltration_volume_locations(details);
    let mut total_m2 = 0.0;
    let mut exterior_m2 = 0.0;
    for location in &within {
        for (group, element) in [
            ("Roofs", "Roof"),
            ("RimJoists", "RimJoist"),
            ("Walls", "Wall"),
            ("FoundationWalls", "FoundationWall"),
            ("Floors", "Floor"),
            ("Slabs", "Slab"),
        ] {
            for (interior, surface) in surfaces(details, group, element) {
                let (interior, exterior) = sides(&interior, surface);
                let adiabatic = exterior.as_deref() == Some(interior.as_str());
                if interior != *location && exterior.as_deref() != Some(location) {
                    continue;
                }
                if !adiabatic
                    && within.contains(&interior.as_str())
                    && exterior.as_deref().is_some_and(|e| within.contains(&e))
                {
                    continue;
                }
                let area_m2 = parse_value_with_units(surface.child("Area"), ValueKind::Area)?
                    .ok_or_else(|| {
                        HpxmlError::Parse(format!("{element} in '{interior}' has no Area").into())
                    })?;
                total_m2 += area_m2;
                let to_other_space = exterior.as_deref().is_some_and(|e| {
                    matches!(
                        e,
                        "garage"
                            | "other housing unit"
                            | "other heated space"
                            | "other multifamily buffer space"
                            | "other non-freezing space"
                    )
                });
                if !to_other_space && !adiabatic {
                    exterior_m2 += area_m2;
                }
            }
        }
    }
    if total_m2 <= 0.0 {
        return Err(HpxmlError::Parse(
            "no surface bounds the infiltration volume to split its leakage by".into(),
        ));
    }
    Ok((exterior_m2 / total_m2 * 1e5).round() / 1e5)
}

/// The exterior share of the measured leakage (airflow.rb:173-176 and
/// defaults.rb:1244-1250): the `Aext` of a unit-total measurement,
/// OS-HPXML's compartmentalization split for an attached or apartment unit
/// without one, else 1. A file without a measurement takes OS-HPXML's
/// defaulted unit-total leakage. OS-HPXML scales the unit's ACH50 by it for
/// the dwelling and its garage.
pub fn exterior_leakage_fraction(building: &Building) -> Result<f64, HpxmlError> {
    let details = &building.details_xml;
    let measurement = details.first_descendant("AirInfiltrationMeasurement");
    let unit_total = match measurement {
        Some(m) => child_text(m, "TypeOfInfiltrationLeakage").as_deref() == Some("unit total"),
        None => true,
    };
    if !unit_total {
        return Ok(1.0);
    }
    let given = match measurement {
        Some(m) => parse_value_with_units(m.path(&["extension", "Aext"]), ValueKind::Raw)?,
        None => None,
    };
    if let Some(a_ext) = given {
        return Ok(a_ext);
    }
    match building.residential_facility_type.as_deref() {
        Some("single-family attached" | "apartment unit") => {
            compartmentalization_exterior_fraction(details)
        }
        _ => Ok(1.0),
    }
}

/// Top of the foundation above grade (geometry.rb:835-843,
/// `foundation_height_above_grade`): the unit's height above grade, raised
/// to the top of any foundation wall (its `Height` less its
/// `DepthBelowGrade`). A garage's leakage is driven at this height.
pub fn foundation_top_m(building: &Building) -> Result<f64, HpxmlError> {
    foundation_top_in(&building.details_xml)
}

/// Top of the conditioned walls above grade (geometry.rb:844-847,
/// `walls_height_above_grade`): the foundation top plus OS-HPXML's average
/// ceiling height per conditioned floor above grade. A vented attic's
/// leakage is driven at this height.
pub fn walls_top_m(building: &Building) -> Result<f64, HpxmlError> {
    let details = &building.details_xml;
    let construction = details.path(&["BuildingSummary", "BuildingConstruction"]);
    let required = |name: &str, kind: ValueKind| -> Result<f64, HpxmlError> {
        parse_value_with_units(construction.and_then(|c| c.child(name)), kind)?.ok_or_else(|| {
            HpxmlError::Parse(
                format!("missing {name}; the height of the walls above grade derives from it")
                    .into(),
            )
        })
    };
    let floors = required("NumberofConditionedFloorsAboveGrade", ValueKind::Raw)?;
    let conditioned_floor_area_m2 = required("ConditionedFloorArea", ValueKind::Area)?;
    let ceiling_m = average_ceiling_height_m(
        details,
        construction.and_then(|c| c.child("AverageCeilingHeight")),
        parse_value_with_units(
            construction.and_then(|c| c.child("ConditionedBuildingVolume")),
            ValueKind::Volume,
        )?,
        conditioned_floor_area_m2,
        &mut Vec::new(),
    )?;
    Ok(foundation_top_in(details)? + ceiling_m * floors)
}
