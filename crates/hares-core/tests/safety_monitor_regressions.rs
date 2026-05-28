//! Regression tests verifying the SafetyMonitor's interaction with the
//! dispatch tier ordering: Safety-tier signals must override Grid-tier
//! signals from DR compliance during emergency conditions.

use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hares_control::{DispatchRequest, DispatchTarget, PriorityTier};
use hares_core::Dwelling;
use hares_core::actor::Actor;
use hares_core::actors::SafetyMonitor;
use hares_equipment::{Equipment, EquipmentConfig};
use hares_types::{
    ControlCapabilities, ControlSignal, CoreOutput, EndUse, EnvironmentState, EquipmentDescriptor,
    EquipmentId, ExecutionStage, FuelType, HaresError, OperatingMode, PortDeclaration, PortSlots,
    Telemetry,
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
    path.push(format!("hares-safety-{tag}-{}.toml", nanos_suffix()));
    path
}

fn write_minimal_toml(path: &PathBuf) {
    let content = r#"building_id = 4242

[simulation]
start_time = "2024-01-15T06:00:00Z"
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
outdoor_temp_c = -5.0
dew_point_c = -10.0
rel_humidity_pct = 70.0
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
// ModeTrackingEquipment — tracks the last mode dispatched via ModeOverride.
// Stores OperatingMode::as_code() in telemetry so tests can observe it
// without downcasting.
// ---------------------------------------------------------------------------

struct ModeTrackingEquipment {
    descriptor: EquipmentDescriptor,
    telemetry: Telemetry,
    core_output: CoreOutput,
}

impl ModeTrackingEquipment {
    fn new(name: &str) -> Self {
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(0x7100),
            name: name.to_string(),
            end_use: EndUse::HVAC_HEATING,
            equipment_type: Cow::Borrowed("ModeTrackingEquipment"),
            zone: None,
            fuel: FuelType::Electric,
            stage: ExecutionStage::Independent,
            control_capabilities: ControlCapabilities::MODE_OVERRIDE,
            core_capabilities: hares_types::CoreCapabilities::empty(),
            telemetry_fields: vec![],
        };
        let mut telemetry = Telemetry::with_capacity(2);
        telemetry.insert("last_mode_code", OperatingMode::Off.as_code());
        Self {
            descriptor,
            telemetry,
            core_output: CoreOutput::default(),
        }
    }
}

impl Equipment for ModeTrackingEquipment {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
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
        _ports: &mut PortSlots,
    ) -> Result<(), HaresError> {
        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core_output
    }

    fn save_state(&self) -> Vec<u8> {
        vec![]
    }

    fn load_state(&mut self, _state: &[u8]) -> Result<(), HaresError> {
        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> Result<(), HaresError> {
        if let ControlSignal::ModeOverride { mode } = signal {
            self.telemetry.set("last_mode_code", mode.as_code());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// GridOffActor — emits a Grid-tier ModeOverride { Off } every step
// ---------------------------------------------------------------------------

struct GridOffActor {
    name: Arc<str>,
    target: DispatchTarget,
}

impl GridOffActor {
    fn new(name: &str, target: DispatchTarget) -> Self {
        Self {
            name: Arc::from(name),
            target,
        }
    }
}

impl Actor for GridOffActor {
    fn name(&self) -> &str {
        &self.name
    }

    fn decide(&mut self, _env: &EnvironmentState, out: &mut Vec<DispatchRequest>) {
        out.push(DispatchRequest {
            target: self.target.clone(),
            signal: ControlSignal::ModeOverride {
                mode: OperatingMode::Off,
            },
            priority: PriorityTier::Grid,
        });
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn safety_tier_heating_overrides_grid_tier_off_during_freeze() {
    let equip_name = "hvac_stub";
    let target = DispatchTarget::ByEndUse(EndUse::HVAC_HEATING);

    let mut dwelling = build_dwelling("safety-beats-grid");
    dwelling.add_equipment(Box::new(ModeTrackingEquipment::new(equip_name)));

    // Register GridOffActor that emits Grid-tier Off every step.
    let grid_actor = GridOffActor::new("grid_off", target.clone());
    dwelling.add_actor(Box::new(grid_actor));

    // Register SafetyMonitor configured with freeze threshold of 30°C.
    // Since zone temp will be near 21°C (default indoor), this guarantees
    // a freeze breach and a Safety-tier Heating dispatch.
    let monitor = SafetyMonitor::new("safety")
        .with_target(target.clone())
        .with_freeze_protection_threshold(30.0);
    dwelling.add_actor(Box::new(monitor));

    dwelling.step().expect("step must succeed");

    // Verify the ModeTrackingEquipment received Heating (Safety won over Grid).
    let eq = dwelling
        .equipment()
        .iter()
        .find(|eq| eq.descriptor().name == equip_name)
        .expect("ModeTrackingEquipment must be present");

    let last_mode_code = eq
        .telemetry()
        .get("last_mode_code")
        .expect("last_mode_code telemetry must be set");

    assert_eq!(
        last_mode_code,
        OperatingMode::Heating.as_code(),
        "Safety-tier Heating ({}) should override Grid-tier Off ({}); got {}",
        OperatingMode::Heating.as_code(),
        OperatingMode::Off.as_code(),
        last_mode_code
    );
}

#[test]
fn safety_tier_heating_vs_grid_off_both_target_by_name() {
    let equip_name = "named_equip";
    let target_name = DispatchTarget::ByName(Arc::from(equip_name));

    let mut dwelling = build_dwelling("safety-vs-grid-named");
    dwelling.add_equipment(Box::new(ModeTrackingEquipment::new(equip_name)));

    let grid_actor = GridOffActor::new("grid_off_named", target_name.clone());
    dwelling.add_actor(Box::new(grid_actor));

    let monitor = SafetyMonitor::new("safety_named")
        .with_target(target_name.clone())
        .with_freeze_protection_threshold(30.0);
    dwelling.add_actor(Box::new(monitor));

    dwelling.step().expect("step must succeed");

    let eq = dwelling
        .equipment()
        .iter()
        .find(|eq| eq.descriptor().name == equip_name)
        .expect("ModeTrackingEquipment must be present");

    let last_mode_code = eq
        .telemetry()
        .get("last_mode_code")
        .expect("last_mode_code telemetry must be set");

    assert_eq!(
        last_mode_code,
        OperatingMode::Heating.as_code(),
        "Safety-tier Heating should override Grid-tier Off by name; got {}",
        last_mode_code
    );
}
