//! Regression tests for dispatch ordering and telemetry staging inside a
//! single timestep. Each test pins an invariant that used to be violated by
//! the pre-fix ordering (BLOCKER 1/2/3/4 of the dispatch-ordering audit).
//!
//! Every scenario is deterministic: no stochastic schedules, no RNG, no
//! wall-clock or weather sampling of real files. Scenarios build a minimal
//! synthetic-TOML dwelling and inject a programmatic `StubPowerEquipment`
//! that is fully controllable via the same `ControlSignal` / `DispatchRequest`
//! boundary the production PV/Battery/EV equipment uses. The stub exercises
//! the same dispatch path without requiring a parity HPXML fixture.

use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hares_control::{DispatchRequest, DispatchTarget, PriorityTier};
use hares_core::Dwelling;
use hares_core::actor::Actor;
use hares_equipment::{Equipment, EquipmentConfig};
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CorePerformance,
    CoreState, ElectricPower, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FuelType, HaresError, OperatingMode, PortContribution, PortDeclaration,
    PortSlots, Telemetry, TelemetryField, telemetry_keys as tk,
};

// ---------------------------------------------------------------------------
// Synthetic dwelling setup
// ---------------------------------------------------------------------------

fn nanos_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos()
}

fn unique_temp_toml(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("hares-dispatch-{tag}-{}.toml", nanos_suffix()));
    path
}

/// Write a minimal synthetic-TOML dwelling that loads deterministically.
fn write_minimal_toml(path: &PathBuf) {
    let content = r#"building_id = 4242

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res_s = 60
duration_s = 600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0
wall_area_m2 = 145.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "none"

[weather]
outdoor_temp_c = 20.0
dew_point_c = 10.0
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
"#;
    fs::write(path, content).expect("failed to write synthetic TOML");
}

fn build_dwelling(tag: &str) -> Dwelling {
    let path = unique_temp_toml(tag);
    write_minimal_toml(&path);
    let dwelling = Dwelling::from_toml_config(&path).expect("synthetic TOML must load");
    let _ = fs::remove_file(&path);
    dwelling
}

// ---------------------------------------------------------------------------
// StubPowerEquipment
//
// A non-thermal, electric-generating Equipment analogous to PV/Battery/EV:
//   - Stage: Independent (runs in the non-thermal equipment phase)
//   - Generates a fixed -base_kw signed electric power (negative = source)
//   - Supports CurtailmentPercent to scale that output 0..100%
//   - Writes a telemetry key "ac_power_kw" each step so we can observe
//     that equipment_telemetry is populated
//   - Records the last curtailment seen when step() ran, so tests can
//     assert that the dispatch reached us BEFORE the step for this timestep
// ---------------------------------------------------------------------------

struct StubPowerEquipment {
    descriptor: EquipmentDescriptor,
    telemetry: Telemetry,
    core_output: CoreOutput,
    base_kw: f64,
    /// Curtailment currently applied. 0.0 = no curtailment, 1.0 = 100%.
    curtailment_fraction: f64,
    /// The curtailment fraction that was in effect at the moment step() ran
    /// this timestep. This lets tests distinguish "dispatched and stepped
    /// same-timestep" from "dispatched but stepped pre-dispatch".
    curtailment_at_step_time: f64,
}

impl StubPowerEquipment {
    fn new(name: &str, base_kw: f64) -> Self {
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(0x7001),
            name: name.to_string(),
            end_use: EndUse::PV,
            equipment_type: Cow::Borrowed("StubPowerEquipment"),
            zone: None,
            fuel: FuelType::Electric,
            stage: ExecutionStage::Independent,
            control_capabilities: ControlCapabilities::CURTAILMENT_PERCENT,
            core_capabilities: CoreCapabilities::ELECTRIC,
            telemetry_fields: vec![TelemetryField {
                name: tk::AC_POWER_KW.to_string(),
                unit: "kW".to_string(),
                description: "stub AC output".to_string(),
            }],
            zone_type: None,
        };
        let mut telemetry = Telemetry::with_capacity(1);
        telemetry.insert(tk::AC_POWER_KW, 0.0);
        Self {
            descriptor,
            telemetry,
            core_output: CoreOutput::default(),
            base_kw,
            curtailment_fraction: 0.0,
            curtailment_at_step_time: f64::NAN,
        }
    }
}

impl Equipment for StubPowerEquipment {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn rename(&mut self, name: String) {
        self.descriptor.name = name;
    }

    fn ports(&self) -> &[PortDeclaration] {
        &[]
    }

    fn init(
        &mut self,
        _config: &EquipmentConfig,
        _env: &EnvironmentState,
    ) -> Result<(), HaresError> {
        Ok(())
    }

    fn update_control(&mut self, _env: &EnvironmentState) -> OperatingMode {
        OperatingMode::On
    }

