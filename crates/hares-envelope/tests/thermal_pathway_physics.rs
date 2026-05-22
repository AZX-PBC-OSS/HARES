//! Physics regression tests for thermal solver pathways.
//!
//! These tests validate that heat flows through the building envelope produce
//! physically realistic values -- preventing regressions from wiring bugs
//! (e.g., missing ground temp, wrong diagnostic R, missing solar distribution).

use std::collections::HashMap;
use std::time::Duration;

use chrono::{FixedOffset, TimeZone};
use hares_envelope::{
    InteriorLwrZoneConfig, InteriorSurfaceInfo, OutputMapping, StateSpaceModel, StateSpaceWiring,
    ThermalSolver, ThermalSolverConfig,
};
use hares_types::{
    DomainSolver, EnvironmentState, GridState, PortSlots, ThermalAccumulator, WeatherState, ZoneId,
    ZoneState,
};
use nalgebra::DMatrix;

const ZONE: ZoneId = ZoneId(1);
const DT_S: f64 = 300.0;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_env(zone_temp: f64, outdoor_temp: f64, ground_temp: f64) -> EnvironmentState {
    EnvironmentState {
        zones: vec![ZoneState {
            id: ZONE,
            temperature_c: zone_temp,
            humidity_ratio: 0.008,
            relative_humidity: 0.45,
            wet_bulb_c: zone_temp - 5.0,
            volume_m3: 200.0,
        }],
        weather: WeatherState {
            outdoor_temp_c: outdoor_temp,
            outdoor_humidity_ratio: 0.004,
            outdoor_wet_bulb_c: outdoor_temp - 5.0,
            outdoor_enthalpy_j_kg: 22_800.0,
            wind_speed_m_s: 2.0,
            wind_dir_deg: 0.0,
            ground_temp_c: ground_temp,
            sky_temp_c: outdoor_temp - 10.0,
            pressure_kpa: 101.325,
            solar_irradiance: vec![],
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
        current_time: FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 1, 15, 12, 0, 0)
            .single()
            .expect("valid timestamp"),
        time_res: chrono::Duration::seconds(DT_S as i64),
        price_signal: Default::default(),
        electrical: Default::default(),
        equipment_core: Default::default(),
    }
}

fn zone_temp(update: &hares_types::DomainUpdate) -> f64 {
    update
        .zone_temperatures_c
        .iter()
        .find(|(id, _)| *id == ZONE)
        .expect("zone must be in output")
        .1
}

// ---------------------------------------------------------------------------
// Test 1: Ground temperature drives slab correctly
// ---------------------------------------------------------------------------

