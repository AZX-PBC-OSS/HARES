//! Zero-allocation proof for the thermal solver's hot loop.
//!
//! `thermal_solver_step_allocation_free_after_first_step` runs the dwelling's
//! per-step sequence (`prepare_inputs`, then three
//! `solve_ideal_capacity_for_target` calls for each of the two zones with a
//! sensible input, then `integrate`) and asserts that
//! the allocation bracket around the whole sequence reads zero over 100 steps
//! after the first. It runs twice, once with couplings active (the
//! identity-coupled scalar solve) and once with none (the uncoupled scalar
//! solve), and also brackets 100 calls of `ThermalSolver::resolve`.
//!
//! Zero allocations over a step therefore also proves zero factorizations:
//! every factorization in `hares-envelope` factors a dynamically sized
//! nalgebra matrix and allocates its result. The per-step slot-map refresh is
//! covered by `solar_slot_map_rebuilt_only_on_surface_change` in the solver's
//! unit tests: the map is rebuilt only when the surface-id sequence changes.
//!
//! This file must be a separate test binary because `#[global_allocator]`
//! applies to the whole binary and would interfere with other tests.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{FixedOffset, TimeZone};
use hares_envelope::{
    FilmCoefficientModel, InfiltrationMethod, InteriorLwrZoneConfig, InteriorSurfaceInfo,
    MechanicalVentilationParams, OutputMapping, StateSpaceModel, StateSpaceWiring, ThermalSolver,
    ThermalSolverConfig,
};
use hares_types::alloc_count::{CountingAllocator, thread_allocations};
use hares_types::{
    DomainSolver, DomainUpdate, EnvironmentState, GridState, PortSlots, SurfaceIrradiance,
    THERMAL_CATEGORY_COUNT, ThermalAccumulator, WeatherState, ZoneId, ZoneState,
};
use nalgebra::DMatrix;

