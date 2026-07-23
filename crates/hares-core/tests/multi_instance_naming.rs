//! Tests for multi-instance equipment naming: duplicate detection, ByEndUse
//! fan-out, ByName ambiguity warnings, and `add_equipment` collision handling.

use std::borrow::Cow;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use hares_control::{DispatchRequest, DispatchTarget, PriorityTier};
use hares_core::Dwelling;
use hares_equipment::{Equipment, EquipmentConfig};
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, ElectricPower,
    EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId, ExecutionStage, FuelType,
    HaresError, OperatingMode, PortDeclaration, PortSlots, Telemetry, TelemetryField,
    telemetry_keys as tk,
};

fn nanos_suffix() -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::thread::current().id().hash(&mut h);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos();
    format!("{nanos}-{:x}", h.finish())
}

fn unique_temp_toml(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("hares-multi-inst-{tag}-{}.toml", nanos_suffix()));
    path
}

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

/// Minimal equipment stub that records telemetry on receiving a control signal.
/// Tests inspect `recorded` via `ac_power_kw` telemetry — the key is set to the
/// curtailment fraction received (or 0 when none was received).
struct TelemetryRecordSpy {
    descriptor: EquipmentDescriptor,
    telemetry: Telemetry,
    core_output: CoreOutput,
}

impl TelemetryRecordSpy {
    fn new(name: &str, end_use: EndUse, id: u32) -> Self {
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(id),
            name: name.to_string(),
            end_use,
            equipment_type: Cow::Borrowed("TelemetryRecordSpy"),
            zone: None,
            fuel: FuelType::Electric,
            stage: ExecutionStage::Independent,
            control_capabilities: ControlCapabilities::CURTAILMENT_PERCENT,
            core_capabilities: CoreCapabilities::ELECTRIC,
            telemetry_fields: vec![TelemetryField {
                name: tk::AC_POWER_KW.to_string(),
                unit: "kW".to_string(),
                description: "recorded curtailment fraction".to_string(),
            }],
            zone_type: None,
        };
        let mut telemetry = Telemetry::with_capacity(1);
        telemetry.insert(tk::AC_POWER_KW, 0.0);
        Self {
            descriptor,
            telemetry,
            core_output: CoreOutput {
                flows: CoreFlows {
                    electric_kw: Some(ElectricPower::Generation(0.0)),
                    ..Default::default()
                },
                ..Default::default()
            },
        }
    }

    #[allow(dead_code)]
    fn recorded_value(&self) -> f64 {
        self.telemetry.get(tk::AC_POWER_KW).unwrap_or(0.0)
    }
}

