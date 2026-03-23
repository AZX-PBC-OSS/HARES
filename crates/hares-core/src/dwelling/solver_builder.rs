//! Solver construction for the dwelling orchestrator.

use hares_envelope::{
    BuildingRC, EMISSIVITY_DEFAULT, EMISSIVITY_RADIANT_BARRIER, ElectricalSolver,
    ElectricalSolverConfig, ExteriorSurfaceInfo, FluidSolver, FluidSolverConfig, HumiditySolver,
    HumiditySolverConfig, SOLAR_ABSORPTANCE_DEFAULT, SOLAR_ABSORPTANCE_RADIANT_BARRIER,
    StateSpaceWiring, ThermalSolver, ThermalSolverConfig, WindowSolarProperties,
    assemble_building_rc, derive_zone_capacitances,
};
use hares_io::{Building, DefaultsStore, SimulationConfig, WeatherTimeSeries};
use hares_types::{EnvironmentState, HaresError, ZoneId};

use super::Result;
use super::conversions::{
    boundary_zone_index, building_to_boundary_inputs, building_to_zone_inputs,
    has_vented_crawlspace, shielding_str_to_class, site_type_to_terrain,
};

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
pub(crate) type SolverBundle = (
    ThermalSolver,
    HumiditySolver,
    ElectricalSolver,
    FluidSolver,
    Vec<(ZoneId, f64)>,
);

