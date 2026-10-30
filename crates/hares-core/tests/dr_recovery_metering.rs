//! The post-curtailment cooling recovery's metering witness.
//!
//! Scenario (the brownout round's B3a probe, on the fixture that probe
//! used): the `cz6b_pv_battery_ev` parity fixture at 900 s steps from
//! 2023-06-01, warm-up disabled, a 288-step (3 day) warm-in, then a
//! GridEmergency shed on the ASHP cooler armed at the 15:00 step, then
//! the release. The contract the witness pins: the ideal path's delivery
//! is the zone's computed need for the step: the ideal solve's capacity
//! (OCHRE HVAC.py:424 "h_desired should be equal to
//! self.delivered_heat"; EnergyPlus meters the DX coil's PLR to the
//! zone's computed load, DXCoils.cc), and the recovery never drives the
//! zone below the cooling setpoint.
//!
//! The zone's computed need for a cooling step is the correction energy
//! (pulling the zone from its state to the setpoint) plus the offset
//! energy (the non-HVAC net gains the delivery must also remove to hold
//! the target). The capacitance reference comes from the run's own base
//! cycle (the first cooling step's delivery divided by the drop it
//! bought); the offset's per-step energy is the shed's own drift (the
//! zone's rise per step with zero delivery is exactly the non-HVAC net
//! the delivery will face). The capacitance so measured overstates the
//! zone's true capacitance by the gains' share of the first step's
//! balance, which keeps both terms valid upper bounds.

use chrono::{FixedOffset, TimeDelta, TimeZone};
use hares_core::Dwelling;
use hares_types::telemetry_keys as tk;
use hares_types::{ControlSignal, DRLevel};
use std::path::PathBuf;

const STEP_SECS: f64 = 900.0;
const WARM_IN_STEPS: usize = 288;
const SHED_DURATION_S: f64 = 8.0 * STEP_SECS;

/// The correction term's margin. The zone's step response is an RC
/// network, not a pure capacitance: the solve's share carries the
/// envelope's conduction and the mass-node coupling that the single-step
/// capacitance reference cannot see, so the metered delivery rides above
/// the bare correction early in the recovery (measured: at most 1.8x on
/// the convergence steps, 0.72x on the release step itself). The
/// unmetered parent delivered 4.05x the bare correction on the release
/// step. The bound at 2.5x separates the two with measured room on both
/// sides; the offset term (the shed's own drift) carries the gains'
/// share beside it.
const CORRECTION_MARGIN: f64 = 2.5;

/// The recovery-trajectory bound below the setpoint. The metered delivery
/// lands the zone from above (the coil's sensible split delivers at most
/// the solved share, so the zone undershoots the cooling and settles
/// above the target); the residual control error is bounded by the base
/// cycle's own hold-step excursions (0.024 C measured), and the bound
/// gives that four-fold headroom.
const RECOVERY_FLOOR_C: f64 = 0.1;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn build() -> Dwelling {
    let root = repo_root();
    let fixture = root.join("tests/fixtures/parity/cz6b_pv_battery_ev");
    let defaults = root.join("python/hares/defaults");
    let start = FixedOffset::west_opt(7 * 3600)
        .expect("the fixture's fixed offset")
        .with_ymd_and_hms(2023, 6, 1, 0, 0, 0)
        .single()
        .expect("the run's start time");
    Dwelling::from_hpxml(
        &fixture.join("building.xml"),
        &fixture.join("schedule.csv"),
        &fixture.join("weather.epw"),
        start,
        TimeDelta::seconds(STEP_SECS as i64),
        TimeDelta::seconds((WARM_IN_STEPS as i64 + 400) * STEP_SECS as i64),
        Some(serde_json::json!({ "Battery": { "bms_mode": "\"Manual\"" } })),
        &defaults,
    )
    .expect("cz6b_pv_battery_ev must build")
}

fn zone_temp(dwelling: &Dwelling) -> f64 {
    dwelling
        .latest_env()
        .zones
        .first()
        .map(|z| z.temperature_c)
        .expect("the fixture has a conditioned zone")
}

