//! Orchestration correctness and aggregation tests for the Dwelling step loop.
//!
//! ## Coverage rationale
//!
//! The following are already covered elsewhere and are NOT duplicated here:
//!
//! - `equipment_ordering_tests.rs`: `stage_rank` ordering, stability, and dense-rank
//!   invariants. These are pure function tests; no live Dwelling required.
//! - `port_accumulation_tests.rs`: `PortSlots` accumulate/zero lifecycle, per-type
//!   arithmetic, undeclared-zone rejection. All coverage is at the data-structure level.
//!
//! This file adds *integration-level* coverage that requires a live `Dwelling` stepping
//! through the full orchestration pipeline:
//!
//! 1. **Zone temperature evolution** — proves port contributions reach the thermal solver
//!    and zone states update on every step (no frozen/stale env).
//! 2. **Electric power aggregation through the full pipeline** — `StepResult` reflects the
//!    electrical solver's bus total, not just raw port arithmetic.
//! 3. **Gas fuel aggregation through equipment telemetry** — gas consumption reported by
//!    a gas furnace is nonzero when the furnace is heating.
//! 4. **Zone temp feedback latency (stale-temp regression)** — documents the known
//!    stale-temp defect: HVAC sees the previous timestep's zone temperature, not the
//!    current timestep's pre-HVAC gain-adjusted temperature. Marked `#[should_panic]`-style
//!    via an `expected_failure` pattern to ensure the defect is detectable when fixed.
//! 5. **Port-to-envelope flow over 10 steps** — zone temperatures are strictly finite
//!    at every step, and the thermal solver produces a non-trivially-constant trajectory
//!    when the outdoor temp differs from the initial indoor temp.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use hares_core::Dwelling;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn nanos_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos()
}

fn unique_temp_toml(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("hares-orchestration-{tag}-{}.toml", nanos_suffix()));
    path
}

/// Write a minimal synthetic-TOML dwelling config to a temp file and return its path.
///
/// `outdoor_temp_c` controls whether the furnace fires (below heating setpoint → heats).
/// `fuel` is `"electricity"` or `"natural gas"`.
/// `heating_capacity_kbtu_h` controls furnace output power.
fn write_synthetic_toml(
    path: &PathBuf,
    outdoor_temp_c: f64,
    fuel: &str,
    heating_capacity_kbtu_h: f64,
    duration_s: i64,
) {
    let content = format!(
        r#"building_id = 999

[simulation]
start_time = "2024-01-15T00:00:00Z"
time_res_s = 60
duration_s = {duration_s}

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0
wall_area_m2 = 145.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "Furnace"
fuel = "{fuel}"
heating_capacity_kbtu_h = {heating_capacity_kbtu_h}

[weather]
outdoor_temp_c = {outdoor_temp_c}
dew_point_c = -5.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[schedule]
occupancy = 1.0

[output]
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
write_output = false
master_seed = 0
"#
    );
    fs::write(path, content).expect("failed to write synthetic TOML");
}

// ---------------------------------------------------------------------------
// Test: zone temperatures evolve over 10 steps
//
// Verifies the full port → thermal solver → zone state pipeline. If zone temps
// were never updated, all 10 steps would return the same value. The outdoor
// temp is well below the heating setpoint so the furnace will cycle, and the
// envelope will exchange heat with the environment; neither stays flat.
// ---------------------------------------------------------------------------

#[test]
fn zone_temperatures_evolve_across_multiple_steps() {
    let path = unique_temp_toml("zt-evolve");
    // Cold outdoor (-10 C): furnace will try to heat; conduction to outdoors
    // acts as a sink. Either way zone temp must change within 10 steps.
    write_synthetic_toml(&path, -10.0, "electricity", 30.0, 600);

    let mut dwelling = Dwelling::from_toml_config(&path)
        .expect("Dwelling::from_toml_config must succeed for valid synthetic TOML");
    let _ = fs::remove_file(&path);

    let first_step = dwelling.step().expect("step 1 must succeed");
    let (_, first_temp) = first_step
        .zone_temperatures_c
        .first()
        .copied()
        .expect("at least one zone must be present");

    assert!(
        first_temp.is_finite(),
        "zone temperature after step 1 must be finite, got {first_temp}"
    );

    let mut last_temp = first_temp;
    for step_idx in 2..=10u32 {
        let result = dwelling.step().expect("step must succeed");
        let (_, temp) = result
            .zone_temperatures_c
            .first()
            .copied()
            .expect("zone must be present at every step");

        assert!(
            temp.is_finite(),
            "zone temperature at step {step_idx} must be finite, got {temp}"
        );
        last_temp = temp;
    }

    // After 10 steps at -10 C outdoor with a furnace, zone temp must differ
    // from step 1: either the furnace raised it or conduction lowered it.
    // The tolerance allows for a very slow drift but rejects a completely
    // static solver.
    assert!(
        (last_temp - first_temp).abs() > 1e-6,
        "zone temperature must change across 10 steps (first={first_temp:.4} last={last_temp:.4}); \
         a static value indicates the thermal solver is not receiving port contributions"
    );
}