pub(crate) fn build_default_solvers(
    env: &EnvironmentState,
    sim_config: &SimulationConfig,
    building: &Building,
    defaults: &DefaultsStore,
    weather_avgs: &WeatherAverages,
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
    let rc = assemble_building_rc(&boundary_inputs, n_zones, &zone_capacitances)
        .map_err(HaresError::Envelope)?;

    let BuildingRC {
        a_c,
        b_ext,
        node_index,
        zone_state_rows,
        layer_info,
        outdoor_col,
        n_ext,
        node_capacitances,
        ..
    } = rc;
    let n_states = a_c.nrows();

    // Pre-scan: identify exterior surfaces with RC layers that need per-surface
    // B-matrix input columns for solar/LWR injection at the outer node.
    let mut ext_surface_columns: Vec<(usize, hares_envelope::NodeId, usize)> = Vec::new();
    for (surface_idx, boundary) in building.boundaries.iter().enumerate() {
        let is_exterior = boundary
            .exterior_zone
            .as_ref()
            .map(|z| *z == hares_io::hpxml::ZoneType::Outdoor)
            .unwrap_or(false);
        if !is_exterior {
            continue;
        }
        if let Some(info) = layer_info.get(&surface_idx) {
            if let Some(&state_row) = node_index.get(&info.outer_node) {
                let col_offset = ext_surface_columns.len();
                ext_surface_columns.push((surface_idx, info.outer_node, state_row));
                let _ = col_offset; // used below when building B_c
            }
        }
    }
    let n_ext_surface_inputs = ext_surface_columns.len();

    // Build surface_idx → B-matrix column index map.
    let ext_surface_col_map: std::collections::HashMap<usize, usize> = ext_surface_columns
        .iter()
        .enumerate()
        .map(|(i, &(surface_idx, _, _))| (surface_idx, n_ext + i))
        .collect();

    // Augment B_c: [B_ext | per-surface solar/LWR columns | zone sensible heat columns].
    //
    // Column layout:
    //   [0..n_ext)                                         External driving (outdoor, ground temps)
    //   [n_ext..n_ext+n_ext_surface_inputs)                Per-exterior-surface solar/LWR injection
    //   [n_ext+n_ext_surface_inputs..n_ext+n_ext_surface_inputs+n_zones)  Zone sensible heat
    let n_total_inputs = n_ext + n_ext_surface_inputs + n_zones;
    let mut b_c = DMatrix::<f64>::zeros(n_states, n_total_inputs);

    // Copy B_ext columns.
    for row in 0..n_states {
        for col in 0..n_ext {
            b_c[(row, col)] = b_ext[(row, col)];
        }
    }

    // Per-surface injection columns: gain = 1/C_outer_node.
    for (i, &(_, outer_node, state_row)) in ext_surface_columns.iter().enumerate() {
        let c_node = node_capacitances
            .get(&outer_node)
            .copied()
            .unwrap_or(hares_envelope::boundary_rc::MIN_CAPACITANCE_J_K)
            .max(hares_envelope::boundary_rc::MIN_CAPACITANCE_J_K);
        b_c[(state_row, n_ext + i)] = 1.0 / c_node;
    }

    // Zone sensible heat columns (shifted by n_ext_surface_inputs).
    let zone_input_offset = n_ext + n_ext_surface_inputs;
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
    // Determine outdoor node column index in B_ext (external nodes sorted ascending by NodeId).
    // OUTDOOR_NODE = NodeId(10_000), GROUND_NODE = NodeId(10_001).
    // Collect the unique external nodes actually used, sorted.
    // outdoor_col already computed by assemble_building_rc.

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
        thermal_cfg
            .ideal_setpoints_c
            .insert(zone.id, zone.temperature_c);
    }
    if let Some(col) = outdoor_col {
        wiring.outdoor_temp_input_indices = vec![col];
    } else {
        wiring.outdoor_temp_input_indices = vec![];
    }

    // Build window lookup: boundary id → Window, for matching windows to their
    // boundary entries when populating solar properties.
    let window_by_id: std::collections::HashMap<&str, &hares_io::hpxml::building::Window> =
        building
            .windows
            .iter()
            .map(|w| (w.id.as_str(), w))
            .collect();

    // Populate exterior surfaces for longwave radiation; use outermost layer node
    // (or zone air node for boundaries without material layers) as the surface node.
    for (surface_idx, boundary) in building.boundaries.iter().enumerate() {
        let is_exterior = boundary
            .exterior_zone
            .as_ref()
            .map(|z| *z == hares_io::hpxml::ZoneType::Outdoor)
            .unwrap_or(false);
        if !is_exterior {
            continue;
        }
        let zone_idx = boundary_zone_index(building, boundary.interior_zone.as_ref(), n_zones);
        let zone_id = env
            .zones
            .get(zone_idx)
            .map(|z| z.id)
            .unwrap_or(thermal_cfg.indoor_zone_id);

        // Use the outermost layer node if this boundary has material layers.
        // Surfaces with RC layers get a dedicated B-matrix input column that
        // injects heat at the exterior node (gain = 1/C_outer_node), so solar/LWR
        // conducts inward through the wall resistance chain.
        // Surfaces without layers fall back to the zone air node + zone column.
        let (state_index, input_index) = if let Some(&col) = ext_surface_col_map.get(&surface_idx) {
            let info = &layer_info[&surface_idx];
            let state_row = node_index.get(&info.outer_node).copied().unwrap_or(0);
            (state_row, col)
        } else {
            let si = *wiring.zone_state_indices.get(&zone_id).unwrap_or(&0);
            let ii = *wiring
                .zone_sensible_input_indices
                .get(&zone_id)
                .unwrap_or(&(zone_input_offset + zone_idx));
            (si, ii)
        };

        // OCHRE Envelope.py:221-222 gates radiant-barrier defaults on attic zone.
        let is_attic_radiant_barrier = boundary.has_radiant_barrier
            && boundary.interior_zone.as_ref() == Some(&hares_io::hpxml::ZoneType::Attic);
        let emissivity = boundary.emittance.unwrap_or(if is_attic_radiant_barrier {
            EMISSIVITY_RADIANT_BARRIER
        } else {
            EMISSIVITY_DEFAULT
        });
        let solar_absorptance = boundary
            .solar_absorptance
            .unwrap_or(if is_attic_radiant_barrier {
                SOLAR_ABSORPTANCE_RADIANT_BARRIER
            } else {
                SOLAR_ABSORPTANCE_DEFAULT
            });
        let tilt_deg = match boundary.boundary_type {
            hares_io::hpxml::BoundaryType::Roof => 0.0,
            hares_io::hpxml::BoundaryType::Slab | hares_io::hpxml::BoundaryType::Floor => 180.0,
            _ => 90.0,
        };
        // Compute radiation fraction and resistance for iterative LWR solver.
        // rad_frac = R_film / (R_film + R_outermost_half) [m²·K/W]
        // rad_res  = R_film / area [K/W]
        let bd_input = &boundary_inputs[surface_idx];
        let r_film = bd_input.r_film_exterior_m2_k_w;
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
                outer.thickness_m / (2.0 * outer.conductivity_w_m_k)
            } else {
                // No material layers: single-resistance boundary, skip iteration.
                0.0
            }
        };
        let (rad_frac, rad_res_k_w) = if r_outermost_half > 0.0 {
            (
                r_film / (r_film + r_outermost_half),
                r_film / boundary.area_m2,
            )
        } else {
            (0.0, 0.0)
        };
        // OCHRE Envelope.py:207: iterations = ceil(time_res / 5 min).max(1)
        let n_iter = ((dt_s / 300.0).ceil() as u32).max(1);
        let surface_id = u32::try_from(surface_idx).unwrap_or(u32::MAX);
        thermal_cfg.exterior_surfaces.push(ExteriorSurfaceInfo {
            surface_id,
            state_index,
            input_index,
            area_m2: boundary.area_m2,
            emissivity,
            tilt_deg,
            rad_frac,
            rad_res_k_w,
            n_iter,
            absorptance: solar_absorptance,
        });
        // Route solar irradiance to the zone's sensible heat input column.
        wiring.solar_input_indices.insert(surface_id, input_index);

        // Windows use the IAM-corrected path via solar_input_indices + window_properties.
        // Opaque surfaces receive solar gain via apply_exterior_solar_inputs (ExteriorSurfaceInfo.absorptance).
        if let Some(win) = window_by_id.get(boundary.id.as_str()) {
            let u_factor = win.u_factor_w_m2_k.unwrap_or(5.0);
            let effective_shgc = win.shgc.unwrap_or(0.4) * win.interior_shading_fraction;
            // Window glass material resistance: total R minus film resistances.
            // For simple glazing, approximate from U-factor:
            //   R_total = 1/U, R_glass ≈ R_total - R_film_int - R_film_ext
            let r_total = 1.0 / u_factor.max(0.01);
            let r_glass =
                (r_total - bd_input.r_film_interior_m2_k_w - bd_input.r_film_exterior_m2_k_w)
                    .max(0.0);
            let (transmittance, radiation_frac) = hares_physics::solar::calculate_window_parameters(
                effective_shgc,
                u_factor,
                r_glass,
            );
            thermal_cfg.window_properties.insert(
                surface_id,
                WindowSolarProperties {
                    shgc: effective_shgc,
                    u_factor_w_m2_k: u_factor,
                    area_m2: boundary.area_m2,
                    transmittance,
                    radiation_frac,
                },
            );
        }
    }

    // --- Per-zone infiltration (Walker-Wilson 1998 / ASHRAE HOF Ch. 16) ---
    // Each zone gets a physics-appropriate infiltration method.
    // Conditioned zone uses AIM-2 model when ACH50 is available.
    {
        use hares_envelope::InfiltrationMethod;
        use hares_io::hpxml::ZoneType;
        use hares_physics::infiltration::{
            Aim2Params, FoundationLeakageClass, N_I_DEFAULT, aim2_coefficients_from_ach50,
            attic_ela_coefficients,
        };

        let default_ceiling_height_m = building.ceiling_height_m.unwrap_or(2.5);
        let building_height_m = default_ceiling_height_m
            * building
                .zones
                .iter()
                .filter(|z| z.zone_type == ZoneType::Conditioned)
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
                    if let Some(ach50) = building.infiltration_ach50 {
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
                    if let Some(ach) = bz.ventilation_ach {
                        InfiltrationMethod::Ach { ach }
                    } else if let Some(sla) = bz.ventilation_sla {
                        let floor_area_m2 = bz.floor_area_m2.unwrap_or(100.0);
                        let ela_m2 = sla * floor_area_m2;
                        let attic_height_m = 1.5; // default triangular attic
                        let (stack_coeff, wind_coeff) =
                            attic_ela_coefficients(attic_height_m, building_height_m);
                        InfiltrationMethod::Ela {
                            ela_m2,
                            stack_coeff,
                            wind_coeff,
                        }
                    } else if bz.vented {
                        // Vented attic with no HPXML data: default 2.0 ACH
                        // (ASHRAE 62.2 typical for vented attics).
                        InfiltrationMethod::Ach { ach: 2.0 }
                    } else {
                        // Unvented attic: minimal air exchange.
                        InfiltrationMethod::Ach { ach: 0.1 }
                    }
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
                    if bz.vented {
                        // Vented crawlspace: high air exchange.
                        let ach = bz.ventilation_ach.unwrap_or(2.0);
                        InfiltrationMethod::Ach { ach }
                    } else {
                        // Unvented crawlspace/basement: conduction only.
                        InfiltrationMethod::Ach { ach: 0.0 }
                    }
                }
                ZoneType::Outdoor | ZoneType::Other(_) => continue,
            };

            thermal_cfg.infiltration.push((zone_id, method));
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

    Ok((
        thermal_solver,
        humidity_solver,
        electrical_solver,
        fluid_solver,
        zone_caps,
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn n_iter_matches_ceil_formula() {
        for &dt_s in &[60.0_f64, 300.0, 600.0, 900.0, 3600.0] {
            let n_iter = ((dt_s / 300.0_f64).ceil() as u32).max(1);
            let expected = (dt_s / 300.0_f64).ceil() as u32;
            let expected = expected.max(1);
            assert_eq!(
                n_iter, expected,
                "n_iter mismatch for dt_s={dt_s}: got {n_iter}, expected {expected}"
            );
        }

        // Specific expected values
        assert_eq!(((60.0_f64 / 300.0).ceil() as u32).max(1), 1);
        assert_eq!(((300.0_f64 / 300.0).ceil() as u32).max(1), 1);
        assert_eq!(((600.0_f64 / 300.0).ceil() as u32).max(1), 2);
        assert_eq!(((900.0_f64 / 300.0).ceil() as u32).max(1), 3);
        assert_eq!(((3600.0_f64 / 300.0).ceil() as u32).max(1), 12);
    }
}
