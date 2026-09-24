//! The add entrance's explicit-id collision check and the evictee exemption.
//!
//! `Dwelling::add_equipment` and `Dwelling::replace_equipment` share one
//! collision check whose exemption for the replace path's evictee is keyed
//! on name: an incumbent whose name equals the replacement target is
//! skipped, because its id leaves the vector with the swap. On the add path
//! there is no evictee — the exemption must exempt nobody — but the
//! `None → ""` fallback exempts any incumbent whose name is the empty
//! string, so an id held by an empty-named incumbent passes the collision
//! check and a second equipment joins the vector on that id. Every
//! `EquipmentId`-keyed map then collapses onto the shared id — the
//! observation-misbinding class the identity contract exists to prevent —
//! with no error in debug or release (the name→id map and the descriptors
//! agree under the collapse, so the drift assert cannot fire either).

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

fn sim_config() -> SimulationConfig {
    SimulationConfig {
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
    }
}

fn dwelling_config(hpxml: &std::path::Path) -> DwellingConfig {
    DwellingConfig {
        hpxml_path: hpxml.to_path_buf(),
        schedule_path: fixture_dir().join("in.schedules.csv"),
        weather_path: project_root()
            .join("tests/fixtures/resstock/2025.1/weather/G1500030_2018.csv"),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: sim_config(),
        overrides: None,
        bldg_id: 100,
        initialization_duration: None,
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    }
}

fn build_dwelling() -> Dwelling {
    let hpxml = fixture_dir().join("home.xml");
    Dwelling::from_config(dwelling_config(&hpxml)).expect("dwelling builds from fixture")
}

/// A minimal in-test equipment whose descriptor id and name are both
/// caller-chosen: the empty name is what engages the exemption, the
/// explicit id is what must be collision-checked against it.
struct ExemptionProbeEquipment {
    descriptor: EquipmentDescriptor,
    telemetry: Telemetry,
    core_output: CoreOutput,
}

impl ExemptionProbeEquipment {
    fn named(name: &str, id: EquipmentId) -> Self {
        Self {
            descriptor: EquipmentDescriptor {
                id,
                name: name.to_string(),
                end_use: EndUse::OTHER,
                equipment_type: Cow::Borrowed("ExemptionProbeEquipment"),
                zone: None,
                fuel: FuelType::Electric,
                stage: ExecutionStage::Independent,
                control_capabilities: ControlCapabilities::empty(),
                core_capabilities: CoreCapabilities::empty(),
                telemetry_fields: vec![TelemetryField {
                    name: "probe".to_string(),
                    unit: "-".to_string(),
                    description: "collision-exemption probe".to_string(),
                }],
                zone_type: None,
            },
            telemetry: Telemetry::with_capacity(1),
            core_output: CoreOutput::default(),
        }
    }
}

impl Equipment for ExemptionProbeEquipment {
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

fn ids(dwelling: &Dwelling) -> Vec<u32> {
    dwelling
        .equipment()
        .iter()
        .map(|eq| eq.descriptor().id.0)
        .collect()
}

/// An explicit id held by an empty-named incumbent must still be rejected:
/// the add path has no evictee, so the replace-path exemption must exempt
/// nobody. An incumbent named "" exempting its id lets a second equipment
/// join the vector on that id — duplicate ids collapse every
/// `EquipmentId`-keyed map (equipment_core's snapshot keeps one entry,
/// last-inserted wins) and observation actors read the wrong equipment's
/// output, silently, with the drift assert unable to fire because the
/// name→id map and the descriptors agree under the collapse.
#[test]
fn add_equipment_rejects_an_explicit_id_held_by_an_empty_named_incumbent() {
    let mut dwelling = build_dwelling();
    let before = ids(&dwelling);
    let counter_id = EquipmentId(before.iter().max().copied().unwrap_or(0) + 1);

    // The empty-named incumbent: unassigned, so the entrance auto-assigns.
    dwelling
        .add_equipment(Box::new(ExemptionProbeEquipment::named("", EquipmentId(0))))
        .expect("the empty-named incumbent itself must be accepted (auto-assigned)");

    // A second equipment with an explicit id equal to the incumbent's: the
    // collision check must reject it — the incumbent's name exempts nothing
    // on the add path.
    let result = dwelling.add_equipment(Box::new(ExemptionProbeEquipment::named(
        "Dual Probe",
        counter_id,
    )));

    let err = match result {
        Err(err) => err,
        Ok(_) => panic!(
            "add_equipment must reject an explicit id held by an incumbent even when \
             that incumbent's name is empty — the add path has no evictee, so the \
             name-keyed exemption must exempt nobody; the add returned Ok and the \
             dwelling now carries two equipment on id {:?}, collapsing every \
             EquipmentId-keyed map (the I-06 observation misbinding)",
            counter_id
        ),
    };
    let msg = err.to_string();
    assert!(
        msg.contains("Dual Probe"),
        "the rejection must name the equipment refused at the door, got: {msg}"
    );
    assert_eq!(
        ids(&dwelling).len(),
        before.len() + 1,
        "only the empty-named incumbent may have joined the vector"
    );
}