// ---------------------------------------------------------------------------
// Counting allocator: the workspace-shared per-thread counter, installed for
// this test binary.
// ---------------------------------------------------------------------------

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_env(zone_temp: f64, outdoor_temp: f64) -> EnvironmentState {
    EnvironmentState {
        ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
        zones: vec![
            ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            },
            ZoneState {
                id: ZoneId(2),
                temperature_c: zone_temp - 2.0,
                humidity_ratio: 0.007,
                volume_m3: 120.0,
            },
        ],
        weather: WeatherState {
            outdoor_temp_c: outdoor_temp,
            outdoor_humidity_ratio: 0.004,
            wind_speed_m_s: 3.0,
            wind_dir_deg: 180.0,
            ground_temp_c: outdoor_temp,
            sky_temp_c: outdoor_temp - 5.0,
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
            island_bus_voltage_pu: None,
        },
        schedule_row: None,
        domains: hares_types::DomainSlots::default(),
        equipment_telemetry: HashMap::new(),
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

/// Build a two-zone `ThermalSolver` with two interior LWR surfaces in zone 1
/// so that `distribute_radiant_lwr_surfaces` is exercised on every step, and
/// a sensible input per zone whose B column drives that zone's air so
/// `solve_ideal_capacity_for_target` has a non-zero effective gain in each.
///
/// `infiltration` selects the coupling state of every step: non-empty produces
/// infiltration couplings (the identity-coupled solve path), empty gives the
/// uncoupled path.
fn make_solver(
    env: &EnvironmentState,
    infiltration: Vec<(ZoneId, InfiltrationMethod)>,
) -> ThermalSolver {
    // 4-state model: state 0 = zone 1 air, states 1 & 2 = wall nodes,
    // state 3 = zone 2 air.
    let a_c = DMatrix::from_row_slice(
        4,
        4,
        &[
            -1.0 / 50_000.0,
            0.0,
            0.0,
            0.0,
            0.0,
            -1.0 / 40_000.0,
            0.0,
            0.0,
            0.0,
            0.0,
            -1.0 / 30_000.0,
            0.0,
            0.0,
            0.0,
            0.0,
            -1.0 / 60_000.0,
        ],
    );
    // 5 inputs: outdoor temp, zone 1 sensible (HVAC), 2 surface heat inputs
    // (indexed at 2 and 3), and zone 2 sensible (HVAC). Each zone's sensible
    // column drives its air so the ideal-capacity solve's effective gain is
    // non-zero.
    let b_c = DMatrix::from_row_slice(
        4,
        5,
        &[
            1.0 / 50_000.0,
            1.0 / 50_000.0,
            0.0,
            0.0,
            0.0, // zone 1 air: outdoor + HVAC sensible drive
            0.0,
            0.0,
            1.0 / 40_000.0,
            0.0,
            0.0, // wall 1: surface input
            0.0,
            0.0,
            0.0,
            1.0 / 30_000.0,
            0.0, // wall 2: surface input
            1.0 / 60_000.0,
            0.0,
            0.0,
            0.0,
            1.0 / 60_000.0, // zone 2 air: outdoor + HVAC sensible drive
        ],
    );
    let mapping = OutputMapping {
        output_count: 2,
        node_to_output: vec![(0, 0, 1.0), (1, 3, 1.0)],
        input_to_output: vec![],
    };
    let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();

    let wiring = StateSpaceWiring {
        zone_state_indices: HashMap::from([(ZoneId(1), 0), (ZoneId(2), 3)]),
        zone_output_indices: HashMap::from([(ZoneId(1), 0), (ZoneId(2), 1)]),
        // Each zone's sensible input is its dedicated HVAC column.
        zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1), (ZoneId(2), 4)]),
        outdoor_temp_input_indices: vec![0],
        ground_temp_input_indices: vec![],
        ground_temp_input_depths_m: vec![],
        indoor_temp_input_indices: vec![],
        solar_input_indices: HashMap::new(),
        c_zone_j_k: HashMap::new(),
        node_capacitances: HashMap::new(),
        node_index: HashMap::new(),
    };

    let mut interior_lwr_zone = InteriorLwrZoneConfig {
        zone_id: ZoneId(1),
        surfaces: vec![
            InteriorSurfaceInfo {
                state_index: 1,
                input_index: 2,
                area_m2: 12.0,
                azimuth_deg: 180.0,
                tilt_deg: 90.0,
                emissivity: 0.90,
                radiation_frac: 0.8,
                rad_res_k_w: 250.0,
                solar_absorptance: 0.65,
                is_floor: false,
                driving_temp: None,
            },
            InteriorSurfaceInfo {
                state_index: 2,
                input_index: 3,
                area_m2: 8.0,
                azimuth_deg: 180.0,
                tilt_deg: 90.0,
                emissivity: 0.65,
                radiation_frac: 0.7,
                rad_res_k_w: 175.0,
                solar_absorptance: 0.40,
                is_floor: false,
                driving_temp: None,
            },
        ],
        scriptf: None,
    };
    interior_lwr_zone.compute_scriptf();

    let config = ThermalSolverConfig {
        indoor_zone_id: ZoneId(1),
        window_properties: HashMap::new(),
        window_zone_ids: HashMap::new(),
        window_ids_sorted: Vec::new(),
        exterior_surfaces: vec![],
        interior_lwr_zones: vec![interior_lwr_zone],
        infiltration,
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

    ThermalSolver::new(model, wiring, config, 60.0, env, env.zones[0].temperature_c).unwrap()
}

/// Ideal-capacity targets solved per zone per step: several equipment
/// sharing a zone, as `SolverFeedbackActor::collect_and_solve` runs them.
/// The first solve of a step fills the shared prefix; the later ones reuse
/// it and run only the target-dependent tail.
const ZONE_TARGETS_C: [f64; 3] = [20.0, 21.0, 22.0];

/// One step of the dwelling's sequence: `prepare_inputs`, then
/// `solve_ideal_capacity_for_target` for every target of every zone with a
/// sensible input, then `integrate`.
fn run_dwelling_step(
    solver: &mut ThermalSolver,
    sensible_zones: &[ZoneId],
    ports: &PortSlots,
    env: &EnvironmentState,
    out: &mut DomainUpdate,
) {
    solver.prepare_inputs(ports, env).unwrap();
    for &zone in sensible_zones {
        for target_c in ZONE_TARGETS_C {
            let _capacity = solver.solve_ideal_capacity_for_target(zone, target_c);
        }
    }
    solver.integrate(ports, env, out).unwrap();
}