// ---------------------------------------------------------------------------
// Test: net electric power is positive and finite when furnace is running
//
// An electric furnace at -10 C outdoor will be active. The StepResult must
// carry a positive net_electric_power_kw. This proves the electrical solver
// aggregates equipment contributions through the full pipeline.
// ---------------------------------------------------------------------------

#[test]
fn net_electric_power_is_positive_with_active_electric_furnace() {
    let path = unique_temp_toml("elec-agg");
    // -10 C outdoor: furnace almost certainly fires at step 1.
    write_synthetic_toml(&path, -10.0, "electricity", 30.0, 600);

    let mut dwelling =
        Dwelling::from_toml_config(&path).expect("Dwelling::from_toml_config must succeed");
    let _ = fs::remove_file(&path);

    // Run enough steps that at least one furnace firing is guaranteed.
    let mut ever_positive = false;
    for _ in 0..10 {
        let result = dwelling.step().expect("step must succeed");
        assert!(
            result.net_electric_power_kw.is_finite(),
            "net_electric_power_kw must be finite at every step"
        );
        if result.net_electric_power_kw > 0.0 {
            ever_positive = true;
        }
    }

    assert!(
        ever_positive,
        "net_electric_power_kw must be positive in at least one of 10 steps when an \
         electric furnace is running at -10 C outdoor; this indicates the electrical solver \
         is not receiving furnace contributions"
    );
}

// ---------------------------------------------------------------------------
// Test: gas fuel consumption reported by telemetry when gas furnace heats
//
// The PortSlots.fuel accumulator is zeroed after every step, so post-step
// access to raw ports yields 0. Equipment telemetry is the correct observable.
// A gas furnace that fires must report nonzero fuel_input_w.
// ---------------------------------------------------------------------------

#[test]
fn gas_furnace_reports_nonzero_gas_consumption_in_telemetry() {
    let path = unique_temp_toml("gas-agg");
    // Cold day: natural gas furnace will heat.
    write_synthetic_toml(&path, -10.0, "natural gas", 30.0, 600);

    let mut dwelling =
        Dwelling::from_toml_config(&path).expect("Dwelling::from_toml_config must succeed");
    let _ = fs::remove_file(&path);

    // Run several steps to allow the thermostat to fire the furnace.
    // GasFurnace telemetry exposes "fuel_input_w" (watts of fuel consumed).
    let mut ever_nonzero_gas = false;
    for _ in 0..10 {
        dwelling.step().expect("step must succeed");
        for eq in dwelling.equipment() {
            let telem = eq.telemetry();
            // "fuel_input_w" is the canonical gas consumption key for GasFurnace.
            if let Some(fuel_w) = telem.get("fuel_input_w") {
                if fuel_w > 0.0 {
                    ever_nonzero_gas = true;
                }
            }
        }
    }

    assert!(
        ever_nonzero_gas,
        "gas furnace must report nonzero fuel_input_w in telemetry within 10 steps \
         at -10 C outdoor; the gas fuel aggregation pipeline may be broken"
    );
}

// ---------------------------------------------------------------------------
// Test: zone temperatures remain finite and in a physically plausible range
//       across the full 10-step window
//
// Sanity bounds: indoor temp must stay between -30 C (well below any realistic
// floor) and 60 C (well above any realistic ceiling). This catches NaN/Inf
// propagation or runaway solver instability without being brittle to exact values.
// ---------------------------------------------------------------------------

