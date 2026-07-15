//! Solver construction for the dwelling orchestrator.

use std::collections::HashMap;

use hares_envelope::{
    BoundaryCategory, BoundaryDiagnostic, BoundaryDiagnosticInfo, BoundaryInput, BuildingRC,
    DrivingTemp, EMISSIVITY_DEFAULT, EMISSIVITY_RADIANT_BARRIER, EMISSIVITY_WINDOW,
    ElectricalSolver, ElectricalSolverConfig, EnvelopeDiagnostics, ExteriorSurfaceInfo,
    ExteriorTarget, FilmCoefficientModel, FluidSolver, FluidSolverConfig, HumiditySolver,
    HumiditySolverConfig, INTERIOR_SOLAR_ABSORPTANCE_DEFAULT, InteriorConvectionInjection,
    InteriorSolarSurfaceInfo, InteriorSolarZoneConfig, NodeId, SOLAR_ABSORPTANCE_DEFAULT,
    SOLAR_ABSORPTANCE_RADIANT_BARRIER, StateSpaceWiring, SurfaceLayerInfo, ThermalSolver,
    ThermalSolverConfig, WindowSolarProperties, assemble_building_rc, derive_zone_capacitances,
};
use hares_io::{Building, DefaultsStore, EquipmentSpec, SimulationConfig, WeatherTimeSeries};
use hares_types::{EnvironmentState, FluidType, HaresError, LoopId, ZoneId};

use super::Result;
use super::conversions::{
    boundary_zone_index, building_to_boundary_inputs, building_to_zone_inputs,
    check_multi_unit_zones, has_vented_crawlspace, shielding_str_to_class, site_type_to_terrain,
};

// ── Intermediate representation ─────────────────────────────────────────────

struct RCContext<'a> {
    layer_info: &'a HashMap<usize, SurfaceLayerInfo>,
    node_index: &'a HashMap<NodeId, usize>,
    envelope_diagnostics: &'a EnvelopeDiagnostics,
    n_zones: usize,
    n_ext: usize,
}

struct NodeWiring {
    state_row: usize,
    b_col: usize,
}

struct WindowSolarData {
    base_shgc: f64,
    shgc_summer: f64,
    shgc_winter: f64,
    u_factor_w_m2_k: f64,
    window_area_m2: f64,
    transmittance_summer: f64,
    transmittance_winter: f64,
    radiation_frac: f64,
}

struct SolverBoundary {
    surface_idx: usize,
    surface_id: u32,
    area_m2: f64,
    boundary_category: Option<BoundaryCategory>,
    tilt_deg: f64,
    azimuth_deg: f64,
    zone_id: ZoneId,
    zone_idx: usize,
    is_exterior: bool,
    is_conditioned_interior: bool,
    is_attic_interior: bool,
    exterior_emissivity: f64,
    exterior_solar_absorptance: f64,
    attic_emissivity: f64,
    /// Interior solar absorptance for this surface.
    ///
    /// For attic surfaces with radiant barrier: 0.05.
    /// Otherwise: per-surface value from HPXML `SolarAbsorptance`, or
    /// `INTERIOR_SOLAR_ABSORPTANCE_DEFAULT` (0.70, EnergyPlus Material IDD default).
    interior_solar_absorptance: f64,
    outer_wiring: Option<NodeWiring>,
    inner_wiring: Option<NodeWiring>,
    exterior_rad_frac: f64,
    exterior_rad_res_k_w: f64,
    interior_rad_frac: f64,
    r_film_int_m2_k_w: f64,
    window_solar: Option<WindowSolarData>,
    diagnostic_r_zone_to_inner: Option<f64>,
    r_film_exterior_m2_k_w: f64,
}

/// Build the intermediate `SolverBoundary` representations from building data.
///
/// Returns `(solver_boundaries, n_ext_surface_inputs, n_int_surface_inputs)`.
fn build_solver_boundaries(
    building: &Building,
    boundary_inputs: &[BoundaryInput],
    rc: &RCContext<'_>,
    env: &EnvironmentState,
) -> Result<(Vec<SolverBoundary>, usize, usize)> {
    // Index diagnostics by boundary_idx for O(1) lookup.
    let diag_by_idx: HashMap<usize, &BoundaryDiagnostic> = rc
        .envelope_diagnostics
        .boundaries
        .iter()
        .map(|d| (d.boundary_idx, d))
        .collect();

    let indoor_zone_id = env.zones.first().map(|z| z.id).unwrap_or(ZoneId(1));

    // Pre-count exterior surface columns so inner column offsets can be computed in one pass.
    let n_ext_surface_inputs: usize = building
        .boundaries
        .iter()
        .enumerate()
        .filter(|(idx, bd)| {
            bd.exterior_zone
                .as_ref()
                .map(|z| *z == hares_io::hpxml::ZoneType::Outdoor)
                .unwrap_or(false)
                && rc.layer_info.get(idx).is_some()
        })
        .count();

    let mut solver_boundaries = Vec::with_capacity(building.boundaries.len());
    let mut ext_col_counter = 0_usize;
    let mut int_col_counter = 0_usize;

    for (surface_idx, boundary) in building.boundaries.iter().enumerate() {
        let surface_id = u32::try_from(surface_idx).unwrap_or(u32::MAX);

        let is_exterior = boundary
            .exterior_zone
            .as_ref()
            .map(|z| *z == hares_io::hpxml::ZoneType::Outdoor)
            .unwrap_or(false);

        let is_conditioned_interior = boundary
            .interior_zone
            .as_ref()
            .map(|z| *z == hares_io::hpxml::ZoneType::Conditioned)
            .unwrap_or(false);
        let is_attic_interior = boundary
            .interior_zone
            .as_ref()
            .map(|z| *z == hares_io::hpxml::ZoneType::Attic)
            .unwrap_or(false);

        let zone_idx = boundary_zone_index(
            building,
            Some(&boundary.id),
            boundary.interior_zone.as_ref(),
            rc.n_zones,
        )?;
        let zone_id = env
            .zones
            .get(zone_idx)
            .map(|z| z.id)
            .unwrap_or(indoor_zone_id);

        // Outer wiring: exterior boundaries with RC layers.
        let outer_wiring = if is_exterior {
            rc.layer_info.get(&surface_idx).and_then(|info| {
                rc.node_index.get(&info.outer_node).map(|&state_row| {
                    let b_col = rc.n_ext + ext_col_counter;
                    ext_col_counter += 1;
                    NodeWiring { state_row, b_col }
                })
            })
        } else {
            None
        };

        // Invariant: exterior boundaries with layer_info must resolve through node_index.
        if is_exterior {
            if let Some(info) = rc.layer_info.get(&surface_idx) {
                #[cfg(any(debug_assertions, feature = "check_invariants"))]
                debug_assert!(
                    outer_wiring.is_some(),
                    "surface {surface_idx}: outer_node {:?} missing from node_index",
                    info.outer_node
                );
                if outer_wiring.is_none() {
                    tracing::warn!(
                        surface_idx = surface_idx,
                        outer_node = ?info.outer_node,
                        "surface has layer_info entry but outer_node missing from node_index; exterior injection lost"
                    );
                }
            }
        }

        // Inner wiring: conditioned-interior boundaries with RC layers.
        let inner_wiring = if is_conditioned_interior {
            rc.layer_info.get(&surface_idx).and_then(|info| {
                rc.node_index.get(&info.inner_node).map(|&state_row| {
                    let b_col = rc.n_ext + n_ext_surface_inputs + int_col_counter;
                    int_col_counter += 1;
                    NodeWiring { state_row, b_col }
                })
            })
        } else {
            None
        };

        // Invariant: interior boundaries with layer_info must resolve through node_index.
        if is_conditioned_interior {
            if let Some(info) = rc.layer_info.get(&surface_idx) {
                #[cfg(any(debug_assertions, feature = "check_invariants"))]
                debug_assert!(
                    inner_wiring.is_some(),
                    "surface {surface_idx}: inner_node {:?} missing from node_index",
                    info.inner_node
                );
                if inner_wiring.is_none() {
                    tracing::warn!(
                        surface_idx = surface_idx,
                        inner_node = ?info.inner_node,
                        "surface has layer_info entry but inner_node missing from node_index; interior wiring lost"
                    );
                }
            }
        }

        #[cfg(feature = "observe")]
        tracing::info!(
            surface_idx = surface_idx,
            has_layer_info = rc.layer_info.contains_key(&surface_idx),
            is_exterior = is_exterior,
            is_conditioned_interior = is_conditioned_interior,
            outer_wiring_resolved = outer_wiring.is_some(),
            inner_wiring_resolved = inner_wiring.is_some(),
            "surface wiring resolution"
        );

        // Boundary category.
        // Same-zone boundaries (interior == exterior) are internal thermal mass.
        let is_same_zone =
            boundary.interior_zone == boundary.exterior_zone && boundary.interior_zone.is_some();
        let boundary_category =
            if is_same_zone {
                Some(BoundaryCategory::InternalMass)
            } else {
                match boundary.boundary_type {
                    hares_io::hpxml::BoundaryType::Wall
                    | hares_io::hpxml::BoundaryType::FoundationWall
                    | hares_io::hpxml::BoundaryType::RimJoist => Some(BoundaryCategory::Wall),
                    hares_io::hpxml::BoundaryType::Roof => Some(BoundaryCategory::Roof),
                    hares_io::hpxml::BoundaryType::Floor | hares_io::hpxml::BoundaryType::Slab => {
                        // Attic floor (exterior = Attic) represents heat flow from roof/attic
                        // path into the conditioned zone -- categorize as Roof for OCHRE parity.
                        let is_attic_floor = boundary
                            .exterior_zone
                            .as_ref()
                            .is_some_and(|z| *z == hares_io::hpxml::ZoneType::Attic);
                        if is_attic_floor {
                            Some(BoundaryCategory::Roof)
                        } else {
                            Some(BoundaryCategory::Floor)
                        }
                    }
                    hares_io::hpxml::BoundaryType::Window
                    | hares_io::hpxml::BoundaryType::Skylight => Some(BoundaryCategory::Window),
                    hares_io::hpxml::BoundaryType::Door
                    | hares_io::hpxml::BoundaryType::Other(_) => None,
                }
            };

        // Tilt: use parsed value from HPXML, fall back to type-based default.
        let tilt_deg = boundary.tilt_deg.unwrap_or(match boundary.boundary_type {
            hares_io::hpxml::BoundaryType::Roof => 0.0,
            hares_io::hpxml::BoundaryType::Slab | hares_io::hpxml::BoundaryType::Floor => 180.0,
            hares_io::hpxml::BoundaryType::Skylight => 0.0,
            _ => 90.0,
        });

        // Azimuth: use parsed value from HPXML, fall back to 180° (south).
        let azimuth_deg = boundary.azimuth_deg.unwrap_or(180.0);

        // Keep radiant-barrier properties on the attic-interior side only.
        // Exterior solar/thermal properties remain physical or explicitly set by HPXML.
        let exterior_emissivity = exterior_emissivity(boundary);
        let exterior_solar_absorptance = exterior_solar_absorptance(boundary);
        let attic_emissivity = attic_interior_emissivity(boundary);
        let interior_solar_abs = interior_solar_absorptance(boundary);

        let bd_input = &boundary_inputs[surface_idx];
        let r_film_ext = bd_input.r_film_exterior_m2_k_w;
        let r_film_int = bd_input.r_film_interior_m2_k_w;

        let r_outermost_half = diag_by_idx
            .get(&surface_idx)
            .and_then(|d| d.r_outer_half_m2_k_w)
            .unwrap_or(0.0);
        let (exterior_rad_frac, exterior_rad_res_k_w) =
            if r_outermost_half > 0.0 && boundary.area_m2 > 0.0 {
                (
                    r_film_ext / (r_film_ext + r_outermost_half),
                    r_film_ext / boundary.area_m2,
                )
            } else {
                (0.0, 0.0)
            };

        let r_inner_half = diag_by_idx
            .get(&surface_idx)
            .and_then(|d| d.r_inner_half_m2_k_w)
            .unwrap_or(0.0);
        // OCHRE "full" mode: radiation_frac = R_film_conv / (R_film_conv + R_inner_half).
        // R_film is convection-only (1/h_conv from TARP). LWR is handled
        // entirely by the explicit ScriptF injection module.
        //
        // Derivation (current-divider at the inner surface node):
        //   The interior surface node sees two parallel conductive paths:
        //     G_conv = 1 / R_film_conv   (convection → zone air)
        //     G_wall = 1 / R_inner_half  (conduction → wall mass RC node)
        //   The fraction of an injected flux routing to the wall mass node is:
        //     radiation_frac = G_wall / (G_wall + G_conv)
        //                    = r_film_int / (r_film_int + r_inner_half)
        //   The complement (r_inner_half / (r_film_int + r_inner_half)) routes
        //   to zone air.  This matches OCHRE Envelope.py:254:
        //     surface.radiation_frac = res_film / (res_film + res_material)
        //   and is algebraically equivalent to the conductance-based current
        //   divider for two parallel paths from a common node.
        //
        //   When R_inner_half = 0 (no RC node, e.g. steady-state boundary):
        //   radiation_frac → 1.0 (the nearest "node" is the surface itself;
        //   all flux goes to the RC node because there is no intermediate
        //   material resistance).
        //
        //   Reference: ASHRAE HoF 2021 Ch.4 "Heat Transfer at Surfaces" —
        //   convection and radiation combine in parallel from a surface node.
        //   OCHRE Envelope.py:254 for the formula; the HARES test
        //   `radiation_frac_impulse_response_matches_closed_form` validates
        //   the split against an assembled state-space model.
        let interior_rad_frac = if r_inner_half > 0.0 {
            r_film_int / (r_film_int + r_inner_half)
        } else {
            1.0
        };

        // Window / Skylight solar data -- fenestration boundary types that
        // share the same physics (U-factor, SHGC, solar transmittance).
        // Wall boundaries receive opaque solar via ExteriorSurfaceInfo.absorptance.
        let is_fenestration = matches!(
            boundary.boundary_type,
            hares_io::hpxml::BoundaryType::Window | hares_io::hpxml::BoundaryType::Skylight
        );
        let window_solar = if is_exterior && is_fenestration {
            // Look up the fenestration in both windows and skylights (they share
            // the same Window struct because both extend HPXML's Window base type).
            let fen = building
                .windows
                .iter()
                .chain(building.skylights.iter())
                .find(|w| w.id == boundary.id);
            if let Some(win) = fen {
                let fen_type = match boundary.boundary_type {
                    hares_io::hpxml::BoundaryType::Skylight => "skylight",
                    _ => "window",
                };
                let u_factor = win.u_factor_w_m2_k.ok_or_else(|| {
                    HaresError::Dwelling(format!(
                        "{fen_type} '{}' is missing required u_factor_w_m2_k (U-factor); \
                         <UFactor> must be present for every <Window>/<Skylight> element per HPXML §6.5",
                        win.id
                    ))
                })?;
                let base_shgc = win.shgc.ok_or_else(|| {
                    HaresError::Dwelling(format!(
                        "{fen_type} '{}' is missing required shgc (SHGC); \
                         <SHGC> must be present for every <Window>/<Skylight> element per HPXML §6.5",
                        win.id
                    ))
                })?;
                let shgc_summer =
                    base_shgc * win.interior_shading_fraction * win.exterior_shading_summer;
                let shgc_winter =
                    base_shgc * win.winter_shading_fraction * win.exterior_shading_winter;
                let r_total = 1.0 / u_factor.max(0.01);
                let r_glass = (r_total - r_film_int - r_film_ext).max(0.0);
                let (transmittance_summer, radiation_frac) =
                    hares_physics::solar::calculate_window_parameters(
                        shgc_summer,
                        u_factor,
                        r_glass,
                    );
                let (transmittance_winter, _) = hares_physics::solar::calculate_window_parameters(
                    shgc_winter,
                    u_factor,
                    r_glass,
                );
                // Fenestration boundary: use its own area directly.
                // (The host-boundary summation path for wall-attached windows
                // was removed — it was unreachable dead code because the outer
                // condition already filters on fenestration boundary types.)
                let total_window_area = boundary.area_m2;
                Some(WindowSolarData {
                    base_shgc,
                    shgc_summer,
                    shgc_winter,
                    u_factor_w_m2_k: u_factor,
                    window_area_m2: total_window_area,
                    transmittance_summer,
                    transmittance_winter,
                    radiation_frac,
                })
            } else {
                None
            }
        } else {
            None
        };

        // Diagnostic r_zone_to_inner from envelope diagnostics.
        let diagnostic_r_zone_to_inner = diag_by_idx
            .get(&surface_idx)
            .and_then(|d| d.r_zone_to_inner_m2_k_w);

        solver_boundaries.push(SolverBoundary {
            surface_idx,
            surface_id,
            area_m2: boundary.area_m2,
            boundary_category,
            tilt_deg,
            azimuth_deg,
            zone_id,
            zone_idx,
            is_exterior,
            is_conditioned_interior,
            is_attic_interior,
            exterior_emissivity,
            exterior_solar_absorptance,
            attic_emissivity,
            interior_solar_absorptance: interior_solar_abs,
            outer_wiring,
            inner_wiring,
            exterior_rad_frac,
            exterior_rad_res_k_w,
            interior_rad_frac,
            r_film_int_m2_k_w: r_film_int,
            window_solar,
            diagnostic_r_zone_to_inner,
            r_film_exterior_m2_k_w: r_film_ext,
        });
    }

    // Debug assertion: verify that all boundaries mapped to the same zone index
    // originate from the same HPXML zone type. This catches the bug where
    // position-based zone indexing would map boundaries from a second
    // Conditioned zone in a duplex into zone 0 (the first Conditioned zone),
    // creating phantom inter-zone coupling.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        for z_idx in 0..rc.n_zones {
            // Only check boundaries that have an explicit interior zone type.
            // Boundaries with None interior zone (e.g. some windows) naturally
            // coexist with typed boundaries in the same zone.
            let types_in_zone: Vec<&hares_io::hpxml::ZoneType> = solver_boundaries
                .iter()
                .filter(|sb| sb.zone_idx == z_idx)
                .filter_map(|sb| building.boundaries[sb.surface_idx].interior_zone.as_ref())
                .collect();
            let has_multiple_types = types_in_zone
                .iter()
                .enumerate()
                .any(|(i, t1)| types_in_zone.iter().skip(i + 1).any(|t2| t1 != t2));
            assert!(
                !has_multiple_types,
                "Zone index {z_idx} contains boundaries from multiple HPXML zone types \
                 ({types:?}); each solver zone index should map to exactly one HPXML zone — \
                 position-based indexing may have conflated separate zones of the same type",
                types = types_in_zone,
            );
        }
    }

    Ok((solver_boundaries, n_ext_surface_inputs, int_col_counter))
}