fn cooler_field(dwelling: &Dwelling, key: &str) -> f64 {
    dwelling
        .latest_env()
        .equipment_telemetry
        .get("ASHP Cooler")
        .and_then(|t| t.get(key))
        .unwrap_or_else(|| panic!("the ASHP Cooler publishes {key}"))
}

/// The base cycle's first cooling step: `(delivered sensible W, the zone
/// drop it bought, the setpoint)`. The delivery and the drop give the
/// zone's effective capacitance.
fn base_first_cooling_step() -> (f64, f64, f64) {
    let mut dwelling = build();
    for _ in 0..WARM_IN_STEPS {
        dwelling.step().expect("warm-in step");
    }
    let mut before = zone_temp(&dwelling);
    for _ in 0..120 {
        dwelling.step().expect("base-cycle step");
        let delivered = cooler_field(&dwelling, tk::SENSIBLE_COOLING_W);
        if delivered > 0.0 {
            let setpoint = cooler_field(&dwelling, tk::COOLING_SETPOINT_C);
            return (delivered, before - zone_temp(&dwelling), setpoint);
        }
        before = zone_temp(&dwelling);
    }
    panic!("the base cycle never delivered cooling after the warm-in");
}

/// One step's record: the zone temperature the step started from, the
/// zone temperature it ended at, and the cooler's delivered sensible
/// power for the step.
struct StepRecord {
    zone_before_c: f64,
    zone_after_c: f64,
    delivered_w: f64,
}

/// The shed scenario: the GridEmergency armed on the ASHP Cooler at the
/// 15:00 step, then every step through the release and the recovery,
/// with the zone's temperature and the step's delivery. The dwelling
/// comes back so the run's own health totals can be read.
fn shed_run() -> (Dwelling, Vec<StepRecord>, f64) {
    let mut dwelling = build();
    for _ in 0..WARM_IN_STEPS {
        dwelling.step().expect("warm-in step");
    }
    // Arm at the 15:00 step (the state whose clock reads 15:00).
    let mut armed = false;
    for _ in 0..70 {
        dwelling.step().expect("the run to the 15:00 step");
        if dwelling
            .latest_env()
            .current_time
            .format("%H:%M")
            .to_string()
            == "15:00"
        {
            dwelling.apply_control(
                "ASHP Cooler",
                ControlSignal::DemandResponse {
                    level: DRLevel::GridEmergency,
                    duration_s: Some(SHED_DURATION_S),
                },
            );
            armed = true;
            break;
        }
    }
    assert!(armed, "the warm-in never reached the 15:00 step");

    let mut records = Vec::new();
    let mut setpoint = f64::NAN;
    for _ in 0..24 {
        let zone_before = zone_temp(&dwelling);
        dwelling.step().expect("shed/recovery step");
        records.push(StepRecord {
            zone_before_c: zone_before,
            zone_after_c: zone_temp(&dwelling),
            delivered_w: cooler_field(&dwelling, tk::SENSIBLE_COOLING_W),
        });
        setpoint = cooler_field(&dwelling, tk::COOLING_SETPOINT_C);
    }
    (dwelling, records, setpoint)
}

