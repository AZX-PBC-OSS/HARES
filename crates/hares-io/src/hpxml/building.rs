//! HPXML building envelope and geometry parsing.

use std::collections::HashMap;

use quick_xml::Reader;
use quick_xml::events::Event;

use hares_types::{normalize_ascii, parse_trimmed_f64};

use hares_physics::infiltration::{NATURAL_TO_50PA_EXPONENT, ach_nat_to_ach50};
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
    /// HPXML `<ShieldingOfHome>` -- string value ("normal", "exposed", "well-shielded").
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
    /// Earth/soil boundary condition (not a thermal zone).
    Ground,
    /// Adiabatic boundary to another dwelling unit (multifamily).
    /// OCHRE: "other housing unit", "other heated space", etc. → same-zone thermal mass.
    Adjacent,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloorOrCeiling {
    Floor,
    Ceiling,
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
    /// `None` means use the default: 0.70 (EnergyPlus Material IDD default)
    /// for both exterior and interior sides, 0.05 for attic radiant barriers.
    /// Valid range: 0.0–1.0.
    pub solar_absorptance: Option<f64>,
    /// Longwave emittance [-] from HPXML `<Emittance>`.
    ///
    /// `None` means use the default: 0.90 for most surfaces, 0.05 for attic
    /// radiant barriers.  Ref: OCHRE `Envelope.py:222`.
    /// Valid range: 0.0–1.0.
    pub emittance: Option<f64>,
    /// Override for LUT boundary name resolution. When set, `resolve_boundary_name`
    /// is bypassed and this name is used directly for LUT lookup. Used for
    /// auto-generated boundaries (interior walls, furniture) whose LUT name
    /// can't be derived from zone types alone.
    pub lut_boundary_name: Option<String>,
    /// HPXML `<FloorOrCeiling>` -- distinguishes adjacent floors from ceilings.
    pub floor_or_ceiling: Option<FloorOrCeiling>,
    /// Surface tilt angle [degrees].
    ///
    /// 0 = horizontal facing up (flat roof), 90 = vertical (wall),
    /// 180 = horizontal facing down (floor from above).
    /// For roofs, computed from `<Pitch>` as `atan(pitch / 12)` in degrees.
    /// Ref: OCHRE `hpxml.py` `pitch2deg()`.
    pub tilt_deg: Option<f64>,
    /// Framing factor [-] -- fraction of wall area occupied by structural framing.
    ///
    /// Used by the ASHRAE parallel-path method to compute effective R-value.
    /// Typical values: 0.23 for 2x4 @ 16" OC, 0.22 for 2x6 @ 16" OC.
    /// `None` means no framing correction (insulation R-value used uniformly).
    pub framing_factor: Option<f64>,
    /// Exposed perimeter length [m] for slab-on-grade boundaries.
    ///
    /// Parsed from HPXML `<Slab>/<ExposedPerimeter>` (HPXML 4.x) or
    /// `<Slab>/<Perimeter>` (HPXML 3.x), in feet; converted to meters via
    /// `ValueKind::Length`. When absent, derived from `4 × sqrt(area_m2)` as a
    /// square-plan approximation. Required for the ASHRAE F-factor perimeter
    /// heat loss method.
    pub perimeter_m: Option<f64>,
    /// Perimeter insulation nominal R-value [m²·K/W] (SI) for slab boundaries.
    ///
    /// Parsed from HPXML `<Slab>/<PerimeterInsulation>/<Layer>/<NominalRValue>`
    /// (converted from IP ft²·°F·h/Btu). Used to select the ASHRAE F2 perimeter
    /// heat loss coefficient via [`hares_physics::ground::f2_coefficient`].
    pub perimeter_insulation_r_m2_k_w: Option<f64>,
    /// Foundation depth below grade [m] for ground temperature calculations.
    ///
    /// For foundation walls: the centroid depth of the below-grade portion
    /// (typically `DepthBelowGrade / 2.0`). For slabs: the depth below grade
    /// of the adjacent foundation wall (i.e., `DepthBelowGrade`).
    ///
    /// This depth is used by the Kusuda-Achenbach ground temperature model
    /// in the thermal solver for `DrivingTemp::Ground` boundaries.
    /// `None` for above-grade boundaries. Populated during HPXML post-processing.
    pub foundation_depth_m: Option<f64>,
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
    /// Winter shading coefficient for seasonal SHGC adjustment.
    /// Per ANSI/RESNET/ICC 301: summer/winter shading may differ.
    pub winter_shading_fraction: f64,
    /// Fraction of window area that is operable (0.0–1.0).
    /// Used for natural ventilation flow calculation.
    pub fraction_operable: f64,
    /// Exterior shading transmittance multiplier for summer (0.0–1.0).
    /// 1.0 = unobstructed, 0.0 = fully shaded. Multiplied with interior shading.
    pub exterior_shading_summer: f64,
    /// Exterior shading transmittance multiplier for winter (0.0–1.0).
    pub exterior_shading_winter: f64,
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
    /// Raw leakage in CFM25 units (volumetric flow at 25 Pa).
    /// Converted to fraction in `resolve_hvac::compute_duct_config` once
    /// fan airflow is known — the parser cannot convert without fan flow.
    pub leakage_cfm25: Option<f64>,
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
    /// Blower-door result at 50 Pa in ACH (air changes per hour).
    pub infiltration_ach50: Option<f64>,
    /// Blower-door result at 50 Pa in CFM (cubic feet per minute).
    pub infiltration_cfm50: Option<f64>,
    /// Raw leakage at natural pressure (≈ 4 Pa) in ACH, before conversion to 50 Pa
    /// equivalent. Preserved for diagnostics; the converted value is stored in
    /// [`infiltration_ach50`](Self::infiltration_ach50).
    pub infiltration_ach_natural: Option<f64>,
    /// Raw leakage at natural pressure (≈ 4 Pa) in CFM, before conversion to 50 Pa
    /// equivalent. Preserved for diagnostics; the converted value is stored in
    /// [`infiltration_cfm50`](Self::infiltration_cfm50).
    pub infiltration_cfm_natural: Option<f64>,
    /// Effective Leakage Area converted to cm² from sq-in input.
    pub infiltration_ela_cm2: Option<f64>,
    /// Constant infiltration rate in ACH (bypasses AIM-2 wind/stack model).
    /// Used by BESTEST/ASHRAE 140 synthetic cases that specify a fixed ACH.
    pub infiltration_constant_ach: Option<f64>,
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
    /// Foundation type name for LUT matching (e.g. "Unfinished Basement", "Crawlspace").
    /// Derived from `<Foundation>/<FoundationType>` per OCHRE hpxml.py:276-286.
    pub foundation_name: Option<String>,
    /// Residential facility type from `<BuildingConstruction>/<ResidentialFacilityType>`.
    /// Used for adjusted bedroom count in water heater draw profiles.
    pub residential_facility_type: Option<String>,
    /// Override the zone mass multiplier for all zones.
    /// When set, replaces the default zone-type-based multiplier.
    pub mass_multiplier_override: Option<f64>,
    /// HVAC thermostat deadband/hysteresis in °C.
    /// When set, overrides the default 1.0°C hysteresis for IdealHVAC.
    /// BESTEST/ASHRAE 140 requires 0.0 (ideal setpoint tracking).
    pub hvac_deadband_c: Option<f64>,
    /// Raw HPXML parse tree retained for downstream consumers that still read
    /// fields which have not been promoted to typed members on `Building`.
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

    let total_conditioned_floors = summary
        .path(&["BuildingConstruction", "NumberofConditionedFloors"])
        .and_then(|node| parse_value_with_units(Some(node), ValueKind::Raw));
    let floors_above_grade = summary
        .path(&[
            "BuildingConstruction",
            "NumberofConditionedFloorsAboveGrade",
        ])
        .and_then(|node| parse_value_with_units(Some(node), ValueKind::Raw));

    let residential_facility_type = summary
        .path(&["BuildingConstruction", "ResidentialFacilityType"])
        .map(|n| n.text.trim().to_string())
        .filter(|s| !s.is_empty());

    // InfiltrationHeight lives under AirInfiltrationMeasurement -- HPXML stores it in feet.
    let infiltration_height_m = details
        .first_descendant("InfiltrationHeight")
        .and_then(|node| parse_value_with_units(Some(node), ValueKind::Length));

    // <EffectiveLeakageArea units="sq-in"> -- convert sq inches to cm² (1 in² = 6.4516 cm²).
    let infiltration_ela_cm2 = details
        .first_descendant("EffectiveLeakageArea")
        .and_then(|node| node.text_as_f64())
        .map(|sq_in| sq_in * 6.4516);

    // <AirLeakage units="CFM50"> under AirInfiltrationMeasurement, or
    // <BuildingAirLeakage><UnitofMeasure>CFM</UnitofMeasure>... (HPXML 3.x wrapper form).
    let (infiltration_cfm50, infiltration_cfm_natural) = parse_air_leakage_cfm50(details)?;

    // <extension><HasFlueOrChimneyInConditionedSpace> -- boolean text
    let has_flue_or_chimney = details
        .first_descendant("HasFlueOrChimneyInConditionedSpace")
        .map(|n| n.text.trim().eq_ignore_ascii_case("true"));

    // Foundation type name for LUT matching of foundation wall boundaries.
    // OCHRE hpxml.py:276-286: FoundationType child tag → "Crawlspace" | "Unfinished Basement" | "Finished Basement".
    let foundation_name = details
        .path(&["Enclosure", "Foundations", "Foundation", "FoundationType"])
        .and_then(|ft| ft.children.first())
        .and_then(|child| {
            let tag = child.name.as_str();
            match tag {
                "Crawlspace" => Some("Crawlspace".to_string()),
                "Basement" => {
                    // Prefer HPXML 4.x <Conditioned> element if present.
                    // Fall back to OCHRE heuristic: total_floors > floors_above_grade
                    // means basement is conditioned (finished).
                    let explicit = child
                        .child("Conditioned")
                        .map(|n| n.text.trim().eq_ignore_ascii_case("true"));
                    let inferred = match (total_conditioned_floors, floors_above_grade) {
                        (Some(total), Some(above)) => total > above,
                        _ => false,
                    };
                    let is_finished = explicit.unwrap_or(inferred);
                    if is_finished {
                        Some("Finished Basement".to_string())
                    } else {
                        Some("Unfinished Basement".to_string())
                    }
                }
                "SlabOnGrade" | "Ambient" | "AboveApartment" => None,
                other => {
                    tracing::warn!(
                        foundation_type = other,
                        "Unrecognized FoundationType child tag; ignoring foundation"
                    );
                    None
                }
            }
        });
    let foundation_floor_area_m2 = details
        .path(&["Enclosure", "Foundations"])
        .and_then(|group| group.children_named("Foundation").next())
        .and_then(|foundation| {
            parse_value_with_units(foundation.child("FloorArea"), ValueKind::Area)
        });

    let mut boundaries = parse_boundaries(details)?;
    let windows = parse_windows(details, &mut boundaries)?;

    // Subtract window and door areas from their attached walls.
    // OCHRE hpxml.py:118-126: ext_walls[wall]["Area"] -= boundary["Area"]
    {
        let mut wall_reductions: std::collections::HashMap<String, f64> =
            std::collections::HashMap::new();
        for win in &windows {
            if let Some(ref wall_id) = win.attached_to_wall_id {
                *wall_reductions.entry(wall_id.clone()).or_default() += win.area_m2;
            }
        }
        // Doors also have AttachedToWall in HPXML.
        if let Some(enclosure) = details.child("Enclosure") {
            if let Some(doors) = enclosure.child("Doors") {
                for door in doors.children_named("Door") {
                    if let Some(wall_id) = door
                        .child("AttachedToWall")
                        .and_then(|n| n.attrs.get("idref"))
                    {
                        let area = parse_value_with_units(door.child("Area"), ValueKind::Area)
                            .unwrap_or(0.0);
                        if area > 0.0 {
                            *wall_reductions.entry(wall_id.clone()).or_default() += area;
                        }
                    }
                }
            }
        }
        for bd in &mut boundaries {
            if let Some(&reduction) = wall_reductions.get(&bd.id) {
                let new_area = (bd.area_m2 - reduction).max(0.0);
                if new_area <= 0.0 {
                    tracing::warn!(
                        wall = %bd.id,
                        original = bd.area_m2,
                        reduction,
                        "wall area reduced to zero by window/door subtraction"
                    );
                }
                bd.area_m2 = new_area;
            }
        }
    }

    // Post-process foundation wall boundaries: override construction_type with
    // foundation_name, apply insulation details and area scaling.
    // OCHRE hpxml.py:408-410: boundaries["Foundation Wall"]["Construction Type"] = foundation_name
    let mut foundation_height_m: Option<f64> = None;
    let mut foundation_depth_m: Option<f64> = None;
    for bd in &mut boundaries {
        if bd.boundary_type == BoundaryType::FoundationWall {
            if let Some(ref fnd_name) = foundation_name {
                bd.construction_type = Some(fnd_name.clone());
            }
            let (insulation, area_scale, height_m, depth_below_grade) =
                extract_foundation_wall_insulation(details, &bd.id);
            bd.insulation_details = insulation;
            bd.area_m2 *= area_scale;
            if foundation_height_m.is_none() {
                foundation_height_m = height_m;
            }
            if foundation_depth_m.is_none() && depth_below_grade > 0.0 {
                foundation_depth_m = Some(depth_below_grade);
            }
            bd.foundation_depth_m = Some(depth_below_grade / 2.0);
        }
    }

    // Post-process slab boundaries: extract insulation details from PerimeterInsulation
    // and UnderSlabInsulation elements, and parse perimeter geometry for the ASHRAE
    // F-factor perimeter heat loss method.
    //
    // OCHRE envelope.py:462-485 for insulation details.
    // ASHRAE HoF 2021 Ch. 18.31 for F-factor perimeter method.
    if let Some(slabs_group) = details.path(&["Enclosure", "Slabs"]) {
        for bd in &mut boundaries {
            if bd.boundary_type == BoundaryType::Slab {
                // The slab interface is at the base of the foundation wall
                // (top of slab ≈ bottom of wall). Propagate from the first
                // foundation wall's `DepthBelowGrade`.
                if bd.foundation_depth_m.is_none() {
                    bd.foundation_depth_m = foundation_depth_m;
                }

                if let Some(slab_node) = slabs_group.children_named("Slab").find(|n| {
                    n.child("SystemIdentifier")
                        .and_then(|si| si.attrs.get("id"))
                        .map(|id| id == &bd.id)
                        .unwrap_or(false)
                }) {
                    bd.insulation_details = extract_slab_insulation(slab_node);

                    // Parse exposed perimeter length for F-factor method.
                    // HPXML 4.x <ExposedPerimeter> and 3.x <Perimeter>, in feet;
                    // convert to meters via ValueKind::Length.
                    bd.perimeter_m = slab_node
                        .child("ExposedPerimeter")
                        .or_else(|| slab_node.child("Perimeter"))
                        .and_then(|n| parse_value_with_units(Some(n), ValueKind::Length));

                    // Parse perimeter insulation R-value for F2 coefficient selection.
                    // HPXML NominalRValue is in IP ft²·°F·h/Btu; convert to SI m²·K/W
                    // via ValueKind::RValue.
                    bd.perimeter_insulation_r_m2_k_w = slab_node
                        .path(&["PerimeterInsulation", "Layer", "NominalRValue"])
                        .and_then(|n| parse_value_with_units(Some(n), ValueKind::RValue));
                }
            }
        }
    }

    // The conditioned zone should exclude below-grade foundation area when a basement
    // is present. OCHRE: indoor_floor_area = conditioned_floor_area - first_floor_area * below_grade_floors.
    // If foundation floor area is missing, fall back to the floor-count ratio split.
    let indoor_floor_area_m2 = match (
        conditioned_floor_area_m2,
        total_conditioned_floors,
        floors_above_grade,
        foundation_floor_area_m2,
    ) {
        (Some(total), Some(n_total), Some(n_above), Some(foundation_area))
            if n_total > 0.0 && n_above >= 0.0 && n_above < n_total =>
        {
            let below_grade_floors = (n_total - n_above).max(0.0);
            Some((total - foundation_area * below_grade_floors).max(0.0))
        }
        (Some(total), Some(n_total), Some(n_above), None)
            if n_total > 0.0 && n_above >= 0.0 && n_above < n_total =>
        {
            Some(total * n_above / n_total)
        }
        _ => conditioned_floor_area_m2,
    };

    let mut zones = build_zone_map(details, indoor_floor_area_m2);
    ensure_referenced_zones_exist(&boundaries, &mut zones);
    assign_walls_to_zones(&boundaries, &mut zones);
    parse_duct_systems(details, &mut zones);

    // Auto-generate interior wall boundary (partition thermal mass).
    // Area = conditioned floor area, same-zone (Conditioned→Conditioned).
    if let Some(cond_zone) = zones
        .values()
        .find(|z| z.zone_type == ZoneType::Conditioned)
    {
        if let Some(area) = cond_zone.floor_area_m2 {
            if area > 0.0 {
                boundaries.push(Boundary {
                    id: "interior_wall".to_string(),
                    boundary_type: BoundaryType::Wall,
                    area_m2: area,
                    azimuth_deg: None,
                    assembly_r_value_m2_k_w: None,
                    r_value_layers_m2_k_w: Vec::new(),
                    interior_zone: Some(ZoneType::Conditioned),
                    exterior_zone: Some(ZoneType::Conditioned),
                    material_layers: Vec::new(),
                    framing_factor: None,
                    construction_type: None,
                    finish_type: None,
                    insulation_details: Some("Standard".to_string()),
                    has_radiant_barrier: false,
                    solar_absorptance: None,
                    emittance: None,
                    tilt_deg: Some(90.0),
                    lut_boundary_name: Some("Interior Wall".to_string()),
                    floor_or_ceiling: None,
                    perimeter_m: None,
                    perimeter_insulation_r_m2_k_w: None,
                    foundation_depth_m: None,
                });
            }
        }
    }

    // Auto-generate furniture boundaries per zone (same-zone thermal mass).
    // HPXML extension/FurnitureMass/AreaFraction overrides the conditioned zone default.
    let furniture_area_fraction_override = details
        .path(&["Enclosure", "extension", "FurnitureMass", "AreaFraction"])
        .and_then(XmlNode::text_as_f64);
    const FURNITURE_FRACTIONS: &[(ZoneType, f64)] = &[
        (ZoneType::Conditioned, 0.4),
        (ZoneType::Foundation, 0.4),
        (ZoneType::Garage, 0.1),
        // Attic: 0 (no furniture)
    ];
    for (zone_type, default_fraction) in FURNITURE_FRACTIONS {
        if let Some(zone) = zones.values().find(|z| z.zone_type == *zone_type) {
            if let Some(area) = zone.floor_area_m2 {
                let fraction = if *zone_type == ZoneType::Conditioned {
                    furniture_area_fraction_override.unwrap_or(*default_fraction)
                } else {
                    *default_fraction
                };
                let furniture_area = area * fraction;
                if furniture_area > 0.0 {
                    let lut_name = match zone_type {
                        ZoneType::Conditioned => "Indoor Furniture",
                        ZoneType::Foundation => "Foundation Furniture",
                        ZoneType::Garage => "Garage Furniture",
                        // Unreachable: the FURNITURE_FRACTIONS loop above iterates
                        // exactly Conditioned, Foundation, and Garage — no other
                        // ZoneType variants can appear here.
                        _ => unreachable!(
                            "FURNITURE_FRACTIONS iterated {zone_type:?} which has no furniture LUT entry"
                        ),
                    };
                    boundaries.push(Boundary {
                        id: format!("{}_furniture", zone_key(zone_type)),
                        boundary_type: BoundaryType::Wall,
                        area_m2: furniture_area,
                        azimuth_deg: None,
                        assembly_r_value_m2_k_w: None,
                        r_value_layers_m2_k_w: Vec::new(),
                        interior_zone: Some(zone_type.clone()),
                        exterior_zone: Some(zone_type.clone()),
                        material_layers: Vec::new(),
                        framing_factor: None,
                        construction_type: None,
                        finish_type: None,
                        insulation_details: Some("Standard".to_string()),
                        has_radiant_barrier: false,
                        solar_absorptance: None,
                        emittance: None,
                        tilt_deg: Some(90.0),
                        lut_boundary_name: Some(lut_name.to_string()),
                        floor_or_ceiling: None,
                        perimeter_m: None,
                        perimeter_insulation_r_m2_k_w: None,
                        foundation_depth_m: None,
                    });
                }
            }
        }
    }

    // Filter out Outdoor -- it's a boundary condition, not a thermal zone.
    // OCHRE only creates thermal zones for Conditioned, Attic, Garage, Foundation.
    let mut zones_vec: Vec<Zone> = zones
        .into_values()
        .filter(|z| {
            !matches!(
                z.zone_type,
                ZoneType::Outdoor | ZoneType::Ground | ZoneType::Adjacent
            )
        })
        .collect();
    zones_vec.sort_by_key(|zone| zone_sort_key(&zone.zone_type));

    // Compute ceiling height, warning on fallback.
    let default_height_m = match ceiling_height_m {
        Some(h) => h,
        None => {
            tracing::warn!(
                "ceiling height not derivable from conditioned volume/area; falling back to 2.5 m"
            );
            2.5
        }
    };

    // Compute garage geometry (protruded area) from wall boundaries.
    // Ref: OCHRE hpxml.py:439-494.
    let garage_floor_area_m2 = zones_vec
        .iter()
        .find(|z| z.zone_type == ZoneType::Garage)
        .and_then(|z| z.floor_area_m2)
        .unwrap_or(0.0);
    let garage_geometry = if garage_floor_area_m2 > 0.0 {
        compute_garage_geometry(&boundaries, garage_floor_area_m2)
    } else {
        None
    };
    if garage_floor_area_m2 > 0.0 && garage_geometry.is_none() {
        tracing::warn!(
            garage_floor_area_m2,
            "could not derive garage geometry; compound attic volume unavailable"
        );
    }

    // Attic floor area: OCHRE defines attic_floor_area as the top-floor
    // boundary area (Attic Floor / Roof / Adjacent Ceiling) plus Garage Ceiling area.
    // Ref: OCHRE hpxml.py:428-437.
    //
    // If the zone's <FloorArea> was not specified, derive it from the
    // Conditioned→Attic floor boundary plus any Garage→Attic floor boundaries.
    if let Some(attic_zone) = zones_vec
        .iter_mut()
        .find(|z| z.zone_type == ZoneType::Attic)
    {
        if attic_zone.floor_area_m2.is_none() {
            // Sum "Attic Floor" boundaries (Conditioned↔Attic).
            let attic_floor_from_boundaries: f64 = boundaries
                .iter()
                .filter(|b| {
                    b.boundary_type == BoundaryType::Floor
                        && ((b.interior_zone.as_ref() == Some(&ZoneType::Conditioned)
                            && b.exterior_zone.as_ref() == Some(&ZoneType::Attic))
                            || (b.interior_zone.as_ref() == Some(&ZoneType::Attic)
                                && b.exterior_zone.as_ref() == Some(&ZoneType::Conditioned)))
                })
                .map(|b| b.area_m2)
                .sum();
            if attic_floor_from_boundaries > 0.0 {
                attic_zone.floor_area_m2 = Some(attic_floor_from_boundaries);
            }
        }

        // Add Garage Ceiling areas (Garage↔Attic floor boundaries).
        let garage_ceiling_area_m2: f64 = boundaries
            .iter()
            .filter(|b| {
                b.boundary_type == BoundaryType::Floor
                    && ((b.interior_zone.as_ref() == Some(&ZoneType::Garage)
                        && b.exterior_zone.as_ref() == Some(&ZoneType::Attic))
                        || (b.interior_zone.as_ref() == Some(&ZoneType::Attic)
                            && b.exterior_zone.as_ref() == Some(&ZoneType::Garage)))
            })
            .map(|b| b.area_m2)
            .sum();
        if garage_ceiling_area_m2 > 0.0 {
            attic_zone.floor_area_m2 =
                Some(attic_zone.floor_area_m2.unwrap_or(0.0) + garage_ceiling_area_m2);
        }
    }

    // Assign volumes to all zones from available geometry.
    for zone in &mut zones_vec {
        zone.volume_m3 = match zone.zone_type {
            ZoneType::Conditioned => zone.floor_area_m2.map(|a| a * default_height_m),
            ZoneType::Attic => {
                compute_attic_volume(&boundaries, zone.floor_area_m2, garage_geometry.as_ref())
            }
            ZoneType::Garage => zone.floor_area_m2.map(|a| a * default_height_m),
            ZoneType::Foundation => zone
                .floor_area_m2
                .zip(foundation_height_m)
                .map(|(a, h)| a * h),
            // Outdoor, Ground, Adjacent are filtered above; Other has no volume model.
            ZoneType::Outdoor | ZoneType::Ground | ZoneType::Adjacent | ZoneType::Other(_) => None,
        };
    }

    // <AirLeakage> ACH50 / ACHnatural (HPXML 4.x inline or HPXML 3.x wrapper).
    let (infiltration_ach50, infiltration_ach_natural) = parse_air_leakage_ach50(details)?;

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
        infiltration_cfm50,
        infiltration_cfm_natural,
        infiltration_ela_cm2,
        infiltration_constant_ach: None,
        infiltration_ach_natural,
        infiltration_ach50,
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
        foundation_name,
        residential_facility_type,
        mass_multiplier_override: None,
        hvac_deadband_c: None,
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
        let (interior_shading_fraction, winter_shading_fraction) =
            match window.child("InteriorShading") {
                Some(shading) => {
                    let summer = shading
                        .child("SummerShadingCoefficient")
                        .and_then(XmlNode::text_as_f64)
                        .unwrap_or(0.70)
                        .clamp(0.0, 1.0);
                    let winter = shading
                        .child("WinterShadingCoefficient")
                        .and_then(XmlNode::text_as_f64)
                        .unwrap_or(0.85)
                        .clamp(0.0, 1.0);
                    (summer, winter)
                }
                None => (1.0, 1.0),
            };

        let fraction_operable = window
            .child("FractionOperable")
            .and_then(XmlNode::text_as_f64)
            .unwrap_or(0.67)
            .clamp(0.0, 1.0);

        let (exterior_shading_summer, exterior_shading_winter) =
            match window.child("ExteriorShading") {
                Some(shading) => {
                    let summer = shading
                        .child("SummerShadingCoefficient")
                        .and_then(XmlNode::text_as_f64)
                        .unwrap_or(1.0)
                        .clamp(0.0, 1.0);
                    let winter = shading
                        .child("WinterShadingCoefficient")
                        .and_then(XmlNode::text_as_f64)
                        .unwrap_or(1.0)
                        .clamp(0.0, 1.0);
                    (summer, winter)
                }
                None => (1.0, 1.0),
            };

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
            winter_shading_fraction,
            fraction_operable,
            exterior_shading_summer,
            exterior_shading_winter,
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
            framing_factor: None,
            lut_boundary_name: None,
            floor_or_ceiling: None,
            perimeter_m: None,
            perimeter_insulation_r_m2_k_w: None,
            foundation_depth_m: None,
        });
    }

    Ok(windows)
}

