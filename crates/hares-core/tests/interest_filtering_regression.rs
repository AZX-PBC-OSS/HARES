//! Regression tests for ActorInterest filtering in the simulation hot loop.
//!
//! Verifies that actors with declared interests are only called when their
//! interest triggers fire, and that EveryStep actors (and those with empty
//! interests) are always called.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use hares_control::DispatchRequest;
use hares_core::Dwelling;
use hares_core::actor::{Actor, ActorInterest};
use hares_types::{EnvironmentState, ZoneId};

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn nanos_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos()
}

fn unique_temp_toml(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("hares-interest-{tag}-{}.toml", nanos_suffix()));
    path
}

#[allow(clippy::too_many_arguments)]
fn write_synthetic_toml(
    path: &PathBuf,
    start_time: &str,
    duration_s: i64,
    hvac_equipment: &str,
    hvac_fuel: &str,
    heating_capacity_kbtu_h: f64,
    outdoor_temp_c: f64,
    dew_point_c: f64,
) {
    let content = format!(
        r#"building_id = 4242

[simulation]
start_time = "{start_time}"
time_res_s = 60
duration_s = {duration_s}

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0
wall_area_m2 = 145.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "{hvac_equipment}"
fuel = "{hvac_fuel}"
heating_capacity_kbtu_h = {heating_capacity_kbtu_h}

[weather]
outdoor_temp_c = {outdoor_temp_c}
dew_point_c = {dew_point_c}
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
"#
    );
    fs::write(path, content).expect("failed to write synthetic TOML");
}

fn build_dwelling(tag: &str, start_time: &str, duration_s: i64) -> Dwelling {
    let path = unique_temp_toml(tag);
    write_synthetic_toml(&path, start_time, duration_s, "none", "", 0.0, 20.0, 10.0);
    let dwelling = Dwelling::from_toml_config(&path).expect("synthetic TOML must load");
    let _ = fs::remove_file(&path);
    dwelling
}

fn build_furnace_dwelling(tag: &str) -> Dwelling {
    let path = unique_temp_toml(tag);
    write_synthetic_toml(
        &path,
        "2024-01-15T00:00:00Z",
        36000,
        "Furnace",
        "electricity",
        500.0,
        -20.0,
        -25.0,
    );
    let dwelling = Dwelling::from_toml_config(&path).expect("synthetic TOML must load");
    let _ = fs::remove_file(&path);
    dwelling
}

// ---------------------------------------------------------------------------
// CountingActor — tracks how many times decide() was called
// ---------------------------------------------------------------------------

struct CountingActor {
    name: String,
    interests: Vec<ActorInterest>,
    call_count: Arc<AtomicUsize>,
}

impl CountingActor {
    fn new(name: &str, interests: Vec<ActorInterest>) -> (Self, Arc<AtomicUsize>) {
        let count = Arc::new(AtomicUsize::new(0));
        (
            Self {
                name: name.to_string(),
                interests,
                call_count: Arc::clone(&count),
            },
            count,
        )
    }
}

impl Actor for CountingActor {
    fn name(&self) -> &str {
        &self.name
    }

    fn interests(&self) -> &[ActorInterest] {
        &self.interests
    }

