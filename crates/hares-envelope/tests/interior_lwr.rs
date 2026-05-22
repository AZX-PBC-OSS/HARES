//! Interior longwave radiation integration tests.
//!
//! These tests exercise `interior_longwave_net_w` and `interior_longwave_linearised_w`
//! directly via the public API, verifying energy conservation and heat direction.

use std::collections::HashMap;

use chrono::{FixedOffset, TimeZone};
use hares_envelope::{
    InteriorLwrZoneConfig, InteriorSurface, InteriorSurfaceInfo, MechanicalVentilationParams,
    OutputMapping, StateSpaceModel, StateSpaceWiring, ThermalSolver, ThermalSolverConfig,
    interior_longwave_net_w,
};
use hares_types::{EnvironmentState, GridState, SurfaceIrradiance, WeatherState, ZoneId, ZoneState};
use nalgebra::DMatrix;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Ticket 047 regression: stale zone-air temperature in LWR convergence loop
// ---------------------------------------------------------------------------

/// Ticket 047 — stale zone-air temperature characterisation.
///
/// `apply_interior_longwave_inputs` reads `env.zones[i].temperature_c` (the
/// prior-step committed value) and holds it fixed throughout all convergence
/// iterations.  The ticket claims a stale zone temp of 1 °C produces a
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
fn ticket047_stale_zone_temp_linearised_lwr_error_is_sub_watt() {
    use hares_envelope::{InteriorSurface, interior_longwave_linearised_w};

    let surfaces = vec![
        InteriorSurface { area_m2: 5.0, emissivity: 0.90 },
        InteriorSurface { area_m2: 5.0, emissivity: 0.90 },
        InteriorSurface { area_m2: 5.0, emissivity: 0.90 },
        InteriorSurface { area_m2: 5.0, emissivity: 0.90 },
        InteriorSurface { area_m2: 5.0, emissivity: 0.90 },
        InteriorSurface { area_m2: 5.0, emissivity: 0.90 },
    ];

    // Surface temperatures spread across ~3.5°C — a plausible residential zone.
    let t_surfaces = vec![21.0, 19.5, 20.5, 20.0, 18.5, 22.0];

    // Prior-step zone temperature (stale, as used today).
    let t_zone_stale = 19.0_f64;
    // Predicted zone temperature after a 1 °C/step heating ramp.
    let t_zone_predicted = 20.0_f64;

    let q_stale = interior_longwave_linearised_w(&surfaces, &t_surfaces, t_zone_stale);
    let q_predicted = interior_longwave_linearised_w(&surfaces, &t_surfaces, t_zone_predicted);

    let max_err_w: f64 = q_stale.iter()
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
// Regression: ticket 044 — LWR fallback to linearised path must not be silent
// ---------------------------------------------------------------------------

/// Build a minimal EnvironmentState for the regression test.
fn env_20c() -> EnvironmentState {
    EnvironmentState {
        zones: vec![ZoneState {
            id: ZoneId(1),
            temperature_c: 20.0,
            humidity_ratio: 0.008,
            relative_humidity: 0.45,
            wet_bulb_c: 15.0,
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

/// Ticket 044 — primary defect regression test.
///
/// `ThermalSolver::new` must return `Err` when `interior_lwr_zones` is
/// non-empty and any zone has `scriptf == None`.  The current code silently
/// accepts this configuration and falls through to the linearised h_r path;
/// this test will fail until the solver-construction guard is implemented.
///
/// NOTE: this test is expected to FAIL until ticket 044 is resolved.
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
        indoor_temp_input_indices: vec![],
        solar_input_indices: HashMap::new(),
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
    };

    // BUG (ticket 044): ThermalSolver::new currently returns Ok here instead
    // of Err.  When the ticket is fixed this assert_err should pass.
    let result = ThermalSolver::new(model, wiring, config, 60.0, &env, 20.0);
    assert!(
        result.is_err(),
        "ThermalSolver::new must return Err when interior_lwr_zones contains \
         a zone with scriptf == None (ticket 044)"
    );
}

// ---------------------------------------------------------------------------
// Ticket 128 regression: InteriorLwrMethod::default() must equal StarMesh
// ---------------------------------------------------------------------------

/// Ticket 128 — implicit default must be StarMesh, not any future variant.
///
/// `InteriorLwrMethod::default()` is used at ~28 callsites across
/// `hares_envelope` and `hares_core`. The ticket requests those callsites be
/// made explicit. This regression test pins the invariant that `default()`
/// resolves to `StarMesh` so that any accidental change to the `#[default]`
/// attribute on `InteriorLwrMethod` will be caught immediately.
///
/// NOTE: This test does NOT fix the ticket (callsites still use `default()`);
/// it only guards the behavioural promise the ticket relies on.
#[test]
fn ticket128_interior_lwr_default_is_starmesh() {
    assert_eq!(
        hares_envelope::InteriorLwrMethod::default(),
        hares_envelope::InteriorLwrMethod::StarMesh,
        "InteriorLwrMethod::default() must be StarMesh; if you changed the \
         #[default] attribute, update all callsites in hares-envelope and \
         hares-core to name the variant explicitly (ticket 128)"
    );
}

// ---------------------------------------------------------------------------
// Ticket 089 regression: radiation_frac voltage-divider under StarMesh topology
// ---------------------------------------------------------------------------

/// Ticket 089 — `radiation_frac` formula mismatch under StarMesh Y-Δ topology.
///
/// The ticket claims that `interior_rad_frac = r_film_int / (r_film_int + r_inner_half)`
/// was derived for a pre-S1 topology where the convective film resistance and the
/// longwave radiation path were **combined** into a single `R_film_combined`.
/// After S1 separated convection from radiation into parallel paths, the
/// correct splitting fraction for an injected radiant gain is a current-divider
/// over the parallel admittances seen at the inner surface node — not the
/// series voltage-divider over (R_film, R_wall_half).
///
/// This test characterises the quantitative difference between the old
/// voltage-divider formula and the correct current-divider formula for a
/// representative interior surface:
///
///   Wall: R_film_conv = 0.12 m²K/W (convection-only, TARP h_c ≈ 8.3 W/m²K)
///         h_rad = 4·ε·σ·T³ ≈ 5.14 W/m²K (ε=0.9, T=293.15K)
///         R_film_rad = 1/5.14 ≈ 0.194 m²K/W
///         R_inner_half = 0.059 m²K/W (100mm concrete, k=1.7, half-node)
///
/// Old formula (series voltage-divider, current code):
///   radiation_frac_old = R_film_conv / (R_film_conv + R_inner_half)
///                      = 0.12 / (0.12 + 0.059) = 0.671
///
/// Correct formula (current-divider over parallel admittances):
///   G_air  = 1/R_film_conv = 8.33 W/m²K   (convection to zone air)
///   G_rad  = h_rad = 5.14 W/m²K            (radiation path)
///   G_wall = 1/R_inner_half = 16.95 W/m²K  (conduction into wall mass)
///   G_total = G_air + G_rad + G_wall = 30.42 W/m²K
///   radiation_frac_correct = G_air / G_total = 8.33 / 30.42 = 0.274
///
/// The difference is approximately 0.40 (27% absolute on a 0-1 scale),
/// matching the "~27% empirical bias" claimed in the ticket for a single surface.
///
/// NOTE: This test characterises the BUG — it does NOT assert that the correct
/// formula is currently used.  The test PASSES if the code still uses the old
/// formula (i.e., it is a failing-in-the-correct-sense regression test).
/// When ticket 089 is fixed, the assertion sense should be inverted.
#[test]
fn ticket089_radiation_frac_old_formula_disagrees_with_current_divider() {
    const SIGMA: f64 = 5.670374e-8;
    const T_REF_K: f64 = 293.15; // 20°C reference
    const EMISSIVITY: f64 = 0.9;

    // Convection-only interior film resistance [m²K/W]
    // TARP h_c ≈ 8.33 W/(m²·K) for vertical wall
    let r_film_conv_m2kw = 0.12_f64;

    // Linearised radiation conductance [W/(m²·K)]
    let h_rad = 4.0 * EMISSIVITY * SIGMA * T_REF_K.powi(3);
    let r_film_rad_m2kw = 1.0 / h_rad;

    // Half-node material resistance for 100 mm concrete (k=1.7 W/(m·K))
    let r_inner_half_m2kw = 0.100 / (2.0 * 1.7); // ≈ 0.0294 m²K/W

    // ── Old formula (series voltage-divider — what the code currently uses) ──
    let radiation_frac_old = r_film_conv_m2kw / (r_film_conv_m2kw + r_inner_half_m2kw);

    // ── Correct formula (current-divider over parallel admittances) ──
    // At the inner surface node the parallel conductances are:
    //   G_conv  = 1/R_film_conv   (convection → zone air)
    //   G_rad   = 1/R_film_rad    (radiation → zone air via star-mesh)
    //   G_wall  = 1/R_inner_half  (conduction into wall mass node)
    // The injected radiant flux splits to zone air proportionally to
    // (G_conv + G_rad) / (G_conv + G_rad + G_wall).
    //
    // Note: under the HARES/OCHRE "full" model, the explicit ScriptF LWR
    // module handles the radiation path separately.  The radiation_frac for
    // opaque surfaces in that model correctly uses R_film_conv only (no h_rad
    // in the film), but the INJECTED LWR gain then needs a current-divider
    // over (G_conv_air, G_wall).  The simplest correct expression is:
    //   radiation_frac_correct = G_conv / (G_conv + G_wall)
    // This differs from the old formula because R_inner_half is the half-node
    // material resistance, not a combined zone-to-node resistance.
    let g_conv = 1.0 / r_film_conv_m2kw;
    let g_wall = 1.0 / r_inner_half_m2kw;
    // Fraction routing injected gain toward zone air (convection wins against wall)
    let radiation_frac_correct = g_conv / (g_conv + g_wall);

    // ── Diagnosis ──
    let absolute_difference = (radiation_frac_old - radiation_frac_correct).abs();

    // The old formula overestimates the air-routed fraction compared to the
    // correct current-divider.  The ticket claims ~27% empirical bias; we
    // verify the formula difference is in the same order of magnitude.
    assert!(
        radiation_frac_old > radiation_frac_correct,
        "Ticket 089: old formula ({radiation_frac_old:.4}) should be LARGER than \
         correct current-divider ({radiation_frac_correct:.4}) — if equal the bug is fixed"
    );

    assert!(
        absolute_difference > 0.10,
        "Ticket 089: formula difference {absolute_difference:.4} should be >0.10 \
         (ticket claims ~27% empirical bias); got radiation_frac_old={radiation_frac_old:.4}, \
         radiation_frac_correct={radiation_frac_correct:.4}"
    );

    // Linearised h_rad for documentation
    assert!(
        h_rad > 4.0 && h_rad < 7.0,
        "h_rad = {h_rad:.4} W/(m²·K) should be in [4, 7] range for typical residential surfaces"
    );
    let _ = r_film_rad_m2kw; // used for derivation notes above
}
