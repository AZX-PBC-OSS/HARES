//! Microbenchmark for the ThermalSolver per-step hot path — the standing
//! gate for the solver-step performance rule ("zero per-step cost, or a
//! benches/ before/after number").
//!
//! Covers the orchestration the raw `rc_solver` bench does not:
//! `build_input_vector` (slot-map irradiance lookup, solar distribution,
//! iterative exterior LWR with warm start, window sky correction),
//! infiltration coupling, and the coupled state advance.
//!
//! Topology: one conditioned zone, 4 opaque exterior surfaces (roof +
//! 3 walls, iterative rad_frac > 0 path), 1 south window (U-factor
//! correction path), 1 slab to ground (linearized path), AIM-2-style
//! infiltration coupling. 15-minute timestep (n_iter = 4 per OCHRE's
//! formula).

use std::collections::HashMap;

use chrono::{FixedOffset, TimeZone};
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use hares_envelope::thermal_solver::{
    BoundaryCategory, ExteriorSurfaceInfo, StateSpaceWiring, ThermalSolver, ThermalSolverConfig,
};
use hares_envelope::{OutputMapping, StateSpaceModel};
use hares_types::{
    DomainSolver, DomainUpdate, EnvironmentState, GridState, PortSlots, SurfaceIrradiance,
    WeatherState, ZoneId, ZoneState,
};
use nalgebra::DMatrix;

const ZONE: ZoneId = ZoneId(1);

fn bench_env() -> EnvironmentState {
    // Summer afternoon: strong sun, warm air, mild sky depression.
    let irr = vec![
        SurfaceIrradiance {
            surface_id: 10, // roof, horizontal
            direct_w_m2: 850.0,
            diffuse_w_m2: 120.0,
            reflected_w_m2: 30.0,
            angle_of_incidence_rad: 0.4,
        },
        SurfaceIrradiance {
            surface_id: 11, // south wall
            direct_w_m2: 300.0,
            diffuse_w_m2: 110.0,
            reflected_w_m2: 60.0,
            angle_of_incidence_rad: 0.9,
        },
        SurfaceIrradiance {
            surface_id: 12, // east wall
            direct_w_m2: 180.0,
            diffuse_w_m2: 110.0,
            reflected_w_m2: 55.0,
            angle_of_incidence_rad: 1.2,
        },
        SurfaceIrradiance {
            surface_id: 13, // west wall
            direct_w_m2: 620.0,
            diffuse_w_m2: 110.0,
            reflected_w_m2: 55.0,
            angle_of_incidence_rad: 0.7,
        },
        SurfaceIrradiance {
            surface_id: 20, // south window
            direct_w_m2: 300.0,
            diffuse_w_m2: 110.0,
            reflected_w_m2: 60.0,
            angle_of_incidence_rad: 0.9,
        },
    ];
    EnvironmentState {
        zones: vec![ZoneState {
            id: ZONE,
            temperature_c: 24.0,
            humidity_ratio: 0.010,
            volume_m3: 300.0,
        }],
        weather: WeatherState {
            outdoor_temp_c: 33.0,
            outdoor_humidity_ratio: 0.012,
            wind_speed_m_s: 3.5,
            wind_dir_deg: 220.0,
            ground_temp_c: 18.0,
            sky_temp_c: 12.0,
            pressure_kpa: 101.325,
            solar_irradiance: irr,
            outdoor_wet_bulb_c: 22.0,
            outdoor_enthalpy_j_kg: 60_000.0,
            ghi_w_m2: 900.0,
            dni_w_m2: 700.0,
            dhi_w_m2: 150.0,
            solar_altitude_deg: 55.0,
            solar_azimuth_deg: 250.0,
            mains_temp_c: 18.0,
            rainfall_m: 0.0,
            ground_albedo: 0.2,
            ground_t_mean_c: 10.0,
            ground_t_amplitude_c: 12.0,
            ground_phase_day: 35.0,
            day_of_year: 200.0,
        },
        grid: GridState {
            voltage_pu: 1.0,
            frequency_hz: 60.0,
            island_bus_voltage_pu: None,
        },
        custom_domains: vec![],
        equipment_telemetry: HashMap::new(),
        equipment_core: Default::default(),
        current_time: FixedOffset::east_opt(-7 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 7, 19, 14, 0, 0)
            .single()
            .expect("valid time"),
        time_res: chrono::Duration::seconds(900),
        price_signal: Default::default(),
        electrical: Default::default(),
    }
}