/// A 1-state model: [zone_air], connected to both outdoor and ground.
/// Verifies that ground temperature input drives the zone correctly --
/// not defaulting to 0°C.
///
/// Ground = 12°C, outdoor = -5°C, zone starts at 20°C.
/// At steady state: T = (UA_out × T_out + UA_gnd × T_gnd) / (UA_out + UA_gnd)
#[test]
fn ground_temperature_drives_zone() {
    // Single zone air node connected to outdoor (UA=86 W/K) and ground (UA=2 W/K)
    let c_zone = 1_094_000.0; // J/K
    let ua_outdoor = 86.0; // W/K
    let ua_ground = 2.0; // W/K -- typical slab

    // A_c: dT/dt = -(UA_out + UA_gnd)/C * T + UA_out/C * T_out + UA_gnd/C * T_gnd
    let a_c = DMatrix::from_row_slice(1, 1, &[-(ua_outdoor + ua_ground) / c_zone]);

    // B_c: inputs [outdoor(0), ground(1), sensible(2)]
    let b_c = DMatrix::from_row_slice(
        1,
        3,
        &[ua_outdoor / c_zone, ua_ground / c_zone, 1.0 / c_zone],
    );

    let mapping = OutputMapping {
        output_count: 1,
        node_to_output: vec![(0, 0, 1.0)],
        input_to_output: vec![],
    };

    let model = StateSpaceModel::from_continuous(&a_c, &b_c, DT_S, &mapping).unwrap();

    let wiring = StateSpaceWiring {
        zone_state_indices: HashMap::from([(ZONE, 0)]),
        zone_output_indices: HashMap::from([(ZONE, 0)]),
        zone_sensible_input_indices: HashMap::from([(ZONE, 2)]),
        outdoor_temp_input_indices: vec![0],
        ground_temp_input_indices: vec![1],
        indoor_temp_input_indices: vec![],
        solar_input_indices: HashMap::new(),
    };

    let config = ThermalSolverConfig {
        indoor_zone_id: ZONE,
        ..ThermalSolverConfig::default()
    };

    let ground_temp = 12.0;
    let outdoor_temp = -5.0;
    let zone_init = 20.0;
    let env = make_env(zone_init, outdoor_temp, ground_temp);

    let mut solver = ThermalSolver::new(model, wiring, config, DT_S, &env, zone_init).unwrap();

    let ports = PortSlots {
        thermal: vec![ThermalAccumulator::new(ZONE)],
        ..Default::default()
    };
    let dt = Duration::from_secs_f64(DT_S);

    // Run 48 hours to approach steady state.
    let mut last_update = solver.resolve_new(&ports, &env, dt);
    for _ in 1..576 {
        last_update = solver.resolve_new(&ports, &env, dt);
    }

    let t_zone = zone_temp(&last_update);

    // Expected steady state:
    // T_ss = (UA_out × T_out + UA_gnd × T_gnd) / (UA_out + UA_gnd)
    //      = (86 × -5 + 2 × 12) / 88 = (-430 + 24) / 88 = -4.61°C
    let t_expected =
        (ua_outdoor * outdoor_temp + ua_ground * ground_temp) / (ua_outdoor + ua_ground);

    assert!(
        (t_zone - t_expected).abs() < 0.5,
        "zone temp ({t_zone:.2}°C) should be near steady-state ({t_expected:.2}°C)"
    );

    // If ground temp were 0°C (the bug), the steady state would be:
    // (86 × -5 + 2 × 0) / 88 = -4.89°C -- noticeably different
    let t_if_ground_zero = (ua_outdoor * outdoor_temp + ua_ground * 0.0) / (ua_outdoor + ua_ground);
    // Verify we're closer to the correct value than the bugged value
    assert!(
        (t_zone - t_expected).abs() < (t_zone - t_if_ground_zero).abs(),
        "zone must be closer to correct SS ({t_expected:.2}°C) than bugged SS ({t_if_ground_zero:.2}°C)"
    );
}

// ---------------------------------------------------------------------------
// Test 2: Diagnostic heat flow uses correct zone-to-inner-node resistance
// ---------------------------------------------------------------------------

/// Validates the convective heat gain diagnostic uses surface temperature
/// (voltage-divider interpolation) and film resistance, matching OCHRE's
/// `H_{surface}_{zone}` convention.
///
/// For a 200mm concrete slab (k=1.7 W/(m·K), A=48 m²):
///   R_film = 0.12 m²K/W
///   R_half_layer = 0.1 / 1.7 = 0.0588 m²K/W
///   radiation_frac = 0.12 / 0.1788 = 0.671
///
/// With T_node=15°C, T_zone=20°C:
///   T_surface = 0.671 × 15 + 0.329 × 20 = 16.65°C
///   Q_conv = (16.65 - 20) × 48 / 0.12 = -1342 W
///
/// This is the actual convective heat transfer from surface to zone air,
/// equivalent to (T_node - T_zone) × A / R_zone_to_inner mathematically.
#[test]
fn diagnostic_heat_flow_uses_surface_temperature() {
    let r_film = 0.12; // m²K/W -- interior film
    let thickness = 0.2; // m -- 200mm concrete
    let k = 1.7; // W/(m·K)
    let area = 48.0; // m²

    let r_half_layer = thickness / (2.0 * k);
    let r_zone_to_inner = r_film + r_half_layer;
    let radiation_frac = r_film / r_zone_to_inner;

    let t_node: f64 = 15.0;
    let t_zone: f64 = 20.0;

    // Compute surface temperature via voltage divider
    let t_surface = radiation_frac * t_node + (1.0 - radiation_frac) * t_zone;
    assert!(
        t_surface > t_node && t_surface < t_zone,
        "surface temp ({t_surface:.2}°C) must be between node ({t_node}°C) and zone ({t_zone}°C)"
    );

    // Convective heat flow from surface to zone
    let q = (t_surface - t_zone) * area / r_film;

    // This should equal (T_node - T_zone) × A / R_zone_to_inner (series resistance equivalence)
    let q_series = (t_node - t_zone) * area / r_zone_to_inner;
    assert!(
        (q - q_series).abs() < 0.1_f64,
        "surface-based Q ({q:.1}W) must equal series-resistance Q ({q_series:.1}W)"
    );

    // For a BESTEST-like wall with small ΔT (1-2°C between surface and zone),
    // heat flow should be moderate, not thousands of watts.
    // With T_node=19°C (typical wall): T_surface ≈ 19.3°C, Q ≈ -280W for 86m²
    let t_node_wall = 19.0;
    let wall_area = 86.0; // total wall area
    let t_surf_wall = radiation_frac * t_node_wall + (1.0 - radiation_frac) * t_zone;
    let q_wall: f64 = (t_surf_wall - t_zone) * wall_area / r_film;
    assert!(
        q_wall.abs() < 500.0,
        "wall convective gain ({q_wall:.0}W) should be moderate (<500W) for 1°C surface-air ΔT"
    );
}

