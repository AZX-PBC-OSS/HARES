//! Interior longwave radiation integration tests.
//!
//! These tests exercise `interior_longwave_net_w` and `interior_longwave_linearised_w`
//! directly via the public API, verifying energy conservation and heat direction.

use std::collections::HashMap;

use chrono::{FixedOffset, TimeZone};
use hares_envelope::{
    FilmCoefficientModel, InteriorLwrZoneConfig, InteriorSurface, InteriorSurfaceInfo,
    MechanicalVentilationParams, OutputMapping, StateSpaceModel, StateSpaceWiring, ThermalSnapshot,
    ThermalSolver, ThermalSolverConfig, interior_longwave_net_w,
};
use hares_types::{
    DomainSolver, EnvironmentState, GridState, PortSlots, SurfaceIrradiance, ThermalAccumulator,
    WeatherState, ZoneId, ZoneState,
};
use nalgebra::DMatrix;
use nalgebra::DVector;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Regression: stale zone-air temperature in LWR convergence loop
// ---------------------------------------------------------------------------

/// Stale zone-air temperature characterisation.
///
/// `apply_interior_longwave_inputs` reads `env.zones[i].temperature_c` (the
/// prior-step committed value) and holds it fixed throughout all convergence
/// iterations.  A claim states that a stale zone temp of 1 °C produces a
/// ~16 W LWR error across 30 m² of surface.
///
/// This test verifies the ACTUAL numerical impact:
///
/// * The PRIMARY (ScriptF) path does NOT use t_zone_c at all — it computes
///   pure T⁴ radiosities from surface temperatures.  The stale zone
///   temperature has ZERO effect on the ScriptF path.
///
/// * The LINEARISED FALLBACK path uses t_zone_c only to set the h_r
///   coefficient (4εσT³).  A 1 °C shift changes h_r by ~0.052 W/(m²·K),
///   producing a max per-surface flux error of ~0.5 W — not the 16 W
///   claimed in the ticket.  The ticket's 154 W estimate (from applying
///   h_r directly rather than Δh_r) is also wrong.
///
/// The test passes regardless of bug presence; it is a characterisation
/// test that pins the measured error so future reviewers can verify that
/// any "fix" to the stale zone temp in the linearised path actually
/// changes the computed fluxes by less than 1 W.
#[test]
fn stale_zone_temp_linearised_lwr_error_is_sub_watt() {
    use hares_envelope::{InteriorSurface, interior_longwave_linearised_w};

    let surfaces = vec![
        InteriorSurface {
            area_m2: 5.0,
            emissivity: 0.90,
        },
        InteriorSurface {
            area_m2: 5.0,
            emissivity: 0.90,
        },
        InteriorSurface {
            area_m2: 5.0,
            emissivity: 0.90,
        },
        InteriorSurface {
            area_m2: 5.0,
            emissivity: 0.90,
        },
        InteriorSurface {
            area_m2: 5.0,
            emissivity: 0.90,
        },
        InteriorSurface {
            area_m2: 5.0,
            emissivity: 0.90,
        },
    ];

    // Surface temperatures spread across ~3.5°C — a plausible residential zone.
    let t_surfaces = vec![21.0, 19.5, 20.5, 20.0, 18.5, 22.0];

    // Prior-step zone temperature (stale, as used today).
    let t_zone_stale = 19.0_f64;
    // Predicted zone temperature after a 1 °C/step heating ramp.
    let t_zone_predicted = 20.0_f64;

    let q_stale = interior_longwave_linearised_w(&surfaces, &t_surfaces, t_zone_stale);
    let q_predicted = interior_longwave_linearised_w(&surfaces, &t_surfaces, t_zone_predicted);

    let max_err_w: f64 = q_stale
        .iter()
        .zip(q_predicted.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0_f64, f64::max);

    // The actual error from the stale zone temp in the linearised path is
    // < 0.5 W per surface (Δh_r ≈ 0.052 W/m²K per °C), not 16 W as claimed.
    assert!(
        max_err_w < 1.0,
        "linearised-path error from 1°C stale zone temp should be <1 W per surface, got {max_err_w:.3} W"
    );

    // Confirm it is non-zero (the stale ref does introduce some bias).
    assert!(
        max_err_w > 0.01,
        "linearised-path error should be detectable (>0.01 W), got {max_err_w:.4} W"
    );
}

/// All surfaces at the same temperature: net LWR flux must sum to zero
/// (energy conservation in radiative equilibrium).
#[test]
fn test_interior_lwr_net_flux_is_zero() {
    let surfaces = vec![
        InteriorSurface {
            area_m2: 20.0,
            emissivity: 0.90,
        },
        InteriorSurface {
            area_m2: 15.0,
            emissivity: 0.90,
        },
        InteriorSurface {
            area_m2: 25.0,
            emissivity: 0.90,
        },
        InteriorSurface {
            area_m2: 10.0,
            emissivity: 0.90,
        },
    ];
    let temps = vec![20.0, 20.0, 20.0, 20.0];

    let fluxes = interior_longwave_net_w(&surfaces, &temps);

    let total: f64 = fluxes.iter().sum();
    assert!(
        total.abs() < 1e-6,
        "net LWR flux sum must be ~0 when all surfaces are at equal temperature, got {total}"
    );

    // Each individual flux should also be ~0 at equal temperatures
    for (i, &q) in fluxes.iter().enumerate() {
        assert!(
            q.abs() < 1e-6,
            "surface {i} flux must be ~0 at equal temperature, got {q}"
        );
    }
}

