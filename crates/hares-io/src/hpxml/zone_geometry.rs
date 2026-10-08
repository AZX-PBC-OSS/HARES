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

/// Slab areas per HPXML location, keyed by the location's text.
fn slab_areas_m2_by_location(details: &XmlNode) -> Result<BTreeMap<String, f64>, HpxmlError> {
    let mut areas: BTreeMap<String, f64> = BTreeMap::new();
    for (location, slab) in surfaces(details, "Slabs", "Slab") {
        let area = parse_value_with_units(slab.child("Area"), ValueKind::Area)?
            .ok_or_else(|| HpxmlError::Parse(format!("slab in '{location}' has no Area").into()))?;
        *areas.entry(location).or_default() += area;
    }
    Ok(areas)
}

/// Slab area per HPXML location of `zone_type`.
fn slab_areas_m2(
    details: &XmlNode,
    zone_type: &ZoneType,
) -> Result<BTreeMap<String, f64>, HpxmlError> {
    let mut areas: BTreeMap<String, f64> = BTreeMap::new();
    for (location, area) in slab_areas_m2_by_location(details)? {
        if parse_zone_label(&location) == *zone_type {
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

/// The conditioned storeys a gable attic sits on: the height of their
/// above-grade walls (ceiling height times floors above grade) and the
/// conditioned floor area per floor (conditioned floor area over all
/// conditioned floors, the basement's included).
#[derive(Debug, Clone, Copy)]
pub(super) struct Storey {
    pub wall_height_m: f64,
    pub floor_area_m2: f64,
}

/// How far the attic's floors and the floor area per storey may differ
/// from its roof footprint and still be read as the one rectangle under it.
/// Areas come rounded to the square foot or the tenth of one, and BEopt
/// writes roofs without overhang, so a real match is within a fraction of a
/// percent; a wing or a smaller upper floor differs by far more.
const FOOTPRINT_MATCH_FRACTION: f64 = 0.02;

/// Attic geometry: the roof footprint (roof area over sqrt(1 + slope²),
/// geometry.rb:1373-1393), a height and the air volume, at least 0.01 ft³.
///
/// A gable attic takes its geometric volume, half the footprint times the
/// ridge rise, rise = (span / 2) tan θ, with the span from the storey under
/// it (see [`gable_rise_m`]). This departs from OS-HPXML, whose one attic
/// rule is a square hip whatever the roof (geometry.rb:1315-1330, "Assume
/// square hip roof"): for a gable attic that undercounts the air, by 27 %
/// on base.xml (104.7 m³ against its 143.4 m³). Any other attic, and a
/// gable whose span the HPXML does not determine, is OS-HPXML's square hip:
/// the hip's peak height 0.5 sin(atan(slope)) sqrt(footprint) and a third of
/// the footprint times it. `None` when no roof with a pitch covers the
/// attic.
pub(super) fn attic_geometry(
    details: &XmlNode,
    storey: Option<Storey>,
    warnings: &mut Vec<Warning>,
) -> Result<Option<SpaceGeometry>, HpxmlError> {
    let Some((hip_height_m, footprint_m2)) =
        roof_height_and_footprint_m(details, &ZoneType::Attic, warnings)?
    else {
        return Ok(None);
    };
    let location = first_location(details, &ZoneType::Attic).expect("an attic roof was found");
    let (height_m, volume_m3) = match gable_rise_m(details, footprint_m2, storey, warnings)? {
        Some(rise_m) => (rise_m, footprint_m2 * rise_m / 2.0),
        None => (hip_height_m, footprint_m2 * hip_height_m / 3.0),
    };
    Ok(Some(SpaceGeometry {
        floor_area_m2: footprint_m2,
        height_m,
        volume_m3: volume_m3.max(conv::volume_ft3_to_m3(0.01)),
        location,
    }))
}

/// The ridge rise of a gable attic, (span / 2) tan θ; `None` for an attic
/// its HPXML does not show to be one, or whose span it does not determine.
///
/// A gable attic has walls to the outside, its two gable ends, under roofs
/// of one positive pitch that face two opposite ways where they give an
/// azimuth. Gable walls over roofs of several pitches or orientations leave
/// the roof shape unknown, which is a warning.
///
/// The span is a side of the rectangle under the roof: its area the roof
/// footprint, its perimeter the conditioned walls' area over the storey
/// wall height (see [`storey_rectangle_sides_m`]). The gable ends stand on
/// one pair of sides, the pair whose triangles, side² tan θ / 4, come
/// nearer the gable walls' area per end.
///
/// The area per end comes from the walls to the outside. Two or more are
/// the ends themselves and must be of equal size. The one outside gable
/// wall of a detached sample covers both ends (base.xml's 225 ft² is two
/// 112.5 ft² triangles on a 30 ft span), while an attached unit's one
/// outside gable wall is a single end, its party-side twin the attic
/// wall the HPXML also lists (`base-bldgtype-attached.xml`'s 168.7 ft²
/// Wall3, with Wall4 attic to attic); both readings of a single wall are
/// tried, and the one whose ends agree with a span's triangle wins.
///
/// The gable wall area picks that pair and must agree with its triangle,
/// from [`GABLE_END_RATIO_MIN`] to [`gable_end_ratio_max`] times it. It does
/// not size the rise: BEopt's gable walls include the eave overhang
/// (144.5 ft² per end on a 30 ft span at 6:12, where the triangle is
/// 112.5 ft²), so a rise taken from them, sqrt(end area × tan θ), overstates
/// the volume. The span must also be well determined by the walls: a
/// [`WALL_AREA_TOLERANCE`] error in their area may move it by at most
/// [`SPAN_SENSITIVITY_MAX`].
fn gable_rise_m(
    details: &XmlNode,
    footprint_m2: f64,
    storey: Option<Storey>,
    warnings: &mut Vec<Warning>,
) -> Result<Option<f64>, HpxmlError> {
    let mut gable_areas_m2: Vec<f64> = Vec::new();
    for (location, wall) in surfaces(details, "Walls", "Wall") {
        if parse_zone_label(&location) == ZoneType::Attic
            && child_text(wall, "ExteriorAdjacentTo").as_deref() == Some("outside")
        {
            gable_areas_m2.push(
                parse_value_with_units(wall.child("Area"), ValueKind::Area)?.ok_or_else(|| {
                    HpxmlError::Parse(format!("wall in '{location}' has no Area").into())
                })?,
            );
        }
    }
    if gable_areas_m2.is_empty() {
        return Ok(None);
    }
    let mut pitches = Vec::new();
    let mut azimuths = Vec::new();
    for (location, roof) in surfaces(details, "Roofs", "Roof") {
        if parse_zone_label(&location) != ZoneType::Attic {
            continue;
        }
        if let Some(pitch) = parse_value_with_units(roof.child("Pitch"), ValueKind::Raw)? {
            pitches.push(pitch);
        }
        if let Some(azimuth) = parse_value_with_units(roof.child("Azimuth"), ValueKind::Raw)? {
            let azimuth = azimuth.rem_euclid(360.0);
            if !azimuths.iter().any(|a: &f64| (a - azimuth).abs() < 1e-6) {
                azimuths.push(azimuth);
            }
        }
    }
    let one_pitch = pitches
        .first()
        .is_some_and(|&p| p > 0.0 && pitches.iter().all(|q| (q - p).abs() < 1e-9));
    let two_ways = match azimuths.as_slice() {
        [] => true,
        [a, b] => ((a - b).abs() - 180.0).abs() < 1e-6,
        _ => false,
    };
    if !(one_pitch && two_ways) {
        warnings.push(Warning::new(
            "hpxml",
            format!(
                "the attic has {:.1} m2 of gable walls, but its roofs (pitches \
                 {pitches:?}, azimuths {azimuths:?}) are not one gable; its volume is \
                 OS-HPXML's square hip",
                gable_areas_m2.iter().sum::<f64>()
            ),
        ));
        return Ok(None);
    }
    let slope = pitches[0] / 12.0;
    let Some(storey) = storey else {
        return Ok(span_unknown(
            warnings,
            "the number of conditioned floors is not given",
        ));
    };
    let Some(rectangle) = storey_rectangle_sides_m(details, footprint_m2, storey, warnings)? else {
        return Ok(None);
    };
    let ends_m2: Vec<f64> = if gable_areas_m2.len() == 1 {
        vec![gable_areas_m2[0] / 2.0, gable_areas_m2[0]]
    } else {
        let mean_m2 = gable_areas_m2.iter().sum::<f64>() / gable_areas_m2.len() as f64;
        let unequal: Vec<f64> = gable_areas_m2
            .iter()
            .filter(|a| (*a - mean_m2).abs() > GABLE_ENDS_EQUAL_MAX * mean_m2)
            .copied()
            .collect();
        if !unequal.is_empty() {
            return Ok(span_unknown(
                warnings,
                &format!(
                    "its {} outside gable walls are not of equal size ({:?})",
                    gable_areas_m2.len(),
                    gable_areas_m2
                ),
            ));
        }
        vec![mean_m2]
    };
    let triangle_m2 = |side_m: f64| side_m * side_m * slope / 4.0;
    let mut best: Option<(f64, f64)> = None;
    let mut reasons: Vec<String> = Vec::new();
    for &end_area_m2 in &ends_m2 {
        let span_is_long = (triangle_m2(rectangle.long_m) - end_area_m2).abs()
            < (triangle_m2(rectangle.short_m) - end_area_m2).abs();
        let span_m = if span_is_long {
            rectangle.long_m
        } else {
            rectangle.short_m
        };
        match rectangle.side_sensitivity(span_is_long) {
            Some(sensitivity) if sensitivity <= SPAN_SENSITIVITY_MAX => {}
            sensitivity => {
                reasons.push(format!(
                    "a {:.0} % error in its conditioned walls' area moves the {span_m:.2} m span \
                     by {}, more than {:.0} %: the footprint is too near a square for its walls \
                     to fix the span",
                    WALL_AREA_TOLERANCE * 100.0,
                    sensitivity.map_or("an unbounded amount".to_string(), |s| format!(
                        "{:.0} %",
                        s * 100.0
                    )),
                    SPAN_SENSITIVITY_MAX * 100.0
                ));
                continue;
            }
        }
        let ratio = end_area_m2 / triangle_m2(span_m);
        let ratio_max = gable_end_ratio_max(span_m);
        if !(GABLE_END_RATIO_MIN..=ratio_max).contains(&ratio) {
            reasons.push(format!(
                "its {end_area_m2:.1} m2 gable ends are {ratio:.2} times the {:.1} m2 triangle \
                 of the {span_m:.2} m span the walls give, outside {GABLE_END_RATIO_MIN} to \
                 {ratio_max:.2}",
                triangle_m2(span_m)
            ));
            continue;
        }
        if best.is_none_or(|(apart, _)| (ratio - 1.0).abs() < apart) {
            best = Some(((ratio - 1.0).abs(), span_m / 2.0 * slope));
        }
    }
    match best {
        Some((_, rise_m)) => Ok(Some(rise_m)),
        None => Ok(span_unknown(warnings, &reasons.join("; "))),
    }
}

/// The most a gable end's area may differ from its twin's: 1 %. The two
/// ends of a gable are the same triangle, and every committed file's gable
/// walls of one home agree to the tenth of a square foot; a difference the
/// eave of one end alone would make is far larger.
const GABLE_ENDS_EQUAL_MAX: f64 = 0.01;

/// The least a gable end's area may be over the triangle of the span the
/// walls give. Every committed ResStock and OS-HPXML sample gable attic
/// whose span the walls determine has gable ends of 0.9993 to 1.0008 times
/// their triangle, the attached units' single outside gable wall included:
/// the walls fix the span to rounding. 0.98 leaves rounding room and
/// rejects a smaller end, which no roof of the walls' span can have.
const GABLE_END_RATIO_MIN: f64 = 0.98;

/// The most a gable end's area may be over the triangle of a `span_m` span:
/// an end can include the eave overhang, (1 + 2 depth / span)², as
/// BEopt's do (1.28 for 2 ft eaves on 30 ft), though OS-HPXML's do not.
/// The deepest eave OS-HPXML's home builder offers is 5 ft
/// (`BuildResidentialHPXML/resources/options/geometry_eaves.tsv`).
fn gable_end_ratio_max(span_m: f64) -> f64 {
    const EAVES_DEPTH_MAX_M: f64 = 5.0 * 0.3048;
    (1.0 + 2.0 * EAVES_DEPTH_MAX_M / span_m).powi(2)
}

/// The short and long sides of the rectangle under the attic: area the
/// roof footprint, perimeter the conditioned walls' gross area (walls whose
/// inside is conditioned space and whose outside is neither an attic nor
/// conditioned space) over the storey wall height. The walls bound the attic
/// only when every attic floor lies over conditioned space and those floors,
/// and the conditioned floor area per floor, each match the roof footprint
/// within [`FOOTPRINT_MATCH_FRACTION`]: a wing or a smaller upper floor
/// would add walls the attic does not sit on. `None`, with a warning naming
/// why, when the rectangle is not determined.
fn storey_rectangle_sides_m(
    details: &XmlNode,
    footprint_m2: f64,
    storey: Storey,
    warnings: &mut Vec<Warning>,
) -> Result<Option<StoreyRectangle>, HpxmlError> {
    let mut attic_floor_m2 = 0.0;
    for (location, floor) in surfaces(details, "Floors", "Floor") {
        let exterior = child_text(floor, "ExteriorAdjacentTo").unwrap_or_default();
        let below = match (parse_zone_label(&location), parse_zone_label(&exterior)) {
            (_, ZoneType::Attic) => location.as_str(),
            (ZoneType::Attic, _) => exterior.as_str(),
            _ => continue,
        };
        if parse_zone_label(below) != ZoneType::Conditioned {
            return Ok(span_unknown(
                warnings,
                &format!(
                    "the attic also lies over '{below}', which the conditioned walls do not bound"
                ),
            ));
        }
        attic_floor_m2 += parse_value_with_units(floor.child("Area"), ValueKind::Area)?
            .ok_or_else(|| HpxmlError::Parse(format!("floor over '{below}' has no Area").into()))?;
    }
    let mismatched =
        |area_m2: f64| (area_m2 - footprint_m2).abs() > FOOTPRINT_MATCH_FRACTION * footprint_m2;
    if mismatched(attic_floor_m2) {
        return Ok(span_unknown(
            warnings,
            &format!(
                "its floors over conditioned space, {attic_floor_m2:.1} m2, do not match its \
                 roof footprint, {footprint_m2:.1} m2"
            ),
        ));
    }
    if mismatched(storey.floor_area_m2) {
        return Ok(span_unknown(
            warnings,
            &format!(
                "the conditioned floor area per floor, {:.1} m2, does not match its roof \
                 footprint, {footprint_m2:.1} m2, so the storeys under it are not one rectangle",
                storey.floor_area_m2
            ),
        ));
    }
    let storey_wall_height_m = storey.wall_height_m;
    let mut wall_area_m2 = 0.0;
    for (location, wall) in surfaces(details, "Walls", "Wall") {
        let exterior = child_text(wall, "ExteriorAdjacentTo").unwrap_or_default();
        if parse_zone_label(&location) == ZoneType::Conditioned
            && !matches!(
                parse_zone_label(&exterior),
                ZoneType::Attic | ZoneType::Conditioned
            )
        {
            wall_area_m2 += parse_value_with_units(wall.child("Area"), ValueKind::Area)?
                .ok_or_else(|| {
                    HpxmlError::Parse(format!("wall in '{location}' has no Area").into())
                })?;
        }
    }
    let half_perimeter_m = wall_area_m2 / storey_wall_height_m / 2.0;
    let sides = (wall_area_m2 > 0.0)
        .then(|| rectangle_sides_m(footprint_m2, half_perimeter_m))
        .flatten();
    let Some((short_m, long_m)) = sides else {
        return Ok(span_unknown(
            warnings,
            &format!(
                "{wall_area_m2:.1} m2 of conditioned walls {storey_wall_height_m:.2} m high do \
                 not enclose a {footprint_m2:.1} m2 rectangle"
            ),
        ));
    };
    Ok(Some(StoreyRectangle {
        short_m,
        long_m,
        half_perimeter_m,
        footprint_m2,
    }))
}

/// The rectangle under a gable attic, with what is needed to test how well
/// its sides are determined.
#[derive(Debug, Clone, Copy)]
struct StoreyRectangle {
    short_m: f64,
    long_m: f64,
    half_perimeter_m: f64,
    footprint_m2: f64,
}

impl StoreyRectangle {
    /// The largest relative change of the side `long` (or the short one)
    /// over a ±[`WALL_AREA_TOLERANCE`] change of the half perimeter, or
    /// `None` when such a change leaves no rectangle. Near a square the sides
    /// come from the small difference of two large numbers, and a small error
    /// in the wall area moves them far.
    fn side_sensitivity(&self, long: bool) -> Option<f64> {
        let side = if long { self.long_m } else { self.short_m };
        let mut spread: f64 = 0.0;
        for factor in [1.0 - WALL_AREA_TOLERANCE, 1.0 + WALL_AREA_TOLERANCE] {
            let (short_m, long_m) =
                rectangle_sides_m(self.footprint_m2, self.half_perimeter_m * factor)?;
            let moved = if long { long_m } else { short_m };
            spread = spread.max((moved - side).abs() / side);
        }
        Some(spread)
    }
}

/// The short and long sides of the rectangle with this area and half
/// perimeter, `None` when there is none.
fn rectangle_sides_m(area_m2: f64, half_perimeter_m: f64) -> Option<(f64, f64)> {
    let discriminant = half_perimeter_m * half_perimeter_m - 4.0 * area_m2;
    (discriminant >= 0.0).then(|| {
        let root = discriminant.sqrt();
        (
            (half_perimeter_m - root) / 2.0,
            (half_perimeter_m + root) / 2.0,
        )
    })
}

/// The error the conditioned wall area may carry as the rectangle's
/// perimeter: 0.25 %. A heuristic, not a measured wall-area error: the
/// committed homes' gable ends agree with the span the walls give to 0.08 %
/// in area ([`GABLE_END_RATIO_MIN`]), and 0.25 % is three times that.
const WALL_AREA_TOLERANCE: f64 = 0.0025;

/// The largest relative change of the span that tolerance may cause before
/// the span counts as undetermined: 10 %, the volume's error then. A
/// 30 x 40 ft footprint (BEopt) moves its span 1.6 to 1.9 %; a footprint
/// within a few percent of a square moves it without bound.
const SPAN_SENSITIVITY_MAX: f64 = 0.10;

/// Records that a gable attic's span is not determined and that its volume
/// falls back to OS-HPXML's square hip.
fn span_unknown<T>(warnings: &mut Vec<Warning>, reason: &str) -> Option<T> {
    warnings.push(Warning::new(
        "hpxml",
        format!(
            "the gable attic's span cannot be determined ({reason}); its volume is OS-HPXML's \
             square hip (geometry.rb:1315-1330)"
        ),
    ));
    None
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
    let crawl_area_m2 = slab_areas_m2_by_location(details)?
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
