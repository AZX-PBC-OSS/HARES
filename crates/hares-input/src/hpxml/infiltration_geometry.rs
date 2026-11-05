//! The heights above grade and the exterior share of the measured leakage
//! that OS-HPXML's infiltration of the dwelling, its garage and its vented
//! attic uses (OpenStudio-HPXML v1.12.0 `geometry.rb`, `defaults.rb` and
//! `airflow.rb`), read from a building's HPXML when a model needs them, so
//! an input lacking what one of them derives from fails only where it is
//! used.

use hares_physics::units as conv;

use super::HpxmlError;
use super::building::{Building, ValueKind, XmlNode, parse_value_with_units};
use super::elements::*;
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
        child_text(surface, &EXTERIOR_ADJACENT_TO).map(|e| canonical(&e).to_string()),
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
    if let Some(kind) = child_text(floor, &FLOOR_OR_CEILING) {
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
            &BUILDING_SUMMARY,
            &BUILDING_CONSTRUCTION,
            &UNIT_HEIGHT_ABOVE_GRADE,
        ]),
        ValueKind::Length,
    )?;
    if let Some(height_m) = given {
        return Ok(height_m.max(0.0));
    }
    let mut boundary_floors = 0_usize;
    let mut exterior_floors = 0_usize;
    for (interior, floor) in surfaces(details, &FLOORS, &FLOOR) {
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
    let has_slab = surfaces(details, &SLABS, &SLAB).next().is_some();
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
    for (location, wall) in surfaces(details, &FOUNDATION_WALLS, &FOUNDATION_WALL) {
        let required = |name: &'static super::elements::HpxmlElement| -> Result<f64, HpxmlError> {
            parse_value_with_units(wall.child(name), ValueKind::Length)?.ok_or_else(|| {
                HpxmlError::Parse(
                    format!("foundation wall in '{location}' has no {}", name.name).into(),
                )
            })
        };
        top_m = top_m.max(required(&HEIGHT)? - required(&DEPTH_BELOW_GRADE)?);
    }
    Ok(top_m)
}

/// The OS-HPXML location an `Attic` or `Foundation` stands for, read in
/// OS-HPXML's order (hpxml.rb:3515-3523, 3832-3847, and `to_location` at
/// 3404-3420, 3662-3692), for the types whose location can lie inside the
/// infiltration volume. The type element's child is matched against the
/// declared space-location choices, so a kind the match reads is a
/// declared static.
fn space_location(
    space: &XmlNode,
    type_element: &'static super::elements::HpxmlElement,
) -> Option<&'static str> {
    let kind = space.child(type_element)?.children.first()?;
    let choice = kind.declared_choice(SPACE_LOCATION_CHOICES)?;
    let flag = |element: &'static super::elements::HpxmlElement| {
        child_text(kind, element).map(|text| text == "true")
    };
    if std::ptr::eq(choice, &ATTIC) {
        (flag(&VENTED) == Some(false)).then_some("attic - unvented")
    } else if std::ptr::eq(choice, &BASEMENT) {
        match flag(&CONDITIONED) {
            Some(false) => Some("basement - unconditioned"),
            Some(true) => Some("basement - conditioned"),
            None => None,
        }
    } else if std::ptr::eq(choice, &CRAWLSPACE) {
        match (flag(&VENTED), flag(&CONDITIONED)) {
            (Some(false), _) => Some("crawlspace - unvented"),
            (Some(true), _) => None,
            (_, Some(true)) => Some("crawlspace - conditioned"),
            _ => None,
        }
    } else {
        None
    }
}

