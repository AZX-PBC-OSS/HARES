//! HPXML input validation and error reporting.

use std::fmt;

use chrono::{DateTime, Duration, FixedOffset};

use super::building::{Building, ZoneType, parse_xml_document};
use crate::schedule::{ScheduleTimeSeries, normalize_column_name};
use crate::weather::{WeatherMeta, WeatherTimeSeries};

const MAX_LOCATION_DISTANCE_KM: f64 = 200.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub field: String,
    pub message: String,
}

impl ValidationError {
    #[must_use]
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationWarning {
    pub field: String,
    pub message: String,
}

impl ValidationWarning {
    #[must_use]
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ValidationWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ValidationReport {
    pub errors: Vec<ValidationError>,
    pub warnings: Vec<ValidationWarning>,
}

impl ValidationReport {
    #[must_use]
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

/// Returns true when `value` looks like a namespace URI (starts with `http://` or
/// `https://`) and does *not* belong to HPXML (`hpxmlonline.com`) or well-known
/// XML infrastructure namespaces (`w3.org`).
#[cfg(any(debug_assertions, feature = "check_invariants", feature = "observe"))]
fn looks_like_namespace_uri(value: &str) -> bool {
    let lower = value.to_lowercase();
    (lower.starts_with("http://") || lower.starts_with("https://"))
        && !lower.contains("hpxmlonline.com")
        && !lower.contains("w3.org")
}

/// Structural completeness check for HPXML documents (string input).
///
/// Accepts HPXML 3.x (with warning) and 4.x (silently). Rejects major
/// versions below 3 as unsupported.
pub fn validate_hpxml_schema(xml: &str) -> Result<Vec<ValidationWarning>, ValidationError> {
    let root = parse_xml_document(xml).map_err(|err| {
        ValidationError::new(
            "schema",
            format!("could not parse XML before schema checks: {err}"),
        )
    })?;
    let warnings = validate_hpxml_schema_node(&root)?;

    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        let is_v3 = warnings.iter().any(|w| w.field == "schemaVersion");
        if is_v3 {
            emit_deprecated_path_counts(xml);
        }
    }

