//! PV system sizing from roof geometry.
//!
//! Estimates maximum rooftop PV capacity from parsed roof plane data (area,
//! pitch, azimuth) using production-factor-weighted usable area calculations.
//! Ported from DER_Detection `solar/sizing.py`.

use std::f64::consts::PI;

/// Roof shape classification, determines usable-area fraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoofShape {
    Gable,
    Hip,
    Flat,
}

/// A single roof plane extracted from HPXML boundary data.
#[derive(Debug, Clone, PartialEq)]
pub struct RoofPlane {
    pub area_m2: f64,
    /// Tilt from horizontal in degrees (0 = flat, 90 = vertical).
    pub tilt_deg: f64,
    /// Compass azimuth in degrees (0 = north, 180 = south).
    pub azimuth_deg: Option<f64>,
    /// Roofing material / finish type from HPXML.
    pub material: Option<String>,
    /// Index into `Building.boundaries` for this roof surface. Used to set
    /// `attached_boundary_id` when creating PV equipment from a candidate.
    pub boundary_index: Option<u32>,
}

/// Collection of roof planes for a building.
#[derive(Debug, Clone)]
pub struct RoofInfo {
    pub planes: Vec<RoofPlane>,
    pub total_roof_area_m2: f64,
}

/// Result of usable roof area computation.
#[derive(Debug, Clone)]
pub struct UsableRoofArea {
    /// Index of the best plane in [`RoofInfo::planes`].
    pub best_plane_idx: usize,
    pub usable_m2: f64,
    pub max_panels: u32,
    pub max_capacity_kw: f64,
    pub roof_shape: RoofShape,
    /// Array azimuth in degrees (0 = north, 180 = south).
    pub azimuth_deg: f64,
    /// Array tilt in degrees.
    pub tilt_deg: f64,
}

/// A candidate PV placement on a single roof plane.
#[derive(Debug, Clone)]
pub struct PvCandidate {
    /// Index of the roof plane in [`RoofInfo::planes`].
    pub plane_idx: usize,
    pub azimuth_deg: f64,
    pub tilt_deg: f64,
    pub usable_m2: f64,
    pub max_panels: u32,
    pub max_capacity_kw: f64,
    /// Solar production score (higher = better, relative units).
    pub solar_score: f64,
    pub roof_shape: RoofShape,
    /// Index into `Building.boundaries` for the source roof surface.
    /// Pass this as `attached_boundary_id` when creating PV equipment.
    pub boundary_index: Option<u32>,
}

