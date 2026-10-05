//! Zone-state ordering within a step.
//!
//! Non-thermal (Independent-stage) equipment such as PV, batteries and EVs
//! steps before the envelope integrates, so it reads the zone temperatures
//! the previous step committed, the same predictor state thermal equipment
//! reads. The humidity solver's fallback to `env.zones` humidity is reachable
//! only before its own state exists; a multi-step run keeps its output valid.

use std::borrow::Cow;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hares_core::Dwelling;
use hares_equipment::{Equipment, EquipmentConfig};
use hares_types::{
    ControlCapabilities, CoreCapabilities, CoreOutput, EndUse, EnvironmentState,
    EquipmentDescriptor, EquipmentId, ExecutionStage, FuelType, HaresError, OperatingMode,
    PortDeclaration, PortSlots, Telemetry,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn write_minimal_toml(path: &Path) {
    let content = r#"building_id = 4545

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res_s = 60
duration_s = 600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

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
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join(format!("{tag}.toml"));
    write_minimal_toml(&path);
    Dwelling::from_toml_config(&path).expect("synthetic TOML must load")
}

// ---------------------------------------------------------------------------
// ZoneTemperatureSnifferEquipment
//
// An Independent-stage (non-thermal) equipment stub that records the zone
// temperatures seen by `env.zones` at the moment `step()` is called.
// This lets us verify that non-thermal equipment sees pre-integrate zone
// temperatures (the same predictor-consistent state as thermal equipment),
// NOT post-integrate temperatures written by `apply_thermal_update_to_zones`.
// ---------------------------------------------------------------------------

/// Shared record of zone temperatures captured inside each `step()` call.
type ZoneTempRecord = Arc<Mutex<Vec<f64>>>;

struct ZoneTemperatureSniffer {
    descriptor: EquipmentDescriptor,
    core_output: CoreOutput,
    telemetry: Telemetry,
    /// Zone temperatures recorded at each step() call (one entry per step).
    recorded_temps: ZoneTempRecord,
}

impl ZoneTemperatureSniffer {
    fn new(name: &str, recorded_temps: ZoneTempRecord) -> Self {
        Self {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(0x4501),
                name: name.to_string(),
                end_use: EndUse::PV,
                equipment_type: Cow::Borrowed("ZoneTemperatureSniffer"),
                zone: None,
                fuel: FuelType::Electric,
                stage: ExecutionStage::Independent,
                control_capabilities: ControlCapabilities::empty(),
                core_capabilities: CoreCapabilities::empty(),
                telemetry_fields: vec![],
                zone_type: None,
            },
            core_output: CoreOutput::default(),
            telemetry: Telemetry::with_capacity(0),
            recorded_temps,
        }
    }
}

impl Equipment for ZoneTemperatureSniffer {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }
    fn rename(&mut self, name: String) {
        self.descriptor.name = name;
    }
    fn set_equipment_id(&mut self, id: EquipmentId) -> hares_equipment::Result<()> {
        hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
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
        env: &EnvironmentState,
        _dt: Duration,
        _ports: &mut PortSlots,
    ) -> Result<(), HaresError> {
        let mut record = self.recorded_temps.lock().unwrap();
        for zone in &env.zones {
            record.push(zone.temperature_c);
        }
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
    fn apply_signal(&mut self, _signal: &hares_types::ControlSignal) -> Result<(), HaresError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// In every step, Independent-stage equipment reads exactly the zone
/// temperatures the previous step committed (the dwelling's zone state
/// before the step), never the values the step's own envelope integration
/// produces. The run must move the zone temperature, or the comparison could
/// not tell the two apart.
#[test]
fn nonthermal_equipment_reads_the_previous_steps_zone_temperatures() {
    let recorded: ZoneTempRecord = Arc::new(Mutex::new(Vec::new()));
    let mut dwelling = build_dwelling("zone-state-prior-step");
    dwelling
        .add_equipment(Box::new(ZoneTemperatureSniffer::new(
            "zone_temp_sniffer",
            Arc::clone(&recorded),
        )))
        .expect("add_equipment must succeed");

    let zone_temps = |dwelling: &Dwelling| -> Vec<f64> {
        dwelling
            .latest_env()
            .zones
            .iter()
            .map(|zone| zone.temperature_c)
            .collect()
    };

    let mut zone_state_moved = false;
    for step in 0..5 {
        let before = zone_temps(&dwelling);
        let seen_from = recorded.lock().unwrap().len();
        dwelling.step().expect("step must succeed");
        let seen = recorded.lock().unwrap()[seen_from..].to_vec();
        let after = zone_temps(&dwelling);

        assert_eq!(
            seen, before,
            "step {step}: equipment must see the zone temperatures committed before the step"
        );
        zone_state_moved |= after != before;
    }
    assert!(
        zone_state_moved,
        "the zone temperature never changed, so the run cannot distinguish \
         prior-step from integrated values"
    );
}

/// Defect 2 (mitigated): The humidity solver w_old fallback at
/// humidity_solver.rs:129-133 reads `zone.humidity_ratio` from `env.zones` only
/// when `self.humidity_ratios` lacks an entry for the zone. Because
/// `HumiditySolver::new` populates `humidity_ratios` for every zone in
/// `env.zones`, the fallback cannot be reached in steady-state operation.
///
/// This test verifies that a multi-step simulation completes without panic or
/// NaN in the humidity output — if the fallback were producing stale values
/// that broke the solver, this would surface as NaN or an extreme humidity ratio.
#[test]
fn humidity_solver_produces_valid_output_across_multiple_steps() {
    let mut dwelling = build_dwelling("zone-state-defect2");

    // Run 10 timesteps — enough to pass through any initialization edge case.
    for step in 0..10 {
        dwelling
            .step()
            .unwrap_or_else(|e| panic!("step {step} failed: {e}"));
    }

    // If the humidity fallback were broken (stale w_old) the solver would
    // produce NaN or a negative humidity ratio. Verify via the latest_env zones.
    let env = dwelling.latest_env();
    for zone in &env.zones {
        assert!(
            zone.humidity_ratio.is_finite() && zone.humidity_ratio >= 0.0,
            "zone {} humidity ratio must be finite and non-negative after 10 steps; got {}",
            zone.id.0,
            zone.humidity_ratio
        );
    }
}