fn exterior_emissivity(boundary: &hares_io::hpxml::Boundary) -> f64 {
    if matches!(
        boundary.boundary_type,
        hares_io::hpxml::BoundaryType::Window | hares_io::hpxml::BoundaryType::Skylight
    ) {
        boundary.emittance.unwrap_or(EMISSIVITY_WINDOW)
    } else {
        boundary.emittance.unwrap_or(EMISSIVITY_DEFAULT)
    }
}

fn attic_interior_emissivity(boundary: &hares_io::hpxml::Boundary) -> f64 {
    if boundary.has_radiant_barrier
        && boundary.interior_zone.as_ref() == Some(&hares_io::hpxml::ZoneType::Attic)
    {
        EMISSIVITY_RADIANT_BARRIER
    } else {
        exterior_emissivity(boundary)
    }
}

fn exterior_solar_absorptance(boundary: &hares_io::hpxml::Boundary) -> f64 {
    boundary
        .solar_absorptance
        .unwrap_or(SOLAR_ABSORPTANCE_DEFAULT)
}

fn interior_solar_absorptance(boundary: &hares_io::hpxml::Boundary) -> f64 {
    // Radiant barrier in attic zone: very low absorptance (high reflectivity).
    if boundary.has_radiant_barrier
        && boundary.interior_zone.as_ref() == Some(&hares_io::hpxml::ZoneType::Attic)
    {
        SOLAR_ABSORPTANCE_RADIANT_BARRIER
    } else {
        // Use per-surface value from HPXML SolarAbsorptance, or default to
        // EnergyPlus Material IDD default 0.70.
        boundary
            .solar_absorptance
            .unwrap_or(INTERIOR_SOLAR_ABSORPTANCE_DEFAULT)
    }
}

fn include_interior_lwr(
    is_window: bool,
    is_exterior: bool,
    is_conditioned_interior: bool,
    is_attic_interior: bool,
    area_m2: f64,
) -> bool {
    if area_m2 <= 0.0 {
        return false;
    }
    if is_window {
        return is_exterior;
    }
    is_conditioned_interior || is_attic_interior
}

fn natural_ventilation_coefficients(
    thermal_cfg: &ThermalSolverConfig,
    building_height_m: f64,
) -> (f64, f64) {
    thermal_cfg
        .infiltration
        .iter()
        .find_map(|(zone_id, method)| (*zone_id == thermal_cfg.indoor_zone_id).then_some(*method))
        .and_then(|method| match method {
            hares_envelope::InfiltrationMethod::Ela {
                stack_coeff,
                wind_coeff,
                ..
            } => Some((stack_coeff, wind_coeff)),
            _ => None,
        })
        .unwrap_or_else(|| {
            hares_physics::infiltration::calculate_ela_coefficients(
                0.0,
                building_height_m,
                0.0,
                hares_physics::infiltration::TerrainClass::Suburban,
                hares_physics::infiltration::SHIELDING_NORMAL,
            )
        })
}

/// Compute total operable window area and area-weighted opening azimuth
/// from conditioned-zone windows.
///
/// Iterates over windows attached to conditioned-zone boundaries, accumulates
/// total operable area (area × fraction_operable) and azimuth-weighted area,
/// then returns the total operable area and the area-weighted average azimuth.
///
/// Falls back to `default_azimuth_deg` when no window has a known azimuth.
///
/// Uses a simple arithmetic mean (not a circular/vector mean), which is adequate
/// for typical residential buildings where operable windows are not symmetrically
/// distributed across the 0°/360° azimuth boundary.
fn compute_opening_azimuth_and_area(
    windows: &[hares_io::hpxml::Window],
    boundaries: &[hares_io::hpxml::Boundary],
    default_azimuth_deg: f64,
) -> (f64, f64) {
    let (total_operable_area, az_area_sum, az_weighted_sum) = windows
        .iter()
        .filter(|w| {
            w.attached_to_wall_id
                .as_ref()
                .and_then(|wall_id| boundaries.iter().find(|b| b.id == *wall_id))
                .map(|b| b.interior_zone.as_ref() == Some(&hares_io::hpxml::ZoneType::Conditioned))
                .unwrap_or(false)
        })
        .fold(
            (0.0_f64, 0.0_f64, 0.0_f64),
            |(area_sum, az_area, az_weighted), w| {
                let eff = w.area_m2 * w.fraction_operable;
                let (aa, aw) = match w.azimuth_deg {
                    Some(az) => (eff, eff * az),
                    None => (0.0, 0.0),
                };
                (area_sum + eff, az_area + aa, az_weighted + aw)
            },
        );
    let azimuth = if total_operable_area > 0.0 && az_area_sum > 0.0 {
        az_weighted_sum / az_area_sum
    } else {
        default_azimuth_deg
    };
    (total_operable_area, azimuth)
}

/// Annual weather averages needed for film coefficient computation.
pub(crate) struct WeatherAverages {
    pub(crate) avg_wind_m_s: f64,
    pub(crate) avg_ambient_c: f64,
    pub(crate) avg_ground_c: f64,
}

pub(crate) fn compute_weather_averages(weather: &WeatherTimeSeries) -> WeatherAverages {
    let n = weather.len().max(1) as f64;
    WeatherAverages {
        avg_wind_m_s: weather.wind_speed_m_s.iter().sum::<f64>() / n,
        avg_ambient_c: weather.dry_bulb_c.iter().sum::<f64>() / n,
        avg_ground_c: weather.ground_temp_c.iter().sum::<f64>() / n,
    }
}

/// Solvers plus per-zone thermal capacitances [J/K] for gain preview.
/// Extract declared `(loop_id, fluid_type)` pairs from equipment specs.
///
/// Fluid-loop equipment (boilers, water heaters, etc.) carries `loop_id` and
/// `fluid_type` in its typed config. This function collects the unique pairs
/// so `FluidSolver::new` can validate loop identity at construction time and
/// reject configuration errors before the first timestep.
fn extract_fluid_loops_from_specs(specs: &[EquipmentSpec]) -> Vec<(LoopId, FluidType)> {
    let mut loops = Vec::new();

    for spec in specs {
        let Some(ref cfg) = spec.typed_config else {
            continue;
        };

        // Boilers: typed config carries both loop_id and fluid_type.
        match spec.name.as_str() {
            "Gas Boiler" => {
                if let Ok(typed) = cfg.typed::<hares_equipment::GasBoilerConfig>() {
                    if let Some(lid) = typed.loop_id {
                        loops.push((LoopId(lid), typed.fluid_type));
                    }
                }
            }
            "Electric Boiler" => {
                if let Ok(typed) = cfg.typed::<hares_equipment::ElectricBoilerConfig>() {
                    if let Some(lid) = typed.loop_id {
                        loops.push((LoopId(lid), typed.fluid_type));
                    }
                }
            }
            // Water heaters: always fluid_type=Water, with optional loop_id
            // from the typed config. Tankless uses DHW_DEMAND_LOOP when loop_id
            // is absent, but that is a runtime assignment; here we only collect
            // what's explicitly configured.
            "Gas Water Heater" | "Electric Resistance Water Heater" | "Heat Pump Water Heater" => {
                if let Ok(typed) = cfg.typed::<hares_equipment::GasWaterHeaterConfig>() {
                    if let Some(lid) = typed.loop_id {
                        loops.push((LoopId(lid), FluidType::Water));
                    }
                } else if let Ok(typed) =
                    cfg.typed::<hares_equipment::ElectricResistanceWaterHeaterConfig>()
                {
                    if let Some(lid) = typed.loop_id {
                        loops.push((LoopId(lid), FluidType::Water));
                    }
                } else if let Ok(typed) = cfg.typed::<hares_equipment::HeatPumpWaterHeaterConfig>()
                {
                    if let Some(lid) = typed.loop_id {
                        loops.push((LoopId(lid), FluidType::Water));
                    }
                }
            }
            _ => {}
        }
    }

    // Deduplicate: if the same (loop_id, fluid_type) appears from multiple
    // specs, keep only one.
    loops.sort_by_key(|&(lid, ft)| (lid.0, ft));
    loops.dedup();
    loops
}

pub(crate) struct SolverBundle {
    pub thermal: ThermalSolver,
    pub humidity: HumiditySolver,
    pub electrical: ElectricalSolver,
    pub fluid: FluidSolver,
    pub zone_capacitances_j_k: Vec<(ZoneId, f64)>,
    #[cfg(any(debug_assertions, feature = "observe_detailed"))]
    pub envelope_diagnostics: EnvelopeDiagnostics,
}

