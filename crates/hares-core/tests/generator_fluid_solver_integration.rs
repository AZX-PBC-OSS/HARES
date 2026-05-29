//! End-to-end integration test: generator → fluid port → FluidSolver.
//!
//! Verifies that a CHP generator with a fluid port produces self-consistent
//! port data (flow × Cp × ΔT = declared thermal_power_w) and that the fluid
//! solver invariant passes when consuming that data. This is the test the
//! T-0084 ticket explicitly required ("Wire a generator with fluid port into
//! a hydronic loop").

use std::time::Duration;

use chrono::{FixedOffset, TimeZone};

use hares_envelope::fluid_solver::{FluidSolver, FluidSolverConfig};
use hares_equipment::generator::{Generator, GeneratorConfig, GeneratorKind};
use hares_equipment::{Equipment, EquipmentConfig};
use hares_types::{
    ControlSignal, DomainSolver, EnvironmentState, FluidAccumulator, FluidDomainPayload, FluidType,
    GridState, LoopId, PortSlots, ThermalAccumulator, WeatherState, ZoneId, ZoneState,
};

fn env() -> EnvironmentState {
    EnvironmentState {
        zones: vec![ZoneState {
            id: ZoneId(1),
            temperature_c: 21.0,
            humidity_ratio: 0.008,
            relative_humidity: 0.5,
            wet_bulb_c: 15.0,
            volume_m3: 200.0,
        }],
        weather: WeatherState {
            outdoor_temp_c: 10.0,
            outdoor_humidity_ratio: 0.005,
            outdoor_wet_bulb_c: 7.0,
            outdoor_enthalpy_j_kg: 22_800.0,
            wind_speed_m_s: 2.0,
            wind_dir_deg: 0.0,
            ground_temp_c: 12.0,
            sky_temp_c: 7.0,
            pressure_kpa: 101.325,
            ghi_w_m2: 400.0,
            dni_w_m2: 300.0,
            dhi_w_m2: 100.0,
            solar_altitude_deg: 0.0,
            solar_azimuth_deg: 180.0,
            mains_temp_c: 15.0,
            solar_irradiance: vec![],
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
        equipment_core: std::collections::HashMap::new(),
        current_time: FixedOffset::east_opt(0)
            .expect("UTC offset")
            .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
            .single()
            .expect("valid UTC timestamp"),
        time_res: chrono::Duration::minutes(5),
        price_signal: Default::default(),
        electrical: Default::default(),
    }
}

#[test]
fn generator_fluid_solver_invariant_passes() {
    let loop_id = LoopId(42);

    // Build a generator with CHP active and a fluid port.
    let gen_cfg = GeneratorConfig {
        equipment_id: None,
        zone_id: Some(1),
        fuel_type: None,
        rated_power_kw: 10.0,
        eta_electric: Some(0.30),
        eta_thermal: Some(0.35),
        eta_jacket_water: None,
        eta_lube_oil: None,
        eta_exhaust: None,
        efficiency_type: None,
        efficiency_curve_points: None,
        delta_kw_per_s: Some(100.0),
        capacity_min_kw: None,
        grid_import_limit_kw: None,
        export_limit_kw: None,
        loop_id: Some(loop_id.0),
        flow_rate_kg_s: Some(0.2),
        supply_temp_c: Some(70.0),
        return_temp_c: Some(60.0),
        inverter_efficiency: None,
        stack_temp_c: None,
        stack_cooler_r0: None,
        stack_cooler_r1: None,
        stack_cooler_r2: None,
        stack_cooler_r3: None,
        stack_nominal_temp_c: None,
        heat_rec_max_temp_c: None,
    };
    let config = EquipmentConfig::from_typed(
        "Test CHP Gen".to_string(),
        "Gas Generator".to_string(),
        gen_cfg,
    );

    let mut generator = Generator::new(config.clone(), GeneratorKind::GasGenerator);
    generator.init(&config, &env()).unwrap();

    // Ramp to steady-state at 8 kW electrical.
    generator
        .apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: 8.0,
            reactive_power_kvar: None,
        })
        .unwrap();

    let mut slots = PortSlots {
        thermal: vec![ThermalAccumulator::new(ZoneId(1))],
        fluid: vec![FluidAccumulator::new(loop_id, FluidType::Water)],
        ..Default::default()
    };

    // Ramp: rated_power_kw = 10 kW, ramp_rate = 100 kW/s, so at 1s step
    // it reaches steady-state in 1 step. Use rated_power_kw + 1 with 1s timestep
    // to match the generator test helper.
    let ramp_steps = 11; // rated_power_kw (10) + 1
    for _ in 0..ramp_steps {
        slots.zero();
        generator
            .step(&env(), Duration::from_secs(1), &mut slots)
            .unwrap();
    }

    // Verify the generator produced thermal output to the fluid port.
    assert_eq!(slots.fluid.len(), 1);
    let declared = slots.fluid[0].total_thermal_power_w;
    assert!(
        declared > 0.0,
        "generator must declare thermal power to fluid port (got {declared} W)"
    );

    // Feed the port data into a FluidSolver. This triggers the T-0084 invariant
    // (debug_assert! in debug builds) that checks sum(declared_thermal_power_w)
    // against sum(flow × Cp × ΔT). If the invariant fires, the test panics.
    let mut solver =
        FluidSolver::new(FluidSolverConfig::default(), &[(loop_id, FluidType::Water)]).unwrap();

    let update = solver.resolve_new(&slots, &env(), Duration::from_secs(60));
    let states = FluidDomainPayload::decode(&update.custom_payload.unwrap()).unwrap();
    assert_eq!(states.len(), 1);

    let diff = (states[0].net_power_w - declared).abs();
    assert!(
        diff < 1.0,
        "fluid solver net_power_w ({net_w}) should match declared thermal_power_w ({declared}); diff = {diff}",
        net_w = states[0].net_power_w
    );
}