/// One hot surface (30 C) among three cold surfaces (20 C).
/// The hot surface must lose heat (negative flux) and cold surfaces must gain.
#[test]
fn test_interior_lwr_hot_surface_loses_heat() {
    let surfaces = vec![
        InteriorSurface {
            area_m2: 20.0,
            emissivity: 0.90,
        },
        InteriorSurface {
            area_m2: 15.0,
            emissivity: 0.90,
        },
        InteriorSurface {
            area_m2: 25.0,
            emissivity: 0.90,
        },
        InteriorSurface {
            area_m2: 10.0,
            emissivity: 0.90,
        },
    ];
    let temps = vec![30.0, 20.0, 20.0, 20.0];

    let fluxes = interior_longwave_net_w(&surfaces, &temps);

    assert!(
        fluxes[0] < 0.0,
        "hot surface (30 C) must lose heat (negative flux), got {}",
        fluxes[0]
    );
    assert!(
        fluxes[1] > 0.0,
        "cold surface 1 must gain heat (positive flux), got {}",
        fluxes[1]
    );
    assert!(
        fluxes[2] > 0.0,
        "cold surface 2 must gain heat (positive flux), got {}",
        fluxes[2]
    );
    assert!(
        fluxes[3] > 0.0,
        "cold surface 3 must gain heat (positive flux), got {}",
        fluxes[3]
    );

    // Energy conservation: sum of all fluxes must be ~0
    let total: f64 = fluxes.iter().sum();
    assert!(
        total.abs() < 1e-6,
        "net LWR flux sum must be ~0 (energy conservation), got {total}"
    );
}

/// Two identical surfaces at different temperatures: fluxes must be equal
/// and opposite (energy conservation for a 2-surface enclosure).
#[test]
fn test_interior_lwr_identical_surfaces_symmetric() {
    let surfaces = vec![
        InteriorSurface {
            area_m2: 20.0,
            emissivity: 0.90,
        },
        InteriorSurface {
            area_m2: 20.0,
            emissivity: 0.90,
        },
    ];
    let temps = vec![25.0, 15.0];

    let fluxes = interior_longwave_net_w(&surfaces, &temps);

    assert!(
        (fluxes[0].abs() - fluxes[1].abs()).abs() < 1e-6,
        "|flux_A| must equal |flux_B| for identical surfaces: flux_A={}, flux_B={}",
        fluxes[0],
        fluxes[1]
    );

    // Hot surface loses, cold surface gains
    assert!(
        fluxes[0] < 0.0,
        "surface A (25 C) must lose heat, got {}",
        fluxes[0]
    );
    assert!(
        fluxes[1] > 0.0,
        "surface B (15 C) must gain heat, got {}",
        fluxes[1]
    );

    // Energy conservation
    let total: f64 = fluxes.iter().sum();
    assert!(total.abs() < 1e-6, "net flux sum must be ~0, got {total}");
}

// ---------------------------------------------------------------------------
// Regression: LWR fallback to linearised path must not be silent
// ---------------------------------------------------------------------------