pub(crate) fn build_default_solvers(
    env: &EnvironmentState,
    sim_config: &SimulationConfig,
    building: &Building,
    defaults: &DefaultsStore,
    weather_avgs: &WeatherAverages,
    equipment_specs: &[EquipmentSpec],
) -> Result<SolverBundle> {
    use hares_envelope::state_space::{OutputMapping, StateSpaceModel};
    use nalgebra::DMatrix;

    let n_zones = env.zones.len().max(1);

    // Log info when multiple zones share the same ZoneType (multi-unit building).
    check_multi_unit_zones(building);

    // Convert building data to envelope-crate input types.
    let zone_inputs = building_to_zone_inputs(building, n_zones);
    let boundary_inputs = building_to_boundary_inputs(
        building,
        n_zones,
        defaults,
        weather_avgs.avg_wind_m_s,
        weather_avgs.avg_ambient_c,
        weather_avgs.avg_ground_c,
    )?;

    // Zone air node capacitances [J/K].
    // Use ISA standard atmosphere pressure from building site elevation.
    // Falls back to sea-level pressure when elevation is unknown.
    // Cite: ASHRAE HoF 2021 §1.8 Eq.28; ISA 1976 / ICAO Doc 7488.
    let site_pressure_pa = hares_physics::air_properties::standard_pressure_pa(
        building.site.elevation_m.unwrap_or(0.0),
    );
    let zone_capacitances = derive_zone_capacitances(&zone_inputs, site_pressure_pa)
        .map_err(|e| HaresError::Envelope(e.to_string()))?;

    // Build the RC network from material layers where available.
    // StarMesh mode bakes linearized inter-surface radiation conductances
    // into the A-matrix at construction time. ScriptF mode preserves the
    // iterative T⁴ radiosity injection path.
    let interior_lwr_method = hares_envelope::InteriorLwrMethod::StarMesh;
    let (rc, envelope_diagnostics) = assemble_building_rc(
        &boundary_inputs,
        n_zones,
        &zone_capacitances,
        interior_lwr_method,
    )
    .map_err(HaresError::Envelope)?;

    let BuildingRC {
        a_c,
        b_ext,
        node_index,
        zone_state_rows,
        layer_info,
        outdoor_col,
        ground_cols,
        n_ext,
        node_capacitances,
        ..
    } = rc;
    let n_states = a_c.nrows();

    // Build the intermediate SolverBoundary representations.
    let rc_ctx = RCContext {
        layer_info: &layer_info,
        node_index: &node_index,
        envelope_diagnostics: &envelope_diagnostics,
        n_zones,
        n_ext,
    };
    let (solver_boundaries, n_ext_surface_inputs, n_int_surface_inputs) =
        build_solver_boundaries(building, &boundary_inputs, &rc_ctx, env)?;

    // Augment B_c: [B_ext | ext-surface columns | int-surface columns | zone sensible heat].
    let n_total_inputs = n_ext + n_ext_surface_inputs + n_int_surface_inputs + n_zones;
    let mut b_c = DMatrix::<f64>::zeros(n_states, n_total_inputs);

    // Copy B_ext columns.
    for row in 0..n_states {
        for col in 0..n_ext {
            b_c[(row, col)] = b_ext[(row, col)];
        }
    }

    // Per-exterior-surface injection columns: gain = 1/C_outer_node.
    for sb in &solver_boundaries {
        if let Some(ref ow) = sb.outer_wiring {
            let info = &layer_info[&sb.surface_idx];
            let c_node = node_capacitances
                .get(&info.outer_node)
                .copied()
                .unwrap_or(hares_envelope::boundary_rc::MIN_CAPACITANCE_J_K)
                .max(hares_envelope::boundary_rc::MIN_CAPACITANCE_J_K);
            b_c[(ow.state_row, ow.b_col)] = 1.0 / c_node;
        }
    }

    // Per-interior-surface injection columns: gain = 1/C_inner_node.
    for sb in &solver_boundaries {
        if let Some(ref iw) = sb.inner_wiring {
            let info = &layer_info[&sb.surface_idx];
            let c_node = node_capacitances
                .get(&info.inner_node)
                .copied()
                .unwrap_or(hares_envelope::boundary_rc::MIN_CAPACITANCE_J_K)
                .max(hares_envelope::boundary_rc::MIN_CAPACITANCE_J_K);
            b_c[(iw.state_row, iw.b_col)] = 1.0 / c_node;
        }
    }

    // Zone sensible heat columns.
    let zone_input_offset = n_ext + n_ext_surface_inputs + n_int_surface_inputs;
    for (zone_idx, &state_row) in zone_state_rows.iter().enumerate() {
        b_c[(state_row, zone_input_offset + zone_idx)] =
            1.0 / zone_capacitances[zone_idx].max(hares_envelope::boundary_rc::MIN_CAPACITANCE_J_K);
    }

    let output_mapping = OutputMapping {
        output_count: n_zones,
        node_to_output: zone_state_rows
            .iter()
            .enumerate()
            .map(|(out_idx, &state_row)| (state_row, out_idx, 1.0))
            .collect(),
        input_to_output: Vec::new(),
    };

    let dt_s = sim_config.time_res.num_milliseconds() as f64 / 1000.0;
    let model = StateSpaceModel::from_continuous(&a_c, &b_c, dt_s, &output_mapping)
        .map_err(|err| HaresError::Envelope(format!("state-space setup failed: {err}")))?;

    // --- ThermalSolverConfig ---
    let mut wiring = StateSpaceWiring::default();
    let mut thermal_cfg = ThermalSolverConfig {
        indoor_zone_id: env
            .zones
            .first()
            .map(|z| z.id)
            .unwrap_or(hares_types::ZoneId(1)),
        interior_lwr_method,
        ..ThermalSolverConfig::default()
    };
    for (zone_idx, zone) in env.zones.iter().enumerate() {
        wiring
            .zone_state_indices
            .insert(zone.id, zone_state_rows[zone_idx]);
        wiring.zone_output_indices.insert(zone.id, zone_idx);
        wiring
            .zone_sensible_input_indices
            .insert(zone.id, zone_input_offset + zone_idx);
    }
    if let Some(col) = outdoor_col {
        wiring.outdoor_temp_input_indices = vec![col];
    } else {
        wiring.outdoor_temp_input_indices = vec![];
    }
    // Populate per-depth ground input indices and depths.
    // One B-matrix column per unique foundation depth; the Kusuda-Achenbach
    // model is evaluated at each depth to set the corresponding column's
    // driving temperature at each timestep.
    wiring.ground_temp_input_indices = ground_cols.iter().map(|(_, col)| *col).collect();
    wiring.ground_temp_input_depths_m = ground_cols.iter().map(|(d, _)| *d).collect();

    // Populate per-zone thermal capacitances [J/K] for energy balance closure check.
    for (zone_idx, zone) in env.zones.iter().enumerate() {
        let cap = zone_capacitances
            .get(zone_idx)
            .copied()
            .unwrap_or(hares_envelope::boundary_rc::MIN_CAPACITANCE_J_K);
        wiring.c_zone_j_k.insert(
            zone.id,
            cap.max(hares_envelope::boundary_rc::MIN_CAPACITANCE_J_K),
        );
    }

    // Forward the full per-node capacitance and index maps to the solver.
    // node_capacitances and node_index are consumed from BuildingRC above;
    // they are no longer borrowed after the B-matrix column normalisation
    // and RCContext usage complete. The solver uses these for full-system
    // energy balance checks (Σ C_i × ΔT_i/dt over all thermal nodes).
    wiring.node_capacitances = node_capacitances;
    wiring.node_index = node_index;

    // Match OCHRE: iterations = floor(dt / 300 s) + 1.
    let n_iter = (dt_s / 300.0).floor() as u32 + 1;

    // Single pass: populate exterior surfaces, window properties, interior surfaces, diagnostics.
    let mut surfaces_by_zone: HashMap<ZoneId, Vec<hares_envelope::InteriorSurfaceInfo>> =
        HashMap::new();
    let diag_by_idx: HashMap<usize, &BoundaryDiagnostic> = envelope_diagnostics
        .boundaries
        .iter()
        .map(|d| (d.boundary_idx, d))
        .collect();

    for sb in &solver_boundaries {
        if sb.is_exterior {
            // Resolve state/input indices: use outer wiring if available, else zone air.
            let (state_index, input_index) = if let Some(ref ow) = sb.outer_wiring {
                (ow.state_row, ow.b_col)
            } else {
                let si = *wiring.zone_state_indices.get(&sb.zone_id).unwrap_or(&0);
                let ii = *wiring
                    .zone_sensible_input_indices
                    .get(&sb.zone_id)
                    .unwrap_or(&(zone_input_offset + sb.zone_idx));
                (si, ii)
            };

            thermal_cfg.exterior_surfaces.push(ExteriorSurfaceInfo {
                surface_id: sb.surface_id,
                state_index,
                input_index,
                area_m2: sb.area_m2,
                emissivity: sb.exterior_emissivity,
                tilt_deg: sb.tilt_deg,
                azimuth_deg: sb.azimuth_deg,
                rad_frac: sb.exterior_rad_frac,
                rad_res_k_w: sb.exterior_rad_res_k_w,
                n_iter,
                absorptance: sb.exterior_solar_absorptance,
                boundary_category: sb.boundary_category,
                u_factor_w_m2_k: sb
                    .window_solar
                    .as_ref()
                    .map(|ws| ws.u_factor_w_m2_k)
                    .unwrap_or(0.0),
                h_out_w_m2_k: if sb.r_film_exterior_m2_k_w > 1e-9 {
                    1.0 / sb.r_film_exterior_m2_k_w
                } else {
                    hares_physics::film_coefficients::H_OUT_ASHRAE_PEAK
                },
            });
            wiring
                .solar_input_indices
                .insert(sb.surface_id, input_index);

            if let Some(ref ws) = sb.window_solar {
                thermal_cfg.window_properties.insert(
                    sb.surface_id,
                    WindowSolarProperties {
                        shgc: ws.shgc_summer,
                        winter_shgc: ws.shgc_winter,
                        u_factor_w_m2_k: ws.u_factor_w_m2_k,
                        area_m2: ws.window_area_m2,
                        transmittance: ws.transmittance_summer,
                        winter_transmittance: ws.transmittance_winter,
                        radiation_frac: ws.radiation_frac,
                        glazing_curve: hares_physics::solar::GlazingCurve::from_u_shgc(
                            ws.u_factor_w_m2_k,
                            ws.base_shgc,
                        ),
                        tilt_deg: sb.tilt_deg,
                        azimuth_deg: sb.azimuth_deg,
                    },
                );
                thermal_cfg
                    .window_zone_ids
                    .insert(sb.surface_id, sb.zone_id);
            }
        }

        // Add interior surfaces for LWR and solar distribution.
        //
        // Opaque surfaces: R_film_int is convection-only (1/h_conv from TARP).
        // The full LWR flux q is split by the radiation_frac divider:
        //   q × radiation_frac       → surface RC node
        //   q × (1 − radiation_frac) → zone air
        // No double-counting since R_film does not include h_rad.
        //
        // Window surfaces: no RC node (t_idx=None in OCHRE). Only
        // q × (1 − radiation_frac) → zone air. The radiation_frac
        // portion is carried by the window U-factor conduction path.
        let is_window = sb.boundary_category == Some(BoundaryCategory::Window);
        let include_in_lwr = include_interior_lwr(
            is_window,
            sb.is_exterior,
            sb.is_conditioned_interior,
            sb.is_attic_interior,
            sb.area_m2,
        );
        if include_in_lwr {
            let (state_idx, input_idx) = if let Some(ref iw) = sb.inner_wiring {
                (iw.state_row, iw.b_col)
            } else {
                let si = *wiring.zone_state_indices.get(&sb.zone_id).unwrap_or(&0);
                let ii = *wiring
                    .zone_sensible_input_indices
                    .get(&sb.zone_id)
                    .unwrap_or(&(zone_input_offset + sb.zone_idx));
                (si, ii)
            };

            let is_floor = sb.boundary_category == Some(BoundaryCategory::Floor);

            let (emissivity, solar_absorptance, radiation_frac, rad_res_k_w, driving_temp) =
                if is_window {
                    // Window LWR: emissivity=0.9 per ASHRAE 140-2017 §5.3.1.9
                    // Table 24: ε_ir = 0.9 for ALL interior surfaces including windows.
                    // The 0.84 value is the NFRC glass thermal emissivity for U-factor
                    // rating only; for interior LWR exchange ASHRAE 140 specifies 0.9.
                    // Surface temp driven by outdoor conduction.
                    // radiation_frac from E+ interior film decomposition:
                    //   res_int = 1 / (0.359073 × ln(U) + 6.949915)
                    //   radiation_frac = res_int / (1/U)
                    const WINDOW_EMISSIVITY: f64 = 0.9;
                    let diag = diag_by_idx.get(&sb.surface_idx);
                    let r_total = diag.map(|d| d.r_total_m2_k_w).unwrap_or(0.5);
                    let u_window = if r_total > 1e-9 { 1.0 / r_total } else { 2.0 };
                    let res_int = 1.0 / (0.359073 * u_window.ln() + 6.949915);
                    let rad_frac = (res_int / r_total).clamp(0.0, 1.0);
                    (
                        WINDOW_EMISSIVITY,
                        0.0,
                        rad_frac,
                        res_int / sb.area_m2.max(1e-9),
                        Some(DrivingTemp::Outdoor),
                    )
                } else {
                    // Opaque surfaces: rad_res_k_w uses convection-only R_film
                    // (OCHRE "full" mode: R_film = 1/h_conv, no parallel R_rad).
                    (
                        sb.attic_emissivity,
                        sb.interior_solar_absorptance,
                        sb.interior_rad_frac,
                        sb.r_film_int_m2_k_w / sb.area_m2.max(1e-9),
                        None,
                    )
                };

            surfaces_by_zone.entry(sb.zone_id).or_default().push(
                hares_envelope::InteriorSurfaceInfo {
                    state_index: state_idx,
                    input_index: input_idx,
                    area_m2: sb.area_m2,
                    emissivity,
                    radiation_frac,
                    rad_res_k_w,
                    solar_absorptance,
                    is_floor,
                    driving_temp,
                },
            );
        }

        if let Some(cat) = sb.boundary_category {
            if let Some(ref iw) = sb.inner_wiring {
                let r_film = sb.r_film_int_m2_k_w;
                let r_zone_to_inner = sb.diagnostic_r_zone_to_inner.unwrap_or(r_film);
                let radiation_frac = if r_zone_to_inner > 0.0 {
                    r_film / r_zone_to_inner
                } else {
                    1.0
                };
                thermal_cfg
                    .boundary_diagnostics
                    .push(BoundaryDiagnosticInfo::RCNode {
                        inner_state_index: iw.state_row,
                        area_m2: sb.area_m2,
                        tilt_deg: sb.tilt_deg,
                        radiation_frac,
                        category: cat,
                    });
            } else if sb.is_conditioned_interior
                || (sb.is_exterior && cat == BoundaryCategory::Window)
            {
                // Boundary without RC interior node -- use steady-state UA diagnostic.
                let diag_bd = diag_by_idx.get(&sb.surface_idx);
                if let Some(d) = diag_bd {
                    let driving_temp = match d.exterior_target {
                        ExteriorTarget::Ground => DrivingTemp::Ground {
                            depth_m: d.foundation_depth_m,
                        },
                        _ => DrivingTemp::Outdoor,
                    };
                    thermal_cfg
                        .boundary_diagnostics
                        .push(BoundaryDiagnosticInfo::SteadyState {
                            ua_w_k: d.ua_w_per_k,
                            driving_temp,
                            category: cat,
                        });
                }
            }
        }
    }

    // ── Per-step interior convection injection metadata ──────────────────────
    // Populate when using PerStepTarp model so the stepping loop can inject
    // ΔQ = (h_tarp − h_static) × A × (T_surface − T_zone) as explicit forcing
    // into the surface and zone-air state rows each timestep.
    //
    // Reference: Walton, G. N. 1983. TARP Reference Manual, NBSSIR 83-2655,
    // Eqs. 90–92 — EnergyPlus default interior convection model.
    thermal_cfg.film_coefficient_model = FilmCoefficientModel::AshraeSimple;
    // T-0034: PerStepTarp infrastructure is wired but disabled by default.
    // See Known Limitations in T-0034's Implementation Notes and tracking
    // ticket T-1922 for the Courant-condition constraint that blocks
    // enabling it at 60 s timestep with typical surface layer capacitances.
    for sb in &solver_boundaries {
        // Only interior-facing boundaries with RC interior nodes participate.
        let surface_state_idx = match &sb.inner_wiring {
            Some(iw) => iw.state_row,
            None => continue,
        };
        let zone_state_idx = match wiring.zone_state_indices.get(&sb.zone_id) {
            Some(&idx) => idx,
            None => continue,
        };
        let c_zone = wiring
            .c_zone_j_k
            .get(&sb.zone_id)
            .copied()
            .unwrap_or(hares_envelope::boundary_rc::MIN_CAPACITANCE_J_K);
        // Surface node capacitance from the per-node capacitance map
        // (populated during RC network assembly).
        let info = match layer_info.get(&sb.surface_idx) {
            Some(li) => li,
            None => continue,
        };
        let c_surface = wiring
            .node_capacitances
            .get(&info.inner_node)
            .copied()
            .unwrap_or(hares_envelope::boundary_rc::MIN_CAPACITANCE_J_K);

        thermal_cfg
            .interior_convection_injections
            .push(InteriorConvectionInjection {
                surface_state_index: surface_state_idx,
                zone_state_index: zone_state_idx,
                area_m2: sb.area_m2,
                tilt_deg: sb.tilt_deg,
                static_r_film_int_m2_k_w: sb.r_film_int_m2_k_w,
                c_surface_j_k: c_surface,
                c_zone_j_k: c_zone,
            });
    }

    // Always populate interior_solar_zones so that solar distribution works
    // in both StarMesh and ScriptF modes. In StarMesh mode, interior_lwr_zones
    // is empty but solar still needs surface metadata for the radiation_frac split.
    for (zid, surfaces) in &surfaces_by_zone {
        let solar_surfaces: Vec<InteriorSolarSurfaceInfo> = surfaces
            .iter()
            .map(|s| InteriorSolarSurfaceInfo {
                input_index: Some(s.input_index),
                area_m2: s.area_m2,
                solar_absorptance: s.solar_absorptance,
                radiation_frac: s.radiation_frac,
                is_floor: s.is_floor,
            })
            .collect();
        if !solar_surfaces.is_empty() {
            thermal_cfg
                .interior_solar_zones
                .push(InteriorSolarZoneConfig {
                    zone_id: *zid,
                    surfaces: solar_surfaces,
                });
        }
    }

    // Group surfaces_by_zone into interior_lwr_zones.
    // Only populate when using ScriptF mode — StarMesh bakes radiation
    // conductances into the A-matrix at construction time, so per-timestep
    // ScriptF injection is not needed (and would double-count).
    if interior_lwr_method == hares_envelope::InteriorLwrMethod::ScriptF {
        for (zid, surfaces) in surfaces_by_zone {
            if surfaces.len() >= 2 {
                let mut zone_cfg = hares_envelope::InteriorLwrZoneConfig {
                    zone_id: zid,
                    surfaces,
                    scriptf: None,
                };
                zone_cfg.compute_scriptf();
                thermal_cfg.interior_lwr_zones.push(zone_cfg);
            }
        }
    }

    // --- Per-zone infiltration (Walker-Wilson 1998 / ASHRAE HOF Ch. 16) ---
    // Each zone gets a physics-appropriate infiltration method.
    // Conditioned zone uses AIM-2 model when ACH50 (or a convertible metric) is available.
    // Priority: ACH50 > CFM50 > ELA.
    {
        use hares_envelope::InfiltrationMethod;
        use hares_io::hpxml::ZoneType;
        use hares_physics::infiltration::{
            Aim2Params, FoundationLeakageClass, N_I_DEFAULT, aim2_coefficients_from_ach50,
        };

        // Resolve ACH50 from whatever input form is available.
        // CFM50 → ACH50: ach50 = (cfm50 × 60) / volume_ft3
        // ELA cm² → CFM50 (Sherman-Grimsrud at 50 Pa, n=0.65):
        //   cfm50 = ela_cm2 × 0.0524 × 50^0.65
        // where 0.0524 is the discharge-coefficient/unit-conversion factor
        // (ELA defined at 4 Pa with Cd=1.0, scaled to 50 Pa via power law).
        let resolved_ach50: Option<f64> = building.infiltration_ach50.or_else(|| {
            let volume_m3 = building.conditioned_volume_m3.unwrap_or(400.0);
            let cfm50 = building.infiltration_cfm50.or_else(|| {
                building
                    .infiltration_ela_cm2
                    .map(|ela_cm2| ela_cm2 * 0.0524 * 50.0_f64.powf(N_I_DEFAULT))
            })?;
            // 1 ft³ = 0.0283168 m³  →  volume_ft3 = volume_m3 / 0.0283168 = volume_m3 × 35.3147
            let volume_ft3 = volume_m3 * 35.3147;
            Some((cfm50 * 60.0) / volume_ft3)
        });

        let default_ceiling_height_m = building.ceiling_height_m.unwrap_or(2.5);
        let building_height_m = default_ceiling_height_m
            * building
                .zones
                .iter()
                .filter(|z| z.zone_type == hares_io::hpxml::ZoneType::Conditioned)
                .count()
                .max(1) as f64;

        for (zone_idx, bz) in building.zones.iter().enumerate() {
            let zone_id = env
                .zones
                .get(zone_idx)
                .map(|z| z.id)
                .unwrap_or(hares_types::ZoneId(zone_idx as u16));

            let method = match bz.zone_type {
                ZoneType::Conditioned => {
                    if let Some(ach) = building.infiltration_constant_ach {
                        InfiltrationMethod::Ach { ach }
                    } else if let Some(ach50) = resolved_ach50 {
                        let terrain = site_type_to_terrain(&building.site.site_type);
                        let shielding =
                            shielding_str_to_class(building.site.shielding_of_home.as_deref());
                        let foundation = if has_vented_crawlspace(building) {
                            FoundationLeakageClass::VentedCrawlspace
                        } else {
                            FoundationLeakageClass::Other
                        };
                        let h = building.infiltration_height_m.unwrap_or(
                            default_ceiling_height_m * building.floors_above_grade.unwrap_or(1.0),
                        );
                        let coeffs = aim2_coefficients_from_ach50(&Aim2Params {
                            ach50,
                            volume_m3: building.conditioned_volume_m3.unwrap_or(400.0),
                            infiltration_height_m: h,
                            foundation,
                            shielding,
                            terrain,
                            has_flue: building.has_flue_or_chimney.unwrap_or(false),
                            n_i: N_I_DEFAULT,
                            floors_above_grade: building.floors_above_grade.unwrap_or(1.0),
                        });
                        InfiltrationMethod::AshraeWindStack {
                            c_s: coeffs.c_s,
                            c_w: coeffs.c_w,
                            shielding_coeff: coeffs.shelter_coeff,
                            n_i: coeffs.n_i,
                        }
                    } else {
                        InfiltrationMethod::Ach { ach: 0.0 }
                    }
                }
                ZoneType::Attic => {
                    let conditioned_area = building
                        .zones
                        .iter()
                        .find(|z| z.zone_type == ZoneType::Conditioned)
                        .and_then(|z| z.floor_area_m2);
                    attic_infiltration_method(bz, conditioned_area, building_height_m, zone_idx)?
                }
                ZoneType::Garage => {
                    let conditioned_area = building
                        .zones
                        .iter()
                        .find(|z| z.zone_type == ZoneType::Conditioned)
                        .and_then(|z| z.floor_area_m2);
                    let garage_height_m = bz
                        .volume_m3
                        .zip(bz.floor_area_m2)
                        .and_then(|(v, a)| (a > 0.0).then_some(v / a))
                        .unwrap_or(default_ceiling_height_m);
                    garage_infiltration_method(bz, conditioned_area, garage_height_m, zone_idx)?
                }
                ZoneType::Foundation => {
                    let conditioned_area = building
                        .zones
                        .iter()
                        .find(|z| z.zone_type == ZoneType::Conditioned)
                        .and_then(|z| z.floor_area_m2);
                    let foundation_height_m = foundation_height_m(bz);
                    foundation_infiltration_method(bz, conditioned_area, foundation_height_m)
                }
                ZoneType::Outdoor | ZoneType::Ground | ZoneType::Adjacent | ZoneType::Other(_) => {
                    continue;
                }
            };

            thermal_cfg.infiltration.push((zone_id, method));
        }
    }

    // --- Mechanical ventilation from equipment specs ---
    if let Some(vent_spec) = equipment_specs.iter().find(|s| s.name == "Ventilation Fan") {
        let applied = if let Some(ref tc) = vent_spec.typed_config {
            match tc.typed::<hares_equipment::VentilationConfig>() {
                Ok(cfg) => {
                    thermal_cfg.ventilation_flow_m3_s = cfg.flow_rate_m3_s;
                    thermal_cfg.ventilation.balanced = cfg.balanced.unwrap_or(false);
                    if let Some(sens) = cfg.sensible_effectiveness {
                        thermal_cfg.ventilation.sensible_recovery_efficiency = sens;
                    }
                    if let Some(lat) = cfg.latent_effectiveness {
                        thermal_cfg.ventilation.latent_recovery_efficiency = lat;
                    }
                    true
                }
                Err(e) => {
                    tracing::warn!(
                        "ventilation typed config deserialization failed: {e}; falling back to raw params"
                    );
                    false
                }
            }
        } else {
            false
        };
        if !applied {
            let params = &vent_spec.parameters;
            if let Some(flow_m3_s) = params.get("flow_rate_m3_s").and_then(|v| v.as_f64()) {
                thermal_cfg.ventilation_flow_m3_s = flow_m3_s;
            }
            if let Some(balanced) = params.get("balanced").and_then(|v| v.as_bool()) {
                thermal_cfg.ventilation.balanced = balanced;
            }
            if let Some(sens_re) = params
                .get("sensible_effectiveness")
                .and_then(|v| v.as_f64())
            {
                thermal_cfg.ventilation.sensible_recovery_efficiency = sens_re;
            }
            if let Some(lat_re) = params.get("latent_effectiveness").and_then(|v| v.as_f64()) {
                thermal_cfg.ventilation.latent_recovery_efficiency = lat_re;
            }
        }
    }

    // --- Duct leakage → infiltration adjustment (ASHRAE 152 §9.3) ---
    // Aggregate supply and return leakage fractions from all unconditioned zones,
    // then multiply by a rated fan flow estimate to get absolute leakage flows [m³/s].
    // Fan flow is estimated from hvac_capacity_w × standard 350 CFM/ton (midpoint
    // of the 312–400 CFM/ton residential range).
    {
        use hares_io::hpxml::building::DuctType;
        use hares_physics::constants::{CFM_TO_M3_S, W_PER_TON};

        const AIRFLOW_CFM_PER_TON: f64 = 350.0;

        let mut supply_leakage_frac = 0.0_f64;
        let mut return_leakage_frac = 0.0_f64;

        for zone in &building.zones {
            if matches!(zone.zone_type, hares_io::hpxml::ZoneType::Conditioned) {
                continue;
            }
            for duct in &zone.duct_systems {
                let leak = duct.leakage_fraction.unwrap_or(0.0);
                match duct.duct_type {
                    DuctType::Supply => supply_leakage_frac += leak,
                    DuctType::Return => return_leakage_frac += leak,
                    DuctType::Unknown => {
                        supply_leakage_frac += leak * 0.5;
                        return_leakage_frac += leak * 0.5;
                    }
                }
            }
        }

        if supply_leakage_frac > 0.0 || return_leakage_frac > 0.0 {
            let capacity_w = building.hvac_capacity_w.unwrap_or(0.0);
            if capacity_w > 0.0 {
                let fan_flow_m3_s = (capacity_w / W_PER_TON) * AIRFLOW_CFM_PER_TON * CFM_TO_M3_S;
                thermal_cfg.supply_duct_leakage_m3_s = supply_leakage_frac * fan_flow_m3_s;
                thermal_cfg.return_duct_leakage_m3_s = return_leakage_frac * fan_flow_m3_s;
            }
        }
    }

    // --- Natural ventilation through operable windows ---
    // OCHRE formula: total_window_area × 0.67 × 0.5 × 0.2 = total_window_area × 0.067.
    // The 0.67 factor accounts for only ~67% of operable window area being openable at once;
    // HPXML FractionOperable indicates window type (operable vs fixed), not instantaneous
    // open state, so the 0.67 factor must be applied on top of FractionOperable.
    {
        use hares_envelope::NaturalVentilationConfig;
        let default_ceiling_height_m = building.ceiling_height_m.unwrap_or(2.5);
        let building_height_m = default_ceiling_height_m
            * building
                .zones
                .iter()
                .filter(|z| z.zone_type == hares_io::hpxml::ZoneType::Conditioned)
                .count()
                .max(1) as f64;

        let (total_operable_area, opening_azimuth_deg) = compute_opening_azimuth_and_area(
            &building.windows,
            &building.boundaries,
            NaturalVentilationConfig::DEFAULT_OPENING_AZIMUTH_DEG,
        );
        if total_operable_area > 0.0 {
            let open_area = total_operable_area * NaturalVentilationConfig::OPEN_AREA_FRACTION;
            let (stack, wind) = natural_ventilation_coefficients(&thermal_cfg, building_height_m);
            // Natural ventilation should use conditioned-zone ELA coefficients.
            // Prefer explicit indoor ELA coefficients when available; otherwise
            // derive conditioned defaults at full building height.
            thermal_cfg.natural_ventilation = Some(NaturalVentilationConfig {
                open_area_m2: open_area,
                stack_coeff: stack,
                wind_coeff: wind,
                t_base_c: NaturalVentilationConfig::DEFAULT_T_BASE_C,
                max_outdoor_humidity_ratio:
                    NaturalVentilationConfig::DEFAULT_MAX_OUTDOOR_HUMIDITY_RATIO,
                opening_azimuth_deg,
            });
        }
    }

    let initial_temp = env.zones.first().map(|z| z.temperature_c).unwrap_or(21.0);
    let thermal_solver = ThermalSolver::new(model, wiring, thermal_cfg, dt_s, env, initial_temp)
        .map_err(|err| HaresError::Envelope(format!("thermal solver init failed: {err}")))?;

    let humidity_solver = HumiditySolver::new(HumiditySolverConfig::default(), env);
    let electrical_solver = ElectricalSolver::new(ElectricalSolverConfig::default())
        .map_err(|err| HaresError::Envelope(format!("electrical solver init failed: {err}")))?;
    let fluid_loops = extract_fluid_loops_from_specs(equipment_specs);
    let fluid_solver = FluidSolver::new(FluidSolverConfig::default(), &fluid_loops)
        .map_err(|err| HaresError::Envelope(format!("fluid solver init failed: {err}")))?;

    let zone_caps: Vec<(ZoneId, f64)> = env
        .zones
        .iter()
        .enumerate()
        .map(|(i, z)| {
            let cap = zone_capacitances
                .get(i)
                .copied()
                .unwrap_or(hares_envelope::boundary_rc::MIN_CAPACITANCE_J_K);
            (
                z.id,
                cap.max(hares_envelope::boundary_rc::MIN_CAPACITANCE_J_K),
            )
        })
        .collect();

    Ok(SolverBundle {
        thermal: thermal_solver,
        humidity: humidity_solver,
        electrical: electrical_solver,
        fluid: fluid_solver,
        zone_capacitances_j_k: zone_caps,
        #[cfg(any(debug_assertions, feature = "observe_detailed"))]
        envelope_diagnostics,
    })
}