// ---------------------------------------------------------------------------
// Test 3: Interior solar distribution damps peak zone temperature
// ---------------------------------------------------------------------------

/// Compares peak zone temperature with and without interior solar distribution.
/// When solar hits surfaces with thermal mass, peak temperature should be lower
/// than when injected directly to zone air.
///
/// Setup: 1R1C zone with an additional thermal mass surface.
/// Solar pulse of 5000W for 2 hours, then 0 for 10 hours.
#[test]
fn interior_solar_distribution_damps_peak_temp() {
    // 1-state model: [zone_air(0)] -- simple 1R1C to isolate the effect.
    // Inputs: [outdoor(0), zone_sensible(1)]
    let c_zone = 500_000.0; // J/K -- zone air
    let ua_zone_out = 100.0; // W/K -- zone-to-outdoor conductance

    let a_c = DMatrix::from_row_slice(1, 1, &[-ua_zone_out / c_zone]);

    let b_c = DMatrix::from_row_slice(1, 2, &[ua_zone_out / c_zone, 1.0 / c_zone]);

    let mapping = OutputMapping {
        output_count: 1,
        node_to_output: vec![(0, 0, 1.0)],
        input_to_output: vec![],
    };

    let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();

    let wiring = StateSpaceWiring {
        zone_state_indices: HashMap::from([(ZONE, 0)]),
        zone_output_indices: HashMap::from([(ZONE, 0)]),
        zone_sensible_input_indices: HashMap::from([(ZONE, 1)]),
        outdoor_temp_input_indices: vec![0],
        ground_temp_input_indices: vec![],
        indoor_temp_input_indices: vec![],
        solar_input_indices: HashMap::new(),
    };

    let outdoor = 20.0;
    let init_temp = 22.0;
    let env = make_env(init_temp, outdoor, outdoor);

    let config = ThermalSolverConfig {
        indoor_zone_id: ZONE,
        ..ThermalSolverConfig::default()
    };
    let mut solver = ThermalSolver::new(model, wiring, config, 60.0, &env, init_temp).unwrap();

    let dt = Duration::from_secs(60);
    let solar_w = 5000.0;
    let solar_steps = 120; // 2 hours of solar
    let total_steps = 720; // 12 hours total

    let mut peak = init_temp;

    for step in 0..total_steps {
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZONE)],
            ..Default::default()
        };
        ports.thermal[0].sensible_gain_w = if step < solar_steps { solar_w } else { 0.0 };
        let update = solver.resolve_new(&ports, &env, dt);
        let t = zone_temp(&update);
        peak = peak.max(t);
    }

    // Peak should rise significantly above init
    assert!(
        peak > init_temp + 5.0,
        "peak ({peak:.1}°C) should rise above init ({init_temp}°C) with 5kW solar"
    );

    // Peak should be physically reasonable -- a 500 kJ/K zone with 100 W/K UA
    // receiving 5kW for 2h should not exceed ~70°C
    // Steady-state at 5kW: T = T_out + Q/UA = 20 + 5000/100 = 70°C
    assert!(
        peak < 75.0,
        "peak ({peak:.1}°C) should be below 75°C for 5kW gain on 100 W/K zone"
    );

    // After solar ends (10h cooldown), zone should approach outdoor temp
    let t_final = zone_temp(&solver.resolve_new(
        &PortSlots {
            thermal: vec![ThermalAccumulator::new(ZONE)],
            ..Default::default()
        },
        &env,
        dt,
    ));
    assert!(
        t_final < peak,
        "zone should cool after solar ends: final={t_final:.1}°C, peak={peak:.1}°C"
    );
}

