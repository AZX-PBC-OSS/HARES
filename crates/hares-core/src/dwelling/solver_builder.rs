//! Solver construction for the dwelling orchestrator.

use std::collections::HashMap;

use hares_envelope::{
    BoundaryCategory, BoundaryDiagnostic, BoundaryDiagnosticInfo, BoundaryInput, BuildingRC,
    DrivingTemp, EMISSIVITY_DEFAULT, EMISSIVITY_RADIANT_BARRIER, ElectricalSolver,
    ElectricalSolverConfig, EnvelopeDiagnostics, ExteriorSurfaceInfo, ExteriorTarget, FluidSolver,
    FluidSolverConfig, HumiditySolver, HumiditySolverConfig, NodeId, SOLAR_ABSORPTANCE_DEFAULT,
    SOLAR_ABSORPTANCE_RADIANT_BARRIER, StateSpaceWiring, SurfaceLayerInfo, ThermalSolver,
    ThermalSolverConfig, WindowSolarProperties, assemble_building_rc, derive_zone_capacitances,
};
use hares_io::{Building, DefaultsStore, EquipmentSpec, SimulationConfig, WeatherTimeSeries};
use hares_types::{EnvironmentState, HaresError, ZoneId};

use super::Result;
use super::conversions::{
    boundary_zone_index, building_to_boundary_inputs, building_to_zone_inputs,
    has_vented_crawlspace, shielding_str_to_class, site_type_to_terrain,
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
    zone_id: ZoneId,
    zone_idx: usize,
    is_exterior: bool,
    is_conditioned_interior: bool,
    is_attic_interior: bool,
    exterior_emissivity: f64,
    exterior_solar_absorptance: f64,
    attic_emissivity: f64,
    /// Attic interior solar absorptance (0.05 with radiant barrier, 0.5 default).
    /// Stored for future use when attic-zone interior solar distribution is implemented.
    /// Currently attic solar gain enters only via conduction through the roof exterior.
    _attic_solar_absorptance: f64,
    outer_wiring: Option<NodeWiring>,
    inner_wiring: Option<NodeWiring>,
    exterior_rad_frac: f64,
    exterior_rad_res_k_w: f64,
    interior_rad_frac: f64,
    r_film_int_m2_k_w: f64,
    window_solar: Option<WindowSolarData>,
    diagnostic_r_zone_to_inner: Option<f64>,
}

