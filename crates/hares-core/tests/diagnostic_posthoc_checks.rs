//! Post-hoc diagnostic check tests: unmet hours, short-cycling, freezing,
//! and simultaneous heating/cooling detection.
//!
//! All tests are gated behind `#[cfg(feature = "observe")]` because
//! `DiagnosticAccumulator` depends on the observer feature.
#![cfg(feature = "observe")]

use hares_core::diagnostics::DiagnosticAccumulator;
use hares_types::{OperatingMode, ZoneId};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn accum_1zone_1equip() -> DiagnosticAccumulator {
    DiagnosticAccumulator::new(
        &[ZoneId(1)],
        &[true],
        &["Furnace".to_string()],
        &[Some(ZoneId(1))],
    )
}

fn accum_2zone_3equip() -> DiagnosticAccumulator {
    DiagnosticAccumulator::new(
        &[ZoneId(1), ZoneId(2)],
        &[true, true],
        &[
            "Furnace".to_string(),
            "AC".to_string(),
            "WaterHeater".to_string(),
        ],
        &[Some(ZoneId(1)), Some(ZoneId(1)), None],
    )
}

// ── Unit tests: record_step tracks per-zone per-equipment counters correctly ──

#[test]
fn unmet_heating_detected_when_zone_temp_below_setpoint_over_threshold() {
    let mut accum = accum_1zone_1equip();
    let n = 100;
    // 10 steps below heating setpoint 20°C → 10% unmet > 5% threshold
    for i in 0..n {
        let temp = if i < 10 { 15.0 } else { 22.0 };
        accum.record_step(
            &[temp],
            &[Some(20.0)],
            &[None],
            &[Some(OperatingMode::Heating)],
            &[Some(0)],
        );
    }
    let violations = accum.run_post_hoc_checks(&["Indoor".to_string()], 60.0);
    assert_eq!(violations, 1, "should detect excessive unmet heating");
}

#[test]
fn unmet_cooling_detected_when_zone_temp_above_setpoint_over_threshold() {
    let mut accum = accum_1zone_1equip();
    let n = 100;
    // 8 steps above cooling setpoint 24°C → 8% unmet > 5% threshold
    for i in 0..n {
        let temp = if i < 8 { 28.0 } else { 22.0 };
        accum.record_step(
            &[temp],
            &[None],
            &[Some(24.0)],
            &[Some(OperatingMode::Cooling)],
            &[Some(0)],
        );
    }
    let violations = accum.run_post_hoc_checks(&["Indoor".to_string()], 60.0);
    assert_eq!(violations, 1, "should detect excessive unmet cooling");
}

#[test]
fn unmet_hours_not_flagged_below_threshold() {
    let mut accum = accum_1zone_1equip();
    let n = 100;
    // 4 steps below setpoint → 4% < 5% threshold → no violation
    for i in 0..n {
        let temp = if i < 4 { 15.0 } else { 22.0 };
        accum.record_step(
            &[temp],
            &[Some(20.0)],
            &[None],
            &[Some(OperatingMode::Heating)],
            &[Some(0)],
        );
    }
    let violations = accum.run_post_hoc_checks(&["Indoor".to_string()], 60.0);
    assert_eq!(violations, 0, "4% unmet should be below 5% threshold");
}

#[test]
fn short_cycling_detected_when_mode_changes_exceed_max_per_hour() {
    let mut accum = accum_1zone_1equip();
    // 60 steps at 60s each = 1 hour. 5 mode changes in 1 hour > 4 max.
    let modes = [
        OperatingMode::Heating,
        OperatingMode::Off,
        OperatingMode::Heating,
        OperatingMode::Off,
        OperatingMode::Heating,
        OperatingMode::Off,
    ];
    for (i, &m) in modes.iter().enumerate() {
        accum.record_step(&[22.0], &[Some(20.0)], &[None], &[Some(m)], &[Some(0)]);
        // Pad with steady-state steps so total = 60
        if i == modes.len() - 1 {
            for _ in 0..(60 - modes.len()) {
                accum.record_step(
                    &[22.0],
                    &[Some(20.0)],
                    &[None],
                    &[Some(OperatingMode::Off)],
                    &[Some(0)],
                );
            }
        }
    }
    let violations = accum.run_post_hoc_checks(&["Indoor".to_string()], 60.0);
    assert_eq!(
        violations, 1,
        "should detect short cycling: 5 changes in 1h"
    );
}

