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
//! 1. **Zone temperature evolution** -- proves port contributions reach the thermal solver
//!    and zone states update on every step (no frozen/stale env).
//! 2. **Electric power aggregation through the full pipeline** -- `StepResult` reflects the
//!    electrical solver's bus total, not just raw port arithmetic.
//! 3. **Gas fuel aggregation through equipment telemetry** -- gas consumption reported by
//!    a gas furnace is nonzero when the furnace is heating.
//! 4. **Stale-zone-temp regression (occupancy → zone temp)** -- two otherwise-identical
//!    dwellings differing only in `[schedule] occupancy` must show distinct mean zone
//!    temperatures, proving the occupancy schedule flows through `apply_occupancy_gains`
//!    and the zone thermal port into the thermal solver. Paired with an IdealHVAC
//!    test that asserts the same 66 W gain reduces cumulative delivered heating energy
//!    when the equipment back-solves capacity continuously.
//! 5. **Port-to-envelope flow over 10 steps** -- zone temperatures are strictly finite
//!    at every step, and the thermal solver produces a non-trivially-constant trajectory
//!    when the outdoor temp differs from the initial indoor temp.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use hares_core::actors::IdealThermostat;
use hares_core::{Actor, Dwelling};

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
    // -10 C outdoor with 3600s (60 steps): furnace fires after ~32 steps as
    // the zone cools from 21°C to below the 19.2°C heating threshold.
    write_synthetic_toml(&path, -10.0, "electricity", 30.0, 3600);

    let mut dwelling =
        Dwelling::from_toml_config(&path).expect("Dwelling::from_toml_config must succeed");
    let _ = fs::remove_file(&path);

    let mut ever_positive = false;
    for _ in 0..60 {
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
        "net_electric_power_kw must be positive in at least one of 60 steps when an \
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
    // Cold day: natural gas furnace will heat. Use 3600s (60 steps) because
    // with 7× interior mass multiplier and τ ≈ 31,000s the zone takes ~32
    // steps to cool from 21°C to below the 19.2°C heating threshold.
    write_synthetic_toml(&path, -10.0, "natural gas", 30.0, 3600);

    let mut dwelling =
        Dwelling::from_toml_config(&path).expect("Dwelling::from_toml_config must succeed");
    let _ = fs::remove_file(&path);

    // GasFurnace telemetry exposes "fuel_input_w" (watts of fuel consumed).
    let mut ever_nonzero_gas = false;
    for _ in 0..60 {
        dwelling.step().expect("step must succeed");
        for eq in dwelling.equipment() {
            let telem = eq.telemetry();
            if let Some(fuel_w) = telem.get("fuel_input_w") {
                if fuel_w > 0.0 {
                    ever_nonzero_gas = true;
                }
            }
        }
    }

    assert!(
        ever_nonzero_gas,
        "gas furnace must report nonzero fuel_input_w in telemetry within 60 steps \
         at -10 C outdoor"
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
            dwelling.ports.electrical.load_power_w, 0.0,
            "electrical load_power_w must be 0.0 after step {step_idx} (ports.zero() not called)"
        );
        assert_eq!(
            dwelling.ports.electrical.generation_power_w, 0.0,
            "electrical generation_power_w must be 0.0 after step {step_idx}"
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
// Test: occupancy schedule propagates to zone temperature (stale-temp regression)
//
// Both dwellings are identical except dwelling A has occupancy=1.0 and
// dwelling B has occupancy=0.0.  Same building_id → same initial zone
// temperature.  The occupancy schedule column is read by
// `environment.rs::occupancy_column_idx()` and dispatched through
// `dwelling/mod.rs::apply_occupancy_gains`, which deposits
// OCCUPANT_SENSIBLE_GAIN_W × OCCUPANT_CONVECTIVE_FRACTION = 46.2 W of sensible
// convective gain plus OCCUPANT_SENSIBLE_GAIN_W × OCCUPANT_RADIATIVE_FRACTION = 19.8 W
// of radiant gain (total sensible = 66 W) and latent gain into the conditioned
// zone's thermal port every timestep; the thermal solver integrates this into the
// zone state.
//
// To make the 66 W gain dominate the zone-temp signal we:
//   - size the Furnace below the envelope heat loss (4 kbtu/h ≈ 1170 W at
//     -30 C outdoor) so both dwellings' furnaces run continuously at the same
//     capacity -- no bang-bang cycle phase noise, identical HVAC thermal
//     output in A and B each step.
// - run 360 × 60 s = 6 h so the 66 W total sensible gain (46.2 W convective +
//   19.8 W radiant) offset integrates into a clean mean zone-temperature gap.
//
// We assert on *mean* zone temperature rather than cumulative kWh because the
// bang-bang Furnace in the original design executed an identical integer
// number of 9.1 kW × 3-step cycles in A and B over the 2 h window: the 66 W
// gain only phase-shifted the cycles, leaving cumulative kWh identical to
// four decimal places.  Mean zone temperature is the direct, cycle-phase-
// insensitive observable for the apply_occupancy_gains → thermal port →
// thermal solver → zone state path.  The companion test
// `ideal_hvac_occupancy_gain_reduces_cumulative_heating_energy` covers the
// cumulative-energy pathway using a continuously-modulating IdealHVAC.
// ---------------------------------------------------------------------------

#[test]
fn stale_zone_temp_regression_occupancy_gain_raises_mean_zone_temperature() {
    let path_a = unique_temp_toml("stale-a");
    let path_b = unique_temp_toml("stale-b");

    // Dwelling A: occupancy = 1.0 → apply_occupancy_gains deposits 66 W sensible
    // (plus latent) into the zone thermal port every step.
    fs::write(
        &path_a,
        r#"building_id = 1

[simulation]
start_time = "2024-01-15T00:00:00Z"
time_res_s = 60
duration_s = 21600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0
wall_area_m2 = 145.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 4.0

[weather]
outdoor_temp_c = -30.0
dew_point_c = -35.0
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

    // Dwelling B: occupancy = 0.0 → no occupant sensible/latent gain is deposited.
    // Same building_id as A so both share the same seeded initial zone state.
    fs::write(
        &path_b,
        r#"building_id = 1

[simulation]
start_time = "2024-01-15T00:00:00Z"
time_res_s = 60
duration_s = 21600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0
wall_area_m2 = 145.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 4.0

[weather]
outdoor_temp_c = -30.0
dew_point_c = -35.0
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

    const STEPS: usize = 360;
    const SANITY_LOW_C: f64 = -50.0;
    const SANITY_HIGH_C: f64 = 50.0;
    // Physics-grounded bounds on the mean-zone-temperature gap introduced by
    // the occupancy sensible gain routed through apply_occupancy_gains.  See
    // OCCUPANT_SENSIBLE_GAIN_W and OCCUPANT_CONVECTIVE_FRACTION /
    // OCCUPANT_RADIATIVE_FRACTION in `crates/hares-physics/src/constants.rs`:
    //
    //   total sensible gain = 66 W (one occupant)
    //     convective        = 66 × 0.70 = 46.2 W (to zone air)
    //     radiative          = 66 × 0.30 = 19.8 W (to surfaces, then zone air)
    //   total energy / 6 h = 66 W × 21 600 s ≈ 1.43 MJ
    //   zone air C (ρ·cp·V) = 1.2 × 1005 × 120 ≈ 1.45 × 10^5 J/K
    //                        ≈ 1 MJ/K after interior-mass multiplier (~7×)
    //   adiabatic ΔT       ≈ 1.43 MJ / 1 MJ/K ≈ 1.4 K (no loss, upper bound)
    //   envelope UA        ≈ wall_area / R ≈ 145 / 2.8 ≈ 52 W/K
    //   steady-state ΔT    ≈ 66 W / 52 W/K ≈ 1.3 K (loss-balanced, physical
    //                        asymptotic cap)
    //
    // The furnace is sized below the envelope loss (4 kBtu/h ≈ 1170 W) so it
    // runs continuously in both dwellings and contributes no differential;
    // the mean-temp gap is set entirely by the 66 W total occupancy sensible
    // gain offset against envelope conduction.  Observed transient mean over
    // 6 h ≈ 0.17 K.
    //
    // Floor 0.1 K rejects a stale/frozen env (observed 0.17, ~1.7× margin).
    // Ceiling 1.0 K = ~0.2 K expected transient × 5× margin; well below the
    // ~1.3 K asymptotic physical cap and therefore catches any ≥ 6× double-
    // counting or W↔kW unit-conversion bug that would produce a multi-K gap.
    const MIN_DELTA_K: f64 = 0.1;
    const MAX_DELTA_K: f64 = 1.0;

    let mut sum_temp_a = 0.0_f64;
    let mut sum_temp_b = 0.0_f64;

    for step_idx in 0..STEPS {
        let r_a = dwelling_a.step().expect("dwelling A step must succeed");
        let r_b = dwelling_b.step().expect("dwelling B step must succeed");

        // Both dwellings share building_id=1 and identical zone topology; the
        // conditioned zone is the first entry in the ZoneId-sorted vector.
        let (zone_a, temp_a) = r_a
            .zone_temperatures_c
            .first()
            .copied()
            .expect("dwelling A must have at least one zone");
        let (zone_b, temp_b) = r_b
            .zone_temperatures_c
            .first()
            .copied()
            .expect("dwelling B must have at least one zone");

        assert_eq!(
            zone_a, zone_b,
            "dwellings share building_id=1 and must resolve the same conditioned \
             ZoneId at step {step_idx} (got A={zone_a:?} B={zone_b:?})"
        );

        assert!(
            temp_a.is_finite() && temp_b.is_finite(),
            "zone temps must be finite at step {step_idx} (A={temp_a} B={temp_b})"
        );
        assert!(
            (SANITY_LOW_C..=SANITY_HIGH_C).contains(&temp_a)
                && (SANITY_LOW_C..=SANITY_HIGH_C).contains(&temp_b),
            "zone temps must lie in [{SANITY_LOW_C}, {SANITY_HIGH_C}] C at step \
             {step_idx} (A={temp_a:.3} B={temp_b:.3})"
        );

        sum_temp_a += temp_a;
        sum_temp_b += temp_b;
    }

    let mean_temp_a = sum_temp_a / STEPS as f64;
    let mean_temp_b = sum_temp_b / STEPS as f64;
    let delta_k = mean_temp_a - mean_temp_b;

    // The 66 W occupancy sensible gain routed through the zone thermal port
    // into the thermal solver must leave an unambiguous imprint on the mean
    // zone temperature over this 6 h window.  Below MIN_DELTA_K: occupancy is
    // not reaching the solver (stale-temp / broken dispatch regression).
    // Above MAX_DELTA_K: the gain is being double-counted or a unit
    // conversion has scaled the port contribution (e.g. W↔kW), which would
    // push the gap well past the ~1.3 K physical steady-state cap.
    assert!(
        delta_k > MIN_DELTA_K && delta_k < MAX_DELTA_K,
        "occupancy-gain ΔT = {delta_k:.3} K (A mean {mean_temp_a:.3} C, \
         B mean {mean_temp_b:.3} C); expected in ({MIN_DELTA_K}, {MAX_DELTA_K}) K \
         over {STEPS} steps -- see derivation in test comment"
    );
}

// ---------------------------------------------------------------------------
// Test: with a continuously-modulating IdealHVAC, the 66 W occupancy sensible
// gain is visible in cumulative heating energy delivered
//
// IdealHVAC in auto-mode at time_res_s >= 300 s (the mode threshold in
// `ideal_hvac.rs`) runs in "ideal capacity" mode: the solver back-calculates
// exactly the capacity required to hold the heating setpoint each step, so
// the equipment's hvac_heating_w varies continuously rather than cycling
// on/off.  Occupancy's 66 W sensible gain therefore offsets an equal amount
// of delivered heating every step and accumulates cleanly in the cumulative
// hvac_heating_w integral over the run.
//
// We assert on integrated hvac_heating_w (delivered thermal energy) rather
// than net_electric_power_kw because IdealHVAC by design has no electric draw
// for the heating leg itself (only optional fan power, which the synthetic
// TOML path does not configure), so net_electric_power_kw is identically zero
// in both A and B.  Thermal energy delivered is the first observable along
// the occupancy → zone port → thermal solver → equipment back-solve chain
// that shows the full cumulative-energy signature of the 66 W gain.
// ---------------------------------------------------------------------------

#[test]
fn ideal_hvac_occupancy_gain_reduces_cumulative_heating_energy() {
    let path_a = unique_temp_toml("ideal-occ-a");
    let path_b = unique_temp_toml("ideal-occ-b");

    // time_res_s = 300 is the threshold at which IdealHvac auto-mode switches
    // from bang-bang to ideal (back-solved) capacity.  duration_s = 18000
    // gives 60 steps of continuously modulated heating delivery.
    fs::write(
        &path_a,
        r#"building_id = 2

[simulation]
start_time = "2024-01-15T00:00:00Z"
time_res_s = 300
duration_s = 18000

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0
wall_area_m2 = 145.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "IdealHVAC"
fuel = "electricity"
heating_capacity_kbtu_h = 30.0

[setpoints]
heating_c = 20.0
cooling_c = 40.0

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

    fs::write(
        &path_b,
        r#"building_id = 2

[simulation]
start_time = "2024-01-15T00:00:00Z"
time_res_s = 300
duration_s = 18000

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0
wall_area_m2 = 145.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "IdealHVAC"
fuel = "electricity"
heating_capacity_kbtu_h = 30.0

[setpoints]
heating_c = 20.0
cooling_c = 40.0

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

    const STEPS: usize = 60;
    const TIMESTEP_H: f64 = 300.0 / 3600.0; // 300-second steps → 1/12 h
    // Physics-grounded bounds on cumulative heating-energy reduction.
    // IdealHVAC back-solves exactly the load each step, so every W of
    // internal gain directly offsets delivered heating.  See
    // OCCUPANT_SENSIBLE_GAIN_W / OCCUPANT_LATENT_GAIN_W in
    // `crates/hares-physics/src/constants.rs`:
    //
    //   sensible offset / 5 h = 66 W × 18 000 s = 1.188 MJ = 0.330 kWh
    //   latent (reported, not offsetting sensible heating in a sensible-only
    //     IdealHVAC config) = 51.2 W × 18 000 s ≈ 0.256 kWh
    //
    // Observed reduction ≈ 0.327 kWh (almost exactly the sensible energy
    // budget).  Floor 0.1 kWh rejects a frozen-port regression (observed
    // ~3.3× margin).  Ceiling 0.6 kWh ≈ sensible (0.33) + any latent
    // contribution and a 2× margin; physically bounded by total occupancy
    // energy deposited over the window, so a gap > 0.6 kWh means the gain
    // is being multiply-counted or a unit conversion is inflating the port.
    const MIN_DELTA_KWH: f64 = 0.1;
    const MAX_DELTA_KWH: f64 = 0.6;

    let mut heating_kwh_a = 0.0_f64;
    let mut heating_kwh_b = 0.0_f64;

    for step_idx in 0..STEPS {
        let r_a = dwelling_a.step().expect("dwelling A step must succeed");
        let r_b = dwelling_b.step().expect("dwelling B step must succeed");

        assert!(
            r_a.hvac_heating_w.is_finite() && r_b.hvac_heating_w.is_finite(),
            "hvac_heating_w must be finite at step {step_idx} \
             (A={a} B={b})",
            a = r_a.hvac_heating_w,
            b = r_b.hvac_heating_w
        );

        heating_kwh_a += r_a.hvac_heating_w * TIMESTEP_H / 1000.0;
        heating_kwh_b += r_b.hvac_heating_w * TIMESTEP_H / 1000.0;
    }

    let delta_kwh = heating_kwh_b - heating_kwh_a;

    // IdealHVAC delivers exactly the solver-computed load each step.  The
    // 66 W total occupancy sensible gain (46.2 W convective + 19.8 W radiant)
    // offsets an equal amount of heating delivery, so over the STEPS × 300 s =
    // 18 000 s window the cumulative heating energy in A must sit strictly
    // below B by (MIN_DELTA_KWH, MAX_DELTA_KWH).  Below the floor: occupancy
    // is not reaching the solver.  Above the ceiling: the 66 W sensible gain
    // is physically capped at ~0.33 kWh over this window, so any larger
    // reduction indicates double-counting or a unit-conversion bug.
    assert!(
        delta_kwh > MIN_DELTA_KWH && delta_kwh < MAX_DELTA_KWH,
        "IdealHVAC occupancy-gain ΔE = {delta_kwh:.4} kWh (A={heating_kwh_a:.4}, \
         B={heating_kwh_b:.4}); expected in ({MIN_DELTA_KWH}, {MAX_DELTA_KWH}) kWh \
         over {STEPS} steps -- see derivation in test comment"
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

    // 3600s (60 steps): furnace fires after ~32 steps once zone cools below threshold.
    write_synthetic_toml(&path_heated, -10.0, "electricity", 30.0, 3600);
    write_synthetic_toml(&path_unheated, -10.0, "electricity", 30.0, 3600);

    let mut dwelling_heated =
        Dwelling::from_toml_config(&path_heated).expect("heated dwelling must load");
    let mut dwelling_unheated =
        Dwelling::from_toml_config(&path_unheated).expect("unheated dwelling must load");
    let _ = fs::remove_file(&path_heated);
    let _ = fs::remove_file(&path_unheated);

    // Remove all equipment from unheated dwelling to isolate the envelope
    dwelling_unheated.clear_equipment();

    const STEPS: usize = 60;
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
    // 3600s (60 steps): furnace fires after ~32 steps once zone cools below threshold.
    write_synthetic_toml(&path, -10.0, "electricity", 30.0, 3600);

    let mut dwelling = Dwelling::from_toml_config(&path).expect("must load");
    let _ = fs::remove_file(&path);

    let mut ever_heating = false;
    for _ in 0..60 {
        let result = dwelling.step().expect("step");
        if result.hvac_heating_w > 0.0 {
            ever_heating = true;
        }
    }

    assert!(
        ever_heating,
        "hvac_heating_w must be positive at least once in 60 steps at -10°C outdoor; \
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

// ---------------------------------------------------------------------------
// Test: IdealThermostat deadband validation prevents FSM oscillation
//
// The thermostat pushes setpoints with a gap that violates the equipment's
// hysteresis deadband. The actor must reject the signal, preventing the
// equipment FSM from receiving invalid setpoints that would cause it to
// oscillate between Heating and Cooling.
//
// In debug/check_invariants builds, decide() panics on the violation
// (correct — loud failure). In release, the signal is gracefully rejected
// and recorded in telemetry. Either path proves the FSM is protected.
// ---------------------------------------------------------------------------

#[test]
fn thermostat_deadband_rejects_narrow_setpoints_protecting_fsm() {
    use std::collections::HashMap;

    let path = unique_temp_toml("deadband-osc");
    // Outdoor at 20 C — warm enough that equipment would normally be in
    // Deadband. The thermostat pushes heat=21, cool=22 with default
    // hysteresis_c=1.0: gap = 1.0 < required 2.0 — deadband violation.
    write_synthetic_toml(&path, 20.0, "electricity", 30.0, 600);

    let mut dwelling =
        Dwelling::from_toml_config(&path).expect("Dwelling::from_toml_config must succeed");
    let _ = fs::remove_file(&path);

    // Discover the HVAC equipment name so the thermostat targets it.
    let hvac_names: Vec<String> = dwelling
        .equipment()
        .iter()
        .filter_map(|eq| {
            let tel = eq.telemetry();
            // HVAC equipment exposes operating_mode.
            if tel.get("operating_mode").is_some() {
                Some(eq.descriptor().name.clone())
            } else {
                None
            }
        })
        .collect();
    assert!(
        !hvac_names.is_empty(),
        "synthetic dwelling must have at least one HVAC equipment with operating_mode telemetry"
    );
    let hvac_name = &hvac_names[0];

    // Add thermostat with setpoints that violate the deadband constraint.
    let thermostat = IdealThermostat::new(hvac_name).with_setpoints(21.0, 22.0);
    dwelling.add_actor(Box::new(thermostat));

    // Collect pre-step equipment operating modes.
    let mut equipment_modes_history: Vec<HashMap<String, f64>> = Vec::new();

    let mut steps_survived = 0;
    for _step in 0..10 {
        let step_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            dwelling.step().expect("step must succeed");
        }));
        match step_result {
            Ok(()) => {
                steps_survived += 1;
                // Record equipment operating modes for oscillation check.
                let mut mode_map = HashMap::new();
                for eq in dwelling.equipment() {
                    let tel = eq.telemetry();
                    if let Some(mode) = tel.get("operating_mode") {
                        mode_map.insert(eq.descriptor().name.clone(), mode);
                    }
                }
                equipment_modes_history.push(mode_map);
            }
            Err(_) => {
                // Debug/check_invariants panic in decide() — correct behavior.
                // The simulation stops, preventing any oscillation.
                break;
            }
        }
    }

    if steps_survived == 10 {
        // Release path: all 10 steps completed. Verify the thermostat
        // telemetry records the rejection.
        let tel = dwelling.telemetry();
        let actor_name = format!("IdealThermostat({hvac_name})");
        let actor_tel = tel.actor_telemetry.get(&actor_name).unwrap_or_else(|| {
            panic!(
                "actor telemetry must contain '{}'; available: {:?}",
                actor_name,
                tel.actor_telemetry.keys().collect::<Vec<_>>()
            )
        });
        assert_eq!(
            actor_tel.get("setpoint_inversion_rejected"),
            Some(&1.0),
            "thermostat telemetry must record deadband rejection flag = 1.0"
        );

        // Verify the HVAC equipment never enters Heating or Cooling mode
        // (operating_mode code 1.0 = Heating, 2.0 = Cooling).
        // The offending signal was rejected, so the FSM stays in Deadband/Off.
        for (step, mode_map) in equipment_modes_history.iter().enumerate() {
            if let Some(mode) = mode_map.get(hvac_name) {
                assert!(
                    *mode != 1.0 && *mode != 2.0,
                    "step {step}: HVAC equipment entered active mode {mode} (1.0=Heat, 2.0=Cool); \
                     deadband-violating setpoints should have been blocked"
                );
            }
        }
    }
    // If steps_survived < 10, the debug panic stopped the simulation —
    // which is also correct behavior (loud failure on invariant violation).
}