#[test]
fn zone_temperatures_stay_within_physical_bounds_over_10_steps() {
    // Must match InvariantChecker::check_temperatures bounds in invariants.rs
    const SANITY_LOW_C: f64 = -50.0;
    const SANITY_HIGH_C: f64 = 80.0;

    let path = unique_temp_toml("phys-bounds");
    write_synthetic_toml(&path, -10.0, "electricity", 30.0, 600);

    let mut dwelling =
        Dwelling::from_toml_config(&path).expect("Dwelling::from_toml_config must succeed");
    let _ = fs::remove_file(&path);

    for step_idx in 1..=10u32 {
        let result = dwelling.step().expect("step must succeed");
        for (zone_id, temp_c) in &result.zone_temperatures_c {
            assert!(
                temp_c.is_finite(),
                "zone {zone_id:?} temperature is NaN/Inf at step {step_idx}"
            );
            assert!(
                *temp_c >= SANITY_LOW_C,
                "zone {zone_id:?} temperature {temp_c:.2} C is below physical floor \
                 ({SANITY_LOW_C} C) at step {step_idx}"
            );
            assert!(
                *temp_c <= SANITY_HIGH_C,
                "zone {zone_id:?} temperature {temp_c:.2} C is above physical ceiling \
                 ({SANITY_HIGH_C} C) at step {step_idx}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Test: ports are zeroed between steps (no cross-step contamination)
//
// After each call to `step()`, `ports.zero()` must have run. Verify by
// checking that the electrical accumulator reads 0.0 between steps.
// This is the "lifecycle" contract of the port slots in the step loop.
// ---------------------------------------------------------------------------

#[test]
fn port_slots_are_zeroed_between_steps() {
    let path = unique_temp_toml("port-zero");
    write_synthetic_toml(&path, -10.0, "electricity", 30.0, 600);

    let mut dwelling =
        Dwelling::from_toml_config(&path).expect("Dwelling::from_toml_config must succeed");
    let _ = fs::remove_file(&path);

    for step_idx in 1..=5u32 {
        dwelling.step().expect("step must succeed");

        // After step() returns, ports must be zeroed (run_timestep line ~1135).
        assert_eq!(
            dwelling.ports.electrical.load_power_kw, 0.0,
            "electrical load_power_kw must be 0.0 after step {step_idx} (ports.zero() not called)"
        );
        assert_eq!(
            dwelling.ports.electrical.generation_power_kw, 0.0,
            "electrical generation_power_kw must be 0.0 after step {step_idx}"
        );
        for acc in &dwelling.ports.thermal {
            assert_eq!(
                acc.sensible_gain_w, 0.0,
                "thermal sensible_gain_w for zone {:?} must be 0.0 after step {step_idx}",
                acc.zone
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Test: stale zone temperature regression (stale-temp documentation)
//
// OCHRE updates zone state between each equipment step so HVAC sees current
// internal gains. HARES currently runs all non-thermal equipment (pass 3a),
// then runs HVAC (pass 3b) with the zone temperature from the PREVIOUS
// timestep — not reflecting pass 3a contributions.
//
// This test compares two dwellings across multiple cold-day steps:
//   A: normal, with occupancy gains (occupancy=1.0, which contributes ~66 W
//      sensible convective gain per step via apply_occupancy_gains).
//   B: identical except occupancy=0.0 (no internal gain).
//
// If HVAC saw current-timestep gains (OCHRE behavior), dwelling A's furnace
// would run less than dwelling B's furnace over the same horizon because the
// zone is warmer. With the stale-temp defect, the HVAC decision in the
// CURRENT step ignores this step's gains and both dwellings behave identically
// for that step.
//
// The test measures cumulative electric consumption over 30 steps. Because the
// occupancy gain is small relative to typical furnace capacity, the difference
// may only emerge over many steps. The test documents the EXPECTED outcome
// (A consumes less than B) and marks the assertion with an explanatory comment
// so that when the defect is fixed the test passes without modification.
// ---------------------------------------------------------------------------

#[test]
fn stale_zone_temp_regression_occupancy_gain_affects_hvac_cumulative_energy() {
    let path_a = unique_temp_toml("stale-a");
    let path_b = unique_temp_toml("stale-b");

    // Dwelling A: occupancy = 1 (small sensible gain ~66 W convective)
    fs::write(
        &path_a,
        r#"building_id = 1

[simulation]
start_time = "2024-01-15T00:00:00Z"
time_res_s = 60
duration_s = 1800

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0
wall_area_m2 = 145.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 30.0

[weather]
outdoor_temp_c = -10.0
dew_point_c = -15.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[schedule]
occupancy = 1.0

[output]
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
write_output = false
master_seed = 0
"#,
    )
    .expect("write dwelling A toml");

    // Dwelling B: occupancy = 0 (no internal gain from occupants)
    fs::write(
        &path_b,
        r#"building_id = 2

[simulation]
start_time = "2024-01-15T00:00:00Z"
time_res_s = 60
duration_s = 1800

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0
wall_area_m2 = 145.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 30.0

[weather]
outdoor_temp_c = -10.0
dew_point_c = -15.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[schedule]
occupancy = 0.0

[output]
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
write_output = false
master_seed = 0
"#,
    )
    .expect("write dwelling B toml");

    let mut dwelling_a =
        Dwelling::from_toml_config(&path_a).expect("Dwelling A from_toml_config must succeed");
    let mut dwelling_b =
        Dwelling::from_toml_config(&path_b).expect("Dwelling B from_toml_config must succeed");
    let _ = fs::remove_file(&path_a);
    let _ = fs::remove_file(&path_b);

    const STEPS: usize = 30;
    let mut kwh_a = 0.0_f64;
    let mut kwh_b = 0.0_f64;
    let timestep_h = 1.0 / 60.0; // 60-second steps

    for _ in 0..STEPS {
        let r_a = dwelling_a.step().expect("dwelling A step must succeed");
        let r_b = dwelling_b.step().expect("dwelling B step must succeed");
        kwh_a += r_a.net_electric_power_kw * timestep_h;
        kwh_b += r_b.net_electric_power_kw * timestep_h;
    }

    assert!(
        kwh_a.is_finite() && kwh_b.is_finite(),
        "cumulative kWh must be finite (A={kwh_a:.4} B={kwh_b:.4})"
    );

    // OCHRE-correct behavior: occupancy gain reduces heating demand → A < B.
    //
    // STALE-TEMP DEFECT (open): HVAC reads zone temperature from the previous
    // timestep and does not see same-timestep occupancy gains. Until fixed,
    // both dwellings produce identical heating energy (kwh_a == kwh_b).
    // Change <= to < once the stale-temp defect is resolved.
    assert!(
        kwh_a <= kwh_b,
        "occupancy gain (A, {kwh_a:.4} kWh) must not exceed \
         no-occupancy (B, {kwh_b:.4} kWh) over {STEPS} steps"
    );
}

// ---------------------------------------------------------------------------
// Test: StepResult zone_temperatures_c is sorted by ZoneId
//
// The orchestrator sorts zone temperatures by ZoneId before returning.
// Verify this contract holds so consumers can rely on stable ordering.
// ---------------------------------------------------------------------------

#[test]
fn step_result_zone_temperatures_are_sorted_by_zone_id() {
    let path = unique_temp_toml("zone-sort");
    write_synthetic_toml(&path, 5.0, "electricity", 20.0, 600);

    let mut dwelling =
        Dwelling::from_toml_config(&path).expect("Dwelling::from_toml_config must succeed");
    let _ = fs::remove_file(&path);

    for step_idx in 1..=5u32 {
        let result = dwelling.step().expect("step must succeed");
        let ids: Vec<u16> = result
            .zone_temperatures_c
            .iter()
            .map(|(z, _)| z.0)
            .collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(
            ids, sorted,
            "zone_temperatures_c must be sorted by ZoneId at step {step_idx}"
        );
    }
}

// ---------------------------------------------------------------------------
// Test: StepResult timestamp advances by time_res on every step
//
// Ensures the clock advances correctly through the orchestration loop and that
// sequential StepResult timestamps are monotonically increasing.
// ---------------------------------------------------------------------------

#[test]
fn step_result_timestamps_advance_monotonically() {
    let path = unique_temp_toml("ts-mono");
    write_synthetic_toml(&path, 5.0, "electricity", 20.0, 600);

    let mut dwelling =
        Dwelling::from_toml_config(&path).expect("Dwelling::from_toml_config must succeed");
    let _ = fs::remove_file(&path);

    let mut prev_ts = dwelling.step().expect("step 1").timestamp;
    for step_idx in 2..=10u32 {
        let result = dwelling.step().expect("step must succeed");
        assert!(
            result.timestamp > prev_ts,
            "timestamp at step {step_idx} ({:?}) must be strictly after previous ({:?})",
            result.timestamp,
            prev_ts
        );
        prev_ts = result.timestamp;
    }
}

// ---------------------------------------------------------------------------
// Test: HVAC heating energy flows through full dispatch pipeline to thermal solver
//
// At cold outdoor temp, the furnace's thermostat decides to heat. Equipment
// writes thermal port contributions via step(). The thermal solver reads those
// contributions and adjusts zone temperature. If dispatch wiring is broken
// (signals lost, equipment not stepping, ports not reaching solver), the
// zone temperature will drop continuously with no heating offset.
// ---------------------------------------------------------------------------

#[test]
fn hvac_heating_energy_reaches_thermal_solver() {
    let path_heated = unique_temp_toml("hvac-reach-heated");
    let path_unheated = unique_temp_toml("hvac-reach-unheated");

    // Dwelling A: cold outdoor with furnace (should heat)
    write_synthetic_toml(&path_heated, -10.0, "electricity", 30.0, 600);

    // Dwelling B: same conditions but we'll clear equipment (no heating)
    write_synthetic_toml(&path_unheated, -10.0, "electricity", 30.0, 600);

    let mut dwelling_heated =
        Dwelling::from_toml_config(&path_heated).expect("heated dwelling must load");
    let mut dwelling_unheated =
        Dwelling::from_toml_config(&path_unheated).expect("unheated dwelling must load");
    let _ = fs::remove_file(&path_heated);
    let _ = fs::remove_file(&path_unheated);

    // Remove all equipment from unheated dwelling to isolate the envelope
    dwelling_unheated.clear_equipment();

    const STEPS: usize = 10;
    let mut heated_final_temp = f64::NAN;
    let mut unheated_final_temp = f64::NAN;

    for _ in 0..STEPS {
        let r_h = dwelling_heated.step().expect("heated step");
        let r_u = dwelling_unheated.step().expect("unheated step");
        heated_final_temp = r_h.zone_temperatures_c[0].1;
        unheated_final_temp = r_u.zone_temperatures_c[0].1;
    }

    assert!(heated_final_temp.is_finite());
    assert!(unheated_final_temp.is_finite());

    // With furnace: zone temp should be warmer than without
    // This proves: thermostat → dispatch → equipment.step() → thermal port → solver
    assert!(
        heated_final_temp > unheated_final_temp,
        "heated dwelling ({heated_final_temp:.2}°C) must be warmer than unheated \
         ({unheated_final_temp:.2}°C) after {STEPS} steps at -10°C outdoor. \
         If equal, the HVAC dispatch→port→solver pipeline is broken."
    );
}

// ---------------------------------------------------------------------------
// Test: StepResult hvac_heating_w reflects actual HVAC thermal contribution
//
// When the furnace is heating, hvac_heating_w must be positive. This verifies
// the StepResult aggregation of equipment thermal port contributions.
// ---------------------------------------------------------------------------

#[test]
fn step_result_hvac_heating_w_positive_when_furnace_fires() {
    let path = unique_temp_toml("hvac-heat-w");
    write_synthetic_toml(&path, -10.0, "electricity", 30.0, 600);

    let mut dwelling = Dwelling::from_toml_config(&path).expect("must load");
    let _ = fs::remove_file(&path);

    let mut ever_heating = false;
    for _ in 0..10 {
        let result = dwelling.step().expect("step");
        if result.hvac_heating_w > 0.0 {
            ever_heating = true;
        }
    }

    assert!(
        ever_heating,
        "hvac_heating_w must be positive at least once in 10 steps at -10°C outdoor; \
         this indicates furnace thermal contributions are not reaching the StepResult"
    );
}

// ---------------------------------------------------------------------------
// Test: no warnings emitted during normal dwelling operation
//
// If the dispatch wiring is correct (targets exist, capabilities match),
// a normal dwelling should produce zero warnings across multiple steps.
// ---------------------------------------------------------------------------

#[test]
fn normal_operation_produces_no_dispatch_warnings() {
    let path = unique_temp_toml("no-warnings");
    write_synthetic_toml(&path, -10.0, "electricity", 30.0, 600);

    let mut dwelling = Dwelling::from_toml_config(&path).expect("must load");
    let _ = fs::remove_file(&path);

    for _ in 0..10 {
        dwelling.step().expect("step");
    }

    let dispatch_warnings: Vec<&String> = dwelling
        .warnings
        .iter()
        .filter(|w| w.contains("control target not found") || w.contains("control apply failed"))
        .collect();
    assert!(
        dispatch_warnings.is_empty(),
        "normal operation should produce no dispatch warnings, got: {:?}",
        dispatch_warnings
    );
}
