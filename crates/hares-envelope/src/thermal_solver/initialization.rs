//! Steady-state initialisation for the thermal solver.

use nalgebra::{DMatrix, DVector};

use crate::state_space::StateSpaceModel;

use super::{Result, StateSpaceWiring, ThermalSolverError};
use hares_types::{EnvironmentState, ZoneId};

/// Compute steady-state temperatures by solving `x = A_d x + B_d u` for `x`,
/// with the conditioned zone air node(s) pinned to their initial temperature.
///
/// Unconditioned zones (attic, garage, foundation) are left free so their
/// steady-state temperature is determined by conduction through surrounding
/// boundaries. This mirrors OCHRE's `Envelope.initialize_state()` at
/// `vendors/OCHRE/ochre/Models/Envelope.py:1012-1033`, which only removes the
/// `T_LIV` column from the reduced system.
///
/// Pinning unconditioned zones to `env.zones[i].temperature_c` (typically set
/// to outdoor temperature at startup) forces an artificially cold boundary on
/// the conditioned zone's ceiling/garage walls, producing an inflated step-0
/// ideal-capacity back-solve (ASHRAE Fundamentals 2021 Ch. 18, steady-state
/// conduction through multi-zone envelopes).
///
/// `pinned_zones` is the set of zone state indices to fix as boundary
/// conditions (typically the single conditioned zone's state index).
pub(crate) fn initialize_steady_state(
    model: &StateSpaceModel,
    wiring: &StateSpaceWiring,
    env: &EnvironmentState,
    indoor_temp_c: f64,
    pinned_zones: &[ZoneId],
) -> Result<DVector<f64>> {
    let n = model.state_dim();
    let m = model.input_dim();

    // Build u_initial: outdoor temps + indoor temps (no HVAC heat, no solar).
    let mut u = DVector::<f64>::zeros(m);
    for &idx in &wiring.outdoor_temp_input_indices {
        if idx < m {
            u[idx] = env.weather.outdoor_temp_c;
        }
    }
    for (&idx, &depth_m) in wiring
        .ground_temp_input_indices
        .iter()
        .zip(wiring.ground_temp_input_depths_m.iter())
    {
        if idx < m {
            u[idx] = hares_physics::ground::kusuda_achenbach_temp(
                depth_m,
                env.weather.day_of_year,
                env.weather.ground_t_mean_c,
                env.weather.ground_t_amplitude_c,
                env.weather.ground_phase_day,
                hares_physics::ground::DEFAULT_SOIL_DIFFUSIVITY_M2_PER_DAY,
            );
        }
    }
    for &idx in &wiring.indoor_temp_input_indices {
        if idx < m {
            u[idx] = indoor_temp_c;
        }
    }

    // Verify all pinned zones are registered in zone_state_indices.
    // A missing indoor zone ID is a configuration error that must be surfaced
    // loudly — silently falling back to a flat temperature vector hides the
    // misconfiguration and produces physically wrong initial conditions.
    // OCHRE raises ValueError for missing zone state names; EnergyPlus issues
    // Severe/Fatal errors for missing zone references. HARES must not silently
    // substitute a fallback where those implementations error.
    for zone_id in pinned_zones {
        if !wiring.zone_state_indices.contains_key(zone_id) {
            let registered: Vec<ZoneId> = wiring.zone_state_indices.keys().copied().collect();
            return Err(ThermalSolverError::IndoorZoneIdNotRegistered {
                id: *zone_id,
                registered,
            });
        }
    }

    // Verify all pinned zone indices are within the state dimension.
    // After the pre-check above guarantees every zone is registered in
    // zone_state_indices, an index that exceeds the state dimension represents
    // internally inconsistent wiring — a zone pointing beyond the state vector.
    // This must be surfaced as an error rather than silently dropping the zone,
    // which would cause the initial state to be computed without the pinned zone
    // temperature and produce physically wrong initial conditions.
    for zone_id in pinned_zones {
        // SAFETY: the pre-check above guarantees every pinned_zone is
        // present in zone_state_indices.
        let idx = wiring.zone_state_indices[zone_id];
        if idx >= n {
            return Err(ThermalSolverError::ZoneStateIndexOutOfBounds {
                zone_id: *zone_id,
                index: idx,
                state_dim: n,
            });
        }
    }

    // Verify all pinned zones are present in env.zones.
    // A zone registered in zone_state_indices but absent from env.zones
    // represents a wiring/environment consistency issue. Silently substituting
    // indoor_temp_c hides the misconfiguration and produces physically wrong
    // initial conditions — the solver cannot know the correct temperature.
    for zone_id in pinned_zones {
        if !env.zones.iter().any(|z| z.id == *zone_id) {
            let present_zones: Vec<ZoneId> = env.zones.iter().map(|z| z.id).collect();
            return Err(ThermalSolverError::ZoneNotInEnvironment {
                zone_id: *zone_id,
                present_zones,
            });
        }
    }

    // Verify every pinned zone carries enough thermal capacitance for the
    // state-pinning solve. A zone air node with negligible capacitance (massless
    // node) creates a kinematic constraint with no energy storage to absorb it;
    // the reduced (I − A_d) or −A_c matrix becomes singular.
    //
    // The standard construction path clamps zone capacitance to ≥ 1,000 J/K
    // (MIN_CAPACITANCE_J_K in boundary_rc.rs), so this guard fires only when a
    // nonstandard construction path, a model format that allows zero-mass zones,
    // or a refactoring that separates the capacitance clamp from the construction
    // path reaches initialization. OCHRE uses np.linalg.inv(A) directly —
    // NumPy raises LinAlgError on singularity rather than silently substituting
    // uniform temperatures.
    const ZONE_PINNING_MIN_CAPACITANCE_J_K: f64 = 1e-6;

    #[cfg(any(debug_assertions, feature = "observe_detailed"))]
    {
        let c_zone_diagnostics: Vec<(ZoneId, f64)> = pinned_zones
            .iter()
            .map(|zone_id| {
                let c_zone = wiring.c_zone_j_k.get(zone_id).copied().unwrap_or(0.0);
                (*zone_id, c_zone)
            })
            .collect();
        tracing::debug!(
            pinned_zone_capacitances_j_k = ?c_zone_diagnostics,
            "pinned zone capacitances used for steady-state initialization"
        );
    }

    // Guard: reject zones whose capacitance is too small for the state-pinning
    // solve before we inspect the matrices. This fires as a returned error
    // (not a panic) so callers can handle the condition at the construction
    // boundary without crashing the entire process in debug builds.
    for zone_id in pinned_zones {
        if let Some(&c_zone) = wiring.c_zone_j_k.get(zone_id) {
            if c_zone < ZONE_PINNING_MIN_CAPACITANCE_J_K {
                return Err(ThermalSolverError::SingularInitialization {
                    zone_id: *zone_id,
                    capacitance_j_k: c_zone,
                });
            }
        }
    }

    // Invariant check: in debug/test builds and when check_invariants is
    // enabled, assert every pinned zone has at least 1 J/K. Zones that reached
    // this point have already passed the guard above (c_zone >= 1e-6 J/K), but
    // 1 J/K is the absolute physical minimum for a meaningful thermal
    // capacitance. Values in [1e-6, 1.0) represent a suspicious path that
    // passed the soft error guard but indicates a latent construction or
    // configuration issue that should be investigated.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        for zone_id in pinned_zones {
            if let Some(&c_zone) = wiring.c_zone_j_k.get(zone_id) {
                assert!(
                    c_zone >= 1.0,
                    "zone {zone_id:?} capacitance {c_zone:.3e} J/K below absolute minimum 1 J/K \
                     for state pinning"
                );
            }
        }
    }

    // Collect zone state indices to fix as boundary conditions.
    // Only conditioned zones listed in `pinned_zones` are pinned; unconditioned
    // zones (attic, garage, foundation) are left free so their steady-state
    // temperature is determined by conduction through the surrounding envelope.
    // Sort descending so we can remove rows/cols without invalidating earlier indices.
    let mut zone_fixes: Vec<(usize, f64)> = pinned_zones
        .iter()
        .map(|zone_id| {
            // SAFETY: pre-checks above guarantee every pinned zone is present
            // in zone_state_indices, its index is within bounds, and it is
            // present in env.zones.
            let idx = *wiring
                .zone_state_indices
                .get(zone_id)
                .expect("verified above");
            let t = env
                .zones
                .iter()
                .find(|z| z.id == *zone_id)
                .map(|z| z.temperature_c)
                .expect("pre-check above guarantees zone is in env.zones");
            (idx, t)
        })
        .collect();
    zone_fixes.sort_by_key(|b| std::cmp::Reverse(b.0));
    zone_fixes.dedup_by_key(|f| f.0);

    if zone_fixes.is_empty() {
        return match model.steady_state(&u) {
            Some(x) => Ok(x),
            None => Ok(DVector::from_element(model.state_dim(), indoor_temp_c)),
        };
    }

    // Partition the system: remove zone states from the state vector and
    // move their coupling into the RHS as fixed boundary conditions.
    //
    // Continuous path (from_continuous): solve A_c·x = -B_c·u with zone states pinned.
    //   Partitioned: -A_c_reduced·x_reduced = B_c·u + A_c[:,j]·T_j for each fixed j.
    //
    // Discrete path (from_discrete): solve (I - A_d)·x = B_d·u with zone states pinned.
    //   Partitioned: (I - A_d_reduced)·x_reduced = B_d·u + A_d[:,j]·T_j for each fixed j.
    let use_continuous = model.a_c().is_some() && model.b_c().is_some();
    let (mut a_reduced, mut b_rhs) = if let (Some(a_c), Some(b_c)) = (model.a_c(), model.b_c()) {
        (a_c.clone(), b_c * &u)
    } else {
        (model.n_mat().clone(), model.b_eff() * &u)
    };

    // Add coupling from fixed zone states to RHS, then remove those rows/cols.
    for &(j, t_fixed) in &zone_fixes {
        let col_j = a_reduced.column(j).into_owned();
        b_rhs += &col_j * t_fixed;
    }

    // Remove rows and columns for fixed states (indices are sorted descending).
    for &(j, _) in &zone_fixes {
        a_reduced = a_reduced.remove_row(j).remove_column(j);
        b_rhs = b_rhs.remove_row(j);
    }

    let n_reduced = a_reduced.nrows();
    if n_reduced == 0 {
        let mut x_full = DVector::zeros(0);
        for &(j, t_fixed) in zone_fixes.iter().rev() {
            x_full = x_full.insert_row(j, t_fixed);
        }
        return Ok(x_full);
    }

    let x_reduced = if use_continuous {
        // Continuous: 0 = A_c·x + B_c·u → -A_c·x = B_c·u (b_rhs already = B_c·u + couplings)
        let neg_a = -&a_reduced;
        match neg_a.try_inverse() {
            Some(inv) => inv * b_rhs,
            None => {
                return Ok(DVector::from_element(n, indoor_temp_c));
            }
        }
    } else {
        // Discrete: (I - N)·x = B_eff·u
        let eye = DMatrix::<f64>::identity(n_reduced, n_reduced);
        let lhs = eye - a_reduced;
        match lhs.try_inverse() {
            Some(inv) => inv * b_rhs,
            None => {
                return Ok(DVector::from_element(n, indoor_temp_c));
            }
        }
    };

    // Re-insert zone temperatures at their fixed values.
    // zone_fixes is sorted descending, so insert in ascending order.
    let mut x_full = x_reduced;
    for &(j, t_fixed) in zone_fixes.iter().rev() {
        x_full = x_full.insert_row(j, t_fixed);
    }

    Ok(x_full)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::FixedOffset;
    use chrono::TimeZone;
    use hares_types::{
        EnvironmentState, GridState, SurfaceIrradiance, WeatherState, ZoneId, ZoneState,
    };
    use nalgebra::DMatrix;

    use super::*;
    use crate::state_space::StateSpaceModel;

    fn minimal_env(zone_temp_c: f64, outdoor_temp_c: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c,
                outdoor_humidity_ratio: 0.004,
                wind_speed_m_s: 3.0,
                wind_dir_deg: 180.0,
                ground_temp_c: outdoor_temp_c,
                sky_temp_c: outdoor_temp_c - 5.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![SurfaceIrradiance {
                    surface_id: 1,
                    direct_w_m2: 0.0,
                    diffuse_w_m2: 0.0,
                    reflected_w_m2: 0.0,
                    angle_of_incidence_rad: 0.0,
                }],
                outdoor_wet_bulb_c: 0.0,
                outdoor_enthalpy_j_kg: 0.0,
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                solar_altitude_deg: 0.0,
                solar_azimuth_deg: 180.0,
                mains_temp_c: 15.0,
                rainfall_m: 0.0,
                ground_albedo: 0.2,
                ground_t_mean_c: 10.0,
                ground_t_amplitude_c: 0.0,
                ground_phase_day: 35.0,
                day_of_year: 1.0,
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: Default::default(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
                .single()
                .expect("valid time"),
            time_res: chrono::Duration::seconds(60),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    /// Build a 1-state model with one outdoor-temp input:
    ///   x[k+1] = a * x[k] + b * u_outdoor
    /// Steady state: x = a*x + b*u  =>  x = b*u / (1 - a)
    fn one_node_model(a: f64, b: f64) -> (StateSpaceModel, StateSpaceWiring) {
        let a_d = DMatrix::from_element(1, 1, a);
        let b_d = DMatrix::from_element(1, 1, b);
        let c = DMatrix::from_element(1, 1, 1.0);
        let d = DMatrix::zeros(1, 1);

        let model = StateSpaceModel::from_discrete(a_d, b_d, c, d).unwrap();

        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::new(), // no zone state pinning
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::new(),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };

        (model, wiring)
    }

    #[test]
    fn single_node_steady_state() {
        let a = 0.8;
        let b = 0.2;
        let outdoor_temp = -5.0;
        let indoor_temp = 22.0;

        let (model, wiring) = one_node_model(a, b);
        let env = minimal_env(indoor_temp, outdoor_temp);

        let x = initialize_steady_state(&model, &wiring, &env, indoor_temp, &[]).unwrap();

        // Expected: x = b * outdoor / (1 - a) = 0.2 * (-5) / 0.2 = -5
        let expected = b * outdoor_temp / (1.0 - a);
        assert!(
            (x[0] - expected).abs() < 1e-10,
            "expected {expected}, got {}",
            x[0]
        );
    }

    #[test]
    fn zone_state_pinned_as_boundary() {
        // 2-state system: state 0 = zone air (pinned), state 1 = wall node
        // A_d couples wall to zone air; with zone pinned the wall should
        // converge to a weighted average of outdoor and indoor.
        let a = 0.5;
        let coupling = 0.3; // A_d[1,0]: wall <- zone coupling

        let a_d = DMatrix::from_row_slice(2, 2, &[0.0, 0.0, coupling, a]);
        // One input: outdoor temp feeds wall node only
        let b_d = DMatrix::from_row_slice(2, 1, &[0.0, 0.2]);
        let c = DMatrix::identity(2, 2);
        let d = DMatrix::zeros(2, 1);

        let model = StateSpaceModel::from_discrete(a_d, b_d, c, d).unwrap();

        let mut zone_state_indices = HashMap::new();
        zone_state_indices.insert(ZoneId(1), 0_usize);

        let wiring = StateSpaceWiring {
            zone_state_indices,
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::new(),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };

        let outdoor = -10.0;
        let indoor = 20.0;
        let env = minimal_env(indoor, outdoor);

        let x = initialize_steady_state(&model, &wiring, &env, indoor, &[ZoneId(1)]).unwrap();

        assert_eq!(x.len(), 2);
        // State 0 should be pinned at indoor temp.
        assert!(
            (x[0] - indoor).abs() < 1e-10,
            "zone state should be pinned at {indoor}, got {}",
            x[0]
        );

        // Reduced system for state 1 (wall):
        //   x1 = a*x1 + coupling*indoor + 0.2*outdoor
        //   x1(1 - a) = coupling*indoor + 0.2*outdoor
        //   x1 = (coupling*indoor + 0.2*outdoor) / (1 - a)
        let expected_wall = (coupling * indoor + 0.2 * outdoor) / (1.0 - a);
        assert!(
            (x[1] - expected_wall).abs() < 1e-10,
            "expected wall temp {expected_wall}, got {}",
            x[1]
        );
    }

    /// `initialize_steady_state` returns `Err(IndoorZoneIdNotRegistered)` when
    /// a pinned zone is absent from `zone_state_indices`, surfacing the
    /// configuration error rather than silently substituting a fallback initial
    /// state.
    #[test]
    fn missing_indoor_zone_id_in_zone_state_indices_errors() {
        // 2-state model; the conditioned zone is ZoneId(2), but the wiring
        // only contains ZoneId(1) — so indoor_zone_id is not registered.
        let a_d = DMatrix::from_row_slice(2, 2, &[0.8, 0.0, 0.1, 0.9]);
        let b_d = DMatrix::from_row_slice(2, 1, &[0.2, 0.1]);
        let c = DMatrix::identity(2, 2);
        let d = DMatrix::zeros(2, 1);

        use crate::state_space::StateSpaceModel;
        let model = StateSpaceModel::from_discrete(a_d, b_d, c, d).unwrap();

        let mut zone_state_indices = HashMap::new();
        zone_state_indices.insert(ZoneId(1), 0_usize); // ZoneId(2) intentionally absent

        let wiring = StateSpaceWiring {
            zone_state_indices,
            zone_output_indices: HashMap::from([(ZoneId(2), 1)]),
            zone_sensible_input_indices: HashMap::new(),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };

        let indoor = 21.0;
        let env = minimal_env(indoor, -5.0);

        // Pass ZoneId(2) as the pinned zone — it is not in zone_state_indices.
        let result = initialize_steady_state(&model, &wiring, &env, indoor, &[ZoneId(2)]);

        match result {
            Err(ThermalSolverError::IndoorZoneIdNotRegistered { id, registered }) => {
                assert_eq!(id, ZoneId(2));
                assert_eq!(registered, vec![ZoneId(1)]);
            }
            Ok(x) => panic!(
                "expected Err(IndoorZoneIdNotRegistered) when indoor_zone_id is absent \
                 from zone_state_indices, but got Ok({:?})",
                x
            ),
            Err(other) => {
                panic!("expected Err(IndoorZoneIdNotRegistered) but got different error: {other:?}")
            }
        }
    }

    #[test]
    fn singular_matrix_fallback() {
        // A_d = identity => (I - A_d) = 0 => singular, should fall back to uniform indoor temp.
        let a_d = DMatrix::identity(2, 2);
        let b_d = DMatrix::from_element(2, 1, 0.1);
        let c = DMatrix::identity(2, 2);
        let d = DMatrix::zeros(2, 1);

        let model = StateSpaceModel::from_discrete(a_d, b_d, c, d).unwrap();

        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::new(),
            zone_output_indices: HashMap::new(),
            zone_sensible_input_indices: HashMap::new(),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };

        let indoor = 21.0;
        let env = minimal_env(indoor, -5.0);

        let x = initialize_steady_state(&model, &wiring, &env, indoor, &[]).unwrap();

        assert_eq!(x.len(), 2);
        for i in 0..x.len() {
            assert!(
                (x[i] - indoor).abs() < 1e-10,
                "state {i}: expected fallback to {indoor}, got {}",
                x[i]
            );
        }
    }

    #[test]
    fn zone_state_index_exceeds_state_dimension_errors() {
        // 1-state model; wiring registers ZoneId(1) at index 5, which is >= n=1.
        // This represents internally inconsistent wiring — a registered zone
        // with an index pointing beyond the state vector.
        let a_d = DMatrix::from_element(1, 1, 0.8);
        let b_d = DMatrix::from_element(1, 1, 0.2);
        let c = DMatrix::from_element(1, 1, 1.0);
        let d = DMatrix::zeros(1, 1);

        let model = StateSpaceModel::from_discrete(a_d, b_d, c, d).unwrap();

        let mut zone_state_indices = HashMap::new();
        zone_state_indices.insert(ZoneId(1), 5_usize);

        let wiring = StateSpaceWiring {
            zone_state_indices,
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::new(),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };

        let indoor = 21.0;
        let env = minimal_env(indoor, -5.0);

        let result = initialize_steady_state(&model, &wiring, &env, indoor, &[ZoneId(1)]);

        match result {
            Err(ThermalSolverError::ZoneStateIndexOutOfBounds {
                zone_id,
                index,
                state_dim,
            }) => {
                assert_eq!(zone_id, ZoneId(1));
                assert_eq!(index, 5);
                assert_eq!(state_dim, 1);
            }
            Ok(x) => panic!(
                "expected Err(ZoneStateIndexOutOfBounds), but got Ok({:?})",
                x
            ),
            Err(other) => {
                panic!("expected Err(ZoneStateIndexOutOfBounds) but got different error: {other:?}")
            }
        }
    }

    /// `initialize_steady_state` returns `Err(SingularInitialization)` when
    /// a pinned zone has zero thermal capacitance in `c_zone_j_k`. A massless
    /// zone node pinned at a fixed temperature creates a kinematic constraint
    /// with no energy storage to absorb it, making the reduced system singular.
    #[test]
    fn zero_capacitance_zone_guard_errors() {
        let a_d = DMatrix::from_row_slice(2, 2, &[0.0, 0.0, 0.3, 0.5]);
        let b_d = DMatrix::from_row_slice(2, 1, &[0.0, 0.2]);
        let c = DMatrix::identity(2, 2);
        let d = DMatrix::zeros(2, 1);

        let model = StateSpaceModel::from_discrete(a_d, b_d, c, d).unwrap();

        let mut zone_state_indices = HashMap::new();
        zone_state_indices.insert(ZoneId(1), 0_usize);

        let mut c_zone_j_k = HashMap::new();
        c_zone_j_k.insert(ZoneId(1), 0.0);

        let wiring = StateSpaceWiring {
            zone_state_indices,
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::new(),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k,
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };

        let indoor = 21.0;
        let env = minimal_env(indoor, -5.0);

        let result = initialize_steady_state(&model, &wiring, &env, indoor, &[ZoneId(1)]);

        match result {
            Err(ThermalSolverError::SingularInitialization {
                zone_id,
                capacitance_j_k,
            }) => {
                assert_eq!(zone_id, ZoneId(1));
                assert_eq!(capacitance_j_k, 0.0);
            }
            Ok(x) => panic!(
                "expected Err(SingularInitialization) for zero-capacitance zone, \
                 but got Ok({:?})",
                x
            ),
            Err(other) => {
                panic!("expected Err(SingularInitialization) but got different error: {other:?}")
            }
        }
    }

    /// `initialize_steady_state` returns `Err(SingularInitialization)` when
    /// a pinned zone's capacitance is below the 1e-6 J/K pinning threshold but
    /// not exactly zero. Small non-zero capacitances still create near-singular
    /// reduced systems whose solution is physically meaningless.
    #[test]
    fn near_zero_capacitance_zone_errors() {
        let a_d = DMatrix::from_row_slice(2, 2, &[0.0, 0.0, 0.3, 0.5]);
        let b_d = DMatrix::from_row_slice(2, 1, &[0.0, 0.2]);
        let c = DMatrix::identity(2, 2);
        let d = DMatrix::zeros(2, 1);

        let model = StateSpaceModel::from_discrete(a_d, b_d, c, d).unwrap();

        let mut zone_state_indices = HashMap::new();
        zone_state_indices.insert(ZoneId(1), 0_usize);

        let mut c_zone_j_k = HashMap::new();
        c_zone_j_k.insert(ZoneId(1), 1e-12);

        let wiring = StateSpaceWiring {
            zone_state_indices,
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::new(),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k,
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };

        let indoor = 21.0;
        let env = minimal_env(indoor, -5.0);

        let result = initialize_steady_state(&model, &wiring, &env, indoor, &[ZoneId(1)]);

        match result {
            Err(ThermalSolverError::SingularInitialization {
                zone_id,
                capacitance_j_k,
            }) => {
                assert_eq!(zone_id, ZoneId(1));
                assert!((capacitance_j_k - 1e-12).abs() < 1e-20);
            }
            Ok(x) => panic!(
                "expected Err(SingularInitialization) for near-zero-capacitance zone, \
                 but got Ok({:?})",
                x
            ),
            Err(other) => {
                panic!("expected Err(SingularInitialization) but got different error: {other:?}")
            }
        }
    }

    /// `initialize_steady_state` returns `Err(ZoneNotInEnvironment)` when
    /// a pinned zone is registered in `zone_state_indices` but absent from
    /// `env.zones`, surfacing the wiring/environment consistency error rather
    /// than silently substituting `indoor_temp_c`.
    #[test]
    fn missing_zone_in_env_zones_errors() {
        let a_d = DMatrix::from_row_slice(2, 2, &[0.8, 0.0, 0.1, 0.9]);
        let b_d = DMatrix::from_row_slice(2, 1, &[0.2, 0.1]);
        let c = DMatrix::identity(2, 2);
        let d = DMatrix::zeros(2, 1);

        let model = StateSpaceModel::from_discrete(a_d, b_d, c, d).unwrap();

        let mut zone_state_indices = HashMap::new();
        zone_state_indices.insert(ZoneId(1), 0_usize);

        let wiring = StateSpaceWiring {
            zone_state_indices,
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::new(),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };

        let indoor = 21.0;
        // env.zones is empty — ZoneId(1) is in zone_state_indices but absent
        // from the environment's zone list.
        let env = minimal_env(indoor, -5.0);
        let env = EnvironmentState {
            zones: vec![],
            ..env
        };

        let result = initialize_steady_state(&model, &wiring, &env, indoor, &[ZoneId(1)]);

        match result {
            Err(ThermalSolverError::ZoneNotInEnvironment {
                zone_id,
                present_zones,
            }) => {
                assert_eq!(zone_id, ZoneId(1));
                assert!(present_zones.is_empty());
            }
            Ok(x) => panic!("expected Err(ZoneNotInEnvironment), but got Ok({:?})", x),
            Err(other) => {
                panic!("expected Err(ZoneNotInEnvironment) but got different error: {other:?}")
            }
        }
    }
}