    fn step(
        &mut self,
        _env: &EnvironmentState,
        _dt: Duration,
        ports: &mut PortSlots,
    ) -> Result<(), HaresError> {
        self.curtailment_at_step_time = self.curtailment_fraction;
        let effective_kw = self.base_kw * (1.0 - self.curtailment_fraction);
        // Deposit the generation on the electrical bus so the CoreOutput
        // below stays consistent with the port contribution (enforced by
        // validate_port_core_electrical_consistency in debug builds).
        ports.accumulate(&PortContribution::Electrical {
            active_power_w: -effective_kw.max(0.0) * 1000.0,
            reactive_power_kvar: 0.0,
        })?;
        // Generation convention: signed_kw will be negative for this source.
        self.core_output = CoreOutput {
            state: CoreState::default(),
            flows: CoreFlows {
                electric_kw: Some(
                    ElectricPower::generation(effective_kw.max(0.0))
                        .expect("effective_kw must be finite and non-negative"),
                ),
                ..Default::default()
            },
            performance: CorePerformance::default(),
        };
        self.telemetry.insert(tk::AC_POWER_KW, effective_kw);
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

    fn load_state(&mut self, _state: &[u8]) -> Result<(), HaresError> {
        Ok(())
    }

    fn apply_signal(&mut self, signal: &ControlSignal) -> Result<(), HaresError> {
        if let ControlSignal::CurtailmentPercent { percent } = signal {
            self.curtailment_fraction = (percent / 100.0).clamp(0.0, 1.0);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// OneShotActor — deterministic actor that emits one DispatchRequest on a
// specified step index. No RNG, no schedules.
// ---------------------------------------------------------------------------

struct OneShotActor {
    name: String,
    step_counter: u64,
    fire_on_step: u64,
    request: Option<DispatchRequest>,
}

impl OneShotActor {
    fn new(name: &str, fire_on_step: u64, request: DispatchRequest) -> Self {
        Self {
            name: name.to_string(),
            step_counter: 0,
            fire_on_step,
            request: Some(request),
        }
    }
}

impl Actor for OneShotActor {
    fn name(&self) -> &str {
        &self.name
    }

    fn decide(&mut self, _env: &EnvironmentState, out: &mut Vec<DispatchRequest>) {
        if self.step_counter == self.fire_on_step
            && let Some(req) = self.request.take()
        {
            out.push(req);
        }
        self.step_counter += 1;
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Magnitude of generation (positive) observed on the stub. Generation is
/// stored via `ElectricPower::Generation(effective_kw)` whose `signed_kw()`
/// is negative; we flip sign to report a positive generation magnitude.
fn stub_output_kw(dwelling: &Dwelling, name: &str) -> f64 {
    let signed = dwelling
        .equipment()
        .iter()
        .find(|eq| eq.descriptor().name == name)
        .and_then(|eq| eq.core_output().flows.electric_kw)
        .map(|p| p.signed_kw())
        .expect("stub equipment must be present");
    -signed
}

fn stub_equipment_id(dwelling: &Dwelling, name: &str) -> EquipmentId {
    dwelling
        .equipment()
        .iter()
        .find(|eq| eq.descriptor().name == name)
        .map(|eq| eq.descriptor().id)
        .expect("stub equipment must be present")
}

// ---------------------------------------------------------------------------
// BLOCKER 1: Non-thermal equipment previously stepped BEFORE actor dispatch,
// so a control emitted by an actor at step N only took effect at step N+1.
// With the fix, the non-thermal equipment step runs AFTER the actor decide
// and dispatcher apply phases, so the signal is in effect the same step.
// ---------------------------------------------------------------------------

#[test]
fn actor_curtailment_applies_same_step_to_nonthermal_equipment() {
    let stub_name = "pv_stub_same_step";
    let base_kw = 5.0;
    let mut dwelling = build_dwelling("blocker1");
    dwelling
        .add_equipment(Box::new(StubPowerEquipment::new(stub_name, base_kw)))
        .expect("add_equipment must succeed");

    // Actor fires on step 0 (first simulated step). With the fix the stub's
    // step() sees the curtailment in the same timestep the actor emitted it.
    dwelling
        .add_actor(Box::new(OneShotActor::new(
            "pv_derate",
            0,
            DispatchRequest {
                target: DispatchTarget::ByName(Arc::from(stub_name)),
                signal: ControlSignal::CurtailmentPercent { percent: 50.0 },
                priority: PriorityTier::UserOverride,
            },
        )))
        .unwrap();

    dwelling.step().expect("step must succeed");

    // With pre-fix ordering the stub would have stepped BEFORE the actor
    // dispatch and its output would be the full base_kw.
    let observed_kw = stub_output_kw(&dwelling, stub_name);
    assert!(
        (observed_kw - base_kw * 0.5).abs() < 1e-9,
        "CurtailmentPercent(50) emitted by actor on step 0 must reduce stub output to 50% same step: expected={:.4} kW observed={:.4} kW",
        base_kw * 0.5,
        observed_kw
    );

    // Introspect the stub to confirm the dispatch was in effect at step() time,
    // not just applied after step().
    let stub_ref = dwelling
        .equipment()
        .iter()
        .find(|eq| eq.descriptor().name == stub_name)
        .expect("stub must exist");
    // Downcast via telemetry readback is sufficient: AC_POWER_KW is written
    // inside step() using the curtailment that was active there.
    let ac = stub_ref
        .telemetry()
        .get(tk::AC_POWER_KW)
        .expect("telemetry must be populated");
    assert!(
        (ac - base_kw * 0.5).abs() < 1e-9,
        "step-time telemetry must reflect the curtailment dispatched pre-step: expected={:.4} observed={:.4}",
        base_kw * 0.5,
        ac
    );
}

// ---------------------------------------------------------------------------
// BLOCKER 2: latest_env.equipment_core must snapshot each equipment's
// current-step post-step CoreOutput so downstream telemetry consumers see
// *this* step's value, never the prior step's value.
// ---------------------------------------------------------------------------

#[test]
fn equipment_core_reflects_current_step_after_dispatch() {
    let stub_name = "pv_stub_env_core";
    let base_kw = 4.0;
    let mut dwelling = build_dwelling("blocker2");
    dwelling
        .add_equipment(Box::new(StubPowerEquipment::new(stub_name, base_kw)))
        .expect("add_equipment must succeed");

    // Step once at baseline.
    dwelling.step().expect("step1 ok");
    let baseline_core_kw = stub_output_kw(&dwelling, stub_name);
    assert!(
        (baseline_core_kw - base_kw).abs() < 1e-9,
        "baseline step must see full base_kw output ({base_kw} kW), got {baseline_core_kw} kW"
    );

    // Queue a dispatch before step 2; after step 2 both the equipment's own
    // core_output AND latest_env.equipment_core must show the derated value.
    dwelling.queue_dispatch(DispatchRequest {
        target: DispatchTarget::ByName(Arc::from(stub_name)),
        signal: ControlSignal::CurtailmentPercent { percent: 80.0 },
        priority: PriorityTier::UserOverride,
    });
    dwelling.step().expect("step2 ok");

    let post_core_kw = stub_output_kw(&dwelling, stub_name);
    let expected = base_kw * 0.2;
    assert!(
        (post_core_kw - expected).abs() < 1e-9,
        "post-dispatch equipment core must be derated to 20%: expected={expected:.4} observed={post_core_kw:.4}"
    );

    // latest_env.equipment_core must mirror the equipment's core_output for
    // the same step (not lag by one step).
    let id = stub_equipment_id(&dwelling, stub_name);
    let env_core = dwelling
        .latest_env()
        .equipment_core
        .get(&id)
        .expect("latest_env.equipment_core must contain stub after step");
    let env_kw = env_core
        .flows
        .electric_kw
        .map(|p| -p.signed_kw())
        .expect("env core must carry electric_kw");
    assert!(
        (env_kw - post_core_kw).abs() < 1e-12,
        "latest_env.equipment_core must mirror the equipment's current-step CoreOutput: env={env_kw:.6} eq={post_core_kw:.6}"
    );
}

// ---------------------------------------------------------------------------
// BLOCKER 3: latest_env.equipment_telemetry must be populated each step so
// downstream consumers (Python bindings, actors on the next step) can read
// per-equipment telemetry. Pre-fix: the map was declared but never written.
// ---------------------------------------------------------------------------

#[test]
fn equipment_telemetry_populated_for_every_equipment_after_step() {
    let stub_name = "pv_stub_telemetry";
    let base_kw = 3.0;
    let mut dwelling = build_dwelling("blocker3");
    dwelling
        .add_equipment(Box::new(StubPowerEquipment::new(stub_name, base_kw)))
        .expect("add_equipment must succeed");

    assert!(
        dwelling.latest_env().equipment_telemetry.is_empty(),
        "pre-step telemetry map must start empty"
    );

    dwelling.step().expect("step must succeed");

    let env = dwelling.latest_env();
    assert!(
        !env.equipment_telemetry.is_empty(),
        "equipment_telemetry must be populated after the first step"
    );
    for eq in dwelling.equipment() {
        let name = &eq.descriptor().name;
        let snapshot = env
            .equipment_telemetry
            .get(name)
            .unwrap_or_else(|| panic!("missing equipment_telemetry entry for '{name}'"));
        let live = eq.telemetry();
        assert_eq!(
            snapshot.0, live.0,
            "equipment_telemetry snapshot for '{name}' must mirror the equipment's live telemetry"
        );
    }
}

// ---------------------------------------------------------------------------
// BLOCKER 4: Priority inversion across dispatch passes in one step. A Safety
// signal arriving in the pre-thermal-FSM pass must NOT be overwritten by a
// lower-tier (Schedule) signal emitted by an actor later in the same step;
// and vice versa. The dispatcher carries a cross-pass seen-targets ledger.
// ---------------------------------------------------------------------------

#[test]
fn priority_inversion_safety_wins_over_later_low_priority_signal() {
    let stub_name = "pv_stub_priority";
    let base_kw = 6.0;

    // Scenario A: Safety queued externally (pre-dispatch), Schedule emitted
    // by actor (post-decide). Safety must win -> full curtailment observed.
    let mut dwelling_a = build_dwelling("blocker4a");
    dwelling_a
        .add_equipment(Box::new(StubPowerEquipment::new(stub_name, base_kw)))
        .expect("add_equipment must succeed");
    dwelling_a.queue_dispatch(DispatchRequest {
        target: DispatchTarget::ByName(Arc::from(stub_name)),
        signal: ControlSignal::CurtailmentPercent { percent: 100.0 },
        priority: PriorityTier::Safety,
    });
    dwelling_a
        .add_actor(Box::new(OneShotActor::new(
            "schedule_reenable",
            0,
            DispatchRequest {
                target: DispatchTarget::ByName(Arc::from(stub_name)),
                signal: ControlSignal::CurtailmentPercent { percent: 0.0 },
                priority: PriorityTier::Schedule,
            },
        )))
        .unwrap();
    dwelling_a.step().expect("scenario A step must succeed");
    let observed_a = stub_output_kw(&dwelling_a, stub_name);
    assert!(
        observed_a.abs() < 1e-9,
        "Scenario A: Safety queued pre-dispatch must beat later Schedule (observed {observed_a:.6} kW, expected ~0)"
    );

    // Scenario B: Schedule queued externally, Safety emitted by actor.
    // Safety must still win -> full curtailment.
    let mut dwelling_b = build_dwelling("blocker4b");
    dwelling_b
        .add_equipment(Box::new(StubPowerEquipment::new(stub_name, base_kw)))
        .expect("add_equipment must succeed");
    dwelling_b.queue_dispatch(DispatchRequest {
        target: DispatchTarget::ByName(Arc::from(stub_name)),
        signal: ControlSignal::CurtailmentPercent { percent: 0.0 },
        priority: PriorityTier::Schedule,
    });
    dwelling_b
        .add_actor(Box::new(OneShotActor::new(
            "safety_block",
            0,
            DispatchRequest {
                target: DispatchTarget::ByName(Arc::from(stub_name)),
                signal: ControlSignal::CurtailmentPercent { percent: 100.0 },
                priority: PriorityTier::Safety,
            },
        )))
        .unwrap();
    dwelling_b.step().expect("scenario B step must succeed");
    let observed_b = stub_output_kw(&dwelling_b, stub_name);
    assert!(
        observed_b.abs() < 1e-9,
        "Scenario B: Safety emitted by actor must beat earlier Schedule (observed {observed_b:.6} kW, expected ~0)"
    );
}

// ---------------------------------------------------------------------------
// BLOCKER 5: HumiditySolver telemetry survives the equipment_telemetry retain.
//
// `apply_humidity_update_to_zones` writes the per-zone semi-implicit alpha
// under a synthetic "HumiditySolver" key. The `retain` that keeps only
// registered equipment names previously evicted this entry, making the alpha
// unobservable. The retain now exempts "HumiditySolver".
// ---------------------------------------------------------------------------

#[test]
fn humidity_solver_telemetry_survives_step_retain() {
    let mut dwelling = build_dwelling("blocker5");

    dwelling.step().expect("step must succeed");

    let env = dwelling.latest_env();
    assert!(
        env.equipment_telemetry
            .contains_key(hares_types::telemetry_keys::HUMIDITY_SOLVER_TELEMETRY_KEY),
        "HumiditySolver entry must survive the equipment_telemetry retain after step"
    );

    let telem = env
        .equipment_telemetry
        .get(hares_types::telemetry_keys::HUMIDITY_SOLVER_TELEMETRY_KEY)
        .expect("HumiditySolver telemetry must be present");

    let alpha_key_prefix = hares_types::telemetry_keys::HUMIDITY_SEMI_IMPLICIT_ALPHA;
    let has_alpha_keys = telem.0.keys().any(|k| k.starts_with(alpha_key_prefix));
    assert!(
        has_alpha_keys,
        "HumiditySolver telemetry must contain per-zone alpha keys (prefix: {alpha_key_prefix})"
    );
}