impl Equipment for TelemetryRecordSpy {
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
        OperatingMode::Off
    }

    fn step(
        &mut self,
        _env: &EnvironmentState,
        _dt: std::time::Duration,
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

    fn save_state(&self) -> hares_equipment::Result<Vec<u8>> {
        Ok(vec![])
    }

    fn load_state(&mut self, _state: &[u8]) -> hares_equipment::Result<()> {
        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> hares_equipment::Result<()> {
        let fraction = match signal {
            ControlSignal::CurtailmentPercent { percent } => *percent,
            _ => 100.0,
        };
        self.telemetry.insert(tk::AC_POWER_KW, fraction);
        Ok(())
    }
}

fn spy_recorded(dwelling: &Dwelling, name: &str) -> f64 {
    dwelling
        .equipment()
        .iter()
        .find(|eq| eq.descriptor().name == name)
        .map(|eq| eq.telemetry().get(tk::AC_POWER_KW).unwrap_or(0.0))
        .unwrap_or_else(|| panic!("equipment '{name}' not found"))
}

#[test]
fn add_equipment_rejects_duplicate_names() {
    let mut dwelling = build_dwelling("reject_dupes");
    dwelling
        .add_equipment(Box::new(TelemetryRecordSpy::new("PV", EndUse::PV, 0x8001)))
        .expect("first add_equipment must succeed");
    let result =
        dwelling.add_equipment(Box::new(TelemetryRecordSpy::new("PV", EndUse::PV, 0x8002)));
    assert!(
        result.is_err(),
        "add_equipment with duplicate name 'PV' must return Err"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("duplicate equipment name"),
        "error message must mention duplicate name, got: {err_msg}"
    );
}

#[test]
fn add_equipment_accepts_unique_names() {
    let mut dwelling = build_dwelling("unique_names");
    dwelling
        .add_equipment(Box::new(TelemetryRecordSpy::new(
            "PV #1",
            EndUse::PV,
            0x8001,
        )))
        .expect("add_equipment must succeed");
    dwelling
        .add_equipment(Box::new(TelemetryRecordSpy::new(
            "PV #2",
            EndUse::PV,
            0x8002,
        )))
        .expect("add_equipment must succeed");
    assert_eq!(dwelling.equipment().len(), 2);
}

#[test]
fn add_equipment_unique_different_types_ok() {
    let mut dwelling = build_dwelling("diff_types");
    dwelling
        .add_equipment(Box::new(TelemetryRecordSpy::new("PV", EndUse::PV, 0x8001)))
        .expect("add_equipment must succeed");
    dwelling
        .add_equipment(Box::new(TelemetryRecordSpy::new(
            "Battery",
            EndUse::BATTERY,
            0x8002,
        )))
        .expect("add_equipment must succeed");
    dwelling
        .add_equipment(Box::new(TelemetryRecordSpy::new("EV", EndUse::EV, 0x8003)))
        .expect("add_equipment must succeed");
    assert_eq!(dwelling.equipment().len(), 3);
}

#[test]
fn by_end_use_fans_out_to_all_matching_instances() {
    let mut dwelling = build_dwelling("fan_out");
    dwelling
        .add_equipment(Box::new(TelemetryRecordSpy::new(
            "Battery #1",
            EndUse::BATTERY,
            0x8001,
        )))
        .expect("add_equipment must succeed");
    dwelling
        .add_equipment(Box::new(TelemetryRecordSpy::new(
            "Battery #2",
            EndUse::BATTERY,
            0x8002,
        )))
        .expect("add_equipment must succeed");
    dwelling
        .add_equipment(Box::new(TelemetryRecordSpy::new(
            "PV #1",
            EndUse::PV,
            0x8003,
        )))
        .expect("add_equipment must succeed");

    dwelling.queue_dispatch(DispatchRequest {
        target: DispatchTarget::ByEndUse(EndUse::BATTERY),
        signal: ControlSignal::CurtailmentPercent { percent: 50.0 },
        priority: PriorityTier::UserOverride,
    });
    dwelling.step().expect("step must succeed");

    let bat1 = spy_recorded(&dwelling, "Battery #1");
    let bat2 = spy_recorded(&dwelling, "Battery #2");
    let pv1 = spy_recorded(&dwelling, "PV #1");
    assert!(
        (bat1 - 50.0).abs() < 1e-9,
        "Battery #1 must receive the signal"
    );
    assert!(
        (bat2 - 50.0).abs() < 1e-9,
        "Battery #2 must receive the signal"
    );
    assert!(
        (pv1 - 0.0).abs() < 1e-9,
        "PV #1 must NOT receive the signal"
    );
}

#[test]
fn by_name_targets_single_instance_only() {
    let mut dwelling = build_dwelling("by_name_one");
    dwelling
        .add_equipment(Box::new(TelemetryRecordSpy::new(
            "Battery #1",
            EndUse::BATTERY,
            0x8001,
        )))
        .expect("add_equipment must succeed");
    dwelling
        .add_equipment(Box::new(TelemetryRecordSpy::new(
            "Battery #2",
            EndUse::BATTERY,
            0x8002,
        )))
        .expect("add_equipment must succeed");

    dwelling.queue_dispatch(DispatchRequest {
        target: DispatchTarget::ByName(Arc::from("Battery #2")),
        signal: ControlSignal::CurtailmentPercent { percent: 30.0 },
        priority: PriorityTier::UserOverride,
    });
    dwelling.step().expect("step must succeed");

    let bat1 = spy_recorded(&dwelling, "Battery #1");
    let bat2 = spy_recorded(&dwelling, "Battery #2");
    assert!(
        (bat1 - 0.0).abs() < 1e-9,
        "Battery #1 must NOT receive the signal"
    );
    assert!(
        (bat2 - 30.0).abs() < 1e-9,
        "Battery #2 must receive the signal"
    );
}

#[test]
fn ambiguous_by_name_warns_and_delivers_nothing() {
    let mut dwelling = build_dwelling("ambiguous_by_name");
    dwelling
        .add_equipment(Box::new(TelemetryRecordSpy::new(
            "PV #1",
            EndUse::PV,
            0x8001,
        )))
        .expect("add_equipment must succeed");
    dwelling
        .add_equipment(Box::new(TelemetryRecordSpy::new(
            "PV #2",
            EndUse::PV,
            0x8002,
        )))
        .expect("add_equipment must succeed");

    dwelling.queue_dispatch(DispatchRequest {
        target: DispatchTarget::ByName(Arc::from("PV")),
        signal: ControlSignal::CurtailmentPercent { percent: 50.0 },
        priority: PriorityTier::UserOverride,
    });
    dwelling.step().expect("step must succeed");

    let pv1 = spy_recorded(&dwelling, "PV #1");
    let pv2 = spy_recorded(&dwelling, "PV #2");
    assert!(
        (pv1 - 0.0).abs() < 1e-9,
        "ByName('PV') must not match 'PV #1'"
    );
    assert!(
        (pv2 - 0.0).abs() < 1e-9,
        "ByName('PV') must not match 'PV #2'"
    );

    let warnings = dwelling.take_warnings();
    let has_ambig = warnings
        .iter()
        .any(|w| w.contains("ambiguous") && w.contains("PV"));
    assert!(
        has_ambig,
        "expected ambiguity warning for ByName('PV'), got: {warnings:?}"
    );
}

#[test]
fn single_instance_by_name_still_matches_bare_name() {
    let mut dwelling = build_dwelling("single_bare");
    dwelling
        .add_equipment(Box::new(TelemetryRecordSpy::new("PV", EndUse::PV, 0x8001)))
        .expect("add_equipment must succeed");

    dwelling.queue_dispatch(DispatchRequest {
        target: DispatchTarget::ByName(Arc::from("PV")),
        signal: ControlSignal::CurtailmentPercent { percent: 75.0 },
        priority: PriorityTier::UserOverride,
    });
    dwelling.step().expect("step must succeed");

    let pv = spy_recorded(&dwelling, "PV");
    assert!(
        (pv - 75.0).abs() < 1e-9,
        "single-instance ByName('PV') must match bare 'PV'"
    );

    let warnings = dwelling.take_warnings();
    assert!(
        !warnings.iter().any(|w| w.contains("ambiguous")),
        "single-instance ByName('PV') must not produce ambiguity warning, got: {warnings:?}"
    );
}