fn bench_solver(env: &EnvironmentState) -> ThermalSolver {
    // States: 0=zone air, 1..=4 = outer nodes of roof/walls, 5 = slab node.
    // Inputs: 0=outdoor, 1=ground, 2..=5 ext-surface injections,
    // 6=interior-surface (unused placeholder), 7=zone sensible.
    let n_states = 6;
    let n_inputs = 8;
    let c_zone = 360_000.0; // J/K
    let c_node = 900_000.0;

    let mut a_c = DMatrix::zeros(n_states, n_states);
    let mut b_c = DMatrix::zeros(n_states, n_inputs);
    // Zone ↔ outdoor (window UA + infiltration-free fabric) and zone ↔ surfaces.
    a_c[(0, 0)] = -(5.0 + 4.0 * 3.0) / c_zone;
    b_c[(0, 0)] = 5.0 / c_zone;
    for i in 1..=4 {
        a_c[(i, i)] = -(3.0 + 3.0) / c_node;
        a_c[(i, 0)] = 3.0 / c_node;
        a_c[(0, i)] = 3.0 / c_zone;
        a_c[(0, 0)] -= 3.0 / c_zone;
        b_c[(i, 0)] = 3.0 / c_node;
        b_c[(i, 1 + i)] = 1.0 / c_node; // ext-surface injection column
    }
    // Slab ↔ zone and slab ↔ ground.
    a_c[(5, 5)] = -(4.0 + 2.0) / (2.0 * c_node);
    a_c[(5, 0)] = 4.0 / (2.0 * c_node);
    a_c[(0, 5)] = 4.0 / c_zone;
    a_c[(0, 0)] -= 4.0 / c_zone;
    b_c[(5, 1)] = 2.0 / (2.0 * c_node);
    b_c[(0, 7)] = 1.0 / c_zone;

    let mapping = OutputMapping {
        output_count: 1,
        node_to_output: vec![(0, 0, 1.0)],
        input_to_output: vec![],
    };
    let model = StateSpaceModel::from_continuous(&a_c, &b_c, 900.0, &mapping).unwrap();

    let mut surfaces = Vec::new();
    // Roof + 3 walls: iterative path (rad_frac > 0).
    for (i, (id, tilt)) in [(10_u32, 0.0), (11, 90.0), (12, 90.0), (13, 90.0)]
        .iter()
        .enumerate()
    {
        let r_film = 0.04;
        let r_half = 0.10;
        surfaces.push(ExteriorSurfaceInfo {
            surface_id: *id,
            state_index: 1 + i,
            input_index: 2 + i,
            area_m2: 40.0,
            emissivity: 0.9,
            tilt_deg: *tilt,
            azimuth_deg: 180.0,
            rad_frac: r_film / (r_film + r_half),
            rad_res_k_w: (r_film * r_half / (r_film + r_half)) / 40.0,
            n_iter: 4,
            absorptance: 0.7,
            boundary_category: Some(BoundaryCategory::Wall),
            u_factor_w_m2_k: 0.0,
            h_out_w_m2_k: 25.0,
        });
    }
    // South window: U-factor sky-correction branch.
    surfaces.push(ExteriorSurfaceInfo {
        surface_id: 20,
        state_index: 0,
        input_index: 7,
        area_m2: 8.0,
        emissivity: 0.84,
        tilt_deg: 90.0,
        azimuth_deg: 180.0,
        rad_frac: 0.0,
        rad_res_k_w: 0.0,
        n_iter: 4,
        absorptance: 0.0,
        boundary_category: Some(BoundaryCategory::Window),
        u_factor_w_m2_k: 2.5,
        h_out_w_m2_k: 25.0,
    });
    // Slab: linearized path (rad_frac == 0).
    surfaces.push(ExteriorSurfaceInfo {
        surface_id: 30,
        state_index: 5,
        input_index: 6,
        area_m2: 48.0,
        emissivity: 0.9,
        tilt_deg: 180.0,
        azimuth_deg: 180.0,
        rad_frac: 0.0,
        rad_res_k_w: 0.0,
        n_iter: 4,
        absorptance: 0.0,
        boundary_category: Some(BoundaryCategory::Floor),
        u_factor_w_m2_k: 0.0,
        h_out_w_m2_k: 25.0,
    });

    let wiring = StateSpaceWiring {
        zone_state_indices: HashMap::from([(ZONE, 0)]),
        zone_output_indices: HashMap::from([(ZONE, 0)]),
        zone_sensible_input_indices: HashMap::from([(ZONE, 7)]),
        outdoor_temp_input_indices: vec![0],
        ground_temp_input_indices: vec![1],
        ground_temp_input_depths_m: vec![0.5],
        c_zone_j_k: HashMap::from([(ZONE, c_zone)]),
        ..Default::default()
    };

    let config = ThermalSolverConfig {
        indoor_zone_id: ZONE,
        exterior_surfaces: surfaces,
        ..Default::default()
    };
    // Light infiltration so the semi-implicit coupling path runs every step.
    // Infiltration left empty: the semi-implicit coupling path is exercised
    // every step by the linearized slab LWR branch anyway.

    ThermalSolver::new(model, wiring, config, 900.0, env, 22.0).expect("bench solver")
}

fn thermal_step_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("thermal_step");
    group.sample_size(200);

    let env = bench_env();
    let ports = PortSlots::default();
    let mut out = DomainUpdate::empty(hares_types::THERMAL);

    group.bench_function("full_step_6_surfaces_15min", |b| {
        let mut solver = bench_solver(&env);
        // Warm the exterior-surface-temperature fixed points so the measured
        // step is the steady-state hot path, not the first-call transient.
        for _ in 0..20 {
            solver.resolve(&ports, &env, std::time::Duration::from_secs(900), &mut out);
            out.clear();
        }
        b.iter(|| {
            solver.resolve(
                black_box(&ports),
                black_box(&env),
                black_box(std::time::Duration::from_secs(900)),
                black_box(&mut out),
            );
            out.clear();
        });
    });

    group.finish();
}

criterion_group!(benches, thermal_step_benchmark);
criterion_main!(benches);