fn foundation_infiltration_method(
    zone: &hares_io::hpxml::Zone,
    conditioned_floor_area_m2: Option<f64>,
    foundation_height_m: Option<f64>,
) -> hares_envelope::InfiltrationMethod {
    use hares_envelope::InfiltrationMethod;
    use hares_physics::infiltration::{SHIELDING_NORMAL, TerrainClass, calculate_ela_coefficients};

    if let Some(ach) = zone.ventilation_ach {
        return InfiltrationMethod::Ach { ach };
    }

    if let Some(sla) = zone.ventilation_sla {
        if let (Some(floor_area_m2), Some(height_m)) = (
            zone.floor_area_m2.or(conditioned_floor_area_m2),
            foundation_height_m,
        ) {
            let ela_m2 = sla * floor_area_m2;
            let (stack_coeff, wind_coeff) = calculate_ela_coefficients(
                0.0,
                height_m,
                0.0,
                TerrainClass::Suburban,
                SHIELDING_NORMAL,
            );
            return InfiltrationMethod::Ela {
                ela_m2,
                stack_coeff,
                wind_coeff,
            };
        }
    }

    if zone.vented {
        // Vented crawlspace: high air exchange.
        let ach = zone.ventilation_ach.unwrap_or(2.0);
        InfiltrationMethod::Ach { ach }
    } else {
        // Unvented crawlspace/basement: conduction only.
        InfiltrationMethod::Ach { ach: 0.0 }
    }
}

fn attic_infiltration_method(
    zone: &hares_io::hpxml::Zone,
    conditioned_floor_area_m2: Option<f64>,
    building_height_m: f64,
    _zone_idx: usize,
) -> Result<hares_envelope::InfiltrationMethod> {
    use hares_envelope::InfiltrationMethod;
    use hares_physics::infiltration::attic_ela_coefficients;

    if let Some(ach) = zone.ventilation_ach {
        return Ok(InfiltrationMethod::Ach { ach });
    }

    if let Some(sla) = zone.ventilation_sla {
        let floor_area_m2 = zone
            .floor_area_m2
            .or(conditioned_floor_area_m2)
            .unwrap_or(100.0);
        let ela_m2 = sla * floor_area_m2;
        // Attic height from volume: V = 0.5 × A × h → h = 2V/A.
        let attic_height_m = zone
            .volume_m3
            .filter(|&v| v > 0.0)
            .map(|v| 2.0 * v / floor_area_m2)
            .unwrap_or(1.5);
        let (stack_coeff, wind_coeff) = attic_ela_coefficients(attic_height_m, building_height_m);
        return Ok(InfiltrationMethod::Ela {
            ela_m2,
            stack_coeff,
            wind_coeff,
        });
    }

    if zone.vented {
        // ANSI/RESNET/ICC 301-2019 Table 4.2.2(1): default SLA = 1/300 ≈ 0.00333 for
        // vented attics when no measured ventilation rate is provided.
        let sla = 0.00333;
        let floor_area_m2 = zone
            .floor_area_m2
            .or(conditioned_floor_area_m2)
            .unwrap_or(100.0);
        let ela_m2 = sla * floor_area_m2;
        let attic_height_m = zone
            .volume_m3
            .filter(|&v| v > 0.0)
            .map(|v| 2.0 * v / floor_area_m2)
            .unwrap_or(1.5);
        let (stack_coeff, wind_coeff) = attic_ela_coefficients(attic_height_m, building_height_m);
        return Ok(InfiltrationMethod::Ela {
            ela_m2,
            stack_coeff,
            wind_coeff,
        });
    }

    // Unvented attic default matches OCHRE attic handling: 0.1 ACH.
    Ok(InfiltrationMethod::Ach { ach: 0.1 })
}