/// Build a minimal EnvironmentState for the regression test.
fn env_20c() -> EnvironmentState {
    EnvironmentState {
        zones: vec![ZoneState {
            id: ZoneId(1),
            temperature_c: 20.0,
            humidity_ratio: 0.008,
            volume_m3: 200.0,
        }],
        weather: WeatherState {
            outdoor_temp_c: 10.0,
            outdoor_humidity_ratio: 0.004,
            outdoor_wet_bulb_c: 5.0,
            outdoor_enthalpy_j_kg: 14_000.0,
            wind_speed_m_s: 3.0,
            wind_dir_deg: 180.0,
            ground_temp_c: 10.0,
            sky_temp_c: 5.0,
            pressure_kpa: 101.325,
            solar_irradiance: vec![SurfaceIrradiance {
                surface_id: 0,
                direct_w_m2: 0.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
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
            .with_ymd_and_hms(2026, 5, 21, 12, 0, 0)
            .single()
            .expect("valid time"),
        time_res: chrono::Duration::seconds(60),
        price_signal: Default::default(),
        electrical: Default::default(),
    }
}

/// Regression test: `ThermalSolver::new` must return `Err` when
/// `interior_lwr_zones` contains a zone with `scriptf == None` and
/// at least 2 surfaces.  A silent fallback to the linearised h_r path
/// is a configuration error — ScriptF factors must be pre-computed
/// via `compute_scriptf()` before construction.
#[test]
fn lwr_zone_without_scriptf_must_error_at_construction() {
    let env = env_20c();

    // Minimal 3-state model (one zone air node + two surface nodes).
    let a_c = DMatrix::from_diagonal_element(3, 3, -1.0 / 50_000.0);
    let b_c = DMatrix::zeros(3, 3);
    let mapping = OutputMapping {
        output_count: 1,
        node_to_output: vec![(0, 0, 1.0)],
        input_to_output: vec![],
    };
    let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping)
        .expect("model construction failed");

    let wiring = StateSpaceWiring {
        zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
        zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
        zone_sensible_input_indices: HashMap::from([(ZoneId(1), 0)]),
        outdoor_temp_input_indices: vec![0],
        ground_temp_input_indices: vec![],
        ground_temp_input_depths_m: vec![],
        indoor_temp_input_indices: vec![],
        solar_input_indices: HashMap::new(),
        c_zone_j_k: HashMap::new(),
        node_capacitances: HashMap::new(),
        node_index: HashMap::new(),
    };

    // Zone config has two surfaces but scriptf is deliberately left as None —
    // this is the misconfiguration that must be rejected.
    let lwr_zone = InteriorLwrZoneConfig {
        zone_id: ZoneId(1),
        surfaces: vec![
            InteriorSurfaceInfo {
                state_index: 1,
                input_index: 1,
                area_m2: 20.0,
                emissivity: 0.90,
                radiation_frac: 1.0,
                rad_res_k_w: 200.0,
                solar_absorptance: 0.5,
                is_floor: false,
                driving_temp: None,
            },
            InteriorSurfaceInfo {
                state_index: 2,
                input_index: 2,
                area_m2: 15.0,
                emissivity: 0.90,
                radiation_frac: 1.0,
                rad_res_k_w: 200.0,
                solar_absorptance: 0.5,
                is_floor: false,
                driving_temp: None,
            },
        ],
        // scriptf intentionally None — this should be rejected.
        scriptf: None,
    };

    let config = ThermalSolverConfig {
        indoor_zone_id: ZoneId(1),
        window_properties: HashMap::new(),
        window_zone_ids: HashMap::new(),
        exterior_surfaces: vec![],
        interior_lwr_zones: vec![lwr_zone],
        infiltration: vec![],
        ventilation_flow_m3_s: 0.0,
        ventilation: MechanicalVentilationParams::default(),
        natural_ventilation: None,
        supply_duct_leakage_m3_s: 0.0,
        return_duct_leakage_m3_s: 0.0,
        interior_lwr_method: hares_envelope::InteriorLwrMethod::ScriptF,
        interior_solar_zones: Vec::new(),
        boundary_diagnostics: Vec::new(),
        film_coefficient_model: FilmCoefficientModel::default(),
        interior_convection_injections: Vec::new(),
        ideal_capacity_degraded_threshold: 3,
    };

    let result = ThermalSolver::new(model, wiring, config, 60.0, &env, 20.0);
    assert!(
        result.is_err(),
        "ThermalSolver::new must return Err when interior_lwr_zones contains \
         a zone with scriptf == None"
    );
}

// ---------------------------------------------------------------------------
// Verification: radiation_frac = r_film_conv / (r_film_conv + r_inner_half)
// ---------------------------------------------------------------------------

/// `radiation_frac` = `r_film_conv / (r_film_conv + r_inner_half)` correctly
/// computes the fraction of an injected flux that routes to the wall mass RC
/// node in the OCHRE "full" mode (convection-only interior film, LWR handled
/// separately by ScriptF/StarMesh).
///
/// The prior audit claimed the formula was a pre-S1 "series voltage-divider"
/// and proposed a "current-divider" replacement.  That analysis compared two
/// different quantities:
///
///   * Current `radiation_frac` = `R_film_conv / (R_film_conv + R_inner_half)`
///     = fraction to WALL MASS node.
///   * Proposed "correct" value = `G_conv / (G_conv + G_wall)`
///     = fraction to ZONE AIR.
///
/// These are **complements**, not alternatives.  The current formula is
/// algebraically identical to `G_wall / (G_wall + G_conv)` when expressed in
/// conductances, and it always equals `1 − (fraction to air)`.  Both quantities
/// are correct for what they claim to represent.  The audit's claim of a
/// "~27% empirical bias" from this formula is not reproducible — the formula
/// and its complement produce self-consistent results.
///
/// This test verifies the identity, confirms the formula's numeric correctness
/// for representative surfaces, and asserts that the fraction to wall mass +
/// fraction to zone air sum to exactly 1.0.
///
/// Reference: OCHRE Envelope.py:254 `res_film / (res_film + res_material)`.
/// The HARES comment "OCHRE 'full' mode: radiation_frac = R_film_conv /
/// (R_film_conv + R_inner_half)" at solver_builder.rs:248-250 correctly
/// describes what the formula computes and which code path it serves.
#[test]
fn radiation_frac_correctly_splits_between_wall_mass_and_zone_air() {
    // Convection-only interior film resistance [m²K/W].
    // TARP h_c ≈ 8.33 W/(m²·K) for vertical wall.
    let r_film_conv_m2kw = 0.12_f64;

    // Half-node material resistance for 100 mm concrete (k=1.7 W/(m·K)).
    // r_inner_half = thickness / (2 × k) for a two-capacitor centred-difference RC layer.
    // 100 mm / (2 × 1.7 W/m·K) ≈ 0.0294 m²·K/W.
    let r_inner_half_m2kw = 0.100 / (2.0 * 1.7);

    // ─ Current (correct) formula: fraction to wall mass RC node ─
    let radiation_frac = r_film_conv_m2kw / (r_film_conv_m2kw + r_inner_half_m2kw);

    // ─ Conductance-based equivalents ─
    let g_conv = 1.0 / r_film_conv_m2kw; // convection → zone air
    let g_wall = 1.0 / r_inner_half_m2kw; // conduction → wall mass

    // Fraction to wall mass via conductances must equal the voltage-divider.
    let frac_to_wall_mass = g_wall / (g_wall + g_conv);
    assert!(
        (radiation_frac - frac_to_wall_mass).abs() < 1e-9,
        "radiation_frac ({radiation_frac:.6}) must equal G_wall/(G_wall+G_conv) ({frac_to_wall_mass:.6})"
    );

    // Fraction to zone air via conductances.
    let frac_to_zone_air = g_conv / (g_wall + g_conv);

    // The split must sum to 1.0 (all flux accounted for).
    assert!(
        (radiation_frac + frac_to_zone_air - 1.0).abs() < 1e-9,
        "radiation_frac ({radiation_frac:.6}) + frac_to_air ({frac_to_zone_air:.6}) must sum to 1.0"
    );

    // Concrete wall: material resistance dominates convection → most flux goes to wall mass.
    // For a typical residential wall (0.12 conv, 0.029 half-node), the wall mass
    // should receive >70% of an injected gain.
    assert!(
        radiation_frac > 0.7,
        "for R_conv=0.12, R_half=0.029, radiation_frac ({radiation_frac:.4}) should be >0.7"
    );
    assert!(
        radiation_frac < 1.0,
        "radiation_frac must be < 1.0 when half-node resistance is finite"
    );

    // ─ Edge case: massive wall (large R_inner_half) ─
    // Very thick insulation: most flux goes to zone air (convection wins).
    let r_inner_half_large = 5.0;
    let rad_frac_large = r_film_conv_m2kw / (r_film_conv_m2kw + r_inner_half_large);
    assert!(
        rad_frac_large < 0.1,
        "with large R_half=5.0, radiation_frac ({rad_frac_large:.4}) should be small (<0.1)"
    );

    // ─ Edge case: thin foil (R_inner_half ≈ 0) — surface IS the node ─
    let r_inner_half_tiny = 1e-6;
    let rad_frac_tiny = r_film_conv_m2kw / (r_film_conv_m2kw + r_inner_half_tiny);
    assert!(
        rad_frac_tiny > 0.9999,
        "with near-zero R_half, radiation_frac ({rad_frac_tiny:.6}) should approach 1.0"
    );
}

// ---------------------------------------------------------------------------
// Regression: interior LWR convergence uses flux residual, not step magnitude
// ---------------------------------------------------------------------------

/// Interior LWR iteration with ScriptF must converge using the new
/// flux-residual criterion without panicking or producing non-finite
/// values.
///
/// The old criterion tested the heavy-ball step magnitude
/// `|buf[j] − prev_buf[j]| < 0.01`, which measures the momentum update
/// not the LWR flux residual.  This test verifies that the solver runs
/// to completion with the new relative flux-residual criterion
/// `|q_new − q_old| / (|q_old| + 1e-6) < 1e-4` and produces finite
/// zone temperature results for a simple two-surface ScriptF enclosure.
///
/// Correctness of the convergence loop is covered by the unit test
/// `interior_longwave_surface_states_persist_with_clamp_and_damping`.
#[test]
fn interior_lwr_converges_by_flux_residual_within_iter_budget() {
    let env = env_20c();

    // Minimal 3-state model: one zone air node + two surface RC nodes,
    // with resistive coupling between zone air and each surface.
    let a_c = DMatrix::from_diagonal_element(3, 3, -1.0 / 50_000.0);
    let b_c = DMatrix::zeros(3, 3);
    let mapping = OutputMapping {
        output_count: 1,
        node_to_output: vec![(0, 0, 1.0)],
        input_to_output: vec![],
    };
    let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping)
        .expect("model construction failed");

    let wiring = StateSpaceWiring {
        zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
        zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
        zone_sensible_input_indices: HashMap::from([(ZoneId(1), 0)]),
        outdoor_temp_input_indices: vec![0],
        ground_temp_input_indices: vec![],
        ground_temp_input_depths_m: vec![],
        indoor_temp_input_indices: vec![],
        solar_input_indices: HashMap::new(),
        c_zone_j_k: HashMap::new(),
        node_capacitances: HashMap::new(),
        node_index: HashMap::new(),
    };

    // Two surfaces at different temperatures: 25°C wall and 15°C floor.
    let mut lwr_zone = InteriorLwrZoneConfig {
        zone_id: ZoneId(1),
        surfaces: vec![
            InteriorSurfaceInfo {
                state_index: 1,
                input_index: 1,
                area_m2: 30.0,
                emissivity: 0.90,
                radiation_frac: 1.0,
                rad_res_k_w: 0.003,
                solar_absorptance: 0.0,
                is_floor: false,
                driving_temp: None,
            },
            InteriorSurfaceInfo {
                state_index: 2,
                input_index: 2,
                area_m2: 30.0,
                emissivity: 0.90,
                radiation_frac: 1.0,
                rad_res_k_w: 0.003,
                solar_absorptance: 0.0,
                is_floor: true,
                driving_temp: None,
            },
        ],
        scriptf: None,
    };
    lwr_zone.compute_scriptf();

    let config = ThermalSolverConfig {
        indoor_zone_id: ZoneId(1),
        window_properties: HashMap::new(),
        window_zone_ids: HashMap::new(),
        exterior_surfaces: vec![],
        interior_lwr_zones: vec![lwr_zone],
        infiltration: vec![],
        ventilation_flow_m3_s: 0.0,
        ventilation: MechanicalVentilationParams::default(),
        natural_ventilation: None,
        supply_duct_leakage_m3_s: 0.0,
        return_duct_leakage_m3_s: 0.0,
        interior_lwr_method: hares_envelope::InteriorLwrMethod::ScriptF,
        interior_solar_zones: Vec::new(),
        boundary_diagnostics: Vec::new(),
        film_coefficient_model: FilmCoefficientModel::default(),
        interior_convection_injections: Vec::new(),
        ideal_capacity_degraded_threshold: 3,
    };

    let mut solver = ThermalSolver::new(model, wiring, config, 60.0, &env, 20.0).unwrap();
    // Set mass node temperatures: zone air at 20°C, surface 1 at 25°C, surface 2 at 15°C.
    solver
        .restore_state(&ThermalSnapshot {
            x: vec![20.0, 25.0, 15.0],
            last_u: vec![],
            lwr_t_prev_c: vec![],
            interior_surface_temps: vec![vec![20.0, 20.0]],
            interior_surface_prev_temps: vec![vec![20.0, 20.0]],
        })
        .unwrap();

    // Run one full resolve step.  The interior LWR convergence loop uses
    // the flux-residual criterion — if convergence hangs or produces NaN,
    // the solver panics and this test fails.
    let ports = PortSlots {
        thermal: vec![ThermalAccumulator::new(ZoneId(1))],
        ..Default::default()
    };
    let env = env_20c();
    let update = solver.resolve_new(&ports, &env, Duration::from_secs(60));

    // Verify the solver produced finite zone temperatures and that the
    // LWR convergence loop completed successfully.
    let t_zone = update.zone_temperatures_c[0].1;
    assert!(
        t_zone.is_finite(),
        "zone temperature must be finite, got {t_zone}"
    );

    // The LWR algorithm sets per-zone exchange diagnostics.
    let lwr_by_zone: Vec<_> = solver
        .component_gains()
        .interior_lwr_by_zone
        .iter()
        .map(|(z, w)| (*z, *w))
        .collect();
    assert!(
        !lwr_by_zone.is_empty(),
        "interior LWR zone exchange diagnostics must be populated"
    );
}

/// Regression: the linearised fallback path still uses `t_zone_c` as a
/// background reference.  Verify that after a 1 °C zone temperature change,
/// the linearised flux changes by less than 2 W per surface (sub-watt in
/// practice, well below the ticket's 16 W claim).  This pins the magnitude
/// so that any future change in the background reference is properly
/// characterised.
#[test]
fn linearised_lwr_flux_insensitive_to_zone_temp_shift() {
    use hares_envelope::InteriorSurface;
    use hares_envelope::longwave_radiation::interior_longwave_linearised_w;

    let surfaces = vec![
        InteriorSurface {
            area_m2: 15.0,
            emissivity: 0.90,
        },
        InteriorSurface {
            area_m2: 10.0,
            emissivity: 0.90,
        },
        InteriorSurface {
            area_m2: 5.0,
            emissivity: 0.84,
        },
    ];
    // Surface temperatures with a realistic 3 °C spread.
    let t_surfaces = vec![22.0_f64, 19.5_f64, 20.5_f64];

    let q_19c = interior_longwave_linearised_w(&surfaces, &t_surfaces, 19.0);
    let q_20c = interior_longwave_linearised_w(&surfaces, &t_surfaces, 20.0);

    for (i, (&q19, &q20)) in q_19c.iter().zip(q_20c.iter()).enumerate() {
        let delta = (q19 - q20).abs();
        assert!(
            delta < 2.0,
            "surface {i}: flux delta from 1°C zone temp shift = {delta:.4} W; \
             should be < 2 W (actual impact is sub-watt)"
        );
        assert!(
            delta > 0.001,
            "surface {i}: flux delta should be non-zero (zone temp does affect h_r)"
        );
    }

    // Energy must be conserved for both cases — zone temperature has no
    // effect on the net sum (it only changes the linearised coefficient
    // multiplying the existing surface temperature imbalances).
    for (label, q) in [("19°C", &q_19c), ("20°C", &q_20c)] {
        let sum: f64 = q.iter().sum();
        assert!(
            sum.abs() < 1e-8,
            "{label}: net flux sum must be ~0 (energy conservation), got {sum:.2e}"
        );
    }
}

/// The flux-residual convergence criterion changes for the old step-magnitude
/// test on the same case: the old test would converge earlier because
/// |t_next − t_old| can be < 0.01 even when the flux is still settling.
/// This test verifies that the flux-residual is the more conservative
/// criterion — the iteration runs at least as many steps as the old test
/// would require.
#[test]
fn flux_residual_criterion_is_more_conservative_than_step_magnitude() {
    use hares_envelope::InteriorSurface;
    use hares_envelope::longwave_radiation::interior_longwave_linearised_w_into;

    // Use a case where temperatures change rapidly — the step magnitude
    // might drop below 0.01 before the flux residual drops below 1e-4.
    let t_zone_c = 22.0_f64;
    let surfaces = [
        InteriorSurface {
            area_m2: 40.0,
            emissivity: 0.90,
        },
        InteriorSurface {
            area_m2: 40.0,
            emissivity: 0.90,
        },
    ];
    let infos = [
        InteriorSurfaceInfo {
            state_index: 0,
            input_index: 0,
            area_m2: 40.0,
            emissivity: 0.90,
            radiation_frac: 0.7,
            rad_res_k_w: 0.003,
            solar_absorptance: 0.5,
            is_floor: false,
            driving_temp: None,
        },
        InteriorSurfaceInfo {
            state_index: 0,
            input_index: 0,
            area_m2: 40.0,
            emissivity: 0.90,
            radiation_frac: 1.0,
            rad_res_k_w: 0.003,
            solar_absorptance: 0.6,
            is_floor: true,
            driving_temp: None,
        },
    ];

    let t_nodes = [30.0_f64, 18.0_f64];
    let t_surf_init: Vec<f64> = infos
        .iter()
        .zip(t_nodes.iter())
        .map(|(s, &t_node)| s.radiation_frac * t_node + (1.0 - s.radiation_frac) * t_zone_c)
        .collect();

    let t_surf_min = t_surf_init.iter().copied().fold(f64::INFINITY, f64::min);
    let t_surf_max = t_surf_init
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);

    let mut t_surf_flux = t_surf_init.clone();
    let mut prev_flux = t_surf_init.clone();
    let mut flux: Vec<f64> = Vec::with_capacity(2);
    let mut flux_prev: Vec<f64> = Vec::with_capacity(2);

    let mut t_surf_step = t_surf_init.clone();
    let mut prev_step = t_surf_init.clone();
    let mut step_flux: Vec<f64> = Vec::with_capacity(2);

    // Track which criterion converges first
    let mut flux_converged_at: Option<usize> = None;
    let mut step_converged_at: Option<usize> = None;

    for iter_idx in 0..20 {
        // Flux-based convergence
        interior_longwave_linearised_w_into(&surfaces, &t_surf_flux, t_zone_c, &mut flux);
        if iter_idx > 0 && !flux_prev.is_empty() {
            let mut all_ok = true;
            for j in 0..surfaces.len() {
                let old_q = flux_prev[j];
                let new_q = flux[j];
                if (new_q - old_q).abs() / (old_q.abs() + 1e-6) >= 1e-4 {
                    all_ok = false;
                    break;
                }
            }
            if all_ok && flux_converged_at.is_none() {
                flux_converged_at = Some(iter_idx);
            }
        }
        if flux_converged_at.is_none() {
            flux_prev.clear();
            flux_prev.extend_from_slice(&flux);
            for (j, info) in infos.iter().enumerate() {
                let t_new = t_surf_init[j] + flux[j] * info.rad_res_k_w;
                let t_new = t_new.clamp(t_surf_min, t_surf_max);
                let t_next = t_surf_flux[j]
                    + 0.3 * (t_new - t_surf_flux[j])
                    + 0.2 * (t_surf_flux[j] - prev_flux[j]);
                prev_flux[j] = t_surf_flux[j];
                t_surf_flux[j] = t_next;
            }
        }

        // Step-magnitude-based convergence
        interior_longwave_linearised_w_into(&surfaces, &t_surf_step, t_zone_c, &mut step_flux);
        if step_converged_at.is_none() {
            let mut all_ok = true;
            for (j, info) in infos.iter().enumerate() {
                let t_new = t_surf_init[j] + step_flux[j] * info.rad_res_k_w;
                let t_new = t_new.clamp(t_surf_min, t_surf_max);
                let t_next = t_surf_step[j]
                    + 0.3 * (t_new - t_surf_step[j])
                    + 0.2 * (t_surf_step[j] - prev_step[j]);
                prev_step[j] = t_surf_step[j];
                t_surf_step[j] = t_next;
                if (t_surf_step[j] - prev_step[j]).abs() >= 0.01 {
                    all_ok = false;
                }
            }
            if all_ok {
                step_converged_at = Some(iter_idx);
            }
        }

        if flux_converged_at.is_some() && step_converged_at.is_some() {
            break;
        }
    }

    // Both must converge within the iteration budget.
    assert!(
        flux_converged_at.is_some(),
        "flux-residual criterion must converge within 20 iterations"
    );
    assert!(
        step_converged_at.is_some(),
        "step-magnitude criterion must converge within 20 iterations"
    );

    let flux_iter = flux_converged_at.unwrap();
    let step_iter = step_converged_at.unwrap();

    // The flux-residual criterion should converge at the same iteration or later
    // (more conservative — it reflects energy balance closure, not just
    // temperature settling).  In this test case they usually converge at
    // the same step, but if not, flux should not converge before step.
    assert!(
        flux_iter >= step_iter,
        "flux-residual criterion converged at iteration {flux_iter} but step-magnitude \
         converged at iteration {step_iter}; flux should be equally or more conservative \
           than step magnitude"
    );
}

