//! OS-HPXML's zone volume rules (OpenStudio-HPXML v1.12.0
//! `HPXMLtoOpenStudio/resources/geometry.rb` and `defaults.rb`), for the
//! volumes an HPXML file does not give and the OCHRE geometry cannot
//! derive.

use std::collections::BTreeSet;

use hares_physics::units as conv;

use super::HpxmlError;
use super::building::{ValueKind, XmlNode, ZoneType, parse_value_with_units, parse_zone_label};
use super::xml_helpers::child_text;

/// Assumed height of a foundation or garage space with no foundation walls
/// (geometry.rb:1352-1358): 8 ft for basements and garages, 3 ft for
/// crawlspaces.
fn assumed_zone_height_m(location: &str) -> f64 {
    if location.starts_with("crawlspace") {
        conv::length_ft_to_m(3.0)
    } else {
        conv::length_ft_to_m(8.0)
    }
}

/// Every enclosure surface of `group`/`element` with its
/// `InteriorAdjacentTo` text.
fn surfaces<'a>(
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

/// Volume of the HPXML locations of `zone_type` that slabs lie in: per
/// location, the slab area times the tallest foundation wall of that
/// location, or the assumed height when it has none (geometry.rb:1315-1324,
/// `calculate_zone_volume`, with `calculate_zone_height`,
/// geometry.rb:1340-1365). `None` when no slab lies in such a location.
pub(super) fn slab_zone_volume_m3(
    details: &XmlNode,
    zone_type: &ZoneType,
) -> Result<Option<f64>, HpxmlError> {
    let locations: BTreeSet<String> = surfaces(details, "Slabs", "Slab")
        .map(|(location, _)| location)
        .filter(|location| parse_zone_label(location) == *zone_type)
        .collect();
    let mut volume_m3 = 0.0;
    for location in &locations {
        volume_m3 += slab_zone_volume_m3_at(details, location)?.unwrap_or(0.0);
    }
    Ok((volume_m3 > 0.0).then_some(volume_m3))
}

/// Height and footprint of the roofs over the HPXML locations of
/// `zone_type`, as a square hip roof of their average pitch
/// (geometry.rb:1373-1393, `calculate_height_and_footprint_of_roofs`); a
/// flat roof over an attic is 2 ft high. `None` when no roof covers such a
/// location or a roof has no pitch.
fn roof_height_and_footprint_m(
    details: &XmlNode,
    zone_type: &ZoneType,
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

/// Attic volume: the roof footprint times a third of the roof height, a
/// square hip roof, and at least 0.01 ft³ (geometry.rb:1325-1329). `None`
/// when no roof with a pitch covers the attic.
pub(super) fn attic_volume_m3(details: &XmlNode) -> Result<Option<f64>, HpxmlError> {
    Ok(roof_height_and_footprint_m(details, &ZoneType::Attic)?
        .map(|(height, footprint)| (footprint * height / 3.0).max(conv::volume_ft3_to_m3(0.01))))
}

/// OS-HPXML's `ConditionedBuildingVolume` for a file that gives none
/// (defaults.rb:899-924, `apply_building_construction`): the conditioned
/// floor area times the average ceiling height, plus the conditioned
/// crawlspace volume, rounded to the cubic foot. The average ceiling
/// height is the file's `AverageCeilingHeight`, or 8 ft raised over the
/// share of the floor under roofs of the conditioned space (a cathedral
/// ceiling or conditioned attic), rounded to 0.01 ft.
pub(super) fn default_conditioned_volume_m3(
    details: &XmlNode,
    average_ceiling_height: Option<&XmlNode>,
    conditioned_floor_area_m2: f64,
) -> Result<f64, HpxmlError> {
    let round_to = |value: f64, places: i32| {
        let scale = 10f64.powi(places);
        (value * scale).round() / scale
    };
    let base_ft = 8.0;
    let ceiling_height_ft = match parse_value_with_units(average_ceiling_height, ValueKind::Length)?
    {
        Some(height_m) => conv::length_m_to_ft(height_m),
        None => match roof_height_and_footprint_m(details, &ZoneType::Conditioned)? {
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
    let crawl_ft3 = conv::volume_m3_to_ft3(
        slab_zone_volume_m3_at(details, "crawlspace - conditioned")?.unwrap_or(0.0),
    );
    let cfa_ft2 = conv::area_m2_to_ft2(conditioned_floor_area_m2);
    Ok(conv::volume_ft3_to_m3(
        (cfa_ft2 * ceiling_height_ft + crawl_ft3).round(),
    ))
}

/// [`slab_zone_volume_m3`] for the one HPXML location `location`.
fn slab_zone_volume_m3_at(details: &XmlNode, location: &str) -> Result<Option<f64>, HpxmlError> {
    let mut area_m2 = 0.0;
    for (slab_location, slab) in surfaces(details, "Slabs", "Slab") {
        if slab_location == location {
            area_m2 +=
                parse_value_with_units(slab.child("Area"), ValueKind::Area)?.ok_or_else(|| {
                    HpxmlError::Parse(format!("slab in '{location}' has no Area").into())
                })?;
        }
    }
    if area_m2 <= 0.0 {
        return Ok(None);
    }
    let mut height_m: Option<f64> = None;
    for (wall_location, wall) in surfaces(details, "FoundationWalls", "FoundationWall") {
        if wall_location == location
            && let Some(height) = parse_value_with_units(wall.child("Height"), ValueKind::Length)?
        {
            height_m = Some(height_m.map_or(height, |h: f64| h.max(height)));
        }
    }
    Ok(Some(
        area_m2 * height_m.unwrap_or_else(|| assumed_zone_height_m(location)),
    ))
}