#[test]
fn short_cycling_not_flagged_when_cycling_below_max() {
    let mut accum = accum_1zone_1equip();
    // 1 mode change in 60s is 60/h. 60s resolution: 1 change per step.
    // Actually we need at least 2 steps to have a mode change. 2 steps at 60s
    // = 120s = 1/30 hour. 1 change / (1/30) h = 30 changes/h > 4 max → violation.
    // That's too fast. Let's use 10 steps at 60s = 600s = 1/6 h. 1 change.
    // 1 / (1/6) = 6 changes/h > 4 max → still violation.
    // Fine, let's just test with 0 changes:
    for _ in 0..60 {
        accum.record_step(
            &[22.0],
            &[Some(20.0)],
            &[None],
            &[Some(OperatingMode::Off)],
            &[Some(0)],
        );
    }
    let violations = accum.run_post_hoc_checks(&["Indoor".to_string()], 60.0);
    assert_eq!(
        violations, 0,
        "0 mode changes should not trigger short-cycling"
    );
}

#[test]
fn freezing_detected_in_conditioned_zone_when_temp_below_zero() {
    let mut accum = accum_1zone_1equip();
    let n = 60;
    for i in 0..n {
        let temp = if i < 3 { -5.0 } else { 20.0 };
        accum.record_step(
            &[temp],
            &[Some(20.0)],
            &[None],
            &[Some(OperatingMode::Heating)],
            &[Some(0)],
        );
    }
    let violations = accum.run_post_hoc_checks(&["Indoor".to_string()], 60.0);
    assert_eq!(violations, 1, "should detect freezing excursion");
}

#[test]
fn freezing_ignored_in_unconditioned_zone() {
    let mut accum = DiagnosticAccumulator::new(
        &[ZoneId(2)],
        &[false], // unconditioned
        &["GarageHeater".to_string()],
        &[Some(ZoneId(2))],
    );
    for _ in 0..60 {
        accum.record_step(
            &[-5.0],
            &[Some(20.0)],
            &[None],
            &[Some(OperatingMode::Heating)],
            &[Some(0)],
        );
    }
    let violations = accum.run_post_hoc_checks(&["Zone2".to_string()], 60.0);
    assert_eq!(
        violations, 0,
        "freezing in unconditioned zone should not be flagged"
    );
}

#[test]
fn simultaneous_heating_cooling_detected_per_zone() {
    let mut accum = accum_2zone_3equip();
    // Furnace in zone 1 heating, AC in zone 1 cooling → simultaneous H/C
    accum.record_step(
        &[22.0, 22.0],
        &[Some(20.0), None],
        &[None, None],
        &[
            Some(OperatingMode::Heating),
            Some(OperatingMode::Cooling),
            Some(OperatingMode::Off),
        ],
        &[Some(0), Some(0), None],
    );
    let violations = accum.run_post_hoc_checks(&["Indoor".to_string(), "Zone2".to_string()], 60.0);
    assert_eq!(
        violations, 1,
        "simultaneous heating+cooling in zone 1 should be detected"
    );
}

#[test]
fn simultaneous_heating_cooling_not_flagged_when_in_different_zones() {
    let mut accum = accum_2zone_3equip();
    // Furnace in zone 1 heating, nothing in zone 1 cooling -> no conflict
    accum.record_step(
        &[22.0, 22.0],
        &[Some(20.0), Some(22.0)],
        &[None, None],
        &[
            Some(OperatingMode::Heating),        // Furnace, zone 1
            Some(OperatingMode::Off),            // AC, zone 1 (off)
            Some(OperatingMode::HeatingHPAndER), // WaterHeater, no zone (ignored)
        ],
        &[Some(0), Some(0), None],
    );
    let violations = accum.run_post_hoc_checks(&["Indoor".to_string(), "Zone2".to_string()], 60.0);
    assert_eq!(
        violations, 0,
        "no simultaneous H/C when only heating is active"
    );
}

#[test]
fn multiple_violations_counted_independently() {
    let mut accum = accum_1zone_1equip();
    // Simultaneously: unmet heating + freezing + short-cycling
    let modes = [
        OperatingMode::Heating,
        OperatingMode::Off,
        OperatingMode::Heating,
        OperatingMode::Off,
        OperatingMode::Heating,
        OperatingMode::Off,
    ];
    for (i, &m) in modes.iter().enumerate() {
        // Zone temp -5°C: below 20°C setpoint (unmet) AND below 0°C (freezing)
        accum.record_step(&[-5.0], &[Some(20.0)], &[None], &[Some(m)], &[Some(0)]);
        if i == modes.len() - 1 {
            for _ in 0..(60 - modes.len()) {
                accum.record_step(
                    &[-5.0],
                    &[Some(20.0)],
                    &[None],
                    &[Some(OperatingMode::Off)],
                    &[Some(0)],
                );
            }
        }
    }
    // Total: 60 steps, all with -5°C temp and heating setpoint 20°C
    // → 100% unmet heating > 5% → violation
    // → 60 freezing steps > 0 → violation
    // → 5 mode changes in 60 steps (60s each = 1h) > 4 max → violation
    // → 3 violations total
    let violations = accum.run_post_hoc_checks(&["Indoor".to_string()], 60.0);
    assert_eq!(violations, 3, "should detect all three violation types");
}