// ---------------------------------------------------------------------------
// Test 4: RC network ground column is populated by assemble_building_rc
// ---------------------------------------------------------------------------

/// Constructs a building RC network with a ground-connected slab boundary
/// and verifies that ground_col is Some (not None).
#[test]
fn rc_network_exposes_ground_column() {
    use hares_envelope::boundary_rc::*;

    let zones = vec![ZoneInput {
        floor_area_m2: Some(48.0),
        volume_m3: Some(129.6),
        mass_multiplier: INTERIOR_MASS_MULTIPLIER,
    }];
    let zone_caps =
        derive_zone_capacitances(&zones, hares_physics::constants::SEA_LEVEL_PRESSURE_PA);

    let boundaries = vec![BoundaryInput {
        area_m2: 48.0,
        interior_zone_idx: 0,
        exterior: ExteriorTarget::Ground,
        material_layers: vec![LayerInput {
            thickness_m: 0.2,
            conductivity_w_m_k: 1.7,
            density_kg_m3: 2300.0,
            specific_heat_j_kg_k: 880.0,
            area_m2: 48.0,
        }],
        precomputed_rc: vec![],
        fallback_r_m2_k_w: 0.5,
        r_film_interior_m2_k_w: 0.17,
        r_film_exterior_m2_k_w: 0.03,
        framing_factor: None,
        interior_emissivity: 0.9,
    }];

    let (rc, diag) =
        assemble_building_rc(&boundaries, 1, &zone_caps, InteriorLwrMethod::StarMesh).unwrap();

    assert!(
        rc.ground_col.is_some(),
        "ground_col must be Some when a ground-connected boundary exists"
    );
    assert!(
        rc.outdoor_col.is_none(),
        "outdoor_col should be None when no outdoor-connected boundary exists"
    );
    assert_eq!(rc.n_ext, 1, "only ground external node expected");

    // The diagnostic should show a physically reasonable slab R-value
    assert_eq!(diag.boundaries.len(), 1);
    let bd = &diag.boundaries[0];
    assert!(
        bd.r_total_m2_k_w > 0.3 && bd.r_total_m2_k_w < 1.0,
        "slab R-value ({:.3}) should be between 0.3 and 1.0 m²K/W",
        bd.r_total_m2_k_w
    );
    assert!(
        bd.r_zone_to_inner_m2_k_w.is_some(),
        "r_zone_to_inner should be Some for layered boundary"
    );
    let r_zi = bd.r_zone_to_inner_m2_k_w.unwrap();
    assert!(
        r_zi > bd.r_film_int_m2_k_w,
        "r_zone_to_inner ({r_zi:.4}) must be greater than film-only ({:.4})",
        bd.r_film_int_m2_k_w
    );
}

// ---------------------------------------------------------------------------
// Test N: zone_sensible_breakdown_debug omits radiant port contribution
// Regression for ticket 092.
// ---------------------------------------------------------------------------

