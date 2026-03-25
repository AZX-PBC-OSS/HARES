//! Steady-state initialisation for the thermal solver.

use nalgebra::{DMatrix, DVector};

use crate::state_space::StateSpaceModel;

use super::{Result, StateSpaceWiring};
use hares_types::EnvironmentState;

/// Compute steady-state temperatures by solving `x = A_d x + B_d u` for `x`,
/// with zone air nodes pinned to `indoor_temp_c` as boundary conditions.
pub(crate) fn initialize_steady_state(
    model: &StateSpaceModel,
    wiring: &StateSpaceWiring,
    env: &EnvironmentState,
    indoor_temp_c: f64,
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
    for &idx in &wiring.ground_temp_input_indices {
        if idx < m {
            u[idx] = env.weather.ground_temp_c;
        }
    }
    for &idx in &wiring.indoor_temp_input_indices {
        if idx < m {
            u[idx] = indoor_temp_c;
        }
    }

    // Collect zone state indices to fix as boundary conditions.
    // Sort descending so we can remove rows/cols without invalidating earlier indices.
    // Pin each zone air node to its initial temperature from the environment.
    // Conditioned zones use the HVAC setpoint (indoor_temp_c).
    // Unconditioned zones (attic, garage) use their env temperature (near outdoor).
    let mut zone_fixes: Vec<(usize, f64)> = wiring
        .zone_state_indices
        .iter()
        .filter(|(_, idx)| **idx < n)
        .map(|(zone_id, idx)| {
            let idx = *idx;
            let t = env
                .zones
                .iter()
                .find(|z| z.id == *zone_id)
                .map(|z| z.temperature_c)
                .unwrap_or(indoor_temp_c);
            (idx, t)
        })
        .collect();
    zone_fixes.sort_by(|a, b| b.0.cmp(&a.0));
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
                relative_humidity: 0.45,
                wet_bulb_c: zone_temp_c - 5.0,
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
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
                .single()
                .expect("valid time"),
            time_res: chrono::Duration::seconds(60),
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
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
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

        let x = initialize_steady_state(&model, &wiring, &env, indoor_temp).unwrap();

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
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
        };

        let outdoor = -10.0;
        let indoor = 20.0;
        let env = minimal_env(indoor, outdoor);

        let x = initialize_steady_state(&model, &wiring, &env, indoor).unwrap();

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
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
        };

        let indoor = 21.0;
        let env = minimal_env(indoor, -5.0);

        let x = initialize_steady_state(&model, &wiring, &env, indoor).unwrap();

        assert_eq!(x.len(), 2);
        for i in 0..x.len() {
            assert!(
                (x[i] - indoor).abs() < 1e-10,
                "state {i}: expected fallback to {indoor}, got {}",
                x[i]
            );
        }
    }
}