fn parse_boundary(node: &XmlNode, boundary_type: BoundaryType) -> Result<Boundary, HpxmlError> {
    let id = element_id(node).unwrap_or_else(|| "unknown".to_string());
    let area_m2 = parse_boundary_area(node, &boundary_type, &id)?;
    let r_value_layers_m2_k_w = parse_nominal_r_layers(node);
    let assembly_r_value_m2_k_w = parse_value_with_units(
        node.first_descendant("AssemblyEffectiveRValue"),
        ValueKind::RValue,
    )
    .or_else(|| parse_value_with_units(node.first_descendant("RValue"), ValueKind::RValue));
    let material_layers = parse_material_layers(node, area_m2);

    let has_radiant_barrier = node
        .first_descendant("RadiantBarrier")
        .map(|n| n.text.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    // Solar absorptance and emittance from HPXML, validated to [0, 1].
    // Ref: OCHRE hpxml.py:155-158, OCHRE Envelope.py:222.
    let solar_absorptance = parse_value_with_units(node.child("SolarAbsorptance"), ValueKind::Raw)
        .map(|v| v.clamp(0.0, 1.0));
    let emittance =
        parse_value_with_units(node.child("Emittance"), ValueKind::Raw).map(|v| v.clamp(0.0, 1.0));

    // Extract construction metadata for OCHRE LUT matching.
    let (construction_type, finish_type) = extract_construction_metadata(node, &boundary_type);
    let insulation_details = extract_insulation_details(node);

    // Surface tilt from HPXML <Pitch> (roofs) or implied by boundary type.
    // Pitch is rise:12 run (US roofing convention); tilt = atan(pitch/12).
    // Ref: OCHRE hpxml.py pitch2deg().
    let tilt_deg = match boundary_type {
        BoundaryType::Roof => {
            let pitch = parse_value_with_units(node.child("Pitch"), ValueKind::Raw).unwrap_or(0.0);
            Some((pitch / 12.0).atan().to_degrees())
        }
        BoundaryType::Wall | BoundaryType::FoundationWall | BoundaryType::RimJoist => Some(90.0),
        BoundaryType::Floor => Some(0.0),
        BoundaryType::Slab => Some(180.0),
        BoundaryType::Door => Some(90.0),
        BoundaryType::Window => Some(90.0),
        BoundaryType::Other(_) => {
            tracing::warn!("Unknown boundary type; no tilt inferred");
            None
        }
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
        framing_factor: parse_framing_factor(node, construction_type.as_deref()),
        construction_type,
        finish_type,
        insulation_details,
        has_radiant_barrier,
        solar_absorptance,
        emittance,
        tilt_deg,
        lut_boundary_name: None,
        floor_or_ceiling: node.child("FloorOrCeiling").and_then(|n| {
            match n.text.trim().to_ascii_lowercase().as_str() {
                "floor" => Some(FloorOrCeiling::Floor),
                "ceiling" => Some(FloorOrCeiling::Ceiling),
                other => {
                    tracing::warn!(
                        floor_or_ceiling = other,
                        "Unrecognized FloorOrCeiling value; defaulting to None"
                    );
                    None
                }
            }
        }),
        perimeter_m: None,
        perimeter_insulation_r_m2_k_w: None,
        foundation_depth_m: None,
    })
}

/// Compute the assembly-level framing fraction from stud geometry per
/// ASHRAE Handbook of Fundamentals 2021, Ch. 27, Table 6. Includes studs,
/// double top plates, single bottom plate, headers, corners, and miscellaneous
/// framing members (sills, blocking, partition intersections).
///
/// - `stud_width_in`: nominal stud width in inches (e.g. 1.5 for 2× lumber)
/// - `stud_spacing_in`: on-center stud spacing in inches (e.g. 16.0)
/// - `wall_height_in`: wall height in inches (default 96.0 for 8 ft)
///
/// Returns the framing fraction clamped to [0.10, 0.35].
///
/// Calibration: the geometric terms (studs + plates) alone give ~0.141 for
/// 2×4 at 16" OC, well below the ASHRAE Table 6 assembly value of 0.23.
/// The misc term captures headers (≈0.04) plus corner/blocking framing and
/// is calibrated to hit the ASHRAE assembly value at 16" OC.
/// The (16/spacing)² scaling approximates the reduced header/corner framing
/// density with wider stud spacing — advanced framing at 24" OC uses lighter
/// headers, 2-stud corners, and single top plates, all of which reduce the
/// assembly fraction below what a fixed constant would predict.
pub(crate) fn assembly_framing_factor(
    stud_width_in: f64,
    stud_spacing_in: f64,
    wall_height_in: f64,
) -> f64 {
    // Stud face fraction — the fraction of wall area covering stud faces.
    let ff_studs = stud_width_in / stud_spacing_in;

    // Top and bottom plate fraction. Standard framing: double top plate
    // (3.0" for 2× lumber) + single bottom plate (1.5") = 3 × stud_width.
    let ff_plates = 3.0 * stud_width_in / wall_height_in;

    // Headers, corners, sills, blocking, and partition intersections.
    // Calibrated to 0.09 at 16" OC to match ASHRAE HoF 2021 Ch. 27 Table 6
    // assembly value of 0.23 for 2×4 wood stud walls:
    //   0.094 (studs) + 0.047 (plates) + 0.09 (misc) = 0.231.
    // The (16/spacing)² scaling reduces the misc term for wider spacing
    // to approximate advanced framing provisions (lighter headers,
    // fewer corner assemblies) that accompany lower stud density.
    let ff_misc = 0.09 * (16.0 / stud_spacing_in).powi(2);

    (ff_studs + ff_plates + ff_misc).clamp(0.10, 0.35)
}

/// Parse framing factor from HPXML `<FramingFactor>` element, or derive from
/// `<StudSpacing>` and `<StudWidth>`, or default by construction type.
///
/// ASHRAE Handbook of Fundamentals Ch. 27.3: parallel-path method requires
/// the fraction of wall area that is structural framing.
fn parse_framing_factor(node: &XmlNode, construction_type: Option<&str>) -> Option<f64> {
    // Explicit FramingFactor from HPXML
    // Range (0, 1) excludes boundaries: 0.0 = no framing (equivalent to None),
    // 1.0 = all framing (physically impossible for an insulated wall).
    if let Some(ff) = find_descendant_f64(node, "FramingFactor", ValueKind::Raw) {
        if ff > 0.0 && ff < 1.0 {
            return Some(ff);
        }
    }

    // Derive assembly framing fraction from stud geometry and wall height.
    // Per ASHRAE HoF 2021 Ch. 27 Table 6: the assembly-level framing fraction
    // includes studs, plates, headers, corners, and miscellaneous members.
    if let (Some(spacing_in), Some(width_in)) = (
        find_descendant_f64(node, "StudSpacing", ValueKind::Raw),
        find_descendant_f64(node, "StudWidth", ValueKind::Raw),
    ) {
        if spacing_in > 0.0 && width_in > 0.0 && width_in < spacing_in {
            // WallHeight defaults to 96 in (8 ft) per ASHRAE/HPXML convention.
            // HPXML WallHeight is typically in feet; convert to inches.
            let wall_height_in = node
                .child("WallHeight")
                .and_then(|n| {
                    let raw = n.text_as_f64()?;
                    let units_norm = n
                        .attrs
                        .get("units")
                        .or_else(|| n.attrs.get("unit"))
                        .map(|u| normalize_ascii(u));
                    let inches = match units_norm.as_deref() {
                        Some("ft") | Some("feet") => raw * 12.0,
                        Some("in") | Some("inch") | Some("inches") => raw,
                        None => raw * 12.0, // HPXML default: feet
                        _ => {
                            tracing::warn!(
                                units = ?units_norm,
                                value = raw,
                                "unrecognized unit for WallHeight; assuming feet"
                            );
                            raw * 12.0
                        }
                    };
                    if inches <= 0.0 {
                        tracing::warn!(
                            wall_height_in = inches,
                            "WallHeight must be positive; defaulting to 96 in (8 ft)"
                        );
                        None
                    } else {
                        Some(inches)
                    }
                })
                .unwrap_or(96.0);

            return Some(assembly_framing_factor(
                width_in,
                spacing_in,
                wall_height_in,
            ));
        }
    }

    // Default by construction type per ASHRAE Handbook of Fundamentals.
    // 25% for 16" OC (standard), 22% for 24" OC (advanced framing).
    // HPXML WallType first-child element names.
    //
    // SteelFrame is intentionally excluded from the default branch:
    // ASHRAE HoF 2021 Ch. 27 requires the zone method (series-parallel)
    // for metal framing, not the parallel-path method with softwood
    // conductivity. Without explicit <StudSpacing> and <StudWidth>
    // the zone method cannot be applied, so we return None to prevent
    // the silent use of wood-based parallel-path correction.
    match construction_type {
        Some("WoodStud") => Some(0.25),
        Some(other) => {
            tracing::warn!(
                construction_type = other,
                "Unrecognized construction type; no framing fraction applied \
                 (SteelFrame requires explicit StudSpacing/StudWidth for zone method)"
            );
            None
        }
        None => None,
    }
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
        BoundaryType::Slab => Some(ZoneType::Ground),
        // Floor/FrameFloor: HPXML requires <ExteriorAdjacentTo>, so exterior
        // zone is always parsed from the element rather than inferred here.
        _ => None,
    }
}

/// Extract foundation wall insulation details, area scale factor, height,
/// and depth below grade [m].
///
/// Mirrors OCHRE `get_fnd_wall_insulation` (envelope.py:434-459):
/// - Area scaled by `DepthBelowGrade / Height` when they differ.
/// - Insulation details: "Half R{n}", "R{n}", or "Uninsulated".
///
/// Returns `(insulation_details, area_scale, height_m, depth_below_grade_m)`.
/// `depth_below_grade_m` defaults to the wall height when absent from HPXML.
///
/// `details` is the BuildingDetails node; `wall_id` identifies which FoundationWall.
fn extract_foundation_wall_insulation(
    details: &XmlNode,
    wall_id: &str,
) -> (Option<String>, f64, Option<f64>, f64) {
    // Find the FoundationWall element matching this boundary's ID.
    let wall_node = details
        .path(&["Enclosure", "FoundationWalls"])
        .and_then(|group| {
            group.children_named("FoundationWall").find(|n| {
                n.child("SystemIdentifier")
                    .and_then(|si| si.attrs.get("id"))
                    .map(|id| id == wall_id)
                    .unwrap_or(false)
            })
        });
    let Some(node) = wall_node else {
        return (Some("Uninsulated".to_string()), 1.0, None, 0.0);
    };

    // Area scaling: depth_below_grade / height.
    let height = parse_value_with_units(node.child("Height"), ValueKind::Length);
    let height_for_scale = height.unwrap_or(1.0);
    let depth_below_grade =
        parse_value_with_units(node.child("DepthBelowGrade"), ValueKind::Length)
            .unwrap_or(height_for_scale);
    let area_scale = if let Some(height_m) = height {
        if height_m > 0.0 && (depth_below_grade - height_m).abs() > 0.01 {
            (depth_below_grade / height_m).clamp(0.0, 1.0)
        } else {
            1.0
        }
    } else {
        1.0
    };
    let height_m = height.filter(|h| *h > 0.0);

    // Sum nominal R-values from insulation layers.
    // Read raw IP values (no unit conversion) for the LUT insulation details string,
    // since the LUT CSV uses IP R-values ("R10", "Half R5", etc.).
    let mut insulation_layers = Vec::new();
    if let Some(ins) = node.child("Insulation") {
        ins.descendants("Layer", &mut insulation_layers);
    }
    let r_ip: f64 = insulation_layers
        .iter()
        .filter_map(|layer| layer.child("NominalRValue").and_then(|n| n.text_as_f64()))
        .sum();

    let insulation_details = if r_ip > 0.0 {
        // Insulation height from DistanceToBottom - DistanceToTop per layer.
        // These are in the same units as Height (both converted to meters).
        let insulation_height = insulation_layers
            .iter()
            .map(|layer| {
                let dist_bottom = parse_value_with_units(
                    layer.child("DistanceToBottomOfInsulation"),
                    ValueKind::Length,
                )
                .unwrap_or(height_for_scale);
                let dist_top = parse_value_with_units(
                    layer.child("DistanceToTopOfInsulation"),
                    ValueKind::Length,
                )
                .unwrap_or(0.0);
                dist_bottom - dist_top
            })
            .reduce(f64::min)
            .unwrap_or(height_for_scale);

        let r_int = r_ip.round() as i32;
        if insulation_height > 0.0 && height_m.is_some_and(|h| insulation_height <= h / 2.0) {
            format!("Half R{r_int}")
        } else {
            format!("R{r_int}")
        }
    } else {
        "Uninsulated".to_string()
    };

    (
        Some(insulation_details),
        area_scale,
        height_m,
        depth_below_grade,
    )
}

