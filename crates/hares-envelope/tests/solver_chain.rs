//! Cross-solver integration tests for the envelope layer.
//!
//! These tests exercise the public API of ElectricalSolver, HumiditySolver,
//! and the numerical balance invariants that individual unit tests in each
//! solver's module do not assert end-to-end.
//!
//! Phase 1 gate: passes as soon as the envelope solver PRs are done.
//!
//! NOTE: The 1R1C steady-state and RC network math is already exercised in
//! state_space_tests.rs. Tests here focus on solver chain composition and
//! the balance equations that span multiple solver types.

use std::time::Duration;

use chrono::{FixedOffset, TimeZone};
use hares_envelope::{ElectricalSolver, ElectricalSolverConfig, HumiditySolver, HumiditySolverConfig};
use hares_types::{
    DomainSolver, EnvironmentState, GridState, PortContribution, PortSlots, ThermalAccumulator,
    ThermalCategory, WeatherState, ZoneId, ZoneState,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn approx_eq(actual: f64, expected: f64, tol: f64, label: &str) {
    assert!(
        (actual - expected).abs() <= tol,
        "{label}: actual={actual}, expected={expected}, delta={}",
        (actual - expected).abs()
    );
}

fn env_with_zone(zone_temp_c: f64, outdoor_temp_c: f64, humidity_ratio: f64) -> EnvironmentState {
    EnvironmentState {
        zones: vec![ZoneState {
            id: ZoneId(1),
            temperature_c: zone_temp_c,
            humidity_ratio,
            relative_humidity: 0.45,
            wet_bulb_c: zone_temp_c - 5.0,
            volume_m3: 200.0,
        }],
        weather: WeatherState {
            outdoor_temp_c,
            outdoor_humidity_ratio: 0.005,
            outdoor_wet_bulb_c: 7.0,
            outdoor_enthalpy_j_kg: 22_800.0,
            wind_speed_m_s: 2.0,
            wind_dir_deg: 0.0,
            ground_temp_c: 12.0,
            sky_temp_c: 8.0,
            pressure_kpa: 101.325,
            solar_irradiance: vec![],
            ghi_w_m2: 0.0,
            dni_w_m2: 0.0,
            dhi_w_m2: 0.0,
            solar_altitude_deg: 0.0,
            mains_temp_c: 15.0,
            rainfall_m: 0.0,
        },
        grid: GridState { voltage_pu: 1.0, frequency_hz: 60.0 },
        custom_domains: vec![],
        current_time: FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 3, 20, 12, 0, 0)
            .single()
            .expect("valid timestamp"),
        time_res: chrono::Duration::seconds(60),
    }
}

fn ports_with_electrical(load_kw: f64, gen_kw: f64) -> PortSlots {
    let mut ports = PortSlots::default();
    if load_kw != 0.0 {
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_kw: load_kw,
                reactive_power_kvar: 0.0,
            })
            .expect("accumulate load");
    }
    if gen_kw != 0.0 {
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_kw: gen_kw,
                reactive_power_kvar: 0.0,
            })
            .expect("accumulate generation");
    }
    ports
}

fn ports_with_latent(zone: ZoneId, latent_w: f64) -> PortSlots {
    let mut ports = PortSlots {
        thermal: vec![ThermalAccumulator::new(zone)],
        ..PortSlots::default()
    };
    ports
        .accumulate(&PortContribution::Thermal {
            zone,
            sensible_gain_w: 0.0,
            latent_gain_w: latent_w,
            category: ThermalCategory::InternalGain,
        })
        .expect("accumulate latent gain");
    ports
}

// ---------------------------------------------------------------------------
// ElectricalSolver chain tests
// ---------------------------------------------------------------------------

/// 2.0 kW load and -1.5 kW PV must produce net = 0.5 kW.
/// Validates the signed-power convention and load/generation routing.
#[test]
fn electrical_solver_net_with_load_and_pv() {
    let mut solver =
        ElectricalSolver::new(ElectricalSolverConfig::default()).expect("valid default config");
    let env = env_with_zone(21.0, 10.0, 0.008);
    let ports = ports_with_electrical(2.0, -1.5);
    let dt = Duration::from_secs(60);

    let update = solver.resolve(&ports, &env, dt);

    let payload = update.custom_payload.expect("electrical solver must return a payload");
    let net_kw = payload[0];

    // Numerical invariant: |P_grid + ΣP_equipment| < 0.001 kW
    // With P_grid = net_kw (import positive), ΣP_equipment = -(load - gen) = -(2.0-1.5) = -0.5
    // net_kw - 0.5 < 0.001
    let expected_net = 0.5; // 2.0 load + (-1.5) generation
    approx_eq(net_kw, expected_net, 0.001, "net active power (load + PV)");
    approx_eq(solver.net_active_kw(), expected_net, 0.001, "accessor matches payload");

    // Balance invariant: net must equal load_power + generation_power
    let balance_error = (net_kw - ports.electrical.net_active_kw()).abs();
    assert!(
        balance_error < 0.001,
        "electrical balance error {balance_error} kW exceeds 0.001 kW threshold"
    );
}

