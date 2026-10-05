//! OS-HPXML's zone geometry rules (OpenStudio-HPXML v1.12.0
//! `HPXMLtoOpenStudio/resources/geometry.rb` and `defaults.rb`): the floor
//! area, height and volume of the foundation, garage and attic spaces, and
//! the conditioned volume a file does not give.

use std::collections::BTreeMap;

use hares_physics::units as conv;
use hares_types::Warning;

use super::HpxmlError;
use super::building::{ValueKind, XmlNode, ZoneType, parse_value_with_units, parse_zone_label};
use super::xml_helpers::{child_text, element_id};

/// Floor area, height and air volume of one HARES zone, and the HPXML
/// location that covers most of its floor.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SpaceGeometry {
    pub floor_area_m2: f64,
    pub height_m: f64,
    pub volume_m3: f64,
    pub location: String,
}

/// Assumed height of a foundation or garage space with no foundation walls
/// (geometry.rb:1352-1358): 8 ft for basements and garages, 3 ft for
/// crawlspaces.
fn assumed_zone_height_ft(location: &str) -> f64 {
    if location.starts_with("crawlspace") {
        3.0
    } else {
        8.0
    }
}

/// Every enclosure surface of `group`/`element` with its
/// `InteriorAdjacentTo` text.
pub(super) fn surfaces<'a>(
    details: &'a XmlNode,
    group: &'a str,
    element: &'a str,
) -> impl Iterator<Item = (String, &'a XmlNode)> {
    details
        .path(&["Enclosure", group])
        .into_iter()
        .flat_map(move |node| node.children_named(element))
        .filter_map(|node| child_text(node, "InteriorAdjacentTo").map(|loc| (loc, node)))
}

/// The height of the space at `location`: its tallest foundation wall, or
/// the assumed height when it has none (geometry.rb:1340-1365,
/// `calculate_zone_height`). A basement or crawlspace without foundation
/// walls lacks an input, so its assumption is recorded as a warning; a
/// garage's height has no HPXML input at all (its walls carry no height),
/// so 8 ft is OS-HPXML's model of every garage and is not.
fn location_height_m(
    details: &XmlNode,
    location: &str,
    warnings: &mut Vec<Warning>,
) -> Result<f64, HpxmlError> {
    let mut tallest_m: Option<f64> = None;
    for (wall_location, wall) in surfaces(details, "FoundationWalls", "FoundationWall") {
        if wall_location == location
            && let Some(height) = parse_value_with_units(wall.child("Height"), ValueKind::Length)?
        {
            tallest_m = Some(tallest_m.map_or(height, |h: f64| h.max(height)));
        }
    }
    Ok(match tallest_m {
        Some(height) => height,
        None => {
            let assumed_ft = assumed_zone_height_ft(location);
            // 8 ft is OS-HPXML's model of every garage, not a substitution for a missing input.
            if parse_zone_label(location) != ZoneType::Garage {
                warnings.push(Warning::new(
                    "hpxml",
                    format!(
                        "'{location}' has no foundation wall with a Height; its height is \
                         assumed to be {assumed_ft} ft as OS-HPXML does"
                    ),
                ));
            }
            conv::length_ft_to_m(assumed_ft)
        }
    })
}

/// Slab area per HPXML location of `zone_type`.
fn slab_areas_m2(
    details: &XmlNode,
    zone_type: &ZoneType,
) -> Result<BTreeMap<String, f64>, HpxmlError> {
    let mut areas: BTreeMap<String, f64> = BTreeMap::new();
    for (location, slab) in surfaces(details, "Slabs", "Slab") {
        if parse_zone_label(&location) == *zone_type {
            let area =
                parse_value_with_units(slab.child("Area"), ValueKind::Area)?.ok_or_else(|| {
                    HpxmlError::Parse(format!("slab in '{location}' has no Area").into())
                })?;
            *areas.entry(location).or_default() += area;
        }
    }
    Ok(areas)
}