/// Extract slab insulation details for LUT matching.
///
/// Mirrors OCHRE `get_slab_insulation` (envelope.py:462-485).
/// Reads `PerimeterInsulation` and `UnderSlabInsulation` from the Slab element
/// to produce format strings like "2ft R10 Perimeter", "R10 Whole Slab", etc.
///
/// All numeric values are raw IP (HPXML native) -- no unit conversion needed
/// since the LUT CSV uses IP values.
fn extract_slab_insulation(node: &XmlNode) -> Option<String> {
    let r_perimeter = node
        .path(&["PerimeterInsulation", "Layer", "NominalRValue"])
        .and_then(|n| n.text_as_f64())
        .unwrap_or(0.0);
    let r_under = node
        .path(&["UnderSlabInsulation", "Layer", "NominalRValue"])
        .and_then(|n| n.text_as_f64())
        .unwrap_or(0.0);

    // R >= 100 is OCHRE's threshold for "Minimal" (essentially no insulation modeled).
    // Ref: OCHRE envelope.py:466.
    let insulation = if r_perimeter >= 100.0 && r_under >= 100.0 {
        "Minimal".to_string()
    } else if r_perimeter > 0.0 && r_under <= 0.0 {
        let depth = node
            .path(&["PerimeterInsulation", "Layer", "InsulationDepth"])
            .and_then(|n| n.text_as_f64())
            .unwrap_or(0.0);
        let d = depth.round() as i32;
        let r = r_perimeter.round() as i32;
        format!("{d}ft R{r} Perimeter")
    } else if r_perimeter <= 0.0 && r_under > 0.0 {
        let full_width = node
            .path(&["UnderSlabInsulation", "Layer", "InsulationSpansEntireSlab"])
            .map(|n| n.text.trim().eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let r = r_under.round() as i32;
        if full_width {
            format!("R{r} Whole Slab")
        } else {
            let width = node
                .path(&["UnderSlabInsulation", "Layer", "InsulationWidth"])
                .and_then(|n| n.text_as_f64())
                .unwrap_or(0.0);
            let w = width.round() as i32;
            format!("{w}ft R{r} Exterior")
        }
    } else {
        "Uninsulated".to_string()
    };

    Some(insulation)
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
        BoundaryType::Window | BoundaryType::Door => (None, None),
        BoundaryType::Other(_) => (None, None),
    }
}