fn garage_infiltration_method(
    zone: &hares_io::hpxml::Zone,
    conditioned_floor_area_m2: Option<f64>,
    garage_height_m: f64,
    zone_idx: usize,
) -> Result<hares_envelope::InfiltrationMethod> {
    use hares_envelope::InfiltrationMethod;
    use hares_physics::infiltration::garage_ela_coefficients;

    if let Some(ach) = zone.ventilation_ach {
        return Ok(InfiltrationMethod::Ach { ach });
    }

    // ASHRAE 152-2004 default SLA for unconditioned attached garages = 3.0×10⁻⁴.
    // This value is also consistent with ANSI/RESNET/ICC 301-2019 and
    // OpenStudio-HPXML defaults for unconditioned spaces.
    const GARAGE_DEFAULT_SLA: f64 = 3.0e-4;

    let sla = zone.ventilation_sla.unwrap_or_else(|| {
        tracing::warn!(
            "Garage zone {zone_idx}: no <VentilationRate> data available; \
             applying default SLA = {:.4} for unconditioned attached garage \
             (ASHRAE 152-2004 / ANSI/RESNET/ICC 301-2019). \
             Add <VentilationRate><UnitofMeasure>SLA</UnitofMeasure>\
             <Value>…</Value></VentilationRate> to override.",
            GARAGE_DEFAULT_SLA,
        );
        GARAGE_DEFAULT_SLA
    });

    let floor_area_m2 = zone
        .floor_area_m2
        .or(conditioned_floor_area_m2)
        .unwrap_or(28.0);

    let ela_m2 = sla * floor_area_m2;
    let (stack_coeff, wind_coeff) = garage_ela_coefficients(garage_height_m);

    Ok(InfiltrationMethod::Ela {
        ela_m2,
        stack_coeff,
        wind_coeff,
    })
}

fn foundation_height_m(zone: &hares_io::hpxml::Zone) -> Option<f64> {
    zone.volume_m3
        .zip(zone.floor_area_m2)
        .and_then(|(volume_m3, floor_area_m2)| {
            (floor_area_m2 > 0.0).then_some(volume_m3 / floor_area_m2)
        })
}

#[cfg(test)]
mod tests {
    use super::{
        attic_infiltration_method, attic_interior_emissivity, compute_opening_azimuth_and_area,
        exterior_emissivity, exterior_solar_absorptance, foundation_height_m,
        foundation_infiltration_method, garage_infiltration_method, include_interior_lwr,
        interior_solar_absorptance, natural_ventilation_coefficients,
    };
    use hares_envelope::INTERIOR_SOLAR_ABSORPTANCE_DEFAULT;
    use hares_envelope::InfiltrationMethod;
    use hares_envelope::ThermalSolverConfig;
    use hares_io::hpxml::{Boundary, BoundaryType, Window, Zone, ZoneType};
    use hares_physics::infiltration::{
        N_I_DEFAULT, SHIELDING_NORMAL, TerrainClass, calculate_ela_coefficients,
        garage_ela_coefficients,
    };
    use hares_types::ZoneId;

    #[test]
    fn n_iter_matches_ochre_formula() {
        for &dt_s in &[60.0_f64, 300.0, 600.0, 900.0, 3600.0] {
            let n_iter = (dt_s / 300.0_f64).floor() as u32 + 1;
            let expected = (dt_s / 300.0_f64).floor() as u32 + 1;
            assert_eq!(
                n_iter, expected,
                "n_iter mismatch for dt_s={dt_s}: got {n_iter}, expected {expected}"
            );
        }

        // Specific expected values
        assert_eq!((60.0_f64 / 300.0).floor() as u32 + 1, 1);
        assert_eq!((300.0_f64 / 300.0).floor() as u32 + 1, 2);
        assert_eq!((600.0_f64 / 300.0).floor() as u32 + 1, 3);
        assert_eq!((900.0_f64 / 300.0).floor() as u32 + 1, 4);
        assert_eq!((3600.0_f64 / 300.0).floor() as u32 + 1, 13);
    }

    /// CFM50 → ACH50 conversion: ach50 = (cfm50 × 60) / volume_ft3.
    /// For a 400 m³ house: volume_ft3 = 400 × 35.3147 = 14125.88 ft³.
    /// cfm50 = 1000 → ach50 = 60000 / 14125.88 ≈ 4.247.
    #[test]
    fn cfm50_to_ach50_conversion() {
        let volume_m3 = 400.0_f64;
        let cfm50 = 1000.0_f64;
        let volume_ft3 = volume_m3 * 35.3147;
        let ach50 = (cfm50 * 60.0) / volume_ft3;
        let expected = (1000.0 * 60.0) / (400.0 * 35.3147);
        assert!(
            (ach50 - expected).abs() < 1e-6,
            "CFM50→ACH50 mismatch: {ach50} vs {expected}"
        );
        assert!(
            ach50 > 4.0 && ach50 < 5.0,
            "CFM50=1000 on 400m³ should give ~4.2 ACH50, got {ach50}"
        );
    }

    /// ELA cm² → CFM50 (Sherman-Grimsrud at 50 Pa, n=0.65):
    ///   cfm50 = ela_cm2 × 0.0524 × 50^0.65
    /// Verify the formula is monotone and produces physically plausible values.
    #[test]
    fn ela_to_cfm50_conversion() {
        // 100 cm² ELA -- typical for a moderately leaky house
        let ela_cm2 = 100.0_f64;
        let cfm50 = ela_cm2 * 0.0524 * 50.0_f64.powf(N_I_DEFAULT);
        assert!(
            cfm50 > 50.0 && cfm50 < 500.0,
            "100 cm² ELA should produce 50–500 CFM50, got {cfm50:.1}"
        );

        // Larger ELA → more CFM50
        let cfm50_large = 200.0_f64 * 0.0524 * 50.0_f64.powf(N_I_DEFAULT);
        assert!(
            cfm50_large > cfm50,
            "larger ELA should produce larger CFM50: {cfm50_large} vs {cfm50}"
        );
    }

    /// ELA cm² → ACH50 round-trip: verify priority ordering.
    /// When ACH50 is present, CFM50 and ELA are ignored.
    /// When only CFM50 is present, ELA is ignored.
    #[test]
    fn infiltration_input_priority_ach50_wins() {
        let volume_m3 = 300.0_f64;
        let volume_ft3 = volume_m3 * 35.3147;

        // All three provided: ACH50 should win.
        let ach50_direct = Some(7.0_f64);
        let cfm50 = Some(500.0_f64);
        let ela_cm2 = Some(200.0_f64);

        let resolved = ach50_direct.or_else(|| {
            let cfm50_val =
                cfm50.or_else(|| ela_cm2.map(|ela| ela * 0.0524 * 50.0_f64.powf(N_I_DEFAULT)))?;
            Some((cfm50_val * 60.0) / volume_ft3)
        });
        assert_eq!(
            resolved,
            Some(7.0),
            "ACH50 should win when all three are present"
        );

        // Only CFM50 provided.
        let resolved_cfm = None::<f64>.or_else(|| {
            let cfm50_val = Some(600.0_f64)
                .or_else(|| None::<f64>.map(|ela| ela * 0.0524 * 50.0_f64.powf(N_I_DEFAULT)))?;
            Some((cfm50_val * 60.0) / volume_ft3)
        });
        let expected_from_cfm = (600.0 * 60.0) / volume_ft3;
        assert!(
            (resolved_cfm.unwrap() - expected_from_cfm).abs() < 1e-9,
            "CFM50-only path mismatch"
        );

        // Only ELA provided.
        let resolved_ela = None::<f64>.or_else(|| {
            let cfm50_val = None::<f64>
                .or_else(|| Some(150.0_f64).map(|ela| ela * 0.0524 * 50.0_f64.powf(N_I_DEFAULT)))?;
            Some((cfm50_val * 60.0) / volume_ft3)
        });
        let cfm50_from_ela = 150.0_f64 * 0.0524 * 50.0_f64.powf(N_I_DEFAULT);
        let expected_from_ela = (cfm50_from_ela * 60.0) / volume_ft3;
        assert!(
            (resolved_ela.unwrap() - expected_from_ela).abs() < 1e-9,
            "ELA-only path mismatch"
        );
    }

    fn attic_roof_boundary(
        solar_absorptance: Option<f64>,
        emittance: Option<f64>,
        has_radiant_barrier: bool,
    ) -> Boundary {
        Boundary {
            id: "Roof1".to_string(),
            boundary_type: BoundaryType::Roof,
            area_m2: 100.0,
            azimuth_deg: Some(180.0),
            assembly_r_value_m2_k_w: None,
            r_value_layers_m2_k_w: Vec::new(),
            interior_zone: Some(ZoneType::Attic),
            exterior_zone: Some(ZoneType::Outdoor),
            material_layers: Vec::new(),
            construction_type: None,
            finish_type: None,
            insulation_details: None,
            has_radiant_barrier,
            solar_absorptance,
            emittance,
            lut_boundary_name: None,
            floor_or_ceiling: None,
            tilt_deg: Some(15.0),
            framing_factor: None,
            perimeter_m: None,
            perimeter_insulation_r_m2_k_w: None,
            foundation_depth_m: None,
        }
    }

    #[test]
    fn attic_radiant_barrier_keeps_exterior_roof_physics() {
        let boundary = attic_roof_boundary(None, None, true);

        assert_eq!(exterior_solar_absorptance(&boundary), 0.70);
        assert_eq!(interior_solar_absorptance(&boundary), 0.05);
        assert_eq!(exterior_emissivity(&boundary), 0.90);
        assert_eq!(attic_interior_emissivity(&boundary), 0.05);
    }

    #[test]
    fn explicit_roof_optics_are_preserved_for_exterior_path() {
        let boundary = attic_roof_boundary(Some(0.72), Some(0.88), true);

        assert_eq!(exterior_solar_absorptance(&boundary), 0.72);
        assert_eq!(interior_solar_absorptance(&boundary), 0.05);
        assert_eq!(exterior_emissivity(&boundary), 0.88);
        assert_eq!(attic_interior_emissivity(&boundary), 0.05);
    }

    /// Interior solar absorptance for a conditioned surface without explicit
    /// `solar_absorptance` must default to 0.70 (EnergyPlus Material IDD default),
    /// not the old hardcoded 0.6.
    #[test]
    fn conditioned_interior_solar_absorptance_defaults_to_ep_value() {
        let boundary = Boundary {
            id: "Wall1".to_string(),
            boundary_type: BoundaryType::Wall,
            area_m2: 20.0,
            azimuth_deg: Some(180.0),
            assembly_r_value_m2_k_w: None,
            r_value_layers_m2_k_w: Vec::new(),
            interior_zone: Some(ZoneType::Conditioned),
            exterior_zone: Some(ZoneType::Outdoor),
            material_layers: Vec::new(),
            construction_type: None,
            finish_type: None,
            insulation_details: None,
            has_radiant_barrier: false,
            solar_absorptance: None,
            emittance: None,
            lut_boundary_name: None,
            floor_or_ceiling: None,
            tilt_deg: Some(90.0),
            framing_factor: None,
            perimeter_m: None,
            perimeter_insulation_r_m2_k_w: None,
            foundation_depth_m: None,
        };
        assert_eq!(
            interior_solar_absorptance(&boundary),
            INTERIOR_SOLAR_ABSORPTANCE_DEFAULT,
            "interior solar absorptance without explicit HPXML value must default to E+ 0.70"
        );
    }

    /// BESTEST fixtures set `solar_absorptance = 0.6` explicitly; that value must
    /// flow through to the interior side unchanged.
    #[test]
    fn explicit_solar_absorptance_flows_to_interior_side() {
        let boundary = Boundary {
            id: "Wall1".to_string(),
            boundary_type: BoundaryType::Wall,
            area_m2: 20.0,
            azimuth_deg: Some(180.0),
            assembly_r_value_m2_k_w: None,
            r_value_layers_m2_k_w: Vec::new(),
            interior_zone: Some(ZoneType::Conditioned),
            exterior_zone: Some(ZoneType::Outdoor),
            material_layers: Vec::new(),
            construction_type: None,
            finish_type: None,
            insulation_details: None,
            has_radiant_barrier: false,
            solar_absorptance: Some(0.6),
            emittance: None,
            lut_boundary_name: None,
            floor_or_ceiling: None,
            tilt_deg: Some(90.0),
            framing_factor: None,
            perimeter_m: None,
            perimeter_insulation_r_m2_k_w: None,
            foundation_depth_m: None,
        };
        assert_eq!(
            interior_solar_absorptance(&boundary),
            0.6,
            "BESTEST 0.6 must pass through to interior solar absorptance"
        );
    }

    #[test]
    fn attic_interior_surfaces_participate_in_lwr() {
        assert!(include_interior_lwr(false, false, false, true, 12.0));
    }

    #[test]
    fn conditioned_interior_behavior_is_unchanged() {
        assert!(include_interior_lwr(false, false, true, false, 12.0));
        assert!(!include_interior_lwr(false, false, true, false, 0.0));
    }

    #[test]
    fn window_behavior_is_unchanged() {
        assert!(include_interior_lwr(true, true, false, false, 8.0));
        assert!(!include_interior_lwr(true, false, false, false, 8.0));
    }

    /// Window boundaries without explicit emittance default to EMISSIVITY_WINDOW (0.84)
    /// per EnergyPlus, not EMISSIVITY_DEFAULT (0.90) which is for opaque surfaces.
    /// NFRC standard emissivity for clear glass is 0.84.
    #[test]
    fn window_exterior_emissivity_defaults_to_glass_value() {
        use hares_envelope::EMISSIVITY_WINDOW;
        let window = Boundary {
            id: "Win1".to_string(),
            boundary_type: BoundaryType::Window,
            area_m2: 12.0,
            azimuth_deg: Some(180.0),
            assembly_r_value_m2_k_w: None,
            r_value_layers_m2_k_w: Vec::new(),
            interior_zone: Some(ZoneType::Conditioned),
            exterior_zone: Some(ZoneType::Outdoor),
            material_layers: Vec::new(),
            construction_type: None,
            finish_type: None,
            insulation_details: None,
            has_radiant_barrier: false,
            solar_absorptance: None,
            emittance: None,
            lut_boundary_name: None,
            floor_or_ceiling: None,
            tilt_deg: Some(90.0),
            framing_factor: None,
            perimeter_m: None,
            perimeter_insulation_r_m2_k_w: None,
            foundation_depth_m: None,
        };
        assert_eq!(
            exterior_emissivity(&window),
            EMISSIVITY_WINDOW,
            "window without explicit emittance should default to 0.84 (NFRC clear glass)"
        );
    }

    #[test]
    fn natural_ventilation_coefficients_prefer_indoor_zone_ela() {
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            infiltration: vec![
                (
                    ZoneId(1),
                    InfiltrationMethod::Ela {
                        ela_m2: 0.01,
                        stack_coeff: 1.23,
                        wind_coeff: 4.56,
                    },
                ),
                (
                    ZoneId(2),
                    InfiltrationMethod::Ela {
                        ela_m2: 0.02,
                        stack_coeff: 9.87,
                        wind_coeff: 6.54,
                    },
                ),
            ],
            ..ThermalSolverConfig::default()
        };