    Ok(warnings)
}

/// Structural completeness check for a pre-parsed HPXML document tree.
pub fn validate_hpxml_schema_node(
    root: &super::building::XmlNode,
) -> Result<Vec<ValidationWarning>, ValidationError> {
    let mut warnings = Vec::new();

    if root.name != "HPXML" {
        return Err(ValidationError::new(
            "schema",
            format!("root element must be HPXML, found `{}`", root.name),
        ));
    }

    let version_attr = root.attrs.get("schemaVersion");
    let major_version = version_attr
        .and_then(|s| s.split('.').next())
        .and_then(|s| s.parse::<u32>().ok());

    match (version_attr, major_version) {
        (_, Some(v)) if v >= 4 => {}
        (Some(s), Some(3)) => {
            warnings.push(ValidationWarning::new(
                "schemaVersion",
                format!("HPXML {s} may have untested element paths; 4.x is recommended"),
            ));
        }
        (None, _) => {
            return Err(ValidationError::new(
                "schemaVersion",
                "schemaVersion attribute is missing",
            ));
        }
        (Some(s), _) => {
            return Err(ValidationError::new(
                "schemaVersion",
                format!("unsupported HPXML schemaVersion `{s}`; requires 3.x or 4.x"),
            ));
        }
    }

    #[cfg(feature = "observe")]
    {
        let raw_version = version_attr.map(|s| s.as_str()).unwrap_or("<missing>");
        let resolved = match major_version {
            Some(3) => "3.x",
            Some(m) if m >= 4 => "4.x",
            _ => "unknown",
        };
        tracing::info!(
            target: "observe",
            column = "hpxml_schema_version",
            schema_version_raw = raw_version,
            schema_version_resolved = resolved,
            "HPXML schema version detected"
        );
    }

    let xmlns = root
        .attrs
        .get("xmlns")
        .map(String::as_str)
        .unwrap_or_default();
    if !xmlns.contains("hpxmlonline.com") {
        return Err(ValidationError::new(
            "xmlns",
            format!("unexpected HPXML namespace `{xmlns}`"),
        ));
    }

    // Detect non-HPXML namespace URIs declared on the root element.
    //
    // The XML parser normalizes attribute keys by stripping the namespace prefix
    // (e.g. `xmlns:bsync` → `bsync`).  To detect foreign namespace declarations
    // reliably, we scan all attribute *values* for URI-looking values that do not
    // belong to HPXML, XML Schema, or XSI — the canonical small set of root-level
    // namespace attributes.
    #[cfg(any(debug_assertions, feature = "check_invariants", feature = "observe"))]
    let non_hpxml_namespaces: Vec<String> = root
        .attrs
        .iter()
        .filter(|(k, v)| {
            // The default `xmlns` key is never stripped; all other namespace
            // declarations have their prefix-stripped key.  We exclude the
            // HPXML namespace URI and well-known XML infrastructure URIs.
            *k != "xmlns" && looks_like_namespace_uri(v)
        })
        .map(|(_, v)| v.clone())
        .collect();

    #[cfg(feature = "observe")]
    {
        if !non_hpxml_namespaces.is_empty() {
            tracing::info!(
                target: "observe",
                column = "hpxml_foreign_namespaces",
                foreign_namespace_count = non_hpxml_namespaces.len(),
                foreign_namespaces = ?non_hpxml_namespaces,
                "non-HPXML namespaces detected on root element; tolerated during schema validation"
            );
        }
    }

    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        if !non_hpxml_namespaces.is_empty() {
            // HPXML §2.4 / BuildingSync audit data: a document that declares
            // non-HPXML namespaces MUST still carry the core HPXML structure.
            let has_building = root.path(&["Building"]).is_some()
                || root.path(&["Building", "BuildingDetails"]).is_some();
            let has_systems = root
                .path(&["Building", "BuildingDetails", "Systems"])
                .is_some()
                || root.path(&["Building", "Systems"]).is_some();
            if !has_building || !has_systems {
                let missing: Vec<&str> = {
                    let mut parts = Vec::new();
                    if !has_building {
                        parts.push("Building/BuildingDetails");
                    }
                    if !has_systems {
                        parts.push("Building/BuildingDetails/Systems");
                    }
                    parts
                };
                return Err(ValidationError::new(
                    "schema",
                    format!(
                        "non-HPXML namespaces {} present but HPXML core structure missing: {}",
                        non_hpxml_namespaces.join(", "),
                        missing.join(", "),
                    ),
                ));
            }
        }
    }

    let schema_location = root
        .attrs
        .get("schemaLocation")
        .or_else(|| root.attrs.get("xsi:schemaLocation"))
        .map(String::as_str)
        .unwrap_or_default();
    if !schema_location.is_empty() && !schema_location.contains("hpxmlonline.com") {
        return Err(ValidationError::new(
            "schemaLocation",
            format!("unexpected schemaLocation `{schema_location}`"),
        ));
    }

    // Required structural elements (present in both 3.x and 4.x)
    let required_paths: &[(&[&str], &str)] = &[
        (
            &["Building", "BuildingDetails", "BuildingSummary"],
            "Building/BuildingDetails/BuildingSummary",
        ),
        (
            &["Building", "BuildingDetails", "BuildingSummary", "Site"],
            "Building/BuildingDetails/BuildingSummary/Site",
        ),
        (
            &[
                "Building",
                "BuildingDetails",
                "BuildingSummary",
                "BuildingConstruction",
                "ConditionedFloorArea",
            ],
            "Building/BuildingDetails/BuildingSummary/BuildingConstruction/ConditionedFloorArea",
        ),
        (
            &["Building", "BuildingDetails", "Enclosure"],
            "Building/BuildingDetails/Enclosure",
        ),
    ];

    for (path, label) in required_paths {
        if root.path(path).is_none() {
            return Err(ValidationError::new(
                "schema",
                format!("missing required path {label}"),
            ));
        }
    }

    // Building/Site is required
    if root.path(&["Building", "Site"]).is_none()
        && root
            .path(&["Building", "BuildingDetails", "BuildingSummary", "Site"])
            .is_none()
    {
        return Err(ValidationError::new(
            "schema",
            "missing required path Building/Site or Building/BuildingDetails/BuildingSummary/Site",
        ));
    }