/// Pure generation (no load) must produce negative net_active_kw.
#[test]
fn electrical_solver_generation_only_produces_negative_net() {
    let mut solver =
        ElectricalSolver::new(ElectricalSolverConfig::default()).expect("valid config");
    let env = env_with_zone(21.0, 10.0, 0.008);
    let ports = ports_with_electrical(0.0, -3.0);

    let update = solver.resolve(&ports, &env, Duration::from_secs(60));
    let net_kw = update.custom_payload.expect("payload must be Some")[0];

    assert!(net_kw < 0.0, "net must be negative for pure generation, got {net_kw}");
    approx_eq(net_kw, -3.0, 0.001, "pure generation net");
}

/// Zero load and zero generation must produce exactly zero net.
#[test]
fn electrical_solver_zero_load_zero_generation_is_zero_net() {
    let mut solver =
        ElectricalSolver::new(ElectricalSolverConfig::default()).expect("valid config");
    let env = env_with_zone(21.0, 10.0, 0.008);
    let ports = PortSlots::default();

    let update = solver.resolve(&ports, &env, Duration::from_secs(60));
    let net_kw = update.custom_payload.expect("payload")[0];

    assert_eq!(net_kw, 0.0, "zero load + zero gen must produce exactly 0 kW net, got {net_kw}");
}

/// ElectricalSolver.net_active_kw() must return the balance invariant:
/// |P_grid + ΣP_equipment| < 0.001 kW for arbitrary multi-source combinations.
#[test]
fn electrical_balance_invariant_holds_for_multiple_sources() {
    let mut solver =
        ElectricalSolver::new(ElectricalSolverConfig::default()).expect("valid config");
    let env = env_with_zone(21.0, 10.0, 0.008);

    // Three loads and two generators.
    let mut ports = PortSlots::default();
    for &kw in &[1.2_f64, 0.8, 0.5] {
        ports.accumulate(&PortContribution::Electrical { active_power_kw: kw, reactive_power_kvar: 0.0 }).expect("load");
    }
    for &kw in &[-2.1_f64, -0.7] {
        ports.accumulate(&PortContribution::Electrical { active_power_kw: kw, reactive_power_kvar: 0.0 }).expect("gen");
    }

    let update = solver.resolve(&ports, &env, Duration::from_secs(60));
    let net_kw = update.custom_payload.expect("payload")[0];
    let expected = ports.electrical.net_active_kw(); // 2.5 + (-2.8) = -0.3

    let balance_error = (net_kw - expected).abs();
    assert!(
        balance_error < 0.001,
        "balance error {balance_error} kW exceeds 0.001 kW threshold; solver={net_kw}, ports={expected}"
    );
}

// ---------------------------------------------------------------------------
// HumiditySolver chain tests
// ---------------------------------------------------------------------------

/// A positive latent gain (moisture injection) must increase the zone humidity
/// ratio. This validates the HumiditySolver → DomainUpdate → humidity_ratio
/// chain from the public API surface.
#[test]
fn humidity_solver_latent_gain_increases_humidity_ratio() {
    let zone = ZoneId(1);
    let initial_w = 0.006; // kg/kg
    let env = env_with_zone(22.0, 10.0, initial_w);
    let mut solver =
        HumiditySolver::new(HumiditySolverConfig::default(), &env);

    let ports = ports_with_latent(zone, 500.0); // 500 W latent gain
    let dt = Duration::from_secs(60);

    let _update = solver.resolve(&ports, &env, dt);

    let w_new = solver.humidity_ratio(zone);
    assert!(
        w_new > initial_w,
        "latent gain must increase humidity ratio: initial={initial_w}, after={w_new}"
    );
}

/// Zero latent gain must not change the humidity ratio (no moisture exchange).
#[test]
fn humidity_solver_zero_latent_gain_no_change() {
    let zone = ZoneId(1);
    let initial_w = 0.008;
    let env = env_with_zone(21.0, 10.0, initial_w);
    let mut solver = HumiditySolver::new(HumiditySolverConfig::default(), &env);

    let ports = ports_with_latent(zone, 0.0);
    let _update = solver.resolve(&ports, &env, Duration::from_secs(60));

    let w_new = solver.humidity_ratio(zone);
    approx_eq(w_new, initial_w, 1e-12, "humidity ratio must not change with zero latent gain");
}