/// Locations inside the infiltration volume (defaults.rb:5872-5884): the
/// conditioned space, and each attic or foundation whose
/// `WithinInfiltrationVolume` is true or, when absent, defaults to true
/// (a conditioned basement or crawlspace, defaults.rb:1074-1113).
fn infiltration_volume_locations(details: &XmlNode) -> Vec<&'static str> {
    let mut locations = vec![CONDITIONED_SPACE];
    for (group, element, type_element) in [
        (&ATTICS, &ATTIC, &ATTIC_TYPE),
        (&FOUNDATIONS, &FOUNDATION, &FOUNDATION_TYPE),
    ] {
        for space in details
            .path(&[&ENCLOSURE, group])
            .into_iter()
            .flat_map(|node| node.children_named(element))
        {
            let Some(location) = space_location(space, type_element) else {
                continue;
            };
            let within = child_text(space, &WITHIN_INFILTRATION_VOLUME)
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
            (&ROOFS, &ROOF),
            (&RIM_JOISTS, &RIM_JOIST),
            (&WALLS, &WALL),
            (&FOUNDATION_WALLS, &FOUNDATION_WALL),
            (&FLOORS, &FLOOR),
            (&SLABS, &SLAB),
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
                let area_m2 = parse_value_with_units(surface.child(&AREA), ValueKind::Area)?
                    .ok_or_else(|| {
                        HpxmlError::Parse(
                            format!("{} in '{interior}' has no Area", element.name).into(),
                        )
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
    let measurement = details.first_descendant(&AIR_INFILTRATION_MEASUREMENT);
    let unit_total = match measurement {
        Some(m) => child_text(m, &TYPE_OF_INFILTRATION_LEAKAGE).as_deref() == Some("unit total"),
        None => true,
    };
    if !unit_total {
        return Ok(1.0);
    }
    let given = match measurement {
        Some(m) => parse_value_with_units(m.path(&[&EXTENSION, &AEXT]), ValueKind::Raw)?,
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
    let construction = details.path(&[&BUILDING_SUMMARY, &BUILDING_CONSTRUCTION]);
    let required = |name: &'static super::elements::HpxmlElement,
                    kind: ValueKind|
     -> Result<f64, HpxmlError> {
        parse_value_with_units(construction.and_then(|c| c.child(name)), kind)?.ok_or_else(|| {
            HpxmlError::Parse(
                format!(
                    "missing {}; the height of the walls above grade derives from it",
                    name.name
                )
                .into(),
            )
        })
    };
    let floors = required(&NUMBEROF_CONDITIONED_FLOORS_ABOVE_GRADE, ValueKind::Raw)?;
    let conditioned_floor_area_m2 = required(&CONDITIONED_FLOOR_AREA, ValueKind::Area)?;
    let ceiling_m = average_ceiling_height_m(
        details,
        construction.and_then(|c| c.child(&AVERAGE_CEILING_HEIGHT)),
        parse_value_with_units(
            construction.and_then(|c| c.child(&CONDITIONED_BUILDING_VOLUME)),
            ValueKind::Volume,
        )?,
        conditioned_floor_area_m2,
        &mut Vec::new(),
    )?;
    Ok(foundation_top_in(details)? + ceiling_m * floors)
}

/// The infiltration height OS-HPXML defaults when the air infiltration
/// measurement declares none, in metres (OpenStudio-HPXML v1.12.0
/// `HPXMLtoOpenStudio/resources/defaults.rb:5789-5856`,
/// `get_infiltration_height_and_volume`, ported term for term): the
/// average ceiling height per conditioned floor above grade, plus half a
/// foot of rim joist and subfloor per added storey above the first (a 2x6
/// rim joist at 5.5 in plus a half-inch subfloor, defaults.rb:5804), plus
/// the tallest attic within the infiltration volume, plus the tallest
/// above-grade foundation height over the foundations within the volume,
/// raised by their rim joist and sill plate (9 in, a 2x8 rim joist at
/// 7.5 in plus a 1.5 in sill plate, per ASHRAE 140,
/// defaults.rb:5834-5835); a foundation with no FoundationWall
/// contributes the assumed zone height instead (geometry.rb:1352-1359,
/// 8 ft for a basement or garage, 3 ft for a crawlspace). The foundation
/// term drops for a walkout
/// basement, whose height the base term already carries
/// (defaults.rb:5845-5848: a conditioned basement with every conditioned
/// floor above grade). The arithmetic is SI-native (the heights HARES
/// stores are metres; only the ported inch constants convert), so a
/// building whose terms are all zero keeps the height the previous
/// fallback computed, on bits.
///
/// The attic term reads the same roof height the attic zone's geometry
/// carries (`calculate_height_and_footprint_of_roofs`), so the estimate
/// and the model agree on one attic.
///
/// # Errors
///
/// [`HpxmlError::MissingField`] or a parse error naming the element when an
/// input the estimate reads is missing or malformed: the conditioned floor
/// area the average ceiling height derives from, a foundation wall without
/// a height.
pub fn infiltration_height_estimate_m(building: &Building) -> Result<f64, HpxmlError> {
    use super::zone_geometry::roof_height_and_footprint_m;
    use crate::hpxml::ZoneType;

    const RIM_JOIST_PER_STORY_FT: f64 = 0.5; // a 2x6 rim joist at 5.5 in plus a half-inch subfloor
    const RIM_JOIST_SILL_PLATE_FT: f64 = 9.0 / 12.0; // a 2x8 rim joist at 7.5 in plus a 1.5 in sill plate, ASHRAE 140

    let details = &building.details_xml;
    let construction = details.path(&[&BUILDING_SUMMARY, &BUILDING_CONSTRUCTION]);
    // The ceiling the engine's own geometry carries: the same value the
    // previous fallback ran, so a building whose estimate terms are all
    // zero keeps its height on bits. (OS-HPXML's port reads its model's
    // average ceiling height the same way: the defaulted, rounded value
    // the geometry uses, not a re-derivation.)
    let ceiling_m = building.ceiling_height_m;
    if !(ceiling_m.is_finite() && ceiling_m > 0.0) {
        return Err(HpxmlError::Parse(
            format!(
                "the building gives no usable ceiling height ({ceiling_m} m); the infiltration \
             height estimate derives from it"
            )
            .into(),
        ));
    }
    let floors_above_grade = building.floors_above_grade;
    let total_floors = parse_value_with_units(
        construction.and_then(|c| c.child(&NUMBEROF_CONDITIONED_FLOORS)),
        ValueKind::Raw,
    )?;

    // The base height, excluding foundations and attics
    // (defaults.rb:5801-5806).
    let mut height_m = ceiling_m * floors_above_grade;
    if floors_above_grade > 1.0 {
        height_m += (floors_above_grade - 1.0) * conv::length_ft_to_m(RIM_JOIST_PER_STORY_FT);
    }

    let within_locations = infiltration_volume_locations(details);

    // The attics within the infiltration volume: the tallest zone height
    // (defaults.rb:5817-5831). HARES builds one attic zone, so its roof
    // height is the term.
    if within_locations
        .iter()
        .any(|location| location.starts_with("attic"))
        && let Some((attic_height_m, _)) =
            roof_height_and_footprint_m(details, &ZoneType::Attic, &mut Vec::new())?
    {
        height_m += attic_height_m;
    }

    // The foundations within the infiltration volume: the tallest
    // above-grade height, raised by the rim joist and sill plate
    // (defaults.rb:5833-5843). The above-grade height is the tallest
    // foundation wall's height less its depth below grade
    // (geometry.rb:1347-1348); a foundation with no FoundationWall
    // contributes the assumed zone height instead (geometry.rb:1352-1359:
    // 8 ft for a basement or a garage, 3 ft for a crawlspace, the
    // assumption HPXML's Wall elements carry no height).
    let mut foundation_height_m = 0.0_f64;
    let mut foundation_locations: Vec<(String, f64)> = Vec::new();
    for (location, wall) in surfaces(details, &FOUNDATION_WALLS, &FOUNDATION_WALL) {
        let height =
            parse_value_with_units(wall.child(&HEIGHT), ValueKind::Length)?.ok_or_else(|| {
                HpxmlError::Parse(format!("foundation wall in '{location}' has no Height").into())
            })?;
        let depth = parse_value_with_units(wall.child(&DEPTH_BELOW_GRADE), ValueKind::Length)?;
        let above_grade_m = (height - depth.unwrap_or(0.0)).max(0.0);
        let canonical = canonical(&location).to_string();
        let entry = foundation_locations
            .iter_mut()
            .find(|(known, _)| *known == canonical);
        match entry {
            Some((_, tallest)) => *tallest = tallest.max(above_grade_m),
            None => foundation_locations.push((canonical, above_grade_m)),
        }
    }
    for location in within_locations.iter() {
        let Some(above_grade_m) = foundation_locations
            .iter()
            .find(|(known, _)| known == location)
            .map(|(_, above_grade_m)| *above_grade_m)
            .or_else(|| {
                // The no-foundation-wall assumption, in the location names
                // HARES's parse resolves (a vented crawlspace resolves
                // elsewhere): a basement or garage is 8 ft, a crawlspace
                // 3 ft (geometry.rb:1352-1359). A location that is no
                // foundation kind (the conditioned space, an attic) has no
                // term here at all.
                match *location {
                    "basement - conditioned" | "basement - unconditioned" | "garage" => {
                        Some(conv::length_ft_to_m(8.0))
                    }
                    "crawlspace - unvented" | "crawlspace - conditioned" => {
                        Some(conv::length_ft_to_m(3.0))
                    }
                    _ => None,
                }
            })
        else {
            continue;
        };
        let raised_m = above_grade_m + conv::length_ft_to_m(RIM_JOIST_SILL_PLATE_FT);
        foundation_height_m = foundation_height_m.max(raised_m);
    }
    let has_conditioned_basement = details
        .path(&[&ENCLOSURE, &FOUNDATIONS])
        .into_iter()
        .flat_map(|foundations| foundations.children_named(&FOUNDATION))
        .any(|foundation| {
            space_location(foundation, &FOUNDATION_TYPE)
                .is_some_and(|location| location == "basement - conditioned")
        });
    if has_conditioned_basement && total_floors.is_some_and(|total| total == floors_above_grade) {
        // A walkout basement: its height is already in the base term
        // (defaults.rb:5845-5848).
        foundation_height_m = 0.0;
    }
    height_m += foundation_height_m;

    Ok(height_m)
}