    Ok(warnings)
}

pub fn validate_building_ranges(building: &Building) -> ValidationReport {
    let mut report = ValidationReport::default();

    for zone in &building.zones {
        if matches!(zone.zone_type, ZoneType::Conditioned)
            && let Some(area_m2) = zone.floor_area_m2
            && !(20.0..=1_000.0).contains(&area_m2)
        {
            report.errors.push(ValidationError::new(
                "ConditionedFloorArea",
                format!("must be in [20, 1000] m^2, got {area_m2:.3}"),
            ));
        }
    }

    if let Some(ach50) = building.infiltration_ach50
        && !(0.5..=30.0).contains(&ach50)
    {
        report.warnings.push(ValidationWarning::new(
            "InfiltrationACH50",
            format!("outside recommended range [0.5, 30]: {ach50:.3}"),
        ));
    }

    if let Some(capacity_w) = building.hvac_capacity_w
        && !(293.0..=58_614.0).contains(&capacity_w)
    {
        report.errors.push(ValidationError::new(
            "HVACCapacity",
            format!("must be in [293, 58614] W (1-200 kBtu/h), got {capacity_w:.1}"),
        ));
    }

    if let Some(seer2) = building.seer2
        && !(10.0..=40.0).contains(&seer2)
    {
        report.errors.push(ValidationError::new(
            "SEER2",
            format!("must be in [10, 40], got {seer2:.3}"),
        ));
    }

    if let Some(hspf2) = building.hspf2
        && !(6.0..=15.0).contains(&hspf2)
    {
        report.errors.push(ValidationError::new(
            "HSPF2",
            format!("must be in [6, 15], got {hspf2:.3}"),
        ));
    }

    if let Some(setpoint_c) = building.water_heater_setpoint_c {
        if !(40.0..=70.0).contains(&setpoint_c) {
            report.errors.push(ValidationError::new(
                "WaterHeaterSetpoint",
                format!("must be in [40, 70] C, got {setpoint_c:.3}"),
            ));
        } else if setpoint_c < 49.0 {
            report.warnings.push(ValidationWarning::new(
                "WaterHeaterSetpoint",
                format!("{setpoint_c:.3} C is below 49 C and may increase Legionella risk"),
            ));
        }
    }

    if let Some(rte) = building.battery_round_trip_efficiency
        && !(0.70..=0.99).contains(&rte)
    {
        report.errors.push(ValidationError::new(
            "BatteryRoundTripEfficiency",
            format!("must be in [0.70, 0.99], got {rte:.3}"),
        ));
    }

    if let Some(tilt) = building.pv_tilt_deg {
        if !(0.0..=90.0).contains(&tilt) {
            report.errors.push(ValidationError::new(
                "PVTilt",
                format!("must be in [0, 90] deg, got {tilt:.3}"),
            ));
        } else if tilt > 60.0 {
            report.warnings.push(ValidationWarning::new(
                "PVTilt",
                format!("high PV tilt {tilt:.3} deg (> 60)"),
            ));
        }
    }

    // Window and Skylight U-factor and SHGC presence — every <Window> and
    // <Skylight> must carry both <UFactor> and <SHGC> per HPXML §6.5 "Windows".
    // A silent solver-layer default produces catastrophic heat transfer bias
    // with no diagnostic.
    for win in building.windows.iter().chain(building.skylights.iter()) {
        let fen_type = if building.skylights.iter().any(|s| s.id == win.id) {
            "skylight"
        } else {
            "window"
        };
        if win.u_factor_w_m2_k.is_none() {
            report.errors.push(ValidationError::new(
                "WindowUFactor",
                format!(
                    "{fen_type} '{}': <UFactor> element is required per HPXML §6.5; \
                     supply the NFRC-rated whole-product U-factor in Btu/(h·ft²·°F) \
                     or SI units",
                    win.id
                ),
            ));
        }
        if win.shgc.is_none() {
            report.errors.push(ValidationError::new(
                "WindowSHGC",
                format!(
                    "{fen_type} '{}': <SHGC> element is required per HPXML §6.5; \
                     supply the NFRC-rated whole-product solar heat gain coefficient",
                    win.id
                ),
            ));
        }
    }

    report
}

pub fn validate_cross_inputs(
    building: &Building,
    weather_meta: &WeatherMeta,
    weather: &WeatherTimeSeries,
    schedule: &ScheduleTimeSeries,
    required_schedule_columns: &[&str],
    simulation_period: Option<(DateTime<FixedOffset>, DateTime<FixedOffset>)>,
    // EPW timestamps, passed separately because `WeatherTimeSeries` stores
    // only hour-of-year indices. `None` when timestamps are unavailable
    // (e.g. non-EPW weather sources).
    epw_timestamps: Option<&[DateTime<FixedOffset>]>,
) -> ValidationReport {
    let mut report = ValidationReport::default();

    if let Some(warn) =
        validate_epw_location_distance(building, weather_meta, MAX_LOCATION_DISTANCE_KM)
    {
        report.warnings.push(warn);
    }

    report
        .errors
        .extend(validate_epw_dewpoint_leq_dry_bulb(weather));

    if let Some(timestamps) = epw_timestamps {
        report
            .errors
            .extend(validate_epw_time_gaps(timestamps, Duration::hours(2)));
    }

    report.errors.extend(validate_schedule_required_columns(
        schedule,
        required_schedule_columns,
    ));
    report.errors.extend(validate_schedule_no_nan(schedule));
    if let Some((start, end)) = simulation_period
        && let Some(err) = validate_schedule_temporal_coverage(schedule, start, end)
    {
        report.errors.push(err);
    }

    report
}

pub fn validate_epw_location_distance(
    building: &Building,
    weather_meta: &WeatherMeta,
    max_distance_km: f64,
) -> Option<ValidationWarning> {
    let (Some(lat), Some(lon)) = (building.site.latitude_deg, building.site.longitude_deg) else {
        return None;
    };

    let distance_km = haversine_km(lat, lon, weather_meta.latitude, weather_meta.longitude);
    if distance_km > max_distance_km {
        return Some(ValidationWarning::new(
            "EPWLocationDistance",
            format!(
                "EPW site is {:.2} km from HPXML site (limit {:.1} km)",
                distance_km, max_distance_km
            ),
        ));
    }

    None
}

pub fn validate_epw_dewpoint_leq_dry_bulb(weather: &WeatherTimeSeries) -> Vec<ValidationError> {
    weather
        .dew_point_c
        .iter()
        .zip(weather.dry_bulb_c.iter())
        .enumerate()
        .filter_map(|(idx, (dew, dry))| {
            if dew > dry {
                Some(ValidationError::new(
                    "EPWDewPoint",
                    format!(
                        "row {} has dew_point_c ({dew}) > dry_bulb_c ({dry})",
                        idx + 1
                    ),
                ))
            } else {
                None
            }
        })
        .collect()
}

pub fn validate_epw_time_gaps(
    timestamps: &[DateTime<FixedOffset>],
    max_gap: Duration,
) -> Vec<ValidationError> {
    if timestamps.len() < 2 {
        return Vec::new();
    }

    let mut errors = Vec::new();
    for idx in 1..timestamps.len() {
        let gap = timestamps[idx] - timestamps[idx - 1];
        if gap > max_gap {
            errors.push(ValidationError::new(
                "EPWTimeGap",
                format!(
                    "gap {} exceeds max {} between index {} ({}) and {} ({})",
                    gap,
                    max_gap,
                    idx - 1,
                    timestamps[idx - 1],
                    idx,
                    timestamps[idx]
                ),
            ));
        }
    }
    errors
}

pub fn validate_schedule_required_columns(
    schedule: &ScheduleTimeSeries,
    required_columns: &[&str],
) -> Vec<ValidationError> {
    let normalized_existing = schedule
        .column_names
        .iter()
        .map(|name| normalize_column_name(name))
        .collect::<Vec<_>>();

    let missing = required_columns
        .iter()
        .map(|name| normalize_column_name(name))
        .filter(|name| !normalized_existing.iter().any(|existing| existing == name))
        .collect::<Vec<_>>();

    if missing.is_empty() {
        Vec::new()
    } else {
        vec![ValidationError::new(
            "ScheduleRequiredColumns",
            format!("missing required columns: {}", missing.join(", ")),
        )]
    }
}

pub fn validate_schedule_no_nan(schedule: &ScheduleTimeSeries) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    for (col_idx, col_name) in schedule.column_names.iter().enumerate() {
        for (row_idx, value) in schedule.columns[col_idx].iter().enumerate() {
            if value.is_nan() {
                errors.push(ValidationError::new(
                    "ScheduleNaN",
                    format!("NaN value in column `{col_name}` at row {}", row_idx + 1),
                ));
            }
        }
    }
    errors
}