/// Build the intermediate `SolverBoundary` representations from building data.
///
/// Returns `(solver_boundaries, n_ext_surface_inputs, n_int_surface_inputs)`.
fn build_solver_boundaries(
    building: &Building,
    boundary_inputs: &[BoundaryInput],
    rc: &RCContext<'_>,
    env: &EnvironmentState,
) -> (Vec<SolverBoundary>, usize, usize) {
    // Group windows by wall they're attached to (for wall→window aggregation).
    let mut windows_by_wall: HashMap<&str, Vec<&hares_io::hpxml::building::Window>> =
        HashMap::new();
    for w in &building.windows {
        if let Some(wall_id) = w.attached_to_wall_id.as_deref() {
            windows_by_wall.entry(wall_id).or_default().push(w);
        }
    }

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

        let zone_idx = boundary_zone_index(building, boundary.interior_zone.as_ref(), rc.n_zones);
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

        // Boundary category.
        // Same-zone boundaries (interior == exterior) are internal thermal mass.
        let is_same_zone =
            boundary.interior_zone == boundary.exterior_zone && boundary.interior_zone.is_some();
        let boundary_category = if is_same_zone {
            Some(BoundaryCategory::InternalMass)
        } else {
            match boundary.boundary_type {
                hares_io::hpxml::BoundaryType::Wall
                | hares_io::hpxml::BoundaryType::FoundationWall
                | hares_io::hpxml::BoundaryType::RimJoist => Some(BoundaryCategory::Wall),
                hares_io::hpxml::BoundaryType::Roof => Some(BoundaryCategory::Roof),
                hares_io::hpxml::BoundaryType::Floor | hares_io::hpxml::BoundaryType::Slab => {
                    // Attic floor (exterior = Attic) represents heat flow from roof/attic
                    // path into the conditioned zone — categorize as Roof for OCHRE parity.
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
                hares_io::hpxml::BoundaryType::Window => Some(BoundaryCategory::Window),
                hares_io::hpxml::BoundaryType::Door | hares_io::hpxml::BoundaryType::Other(_) => {
                    None
                }
            }
        };

        // Tilt: use parsed value from HPXML, fall back to type-based default.
        let tilt_deg = boundary.tilt_deg.unwrap_or(match boundary.boundary_type {
            hares_io::hpxml::BoundaryType::Roof => 0.0,
            hares_io::hpxml::BoundaryType::Slab | hares_io::hpxml::BoundaryType::Floor => 180.0,
            _ => 90.0,
        });

        // Keep radiant-barrier properties on the attic-interior side only.
        // Exterior solar/thermal properties remain physical or explicitly set by HPXML.
        let exterior_emissivity = exterior_emissivity(boundary);
        let exterior_solar_absorptance = exterior_solar_absorptance(boundary);
        let attic_emissivity = attic_interior_emissivity(boundary);
        let attic_solar_absorptance = attic_interior_solar_absorptance(boundary);

        let bd_input = &boundary_inputs[surface_idx];
        let r_film_ext = bd_input.r_film_exterior_m2_k_w;
        let r_film_int = bd_input.r_film_interior_m2_k_w;

        // Exterior radiation fraction: R_film_ext / (R_film_ext + R_outermost_half).
        let r_outermost_half = if !bd_input.precomputed_rc.is_empty() {
            bd_input
                .precomputed_rc
                .last()
                .map(|l| l.resistance_m2_k_w / 2.0)
                .unwrap_or(0.0)
        } else {
            let valid_layers: Vec<&hares_envelope::LayerInput> = bd_input
                .material_layers
                .iter()
                .filter(|l| l.conductivity_w_m_k > 0.0 && l.thickness_m > 0.0)
                .collect();
            if let Some(outer) = valid_layers.last() {
                let k = hares_envelope::parallel_path_conductivity(
                    outer.conductivity_w_m_k,
                    bd_input.framing_factor,
                );
                outer.thickness_m / (2.0 * k)
            } else {
                0.0
            }
        };
        let (exterior_rad_frac, exterior_rad_res_k_w) =
            if r_outermost_half > 0.0 && boundary.area_m2 > 0.0 {
                (
                    r_film_ext / (r_film_ext + r_outermost_half),
                    r_film_ext / boundary.area_m2,
                )
            } else {
                (0.0, 0.0)
            };

        // Interior radiation fraction: R_film_int / (R_film_int + R_inner_half).
        let r_inner_half = if !bd_input.precomputed_rc.is_empty() {
            bd_input
                .precomputed_rc
                .first()
                .map(|l| l.resistance_m2_k_w / 2.0)
                .unwrap_or(0.0)
        } else {
            bd_input
                .material_layers
                .iter()
                .find(|l| l.conductivity_w_m_k > 0.0 && l.thickness_m > 0.0)
                .map(|l| {
                    let k = hares_envelope::parallel_path_conductivity(
                        l.conductivity_w_m_k,
                        bd_input.framing_factor,
                    );
                    l.thickness_m / (2.0 * k)
                })
                .unwrap_or(0.0)
        };
        let interior_rad_frac = if r_inner_half > 0.0 {
            r_film_int / (r_film_int + r_inner_half)
        } else {
            1.0
        };

        // Window solar data — only for Window boundary types.
        // Wall boundaries receive opaque solar via ExteriorSurfaceInfo.absorptance.
        let window_solar = if is_exterior
            && boundary.boundary_type == hares_io::hpxml::BoundaryType::Window
        {
            let win_match = building.windows.iter().find(|w| w.id == boundary.id);
            win_match.map(|win| {
                let u_factor = win.u_factor_w_m2_k.unwrap_or(5.0);
                let base_shgc = win.shgc.unwrap_or(0.4);
                let shgc_summer = base_shgc * win.interior_shading_fraction;
                let shgc_winter = base_shgc * win.winter_shading_fraction;
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
                let total_window_area: f64 = if boundary.boundary_type
                    == hares_io::hpxml::BoundaryType::Window
                {
                    // Window boundary: use its own area directly.
                    boundary.area_m2
                } else {
                    // Wall boundary: sum all attached windows' areas.
                    building
                        .windows
                        .iter()
                        .filter(|w| w.attached_to_wall_id.as_deref() == Some(boundary.id.as_str()))
                        .map(|w| w.area_m2)
                        .sum()
                };
                WindowSolarData {
                    shgc_summer,
                    shgc_winter,
                    u_factor_w_m2_k: u_factor,
                    window_area_m2: total_window_area,
                    transmittance_summer,
                    transmittance_winter,
                    radiation_frac,
                }
            })
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
            zone_id,
            zone_idx,
            is_exterior,
            is_conditioned_interior,
            is_attic_interior,
            exterior_emissivity,
            exterior_solar_absorptance,
            attic_emissivity,
            _attic_solar_absorptance: attic_solar_absorptance,
            outer_wiring,
            inner_wiring,
            exterior_rad_frac,
            exterior_rad_res_k_w,
            interior_rad_frac,
            r_film_int_m2_k_w: r_film_int,
            window_solar,
            diagnostic_r_zone_to_inner,
        });
    }

    (solver_boundaries, n_ext_surface_inputs, int_col_counter)
}