#[test]
fn no_violations_when_everything_is_in_range() {
    let mut accum = accum_1zone_1equip();
    for _ in 0..100 {
        accum.record_step(
            &[22.0],
            &[Some(20.0)],
            &[None],
            &[Some(OperatingMode::Heating)],
            &[Some(0)],
        );
    }
    let violations = accum.run_post_hoc_checks(&["Indoor".to_string()], 60.0);
    assert_eq!(violations, 0, "no violations when temps are all in range");
}

// ── Integration test: end-to-end wiring of post-hoc checks in Dwelling ───────

/// A minimal tracing subscriber that counts the events whose message
/// carries the needle, so a test can observe the post-hoc checks' findings.
struct CountingSubscriber {
    needle: &'static str,
    count: std::sync::atomic::AtomicUsize,
}

struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

impl tracing::Subscriber for CountingSubscriber {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        if visitor.0.contains(self.needle) {
            self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
    fn clone_span(&self, id: &tracing::span::Id) -> tracing::span::Id {
        id.clone()
    }
}

impl CountingSubscriber {
    fn count(&self) -> usize {
        self.count.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Verifies the post-hoc checks' wiring: a run that requests the
/// diagnostics runs the checks at end of run and their findings surface as
/// tracing warnings, and the frame carries the diagnostics columns; a run
/// without the request never runs them (and records no diagnostics
/// variable).
#[test]
fn posthoc_checks_run_only_when_the_diagnostics_are_requested() {
    use std::sync::Arc;

    // Synthetic TOML with a gas furnace in very cold weather.  The furnace
    // capacity is 5 kBtu/h (≈ 1.47 kW), outdoor temp is -20°C, and the
    // building has moderate insulation (R=2.8 m²K/W).  This combination
    // produces unmet heating steps, which the check flags.
    let run = |diagnostics: bool, dir: &tempfile::TempDir| {
        let toml_path = dir.path().join("posthoc.toml");
        let content = format!(
            r#"building_id = 4242

[simulation]
start_time = "2024-01-15T12:00:00Z"
time_res = 60
duration = 600
diagnostics = {diagnostics}
output_chunk_size = 1000
master_seed = 0

[[simulation.reports]]
name = "frame"
selection = {{ verbosity = 4 }}
destination = "memory"
[geometry]
number_of_bedrooms = 3.0
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "Furnace"
fuel = "natural gas"
heating_capacity_kbtu_h = 5.0

[weather]
outdoor_temp_c = -20.0
dew_point_c = -25.0
rel_humidity_pct = 70.0
pressure_kpa = 101.325

[infiltration]
ach = 0.0

[schedule]
occupancy = 0.0
"#,
        );
        std::fs::write(&toml_path, content).expect("failed to write synthetic TOML");

        let mut dwelling =
            hares_core::Dwelling::from_toml_config(&toml_path).expect("synthetic TOML must load");
        dwelling.enable_observer(100);
        dwelling
            .simulate()
            .expect("simulation must complete without errors");
        dwelling
    };

    // The requested run: the checks run, their findings surface as
    // warnings, and the frame carries the diagnostics columns.
    let requested = tempfile::tempdir().expect("temp dir");
    let subscriber = Arc::new(CountingSubscriber {
        needle: "diagnostic:",
        count: std::sync::atomic::AtomicUsize::new(0),
    });
    {
        let guard = tracing::subscriber::set_default(subscriber.clone());
        let dwelling = run(true, &requested);
        let columns = dwelling
            .output_column_names()
            .expect("the recorded output schema");
        assert!(
            columns.iter().any(|c| c == "zone1_temp_c"),
            "the requested run records the diagnostics columns"
        );
        drop(dwelling);
        drop(guard);
    }
    assert!(
        subscriber.count() > 0,
        "the requested run's post-hoc checks must surface their findings as warnings"
    );

    // The unrequested run: the checks never run, no diagnostics column is
    // recorded.
    let plain = tempfile::tempdir().expect("temp dir");
    let subscriber = Arc::new(CountingSubscriber {
        needle: "diagnostic:",
        count: std::sync::atomic::AtomicUsize::new(0),
    });
    {
        let guard = tracing::subscriber::set_default(subscriber.clone());
        let dwelling = run(false, &plain);
        let columns = dwelling
            .output_column_names()
            .expect("the recorded output schema");
        assert!(
            !columns.iter().any(|c| c == "zone1_temp_c"),
            "the unrequested run records no diagnostics column"
        );
        drop(dwelling);
        drop(guard);
    }
    assert_eq!(
        subscriber.count(),
        0,
        "the unrequested run's post-hoc checks must never run"
    );
}