pub fn validate_schedule_temporal_coverage(
    schedule: &ScheduleTimeSeries,
    simulation_start: DateTime<FixedOffset>,
    simulation_end: DateTime<FixedOffset>,
) -> Option<ValidationError> {
    if schedule.timestamps.is_empty() {
        return Some(ValidationError::new(
            "ScheduleCoverage",
            "schedule contains no timestamps",
        ));
    }

    let schedule_start = schedule.timestamps[0];
    let schedule_end_exclusive = *schedule.timestamps.last().expect("non-empty timestamps")
        + Duration::seconds(i64::from(schedule.source_step_secs));

    let mut gaps = Vec::new();
    if simulation_start < schedule_start {
        gaps.push(format!(
            "missing start coverage: [{simulation_start}, {schedule_start})"
        ));
    }
    if simulation_end > schedule_end_exclusive {
        gaps.push(format!(
            "missing end coverage: [{schedule_end_exclusive}, {simulation_end})"
        ));
    }

    if gaps.is_empty() {
        None
    } else {
        Some(ValidationError::new("ScheduleCoverage", gaps.join("; ")))
    }
}

fn haversine_km(lat1_deg: f64, lon1_deg: f64, lat2_deg: f64, lon2_deg: f64) -> f64 {
    let lat1 = lat1_deg.to_radians();
    let lon1 = lon1_deg.to_radians();
    let lat2 = lat2_deg.to_radians();
    let lon2 = lon2_deg.to_radians();

    let dlat = lat2 - lat1;
    let dlon = lon2 - lon1;
    let a = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().asin();
    6_371.0 * c
}

