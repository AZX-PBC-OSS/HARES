//! Regression tests for ActorInterest filtering in the simulation hot loop.
//!
//! Verifies that actors with declared interests are only called when their
//! interest triggers fire, and that EveryStep actors (and those with empty
//! interests) are always called.

use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration as StdDuration, SystemTime, UNIX_EPOCH};

use hares_control::DispatchRequest;
use hares_control::DispatchTarget;
use hares_core::Dwelling;
use hares_core::actor::{Actor, ActorInterest};
use hares_equipment::Equipment;
use hares_types::{
    ControlCapabilities, CoreCapabilities, CoreOutput, CoreState, EndUse, EnvironmentState,
    EquipmentDescriptor, EquipmentId, ExecutionStage, FuelType, HaresError, OperatingMode,
    PortSlots, Telemetry, TelemetryField, ZoneId,
};

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

/// A dwelling whose furnace makes a real mid-run operating-mode transition:
/// an oversized furnace (500 kBtu/h against a 48 m² zone at −20 °C) heats
/// the zone to setpoint within a few steps and cuts Heating → Off, beside
/// an event-load Cooking Range whose operating mode never changes. Built
/// through the real assembly path (`from_toml_config` →
/// `build_from_blueprint` → the id-injection pass), so equipment identity
/// is the production assignment, not hand-injected ids.
fn write_mode_toml(path: &std::path::Path) {
    let content = r#"building_id = 4243
[simulation]
start_time = "2024-01-15T00:00:00Z"
time_res_s = 60
duration_s = 3600
[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0
wall_area_m2 = 145.0
[materials]
wall_r_value_m2_k_w = 2.8
[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 500.0
[weather]
outdoor_temp_c = -20.0
dew_point_c = -5.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325
[schedule]
occupancy = 1.0
[event_load]
active_power_kw = 1.5
event_probability = 1.0
[output]
write_output = false
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
master_seed = 0
"#;
    fs::write(path, content).expect("write synthetic TOML");
}

/// `ActorInterest::EquipmentModeChange` must wake an actor exactly when its
/// target equipment's operating mode changes, attributed per equipment: a
/// transition on one equipment wakes that equipment's watcher and never
/// another equipment's watcher. This is the `prev_equipment_modes` consumer
/// from the I-06 class audit — under the pre-fix identity collapse every
/// equipment shared `EquipmentId(0)`, the map collapsed to one entry, and
/// every watcher compared against the wrong equipment's previous mode.
///
/// The wiring defect this test originally pinned red (via the
/// constitution's `#[should_panic]` carrier) is fixed:
/// `prev_equipment_modes` is now captured from the pre-snapshot
/// `equipment_core` — one generation behind the map the comparison reads —
/// so the interest fires on real transitions. The assertions below are
/// the permanent contract: a transition during step t must wake the
/// watcher at step t+1, and only then.
#[test]
fn equipment_mode_change_interest_wakes_the_changed_equipments_watcher() {
    let path = unique_temp_toml("mode_attribution");
    write_mode_toml(&path);
    let mut dwelling = Dwelling::from_toml_config(&path).expect("synthetic TOML must load");
    let _ = fs::remove_file(&path);

    // Discover the two equipment by end-use rather than hard-coding instance
    // names: the mode-transitioning furnace and the mode-invariant event load.
    let furnace_name = dwelling
        .equipment()
        .iter()
        .find(|eq| eq.descriptor().end_use == EndUse::HVAC_HEATING)
        .map(|eq| eq.descriptor().name.clone())
        .expect("the fixture must contain HVAC heating equipment");
    let other_name = dwelling
        .equipment()
        .iter()
        .find(|eq| eq.descriptor().end_use == EndUse::OTHER)
        .map(|eq| eq.descriptor().name.clone())
        .expect("the fixture must contain a second, non-HVAC equipment");

    let (furnace_watcher, furnace_calls) = CountingActor::new(
        "furnace_mode_watcher",
        vec![ActorInterest::EquipmentModeChange {
            target: DispatchTarget::ByName(furnace_name.clone().into()),
        }],
    );
    let (other_watcher, other_calls) = CountingActor::new(
        "other_mode_watcher",
        vec![ActorInterest::EquipmentModeChange {
            target: DispatchTarget::ByName(other_name.clone().into()),
        }],
    );
    // The same two watchers through the ByEndUse arm: the end-use loop
    // resolves the same equipment and reads the same previous-generation
    // map, so it must wake on exactly the same events as ByName — the
    // transition path of `equipment_mode_changed`'s second arm.
    let (furnace_end_use_watcher, furnace_end_use_calls) = CountingActor::new(
        "furnace_mode_watcher_end_use",
        vec![ActorInterest::EquipmentModeChange {
            target: DispatchTarget::ByEndUse(EndUse::HVAC_HEATING),
        }],
    );
    let (other_end_use_watcher, other_end_use_calls) = CountingActor::new(
        "other_mode_watcher_end_use",
        vec![ActorInterest::EquipmentModeChange {
            target: DispatchTarget::ByEndUse(EndUse::OTHER),
        }],
    );
    dwelling.add_actor(Box::new(furnace_watcher)).unwrap();
    dwelling.add_actor(Box::new(other_watcher)).unwrap();
    dwelling
        .add_actor(Box::new(furnace_end_use_watcher))
        .unwrap();
    dwelling.add_actor(Box::new(other_end_use_watcher)).unwrap();

    let mode_of = |dwelling: &Dwelling, name: &str| {
        dwelling
            .equipment()
            .iter()
            .find(|eq| eq.descriptor().name == name)
            .expect("watched equipment must stay in the vector")
            .core_output()
            .state
            .operating_mode
    };

    const STEPS: usize = 12;
    // Mode as of the end of each step; index 0 is pre-run (no snapshot
    // exists yet, so the mode reads as None).
    let mut furnace_modes: Vec<Option<OperatingMode>> = vec![None];
    let mut other_modes: Vec<Option<OperatingMode>> = vec![None];

    let mut prev_furnace_calls = 0usize;
    for step in 1..=STEPS {
        dwelling.step().expect("step");
        furnace_modes.push(mode_of(&dwelling, &furnace_name));
        other_modes.push(mode_of(&dwelling, &other_name));

        // A mode transition during step t must wake the watcher at step
        // t+1: the ActorDecide phase of step t+1 compares the end-of-step-t
        // snapshot against the previous generation. Step 2 is the
        // first-comparison warmup (pre-run None vs the first snapshot) —
        // allowed but not required, matching the first-comparison semantics
        // the other interests use.
        let calls = furnace_calls.load(Ordering::SeqCst);
        let fired_this_step = calls - prev_furnace_calls;
        let transition_last_step = step >= 3 && furnace_modes[step - 1] != furnace_modes[step - 2];
        if transition_last_step {
            assert!(
                fired_this_step >= 1,
                "EquipmentModeChange watcher of '{furnace_name}' was not woken \
                 after its target's mode transition at step {} ({:?} -> {:?}): \
                 watcher calls after step {step} = {calls} — the interest must \
                 fire when its target equipment's operating mode changes",
                step - 1,
                furnace_modes[step - 2],
                furnace_modes[step - 1],
            );
        } else if step != 2 {
            assert_eq!(
                fired_this_step,
                0,
                "EquipmentModeChange watcher of '{furnace_name}' woke at step \
                 {step} without a mode transition on its target (mode stayed \
                 {:?}) — a watcher must only wake on its own equipment's \
                 changes",
                furnace_modes[step - 1],
            );
        }
        prev_furnace_calls = calls;

        // The other equipment's mode never changes, so its watcher must
        // never wake — in particular not on the furnace's transitions.
        assert_eq!(
            other_calls.load(Ordering::SeqCst),
            0,
            "EquipmentModeChange watcher of '{other_name}' woke although its \
             target's mode never changed (mode {:?}) — mode changes must \
             attribute to the equipment that changed",
            other_modes[step - 1],
        );
        // Both arms of `equipment_mode_changed` must agree per step: the
        // ByEndUse loop resolves the same equipment the ByName watcher
        // names and compares against the same previous-generation map, so
        // any divergence is an arm-specific wiring defect.
        assert_eq!(
            furnace_end_use_calls.load(Ordering::SeqCst),
            calls,
            "the ByEndUse(HVAC_HEATING) watcher must wake on exactly the same \
             events as the ByName watcher of '{furnace_name}' (ByEndUse calls \
             = {}, ByName calls = {calls}) — both arms read the same \
             previous generation for the same equipment",
            furnace_end_use_calls.load(Ordering::SeqCst),
        );
        assert_eq!(
            other_end_use_calls.load(Ordering::SeqCst),
            0,
            "the ByEndUse({:?}) watcher woke although no equipment in that \
             end-use changed mode — mode changes must attribute to the \
             equipment that changed",
            EndUse::OTHER,
        );
    }

    // The fixture must actually exercise the mechanism: at least one
    // mid-run furnace transition, and a mode-invariant second equipment.
    assert!(
        (2..=STEPS).any(|t| furnace_modes[t] != furnace_modes[t - 1]),
        "fixture precondition: the furnace must transition mid-run at least \
         once, else this test asserts nothing (modes: {furnace_modes:?})"
    );
    assert!(
        (1..=STEPS).all(|t| other_modes[t] == other_modes[1]),
        "fixture precondition: the second equipment's mode must stay constant, \
         else the never-wake assertion in the loop above is unsound (modes: {other_modes:?})"
    );
}

// ---------------------------------------------------------------------------
// ModeProbeEquipment — a minimal mode-reporting equipment for the
// EquipmentModeChange interest tests
// ---------------------------------------------------------------------------

/// Real equipment populate `core_output` only inside `step()`, so a freshly
/// constructed (or freshly added) instance reports `operating_mode: None`
/// and cannot exercise the mode-change interest's first-comparison arm.
/// This stub carries `Some(OperatingMode::Off)` from construction and never
/// changes it, under a dedicated end-use so `ByEndUse` watchers isolate it.
struct ModeProbeEquipment {
    descriptor: EquipmentDescriptor,
    telemetry: Telemetry,
    core_output: CoreOutput,
    ports: Vec<hares_types::PortDeclaration>,
}

impl ModeProbeEquipment {
    fn new(name: &str) -> Self {
        Self {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(0),
                name: name.to_string(),
                end_use: EndUse::new("probe_load"),
                equipment_type: Cow::Borrowed("ModeProbeEquipment"),
                zone: None,
                fuel: FuelType::Electric,
                stage: ExecutionStage::Independent,
                control_capabilities: ControlCapabilities::empty(),
                core_capabilities: CoreCapabilities::HAS_MODE,
                telemetry_fields: vec![TelemetryField {
                    name: "probe".to_string(),
                    unit: "-".to_string(),
                    description: "mode probe".to_string(),
                }],
                zone_type: None,
            },
            telemetry: Telemetry::with_capacity(1),
            core_output: CoreOutput {
                state: CoreState {
                    operating_mode: Some(OperatingMode::Off),
                    ..CoreState::default()
                },
                ..CoreOutput::default()
            },
            ports: Vec::new(),
        }
    }
}

impl Equipment for ModeProbeEquipment {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn rename(&mut self, name: String) {
        self.descriptor.name = name;
    }

    fn set_equipment_id(&mut self, id: EquipmentId) -> Result<(), HaresError> {
        hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
    }

    fn ports(&self) -> &[hares_types::PortDeclaration] {
        &self.ports
    }

    fn init(
        &mut self,
        _: &hares_equipment::EquipmentConfig,
        _: &EnvironmentState,
    ) -> Result<(), HaresError> {
        Ok(())
    }

    fn update_control(&mut self, _: &EnvironmentState) -> OperatingMode {
        OperatingMode::Off
    }

    fn step(
        &mut self,
        _: &EnvironmentState,
        _: StdDuration,
        _: &mut PortSlots,
    ) -> Result<(), HaresError> {
        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core_output
    }

    fn save_state(&self) -> Result<Vec<u8>, HaresError> {
        Ok(vec![])
    }

    fn load_state(&mut self, _: &[u8]) -> Result<(), HaresError> {
        Ok(())
    }

    fn apply_signal(&mut self, _: &hares_types::ControlSignal) -> Result<(), HaresError> {
        Ok(())
    }
}

/// The `EquipmentModeChange` interest's first-comparison arm: when equipment
/// enters the vector mid-run, the identity refresh seeds its `equipment_core`
/// entry (the equipment's current core output) while `prev_equipment_modes`
/// still lacks the id — the capture runs at end-of-step, before the refresh
/// — so the joining step's comparison reads `Some(mode)` against `None` and
/// wakes the watcher — the same first-comparison convention every other
/// interest uses (`ZoneTemperatureDelta` fires when a zone is absent from
/// the prior map). First observation of an equipment is a real event an
/// actor must see, and the refresh seeding it depends on is itself a
/// landed fix (actors were blind to entering equipment on their joining
/// step).
///
/// Covers all three arms of `equipment_mode_changed`: `ByName` (resolved),
/// `ByEndUse` (the end-use loop), and `ByName` with an unresolved name
/// (must never wake — the map lookup miss, not a panic or a fallback).
#[test]
fn mode_change_interest_wakes_on_first_observation_of_midrun_added_equipment() {
    let mut dwelling = build_dwelling("mode_first_observation", "2024-06-15T12:00:00Z", 7200);
    // One step before the add settles the filter's previous-generation
    // maps, so the joining step's firing is attributable to the add alone.
    dwelling.step().expect("pre-add step");

    dwelling
        .add_equipment(Box::new(ModeProbeEquipment::new("Mode Probe")))
        .expect("mid-run add must succeed");

    let (by_name, by_name_calls) = CountingActor::new(
        "probe_watcher_by_name",
        vec![ActorInterest::EquipmentModeChange {
            target: DispatchTarget::ByName("Mode Probe".into()),
        }],
    );
    let (by_end_use, by_end_use_calls) = CountingActor::new(
        "probe_watcher_by_end_use",
        vec![ActorInterest::EquipmentModeChange {
            target: DispatchTarget::ByEndUse(EndUse::new("probe_load")),
        }],
    );
    let (unresolved, unresolved_calls) = CountingActor::new(
        "probe_watcher_unresolved",
        vec![ActorInterest::EquipmentModeChange {
            target: DispatchTarget::ByName("No Such Equipment".into()),
        }],
    );
    dwelling.add_actor(Box::new(by_name)).unwrap();
    dwelling.add_actor(Box::new(by_end_use)).unwrap();
    dwelling.add_actor(Box::new(unresolved)).unwrap();

    // Joining step: first observation of the added equipment wakes the
    // watchers that target it, through both dispatch-target arms.
    dwelling.step().expect("joining step");
    assert_eq!(
        by_name_calls.load(Ordering::SeqCst),
        1,
        "the ByName watcher must wake on first observation of its target \
         (the identity refresh seeds the equipment_core entry while \
         prev_equipment_modes still lacks the id)"
    );
    assert_eq!(
        by_end_use_calls.load(Ordering::SeqCst),
        1,
        "the ByEndUse watcher must wake on first observation of equipment in \
         its end-use — the end-use loop resolves the added equipment's name \
         to its id and compares against the previous generation"
    );
    assert_eq!(
        unresolved_calls.load(Ordering::SeqCst),
        0,
        "a watcher whose target name resolves to no equipment must never \
         wake — the name lookup miss is a quiet false, not an error"
    );

    // The probe's mode never changes, so no watcher wakes again — the
    // steady-state contract on the same mechanism.
    for _ in 0..3 {
        dwelling.step().expect("settling step");
    }
    assert_eq!(
        by_name_calls.load(Ordering::SeqCst),
        1,
        "no further ByName wakes: the probe's mode is constant"
    );
    assert_eq!(
        by_end_use_calls.load(Ordering::SeqCst),
        1,
        "no further ByEndUse wakes: the probe's mode is constant"
    );
    assert_eq!(
        unresolved_calls.load(Ordering::SeqCst),
        0,
        "an unresolved target stays silent forever"
    );
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
