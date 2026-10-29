//! The run's energy-closure report: its discrimination witnesses.
//!
//! The per-step thermal invariant (`invariants.rs::check_thermal`) bounds
//! each step's balance residual by `max(1 W, 1e-6·gross)`; a step inside
//! that tolerance passes silently, and the tolerance integrates to at most
//! 8.76 kWh/yr of bias a run never had to show anyone (the 1 W floor over
//! the year's 31,536,000 s). The report on
//! `RunHealth::energy_closure` integrates the same three terms over the
//! run's production steps and states the accumulated residual (Wh in the
//! fields, 0.001 kWh per Wh) in energy and in ppm of the tolerance's
//! gross, making that bias visible.
//!
//! The three terms are the solver's affine decomposition against a 0 C
//! boundary reference, and the report publishes no gains and no conduction
//! column: each term carries a component proportional to the absolute
//! boundary temperatures that cancels only in the residual (the record has
//! the derivation; the misread this refuses is a 9.2 MWh/yr house reading
//! 1,056.3 MWh of annual gains and conduction). What the witnesses pin is
//! the report's published account: the residual, the tolerance's gross,
//! the stored change, the delivered meters and the per-zone rows.
//!
//! Three witnesses, all hand-derived:
//!
//! 1. The honest case: a single-zone dwelling on constant inputs. The
//!    zero-order-hold identity `x_next = A_d·x_prev + B_d·u`, weighted by
//!    node capacitance and divided by dt, reads
//!    `Σq_gains = ΔE_stored + q_loss` exactly (the three terms are the same
//!    matrix products the solver evaluated), so each step's residual is a
//!    float-rounding artifact of the three separately summed terms, at most
//!    order 1e-9 W, and its hourly integral is at most order 1e-9 Wh; this
//!    fixture measures 1.63e-10 Wh. The report's residual must read zero at
//!    that scale.
//! 2. The delivered control: the same dwelling's meters. The fixture
//!    drives constant 200 W internal gains through the thermal ports for
//!    the 3600 s horizon: 200 Wh exactly, delivered as the ports' sensible
//!    + radiant + short-wave watts, on the system row and the zone's own.
//! 3. The perturbed case: the same dwelling with 100 W per step of
//!    uncounted loss forced into the report's loss term by the test seam
//!    (a loss with no gains-side counterpart; the per-step invariant
//!    is never reached by the perturbation, so the run completes clean and
//!    only the report can surface it). 100 W × 3600 s = 360,000 J = 100 Wh
//!    exactly: the report's residual must name that number, and nothing
//!    else in the account may move (the seam perturbs the report's loss
//!    input alone, so the physics is the honest run's and every other
//!    column is the honest run's own, bitwise).
//!
//! All run on the minimal synthetic RC dwelling
//! (`thermal_invariant_wiring.rs`'s fixture): one zone, one wall and floor
//! boundary, constant 200 W internal gains, 60 steps of 60 s.

#![cfg(debug_assertions)]

use std::fs;

use hares_core::Dwelling;

/// TOML configuration for a minimal synthetic dwelling with an RC envelope.
///
/// One material layer per boundary populates `node_capacitances`, so the
/// solver's full-system balance terms are live and the report accumulates
/// real terms rather than a vacuous pass.
const SYNTHETIC_RC_DWELLING_TOML: &str = r#"building_id = 999

[simulation]
start_time = "2024-01-15T00:00:00Z"
time_res = 60
duration = 3600
master_seed = 0
[geometry]
number_of_bedrooms = 3.0
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

[[boundaries]]
id = "test-floor"
boundary_type = "Floor"
area_m2 = 48.0

[[boundaries.material_layers]]
thickness_m = 0.1
conductivity_w_m_k = 0.5
density_kg_m3 = 1000.0
specific_heat_j_kg_k = 1000.0

[schedule]
occupancy = 0.0
occupants_present = false
"#;

/// Builds the synthetic dwelling; its configured horizon is 60 steps of
/// 60 s (3600 s). Construction runs the warm-up convergence replay; the
/// report accumulates production steps only, so the steps below are the
/// run's whole account.
fn rc_dwelling() -> Dwelling {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("dwelling.toml");
    fs::write(&path, SYNTHETIC_RC_DWELLING_TOML).expect("write toml");
    Dwelling::from_toml_config(&path).expect("create dwelling")
}

/// Steps the dwelling through its full configured horizon (3600 s / 60 s).
fn run_horizon(dwelling: &mut Dwelling) {
    for step in 0..60 {
        dwelling
            .step()
            .unwrap_or_else(|err| panic!("step {step} must succeed: {err}"));
    }
}

/// WITNESS 1 (honest case): the report's residual reads approximately zero.
///
/// Hand-derivation (see the module comment): the ZOH identity makes each
/// step's residual a rounding artifact of at most order 1e-9 W; 60 steps of
/// 60 s integrate to at most order 1e-9 Wh, and this fixture measures
/// 1.63e-10 Wh. The bound below is a thousand times coarser than that
/// expectation and still a million times below the 60-step
/// silent-bias ceiling (60 x 1 W x 60 s = 3.6 kJ = 1 Wh) the per-step
/// tolerance admits.
#[test]
fn closure_residual_reads_zero_on_the_honest_run() {
    let mut dwelling = rc_dwelling();
    run_horizon(&mut dwelling);

    let closure = &dwelling.health().energy_closure;
    let system = &closure.system;
    assert!(
        system.tolerance_gross_wh > 0.0,
        "the honest run's tolerance gross must be positive: the ppm denominator is live"
    );
    assert!(
        system.residual_wh.abs() < 1e-6,
        "the honest run's integrated residual must read zero, got {} Wh",
        system.residual_wh
    );
    assert!(
        closure.residual_ppm_of_gross().abs() < 1e-3,
        "the honest run's residual must read zero in ppm of gross, got {}",
        closure.residual_ppm_of_gross()
    );
    // The stored change is a physical difference (no boundary reference):
    // the transient hour stores or releases a finite account, and the row's
    // business is to state it, not to pin its sign.
    assert!(
        system.stored_delta_wh.is_finite(),
        "the stored change must be a finite reported quantity"
    );
}