// ---------------------------------------------------------------------------
// Zero-allocation test: the hot loop performs no heap allocation
// ---------------------------------------------------------------------------

/// The allocation bracket around the dwelling's per-step sequence
/// (`prepare_inputs` → three `solve_ideal_capacity_for_target` calls per
/// zone → `integrate`) reads ZERO over 100 steps after the first, with couplings
/// active (identity-coupled solve) and without (uncoupled solve), and around
/// 100 calls of `ThermalSolver::resolve`. Zero allocations also prove zero
/// factorizations: every factorization in `hares-envelope` factors a
/// dynamically sized nalgebra matrix and allocates its result.
#[test]
fn thermal_solver_step_allocation_free_after_first_step() {
    // Non-zero infiltration conductance keeps a coupling entry alive on every
    // step, which routes the ideal-capacity solve through the
    // identity-coupled path and the step through the coupled integrator.
    let couplings_active: Vec<(ZoneId, InfiltrationMethod)> = vec![
        (ZoneId(1), InfiltrationMethod::Ach { ach: 0.5 }),
        (ZoneId(2), InfiltrationMethod::Ach { ach: 0.3 }),
    ];
    let none: Vec<(ZoneId, InfiltrationMethod)> = Vec::new();

    for (couplings_active, infiltration) in [(false, none), (true, couplings_active)] {
        let scenario = if couplings_active {
            "couplings active (identity-coupled solve)"
        } else {
            "no couplings (uncoupled solve)"
        };

        let env = make_env(20.0, 0.0);
        let mut solver = make_solver(&env, infiltration);
        let mut out = DomainUpdate::empty(hares_types::THERMAL);
        // Every zone with a sensible input: the ideal-capacity solves run
        // for each of them, as the dwelling does between its two phases.
        let sensible_zones: Vec<ZoneId> = solver
            .wiring()
            .zone_sensible_input_indices
            .keys()
            .copied()
            .collect();
        assert_eq!(sensible_zones.len(), 2, "both zones must be solved");

        // Port with a non-zero radiant gain to ensure the distribution path is taken.
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator {
                zone: ZoneId(1),
                sensible_gain_w: 50.0,
                radiant_gain_w: 100.0,
                latent_gain_w: 0.0,
                shortwave_gain_w: 0.0,
                sensible_by_category: [0.0; THERMAL_CATEGORY_COUNT],
                radiant_by_category: [100.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                latent_by_category: [0.0; THERMAL_CATEGORY_COUNT],
            }],
            ..Default::default()
        };

        // Warm-up step: grows the last one-time buffers (coupling vecs, the
        // latent map's table, the output payload slot) so the bracket below
        // measures steady state.
        run_dwelling_step(&mut solver, &sensible_zones, &ports, &env, &mut out);

        // The per-thread counter cannot be reset, so the hot loop is counted
        // as a delta between two reads on this thread.
        let before = thread_allocations().expect("the test installs the counting allocator");
        for _ in 0..100 {
            run_dwelling_step(&mut solver, &sensible_zones, &ports, &env, &mut out);
        }
        let after = thread_allocations().expect("the test installs the counting allocator");
        let step_allocs = after - before;
        assert_eq!(
            step_allocs, 0,
            "{scenario}: the dwelling's per-step sequence (prepare_inputs, \
             three solve_ideal_capacity_for_target calls per zone, \
             integrate) allocated \
             {step_allocs} times over 100 steps after the first; expected \
             zero: a step of the thermal solver's hot loop must not touch \
             the heap (and therefore performs zero factorizations)."
        );

        // `resolve` (prepare + integrate, the non-dwelling caller path) must
        // be allocation-free over the same steady state.
        let before = thread_allocations().expect("the test installs the counting allocator");
        for _ in 0..100 {
            solver
                .resolve(&ports, &env, Duration::from_secs(60), &mut out)
                .unwrap();
        }
        let after = thread_allocations().expect("the test installs the counting allocator");
        let resolve_allocs = after - before;
        assert_eq!(
            resolve_allocs, 0,
            "{scenario}: ThermalSolver::resolve allocated {resolve_allocs} \
             times over 100 calls; expected zero (the hot loop must not \
             touch the heap)."
        );
    }
}