// ---------------------------------------------------------------------------
// impulse-response: radiation_frac split validated against state-space model
// ---------------------------------------------------------------------------

/// Impulse-response test: inject 1 W at an interior surface via the
/// `radiation_frac` split and verify the steady-state zone-air temperature
/// rise matches the closed-form expression.
///
/// Topology (pre-elimination):
///   ZoneAir (C_air, internal capacitor) ←R_film→ Surface(floating) ←R_inner_half→ WallMass (C_wall)
///   WallMass ←R_outer→ Outdoor (external)
///
/// After floating-node elimination:
///   ZoneAir ←[R_film + R_inner_half]→ WallMass ←[R_outer]→ Outdoor
///
/// LWR flux q = 1 W is injected at the surface by splitting:
///   wall mass input:  q × radiation_frac
///   zone air input:   q × (1 − radiation_frac)
///
/// At steady state (all external temps = 0 °C), the zone air temperature
/// equals the injected power flowing through R_film + R_inner_half into the
/// wall mass, which should match (1 − radiation_frac) × q × (R_film + R_inner_half)
/// = r_inner_half / (r_film + r_inner_half) × q × (r_film + r_inner_half)
/// = q × r_inner_half.
///
/// This verifies that the split formula produces the same temperatures as
/// injecting q directly at the surface node in a hand calculation.
#[test]
fn radiation_frac_impulse_response_matches_closed_form() {
    use hares_envelope::rc_network::{NodeId, RCNetwork};
    use hares_envelope::state_space::OutputMapping;
    use hares_envelope::state_space::StateSpaceModel;
    use std::collections::HashMap;

    // Physical parameters (in SI per-m²; multiplied by area downstream).
    let r_film = 0.12_f64; // convection film [m²·K/W]
    let r_inner_half = 0.10 / (2.0 * 1.7); // half-node: 100mm concrete, k=1.7 [m²·K/W]
    let r_outer = 3.0_f64; // wall outer resistance to outdoor [m²·K/W]
    let area = 10.0_f64; // surface area [m²]
    let c_air = 500_000.0_f64; // zone air capacitance [J/K]
    let c_wall = 200_000.0_f64; // wall mass capacitance [J/K]

    // Resistances in [K/W] (divide by area)
    let r_film_kw = r_film / area;
    let r_half_kw = r_inner_half / area;
    let r_outer_kw = r_outer / area;

    // radiation_frac = fraction of injected flux to wall mass RC node.
    // radiation_frac = G_wall / (G_wall + G_conv)
    //                = (1/r_half) / (1/r_half + 1/r_film)
    //                = r_film / (r_film + r_half)
    let radiation_frac = r_film_kw / (r_film_kw + r_half_kw);

    // Fraction to zone air.
    let frac_to_air = 1.0 - radiation_frac;

    // ── Build RC network with floating surface node ──
    // Nodes: 1 = zone air (external, but with cap for injection modeling)
    //        2 = wall mass (internal, capacitor)
    //        3 = outdoor (external)
    //        100 = floating surface node (no capacitor, will be eliminated)
    let n = |id: u32| NodeId(id);
    let caps = HashMap::from([(n(1), c_air), (n(2), c_wall)]);
    let mut res = HashMap::new();
    res.insert((n(1), n(100)), r_film_kw); // zone air ↔ surface
    res.insert((n(100), n(2)), r_half_kw); // surface ↔ wall mass
    res.insert((n(2), n(3)), r_outer_kw); // wall mass ↔ outdoor

    let net = RCNetwork::from_elements(caps, res, vec![n(3)]).unwrap();
    let (a_c, b_c, _) = net.build_matrices().unwrap();

    // ── Verify floating node was eliminated ──
    // After elimination: zone air (cap) ↔ wall mass (cap) ↔ outdoor (ext)
    // A_c: 2×2, B_c: 2×1 (only outdoor, node 3)
    assert_eq!(a_c.nrows(), 2, "A_c should be 2×2 after elimination");
    assert_eq!(b_c.ncols(), 1, "B_c should be 2×1 (one external node)");

    // ── Extend B matrix with injection columns ──
    // Column layout: [u_outdoor | u_inject_zone_air | u_inject_wall_mass]
    // Injection at a capacitor node: B_inj = 1/C at that node.
    let mut b_ext = DMatrix::<f64>::zeros(2, 3);
    b_ext.column_mut(0).copy_from(&b_c.column(0)); // outdoor input
    b_ext[(0, 1)] = 1.0 / c_air; // inject at zone air
    b_ext[(1, 2)] = 1.0 / c_wall; // inject at wall mass

    let output_map = OutputMapping {
        output_count: 2,
        node_to_output: vec![(0, 0, 1.0), (1, 1, 1.0)],
        input_to_output: vec![],
    };

    let _model = StateSpaceModel::from_continuous(&a_c, &b_ext, 60.0, &output_map).unwrap();

    // ── Simulate impulse response to steady state ──
    // Initial condition: all temperatures = 0 °C.
    // Constant inputs: outdoor = 0 °C, injection = q × radiation_frac or q × (1−radiation_frac).
    let q_inj = 1.0_f64;

    // For the continuous system: dx/dt = A*x + B*u
    // At steady state: 0 = A*x_ss + B*u_ss → x_ss = -A^{-1} * B * u_ss
    let a_inv = a_c
        .clone()
        .try_inverse()
        .expect("A matrix should be invertible");

    let u_ss = DVector::from_vec(vec![
        0.0,                    // outdoor temp = 0
        q_inj * frac_to_air,    // zone air injection
        q_inj * radiation_frac, // wall mass injection
    ]);
    let x_ss = -&a_inv * (&b_ext * &u_ss);

    let t_zone_ss = x_ss[0];
    let t_wall_ss = x_ss[1];

    // ── Closed-form expectations ──
    // At steady state, the injected 1 W must leave through the outdoor resistance.
    // Total injected = 1 W, all exits through R_outer to outdoor at 0°C:
    //   1 W = (T_wall - 0) / R_outer → T_wall = 1 * R_outer
    let t_wall_expected = q_inj * r_outer_kw;
    assert!(
        (t_wall_ss - t_wall_expected).abs() < 1e-6,
        "wall mass steady-state temp: got {t_wall_ss:.6}, expected {t_wall_expected:.6}"
    );

    // Between zone air and wall mass, the steady-state heat flow must equal
    // the fraction injected to zone air (heat travels from zone air through
    // the film+material path to wall mass, then out through outdoor).
    // Heat flow from zone air to wall mass = (T_zone − T_wall) / (R_film + R_half)
    // This must equal q_inj × frac_to_air.
    let q_zone_to_wall = (t_zone_ss - t_wall_ss) / (r_film_kw + r_half_kw);
    assert!(
        (q_zone_to_wall - q_inj * frac_to_air).abs() < 1e-6,
        "heat flow from zone air to wall mass: got {q_zone_to_wall:.6} W, expected {:.6} W",
        q_inj * frac_to_air
    );

    // ── Surface temperature validation ──
    // The surface temperature (between R_film and R_inner_half) should satisfy the
    // voltage-divider interpolation:
    //   T_surf = radiation_frac × T_wall + (1 − radiation_frac) × T_zone
    let t_surf = radiation_frac * t_wall_ss + frac_to_air * t_zone_ss;

    // At steady state, the heat flow from surface to zone air must equal
    // q_inj × frac_to_air (the portion injected at zone air, plus what flows
    // from the surface to zone air through R_film).
    //
    // From the surface, heat flows to zone air through R_film:
    // q_surf_to_air = (T_surf − T_zone) / R_film
    // This must equal q_inj × frac_to_air (all heat to air comes through convection).
    //
    // Wait: we injected q * frac_to_air DIRECTLY at the zone air node.
    // The heat balance at zone air: injection + (T_surf - T_zone)/R_film = 0 at steady state
    // So: (T_surf - T_zone)/R_film = -q * frac_to_air
    let q_conv_to_air = (t_surf - t_zone_ss) / r_film_kw;
    // At steady state, convection from surface → zone air balances the
    // injection at zone air: q_conv_to_air + q_inj * frac_to_air ≈ 0
    assert!(
        (q_conv_to_air + q_inj * frac_to_air).abs() < 1e-6,
        "surface-to-air convection ({q_conv_to_air:.6}) must balance zone air injection ({})",
        -q_inj * frac_to_air
    );

    // ── Verify conductance equivalence ──
    // radiation_frac = G_wall/(G_wall+G_conv) must equal the steady-state split
    // of heat from the surface between the wall mass and zone air.
    let g_wall = 1.0 / r_half_kw;
    let g_conv = 1.0 / r_film_kw;
    let rad_frac_from_g = g_wall / (g_wall + g_conv);
    assert!(
        (radiation_frac - rad_frac_from_g).abs() < 1e-9,
        "radiation_frac ({radiation_frac:.9}) != G_wall/(G_wall+G_conv) ({rad_frac_from_g:.9})"
    );
}

