//! Integration tests verifying the thermal balance invariant is wired through
//! the Dwelling step loop.
//!
//! Creates a minimal dwelling with an RC envelope (material layers on a
//! boundary to populate `node_capacitances`) and verifies both that stepping
//! succeeds when the solver is correct and that the invariant fires
//! (`InvariantViolation { check_name: "thermal_balance" }`) when balance terms
//! are deliberately broken.
//!
//! Tests are gated on `debug_assertions` because the invariant check is
//! compiled only when `cfg(any(debug_assertions, feature = "check_invariants"))`
//! is active. In release-mode test builds the invariant is not compiled and
//! these tests would provide no signal.
#![cfg(debug_assertions)]

use std::env;
use std::fs;

use hares_core::Dwelling;
use hares_types::HaresError;

/// TOML configuration for a minimal synthetic dwelling with an RC envelope.
///
/// A single boundary with one material layer produces RC nodes which populate
/// `node_capacitances`, enabling the full-system stored energy computation and
/// three-term affine balance terms consumed by the thermal invariant in
/// `Dwelling::check_invariants`.
const SYNTHETIC_RC_DWELLING_TOML: &str = r#"building_id = 999

[simulation]
start_time = "2024-01-15T00:00:00Z"
time_res_s = 60
duration_s = 3600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 129.6

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 30.0

[weather]
outdoor_temp_c = 30.0
dew_point_c = 5.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[infiltration]
ach = 0.5

internal_gains_w = 200.0
internal_gains_constant = true
internal_gains_sensible_fraction = 1.0
internal_gains_radiant_fraction = 0.3

[[boundaries]]
id = "test-wall"
boundary_type = "Wall"
area_m2 = 48.0

[[boundaries.material_layers]]
thickness_m = 0.1
conductivity_w_m_k = 0.5
density_kg_m3 = 1000.0
specific_heat_j_kg_k = 1000.0

[schedule]
occupancy = 0.0
occupants_present = false

[output]
output_verbosity = 0
output_format = "csv"
write_output = false
master_seed = 0
"#;

/// Builds a synthetic dwelling with RC envelope from the shared TOML fixture.
fn rc_dwelling() -> (std::path::PathBuf, Dwelling) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let tmp = env::temp_dir().join(format!("hares_thermal_invariant_rc_test_{nanos}.toml"));
    fs::write(&tmp, SYNTHETIC_RC_DWELLING_TOML).expect("write toml");
    let dwelling = Dwelling::from_toml_config(&tmp).expect("create dwelling");
    (tmp, dwelling)
}

/// Verifies that a correctly-functioning RC dwelling steps cleanly through
/// the thermal invariant check without false positives.
///
/// Works in conjunction with the envelope-level test in
/// `solver_energy_conservation.rs` which verifies `thermal_balance_terms()`
/// returns non-empty, correct results when `node_capacitances` is populated.
#[test]
fn dwelling_step_thermal_invariant_wired() {
    let (tmp, mut dwelling) = rc_dwelling();

    let result = dwelling.step();
    let _ = fs::remove_file(&tmp);

    assert!(
        result.is_ok(),
        "thermal invariant must not false-positive on correctly-functioning solver; got {result:?}"
    );
}

/// Verifies that the thermal invariant wiring inside `Dwelling::check_invariants`
/// catches deliberately broken balance terms.
///
/// Steps the dwelling once to confirm the invariant passes for correct models,
/// then enables the test seam (`set_thermal_invariant_failure_for_test`) which
/// poisons the balance terms fed to `InvariantChecker::check_thermal`. The
/// second step must return `Err(HaresError::InvariantViolation { check_name:
/// "thermal_balance", .. })`.
///
/// If the `check_thermal` call were accidentally removed from
/// `check_invariants`, or if the balance terms were cleared before the check,
/// this test would fail — the seam would have no effect and `step()` would
/// return `Ok`.
#[test]
fn thermal_invariant_catches_broken_gain() {
    let (tmp, mut dwelling) = rc_dwelling();

    // Step once: confirm the invariant passes for the correct solver.
    dwelling
        .step()
        .expect("first step must pass invariant check on correct model");

    // Enable test seam: poisons balance terms inside check_invariants.
    dwelling.set_thermal_invariant_failure_for_test();

    // This step must fail with thermal_balance invariant violation.
    let result = dwelling.step();
    let _ = fs::remove_file(&tmp);

    match result {
        Err(HaresError::InvariantViolation { ref check_name, .. })
            if check_name == "thermal_balance" => {}
        other => panic!(
            "expected Err(InvariantViolation {{ check_name: \"thermal_balance\", .. }}), \
             got {other:?}"
        ),
    }
}