/// When `check_invariants` is enabled (or debug_assertions), scan a 3.x HPXML
/// document for deprecated element paths and emit a diagnostic warning quantifying
/// the risk of relying on deprecated or untested paths.
#[cfg(any(debug_assertions, feature = "check_invariants"))]
fn emit_deprecated_path_counts(xml: &str) {
    let energy_factor = xml.match_indices("<EnergyFactor").count();
    let uniform_ef = xml.match_indices("<UniformEnergyFactor").count();

    if energy_factor > 0 || uniform_ef > 0 {
        tracing::warn!(
            deprecated_energy_factor = energy_factor,
            uniform_energy_factor = uniform_ef,
            "HPXML 3.x document: deprecated <EnergyFactor> count={} vs 4.x <UniformEnergyFactor> count={}",
            energy_factor,
            uniform_ef,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{ValidationWarning, validate_building_ranges, validate_hpxml_schema};
    use crate::hpxml::building::parse_building;

    const BASE_XML: &str = r#"
<HPXML xmlns="http://hpxmlonline.com/2019/10" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:schemaLocation="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site>
          <Elevation units="ft">100</Elevation>
          <SiteType>suburban</SiteType>
          <ShieldingOfHome>0.7</ShieldingOfHome>
        </Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
          <ConditionedBuildingVolume units="ft3">14400</ConditionedBuildingVolume>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <Walls>
          <Wall>
            <SystemIdentifier id="Wall1"/>
            <InteriorAdjacentTo>conditioned space</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <Area units="ft2">100</Area>
          </Wall>
        </Walls>
        <Windows>
          <Window>
            <SystemIdentifier id="Window1"/>
            <InteriorAdjacentTo>conditioned space</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <Area units="ft2">35</Area>
            <UFactor>0.3</UFactor>
            <SHGC>0.25</SHGC>
          </Window>
        </Windows>
      </Enclosure>
      <AirInfiltration>
        <AirInfiltrationMeasurement><AirLeakage>35</AirLeakage></AirInfiltrationMeasurement>
      </AirInfiltration>
      <Systems>
        <HVAC>
          <HeatingSystem><HeatingCapacity>250</HeatingCapacity></HeatingSystem>
          <HeatPump><SEER2>9</SEER2><HSPF2>5</HSPF2></HeatPump>
        </HVAC>
        <WaterHeating><WaterHeatingSystem><HotWaterTemperature units="F">110</HotWaterTemperature></WaterHeatingSystem></WaterHeating>
      </Systems>
      <Generation>
        <Battery><RoundTripEfficiency>0.65</RoundTripEfficiency></Battery>
        <PVSystem><Tilt>75</Tilt></PVSystem>
      </Generation>
    </BuildingDetails>
  </Building>
</HPXML>
"#;

    #[test]
    fn schema_validation_rejects_missing_required_elements() {
        let bad = r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0"><Building /></HPXML>"#;
        let err = validate_hpxml_schema(bad).expect_err("schema should fail");
        assert!(err.message.contains("missing required path"));
    }

    #[test]
    fn schema_version_3x_accepted_with_warning() {
        let xml = BASE_XML.replace(r#"schemaVersion="4.0""#, r#"schemaVersion="3.0""#);
        let warnings = validate_hpxml_schema(&xml).expect("3.x should be accepted");
        assert!(
            warnings.iter().any(|w| w.field == "schemaVersion"),
            "3.x should produce a schemaVersion warning"
        );
    }

    #[test]
    fn schema_version_3_4_accepted_with_warning() {
        let xml = BASE_XML.replace(r#"schemaVersion="4.0""#, r#"schemaVersion="3.4""#);
        let warnings = validate_hpxml_schema(&xml).expect("3.4 should be accepted");
        assert!(
            warnings.iter().any(|w| w.field == "schemaVersion"),
            "3.4 should produce a schemaVersion warning"
        );
    }

    #[test]
    fn schema_version_4_2_accepted_without_warning() {
        let xml = BASE_XML.replace(r#"schemaVersion="4.0""#, r#"schemaVersion="4.2""#);
        let warnings = validate_hpxml_schema(&xml).expect("4.2 should be accepted");
        assert!(
            warnings.is_empty(),
            "4.x should not produce warnings, got: {warnings:?}"
        );
    }

    #[test]
    fn schema_version_2x_rejected() {
        let xml = BASE_XML.replace(r#"schemaVersion="4.0""#, r#"schemaVersion="2.3""#);
        let err = validate_hpxml_schema(&xml).expect_err("2.x should be rejected");
        assert!(
            err.field == "schemaVersion",
            "error should be on schemaVersion field"
        );
    }

    #[test]
    fn schema_version_missing_rejected() {
        let xml = BASE_XML.replace(r#" schemaVersion="4.0""#, "");
        let err = validate_hpxml_schema(&xml).expect_err("missing version should be rejected");
        assert!(err.field == "schemaVersion");
        assert!(err.message.contains("missing"));
    }

    #[test]
    fn schema_version_malformed_rejected() {
        let xml = BASE_XML.replace(r#"schemaVersion="4.0""#, r#"schemaVersion="four.0""#);
        let err = validate_hpxml_schema(&xml).expect_err("malformed version should be rejected");
        assert!(err.field == "schemaVersion");
        assert!(
            err.message.contains("four.0"),
            "error should include the malformed value, got: {}",
            err.message
        );
    }

    #[test]
    fn range_validation_applies_error_warning_policy() {
        let building = parse_building(BASE_XML).expect("parse should succeed");
        let report = validate_building_ranges(&building);

        assert!(report.errors.iter().any(|e| e.field == "HVACCapacity"));
        assert!(report.errors.iter().any(|e| e.field == "SEER2"));
        assert!(report.errors.iter().any(|e| e.field == "HSPF2"));
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.field == "BatteryRoundTripEfficiency")
        );
        assert!(
            report.warnings.iter().any(
                |w| matches!(w, ValidationWarning { field, .. } if field == "InfiltrationACH50")
            )
        );
        assert!(report.warnings.iter().any(
            |w| matches!(w, ValidationWarning { field, .. } if field == "WaterHeaterSetpoint")
        ));
        assert!(
            report
                .warnings
                .iter()
                .any(|w| matches!(w, ValidationWarning { field, .. } if field == "PVTilt"))
        );
    }

    // ---------------------------------------------------------------------------
    // BuildingSync namespace handling
    // ---------------------------------------------------------------------------

    /// An HPXML 4.0 document with a co-existing BuildingSync namespace
    /// (xmlns:bsync) alongside the HPXML default namespace.
    const BUILDINGSYNC_XML: &str = r#"
<HPXML xmlns="http://hpxmlonline.com/2019/10"
       xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
       xmlns:bsync="http://buildingsync.net/schemas/bedes-audit/2019/9"
       xsi:schemaLocation="http://hpxmlonline.com/2019/10 http://hpxmlonline.com/2019/10/HPXML.xsd"
       schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site>
          <SiteType>suburban</SiteType>
        </Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
          <ConditionedBuildingVolume units="ft3">14400</ConditionedBuildingVolume>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <Walls>
          <Wall>
            <SystemIdentifier id="Wall1"/>
            <InteriorAdjacentTo>conditioned space</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <Area units="ft2">100</Area>
          </Wall>
        </Walls>
        <Windows>
          <Window>
            <SystemIdentifier id="Window1"/>
            <InteriorAdjacentTo>conditioned space</InteriorAdjacentTo>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <Area units="ft2">35</Area>
            <UFactor>0.3</UFactor>
            <SHGC>0.25</SHGC>
          </Window>
        </Windows>
      </Enclosure>
      <AirInfiltration>
        <AirInfiltrationMeasurement><AirLeakage>35</AirLeakage></AirInfiltrationMeasurement>
      </AirInfiltration>
      <Systems>
        <HVAC>
          <HeatingSystem><HeatingCapacity>250</HeatingCapacity></HeatingSystem>
          <HeatPump><SEER2>9</SEER2><HSPF2>5</HSPF2></HeatPump>
        </HVAC>
        <WaterHeating><WaterHeatingSystem><HotWaterTemperature units="F">110</HotWaterTemperature></WaterHeatingSystem></WaterHeating>
      </Systems>
    </BuildingDetails>
  </Building>
</HPXML>
"#;

    #[test]
    fn building_sync_namespace_passes_schema_validation() {
        let warnings = validate_hpxml_schema(BUILDINGSYNC_XML)
            .expect("HPXML 4.0 + BuildingSync should validate");
        assert!(
            warnings.iter().all(|w| w.field != "xmlns"),
            "no xmlns warnings expected, got: {warnings:?}"
        );
    }

    #[test]
    fn building_sync_namespace_passes_schema_node_validation() {
        let root = super::super::building::parse_xml_document(BUILDINGSYNC_XML)
            .expect("parse should succeed");
        let warnings = super::super::validation::validate_hpxml_schema_node(&root)
            .expect("4.0 + BuildingSync should pass schema node check");
        assert!(
            warnings.iter().all(|w| w.field != "xmlns"),
            "no xmlns warnings expected, got: {warnings:?}"
        );
    }

    #[test]
    fn clean_hpxml_40_no_extra_namespaces_validates_without_false_positive() {
        // Regression guard: a clean HPXML 4.0 document (no BuildingSync)
        // must continue to validate without false positives.
        let warnings =
            validate_hpxml_schema(BASE_XML).expect("clean HPXML 4.0 should continue to validate");
        assert!(
            warnings.is_empty()
                || warnings
                    .iter()
                    .all(|w| w.field == "schemaVersion" && w.message.contains("3.x")),
            "clean HPXML 4.0 should produce no warnings (or only a 3.x deprecation warning), got: {warnings:?}"
        );
    }

    #[test]
    fn building_sync_fixture_resolves_building_properties() {
        let building = parse_building(BUILDINGSYNC_XML).expect("parse should succeed");
        let conditioned = building
            .zones
            .iter()
            .find(|z| matches!(z.zone_type, crate::hpxml::building::ZoneType::Conditioned))
            .expect("should have conditioned zone");
        // 1800 ft² → 167.225 m²
        let area_m2 = conditioned.floor_area_m2.expect("should have area");
        assert!(
            (area_m2 - 167.225).abs() < 0.05,
            "conditioned floor area should be ~167.2 m² from 1800 ft², got {area_m2:.3}"
        );
        // HVAC capacity: 250 BTU/h → 250 * 0.293_071_07 ≈ 73.27 W
        // (HPXML <HeatingCapacity> defaults to BTU/h when no units attribute is present)
        let cap_w = building.hvac_capacity_w.expect("should have capacity");
        assert!(
            (cap_w - 73.27).abs() < 0.1,
            "250 BTU/h should be ~73.27 W, got {cap_w:.2}"
        );
    }
}
