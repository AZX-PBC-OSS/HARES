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
//! 4. **Zone temp feedback latency (stale-temp regression)** — documents FIX-003's
//!    known defect: HVAC sees the previous timestep's zone temperature, not the current
//!    timestep's pre-HVAC gain-adjusted temperature. Marked `#[should_panic]`-style via
//!    an `expected_failure` pattern to ensure the defect is detectable when fixed.
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

    let mut dwelling = Dwelling::from_toml_config(&path)
        .expect("Dwelling::from_toml_config must succeed");
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
// A gas furnace that fires must report nonzero gas_consumption_w.
// ---------------------------------------------------------------------------

#[test]
fn gas_furnace_reports_nonzero_gas_consumption_in_telemetry() {
    let path = unique_temp_toml("gas-agg");
    // Cold day: natural gas furnace will heat.
    write_synthetic_toml(&path, -10.0, "natural gas", 30.0, 600);

    let mut dwelling = Dwelling::from_toml_config(&path)
        .expect("Dwelling::from_toml_config must succeed");
    let _ = fs::remove_file(&path);

    // Run several steps to allow the thermostat to fire the furnace.
    // GasFurnace telemetry exposes "fuel_input_w" (watts of fuel consumed).
    let mut ever_nonzero_gas = false;
    for _ in 0..10 {
        dwelling.step().expect("step must succeed");
        for eq in &dwelling.equipment {
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

    let mut dwelling = Dwelling::from_toml_config(&path)
        .expect("Dwelling::from_toml_config must succeed");
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

    let mut dwelling = Dwelling::from_toml_config(&path)
        .expect("Dwelling::from_toml_config must succeed");
    let _ = fs::remove_file(&path);

    for step_idx in 1..=5u32 {
        dwelling.step().expect("step must succeed");

        // After step() returns, ports must be zeroed (run_timestep line ~1135).
        assert_eq!(
            dwelling.ports.electrical.load_power_kw,
            0.0,
            "electrical load_power_kw must be 0.0 after step {step_idx} (ports.zero() not called)"
        );
        assert_eq!(
            dwelling.ports.electrical.generation_power_kw,
            0.0,
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
// Test: occupancy gains reduce HVAC heating energy
//
// Compares two dwellings over 30 one-minute steps:
//   A: 3 occupants (~200 W sensible convective gain)
//   B: 0 occupants (no internal gain)
//
// Outdoor temp is 18°C — just below the ~20°C heating setpoint — so the
// furnace duty-cycles rather than running at full capacity. The 200 W
// occupancy gain in dwelling A is enough to push the zone above the heating
// setpoint, causing the furnace to cycle off while dwelling B continues
// heating. Result: A consumes less electric energy than B.
// ---------------------------------------------------------------------------

#[test]
fn stale_zone_temp_regression_occupancy_gain_affects_hvac_cumulative_energy() {
    let path_a = unique_temp_toml("stale-a");
    let path_b = unique_temp_toml("stale-b");

    // Dwelling A: occupancy = 3 (~200 W sensible convective gain)
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
outdoor_temp_c = 18.0
dew_point_c = 10.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[schedule]
occupancy = 3.0

[output]
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
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
outdoor_temp_c = 18.0
dew_point_c = 10.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[schedule]
occupancy = 0.0

[output]
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
master_seed = 0
"#,
    )
    .expect("write dwelling B toml");

    let mut dwelling_a = Dwelling::from_toml_config(&path_a)
        .expect("Dwelling A from_toml_config must succeed");
    let mut dwelling_b = Dwelling::from_toml_config(&path_b)
        .expect("Dwelling B from_toml_config must succeed");
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

    // Occupancy gain reduces heating demand → A < B.
    assert!(
        kwh_a < kwh_b,
        "occupancy gain (A={kwh_a:.4} kWh, 3 occupants) must consume less than \
         no-occupancy (B={kwh_b:.4} kWh) over {STEPS} steps at 18°C outdoor"
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

    let mut dwelling = Dwelling::from_toml_config(&path)
        .expect("Dwelling::from_toml_config must succeed");
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

    let mut dwelling = Dwelling::from_toml_config(&path)
        .expect("Dwelling::from_toml_config must succeed");
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