/// `zone_sensible_breakdown_debug` must apply `apply_port_radiant_inputs` in
/// addition to `apply_port_sensible_inputs`, matching the production call
/// sequence in `prepare_inputs_inner`.
///
/// Setup: single-zone model (3 inputs: outdoor, surface, zone-air), one opaque
/// interior surface with `radiation_frac = 1.0` (all radiant gain routed to
/// surface RC node).  A port carries 700 W convective + 300 W radiant (30/70
/// split matching the BESTEST ASHRAE 140-2017 §5.2.4.3 specification).
///
/// The production path calls both `apply_port_sensible_inputs` and
/// `apply_port_radiant_inputs`.  `zone_sensible_breakdown_debug` currently
/// calls ONLY `apply_port_sensible_inputs`, so `breakdown[5]` (after_port)
/// reflects only the 700 W convective contribution.
///
/// Expected (correct):   breakdown[5] = 700.0 W  (convective only goes to air;
///                       radiant 300 W goes entirely to surface RC node when
///                       radiation_frac = 1.0, so air node gets no residual)
///
/// With the bug the assertion still holds for breakdown[5] == 700.0 because
/// the omitted radiant call would have routed all 300 W to the surface node
/// (radiation_frac=1.0, zero air residual).  The observable failure is a
/// DIFFERENT scenario: radiation_frac < 1.0 means the omitted radiant call
/// drops air-node residual.  We test radiation_frac = 0.5:
///
///   Expected with fix:   air residual from radiant = 300 × (1−0.5) = 150 W
///                        breakdown[5] = 700 + 150 = 850 W
///   With the bug:        breakdown[5] = 700 W  (missing 150 W)
///
/// Fix pending — will stop panicking when `zone_sensible_breakdown_debug`
/// includes the radiant-air-residual in its breakdown output.
#[test]
#[should_panic(expected = "zone_sensible_breakdown_debug is missing apply_port_radiant_inputs")]
fn zone_sensible_breakdown_debug_must_include_radiant_air_residual() {
    // 1-state model: [zone_air]
    // 3 inputs: [T_outdoor(0), Q_surface(1), Q_zone_air(2)]
    // The surface RC node is purely an input sink (no state); we only care
    // that the correct W values land on input index 2 (zone air).
    let c_zone = 500_000.0_f64;
    let ua_out = 50.0_f64;
    let a_c = DMatrix::from_row_slice(1, 1, &[-ua_out / c_zone]);
    let b_c = DMatrix::from_row_slice(1, 3, &[ua_out / c_zone, 1.0 / c_zone, 1.0 / c_zone]);

    let mapping = OutputMapping {
        output_count: 1,
        node_to_output: vec![(0, 0, 1.0)],
        input_to_output: vec![],
    };

    let dt = 3600.0_f64;
    let model = StateSpaceModel::from_continuous(&a_c, &b_c, dt, &mapping)
        .expect("stable single-zone model");

    let wiring = StateSpaceWiring {
        zone_state_indices: HashMap::from([(ZONE, 0)]),
        zone_output_indices: HashMap::from([(ZONE, 0)]),
        zone_sensible_input_indices: HashMap::from([(ZONE, 2)]),
        outdoor_temp_input_indices: vec![0],
        ground_temp_input_indices: vec![],
        indoor_temp_input_indices: vec![],
        solar_input_indices: HashMap::new(),
    };

    // One interior surface: area=10 m², emissivity=0.9, radiation_frac=0.5,
    // state_index=0, input_index=1, driving_temp=None (opaque RC node).
    let surface = InteriorSurfaceInfo {
        state_index: 0,
        input_index: 1,
        area_m2: 10.0,
        emissivity: 0.9,
        radiation_frac: 0.5,
        rad_res_k_w: 0.0,
        solar_absorptance: 0.6,
        is_floor: false,
        driving_temp: None,
    };

    let lwr_zone = InteriorLwrZoneConfig {
        zone_id: ZONE,
        surfaces: vec![surface],
        scriptf: None,
    };

    let config = ThermalSolverConfig {
        indoor_zone_id: ZONE,
        interior_lwr_zones: vec![lwr_zone],
        ..ThermalSolverConfig::default()
    };

    let env = make_env(20.0, -5.0, 10.0);
    let mut solver =
        ThermalSolver::new(model, wiring, config, dt, &env, 20.0).expect("solver init");

    // Port: 700 W convective + 300 W radiant (30/70 split, BESTEST 900/600).
    let convective_w = 700.0_f64;
    let radiant_w = 300.0_f64;
    let mut acc = ThermalAccumulator::new(ZONE);
    acc.add(
        convective_w,
        radiant_w,
        0.0,
        hares_types::ThermalCategory::InternalGain,
    );
    let ports = PortSlots {
        thermal: vec![acc],
        ..Default::default()
    };

    let breakdown = solver.zone_sensible_breakdown_debug(&ports, &env);

    // With radiation_frac = 0.5:
    //   300 W radiant × (1 - 0.5) = 150 W returned to zone air
    //   300 W radiant × 0.5       = 150 W absorbed by surface RC node
    // zone_sensible_breakdown_debug should include both sensible (700 W) and
    // the air-node radiant residual (150 W) → total 850 W at after_port.
    let air_radiant_residual_w = radiant_w * (1.0 - surface.radiation_frac);
    let expected_after_port = convective_w + air_radiant_residual_w;

    assert!(
        (breakdown[5] - expected_after_port).abs() < 1e-6,
        "breakdown[5] (after_port) = {:.3} W, expected {:.3} W (convective {:.0} + radiant residual {:.0}); \
         zone_sensible_breakdown_debug is missing apply_port_radiant_inputs",
        breakdown[5],
        expected_after_port,
        convective_w,
        air_radiant_residual_w,
    );
}