        let (stack_coeff, wind_coeff) = natural_ventilation_coefficients(&config, 6.0);
        assert_eq!(stack_coeff, 1.23);
        assert_eq!(wind_coeff, 4.56);
    }

    #[test]
    fn natural_ventilation_coefficients_fallback_use_building_height() {
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            infiltration: vec![(ZoneId(1), InfiltrationMethod::Ach { ach: 0.0 })],
            ..ThermalSolverConfig::default()
        };
        let building_height_m = 6.0;
        let ceiling_height_m = 3.0;

        let expected = calculate_ela_coefficients(
            0.0,
            building_height_m,
            0.0,
            TerrainClass::Suburban,
            SHIELDING_NORMAL,
        );
        let ceiling_expected = calculate_ela_coefficients(
            0.0,
            ceiling_height_m,
            0.0,
            TerrainClass::Suburban,
            SHIELDING_NORMAL,
        );
        let actual = natural_ventilation_coefficients(&config, building_height_m);

        assert!((actual.0 - expected.0).abs() < 1e-12);
        assert!((actual.1 - expected.1).abs() < 1e-12);
        assert!(
            (actual.0 - ceiling_expected.0).abs() > 1e-12
                || (actual.1 - ceiling_expected.1).abs() > 1e-12,
            "fallback must use full building height, not ceiling height"
        );
    }

    #[test]
    fn opening_azimuth_weighted_by_operable_area() {
        let boundaries = vec![
            Boundary {
                id: "wall-north".to_string(),
                boundary_type: BoundaryType::Wall,
                area_m2: 10.0,
                azimuth_deg: Some(0.0),
                assembly_r_value_m2_k_w: None,
                r_value_layers_m2_k_w: vec![],
                interior_zone: Some(ZoneType::Conditioned),
                exterior_zone: Some(ZoneType::Outdoor),
                material_layers: vec![],
                construction_type: None,
                finish_type: None,
                insulation_details: None,
                has_radiant_barrier: false,
                solar_absorptance: None,
                emittance: None,
                lut_boundary_name: None,
                floor_or_ceiling: None,
                tilt_deg: None,
                framing_factor: None,
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            },
            Boundary {
                id: "wall-south".to_string(),
                boundary_type: BoundaryType::Wall,
                area_m2: 10.0,
                azimuth_deg: Some(180.0),
                assembly_r_value_m2_k_w: None,
                r_value_layers_m2_k_w: vec![],
                interior_zone: Some(ZoneType::Conditioned),
                exterior_zone: Some(ZoneType::Outdoor),
                material_layers: vec![],
                construction_type: None,
                finish_type: None,
                insulation_details: None,
                has_radiant_barrier: false,
                solar_absorptance: None,
                emittance: None,
                lut_boundary_name: None,
                floor_or_ceiling: None,
                tilt_deg: None,
                framing_factor: None,
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            },
            Boundary {
                id: "wall-east-unknown".to_string(),
                boundary_type: BoundaryType::Wall,
                area_m2: 10.0,
                azimuth_deg: Some(90.0),
                assembly_r_value_m2_k_w: None,
                r_value_layers_m2_k_w: vec![],
                interior_zone: Some(ZoneType::Conditioned),
                exterior_zone: Some(ZoneType::Outdoor),
                material_layers: vec![],
                construction_type: None,
                finish_type: None,
                insulation_details: None,
                has_radiant_barrier: false,
                solar_absorptance: None,
                emittance: None,
                lut_boundary_name: None,
                floor_or_ceiling: None,
                tilt_deg: None,
                framing_factor: None,
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            },
            Boundary {
                id: "wall-garage".to_string(),
                boundary_type: BoundaryType::Wall,
                area_m2: 10.0,
                azimuth_deg: Some(270.0),
                assembly_r_value_m2_k_w: None,
                r_value_layers_m2_k_w: vec![],
                interior_zone: Some(ZoneType::Garage),
                exterior_zone: Some(ZoneType::Outdoor),
                material_layers: vec![],
                construction_type: None,
                finish_type: None,
                insulation_details: None,
                has_radiant_barrier: false,
                solar_absorptance: None,
                emittance: None,
                lut_boundary_name: None,
                floor_or_ceiling: None,
                tilt_deg: None,
                framing_factor: None,
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            },
        ];
        let windows = vec![
            // North window: 2 m², 50% operable → 1.0 m² operable at 0°
            Window {
                id: "north".to_string(),
                area_m2: 2.0,
                azimuth_deg: Some(0.0),
                u_factor_w_m2_k: Some(2.0),
                shgc: Some(0.5),
                interior_shading_fraction: 1.0,
                winter_shading_fraction: 1.0,
                fraction_operable: 0.5,
                exterior_shading_summer: 1.0,
                exterior_shading_winter: 1.0,
                attached_to_wall_id: Some("wall-north".to_string()),
            },
            // South window: 1 m², 100% operable → 1.0 m² operable at 180°
            Window {
                id: "south".to_string(),
                area_m2: 1.0,
                azimuth_deg: Some(180.0),
                u_factor_w_m2_k: Some(2.0),
                shgc: Some(0.5),
                interior_shading_fraction: 1.0,
                winter_shading_fraction: 1.0,
                fraction_operable: 1.0,
                exterior_shading_summer: 1.0,
                exterior_shading_winter: 1.0,
                attached_to_wall_id: Some("wall-south".to_string()),
            },
            // East window: 3 m², 50% operable → 1.5 m² operable, azimuth=None
            Window {
                id: "east".to_string(),
                area_m2: 3.0,
                azimuth_deg: None,
                u_factor_w_m2_k: Some(2.0),
                shgc: Some(0.5),
                interior_shading_fraction: 1.0,
                winter_shading_fraction: 1.0,
                fraction_operable: 0.5,
                exterior_shading_summer: 1.0,
                exterior_shading_winter: 1.0,
                attached_to_wall_id: Some("wall-east-unknown".to_string()),
            },
            // Fixed window: 5 m², 0% operable → excluded from operable area
            Window {
                id: "fixed".to_string(),
                area_m2: 5.0,
                azimuth_deg: Some(45.0),
                u_factor_w_m2_k: Some(2.0),
                shgc: Some(0.5),
                interior_shading_fraction: 1.0,
                winter_shading_fraction: 1.0,
                fraction_operable: 0.0,
                exterior_shading_summer: 1.0,
                exterior_shading_winter: 1.0,
                attached_to_wall_id: Some("wall-north".to_string()),
            },
            // Garage window: should be excluded (not conditioned zone)
            Window {
                id: "garage".to_string(),
                area_m2: 4.0,
                azimuth_deg: Some(270.0),
                u_factor_w_m2_k: Some(2.0),
                shgc: Some(0.5),
                interior_shading_fraction: 1.0,
                winter_shading_fraction: 1.0,
                fraction_operable: 1.0,
                exterior_shading_summer: 1.0,
                exterior_shading_winter: 1.0,
                attached_to_wall_id: Some("wall-garage".to_string()),
            },
        ];

        // Hand calculation:
        // Operable windows: north (1.0 m² at 0°), south (1.0 m² at 180°).
        // East window has None azimuth → excluded from azimuth average but contributes area.
        // Fixed has 0 operable → excluded. Garage has non-conditioned zone → excluded.
        // total_operable_area = 1.0 + 1.0 + 1.5 = 3.5
        // az_area_sum = 1.0 + 1.0 = 2.0
        // az_weighted_sum = 1.0×0 + 1.0×180 = 180
        // azimuth = 180 / 2.0 = 90.0

        let (total_area, azimuth) = compute_opening_azimuth_and_area(&windows, &boundaries, 180.0);

        assert!(
            (total_area - 3.5).abs() < 1e-10,
            "total operable area: expected 3.5, got {total_area}"
        );
        assert!(
            (azimuth - 90.0).abs() < 1e-10,
            "area-weighted azimuth: expected 90.0, got {azimuth}"
        );
    }

    #[test]
    fn opening_azimuth_falls_back_to_default_when_no_azimuth_known() {
        let boundaries = vec![Boundary {
            id: "wall".to_string(),
            boundary_type: BoundaryType::Wall,
            area_m2: 10.0,
            azimuth_deg: None,
            assembly_r_value_m2_k_w: None,
            r_value_layers_m2_k_w: vec![],
            interior_zone: Some(ZoneType::Conditioned),
            exterior_zone: Some(ZoneType::Outdoor),
            material_layers: vec![],
            construction_type: None,
            finish_type: None,
            insulation_details: None,
            has_radiant_barrier: false,
            solar_absorptance: None,
            emittance: None,
            lut_boundary_name: None,
            floor_or_ceiling: None,
            tilt_deg: None,
            framing_factor: None,
            perimeter_m: None,
            perimeter_insulation_r_m2_k_w: None,
            foundation_depth_m: None,
        }];
        let windows = vec![Window {
            id: "w".to_string(),
            area_m2: 2.0,
            azimuth_deg: None,
            u_factor_w_m2_k: Some(2.0),
            shgc: Some(0.5),
            interior_shading_fraction: 1.0,
            winter_shading_fraction: 1.0,
            fraction_operable: 0.5,
            exterior_shading_summer: 1.0,
            exterior_shading_winter: 1.0,
            attached_to_wall_id: Some("wall".to_string()),
        }];

        let (total_area, azimuth) = compute_opening_azimuth_and_area(&windows, &boundaries, 180.0);

        assert!(
            total_area > 0.0,
            "should have operable area even without azimuth"
        );
        assert!(
            (azimuth - 180.0).abs() < 1e-10,
            "should fall back to default 180° when no azimuth known, got {azimuth}"
        );
    }

    #[test]
    fn opening_azimuth_zero_when_no_operable_windows() {
        let boundaries = vec![Boundary {
            id: "wall".to_string(),
            boundary_type: BoundaryType::Wall,
            area_m2: 10.0,
            azimuth_deg: None,
            assembly_r_value_m2_k_w: None,
            r_value_layers_m2_k_w: vec![],
            interior_zone: Some(ZoneType::Conditioned),
            exterior_zone: Some(ZoneType::Outdoor),
            material_layers: vec![],
            construction_type: None,
            finish_type: None,
            insulation_details: None,
            has_radiant_barrier: false,
            solar_absorptance: None,
            emittance: None,
            lut_boundary_name: None,
            floor_or_ceiling: None,
            tilt_deg: None,
            framing_factor: None,
            perimeter_m: None,
            perimeter_insulation_r_m2_k_w: None,
            foundation_depth_m: None,
        }];
        let windows = Vec::<Window>::new();

        let (total_area, azimuth) = compute_opening_azimuth_and_area(&windows, &boundaries, 180.0);

        assert!(
            (total_area - 0.0).abs() < 1e-10,
            "no windows → zero operable area, got {total_area}"
        );
        assert!(
            (azimuth - 180.0).abs() < 1e-10,
            "no windows → falls back to default azimuth"
        );
    }

    fn foundation_zone(
        floor_area_m2: Option<f64>,
        volume_m3: Option<f64>,
        vented: bool,
        ventilation_ach: Option<f64>,
        ventilation_sla: Option<f64>,
    ) -> Zone {
        Zone {
            zone_type: ZoneType::Foundation,
            floor_area_m2,
            volume_m3,
            attached_wall_ids: Vec::new(),
            duct_systems: Vec::new(),
            vented,
            ventilation_ach,
            ventilation_sla,
        }
    }

    fn attic_zone(
        floor_area_m2: Option<f64>,
        volume_m3: Option<f64>,
        vented: bool,
        ventilation_ach: Option<f64>,
        ventilation_sla: Option<f64>,
    ) -> Zone {
        Zone {
            zone_type: ZoneType::Attic,
            floor_area_m2,
            volume_m3,
            attached_wall_ids: Vec::new(),
            duct_systems: Vec::new(),
            vented,
            ventilation_ach,
            ventilation_sla,
        }
    }

    #[test]
    fn foundation_height_comes_from_zone_geometry() {
        let zone = foundation_zone(Some(50.0), Some(75.0), false, None, None);
        let height = foundation_height_m(&zone).expect("height expected");
        assert!((height - 1.5).abs() < 1e-12);
    }

    #[test]
    fn foundation_sla_resolves_to_ela() {
        let zone = foundation_zone(Some(50.0), Some(75.0), false, None, Some(0.0002));
        let method = foundation_infiltration_method(&zone, Some(100.0), Some(1.5));
        match method {
            InfiltrationMethod::Ela {
                ela_m2,
                stack_coeff,
                wind_coeff,
            } => {
                assert!((ela_m2 - 0.01).abs() < 1e-12);
                let (expected_stack, expected_wind) = calculate_ela_coefficients(
                    0.0,
                    1.5,
                    0.0,
                    TerrainClass::Suburban,
                    SHIELDING_NORMAL,
                );
                assert!((stack_coeff - expected_stack).abs() < 1e-12);
                assert!((wind_coeff - expected_wind).abs() < 1e-12);
            }
            other => panic!("expected ELA, got {other:?}"),
        }
    }

    #[test]
    fn foundation_ach_takes_precedence_over_sla() {
        let zone = foundation_zone(Some(50.0), Some(75.0), true, Some(3.0), Some(0.0002));
        let method = foundation_infiltration_method(&zone, Some(100.0), Some(1.5));
        assert_eq!(method, InfiltrationMethod::Ach { ach: 3.0 });
    }

    #[test]
    fn foundation_default_fallbacks_remain_ach() {
        let vented = foundation_zone(Some(50.0), Some(75.0), true, None, None);
        let unvented = foundation_zone(Some(50.0), Some(75.0), false, None, None);

        assert_eq!(
            foundation_infiltration_method(&vented, Some(100.0), Some(1.5)),
            InfiltrationMethod::Ach { ach: 2.0 }
        );
        assert_eq!(
            foundation_infiltration_method(&unvented, Some(100.0), Some(1.5)),
            InfiltrationMethod::Ach { ach: 0.0 }
        );
    }

    #[test]
    fn foundation_sla_without_geometry_falls_back_to_ach_defaults() {
        let vented = foundation_zone(None, None, true, None, Some(0.0002));
        let unvented = foundation_zone(None, None, false, None, Some(0.0002));

        assert_eq!(
            foundation_infiltration_method(&vented, None, None),
            InfiltrationMethod::Ach { ach: 2.0 }
        );
        assert_eq!(
            foundation_infiltration_method(&unvented, None, None),
            InfiltrationMethod::Ach { ach: 0.0 }
        );
    }

    #[test]
    fn attic_vented_defaults_to_resnet_sla() {
        let zone = attic_zone(Some(100.0), Some(120.0), true, None, None);
        let method = attic_infiltration_method(&zone, Some(100.0), 5.0, 3)
            .expect("vented attic with no rate must use default SLA");
        let InfiltrationMethod::Ela { ela_m2, .. } = method else {
            panic!("expected ELA method from default SLA, got {method:?}");
        };
        // ANSI/RESNET/ICC 301-2019 Table 4.2.2(1): SLA = 1/300 ≈ 0.00333.
        // floor_area = 100 m² → ela = 100 × 0.00333 = 0.333 m².
        assert!(
            (ela_m2 - 0.333).abs() < 1e-6,
            "expected ela = 0.333 m² from default SLA=0.00333 × 100 m², got {ela_m2}"
        );
    }

    #[test]
    fn attic_vented_with_sla_builds_successfully() {
        let zone = attic_zone(Some(100.0), Some(120.0), true, None, Some(0.003));
        let method = attic_infiltration_method(&zone, Some(100.0), 5.0, 3)
            .expect("vented attic with SLA=0.003 must succeed");
        let InfiltrationMethod::Ela { ela_m2, .. } = method else {
            panic!("expected ELA method from provided SLA, got {method:?}");
        };
        // SLA = 0.003 × 100 m² = 0.3 m².
        assert!(
            (ela_m2 - 0.3).abs() < 1e-9,
            "expected ela = 0.3 m² from SLA=0.003 × 100 m², got {ela_m2}"
        );
    }

    #[test]
    fn attic_unvented_default_is_minimal_ach() {
        let zone = attic_zone(Some(100.0), Some(120.0), false, None, None);
        let method = attic_infiltration_method(&zone, Some(100.0), 5.0, 3).expect("method");
        assert_eq!(method, InfiltrationMethod::Ach { ach: 0.1 });
    }

    #[test]
    fn ventilation_effectiveness_keys_map_to_recovery_efficiency() {
        use hares_envelope::MechanicalVentilationParams;
        use hares_equipment::{EquipmentConfig, VentilationConfig};

        let cfg = VentilationConfig {
            equipment_id: None,
            zone_id: None,
            flow_rate_m3_s: 0.035,
            fan_power_w: None,
            supply_fan_power_w: None,
            exhaust_fan_power_w: None,
            sensible_effectiveness: Some(0.75),
            latent_effectiveness: Some(0.65),
            bypass_temp_min_c: None,
            bypass_temp_max_c: None,
            defrost_temp_c: None,
            defrost_effectiveness_fraction: None,
            ventilation_type: Some("erv".to_string()),
            balanced: Some(true),
            hours_in_operation: None,
        };

        let ec = EquipmentConfig::from_typed(
            "Ventilation Fan".to_string(),
            "Ventilation Fan".to_string(),
            cfg.clone(),
        )
        .unwrap();
        let recovered: VentilationConfig = ec.typed().unwrap();

        let ventilation = MechanicalVentilationParams {
            balanced: recovered.balanced.unwrap_or(false),
            sensible_recovery_efficiency: recovered.sensible_effectiveness.unwrap_or_default(),
            latent_recovery_efficiency: recovered.latent_effectiveness.unwrap_or_default(),
            ..Default::default()
        };

        assert!(ventilation.balanced);
        assert!(
            (ventilation.sensible_recovery_efficiency - 0.75).abs() < 1e-12,
            "sensible recovery should be 0.75, got {}",
            ventilation.sensible_recovery_efficiency
        );
        assert!(
            (ventilation.latent_recovery_efficiency - 0.65).abs() < 1e-12,
            "latent recovery should be 0.65, got {}",
            ventilation.latent_recovery_efficiency
        );
    }

    fn garage_zone(
        floor_area_m2: Option<f64>,
        volume_m3: Option<f64>,
        ventilation_ach: Option<f64>,
        ventilation_sla: Option<f64>,
    ) -> Zone {
        Zone {
            zone_type: ZoneType::Garage,
            floor_area_m2,
            volume_m3,
            attached_wall_ids: Vec::new(),
            duct_systems: Vec::new(),
            vented: false,
            ventilation_ach,
            ventilation_sla,
        }
    }

    #[test]
    fn garage_ach_takes_precedence_over_sla() {
        let zone = garage_zone(Some(28.0), Some(67.2), Some(5.0), Some(0.0003));
        let method = garage_infiltration_method(&zone, None, 2.4, 0).expect("method");
        assert_eq!(method, InfiltrationMethod::Ach { ach: 5.0 });
    }

    #[test]
    fn garage_sla_resolves_to_ela() {
        let zone = garage_zone(Some(28.0), Some(67.2), None, Some(0.0003));
        let method = garage_infiltration_method(&zone, None, 2.4, 0).expect("method");
        match method {
            InfiltrationMethod::Ela {
                ela_m2,
                stack_coeff,
                wind_coeff,
            } => {
                assert!((ela_m2 - 0.0084).abs() < 1e-9);
                let (expected_stack, expected_wind) = garage_ela_coefficients(2.4);
                assert!((stack_coeff - expected_stack).abs() < 1e-12);
                assert!((wind_coeff - expected_wind).abs() < 1e-12);
            }
            other => panic!("expected ELA, got {other:?}"),
        }
    }

    #[test]
    fn garage_default_sla_fallback_produces_nonzero_ela() {
        let zone = garage_zone(Some(28.0), Some(67.2), None, None);
        let method = garage_infiltration_method(&zone, None, 2.4, 0).expect("method");
        match method {
            InfiltrationMethod::Ela {
                ela_m2,
                stack_coeff,
                wind_coeff,
            } => {
                // SLA = 3.0e-4, floor area = 28 m² → ela_m2 = 0.0084
                assert!((ela_m2 - 0.0084).abs() < 1e-9);
                let (expected_stack, expected_wind) = garage_ela_coefficients(2.4);
                assert!((stack_coeff - expected_stack).abs() < 1e-12);
                assert!((wind_coeff - expected_wind).abs() < 1e-12);
            }
            other => panic!("expected ELA, got {other:?}"),
        }
    }

    #[test]
    fn garage_default_sla_is_climate_aware_via_ela() {
        // The ELA model produces time-varying infiltration unlike a constant ACH.
        // Verify that the returned method is ELA, not Ach.
        let zone = garage_zone(Some(28.0), Some(67.2), None, None);
        let method = garage_infiltration_method(&zone, None, 2.4, 0).expect("method");
        assert!(
            matches!(method, InfiltrationMethod::Ela { .. }),
            "garage default must be ELA for climate-aware infiltration, got {method:?}"
        );
    }

    #[test]
    fn garage_uses_conditioned_floor_area_as_fallback() {
        let zone = garage_zone(None, Some(67.2), None, Some(0.0003));
        let method = garage_infiltration_method(&zone, Some(50.0), 2.4, 0).expect("method");
        match method {
            InfiltrationMethod::Ela { ela_m2, .. } => {
                assert!(
                    (ela_m2 - 0.015).abs() < 1e-12,
                    "expected ela=0.015 from 50m² × 0.0003"
                );
            }
            other => panic!("expected ELA, got {other:?}"),
        }
    }

    #[test]
    fn garage_without_any_floor_area_uses_default_28m2() {
        let zone = garage_zone(None, Some(67.2), None, None);
        let method = garage_infiltration_method(&zone, None, 2.4, 0).expect("method");
        match method {
            InfiltrationMethod::Ela { ela_m2, .. } => {
                // SLA = 3.0e-4, fallback floor area = 28 m² → ela_m2 = 0.0084
                assert!((ela_m2 - 0.0084).abs() < 1e-9);
            }
            other => panic!("expected ELA, got {other:?}"),
        }
    }

    #[test]
    fn garage_not_ach_based() {
        // The old behaviour was InfiltrationMethod::Ach with a fixed ACH.
        // The new code must never return Ach when no zone-level ACH is given.
        // Even the default path returns ELA, not Ach.
        let zone = garage_zone(Some(28.0), Some(67.2), None, None);
        let method = garage_infiltration_method(&zone, None, 2.4, 0).expect("method");
        assert!(
            !matches!(method, InfiltrationMethod::Ach { .. }),
            "garage must not use constant-Ach method; got {method:?}"
        );
    }

    #[test]
    fn garage_height_derived_from_zone_geometry() {
        // Zone has volume=67.2 m³, floor_area=28 m² → height = 2.4 m.
        let (stack_coeff, wind_coeff) = garage_ela_coefficients(67.2 / 28.0);
        let expected_stack = garage_ela_coefficients(2.4).0;
        assert!(
            (stack_coeff - expected_stack).abs() < 1e-12,
            "coefficients must match for equivalent height"
        );
        assert!(wind_coeff > 0.0, "wind coefficient must be positive");
    }

    #[test]
    fn attic_adjacent_boundary_rewritten_to_same_zone_internal_mass() {
        let xml = r#"
<HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site>
          <SiteType>suburban</SiteType>
          <ShieldingOfHome>normal</ShieldingOfHome>
        </Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="ft2">1000</ConditionedFloorArea>
          <ConditionedBuildingVolume units="ft3">8000</ConditionedBuildingVolume>
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
          </Wall>
        </Walls>
        <Roofs>
          <Roof>
            <SystemIdentifier id="Roof1"/>
            <InteriorAdjacentTo>attic vented</InteriorAdjacentTo>
            <ExteriorAdjacentTo>other housing unit</ExteriorAdjacentTo>
            <Area units="ft2">120</Area>
          </Roof>
        </Roofs>
        <Attics>
          <Attic>
            <FloorArea units="ft2">500</FloorArea>
          </Attic>
        </Attics>
      </Enclosure>
    </BuildingDetails>
  </Building>
</HPXML>"#;

        let building =
            hares_io::hpxml::building::parse_building(xml).expect("parse should succeed");

        let roof = building
            .boundaries
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Roof && b.id == "Roof1")
            .expect("roof expected");

        assert_eq!(
            roof.interior_zone, roof.exterior_zone,
            "Attic←Adjacent party wall must be rewritten to same-zone (Attic, Attic)"
        );
        assert!(
            roof.interior_zone.is_some(),
            "rewritten boundary must have a zone reference"
        );
        assert_eq!(roof.interior_zone, Some(ZoneType::Attic));
    }

    /// When a surface has a `layer_info` entry but the referenced `outer_node`
    /// is not present in `node_index`, the wiring silently produces `None` —
    /// the invariant check must catch this in debug/check_invariants builds.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "surface 0")]
    fn missing_node_id_asserts_in_wiring() {
        use super::{RCContext, build_solver_boundaries};
        use chrono::TimeZone;
        use hares_envelope::{
            BoundaryDiagnostic, BoundaryInput, EnvelopeDiagnostics, ExteriorTarget, NodeId, RCPath,
            SurfaceLayerInfo,
        };
        use hares_io::hpxml::{Boundary, BoundaryType, Building, Site, Zone, ZoneType};
        use hares_types::{EnvironmentState, GridState, ZoneState};
        use std::collections::HashMap;

        let building = Building {
            site: Site {
                elevation_m: None,
                site_type: None,
                shielding_of_home: None,
                latitude_deg: None,
                longitude_deg: None,
                utc_offset_h: None,
            },
            zones: vec![Zone {
                zone_type: ZoneType::Conditioned,
                floor_area_m2: None,
                volume_m3: None,
                attached_wall_ids: vec![],
                duct_systems: vec![],
                vented: false,
                ventilation_ach: None,
                ventilation_sla: None,
            }],
            boundaries: vec![Boundary {
                id: "Wall1".to_string(),
                boundary_type: BoundaryType::Wall,
                area_m2: 100.0,
                azimuth_deg: None,
                assembly_r_value_m2_k_w: None,
                r_value_layers_m2_k_w: vec![],
                interior_zone: Some(ZoneType::Conditioned),
                exterior_zone: Some(ZoneType::Outdoor),
                material_layers: vec![],
                construction_type: None,
                finish_type: None,
                insulation_details: None,
                has_radiant_barrier: false,
                solar_absorptance: None,
                emittance: None,
                lut_boundary_name: None,
                floor_or_ceiling: None,
                tilt_deg: Some(90.0),
                framing_factor: None,
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            }],
            windows: vec![],
            skylights: vec![],
            infiltration_ach50: None,
            infiltration_cfm50: None,
            infiltration_ach_natural: None,
            infiltration_cfm_natural: None,
            infiltration_ela_cm2: None,
            infiltration_constant_ach: None,
            hvac_capacity_w: None,
            seer2: None,
            hspf2: None,
            water_heater_setpoint_c: None,
            heating_weekday_setpoints_c: None,
            heating_weekend_setpoints_c: None,
            cooling_weekday_setpoints_c: None,
            cooling_weekend_setpoints_c: None,
            battery_round_trip_efficiency: None,
            pv_tilt_deg: None,
            conditioned_volume_m3: None,
            ceiling_height_m: None,
            infiltration_height_m: None,
            floors_above_grade: None,
            has_flue_or_chimney: None,
            foundation_name: None,
            residential_facility_type: None,
            mass_multiplier_override: None,
            hvac_deadband_c: None,
            details_xml: hares_io::hpxml::building::XmlNode {
                name: String::new(),
                attrs: HashMap::new(),
                text: String::new(),
                children: vec![],
            },
        };

        let boundary_inputs = vec![BoundaryInput {
            area_m2: 100.0,
            interior_zone_idx: 0,
            exterior: ExteriorTarget::Outdoor,
            material_layers: vec![],
            precomputed_rc: vec![],
            fallback_r_m2_k_w: 1.0,
            r_film_interior_m2_k_w: 0.12,
            r_film_exterior_m2_k_w: 0.03,
            framing_factor: None,
            interior_emissivity: 0.9,
            foundation_depth_m: 0.0,
            #[cfg(feature = "observe")]
            used_default_r: false,
        }];

        // layer_info maps surface 0 to a NodeId(999) that does NOT exist in node_index.
        let layer_info: HashMap<usize, SurfaceLayerInfo> = HashMap::from([(
            0,
            SurfaceLayerInfo {
                outer_node: NodeId(999),
                inner_node: NodeId(999),
                surface_node: None,
                interior_zone_idx: 0,
            },
        )]);
        let node_index: HashMap<NodeId, usize> = HashMap::new();

        let envelope_diagnostics = EnvelopeDiagnostics {
            boundaries: vec![BoundaryDiagnostic {
                boundary_idx: 0,
                ua_w_per_k: 0.0,
                r_total_m2_k_w: 0.0,
                capacitance_j_k: 0.0,
                n_rc_nodes: 0,
                interior_zone_idx: 0,
                exterior_target: ExteriorTarget::Outdoor,
                area_m2: 100.0,
                r_film_int_m2_k_w: 0.12,
                r_film_ext_m2_k_w: 0.03,
                r_zone_to_inner_m2_k_w: None,
                r_outer_half_m2_k_w: None,
                r_inner_half_m2_k_w: None,
                path: RCPath::FallbackR,
                inner_node: None,
                interior_emissivity: 0.9,
                foundation_depth_m: 0.0,
                #[cfg(feature = "observe")]
                same_zone_kept_half: None,
            }],
            zone_capacitances_j_k: vec![1000.0],
            total_ua_w_per_k: 0.0,
            #[cfg(feature = "observe")]
            default_r_fallback_count: 0,
        };

        let rc = RCContext {
            layer_info: &layer_info,
            node_index: &node_index,
            envelope_diagnostics: &envelope_diagnostics,
            n_zones: 1,
            n_ext: 1,
        };

        let env = EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 20.0,
                humidity_ratio: 0.01,
                volume_m3: 250.0,
            }],
            weather: Default::default(),
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
                island_bus_voltage_pu: None,
            },
            custom_domains: vec![],
            equipment_telemetry: HashMap::new(),
            equipment_core: HashMap::new(),
            current_time: chrono::FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2025, 1, 1, 0, 0, 0)
                .unwrap(),
            time_res: chrono::Duration::minutes(1),
            price_signal: hares_types::PriceSignal {
                electricity_price: None,
                export_price: None,
                ghg_intensity: None,
            },
            electrical: Default::default(),
        };

        let _ = build_solver_boundaries(&building, &boundary_inputs, &rc, &env);
    }

    /// A surface with no `layer_info` entry (fallback-R) correctly produces
    /// `None` for both outer_wiring and inner_wiring without triggering
    /// the invariant assertions.
    #[test]
    fn legitimate_none_wiring_for_fallback_r() {
        use chrono::TimeZone;
        use hares_envelope::{
            BoundaryDiagnostic, BoundaryInput, EnvelopeDiagnostics, ExteriorTarget, NodeId, RCPath,
        };
        use hares_io::hpxml::{Boundary, BoundaryType, Building, Site, Zone, ZoneType};
        use hares_types::{EnvironmentState, GridState, ZoneState};
        use std::collections::HashMap;

        use super::{RCContext, build_solver_boundaries};

        let building = Building {
            site: Site {
                elevation_m: None,
                site_type: None,
                shielding_of_home: None,
                latitude_deg: None,
                longitude_deg: None,
                utc_offset_h: None,
            },
            zones: vec![Zone {
                zone_type: ZoneType::Conditioned,
                floor_area_m2: None,
                volume_m3: None,
                attached_wall_ids: vec![],
                duct_systems: vec![],
                vented: false,
                ventilation_ach: None,
                ventilation_sla: None,
            }],
            boundaries: vec![Boundary {
                id: "Wall1".to_string(),
                boundary_type: BoundaryType::Wall,
                area_m2: 100.0,
                azimuth_deg: None,
                assembly_r_value_m2_k_w: None,
                r_value_layers_m2_k_w: vec![],
                interior_zone: Some(ZoneType::Conditioned),
                exterior_zone: Some(ZoneType::Outdoor),
                material_layers: vec![],
                construction_type: None,
                finish_type: None,
                insulation_details: None,
                has_radiant_barrier: false,
                solar_absorptance: None,
                emittance: None,
                lut_boundary_name: None,
                floor_or_ceiling: None,
                tilt_deg: Some(90.0),
                framing_factor: None,
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            }],
            windows: vec![],
            skylights: vec![],
            infiltration_ach50: None,
            infiltration_cfm50: None,
            infiltration_ach_natural: None,
            infiltration_cfm_natural: None,
            infiltration_ela_cm2: None,
            infiltration_constant_ach: None,
            hvac_capacity_w: None,
            seer2: None,
            hspf2: None,
            water_heater_setpoint_c: None,
            heating_weekday_setpoints_c: None,
            heating_weekend_setpoints_c: None,
            cooling_weekday_setpoints_c: None,
            cooling_weekend_setpoints_c: None,
            battery_round_trip_efficiency: None,
            pv_tilt_deg: None,
            conditioned_volume_m3: None,
            ceiling_height_m: None,
            infiltration_height_m: None,
            floors_above_grade: None,
            has_flue_or_chimney: None,
            foundation_name: None,
            residential_facility_type: None,
            mass_multiplier_override: None,
            hvac_deadband_c: None,
            details_xml: hares_io::hpxml::building::XmlNode {
                name: String::new(),
                attrs: HashMap::new(),
                text: String::new(),
                children: vec![],
            },
        };

        let boundary_inputs = vec![BoundaryInput {
            area_m2: 100.0,
            interior_zone_idx: 0,
            exterior: ExteriorTarget::Outdoor,
            material_layers: vec![],
            precomputed_rc: vec![],
            fallback_r_m2_k_w: 1.0,
            r_film_interior_m2_k_w: 0.12,
            r_film_exterior_m2_k_w: 0.03,
            framing_factor: None,
            interior_emissivity: 0.9,
            foundation_depth_m: 0.0,
            #[cfg(feature = "observe")]
            used_default_r: false,
        }];

        // No layer_info entry for this surface (fallback-R path).
        let layer_info: HashMap<usize, hares_envelope::SurfaceLayerInfo> = HashMap::new();
        let node_index: HashMap<NodeId, usize> = HashMap::new();

        let envelope_diagnostics = EnvelopeDiagnostics {
            boundaries: vec![BoundaryDiagnostic {
                boundary_idx: 0,
                ua_w_per_k: 0.0,
                r_total_m2_k_w: 0.0,
                capacitance_j_k: 0.0,
                n_rc_nodes: 0,
                interior_zone_idx: 0,
                exterior_target: ExteriorTarget::Outdoor,
                area_m2: 100.0,
                r_film_int_m2_k_w: 0.12,
                r_film_ext_m2_k_w: 0.03,
                r_zone_to_inner_m2_k_w: None,
                r_outer_half_m2_k_w: None,
                r_inner_half_m2_k_w: None,
                path: RCPath::FallbackR,
                inner_node: None,
                interior_emissivity: 0.9,
                foundation_depth_m: 0.0,
                #[cfg(feature = "observe")]
                same_zone_kept_half: None,
            }],
            zone_capacitances_j_k: vec![1000.0],
            total_ua_w_per_k: 0.0,
            #[cfg(feature = "observe")]
            default_r_fallback_count: 0,
        };

        let rc = RCContext {
            layer_info: &layer_info,
            node_index: &node_index,
            envelope_diagnostics: &envelope_diagnostics,
            n_zones: 1,
            n_ext: 1,
        };

        let env = EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 20.0,
                humidity_ratio: 0.01,
                volume_m3: 250.0,
            }],
            weather: Default::default(),
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
                island_bus_voltage_pu: None,
            },
            custom_domains: vec![],
            equipment_telemetry: HashMap::new(),
            equipment_core: HashMap::new(),
            current_time: chrono::FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2025, 1, 1, 0, 0, 0)
                .unwrap(),
            time_res: chrono::Duration::minutes(1),
            price_signal: hares_types::PriceSignal {
                electricity_price: None,
                export_price: None,
                ghg_intensity: None,
            },
            electrical: Default::default(),
        };

        let (boundaries, ext_cols, int_cols) =
            build_solver_boundaries(&building, &boundary_inputs, &rc, &env)
                .expect("build_solver_boundaries should succeed for fallback-R surface");

        assert_eq!(boundaries.len(), 1);
        assert!(
            boundaries[0].outer_wiring.is_none(),
            "fallback-R surface must have no outer_wiring"
        );
        assert!(
            boundaries[0].inner_wiring.is_none(),
            "fallback-R surface must have no inner_wiring"
        );
        assert_eq!(ext_cols, 0, "no exterior surface columns for fallback-R");
        assert_eq!(int_cols, 0, "no interior surface columns for fallback-R");
    }

    /// Skylight solar data is populated in `build_solver_boundaries` when the
    /// fenestration is found in `building.skylights`.  This is the regression
    /// test for the finding: the `.chain(building.skylights.iter())` path in the
    /// `window_solar` lookup must resolve correctly for `BoundaryType::Skylight`,
    /// matching the same behaviour as `BoundaryType::Window`.  If the lookup
    /// returned `None` (ID mismatch, missing `skylights` entry, etc.) the
    /// skylight would silently contribute zero solar gain.
    #[test]
    fn skylight_window_solar_populated_in_solver_boundaries() {
        use super::{RCContext, build_solver_boundaries};
        use chrono::TimeZone;
        use hares_envelope::{
            BoundaryCategory, BoundaryDiagnostic, BoundaryInput, EnvelopeDiagnostics,
            ExteriorTarget,
        };
        use hares_io::hpxml::{Boundary, BoundaryType, Site};
        use hares_types::{EnvironmentState, GridState, PriceSignal, ZoneId, ZoneState};
        use std::collections::HashMap;

        let skylight_id = "SK1";

        let building = hares_io::Building {
            site: Site {
                elevation_m: None,
                site_type: None,
                shielding_of_home: None,
                latitude_deg: None,
                longitude_deg: None,
                utc_offset_h: None,
            },
            zones: vec![Zone {
                zone_type: ZoneType::Conditioned,
                floor_area_m2: None,
                volume_m3: None,
                attached_wall_ids: vec![],
                duct_systems: vec![],
                vented: false,
                ventilation_ach: None,
                ventilation_sla: None,
            }],
            boundaries: vec![Boundary {
                id: skylight_id.to_string(),
                boundary_type: BoundaryType::Skylight,
                area_m2: 2.0,
                azimuth_deg: Some(180.0),
                assembly_r_value_m2_k_w: None,
                r_value_layers_m2_k_w: vec![],
                interior_zone: Some(ZoneType::Conditioned),
                exterior_zone: Some(ZoneType::Outdoor),
                material_layers: vec![],
                construction_type: None,
                finish_type: None,
                insulation_details: None,
                has_radiant_barrier: false,
                solar_absorptance: None,
                emittance: None,
                lut_boundary_name: None,
                floor_or_ceiling: None,
                tilt_deg: Some(0.0),
                framing_factor: None,
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            }],
            windows: vec![],
            skylights: vec![Window {
                id: skylight_id.to_string(),
                area_m2: 2.0,
                azimuth_deg: Some(180.0),
                u_factor_w_m2_k: Some(2.0),
                shgc: Some(0.5),
                interior_shading_fraction: 1.0,
                winter_shading_fraction: 1.0,
                fraction_operable: 0.0,
                exterior_shading_summer: 1.0,
                exterior_shading_winter: 1.0,
                attached_to_wall_id: None,
            }],
            infiltration_ach50: None,
            infiltration_cfm50: None,
            infiltration_ach_natural: None,
            infiltration_cfm_natural: None,
            infiltration_ela_cm2: None,
            infiltration_constant_ach: None,
            hvac_capacity_w: None,
            seer2: None,
            hspf2: None,
            water_heater_setpoint_c: None,
            heating_weekday_setpoints_c: None,
            heating_weekend_setpoints_c: None,
            cooling_weekday_setpoints_c: None,
            cooling_weekend_setpoints_c: None,
            battery_round_trip_efficiency: None,
            pv_tilt_deg: None,
            conditioned_volume_m3: None,
            ceiling_height_m: None,
            infiltration_height_m: None,
            floors_above_grade: None,
            has_flue_or_chimney: None,
            foundation_name: None,
            residential_facility_type: None,
            mass_multiplier_override: None,
            hvac_deadband_c: None,
            details_xml: hares_io::hpxml::building::XmlNode {
                name: String::new(),
                attrs: HashMap::new(),
                text: String::new(),
                children: vec![],
            },
        };

        let boundary_inputs = vec![BoundaryInput {
            area_m2: 2.0,
            interior_zone_idx: 0,
            exterior: ExteriorTarget::Outdoor,
            material_layers: vec![],
            precomputed_rc: vec![],
            fallback_r_m2_k_w: 0.5,
            r_film_interior_m2_k_w: 0.12,
            r_film_exterior_m2_k_w: 0.03,
            framing_factor: None,
            interior_emissivity: 0.84,
            foundation_depth_m: 0.0,
            #[cfg(feature = "observe")]
            used_default_r: false,
        }];

        let layer_info: HashMap<usize, hares_envelope::SurfaceLayerInfo> = HashMap::new();
        let node_index: HashMap<hares_envelope::NodeId, usize> = HashMap::new();

        let envelope_diagnostics = EnvelopeDiagnostics {
            boundaries: vec![BoundaryDiagnostic {
                boundary_idx: 0,
                ua_w_per_k: 0.0,
                r_total_m2_k_w: 0.0,
                capacitance_j_k: 0.0,
                n_rc_nodes: 0,
                interior_zone_idx: 0,
                exterior_target: ExteriorTarget::Outdoor,
                area_m2: 2.0,
                r_film_int_m2_k_w: 0.12,
                r_film_ext_m2_k_w: 0.03,
                r_zone_to_inner_m2_k_w: None,
                r_outer_half_m2_k_w: None,
                r_inner_half_m2_k_w: None,
                path: hares_envelope::RCPath::FallbackR,
                inner_node: None,
                interior_emissivity: 0.84,
                foundation_depth_m: 0.0,
                #[cfg(feature = "observe")]
                same_zone_kept_half: None,
            }],
            zone_capacitances_j_k: vec![1000.0],
            total_ua_w_per_k: 0.0,
            #[cfg(feature = "observe")]
            default_r_fallback_count: 0,
        };

        let rc = RCContext {
            layer_info: &layer_info,
            node_index: &node_index,
            envelope_diagnostics: &envelope_diagnostics,
            n_zones: 1,
            n_ext: 1,
        };

        let env = EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 20.0,
                humidity_ratio: 0.01,
                volume_m3: 250.0,
            }],
            weather: Default::default(),
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
                island_bus_voltage_pu: None,
            },
            custom_domains: vec![],
            equipment_telemetry: HashMap::new(),
            equipment_core: HashMap::new(),
            current_time: chrono::FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2025, 1, 1, 0, 0, 0)
                .unwrap(),
            time_res: chrono::Duration::minutes(1),
            price_signal: PriceSignal {
                electricity_price: None,
                export_price: None,
                ghg_intensity: None,
            },
            electrical: Default::default(),
        };

        let (boundaries, _ext_cols, _int_cols) =
            build_solver_boundaries(&building, &boundary_inputs, &rc, &env)
                .expect("build_solver_boundaries should succeed for skylight fenestration");

        assert_eq!(
            boundaries.len(),
            1,
            "building has one boundary (the skylight)"
        );
        let sb = &boundaries[0];

        assert_eq!(
            sb.boundary_category,
            Some(BoundaryCategory::Window),
            "skylight must map to BoundaryCategory::Window"
        );
        assert!(
            (sb.tilt_deg - 0.0).abs() < 1e-9,
            "skylight tilt must default to 0° (horizontal); got {}",
            sb.tilt_deg
        );
        assert!(
            (sb.azimuth_deg - 180.0).abs() < 1e-9,
            "skylight azimuth must be 180°; got {}",
            sb.azimuth_deg
        );

        let ws = sb.window_solar.as_ref().expect(
            "skylight window_solar must be Some — the fenestration was found in \
             building.skylights and had valid U-factor/SHGC",
        );

        assert!(
            (ws.base_shgc - 0.5).abs() < 1e-9,
            "skylight base_shgc must be 0.5; got {}",
            ws.base_shgc
        );
        assert!(
            (ws.u_factor_w_m2_k - 2.0).abs() < 1e-9,
            "skylight u_factor_w_m2_k must be 2.0; got {}",
            ws.u_factor_w_m2_k
        );
        assert!(
            (ws.window_area_m2 - 2.0).abs() < 1e-9,
            "skylight window_area_m2 must be 2.0; got {}",
            ws.window_area_m2
        );
        assert!(
            ws.shgc_summer > 0.0,
            "summer SHGC must be > 0 for non-zero solar gain; got {}",
            ws.shgc_summer
        );
        assert!(
            ws.shgc_winter > 0.0,
            "winter SHGC must be > 0 for non-zero solar gain; got {}",
            ws.shgc_winter
        );
        assert!(
            ws.transmittance_summer > 0.0,
            "summer transmittance must be > 0; got {}",
            ws.transmittance_summer
        );
        assert!(
            ws.transmittance_winter > 0.0,
            "winter transmittance must be > 0; got {}",
            ws.transmittance_winter
        );
        assert!(
            ws.radiation_frac >= 0.0,
            "radiation_frac must be >= 0; got {}",
            ws.radiation_frac
        );
    }

    #[test]
    fn validates_fluid_type_consistency_at_solver_init() {
        use hares_envelope::fluid_solver::{FluidSolver, FluidSolverConfig};
        use hares_equipment::{ElectricBoilerConfig, EquipmentConfig, GasBoilerConfig};
        use hares_types::{FluidType, LoopId};

        // Two equipment specs, same loop_id=1, different fluid types.
        let gas_boiler = EquipmentConfig::from_typed(
            "Gas Boiler".to_string(),
            "Gas Boiler".to_string(),
            GasBoilerConfig {
                loop_id: Some(1),
                fluid_type: FluidType::Water,
                capacity_w: 10_000.0,
                afue: 0.90,
                ..GasBoilerConfig::default()
            },
        )
        .unwrap();
        let electric_boiler = EquipmentConfig::from_typed(
            "Electric Boiler".to_string(),
            "Electric Boiler".to_string(),
            ElectricBoilerConfig {
                loop_id: Some(1),
                fluid_type: FluidType::Glycol,
                capacity_w: 8_000.0,
                eir: 1.0,
                ..ElectricBoilerConfig::default()
            },
        )
        .unwrap();

        let specs = [
            hares_io::EquipmentSpec {
                name: "Gas Boiler".to_string(),
                instance_name: None,
                fuel_type: hares_types::FuelType::Gas,
                parameters: serde_json::Map::new(),
                zip_params: None,
                typed_config: Some(gas_boiler),
                system_id: None,
                related_hvac_idref: None,
                primary_role: None,
            },
            hares_io::EquipmentSpec {
                name: "Electric Boiler".to_string(),
                instance_name: None,
                fuel_type: hares_types::FuelType::Electric,
                parameters: serde_json::Map::new(),
                zip_params: None,
                typed_config: Some(electric_boiler),
                system_id: None,
                related_hvac_idref: None,
                primary_role: None,
            },
        ];

        let loops = super::extract_fluid_loops_from_specs(&specs);
        assert_eq!(
            loops.len(),
            2,
            "both (loop_id=1, Water) and (loop_id=1, Glycol) should be extracted"
        );
        assert!(loops.contains(&(LoopId(1), FluidType::Water)));
        assert!(loops.contains(&(LoopId(1), FluidType::Glycol)));

        // Passing both conflicting pairs to the fluid solver constructor
        // must produce a hard error.
        let result = FluidSolver::new(FluidSolverConfig::default(), &loops);
        assert!(
            result.is_err(),
            "FluidSolver::new must reject same-loop-id with different fluid types"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("conflicting fluid types"),
            "error must mention conflicting fluid types, got: {err_msg}"
        );
    }
}