/// Final PV sizing result.
#[derive(Debug, Clone)]
pub struct PvSizingResult {
    pub capacity_kw: f64,
    pub num_panels: u32,
    pub collector_area_m2: f64,
    pub array_azimuth_deg: f64,
    pub array_tilt_deg: f64,
    pub system_losses_fraction: f64,
    pub max_roof_capacity_kw: f64,
    pub panel_watts: u32,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Modern panel: 420 W, ~2.0 m² (21.5 sqft).
const DEFAULT_PANEL_WATTS: u32 = 420;
const DEFAULT_PANEL_AREA_M2: f64 = 2.0;
const DEFAULT_SYSTEM_LOSSES: f64 = 0.14;
const FLAT_TILT_FALLBACK_DEG: f64 = 10.0;

/// Per-shape usable fraction of gross roof area. Accounts for fire-code
/// setbacks (~15%) and obstruction deductions (~12%). Flat roofs handle
/// row-spacing separately via GCR.
fn usable_fraction(shape: RoofShape) -> f64 {
    match shape {
        RoofShape::Gable => 0.75,
        RoofShape::Hip => 0.35,
        RoofShape::Flat => 0.70,
    }
}

// ---------------------------------------------------------------------------
// Azimuth helpers
// ---------------------------------------------------------------------------

/// Angular distance from due south (180°), result in [0, 180].
fn south_distance(azimuth_deg: f64) -> f64 {
    let delta = (azimuth_deg - 180.0).abs();
    delta.min(360.0 - delta)
}

/// True if the azimuth faces roughly north (within ±45° of 0°/360°).
pub fn is_north_facing(azimuth_deg: f64) -> bool {
    let az = azimuth_deg.rem_euclid(360.0);
    az >= 315.0 || az <= 45.0
}

/// Snap to nearest 45° cardinal/intercardinal and return discrete production
/// factor relative to due south (1.0). Keys use HPXML convention (0 = north).
fn azimuth_production_factor_lut(azimuth_deg: f64) -> f64 {
    let az = azimuth_deg.rem_euclid(360.0);
    // Snap to nearest 45° point.
    let snapped = ((az / 45.0).round() * 45.0) as u32 % 360;
    match snapped {
        180 => 1.00, // South
        135 | 225 => 0.90,
        90 => 0.80,
        270 => 0.85,
        45 => 0.65,
        315 => 0.70,
        0 => 0.45,
        _ => 0.75,
    }
}

/// Continuous production factor with latitude-dependent roll-off.
///
/// `production = diffuse_frac + (1 - diffuse_frac) × cos(θ)^n`
/// where θ is deviation from south and n grows with latitude.
fn plane_solar_score(area_m2: f64, azimuth_deg: f64, shape: RoofShape, latitude: f64) -> f64 {
    const DIFFUSE_FRAC: f64 = 0.18;
    let deviation_rad = (south_distance(azimuth_deg).to_radians()).min(PI / 2.0);
    let exponent = 1.0 + 0.005 * (latitude - 35.0);
    let production_factor =
        DIFFUSE_FRAC + (1.0 - DIFFUSE_FRAC) * deviation_rad.cos().powf(exponent);
    area_m2 * usable_fraction(shape) * production_factor
}

/// Ground coverage ratio for flat roofs, latitude-dependent.
fn flat_roof_gcr(latitude: Option<f64>) -> f64 {
    match latitude {
        Some(lat) if lat > 40.0 => 0.35,
        Some(lat) if lat > 30.0 => 0.40,
        Some(_) => 0.50,
        None => 0.40,
    }
}

/// Resolve the azimuth for a roof plane, falling back to the most-southerly
/// wall azimuth, then to 180° (due south).
fn resolve_azimuth(plane: &RoofPlane, wall_azimuths: &[f64]) -> f64 {
    if let Some(az) = plane.azimuth_deg {
        return az;
    }
    if !wall_azimuths.is_empty() {
        // Prefer the wall azimuth closest to south; break ties west-of-south.
        return *wall_azimuths
            .iter()
            .min_by(|a, b| {
                let da = south_distance(**a);
                let db = south_distance(**b);
                da.partial_cmp(&db)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| {
                        // Prefer west-of-south (az > 180) over east-of-south.
                        let wa = if **a > 180.0 { 0 } else { 1 };
                        let wb = if **b > 180.0 { 0 } else { 1 };
                        wa.cmp(&wb)
                    })
            })
            .unwrap();
    }
    180.0
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compute the usable roof area and maximum PV capacity for a building.
///
/// `wall_azimuths` provides fallback orientation when roof planes lack an
/// explicit azimuth. `latitude` enables latitude-dependent scoring and
/// flat-roof tilt selection.
pub fn compute_usable_area(
    roof: &RoofInfo,
    roof_shape: RoofShape,
    wall_azimuths: &[f64],
    latitude: Option<f64>,
    panel_watts: Option<u32>,
    panel_area_m2: Option<f64>,
) -> Result<UsableRoofArea, PvSizingError> {
    let panel_watts = panel_watts.unwrap_or(DEFAULT_PANEL_WATTS);
    let panel_area_m2 = panel_area_m2.unwrap_or(DEFAULT_PANEL_AREA_M2);

    if roof.planes.is_empty() {
        return Err(PvSizingError::NoRoofPlanes);
    }

    // Filter to non-north-facing candidates with resolved azimuths.
    let candidates: Vec<(usize, f64)> = roof
        .planes
        .iter()
        .enumerate()
        .filter_map(|(i, plane)| {
            let az = resolve_azimuth(plane, wall_azimuths);
            if is_north_facing(az) {
                None
            } else {
                Some((i, az))
            }
        })
        .collect();

    if candidates.is_empty() {
        return Err(PvSizingError::AllNorthFacing);
    }

    let lat = latitude.unwrap_or(35.0);

    // Select the best plane.
    let (best_idx, best_az) = if roof_shape == RoofShape::Hip {
        // Hip: pick the most-southerly plane (smallest south_distance), breaking
        // ties by largest area then west-of-south preference.
        *candidates
            .iter()
            .min_by(|(ia, az_a), (ib, az_b)| {
                let da = south_distance(*az_a);
                let db = south_distance(*az_b);
                da.partial_cmp(&db)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| {
                        let area_a = roof.planes[*ia].area_m2;
                        let area_b = roof.planes[*ib].area_m2;
                        area_b
                            .partial_cmp(&area_a)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .then_with(|| {
                        let wa = if *az_a > 180.0 { 0 } else { 1 };
                        let wb = if *az_b > 180.0 { 0 } else { 1 };
                        wa.cmp(&wb)
                    })
            })
            .unwrap()
    } else {
        // Gable/Flat: pick the plane with the highest solar score.
        *candidates
            .iter()
            .max_by(|(ia, az_a), (ib, az_b)| {
                let sa = plane_solar_score(roof.planes[*ia].area_m2, *az_a, roof_shape, lat);
                let sb = plane_solar_score(roof.planes[*ib].area_m2, *az_b, roof_shape, lat);
                sa.partial_cmp(&sb).unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap()
    };

    let best_plane = &roof.planes[best_idx];

    if roof_shape == RoofShape::Hip {
        // Aggregate panel capacity across all viable planes, weighted by
        // azimuth production factor.
        let mut total_weighted_panels: u32 = 0;
        for &(idx, az) in &candidates {
            let plane = &roof.planes[idx];
            let plane_usable = plane.area_m2 * usable_fraction(RoofShape::Hip);
            let plane_panels = (plane_usable / panel_area_m2).floor() as u32;
            let prod_factor = azimuth_production_factor_lut(az);
            total_weighted_panels += ((plane_panels as f64) * prod_factor).floor() as u32;
        }

        let tilt_deg = best_plane.tilt_deg;
        let best_usable = best_plane.area_m2 * usable_fraction(RoofShape::Hip);

        return Ok(UsableRoofArea {
            best_plane_idx: best_idx,
            usable_m2: best_usable,
            max_panels: total_weighted_panels,
            max_capacity_kw: (total_weighted_panels as f64) * (panel_watts as f64) / 1000.0,
            roof_shape,
            azimuth_deg: best_az,
            tilt_deg,
        });
    }

    // Gable / Flat path.
    let area_m2 = best_plane.area_m2;

    // Gable with a single plane: area is whole-roof footprint, halve for one face.
    let effective_area = if roof_shape == RoofShape::Gable && roof.planes.len() == 1 {
        area_m2 / 2.0
    } else {
        area_m2
    };

    let usable_m2 = effective_area * usable_fraction(roof_shape);

    let panel_footprint = if roof_shape == RoofShape::Flat {
        panel_area_m2 / flat_roof_gcr(latitude)
    } else {
        panel_area_m2
    };

    let max_panels = (usable_m2 / panel_footprint).floor() as u32;
    let max_capacity_kw = (max_panels as f64) * (panel_watts as f64) / 1000.0;

    let tilt_deg = if roof_shape == RoofShape::Flat || best_plane.tilt_deg < 1.0 {
        latitude
            .map(|l| l.min(25.0))
            .unwrap_or(FLAT_TILT_FALLBACK_DEG)
    } else {
        best_plane.tilt_deg
    };

    Ok(UsableRoofArea {
        best_plane_idx: best_idx,
        usable_m2,
        max_panels,
        max_capacity_kw,
        roof_shape,
        azimuth_deg: best_az,
        tilt_deg,
    })
}

/// Size a PV system to a target capacity, clamped by roof constraints.
pub fn size_pv_system(
    usable: &UsableRoofArea,
    target_kw: f64,
    min_kw: f64,
    max_kw: f64,
    system_losses: Option<f64>,
    panel_watts: Option<u32>,
    panel_area_m2: Option<f64>,
) -> Result<PvSizingResult, PvSizingError> {
    let panel_watts = panel_watts.unwrap_or(DEFAULT_PANEL_WATTS);
    let panel_area_m2 = panel_area_m2.unwrap_or(DEFAULT_PANEL_AREA_M2);
    let system_losses = system_losses.unwrap_or(DEFAULT_SYSTEM_LOSSES);

    if usable.max_capacity_kw < min_kw {
        return Err(PvSizingError::InsufficientRoof {
            available_kw: usable.max_capacity_kw,
            min_kw,
        });
    }

    let upper_bound = max_kw.min(usable.max_capacity_kw);
    let clamped_kw = target_kw.clamp(min_kw, upper_bound);
    let num_panels =
        ((clamped_kw * 1000.0 / panel_watts as f64).ceil() as u32).min(usable.max_panels);
    let capacity_kw = (num_panels as f64) * (panel_watts as f64) / 1000.0;
    let collector_area_m2 = (num_panels as f64) * panel_area_m2;

    Ok(PvSizingResult {
        capacity_kw,
        num_panels,
        collector_area_m2,
        array_azimuth_deg: usable.azimuth_deg,
        array_tilt_deg: usable.tilt_deg,
        system_losses_fraction: system_losses,
        max_roof_capacity_kw: usable.max_capacity_kw,
        panel_watts,
    })
}

/// Enumerate all viable PV candidate placements, one per non-north-facing
/// roof plane. Results are sorted by solar score (best first).
///
/// This lets users see all placement options rather than just the single best.
pub fn enumerate_pv_candidates(
    roof: &RoofInfo,
    roof_shape: RoofShape,
    wall_azimuths: &[f64],
    latitude: Option<f64>,
    panel_watts: Option<u32>,
    panel_area_m2: Option<f64>,
) -> Vec<PvCandidate> {
    let panel_watts = panel_watts.unwrap_or(DEFAULT_PANEL_WATTS);
    let panel_area_m2 = panel_area_m2.unwrap_or(DEFAULT_PANEL_AREA_M2);
    let lat = latitude.unwrap_or(35.0);

    let mut candidates: Vec<PvCandidate> = roof
        .planes
        .iter()
        .enumerate()
        .filter_map(|(i, plane)| {
            let az = resolve_azimuth(plane, wall_azimuths);
            if is_north_facing(az) {
                return None;
            }

            let area = plane.area_m2;
            // For single-plane gable, halve.
            let effective_area = if roof_shape == RoofShape::Gable && roof.planes.len() == 1 {
                area / 2.0
            } else {
                area
            };

            let usable_m2 = effective_area * usable_fraction(roof_shape);
            let panel_footprint = if roof_shape == RoofShape::Flat {
                panel_area_m2 / flat_roof_gcr(latitude)
            } else {
                panel_area_m2
            };
            let max_panels = (usable_m2 / panel_footprint).floor() as u32;
            let max_capacity_kw = (max_panels as f64) * (panel_watts as f64) / 1000.0;

            let tilt_deg = if roof_shape == RoofShape::Flat || plane.tilt_deg < 1.0 {
                latitude
                    .map(|l| l.min(25.0))
                    .unwrap_or(FLAT_TILT_FALLBACK_DEG)
            } else {
                plane.tilt_deg
            };

            let solar_score = plane_solar_score(effective_area, az, roof_shape, lat);

            Some(PvCandidate {
                plane_idx: i,
                azimuth_deg: az,
                tilt_deg,
                usable_m2,
                max_panels,
                max_capacity_kw,
                solar_score,
                roof_shape,
                boundary_index: plane.boundary_index,
            })
        })
        .collect();

    // Sort by solar score descending (best first).
    candidates.sort_by(|a, b| {
        b.solar_score
            .partial_cmp(&a.solar_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    candidates
}

/// Errors from PV sizing.
#[derive(Debug, Clone, thiserror::Error)]
pub enum PvSizingError {
    #[error("no roof planes available")]
    NoRoofPlanes,
    #[error("all roof planes are north-facing or missing azimuth data")]
    AllNorthFacing,
    #[error("roof capacity {available_kw:.1} kW below minimum {min_kw} kW")]
    InsufficientRoof { available_kw: f64, min_kw: f64 },
}

// ---------------------------------------------------------------------------
// Roof shape inference
// ---------------------------------------------------------------------------

/// Infer roof shape from building metadata.
///
/// `facility_type` is the HPXML `ResidentialFacilityType` string.
/// Falls back to `Gable` when evidence is ambiguous.
pub fn infer_roof_shape(
    roof: &RoofInfo,
    facility_type: Option<&str>,
    latitude: Option<f64>,
) -> RoofShape {
    // Apartments / multifamily 5+ → Flat.
    if let Some(ft) = facility_type {
        let ft_lower = ft.to_lowercase();
        if ft_lower.contains("apartment") || ft_lower.contains("5+") {
            return RoofShape::Flat;
        }
    }

    // All planes have tilt ≈ 0 → Flat.
    if !roof.planes.is_empty() && roof.planes.iter().all(|p| p.tilt_deg < 1.0) {
        return RoofShape::Flat;
    }

    // Multiple planes with distinct azimuths → Hip.
    let distinct_azimuths: std::collections::HashSet<u32> = roof
        .planes
        .iter()
        .filter_map(|p| p.azimuth_deg.map(|a| (a / 45.0).round() as u32))
        .collect();
    if distinct_azimuths.len() >= 3 {
        return RoofShape::Hip;
    }

    // Material hint: tile/slate common on hip roofs.
    let has_tile_slate = roof.planes.iter().any(|p| {
        p.material.as_ref().is_some_and(|m| {
            let ml = m.to_lowercase();
            ml.contains("tile") || ml.contains("slate")
        })
    });
    if has_tile_slate && distinct_azimuths.len() >= 2 {
        return RoofShape::Hip;
    }

    // Low-latitude regions with 2 planes — mild hip signal.
    if distinct_azimuths.len() >= 2 && latitude.is_some_and(|l| l < 30.0) {
        return RoofShape::Hip;
    }

    RoofShape::Gable
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(area_m2: f64, tilt_deg: f64, azimuth_deg: Option<f64>) -> RoofPlane {
        RoofPlane {
            area_m2,
            tilt_deg,
            azimuth_deg,
            material: None,
            boundary_index: None,
        }
    }

    #[test]
    fn south_distance_symmetric() {
        assert!((south_distance(180.0) - 0.0).abs() < 1e-10);
        assert!((south_distance(135.0) - 45.0).abs() < 1e-10);
        assert!((south_distance(225.0) - 45.0).abs() < 1e-10);
        assert!((south_distance(0.0) - 180.0).abs() < 1e-10);
    }

    #[test]
    fn north_facing_detection() {
        assert!(is_north_facing(0.0));
        assert!(is_north_facing(45.0));
        assert!(is_north_facing(315.0));
        assert!(is_north_facing(350.0));
        assert!(!is_north_facing(90.0));
        assert!(!is_north_facing(180.0));
        assert!(!is_north_facing(270.0));
    }

    #[test]
    fn single_south_gable() {
        let roof = RoofInfo {
            planes: vec![plane(100.0, 26.0, Some(180.0))],
            total_roof_area_m2: 100.0,
        };
        let usable =
            compute_usable_area(&roof, RoofShape::Gable, &[], Some(40.0), None, None).unwrap();
        // Single gable plane → halved, then ×0.75.
        assert!((usable.usable_m2 - 100.0 / 2.0 * 0.75).abs() < 0.01);
        assert!((usable.azimuth_deg - 180.0).abs() < 0.01);
        assert!((usable.tilt_deg - 26.0).abs() < 0.01);
    }

    #[test]
    fn two_plane_gable_picks_south() {
        let roof = RoofInfo {
            planes: vec![
                plane(50.0, 26.0, Some(180.0)), // south
                plane(50.0, 26.0, Some(0.0)),   // north (filtered)
            ],
            total_roof_area_m2: 100.0,
        };
        let usable =
            compute_usable_area(&roof, RoofShape::Gable, &[], Some(40.0), None, None).unwrap();
        // Two planes → no halving; south plane used directly.
        assert!((usable.usable_m2 - 50.0 * 0.75).abs() < 0.01);
    }

    #[test]
    fn all_north_facing_returns_error() {
        let roof = RoofInfo {
            planes: vec![plane(100.0, 26.0, Some(0.0))],
            total_roof_area_m2: 100.0,
        };
        let result = compute_usable_area(&roof, RoofShape::Gable, &[], None, None, None);
        assert!(result.is_err());
    }

    #[test]
    fn flat_roof_uses_gcr() {
        let roof = RoofInfo {
            planes: vec![plane(200.0, 0.0, Some(180.0))],
            total_roof_area_m2: 200.0,
        };
        let usable =
            compute_usable_area(&roof, RoofShape::Flat, &[], Some(35.0), None, None).unwrap();
        // Flat: effective = 200 × 0.70 = 140 m².
        // Panel footprint = 2.0 / 0.40 = 5.0 m² (lat 35 → GCR 0.40).
        // Max panels = floor(140 / 5) = 28.
        assert_eq!(usable.max_panels, 28);
    }

    #[test]
    fn hip_aggregates_multiple_planes() {
        let roof = RoofInfo {
            planes: vec![
                plane(60.0, 26.0, Some(180.0)), // south
                plane(40.0, 26.0, Some(225.0)), // southwest
                plane(40.0, 26.0, Some(0.0)),   // north (filtered)
            ],
            total_roof_area_m2: 140.0,
        };
        let usable =
            compute_usable_area(&roof, RoofShape::Hip, &[], Some(40.0), None, None).unwrap();
        assert!(usable.max_panels > 0);
        assert!((usable.azimuth_deg - 180.0).abs() < 0.01);
    }

    #[test]
    fn size_pv_clamps_to_roof() {
        let usable = UsableRoofArea {
            best_plane_idx: 0,
            usable_m2: 37.5,
            max_panels: 18,
            max_capacity_kw: 7.56,
            roof_shape: RoofShape::Gable,
            azimuth_deg: 180.0,
            tilt_deg: 26.0,
        };
        let result = size_pv_system(&usable, 10.0, 2.0, 14.0, None, None, None).unwrap();
        assert!(result.capacity_kw <= usable.max_capacity_kw + 0.01);
        assert!(result.num_panels <= usable.max_panels);
    }

    #[test]
    fn size_pv_errors_below_minimum() {
        let usable = UsableRoofArea {
            best_plane_idx: 0,
            usable_m2: 3.0,
            max_panels: 1,
            max_capacity_kw: 0.42,
            roof_shape: RoofShape::Gable,
            azimuth_deg: 180.0,
            tilt_deg: 26.0,
        };
        let result = size_pv_system(&usable, 6.0, 2.0, 14.0, None, None, None);
        assert!(result.is_err());
    }

    #[test]
    fn infer_flat_from_apartment() {
        let roof = RoofInfo {
            planes: vec![plane(100.0, 0.0, Some(180.0))],
            total_roof_area_m2: 100.0,
        };
        assert_eq!(
            infer_roof_shape(&roof, Some("apartment"), None),
            RoofShape::Flat
        );
    }

    #[test]
    fn infer_flat_from_zero_pitch() {
        let roof = RoofInfo {
            planes: vec![plane(100.0, 0.0, Some(180.0))],
            total_roof_area_m2: 100.0,
        };
        assert_eq!(
            infer_roof_shape(&roof, Some("single-family detached"), None),
            RoofShape::Flat
        );
    }

    #[test]
    fn infer_gable_default() {
        let roof = RoofInfo {
            planes: vec![plane(50.0, 26.0, Some(180.0)), plane(50.0, 26.0, Some(0.0))],
            total_roof_area_m2: 100.0,
        };
        assert_eq!(
            infer_roof_shape(&roof, Some("single-family detached"), Some(40.0)),
            RoofShape::Gable
        );
    }

    #[test]
    fn infer_hip_from_many_azimuths() {
        let roof = RoofInfo {
            planes: vec![
                plane(30.0, 26.0, Some(90.0)),
                plane(30.0, 26.0, Some(180.0)),
                plane(30.0, 26.0, Some(270.0)),
            ],
            total_roof_area_m2: 90.0,
        };
        assert_eq!(infer_roof_shape(&roof, None, None), RoofShape::Hip);
    }

    #[test]
    fn enumerate_returns_multiple_candidates() {
        let roof = RoofInfo {
            planes: vec![
                plane(60.0, 26.0, Some(180.0)), // south — best
                plane(40.0, 26.0, Some(225.0)), // southwest
                plane(30.0, 26.0, Some(90.0)),  // east
                plane(50.0, 26.0, Some(0.0)),   // north — filtered
            ],
            total_roof_area_m2: 180.0,
        };
        let candidates =
            enumerate_pv_candidates(&roof, RoofShape::Gable, &[], Some(40.0), None, None);
        // 3 non-north planes.
        assert_eq!(candidates.len(), 3);
        // Best first (south with most area).
        assert!((candidates[0].azimuth_deg - 180.0).abs() < 0.01);
        // All have positive capacity.
        for c in &candidates {
            assert!(c.max_capacity_kw > 0.0);
            assert!(c.max_panels > 0);
        }
    }

    #[test]
    fn wall_azimuth_fallback() {
        let roof = RoofInfo {
            planes: vec![plane(100.0, 26.0, None)], // no azimuth
            total_roof_area_m2: 100.0,
        };
        let usable = compute_usable_area(
            &roof,
            RoofShape::Gable,
            &[90.0, 180.0, 270.0],
            Some(40.0),
            None,
            None,
        )
        .unwrap();
        assert!((usable.azimuth_deg - 180.0).abs() < 0.01);
    }
}
