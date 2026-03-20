//! HPXML building envelope and geometry parsing.

use std::collections::HashMap;

use quick_xml::Reader;
use quick_xml::events::Event;

use hares_types::{normalize_ascii, parse_trimmed_f64};

use hares_physics::units as conv;

use super::HpxmlError;
use super::xml_helpers::element_id;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SiteType {
    Rural,
    Suburban,
    Urban,
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Site {
    pub elevation_m: Option<f64>,
    pub site_type: Option<SiteType>,
    /// HPXML `<ShieldingOfHome>` — string value ("normal", "exposed", "well-shielded").
    pub shielding_of_home: Option<String>,
    pub latitude_deg: Option<f64>,
    pub longitude_deg: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZoneType {
    Conditioned,
    Attic,
    Garage,
    Foundation,
    Outdoor,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundaryType {
    Wall,
    Roof,
    Floor,
    Window,
    Door,
    FoundationWall,
    RimJoist,
    Slab,
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MaterialLayer {
    pub thickness_m: f64,
    pub conductivity_w_m_k: f64,
    pub density_kg_m3: f64,
    pub specific_heat_j_kg_k: f64,
    pub area_m2: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Boundary {
    pub id: String,
    pub boundary_type: BoundaryType,
    pub area_m2: f64,
    pub azimuth_deg: Option<f64>,
    pub assembly_r_value_m2_k_w: Option<f64>,
    pub r_value_layers_m2_k_w: Vec<f64>,
    pub interior_zone: Option<ZoneType>,
    pub exterior_zone: Option<ZoneType>,
    pub material_layers: Vec<MaterialLayer>,
    /// HPXML construction type (e.g. "WoodStud", "ConcreteMasonryUnit") for LUT matching.
    pub construction_type: Option<String>,
    /// Exterior finish type (e.g. "vinyl siding", "asphalt or fiberglass shingles").
    pub finish_type: Option<String>,
    /// Insulation details string (e.g. "R-13", "Uninsulated") for LUT matching.
    pub insulation_details: Option<String>,
    /// Whether an attic radiant barrier is present on this boundary surface.
    ///
    /// When true, longwave emissivity should be set to [`EMISSIVITY_RADIANT_BARRIER`]
    /// (0.05) rather than the default 0.90.  Applies to roof/attic boundary types.
    pub has_radiant_barrier: bool,
    /// Solar absorptance [-] from HPXML `<SolarAbsorptance>`.
    ///
    /// `None` means use the default: 0.60 for most surfaces, 0.05 for attic
    /// radiant barriers.  Ref: OCHRE `Envelope.py:222`.
    /// Valid range: 0.0–1.0.
    pub solar_absorptance: Option<f64>,
    /// Longwave emittance [-] from HPXML `<Emittance>`.
    ///
    /// `None` means use the default: 0.90 for most surfaces, 0.05 for attic
    /// radiant barriers.  Ref: OCHRE `Envelope.py:222`.
    /// Valid range: 0.0–1.0.
    pub emittance: Option<f64>,
    /// Surface tilt angle [degrees].
    ///
    /// 0 = horizontal facing up (flat roof), 90 = vertical (wall),
    /// 180 = horizontal facing down (floor from above).
    /// For roofs, computed from `<Pitch>` as `atan(pitch / 12)` in degrees.
    /// Ref: OCHRE `hpxml.py` `pitch2deg()`.
    pub tilt_deg: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub id: String,
    pub area_m2: f64,
    pub azimuth_deg: Option<f64>,
    pub u_factor_w_m2_k: Option<f64>,
    pub shgc: Option<f64>,
    /// Interior shading transmittance multiplier applied to SHGC.
    /// Per ANSI/RESNET/ICC 301: `effective_shgc = shgc * interior_shading_fraction`.
    /// A value of 0.70 means 70% of solar passes through (30% blocked).
    pub interior_shading_fraction: f64,
    pub frame_type: Option<String>,
    pub attached_to_wall_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DuctLocation {
    InsideConditionedSpace,
    OutsideConditionedSpace,
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuctType {
    Supply,
    Return,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DuctSystem {
    pub id: String,
    pub leakage_fraction: Option<f64>,
    pub insulation_r_value_m2_k_w: Option<f64>,
    pub surface_area_m2: Option<f64>,
    pub location: DuctLocation,
    pub duct_type: DuctType,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Zone {
    pub zone_type: ZoneType,
    pub floor_area_m2: Option<f64>,
    pub volume_m3: Option<f64>,
    pub attached_wall_ids: Vec<String>,
    pub duct_systems: Vec<DuctSystem>,
    pub vented: bool,
    pub ventilation_ach: Option<f64>,
    pub ventilation_sla: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Building {
    pub site: Site,
    pub zones: Vec<Zone>,
    pub boundaries: Vec<Boundary>,
    pub windows: Vec<Window>,
    pub infiltration_ach50: Option<f64>,
    pub hvac_capacity_w: Option<f64>,
    pub seer2: Option<f64>,
    pub hspf2: Option<f64>,
    pub water_heater_setpoint_c: Option<f64>,
    /// HVAC thermostat setpoints: 24 hourly values in °C (weekday/weekend).
    pub heating_weekday_setpoints_c: Option<Vec<f64>>,
    pub heating_weekend_setpoints_c: Option<Vec<f64>>,
    pub cooling_weekday_setpoints_c: Option<Vec<f64>>,
    pub cooling_weekend_setpoints_c: Option<Vec<f64>>,
    pub battery_round_trip_efficiency: Option<f64>,
    pub pv_tilt_deg: Option<f64>,
    pub conditioned_volume_m3: Option<f64>,
    pub ceiling_height_m: Option<f64>,
    /// `<InfiltrationHeight>` converted from ft to m.
    pub infiltration_height_m: Option<f64>,
    /// `<NumberofConditionedFloorsAboveGrade>` from `<BuildingConstruction>`.
    pub floors_above_grade: Option<f64>,
    /// `<extension><HasFlueOrChimneyInConditionedSpace>` boolean.
    pub has_flue_or_chimney: Option<bool>,
    // TODO: `details_xml` leaks the parse tree (`XmlNode`) into the domain model,
    // forcing `hares-core` to construct XmlNode trees. Extract remaining
    // XML-dependent fields into typed struct members and remove this field.
    pub details_xml: XmlNode,
}

#[derive(Debug, Clone, PartialEq)]
pub struct XmlNode {
    pub name: String,
    pub attrs: HashMap<String, String>,
    pub text: String,
    pub children: Vec<XmlNode>,
}

impl XmlNode {
    pub(crate) fn child(&self, name: &str) -> Option<&XmlNode> {
        self.children.iter().find(|n| n.name == name)
    }

    pub(crate) fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a XmlNode> {
        self.children.iter().filter(move |n| n.name == name)
    }

    fn descendants<'a>(&'a self, name: &'a str, out: &mut Vec<&'a XmlNode>) {
        if self.name == name {
            out.push(self);
        }
        for child in &self.children {
            child.descendants(name, out);
        }
    }

    pub(crate) fn first_descendant(&self, name: &str) -> Option<&XmlNode> {
        if self.name == name {
            return Some(self);
        }
        for child in &self.children {
            if let Some(found) = child.first_descendant(name) {
                return Some(found);
            }
        }
        None
    }

    pub(crate) fn path<'a>(&'a self, path: &[&str]) -> Option<&'a XmlNode> {
        let mut cur = self;
        for segment in path {
            cur = cur.child(segment)?;
        }
        Some(cur)
    }

    fn text_as_f64(&self) -> Option<f64> {
        parse_trimmed_f64(&self.text)
    }
}

pub fn parse_xml_document(xml: &str) -> Result<XmlNode, HpxmlError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut stack: Vec<XmlNode> = Vec::new();
    let mut root: Option<XmlNode> = None;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(tag)) => {
                let name = normalize_name(&String::from_utf8_lossy(tag.name().as_ref()));
                let mut attrs = HashMap::new();
                for attr in tag.attributes().flatten() {
                    let key = normalize_name(&String::from_utf8_lossy(attr.key.as_ref()));
                    let value = attr
                        .decode_and_unescape_value(reader.decoder())
                        .map(|v| v.into_owned())
                        .unwrap_or_default();
                    attrs.insert(key, value);
                }
                stack.push(XmlNode {
                    name,
                    attrs,
                    text: String::new(),
                    children: Vec::new(),
                });
            }
            Ok(Event::Empty(tag)) => {
                let name = normalize_name(&String::from_utf8_lossy(tag.name().as_ref()));
                let mut attrs = HashMap::new();
                for attr in tag.attributes().flatten() {
                    let key = normalize_name(&String::from_utf8_lossy(attr.key.as_ref()));
                    let value = attr
                        .decode_and_unescape_value(reader.decoder())
                        .map(|v| v.into_owned())
                        .unwrap_or_default();
                    attrs.insert(key, value);
                }
                let node = XmlNode {
                    name,
                    attrs,
                    text: String::new(),
                    children: Vec::new(),
                };
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                } else {
                    root = Some(node);
                }
            }
            Ok(Event::Text(text)) => {
                if let Some(node) = stack.last_mut() {
                    if let Ok(value) = text.decode() {
                        if !value.trim().is_empty() {
                            if !node.text.is_empty() {
                                node.text.push(' ');
                            }
                            node.text.push_str(value.trim());
                        }
                    }
                }
            }
            Ok(Event::End(_)) => {
                let node = stack.pop().ok_or_else(|| {
                    HpxmlError::Parse("malformed XML: end tag without start tag".to_string())
                })?;
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                } else {
                    root = Some(node);
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(err) => {
                return Err(HpxmlError::Parse(format!("XML parse failure: {err}")));
            }
        }
        buf.clear();
    }

    root.ok_or_else(|| HpxmlError::Parse("empty XML document".to_string()))
}

pub fn parse_building(xml: &str) -> Result<Building, HpxmlError> {
    let root = parse_xml_document(xml)?;
    parse_building_from_node(&root)
}

pub fn parse_building_from_node(root: &XmlNode) -> Result<Building, HpxmlError> {
    let details = root
        .path(&["Building", "BuildingDetails"])
        .ok_or_else(|| HpxmlError::Parse("missing Building/BuildingDetails".to_string()))?;

    let summary = details
        .child("BuildingSummary")
        .ok_or_else(|| HpxmlError::Parse("missing BuildingSummary".to_string()))?;

    let site_node = summary
        .child("Site")
        .ok_or_else(|| HpxmlError::Parse("missing BuildingSummary/Site".to_string()))?;

    let elevation_m = parse_value_with_units(site_node.child("Elevation"), ValueKind::Length)
        .or_else(|| {
            root.path(&["Building", "Site", "Elevation"])
                .and_then(|n| parse_value_with_units(Some(n), ValueKind::Length))
        })
        .or_else(|| {
            root.first_descendant("Altitude")
                .and_then(|n| parse_value_with_units(Some(n), ValueKind::Length))
        });
    let site_type = site_node
        .child("SiteType")
        .map(|node| parse_site_type(node.text.trim()));
    let shielding_of_home = site_node
        .child("ShieldingOfHome")
        .map(|n| normalize_ascii(&n.text))
        .filter(|s| !s.is_empty());
    let latitude_deg = root
        .path(&["Building", "Site", "Latitude"])
        .and_then(XmlNode::text_as_f64)
        .or_else(|| find_descendant_f64(root, "Latitude", ValueKind::Raw));
    let longitude_deg = root
        .path(&["Building", "Site", "Longitude"])
        .and_then(XmlNode::text_as_f64)
        .or_else(|| find_descendant_f64(root, "Longitude", ValueKind::Raw));

    let conditioned_floor_area_m2 = summary
        .path(&["BuildingConstruction", "ConditionedFloorArea"])
        .and_then(|node| parse_value_with_units(Some(node), ValueKind::Area));

    let conditioned_volume_m3 = summary
        .path(&["BuildingConstruction", "ConditionedBuildingVolume"])
        .and_then(|node| parse_value_with_units(Some(node), ValueKind::Volume));

    let ceiling_height_m = match (conditioned_volume_m3, conditioned_floor_area_m2) {
        (Some(vol), Some(area)) if area > 0.0 => Some(vol / area),
        _ => None,
    };

    let floors_above_grade = summary
        .path(&["BuildingConstruction", "NumberofConditionedFloorsAboveGrade"])
        .and_then(|node| parse_value_with_units(Some(node), ValueKind::Raw));

    // InfiltrationHeight lives under AirInfiltrationMeasurement — HPXML stores it in feet.
    let infiltration_height_m = details
        .first_descendant("InfiltrationHeight")
        .and_then(|node| parse_value_with_units(Some(node), ValueKind::Length));

    // <extension><HasFlueOrChimneyInConditionedSpace> — boolean text
    let has_flue_or_chimney = details
        .first_descendant("HasFlueOrChimneyInConditionedSpace")
        .map(|n| n.text.trim().eq_ignore_ascii_case("true"));

    let mut boundaries = parse_boundaries(details)?;
    let windows = parse_windows(details, &mut boundaries)?;

    let mut zones = build_zone_map(details, conditioned_floor_area_m2);
    assign_walls_to_zones(&boundaries, &mut zones);
    parse_duct_systems(details, &mut zones);

    let mut zones_vec: Vec<Zone> = zones.into_values().collect();
    zones_vec.sort_by_key(|zone| zone_sort_key(&zone.zone_type));

    // Assign volumes to all zones from available geometry.
    let default_height_m = ceiling_height_m.unwrap_or(2.5);
    for zone in &mut zones_vec {
        zone.volume_m3 = match zone.zone_type {
            ZoneType::Conditioned => zone.floor_area_m2.map(|a| a * default_height_m),
            ZoneType::Attic => compute_attic_volume(&boundaries, zone.floor_area_m2),
            ZoneType::Garage | ZoneType::Foundation => {
                zone.floor_area_m2.map(|a| a * default_height_m)
            }
            _ => None,
        };
    }

    Ok(Building {
        site: Site {
            elevation_m,
            site_type,
            shielding_of_home,
            latitude_deg,
            longitude_deg,
        },
        zones: zones_vec,
        boundaries,
        windows,
        infiltration_ach50: extract_first_f64(
            details,
            &[
                "Enclosure",
                "AirInfiltration",
                "AirInfiltrationMeasurement",
                "AirLeakage",
            ],
            ValueKind::Raw,
        )
        .or_else(|| {
            extract_first_f64(
                details,
                &[
                    "AirInfiltration",
                    "AirInfiltrationMeasurement",
                    "AirLeakage",
                ],
                ValueKind::Raw,
            )
        }),
        hvac_capacity_w: find_descendant_f64(details, "HeatingCapacity", ValueKind::Raw)
            .or_else(|| find_descendant_f64(details, "CoolingCapacity", ValueKind::Raw))
            .map(conv::power_btu_h_to_w),
        seer2: find_descendant_f64(details, "SEER2", ValueKind::Raw),
        hspf2: find_descendant_f64(details, "HSPF2", ValueKind::Raw),
        water_heater_setpoint_c: details
            .first_descendant("WaterHeatingSystem")
            .and_then(|wh| find_descendant_f64(wh, "HotWaterTemperature", ValueKind::Temperature)),
        heating_weekday_setpoints_c: parse_hvac_setpoints(details, "Heating", true),
        heating_weekend_setpoints_c: parse_hvac_setpoints(details, "Heating", false),
        cooling_weekday_setpoints_c: parse_hvac_setpoints(details, "Cooling", true),
        cooling_weekend_setpoints_c: parse_hvac_setpoints(details, "Cooling", false),
        battery_round_trip_efficiency: find_descendant_f64(
            details,
            "RoundTripEfficiency",
            ValueKind::Raw,
        ),
        pv_tilt_deg: find_descendant_f64(details, "Tilt", ValueKind::Raw),
        conditioned_volume_m3,
        ceiling_height_m,
        infiltration_height_m,
        floors_above_grade,
        has_flue_or_chimney,
        details_xml: details.clone(),
    })
}

/// Parse HVAC thermostat setpoints from `<HVACControl>`.
///
/// Delegates to the shared `xml_helpers::parse_setpoint_from_control` after
/// locating the HVACControl node.
fn parse_hvac_setpoints(details: &XmlNode, hvac_type: &str, weekday: bool) -> Option<Vec<f64>> {
    let control = super::xml_helpers::find_hvac_control(details)?;
    super::xml_helpers::parse_setpoint_from_control(control, hvac_type, weekday)
}

fn parse_boundaries(details: &XmlNode) -> Result<Vec<Boundary>, HpxmlError> {
    let mut out = Vec::new();
    let boundary_specs = [
        ("Walls", "Wall", BoundaryType::Wall),
        ("Roofs", "Roof", BoundaryType::Roof),
        ("Floors", "Floor", BoundaryType::Floor),
        ("FrameFloors", "FrameFloor", BoundaryType::Floor),
        ("Doors", "Door", BoundaryType::Door),
        ("RimJoists", "RimJoist", BoundaryType::RimJoist),
        (
            "FoundationWalls",
            "FoundationWall",
            BoundaryType::FoundationWall,
        ),
        ("Slabs", "Slab", BoundaryType::Slab),
    ];

    let enclosure = match details.child("Enclosure") {
        Some(node) => node,
        None => return Ok(out),
    };

    for (container, item_name, boundary_type) in boundary_specs {
        if let Some(group) = enclosure.child(container) {
            for node in group.children_named(item_name) {
                out.push(parse_boundary(node, boundary_type.clone())?);
            }
        }
    }

    Ok(out)
}

fn parse_windows(
    details: &XmlNode,
    boundaries: &mut Vec<Boundary>,
) -> Result<Vec<Window>, HpxmlError> {
    let mut windows = Vec::new();
    let Some(enclosure) = details.child("Enclosure") else {
        return Ok(windows);
    };
    let Some(group) = enclosure.child("Windows") else {
        return Ok(windows);
    };

    for window in group.children_named("Window") {
        let id = element_id(window).unwrap_or_else(|| "unknown".to_string());
        let area_m2 =
            parse_value_with_units(window.child("Area"), ValueKind::Area).ok_or_else(|| {
                HpxmlError::Parse(format!("window '{}' is missing required Area element", id))
            })?;
        if area_m2 <= 0.0 {
            return Err(HpxmlError::Parse(format!(
                "window '{}' has non-positive area: {}",
                id, area_m2
            )));
        }
        let azimuth_deg = parse_value_with_units(window.child("Azimuth"), ValueKind::Raw);
        let u_factor_w_m2_k = parse_value_with_units(window.child("UFactor"), ValueKind::UValue);
        let shgc = window.child("SHGC").and_then(XmlNode::text_as_f64);

        // InteriorShading/SummerShadingCoefficient is a transmittance multiplier
        // per ANSI/RESNET/ICC 301-2019 Table 4.2.2(1). Default 0.70 when
        // InteriorShading present but coefficient absent; 1.0 when absent entirely.
        let interior_shading_fraction = match window.child("InteriorShading") {
            Some(shading) => shading
                .child("SummerShadingCoefficient")
                .and_then(XmlNode::text_as_f64)
                .unwrap_or(0.70)
                .clamp(0.0, 1.0),
            None => 1.0,
        };

        let frame_type = window.child("FrameType").map(|n| n.text.trim().to_string());
        let attached_to_wall_id = window
            .child("AttachedToWall")
            .and_then(|n| n.attrs.get("idref").cloned());

        windows.push(Window {
            id: id.clone(),
            area_m2,
            azimuth_deg,
            u_factor_w_m2_k,
            shgc,
            interior_shading_fraction,
            frame_type,
            attached_to_wall_id,
        });

        boundaries.push(Boundary {
            id,
            boundary_type: BoundaryType::Window,
            area_m2,
            azimuth_deg,
            assembly_r_value_m2_k_w: None,
            r_value_layers_m2_k_w: Vec::new(),
            interior_zone: parse_zone_ref(window.child("InteriorAdjacentTo")),
            exterior_zone: parse_zone_ref(window.child("ExteriorAdjacentTo"))
                .or_else(|| infer_exterior_zone(&BoundaryType::Window)),
            material_layers: Vec::new(),
            construction_type: None,
            finish_type: None,
            insulation_details: None,
            has_radiant_barrier: false,
            solar_absorptance: None,
            emittance: None,
            tilt_deg: Some(90.0),
        });
    }

    Ok(windows)
}

fn parse_boundary(node: &XmlNode, boundary_type: BoundaryType) -> Result<Boundary, HpxmlError> {
    let id = element_id(node).unwrap_or_else(|| "unknown".to_string());
    let area_m2 = parse_boundary_area(node, &boundary_type, &id)?;
    let r_value_layers_m2_k_w = parse_nominal_r_layers(node);
    let assembly_r_value_m2_k_w =
        parse_value_with_units(node.child("AssemblyEffectiveRValue"), ValueKind::RValue);
    let material_layers = parse_material_layers(node, area_m2);

    let has_radiant_barrier = node
        .first_descendant("RadiantBarrier")
        .map(|n| n.text.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    // Solar absorptance and emittance from HPXML, validated to [0, 1].
    // Ref: OCHRE hpxml.py:155-158, OCHRE Envelope.py:222.
    let solar_absorptance = parse_value_with_units(
        node.child("SolarAbsorptance"),
        ValueKind::Raw,
    )
    .map(|v| v.clamp(0.0, 1.0));
    let emittance = parse_value_with_units(
        node.child("Emittance"),
        ValueKind::Raw,
    )
    .map(|v| v.clamp(0.0, 1.0));

    // Extract construction metadata for OCHRE LUT matching.
    let (construction_type, finish_type) = extract_construction_metadata(node, &boundary_type);
    let insulation_details = extract_insulation_details(node);

    // Surface tilt from HPXML <Pitch> (roofs) or implied by boundary type.
    // Pitch is rise:12 run (US roofing convention); tilt = atan(pitch/12).
    // Ref: OCHRE hpxml.py pitch2deg().
    let tilt_deg = match boundary_type {
        BoundaryType::Roof => {
            let pitch = parse_value_with_units(node.child("Pitch"), ValueKind::Raw)
                .unwrap_or(0.0);
            Some((pitch / 12.0).atan().to_degrees())
        }
        BoundaryType::Wall | BoundaryType::FoundationWall | BoundaryType::RimJoist => {
            Some(90.0)
        }
        BoundaryType::Floor => Some(0.0),
        BoundaryType::Slab => Some(180.0),
        BoundaryType::Door => Some(90.0),
        _ => None,
    };

    let interior_zone = parse_zone_ref(node.child("InteriorAdjacentTo"));
    let exterior_zone = parse_zone_ref(node.child("ExteriorAdjacentTo"))
        .or_else(|| infer_exterior_zone(&boundary_type));

    Ok(Boundary {
        id,
        boundary_type,
        area_m2,
        azimuth_deg: parse_value_with_units(node.child("Azimuth"), ValueKind::Raw),
        assembly_r_value_m2_k_w,
        r_value_layers_m2_k_w,
        interior_zone,
        exterior_zone,
        material_layers,
        construction_type,
        finish_type,
        insulation_details,
        has_radiant_barrier,
        solar_absorptance,
        emittance,
        tilt_deg,
    })
}

fn parse_boundary_area(
    node: &XmlNode,
    boundary_type: &BoundaryType,
    id: &str,
) -> Result<f64, HpxmlError> {
    let area = parse_value_with_units(node.child("Area"), ValueKind::Area);
    match boundary_type {
        BoundaryType::FoundationWall | BoundaryType::Slab => Ok(area.unwrap_or(0.0)),
        _ => {
            let area_m2 = area.ok_or_else(|| {
                HpxmlError::Parse(format!(
                    "{} '{}' is missing required Area element",
                    boundary_type_label(boundary_type),
                    id
                ))
            })?;
            if area_m2 <= 0.0 {
                return Err(HpxmlError::Parse(format!(
                    "{} '{}' has non-positive area: {}",
                    boundary_type_label(boundary_type),
                    id,
                    area_m2
                )));
            }
            Ok(area_m2)
        }
    }
}

fn boundary_type_label(boundary_type: &BoundaryType) -> &str {
    match boundary_type {
        BoundaryType::Wall => "wall",
        BoundaryType::Roof => "roof",
        BoundaryType::Floor => "floor",
        BoundaryType::Window => "window",
        BoundaryType::Door => "door",
        BoundaryType::FoundationWall => "foundation wall",
        BoundaryType::RimJoist => "rim joist",
        BoundaryType::Slab => "slab",
        BoundaryType::Other(text) => text.as_str(),
    }
}

/// Infer the exterior zone when `<ExteriorAdjacentTo>` is absent.
///
/// HPXML frequently omits this tag for boundary types with unambiguous
/// exterior adjacency (e.g. Roof → outdoor, Slab → ground).
fn infer_exterior_zone(boundary_type: &BoundaryType) -> Option<ZoneType> {
    match boundary_type {
        BoundaryType::Roof | BoundaryType::RimJoist | BoundaryType::Window => {
            Some(ZoneType::Outdoor)
        }
        BoundaryType::Slab => Some(ZoneType::Outdoor), // ground handled later by resolve_exterior
        // Floor/FrameFloor: HPXML requires <ExteriorAdjacentTo>, so exterior
        // zone is always parsed from the element rather than inferred here.
        _ => None,
    }
}

/// Extract construction type and finish type from HPXML boundary elements.
fn extract_construction_metadata(
    node: &XmlNode,
    boundary_type: &BoundaryType,
) -> (Option<String>, Option<String>) {
    match boundary_type {
        BoundaryType::Wall | BoundaryType::FoundationWall | BoundaryType::RimJoist => {
            let construction_type = node
                .child("WallType")
                .and_then(|wt| wt.children.first())
                .map(|child| child.name.clone());
            let finish_type = node
                .child("Siding")
                .map(|n| n.text.trim().to_string())
                .filter(|s| !s.is_empty());
            (construction_type, finish_type)
        }
        BoundaryType::Roof => {
            let construction_type = node.child("Pitch").map(|p| {
                let val = p.text_as_f64().unwrap_or(0.0);
                if val > 0.0 {
                    "Pitched".to_string()
                } else {
                    "Flat".to_string()
                }
            });
            let finish_type = node
                .child("RoofType")
                .map(|n| n.text.trim().to_string())
                .filter(|s| !s.is_empty());
            (construction_type, finish_type)
        }
        BoundaryType::Floor => {
            // <FloorType> child tag name → construction_type
            let construction_type = node
                .child("FloorType")
                .and_then(|ft| ft.children.first())
                .map(|child| child.name.clone());
            (construction_type, None)
        }
        BoundaryType::Slab => {
            let construction_type = node
                .child("FoundationType")
                .and_then(|ft| ft.children.first())
                .map(|child| child.name.clone());
            (construction_type, None)
        }
        _ => (None, None),
    }
}

/// Extract insulation details string from nominal R-value layers.
fn extract_insulation_details(node: &XmlNode) -> Option<String> {
    let r_layers = parse_nominal_r_layers(node);
    if r_layers.is_empty() {
        return None;
    }
    // Sum nominal R-values, format as "R-{value}" (imperial, matching OCHRE convention).
    let total_r: f64 = r_layers.iter().sum();
    // Convert from m²·K/W back to imperial R for LUT matching.
    let imperial_r = total_r / conv::r_value_ip_to_si(1.0);
    if imperial_r < 1.0 {
        Some("Uninsulated".to_string())
    } else {
        Some(format!("R-{}", imperial_r.round() as i32))
    }
}

fn parse_material_layers(node: &XmlNode, area_m2: f64) -> Vec<MaterialLayer> {
    let mut layers = Vec::new();
    let mut layer_nodes = Vec::new();
    node.descendants("Layer", &mut layer_nodes);

    for layer in layer_nodes {
        let thickness_m = parse_value_with_units(layer.child("Thickness"), ValueKind::Length);
        let conductivity_w_m_k =
            parse_value_with_units(layer.child("Conductivity"), ValueKind::Conductivity);
        let density_kg_m3 = parse_value_with_units(layer.child("Density"), ValueKind::Density);
        let specific_heat_j_kg_k =
            parse_value_with_units(layer.child("SpecificHeat"), ValueKind::SpecificHeat);
        let nominal_r_m2_k_w =
            parse_value_with_units(layer.child("NominalRValue"), ValueKind::RValue);

        let conductivity = match (conductivity_w_m_k, thickness_m, nominal_r_m2_k_w) {
            (Some(k), _, _) => Some(k),
            (None, Some(thickness), Some(r)) if r > 0.0 => Some(thickness / r),
            _ => None,
        };

        let Some(thickness_m) = thickness_m else {
            continue;
        };
        let Some(conductivity_w_m_k) = conductivity else {
            continue;
        };

        layers.push(MaterialLayer {
            thickness_m,
            conductivity_w_m_k,
            density_kg_m3: density_kg_m3.unwrap_or(0.0),
            specific_heat_j_kg_k: specific_heat_j_kg_k.unwrap_or(0.0),
            area_m2,
        });
    }

    layers
}

/// Parse `<VentilationRate>` from an attic or foundation HPXML node.
///
/// Returns `(ventilation_ach, ventilation_sla)` based on the `UnitofMeasure` attribute.
fn parse_ventilation_rate(node: &XmlNode) -> (Option<f64>, Option<f64>) {
    let Some(vr) = node.child("VentilationRate") else {
        return (None, None);
    };
    let value = vr.text_as_f64();
    let unit = vr
        .attrs
        .get("UnitofMeasure")
        .or_else(|| vr.attrs.get("unitofmeasure"))
        .map(|s| normalize_ascii(s));
    match (value, unit.as_deref()) {
        (Some(v), Some("achnatural")) => (Some(v), None),
        (Some(v), Some("sla")) => (None, Some(v)),
        _ => {
            // If there's a Value child element, try that path too
            let value_node = vr.child("Value").and_then(|n| n.text_as_f64());
            let unit_node = vr
                .child("UnitofMeasure")
                .map(|n| normalize_ascii(&n.text));
            match (value_node, unit_node.as_deref()) {
                (Some(v), Some("achnatural")) => (Some(v), None),
                (Some(v), Some("sla")) => (None, Some(v)),
                _ => (None, None),
            }
        }
    }
}

fn build_zone_map(
    details: &XmlNode,
    conditioned_floor_area_m2: Option<f64>,
) -> HashMap<String, Zone> {
    let mut zones: HashMap<String, Zone> = HashMap::new();

    zones.insert(
        "conditioned".to_string(),
        Zone {
            zone_type: ZoneType::Conditioned,
            floor_area_m2: conditioned_floor_area_m2,
            volume_m3: None,
            attached_wall_ids: Vec::new(),
            duct_systems: Vec::new(),
            vented: false,
            ventilation_ach: None,
            ventilation_sla: None,
        },
    );

    zones.insert(
        "outdoor".to_string(),
        Zone {
            zone_type: ZoneType::Outdoor,
            floor_area_m2: None,
            volume_m3: None,
            attached_wall_ids: Vec::new(),
            duct_systems: Vec::new(),
            vented: false,
            ventilation_ach: None,
            ventilation_sla: None,
        },
    );

    if let Some(enclosure) = details.child("Enclosure") {
        // Attics
        if let Some(group) = enclosure.child("Attics") {
            for node in group.children_named("Attic") {
                let floor_area_m2 =
                    parse_value_with_units(node.child("FloorArea"), ValueKind::Area);

                // Parse vented status from <AtticType><Attic><Vented>
                let vented = node
                    .child("AtticType")
                    .and_then(|at| at.children.first())
                    .and_then(|child| child.child("Vented"))
                    .map(|v| v.text.trim().eq_ignore_ascii_case("true"))
                    .unwrap_or(true); // default vented for attics

                let (ventilation_ach, ventilation_sla) = parse_ventilation_rate(node);

                zones.entry("attic".to_string()).or_insert(Zone {
                    zone_type: ZoneType::Attic,
                    floor_area_m2,
                    volume_m3: None,
                    attached_wall_ids: Vec::new(),
                    duct_systems: Vec::new(),
                    vented,
                    ventilation_ach,
                    ventilation_sla,
                });
            }
        }

        // Garages
        if let Some(group) = enclosure.child("Garages") {
            for node in group.children_named("Garage") {
                let floor_area_m2 =
                    parse_value_with_units(node.child("FloorArea"), ValueKind::Area);
                zones.entry("garage".to_string()).or_insert(Zone {
                    zone_type: ZoneType::Garage,
                    floor_area_m2,
                    volume_m3: None,
                    attached_wall_ids: Vec::new(),
                    duct_systems: Vec::new(),
                    vented: false,
                    ventilation_ach: None,
                    ventilation_sla: None,
                });
            }
        }

        // Foundations
        if let Some(group) = enclosure.child("Foundations") {
            for node in group.children_named("Foundation") {
                let floor_area_m2 =
                    parse_value_with_units(node.child("FloorArea"), ValueKind::Area);

                // Determine vented status from <FoundationType> child tag name
                let vented = node
                    .child("FoundationType")
                    .and_then(|ft| ft.children.first())
                    .map(|child| {
                        let name = child.name.to_ascii_lowercase();
                        name.contains("vented") && !name.contains("unvented")
                    })
                    .unwrap_or(false);

                let (ventilation_ach, ventilation_sla) = parse_ventilation_rate(node);

                zones.entry("foundation".to_string()).or_insert(Zone {
                    zone_type: ZoneType::Foundation,
                    floor_area_m2,
                    volume_m3: None,
                    attached_wall_ids: Vec::new(),
                    duct_systems: Vec::new(),
                    vented,
                    ventilation_ach,
                    ventilation_sla,
                });
            }
        }
    }

    zones
}

fn assign_walls_to_zones(boundaries: &[Boundary], zones: &mut HashMap<String, Zone>) {
    for boundary in boundaries {
        if !matches!(
            boundary.boundary_type,
            BoundaryType::Wall | BoundaryType::FoundationWall
        ) {
            continue;
        }
        if let Some(interior_zone) = &boundary.interior_zone {
            let key = zone_key(interior_zone);
            if let Some(zone) = zones.get_mut(&key)
                && !zone.attached_wall_ids.iter().any(|id| id == &boundary.id)
            {
                zone.attached_wall_ids.push(boundary.id.clone());
            }
        }
        if let Some(exterior_zone) = &boundary.exterior_zone {
            let key = zone_key(exterior_zone);
            if let Some(zone) = zones.get_mut(&key)
                && !zone.attached_wall_ids.iter().any(|id| id == &boundary.id)
            {
                zone.attached_wall_ids.push(boundary.id.clone());
            }
        }
    }
}

fn parse_duct_systems(details: &XmlNode, zones: &mut HashMap<String, Zone>) {
    let mut ducts = Vec::new();
    details.descendants("DuctSystem", &mut ducts);

    for duct_node in ducts {
        let id = element_id(duct_node).unwrap_or_else(|| "unknown".to_string());
        let leakage_fraction = duct_node
            .first_descendant("LeakageFraction")
            .and_then(XmlNode::text_as_f64)
            .or_else(|| {
                duct_node
                    .first_descendant("DuctLeakage")
                    .and_then(XmlNode::text_as_f64)
            })
            .or_else(|| {
                duct_node
                    .first_descendant("AnnualDuctLeakageValue")
                    .and_then(XmlNode::text_as_f64)
            });

        let insulation_r_value_m2_k_w = duct_node
            .first_descendant("DuctInsulationRValue")
            .and_then(|n| parse_value_with_units(Some(n), ValueKind::RValue))
            .or_else(|| {
                duct_node
                    .first_descendant("InsulationRValue")
                    .and_then(|n| parse_value_with_units(Some(n), ValueKind::RValue))
            });

        let surface_area_m2 = duct_node
            .first_descendant("DuctSurfaceArea")
            .and_then(|n| parse_value_with_units(Some(n), ValueKind::Area));

        let location_text = duct_node
            .first_descendant("DuctLocation")
            .map(|n| n.text.trim().to_string())
            .or_else(|| {
                duct_node
                    .first_descendant("Location")
                    .map(|n| n.text.trim().to_string())
            })
            .unwrap_or_else(|| "outside".to_string());

        let location = parse_duct_location(&location_text);

        let duct_type = duct_node
            .first_descendant("DuctType")
            .map(|n| match normalize_ascii(&n.text).as_str() {
                "supply" => DuctType::Supply,
                "return" => DuctType::Return,
                _ => DuctType::Unknown,
            })
            .unwrap_or(DuctType::Unknown);

        let zone_key = if location_text.to_ascii_lowercase().contains("condition") {
            "conditioned"
        } else if location_text.to_ascii_lowercase().contains("attic") {
            "attic"
        } else if location_text.to_ascii_lowercase().contains("garage") {
            "garage"
        } else if location_text.to_ascii_lowercase().contains("foundation")
            || location_text.to_ascii_lowercase().contains("basement")
            || location_text.to_ascii_lowercase().contains("crawl")
        {
            "foundation"
        } else {
            "outdoor"
        }
        .to_string();

        if let Some(zone) = zones.get_mut(&zone_key) {
            zone.duct_systems.push(DuctSystem {
                id,
                leakage_fraction,
                insulation_r_value_m2_k_w,
                surface_area_m2,
                location,
                duct_type,
            });
        }
    }
}

fn parse_nominal_r_layers(node: &XmlNode) -> Vec<f64> {
    let mut layer_nodes = Vec::new();
    node.descendants("NominalRValue", &mut layer_nodes);
    layer_nodes
        .iter()
        .filter_map(|layer| parse_value_with_units(Some(layer), ValueKind::RValue))
        .collect()
}

fn extract_first_f64(root: &XmlNode, path: &[&str], kind: ValueKind) -> Option<f64> {
    root.path(path)
        .and_then(|node| parse_value_with_units(Some(node), kind))
}

fn find_descendant_f64(root: &XmlNode, name: &str, kind: ValueKind) -> Option<f64> {
    root.first_descendant(name)
        .and_then(|node| parse_value_with_units(Some(node), kind))
}

#[derive(Clone, Copy)]
enum ValueKind {
    Raw,
    Area,
    UValue,
    RValue,
    Conductivity,
    Length,
    Density,
    SpecificHeat,
    Temperature,
    Volume,
}

fn parse_value_with_units(node: Option<&XmlNode>, kind: ValueKind) -> Option<f64> {
    let node = node?;
    let value = node.text_as_f64()?;
    let units = node
        .attrs
        .get("units")
        .or_else(|| node.attrs.get("unit"))
        .map(|s| normalize_ascii(s));

    match kind {
        ValueKind::Raw => Some(value),
        ValueKind::Area => Some(convert_area_to_m2(value, units.as_deref())),
        ValueKind::UValue => Some(convert_u_to_w_m2_k(value, units.as_deref())),
        ValueKind::RValue => Some(convert_r_to_m2_k_w(value, units.as_deref())),
        ValueKind::Conductivity => Some(convert_conductivity_to_w_m_k(value, units.as_deref())),
        ValueKind::Length => Some(convert_length_to_m(value, units.as_deref())),
        ValueKind::Density => Some(convert_density_to_kg_m3(value, units.as_deref())),
        ValueKind::SpecificHeat => Some(convert_specific_heat_to_j_kg_k(value, units.as_deref())),
        ValueKind::Temperature => Some(convert_temperature_to_c(value, units.as_deref())),
        ValueKind::Volume => Some(convert_volume_to_m3(value, units.as_deref())),
    }
}

fn convert_area_to_m2(value: f64, units: Option<&str>) -> f64 {
    match units {
        Some("ft2") | Some("ft^2") | Some("ftsq") | Some("ftsq.") | Some("square feet") => {
            conv::area_ft2_to_m2(value)
        }
        Some(_) => value,
        None => {
            tracing::debug!(value, "Area value has no units attribute; assuming ft² and converting to m²");
            conv::area_ft2_to_m2(value)
        }
    }
}

fn convert_volume_to_m3(value: f64, units: Option<&str>) -> f64 {
    match units {
        Some("ft3") | Some("ft^3") | Some("cubic feet") => {
            conv::volume_ft3_to_m3(value)
        }
        Some(_) => value,
        None => {
            tracing::warn!(value, "Volume value has no units attribute; assuming ft³ and converting to m³");
            conv::volume_ft3_to_m3(value)
        }
    }
}

fn convert_u_to_w_m2_k(value: f64, units: Option<&str>) -> f64 {
    match units {
        Some("btu/hr-ft2-f") | Some("btu/hr-ft^2-f") | Some("btu/(h*ft2*f)") => {
            conv::u_value_ip_to_si(value)
        }
        Some(_) => value,
        None => {
            tracing::debug!(value, "U-value has no units attribute; assuming BTU/(hr*ft2*F) and converting to W/(m2*K)");
            conv::u_value_ip_to_si(value)
        }
    }
}

fn convert_r_to_m2_k_w(value: f64, units: Option<&str>) -> f64 {
    match units {
        Some("hr-ft2-f/btu") | Some("hr-ft^2-f/btu") | Some("h*ft2*f/btu") => {
            conv::r_value_ip_to_si(value)
        }
        Some(_) => value,
        None => {
            tracing::debug!(value, "R-value has no units attribute; assuming hr*ft2*F/BTU and converting to m2*K/W");
            conv::r_value_ip_to_si(value)
        }
    }
}

fn convert_conductivity_to_w_m_k(value: f64, units: Option<&str>) -> f64 {
    match units {
        Some("btu/hr-ft-f") | Some("btu/(h*ft*f)") => {
            conv::conductivity_btu_h_ft_f_to_w_m_k(value)
        }
        Some("btu-in/hr-ft2-f") | Some("btu in/hr ft2 f") | Some("btu*in/(h*ft2*f)") => {
            conv::conductivity_btu_in_h_ft2_f_to_w_m_k(value)
        }
        _ => value,
    }
}

fn convert_length_to_m(value: f64, units: Option<&str>) -> f64 {
    match units {
        Some("in") | Some("inch") | Some("inches") => conv::length_in_to_m(value),
        Some("ft") | Some("feet") => conv::length_ft_to_m(value),
        Some(_) => value,
        None => {
            tracing::debug!(value, "Length value has no units attribute; assuming feet and converting to meters");
            conv::length_ft_to_m(value)
        }
    }
}

fn convert_density_to_kg_m3(value: f64, units: Option<&str>) -> f64 {
    match units {
        Some("lb/ft3") | Some("lb/ft^3") | Some("lbm/ft3") => {
            conv::density_lb_ft3_to_kg_m3(value)
        }
        _ => value,
    }
}

fn convert_specific_heat_to_j_kg_k(value: f64, units: Option<&str>) -> f64 {
    match units {
        Some("btu/lb-f") | Some("btu/(lb*f)") => conv::specific_heat_btu_lb_f_to_j_kg_k(value),
        _ => value,
    }
}

fn convert_temperature_to_c(value: f64, units: Option<&str>) -> f64 {
    match units {
        Some("F") | Some("f") | Some("degF") | Some("degf") | Some("fahrenheit") => {
            conv::temperature_f_to_c(value)
        }
        Some("C") | Some("c") | Some("degC") | Some("degc") | Some("celsius") => value,
        Some(_) => value,
        None => {
            tracing::debug!(value, "Temperature value has no units attribute; assuming F and converting to C");
            conv::temperature_f_to_c(value)
        }
    }
}

fn parse_site_type(text: &str) -> SiteType {
    match normalize_ascii(text).as_str() {
        "rural" => SiteType::Rural,
        "suburban" => SiteType::Suburban,
        "urban" => SiteType::Urban,
        other => SiteType::Other(other.to_string()),
    }
}

fn parse_zone_ref(node: Option<&XmlNode>) -> Option<ZoneType> {
    let node = node?;
    Some(parse_zone_label(node.text.trim()))
}

fn parse_zone_label(text: &str) -> ZoneType {
    let norm = normalize_ascii(text);
    if norm.contains("condition") || norm == "living space" {
        ZoneType::Conditioned
    } else if norm.contains("attic") {
        ZoneType::Attic
    } else if norm.contains("garage") {
        ZoneType::Garage
    } else if norm.contains("foundation")
        || norm.contains("basement")
        || norm.contains("crawl")
    {
        ZoneType::Foundation
    } else if norm.contains("out") || norm.contains("ambient") || norm == "ground"
    {
        ZoneType::Outdoor
    } else {
        ZoneType::Other(text.trim().to_string())
    }
}

fn parse_duct_location(text: &str) -> DuctLocation {
    let norm = normalize_ascii(text);
    if norm.contains("condition") {
        DuctLocation::InsideConditionedSpace
    } else if norm.contains("out")
        || norm.contains("attic")
        || norm.contains("garage")
        || norm.contains("crawl")
        || norm.contains("basement")
    {
        DuctLocation::OutsideConditionedSpace
    } else {
        DuctLocation::Other(text.trim().to_string())
    }
}

fn zone_key(zone_type: &ZoneType) -> String {
    match zone_type {
        ZoneType::Conditioned => "conditioned".to_string(),
        ZoneType::Attic => "attic".to_string(),
        ZoneType::Garage => "garage".to_string(),
        ZoneType::Foundation => "foundation".to_string(),
        ZoneType::Outdoor => "outdoor".to_string(),
        ZoneType::Other(label) => label.to_ascii_lowercase(),
    }
}

/// Compute attic volume from gable wall areas and roof pitch.
///
/// Treats the attic as a triangular prism:
///   V = 0.5 × floor_area × height
/// where height = √(gable_area × tan(roof_tilt_rad)).
///
/// The gable area is the largest attic-facing wall, and roof tilt comes from
/// parsed `<Pitch>` on Roof boundaries adjacent to the attic.
///
/// Ref: OCHRE `hpxml.py` `parse_hpxml_zones()` lines 584–631.
fn compute_attic_volume(boundaries: &[Boundary], attic_floor_area_m2: Option<f64>) -> Option<f64> {
    // Attic floor area: prefer explicit zone value, fall back to the Floor boundary
    // between conditioned space and attic (OCHRE calls this "Attic Floor").
    let floor_area = attic_floor_area_m2
        .or_else(|| {
            boundaries
                .iter()
                .find(|b| {
                    b.boundary_type == BoundaryType::Floor
                        && ((b.interior_zone.as_ref() == Some(&ZoneType::Conditioned)
                            && b.exterior_zone.as_ref() == Some(&ZoneType::Attic))
                            || (b.interior_zone.as_ref() == Some(&ZoneType::Attic)
                                && b.exterior_zone.as_ref() == Some(&ZoneType::Conditioned)))
                })
                .map(|b| b.area_m2)
        })
        .filter(|&a| a > 0.0)?;

    // Find attic gable walls: Wall boundaries with interior=Attic, exterior=Outdoor.
    let gable_areas: Vec<f64> = boundaries
        .iter()
        .filter(|b| {
            b.boundary_type == BoundaryType::Wall
                && b.interior_zone.as_ref() == Some(&ZoneType::Attic)
                && matches!(
                    b.exterior_zone.as_ref(),
                    Some(&ZoneType::Outdoor) | None
                )
        })
        .map(|b| b.area_m2)
        .collect();

    // Find roof tilt from Roof boundaries facing the attic.
    let roof_tilt_deg: Option<f64> = boundaries
        .iter()
        .filter(|b| {
            b.boundary_type == BoundaryType::Roof
                && b.interior_zone.as_ref() == Some(&ZoneType::Attic)
        })
        .find_map(|b| b.tilt_deg);

    let tilt_rad = roof_tilt_deg?.to_radians();
    if tilt_rad <= 0.0 {
        return None;
    }

    // Select gable area following OCHRE's convention:
    // - Standard 2-gable roof: both gables should be equal (within 0.2 m²), use first.
    // - 3-gable (garage-combined): use median (index 1 after sorting).
    // Ref: OCHRE hpxml.py parse_hpxml_zones() lines 599–611.
    let gable_area = match gable_areas.len() {
        0 => return None,
        1 => gable_areas[0],
        2 => gable_areas[0], // standard symmetric gable
        _ => {
            // 3+ gables: sort and take median (index 1)
            let mut sorted = gable_areas;
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            sorted[1]
        }
    };
    if gable_area <= 0.0 {
        return None;
    }

    let attic_height = (gable_area * tilt_rad.tan()).sqrt();
    Some(0.5 * floor_area * attic_height)
}

fn zone_sort_key(zone_type: &ZoneType) -> u8 {
    match zone_type {
        ZoneType::Conditioned => 0,
        ZoneType::Attic => 1,
        ZoneType::Garage => 2,
        ZoneType::Foundation => 3,
        ZoneType::Outdoor => 4,
        ZoneType::Other(_) => 5,
    }
}

fn normalize_name(name: &str) -> String {
    if let Some((_, tail)) = name.rsplit_once(':') {
        return tail.to_string();
    }
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::{BoundaryType, HpxmlError, ZoneType, parse_building};

    const SAMPLE_XML: &str = r#"
<HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site>
          <Elevation units="ft">5280</Elevation>
          <SiteType>suburban</SiteType>
          <ShieldingOfHome>normal</ShieldingOfHome>
        </Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="ft2">2152</ConditionedFloorArea>
          <ConditionedBuildingVolume units="ft3">17216</ConditionedBuildingVolume>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <Walls>
          <Wall>
            <SystemIdentifier id="Wall1"/>
            <InteriorAdjacentTo>conditioned space</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <Area units="ft2">100</Area>
            <Azimuth>180</Azimuth>
            <Insulation>
              <Layer>
                <Thickness units="in">5.5</Thickness>
                <NominalRValue>19</NominalRValue>
                <Density units="lb/ft3">0.5</Density>
                <SpecificHeat units="Btu/lb-F">0.2</SpecificHeat>
              </Layer>
            </Insulation>
          </Wall>
        </Walls>
        <Roofs>
          <Roof>
            <SystemIdentifier id="Roof1"/>
            <InteriorAdjacentTo>attic vented</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <Area units="ft2">120</Area>
          </Roof>
        </Roofs>
        <FoundationWalls>
          <FoundationWall>
            <SystemIdentifier id="FoundationWall1"/>
            <InteriorAdjacentTo>basement - conditioned</InteriorAdjacentTo>
            <ExteriorAdjacentTo>ground</ExteriorAdjacentTo>
            <Area units="ft2">60</Area>
          </FoundationWall>
        </FoundationWalls>
        <Slabs>
          <Slab>
            <SystemIdentifier id="Slab1"/>
            <InteriorAdjacentTo>basement - conditioned</InteriorAdjacentTo>
            <ExteriorAdjacentTo>ground</ExteriorAdjacentTo>
            <Area units="ft2">80</Area>
          </Slab>
        </Slabs>
        <Windows>
          <Window>
            <SystemIdentifier id="Window1"/>
            <InteriorAdjacentTo>conditioned space</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <Area units="ft2">15</Area>
            <Azimuth>180</Azimuth>
            <UFactor>0.31</UFactor>
            <SHGC>0.25</SHGC>
            <FrameType>vinyl</FrameType>
            <AttachedToWall idref="Wall1"/>
          </Window>
        </Windows>
        <Attics>
          <Attic>
            <FloorArea units="ft2">500</FloorArea>
          </Attic>
        </Attics>
        <Garages>
          <Garage>
            <FloorArea units="ft2">400</FloorArea>
          </Garage>
        </Garages>
        <Foundations>
          <Foundation>
            <FloorArea units="ft2">800</FloorArea>
          </Foundation>
        </Foundations>
      </Enclosure>
      <Systems>
        <HVAC>
          <HVACDistribution>
            <DuctSystem>
              <SystemIdentifier id="Duct1"/>
              <LeakageFraction>0.12</LeakageFraction>
              <DuctInsulationRValue>8</DuctInsulationRValue>
              <DuctLocation>attic vented</DuctLocation>
            </DuctSystem>
          </HVACDistribution>
          <HeatingSystem>
            <HeatingCapacity>60</HeatingCapacity>
          </HeatingSystem>
          <HeatPump>
            <SEER2>17</SEER2>
            <HSPF2>9</HSPF2>
          </HeatPump>
        </HVAC>
        <WaterHeating>
          <WaterHeatingSystem>
            <HotWaterTemperature units="F">120</HotWaterTemperature>
          </WaterHeatingSystem>
        </WaterHeating>
      </Systems>
      <Generation>
        <Battery>
          <RoundTripEfficiency>0.9</RoundTripEfficiency>
        </Battery>
        <PVSystem>
          <Tilt>35</Tilt>
        </PVSystem>
      </Generation>
      <AirInfiltration>
        <AirInfiltrationMeasurement>
          <AirLeakage>5.0</AirLeakage>
        </AirInfiltrationMeasurement>
      </AirInfiltration>
    </BuildingDetails>
  </Building>
</HPXML>
"#;

    #[test]
    fn parses_zones_boundaries_windows_and_ducts() {
        let building = parse_building(SAMPLE_XML).expect("expected parser success");

        assert_eq!(building.zones.len(), 5);
        assert!(
            building
                .zones
                .iter()
                .any(|z| matches!(z.zone_type, ZoneType::Conditioned))
        );

        let wall = building
            .boundaries
            .iter()
            .find(|b| matches!(b.boundary_type, BoundaryType::Wall))
            .expect("wall boundary expected");
        assert!((wall.area_m2 - 9.290_304).abs() < 1.0e-6);

        let layer = wall
            .material_layers
            .first()
            .expect("material layer expected");
        assert!((layer.conductivity_w_m_k - 0.040_02).abs() < 0.002);

        let attic = building
            .zones
            .iter()
            .find(|z| matches!(z.zone_type, ZoneType::Attic))
            .expect("attic zone expected");
        assert_eq!(attic.duct_systems.len(), 1);
        assert!(
            (attic.duct_systems[0]
                .insulation_r_value_m2_k_w
                .unwrap_or_default()
                - 1.4088)
                .abs()
                < 1e-4
        );

        assert_eq!(building.windows.len(), 1);
        assert!((building.windows[0].area_m2 - 1.393_545_6).abs() < 1e-6);
        assert!(building.windows[0].area_m2 > 0.0);
        assert!((building.windows[0].u_factor_w_m2_k.unwrap_or_default() - 1.760_18).abs() < 1e-3);

        // Volume: 17216 ft³ × 0.028316846592 ≈ 487.49 m³
        let expected_volume_m3 = 17_216.0 * 0.028_316_846_592;
        assert!(
            (building.conditioned_volume_m3.unwrap() - expected_volume_m3).abs() < 0.01,
            "conditioned_volume_m3: got {}, expected {}",
            building.conditioned_volume_m3.unwrap(),
            expected_volume_m3,
        );

        // Ceiling height: volume / floor_area
        let expected_floor_area_m2 = 2152.0 * 0.092_903_04;
        let expected_ceiling_height = expected_volume_m3 / expected_floor_area_m2;
        assert!(
            (building.ceiling_height_m.unwrap() - expected_ceiling_height).abs() < 1e-6,
        );

        // Conditioned zone should have volume derived from ceiling height × floor area
        let conditioned = building
            .zones
            .iter()
            .find(|z| matches!(z.zone_type, ZoneType::Conditioned))
            .expect("conditioned zone");
        assert!(conditioned.volume_m3.is_some());

        // Non-conditioned zones should have no volume assigned
        let attic_vol = building
            .zones
            .iter()
            .find(|z| matches!(z.zone_type, ZoneType::Attic))
            .expect("attic zone");
        assert!(attic_vol.volume_m3.is_none());
    }

    #[test]
    fn window_missing_area_returns_error() {
        let xml = SAMPLE_XML.replace("<Area units=\"ft2\">15</Area>", "");
        let err = parse_building(&xml).expect_err("expected missing window area failure");
        assert!(matches!(err, HpxmlError::Parse(_)));
        let msg = err.to_string();
        assert!(msg.contains("window 'Window1' is missing required Area element"));
    }

    #[test]
    fn window_zero_area_returns_error() {
        let xml = SAMPLE_XML.replace(
            "<Area units=\"ft2\">15</Area>",
            "<Area units=\"ft2\">0</Area>",
        );
        let err = parse_building(&xml).expect_err("expected zero window area failure");
        assert!(matches!(err, HpxmlError::Parse(_)));
        let msg = err.to_string();
        assert!(msg.contains("window 'Window1' has non-positive area: 0"));
    }

    #[test]
    fn wall_missing_area_returns_error() {
        let xml = SAMPLE_XML.replace("<Area units=\"ft2\">100</Area>", "");
        let err = parse_building(&xml).expect_err("expected missing wall area failure");
        assert!(matches!(err, HpxmlError::Parse(_)));
        let msg = err.to_string();
        assert!(msg.contains("wall 'Wall1' is missing required Area element"));
    }

    /// FrameFloor elements (pier-and-beam / raised-floor homes) must parse as
    /// `BoundaryType::Floor`, matching OCHRE's treatment of FrameFloors.
    #[test]
    fn frame_floor_raised_floor_parses_as_floor() {
        let xml = SAMPLE_XML.replace(
            "</Enclosure>",
            r#"<FrameFloors>
              <FrameFloor>
                <SystemIdentifier id="FrameFloor1"/>
                <InteriorAdjacentTo>conditioned space</InteriorAdjacentTo>
                <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
                <Area units="ft2">200</Area>
                <Insulation>
                  <Layer>
                    <NominalRValue>19</NominalRValue>
                  </Layer>
                </Insulation>
              </FrameFloor>
            </FrameFloors>
            </Enclosure>"#,
        );
        let building = parse_building(&xml).expect("should parse FrameFloor XML");
        let ff = building
            .boundaries
            .iter()
            .find(|b| b.id == "FrameFloor1")
            .expect("FrameFloor1 boundary should exist");

        assert!(matches!(ff.boundary_type, BoundaryType::Floor));
        // 200 ft² → 18.580608 m²
        assert!((ff.area_m2 - 18.580_608).abs() < 1e-4);
        // R-19 (IP) → 3.3450 m²·K/W
        assert!(!ff.r_value_layers_m2_k_w.is_empty());
        let total_r: f64 = ff.r_value_layers_m2_k_w.iter().sum();
        assert!((total_r - 3.345).abs() < 0.01, "R-value: got {total_r}");
    }

    /// FrameFloor over a crawlspace (ExteriorAdjacentTo = crawlspace) parses
    /// correctly as a Floor boundary with foundation exterior.
    #[test]
    fn frame_floor_crawlspace_ceiling_parses_as_floor() {
        let xml = SAMPLE_XML.replace(
            "</Enclosure>",
            r#"<FrameFloors>
              <FrameFloor>
                <SystemIdentifier id="FrameFloor2"/>
                <InteriorAdjacentTo>conditioned space</InteriorAdjacentTo>
                <ExteriorAdjacentTo>crawlspace - vented</ExteriorAdjacentTo>
                <Area units="ft2">150</Area>
                <Insulation>
                  <Layer>
                    <NominalRValue>30</NominalRValue>
                  </Layer>
                </Insulation>
              </FrameFloor>
            </FrameFloors>
            </Enclosure>"#,
        );
        let building = parse_building(&xml).expect("should parse crawlspace FrameFloor");
        let ff = building
            .boundaries
            .iter()
            .find(|b| b.id == "FrameFloor2")
            .expect("FrameFloor2 boundary should exist");

        assert!(matches!(ff.boundary_type, BoundaryType::Floor));
        // 150 ft² → 13.935456 m²
        assert!((ff.area_m2 - 13.935_456).abs() < 1e-4);
        // R-30 (IP) → 5.2834 m²·K/W
        assert!(!ff.r_value_layers_m2_k_w.is_empty());
        let total_r: f64 = ff.r_value_layers_m2_k_w.iter().sum();
        assert!((total_r - 5.283).abs() < 0.01, "R-value: got {total_r}");

    }

    fn xml_with_window(window_xml: &str) -> String {
        format!(
            r#"
<HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <Walls />
        <Windows>
          {window_xml}
        </Windows>
      </Enclosure>
    </BuildingDetails>
  </Building>
</HPXML>
"#
        )
    }

    #[test]
    fn window_no_interior_shading_fraction_is_1() {
        let xml = xml_with_window(
            r#"<Window>
                <SystemIdentifier id="W1"/>
                <Area>10</Area>
                <SHGC>0.40</SHGC>
            </Window>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        assert_eq!(building.windows.len(), 1);
        let w = &building.windows[0];
        assert!((w.interior_shading_fraction - 1.0).abs() < f64::EPSILON);
        // effective SHGC unmodified: 0.40 * 1.0 = 0.40
        let effective = w.shgc.unwrap() * w.interior_shading_fraction;
        assert!((effective - 0.40).abs() < f64::EPSILON);
    }

    #[test]
    fn window_with_summer_shading_coefficient() {
        let xml = xml_with_window(
            r#"<Window>
                <SystemIdentifier id="W1"/>
                <Area>10</Area>
                <SHGC>0.40</SHGC>
                <InteriorShading>
                    <SystemIdentifier id="W1Shade"/>
                    <SummerShadingCoefficient>0.70</SummerShadingCoefficient>
                    <WinterShadingCoefficient>0.85</WinterShadingCoefficient>
                </InteriorShading>
            </Window>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let w = &building.windows[0];
        assert!((w.interior_shading_fraction - 0.70).abs() < f64::EPSILON);
        // effective SHGC: 0.40 * 0.70 = 0.28
        let effective = w.shgc.unwrap() * w.interior_shading_fraction;
        assert!((effective - 0.28).abs() < 1e-10);
    }

    #[test]
    fn window_interior_shading_without_summer_coefficient_defaults_070() {
        let xml = xml_with_window(
            r#"<Window>
                <SystemIdentifier id="W1"/>
                <Area>10</Area>
                <SHGC>0.40</SHGC>
                <InteriorShading>
                    <SystemIdentifier id="W1Shade"/>
                </InteriorShading>
            </Window>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let w = &building.windows[0];
        assert!(
            (w.interior_shading_fraction - 0.70).abs() < f64::EPSILON,
            "should default to 0.70 per RESNET 301, got {}",
            w.interior_shading_fraction,
        );
        // effective SHGC: 0.40 * 0.70 = 0.28
        let effective = w.shgc.unwrap() * w.interior_shading_fraction;
        assert!((effective - 0.28).abs() < 1e-10);
    }
}