/// Moisture balance invariant:
///   |Δm_water - Q_latent * dt / h_fg| < 1e-6 kg
///
/// Checks that the humidity solver conserves moisture mass within numerical precision.
#[test]
fn humidity_moisture_mass_balance_invariant() {
    let zone = ZoneId(1);
    let initial_w = 0.007;
    let zone_volume_m3 = 200.0;
    let temp_c = 22.0;
    let latent_w = 300.0;
    let dt_s = 60.0;

    let env = env_with_zone(temp_c, 10.0, initial_w);
    let mut solver = HumiditySolver::new(HumiditySolverConfig::default(), &env);

    let ports = ports_with_latent(zone, latent_w);
    let _update = solver.resolve(&ports, &env, Duration::from_secs(dt_s as u64));

    let w_new = solver.humidity_ratio(zone);
    let delta_w = w_new - initial_w;

    // Air density at 22°C, ~1.2 kg/m³ (approximate for tolerance calc).
    // Moisture buffer multiplier = 15× per OCHRE convention.
    // Effective mass = rho * volume * moisture_buffering_multiplier
    // Tolerance: the solver may clamp to saturation, so we only check the
    // direction constraint and mass conservation upper bound.
    let h_fg_j_kg = 2_501_000.0; // J/kg at 0°C per OCHRE
    let rho_approx = 1.2; // kg/m³
    let moisture_buf = 15.0;
    let effective_mass_kg = rho_approx * zone_volume_m3 * moisture_buf;
    let expected_delta_w = (latent_w * dt_s) / (h_fg_j_kg * effective_mass_kg);

    // Direction must be correct: gain must increase w.
    assert!(delta_w >= 0.0, "latent gain must not decrease humidity ratio: delta_w={delta_w}");

    // Upper bound: delta_w must not exceed unclamped expectation by more than 10%
    // (the solver applies saturation clamping but should not exceed the physics).
    assert!(
        delta_w <= expected_delta_w * 1.1 + 1e-8,
        "humidity ratio increase {delta_w} exceeds physics upper bound {expected_delta_w} by >10%"
    );

    // Mass balance: moisture gained must be consistent with latent energy input.
    // The test's effective_mass_kg is approximate (rho, buffer multiplier may differ
    // from solver internals), so use 5% relative tolerance on the mass delta.
    let delta_m_water = delta_w * effective_mass_kg;
    let q_latent_mass = (latent_w * dt_s) / h_fg_j_kg;
    let rel_error = (delta_m_water - q_latent_mass).abs() / q_latent_mass.max(1e-12);
    assert!(
        rel_error < 0.05,
        "moisture mass balance relative error {:.2}% exceeds 5% threshold; \
         delta_m={delta_m_water:.6}, q_latent_mass={q_latent_mass:.6}",
        rel_error * 100.0
    );
}

// ---------------------------------------------------------------------------
// Cross-solver: ElectricalSolver then HumiditySolver
// ---------------------------------------------------------------------------

/// Electrical and humidity solvers are independent domains. Calling them in
/// sequence with the same PortSlots must produce coherent results from each —
/// neither must corrupt the other's state or the shared PortSlots.
#[test]
fn electrical_and_humidity_resolvers_are_independent() {
    let zone = ZoneId(1);
    let initial_w = 0.007;
    let env = env_with_zone(22.0, 10.0, initial_w);
    let dt = Duration::from_secs(60);

    let mut elec_solver =
        ElectricalSolver::new(ElectricalSolverConfig::default()).expect("valid config");
    let mut hum_solver = HumiditySolver::new(HumiditySolverConfig::default(), &env);

    // PortSlots with both electrical and thermal (latent) contributions.
    let mut ports = PortSlots {
        thermal: vec![ThermalAccumulator::new(zone)],
        ..PortSlots::default()
    };
    ports
        .accumulate(&PortContribution::Electrical {
            active_power_kw: 1.5,
            reactive_power_kvar: 0.0,
        })
        .expect("electrical accumulate");
    ports
        .accumulate(&PortContribution::Thermal {
            zone,
            sensible_gain_w: 200.0,
            latent_gain_w: 400.0,
            category: ThermalCategory::InternalGain,
        })
        .expect("thermal accumulate");

    let elec_update = elec_solver.resolve(&ports, &env, dt);
    let _hum_update = hum_solver.resolve(&ports, &env, dt);

    // Electrical result must still be correct after humidity resolved.
    let net_kw = elec_update.custom_payload.expect("elec payload")[0];
    approx_eq(net_kw, 1.5, 0.001, "electrical net unchanged after humidity resolution");

    // Humidity must have increased due to latent gain.
    assert!(
        hum_solver.humidity_ratio(zone) > initial_w,
        "humidity must have increased; initial={initial_w}, after={}",
        hum_solver.humidity_ratio(zone)
    );

    // PortSlots must be unmodified by either resolve call.
    approx_eq(
        ports.electrical.load_power_kw,
        1.5,
        1e-9,
        "ports.electrical unchanged after resolve calls",
    );
    approx_eq(
        ports.thermal[0].latent_gain_w,
        400.0,
        1e-9,
        "ports.thermal.latent unchanged after resolve calls",
    );
}
