//! HPXML building description parser.

use std::fs;
use std::path::Path;

use thiserror::Error;

pub mod building;
pub mod equipment;
mod resolve_der;
pub mod resolve_hvac;
mod resolve_loads;
mod resolve_pool;
mod resolve_water_heater;
pub mod validation;
pub mod water_heater_ua;
pub(crate) mod xml_helpers;

use building::{parse_building_from_node, parse_xml_document};
use validation::{ValidationError, validate_building_ranges, validate_hpxml_schema_node};

pub use building::{
    Boundary, BoundaryType, Building, DuctLocation, DuctSystem, DuctType, MaterialLayer, Site,
    SiteType, Window, Zone, ZoneType,
};
pub use equipment::{EquipmentSpec, nested_update, resolve_equipment};
pub use validation::{ValidationReport, ValidationWarning};

#[derive(Debug, Error)]
pub enum HpxmlError {
    #[error("io error reading `{path}`: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("HPXML parse error: {0}")]
    Parse(String),
    #[error("HPXML schema validation error: {0}")]
    SchemaValidation(ValidationError),
    #[error("HPXML domain validation failed ({error_count} errors)")]
    DomainValidation {
        report: validation::ValidationReport,
        error_count: usize,
    },
    /// A required physics/engineering field is missing from the HPXML input.
    ///
    /// Emitted when the parser would otherwise silently substitute a literal
    /// default (e.g. AFUE=0.80, PV capacity=0 kW). Includes the HPXML element
    /// path and, when available, the enclosing system identifier.
    #[error("HPXML is missing required field `{path}` on {system_kind} `{system_id}` -- {reason}")]
    MissingField {
        /// HPXML element path (e.g. `PVSystem/MaxPowerOutput`).
        path: &'static str,
        /// Short type label for the enclosing system (e.g. `PV`, `Gas Furnace`).
        system_kind: &'static str,
        /// Identifier from `SystemIdentifier/@id` or `unknown` when absent.
        system_id: String,
        /// Human-readable explanation of what the field represents and why it
        /// must be specified explicitly instead of defaulted.
        reason: &'static str,
    },
    /// A supplied HPXML field value is outside the acceptable range.
    #[error(
        "HPXML field `{path}` on {system_kind} `{system_id}` has invalid value \
         `{value_received}` -- {reason}"
    )]
    InvalidField {
        /// HPXML element path (e.g. `HeatingSystem/AnnualHeatingEfficiency`).
        path: &'static str,
        /// Short type label for the enclosing system.
        system_kind: &'static str,
        /// Identifier from `SystemIdentifier/@id` or `unknown` when absent.
        system_id: String,
        /// String representation of the invalid value received.
        value_received: String,
        /// Human-readable explanation of the valid range or constraint.
        reason: &'static str,
    },
}

pub type Result<T> = std::result::Result<T, HpxmlError>;

/// Parse HPXML from a file path.
///
/// Validation order:
/// 1. Schema/structure checks: 3.x accepted with warning, 4.x silently, below 3 rejected.
/// 2. Structural extraction into the `Building` model.
/// 3. Domain range checks.
pub fn parse_hpxml(path: &Path) -> Result<Building> {
    let xml = fs::read_to_string(path).map_err(|source| HpxmlError::Io {
        path: path.display().to_string(),
        source,
    })?;

    parse_hpxml_str(&xml)
}

/// Parse HPXML from a string input.
pub fn parse_hpxml_str(xml: &str) -> Result<Building> {
    let root = parse_xml_document(xml).map_err(|e| HpxmlError::Parse(e.to_string()))?;

    let schema_warnings =
        validate_hpxml_schema_node(&root).map_err(HpxmlError::SchemaValidation)?;
    for w in &schema_warnings {
        tracing::warn!(field = %w.field, message = %w.message, "HPXML schema warning");
    }

    let building = parse_building_from_node(&root)?;

    let report = validate_building_ranges(&building);
    if report.has_errors() {
        return Err(HpxmlError::DomainValidation {
            error_count: report.errors.len(),
            report,
        });
    }

    Ok(building)
}

#[cfg(test)]
mod tests {
    use super::{HpxmlError, parse_hpxml_str};

    #[test]
    fn parse_fails_on_floor_area_out_of_range() {
        let xml = r#"
<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><Elevation>100</Elevation><SiteType>suburban</SiteType><ShieldingOfHome>0.5</ShieldingOfHome></Site>
        <BuildingConstruction><ConditionedFloorArea units="m2">5</ConditionedFloorArea><ConditionedBuildingVolume units="m3">12.5</ConditionedBuildingVolume></BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
    </BuildingDetails>
  </Building>
</HPXML>
"#;
        let err = parse_hpxml_str(xml).expect_err("expected range validation failure");
        assert!(matches!(err, HpxmlError::DomainValidation { .. }));
    }
}