/// The pipeline's starvation of the ideal solve is counted, never
/// silent. With the DR countdown advancing once per step, the
/// post-curtailment release lands on the first control pass ahead of
/// the solve, so the shed run starves nothing: its release delivers the
/// solved capacity. The count's honesty pins on the case that still
/// starves: a thermal-setpoint override lands with the control dispatch
/// (after the solver-feedback phase), so the override's first cooling
/// step runs on the fallback duty and the run's health records it.
#[test]
fn starved_ideal_steps_are_counted_in_run_health() {
    let mut dwelling = build();
    for _ in 0..WARM_IN_STEPS {
        dwelling.step().expect("warm-in step");
    }
    let cool = ControlSignal::ThermalSetpoint {
        heating_setpoint_c: None,
        cooling_setpoint_c: Some(21.0),
        deadband_c: None,
    };
    // The warm-in must be clean: a plain ideal base run never starves the
    // solve (the solve runs before the control dispatch, so its own
    // capacity signal arrives every cooling step). A non-zero count here
    // would make the delta below prove nothing.
    let before = dwelling.health().ideal_capacity_unsolved_steps;
    assert_eq!(
        before, 0,
        "the warm-in's ideal steps all ran on their own solve, got {before} \
         starved steps"
    );

    let mut delivered_any = false;
    for _ in 0..48 {
        dwelling.apply_control("ASHP Cooler", cool.clone());
        dwelling.step().expect("override-run step");
        if cooler_field(&dwelling, tk::SENSIBLE_COOLING_W) > 0.0 {
            delivered_any = true;
            break;
        }
    }
    assert!(delivered_any, "the override run must reach cooling");
    let after = dwelling.health().ideal_capacity_unsolved_steps;
    assert!(
        after > before,
        "the override's first cooling step lands after the solve: its \
         fallback delivery must be counted in the run's health, got {after} \
         against the warm-in's {before}"
    );

    // The shed scenario: the release delivers the solved capacity (the
    // witness's delivery bound), so its run starves nothing.
    let (dwelling, _records, _setpoint) = shed_run();
    let health = dwelling.health();
    assert_eq!(
        health.ideal_capacity_unsolved_steps, 0,
        "the post-curtailment release runs on its own solve: the \
         starvation count must stay zero"
    );
}

/// The first post-curtailment cooling step delivers at most the zone's
/// computed need, and the recovery never drives the zone below the
/// cooling setpoint.
#[test]
fn post_event_cooling_delivery_stays_within_the_zone_computed_need() {
    let (base_delivered_w, base_drop_c, _setpoint) = base_first_cooling_step();
    let capacitance_j_per_c = base_delivered_w * STEP_SECS / base_drop_c;

    let (_dwelling, records, setpoint) = shed_run();
    let first_delivery = records
        .iter()
        .position(|r| r.delivered_w > 0.0)
        .expect("the cooler must deliver after the shed's release");

    // The shed's own drift is the non-HVAC net's per-step energy: the
    // offset term every post-release delivery must also carry. The shed's
    // last step is the freshest measurement.
    let shed_steps = &records[..first_delivery];
    let offset_j = shed_steps
        .last()
        .map(|last| {
            let prior = shed_steps
                .iter()
                .rev()
                .nth(1)
                .map(|r| r.zone_before_c)
                .unwrap_or(last.zone_before_c);
            (last.zone_before_c - prior) * capacitance_j_per_c
        })
        .expect("the shed recorded steps");

    // Every post-release cooling step's delivered energy is bounded by the
    // zone's computed need for that step: the correction plus the offset,
    // by the proven margin. Steps past the recovery, where the zone has
    // reached the setpoint, carry no cooling need and no bound (their
    // residual hold noise is what the trajectory bound below governs).
    for record in &records[first_delivery..] {
        let correction_j = capacitance_j_per_c * (record.zone_before_c - setpoint);
        if correction_j <= 0.0 {
            break;
        }
        let delivered_j = record.delivered_w * STEP_SECS;
        let bound_j = correction_j * CORRECTION_MARGIN + offset_j;
        assert!(
            delivered_j <= bound_j,
            "the post-curtailment cooling step delivered {delivered_j} J \
             against a computed need of {correction_j} J of correction plus \
             {offset_j} J of gains offset (bound {bound_j} J): the recovery \
             must be metered to the zone's need, not dumped at the unit's \
             raw duty"
        );
    }

    // The recovery never crosses below the setpoint on a step that
    // delivered cooling: the metered delivery lands the zone from above.
    // (A step past the recovery whose zone drifts below the setpoint with
    // no delivery is the evening's natural drift, not an overshoot.)
    for (i, record) in records[first_delivery..].iter().enumerate() {
        assert!(
            record.delivered_w <= 0.0 || record.zone_after_c >= setpoint - RECOVERY_FLOOR_C,
            "the recovery's step {} delivered {} W and drove the zone to \
             {} C against the {setpoint} C setpoint: the post-curtailment \
             recovery must not overshoot below the setpoint and rebound",
            first_delivery + i,
            record.delivered_w,
            record.zone_after_c,
        );
    }
}
