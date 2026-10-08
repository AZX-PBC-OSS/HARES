//! HPXML building envelope and geometry parsing.

use std::collections::HashMap;

use quick_xml::Reader;
use quick_xml::XmlVersion;
use quick_xml::events::Event;

use hares_types::{Warning, normalize_ascii, parse_trimmed_f64};

use hares_physics::check_specific_heat_plausible;
use hares_physics::infiltration::{NATURAL_TO_50PA_EXPONENT, ach_nat_to_ach50};
use hares_physics::units as conv;

use super::HpxmlError;
use super::ParseError;
use super::xml_helpers::{element_id, xs_boolean};

/// HPXML `<SiteType>` -- the terrain class of the building's surroundings.
///
/// Allowed values per the HPXML data dictionary: `rural`, `suburban`,
/// `urban`. OpenStudio-HPXML defaults a missing element to `suburban`
/// ("HPXML Site", Workflow Inputs); HARES resolves the missing case the
/// same way in `site_type_to_terrain`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteType {
    Rural,
    Suburban,
    Urban,
}

/// HPXML `<ShieldingofHome>` -- the wind shielding class of the site.
///
/// Allowed values per the HPXML data dictionary: `normal`, `exposed`,
/// `well-shielded`. A missing element stays `None` for the solver's
/// shielding default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShieldingOfHome {
    Normal,
    Exposed,
    WellShielded,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Site {
    pub elevation_m: Option<f64>,
    pub site_type: Option<SiteType>,
    /// HPXML `<ShieldingofHome>` -- parsed enum value.
    pub shielding_of_home: Option<ShieldingOfHome>,
    pub latitude_deg: Option<f64>,
    pub longitude_deg: Option<f64>,
    /// HPXML `<Site>/<TimeZone>/<UTCOffset>` — the site's offset from UTC in
    /// **Standard Time** (no DST), e.g. `-5.0` for US Eastern. Optional in the
    /// HPXML schema; `None` when the element is absent, in which case the
    /// site-location resolver derives the offset from the weather file or
    /// longitude. See HPXML Data Dictionary `Building/Site/TimeZone/UTCOffset`.
    pub utc_offset_h: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BoundaryType {
    Wall,
    Roof,
    Floor,
    Window,
    Skylight,
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
    /// Outside material: a wall's or rim joist's `Siding` (e.g. "vinyl
    /// siding"), a roof's `RoofType` (e.g. "asphalt or fiberglass
    /// shingles"), a foundation wall's `Type` (e.g. "solid concrete").
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
    /// Height of the space: the attic's gable rise or hip peak, the
    /// foundation or garage height (OS-HPXML `calculate_zone_height`). The one height
    /// the volume and the infiltration model both read.
    pub height_m: Option<f64>,
    /// The HPXML location covering most of the zone's floor
    /// (`"crawlspace - vented"`, `"basement - unconditioned"`, ...).
    pub hpxml_location: Option<String>,
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
    pub skylights: Vec<Window>,
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
    /// `ConditionedBuildingVolume`, or OS-HPXML's default for it.
    pub conditioned_volume_m3: f64,
    /// Average ceiling height in metres: the conditioned volume over the
    /// conditioned floor area, which the parser requires.
    pub ceiling_height_m: f64,
    /// `<InfiltrationHeight>` converted from ft to m.
    pub infiltration_height_m: Option<f64>,
    /// `<NumberofConditionedFloorsAboveGrade>` from `<BuildingConstruction>`;
    /// the parser requires the element.
    pub floors_above_grade: f64,
    /// `<extension><HasFlueOrChimneyInConditionedSpace>` boolean.
    pub has_flue_or_chimney: Option<bool>,
    /// Foundation type name for LUT matching (e.g. "Unfinished Basement", "Crawlspace").
    /// Derived from `<Foundation>/<FoundationType>` per OCHRE hpxml.py:276-286.
    pub foundation_name: Option<String>,
    /// A foundation the HPXML declares conditioned (a finished basement, or
    /// an explicitly conditioned crawlspace) is merged into the conditioned
    /// space per OS-HPXML (geometry.rb `create_or_get_space`, 1704-1716;
    /// hpxml.rb `conditioned_locations`, 12311-12316) and has no thermal
    /// zone of its own. HARES models at most one foundation, matching the
    /// single-foundation read of `foundation_name`.
    pub conditioned_foundation_merged: bool,
    /// Residential facility type from `<BuildingConstruction>/<ResidentialFacilityType>`.
    /// Used for adjusted bedroom count in water heater draw profiles.
    pub residential_facility_type: Option<String>,
    /// The zone air temperature capacitance multiplier, one for every zone,
    /// as OS-HPXML's `ZoneCapacitanceMultiplier:ResearchSpecial`
    /// (simcontrols.rb:27-28): `SoftwareInfo/extension/SimulationControl/
    /// AdvancedResearchFeatures/TemperatureCapacitanceMultiplier`, else 7
    /// (defaults.rb:219-221).
    pub temperature_capacitance_multiplier: f64,
    /// HVAC thermostat deadband/hysteresis in °C.
    /// When set, overrides the default 1.0°C hysteresis for IdealHVAC.
    /// BESTEST/ASHRAE 140 requires 0.0 (ideal setpoint tracking).
    pub hvac_deadband_c: Option<f64>,
    /// The first `ClimateandRiskZones/ClimateZoneIECC/ClimateZone`, or, when
    /// the HPXML has none, the zone OS-HPXML derives from the weather
    /// station (`climate_zone::apply_climate_zone_default`).
    pub climate_zone_iecc: Option<String>,
    /// Raw HPXML parse tree retained for downstream consumers that still read
    /// fields which have not been promoted to typed members on `Building`.
    pub details_xml: XmlNode,
    /// Schema warnings the parse raised (for example the HPXML 3.x
    /// deprecated-`EnergyFactor` notice), carried to the run instead of
    /// living only as `tracing` lines.
    pub parse_warnings: Vec<Warning>,
}

/// A building declares more than one conditioned zone. HARES models one
/// conditioned zone per dwelling unit, as OS-HPXML does, and resolves
/// everything defined relative to "the conditioned zone" (HVAC, the
/// dehumidifier, ventilation, the thermal solver's indoor zone, the
/// scheduled-space ambient air) through it; with several there is no single
/// answer, so the building is rejected rather than resolved to whichever
/// comes first.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the building declares {count} conditioned zones; HARES models one per dwelling unit")]
pub struct MultipleConditionedZones {
    pub count: usize,
}

impl Building {
    /// Index into `zones` of the building's conditioned zone (its 1-indexed
    /// `ZoneId` is the index plus one); `None` when there is none.
    pub fn conditioned_zone_index(&self) -> Result<Option<usize>, MultipleConditionedZones> {
        let mut conditioned = self
            .zones
            .iter()
            .enumerate()
            .filter(|(_, zone)| zone.zone_type == ZoneType::Conditioned)
            .map(|(idx, _)| idx);
        match (conditioned.next(), conditioned.count()) {
            (first, 0) => Ok(first),
            (_, others) => Err(MultipleConditionedZones { count: others + 1 }),
        }
    }

    /// Whether a garage is modeled: the building resolves at least one
    /// Garage zone. Gates garage-only equipment (garage lighting): OCHRE
    /// hpxml.py:1703-1709 creates it only when a garage is modeled, and an
    /// equipment giving zone heat to a Garage zone that does not exist
    /// cannot join the dwelling.
    pub fn models_garage(&self) -> bool {
        self.zones
            .iter()
            .any(|z| matches!(z.zone_type, ZoneType::Garage))
    }
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

    pub(crate) fn first_descendant<'a>(&'a self, name: &'a str) -> Option<&'a XmlNode> {
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

fn byte_to_line_col(input: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(input.len());
    let prefix = &input[..offset];
    let line = prefix.bytes().filter(|&b| b == b'\n').count() + 1;
    let col = prefix.rfind('\n').map_or(offset + 1, |i| offset - i);
    (line, col)
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
                let name = normalize_name(tag.name().as_ref());
                let mut attrs = HashMap::new();
                for attr in tag.attributes().flatten() {
                    let key = normalize_name(attr.key.as_ref());
                    let value = attr
                        .normalized_value(XmlVersion::Implicit1_0)
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
                let name = normalize_name(tag.name().as_ref());
                let mut attrs = HashMap::new();
                for attr in tag.attributes().flatten() {
                    let key = normalize_name(attr.key.as_ref());
                    let value = attr
                        .normalized_value(XmlVersion::Implicit1_0)
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
                let value = text.xml_content(XmlVersion::Implicit1_0);
                if let Some(node) = stack.last_mut()
                    && !value.trim().is_empty()
                {
                    if !node.text.is_empty() {
                        node.text.push(' ');
                    }
                    node.text.push_str(value.trim());
                }
            }
            Ok(Event::End(_)) => {
                let node = stack.pop().ok_or_else(|| {
                    let pos = reader.buffer_position() as usize;
                    let (line, col) = byte_to_line_col(xml, pos);
                    HpxmlError::Parse(ParseError {
                        message: "malformed XML: end tag without start tag".to_string(),
                        byte_offset: pos,
                        line,
                        column: col,
                        element_name: None,
                    })
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
                let pos = reader.error_position() as usize;
                let (line, col) = byte_to_line_col(xml, pos);
                let element_name = stack.last().map(|n| n.name.clone());
                return Err(HpxmlError::Parse(ParseError {
                    message: format!("{err}"),
                    byte_offset: pos,
                    line,
                    column: col,
                    element_name,
                }));
            }
        }
        buf.clear();
    }

    root.ok_or_else(|| {
        HpxmlError::Parse(ParseError {
            message: "empty XML document".to_string(),
            byte_offset: 0,
            line: 0,
            column: 0,
            element_name: None,
        })
    })
}

pub fn parse_building(xml: &str) -> Result<Building, HpxmlError> {
    let root = parse_xml_document(xml)?;
    parse_building_from_node(&root)
}

pub fn parse_building_from_node(root: &XmlNode) -> Result<Building, HpxmlError> {
    let details = root
        .path(&["Building", "BuildingDetails"])
        .ok_or_else(|| HpxmlError::Parse("missing Building/BuildingDetails".into()))?;

    let summary = details
        .child("BuildingSummary")
        .ok_or_else(|| HpxmlError::Parse("missing BuildingSummary".into()))?;

    let site_node = summary
        .child("Site")
        .ok_or_else(|| HpxmlError::Parse("missing BuildingSummary/Site".into()))?;

    let elevation_primary =
        parse_value_with_units(site_node.child("Elevation"), ValueKind::Length)?;
    let elevation_fallback1 = parse_value_with_units(
        root.path(&["Building", "Site", "Elevation"]),
        ValueKind::Length,
    )?;
    let elevation_fallback2 =
        parse_value_with_units(root.first_descendant("Altitude"), ValueKind::Length)?;
    let elevation_m = elevation_primary
        .or(elevation_fallback1)
        .or(elevation_fallback2);
    let site_type = match site_node.child("SiteType") {
        Some(node) => Some(parse_site_type(&node.text)?),
        None => None,
    };
    // HPXML.xsd spells the element ShieldingofHome.
    let shielding_of_home = match site_node.child("ShieldingofHome") {
        Some(node) => Some(parse_shielding_of_home(&node.text)?),
        None => None,
    };
    let latitude_primary = root
        .path(&["Building", "Site", "Latitude"])
        .and_then(XmlNode::text_as_f64);
    let latitude_fallback = find_descendant_f64(root, "Latitude", ValueKind::Raw)?;
    let latitude_deg = latitude_primary.or(latitude_fallback);
    let longitude_primary = root
        .path(&["Building", "Site", "Longitude"])
        .and_then(XmlNode::text_as_f64);
    let longitude_fallback = find_descendant_f64(root, "Longitude", ValueKind::Raw)?;
    let longitude_deg = longitude_primary.or(longitude_fallback);

    // HPXML Site/TimeZone/UTCOffset — standard-time offset from UTC (no DST).
    // Optional; the site-location resolver falls back to the weather file's
    // timezone or a longitude-derived estimate when this is absent.
    let utc_offset_primary = site_node
        .child("TimeZone")
        .and_then(|tz| tz.child("UTCOffset"))
        .and_then(XmlNode::text_as_f64);
    let utc_offset_fallback = find_descendant_f64(root, "UTCOffset", ValueKind::Raw)?;
    let utc_offset_h = utc_offset_primary.or(utc_offset_fallback);

    let conditioned_floor_area_m2 = parse_value_with_units(
        summary.path(&["BuildingConstruction", "ConditionedFloorArea"]),
        ValueKind::Area,
    )?;

    let mut parse_warnings = Vec::new();
    let conditioned_floor_area_m2 = match conditioned_floor_area_m2 {
        Some(area) if area > 0.0 => area,
        Some(_) => {
            return Err(HpxmlError::Parse(
                "ConditionedFloorArea must be positive to derive ceiling height".into(),
            ));
        }
        None => {
            return Err(HpxmlError::Parse(
                "missing ConditionedFloorArea; cannot derive ceiling height".into(),
            ));
        }
    };
    let given_conditioned_volume_m3 = parse_value_with_units(
        summary.path(&["BuildingConstruction", "ConditionedBuildingVolume"]),
        ValueKind::Volume,
    )?;
    let average_ceiling_height_m = super::zone_geometry::average_ceiling_height_m(
        details,
        summary.path(&["BuildingConstruction", "AverageCeilingHeight"]),
        given_conditioned_volume_m3,
        conditioned_floor_area_m2,
        &mut parse_warnings,
    )?;
    let conditioned_volume_m3 = match given_conditioned_volume_m3 {
        Some(volume) => volume,
        None => {
            let volume = super::zone_geometry::default_conditioned_volume_m3(
                details,
                average_ceiling_height_m,
                conditioned_floor_area_m2,
                &mut parse_warnings,
            )?;
            parse_warnings.push(Warning::new(
                "hpxml",
                format!(
                    "no ConditionedBuildingVolume; defaulted to {volume:.1} m3 as OS-HPXML does"
                ),
            ));
            volume
        }
    };
    let ceiling_height_m = conditioned_volume_m3 / conditioned_floor_area_m2;

    let total_conditioned_floors = parse_value_with_units(
        summary.path(&["BuildingConstruction", "NumberofConditionedFloors"]),
        ValueKind::Raw,
    )?;
    let floors_above_grade = parse_value_with_units(
        summary.path(&[
            "BuildingConstruction",
            "NumberofConditionedFloorsAboveGrade",
        ]),
        ValueKind::Raw,
    )?.ok_or_else(|| HpxmlError::MissingField {
        path: "BuildingSummary/BuildingConstruction/NumberofConditionedFloorsAboveGrade",
        system_kind: "Building",
        system_id: "construction".to_string(),
        reason: "the number of conditioned floors above grade is required to set up infiltration and foundations; no silent default permitted",
    })?;

    let residential_facility_type = summary
        .path(&["BuildingConstruction", "ResidentialFacilityType"])
        .map(|n| n.text.trim().to_string())
        .filter(|s| !s.is_empty());

    if let Some(ref facility_type) = residential_facility_type {
        tracing::info!(
            facility_type = %facility_type,
            "parsed residential facility type from HPXML BuildingConstruction"
        );
    }

    // InfiltrationHeight lives under AirInfiltrationMeasurement -- HPXML stores it in feet.
    let infiltration_height_m = parse_value_with_units(
        details.first_descendant("InfiltrationHeight"),
        ValueKind::Length,
    )?;

    // <EffectiveLeakageArea units="sq-in"> -- convert sq inches to cm² (1 in² = 6.4516 cm²).
    let infiltration_ela_cm2 = details
        .first_descendant("EffectiveLeakageArea")
        .and_then(|node| node.text_as_f64())
        .map(|sq_in| sq_in * 6.4516);

    // <AirLeakage units="CFM50"> under AirInfiltrationMeasurement, or
    // <BuildingAirLeakage><UnitofMeasure>CFM</UnitofMeasure>... (HPXML 3.x wrapper form).
    let (infiltration_cfm50, infiltration_cfm_natural) = parse_air_leakage_cfm50(details)?;

    // <extension><HasFlueOrChimneyInConditionedSpace> -- xsd:boolean text.
    // The element's older name <HasFlueOrChimney> is not read; a document
    // carrying it is rejected so the declaration cannot be dropped silently.
    if details.first_descendant("HasFlueOrChimney").is_some() {
        return Err(HpxmlError::Parse(
            "deprecated element <HasFlueOrChimney>; rename it to <HasFlueOrChimneyInConditionedSpace>".into(),
        ));
    }
    let has_flue_or_chimney = details
        .first_descendant("HasFlueOrChimneyInConditionedSpace")
        .map(|n| {
            xs_boolean(
                n,
                "extension/HasFlueOrChimneyInConditionedSpace",
                "Building",
                "building",
            )
        })
        .transpose()?;
    // Foundation type name for LUT matching of foundation wall boundaries.
    // OCHRE hpxml.py:276-286: FoundationType child tag → "Crawlspace" | "Unfinished Basement" | "Finished Basement".
    //
    // When <Foundations> is absent, apply a conservative slab-on-grade default:
    // no Foundation thermal zone, foundation_name=None, with a warning.
    // When <FoundationType> is present but unrecognised, fail loudly matching
    // OCHRE's OCHREException for unknown foundation types.
    let has_foundations_group = details.path(&["Enclosure", "Foundations"]).is_some();

    let foundation_name = if has_foundations_group {
        let ft_child = details
            .path(&["Enclosure", "Foundations", "Foundation", "FoundationType"])
            .and_then(|ft| ft.children.first());

        match ft_child {
            Some(child) => {
                let tag = child.name.as_str();
                match tag {
                    "Crawlspace" => Some("Crawlspace".to_string()),
                    "Basement" => {
                        let foundation_id = details
                            .path(&["Enclosure", "Foundations", "Foundation"])
                            .and_then(element_id)
                            .unwrap_or_else(|| "unknown".to_string());
                        let is_finished = foundation_node_is_conditioned(
                            child,
                            &foundation_id,
                            total_conditioned_floors,
                            floors_above_grade,
                        )?;
                        if is_finished {
                            Some("Finished Basement".to_string())
                        } else {
                            Some("Unfinished Basement".to_string())
                        }
                    }
                    "SlabOnGrade" | "Ambient" | "AboveApartment" => None,
                    other => {
                        tracing::error!(
                            foundation_type = other,
                            "Unrecognized FoundationType child tag"
                        );
                        return Err(HpxmlError::Parse(
                            format!(
                                "Unrecognized FoundationType '{}' in <Foundations> group",
                                other
                            )
                            .into(),
                        ));
                    }
                }
            }
            None => None,
        }
    } else {
        tracing::warn!(
            "HPXML has no <Foundations> group; applying slab-on-grade default. \
             Foundation thermal mass and below-grade zone air buffering will not be modeled."
        );
        None
    };
    let foundation_node = details
        .path(&["Enclosure", "Foundations"])
        .and_then(|group| group.children_named("Foundation").next());
    let foundation_floor_area_m2 = match foundation_node {
        Some(foundation) => parse_value_with_units(foundation.child("FloorArea"), ValueKind::Area)?,
        None => None,
    };

    let (mut boundaries, pitch_absent_ids) = parse_boundaries(details, &mut parse_warnings)?;
    let windows = parse_windows(details, &mut boundaries)?;
    let skylights = parse_skylights(details, &mut boundaries)?;

    // Subtract window, skylight, and door areas from their attached boundaries.
    // OCHRE hpxml.py:118-126: ext_walls[wall]["Area"] -= boundary["Area"]
    {
        let mut boundary_reductions: std::collections::HashMap<String, f64> =
            std::collections::HashMap::new();
        for win in &windows {
            if let Some(ref wall_id) = win.attached_to_wall_id {
                *boundary_reductions.entry(wall_id.clone()).or_default() += win.area_m2;
            }
        }
        for skylight in &skylights {
            if let Some(ref roof_id) = skylight.attached_to_wall_id {
                *boundary_reductions.entry(roof_id.clone()).or_default() += skylight.area_m2;
            }
        }
        // Doors also have AttachedToWall in HPXML.
        if let Some(enclosure) = details.child("Enclosure")
            && let Some(doors) = enclosure.child("Doors")
        {
            for door in doors.children_named("Door") {
                if let Some(wall_id) = door
                    .child("AttachedToWall")
                    .and_then(|n| n.attrs.get("idref"))
                {
                    let area =
                        parse_value_with_units(door.child("Area"), ValueKind::Area)?.unwrap_or(0.0);
                    if area > 0.0 {
                        *boundary_reductions.entry(wall_id.clone()).or_default() += area;
                    }
                }
            }
        }
        for bd in &mut boundaries {
            if let Some(&reduction) = boundary_reductions.get(&bd.id) {
                let new_area = (bd.area_m2 - reduction).max(0.0);
                if new_area <= 0.0 {
                    tracing::warn!(
                        wall = %bd.id,
                        original = bd.area_m2,
                        reduction,
                        "boundary area reduced to zero by window/skylight/door subtraction"
                    );
                }
                bd.area_m2 = new_area;
            }
        }
    }

    // Attempt geometric inference for roofs with missing Pitch. Uses gable
    // end wall area and attic floor area to compute a better tilt estimate
    // than the 4:12 default.
    infer_roof_tilt_from_geometry(&mut boundaries, &pitch_absent_ids);

    // Post-process foundation wall boundaries: override construction_type with
    // foundation_name, apply insulation details and area scaling.
    // OCHRE hpxml.py:408-410: boundaries["Foundation Wall"]["Construction Type"] = foundation_name
    let mut foundation_depth_m: Option<f64> = None;
    for bd in &mut boundaries {
        if bd.boundary_type == BoundaryType::FoundationWall {
            if let Some(ref fnd_name) = foundation_name {
                bd.construction_type = Some(fnd_name.clone());
            }
            let (insulation, area_scale, depth_below_grade) =
                extract_foundation_wall_insulation(details, &bd.id)?;
            bd.insulation_details = insulation;
            bd.area_m2 *= area_scale;
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
    // ASHRAE HoF 2021 Ch. 17 for F-factor perimeter method.
    if let Some(slabs_group) = details.path(&["Enclosure", "Slabs"]) {
        for bd in &mut boundaries {
            if bd.boundary_type == BoundaryType::Slab {
                // The slab interface is at the base of the foundation wall
                // (top of slab ≈ bottom of wall). Propagate from the first
                // foundation wall's `DepthBelowGrade`.
                if bd.foundation_depth_m.is_none() {
                    // Fall back to foundation wall depth, or the EPW reference
                    // depth for building surface ground contact (0.5 m) if
                    // no foundation wall exists. Depth=0 evaluates Kusuda-
                    // Achenbach at the ground surface, tracking outdoor air
                    // with no seasonal damping — physically incorrect for a
                    // slab-on-grade.
                    bd.foundation_depth_m = foundation_depth_m
                        .or(Some(hares_physics::ground::DEFAULT_SLAB_GROUND_DEPTH_M));
                }

                if let Some(slab_node) = slabs_group.children_named("Slab").find(|n| {
                    n.child("SystemIdentifier")
                        .and_then(|si| si.attrs.get("id"))
                        .map(|id| id == &bd.id)
                        .unwrap_or(false)
                }) {
                    bd.insulation_details = extract_slab_insulation(slab_node)?;

                    // Parse exposed perimeter length for F-factor method.
                    // HPXML 4.x <ExposedPerimeter> and 3.x <Perimeter>, in feet;
                    // convert to meters via ValueKind::Length.
                    bd.perimeter_m = parse_value_with_units(
                        slab_node
                            .child("ExposedPerimeter")
                            .or_else(|| slab_node.child("Perimeter")),
                        ValueKind::Length,
                    )?;

                    // Parse perimeter insulation R-value for F2 coefficient selection.
                    // HPXML NominalRValue is in IP ft²·°F·h/Btu; convert to SI m²·K/W
                    // via ValueKind::RValue.
                    bd.perimeter_insulation_r_m2_k_w = parse_value_with_units(
                        slab_node.path(&["PerimeterInsulation", "Layer", "NominalRValue"]),
                        ValueKind::RValue,
                    )?;
                }
            }
        }
    }

    // The conditioned zone should exclude below-grade foundation area when a basement
    // is present. OCHRE: indoor_floor_area = conditioned_floor_area - first_floor_area * below_grade_floors.
    // When the HPXML declares no <Foundation><FloorArea>, the foundation's
    // floor area is its slabs' area sum, as OS-HPXML v1.12.0 derives it
    // (geometry.rb:1315-1324, calculate_zone_volume: a foundation zone's
    // floor area is the area of the slabs adjacent to it; geometry.rb
    // 750-771, apply_conditioned_floor_area: the conditioned floor area is
    // the floors and slabs adjacent to conditioned space, so the
    // foundation's own area is what leaves it). A home whose below-grade
    // foundation has no slabs has no foundation zone at all (a space exists
    // only where a surface names it, geometry.rb create_or_get_space), so
    // the conditioned zone holds the full conditioned floor area.
    let total = conditioned_floor_area_m2;
    let derived_foundation_floor_area_m2: f64 = boundaries
        .iter()
        .filter(|bd| {
            bd.boundary_type == BoundaryType::Slab
                && bd.interior_zone.as_ref() == Some(&ZoneType::Foundation)
        })
        .map(|bd| bd.area_m2)
        .sum();
    let indoor_floor_area_m2 = match (total_conditioned_floors, foundation_floor_area_m2) {
        (Some(n_total), Some(foundation_area))
            if n_total > 0.0 && floors_above_grade >= 0.0 && floors_above_grade < n_total =>
        {
            let below_grade_floors = (n_total - floors_above_grade).max(0.0);
            Some((total - foundation_area * below_grade_floors).max(0.0))
        }
        (Some(n_total), None)
            if n_total > 0.0
                && floors_above_grade >= 0.0
                && floors_above_grade < n_total
                && derived_foundation_floor_area_m2 > 0.0 =>
        {
            let below_grade_floors = (n_total - floors_above_grade).max(0.0);
            parse_warnings.push(Warning::new(
                "hpxml",
                format!(
                    "the foundation declares no FloorArea; its floor area is its slabs' area \
                     sum ({derived_foundation_floor_area_m2:.1} m2), as OS-HPXML v1.12.0 derives \
                     it (geometry.rb:1315-1324, calculate_zone_volume; the conditioned floor \
                     area's split, geometry.rb:750-771, apply_conditioned_floor_area)"
                ),
            ));
            Some((total - derived_foundation_floor_area_m2 * below_grade_floors).max(0.0))
        }
        _ => Some(total),
    };

    let (mut zones, conditioned_foundation_merged) = build_zone_map(
        details,
        Some(conditioned_floor_area_m2),
        indoor_floor_area_m2,
        total_conditioned_floors,
        floors_above_grade,
    )?;
    ensure_referenced_zones_exist(&boundaries, &mut zones);
    // A space exists where an enclosure surface is adjacent to it: OS-HPXML
    // creates spaces only for the locations surfaces name (geometry.rb:1738,
    // `create_or_get_space`), and OCHRE builds no foundation zone for a
    // slab, ambient or above-apartment foundation (hpxml.py:286) and no
    // attic zone without an attic roof (hpxml.py:577). An attic, garage or
    // foundation the HPXML groups declare with neither a floor area nor a
    // surface touching it (a below-apartment attic, a slab-on-grade
    // foundation) has no geometry and is no zone. A zone ducts run in
    // stays, so their data is not dropped; with no geometry its volume is
    // then an error.
    assign_walls_to_zones(&boundaries, &mut zones);
    parse_duct_systems(details, &mut zones)?;
    zones.retain(|_, zone| {
        !matches!(
            zone.zone_type,
            ZoneType::Attic | ZoneType::Garage | ZoneType::Foundation
        ) || zone.floor_area_m2.is_some()
            || !zone.duct_systems.is_empty()
            || boundaries.iter().any(|b| {
                b.interior_zone.as_ref() == Some(&zone.zone_type)
                    || b.exterior_zone.as_ref() == Some(&zone.zone_type)
            })
    });

    // Auto-generate interior wall boundary (partition thermal mass).
    // Area = conditioned floor area, same-zone (Conditioned→Conditioned).
    if let Some(cond_zone) = zones
        .values()
        .find(|z| z.zone_type == ZoneType::Conditioned)
        && let Some(area) = cond_zone.floor_area_m2
        && area > 0.0
    {
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
        if let Some(zone) = zones.values().find(|z| z.zone_type == *zone_type)
            && let Some(area) = zone.floor_area_m2
        {
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
                    // exactly Conditioned, Foundation, and Garage; no other
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

    // Zone geometry, one rule per zone type. Conditioned: floor area times
    // the ceiling height. Foundation and garage: OS-HPXML's slab-area times
    // tallest-wall rule (`zone_geometry::slab_space_geometry`). Attic: a
    // gable attic's geometric volume, else OS-HPXML's square hip under its
    // roofs (`zone_geometry::attic_geometry`). Each zone carries the
    // height its rule used. A zone with no geometry keeps no volume and the
    // environment rejects it.
    for zone in &mut zones_vec {
        let geometry = match zone.zone_type {
            ZoneType::Conditioned => {
                zone.volume_m3 = zone.floor_area_m2.map(|a| a * ceiling_height_m);
                zone.height_m = Some(ceiling_height_m);
                continue;
            }
            ZoneType::Attic => super::zone_geometry::attic_geometry(
                details,
                total_conditioned_floors.map(|all| super::zone_geometry::Storey {
                    wall_height_m: floors_above_grade * average_ceiling_height_m,
                    floor_area_m2: conditioned_floor_area_m2 / all,
                }),
                &mut parse_warnings,
            )?,
            ZoneType::Garage | ZoneType::Foundation => super::zone_geometry::slab_space_geometry(
                details,
                &zone.zone_type,
                zone.floor_area_m2,
                &mut parse_warnings,
            )?,
            // Outdoor, Ground, Adjacent are filtered above; Other has no volume model.
            ZoneType::Outdoor | ZoneType::Ground | ZoneType::Adjacent | ZoneType::Other(_) => None,
        };
        if let Some(geometry) = geometry {
            zone.floor_area_m2 = zone.floor_area_m2.or(Some(geometry.floor_area_m2));
            zone.volume_m3 = Some(geometry.volume_m3);
            zone.height_m = Some(geometry.height_m);
            zone.hpxml_location = Some(geometry.location);
        }
    }
    for zone in &mut zones_vec {
        default_vented_space_sla(zone, &mut parse_warnings);
    }

    // <AirLeakage> ACH50 / ACHnatural (HPXML 4.x inline or HPXML 3.x wrapper).
    let (infiltration_ach50, infiltration_ach_natural) = parse_air_leakage_ach50(details)?;

    // Invariant: no boundary pair contains ZoneType::Adjacent after the rewrite stage.
    // OCHRE hpxml.py:96-97 rewrites exterior=interior when exterior is "Adjacent";
    // HARES applies the same rewrite via rewrite_adjacent_zone_pair during boundary
    // and window parsing. An Adjacent zone surviving to this point is a bug.
    // Debug-build check of the rewrite logic: no input reaches it.
    #[cfg(debug_assertions)]
    {
        for bd in &boundaries {
            assert!(
                !matches!(bd.interior_zone, Some(ZoneType::Adjacent)),
                "boundary '{}' interior_zone is Adjacent after rewrite stage",
                bd.id
            );
            assert!(
                !matches!(bd.exterior_zone, Some(ZoneType::Adjacent)),
                "boundary '{}' exterior_zone is Adjacent after rewrite stage",
                bd.id
            );
        }
    }

    Ok(Building {
        site: Site {
            elevation_m,
            site_type,
            shielding_of_home,
            latitude_deg,
            longitude_deg,
            utc_offset_h,
        },
        zones: zones_vec,
        boundaries,
        windows,
        skylights,
        infiltration_cfm50,
        infiltration_cfm_natural,
        infiltration_ela_cm2,
        infiltration_constant_ach: None,
        infiltration_ach_natural,
        infiltration_ach50,
        hvac_capacity_w: {
            let heating = find_descendant_f64(details, "HeatingCapacity", ValueKind::Raw)?;
            let cooling = find_descendant_f64(details, "CoolingCapacity", ValueKind::Raw)?;
            heating.or(cooling).map(conv::power_btu_h_to_w)
        },
        seer2: find_descendant_f64(details, "SEER2", ValueKind::Raw)?,
        hspf2: find_descendant_f64(details, "HSPF2", ValueKind::Raw)?,
        water_heater_setpoint_c: match details.first_descendant("WaterHeatingSystem") {
            Some(wh) => find_descendant_f64(wh, "HotWaterTemperature", ValueKind::Temperature)?,
            None => None,
        },
        heating_weekday_setpoints_c: parse_hvac_setpoints(details, "Heating", true)?,
        heating_weekend_setpoints_c: parse_hvac_setpoints(details, "Heating", false)?,
        cooling_weekday_setpoints_c: parse_hvac_setpoints(details, "Cooling", true)?,
        cooling_weekend_setpoints_c: parse_hvac_setpoints(details, "Cooling", false)?,
        battery_round_trip_efficiency: find_descendant_f64(
            details,
            "RoundTripEfficiency",
            ValueKind::Raw,
        )?,
        pv_tilt_deg: find_descendant_f64(details, "Tilt", ValueKind::Raw)?,
        conditioned_volume_m3,
        ceiling_height_m,
        infiltration_height_m,
        floors_above_grade,
        has_flue_or_chimney,
        foundation_name,
        conditioned_foundation_merged,
        residential_facility_type,
        temperature_capacitance_multiplier: temperature_capacitance_multiplier(
            root,
            &mut parse_warnings,
        )?,
        hvac_deadband_c: None,
        climate_zone_iecc: details
            .path(&["ClimateandRiskZones", "ClimateZoneIECC"])
            .and_then(|iecc| super::xml_helpers::child_text(iecc, "ClimateZone"))
            .filter(|zone| !zone.is_empty()),
        details_xml: details.clone(),
        parse_warnings,
    })
}

/// Parse HVAC thermostat setpoints from `<HVACControl>`.
///
/// Delegates to the shared `xml_helpers::parse_setpoint_from_control` after
/// locating the HVACControl node.
fn parse_hvac_setpoints(
    details: &XmlNode,
    hvac_type: &str,
    weekday: bool,
) -> Result<Option<Vec<f64>>, HpxmlError> {
    match super::xml_helpers::find_hvac_control(details) {
        Some(control) => {
            super::xml_helpers::parse_setpoint_from_control(control, hvac_type, weekday)
        }
        None => Ok(None),
    }
}

fn parse_boundaries(
    details: &XmlNode,
    _parse_warnings: &mut Vec<Warning>,
) -> Result<(Vec<Boundary>, Vec<String>), HpxmlError> {
    let mut out = Vec::new();
    let mut pitch_absent_ids = Vec::new();
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
    // Skylights are NOT in this list — they are parsed separately via
    // parse_skylights() because, like windows, they have a dual representation
    // (Window struct in building.skylights + Boundary in building.boundaries)
    // and use fenestration-specific parsing (UFactor, SHGC, shading, etc.).

    let enclosure = match details.child("Enclosure") {
        Some(node) => node,
        None => return Ok((out, pitch_absent_ids)),
    };

    for (container, item_name, boundary_type) in boundary_specs {
        if let Some(group) = enclosure.child(container) {
            for node in group.children_named(item_name) {
                out.push(parse_boundary(
                    node,
                    boundary_type.clone(),
                    &mut pitch_absent_ids,
                )?);
            }
        }
    }

    // A subsurface (door) inherits the zones of the wall it is attached to:
    // OS-HPXML gives a subsurface its parent surface's outside boundary
    // condition (hpxml.rb subsurface handling, `OutsideBoundaryCondition`
    // from AttachedToWall), so an exterior adjacency the door does not state
    // is derived from the wall, never defaulted to Outdoor. An attached
    // wall the boundary list does not hold is an error naming both ids.
    // A subsurface (door) inherits the zones of the wall it is attached to:
    // OS-HPXML gives a subsurface its parent surface's outside boundary
    // condition (hpxml.rb subsurface handling, `OutsideBoundaryCondition`
    // from AttachedToWall), so an exterior adjacency the door does not state
    // is derived from the wall, never defaulted to Outdoor. An attached
    // wall the boundary list does not hold is an error naming both ids.
    // The derivation is recorded as a parse warning naming the door and the
    // wall it was taken from.
    if let Some(doors) = enclosure.child("Doors") {
        for door in doors.children_named("Door") {
            let Some(wall_id) = door
                .child("AttachedToWall")
                .and_then(|n| n.attrs.get("idref"))
                .map(|id| id.to_string())
            else {
                continue;
            };
            let door_id = element_id(door).unwrap_or_else(|| "unknown".to_string());
            let Some(wall) = out.iter().find(|bd| bd.id == wall_id) else {
                return Err(HpxmlError::MissingField {
                    path: "Door/AttachedToWall",
                    system_kind: "Door",
                    system_id: door_id,
                    reason: "the attached wall's idref matches no parsed boundary, so \
                             the door's zones cannot be derived from it",
                });
            };
            let (wall_interior, wall_exterior) =
                (wall.interior_zone.clone(), wall.exterior_zone.clone());
            let Some(bd) = out
                .iter_mut()
                .find(|bd| bd.boundary_type == BoundaryType::Door && bd.id == door_id)
            else {
                continue;
            };
            let mut inherited: Vec<String> = Vec::new();
            if bd.interior_zone.is_none() {
                bd.interior_zone = wall_interior;
                inherited.push("interior".to_string());
            }
            if bd.exterior_zone.is_none() {
                bd.exterior_zone = wall_exterior;
                inherited.push("exterior".to_string());
            }
            if !inherited.is_empty() {
                // A derivation from stated input (the wall's own adjacency),
                // not a substituted default: recorded as a parse trace, not
                // a run warning.
                tracing::debug!(
                    door = %door_id,
                    wall = %wall_id,
                    zones = %inherited.join(" and "),
                    "door zones derived from its attached wall (the subsurface's \
                     parent surface's boundary condition, as OS-HPXML does)"
                );
            }
        }
    }

    Ok((out, pitch_absent_ids))
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
            parse_value_with_units(window.child("Area"), ValueKind::Area)?.ok_or_else(|| {
                HpxmlError::Parse(
                    format!("window '{}' is missing required Area element", id).into(),
                )
            })?;
        if area_m2 <= 0.0 {
            return Err(HpxmlError::Parse(
                format!("window '{}' has non-positive area: {}", id, area_m2).into(),
            ));
        }
        let azimuth_deg = parse_value_with_units(window.child("Azimuth"), ValueKind::Raw)?;
        let u_factor_w_m2_k = parse_value_with_units(window.child("UFactor"), ValueKind::UValue)?;
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

        let window_interior = parse_zone_ref(window.child("InteriorAdjacentTo"));
        let window_exterior = parse_zone_ref(window.child("ExteriorAdjacentTo"))
            .or_else(|| infer_exterior_zone(&BoundaryType::Window));

        #[cfg(feature = "observe")]
        if window_interior == Some(ZoneType::Adjacent)
            || window_exterior == Some(ZoneType::Adjacent)
        {
            tracing::info!(
                target: "observe",
                column = "adjacent_boundary_rewrite",
                boundary_id = id,
                interior_before = ?window_interior,
                exterior_before = ?window_exterior,
                "rewriting Adjacent zone reference to match non-Adjacent zone"
            );
        }

        let (window_interior, window_exterior) =
            rewrite_adjacent_zone_pair(window_interior, window_exterior);

        boundaries.push(Boundary {
            id,
            boundary_type: BoundaryType::Window,
            area_m2,
            azimuth_deg,
            assembly_r_value_m2_k_w: None,
            r_value_layers_m2_k_w: Vec::new(),
            interior_zone: window_interior,
            exterior_zone: window_exterior,
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

/// Parse `<Enclosure>/<Skylights>/<Skylight>` elements.
///
/// HPXML 4.x defines Skylight as an extension of the Window base type, so the
/// schema is identical: `Area`, `Azimuth`, `UFactor`, `SHGC`, `InteriorShading`,
/// `ExteriorShading`, `FractionOperable`, and `AttachedToWall`.  The key
/// difference is the tilt: skylights are roof-mounted (horizontal, 0° tilt)
/// rather than wall-mounted (vertical, 90° tilt).
///
/// Ref: HPXML 4.2 §6.5 "Windows"; Skylight extends Window base type.
fn parse_skylights(
    details: &XmlNode,
    boundaries: &mut Vec<Boundary>,
) -> Result<Vec<Window>, HpxmlError> {
    let mut skylights = Vec::new();
    let Some(enclosure) = details.child("Enclosure") else {
        return Ok(skylights);
    };
    let Some(group) = enclosure.child("Skylights") else {
        return Ok(skylights);
    };

    for skylight in group.children_named("Skylight") {
        let id = element_id(skylight).unwrap_or_else(|| "unknown".to_string());
        let area_m2 =
            parse_value_with_units(skylight.child("Area"), ValueKind::Area)?.ok_or_else(|| {
                HpxmlError::Parse(
                    format!("skylight '{}' is missing required Area element", id).into(),
                )
            })?;
        if area_m2 <= 0.0 {
            return Err(HpxmlError::Parse(
                format!("skylight '{}' has non-positive area: {}", id, area_m2).into(),
            ));
        }
        let azimuth_deg = parse_value_with_units(skylight.child("Azimuth"), ValueKind::Raw)?;
        let u_factor_w_m2_k = parse_value_with_units(skylight.child("UFactor"), ValueKind::UValue)?;
        let shgc = skylight.child("SHGC").and_then(XmlNode::text_as_f64);

        // InteriorShading/SummerShadingCoefficient — same defaults as windows.
        let (interior_shading_fraction, winter_shading_fraction) =
            match skylight.child("InteriorShading") {
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

        let fraction_operable = skylight
            .child("FractionOperable")
            .and_then(XmlNode::text_as_f64)
            .unwrap_or(0.67)
            .clamp(0.0, 1.0);

        let (exterior_shading_summer, exterior_shading_winter) =
            match skylight.child("ExteriorShading") {
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

        let attached_to_wall_id = skylight
            .child("AttachedToWall")
            .and_then(|n| n.attrs.get("idref").cloned());

        skylights.push(Window {
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

        let skylight_interior = parse_zone_ref(skylight.child("InteriorAdjacentTo"));
        let skylight_exterior = parse_zone_ref(skylight.child("ExteriorAdjacentTo"))
            .or_else(|| infer_exterior_zone(&BoundaryType::Skylight));

        #[cfg(feature = "observe")]
        if skylight_interior == Some(ZoneType::Adjacent)
            || skylight_exterior == Some(ZoneType::Adjacent)
        {
            tracing::info!(
                target: "observe",
                column = "adjacent_boundary_rewrite",
                boundary_id = id,
                interior_before = ?skylight_interior,
                exterior_before = ?skylight_exterior,
                "rewriting Adjacent zone reference to match non-Adjacent zone"
            );
        }

        let (skylight_interior, skylight_exterior) =
            rewrite_adjacent_zone_pair(skylight_interior, skylight_exterior);

        // Skylight tilt defaults to 0° (horizontal, roof-mounted).
        // Per IECC 2021 Table R402.1.2, skylights have different U-factor
        // requirements than vertical fenestration because of their orientation —
        // horizontal surfaces receive more diffuse sky radiation and higher
        // peak solar gains.
        boundaries.push(Boundary {
            id,
            boundary_type: BoundaryType::Skylight,
            area_m2,
            azimuth_deg,
            assembly_r_value_m2_k_w: None,
            r_value_layers_m2_k_w: Vec::new(),
            interior_zone: skylight_interior,
            exterior_zone: skylight_exterior,
            material_layers: Vec::new(),
            construction_type: None,
            finish_type: None,
            insulation_details: None,
            has_radiant_barrier: false,
            solar_absorptance: None,
            emittance: None,
            tilt_deg: Some(0.0),
            framing_factor: None,
            lut_boundary_name: None,
            floor_or_ceiling: None,
            perimeter_m: None,
            perimeter_insulation_r_m2_k_w: None,
            foundation_depth_m: None,
        });
    }

    Ok(skylights)
}

fn parse_boundary(
    node: &XmlNode,
    boundary_type: BoundaryType,
    pitch_absent_ids: &mut Vec<String>,
) -> Result<Boundary, HpxmlError> {
    let id = element_id(node).unwrap_or_else(|| "unknown".to_string());
    let area_m2 = parse_boundary_area(node, &boundary_type, &id)?;
    let r_value_layers_m2_k_w = parse_nominal_r_layers(node)?;
    let assembly_r_value_primary = parse_value_with_units(
        node.first_descendant("AssemblyEffectiveRValue"),
        ValueKind::RValue,
    )?;
    let assembly_r_value_fallback =
        parse_value_with_units(node.first_descendant("RValue"), ValueKind::RValue)?;
    let assembly_r_value_m2_k_w = assembly_r_value_primary.or(assembly_r_value_fallback);
    let material_layers = parse_material_layers(node, area_m2)?;

    let has_radiant_barrier = node
        .first_descendant("RadiantBarrier")
        .map(|n| xs_boolean(n, "RadiantBarrier", "Envelope Surface", &id))
        .transpose()?
        .unwrap_or(false);

    // Solar absorptance and emittance from HPXML, validated to [0, 1].
    // Ref: OCHRE hpxml.py:155-158, OCHRE Envelope.py:222.
    let solar_absorptance = parse_value_with_units(node.child("SolarAbsorptance"), ValueKind::Raw)?
        .map(|v| v.clamp(0.0, 1.0));
    let emittance =
        parse_value_with_units(node.child("Emittance"), ValueKind::Raw)?.map(|v| v.clamp(0.0, 1.0));

    // Extract construction metadata for OCHRE LUT matching.
    let (construction_type, finish_type) = extract_construction_metadata(node, &boundary_type);
    let insulation_details = extract_insulation_details(node);

    // Surface tilt from HPXML <Pitch> (roofs) or implied by boundary type.
    // Pitch is rise:12 run (US roofing convention); tilt = atan(pitch/12).
    // Ref: OCHRE hpxml.py pitch2deg().
    let interior_zone = parse_zone_ref(node.child("InteriorAdjacentTo"));
    let exterior_zone = parse_zone_ref(node.child("ExteriorAdjacentTo"))
        .or_else(|| infer_exterior_zone(&boundary_type));

    let tilt_deg = match boundary_type {
        BoundaryType::Roof => {
            let pitch = parse_value_with_units(node.child("Pitch"), ValueKind::Raw)?;
            let pitch_value = match pitch {
                Some(p) => p,
                None => {
                    tracing::warn!(
                        boundary_id = id,
                        area_m2 = area_m2,
                        interior_zone = ?interior_zone,
                        exterior_zone = ?exterior_zone,
                        "Roof boundary missing <Pitch> element; defaulting to 4:12 pitch \
                         (~18.4°). HPXML v4.0 does not require <Pitch> — roof slope may be \
                         implied by other building geometry. If inference from gable end \
                         walls is possible it will be applied in a later pass."
                    );
                    // Default to typical residential roof pitch of 4:12 (~18.4°)
                    // rather than 0° (flat). 4:12 is the most common US residential
                    // roof pitch; EnergyPlus PVWatts uses a similar default tilt=20°
                    // (vendors/EnergyPlus/src/EnergyPlus/PVWatts.hh:188).
                    // HPXML v4.0: Roof/Pitch is optional (0..1 per hpxml-elements.md).
                    pitch_absent_ids.push(id.clone());
                    4.0
                }
            };
            #[cfg(feature = "observe")]
            {
                let pitch_source = if pitch.is_some() {
                    "explicit"
                } else {
                    "defaulted"
                };
                tracing::info!(
                    target: "observe",
                    column = "pitch_source",
                    boundary_id = id,
                    pitch_source = pitch_source,
                    pitch_value = pitch_value,
                    tilt_deg = (pitch_value / 12.0).atan().to_degrees(),
                );
            }
            Some((pitch_value / 12.0).atan().to_degrees())
        }
        BoundaryType::Wall | BoundaryType::FoundationWall | BoundaryType::RimJoist => Some(90.0),
        BoundaryType::Floor => Some(0.0),
        BoundaryType::Slab => Some(180.0),
        BoundaryType::Door => Some(90.0),
        BoundaryType::Window => Some(90.0),
        BoundaryType::Skylight => Some(0.0),
        BoundaryType::Other(_) => {
            tracing::warn!("Unknown boundary type; no tilt inferred");
            None
        }
    };

    #[cfg(feature = "observe")]
    if interior_zone == Some(ZoneType::Adjacent) || exterior_zone == Some(ZoneType::Adjacent) {
        tracing::info!(
            target: "observe",
            column = "adjacent_boundary_rewrite",
            boundary_id = id,
            interior_before = ?interior_zone,
            exterior_before = ?exterior_zone,
            "rewriting Adjacent zone reference to match non-Adjacent zone"
        );
    }

    let (interior_zone, exterior_zone) = rewrite_adjacent_zone_pair(interior_zone, exterior_zone);

    Ok(Boundary {
        id,
        boundary_type,
        area_m2,
        azimuth_deg: parse_value_with_units(node.child("Azimuth"), ValueKind::Raw)?,
        assembly_r_value_m2_k_w,
        r_value_layers_m2_k_w,
        interior_zone,
        exterior_zone,
        material_layers,
        framing_factor: parse_framing_factor(node, construction_type.as_deref())?,
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
fn parse_framing_factor(
    node: &XmlNode,
    construction_type: Option<&str>,
) -> Result<Option<f64>, HpxmlError> {
    // Explicit FramingFactor from HPXML
    // Range (0, 1) excludes boundaries: 0.0 = no framing (equivalent to None),
    // 1.0 = all framing (physically impossible for an insulated wall).
    if let Some(ff) = find_descendant_f64(node, "FramingFactor", ValueKind::Raw)?
        && ff > 0.0
        && ff < 1.0
    {
        return Ok(Some(ff));
    }

    // Derive assembly framing fraction from stud geometry and wall height.
    // Per ASHRAE HoF 2021 Ch. 27 Table 6: the assembly-level framing fraction
    // includes studs, plates, headers, corners, and miscellaneous members.
    let stud_spacing = find_descendant_f64(node, "StudSpacing", ValueKind::Raw)?;
    let stud_width = find_descendant_f64(node, "StudWidth", ValueKind::Raw)?;
    if let (Some(spacing_in), Some(width_in)) = (stud_spacing, stud_width)
        && spacing_in > 0.0
        && width_in > 0.0
        && width_in < spacing_in
    {
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

        return Ok(Some(assembly_framing_factor(
            width_in,
            spacing_in,
            wall_height_in,
        )));
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
        Some("WoodStud") => Ok(Some(0.25)),
        Some(other) => {
            tracing::warn!(
                construction_type = other,
                "Unrecognized construction type; no framing fraction applied \
                 (SteelFrame requires explicit StudSpacing/StudWidth for zone method)"
            );
            Ok(None)
        }
        None => Ok(None),
    }
}

fn parse_boundary_area(
    node: &XmlNode,
    boundary_type: &BoundaryType,
    id: &str,
) -> Result<f64, HpxmlError> {
    let area = parse_value_with_units(node.child("Area"), ValueKind::Area)?;
    let interior = node
        .child("InteriorAdjacentTo")
        .map(|n| n.text.trim().to_string())
        .unwrap_or_default();
    match boundary_type {
        BoundaryType::FoundationWall => {
            if let Some(a) = area {
                if a <= 0.0 {
                    return Err(HpxmlError::Parse(
                        format!("foundation wall '{}' has non-positive area: {}", id, a).into(),
                    ));
                }
                return Ok(a);
            }
            let length = parse_value_with_units(node.child("Length"), ValueKind::Length)?;
            let height = parse_value_with_units(node.child("Height"), ValueKind::Length)?;
            match (length, height) {
                (Some(l), Some(h)) if l > 0.0 && h > 0.0 => {
                    // HPXML 4.2: FoundationWall/Length = "Total length of foundation wall" [ft];
                    // FoundationWall/Height = "Total height in feet of foundation wall" [ft].
                    // Both are orthogonal horizontal/vertical dimensions of the rectangular
                    // wall face, so Area = Length × Height.
                    let derived = l * h;
                    tracing::debug!(
                        boundary_id = id,
                        length_m = l,
                        height_m = h,
                        derived_area_m2 = derived,
                        "Area element missing; derived from Length × Height"
                    );
                    Ok(derived)
                }
                _ => {
                    tracing::warn!(
                        boundary_id = id,
                        boundary_type = boundary_type_label(boundary_type),
                        "Area element missing; defaulting to 0.0 — heat loss through this surface will be zero"
                    );
                    Ok(0.0)
                }
            }
        }
        BoundaryType::Slab => {
            if let Some(a) = area {
                if a <= 0.0 {
                    return Err(HpxmlError::Parse(
                        format!("slab '{}' has non-positive area: {}", id, a).into(),
                    ));
                }
                return Ok(a);
            }
            Err(HpxmlError::Parse(
                format!("slab in '{interior}' has no Area").into(),
            ))
        }
        _ => {
            let area_m2 = area.ok_or_else(|| {
                HpxmlError::Parse(
                    format!(
                        "{} '{}' is missing required Area element",
                        boundary_type_label(boundary_type),
                        id
                    )
                    .into(),
                )
            })?;
            if area_m2 <= 0.0 {
                return Err(HpxmlError::Parse(
                    format!(
                        "{} '{}' has non-positive area: {}",
                        boundary_type_label(boundary_type),
                        id,
                        area_m2
                    )
                    .into(),
                ));
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
        BoundaryType::Skylight => "skylight",
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
        BoundaryType::Roof
        | BoundaryType::RimJoist
        | BoundaryType::Window
        | BoundaryType::Skylight => Some(ZoneType::Outdoor),
        BoundaryType::Slab => Some(ZoneType::Ground),
        // Floor/FrameFloor: HPXML requires <ExteriorAdjacentTo>, so exterior
        // zone is always parsed from the element rather than inferred here.
        _ => None,
    }
}

/// Extract foundation wall insulation details, area scale factor and depth
/// below grade [m].
///
/// Mirrors OCHRE `get_fnd_wall_insulation` (envelope.py:434-459):
/// - Area scaled by `DepthBelowGrade / Height` when they differ.
/// - Insulation details: "Half R{n}", "R{n}", or "Uninsulated".
///
/// Returns `(insulation_details, area_scale, depth_below_grade_m)`.
/// `depth_below_grade_m` defaults to the wall height when absent from HPXML.
///
/// `details` is the BuildingDetails node; `wall_id` identifies which FoundationWall.
fn extract_foundation_wall_insulation(
    details: &XmlNode,
    wall_id: &str,
) -> Result<(Option<String>, f64, f64), HpxmlError> {
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
        return Ok((Some("Uninsulated".to_string()), 1.0, 0.0));
    };

    // Area scaling: depth_below_grade / height.
    let height = parse_value_with_units(node.child("Height"), ValueKind::Length)?;
    let height_for_scale = height.unwrap_or(1.0);
    let depth_below_grade =
        parse_value_with_units(node.child("DepthBelowGrade"), ValueKind::Length)?
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
                )?
                .unwrap_or(height_for_scale);
                let dist_top = parse_value_with_units(
                    layer.child("DistanceToTopOfInsulation"),
                    ValueKind::Length,
                )?
                .unwrap_or(0.0);
                Ok(dist_bottom - dist_top)
            })
            .collect::<Result<Vec<_>, HpxmlError>>()?
            .into_iter()
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

    Ok((Some(insulation_details), area_scale, depth_below_grade))
}

/// Extract slab insulation details for LUT matching.
///
/// Mirrors OCHRE `get_slab_insulation` (envelope.py:462-485).
/// Reads `PerimeterInsulation` and `UnderSlabInsulation` from the Slab element
/// to produce format strings like "2ft R10 Perimeter", "R10 Whole Slab", etc.
///
/// All numeric values are raw IP (HPXML native) -- no unit conversion needed
/// since the LUT CSV uses IP values.
fn extract_slab_insulation(node: &XmlNode) -> Result<Option<String>, HpxmlError> {
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
            .map(|n| {
                xs_boolean(
                    n,
                    "UnderSlabInsulation/Layer/InsulationSpansEntireSlab",
                    "Slab",
                    &element_id(node).unwrap_or_else(|| "unknown".to_string()),
                )
            })
            .transpose()?
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

    Ok(Some(insulation))
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
            // A foundation wall's outside material is its own Type (solid
            // concrete, concrete block, double brick, wood); HPXML gives
            // walls and rim joists a Siding.
            let material_element = if matches!(boundary_type, BoundaryType::FoundationWall) {
                "Type"
            } else {
                "Siding"
            };
            let finish_type = node
                .child(material_element)
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
        BoundaryType::Window | BoundaryType::Skylight | BoundaryType::Door => (None, None),
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

fn parse_material_layers(node: &XmlNode, area_m2: f64) -> Result<Vec<MaterialLayer>, HpxmlError> {
    let mut layers = Vec::new();
    let mut layer_nodes = Vec::new();
    node.descendants("Layer", &mut layer_nodes);

    for layer in layer_nodes {
        let thickness_m = parse_value_with_units(layer.child("Thickness"), ValueKind::Length)?;
        let conductivity_w_m_k =
            parse_value_with_units(layer.child("Conductivity"), ValueKind::Conductivity)?;
        let density_kg_m3 = parse_value_with_units(layer.child("Density"), ValueKind::Density)?;
        let specific_heat_j_kg_k =
            parse_value_with_units(layer.child("SpecificHeat"), ValueKind::SpecificHeat)?;
        let nominal_r_m2_k_w =
            parse_value_with_units(layer.child("NominalRValue"), ValueKind::RValue)?;

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

        if let Some(cp) = specific_heat_j_kg_k
            && cp > 0.0
        {
            check_specific_heat_plausible(cp, "HPXML material layer")?;
        }
    }

    Ok(layers)
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

/// Whether an HPXML `<Foundation>` node's foundation is conditioned: the
/// explicit `<Conditioned>` flag (HPXML 4.x, read under either
/// `<Basement>` or `<Crawlspace>`), else the OCHRE finished-basement
/// inference for a basement (hpxml.py:276-280: total floors above the
/// above-grade floors). A crawlspace with no explicit flag is
/// unconditioned.
fn foundation_node_is_conditioned(
    ft_child: &XmlNode,
    foundation_id: &str,
    total_conditioned_floors: Option<f64>,
    floors_above_grade: f64,
) -> Result<bool, HpxmlError> {
    let path: &'static str = match ft_child.name.as_str() {
        "Crawlspace" => "Crawlspace/Conditioned",
        _ => "Basement/Conditioned",
    };
    let explicit = ft_child
        .child("Conditioned")
        .map(|n| xs_boolean(n, path, "Foundation", foundation_id))
        .transpose()?;
    Ok(match explicit {
        Some(flag) => flag,
        None => {
            ft_child.name == "Basement"
                && total_conditioned_floors.is_some_and(|total| total > floors_above_grade)
        }
    })
}

fn build_zone_map(
    details: &XmlNode,
    conditioned_floor_area_m2: Option<f64>,
    above_grade_floor_area_m2: Option<f64>,
    total_conditioned_floors: Option<f64>,
    floors_above_grade: f64,
) -> Result<(HashMap<String, Zone>, bool), HpxmlError> {
    // A conditioned foundation (finished basement, conditioned crawlspace)
    // merges into the conditioned space per OS-HPXML and builds no zone.
    let mut merged = false;
    let mut zones: HashMap<String, Zone> = HashMap::new();

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
            height_m: None,
            hpxml_location: None,
        },
    );

    if let Some(enclosure) = details.child("Enclosure") {
        // Attics
        if let Some(group) = enclosure.child("Attics") {
            for node in group.children_named("Attic") {
                // <FlatRoof/> means no attic cavity; skip zone creation (OCHRE hpxml.py:575-576).
                let attic_type_child = node.child("AtticType").and_then(|at| at.children.first());
                if let Some(child) = attic_type_child
                    && child.name == "FlatRoof"
                {
                    continue;
                }

                let floor_area_m2 =
                    parse_value_with_units(node.child("FloorArea"), ValueKind::Area)?;

                // Parse vented status from <AtticType><Attic><Vented>
                let vented = attic_type_child
                    .and_then(|child| child.child("Vented"))
                    .map(|v| {
                        xs_boolean(
                            v,
                            "Attic/AtticType/Attic/Vented",
                            "Attic",
                            &element_id(node).unwrap_or_else(|| "unknown".to_string()),
                        )
                    })
                    .transpose()?
                    .unwrap_or(true); // default vented for attics

                let (ventilation_ach, ventilation_sla) = parse_ventilation_rate(node);

                tracing::debug!(
                    vented,
                    "build_zone_map creating attic zone from <Attics> group"
                );
                zones.entry("attic".to_string()).or_insert(Zone {
                    zone_type: ZoneType::Attic,
                    floor_area_m2,
                    volume_m3: None,
                    attached_wall_ids: Vec::new(),
                    duct_systems: Vec::new(),
                    vented,
                    ventilation_ach,
                    ventilation_sla,
                    height_m: None,
                    hpxml_location: None,
                });
            }
        }

        // Garages
        if let Some(group) = enclosure.child("Garages") {
            for node in group.children_named("Garage") {
                let floor_area_m2 =
                    parse_value_with_units(node.child("FloorArea"), ValueKind::Area)?;
                zones.entry("garage".to_string()).or_insert(Zone {
                    zone_type: ZoneType::Garage,
                    floor_area_m2,
                    volume_m3: None,
                    attached_wall_ids: Vec::new(),
                    duct_systems: Vec::new(),
                    vented: false,
                    ventilation_ach: None,
                    ventilation_sla: None,
                    height_m: None,
                    hpxml_location: None,
                });
            }
        }

        // Foundations
        if let Some(group) = enclosure.child("Foundations") {
            for node in group.children_named("Foundation") {
                let floor_area_m2 =
                    parse_value_with_units(node.child("FloorArea"), ValueKind::Area)?;

                // Determine vented status from <FoundationType>.
                // OCHRE hpxml.py:689-691: crawlspaces default to vented: true,
                // basements default to vented: false.
                let ft_child = node
                    .child("FoundationType")
                    .and_then(|ft| ft.children.first());
                let foundation_type = ft_child.map(|child| child.name.as_str());

                // A conditioned foundation is part of the conditioned space
                // (OS-HPXML geometry.rb `create_or_get_space`, 1704-1716; the
                // surfaces name it conditioned through `parse_zone_label`) and
                // has no Foundation zone of its own.
                if let Some(ft_child) = ft_child {
                    let foundation_id = element_id(node).unwrap_or_else(|| "unknown".to_string());
                    if foundation_node_is_conditioned(
                        ft_child,
                        &foundation_id,
                        total_conditioned_floors,
                        floors_above_grade,
                    )? {
                        merged = true;
                        continue;
                    }
                }

                let vented_explicit = ft_child
                    .and_then(|child| child.child("Vented"))
                    .map(|v| {
                        xs_boolean(
                            v,
                            "Foundation/FoundationType/Vented",
                            "Foundation",
                            &element_id(node).unwrap_or_else(|| "unknown".to_string()),
                        )
                    })
                    .transpose()?;
                let vented = match foundation_type {
                    Some("Crawlspace") => vented_explicit.unwrap_or(true),
                    Some("Basement") => vented_explicit.unwrap_or(false),
                    _ => vented_explicit.unwrap_or(false),
                };
                if vented_explicit.is_none() {
                    tracing::debug!(
                        foundation_type = foundation_type.unwrap_or("unknown"),
                        default_vented = vented,
                        "Foundation zone has no explicit <Vented> element; applying default"
                    );
                }
                #[cfg(feature = "observe")]
                {
                    tracing::debug!(
                        target: "observe",
                        foundation_type = foundation_type.unwrap_or("unknown"),
                        vented,
                    );
                }

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
                    height_m: None,
                    hpxml_location: None,
                });
            }
        }
    }

    // The conditioned zone. A merged conditioned foundation is part of it
    // (OS-HPXML geometry.rb `create_or_get_space`, 1704-1716), so its floor
    // area is the HPXML ConditionedFloorArea, the basement's included
    // (OCHRE hpxml.py:253, "indoor + foundation"); otherwise only the
    // above-grade share, the foundation zone holding the rest.
    zones.insert(
        "conditioned".to_string(),
        Zone {
            zone_type: ZoneType::Conditioned,
            floor_area_m2: if merged {
                conditioned_floor_area_m2
            } else {
                above_grade_floor_area_m2
            },
            volume_m3: None,
            attached_wall_ids: Vec::new(),
            duct_systems: Vec::new(),
            vented: false,
            ventilation_ach: None,
            ventilation_sla: None,
            height_m: None,
            hpxml_location: None,
        },
    );

    Ok((zones, merged))
}

fn ensure_referenced_zones_exist(boundaries: &[Boundary], zones: &mut HashMap<String, Zone>) {
    for boundary in boundaries {
        for zone_type in [&boundary.interior_zone, &boundary.exterior_zone]
            .into_iter()
            .flatten()
        {
            match zone_type {
                ZoneType::Attic | ZoneType::Garage | ZoneType::Foundation => {
                    let vented = matches!(zone_type, ZoneType::Attic);
                    zones.entry(zone_key(zone_type)).or_insert_with(|| {
                        tracing::debug!(
                            zone_type = ?zone_type,
                            vented,
                            "ensure_referenced_zones_exist creating zone"
                        );
                        Zone {
                            zone_type: zone_type.clone(),
                            floor_area_m2: None,
                            volume_m3: None,
                            attached_wall_ids: Vec::new(),
                            duct_systems: Vec::new(),
                            vented,
                            ventilation_ach: None,
                            ventilation_sla: None,
                            height_m: None,
                            hpxml_location: None,
                        }
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

fn parse_duct_systems(
    details: &XmlNode,
    zones: &mut HashMap<String, Zone>,
) -> Result<(), HpxmlError> {
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
                            if let Some(existing) = leakage_by_type.get(&dtype) {
                                tracing::warn!(
                                    duct_type = %dtype,
                                    existing_value = %existing,
                                    new_value = value / 100.0,
                                    "Duplicate DuctLeakageMeasurement for duct type; overwriting previous value"
                                );
                            }
                            leakage_by_type.insert(dtype, value / 100.0);
                        }
                        "fraction" => {
                            if let Some(existing) = leakage_by_type.get(&dtype) {
                                tracing::warn!(
                                    duct_type = %dtype,
                                    existing_value = %existing,
                                    new_value = %value,
                                    "Duplicate DuctLeakageMeasurement for duct type; overwriting previous value"
                                );
                            }
                            leakage_by_type.insert(dtype, value);
                        }
                        "cfm25" => {
                            if let Some(existing) = leakage_cfm25_by_type.get(&dtype) {
                                tracing::warn!(
                                    duct_type = %dtype,
                                    existing_cfm25 = %existing,
                                    new_cfm25 = %value,
                                    "Duplicate DuctLeakageMeasurement for duct type; overwriting previous value"
                                );
                            }
                            leakage_cfm25_by_type.insert(dtype, value);
                        }
                        _ => {
                            return Err(HpxmlError::UnrecognisedUnit {
                                value,
                                unit: units.clone(),
                                context: format!(
                                    "a DuctLeakageMeasurement on duct type '{dtype}' \
                                     (a fraction, percent or cfm25 value is required; \
                                     the measurement is dropped otherwise)"
                                ),
                            });
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

        let insulation_primary = parse_value_with_units(
            duct_node.first_descendant("DuctInsulationRValue"),
            ValueKind::RValue,
        )?;
        let insulation_fallback = parse_value_with_units(
            duct_node.first_descendant("InsulationRValue"),
            ValueKind::RValue,
        )?;
        let insulation_r_value_m2_k_w = insulation_primary.or(insulation_fallback);

        let surface_area_m2 = parse_value_with_units(
            duct_node.first_descendant("DuctSurfaceArea"),
            ValueKind::Area,
        )?;

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
    Ok(())
}

fn parse_nominal_r_layers(node: &XmlNode) -> Result<Vec<f64>, HpxmlError> {
    let mut layer_nodes = Vec::new();
    node.descendants("NominalRValue", &mut layer_nodes);
    let mut values = Vec::new();
    for layer in &layer_nodes {
        if let Some(v) = parse_value_with_units(Some(layer), ValueKind::RValue)? {
            values.push(v);
        }
    }
    Ok(values)
}

/// OS-HPXML v1.12's zone temperature capacitance multiplier (hpxml.rb:1051):
/// `AdvancedResearchFeatures/TemperatureCapacitanceMultiplier`, a positive
/// number, else 7 (defaults.rb:219-221). The pre-v1.11 element directly under
/// `SimulationControl`, which v1.12 no longer reads, is ignored with a
/// warning.
fn temperature_capacitance_multiplier(
    root: &XmlNode,
    parse_warnings: &mut Vec<Warning>,
) -> Result<f64, HpxmlError> {
    const ELEMENT: &str = "TemperatureCapacitanceMultiplier";
    let Some(control) = root.path(&["SoftwareInfo", "extension", "SimulationControl"]) else {
        return Ok(hares_envelope::boundary_rc::TEMPERATURE_CAPACITANCE_MULTIPLIER_DEFAULT);
    };
    if let Some(legacy) = control.child(ELEMENT) {
        parse_warnings.push(Warning::new(
            "hpxml",
            format!(
                "SimulationControl/{ELEMENT} ({}) is ignored, as OS-HPXML v1.12 reads it only \
                 under AdvancedResearchFeatures",
                legacy.text.trim()
            ),
        ));
    }
    let Some(node) = control.path(&["AdvancedResearchFeatures", ELEMENT]) else {
        return Ok(hares_envelope::boundary_rc::TEMPERATURE_CAPACITANCE_MULTIPLIER_DEFAULT);
    };
    match node.text_as_f64() {
        Some(value) if value.is_finite() && value > 0.0 => Ok(value),
        _ => Err(HpxmlError::Parse(
            format!(
                "AdvancedResearchFeatures/{ELEMENT} must be a positive number, got '{}'",
                node.text.trim()
            )
            .into(),
        )),
    }
}

fn find_descendant_f64(
    root: &XmlNode,
    name: &str,
    kind: ValueKind,
) -> Result<Option<f64>, HpxmlError> {
    match root.first_descendant(name) {
        Some(node) => parse_value_with_units(Some(node), kind),
        None => Ok(None),
    }
}

#[derive(Clone, Copy)]
pub(super) enum ValueKind {
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

pub(super) fn parse_value_with_units(
    node: Option<&XmlNode>,
    kind: ValueKind,
) -> Result<Option<f64>, HpxmlError> {
    let Some(node) = node else { return Ok(None) };
    let Some(value) = node.text_as_f64() else {
        return Ok(None);
    };
    let units = node
        .attrs
        .get("units")
        .or_else(|| node.attrs.get("unit"))
        .map(|s| normalize_ascii(s));

    match kind {
        ValueKind::Raw => Ok(Some(value)),
        ValueKind::Area => convert_area_to_m2(value, units.as_deref()).map(Some),
        ValueKind::UValue => convert_u_to_w_m2_k(value, units.as_deref()).map(Some),
        ValueKind::RValue => convert_r_to_m2_k_w(value, units.as_deref()).map(Some),
        ValueKind::Conductivity => convert_conductivity_to_w_m_k(value, units.as_deref()).map(Some),
        ValueKind::Length => convert_length_to_m(value, units.as_deref()).map(Some),
        ValueKind::Density => convert_density_to_kg_m3(value, units.as_deref()).map(Some),
        ValueKind::SpecificHeat => {
            convert_specific_heat_to_j_kg_k(value, units.as_deref()).map(Some)
        }
        ValueKind::Temperature => convert_temperature_to_c(value, units.as_deref()).map(Some),
        ValueKind::Volume => convert_volume_to_m3(value, units.as_deref()).map(Some),
    }
}

fn convert_area_to_m2(value: f64, units: Option<&str>) -> Result<f64, HpxmlError> {
    match units {
        Some("ft2") | Some("ft^2") | Some("ftsq") | Some("ftsq.") | Some("square feet") => {
            Ok(conv::area_ft2_to_m2(value))
        }
        Some("m2") | Some("m^2") | Some("sq m") | Some("square meters") => Ok(value),
        Some(unit) => Err(HpxmlError::UnrecognisedUnit {
            value,
            unit: unit.to_string(),
            context: "area".to_string(),
        }),
        None => {
            tracing::debug!(
                value,
                "Area value has no units attribute; assuming ft² and converting to m²"
            );
            Ok(conv::area_ft2_to_m2(value))
        }
    }
}

fn convert_volume_to_m3(value: f64, units: Option<&str>) -> Result<f64, HpxmlError> {
    match units {
        Some("ft3") | Some("ft^3") | Some("cubic feet") => Ok(conv::volume_ft3_to_m3(value)),
        Some("gal") | Some("gallon") | Some("gallons") => Ok(conv::volume_gal_to_m3(value)),
        Some("m3") | Some("m^3") | Some("cubic meters") => Ok(value),
        Some(unit) => Err(HpxmlError::UnrecognisedUnit {
            value,
            unit: unit.to_string(),
            context: "volume".to_string(),
        }),
        None => {
            tracing::warn!(
                value,
                "Volume value has no units attribute; assuming ft³ and converting to m³"
            );
            Ok(conv::volume_ft3_to_m3(value))
        }
    }
}

fn convert_u_to_w_m2_k(value: f64, units: Option<&str>) -> Result<f64, HpxmlError> {
    match units {
        Some("btu/hr-ft2-f")
        | Some("btu/hr-ft^2-f")
        | Some("btu/(h*ft2*f)")
        | Some("btu/(h-ft2-f)") => Ok(conv::u_value_ip_to_si(value)),
        Some("w/(m2*k)") | Some("w/m2-k") | Some("w/m2k") | Some("w/(m^2*k)") => Ok(value),
        Some(unit) => Err(HpxmlError::UnrecognisedUnit {
            value,
            unit: unit.to_string(),
            context: "U-value".to_string(),
        }),
        None => {
            tracing::debug!(
                value,
                "U-value has no units attribute; assuming BTU/(hr*ft2*F) and converting to W/(m2*K)"
            );
            Ok(conv::u_value_ip_to_si(value))
        }
    }
}

fn convert_r_to_m2_k_w(value: f64, units: Option<&str>) -> Result<f64, HpxmlError> {
    match units {
        Some("hr-ft2-f/btu") | Some("hr-ft^2-f/btu") | Some("h*ft2*f/btu") => {
            Ok(conv::r_value_ip_to_si(value))
        }
        Some("m2*k/w") | Some("m2k/w") | Some("m^2*k/w") | Some("k*m2/w") => Ok(value),
        Some(unit) => Err(HpxmlError::UnrecognisedUnit {
            value,
            unit: unit.to_string(),
            context: "R-value".to_string(),
        }),
        None => {
            tracing::debug!(
                value,
                "R-value has no units attribute; assuming hr*ft2*F/BTU and converting to m2*K/W"
            );
            Ok(conv::r_value_ip_to_si(value))
        }
    }
}

fn convert_conductivity_to_w_m_k(value: f64, units: Option<&str>) -> Result<f64, HpxmlError> {
    match units {
        Some("btu/hr-ft-f") | Some("btu/(h*ft*f)") => {
            Ok(conv::conductivity_btu_h_ft_f_to_w_m_k(value))
        }
        Some("btu-in/hr-ft2-f") | Some("btu in/hr ft2 f") | Some("btu*in/(h*ft2*f)") => {
            Ok(conv::conductivity_btu_in_h_ft2_f_to_w_m_k(value))
        }
        Some("w/(m*k)") | Some("w/m-k") | Some("w/mk") => Ok(value),
        Some(unit) => Err(HpxmlError::UnrecognisedUnit {
            value,
            unit: unit.to_string(),
            context: "conductivity".to_string(),
        }),
        None => {
            tracing::debug!(
                value,
                "Conductivity has no units attribute; assuming BTU*in/(hr*ft2*F) and converting"
            );
            Ok(conv::conductivity_btu_in_h_ft2_f_to_w_m_k(value))
        }
    }
}

fn convert_length_to_m(value: f64, units: Option<&str>) -> Result<f64, HpxmlError> {
    match units {
        Some("in") | Some("inch") | Some("inches") => Ok(conv::length_in_to_m(value)),
        Some("ft") | Some("feet") => Ok(conv::length_ft_to_m(value)),
        Some("m") | Some("meter") | Some("meters") => Ok(value),
        Some("cm") | Some("centimeters") => Ok(conv::length_cm_to_m(value)),
        Some(unit) => Err(HpxmlError::UnrecognisedUnit {
            value,
            unit: unit.to_string(),
            context: "length".to_string(),
        }),
        None => {
            tracing::debug!(
                value,
                "Length value has no units attribute; assuming feet and converting to meters"
            );
            Ok(conv::length_ft_to_m(value))
        }
    }
}

fn convert_density_to_kg_m3(value: f64, units: Option<&str>) -> Result<f64, HpxmlError> {
    match units {
        Some("lb/ft3") | Some("lb/ft^3") | Some("lbm/ft3") => {
            Ok(conv::density_lb_ft3_to_kg_m3(value))
        }
        Some("kg/m3") | Some("kg/m^3") | Some("kg m^-3") => Ok(value),
        Some(unit) => Err(HpxmlError::UnrecognisedUnit {
            value,
            unit: unit.to_string(),
            context: "density".to_string(),
        }),
        None => {
            tracing::debug!(
                value,
                "Density has no units attribute; assuming lb/ft3 and converting"
            );
            Ok(conv::density_lb_ft3_to_kg_m3(value))
        }
    }
}

fn convert_specific_heat_to_j_kg_k(value: f64, units: Option<&str>) -> Result<f64, HpxmlError> {
    match units {
        Some("btu/lb-f") | Some("btu/(lb*f)") => Ok(conv::specific_heat_btu_lb_f_to_j_kg_k(value)),
        Some("j/(kg*k)") | Some("j/kg-k") | Some("j/kgk") => Ok(value),
        Some("kj/(kg*k)") | Some("kj/kg-k") | Some("kj/kgk") => {
            Ok(conv::specific_heat_kj_kg_k_to_j_kg_k(value))
        }
        Some(unit) => Err(HpxmlError::UnrecognisedUnit {
            value,
            unit: unit.to_string(),
            context: "specific heat".to_string(),
        }),
        None => {
            tracing::debug!(
                value,
                "Specific heat has no units attribute; assuming Btu/(lb*F) and converting"
            );
            Ok(conv::specific_heat_btu_lb_f_to_j_kg_k(value))
        }
    }
}

fn convert_temperature_to_c(value: f64, units: Option<&str>) -> Result<f64, HpxmlError> {
    match units {
        Some("F") | Some("f") | Some("degF") | Some("degf") | Some("fahrenheit") => {
            Ok(conv::temperature_f_to_c(value))
        }
        Some("C") | Some("c") | Some("degC") | Some("degc") | Some("celsius") => Ok(value),
        Some(unit) => Err(HpxmlError::UnrecognisedUnit {
            value,
            unit: unit.to_string(),
            context: "temperature".to_string(),
        }),
        None => {
            tracing::debug!(
                value,
                "Temperature value has no units attribute; assuming F and converting to C"
            );
            Ok(conv::temperature_f_to_c(value))
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
            return Err(HpxmlError::Parse(
                format!(
                    "unrecognised UnitofMeasure '{unit_str}' on BuildingAirLeakage -- \
                 expected ACH, CFM, ACHnatural, or CFMnatural"
                )
                .into(),
            ));
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
            return Err(HpxmlError::Parse(
                format!(
                    "unrecognised units attribute '{unit_str}' on AirLeakage -- \
                 expected ACH, ACH50, CFM, or CFM50"
                )
                .into(),
            ));
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
            return Err(HpxmlError::Parse(
                format!(
                    "unrecognised UnitofMeasure '{unit_str}' on BuildingAirLeakage -- \
                 expected ACH, CFM, ACHnatural, or CFMnatural"
                )
                .into(),
            ));
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
            return Err(HpxmlError::Parse(
                format!(
                    "unrecognised units attribute '{unit_str}' on AirLeakage -- \
                 expected ACH, ACH50, CFM, or CFM50"
                )
                .into(),
            ));
        }
        // No units attribute — bare value (HPXML convention: ACH50 when HousePressure=50).
        return Ok((al.text_as_f64(), None));
    }
    Ok((None, None))
}

fn parse_site_type(text: &str) -> Result<SiteType, HpxmlError> {
    match normalize_ascii(text).as_str() {
        "rural" => Ok(SiteType::Rural),
        "suburban" => Ok(SiteType::Suburban),
        "urban" => Ok(SiteType::Urban),
        _ => Err(HpxmlError::Parse(
            format!(
                "invalid SiteType value '{}'; allowed values are 'rural', 'suburban' and 'urban'",
                text.trim()
            )
            .into(),
        )),
    }
}

fn parse_shielding_of_home(text: &str) -> Result<ShieldingOfHome, HpxmlError> {
    match normalize_ascii(text).as_str() {
        "normal" => Ok(ShieldingOfHome::Normal),
        "exposed" => Ok(ShieldingOfHome::Exposed),
        "well-shielded" => Ok(ShieldingOfHome::WellShielded),
        _ => Err(HpxmlError::Parse(
            format!(
                "invalid ShieldingofHome value '{}'; allowed values are 'normal', 'exposed' and 'well-shielded'",
                text.trim()
            )
            .into(),
        )),
    }
}

fn parse_zone_ref(node: Option<&XmlNode>) -> Option<ZoneType> {
    let node = node?;
    Some(parse_zone_label(node.text.trim()))
}

pub(crate) fn parse_zone_label(text: &str) -> ZoneType {
    let norm = normalize_ascii(text);
    // A conditioned foundation is conditioned space, not a separate zone:
    // OS-HPXML merges every conditioned location ("basement - conditioned",
    // "crawlspace - conditioned") into the one conditioned space
    // (geometry.rb `create_or_get_space`, 1704-1716; hpxml.rb
    // `conditioned_locations`, 12311-12316). "unconditioned" contains
    // "conditioned" as a substring, so the negation is checked first.
    let is_conditioned = norm.contains("condition") && !norm.contains("unconditioned");
    if norm.contains("attic") {
        ZoneType::Attic
    } else if norm.contains("garage") {
        ZoneType::Garage
    } else if norm.contains("foundation") || norm.contains("basement") || norm.contains("crawl") {
        if is_conditioned {
            ZoneType::Conditioned
        } else {
            ZoneType::Foundation
        }
    } else if is_conditioned || norm == "living space" {
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

/// Whether an HPXML location label names the living space itself rather
/// than a conditioned foundation merged into it. OCHRE builds the
/// "Interior Wall" surface only in the living zone (hpxml.py
/// `add_interior_boundaries`), so the splits that assume that surface
/// (the HPWH wall interaction) key on the living space, not on any
/// conditioned label.
pub(crate) fn is_living_space_label(text: &str) -> bool {
    let norm = normalize_ascii(text);
    norm == "living space"
        || (norm.contains("condition")
            && !norm.contains("unconditioned")
            && !norm.contains("basement")
            && !norm.contains("crawl")
            && !norm.contains("foundation"))
}

/// Rewrite `ZoneType::Adjacent` to match the non-Adjacent zone in the pair.
///
/// OCHRE hpxml.py:96-97: when exterior is `"Adjacent"`, rewrite `exterior = interior`.
/// This makes the adiabatic-same-zone intent explicit and eliminates the fragile
/// dependency on `find_zone_idx` fallback behavior in the downstream conversions layer.
///
/// If both zones are `Adjacent` (two different adjacent dwelling units on each side),
/// both are rewritten to `Conditioned` — the most common zone type for party walls
/// between dwelling units. This produces a same-zone pair that classifies as
/// InternalMass, consistent with OCHRE's treatment of any same-zone boundary.
fn rewrite_adjacent_zone_pair(
    interior: Option<ZoneType>,
    exterior: Option<ZoneType>,
) -> (Option<ZoneType>, Option<ZoneType>) {
    match (interior, exterior) {
        (Some(ZoneType::Adjacent), Some(ZoneType::Adjacent)) => {
            (Some(ZoneType::Conditioned), Some(ZoneType::Conditioned))
        }
        (Some(ZoneType::Adjacent), Some(ref ext)) => (Some(ext.clone()), Some(ext.clone())),
        (Some(ref int), Some(ZoneType::Adjacent)) => (Some(int.clone()), Some(int.clone())),
        (None, Some(ZoneType::Adjacent)) => (None, None),
        (Some(ZoneType::Adjacent), None) => (None, None),
        (int, ext) => (int, ext),
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

/// Attempt to infer roof tilt from attic geometry for roofs that are missing
/// an explicit HPXML `<Pitch>` element.
///
/// Uses the gable relationship
/// `attic_height = sqrt(gable_area * tan(tilt))` and the gable triangular
/// area formula `gable_area = W² * tan(tilt) / 4`, where W is the building
/// width (the dimension the gable sits on). Solving for tilt:
///
/// ```text
/// tan(tilt) = 4 * gable_area / W²
/// tilt = atan(4 * gable_area / floor_area)   // assuming W = sqrt(floor_area)
/// ```
///
/// The square-plan assumption is an approximation; the result is capped to
/// the residential plausible range of 1:12–12:12 (4.8°–45°).
///
/// Only acts on boundary IDs listed in `pitch_absent_ids` (roofs that were
/// parsed without an explicit `<Pitch>`). Roofs with explicit Pitch values
/// are never modified.
fn infer_roof_tilt_from_geometry(boundaries: &mut [Boundary], pitch_absent_ids: &[String]) {
    if pitch_absent_ids.is_empty() {
        return;
    }

    // Find attic floor area: Floor boundary between Conditioned and Attic.
    let attic_floor_area = boundaries
        .iter()
        .find(|b| {
            b.boundary_type == BoundaryType::Floor
                && ((b.interior_zone.as_ref() == Some(&ZoneType::Conditioned)
                    && b.exterior_zone.as_ref() == Some(&ZoneType::Attic))
                    || (b.interior_zone.as_ref() == Some(&ZoneType::Attic)
                        && b.exterior_zone.as_ref() == Some(&ZoneType::Conditioned)))
        })
        .map(|b| b.area_m2);
    let Some(floor_area) = attic_floor_area else {
        return;
    };
    if floor_area <= 0.0 {
        return;
    }

    // Find gable end wall areas: Wall, interior=Attic, exterior=Outdoor.
    let mut gable_areas: Vec<f64> = boundaries
        .iter()
        .filter(|b| {
            b.boundary_type == BoundaryType::Wall
                && b.interior_zone.as_ref() == Some(&ZoneType::Attic)
                && matches!(b.exterior_zone.as_ref(), Some(&ZoneType::Outdoor) | None)
        })
        .map(|b| b.area_m2)
        .collect();
    if gable_areas.is_empty() {
        return;
    }

    // Use the median gable wall area, the most representative.
    gable_areas.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let gable_area = gable_areas[gable_areas.len() / 2];
    if gable_area <= 0.0 {
        return;
    }

    // tan(tilt) = 4 * gable_area / W², assuming square W = sqrt(floor_area)
    let tan_tilt = 4.0 * gable_area / floor_area;
    if tan_tilt <= 0.0 || !tan_tilt.is_finite() {
        return;
    }

    let inferred_tilt_deg = tan_tilt.atan().to_degrees();

    // Cap to plausible residential range: 1:12 (4.8°) to 12:12 (45°).
    // Values outside this range indicate the square-plan assumption is
    // not valid for this building.
    if !(4.5..=46.0).contains(&inferred_tilt_deg) {
        tracing::warn!(
            inferred_tilt_deg,
            gable_area_m2 = gable_area,
            attic_floor_area_m2 = floor_area,
            "geometric tilt inference produced implausible value; \
             keeping 4:12 default for roofs with absent Pitch"
        );
        return;
    }

    // Apply the inferred tilt to all Roof boundaries that have absent Pitch.
    for bd in boundaries.iter_mut() {
        if bd.boundary_type == BoundaryType::Roof && pitch_absent_ids.iter().any(|id| id == &bd.id)
        {
            bd.tilt_deg = Some(inferred_tilt_deg);
            #[cfg(feature = "observe")]
            {
                tracing::info!(
                    target: "observe",
                    column = "pitch_source",
                    boundary_id = %bd.id,
                    pitch_source = "inferred_from_geometry",
                    inferred_tilt_deg = inferred_tilt_deg,
                    gable_area_m2 = gable_area,
                    attic_floor_area_m2 = floor_area,
                    "inferred roof tilt from attic geometry"
                );
            }
        }
    }
}

/// A vented crawlspace or vented attic with no `VentilationRate` takes
/// OS-HPXML's default specific leakage area, 1/150 and 1/300 rounded to six
/// places (defaults.rb:1120-1131 and 1022-1031, values at 5717-5727, after
/// ANSI/RESNET/ICC 301 Table 4.2.2(1)), recorded as a warning.
fn default_vented_space_sla(zone: &mut Zone, warnings: &mut Vec<Warning>) {
    if zone.ventilation_sla.is_some() || zone.ventilation_ach.is_some() {
        return;
    }
    let denominator = match zone.hpxml_location.as_deref() {
        Some("crawlspace - vented") => 150.0,
        Some("attic - vented") => 300.0,
        _ => return,
    };
    let sla = ((1.0 / denominator) * 1e6_f64).round() / 1e6;
    let location = zone.hpxml_location.as_deref().unwrap_or_default();
    warnings.push(Warning::new(
        "hpxml",
        format!(
            "'{location}' has no VentilationRate; its specific leakage area defaults to \
             {sla} as OS-HPXML does"
        ),
    ));
    zone.ventilation_sla = Some(sla);
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

/// Gated invariant: every dwelling with a named foundation type (Basement or
/// Crawlspace) must have a Foundation thermal zone. A missing zone when the
/// foundation type is set means foundation thermal mass is absent from the RC
/// network.
///
/// Runs in every build profile as a warning diagnostic.
pub fn check_foundation_zone_invariant(building: &Building) {
    let has_foundation_zone = building
        .zones
        .iter()
        .any(|z| z.zone_type == ZoneType::Foundation);

    // A conditioned foundation merged into the conditioned space has no
    // Foundation zone by design: its mass and surfaces are the conditioned
    // zone's (OS-HPXML geometry.rb `create_or_get_space`, 1704-1716).
    let merged = building.conditioned_foundation_merged;

    if !has_foundation_zone
        && !merged
        && let Some(ref fnd_name) = building.foundation_name
    {
        tracing::warn!(
            foundation_name = %fnd_name,
            "Foundation zone missing despite foundation type being set. \
                 Foundation thermal mass is absent from the RC network."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BoundaryType, DuctType, HpxmlError, ZoneType, assembly_framing_factor, parse_building,
        parse_xml_document,
    };
    use crate::hpxml::xml_helpers::assert_reads_xs_boolean;

    /// HPXML's site types parse whatever their case and spacing; any other
    /// value is an error that keeps the file's own text.
    #[test]
    fn site_types_parse_case_insensitively_and_keep_unknown_text() {
        use super::{SiteType, parse_site_type};
        assert_eq!(parse_site_type("Rural").unwrap(), SiteType::Rural);
        assert_eq!(parse_site_type(" SUBURBAN ").unwrap(), SiteType::Suburban);
        assert_eq!(parse_site_type("urban").unwrap(), SiteType::Urban);
        let err = parse_site_type("Coastal").expect_err("an unknown site type must fail");
        assert!(format!("{err}").contains("'Coastal'"), "{err}");
    }

    const SAMPLE_XML: &str = r#"
<HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site>
          <Elevation units="ft">5280</Elevation>
          <SiteType>suburban</SiteType>
          <ShieldingofHome>normal</ShieldingofHome>
        </Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="ft2">2152</ConditionedFloorArea>
          <ConditionedBuildingVolume units="ft3">17216</ConditionedBuildingVolume>
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
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
            (building.conditioned_volume_m3 - expected_volume_m3).abs() < 0.01,
            "conditioned_volume_m3: got {}, expected {}",
            building.conditioned_volume_m3,
            expected_volume_m3,
        );

        // Ceiling height: volume / floor_area
        let expected_floor_area_m2 = 2152.0 * 0.092_903_04;
        let expected_ceiling_height = expected_volume_m3 / expected_floor_area_m2;
        assert!((building.ceiling_height_m - expected_ceiling_height).abs() < 1e-6,);

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

    /// A Site with explicit Latitude, Longitude, and TimeZone/UTCOffset is
    /// parsed into the corresponding `Site` fields. These feed the
    /// site-location resolver and ultimately the solar-position calculation.
    #[test]
    fn parses_site_latitude_longitude_and_utc_offset() {
        const XML: &str = r#"
<HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site>
          <Latitude>33.52</Latitude>
          <Longitude>-86.81</Longitude>
          <Elevation units="ft">600</Elevation>
          <TimeZone>
            <UTCOffset>-6</UTCOffset>
            <DSTObserved>true</DSTObserved>
          </TimeZone>
        </Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="ft2">2000</ConditionedFloorArea>
          <ConditionedBuildingVolume units="ft3">16000</ConditionedBuildingVolume>
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <Walls>
          <Wall>
            <SystemIdentifier id="Wall1"/>
            <InteriorAdjacentTo>conditioned space</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <Area units="ft2">100</Area>
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
      </Enclosure>
    </BuildingDetails>
  </Building>
</HPXML>
"#;
        let building = parse_building(XML).expect("parser success");
        assert_eq!(building.site.latitude_deg, Some(33.52));
        assert_eq!(building.site.longitude_deg, Some(-86.81));
        assert_eq!(building.site.utc_offset_h, Some(-6.0));
    }

    /// When the Site omits TimeZone/UTCOffset (and lat/lon), those fields parse
    /// as `None` so the resolver can fall back to the weather file or a
    /// coordinate lookup rather than assuming a wrong offset.
    #[test]
    fn site_without_timezone_yields_none_utc_offset() {
        // SAMPLE_XML's Site has Elevation/SiteType/ShieldingofHome only.
        let building = parse_building(SAMPLE_XML).expect("parser success");
        assert_eq!(building.site.utc_offset_h, None);
        assert_eq!(building.site.latitude_deg, None);
        assert_eq!(building.site.longitude_deg, None);
    }

    /// The site's shielding is read from HPXML's `ShieldingofHome` (lower-case
    /// "of", as the schema spells it, HPXML.xsd:4479).
    #[test]
    fn shielding_is_read_from_the_schema_element() {
        let building = parse_building(SAMPLE_XML).expect("parser success");
        assert_eq!(
            building.site.shielding_of_home,
            Some(super::ShieldingOfHome::Normal)
        );
    }

    #[test]
    fn attic_vented_true_by_default_when_no_attic_type_specified() {
        // SAMPLE_XML has <Attics><Attic> with FloorArea but no <AtticType>.
        // Default vented = true (OCHRE hpxml.py:635 Vented=True).
        let building = parse_building(SAMPLE_XML).expect("parse should succeed");
        let attic = building
            .zones
            .iter()
            .find(|z| matches!(z.zone_type, ZoneType::Attic))
            .expect("attic zone expected");
        assert!(
            attic.vented,
            "attic zone without <AtticType> must default to vented: true"
        );
    }

    #[test]
    fn attic_vented_true_from_explicit_vented_element() {
        let xml = SAMPLE_XML.replace(
            "<Attics>\n          <Attic>\n            <FloorArea units=\"ft2\">500</FloorArea>\n          </Attic>\n        </Attics>",
            "<Attics>\n          <Attic>\n            <AtticType>\n              <Attic>\n                <Vented>true</Vented>\n              </Attic>\n            </AtticType>\n            <FloorArea units=\"ft2\">500</FloorArea>\n          </Attic>\n        </Attics>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let attic = building
            .zones
            .iter()
            .find(|z| matches!(z.zone_type, ZoneType::Attic))
            .expect("attic zone expected");
        assert!(
            attic.vented,
            "explicit <Vented>true</Vented> → vented: true"
        );
    }

    #[test]
    fn attic_vented_false_from_explicit_vented_element() {
        let xml = SAMPLE_XML.replace(
            "<Attics>\n          <Attic>\n            <FloorArea units=\"ft2\">500</FloorArea>\n          </Attic>\n        </Attics>",
            "<Attics>\n          <Attic>\n            <AtticType>\n              <Attic>\n                <Vented>false</Vented>\n              </Attic>\n            </AtticType>\n            <FloorArea units=\"ft2\">500</FloorArea>\n          </Attic>\n        </Attics>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let attic = building
            .zones
            .iter()
            .find(|z| matches!(z.zone_type, ZoneType::Attic))
            .expect("attic zone expected");
        assert!(
            !attic.vented,
            "explicit <Vented>false</Vented> → vented: false"
        );
    }

    #[test]
    fn attic_vented_true_when_created_by_boundary_reference_only() {
        // Remove <Attics> group entirely; attic zone created solely by
        // ensure_referenced_zones_exist when Roof boundary references "attic vented".
        let xml = SAMPLE_XML.replace(
            "\n        <Attics>\n          <Attic>\n            <FloorArea units=\"ft2\">500</FloorArea>\n          </Attic>\n        </Attics>",
            "",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let attic = building
            .zones
            .iter()
            .find(|z| matches!(z.zone_type, ZoneType::Attic))
            .expect("attic zone expected from boundary reference");
        assert!(
            attic.vented,
            "attic created by boundary reference (no <Attics> group) must default to vented: true"
        );
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

    #[test]
    fn foundation_wall_missing_area_defaults_to_zero() {
        let xml = SAMPLE_XML.replace("<Area units=\"ft2\">60</Area>", "");
        let building = parse_building(&xml).expect("foundation wall without Area should parse");
        let fw_boundary = building
            .boundaries
            .iter()
            .find(|b| matches!(b.boundary_type, BoundaryType::FoundationWall));
        assert!(
            fw_boundary.is_some(),
            "foundation wall boundary should be present"
        );
        assert_eq!(
            fw_boundary.unwrap().area_m2,
            0.0,
            "FoundationWall with missing Area should default to 0.0"
        );
    }

    #[test]
    fn foundation_wall_area_derived_from_length_and_height() {
        let xml = SAMPLE_XML.replace(
            "<Area units=\"ft2\">60</Area>",
            "<Length units=\"ft\">30</Length><Height units=\"ft\">8</Height>",
        );
        let building =
            parse_building(&xml).expect("foundation wall with Length+Height should parse");
        let fw = building
            .boundaries
            .iter()
            .find(|b| matches!(b.boundary_type, BoundaryType::FoundationWall))
            .expect("foundation wall boundary should be present");
        let length_m = 30.0 * 0.3048;
        let height_m = 8.0 * 0.3048;
        let expected = length_m * height_m;
        assert!(
            (fw.area_m2 - expected).abs() < 1e-6,
            "derived area: got {}, expected {} (30ft × 8ft)",
            fw.area_m2,
            expected
        );
        assert!(fw.area_m2 > 0.0, "derived area must be positive");
    }

    #[test]
    fn foundation_wall_non_positive_area_errors() {
        let xml = SAMPLE_XML.replace(
            "<Area units=\"ft2\">60</Area>",
            "<Area units=\"ft2\">-5</Area>",
        );
        let err =
            parse_building(&xml).expect_err("foundation wall with negative area should error");
        assert!(matches!(err, HpxmlError::Parse(_)));
        let msg = err.to_string();
        assert!(msg.contains("foundation wall 'FoundationWall1' has non-positive area"));
    }

    /// A foundation slab sizes its space's floor and volume, so a slab with
    /// no Area is an error naming its location (OS-HPXML requires it).
    #[test]
    fn slab_missing_area_is_an_error() {
        let xml = SAMPLE_XML.replace("<Area units=\"ft2\">80</Area>", "");
        let err = parse_building(&xml).expect_err("a slab without Area must fail");
        assert!(
            err.to_string()
                .contains("slab in 'basement - conditioned' has no Area"),
            "got: {err}"
        );
    }

    #[test]
    fn slab_non_positive_area_errors() {
        let xml = SAMPLE_XML.replace(
            "<Area units=\"ft2\">80</Area>",
            "<Area units=\"ft2\">0</Area>",
        );
        let err = parse_building(&xml).expect_err("slab with zero area should error");
        assert!(matches!(err, HpxmlError::Parse(_)));
        let msg = err.to_string();
        assert!(msg.contains("slab 'Slab1' has non-positive area: 0"));
    }

    #[test]
    fn roof_missing_area_returns_error() {
        let xml = SAMPLE_XML.replace("<Area units=\"ft2\">120</Area>", "");
        let err = parse_building(&xml).expect_err("expected missing roof area failure");
        assert!(matches!(err, HpxmlError::Parse(_)));
        let msg = err.to_string();
        assert!(msg.contains("roof 'Roof1' is missing required Area element"));
    }

    #[test]
    fn floor_missing_area_returns_error() {
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
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <FrameFloors>
          <FrameFloor>
            <SystemIdentifier id="Floor1"/>
            <InteriorAdjacentTo>conditioned space</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
          </FrameFloor>
        </FrameFloors>
      </Enclosure>
    </BuildingDetails>
  </Building>
</HPXML>"#;
        let err = parse_building(xml).expect_err("expected missing floor area failure");
        assert!(matches!(err, HpxmlError::Parse(_)));
        let msg = err.to_string();
        assert!(msg.contains("floor 'Floor1' is missing required Area element"));
    }

    #[test]
    fn missing_both_floor_area_and_volume_returns_error() {
        let xml = SAMPLE_XML
            .replace(
                "<ConditionedFloorArea units=\"ft2\">2152</ConditionedFloorArea>",
                "",
            )
            .replace(
                "<ConditionedBuildingVolume units=\"ft3\">17216</ConditionedBuildingVolume>",
                "",
            );
        let err = parse_building(&xml).expect_err("expected missing field failure");
        assert!(matches!(err, HpxmlError::Parse(_)));
        let msg = err.to_string();
        assert!(msg.contains("missing ConditionedFloorArea"), "got: {msg}");
    }

    /// No ConditionedBuildingVolume: OS-HPXML's default, the conditioned
    /// floor area times an 8 ft ceiling, recorded as a warning.
    #[test]
    fn missing_volume_only_takes_the_os_hpxml_default() {
        let xml = SAMPLE_XML.replace(
            "<ConditionedBuildingVolume units=\"ft3\">17216</ConditionedBuildingVolume>",
            "",
        );
        let building = parse_building(&xml).expect("a missing volume takes the default");
        assert_eq!(
            building.conditioned_volume_m3,
            hares_physics::units::volume_ft3_to_m3(2152.0 * 8.0)
        );
        assert!(
            building
                .parse_warnings
                .iter()
                .any(|w| w.message.contains("ConditionedBuildingVolume")),
            "got {:?}",
            building.parse_warnings
        );
    }

    #[test]
    fn missing_floor_area_only_returns_error() {
        let xml = SAMPLE_XML.replace(
            "<ConditionedFloorArea units=\"ft2\">2152</ConditionedFloorArea>",
            "",
        );
        let err = parse_building(&xml).expect_err("expected missing floor area failure");
        assert!(matches!(err, HpxmlError::Parse(_)));
        let msg = err.to_string();
        assert!(msg.contains("missing ConditionedFloorArea"), "got: {msg}");
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
          <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
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

    fn xml_with_skylight(skylight_xml: &str) -> String {
        format!(
            r#"
<HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
          <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <Walls />
        <Skylights>
          {skylight_xml}
        </Skylights>
      </Enclosure>
    </BuildingDetails>
  </Building>
</HPXML>
"#
        )
    }

    // ── Skylight parsing tests ──────────────────────────────────────────

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

    /// The foundation is as tall as its tallest foundation wall, not its
    /// first, and its volume is the slab area times that height
    /// (OS-HPXML geometry.rb `calculate_zone_volume`); the declared
    /// Foundation FloorArea does not size it.
    #[test]
    fn foundation_zone_volume_is_the_slab_area_times_the_tallest_wall() {
        let wall = |id: &str, height_ft: f64| {
            format!(
                "<FoundationWall>\n            <SystemIdentifier id=\"{id}\"/>\n            <InteriorAdjacentTo>basement - unconditioned</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>ground</ExteriorAdjacentTo>\n            <Area units=\"ft2\">60</Area>\n            <Height units=\"ft\">{height_ft}</Height>\n          </FoundationWall>"
            )
        };
        let xml = SAMPLE_XML
            .replace(
                "<FoundationWall>\n            <SystemIdentifier id=\"FoundationWall1\"/>\n            <InteriorAdjacentTo>basement - conditioned</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>ground</ExteriorAdjacentTo>\n            <Area units=\"ft2\">60</Area>\n          </FoundationWall>",
                &format!("{}{}", wall("FoundationWall1", 3.0), wall("FoundationWall2", 7.0)),
            )
            .replace(
                "<Slab>\n            <SystemIdentifier id=\"Slab1\"/>\n            <InteriorAdjacentTo>basement - conditioned</InteriorAdjacentTo>",
                "<Slab>\n            <SystemIdentifier id=\"Slab1\"/>\n            <InteriorAdjacentTo>basement - unconditioned</InteriorAdjacentTo>",
            );

        let building = parse_building(&xml).expect("parse should succeed");
        let foundation = building
            .zones
            .iter()
            .find(|z| matches!(z.zone_type, ZoneType::Foundation))
            .expect("foundation zone expected");

        let height_m = hares_physics::units::length_ft_to_m(7.0);
        let expected_volume_m3 = hares_physics::units::area_ft2_to_m2(80.0) * height_m;
        let actual_volume_m3 = foundation.volume_m3.expect("foundation volume expected");
        assert!(
            (actual_volume_m3 - expected_volume_m3).abs() < 1e-9,
            "foundation volume: got {actual_volume_m3}, expected {expected_volume_m3}"
        );
        assert_eq!(foundation.height_m, Some(height_m));
    }

    #[test]
    fn foundation_floor_area_is_derived_from_the_foundation_slabs() {
        // The document declares no <Foundation><FloorArea>: the foundation's
        // floor area is its slabs' area sum (OS-HPXML v1.12.0 geometry.rb
        // 1315-1324, calculate_zone_volume; the conditioned floor area's
        // split is the floors and slabs adjacent to conditioned space,
        // geometry.rb 750-771, apply_conditioned_floor_area), not the
        // floor-count ratio.
        let xml = SAMPLE_XML
            .replace(
                "</BuildingConstruction>",
                "<NumberofConditionedFloors>2</NumberofConditionedFloors>\n          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>\n        </BuildingConstruction>",
            )
            .replace(
                "<Foundation>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
                "<Foundation>\n            <FoundationType><Basement><Conditioned>false</Conditioned></Basement></FoundationType>\n          </Foundation>",
            )
            .replace(
                "<InteriorAdjacentTo>basement - conditioned</InteriorAdjacentTo>",
                "<InteriorAdjacentTo>basement - unconditioned</InteriorAdjacentTo>",
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

        // The slab adjacent to the basement is 80 ft2: the split takes the
        // foundation's slab area, not half the CFA (the floor-count ratio).
        let conditioned_area_m2 = conditioned
            .floor_area_m2
            .expect("conditioned area expected");
        let expected_conditioned_area_m2 = (2152.0 - 80.0) * 0.092_903_04;
        assert!(
            (conditioned_area_m2 - expected_conditioned_area_m2).abs() < 1e-6,
            "conditioned area from the slab-derived split: got {}, expected {}",
            conditioned_area_m2,
            expected_conditioned_area_m2
        );
        let foundation_area_m2 = foundation.floor_area_m2.expect("foundation area expected");
        let expected_foundation_area_m2 = 80.0 * 0.092_903_04;
        assert!(
            (foundation_area_m2 - expected_foundation_area_m2).abs() < 1e-6,
            "foundation area from its slabs: got {}, expected {}",
            foundation_area_m2,
            expected_foundation_area_m2
        );
        // The derivation is recorded, citing the reference's rule.
        let derivation = building
            .parse_warnings
            .iter()
            .find(|w| w.message.contains("slab"))
            .expect("the slab derivation is recorded as a parse warning");
        assert!(
            derivation.message.contains("geometry.rb"),
            "the warning cites the reference: {}",
            derivation.message
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

        let ceiling_height_m = building.ceiling_height_m;
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

    #[test]
    fn unknown_foundation_type_rejected_with_parse_error() {
        let xml = SAMPLE_XML.replace(
            "<Foundation>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
            "<Foundation>\n            <FoundationType><UnknownType/></FoundationType>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
        );
        let result = parse_building(&xml);
        assert!(
            result.is_err(),
            "expected parse error for unknown FoundationType"
        );
        let err = result.unwrap_err();
        assert!(
            matches!(&err, HpxmlError::Parse(msg) if msg.message.contains("UnknownType")),
            "expected Parse error mentioning UnknownType, got: {:?}",
            err
        );
    }

    #[test]
    fn missing_foundations_group_applies_slab_on_grade_default() {
        // Remove <Foundations> entirely — the parser should succeed and apply
        // slab-on-grade defaults (foundation_name=None). A Foundation zone may
        // still be created by ensure_referenced_zones_exist when boundaries
        // reference it, but foundation_name stays None.
        let xml = SAMPLE_XML.replace(
            "\n        <Foundations>\n          <Foundation>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>\n        </Foundations>",
            "",
        );
        let building = parse_building(&xml).expect("parse should succeed without <Foundations>");
        // Slab-on-grade default: foundation_name is None.
        assert!(
            building.foundation_name.is_none(),
            "foundation_name must be None for slab-on-grade default, got {:?}",
            building.foundation_name
        );
        // Verify the parse produced a valid building with zones.
        assert!(
            !building.zones.is_empty(),
            "building must have at least one zone"
        );
        assert!(
            building
                .zones
                .iter()
                .any(|z| z.zone_type == ZoneType::Conditioned),
            "conditioned zone expected"
        );
    }

    // ── Foundation zone vented defaults ─────────────────────────────────

    #[test]
    fn crawlspace_defaults_vented_true_when_no_explicit_vented_element() {
        // OCHRE hpxml.py:689: crawlspaces default to vented: true.
        let xml = SAMPLE_XML.replace(
            "<Foundation>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
            "<Foundation>\n            <FoundationType><Crawlspace/></FoundationType>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let foundation = building
            .zones
            .iter()
            .find(|z| matches!(z.zone_type, ZoneType::Foundation))
            .expect("foundation zone expected");
        assert!(
            foundation.vented,
            "crawlspace without explicit <Vented> must default to vented: true"
        );
    }

    #[test]
    fn crawlspace_explicit_vented_false_overrides_default() {
        let xml = SAMPLE_XML.replace(
            "<Foundation>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
            "<Foundation>\n            <FoundationType><Crawlspace><Vented>false</Vented></Crawlspace></FoundationType>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let foundation = building
            .zones
            .iter()
            .find(|z| matches!(z.zone_type, ZoneType::Foundation))
            .expect("foundation zone expected");
        assert!(
            !foundation.vented,
            "explicit <Vented>false</Vented> must override crawlspace default → vented: false"
        );
    }

    #[test]
    fn basement_defaults_vented_false_when_no_explicit_vented_element() {
        // OCHRE hpxml.py:691: basements default to vented: false.
        let xml = SAMPLE_XML.replace(
            "<Foundation>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
            "<Foundation>\n            <FoundationType><Basement/></FoundationType>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let foundation = building
            .zones
            .iter()
            .find(|z| matches!(z.zone_type, ZoneType::Foundation))
            .expect("foundation zone expected");
        assert!(
            !foundation.vented,
            "basement without explicit <Vented> must default to vented: false"
        );
    }

    #[test]
    fn basement_explicit_vented_true_overrides_default() {
        let xml = SAMPLE_XML.replace(
            "<Foundation>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
            "<Foundation>\n            <FoundationType><Basement><Vented>true</Vented></Basement></FoundationType>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let foundation = building
            .zones
            .iter()
            .find(|z| matches!(z.zone_type, ZoneType::Foundation))
            .expect("foundation zone expected");
        assert!(
            foundation.vented,
            "explicit <Vented>true</Vented> must override basement default → vented: true"
        );
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
        // "other housing unit" parses to ZoneType::Adjacent, then is rewritten
        // to match the non-Adjacent zone in the pair (OCHRE hpxml.py:96-97).
        // (Conditioned, Adjacent) → (Conditioned, Conditioned).
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
        assert_eq!(wall.interior_zone, Some(ZoneType::Conditioned));
        assert_eq!(wall.exterior_zone, Some(ZoneType::Conditioned));
    }

    #[test]
    fn attic_adjacent_pair_rewritten_to_attic() {
        // (Attic, Adjacent) → (Attic, Attic) after rewrite.
        // Uses Roof1 which has interior = "attic vented". Change exterior from
        // "outside" to "other housing unit" (Adjacent), verify both become Attic.
        let xml = SAMPLE_XML.replace(
            "<InteriorAdjacentTo>attic vented</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>",
            "<InteriorAdjacentTo>attic vented</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>other housing unit</ExteriorAdjacentTo>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let roof = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Roof && b.id == "Roof1")
            .expect("roof expected");
        assert_eq!(roof.interior_zone, Some(ZoneType::Attic));
        assert_eq!(roof.exterior_zone, Some(ZoneType::Attic));
    }

    #[test]
    fn adjacent_interior_rewritten_to_match_exterior() {
        // (Adjacent, Attic) → (Attic, Attic) — handles the case where Adjacent is
        // the interior reference (e.g. party ceiling where Attic is the other side).
        let xml = SAMPLE_XML.replace(
            "<InteriorAdjacentTo>attic vented</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>",
            "<InteriorAdjacentTo>other housing unit</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>attic vented</ExteriorAdjacentTo>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let roof = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Roof && b.id == "Roof1")
            .expect("roof expected");
        assert_eq!(roof.interior_zone, Some(ZoneType::Attic));
        assert_eq!(roof.exterior_zone, Some(ZoneType::Attic));
    }

    #[test]
    fn adjacent_window_zone_rewritten_to_conditioned() {
        // Window with (Conditioned, Adjacent) → (Conditioned, Conditioned).
        let xml = SAMPLE_XML.replace(
            "<ExteriorAdjacentTo>outside</ExteriorAdjacentTo>\n            <Area units=\"ft2\">15</Area>\n            <Azimuth>180</Azimuth>\n            <UFactor>0.31</UFactor>\n            <SHGC>0.25</SHGC>",
            "<ExteriorAdjacentTo>other housing unit</ExteriorAdjacentTo>\n            <Area units=\"ft2\">15</Area>\n            <Azimuth>180</Azimuth>\n            <UFactor>0.31</UFactor>\n            <SHGC>0.25</SHGC>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let window = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Window && b.id == "Window1")
            .expect("window expected");
        assert_eq!(window.interior_zone, Some(ZoneType::Conditioned));
        assert_eq!(window.exterior_zone, Some(ZoneType::Conditioned));
    }

    #[test]
    fn adjacent_adjacent_boundary_rewritten_to_conditioned() {
        // (Adjacent, Adjacent) → (Conditioned, Conditioned) after rewrite.
        // Multi-family party wall between two dwelling units where both sides
        // are "other housing unit" (Adjacent). Since neither side provides a
        // non-Adjacent zone reference, both are rewritten to Conditioned —
        // the most common zone type for adjacent-unit boundaries — producing
        // a same-zone pair that classifies as InternalMass.
        let xml = SAMPLE_XML.replace(
            "<InteriorAdjacentTo>conditioned space</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>",
            "<InteriorAdjacentTo>other housing unit</InteriorAdjacentTo>\n            <ExteriorAdjacentTo>other housing unit</ExteriorAdjacentTo>",
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let wall = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Wall && b.id == "Wall1")
            .expect("wall expected");
        assert_eq!(wall.interior_zone, Some(ZoneType::Conditioned));
        assert_eq!(wall.exterior_zone, Some(ZoneType::Conditioned));
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

    // ── Skylight tests ──────────────────────────────────────────────────

    #[test]
    fn skylight_parsed_with_correct_type_and_properties() {
        let xml = xml_with_skylight(
            r#"<Skylight>
                <SystemIdentifier id="SK1"/>
                <Area units="m2">5.0</Area>
                <Azimuth>0</Azimuth>
                <UFactor>0.35</UFactor>
                <SHGC>0.30</SHGC>
            </Skylight>"#,
        );
        let building = parse_building(&xml).expect("parse should succeed");
        assert_eq!(building.skylights.len(), 1);
        let s = &building.skylights[0];
        assert!((s.area_m2 - 5.0).abs() < f64::EPSILON);
        // UFactor 0.35 BTU/(hr·ft²·°F) → 0.35 × 5.678 ≈ 1.9874 W/(m²·K)
        assert!((s.u_factor_w_m2_k.unwrap() - 1.9874).abs() < 1e-4);
        assert!((s.shgc.unwrap() - 0.30).abs() < f64::EPSILON);
        // Verify it created a boundary with BoundaryType::Skylight
        let boundary = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Skylight)
            .expect("skylight boundary expected");
        assert_eq!(boundary.id, "SK1");
        assert!((boundary.area_m2 - 5.0).abs() < f64::EPSILON);
        // Skylight tilt defaults to 0° (horizontal roof-mounted)
        assert_eq!(boundary.tilt_deg, Some(0.0));
    }

    #[test]
    fn skylight_missing_area_errors() {
        let xml = xml_with_skylight(
            r#"<Skylight>
                <SystemIdentifier id="SK1"/>
                <UFactor>0.35</UFactor>
                <SHGC>0.30</SHGC>
            </Skylight>"#,
        );
        let err = parse_building(&xml).expect_err("skylight missing Area should error");
        assert!(matches!(err, HpxmlError::Parse(_)));
        let msg = err.to_string();
        assert!(msg.contains("skylight 'SK1' is missing required Area element"));
    }

    #[test]
    fn skylight_zero_area_errors() {
        let xml = xml_with_skylight(
            r#"<Skylight>
                <SystemIdentifier id="SK1"/>
                <Area>0</Area>
                <UFactor>0.35</UFactor>
                <SHGC>0.30</SHGC>
            </Skylight>"#,
        );
        let err = parse_building(&xml).expect_err("skylight with zero area should error");
        assert!(matches!(err, HpxmlError::Parse(_)));
        let msg = err.to_string();
        assert!(msg.contains("skylight 'SK1' has non-positive area"));
    }

    #[test]
    fn skylight_with_summer_shading_coefficient() {
        let xml = xml_with_skylight(
            r#"<Skylight>
                <SystemIdentifier id="SK1"/>
                <Area units="m2">5.0</Area>
                <UFactor>0.35</UFactor>
                <SHGC>0.30</SHGC>
                <InteriorShading>
                    <SystemIdentifier id="SK1Shade"/>
                    <SummerShadingCoefficient>0.60</SummerShadingCoefficient>
                    <WinterShadingCoefficient>0.80</WinterShadingCoefficient>
                </InteriorShading>
            </Skylight>"#,
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let s = &building.skylights[0];
        assert!((s.interior_shading_fraction - 0.60).abs() < f64::EPSILON);
        assert!((s.winter_shading_fraction - 0.80).abs() < f64::EPSILON);
    }

    #[test]
    fn skylight_exterior_zone_defaults_to_outdoor() {
        let xml = xml_with_skylight(
            r#"<Skylight>
                <SystemIdentifier id="SK1"/>
                <InteriorAdjacentTo>conditioned space</InteriorAdjacentTo>
                <Area units="m2">5.0</Area>
                <UFactor>0.55</UFactor>
                <SHGC>0.35</SHGC>
            </Skylight>"#,
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let boundary = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Skylight)
            .expect("skylight boundary expected");
        assert_eq!(boundary.interior_zone, Some(ZoneType::Conditioned));
        assert_eq!(boundary.exterior_zone, Some(ZoneType::Outdoor));
    }

    #[test]
    fn skylight_interior_adjacent_to_attic_parsed() {
        let xml = xml_with_skylight(
            r#"<Skylight>
                <SystemIdentifier id="SK1"/>
                <InteriorAdjacentTo>attic vented</InteriorAdjacentTo>
                <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
                <Area units="m2">3.0</Area>
                <UFactor>0.55</UFactor>
                <SHGC>0.35</SHGC>
            </Skylight>"#,
        );
        let building = parse_building(&xml).expect("parse should succeed");
        let boundary = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Skylight)
            .expect("skylight boundary expected");
        assert_eq!(boundary.interior_zone, Some(ZoneType::Attic));
        assert_eq!(boundary.exterior_zone, Some(ZoneType::Outdoor));
    }

    #[test]
    fn multiple_skylights_parsed() {
        let xml = xml_with_skylight(
            r#"<Skylight>
                <SystemIdentifier id="SK1"/>
                <Area units="m2">2.0</Area>
                <UFactor>0.35</UFactor>
                <SHGC>0.30</SHGC>
            </Skylight>
            <Skylight>
                <SystemIdentifier id="SK2"/>
                <Area units="m2">3.0</Area>
                <UFactor>0.55</UFactor>
                <SHGC>0.35</SHGC>
            </Skylight>"#,
        );
        let building = parse_building(&xml).expect("parse should succeed");
        assert_eq!(building.skylights.len(), 2);
        assert_eq!(
            building
                .boundaries
                .iter()
                .filter(|b| b.boundary_type == BoundaryType::Skylight)
                .count(),
            2
        );
    }

    #[test]
    fn skylight_attached_to_roof_subtracts_area() {
        // Skylight with AttachedToWall pointing to a Roof boundary.
        let xml = r#"
<HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
          <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <Walls />
        <Roofs>
          <Roof>
            <SystemIdentifier id="Roof1"/>
            <InteriorAdjacentTo>conditioned space</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <Area units="m2">100</Area>
            <Pitch>6</Pitch>
          </Roof>
        </Roofs>
        <Skylights>
          <Skylight>
            <SystemIdentifier id="SK1"/>
            <Area units="m2">5.0</Area>
            <UFactor>0.55</UFactor>
            <SHGC>0.35</SHGC>
            <AttachedToWall idref="Roof1"/>
          </Skylight>
        </Skylights>
      </Enclosure>
    </BuildingDetails>
  </Building>
</HPXML>
"#;
        let building = parse_building(xml).expect("parse should succeed");
        let roof = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Roof)
            .expect("roof expected");
        assert!(
            (roof.area_m2 - 95.0).abs() < f64::EPSILON,
            "roof area should be reduced by skylight area: expected 95, got {}",
            roof.area_m2
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
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
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

    #[test]
    fn attic_garage_walls_are_kept_as_heat_transfer_surfaces() {
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
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
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
          <Wall>
            <SystemIdentifier id="GarageAtticWall"/>
            <InteriorAdjacentTo>garage</InteriorAdjacentTo>
            <ExteriorAdjacentTo>attic vented</ExteriorAdjacentTo>
            <Area units="ft2">40</Area>
            <Azimuth>0</Azimuth>
          </Wall>
        </Walls>
        <Roofs>
          <Roof>
            <SystemIdentifier id="R1"/>
            <InteriorAdjacentTo>attic vented</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
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

        let garage_attic_walls: Vec<_> = building
            .boundaries
            .iter()
            .filter(|b| {
                b.boundary_type == BoundaryType::Wall
                    && ((b.interior_zone.as_ref() == Some(&ZoneType::Garage)
                        && b.exterior_zone.as_ref() == Some(&ZoneType::Attic))
                        || (b.interior_zone.as_ref() == Some(&ZoneType::Attic)
                            && b.exterior_zone.as_ref() == Some(&ZoneType::Garage)))
            })
            .collect();
        assert_eq!(garage_attic_walls.len(), 1);
        assert!((garage_attic_walls[0].area_m2 - 40.0 * 0.092_903_04).abs() < 1e-9);
    }

    #[test]
    fn convert_conductivity_unrecognized_unit_returns_err() {
        let val = super::convert_conductivity_to_w_m_k(1.5, Some("bogus"));
        assert!(matches!(val, Err(HpxmlError::UnrecognisedUnit { .. })));
    }

    #[test]
    fn convert_conductivity_none_assumes_ip() {
        let val = super::convert_conductivity_to_w_m_k(1.0, None).unwrap();
        // Should convert from BTU*in/(hr*ft2*F), not return raw
        assert!(val != 1.0);
    }

    #[test]
    fn convert_density_unrecognized_unit_returns_err() {
        let val = super::convert_density_to_kg_m3(2.5, Some("bogus"));
        assert!(matches!(val, Err(HpxmlError::UnrecognisedUnit { .. })));
    }

    #[test]
    fn convert_density_none_assumes_ip() {
        let val = super::convert_density_to_kg_m3(1.0, None).unwrap();
        assert!(val != 1.0);
    }

    #[test]
    fn convert_specific_heat_unrecognized_unit_returns_err() {
        let val = super::convert_specific_heat_to_j_kg_k(3.0, Some("bogus"));
        assert!(matches!(val, Err(HpxmlError::UnrecognisedUnit { .. })));
    }

    #[test]
    fn convert_specific_heat_none_assumes_ip() {
        let val = super::convert_specific_heat_to_j_kg_k(1.0, None).unwrap();
        assert!(val != 1.0);
    }

    #[test]
    fn convert_temperature_unrecognized_unit_returns_err() {
        let val = super::convert_temperature_to_c(100.0, Some("kelvin"));
        assert!(matches!(val, Err(HpxmlError::UnrecognisedUnit { .. })));
    }

    #[test]
    fn convert_temperature_known_units_work() {
        let c = super::convert_temperature_to_c(212.0, Some("F")).unwrap();
        assert!((c - 100.0).abs() < 0.1);
        let c2 = super::convert_temperature_to_c(25.0, Some("C")).unwrap();
        assert!((c2 - 25.0).abs() < 1e-12);
    }

    #[test]
    fn convert_temperature_none_assumes_ip() {
        let c = super::convert_temperature_to_c(32.0, None).unwrap();
        assert!(
            (c - 0.0).abs() < 0.1,
            "None units should assume F: 32F == 0C, got {c}"
        );
    }

    #[test]
    fn convert_area_unrecognized_unit_returns_err() {
        let val = super::convert_area_to_m2(5.0, Some("bogus"));
        assert!(matches!(val, Err(HpxmlError::UnrecognisedUnit { .. })));
    }

    #[test]
    fn convert_u_value_unrecognized_unit_returns_err() {
        let val = super::convert_u_to_w_m2_k(2.0, Some("bogus"));
        assert!(matches!(val, Err(HpxmlError::UnrecognisedUnit { .. })));
    }

    #[test]
    fn convert_r_value_unrecognized_unit_returns_err() {
        let val = super::convert_r_to_m2_k_w(10.0, Some("bogus"));
        assert!(matches!(val, Err(HpxmlError::UnrecognisedUnit { .. })));
    }

    #[test]
    fn convert_volume_gal_to_m3() {
        let val = super::convert_volume_to_m3(264.172, Some("gal")).unwrap();
        assert!((val - 1.0).abs() < 0.01);
    }

    #[test]
    fn convert_area_m2_identity() {
        let val = super::convert_area_to_m2(10.0, Some("m2")).unwrap();
        assert!((val - 10.0).abs() < 1e-12);
    }

    #[test]
    fn convert_length_cm_to_m() {
        let val = super::convert_length_to_m(100.0, Some("cm")).unwrap();
        assert!((val - 1.0).abs() < 1e-12);
    }

    #[test]
    fn convert_specific_heat_kj_to_j() {
        let val = super::convert_specific_heat_to_j_kg_k(1.0, Some("kj/(kg*k)")).unwrap();
        assert!((val - 1000.0).abs() < 1e-9);
    }

    #[test]
    fn convert_volume_unrecognized_unit_returns_err() {
        let val = super::convert_volume_to_m3(1.0, Some("barrels"));
        assert!(matches!(val, Err(HpxmlError::UnrecognisedUnit { .. })));
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
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
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
        assert!((0.21..=0.25).contains(&ff), "got {ff:.4}");
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
        assert!((0.13..=0.17).contains(&ff), "got {ff:.4}");
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
          <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
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
        let building = parse_building(xml).expect("should parse");
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
          <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
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
        let building = parse_building(xml).expect("should parse");
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
          <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
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

    #[test]
    fn parse_malformed_xml_includes_position_info() {
        // Illegal bare `<` in text content.
        let xml = "<?xml version=\"1.0\"?>\n<root>\n  <child>value < broken</child>\n</root>";
        let err = parse_xml_document(xml).expect_err("malformed XML should fail to parse");
        let msg = err.to_string();
        assert!(
            msg.contains("line 4"),
            "error should include position context, got: {msg}"
        );
    }

    #[test]
    fn parse_truncated_xml_includes_approximate_location() {
        // Missing closing tag for `<child>`, truncated at `</root>`.
        let xml = "<?xml version=\"1.0\"?>\n<root>\n  <child>value\n</root>";
        let err = parse_xml_document(xml).expect_err("truncated XML should fail to parse");
        let msg = err.to_string();
        assert!(
            msg.contains("line 4"),
            "truncation error should include approximate location, got: {msg}"
        );
    }

    #[test]
    fn parse_error_includes_element_context() {
        // Bare `<` inside a nested element — the error should include the nearest
        // element name from the parse stack.
        let xml = "<?xml version=\"1.0\"?>\n<HPXML>\n  <Building>\n    <BuildingDetails>\n      <BuildingSummary>\n        <Site><Elevation>100 <broken</Elevation></Site>\n      </BuildingSummary>\n    </BuildingDetails>\n  </Building>\n</HPXML>";
        let err = parse_xml_document(xml).expect_err("malformed XML with bare < should fail");
        let msg = err.to_string();
        assert!(
            msg.contains("line 6"),
            "error should include position context, got: {msg}"
        );
        assert!(
            msg.contains("near element <"),
            "error should include nearest element name, got: {msg}"
        );
    }

    #[test]
    fn duplicate_duct_leakage_measurement_last_value_wins() {
        let xml = r#"
<HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="ft2">1000</ConditionedFloorArea>
          <ConditionedBuildingVolume units="ft3">8000</ConditionedBuildingVolume>
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <Walls />
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
                  <DuctType>supply</DuctType>
                  <DuctLeakage>
                    <Value>8</Value>
                    <Units>Percent</Units>
                  </DuctLeakage>
                </DuctLeakageMeasurement>
                <Ducts>
                  <SystemIdentifier id="SupplyDuct"/>
                  <DuctType>supply</DuctType>
                  <DuctInsulationRValue>8</DuctInsulationRValue>
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
        let building = parse_building(xml).expect("parse should succeed");
        let supply = building
            .zones
            .iter()
            .flat_map(|z| &z.duct_systems)
            .find(|d| d.duct_type == DuctType::Supply)
            .expect("supply duct expected");

        assert_eq!(
            supply.leakage_fraction,
            Some(0.08),
            "duplicate supply DuctLeakageMeasurement: second value (8%% -> 0.08) must win"
        );
    }

    /// A garage with a declared floor area, no slab and no foundation walls
    /// takes OS-HPXML's 8 ft garage height (geometry.rb
    /// `calculate_zone_height`); there is no roof augmentation (OS-HPXML
    /// sizes the garage by its slab and walls only).
    #[test]
    fn garage_without_slab_or_walls_takes_the_assumed_height() {
        use super::parse_building;
        use hares_physics::units as conv;
        let xml = r#"
<HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="ft2">1000</ConditionedFloorArea>
          <ConditionedBuildingVolume units="ft3">8000</ConditionedBuildingVolume>
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <Attics>
          <Attic>
            <SystemIdentifier id="Attic1"/>
            <AtticType><Attic><Vented>false</Vented></Attic></AtticType>
            <AttachedToRoof idref="Roof1"/>
          </Attic>
        </Attics>
        <Garages>
          <Garage>
            <SystemIdentifier id="Garage1"/>
            <FloorArea units="ft2">600</FloorArea>
          </Garage>
        </Garages>
        <Roofs>
          <Roof>
            <SystemIdentifier id="Roof1"/>
            <InteriorAdjacentTo>attic - unvented</InteriorAdjacentTo>
            <Area>1500</Area>
            <Pitch>6.0</Pitch>
          </Roof>
        </Roofs>
        <Walls>
          <Wall>
            <SystemIdentifier id="GarageExtWallA"/>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <InteriorAdjacentTo>garage</InteriorAdjacentTo>
            <Area>240</Area>
            <Azimuth>0</Azimuth>
          </Wall>
          <Wall>
            <SystemIdentifier id="GarageExtWallB"/>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <InteriorAdjacentTo>garage</InteriorAdjacentTo>
            <Area>200</Area>
            <Azimuth>90</Azimuth>
          </Wall>
          <Wall>
            <SystemIdentifier id="AttachedWall"/>
            <ExteriorAdjacentTo>garage</ExteriorAdjacentTo>
            <InteriorAdjacentTo>living space</InteriorAdjacentTo>
            <Area>180</Area>
            <Azimuth>180</Azimuth>
          </Wall>
        </Walls>
      </Enclosure>
    </BuildingDetails>
  </Building>
</HPXML>"#;
        let building = parse_building(xml).expect("garage HPXML should parse");
        let garage_zone = building
            .zones
            .iter()
            .find(|z| z.zone_type == ZoneType::Garage)
            .expect("must have garage zone");

        let volume = garage_zone
            .volume_m3
            .expect("garage zone must have a computed volume");
        let expected = conv::area_ft2_to_m2(600.0) * conv::length_ft_to_m(8.0);
        assert!(
            (volume - expected).abs() < 1e-9,
            "garage volume {volume}, expected {expected}"
        );
        assert_eq!(garage_zone.height_m, Some(conv::length_ft_to_m(8.0)));
        assert!(
            !building
                .parse_warnings
                .iter()
                .any(|w| w.message.contains("'garage'")),
            "a garage's height has no HPXML input, so 8 ft is the model, not a \
             substitution to warn about; got {:?}",
            building.parse_warnings
        );
    }

    // ── Roof Pitch tests ─────────────────────────────────────────────────

    #[test]
    fn roof_missing_pitch_defaults_to_4_12_not_zero() {
        // A Roof element without <Pitch> defaults to 4:12 pitch (~18.4°)
        // rather than 0° (flat). This prevents misclassification of sloped
        // roofs as flat roofs when Pitch data is absent from the HPXML input.
        // HPXML v4.0: Roof/Pitch is optional (0..1 per hpxml-elements.md).
        let xml = r#"
<HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site>
          <SiteType>suburban</SiteType>
        </Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="ft2">100</ConditionedFloorArea>
          <ConditionedBuildingVolume units="ft3">800</ConditionedBuildingVolume>
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <Roofs>
          <Roof>
            <SystemIdentifier id="R1"/>
            <InteriorAdjacentTo>attic vented</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <Area units="ft2">500</Area>
          </Roof>
        </Roofs>
      </Enclosure>
    </BuildingDetails>
  </Building>
</HPXML>"#;
        let building = parse_building(xml).expect("HPXML with roof missing Pitch should parse");
        let roof = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Roof)
            .expect("must have a Roof boundary");
        let expected = (4.0_f64 / 12.0).atan().to_degrees();
        let tilt = roof.tilt_deg.expect("roof must have tilt_deg");
        assert!(
            (tilt - expected).abs() < 0.01,
            "Roof without <Pitch> must default to 4:12 tilt (~{expected}°), got {tilt}"
        );
    }

    #[test]
    fn roof_with_explicit_pitch_produces_correct_tilt() {
        // A Roof element with <Pitch>6</Pitch> produces tilt = atan(6/12) ≈ 26.565°.
        let xml = r#"
<HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site>
          <SiteType>suburban</SiteType>
        </Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="ft2">100</ConditionedFloorArea>
          <ConditionedBuildingVolume units="ft3">800</ConditionedBuildingVolume>
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <Roofs>
          <Roof>
            <SystemIdentifier id="R1"/>
            <InteriorAdjacentTo>attic vented</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <Area units="ft2">500</Area>
            <Pitch>6</Pitch>
          </Roof>
        </Roofs>
      </Enclosure>
    </BuildingDetails>
  </Building>
</HPXML>"#;
        let building = parse_building(xml).expect("HPXML with pitched roof should parse");
        let roof = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Roof)
            .expect("must have a Roof boundary");
        let expected_tilt = (6.0_f64 / 12.0).atan().to_degrees();
        assert!(
            (roof.tilt_deg.unwrap() - expected_tilt).abs() < 1e-6,
            "Roof with Pitch=6 must produce tilt={expected_tilt}°, got {:?}",
            roof.tilt_deg
        );
    }

    #[test]
    fn pitched_roof_produces_attic_volume() {
        // Regression: pitched-roof HPXML with explicit <Pitch> must produce
        // non-zero attic volume (not zero from absent-pitch default).
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
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <Roofs>
          <Roof>
            <SystemIdentifier id="R1"/>
            <InteriorAdjacentTo>attic vented</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <Area units="ft2">600</Area>
            <Pitch>6</Pitch>
          </Roof>
        </Roofs>
        <Walls>
          <Wall>
            <SystemIdentifier id="W1"/>
            <InteriorAdjacentTo>attic vented</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <Area units="ft2">100</Area>
            <Azimuth>0</Azimuth>
          </Wall>
          <Wall>
            <SystemIdentifier id="W2"/>
            <InteriorAdjacentTo>attic vented</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <Area units="ft2">100</Area>
            <Azimuth>180</Azimuth>
          </Wall>
        </Walls>
        <Floors>
          <Floor>
            <SystemIdentifier id="AtticFloor"/>
            <InteriorAdjacentTo>conditioned space</InteriorAdjacentTo>
            <ExteriorAdjacentTo>attic vented</ExteriorAdjacentTo>
            <Area units="ft2">500</Area>
          </Floor>
        </Floors>
        <Attics>
          <Attic>
            <AtticType><Attic><Vented>true</Vented></Attic></AtticType>
          </Attic>
        </Attics>
      </Enclosure>
    </BuildingDetails>
  </Building>
</HPXML>"#;
        let building = parse_building(xml).expect("HPXML with pitched roof should parse");
        let attic_zone = building
            .zones
            .iter()
            .find(|z| z.zone_type == ZoneType::Attic)
            .expect("must have an Attic zone");
        assert!(
            attic_zone.volume_m3.is_some(),
            "Attic zone must have volume when pitched roof is specified"
        );
        assert!(
            attic_zone.volume_m3.unwrap() > 0.0,
            "Attic volume must be positive for a pitched roof"
        );
    }

    #[test]
    fn roof_missing_pitch_defaults_to_4_12_tilt() {
        // When <Pitch> is absent, the boundary should default to 4:12 pitch
        // (~18.4°), not 0° (flat). SAMPLE_XML has a <Roof> without <Pitch>.
        let building = parse_building(SAMPLE_XML).expect("parse should succeed");
        let roof = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Roof)
            .expect("roof expected");
        let tilt = roof.tilt_deg.expect("roof must have tilt_deg");
        let expected = (4.0_f64 / 12.0).atan().to_degrees();
        assert!(
            (tilt - expected).abs() < 0.01,
            "roof without <Pitch> should default to 4:12 tilt (~{expected}°), got {tilt}"
        );
    }

    #[test]
    fn roof_with_explicit_pitch_uses_provided_value() {
        let xml = r#"
<HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="ft2">1000</ConditionedFloorArea>
          <ConditionedBuildingVolume units="ft3">8000</ConditionedBuildingVolume>
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <Roofs>
          <Roof>
            <SystemIdentifier id="R1"/>
            <InteriorAdjacentTo>attic vented</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <Area units="ft2">600</Area>
            <Pitch>6</Pitch>
          </Roof>
        </Roofs>
      </Enclosure>
    </BuildingDetails>
  </Building>
</HPXML>"#;
        let building = parse_building(xml).expect("parse should succeed");
        let roof = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Roof)
            .expect("roof expected");
        let tilt = roof.tilt_deg.expect("roof must have tilt_deg");
        let expected = (6.0_f64 / 12.0).atan().to_degrees(); // ~26.6°
        assert!(
            (tilt - expected).abs() < 0.01,
            "roof with Pitch=6 should have 6:12 tilt (~{expected}°), got {tilt}"
        );
    }

    #[test]
    fn roof_missing_pitch_gets_geometric_inference_with_gable_walls() {
        // When <Pitch> is absent and gable end walls + attic floor exist,
        // tilt should be inferred from geometry rather than the 4:12 default.
        // Attic floor = 500 ft² ≈ 46.45 m², gable walls 100 ft² each ≈ 9.29 m².
        // tan(tilt) = 4 * gable_area / floor_area = 4 * 9.29 / 46.45 ≈ 0.80
        // tilt ≈ atan(0.80) ≈ 38.7°
        let xml = r#"
<HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="ft2">1000</ConditionedFloorArea>
          <ConditionedBuildingVolume units="ft3">8000</ConditionedBuildingVolume>
          <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <Roofs>
          <Roof>
            <SystemIdentifier id="R1"/>
            <InteriorAdjacentTo>attic vented</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <Area units="ft2">600</Area>
          </Roof>
        </Roofs>
        <Walls>
          <Wall>
            <SystemIdentifier id="W1"/>
            <InteriorAdjacentTo>attic vented</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <Area units="ft2">100</Area>
            <Azimuth>0</Azimuth>
          </Wall>
          <Wall>
            <SystemIdentifier id="W2"/>
            <InteriorAdjacentTo>attic vented</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <Area units="ft2">100</Area>
            <Azimuth>180</Azimuth>
          </Wall>
        </Walls>
        <Floors>
          <Floor>
            <SystemIdentifier id="AtticFloor"/>
            <InteriorAdjacentTo>conditioned space</InteriorAdjacentTo>
            <ExteriorAdjacentTo>attic vented</ExteriorAdjacentTo>
            <Area units="ft2">500</Area>
          </Floor>
        </Floors>
        <Attics>
          <Attic>
            <AtticType><Attic><Vented>true</Vented></Attic></AtticType>
          </Attic>
        </Attics>
      </Enclosure>
    </BuildingDetails>
  </Building>
</HPXML>"#;
        let building = parse_building(xml).expect("parse should succeed");
        let roof = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Roof)
            .expect("roof expected");
        let tilt = roof.tilt_deg.expect("roof must have tilt_deg");
        // The gable inference should produce a tilt > the 4:12 default (~18.4°)
        // since gable_area / floor_area ratio suggests a steeper roof.
        let default_4_12 = (4.0_f64 / 12.0).atan().to_degrees();
        assert!(
            tilt > default_4_12 + 1.0,
            "geometric inference should produce tilt > {default_4_12}° (4:12 default), got {tilt}"
        );
        // tan(tilt) = 4 * 9.2903 / 46.4515 ≈ 0.80, tilt ≈ 38.7°
        let expected_approx = 38.7;
        assert!(
            (tilt - expected_approx).abs() < 2.0,
            "geometric inference should produce tilt ~{expected_approx}°, got {tilt}"
        );
    }

    #[test]
    fn missing_floors_above_grade_is_a_parse_error() {
        let xml = SAMPLE_XML.replace(
            "<NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>",
            "",
        );
        let err = parse_building(&xml).expect_err("expected missing-field failure");
        let msg = err.to_string();
        assert!(
            msg.contains("NumberofConditionedFloorsAboveGrade"),
            "error must name the element, got: {msg}"
        );
        assert!(
            msg.contains("no silent default permitted"),
            "error must state the strictness rule, got: {msg}"
        );
    }

    #[test]
    fn unknown_shielding_or_site_type_is_a_parse_error() {
        let shielding = SAMPLE_XML.replace(
            "<ShieldingofHome>normal</ShieldingofHome>",
            "<ShieldingofHome>windy</ShieldingofHome>",
        );
        let err = parse_building(&shielding).expect_err("expected parse failure");
        let msg = err.to_string();
        assert!(
            msg.contains("ShieldingofHome") && msg.contains("windy"),
            "error must name the element and the value, got: {msg}"
        );
        assert!(
            msg.contains("normal") && msg.contains("exposed") && msg.contains("well-shielded"),
            "error must name the allowed values, got: {msg}"
        );

        let site_type = SAMPLE_XML.replace(
            "<SiteType>suburban</SiteType>",
            "<SiteType>coastal</SiteType>",
        );
        let err = parse_building(&site_type).expect_err("expected parse failure");
        let msg = err.to_string();
        assert!(
            msg.contains("SiteType") && msg.contains("coastal"),
            "error must name the element and the value, got: {msg}"
        );
        assert!(
            msg.contains("rural") && msg.contains("suburban") && msg.contains("urban"),
            "error must name the allowed values, got: {msg}"
        );
    }

    #[test]
    fn xsd_boolean_accepts_one_and_zero() {
        let with_flue = |v: &str| {
            SAMPLE_XML.replacen(
                "        </AirInfiltrationMeasurement>",
                &format!(
                    "          <extension><HasFlueOrChimneyInConditionedSpace>{v}</HasFlueOrChimneyInConditionedSpace></extension>\n        </AirInfiltrationMeasurement>"
                ),
                1,
            )
        };

        let building = parse_building(&with_flue("1")).expect("parse should succeed");
        assert_eq!(
            building.has_flue_or_chimney,
            Some(true),
            "xsd:boolean 1 is true"
        );
        let building = parse_building(&with_flue("0")).expect("parse should succeed");
        assert_eq!(
            building.has_flue_or_chimney,
            Some(false),
            "xsd:boolean 0 is false"
        );

        let err = parse_building(&with_flue("yes")).expect_err("yes is outside the lexical space");
        let msg = err.to_string();
        assert!(
            msg.contains("HasFlueOrChimneyInConditionedSpace") && msg.contains("yes"),
            "error must name the element and the value, got: {msg}"
        );
    }

    #[test]
    fn legacy_flue_element_names_its_replacement() {
        let xml = SAMPLE_XML.replacen(
            "        </AirInfiltrationMeasurement>",
            "          <extension><HasFlueOrChimney>false</HasFlueOrChimney></extension>\n        </AirInfiltrationMeasurement>",
            1,
        );
        let err = parse_building(&xml).expect_err("the deprecated element must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("HasFlueOrChimneyInConditionedSpace"),
            "error must name the current element, got: {msg}"
        );
    }

    fn parse_sample_with(from: &str, to: &str) -> Result<super::Building, HpxmlError> {
        assert!(SAMPLE_XML.contains(from), "SAMPLE_XML lacks {from}");
        parse_building(&SAMPLE_XML.replace(from, to))
    }

    fn zone_vented(building: &super::Building, zone_type: ZoneType) -> bool {
        building
            .zones
            .iter()
            .find(|z| z.zone_type == zone_type)
            .expect("zone")
            .vented
    }

    const EMPTY_FOUNDATION: &str = "<Foundation>\n            <FloorArea units=\"ft2\">800</FloorArea>\n          </Foundation>";
    const EMPTY_ATTIC: &str =
        "<Attic>\n            <FloorArea units=\"ft2\">500</FloorArea>\n          </Attic>";
    const BARE_SLAB: &str = "<Area units=\"ft2\">80</Area>\n          </Slab>";
    const BARE_ROOF: &str = "<Area units=\"ft2\">120</Area>\n          </Roof>";

    #[test]
    fn flue_or_chimney_is_an_xs_boolean() {
        assert_reads_xs_boolean("extension/HasFlueOrChimneyInConditionedSpace", |v| {
            parse_sample_with(
                "<Enclosure>",
                &format!(
                    "<extension><HasFlueOrChimneyInConditionedSpace>{v}\
                     </HasFlueOrChimneyInConditionedSpace></extension><Enclosure>"
                ),
            )
            .map(|b| b.has_flue_or_chimney)
        });
    }

    #[test]
    fn basement_conditioned_is_an_xs_boolean() {
        assert_reads_xs_boolean("Basement/Conditioned", |v| {
            parse_sample_with(
                EMPTY_FOUNDATION,
                &format!(
                    "<Foundation><FoundationType><Basement><Conditioned>{v}</Conditioned>\
                     </Basement></FoundationType><FloorArea units=\"ft2\">800</FloorArea></Foundation>"
                ),
            )
            .map(|b| b.foundation_name)
        });
    }

    #[test]
    fn radiant_barrier_is_an_xs_boolean() {
        assert_reads_xs_boolean("RadiantBarrier", |v| {
            parse_sample_with(
                BARE_ROOF,
                &format!(
                    "<Area units=\"ft2\">120</Area><RadiantBarrier>{v}</RadiantBarrier></Roof>"
                ),
            )
            .map(|b| {
                b.boundaries
                    .iter()
                    .find(|r| r.boundary_type == BoundaryType::Roof)
                    .expect("roof")
                    .has_radiant_barrier
            })
        });
    }

    #[test]
    fn slab_spans_entire_slab_is_an_xs_boolean() {
        assert_reads_xs_boolean("UnderSlabInsulation/Layer/InsulationSpansEntireSlab", |v| {
            parse_sample_with(
                BARE_SLAB,
                &format!(
                    "<Area units=\"ft2\">80</Area><UnderSlabInsulation><Layer>\
                     <NominalRValue>10</NominalRValue>\
                     <InsulationSpansEntireSlab>{v}</InsulationSpansEntireSlab>\
                     </Layer></UnderSlabInsulation></Slab>"
                ),
            )
            .map(|b| {
                b.boundaries
                    .iter()
                    .find(|s| s.boundary_type == BoundaryType::Slab)
                    .expect("slab")
                    .insulation_details
                    .clone()
            })
        });
    }

    #[test]
    fn attic_vented_is_an_xs_boolean() {
        assert_reads_xs_boolean("Attic/AtticType/Attic/Vented", |v| {
            parse_sample_with(
                EMPTY_ATTIC,
                &format!(
                    "<Attic><AtticType><Attic><Vented>{v}</Vented></Attic></AtticType>\
                     <FloorArea units=\"ft2\">500</FloorArea></Attic>"
                ),
            )
            .map(|b| zone_vented(&b, ZoneType::Attic))
        });
    }

    #[test]
    fn foundation_vented_is_an_xs_boolean() {
        assert_reads_xs_boolean("Foundation/FoundationType/Vented", |v| {
            parse_sample_with(
                EMPTY_FOUNDATION,
                &format!(
                    "<Foundation><FoundationType><Crawlspace><Vented>{v}</Vented></Crawlspace>\
                     </FoundationType><FloorArea units=\"ft2\">800</FloorArea></Foundation>"
                ),
            )
            .map(|b| zone_vented(&b, ZoneType::Foundation))
        });
    }
}