/// Geometry of a foundation or garage zone: per HPXML location, the slab
/// area times the location's height (geometry.rb:1315-1324,
/// `calculate_zone_volume`), summed; the floor area is the slab area. A
/// zone with no slab takes its HPXML `FloorArea` and the height of the
/// first location its surfaces name. `None` when it has neither.
pub(super) fn slab_space_geometry(
    details: &XmlNode,
    zone_type: &ZoneType,
    declared_floor_area_m2: Option<f64>,
    warnings: &mut Vec<Warning>,
) -> Result<Option<SpaceGeometry>, HpxmlError> {
    let slab_areas = slab_areas_m2(details, zone_type)?;
    let floor_area_m2: f64 = slab_areas.values().sum();
    if floor_area_m2 > 0.0 {
        let mut volume_m3 = 0.0;
        for (location, area) in &slab_areas {
            volume_m3 += area * location_height_m(details, location, warnings)?;
        }
        let location = slab_areas
            .iter()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(location, _)| location.clone())
            .expect("a positive slab area has a location");
        return Ok(Some(SpaceGeometry {
            floor_area_m2,
            height_m: volume_m3 / floor_area_m2,
            volume_m3,
            location,
        }));
    }
    let Some(floor_area_m2) = declared_floor_area_m2.filter(|area| *area > 0.0) else {
        return Ok(None);
    };
    let Some(location) = first_location(details, zone_type) else {
        return Ok(None);
    };
    let height_m = location_height_m(details, &location, warnings)?;
    Ok(Some(SpaceGeometry {
        floor_area_m2,
        height_m,
        volume_m3: floor_area_m2 * height_m,
        location,
    }))
}

/// Height and footprint of the roofs over the HPXML locations of
/// `zone_type`, as a square hip roof of their average pitch
/// (geometry.rb:1373-1393, `calculate_height_and_footprint_of_roofs`); a
/// flat roof over an attic is 2 ft high. `None` when no roof covers such a
/// location; a roof with no `Pitch` (which OS-HPXML requires) is recorded
/// as a warning and also gives `None`.
fn roof_height_and_footprint_m(
    details: &XmlNode,
    zone_type: &ZoneType,
    warnings: &mut Vec<Warning>,
) -> Result<Option<(f64, f64)>, HpxmlError> {
    let mut area_m2 = 0.0;
    let mut pitches = Vec::new();
    for (location, roof) in surfaces(details, "Roofs", "Roof") {
        if parse_zone_label(&location) != *zone_type {
            continue;
        }
        let area = parse_value_with_units(roof.child("Area"), ValueKind::Area)?
            .ok_or_else(|| HpxmlError::Parse(format!("roof in '{location}' has no Area").into()))?;
        let Some(pitch) = parse_value_with_units(roof.child("Pitch"), ValueKind::Raw)? else {
            warnings.push(Warning::new(
                "hpxml",
                format!(
                    "roof '{}' over '{location}' has no Pitch; the {zone_type:?} space \
                     geometry OS-HPXML derives from its roofs is unavailable",
                    element_id(roof).unwrap_or_default()
                ),
            ));
            return Ok(None);
        };
        area_m2 += area;
        pitches.push(pitch / 12.0);
    }
    if pitches.is_empty() {
        return Ok(None);
    }
    let slope = pitches.iter().sum::<f64>() / pitches.len() as f64;
    let footprint_m2 = area_m2 / (slope * slope + 1.0).sqrt();
    let height_m = if slope > 0.0 {
        0.5 * slope.atan().sin() * footprint_m2.sqrt()
    } else if *zone_type == ZoneType::Attic {
        conv::length_ft_to_m(2.0)
    } else {
        0.0
    };
    Ok(Some((height_m, footprint_m2)))
}

/// The first HPXML location of `zone_type` an enclosure surface names.
pub(super) fn first_location(details: &XmlNode, zone_type: &ZoneType) -> Option<String> {
    [
        "Roofs/Roof",
        "Slabs/Slab",
        "FoundationWalls/FoundationWall",
        "Walls/Wall",
        "Floors/Floor",
    ]
    .iter()
    .filter_map(|path| path.split_once('/'))
    .flat_map(|(group, element)| surfaces(details, group, element))
    .map(|(location, _)| location)
    .find(|location| parse_zone_label(location) == *zone_type)
}