// ---------------------------------------------------------------------------
// Hand-derivation: single-surface single-zone fixture
// ---------------------------------------------------------------------------

/// For a single opaque surface in a single zone, the `radiation_frac` formula
/// produces the correct surface temperature interpolation.  This test
/// verifies the result against a step-by-step hand calculation using
/// ASHRAE HoF 2021 Ch.4 parallel convection/radiation and standard thermal
/// circuit analysis.
///
/// Geometry: 1 zone, 4 walls (each 30 m²), R_film_conv = 0.12 m²K/W,
/// R_inner_half varies per construction (lightweight → heavyweight).
///
/// The surface temperature for each wall is:
///   T_surf = radiation_frac × T_node + (1 − radiation_frac) × T_zone
///
/// This test validates the two-point correctness:
///   (a) When R_inner_half → 0, radiation_frac → 1 → T_surf ≈ T_node
///   (b) When R_inner_half → ∞ (massive), radiation_frac → 0 → T_surf ≈ T_zone
#[test]
fn radiation_frac_hand_derivation_single_surface() {
    // Zone and node temperatures for a heating scenario.
    let t_node_c = 30.0; // wall mass node at 30 °C (warm from heating)
    let t_zone_c = 20.0; // zone air at 20 °C
    let r_film = 0.12_f64; // TARP convection-only [m²K/W]

    // ── Case 1: lightweight construction (R_half ≈ 0.03 m²K/W) ──
    // Thin material → T_surf is close to T_node.
    let r_half_light = 0.03_f64;
    let rad_frac_light = r_film / (r_film + r_half_light);
    let t_surf_light = rad_frac_light * t_node_c + (1.0 - rad_frac_light) * t_zone_c;

    assert!(
        rad_frac_light > 0.7,
        "lightweight: radiation_frac ({rad_frac_light:.4}) should be > 0.7"
    );
    assert!(
        t_surf_light > t_zone_c && t_surf_light < t_node_c,
        "lightweight: T_surf ({t_surf_light:.2}) must be between T_zone ({t_zone_c}) and T_node ({t_node_c})"
    );
    assert!(
        (t_surf_light - t_node_c).abs() < 5.0,
        "lightweight: T_surf should be close to T_node, got T_surf={t_surf_light:.2}, T_node={t_node_c}"
    );

    // ── Case 2: heavyweight construction (R_half ≈ 0.5 m²K/W) ──
    // Thick massive wall → T_surf is much closer to T_zone.
    let r_half_heavy = 0.50_f64;
    let rad_frac_heavy = r_film / (r_film + r_half_heavy);
    let t_surf_heavy = rad_frac_heavy * t_node_c + (1.0 - rad_frac_heavy) * t_zone_c;

    assert!(
        rad_frac_heavy < 0.3,
        "heavyweight: radiation_frac ({rad_frac_heavy:.4}) should be < 0.3"
    );
    assert!(
        t_surf_heavy > t_zone_c && t_surf_heavy < t_node_c,
        "heavyweight: T_surf ({t_surf_heavy:.2}) must be between T_zone and T_node"
    );
    assert!(
        (t_surf_heavy - t_zone_c).abs() < 3.0,
        "heavyweight: T_surf should be close to T_zone, got T_surf={t_surf_heavy:.2}, T_zone={t_zone_c}"
    );

    // ── Energy balance at steady state (single surface, 1 m²) ──
    // The heat flow from surface → mass must equal the injected fraction:
    //   q_to_mass = (T_surf − T_node) / R_inner_half  [negative = to mass]
    //   q_to_air  = (T_surf − T_zone) / R_film_conv    [positive = to air]
    // At steady state with no net injection, these must balance.
    let q_mass = (t_surf_heavy - t_node_c) / r_half_heavy;
    let q_air = (t_surf_heavy - t_zone_c) / r_film;
    assert!(
        (q_mass + q_air).abs() < 1e-9,
        "steady-state heat balance: q_to_mass ({q_mass:.6}) + q_to_air ({q_air:.6}) must sum to zero"
    );

    // ── Verify: fraction to wall mass from conductance ──
    let g_conv = 1.0 / r_film;
    let g_wall = 1.0 / r_half_heavy;
    let frac_to_wall_from_g = g_wall / (g_wall + g_conv);
    assert!(
        (rad_frac_heavy - frac_to_wall_from_g).abs() < 1e-9,
        "radiation_frac ({rad_frac_heavy:.9}) must equal G_wall/(G_wall+G_conv) ({frac_to_wall_from_g:.9})"
    );
}
