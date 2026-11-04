//! Extract PV-sizing inputs from parsed HPXML [`Building`] data.

use hares_physics::pv_sizing::{RoofInfo, RoofPlane};

use crate::hpxml::building::{BoundaryType, Building};

/// Extract roof planes and wall azimuths from a parsed HPXML building.
///
/// Each [`RoofPlane`] carries its `boundary_index` so that PV candidates can
/// be attached back to the correct envelope surface for shading.
///
/// Returns `(roof_info, wall_azimuths)` where wall azimuths are sorted and
/// deduplicated (rounded to nearest degree).
pub fn extract_roof_info(building: &Building) -> (RoofInfo, Vec<f64>) {
    let planes: Vec<RoofPlane> = building
        .boundaries
        .iter()
        .enumerate()
        .filter(|(_, b)| b.boundary_type == BoundaryType::Roof)
        .map(|(idx, b)| RoofPlane {
            area_m2: b.area_m2,
            tilt_deg: b.tilt_deg.unwrap_or(0.0),
            azimuth_deg: b.azimuth_deg,
            material: b.finish_type.clone(),
            boundary_index: Some(idx as u32),
        })
        .collect();

    let total_roof_area_m2 = planes.iter().map(|p| p.area_m2).sum();

    let mut wall_azimuths: Vec<f64> = building
        .boundaries
        .iter()
        .filter(|b| b.boundary_type == BoundaryType::Wall)
        .filter_map(|b| b.azimuth_deg)
        .map(|a| a.round())
        .collect();
    wall_azimuths.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    wall_azimuths.dedup();

    let roof = RoofInfo {
        planes,
        total_roof_area_m2,
    };

    (roof, wall_azimuths)
}