    fn decide(&mut self, _env: &EnvironmentState, out: &mut Vec<DispatchRequest>) {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        // Push a no-op request so the dispatch drain path is exercised.
        out.push(DispatchRequest {
            target: hares_control::DispatchTarget::ByEndUse(hares_types::EndUse::PV),
            signal: hares_types::ControlSignal::CurtailmentPercent { percent: 0.0 },
            priority: hares_control::PriorityTier::Schedule,
        });
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn every_step_actor_called_every_step() {
    let mut dwelling = build_dwelling("every_step", "2024-06-15T12:00:00Z", 7200);
    let (_actor, count) = CountingActor::new("every_step_actor", vec![]);
    dwelling.add_actor(Box::new(_actor)).unwrap();

    dwelling.step().expect("step 1");
    assert_eq!(count.load(Ordering::SeqCst), 1);

    dwelling.step().expect("step 2");
    assert_eq!(count.load(Ordering::SeqCst), 2);

    dwelling.step().expect("step 3");
    assert_eq!(count.load(Ordering::SeqCst), 3);
}

#[test]
fn every_step_variant_called_every_step() {
    let mut dwelling = build_dwelling("every_step_var", "2024-06-15T12:00:00Z", 7200);
    let (_actor, count) = CountingActor::new("every_step_actor", vec![ActorInterest::EveryStep]);
    dwelling.add_actor(Box::new(_actor)).unwrap();

    dwelling.step().expect("step 1");
    assert_eq!(count.load(Ordering::SeqCst), 1);

    dwelling.step().expect("step 2");
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[test]
fn time_of_day_actor_called_at_matching_hour_skipped_elsewhere() {
    // Simulation starts at 12:00Z. Step once = hour 12, step 60 more = hour 13.
    let mut dwelling = build_dwelling("tod", "2024-06-15T12:00:00Z", 7200);
    let (actor, count) =
        CountingActor::new("tod_actor", vec![ActorInterest::TimeOfDay { hour: 12 }]);
    dwelling.add_actor(Box::new(actor)).unwrap();

    // First step is at hour 12 — should trigger.
    dwelling.step().expect("step at hour 12");
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "actor must be called at matching hour 12"
    );

    // Steps 2..60 are at hour 12 — all should trigger.
    for _ in 1..60 {
        dwelling.step().expect("step at hour 12");
    }
    assert_eq!(
        count.load(Ordering::SeqCst),
        60,
        "all steps at hour 12 must trigger"
    );

    // Step 61 is at hour 13 — should skip.
    dwelling.step().expect("step at hour 13");
    assert_eq!(
        count.load(Ordering::SeqCst),
        60,
        "actor must be skipped at hour 13"
    );

    // Step 62 is also at hour 13 — should still skip.
    dwelling.step().expect("another step at hour 13");
    assert_eq!(
        count.load(Ordering::SeqCst),
        60,
        "actor must still be skipped at hour 13"
    );
}

#[test]
fn zone_temperature_delta_actor_skipped_on_small_delta() {
    let mut dwelling = build_dwelling("zone_delta", "2024-06-15T12:00:00Z", 7200);
    let (actor, count) = CountingActor::new(
        "zone_delta_actor",
        vec![ActorInterest::ZoneTemperatureDelta {
            zone: ZoneId(1),
            threshold_c: 2.0,
        }],
    );
    dwelling.add_actor(Box::new(actor)).unwrap();

    // Step 1: prior_zone_temps empty → triggers (first step).
    dwelling.step().expect("step 1");
    assert_eq!(count.load(Ordering::SeqCst), 1, "first step must trigger");

    // Step 2: prior_zone_temps still empty → triggers (first comparison warmup).
    dwelling.step().expect("step 2");
    assert_eq!(
        count.load(Ordering::SeqCst),
        2,
        "second step triggers (prior empty)"
    );

    // Step 3: with stable weather (no HVAC, outdoor=indoor), zone temp delta
    // is negligible → interest skips.
    dwelling.step().expect("step 3");
    assert_eq!(
        count.load(Ordering::SeqCst),
        2,
        "third step must skip (delta below threshold)"
    );

    // Step 4: still negligible delta.
    dwelling.step().expect("step 4");
    assert_eq!(
        count.load(Ordering::SeqCst),
        2,
        "fourth step must still skip"
    );
}

#[test]
fn zone_temperature_delta_actor_called_when_threshold_exceeded() {
    let mut dwelling = build_furnace_dwelling("zone_delta_above");

    let (actor, count) = CountingActor::new(
        "zone_delta_actor",
        vec![ActorInterest::ZoneTemperatureDelta {
            zone: ZoneId(1),
            threshold_c: 2.0,
        }],
    );
    dwelling.add_actor(Box::new(actor)).unwrap();

    // Step 1: prior_zone_temps empty → triggers.
    let result1 = dwelling.step().expect("step 1");
    assert_eq!(count.load(Ordering::SeqCst), 1, "step 1 must trigger");
    let t1 = result1
        .zone_temperatures_c
        .iter()
        .find(|(id, _)| id.0 == 1)
        .map(|(_, t)| *t)
        .expect("zone 1 must be present");

    // Step 2: furnace fires, zone heats many degrees; prior is still empty so
    // the interest fires regardless — first comparison warmup step.
    let result2 = dwelling.step().expect("step 2");
    let t2 = result2
        .zone_temperatures_c
        .iter()
        .find(|(id, _)| id.0 == 1)
        .map(|(_, t)| *t)
        .expect("zone 1 must be present");
    assert_eq!(
        count.load(Ordering::SeqCst),
        2,
        "step 2 fires (prior empty)"
    );

    // Step 3: prior now holds step 1's zone temp, latest_env holds step 2's.
    // The delta between step 1 and step 2 must exceed the threshold because
    // the furnace heats the zone by ~10 °C when it fires.
    dwelling.step().expect("step 3");
    let delta_step2 = (t2 - t1).abs();
    assert!(
        delta_step2 >= 2.0,
        "zone temp delta between step 1 and step 2 must exceed 2.0 °C; \
         observed delta = {delta_step2:.4} °C (t1={t1:.4}, t2={t2:.4})"
    );
    assert_eq!(
        count.load(Ordering::SeqCst),
        3,
        "actor must be called on step 3 when prior delta {delta_step2:.4} °C exceeds 2.0 °C threshold"
    );
}

#[test]
fn multiple_interests_call_actor_when_any_triggers() {
    // Actor with both TimeOfDay { hour: 12 } and PriceSignalChange.
    // On first step, PriceSignalChange triggers (no previous state).
    // On subsequent steps at hour 12, TimeOfDay triggers.
    let mut dwelling = build_dwelling("multi_interest", "2024-06-15T12:00:00Z", 7200);
    let (actor, count) = CountingActor::new(
        "multi_actor",
        vec![
            ActorInterest::TimeOfDay { hour: 12 },
            ActorInterest::PriceSignalChange,
        ],
    );
    dwelling.add_actor(Box::new(actor)).unwrap();

    dwelling.step().expect("step 1");
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "both interests trigger on first step"
    );

    dwelling.step().expect("step 2");
    assert_eq!(
        count.load(Ordering::SeqCst),
        2,
        "TimeOfDay keeps it firing at hour 12"
    );
}

#[test]
fn empty_interests_treated_as_every_step() {
    let mut dwelling = build_dwelling("empty_interests", "2024-06-15T12:00:00Z", 7200);
    let (actor, count) = CountingActor::new("empty_actor", vec![]);
    dwelling.add_actor(Box::new(actor)).unwrap();

    dwelling.step().expect("step 1");
    assert_eq!(count.load(Ordering::SeqCst), 1);

    dwelling.step().expect("step 2");
    assert_eq!(count.load(Ordering::SeqCst), 2);

    dwelling.step().expect("step 3");
    assert_eq!(count.load(Ordering::SeqCst), 3);
}

#[test]
fn price_signal_change_actor_triggered_on_first_step() {
    let mut dwelling = build_dwelling("price_change", "2024-06-15T12:00:00Z", 7200);
    let (actor, count) = CountingActor::new("price_actor", vec![ActorInterest::PriceSignalChange]);
    dwelling.add_actor(Box::new(actor)).unwrap();

    // First step: prev_zone_temps is empty → PriceSignalChange triggers.
    dwelling.step().expect("step 1");
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "first step must trigger PriceSignalChange"
    );

    // Second step: price signal unchanged → skip.
    dwelling.step().expect("step 2");
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "second step must skip (price unchanged)"
    );
}