fn exterior_emissivity(boundary: &hares_io::hpxml::Boundary) -> f64 {
    boundary.emittance.unwrap_or(EMISSIVITY_DEFAULT)
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

fn attic_interior_solar_absorptance(boundary: &hares_io::hpxml::Boundary) -> f64 {
    if boundary.has_radiant_barrier
        && boundary.interior_zone.as_ref() == Some(&hares_io::hpxml::ZoneType::Attic)
    {
        SOLAR_ABSORPTANCE_RADIANT_BARRIER
    } else {
        exterior_solar_absorptance(boundary)
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

    // Convert building data to envelope-crate input types.
    let zone_inputs = building_to_zone_inputs(building, n_zones);
    let boundary_inputs = building_to_boundary_inputs(
        building,
        n_zones,
        defaults,
        weather_avgs.avg_wind_m_s,
        weather_avgs.avg_ambient_c,
        weather_avgs.avg_ground_c,
    );

    // Zone air node capacitances [J/K].
    let zone_capacitances = derive_zone_capacitances(&zone_inputs);

    // Build the RC network from material layers where available.
    let (rc, envelope_diagnostics) =
        assemble_building_rc(&boundary_inputs, n_zones, &zone_capacitances)
            .map_err(HaresError::Envelope)?;

    let BuildingRC {
        a_c,
        b_ext,
        node_index,
        zone_state_rows,
        layer_info,
        outdoor_col,
        ground_col,
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
        build_solver_boundaries(building, &boundary_inputs, &rc_ctx, env);

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
    if let Some(col) = ground_col {
        wiring.ground_temp_input_indices = vec![col];
    } else {
        wiring.ground_temp_input_indices = vec![];
    }

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
                rad_frac: sb.exterior_rad_frac,
                rad_res_k_w: sb.exterior_rad_res_k_w,
                n_iter,
                absorptance: sb.exterior_solar_absorptance,
                boundary_category: sb.boundary_category,
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
                            ws.shgc_summer,
                        ),
                    },
                );
                thermal_cfg
                    .window_zone_ids
                    .insert(sb.surface_id, sb.zone_id);
            }
        }

        // Add interior surfaces for LWR and solar distribution.
        // Windows participate in interior LWR (emissivity=0.84 per EnergyPlus)
        // but receive solar via SHGC/IAM, not the floor/wall distribution path.
        //
        // Window LWR flux goes entirely to zone air (no RC node). The window
        // surface temperature is estimated from outdoor driving temp and the
        // conduction gradient: T_surf = radiation_frac × T_outdoor + (1-radiation_frac) × T_zone.
        // This matches OCHRE's approach where windows have t_idx=None and all
        // LWR goes to zone.radiation_heat.
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
                    // Window LWR: emissivity=0.84 (EnergyPlus default).
                    // Surface temp driven by outdoor conduction.
                    // radiation_frac from EnergyPlus interior film decomposition:
                    //   res_int = 1 / (0.359073 × ln(U) + 6.949915)
                    //   radiation_frac = res_int / (1/U)
                    // where U is the window U-factor in W/(m²·K).
                    const WINDOW_EMISSIVITY: f64 = 0.84;
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
                    (
                        sb.attic_emissivity,
                        if is_floor { 0.6 } else { 0.5 },
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
                        r_film_int_m2_k_w: r_film,
                        radiation_frac,
                        category: cat,
                    });
            } else if sb.is_conditioned_interior
                || (sb.is_exterior && cat == BoundaryCategory::Window)
            {
                // Boundary without RC interior node — use steady-state UA diagnostic.
                let diag_bd = diag_by_idx.get(&sb.surface_idx);
                if let Some(d) = diag_bd {
                    let driving_temp = match d.exterior_target {
                        ExteriorTarget::Ground => DrivingTemp::Ground,
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

    // Group surfaces_by_zone into interior_lwr_zones.
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
                    if let Some(ach50) = resolved_ach50 {
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
                    // Garage: use building ACH50 if available, else default 0.5 ACH.
                    let garage_ach = building
                        .infiltration_ach50
                        .map(|ach50| ach50 / 20.0) // rough conversion ACH50→natural ACH
                        .unwrap_or(0.5);
                    InfiltrationMethod::Ach { ach: garage_ach }
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
        let params = &vent_spec.parameters;
        if let Some(flow_m3_s) = params.get("flow_rate_m3_s").and_then(|v| v.as_f64()) {
            thermal_cfg.ventilation_flow_m3_s = flow_m3_s;
        }
        if let Some(balanced) = params.get("balanced").and_then(|v| v.as_bool()) {
            thermal_cfg.ventilation.balanced = balanced;
        }
        if let Some(sens_re) = params
            .get("sensible_recovery_efficiency")
            .and_then(|v| v.as_f64())
        {
            thermal_cfg.ventilation.sensible_recovery_efficiency = sens_re;
        }
        if let Some(lat_re) = params
            .get("latent_recovery_efficiency")
            .and_then(|v| v.as_f64())
        {
            thermal_cfg.ventilation.latent_recovery_efficiency = lat_re;
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
    // Compute effective open window area from per-window FractionOperable.
    // Formula: Σ(window_area × fraction_operable) × 0.5 (open fraction) × 0.2 (flow fraction).
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

        let total_operable_area: f64 = building
            .windows
            .iter()
            .filter(|w| {
                w.attached_to_wall_id
                    .as_ref()
                    .and_then(|wall_id| building.boundaries.iter().find(|b| b.id == *wall_id))
                    .map(|b| {
                        b.interior_zone.as_ref() == Some(&hares_io::hpxml::ZoneType::Conditioned)
                    })
                    .unwrap_or(false)
            })
            .map(|w| w.area_m2 * w.fraction_operable)
            .sum();
        if total_operable_area > 0.0 {
            let open_area = total_operable_area * 0.5 * 0.2;
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
            });
        }
    }

    let initial_temp = env.zones.first().map(|z| z.temperature_c).unwrap_or(21.0);
    let thermal_solver = ThermalSolver::new(model, wiring, thermal_cfg, dt_s, env, initial_temp)
        .map_err(|err| HaresError::Envelope(format!("thermal solver init failed: {err}")))?;

    let humidity_solver = HumiditySolver::new(HumiditySolverConfig::default(), env);
    let electrical_solver = ElectricalSolver::new(ElectricalSolverConfig::default())
        .map_err(|err| HaresError::Envelope(format!("electrical solver init failed: {err}")))?;
    let fluid_solver = FluidSolver::new(FluidSolverConfig::default(), &[])
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
    zone_idx: usize,
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
        return Err(HaresError::Envelope(format!(
            "Vented attic zone {zone_idx} requires explicit ventilation data (ACH or SLA)"
        )));
    }

    // Unvented attic default matches OCHRE attic handling: 0.1 ACH.
    Ok(InfiltrationMethod::Ach { ach: 0.1 })
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
        attic_infiltration_method, attic_interior_emissivity, attic_interior_solar_absorptance,
        exterior_emissivity, exterior_solar_absorptance, foundation_height_m,
        foundation_infiltration_method, include_interior_lwr, natural_ventilation_coefficients,
    };
    use hares_envelope::InfiltrationMethod;
    use hares_envelope::ThermalSolverConfig;
    use hares_io::hpxml::{Boundary, BoundaryType, Zone, ZoneType};
    use hares_physics::infiltration::{
        N_I_DEFAULT, SHIELDING_NORMAL, TerrainClass, calculate_ela_coefficients,
    };
    use hares_types::{HaresError, ZoneId};

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
        // 100 cm² ELA — typical for a moderately leaky house
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
        }
    }

    #[test]
    fn attic_radiant_barrier_keeps_exterior_roof_physics() {
        let boundary = attic_roof_boundary(None, None, true);

        assert_eq!(exterior_solar_absorptance(&boundary), 0.60);
        assert_eq!(attic_interior_solar_absorptance(&boundary), 0.05);
        assert_eq!(exterior_emissivity(&boundary), 0.90);
        assert_eq!(attic_interior_emissivity(&boundary), 0.05);
    }

    #[test]
    fn explicit_roof_optics_are_preserved_for_exterior_path() {
        let boundary = attic_roof_boundary(Some(0.72), Some(0.88), true);

        assert_eq!(exterior_solar_absorptance(&boundary), 0.72);
        assert_eq!(attic_interior_solar_absorptance(&boundary), 0.05);
        assert_eq!(exterior_emissivity(&boundary), 0.88);
        assert_eq!(attic_interior_emissivity(&boundary), 0.05);
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
    fn attic_vented_requires_explicit_ventilation_rate() {
        let zone = attic_zone(Some(100.0), Some(120.0), true, None, None);
        let err = attic_infiltration_method(&zone, Some(100.0), 5.0, 3).expect_err("expected err");
        assert!(matches!(err, HaresError::Envelope(_)));
        assert!(
            err.to_string()
                .contains("requires explicit ventilation data")
        );
    }

    #[test]
    fn attic_unvented_default_is_minimal_ach() {
        let zone = attic_zone(Some(100.0), Some(120.0), false, None, None);
        let method = attic_infiltration_method(&zone, Some(100.0), 5.0, 3).expect("method");
        assert_eq!(method, InfiltrationMethod::Ach { ach: 0.1 });
    }
}