/// Attic geometry under a square hip roof: the roof footprint, the hip's
/// peak height, and the footprint times a third of that height, at least
/// 0.01 ft³ (geometry.rb:1325-1329, with the height and footprint from
/// geometry.rb:1373-1393). This is OS-HPXML's only attic rule: it applies
/// it to vented and unvented attics alike and has no gable-roof variant,
/// so a gable attic takes the hip volume. `None` when no roof with a pitch
/// covers the attic.
pub(super) fn hip_attic_geometry(
    details: &XmlNode,
    warnings: &mut Vec<Warning>,
) -> Result<Option<SpaceGeometry>, HpxmlError> {
    let Some((height_m, footprint_m2)) =
        roof_height_and_footprint_m(details, &ZoneType::Attic, warnings)?
    else {
        return Ok(None);
    };
    let location = first_location(details, &ZoneType::Attic).expect("an attic roof was found");
    Ok(Some(SpaceGeometry {
        floor_area_m2: footprint_m2,
        height_m,
        volume_m3: (footprint_m2 * height_m / 3.0).max(conv::volume_ft3_to_m3(0.01)),
        location,
    }))
}

fn round_to(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

/// Volume of the conditioned crawlspace, zero without one.
fn conditioned_crawlspace_volume_ft3(
    details: &XmlNode,
    warnings: &mut Vec<Warning>,
) -> Result<f64, HpxmlError> {
    let crawl_location = "crawlspace - conditioned";
    let crawl_area_m2 = slab_areas_m2(details, &ZoneType::Foundation)?
        .get(crawl_location)
        .copied()
        .unwrap_or(0.0);
    if crawl_area_m2 <= 0.0 {
        return Ok(0.0);
    }
    Ok(conv::volume_m3_to_ft3(
        crawl_area_m2 * location_height_m(details, crawl_location, warnings)?,
    ))
}

/// OS-HPXML's average ceiling height (defaults.rb:904-918,
/// `apply_building_construction`): the file's `AverageCeilingHeight`; else,
/// with a `ConditionedBuildingVolume`, that volume less the conditioned
/// crawlspace's over the conditioned floor area; else 8 ft raised over the
/// share of the floor under roofs of the conditioned space (a cathedral
/// ceiling or conditioned attic). Defaults are rounded to 0.01 ft.
pub(super) fn average_ceiling_height_m(
    details: &XmlNode,
    average_ceiling_height: Option<&XmlNode>,
    conditioned_volume_m3: Option<f64>,
    conditioned_floor_area_m2: f64,
    warnings: &mut Vec<Warning>,
) -> Result<f64, HpxmlError> {
    if let Some(height_m) = parse_value_with_units(average_ceiling_height, ValueKind::Length)? {
        return Ok(height_m);
    }
    let cfa_ft2 = conv::area_m2_to_ft2(conditioned_floor_area_m2);
    let base_ft = 8.0;
    let height_ft = match conditioned_volume_m3 {
        Some(volume_m3) => round_to(
            (conv::volume_m3_to_ft3(volume_m3)
                - conditioned_crawlspace_volume_ft3(details, warnings)?)
                / cfa_ft2,
            2,
        ),
        None => match roof_height_and_footprint_m(details, &ZoneType::Conditioned, warnings)? {
            Some((roof_height_m, footprint_m2)) => {
                let roof_average_ft = conv::length_m_to_ft(roof_height_m) / 3.0;
                let roof_fraction = footprint_m2 / conditioned_floor_area_m2;
                round_to(
                    (base_ft + roof_average_ft) * roof_fraction + base_ft * (1.0 - roof_fraction),
                    2,
                )
            }
            None => base_ft,
        },
    };
    Ok(conv::length_ft_to_m(height_ft))
}

/// OS-HPXML's `ConditionedBuildingVolume` for a file that gives none
/// (defaults.rb:919-924, `apply_building_construction`): the conditioned
/// floor area times the average ceiling height, plus the conditioned
/// crawlspace volume, rounded to the cubic foot.
pub(super) fn default_conditioned_volume_m3(
    details: &XmlNode,
    average_ceiling_height_m: f64,
    conditioned_floor_area_m2: f64,
    warnings: &mut Vec<Warning>,
) -> Result<f64, HpxmlError> {
    let cfa_ft2 = conv::area_m2_to_ft2(conditioned_floor_area_m2);
    let crawl_ft3 = conditioned_crawlspace_volume_ft3(details, warnings)?;
    Ok(conv::volume_ft3_to_m3(
        (cfa_ft2 * conv::length_m_to_ft(average_ceiling_height_m) + crawl_ft3).round(),
    ))
}
