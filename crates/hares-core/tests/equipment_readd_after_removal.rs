//! Re-registration of a removed equipment must not trip the identity guard.
//!
//! `Dwelling::remove_equipment` returns the removed `Box<dyn Equipment>` —
//! still carrying the registration guard's initialized mark that
//! `add_equipment` applied (`dwelling/mod.rs` `eq.mark_initialized()` before
//! the push). Re-adding that box is a *new registration*, not a
//! post-registration mutation: the equipment is in no dwelling at the
//! moment of the write, so the T-0534 guard's invariant ("no identity
//! mutation while registered") is not engaged.
//!
//! But `assign_equipment_identity` calls `Equipment::set_equipment_id`
//! unconditionally — including on the explicit-id path, where `assigned ==
//! requested` and the write is a no-op by construction. For a
//! guard-tracking equipment (EV, Battery, or any type overriding
//! `is_initialized`), `apply_identity_write` sees the stale mark and
//! hard-errors, so the re-add fails with "identity assignment rejected:
//! equipment ... is already initialized" — a loud refusal of a legitimate
//! flow on the public Rust API, and the error text misattributes the cause
//! to the equipment's setter implementation.

use std::borrow::Cow;
use std::path::PathBuf;
use std::time::Duration as StdDuration;

use chrono::{Duration, TimeZone};
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
use hares_equipment::{Equipment, EquipmentConfig};
use hares_types::{
    ControlCapabilities, CoreCapabilities, CoreOutput, EndUse, EnvironmentState,
    EquipmentDescriptor, EquipmentId, ExecutionStage, FuelType, HaresError, OperatingMode,
    PortSlots, Telemetry, TelemetryField,
};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture_dir() -> PathBuf {
    project_root().join("tests/fixtures/resstock/2025.1/bldg0000007")
}

fn build_dwelling() -> Dwelling {
    let config = DwellingConfig {
        hpxml_path: fixture_dir().join("home.xml"),
        schedule_path: fixture_dir().join("in.schedules.csv"),
        weather_path: project_root()
            .join("tests/fixtures/resstock/2025.1/weather/G1500030_2018.csv"),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: SimulationConfig {
            start_time: chrono::FixedOffset::west_opt(10 * 3600)
                .expect("UTC-10 offset is valid")
                .with_ymd_and_hms(2018, 1, 1, 0, 0, 0)
                .unwrap(),
            duration: Duration::hours(1),
            time_res: Duration::seconds(900),
            output_verbosity: 0,
            write_output: false,
            output_path: None,
            output_format: hares_io::OutputFormat::Csv,
            output_chunk_size: 1024,
            setpoint_deadband_c: None,
            master_seed: 0,
            civil_timezone: None,
            site_location: hares_io::SiteLocationOverride::default(),
            retain_batches: false,
            rotation: hares_io::RotationPolicy::None,
        },
        overrides: None,
        bldg_id: 100,
        initialization_duration: None,
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    };
    Dwelling::from_config(config).expect("dwelling builds from fixture")
}

/// A minimal equipment that tracks the registration guard flag, the same
/// shape as the production guard-tracking types (EV, Battery): its
/// `set_equipment_id` delegates to the shared `apply_identity_write`, which
/// rejects the write when the initialized flag is up.
struct GuardedProbe {
    descriptor: EquipmentDescriptor,
    telemetry: Telemetry,
    core_output: CoreOutput,
    initialized: bool,
}

impl GuardedProbe {
    fn new(name: &str) -> Self {
        Self {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(0),
                name: name.to_string(),
                end_use: EndUse::OTHER,
                equipment_type: Cow::Borrowed("GuardedProbe"),
                zone: None,
                fuel: FuelType::Electric,
                stage: ExecutionStage::Independent,
                control_capabilities: ControlCapabilities::empty(),
                core_capabilities: CoreCapabilities::empty(),
                telemetry_fields: vec![TelemetryField {
                    name: "probe".to_string(),
                    unit: "-".to_string(),
                    description: "re-registration probe".to_string(),
                }],
                zone_type: None,
            },
            telemetry: Telemetry::with_capacity(1),
            core_output: CoreOutput::default(),
            initialized: false,
        }
    }
}

impl Equipment for GuardedProbe {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn rename(&mut self, name: String) {
        self.descriptor.name = name;
    }

    fn set_equipment_id(&mut self, id: EquipmentId) -> Result<(), HaresError> {
        hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
    }

    fn is_initialized(&self) -> bool {
        self.initialized
    }

    fn mark_initialized(&mut self) {
        self.initialized = true;
    }

    fn unmark_initialized(&mut self) {
        self.initialized = false;
    }

    fn ports(&self) -> &[hares_types::PortDeclaration] {
        &[]
    }

    fn init(&mut self, _: &EquipmentConfig, _: &EnvironmentState) -> Result<(), HaresError> {
        self.telemetry.insert("probe", 0.0);
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

/// A guard-tracking equipment removed from a dwelling must be re-addable:
/// removal ends its registration, so the initialized mark it still carries
/// is stale and must not block the identity write of the *new*
/// registration. Pre-fix this fails: `assign_equipment_identity` calls
/// `set_equipment_id` even when the equipment's id already equals the
/// collision-checked value (a no-op write), and `apply_identity_write`
/// reads the stale mark as "registered" and refuses.
#[test]
fn readding_a_removed_guard_tracking_equipment_preserves_its_identity() {
    let mut dwelling = build_dwelling();

    dwelling
        .add_equipment(Box::new(GuardedProbe::new("Probe")))
        .expect("first add must succeed");
    let assigned = dwelling
        .equipment()
        .iter()
        .find(|eq| eq.descriptor().name == "Probe")
        .map(|eq| eq.descriptor().id)
        .expect("probe is in the vector");
    assert_ne!(assigned, EquipmentId(0), "the entrance auto-assigns");

    let removed = dwelling
        .remove_equipment("Probe")
        .expect("removal returns the equipment box");

    // Re-registration: the same box, no mutation in between. The entrance
    // must accept it — either preserving its collision-free id or
    // auto-assigning a fresh one — not refuse it on the stale guard mark.
    let result = dwelling.add_equipment(removed);
    assert!(
        result.is_ok(),
        "re-adding a removed (unregistered) equipment must succeed — the \
         registration guard protects registered equipment, and this box is in \
         no dwelling at the moment of the write; got: {:?}",
        result.err()
    );
    let probe = dwelling
        .equipment()
        .iter()
        .find(|eq| eq.descriptor().name == "Probe")
        .expect("probe rejoined the vector");
    assert_ne!(
        probe.descriptor().id,
        EquipmentId(0),
        "the re-added equipment must carry an assigned id"
    );
}