/// WITNESS 1 (companion): the delivered end-uses carry the hand-computed
/// internal-gain account.
///
/// Hand-derivation: the fixture drives constant 200 W internal gains
/// (`sensible_fraction` 1.0) into the indoor zone through the thermal ports
/// for the full 3600 s horizon: 200 W x 3600 s = 720,000 J = 200 Wh
/// exactly, delivered as the ports' sensible + radiant + short-wave watts
/// (the radiant fraction 0.3 changes the path, not the zone's total). Any
/// HVAC delivery can only add to it. The single zone's row mirrors the
/// ports' whole account, and its air-node storage is reported beside it.
#[test]
fn closure_zone_row_carries_the_delivered_end_use_account() {
    let mut dwelling = rc_dwelling();
    run_horizon(&mut dwelling);

    let closure = &dwelling.health().energy_closure;
    assert_eq!(
        closure.zones.len(),
        1,
        "the single-zone fixture holds exactly one zone row: {:?}",
        closure.zones
    );
    let zone = &closure.zones[0];
    assert!(
        zone.delivered_end_use_wh >= 200.0 - 1e-6,
        "delivered end-uses must carry at least the 200 Wh the constant internal \
         gains delivered (60 steps of 10/3 Wh accumulate float rounding), got {} Wh",
        zone.delivered_end_use_wh
    );
    assert!(
        (closure.system.delivered_end_use_wh - zone.delivered_end_use_wh).abs() < 1e-9,
        "the single zone's delivery is the system row's whole delivered account"
    );
    // The zone air node stored or released something over the transient
    // hour (the network starts off its steady state); the row's business is
    // to state it, not to pin its sign.
    assert!(
        zone.zone_air_stored_wh.is_finite(),
        "the zone air storage must be a finite reported quantity"
    );
}

/// WITNESS 2 (perturbed case): a forced 100 W uncounted loss is named by
/// the residual, and by nothing else.
///
/// Hand-derivation (see the module comment): the seam adds 100 W to the
/// report's loss term with no gains-side counterpart, so each step's
/// residual is exactly -100 W and the 3600 s horizon integrates to
/// -100 W x 3600 s = -100 Wh. The per-step invariant never sees the
/// perturbation (the run stays clean); a report that silently absorbed the
/// loss would read zero here and read as an honest run. The seam perturbs
/// the report's loss input alone: the trajectory is the honest run's, so
/// every other published column is the honest run's own, bitwise.
#[test]
fn closure_residual_names_a_forced_uncounted_loss() {
    let mut dwelling = rc_dwelling();
    dwelling.set_energy_closure_uncounted_loss_for_test(100.0);
    run_horizon(&mut dwelling);

    let closure = &dwelling.health().energy_closure;
    let system = &closure.system;
    assert!(
        (system.residual_wh + 100.0).abs() < 1e-6,
        "the residual must name the injected 100 W uncounted loss (100 Wh over the \
         3600 s horizon), got {} Wh",
        system.residual_wh
    );
    // The perturbation lives in the report's loss input alone: the stored
    // change, the tolerance's gross, the ports' delivery account and the
    // zone rows are the honest run's own, bitwise.
    let mut honest = rc_dwelling();
    run_horizon(&mut honest);
    let honest_closure = &honest.health().energy_closure;
    let honest_system = &honest_closure.system;
    assert_eq!(
        system.stored_delta_wh.to_bits(),
        honest_system.stored_delta_wh.to_bits(),
        "the injected loss must not move the stored change"
    );
    assert_eq!(
        system.tolerance_gross_wh.to_bits(),
        honest_system.tolerance_gross_wh.to_bits(),
        "the injected loss must not move the tolerance's gross"
    );
    assert_eq!(
        system.delivered_end_use_wh.to_bits(),
        honest_system.delivered_end_use_wh.to_bits(),
        "the injected loss must not move the delivered end-uses"
    );
    assert_eq!(
        closure.zones, honest_closure.zones,
        "the injected loss must not move the zone rows: {:?} against {:?}",
        closure.zones, honest_closure.zones
    );
    // The residual is stated in ppm of gross at the report's face: the
    // perturbed run's ppm is the residual against the tolerance's gross, a
    // large negative number, not a rounding whisper.
    let ppm = closure.residual_ppm_of_gross();
    assert!(
        (ppm - system.residual_wh / system.tolerance_gross_wh * 1e6).abs() < 1e-6,
        "the ppm accessor must denominate the residual against the run's own gross flux"
    );
    assert!(
        ppm < -1000.0,
        "a 100 Wh imbalance against a 1-hour run's gross flux is a reportable fraction, \
         got {ppm} ppm"
    );
}
