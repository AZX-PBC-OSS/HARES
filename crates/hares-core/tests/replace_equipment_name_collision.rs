//! The replace entrance's identity contract: name uniqueness.
//!
//! `Dwelling::add_equipment` rejects duplicate names and the assembly loop
//! rejects them; `Dwelling::replace_equipment` documents that it
//! "mirrors add_equipment" for identity (id 0 never survives, explicit-id
//! collisions are rejected, the identity write is verified) — but a
//! replacement whose *name* collides with a surviving equipment (not the
//! evictee) was admitted. Two equipment then share one name, the
//! name→id map collapses to one entry (last wins), and the map-lookup
//! snapshot sites write one equipment's core output under the other's id —
//! the observation-misbinding class the identity contract exists to
//! prevent, reached through a public, Python-reachable entrance.

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

/// A minimal in-test equipment used as the replacement at the replace
/// entrance: unassigned id 0 (so the id machinery auto-assigns and cannot
/// be the thing that rejects the add) and no ports (no zone validation to
/// trip).
struct NameProbeEquipment {
    descriptor: EquipmentDescriptor,
    telemetry: Telemetry,
    core_output: CoreOutput,
}

impl NameProbeEquipment {
    fn named(name: &str) -> Self {
        Self {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(0),
                name: name.to_string(),
                end_use: EndUse::OTHER,
                equipment_type: Cow::Borrowed("NameProbeEquipment"),
                zone: None,
                fuel: FuelType::Electric,
                stage: ExecutionStage::Independent,
                control_capabilities: ControlCapabilities::empty(),
                core_capabilities: CoreCapabilities::empty(),
                telemetry_fields: vec![TelemetryField {
                    name: "probe".to_string(),
                    unit: "-".to_string(),
                    description: "name-collision probe".to_string(),
                }],
                zone_type: None,
            },
            telemetry: Telemetry::with_capacity(1),
            core_output: CoreOutput::default(),
        }
    }
}

impl Equipment for NameProbeEquipment {
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

fn names(dwelling: &Dwelling) -> Vec<String> {
    dwelling
        .equipment()
        .iter()
        .map(|eq| eq.descriptor().name.clone())
        .collect()
}

/// Replacing equipment with a replacement named after a *surviving* piece
/// of equipment must be rejected: two equipment cannot share a name, or
/// the name→id map collapses to one entry (last wins) and the map-lookup
/// snapshot writes one equipment's core output under the other's id — the
/// observation-misbinding class the identity contract exists to prevent.
/// The evictee's own name is exempt: it leaves the vector with the swap.
#[test]
fn replace_equipment_rejects_a_name_colliding_with_a_surviving_equipment() {
    let mut dwelling = build_dwelling();
    let equipment_names = names(&dwelling);
    assert!(
        equipment_names.len() > 1,
        "fixture must assemble multiple equipment for the collision to be possible"
    );
    let evictee = equipment_names[0].clone();
    let survivor = equipment_names[1].clone();

    let result =
        dwelling.replace_equipment(&evictee, Box::new(NameProbeEquipment::named(&survivor)));

    let err = match result {
        Err(err) => err,
        Ok(_) => panic!(
            "replace_equipment must reject a replacement named '{survivor}' while \
             equipment '{survivor}' is still in the dwelling — duplicate names collapse \
             the name→id map and misbind observation; the replace call returned Ok"
        ),
    };
    let msg = err.to_string();
    assert!(
        msg.contains(&survivor),
        "the rejection must name the colliding equipment '{survivor}', got: {msg}"
    );
    // The rejected replacement must not have entered the vector.
    assert_eq!(
        names(&dwelling),
        equipment_names,
        "a rejected replacement must leave the equipment vector unchanged"
    );
}

/// The exempt half of the contract: replacing an equipment with one that
/// keeps the evictee's own name (replace-in-kind) must keep working, and
/// the identity contract must hold through the swap — one entry per name,
/// every id distinct and non-zero, one core-output entry per equipment
/// after a step.
#[test]
fn replace_equipment_with_the_same_name_keeps_the_identity_contract() {
    let mut dwelling = build_dwelling();
    let equipment_names = names(&dwelling);
    let evictee = equipment_names[0].clone();

    dwelling
        .replace_equipment(&evictee, Box::new(NameProbeEquipment::named(&evictee)))
        .expect("same-name replacement must be accepted");

    let after = names(&dwelling);
    assert_eq!(
        after, equipment_names,
        "a same-name replacement must not change the dwelling's name set"
    );

    let ids: Vec<u32> = dwelling
        .equipment()
        .iter()
        .map(|eq| eq.descriptor().id.0)
        .collect();
    assert!(
        ids.iter().all(|id| *id != 0),
        "no equipment may carry the unassigned sentinel after a replace, got {ids:?}"
    );
    let distinct = ids.iter().collect::<std::collections::HashSet<_>>();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "equipment ids must remain unique after a replace, got {ids:?}"
    );

    dwelling.step().expect("post-replace step succeeds");
    let core_entries = dwelling.latest_env().equipment_core.len();
    assert_eq!(
        core_entries,
        dwelling.equipment().len(),
        "equipment_core must hold one entry per equipment after a replace + step"
    );
}