/// Extract insulation details string from nominal R-value layers.
/// Insulation details are only relevant for foundation walls and slabs, which are
/// handled by post-processing in `parse_building_from_node`. All other boundary
/// types get `None` -- OCHRE does not pass insulation_details for walls/roofs/floors.
fn extract_insulation_details(_node: &XmlNode) -> Option<String> {
    None
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
            let unit_node = vr.child("UnitofMeasure").map(|n| normalize_ascii(&n.text));
            match (value_node, unit_node.as_deref()) {
                (Some(v), Some("achnatural")) => (Some(v), None),
                (Some(v), Some("sla")) => (None, Some(v)),
                (_, other) => {
                    if let Some(u) = other {
                        tracing::warn!(
                            ventilation_unit = u,
                            "Unrecognized ventilation rate unit; expected 'ACHnatural' or 'SLA'"
                        );
                    }
                    (None, None)
                }
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
                // <FlatRoof/> means no attic cavity; skip zone creation (OCHRE hpxml.py:575-576).
                let attic_type_child = node.child("AtticType").and_then(|at| at.children.first());
                if let Some(child) = attic_type_child {
                    if child.name == "FlatRoof" {
                        continue;
                    }
                }

                let floor_area_m2 =
                    parse_value_with_units(node.child("FloorArea"), ValueKind::Area);

                // Parse vented status from <AtticType><Attic><Vented>
                let vented = attic_type_child
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

fn ensure_referenced_zones_exist(boundaries: &[Boundary], zones: &mut HashMap<String, Zone>) {
    for boundary in boundaries {
        for zone_type in [&boundary.interior_zone, &boundary.exterior_zone]
            .into_iter()
            .flatten()
        {
            match zone_type {
                ZoneType::Attic | ZoneType::Garage | ZoneType::Foundation => {
                    zones.entry(zone_key(zone_type)).or_insert_with(|| Zone {
                        zone_type: zone_type.clone(),
                        floor_area_m2: None,
                        volume_m3: None,
                        attached_wall_ids: Vec::new(),
                        duct_systems: Vec::new(),
                        vented: false,
                        ventilation_ach: None,
                        ventilation_sla: None,
                    });
                }
                ZoneType::Conditioned
                | ZoneType::Outdoor
                | ZoneType::Ground
                | ZoneType::Adjacent
                | ZoneType::Other(_) => {}
            }
        }
    }
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
    let mut air_dist_nodes = Vec::new();
    details.descendants("AirDistribution", &mut air_dist_nodes);
    let mut leakage_by_type: std::collections::HashMap<String, f64> =
        std::collections::HashMap::new();
    let mut leakage_cfm25_by_type: std::collections::HashMap<String, f64> =
        std::collections::HashMap::new();
    for air_dist in &air_dist_nodes {
        let mut measurements = Vec::new();
        air_dist.descendants("DuctLeakageMeasurement", &mut measurements);
        for meas in &measurements {
            let dtype = meas
                .first_descendant("DuctType")
                .map(|n| normalize_ascii(&n.text))
                .unwrap_or_default();
            if let Some(leak_node) = meas.first_descendant("DuctLeakage") {
                let units = leak_node
                    .first_descendant("Units")
                    .map(|n| normalize_ascii(&n.text))
                    .unwrap_or_default();
                if let Some(value) = leak_node
                    .first_descendant("Value")
                    .and_then(XmlNode::text_as_f64)
                {
                    match units.as_str() {
                        "percent" => {
                            leakage_by_type.insert(dtype, value / 100.0);
                        }
                        "fraction" => {
                            leakage_by_type.insert(dtype, value);
                        }
                        "cfm25" => {
                            leakage_cfm25_by_type.insert(dtype, value);
                        }
                        _ => {
                            tracing::warn!(
                                units = %units,
                                "unsupported duct leakage unit (cannot convert to fraction without fan flow); skipping"
                            );
                        }
                    }
                }
            }
        }
        let mut child_ducts = Vec::new();
        air_dist.descendants("Ducts", &mut child_ducts);
        ducts.extend(child_ducts);
    }

    for duct_node in ducts {
        let id = element_id(duct_node).unwrap_or_else(|| "unknown".to_string());
        let duct_type_text = duct_node
            .first_descendant("DuctType")
            .map(|n| normalize_ascii(&n.text))
            .unwrap_or_default();
        let leakage_fraction = leakage_by_type.get(&duct_type_text).copied();
        let leakage_cfm25 = leakage_cfm25_by_type.get(&duct_type_text).copied();

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
                other => {
                    tracing::warn!(
                        duct_type = other,
                        "Unrecognized duct type; defaulting to Unknown"
                    );
                    DuctType::Unknown
                }
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
                leakage_cfm25,
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
        Some(unit) => {
            tracing::warn!(unit, value, "unrecognized area unit; returning raw value");
            value
        }
        None => {
            tracing::debug!(
                value,
                "Area value has no units attribute; assuming ft² and converting to m²"
            );
            conv::area_ft2_to_m2(value)
        }
    }
}

fn convert_volume_to_m3(value: f64, units: Option<&str>) -> f64 {
    match units {
        Some("ft3") | Some("ft^3") | Some("cubic feet") => conv::volume_ft3_to_m3(value),
        Some(unit) => {
            tracing::warn!(unit, value, "unrecognized volume unit; returning raw value");
            value
        }
        None => {
            tracing::warn!(
                value,
                "Volume value has no units attribute; assuming ft³ and converting to m³"
            );
            conv::volume_ft3_to_m3(value)
        }
    }
}

fn convert_u_to_w_m2_k(value: f64, units: Option<&str>) -> f64 {
    match units {
        Some("btu/hr-ft2-f") | Some("btu/hr-ft^2-f") | Some("btu/(h*ft2*f)") => {
            conv::u_value_ip_to_si(value)
        }
        Some(unit) => {
            tracing::warn!(
                unit,
                value,
                "unrecognized U-value unit; returning raw value"
            );
            value
        }
        None => {
            tracing::debug!(
                value,
                "U-value has no units attribute; assuming BTU/(hr*ft2*F) and converting to W/(m2*K)"
            );
            conv::u_value_ip_to_si(value)
        }
    }
}

fn convert_r_to_m2_k_w(value: f64, units: Option<&str>) -> f64 {
    match units {
        Some("hr-ft2-f/btu") | Some("hr-ft^2-f/btu") | Some("h*ft2*f/btu") => {
            conv::r_value_ip_to_si(value)
        }
        Some(unit) => {
            tracing::warn!(
                unit,
                value,
                "unrecognized R-value unit; returning raw value"
            );
            value
        }
        None => {
            tracing::debug!(
                value,
                "R-value has no units attribute; assuming hr*ft2*F/BTU and converting to m2*K/W"
            );
            conv::r_value_ip_to_si(value)
        }
    }
}

fn convert_conductivity_to_w_m_k(value: f64, units: Option<&str>) -> f64 {
    match units {
        Some("btu/hr-ft-f") | Some("btu/(h*ft*f)") => conv::conductivity_btu_h_ft_f_to_w_m_k(value),
        Some("btu-in/hr-ft2-f") | Some("btu in/hr ft2 f") | Some("btu*in/(h*ft2*f)") => {
            conv::conductivity_btu_in_h_ft2_f_to_w_m_k(value)
        }
        Some(unit) => {
            tracing::warn!(
                unit,
                value,
                "unrecognized conductivity unit; returning raw value"
            );
            value
        }
        None => {
            tracing::debug!(
                value,
                "Conductivity has no units attribute; assuming BTU*in/(hr*ft2*F) and converting"
            );
            conv::conductivity_btu_in_h_ft2_f_to_w_m_k(value)
        }
    }
}

fn convert_length_to_m(value: f64, units: Option<&str>) -> f64 {
    match units {
        Some("in") | Some("inch") | Some("inches") => conv::length_in_to_m(value),
        Some("ft") | Some("feet") => conv::length_ft_to_m(value),
        Some(unit) => {
            tracing::warn!(unit, value, "unrecognized length unit; returning raw value");
            value
        }
        None => {
            tracing::debug!(
                value,
                "Length value has no units attribute; assuming feet and converting to meters"
            );
            conv::length_ft_to_m(value)
        }
    }
}

fn convert_density_to_kg_m3(value: f64, units: Option<&str>) -> f64 {
    match units {
        Some("lb/ft3") | Some("lb/ft^3") | Some("lbm/ft3") => conv::density_lb_ft3_to_kg_m3(value),
        Some(unit) => {
            tracing::warn!(
                unit,
                value,
                "unrecognized density unit; returning raw value"
            );
            value
        }
        None => {
            tracing::debug!(
                value,
                "Density has no units attribute; assuming lb/ft3 and converting"
            );
            conv::density_lb_ft3_to_kg_m3(value)
        }
    }
}

fn convert_specific_heat_to_j_kg_k(value: f64, units: Option<&str>) -> f64 {
    match units {
        Some("btu/lb-f") | Some("btu/(lb*f)") => conv::specific_heat_btu_lb_f_to_j_kg_k(value),
        Some(unit) => {
            tracing::warn!(
                unit,
                value,
                "unrecognized specific heat unit; returning raw value"
            );
            value
        }
        None => {
            tracing::debug!(
                value,
                "Specific heat has no units attribute; assuming Btu/(lb*F) and converting"
            );
            conv::specific_heat_btu_lb_f_to_j_kg_k(value)
        }
    }
}

fn convert_temperature_to_c(value: f64, units: Option<&str>) -> f64 {
    match units {
        Some("F") | Some("f") | Some("degF") | Some("degf") | Some("fahrenheit") => {
            conv::temperature_f_to_c(value)
        }
        Some("C") | Some("c") | Some("degC") | Some("degc") | Some("celsius") => value,
        Some(unit) => {
            tracing::warn!(
                unit,
                value,
                "unrecognized temperature unit; returning raw value"
            );
            value
        }
        None => {
            tracing::debug!(
                value,
                "Temperature value has no units attribute; assuming F and converting to C"
            );
            conv::temperature_f_to_c(value)
        }
    }
}

/// Extract CFM50 from `<BuildingAirLeakage>` when `UnitofMeasure` is "CFM" or
/// when `<AirLeakage units="CFM50">` appears directly under `AirInfiltrationMeasurement`.
///
/// When the unit is `"CFMnatural"`, the value is converted to a 50 Pa equivalent
/// using [`ach_nat_to_ach50`] (the same power-law conversion applies to CFM rates;
/// it depends only on the pressure ratio and flow exponent).
///
/// Returns `(cfm50, cfm_natural_raw)` where:
/// - `cfm50` is the (possibly converted) value at 50 Pa reference pressure.
/// - `cfm_natural_raw` is the original natural-pressure value when the unit was
///   `"CFMnatural"`, or `None` otherwise.
///
/// Returns `Err(HpxmlError::Parse(...))` when the unit string is unrecognised —
/// the HPXML input is malformed and the user must correct it.
fn parse_air_leakage_cfm50(details: &XmlNode) -> Result<(Option<f64>, Option<f64>), HpxmlError> {
    // HPXML 3.x: BuildingAirLeakage wrapper with UnitofMeasure child element.
    let measurement = match details.first_descendant("AirInfiltrationMeasurement") {
        Some(m) => m,
        None => return Ok((None, None)),
    };
    if let Some(bal) = measurement.child("BuildingAirLeakage") {
        let unit = bal
            .child("UnitofMeasure")
            .map(|n| normalize_ascii(n.text.trim()));
        if let Some(unit_str) = unit.as_deref() {
            if unit_str == "cfm" {
                let value = bal.child("AirLeakage").and_then(XmlNode::text_as_f64);
                return Ok((value, None));
            }
            if unit_str == "cfmnatural" {
                let raw = bal.child("AirLeakage").and_then(XmlNode::text_as_f64);
                let converted = raw.map(|v| ach_nat_to_ach50(v, NATURAL_TO_50PA_EXPONENT));
                return Ok((converted, raw));
            }
            // Known HPXML UnitofMeasure values that are not CFM/CFMnatural.
            if unit_str == "ach" || unit_str == "achnatural" {
                return Ok((None, None));
            }
            return Err(HpxmlError::Parse(format!(
                "unrecognised UnitofMeasure '{unit_str}' on BuildingAirLeakage -- \
                 expected ACH, CFM, ACHnatural, or CFMnatural"
            )));
        }
        // Absent UnitofMeasure — not a CFM measurement.
        return Ok((None, None));
    }
    // HPXML 4.x: <AirLeakage units="CFM50"> directly under AirInfiltrationMeasurement.
    if let Some(al) = measurement.child("AirLeakage") {
        let unit_attr = al
            .attrs
            .get("units")
            .or_else(|| al.attrs.get("unit"))
            .map(|s| normalize_ascii(s));
        if let Some(unit_str) = unit_attr.as_deref() {
            if matches!(unit_str, "cfm50" | "cfm") {
                let value = al.text_as_f64();
                return Ok((value, None));
            }
            if unit_str == "cfmnatural" {
                let raw = al.text_as_f64();
                let converted = raw.map(|v| ach_nat_to_ach50(v, NATURAL_TO_50PA_EXPONENT));
                return Ok((converted, raw));
            }
            // Known HPXML units="" values that are not CFM50/CFM/CFMnatural.
            if matches!(unit_str, "ach50" | "ach" | "achnatural") {
                return Ok((None, None));
            }
            return Err(HpxmlError::Parse(format!(
                "unrecognised units attribute '{unit_str}' on AirLeakage -- \
                 expected ACH, ACH50, CFM, or CFM50"
            )));
        }
        // No units attribute — not a CFM50 measurement.
        return Ok((None, None));
    }
    Ok((None, None))
}

/// Extract ACH50 from `<BuildingAirLeakage>` when `UnitofMeasure` is "ACH" or
/// absent, or from `<AirLeakage units="ACH50">` / `<AirLeakage units="ACH">`
/// under `AirInfiltrationMeasurement`, or from a bare `<AirLeakage>` value.
///
/// When the unit is `"ACHnatural"`, the value is converted to a 50 Pa equivalent
/// using [`ach_nat_to_ach50`] (ASHRAE 119 / ASTM E779 power law: `ACH50 = ACHnat × (50/4)^n`).
///
/// Returns `(ach50, ach_natural_raw)` where:
/// - `ach50` is the (possibly converted) value at 50 Pa reference pressure.
/// - `ach_natural_raw` is the original natural-pressure value when the unit was
///   `"ACHnatural"`, or `None` otherwise.
///
/// Returns `Err(HpxmlError::Parse(...))` when the unit string is unrecognised — this is
/// a genuine input error that the user must fix; silently dropping the value would cause
/// the home to be modelled with effectively zero infiltration.
fn parse_air_leakage_ach50(details: &XmlNode) -> Result<(Option<f64>, Option<f64>), HpxmlError> {
    // HPXML 3.x: BuildingAirLeakage wrapper with UnitofMeasure child element.
    let measurement = match details.first_descendant("AirInfiltrationMeasurement") {
        Some(m) => m,
        None => return Ok((None, None)),
    };
    if let Some(bal) = measurement.child("BuildingAirLeakage") {
        let unit = bal
            .child("UnitofMeasure")
            .map(|n| normalize_ascii(n.text.trim()));
        if let Some(unit_str) = unit.as_deref() {
            if unit_str == "ach" {
                let value = bal.child("AirLeakage").and_then(XmlNode::text_as_f64);
                return Ok((value, None));
            }
            if unit_str == "achnatural" {
                let raw = bal.child("AirLeakage").and_then(XmlNode::text_as_f64);
                let converted = raw.map(|v| ach_nat_to_ach50(v, NATURAL_TO_50PA_EXPONENT));
                return Ok((converted, raw));
            }
            // Known HPXML UnitofMeasure values that are not ACH/ACHnatural.
            if unit_str == "cfm" || unit_str == "cfmnatural" {
                return Ok((None, None));
            }
            return Err(HpxmlError::Parse(format!(
                "unrecognised UnitofMeasure '{unit_str}' on BuildingAirLeakage -- \
                 expected ACH, CFM, ACHnatural, or CFMnatural"
            )));
        }
        // Absent UnitofMeasure — HPXML convention: bare BuildingAirLeakage value is ACH.
        let value = bal.child("AirLeakage").and_then(XmlNode::text_as_f64);
        return Ok((value, None));
    }
    // HPXML 4.x: <AirLeakage units="ACH50"> directly under AirInfiltrationMeasurement,
    // or bare <AirLeakage> with no units attribute (HPXML convention: ACH50 when HousePressure=50).
    if let Some(al) = measurement.child("AirLeakage") {
        let unit_attr = al
            .attrs
            .get("units")
            .or_else(|| al.attrs.get("unit"))
            .map(|s| normalize_ascii(s));
        if let Some(unit_str) = unit_attr.as_deref() {
            if matches!(unit_str, "ach50" | "ach") {
                let value = al.text_as_f64();
                return Ok((value, None));
            }
            if unit_str == "achnatural" {
                let raw = al.text_as_f64();
                let converted = raw.map(|v| ach_nat_to_ach50(v, NATURAL_TO_50PA_EXPONENT));
                return Ok((converted, raw));
            }
            // Known HPXML units="" values that are not ACH50/ACH/ACHnatural.
            if matches!(unit_str, "cfm50" | "cfm" | "cfmnatural") {
                return Ok((None, None));
            }
            return Err(HpxmlError::Parse(format!(
                "unrecognised units attribute '{unit_str}' on AirLeakage -- \
                 expected ACH, ACH50, CFM, or CFM50"
            )));
        }
        // No units attribute — bare value (HPXML convention: ACH50 when HousePressure=50).
        return Ok((al.text_as_f64(), None));
    }
    Ok((None, None))
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

pub(crate) fn parse_zone_label(text: &str) -> ZoneType {
    let norm = normalize_ascii(text);
    if norm.contains("attic") {
        ZoneType::Attic
    } else if norm.contains("garage") {
        ZoneType::Garage
    } else if norm.contains("foundation") || norm.contains("basement") || norm.contains("crawl") {
        ZoneType::Foundation
    } else if norm.contains("condition") || norm == "living space" {
        ZoneType::Conditioned
    } else if norm == "ground" {
        ZoneType::Ground
    } else if norm.contains("out") || norm.contains("ambient") {
        ZoneType::Outdoor
    } else if norm.contains("other") {
        // OCHRE hpxml.py:78: multifamily zones ("other housing unit", "other heated space")
        // are adiabatic same-zone boundaries.
        ZoneType::Adjacent
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

pub(crate) fn zone_key(zone_type: &ZoneType) -> String {
    match zone_type {
        ZoneType::Conditioned => "conditioned".to_string(),
        ZoneType::Attic => "attic".to_string(),
        ZoneType::Garage => "garage".to_string(),
        ZoneType::Foundation => "foundation".to_string(),
        ZoneType::Outdoor => "outdoor".to_string(),
        ZoneType::Ground => "ground".to_string(),
        ZoneType::Adjacent => "adjacent".to_string(),
        ZoneType::Other(label) => label.to_ascii_lowercase(),
    }
}

/// Garage geometry derived from wall areas and azimuths.
///
/// `garage_protruded_area_m2` is the portion of the garage footprint that protrudes
/// beyond the main building rectangle. Used by the compound attic volume formula.
///
/// Ref: OCHRE `hpxml.py` lines 449–494.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
struct GarageGeometry {
    floor_area_m2: f64,
    wall_height_m: f64,
    protruded_area_m2: f64,
}

/// Derive garage geometry from boundary wall areas and azimuths.
///
/// The algorithm:
/// 1. Collect exterior garage walls (Garage→Outdoor) and adjacent garage walls
///    (Garage→Garage); group by azimuth mod 180° and take the max area per direction.
/// 2. The two perpendicular directions yield areas a1, a2.
///    `garage_wall_height = sqrt(a1 * a2 / garage_floor_area)`
/// 3. Count attached walls (Conditioned→Garage walls). Depending on count:
///    - 1 wall: `garage_area_in_main = 0` (detached or single-face)
///    - 2 walls: `garage_area_in_main = a1 * a2 / wall_height²`
///    - 3 walls: group by azimuth mod 180°, take largest per direction → same formula
/// 4. `protruded_area = floor_area - garage_area_in_main`
///
/// Ref: OCHRE `hpxml.py` lines 449–494.
fn compute_garage_geometry(
    boundaries: &[Boundary],
    garage_floor_area_m2: f64,
) -> Option<GarageGeometry> {
    if garage_floor_area_m2 <= 0.0 {
        return None;
    }

    // Collect exterior garage wall areas and azimuths (Garage→Outdoor + Garage→Garage).
    let mut wall_areas: Vec<f64> = Vec::new();
    let mut wall_azimuths: Vec<f64> = Vec::new();
    for b in boundaries {
        if b.boundary_type != BoundaryType::Wall {
            continue;
        }
        let is_garage_exterior = b.interior_zone.as_ref() == Some(&ZoneType::Garage)
            && matches!(b.exterior_zone.as_ref(), Some(&ZoneType::Outdoor) | None);
        let is_adjacent_garage = b.interior_zone.as_ref() == Some(&ZoneType::Garage)
            && b.exterior_zone.as_ref() == Some(&ZoneType::Garage);
        if is_garage_exterior || is_adjacent_garage {
            if let Some(az) = b.azimuth_deg {
                wall_areas.push(b.area_m2);
                wall_azimuths.push(az % 180.0);
            }
        }
    }

    // Group by azimuth mod 180 and find the two perpendicular max areas.
    let perpendicular_maxes = max_areas_by_azimuth(&wall_areas, &wall_azimuths)?;
    let (a1, a2) = perpendicular_maxes;
    let wall_height = (a1 * a2 / garage_floor_area_m2).sqrt();
    if wall_height <= 0.0 {
        return None;
    }

    // Collect attached walls (Conditioned→Garage or Garage→Conditioned walls).
    let mut attached_areas: Vec<f64> = Vec::new();
    let mut attached_azimuths: Vec<f64> = Vec::new();
    for b in boundaries {
        if b.boundary_type != BoundaryType::Wall {
            continue;
        }
        let is_attached = (b.interior_zone.as_ref() == Some(&ZoneType::Conditioned)
            && b.exterior_zone.as_ref() == Some(&ZoneType::Garage))
            || (b.interior_zone.as_ref() == Some(&ZoneType::Garage)
                && b.exterior_zone.as_ref() == Some(&ZoneType::Conditioned));
        if is_attached {
            attached_areas.push(b.area_m2);
            if let Some(az) = b.azimuth_deg {
                attached_azimuths.push(az % 180.0);
            }
        }
    }

    let garage_area_in_main = match attached_areas.len() {
        0 | 1 => 0.0,
        2 => {
            let (aa1, aa2) = (attached_areas[0], attached_areas[1]);
            aa1 * aa2 / (wall_height * wall_height)
        }
        3 => {
            if let Some((aa1, aa2)) = max_areas_by_azimuth(&attached_areas, &attached_azimuths) {
                aa1 * aa2 / (wall_height * wall_height)
            } else {
                tracing::warn!(
                    n_attached = 3,
                    "garage: 3 attached walls but max_areas_by_azimuth returned None; treating as detached"
                );
                0.0
            }
        }
        n => {
            tracing::warn!(
                n_attached = n,
                garage_floor_area_m2,
                "garage: unsupported attached wall count (expected 0-3); treating as detached"
            );
            0.0
        }
    };

    let protruded = (garage_floor_area_m2 - garage_area_in_main).max(0.0);

    Some(GarageGeometry {
        floor_area_m2: garage_floor_area_m2,
        wall_height_m: wall_height,
        protruded_area_m2: protruded,
    })
}

/// Given parallel arrays of wall areas and azimuths (mod 180°), return
/// the maximum area for each of the two perpendicular azimuth groups.
/// Returns None if there aren't exactly 2 distinct azimuth groups.
fn max_areas_by_azimuth(areas: &[f64], azimuths: &[f64]) -> Option<(f64, f64)> {
    use std::collections::BTreeMap;
    // Group by azimuth (quantized to nearest degree to handle floating-point).
    let mut groups: BTreeMap<i32, f64> = BTreeMap::new();
    for (&area, &az) in areas.iter().zip(azimuths.iter()) {
        let key = az.round() as i32;
        let entry = groups.entry(key).or_insert(0.0_f64);
        if area > *entry {
            *entry = area;
        }
    }
    if groups.len() != 2 {
        return None;
    }
    let vals: Vec<f64> = groups.into_values().collect();
    Some((vals[0], vals[1]))
}

/// Compute attic volume from gable wall areas, roof pitch, and garage geometry.
///
/// Two paths following OCHRE `parse_hpxml_zones()` (hpxml.py:582–633):
///
/// **Path A** -- Attic Garage Wall boundaries exist (walls between Garage and Attic):
///   Merge all attic-exterior and attic-garage wall areas; use the max of
///   the first two as gable_area. Simple prism formula.
///
/// **Path B** -- No Attic Garage Wall, has garage, 3 gable walls:
///   Compound formula:
///   ```text
///   V = 0.5 * (attic_floor_area - garage_protruded_area) * attic_height
///     + 0.5 * garage_protruded_area * garage_height
///     + (1/6) * garage_width * garage_depth_in_house * garage_height
///   ```
///
/// Otherwise: simple prism `0.5 * floor_area * attic_height`.
fn compute_attic_volume(
    boundaries: &[Boundary],
    attic_floor_area_m2: Option<f64>,
    garage_geometry: Option<&GarageGeometry>,
) -> Option<f64> {
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

    // Attic gable walls: Wall boundaries with interior=Attic, exterior=Outdoor.
    let mut attic_outdoor_walls: Vec<f64> = boundaries
        .iter()
        .filter(|b| {
            b.boundary_type == BoundaryType::Wall
                && b.interior_zone.as_ref() == Some(&ZoneType::Attic)
                && matches!(b.exterior_zone.as_ref(), Some(&ZoneType::Outdoor) | None)
        })
        .map(|b| b.area_m2)
        .collect();

    // Attic-Garage walls: Wall boundaries between Garage and Attic.
    let attic_garage_walls: Vec<f64> = boundaries
        .iter()
        .filter(|b| {
            b.boundary_type == BoundaryType::Wall
                && ((b.interior_zone.as_ref() == Some(&ZoneType::Garage)
                    && b.exterior_zone.as_ref() == Some(&ZoneType::Attic))
                    || (b.interior_zone.as_ref() == Some(&ZoneType::Attic)
                        && b.exterior_zone.as_ref() == Some(&ZoneType::Garage)))
        })
        .map(|b| b.area_m2)
        .collect();

    // Adjacent attic walls (Attic→Attic).
    let adjacent_attic_walls: Vec<f64> = boundaries
        .iter()
        .filter(|b| {
            b.boundary_type == BoundaryType::Wall
                && b.interior_zone.as_ref() == Some(&ZoneType::Attic)
                && b.exterior_zone.as_ref() == Some(&ZoneType::Attic)
        })
        .map(|b| b.area_m2)
        .collect();

    // Attic roof tilt.
    let roof_tilt_deg: Option<f64> = boundaries
        .iter()
        .filter(|b| {
            b.boundary_type == BoundaryType::Roof
                && b.interior_zone.as_ref() == Some(&ZoneType::Attic)
        })
        .find_map(|b| b.tilt_deg);

    let roof_tilt_rad = roof_tilt_deg?.to_radians();
    if roof_tilt_rad <= 0.0 {
        return None;
    }

    // Garage roof tilt (for compound formula). Falls back to attic roof tilt.
    let garage_tilt_rad = boundaries
        .iter()
        .filter(|b| {
            b.boundary_type == BoundaryType::Roof
                && b.interior_zone.as_ref() == Some(&ZoneType::Garage)
        })
        .find_map(|b| b.tilt_deg)
        .map(|d| d.to_radians())
        .unwrap_or(roof_tilt_rad);

    let has_garage = garage_geometry.is_some();

    // Path A: Attic Garage Wall exists -- merge wall areas, use max of first two.
    if !attic_garage_walls.is_empty() {
        let mut merged = attic_outdoor_walls;
        merged.extend_from_slice(&adjacent_attic_walls);
        merged.extend_from_slice(&attic_garage_walls);
        if merged.len() < 2 {
            return if merged.len() == 1 {
                let h = (merged[0] * roof_tilt_rad.tan()).sqrt();
                Some(0.5 * floor_area * h)
            } else {
                None
            };
        }
        let gable_area = merged[0].max(merged[1]);
        let attic_height = (gable_area * roof_tilt_rad.tan()).sqrt();
        return Some(0.5 * floor_area * attic_height);
    }

    // Merge attic outdoor walls + adjacent attic walls for standard path.
    attic_outdoor_walls.extend_from_slice(&adjacent_attic_walls);
    let gable_areas = attic_outdoor_walls;

    // Path B: No Attic Garage Wall, has garage, 3 gable walls → compound formula.
    if has_garage && gable_areas.len() == 3 {
        // OCHRE uses `attic_wall_areas[1]` -- the second exterior gable wall in
        // parse order (outdoor walls first, then adjacent attic walls). Our
        // `gable_areas` preserves this ordering, so index 1 matches OCHRE.
        let attic_gable_area = gable_areas[1];

        let mut sorted = gable_areas.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let (low, med, high) = (sorted[0], sorted[1], sorted[2]);
        // Third gable: whichever of low/high is "more different" from the median.
        let third_gable_area = if med - low > high - med { low } else { high };

        let attic_height = (attic_gable_area * roof_tilt_rad.tan()).sqrt();

        if third_gable_area > 0.0 {
            if let Some(gg) = garage_geometry {
                let garage_height = (third_gable_area * garage_tilt_rad.tan()).sqrt();
                let garage_width = 2.0 * third_gable_area / garage_height;
                let garage_depth_in_house = garage_height * roof_tilt_rad.tan();
                let square_area = floor_area - gg.protruded_area_m2;

                let volume = 0.5 * square_area * attic_height
                    + 0.5 * gg.protruded_area_m2 * garage_height
                    + (1.0 / 6.0) * garage_width * garage_depth_in_house * garage_height;
                return Some(volume);
            }
        }

        return Some(0.5 * floor_area * attic_height);
    }

    // Standard 2-gable or fallback.
    let gable_area = match gable_areas.len() {
        0 => return None,
        1 => gable_areas[0],
        2 => {
            let abs_diff = (gable_areas[1] - gable_areas[0]).abs();
            if abs_diff > 0.5 {
                tracing::warn!(
                    area_0_m2 = gable_areas[0],
                    area_1_m2 = gable_areas[1],
                    diff_m2 = abs_diff,
                    "attic gable walls differ by {diff:.2} m² (> 0.5 m²); \
                     cannot derive reliable attic height",
                    diff = abs_diff,
                );
                return None;
            }
            gable_areas[0]
        }
        _ => {
            let mut sorted = gable_areas;
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            sorted[1]
        }
    };
    if gable_area <= 0.0 {
        return None;
    }

    let attic_height = (gable_area * roof_tilt_rad.tan()).sqrt();
    Some(0.5 * floor_area * attic_height)
}

fn zone_sort_key(zone_type: &ZoneType) -> u8 {
    match zone_type {
        ZoneType::Conditioned => 0,
        ZoneType::Attic => 1,
        ZoneType::Garage => 2,
        ZoneType::Foundation => 3,
        ZoneType::Outdoor | ZoneType::Ground | ZoneType::Adjacent => 4,
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
    use super::{
        BoundaryType, DuctType, HpxmlError, ZoneType, assembly_framing_factor, parse_building,
    };

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
            <DistributionSystemType>
              <AirDistribution>
                <DuctLeakageMeasurement>
                  <SystemIdentifier id="Leak1"/>
                  <DuctType>supply</DuctType>
                  <DuctLeakage>
                    <Value>12</Value>
                    <Units>Percent</Units>
                  </DuctLeakage>
                </DuctLeakageMeasurement>
                <DuctLeakageMeasurement>
                  <SystemIdentifier id="Leak2"/>
                  <DuctType>return</DuctType>
                  <DuctLeakage>
                    <Value>5</Value>
                    <Units>Percent</Units>
                  </DuctLeakage>
                </DuctLeakageMeasurement>
                <Ducts>
                  <SystemIdentifier id="SupplyDuct"/>
                  <DuctType>supply</DuctType>
                  <DuctInsulationRValue>8</DuctInsulationRValue>
                  <DuctSurfaceArea>50</DuctSurfaceArea>
                  <DuctLocation>attic vented</DuctLocation>
                </Ducts>
                <Ducts>
                  <SystemIdentifier id="ReturnDuct"/>
                  <DuctType>return</DuctType>
                  <DuctInsulationRValue>4</DuctInsulationRValue>
                  <DuctSurfaceArea>30</DuctSurfaceArea>
                  <DuctLocation>attic vented</DuctLocation>
                </Ducts>
              </AirDistribution>
            </DistributionSystemType>
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

        // 4 thermal zones: Conditioned, Attic, Garage, Foundation.
        // Outdoor and Ground are boundary conditions, not thermal zones -- filtered out.
        assert_eq!(building.zones.len(), 4);
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
        // Wall1 = 100 ft² minus Window1 = 15 ft² → 85 ft² = 7.896758 m²
        let expected_wall_area = (100.0 - 15.0) * 0.092_903_04;
        assert!(
            (wall.area_m2 - expected_wall_area).abs() < 1.0e-4,
            "wall area should be net of window: got {}, expected {}",
            wall.area_m2,
            expected_wall_area
        );

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
        assert_eq!(attic.duct_systems.len(), 2);
        let supply = attic
            .duct_systems
            .iter()
            .find(|d| d.duct_type == DuctType::Supply)
            .expect("supply duct expected");
        let return_duct = attic
            .duct_systems
            .iter()
            .find(|d| d.duct_type == DuctType::Return)
            .expect("return duct expected");
        assert_eq!(supply.leakage_fraction, Some(0.12));
        assert_eq!(return_duct.leakage_fraction, Some(0.05));
        assert!((supply.insulation_r_value_m2_k_w.unwrap_or_default() - 1.4088).abs() < 1e-4);
        assert!((return_duct.insulation_r_value_m2_k_w.unwrap_or_default() - 0.7044).abs() < 1e-4);
        assert!((supply.surface_area_m2.unwrap() - 50.0 * 0.092903).abs() < 0.01);
        assert!((return_duct.surface_area_m2.unwrap() - 30.0 * 0.092903).abs() < 0.01);

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
        assert!((building.ceiling_height_m.unwrap() - expected_ceiling_height).abs() < 1e-6,);

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

    #[test]
    fn foundation_wall_gets_construction_type_from_foundation() {
        // Add FoundationType to the Foundation element so foundation_name is parsed.
        let xml = SAMPLE_XML.replace(
            "<Foundation>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
            "<Foundation>\n            <FoundationType><Basement/></FoundationType>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        assert_eq!(
            building.foundation_name.as_deref(),
            Some("Unfinished Basement"),
        );
        let fnd_wall = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::FoundationWall)
            .expect("foundation wall expected");
        assert_eq!(
            fnd_wall.construction_type.as_deref(),
            Some("Unfinished Basement"),
        );
        assert_eq!(fnd_wall.insulation_details.as_deref(), Some("Uninsulated"),);
    }

    #[test]
    fn foundation_wall_crawlspace_construction_type() {
        let xml = SAMPLE_XML.replace(
            "<Foundation>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
            "<Foundation>\n            <FoundationType><Crawlspace/></FoundationType>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        assert_eq!(building.foundation_name.as_deref(), Some("Crawlspace"));
        let fnd_wall = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::FoundationWall)
            .expect("foundation wall expected");
        assert_eq!(fnd_wall.construction_type.as_deref(), Some("Crawlspace"));
    }

    #[test]
    fn foundation_wall_area_scaling_and_insulation() {
        // FoundationWall with Height=8ft, DepthBelowGrade=4ft → area_scale=0.5
        // Plus R-10 insulation covering half the wall height.
        let xml = SAMPLE_XML
            .replace(
                "<Foundation>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
                "<Foundation>\n            <FoundationType><Basement/></FoundationType>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
            )
            .replace(
                "<FoundationWall>\n            <SystemIdentifier id=\"FoundationWall1\"/>\n            <InteriorAdjacentTo>basement - conditioned</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>ground</ExteriorAdjacentTo>\n            <Area units=\"ft2\">60</Area>\n          </FoundationWall>",
                "<FoundationWall>\n            <SystemIdentifier id=\"FoundationWall1\"/>\n            <InteriorAdjacentTo>basement - conditioned</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>ground</ExteriorAdjacentTo>\n            <Area units=\"ft2\">60</Area>\n            <Height units=\"ft\">8</Height>\n            <DepthBelowGrade units=\"ft\">4</DepthBelowGrade>\n            <Insulation>\n              <Layer>\n                <NominalRValue>10</NominalRValue>\n                <DistanceToTopOfInsulation units=\"ft\">0</DistanceToTopOfInsulation>\n                <DistanceToBottomOfInsulation units=\"ft\">4</DistanceToBottomOfInsulation>\n              </Layer>\n            </Insulation>\n          </FoundationWall>",
            );
        let building = parse_building(&xml).expect("parse should succeed");
        let fnd_wall = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::FoundationWall)
            .expect("foundation wall expected");

        // Area: 60 ft² × 0.0929 m²/ft² × (4/8) = ~2.787 m²
        let original_area_m2 = 60.0 * 0.092_903_04;
        assert!(
            (fnd_wall.area_m2 - original_area_m2 * 0.5).abs() < 0.01,
            "area should be halved: got {}, expected {}",
            fnd_wall.area_m2,
            original_area_m2 * 0.5
        );

        // Insulation: 4ft depth, 8ft height → 4/8 = 0.5 ≤ height/2 → "Half R10"
        // NominalRValue=10 is IP (HPXML stores IP), our code converts then back.
        assert_eq!(
            fnd_wall.insulation_details.as_deref(),
            Some("Half R10"),
            "half-height R-10 insulation expected"
        );

        // Regression: foundation_depth_m must be populated from DepthBelowGrade.
        // DepthBelowGrade=4 ft → 1.2192 m; centroid = half the below-grade portion.
        let expected_foundation_depth_m = 4.0 * 0.3048 / 2.0;
        assert!(
            (fnd_wall.foundation_depth_m.unwrap_or(0.0) - expected_foundation_depth_m).abs()
                < 0.001,
            "foundation_depth_m should be half of DepthBelowGrade (converted to m): got {:?}, expected {}",
            fnd_wall.foundation_depth_m,
            expected_foundation_depth_m
        );
    }

    #[test]
    fn foundation_zone_volume_uses_foundation_wall_height() {
        let xml = SAMPLE_XML
            .replace(
                "<FloorArea units=\"ft2\">800</FloorArea>",
                "<FloorArea units=\"ft2\">100</FloorArea>",
            )
            .replace(
                "<FoundationWall>\n            <SystemIdentifier id=\"FoundationWall1\"/>\n            <InteriorAdjacentTo>basement - conditioned</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>ground</ExteriorAdjacentTo>\n            <Area units=\"ft2\">60</Area>\n          </FoundationWall>",
                "<FoundationWall>\n            <SystemIdentifier id=\"FoundationWall1\"/>\n            <InteriorAdjacentTo>basement - conditioned</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>ground</ExteriorAdjacentTo>\n            <Area units=\"ft2\">60</Area>\n            <Height units=\"ft\">3</Height>\n          </FoundationWall>",
            );

        let building = parse_building(&xml).expect("parse should succeed");
        let foundation = building
            .zones
            .iter()
            .find(|z| matches!(z.zone_type, ZoneType::Foundation))
            .expect("foundation zone expected");

        let floor_area_m2 = foundation
            .floor_area_m2
            .expect("foundation floor area expected");
        let expected_volume_m3 = floor_area_m2 * 3.0 * 0.3048;
        let actual_volume_m3 = foundation.volume_m3.expect("foundation volume expected");

        assert!(
            (actual_volume_m3 - expected_volume_m3).abs() < 1e-6,
            "foundation volume: got {}, expected {}",
            actual_volume_m3,
            expected_volume_m3
        );
    }

    #[test]
    fn conditioned_zone_area_excludes_foundation_area_when_basement_present() {
        let xml = SAMPLE_XML
            .replace(
                "</BuildingConstruction>",
                "<NumberofConditionedFloors>2</NumberofConditionedFloors>\n          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>\n        </BuildingConstruction>",
            );

        let building = parse_building(&xml).expect("parse should succeed");
        let conditioned = building
            .zones
            .iter()
            .find(|zone| matches!(zone.zone_type, ZoneType::Conditioned))
            .expect("conditioned zone expected");
        let foundation = building
            .zones
            .iter()
            .find(|zone| matches!(zone.zone_type, ZoneType::Foundation))
            .expect("foundation zone expected");

        let conditioned_area_m2 = conditioned
            .floor_area_m2
            .expect("conditioned area expected");
        let foundation_area_m2 = foundation.floor_area_m2.expect("foundation area expected");
        let expected_conditioned_area_m2 = (2152.0 - 800.0) * 0.092_903_04;
        let expected_foundation_area_m2 = 800.0 * 0.092_903_04;

        assert!(
            (conditioned_area_m2 - expected_conditioned_area_m2).abs() < 1e-6,
            "conditioned area: got {}, expected {}",
            conditioned_area_m2,
            expected_conditioned_area_m2
        );
        assert!(
            (foundation_area_m2 - expected_foundation_area_m2).abs() < 1e-6,
            "foundation area: got {}, expected {}",
            foundation_area_m2,
            expected_foundation_area_m2
        );

        let ceiling_height_m = building.ceiling_height_m.expect("ceiling height expected");
        let conditioned_volume_m3 = conditioned.volume_m3.expect("conditioned volume expected");
        assert!(
            (conditioned_volume_m3 - conditioned_area_m2 * ceiling_height_m).abs() < 1e-6,
            "conditioned volume should follow updated area * ceiling height"
        );
    }

    #[test]
    fn conditioned_zone_area_passthrough_for_slab_without_below_grade_levels() {
        let xml = SAMPLE_XML.replace(
            "<Foundation>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
            "<Foundation>\n            <FoundationType><SlabOnGrade/></FoundationType>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
        );

        let building = parse_building(&xml).expect("parse should succeed");
        let conditioned = building
            .zones
            .iter()
            .find(|zone| matches!(zone.zone_type, ZoneType::Conditioned))
            .expect("conditioned zone expected");

        let conditioned_area_m2 = conditioned
            .floor_area_m2
            .expect("conditioned area expected");
        let expected_conditioned_area_m2 = 2152.0 * 0.092_903_04;

        assert!(
            (conditioned_area_m2 - expected_conditioned_area_m2).abs() < 1e-6,
            "conditioned area: got {}, expected {}",
            conditioned_area_m2,
            expected_conditioned_area_m2
        );
    }

    #[test]
    fn foundation_wall_no_foundation_type_keeps_wall_type() {
        // Without FoundationType, foundation_name is None, so construction_type
        // stays as the WallType (parsed by extract_construction_metadata).
        let building = parse_building(SAMPLE_XML).expect("parse should succeed");
        assert!(building.foundation_name.is_none());
        let fnd_wall = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::FoundationWall)
            .expect("foundation wall expected");
        // No override -- construction_type comes from WallType (None in this XML).
        assert!(fnd_wall.construction_type.is_none());
    }

    #[test]
    fn finished_basement_from_conditioned_element() {
        // HPXML 4.x: <Basement><Conditioned>true</Conditioned></Basement>
        let xml = SAMPLE_XML.replace(
            "<Foundation>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
            "<Foundation>\n            <FoundationType><Basement><Conditioned>true</Conditioned></Basement></FoundationType>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        assert_eq!(
            building.foundation_name.as_deref(),
            Some("Finished Basement")
        );
    }

    #[test]
    fn unfinished_basement_from_conditioned_false() {
        let xml = SAMPLE_XML.replace(
            "<Foundation>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
            "<Foundation>\n            <FoundationType><Basement><Conditioned>false</Conditioned></Basement></FoundationType>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        assert_eq!(
            building.foundation_name.as_deref(),
            Some("Unfinished Basement")
        );
    }

    #[test]
    fn finished_basement_from_floor_count_heuristic() {
        // OCHRE heuristic: total_floors > floors_above_grade → Finished Basement.
        // No <Conditioned> element, so falls back to floor count comparison.
        let xml = SAMPLE_XML
            .replace(
                "<Foundation>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
                "<Foundation>\n            <FoundationType><Basement/></FoundationType>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
            )
            .replace(
                "</BuildingConstruction>",
                "<NumberofConditionedFloors>2</NumberofConditionedFloors>\n          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>\n        </BuildingConstruction>",
            );
        let building = parse_building(&xml).expect("parse should succeed");
        assert_eq!(
            building.foundation_name.as_deref(),
            Some("Finished Basement")
        );
    }

    #[test]
    fn unfinished_basement_from_equal_floor_counts() {
        // total_floors == floors_above_grade → Unfinished Basement.
        let xml = SAMPLE_XML
            .replace(
                "<Foundation>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
                "<Foundation>\n            <FoundationType><Basement/></FoundationType>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
            )
            .replace(
                "</BuildingConstruction>",
                "<NumberofConditionedFloors>1</NumberofConditionedFloors>\n          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>\n        </BuildingConstruction>",
            );
        let building = parse_building(&xml).expect("parse should succeed");
        assert_eq!(
            building.foundation_name.as_deref(),
            Some("Unfinished Basement")
        );
    }

    #[test]
    fn conditioned_element_takes_priority_over_floor_count() {
        // <Conditioned>false</Conditioned> overrides floor count heuristic.
        let xml = SAMPLE_XML
            .replace(
                "<Foundation>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
                "<Foundation>\n            <FoundationType><Basement><Conditioned>false</Conditioned></Basement></FoundationType>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
            )
            .replace(
                "</BuildingConstruction>",
                "<NumberofConditionedFloors>2</NumberofConditionedFloors>\n          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>\n        </BuildingConstruction>",
            );
        let building = parse_building(&xml).expect("parse should succeed");
        // Explicit Conditioned=false wins over floor count heuristic (which would say Finished).
        assert_eq!(
            building.foundation_name.as_deref(),
            Some("Unfinished Basement")
        );
    }

    #[test]
    fn slab_on_grade_foundation_name_is_none() {
        let xml = SAMPLE_XML.replace(
            "<Foundation>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
            "<Foundation>\n            <FoundationType><SlabOnGrade/></FoundationType>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        assert!(building.foundation_name.is_none());
    }

    // ── Slab insulation detail tests ────────────────────────────────────

    #[test]
    fn slab_uninsulated() {
        // Default SAMPLE_XML slab has no insulation elements.
        let building = parse_building(SAMPLE_XML).expect("parse should succeed");
        let slab = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Slab)
            .expect("slab expected");
        assert_eq!(slab.insulation_details.as_deref(), Some("Uninsulated"));
    }

    #[test]
    fn slab_perimeter_insulation() {
        let xml = SAMPLE_XML.replace(
            "<Slab>\n            <SystemIdentifier id=\"Slab1\"/>\n            <InteriorAdjacentTo>basement - conditioned</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>ground</ExteriorAdjacentTo>\n            <Area units=\"ft2\">80</Area>\n          </Slab>",
            "<Slab>\n            <SystemIdentifier id=\"Slab1\"/>\n            <InteriorAdjacentTo>basement - conditioned</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>ground</ExteriorAdjacentTo>\n            <Area units=\"ft2\">80</Area>\n            <PerimeterInsulation><Layer><NominalRValue>10</NominalRValue><InsulationDepth>2</InsulationDepth></Layer></PerimeterInsulation>\n          </Slab>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let slab = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Slab)
            .expect("slab expected");
        assert_eq!(
            slab.insulation_details.as_deref(),
            Some("2ft R10 Perimeter")
        );
    }

    #[test]
    fn slab_underslab_whole() {
        let xml = SAMPLE_XML.replace(
            "<Slab>\n            <SystemIdentifier id=\"Slab1\"/>\n            <InteriorAdjacentTo>basement - conditioned</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>ground</ExteriorAdjacentTo>\n            <Area units=\"ft2\">80</Area>\n          </Slab>",
            "<Slab>\n            <SystemIdentifier id=\"Slab1\"/>\n            <InteriorAdjacentTo>basement - conditioned</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>ground</ExteriorAdjacentTo>\n            <Area units=\"ft2\">80</Area>\n            <UnderSlabInsulation><Layer><NominalRValue>10</NominalRValue><InsulationSpansEntireSlab>true</InsulationSpansEntireSlab></Layer></UnderSlabInsulation>\n          </Slab>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let slab = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Slab)
            .expect("slab expected");
        assert_eq!(slab.insulation_details.as_deref(), Some("R10 Whole Slab"));
    }

    #[test]
    fn slab_underslab_partial() {
        let xml = SAMPLE_XML.replace(
            "<Slab>\n            <SystemIdentifier id=\"Slab1\"/>\n            <InteriorAdjacentTo>basement - conditioned</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>ground</ExteriorAdjacentTo>\n            <Area units=\"ft2\">80</Area>\n          </Slab>",
            "<Slab>\n            <SystemIdentifier id=\"Slab1\"/>\n            <InteriorAdjacentTo>basement - conditioned</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>ground</ExteriorAdjacentTo>\n            <Area units=\"ft2\">80</Area>\n            <UnderSlabInsulation><Layer><NominalRValue>10</NominalRValue><InsulationWidth>4</InsulationWidth></Layer></UnderSlabInsulation>\n          </Slab>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let slab = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Slab)
            .expect("slab expected");
        assert_eq!(slab.insulation_details.as_deref(), Some("4ft R10 Exterior"));
    }

    #[test]
    fn slab_minimal_insulation() {
        let xml = SAMPLE_XML.replace(
            "<Slab>\n            <SystemIdentifier id=\"Slab1\"/>\n            <InteriorAdjacentTo>basement - conditioned</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>ground</ExteriorAdjacentTo>\n            <Area units=\"ft2\">80</Area>\n          </Slab>",
            "<Slab>\n            <SystemIdentifier id=\"Slab1\"/>\n            <InteriorAdjacentTo>basement - conditioned</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>ground</ExteriorAdjacentTo>\n            <Area units=\"ft2\">80</Area>\n            <PerimeterInsulation><Layer><NominalRValue>500</NominalRValue></Layer></PerimeterInsulation>\n            <UnderSlabInsulation><Layer><NominalRValue>500</NominalRValue></Layer></UnderSlabInsulation>\n          </Slab>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let slab = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Slab)
            .expect("slab expected");
        assert_eq!(slab.insulation_details.as_deref(), Some("Minimal"));
    }

    // ── F-factor perimeter parsing regression tests ─────────────────────

    #[test]
    fn slab_exposed_perimeter_parsed_from_hpxml_4x() {
        // Regression: <ExposedPerimeter> (HPXML 4.x) was silently ignored
        // because the code only looked for <Perimeter> (HPXML 3.x).
        // 140.0 ft → 42.672 m.
        let xml = SAMPLE_XML.replace(
            "<Slab>\n            <SystemIdentifier id=\"Slab1\"/>\n            <InteriorAdjacentTo>basement - conditioned</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>ground</ExteriorAdjacentTo>\n            <Area units=\"ft2\">80</Area>\n          </Slab>",
            "<Slab>\n            <SystemIdentifier id=\"Slab1\"/>\n            <InteriorAdjacentTo>basement - conditioned</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>ground</ExteriorAdjacentTo>\n            <Area units=\"ft2\">80</Area>\n            <ExposedPerimeter>140.0</ExposedPerimeter>\n          </Slab>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let slab = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Slab)
            .expect("slab expected");
        let perimeter = slab
            .perimeter_m
            .expect("perimeter should be parsed from <ExposedPerimeter>");
        // 140 ft → 42.672 m (±0.01)
        let expected_m = 140.0 * 0.3048;
        assert!(
            (perimeter - expected_m).abs() < 0.01,
            "expected {expected_m} m, got {perimeter} m"
        );
    }

    #[test]
    fn slab_perimeter_falls_back_to_hpxml_3x_element() {
        // HPXML 3.x uses <Perimeter> instead of <ExposedPerimeter>.
        // The fallback path should still parse it.
        let xml = SAMPLE_XML.replace(
            "<Slab>\n            <SystemIdentifier id=\"Slab1\"/>\n            <InteriorAdjacentTo>basement - conditioned</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>ground</ExteriorAdjacentTo>\n            <Area units=\"ft2\">80</Area>\n          </Slab>",
            "<Slab>\n            <SystemIdentifier id=\"Slab1\"/>\n            <InteriorAdjacentTo>basement - conditioned</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>ground</ExteriorAdjacentTo>\n            <Area units=\"ft2\">80</Area>\n            <Perimeter>100.0</Perimeter>\n          </Slab>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let slab = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Slab)
            .expect("slab expected");
        let perimeter = slab
            .perimeter_m
            .expect("perimeter should be parsed from <Perimeter>");
        let expected_m = 100.0 * 0.3048;
        assert!(
            (perimeter - expected_m).abs() < 0.01,
            "expected {expected_m} m, got {perimeter} m"
        );
    }

    #[test]
    fn slab_perimeter_insulation_r_value_parsed_from_hpxml() {
        // Regression: perimeter_insulation_r_m2_k_w was untested at the
        // HPXML parsing level. NominalRValue=5 IP (ft²·°F·h/Btu)
        // converts to ≈ 0.88 m²·K/W (SI).
        let xml = SAMPLE_XML.replace(
            "<Slab>\n            <SystemIdentifier id=\"Slab1\"/>\n            <InteriorAdjacentTo>basement - conditioned</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>ground</ExteriorAdjacentTo>\n            <Area units=\"ft2\">80</Area>\n          </Slab>",
            "<Slab>\n            <SystemIdentifier id=\"Slab1\"/>\n            <InteriorAdjacentTo>basement - conditioned</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>ground</ExteriorAdjacentTo>\n            <Area units=\"ft2\">80</Area>\n            <PerimeterInsulation><Layer><NominalRValue>5</NominalRValue><InsulationDepth>2</InsulationDepth></Layer></PerimeterInsulation>\n          </Slab>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let slab = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Slab)
            .expect("slab expected");
        let r_si = slab
            .perimeter_insulation_r_m2_k_w
            .expect("perimeter insulation R-value should be parsed");
        // R-5 IP ≈ 0.88 m²·K/W (±0.02)
        let expected_r_si = 0.88;
        assert!(
            (r_si - expected_r_si).abs() < 0.02,
            "expected R-5 IP ≈ {expected_r_si} m²·K/W, got {r_si}"
        );
        assert_eq!(
            slab.insulation_details.as_deref(),
            Some("2ft R5 Perimeter"),
            "insulation_details should identify perimeter-only insulation"
        );
    }

    // ── Furniture boundary auto-generation tests ────────────────────────

    #[test]
    fn furniture_boundaries_generated_for_conditioned_zone() {
        let building = parse_building(SAMPLE_XML).expect("parse should succeed");
        let furniture: Vec<_> = building
            .boundaries
            .iter()
            .filter(|b| b.id.contains("furniture"))
            .collect();
        // Conditioned zone has floor area → generates furniture boundary.
        assert!(
            furniture.iter().any(|b| b.id == "conditioned_furniture"),
            "conditioned furniture boundary expected"
        );
        let cond_furn = furniture
            .iter()
            .find(|b| b.id == "conditioned_furniture")
            .unwrap();
        // Same-zone: interior == exterior.
        assert_eq!(cond_furn.interior_zone, Some(ZoneType::Conditioned));
        assert_eq!(cond_furn.exterior_zone, Some(ZoneType::Conditioned));
        // Area = floor_area × 0.4.
        let expected_floor_m2 = 2152.0 * 0.092_903_04;
        assert!(
            (cond_furn.area_m2 - expected_floor_m2 * 0.4).abs() < 0.1,
            "furniture area: got {}, expected {}",
            cond_furn.area_m2,
            expected_floor_m2 * 0.4
        );
    }

    #[test]
    fn furniture_boundary_for_garage() {
        let building = parse_building(SAMPLE_XML).expect("parse should succeed");
        let garage_furn = building
            .boundaries
            .iter()
            .find(|b| b.id == "garage_furniture");
        assert!(garage_furn.is_some(), "garage furniture boundary expected");
        let gf = garage_furn.unwrap();
        assert_eq!(gf.interior_zone, Some(ZoneType::Garage));
        assert_eq!(gf.exterior_zone, Some(ZoneType::Garage));
        // Area = 400 ft² × 0.0929 × 0.1
        let expected = 400.0 * 0.092_903_04 * 0.1;
        assert!(
            (gf.area_m2 - expected).abs() < 0.1,
            "garage furniture area: got {}, expected {}",
            gf.area_m2,
            expected
        );
    }

    #[test]
    fn furniture_boundary_for_foundation() {
        let building = parse_building(SAMPLE_XML).expect("parse should succeed");
        let fnd_furn = building
            .boundaries
            .iter()
            .find(|b| b.id == "foundation_furniture");
        assert!(fnd_furn.is_some(), "foundation furniture boundary expected");
        let ff = fnd_furn.unwrap();
        assert_eq!(ff.interior_zone, Some(ZoneType::Foundation));
        assert_eq!(ff.exterior_zone, Some(ZoneType::Foundation));
        // Area = 800 ft² × 0.0929 × 0.4
        let expected = 800.0 * 0.092_903_04 * 0.4;
        assert!(
            (ff.area_m2 - expected).abs() < 0.1,
            "foundation furniture area: got {}, expected {}",
            ff.area_m2,
            expected
        );
    }

    #[test]
    fn no_attic_furniture_boundary() {
        let building = parse_building(SAMPLE_XML).expect("parse should succeed");
        let attic_furn = building
            .boundaries
            .iter()
            .find(|b| b.id == "attic_furniture");
        assert!(
            attic_furn.is_none(),
            "attic should not have furniture boundary"
        );
    }

    #[test]
    fn adjacent_zone_label_parsed() {
        // "other housing unit" should parse to ZoneType::Adjacent.
        // Test via a boundary with InteriorAdjacentTo="other housing unit".
        let xml = SAMPLE_XML.replace(
            "<InteriorAdjacentTo>conditioned space</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>",
            "<InteriorAdjacentTo>conditioned space</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>other housing unit</ExteriorAdjacentTo>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let wall = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Wall && b.id == "Wall1")
            .expect("wall expected");
        assert_eq!(wall.exterior_zone, Some(ZoneType::Adjacent));
    }

    // ── Insulation details dispatch tests ───────────────────────────────

    #[test]
    fn wall_has_no_insulation_details() {
        let building = parse_building(SAMPLE_XML).expect("parse should succeed");
        let wall = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Wall)
            .expect("wall expected");
        assert!(
            wall.insulation_details.is_none(),
            "regular walls should not have insulation_details, got {:?}",
            wall.insulation_details
        );
    }

    #[test]
    fn roof_has_no_insulation_details() {
        let building = parse_building(SAMPLE_XML).expect("parse should succeed");
        let roof = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Roof)
            .expect("roof expected");
        assert!(
            roof.insulation_details.is_none(),
            "roofs should not have insulation_details, got {:?}",
            roof.insulation_details
        );
    }

    #[test]
    fn door_has_no_insulation_details() {
        let building = parse_building(SAMPLE_XML).expect("parse should succeed");
        for bd in &building.boundaries {
            if bd.boundary_type == BoundaryType::Door {
                assert!(
                    bd.insulation_details.is_none(),
                    "doors should not have insulation_details"
                );
            }
        }
    }

    // ── Wall area reduction tests ───────────────────────────────────────

    #[test]
    fn wall_area_reduced_by_window() {
        // SAMPLE_XML: Wall1 = 100 ft² = 9.290304 m², Window1 = 15 ft² = 1.393546 m²
        // Wall area should be reduced by window area.
        let building = parse_building(SAMPLE_XML).expect("parse should succeed");
        let wall = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Wall && b.id == "Wall1")
            .expect("wall expected");
        let original = 100.0 * 0.092_903_04;
        let window = 15.0 * 0.092_903_04;
        assert!(
            (wall.area_m2 - (original - window)).abs() < 0.01,
            "wall area should be reduced: got {}, expected {}",
            wall.area_m2,
            original - window
        );
    }

    #[test]
    fn wall_without_openings_keeps_area() {
        // Remove the window from SAMPLE_XML entirely.
        let xml = SAMPLE_XML.replace(
            "        <Windows>\n          <Window>\n            <SystemIdentifier id=\"Window1\"/>\n            <InteriorAdjacentTo>conditioned space</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>\n            <Area units=\"ft2\">15</Area>\n            <Azimuth>180</Azimuth>\n            <UFactor>0.31</UFactor>\n            <SHGC>0.25</SHGC>\n            <FrameType>vinyl</FrameType>\n            <AttachedToWall idref=\"Wall1\"/>\n          </Window>\n        </Windows>",
            "",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let wall = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Wall && b.id == "Wall1")
            .expect("wall expected");
        let original = 100.0 * 0.092_903_04;
        assert!(
            (wall.area_m2 - original).abs() < 0.01,
            "wall area unchanged without windows: got {}, expected {}",
            wall.area_m2,
            original
        );
    }

    #[test]
    fn interior_wall_has_correct_lut_name() {
        let building = parse_building(SAMPLE_XML).expect("parse should succeed");
        let iw = building
            .boundaries
            .iter()
            .find(|b| b.id == "interior_wall")
            .expect("interior wall expected");
        assert_eq!(iw.lut_boundary_name.as_deref(), Some("Interior Wall"));
        assert_eq!(iw.interior_zone, Some(ZoneType::Conditioned));
        assert_eq!(iw.exterior_zone, Some(ZoneType::Conditioned));
    }

    #[test]
    fn furniture_has_correct_lut_name() {
        let building = parse_building(SAMPLE_XML).expect("parse should succeed");
        let cf = building
            .boundaries
            .iter()
            .find(|b| b.id == "conditioned_furniture")
            .expect("conditioned furniture expected");
        assert_eq!(cf.lut_boundary_name.as_deref(), Some("Indoor Furniture"));

        let gf = building
            .boundaries
            .iter()
            .find(|b| b.id == "garage_furniture")
            .expect("garage furniture expected");
        assert_eq!(gf.lut_boundary_name.as_deref(), Some("Garage Furniture"));

        let ff = building
            .boundaries
            .iter()
            .find(|b| b.id == "foundation_furniture")
            .expect("foundation furniture expected");
        assert_eq!(
            ff.lut_boundary_name.as_deref(),
            Some("Foundation Furniture")
        );
    }

    #[test]
    fn hpxml_boundaries_have_no_lut_override() {
        let building = parse_building(SAMPLE_XML).expect("parse should succeed");
        for bd in &building.boundaries {
            if !bd.id.contains("furniture") && bd.id != "interior_wall" {
                assert!(
                    bd.lut_boundary_name.is_none(),
                    "HPXML boundary {} should not have lut_boundary_name override",
                    bd.id
                );
            }
        }
    }

    #[test]
    fn window_fraction_operable_parsed() {
        let building = parse_building(SAMPLE_XML).expect("parse should succeed");
        let w = &building.windows[0];
        assert!(
            (w.fraction_operable - 0.67).abs() < f64::EPSILON,
            "default fraction_operable should be 0.67, got {}",
            w.fraction_operable,
        );
    }

    #[test]
    fn window_fraction_operable_custom() {
        let xml = SAMPLE_XML.replace(
            "<SHGC>0.25</SHGC>",
            "<SHGC>0.25</SHGC>\n            <FractionOperable>0.33</FractionOperable>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let w = &building.windows[0];
        assert!(
            (w.fraction_operable - 0.33).abs() < f64::EPSILON,
            "custom fraction_operable, got {}",
            w.fraction_operable,
        );
    }

    #[test]
    fn window_fraction_operable_half() {
        let xml = SAMPLE_XML.replace(
            "<SHGC>0.25</SHGC>",
            "<SHGC>0.25</SHGC>\n            <FractionOperable>0.5</FractionOperable>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let w = &building.windows[0];
        assert!(
            (w.fraction_operable - 0.5).abs() < f64::EPSILON,
            "FractionOperable=0.5 must be parsed exactly, got {}",
            w.fraction_operable,
        );
    }

    #[test]
    fn window_winter_shading_coefficient_parsed() {
        let building = parse_building(SAMPLE_XML).expect("parse should succeed");
        let w = &building.windows[0];
        // No InteriorShading in SAMPLE_XML → defaults to 1.0.
        assert!(
            (w.winter_shading_fraction - 1.0).abs() < f64::EPSILON,
            "no shading → winter=1.0, got {}",
            w.winter_shading_fraction,
        );
    }

    #[test]
    fn window_winter_shading_with_element() {
        let xml = SAMPLE_XML.replace(
            "<SHGC>0.25</SHGC>",
            "<SHGC>0.25</SHGC>\n            <InteriorShading>\n              <SummerShadingCoefficient>0.70</SummerShadingCoefficient>\n              <WinterShadingCoefficient>0.85</WinterShadingCoefficient>\n            </InteriorShading>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let w = &building.windows[0];
        assert!(
            (w.interior_shading_fraction - 0.70).abs() < f64::EPSILON,
            "summer shading, got {}",
            w.interior_shading_fraction,
        );
        assert!(
            (w.winter_shading_fraction - 0.85).abs() < f64::EPSILON,
            "winter shading, got {}",
            w.winter_shading_fraction,
        );
    }

    #[test]
    fn window_exterior_shading_defaults_to_1() {
        let xml = xml_with_window(
            r#"<Window>
                <SystemIdentifier id="W1"/>
                <Area>10</Area>
                <SHGC>0.40</SHGC>
            </Window>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let w = &building.windows[0];
        assert!(
            (w.exterior_shading_summer - 1.0).abs() < f64::EPSILON,
            "no ExteriorShading → summer=1.0, got {}",
            w.exterior_shading_summer,
        );
        assert!(
            (w.exterior_shading_winter - 1.0).abs() < f64::EPSILON,
            "no ExteriorShading → winter=1.0, got {}",
            w.exterior_shading_winter,
        );
    }

    #[test]
    fn window_exterior_shading_parsed() {
        let xml = xml_with_window(
            r#"<Window>
                <SystemIdentifier id="W1"/>
                <Area>10</Area>
                <SHGC>0.40</SHGC>
                <ExteriorShading>
                    <SystemIdentifier id="W1ExtShade"/>
                    <SummerShadingCoefficient>0.50</SummerShadingCoefficient>
                    <WinterShadingCoefficient>0.80</WinterShadingCoefficient>
                </ExteriorShading>
            </Window>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let w = &building.windows[0];
        assert!(
            (w.exterior_shading_summer - 0.50).abs() < f64::EPSILON,
            "exterior summer=0.50, got {}",
            w.exterior_shading_summer,
        );
        assert!(
            (w.exterior_shading_winter - 0.80).abs() < f64::EPSILON,
            "exterior winter=0.80, got {}",
            w.exterior_shading_winter,
        );
    }

    #[test]
    fn window_exterior_shading_effective_shgc() {
        let xml = xml_with_window(
            r#"<Window>
                <SystemIdentifier id="W1"/>
                <Area>10</Area>
                <SHGC>0.40</SHGC>
                <InteriorShading>
                    <SystemIdentifier id="W1IntShade"/>
                    <SummerShadingCoefficient>0.70</SummerShadingCoefficient>
                    <WinterShadingCoefficient>0.85</WinterShadingCoefficient>
                </InteriorShading>
                <ExteriorShading>
                    <SystemIdentifier id="W1ExtShade"/>
                    <SummerShadingCoefficient>0.50</SummerShadingCoefficient>
                    <WinterShadingCoefficient>0.80</WinterShadingCoefficient>
                </ExteriorShading>
            </Window>"#,
        );
        let building = parse_building(&xml).expect("should parse");
        let w = &building.windows[0];
        let base_shgc = w.shgc.unwrap();
        let effective_summer = base_shgc * w.interior_shading_fraction * w.exterior_shading_summer;
        let effective_winter = base_shgc * w.winter_shading_fraction * w.exterior_shading_winter;
        // 0.40 * 0.70 * 0.50 = 0.14
        assert!(
            (effective_summer - 0.14).abs() < 1e-10,
            "effective summer SHGC = 0.40*0.70*0.50 = 0.14, got {}",
            effective_summer,
        );
        // 0.40 * 0.85 * 0.80 = 0.272
        assert!(
            (effective_winter - 0.272).abs() < 1e-10,
            "effective winter SHGC = 0.40*0.85*0.80 = 0.272, got {}",
            effective_winter,
        );
    }

    #[test]
    fn floor_or_ceiling_parsed() {
        // SAMPLE_XML doesn't have a <Floor> element at the right level,
        // but we can test the parsing via a FrameFloor with FloorOrCeiling.
        // Verify the field exists on parsed boundaries.
        let building = parse_building(SAMPLE_XML).expect("parse should succeed");
        // No FloorOrCeiling in SAMPLE_XML → all boundaries have None.
        for bd in &building.boundaries {
            if !bd.id.contains("furniture") && bd.id != "interior_wall" {
                assert!(
                    bd.floor_or_ceiling.is_none(),
                    "boundary {} should have no floor_or_ceiling without element",
                    bd.id
                );
            }
        }
    }

    #[test]
    fn residential_facility_type_parsed() {
        let xml = SAMPLE_XML.replace(
            "</BuildingConstruction>",
            "<ResidentialFacilityType>single-family detached</ResidentialFacilityType>\n        </BuildingConstruction>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        assert_eq!(
            building.residential_facility_type.as_deref(),
            Some("single-family detached")
        );
    }

    #[test]
    fn residential_facility_type_absent() {
        let building = parse_building(SAMPLE_XML).expect("parse should succeed");
        assert!(building.residential_facility_type.is_none());
    }

    #[test]
    fn infiltration_ach50_parsed_from_sample() {
        let building = parse_building(SAMPLE_XML).expect("parse should succeed");
        assert_eq!(building.infiltration_ach50, Some(5.0));
        assert!(building.infiltration_cfm50.is_none());
        assert!(building.infiltration_ela_cm2.is_none());
    }

    #[test]
    fn infiltration_cfm50_parsed_from_unitofmeasure_cfm() {
        let xml = SAMPLE_XML.replace(
            "<AirLeakage>5.0</AirLeakage>",
            "<BuildingAirLeakage>\
               <UnitofMeasure>CFM</UnitofMeasure>\
               <AirLeakage>850.0</AirLeakage>\
             </BuildingAirLeakage>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        assert_eq!(building.infiltration_cfm50, Some(850.0));
        assert_eq!(
            building.infiltration_ach50, None,
            "CFM UnitofMeasure must not populate infiltration_ach50"
        );
    }

    #[test]
    fn infiltration_cfm50_parsed_from_units_attribute() {
        let xml = SAMPLE_XML.replace(
            "<AirLeakage>5.0</AirLeakage>",
            "<AirLeakage units=\"CFM50\">750.0</AirLeakage>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        assert_eq!(building.infiltration_cfm50, Some(750.0));
    }

    #[test]
    fn infiltration_ela_parsed_from_sq_in() {
        let xml = SAMPLE_XML.replace(
            "<AirLeakage>5.0</AirLeakage>",
            "<EffectiveLeakageArea units=\"sq-in\">10.0</EffectiveLeakageArea>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        // 10 sq-in × 6.4516 = 64.516 cm²
        let ela = building.infiltration_ela_cm2.expect("ELA should be parsed");
        assert!(
            (ela - 64.516).abs() < 0.001,
            "10 sq-in should convert to 64.516 cm², got {ela}"
        );
    }

    #[test]
    fn infiltration_ela_absent_when_not_present() {
        let building = parse_building(SAMPLE_XML).expect("parse should succeed");
        assert!(building.infiltration_ela_cm2.is_none());
    }

    #[test]
    fn assembly_r_value_nested_under_insulation_is_parsed() {
        // Standard HPXML nests <AssemblyEffectiveRValue> under <Insulation>, not as a
        // direct child of the wall element.  The fix changed child() to first_descendant()
        // at line 910 so this value is no longer silently dropped.
        let xml = SAMPLE_XML.replace(
            "<Insulation>\n              <Layer>\n                <Thickness units=\"in\">5.5</Thickness>\n                <NominalRValue>19</NominalRValue>\n                <Density units=\"lb/ft3\">0.5</Density>\n                <SpecificHeat units=\"Btu/lb-F\">0.2</SpecificHeat>\n              </Layer>\n            </Insulation>",
            "<Insulation>\n              <AssemblyEffectiveRValue Units=\"hr-ft2-F/Btu\">13.0</AssemblyEffectiveRValue>\n              <Layer>\n                <Thickness units=\"in\">5.5</Thickness>\n                <NominalRValue>19</NominalRValue>\n                <Density units=\"lb/ft3\">0.5</Density>\n                <SpecificHeat units=\"Btu/lb-F\">0.2</SpecificHeat>\n              </Layer>\n            </Insulation>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let wall = building
            .boundaries
            .iter()
            .find(|b| matches!(b.boundary_type, BoundaryType::Wall))
            .expect("wall boundary expected");
        let r_si = wall
            .assembly_r_value_m2_k_w
            .expect("assembly_r_value_m2_k_w should be Some when nested under <Insulation>");
        // 13.0 hr·ft²·°F/Btu × 0.176110 = 2.28943 m²·K/W
        let expected = 13.0 * 0.176_110;
        assert!(
            (r_si - expected).abs() < 1e-3,
            "expected ~{expected} m²·K/W, got {r_si}",
        );
    }

    // ── Garage geometry tests ─────────────────────────────────────────

    #[test]
    fn garage_protruded_area_two_attached_walls() {
        // Two perpendicular exterior walls (N/S at 6m², E/W at 8m²) on a 12m² garage.
        // garage_wall_height = sqrt(6 * 8 / 12) = 2.0 m
        // Two attached walls (3m² and 4m²): garage_area_in_main = 3*4/4 = 3.0 m²
        // protruded = 12 - 3 = 9 m²
        use super::{Boundary, BoundaryType, ZoneType, compute_garage_geometry};
        fn wall(interior: ZoneType, exterior: ZoneType, area: f64, az: f64) -> Boundary {
            Boundary {
                id: String::new(),
                boundary_type: BoundaryType::Wall,
                area_m2: area,
                azimuth_deg: Some(az),
                assembly_r_value_m2_k_w: None,
                r_value_layers_m2_k_w: Vec::new(),
                interior_zone: Some(interior),
                exterior_zone: Some(exterior),
                material_layers: Vec::new(),
                framing_factor: None,
                construction_type: None,
                finish_type: None,
                insulation_details: None,
                has_radiant_barrier: false,
                solar_absorptance: None,
                emittance: None,
                tilt_deg: Some(90.0),
                lut_boundary_name: None,
                floor_or_ceiling: None,
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            }
        }
        let boundaries = vec![
            // Exterior garage walls: two perpendicular pairs
            wall(ZoneType::Garage, ZoneType::Outdoor, 6.0, 0.0), // N
            wall(ZoneType::Garage, ZoneType::Outdoor, 8.0, 90.0), // E
            // Attached walls (Conditioned→Garage)
            wall(ZoneType::Conditioned, ZoneType::Garage, 3.0, 180.0), // S
            wall(ZoneType::Conditioned, ZoneType::Garage, 4.0, 270.0), // W
        ];
        let gg = compute_garage_geometry(&boundaries, 12.0).expect("geometry should compute");
        let wall_height = (6.0 * 8.0 / 12.0_f64).sqrt();
        assert!((gg.wall_height_m - wall_height).abs() < 1e-6, "wall_height");
        let area_in_main = 3.0 * 4.0 / (wall_height * wall_height);
        let expected_protruded = 12.0 - area_in_main;
        assert!(
            (gg.protruded_area_m2 - expected_protruded).abs() < 1e-6,
            "protruded: got {}, expected {expected_protruded}",
            gg.protruded_area_m2
        );
    }

    #[test]
    fn garage_protruded_area_single_attached_wall() {
        // Single attached wall → garage_area_in_main = 0, protruded = floor_area.
        use super::{Boundary, BoundaryType, ZoneType, compute_garage_geometry};
        fn wall(interior: ZoneType, exterior: ZoneType, area: f64, az: f64) -> Boundary {
            Boundary {
                id: String::new(),
                boundary_type: BoundaryType::Wall,
                area_m2: area,
                azimuth_deg: Some(az),
                assembly_r_value_m2_k_w: None,
                r_value_layers_m2_k_w: Vec::new(),
                interior_zone: Some(interior),
                exterior_zone: Some(exterior),
                material_layers: Vec::new(),
                framing_factor: None,
                construction_type: None,
                finish_type: None,
                insulation_details: None,
                has_radiant_barrier: false,
                solar_absorptance: None,
                emittance: None,
                tilt_deg: Some(90.0),
                lut_boundary_name: None,
                floor_or_ceiling: None,
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            }
        }
        let boundaries = vec![
            wall(ZoneType::Garage, ZoneType::Outdoor, 6.0, 0.0),
            wall(ZoneType::Garage, ZoneType::Outdoor, 8.0, 90.0),
            wall(ZoneType::Conditioned, ZoneType::Garage, 5.0, 180.0),
        ];
        let gg = compute_garage_geometry(&boundaries, 12.0).expect("geometry should compute");
        assert!(
            (gg.protruded_area_m2 - 12.0).abs() < 1e-6,
            "single attached wall → protruded = floor_area, got {}",
            gg.protruded_area_m2
        );
    }

    #[test]
    fn garage_protruded_area_three_attached_walls() {
        // 3 attached walls (2 regular + 1 gable), typical 2-story with protruding garage.
        use super::{Boundary, BoundaryType, ZoneType, compute_garage_geometry};
        fn wall(interior: ZoneType, exterior: ZoneType, area: f64, az: f64) -> Boundary {
            Boundary {
                id: String::new(),
                boundary_type: BoundaryType::Wall,
                area_m2: area,
                azimuth_deg: Some(az),
                assembly_r_value_m2_k_w: None,
                r_value_layers_m2_k_w: Vec::new(),
                interior_zone: Some(interior),
                exterior_zone: Some(exterior),
                material_layers: Vec::new(),
                framing_factor: None,
                construction_type: None,
                finish_type: None,
                insulation_details: None,
                has_radiant_barrier: false,
                solar_absorptance: None,
                emittance: None,
                tilt_deg: Some(90.0),
                lut_boundary_name: None,
                floor_or_ceiling: None,
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            }
        }
        let garage_area = 30.0;
        // Exterior: 10m² at 0°, 15m² at 90°
        // wall_height = sqrt(10*15/30) = sqrt(5) ≈ 2.236
        let wh = (10.0_f64 * 15.0 / garage_area).sqrt();
        let boundaries = vec![
            wall(ZoneType::Garage, ZoneType::Outdoor, 10.0, 0.0),
            wall(ZoneType::Garage, ZoneType::Outdoor, 15.0, 90.0),
            // 3 attached walls: two at 0° (7m²), one at 90° (5m²)
            wall(ZoneType::Conditioned, ZoneType::Garage, 7.0, 0.0),
            wall(ZoneType::Conditioned, ZoneType::Garage, 3.0, 0.0),
            wall(ZoneType::Conditioned, ZoneType::Garage, 5.0, 90.0),
        ];
        let gg =
            compute_garage_geometry(&boundaries, garage_area).expect("geometry should compute");
        // max per-azimuth: 0° → max(7,3) = 7, 90° → 5
        let area_in_main = 7.0 * 5.0 / (wh * wh);
        let expected_protruded = garage_area - area_in_main;
        assert!(
            (gg.protruded_area_m2 - expected_protruded).abs() < 1e-6,
            "protruded: got {}, expected {expected_protruded}",
            gg.protruded_area_m2
        );
    }

    // ── Attic volume tests ──────────────────────────────────────────────

    #[test]
    fn attic_volume_simple_2_gable() {
        use super::{Boundary, BoundaryType, ZoneType, compute_attic_volume};
        fn boundary(
            bt: BoundaryType,
            interior: ZoneType,
            exterior: ZoneType,
            area: f64,
            tilt: Option<f64>,
        ) -> Boundary {
            Boundary {
                id: String::new(),
                boundary_type: bt,
                area_m2: area,
                azimuth_deg: None,
                assembly_r_value_m2_k_w: None,
                r_value_layers_m2_k_w: Vec::new(),
                interior_zone: Some(interior),
                exterior_zone: Some(exterior),
                material_layers: Vec::new(),
                framing_factor: None,
                construction_type: None,
                finish_type: None,
                insulation_details: None,
                has_radiant_barrier: false,
                solar_absorptance: None,
                emittance: None,
                tilt_deg: tilt,
                lut_boundary_name: None,
                floor_or_ceiling: None,
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            }
        }
        // 6:12 pitch → tilt = atan(0.5) ≈ 26.565°
        let tilt_deg = (6.0_f64 / 12.0).atan().to_degrees();
        let gable_area = 10.0; // m²
        let floor_area = 100.0; // m²
        let boundaries = vec![
            boundary(
                BoundaryType::Roof,
                ZoneType::Attic,
                ZoneType::Outdoor,
                50.0,
                Some(tilt_deg),
            ),
            boundary(
                BoundaryType::Wall,
                ZoneType::Attic,
                ZoneType::Outdoor,
                gable_area,
                None,
            ),
            boundary(
                BoundaryType::Wall,
                ZoneType::Attic,
                ZoneType::Outdoor,
                gable_area,
                None,
            ),
        ];
        let vol = compute_attic_volume(&boundaries, Some(floor_area), None)
            .expect("volume should compute");
        let attic_height = (gable_area * tilt_deg.to_radians().tan()).sqrt();
        let expected = 0.5 * floor_area * attic_height;
        assert!(
            (vol - expected).abs() < 1e-6,
            "simple 2-gable: got {vol}, expected {expected}"
        );
    }

    #[test]
    fn attic_volume_3_gable_compound() {
        use super::{Boundary, BoundaryType, GarageGeometry, ZoneType, compute_attic_volume};
        fn boundary(
            bt: BoundaryType,
            interior: ZoneType,
            exterior: ZoneType,
            area: f64,
            tilt: Option<f64>,
        ) -> Boundary {
            Boundary {
                id: String::new(),
                boundary_type: bt,
                area_m2: area,
                azimuth_deg: None,
                assembly_r_value_m2_k_w: None,
                r_value_layers_m2_k_w: Vec::new(),
                interior_zone: Some(interior),
                exterior_zone: Some(exterior),
                material_layers: Vec::new(),
                framing_factor: None,
                construction_type: None,
                finish_type: None,
                insulation_details: None,
                has_radiant_barrier: false,
                solar_absorptance: None,
                emittance: None,
                tilt_deg: tilt,
                lut_boundary_name: None,
                floor_or_ceiling: None,
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            }
        }
        // Regression guard: verifies compound attic volume formula against
        // hand-calculated expected values; not a physics oracle.
        //
        // 6:12 pitch for both attic and garage roofs
        let tilt_deg = (6.0_f64 / 12.0).atan().to_degrees();
        let tilt_rad = tilt_deg.to_radians();

        // 3 gable walls in parse order: [10.0, 12.0, 5.0]
        // OCHRE picks index 1 = 12.0 as attic_gable_area.
        // sorted → [5.0, 10.0, 12.0]; med=10, low=5, high=12
        // med-low=5 > high-med=2 → third_gable = low = 5.0
        let boundaries = vec![
            boundary(
                BoundaryType::Roof,
                ZoneType::Attic,
                ZoneType::Outdoor,
                80.0,
                Some(tilt_deg),
            ),
            boundary(
                BoundaryType::Roof,
                ZoneType::Garage,
                ZoneType::Outdoor,
                20.0,
                Some(tilt_deg),
            ),
            boundary(
                BoundaryType::Wall,
                ZoneType::Attic,
                ZoneType::Outdoor,
                10.0,
                None,
            ),
            boundary(
                BoundaryType::Wall,
                ZoneType::Attic,
                ZoneType::Outdoor,
                12.0,
                None,
            ),
            boundary(
                BoundaryType::Wall,
                ZoneType::Attic,
                ZoneType::Outdoor,
                5.0,
                None,
            ),
        ];
        let garage_geom = GarageGeometry {
            floor_area_m2: 30.0,
            wall_height_m: 2.5,
            protruded_area_m2: 15.0,
        };
        let floor_area = 120.0;
        let vol = compute_attic_volume(&boundaries, Some(floor_area), Some(&garage_geom))
            .expect("volume should compute");

        // Expected compound formula using index-1 gable (12.0 m²):
        let attic_gable_area = 12.0; // gable_areas[1] per OCHRE convention
        let attic_height = (attic_gable_area * tilt_rad.tan()).sqrt();
        let third_gable_area = 5.0;
        let garage_height = (third_gable_area * tilt_rad.tan()).sqrt();
        let garage_width = 2.0 * third_gable_area / garage_height;
        let garage_depth_in_house = garage_height * tilt_rad.tan();
        let square_area = floor_area - garage_geom.protruded_area_m2;
        let expected = 0.5 * square_area * attic_height
            + 0.5 * garage_geom.protruded_area_m2 * garage_height
            + (1.0 / 6.0) * garage_width * garage_depth_in_house * garage_height;
        assert!(
            (vol - expected).abs() < 1e-4,
            "3-gable compound: got {vol}, expected {expected}"
        );

        // Hand-calculated concrete value:
        // tan(26.565°) = 0.5, attic_gable=12 → h_a = sqrt(12*0.5) = sqrt(6) ≈ 2.449
        // third_gable=5 → h_g = sqrt(5*0.5) = sqrt(2.5) ≈ 1.581
        // garage_width = 2*5/1.581 ≈ 6.325
        // garage_depth = 1.581*0.5 ≈ 0.791
        // square_area = 120 - 15 = 105
        // vol = 0.5*105*2.449 + 0.5*15*1.581 + (1/6)*6.325*0.791*1.581
        //     ≈ 128.603 + 11.859 + 1.319 ≈ 141.781
        assert!(
            (vol - 141.781).abs() < 0.1,
            "3-gable compound hand-calc: got {vol}, expected ~141.781"
        );
    }

    #[test]
    fn attic_volume_3_gable_index1_differs_from_median() {
        // Verifies that index-1 (OCHRE convention) is used, not the sorted median.
        // gable_areas in parse order: [15.0, 7.0, 10.0]
        //   index-1 = 7.0
        //   sorted = [7, 10, 15], median = 10.0 (differs from index-1)
        use super::{Boundary, BoundaryType, GarageGeometry, ZoneType, compute_attic_volume};
        fn boundary(
            bt: BoundaryType,
            interior: ZoneType,
            exterior: ZoneType,
            area: f64,
            tilt: Option<f64>,
        ) -> Boundary {
            Boundary {
                id: String::new(),
                boundary_type: bt,
                area_m2: area,
                azimuth_deg: None,
                assembly_r_value_m2_k_w: None,
                r_value_layers_m2_k_w: Vec::new(),
                interior_zone: Some(interior),
                exterior_zone: Some(exterior),
                material_layers: Vec::new(),
                framing_factor: None,
                construction_type: None,
                finish_type: None,
                insulation_details: None,
                has_radiant_barrier: false,
                solar_absorptance: None,
                emittance: None,
                tilt_deg: tilt,
                lut_boundary_name: None,
                floor_or_ceiling: None,
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            }
        }
        let tilt_deg = (6.0_f64 / 12.0).atan().to_degrees();
        let tilt_rad = tilt_deg.to_radians();
        let boundaries = vec![
            boundary(
                BoundaryType::Roof,
                ZoneType::Attic,
                ZoneType::Outdoor,
                80.0,
                Some(tilt_deg),
            ),
            boundary(
                BoundaryType::Roof,
                ZoneType::Garage,
                ZoneType::Outdoor,
                20.0,
                Some(tilt_deg),
            ),
            boundary(
                BoundaryType::Wall,
                ZoneType::Attic,
                ZoneType::Outdoor,
                15.0,
                None,
            ),
            boundary(
                BoundaryType::Wall,
                ZoneType::Attic,
                ZoneType::Outdoor,
                7.0,
                None,
            ),
            boundary(
                BoundaryType::Wall,
                ZoneType::Attic,
                ZoneType::Outdoor,
                10.0,
                None,
            ),
        ];
        let garage_geom = GarageGeometry {
            floor_area_m2: 30.0,
            wall_height_m: 2.5,
            protruded_area_m2: 15.0,
        };
        let floor_area = 120.0;
        let vol = compute_attic_volume(&boundaries, Some(floor_area), Some(&garage_geom))
            .expect("volume should compute");

        // attic_gable_area = gable_areas[1] = 7.0 (NOT sorted median 10.0)
        // sorted = [7, 10, 15]; med=10, low=7, high=15
        // med-low=3, high-med=5 → third_gable = high = 15.0
        let attic_gable_area = 7.0;
        let third_gable_area = 15.0;
        let attic_height = (attic_gable_area * tilt_rad.tan()).sqrt();
        let garage_height = (third_gable_area * tilt_rad.tan()).sqrt();
        let garage_width = 2.0 * third_gable_area / garage_height;
        let garage_depth_in_house = garage_height * tilt_rad.tan();
        let square_area = floor_area - garage_geom.protruded_area_m2;
        let expected = 0.5 * square_area * attic_height
            + 0.5 * garage_geom.protruded_area_m2 * garage_height
            + (1.0 / 6.0) * garage_width * garage_depth_in_house * garage_height;
        assert!(
            (vol - expected).abs() < 1e-4,
            "index-1 vs median: got {vol}, expected {expected}"
        );
    }

    #[test]
    fn attic_volume_path_a_attic_garage_wall() {
        use super::{Boundary, BoundaryType, ZoneType, compute_attic_volume};
        fn boundary(
            bt: BoundaryType,
            interior: ZoneType,
            exterior: ZoneType,
            area: f64,
            tilt: Option<f64>,
        ) -> Boundary {
            Boundary {
                id: String::new(),
                boundary_type: bt,
                area_m2: area,
                azimuth_deg: None,
                assembly_r_value_m2_k_w: None,
                r_value_layers_m2_k_w: Vec::new(),
                interior_zone: Some(interior),
                exterior_zone: Some(exterior),
                material_layers: Vec::new(),
                framing_factor: None,
                construction_type: None,
                finish_type: None,
                insulation_details: None,
                has_radiant_barrier: false,
                solar_absorptance: None,
                emittance: None,
                tilt_deg: tilt,
                lut_boundary_name: None,
                floor_or_ceiling: None,
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            }
        }
        let tilt_deg = (6.0_f64 / 12.0).atan().to_degrees();
        let tilt_rad = tilt_deg.to_radians();
        // Path A: Attic Garage Wall exists → merge all, use max(area[0], area[1])
        let boundaries = vec![
            boundary(
                BoundaryType::Roof,
                ZoneType::Attic,
                ZoneType::Outdoor,
                80.0,
                Some(tilt_deg),
            ),
            boundary(
                BoundaryType::Wall,
                ZoneType::Attic,
                ZoneType::Outdoor,
                10.0,
                None,
            ),
            boundary(
                BoundaryType::Wall,
                ZoneType::Attic,
                ZoneType::Outdoor,
                8.0,
                None,
            ),
            // Attic Garage Wall (Garage→Attic)
            boundary(
                BoundaryType::Wall,
                ZoneType::Garage,
                ZoneType::Attic,
                4.0,
                None,
            ),
        ];
        let floor_area = 100.0;
        let vol = compute_attic_volume(&boundaries, Some(floor_area), None)
            .expect("volume should compute");
        // Merged areas: [10.0, 8.0, 4.0]; max(first two of merged) = max(10, 8) = 10
        let gable_area = 10.0_f64.max(8.0);
        let h = (gable_area * tilt_rad.tan()).sqrt();
        let expected = 0.5 * floor_area * h;
        assert!(
            (vol - expected).abs() < 1e-6,
            "path A: got {vol}, expected {expected}"
        );
    }

    #[test]
    fn attic_volume_asymmetric_gables_returns_none() {
        use super::{Boundary, BoundaryType, ZoneType, compute_attic_volume};
        fn boundary(
            bt: BoundaryType,
            interior: ZoneType,
            exterior: ZoneType,
            area: f64,
            tilt: Option<f64>,
        ) -> Boundary {
            Boundary {
                id: String::new(),
                boundary_type: bt,
                area_m2: area,
                azimuth_deg: None,
                assembly_r_value_m2_k_w: None,
                r_value_layers_m2_k_w: Vec::new(),
                interior_zone: Some(interior),
                exterior_zone: Some(exterior),
                material_layers: Vec::new(),
                framing_factor: None,
                construction_type: None,
                finish_type: None,
                insulation_details: None,
                has_radiant_barrier: false,
                solar_absorptance: None,
                emittance: None,
                tilt_deg: tilt,
                lut_boundary_name: None,
                floor_or_ceiling: None,
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            }
        }
        let tilt_deg = (6.0_f64 / 12.0).atan().to_degrees();
        // diff = 5.0 m² > 0.5 m² threshold → None
        let boundaries = vec![
            boundary(
                BoundaryType::Roof,
                ZoneType::Attic,
                ZoneType::Outdoor,
                50.0,
                Some(tilt_deg),
            ),
            boundary(
                BoundaryType::Wall,
                ZoneType::Attic,
                ZoneType::Outdoor,
                10.0,
                None,
            ),
            boundary(
                BoundaryType::Wall,
                ZoneType::Attic,
                ZoneType::Outdoor,
                15.0,
                None,
            ),
        ];
        assert!(
            compute_attic_volume(&boundaries, Some(100.0), None).is_none(),
            "asymmetric gables (diff=5.0 m²) should return None"
        );
    }

    #[test]
    fn attic_volume_nearly_equal_gables_succeeds() {
        use super::{Boundary, BoundaryType, ZoneType, compute_attic_volume};
        fn boundary(
            bt: BoundaryType,
            interior: ZoneType,
            exterior: ZoneType,
            area: f64,
            tilt: Option<f64>,
        ) -> Boundary {
            Boundary {
                id: String::new(),
                boundary_type: bt,
                area_m2: area,
                azimuth_deg: None,
                assembly_r_value_m2_k_w: None,
                r_value_layers_m2_k_w: Vec::new(),
                interior_zone: Some(interior),
                exterior_zone: Some(exterior),
                material_layers: Vec::new(),
                framing_factor: None,
                construction_type: None,
                finish_type: None,
                insulation_details: None,
                has_radiant_barrier: false,
                solar_absorptance: None,
                emittance: None,
                tilt_deg: tilt,
                lut_boundary_name: None,
                floor_or_ceiling: None,
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            }
        }
        let tilt_deg = (6.0_f64 / 12.0).atan().to_degrees();
        let floor_area = 100.0;
        // diff = 0.3 m² < 0.5 m² threshold → Some(volume)
        let boundaries = vec![
            boundary(
                BoundaryType::Roof,
                ZoneType::Attic,
                ZoneType::Outdoor,
                50.0,
                Some(tilt_deg),
            ),
            boundary(
                BoundaryType::Wall,
                ZoneType::Attic,
                ZoneType::Outdoor,
                10.0,
                None,
            ),
            boundary(
                BoundaryType::Wall,
                ZoneType::Attic,
                ZoneType::Outdoor,
                10.3,
                None,
            ),
        ];
        let vol = compute_attic_volume(&boundaries, Some(floor_area), None)
            .expect("nearly-equal gables (diff=0.3 m²) should return Some");
        let attic_height = (10.0_f64 * tilt_deg.to_radians().tan()).sqrt();
        let expected = 0.5 * floor_area * attic_height;
        assert!(
            (vol - expected).abs() < 1e-6,
            "nearly-equal gables: got {vol}, expected {expected}"
        );
    }

    #[test]
    fn attic_floor_area_includes_garage_ceiling() {
        // When there's a Garage→Attic floor boundary, the attic floor area should
        // include its area. We test this by parsing XML with a garage ceiling.
        use super::parse_building;
        let xml = r#"
<HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site>
          <SiteType>suburban</SiteType>
        </Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="ft2">1000</ConditionedFloorArea>
          <ConditionedBuildingVolume units="ft3">8000</ConditionedBuildingVolume>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <Walls>
          <Wall>
            <SystemIdentifier id="W1"/>
            <InteriorAdjacentTo>conditioned space</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <Area units="ft2">100</Area>
            <Azimuth>0</Azimuth>
          </Wall>
        </Walls>
        <Roofs>
          <Roof>
            <SystemIdentifier id="R1"/>
            <InteriorAdjacentTo>attic vented</InteriorAdjacentTo>
            <Area units="ft2">600</Area>
            <Pitch>6</Pitch>
          </Roof>
        </Roofs>
        <Floors>
          <Floor>
            <SystemIdentifier id="AtticFloor"/>
            <InteriorAdjacentTo>conditioned space</InteriorAdjacentTo>
            <ExteriorAdjacentTo>attic vented</ExteriorAdjacentTo>
            <Area units="ft2">500</Area>
          </Floor>
          <Floor>
            <SystemIdentifier id="GarageCeiling"/>
            <InteriorAdjacentTo>garage</InteriorAdjacentTo>
            <ExteriorAdjacentTo>attic vented</ExteriorAdjacentTo>
            <Area units="ft2">200</Area>
          </Floor>
        </Floors>
        <Attics>
          <Attic>
            <AtticType><Attic><Vented>true</Vented></Attic></AtticType>
          </Attic>
        </Attics>
        <Garages>
          <Garage>
            <FloorArea units="ft2">200</FloorArea>
          </Garage>
        </Garages>
        <AirInfiltration>
          <AirInfiltrationMeasurement>
            <BuildingAirLeakage>
              <AirLeakage>5.0</AirLeakage>
            </BuildingAirLeakage>
          </AirInfiltrationMeasurement>
        </AirInfiltration>
      </Enclosure>
    </BuildingDetails>
  </Building>
</HPXML>"#;
        let building = parse_building(xml).expect("parse should succeed");
        let attic = building
            .zones
            .iter()
            .find(|z| z.zone_type == ZoneType::Attic)
            .expect("attic zone expected");
        // Attic floor area should be 500 + 200 = 700 ft² converted to m²
        let expected_m2 = 700.0 * 0.092_903_04;
        let attic_area = attic.floor_area_m2.expect("attic should have floor area");
        assert!(
            (attic_area - expected_m2).abs() < 0.1,
            "attic floor area should include garage ceiling: got {attic_area}, expected ~{expected_m2}"
        );
    }

    // ── Mass multiplier test (via ZoneInput) ────────────────────────────

    #[test]
    fn max_areas_by_azimuth_two_groups() {
        use super::max_areas_by_azimuth;
        let areas = vec![6.0, 8.0, 4.0];
        let azimuths = vec![0.0, 90.0, 0.0];
        let (a, b) = max_areas_by_azimuth(&areas, &azimuths).unwrap();
        // Group 0°: max(6,4)=6; Group 90°: 8
        assert!((a - 6.0).abs() < 1e-6);
        assert!((b - 8.0).abs() < 1e-6);
    }

    #[test]
    fn max_areas_by_azimuth_returns_none_for_one_group() {
        use super::max_areas_by_azimuth;
        let areas = vec![6.0, 4.0];
        let azimuths = vec![0.0, 0.0];
        assert!(max_areas_by_azimuth(&areas, &azimuths).is_none());
    }

    #[test]
    fn convert_conductivity_unrecognized_unit_returns_raw() {
        let val = super::convert_conductivity_to_w_m_k(1.5, Some("bogus"));
        assert!((val - 1.5).abs() < 1e-12);
    }

    #[test]
    fn convert_conductivity_none_assumes_ip() {
        let val = super::convert_conductivity_to_w_m_k(1.0, None);
        // Should convert from BTU*in/(hr*ft2*F), not return raw
        assert!(val != 1.0);
    }

    #[test]
    fn convert_density_unrecognized_unit_returns_raw() {
        let val = super::convert_density_to_kg_m3(2.5, Some("bogus"));
        assert!((val - 2.5).abs() < 1e-12);
    }

    #[test]
    fn convert_density_none_assumes_ip() {
        let val = super::convert_density_to_kg_m3(1.0, None);
        assert!(val != 1.0);
    }

    #[test]
    fn convert_specific_heat_unrecognized_unit_returns_raw() {
        let val = super::convert_specific_heat_to_j_kg_k(3.0, Some("bogus"));
        assert!((val - 3.0).abs() < 1e-12);
    }

    #[test]
    fn convert_specific_heat_none_assumes_ip() {
        let val = super::convert_specific_heat_to_j_kg_k(1.0, None);
        assert!(val != 1.0);
    }

    #[test]
    fn convert_temperature_unrecognized_unit_returns_raw() {
        let val = super::convert_temperature_to_c(100.0, Some("kelvin"));
        assert!((val - 100.0).abs() < 1e-12);
    }

    #[test]
    fn convert_temperature_known_units_work() {
        let c = super::convert_temperature_to_c(212.0, Some("F"));
        assert!((c - 100.0).abs() < 0.1);
        let c2 = super::convert_temperature_to_c(25.0, Some("C"));
        assert!((c2 - 25.0).abs() < 1e-12);
    }

    #[test]
    fn convert_temperature_none_assumes_ip() {
        let c = super::convert_temperature_to_c(32.0, None);
        assert!(
            (c - 0.0).abs() < 0.1,
            "None units should assume F: 32F == 0C, got {c}"
        );
    }

    #[test]
    fn convert_area_unrecognized_unit_returns_raw() {
        let val = super::convert_area_to_m2(5.0, Some("bogus"));
        assert!((val - 5.0).abs() < 1e-12);
    }

    #[test]
    fn convert_u_value_unrecognized_unit_returns_raw() {
        let val = super::convert_u_to_w_m2_k(2.0, Some("bogus"));
        assert!((val - 2.0).abs() < 1e-12);
    }

    #[test]
    fn convert_r_value_unrecognized_unit_returns_raw() {
        let val = super::convert_r_to_m2_k_w(10.0, Some("bogus"));
        assert!((val - 10.0).abs() < 1e-12);
    }

    #[test]
    fn duct_leakage_cfm25_stored_for_deferred_conversion() {
        let xml = r#"
<HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site>
          <SiteType>suburban</SiteType>
        </Site>
        <BuildingConstruction>
          <ConditionedFloorArea>1000</ConditionedFloorArea>
          <ConditionedBuildingVolume>8000</ConditionedBuildingVolume>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure/>
      <Systems>
        <HVAC>
          <HVACDistribution>
            <SystemIdentifier id="HVACDist1"/>
            <DistributionSystemType>
              <AirDistribution>
                <DuctLeakageMeasurement>
                  <SystemIdentifier id="LeakCFM25"/>
                  <DuctType>supply</DuctType>
                  <DuctLeakage>
                    <Value>100</Value>
                    <Units>CFM25</Units>
                  </DuctLeakage>
                </DuctLeakageMeasurement>
                <Ducts>
                  <SystemIdentifier id="SupplyDuct"/>
                  <DuctType>supply</DuctType>
                  <DuctSurfaceArea>50</DuctSurfaceArea>
                  <DuctLocation>conditioned space</DuctLocation>
                </Ducts>
              </AirDistribution>
            </DistributionSystemType>
          </HVACDistribution>
        </HVAC>
      </Systems>
    </BuildingDetails>
  </Building>
</HPXML>"#;
        let building = parse_building(xml).unwrap();
        let supply = building
            .zones
            .iter()
            .flat_map(|z| &z.duct_systems)
            .find(|d| d.duct_type == DuctType::Supply)
            .expect("supply duct expected in conditioned zone");
        assert_eq!(
            supply.leakage_fraction, None,
            "CFM25 cannot be converted to fraction at parse time; leakage_fraction must be None"
        );
        assert_eq!(
            supply.leakage_cfm25,
            Some(100.0),
            "CFM25 raw value must be preserved in leakage_cfm25 for deferred conversion"
        );
    }

    // Regression tests: infiltration_ach50 must reject CFM50 values.

    #[test]
    fn cfm50_inline_attr_does_not_poison_ach50() {
        // <AirLeakage units="CFM50">750.0</AirLeakage> appears directly under
        // AirInfiltrationMeasurement (HPXML 4.x form).  The ACH50 field must be
        // None; the CFM50 field must hold 750.0.
        let xml = SAMPLE_XML.replace(
            "<AirLeakage>5.0</AirLeakage>",
            "<AirLeakage units=\"CFM50\">750.0</AirLeakage>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        assert_eq!(
            building.infiltration_cfm50,
            Some(750.0),
            "CFM50 should be captured in infiltration_cfm50"
        );
        assert_eq!(
            building.infiltration_ach50, None,
            "CFM50 input must not be stored as ACH50 (bug: 750 CFM50 stored as 750 ACH50)"
        );
    }

    #[test]
    fn ach50_inline_attr_is_accepted() {
        // <AirLeakage units="ACH50">5.0</AirLeakage> — a valid ACH50 measurement
        // expressed via the HPXML 4.x inline-attribute form.
        let xml = SAMPLE_XML.replace(
            "<AirLeakage>5.0</AirLeakage>",
            "<AirLeakage units=\"ACH50\">5.0</AirLeakage>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        assert_eq!(
            building.infiltration_ach50,
            Some(5.0),
            "ACH50 inline-attribute form must be accepted into infiltration_ach50"
        );
        assert!(
            building.infiltration_cfm50.is_none(),
            "ACH50 input must not be stored as CFM50"
        );
    }

    #[test]
    fn unitofmeasure_cfm_wrapper_does_not_set_ach50() {
        // HPXML 3.x wrapper form: <BuildingAirLeakage><UnitofMeasure>CFM</UnitofMeasure>
        // <AirLeakage>850.0</AirLeakage></BuildingAirLeakage>.
        // ACH50 must be None; CFM50 must be 850.0.
        let xml = SAMPLE_XML.replace(
            "<AirLeakage>5.0</AirLeakage>",
            "<BuildingAirLeakage>\
               <UnitofMeasure>CFM</UnitofMeasure>\
               <AirLeakage>850.0</AirLeakage>\
             </BuildingAirLeakage>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        assert_eq!(
            building.infiltration_cfm50,
            Some(850.0),
            "CFM wrapper form should be captured in infiltration_cfm50"
        );
        assert_eq!(
            building.infiltration_ach50, None,
            "CFM wrapper form must not pollute infiltration_ach50"
        );
    }

    #[test]
    fn unrecognised_unitofmeasure_rejects_with_error() {
        // <BuildingAirLeakage><UnitofMeasure>Pa</UnitofMeasure> — "Pa" is used
        // for HousePressure, not AirLeakage.  Parsing must fail loudly, not silently
        // drop the value (which would model the home with zero infiltration).
        let xml = SAMPLE_XML.replace(
            "<AirLeakage>5.0</AirLeakage>",
            "<BuildingAirLeakage>\
               <UnitofMeasure>Pa</UnitofMeasure>\
               <AirLeakage>50.0</AirLeakage>\
             </BuildingAirLeakage>",
        );
        let err = parse_building(&xml).expect_err("unrecognised UnitofMeasure must be an error");
        let msg = format!("{err}");
        assert!(
            msg.contains("unrecognised") && msg.contains("'pa'"),
            "error should name 'unrecognised' and the unit 'pa' (normalised), got: {msg}"
        );
    }

    #[test]
    fn unrecognised_units_attr_rejects_with_error() {
        // <AirLeakage units="L/s"> — not a valid HPXML AirLeakage unit.
        let xml = SAMPLE_XML.replace(
            "<AirLeakage>5.0</AirLeakage>",
            "<AirLeakage units=\"L/s\">12.0</AirLeakage>",
        );
        let err = parse_building(&xml).expect_err("unrecognised units attr must be an error");
        let msg = format!("{err}");
        assert!(
            msg.contains("unrecognised") && msg.contains("'l/s'"),
            "error should name 'unrecognised' and the unit 'l/s' (normalised), got: {msg}"
        );
    }

    #[test]
    fn cfmnatural_wrapper_converts_to_cfm50() {
        // CFMnatural = 1200.0 → CFM50 = 1200.0 × 12.5^0.65 ≈ 6197.22
        let xml = SAMPLE_XML.replace(
            "<AirLeakage>5.0</AirLeakage>",
            "<BuildingAirLeakage>\
               <UnitofMeasure>CFMnatural</UnitofMeasure>\
               <AirLeakage>1200.0</AirLeakage>\
             </BuildingAirLeakage>",
        );
        let building = parse_building(&xml).expect("CFMnatural should parse successfully");
        assert!(
            building.infiltration_ach50.is_none(),
            "CFMnatural must not populate infiltration_ach50"
        );
        let cfm50 = building
            .infiltration_cfm50
            .expect("CFMnatural should be converted to CFM50");
        // 1200 × (50/4)^0.65 = 1200 × 12.5^0.65 ≈ 6197.22
        assert!(
            (cfm50 - 6197.22).abs() < 0.5,
            "converted CFM50={cfm50}, expected ~6197.22"
        );
        // Raw natural value should be preserved for diagnostics.
        assert_eq!(
            building.infiltration_cfm_natural,
            Some(1200.0),
            "raw CFMnatural value must be preserved"
        );
    }

    #[test]
    fn cfmnatural_inline_attr_converts_to_cfm50() {
        // HPXML 4.x: <AirLeakage units="CFMnatural">800.0</AirLeakage>
        let xml = SAMPLE_XML.replace(
            "<AirLeakage>5.0</AirLeakage>",
            "<AirLeakage units=\"CFMnatural\">800.0</AirLeakage>",
        );
        let building = parse_building(&xml).expect("CFMnatural inline attr should parse");
        assert!(building.infiltration_ach50.is_none());
        let cfm50 = building
            .infiltration_cfm50
            .expect("CFMnatural should be converted to CFM50");
        // 800 × 12.5^0.65 ≈ 4131.48
        assert!((cfm50 - 4131.48).abs() < 0.5);
        assert_eq!(building.infiltration_cfm_natural, Some(800.0));
    }

    #[test]
    fn achnatural_wrapper_converts_to_ach50() {
        // ACHnatural = 1.0 → ACH50 = 1.0 × 12.5^0.65 ≈ 5.164
        let xml = SAMPLE_XML.replace(
            "<AirLeakage>5.0</AirLeakage>",
            "<BuildingAirLeakage>\
               <UnitofMeasure>ACHnatural</UnitofMeasure>\
               <AirLeakage>1.0</AirLeakage>\
             </BuildingAirLeakage>",
        );
        let building = parse_building(&xml).expect("ACHnatural should parse successfully");
        assert!(
            building.infiltration_cfm50.is_none(),
            "ACHnatural must not populate infiltration_cfm50"
        );
        let ach50 = building
            .infiltration_ach50
            .expect("ACHnatural should be converted to ACH50");
        assert!(
            (ach50 - 5.164).abs() < 0.01,
            "converted ACH50={ach50}, expected ~5.164"
        );
        assert_eq!(
            building.infiltration_ach_natural,
            Some(1.0),
            "raw ACHnatural value must be preserved"
        );
    }

    #[test]
    fn achnatural_inline_attr_converts_to_ach50() {
        // HPXML 4.x: <AirLeakage units="ACHnatural">2.0</AirLeakage>
        let xml = SAMPLE_XML.replace(
            "<AirLeakage>5.0</AirLeakage>",
            "<AirLeakage units=\"ACHnatural\">2.0</AirLeakage>",
        );
        let building = parse_building(&xml).expect("ACHnatural inline attr should parse");
        assert!(building.infiltration_cfm50.is_none());
        let ach50 = building
            .infiltration_ach50
            .expect("ACHnatural should be converted to ACH50");
        // 2.0 × 12.5^0.65 ≈ 10.329
        assert!((ach50 - 10.329).abs() < 0.02);
        assert_eq!(building.infiltration_ach_natural, Some(2.0));
    }

    // -----------------------------------------------------------------
    // assembly_framing_factor unit tests
    // -----------------------------------------------------------------

    /// 2×4 at 16" OC standard framing, 8 ft wall → per ASHRAE HoF Ch. 27
    /// Table 6, assembly = 0.23. Formula: 0.094 + 0.047 + 0.09 ≈ 0.231.
    #[test]
    fn assembly_framing_factor_2x4_16oc_standard_8ft_wall() {
        let ff = assembly_framing_factor(1.5, 16.0, 96.0);
        assert!(
            (ff - 0.23).abs() < 5.0 * 0.23 / 100.0,
            "2×4 at 16\" OC should be within 5% of 0.23, got {ff:.4}"
        );
        assert!(ff >= 0.21 && ff <= 0.25, "got {ff:.4}");
    }

    /// 2×4 at 24" OC advanced framing, 8 ft wall → per ASHRAE HoF Ch. 27
    /// Table 6, assembly ≈ 0.15. Formula: 0.063 + 0.047 + 0.04 ≈ 0.150.
    #[test]
    fn assembly_framing_factor_2x4_24oc_advanced_8ft_wall() {
        let ff = assembly_framing_factor(1.5, 24.0, 96.0);
        assert!(
            (ff - 0.15).abs() < 5.0 * 0.15 / 100.0,
            "2×4 at 24\" OC advanced should be within 5% of 0.15, got {ff:.4}"
        );
        assert!(ff >= 0.13 && ff <= 0.17, "got {ff:.4}");
    }

    /// 2×6 at 16" OC standard framing, 8 ft wall. Wider stud increases
    /// stud fraction (1.625/16 = 0.1016) and plate fraction (4.875/96 = 0.0508).
    /// Assembly ≈ 0.1016 + 0.0508 + 0.09 = 0.2424.
    #[test]
    fn assembly_framing_factor_2x6_16oc_standard_8ft_wall() {
        let ff = assembly_framing_factor(1.625, 16.0, 96.0);
        assert!(
            ff > 0.20 && ff < 0.30,
            "2×6 at 16\" OC should exceed 2×4 at same spacing, got {ff:.4}"
        );
    }

    /// 2×4 at 16" OC, 9 ft wall. Taller wall reduces plate fraction:
    /// 4.5/108 = 0.042 (vs 4.5/96 = 0.047). Assembly decreases slightly.
    #[test]
    fn assembly_framing_factor_taller_wall_reduces_framing_fraction() {
        let ff_8ft = assembly_framing_factor(1.5, 16.0, 96.0);
        let ff_9ft = assembly_framing_factor(1.5, 16.0, 108.0);
        assert!(
            ff_9ft < ff_8ft,
            "Taller wall should reduce framing fraction (plate area constant, \
             wall area larger), got 8ft={ff_8ft:.4}, 9ft={ff_9ft:.4}"
        );
    }

    /// Framing fraction is clamped to [0.10, 0.35]. Physically implausible
    /// inputs (e.g. studs touching) should not produce values outside range.
    #[test]
    fn assembly_framing_factor_clamps_to_valid_range() {
        // Very narrow spacing would produce high fraction → clamped to 0.35.
        let ff_narrow = assembly_framing_factor(1.5, 4.0, 96.0);
        assert!(
            ff_narrow <= 0.35,
            "narrow spacing must be clamped, got {ff_narrow:.4}"
        );

        // Very wide spacing would produce low fraction → clamped to 0.10.
        let ff_wide = assembly_framing_factor(0.5, 48.0, 96.0);
        assert!(
            ff_wide >= 0.10,
            "wide spacing must be clamped, got {ff_wide:.4}"
        );
    }

    /// Parsing a wall with `<StudSpacing>`, `<StudWidth>`, and explicit
    /// `<WallHeight units="ft">9</WallHeight>` must use the explicit height.
    #[test]
    fn stud_geometry_with_explicit_wall_height_ft() {
        let xml = r#"
<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <Walls>
          <Wall>
            <SystemIdentifier id='Wall1'/>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <InteriorAdjacentTo>living space</InteriorAdjacentTo>
            <WallType><WoodStud/></WallType>
            <Area units="ft2">240.0</Area>
            <StudSpacing>16.0</StudSpacing>
            <StudWidth>1.5</StudWidth>
            <WallHeight units="ft">9</WallHeight>
            <Insulation>
              <SystemIdentifier id='Wall1Ins'/>
              <AssemblyEffectiveRValue>11.0</AssemblyEffectiveRValue>
            </Insulation>
          </Wall>
        </Walls>
      </Enclosure>
    </BuildingDetails>
  </Building>
</HPXML>
"#;
        let building = parse_building(&xml).expect("should parse");
        let wall = building
            .boundaries
            .iter()
            .find(|b| b.id == "Wall1")
            .expect("Wall1 should be present");
        let ff = wall.framing_factor.expect("framing_factor should be set");

        // 9 ft wall → plate fraction = 4.5/(9×12) = 0.0417 (down from 0.0469 at 8 ft)
        let ff_9ft = assembly_framing_factor(1.5, 16.0, 108.0);
        assert!(
            (ff - ff_9ft).abs() < 1e-10,
            "framing_factor should use 9 ft wall height, got {ff:.4}, expected {ff_9ft:.4}"
        );

        // 9 ft wall should give lower ff than 8 ft wall
        let ff_8ft = assembly_framing_factor(1.5, 16.0, 96.0);
        assert!(
            ff < ff_8ft,
            "9 ft wall (ff={ff:.4}) should have lower ff than 8 ft (ff={ff_8ft:.4})"
        );
    }

    /// <WallHeight>0</WallHeight> must be rejected (non-positive) so the
    /// default 96 in (8 ft) applies. Without this guard the parser would
    /// pass 0.0 to assembly_framing_factor, producing ff_plates = ∞ → clamped
    /// to 0.35, a silently wrong value.
    #[test]
    fn stud_geometry_with_zero_wall_height_defaults_to_96in() {
        let xml = r#"
<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <Walls>
          <Wall>
            <SystemIdentifier id='Wall1'/>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <InteriorAdjacentTo>living space</InteriorAdjacentTo>
            <WallType><WoodStud/></WallType>
            <Area units="ft2">240.0</Area>
            <StudSpacing>16.0</StudSpacing>
            <StudWidth>1.5</StudWidth>
            <WallHeight>0</WallHeight>
            <Insulation>
              <SystemIdentifier id='Wall1Ins'/>
              <AssemblyEffectiveRValue>11.0</AssemblyEffectiveRValue>
            </Insulation>
          </Wall>
        </Walls>
      </Enclosure>
    </BuildingDetails>
  </Building>
</HPXML>
"#;
        let building = parse_building(&xml).expect("should parse");
        let wall = building
            .boundaries
            .iter()
            .find(|b| b.id == "Wall1")
            .expect("Wall1 should be present");
        let ff = wall.framing_factor.expect("framing_factor should be set");

        // WallHeight=0 must trigger the default (96 in), yielding ~0.23,
        // NOT 0.35 from division-by-zero clamped infinity.
        let ff_96in = assembly_framing_factor(1.5, 16.0, 96.0);
        assert!(
            (ff - ff_96in).abs() < 1e-10,
            "WallHeight=0 should default to 96 in, got ff={ff:.4}, expected {ff_96in:.4}"
        );
        assert!(
            ff < 0.30,
            "WallHeight=0 should not produce clamped-infinity value 0.35, got {ff:.4}"
        );
    }

    /// <AtticType><FlatRoof/> means no attic cavity (roof sits directly on
    /// conditioned space). HARES must not create an attic thermal zone.
    /// Ref: ResStock 2025.1 uses FlatRoof for slab-on-grade homes.
    #[test]
    fn flat_roof_skips_attic_zone() {
        let xml = r#"
<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <Walls>
          <Wall>
            <SystemIdentifier id='W1'/>
            <InteriorAdjacentTo>conditioned space</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <Area units="m2">100</Area>
          </Wall>
        </Walls>
        <Roofs>
          <Roof>
            <SystemIdentifier id='R1'/>
            <InteriorAdjacentTo>living space</InteriorAdjacentTo>
            <Area units="m2">200</Area>
            <Pitch>0</Pitch>
          </Roof>
        </Roofs>
        <Attics>
          <Attic>
            <AtticType><FlatRoof/></AtticType>
            <AttachedToRoof idref='R1'/>
          </Attic>
        </Attics>
      </Enclosure>
    </BuildingDetails>
  </Building>
</HPXML>
"#;
        let building = parse_building(xml).expect("flat roof HPXML should parse");
        assert!(
            !building
                .zones
                .iter()
                .any(|z| matches!(z.zone_type, ZoneType::Attic)),
            "FlatRoof should not create an attic zone"
        );
    }
}
