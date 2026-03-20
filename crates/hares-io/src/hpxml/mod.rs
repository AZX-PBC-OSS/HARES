//! HPXML building description parser.

use std::fs;
use std::path::Path;

use thiserror::Error;

pub mod building;
pub mod equipment;
pub mod validation;
pub mod water_heater_ua;

use building::parse_building;
use validation::{ValidationError, validate_building_ranges, validate_hpxml_schema};

pub use building::{
    Boundary, BoundaryType, Building, DuctLocation, DuctSystem, MaterialLayer, Site, SiteType,
    Window, Zone, ZoneType,
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
}

pub type Result<T> = std::result::Result<T, HpxmlError>;

/// Parse HPXML from a file path.
///
/// Validation order:
/// 1. Schema/structure checks against HPXML 4.0 expectations.
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
    let schema_warnings = validate_hpxml_schema(xml).map_err(HpxmlError::SchemaValidation)?;
    for w in &schema_warnings {
        tracing::warn!("{w}");
    }

    let building = parse_building(xml)?;

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
        <BuildingConstruction><ConditionedFloorArea units="m2">5</ConditionedFloorArea></BuildingConstruction>
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
