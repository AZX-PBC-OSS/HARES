//! Regression coverage for equipment identity in the dwelling assembly path.
//!
//! Every equipment constructor defaults its descriptor id when the config
//! carries no explicit id, and nothing in the dwelling assembly path
//! overrides that default, so all equipment instances built from real HPXML
//! share a single `EquipmentId`. Every `EquipmentId`-keyed map then
//! collapses to one entry — the per-step core-output snapshot keeps only
//! whichever equipment was inserted last — and observation-driven actors
//! (the EV driver, the battery management actor) read the wrong equipment's
//! state. For the EV driver that means never seeing its own SOC: its belief
//! decays until every trip is cancelled as "out of range" while the
//! physical pack sits at its charge target.
//!
//! Two invariants pin this from opposite ends:
//!
//! 1. `equipment_core_has_one_entry_per_equipment` — the structural rule:
//!    each equipment instance in a dwelling built through the real assembly
//!    path must have its own entry in the `EquipmentId`-keyed core-output
//!    map, so a lookup keyed by equipment can never return another
//!    equipment's state. This fails for an id collision in any equipment
//!    type, not just the one whose symptom was reported.
//! 2. `nightly_strategy_keeps_ev_charged_over_long_horizon` — the
//!    observable behaviour that collapses without it: over a 45-day horizon
//!    a Nightly charging strategy must keep the pack topped up and must
//!    never cancel a trip. The horizon matters: observation belief drift is
//!    slow (roughly 0.13 SOC per driving day), so short-horizon tests pass
//!    while the long run still fails.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::actors::BatteryManagementActor;
use hares_core::{Dwelling, DwellingConfig, SimStatus, SimulationConfig, SimulationEngine};
use hares_equipment::config::ConfigValue;
use hares_equipment::{Equipment, EquipmentConfig, EquipmentRegistry};
use hares_io::OutputFormat;
use hares_types::{
    BmsMode, ControlCapabilities, CoreCapabilities, CoreOutput, ElectricPower, EndUse,
    EnvironmentState, EquipmentDescriptor, EquipmentId, ExecutionStage, FuelType, GridExportRule,
    HaresError, OperatingMode, PortSlots, Soc, Telemetry, TelemetryField,
};

mod common;

/// 900 s timestep → 96 rows per simulated day in the output CSV.
const STEPS_PER_DAY: usize = 96;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// ResStock bldg0000007 fixture directory.
fn fixture_dir() -> PathBuf {
    project_root().join("tests/fixtures/resstock/2025.1/bldg0000007")
}

/// Injects a 75 kWh / 11.5 kW Level-2 EV into the fixture's `<Systems>`
/// element, matching the reported scenario.
fn inject_ev(hpxml: &str) -> String {
    let ev = concat!(
        "<ElectricVehicles>",
        "<ElectricVehicle>",
        "<SystemIdentifier id=\"EV\"/>",
        "<ChargingLevel>Level 2</ChargingLevel>",
        "<MaxChargingPower>11.5</MaxChargingPower>",
        "<BatteryCapacity><Value>75</Value><Units>kWh</Units></BatteryCapacity>",
        "</ElectricVehicle>",
        "</ElectricVehicles>"
    );
    let marker = "</Systems>";
    assert!(hpxml.contains(marker), "fixture must contain </Systems>");
    hpxml.replacen(marker, &format!("{ev}{marker}"), 1)
}

/// Writes the EV-injected fixture HPXML into `dir` and returns its path.
fn write_fixture_hpxml(dir: &Path) -> PathBuf {
    let src = std::fs::read_to_string(fixture_dir().join("home.xml")).expect("fixture home.xml");
    let path = dir.join("bldg0000007_ev.xml");
    std::fs::write(&path, inject_ev(&src)).expect("write injected HPXML");
    path
}

fn sim_config(duration: Duration, output_path: Option<PathBuf>, verbosity: u8) -> SimulationConfig {
    SimulationConfig {
        start_time: FixedOffset::west_opt(10 * 3600)
            .expect("UTC-10 offset is valid")
            .with_ymd_and_hms(2018, 1, 1, 0, 0, 0)
            .unwrap(),
        duration,
        time_res: Duration::seconds(900),
        output_verbosity: verbosity,
        write_output: output_path.is_some(),
        output_path,
        output_format: OutputFormat::Csv,
        output_chunk_size: 1024,
        setpoint_deadband_c: None,
        master_seed: 0,
        civil_timezone: None,
        site_location: hares_io::SiteLocationOverride::default(),
        retain_batches: false,
        rotation: hares_io::RotationPolicy::None,
    }
}

fn dwelling_config(
    hpxml: &Path,
    sim: SimulationConfig,
    overrides: Option<serde_json::Value>,
) -> DwellingConfig {
    DwellingConfig {
        hpxml_path: hpxml.to_path_buf(),
        schedule_path: fixture_dir().join("in.schedules.csv"),
        weather_path: project_root()
            .join("tests/fixtures/resstock/2025.1/weather/G1500030_2018.csv"),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: sim,
        overrides,
        bldg_id: 100,
        initialization_duration: None,
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    }
}

#[test]
fn equipment_core_has_one_entry_per_equipment() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let hpxml = write_fixture_hpxml(tmp.path());
    let config = dwelling_config(&hpxml, sim_config(Duration::hours(1), None, 0), None);

    let mut dwelling = Dwelling::from_config(config).expect("dwelling builds from fixture");
    // One step so the end-of-step snapshot populates `equipment_core`.
    dwelling.step().expect("first timestep succeeds");

    let equipment_count = dwelling.equipment().len();
    assert!(
        equipment_count > 1,
        "fixture must assemble multiple equipment for this invariant to bite, \
         got {equipment_count}"
    );

    let core_entries = dwelling.latest_env().equipment_core.len();
    assert_eq!(
        core_entries, equipment_count,
        "equipment_core must hold one entry per equipment instance so \
         EquipmentId-keyed lookups address the right equipment; \
         {equipment_count} equipment collapsed into {core_entries} entries — \
         equipment identity collides in the dwelling assembly path"
    );
}

#[test]
fn nightly_strategy_keeps_ev_charged_over_long_horizon() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let hpxml = write_fixture_hpxml(tmp.path());
    let output_path = tmp.path().join("ev_nightly_45day.csv");

    // Nightly charging strategy: charge to 90% between 22:00 and 06:00.
    let nightly = serde_json::json!({"Nightly": {
        "off_peak_start_hour": 22.0,
        "off_peak_end_hour": 6.0,
        "target_soc": 0.90,
    }});
    let overrides = serde_json::json!({
        "EV": { "charging_strategy": nightly.to_string() }
    });

    let config = dwelling_config(
        &hpxml,
        sim_config(Duration::days(45), Some(output_path.clone()), 5),
        Some(overrides),
    );
    let result = SimulationEngine::new()
        .run(config)
        .expect("engine.run should succeed");
    assert!(
        !matches!(result.status, SimStatus::Failed(_)),
        "simulation failed: {:?}",
        result.status
    );

    let (charge_days, cancelled_max) = summarize_ev_run(&output_path);

    // With working SOC observation the pack is topped to 0.90 (67.5 kWh of
    // 75 kWh) every night against a ~9.8 kWh daily trip, so a real charging
    // session lands on the vast majority of the 45 days.
    assert!(
        charge_days >= 30,
        "Nightly(22:00-06:00, target 0.90) must top the pack up on most of \
         45 days, got {charge_days} charge-days"
    );
    // A nightly-topped pack never runs out of range: no trip may ever be
    // cancelled.
    assert_eq!(
        cancelled_max, 0.0,
        "no trip should be cancelled with a nightly-topped 75 kWh pack, \
         drive_cancelled peaked at {cancelled_max}"
    );
}

/// Scans the run's CSV output: the number of days on which the EV pack's
/// stored energy actually rose, and the peak of the driver's drive-cancelled
/// counter — the shared charging-isolating metric (see
/// `common::ev_charge_days_and_peak_cancelled`; the pre-fix local copy
/// counted any day with > 1 kWh of positive port power, which a heater-only
/// day also satisfies).
fn summarize_ev_run(csv_path: &Path) -> (usize, f64) {
    common::ev_charge_days_and_peak_cancelled(csv_path, STEPS_PER_DAY)
}

// ===========================================================================
// Entrance-uniform identity coverage
// ===========================================================================
//
// The tests above pin the assembly-path rule and the reported symptom. The
// tests below pin the identity contract at every entrance equipment can
// enter a dwelling, the validation arms, determinism, the checkpoint
// round-trip, the second misbound observation actor (the BMS), and the
// range-anxiety override's raise side through real assembly.

/// A second ResStock fixture with a different equipment mix, for the
/// behavioral-breadth arm.
///
/// Constraint: must be a warm-climate fixture. The EV model's cold-pack
/// charge derate (`linear_temp_derate` below `min_charge_temp_c`)
/// legitimately blocks charging through a cold winter — verified on
/// bldg0000002 (G0900090, January mean −3.4 °C): the pack pins at its
/// post-drain floor with zero charging while the derate is 0, which is the
/// thermal model working, not an identity failure. The identity mechanism
/// under test is fixture-independent (the structural gates prove it on any
/// assembly); this behavioral arm needs the charge path physically open.
fn second_fixture_dir() -> PathBuf {
    project_root().join("tests/fixtures/resstock/2025.1/bldg0000006")
}

/// Resolve the fixture's weather file: the HPXML `<Name>` starting with `G`
/// is the weather station id; fall back to any file in the weather dir.
fn weather_for(bldg_dir: &Path) -> PathBuf {
    let xml = std::fs::read_to_string(bldg_dir.join("home.xml")).expect("fixture home.xml");
    let fips = xml
        .lines()
        .find_map(|line| {
            let s = line.find("<Name>")?;
            let inner = &line[s + 6..];
            let e = inner.find("</Name>")?;
            let n = inner[..e].trim();
            (n.starts_with('G')).then(|| n.to_string())
        })
        .unwrap_or_default();
    let wdir = project_root().join("tests/fixtures/resstock/2025.1/weather");
    [format!("{fips}_2018.csv"), format!("{fips}.csv")]
        .iter()
        .map(|n| wdir.join(n))
        .find(|p| p.exists())
        .unwrap_or_else(|| {
            wdir.read_dir()
                .expect("read weather dir")
                .filter_map(|e| e.ok())
                .find(|e| e.path().is_file())
                .map(|e| e.path())
                .expect("no weather file found")
        })
}

/// Build a fixture dwelling through the real assembly path, optionally with
/// the EV injected and user overrides applied.
fn build_fixture_dwelling(
    bldg_dir: &Path,
    with_ev: bool,
    overrides: Option<serde_json::Value>,
) -> Dwelling {
    let (config, _tmp) = fixture_dwelling_config(bldg_dir, with_ev, overrides);
    Dwelling::from_config(config).expect("dwelling builds from fixture")
}

/// Build a fixture dwelling config through the real assembly path,
/// optionally with the EV injected and user overrides applied. The
/// `TempDir` holding the injected HPXML is returned with the config — the
/// caller must keep it alive until the dwelling is built.
fn fixture_dwelling_config(
    bldg_dir: &Path,
    with_ev: bool,
    overrides: Option<serde_json::Value>,
) -> (DwellingConfig, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("temp dir");
    let hpxml_path = if with_ev {
        let src = std::fs::read_to_string(bldg_dir.join("home.xml")).expect("home.xml");
        let path = tmp.path().join("fixture_ev.xml");
        std::fs::write(&path, inject_ev(&src)).expect("write injected HPXML");
        path
    } else {
        bldg_dir.join("home.xml")
    };
    (
        DwellingConfig {
            hpxml_path,
            schedule_path: bldg_dir.join("in.schedules.csv"),
            weather_path: weather_for(bldg_dir),
            defaults_path: Some(project_root().join("defaults")),
            sim_config: sim_config(Duration::hours(1), None, 0),
            overrides,
            bldg_id: 100,
            initialization_duration: None,
            resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
            patches: None,
        },
        tmp,
    )
}

/// Minimal equipment stub for the `add_equipment` entrance: an electric
/// plug-style load whose descriptor id the test controls. The identity
/// write delegates to the shared guard-checked helper, except when
/// `sabotage_setter` is set — the arm that proves `add_equipment` enforces
/// the write landing instead of assuming it.
struct IdentityProbeEquipment {
    descriptor: EquipmentDescriptor,
    telemetry: Telemetry,
    core_output: CoreOutput,
    ports: Vec<hares_types::PortDeclaration>,
    /// When set, `step` writes a real thermal port contribution for this
    /// zone — the behavioral probe for port-slot coverage (an uncovered
    /// zone fails the step with "undeclared thermal zone").
    thermal_port_zone: Option<hares_types::ZoneId>,
    /// A constant electric load [kW]: claimed in the core output (with the
    /// ELECTRIC capability declared, the core-contract validator's
    /// requirement) and deposited as the matching port contribution in
    /// `step` — for tests that need a live end-use aggregate contribution.
    electric_load_kw: f64,
    sabotage_setter: bool,
}

impl IdentityProbeEquipment {
    fn new(name: &str, id: u32) -> Self {
        Self {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(id),
                name: name.to_string(),
                end_use: EndUse::OTHER,
                equipment_type: Cow::Borrowed("IdentityProbeEquipment"),
                zone: None,
                fuel: FuelType::Electric,
                stage: ExecutionStage::Independent,
                control_capabilities: ControlCapabilities::empty(),
                core_capabilities: CoreCapabilities::empty(),
                telemetry_fields: vec![TelemetryField {
                    name: "probe".to_string(),
                    unit: "-".to_string(),
                    description: "identity probe".to_string(),
                }],
                zone_type: None,
            },
            telemetry: Telemetry::with_capacity(1),
            core_output: CoreOutput::default(),
            ports: Vec::new(),
            thermal_port_zone: None,
            electric_load_kw: 0.0,
            sabotage_setter: false,
        }
    }

    fn with_sabotaged_setter(mut self) -> Self {
        self.sabotage_setter = true;
        self
    }

    /// Declare (and step-write) a thermal port for `zone`.
    fn with_thermal_port(mut self, zone: hares_types::ZoneId) -> Self {
        self.ports.push(hares_types::PortDeclaration::thermal(zone));
        self.thermal_port_zone = Some(zone);
        self
    }

    /// A constant electric load: claimed in the core output and deposited
    /// as the matching electrical port contribution each step.
    fn with_electric_load(mut self, kw: f64) -> Self {
        self.electric_load_kw = kw;
        self.descriptor.core_capabilities = CoreCapabilities::ELECTRIC;
        self.core_output.flows.electric_kw = Some(ElectricPower::Consumption(kw));
        self
    }

    /// Pose as another end use: the descriptor's `end_use` drives
    /// end-use-keyed dwelling machinery (invariant checks, dispatch
    /// fan-out, output aggregation) — this builder lets a test probe that
    /// machinery without a real equipment of that type.
    fn with_end_use(mut self, end_use: EndUse) -> Self {
        self.descriptor.end_use = end_use;
        self
    }

    /// Publish a core-output SOC with the matching HAS_SOC capability,
    /// mirroring the real storage equipment the probe stands in for (every
    /// real Battery/EV publishes SOC at every step, and the dwelling's
    /// soc_bounds monitor fails loudly on an absent SOC for that
    /// population) — so a probe isolating a different gate stays green
    /// there.
    fn with_published_soc(mut self, soc: f64) -> Self {
        self.descriptor.core_capabilities |= CoreCapabilities::HAS_SOC;
        self.core_output.state.soc =
            Some(Soc::try_from(soc).expect("probe SOC must be within [0, 1]"));
        self
    }
}

impl Equipment for IdentityProbeEquipment {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn rename(&mut self, name: String) {
        self.descriptor.name = name;
    }

    fn set_equipment_id(&mut self, id: EquipmentId) -> Result<(), HaresError> {
        if self.sabotage_setter {
            // Deliberately broken body: writes nothing, reports success.
            // `add_equipment`'s postcondition must reject the registration.
            let _ = id;
            return Ok(());
        }
        hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
    }

    fn ports(&self) -> &[hares_types::PortDeclaration] {
        &self.ports
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
        ports: &mut PortSlots,
    ) -> Result<(), HaresError> {
        if let Some(zone) = self.thermal_port_zone {
            ports.accumulate(&hares_types::PortContribution::Thermal {
                zone,
                sensible_gain_w: 100.0,
                radiant_gain_w: 0.0,
                latent_gain_w: 0.0,
                category: hares_types::ThermalCategory::HvacHeating,
            })?;
        }
        if self.electric_load_kw > 0.0 {
            ports.accumulate(&hares_types::PortContribution::Electrical {
                active_power_w: self.electric_load_kw * 1000.0,
                reactive_power_kvar: 0.0,
            })?;
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

    fn load_state(&mut self, _: &[u8]) -> Result<(), HaresError> {
        Ok(())
    }

    fn apply_signal(&mut self, _: &hares_types::ControlSignal) -> Result<(), HaresError> {
        Ok(())
    }
}

/// The observable name→id mapping of a dwelling's equipment vector.
fn id_map(dwelling: &Dwelling) -> BTreeMap<String, u32> {
    dwelling
        .equipment()
        .iter()
        .map(|eq| (eq.descriptor().name.clone(), eq.descriptor().id.0))
        .collect()
}

/// Every equipment in a dwelling built through the real assembly path has a
/// distinct, non-zero id — the direct pin of the root-cause fix at entrance
/// 1 (assembly), complementing the `equipment_core` cardinality gate above.
#[test]
fn assembly_equipment_ids_are_distinct_and_nonzero() {
    let dwelling = build_fixture_dwelling(&fixture_dir(), true, None);
    let map = id_map(&dwelling);
    assert!(
        map.len() > 1,
        "fixture must assemble multiple equipment, got {}",
        map.len()
    );
    assert!(
        map.values().all(|id| *id != 0),
        "no equipment may carry the unassigned sentinel after assembly, got {map:?}"
    );
    let mut ids: Vec<u32> = map.values().copied().collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(
        ids.len(),
        map.len(),
        "every assembly-assigned id must be distinct, got {map:?}"
    );
}

/// Determinism: the same config built twice yields an identical name→id
/// mapping. The mapping is asserted — never contiguity, which the design
/// does not promise (dropped non-critical equipment leaves gaps).
#[test]
fn same_config_builds_identical_name_to_id_mapping() {
    let first = id_map(&build_fixture_dwelling(&fixture_dir(), true, None));
    let second = id_map(&build_fixture_dwelling(&fixture_dir(), true, None));
    assert_eq!(
        first, second,
        "the assembly id assignment must be deterministic from spec order"
    );
}

/// Entrance 2 auto-assignment: equipment joining through `add_equipment`
/// with the unassigned sentinel receives the dwelling's next never-reused
/// id; an explicit unique id survives, collision-checked; the counter
/// sequence holds across explicit-then-auto adds; and removal never
/// reissues an id.
#[test]
fn add_equipment_assigns_never_reused_ids_across_both_entrances() {
    let mut dwelling = build_fixture_dwelling(&fixture_dir(), true, None);
    let assembly_max = dwelling
        .equipment()
        .iter()
        .map(|eq| eq.descriptor().id.0)
        .max()
        .expect("fixture has equipment");

    // Explicit id past the assembly range: accepted, counter advances past it.
    dwelling
        .add_equipment(Box::new(IdentityProbeEquipment::new(
            "ProbeExplicit",
            assembly_max + 1,
        )))
        .expect("explicit non-colliding id must be accepted");
    assert_eq!(
        id_map(&dwelling)["ProbeExplicit"],
        assembly_max + 1,
        "an explicit unique id must survive registration"
    );

    // The next id-less add takes the counter's value — past the explicit id,
    // never colliding with anything already in the vector.
    dwelling
        .add_equipment(Box::new(IdentityProbeEquipment::new("ProbeAuto1", 0)))
        .expect("unassigned equipment must be auto-assigned");
    assert_eq!(
        id_map(&dwelling)["ProbeAuto1"],
        assembly_max + 2,
        "the counter must advance past the explicit id: next auto id = {}",
        assembly_max + 2
    );

    // Removal frees no ids: the next id-less add never reissues a removed id.
    dwelling
        .remove_equipment("ProbeAuto1")
        .expect("remove probe");
    dwelling
        .add_equipment(Box::new(IdentityProbeEquipment::new("ProbeAuto2", 0)))
        .expect("unassigned equipment must be auto-assigned");
    assert_eq!(
        id_map(&dwelling)["ProbeAuto2"],
        assembly_max + 3,
        "the id counter is monotonic — a removed id must never be reissued"
    );

    // The whole vector — both entrances — stays unique and non-zero.
    let map = id_map(&dwelling);
    assert!(map.values().all(|id| *id != 0), "got {map:?}");
    let mut ids: Vec<u32> = map.values().copied().collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), map.len(), "ids must stay unique, got {map:?}");
}

/// An explicit id colliding with equipment already in the dwelling is
/// rejected, naming both — never silently collapsing the maps.
#[test]
fn add_equipment_rejects_explicit_id_collision_with_existing_equipment() {
    let mut dwelling = build_fixture_dwelling(&fixture_dir(), true, None);
    let (existing_name, existing_id) = dwelling
        .equipment()
        .iter()
        .map(|eq| (eq.descriptor().name.clone(), eq.descriptor().id.0))
        .next()
        .expect("fixture has equipment");

    let err = dwelling
        .add_equipment(Box::new(IdentityProbeEquipment::new(
            "ProbeCollision",
            existing_id,
        )))
        .expect_err("a colliding explicit id must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("ProbeCollision") && msg.contains(&existing_name),
        "the collision error must name both equipment, got: {msg}"
    );
    assert!(
        !dwelling
            .equipment()
            .iter()
            .any(|eq| eq.descriptor().name == "ProbeCollision"),
        "the rejected equipment must not join the vector"
    );
}

/// The entrance-2 postcondition: an equipment whose `set_equipment_id`
/// body writes nothing cannot join the vector — the write's landing is
/// verified, not assumed. This is the arm that would catch a silently
/// no-op `Equipment` implementation.
#[test]
fn add_equipment_rejects_equipment_whose_identity_write_does_not_land() {
    let mut dwelling = build_fixture_dwelling(&fixture_dir(), true, None);
    let err = dwelling
        .add_equipment(Box::new(
            IdentityProbeEquipment::new("ProbeSabotage", 0).with_sabotaged_setter(),
        ))
        .expect_err("a no-op identity write must fail registration");
    let msg = err.to_string();
    assert!(
        msg.contains("ProbeSabotage"),
        "the error must name the equipment, got: {msg}"
    );
    assert!(
        msg.contains("set_equipment_id"),
        "the error must point at the setter implementation, got: {msg}"
    );
    assert!(
        !dwelling
            .equipment()
            .iter()
            .any(|eq| eq.descriptor().name == "ProbeSabotage"),
        "the rejected equipment must not join the vector"
    );
}

/// A hand-built Dehumidifier spec (all-optional typed config, so it inits
/// cleanly) carrying an explicit id through the blueprint entrance.
fn dehumidifier_spec(instance: &str, equipment_id: u32) -> hares_io::EquipmentSpec {
    let config: hares_equipment::DehumidifierConfig =
        serde_json::from_value(serde_json::json!({ "equipment_id": equipment_id }))
            .expect("minimal DehumidifierConfig");
    let typed =
        EquipmentConfig::from_typed(instance.to_string(), "Dehumidifier".to_string(), config)
            .expect("typed config");
    hares_io::EquipmentSpec {
        name: "Dehumidifier".to_string(),
        instance_name: Some(instance.to_string()),
        fuel_type: FuelType::Electric,
        parameters: serde_json::Map::new(),
        zip_params: None,
        typed_config: Some(typed),
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
    }
}

/// Entrance 1 rejects duplicate explicit ids, naming both equipment — the
/// validation the pre-fix assembly never performed (it checked name
/// uniqueness only).
#[test]
fn assembly_rejects_duplicate_explicit_equipment_ids() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let hpxml = write_fixture_hpxml(tmp.path());
    let config = dwelling_config(&hpxml, sim_config(Duration::hours(1), None, 0), None);
    let mut blueprint =
        hares_core::dwelling::DwellingBlueprint::from_config(config).expect("blueprint");
    blueprint
        .add_equipment_spec(dehumidifier_spec("Humid Fighter A", 5000))
        .expect("add spec");
    blueprint
        .add_equipment_spec(dehumidifier_spec("Humid Fighter B", 5000))
        .expect("add spec");
    let err = blueprint
        .build()
        .err()
        .expect("duplicate explicit ids must fail the build");
    let msg = err.to_string();
    assert!(
        msg.contains("Dehumidifier #1") && msg.contains("Dehumidifier #2"),
        "the duplicate-id error must name both equipment (the assembly name \
         pass numbers same-class instances), got: {msg}"
    );
    assert!(
        msg.contains("duplicate equipment id"),
        "the error must name the violation, got: {msg}"
    );
}

/// An explicit `equipment_id: 0` means "unassigned" and never survives into
/// a dwelling's equipment vector — assembly rejects it loudly.
#[test]
fn assembly_rejects_unassigned_equipment_id_zero() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let hpxml = write_fixture_hpxml(tmp.path());
    let config = dwelling_config(&hpxml, sim_config(Duration::hours(1), None, 0), None);
    let mut blueprint =
        hares_core::dwelling::DwellingBlueprint::from_config(config).expect("blueprint");
    blueprint
        .add_equipment_spec(dehumidifier_spec("Humid Fighter Zero", 0))
        .expect("add spec");
    let err = blueprint
        .build()
        .err()
        .expect("an explicit unassigned id must fail the build");
    let msg = err.to_string();
    assert!(
        msg.contains("unassigned"),
        "the error must name the unassigned sentinel, got: {msg}"
    );
    // The four-way diagnostic: an EXPLICIT 0 is its own case — the error
    // must say the channel delivered the sentinel and tell the user to
    // remove the field, not claim the channel delivered nothing (which
    // would misdirect toward hunting for a missing write instead of
    // deleting the explicit zero).
    assert!(
        msg.contains("sentinel") && msg.contains("remove the field"),
        "an explicit equipment_id 0 must be diagnosed as the delivered \
         sentinel (not as a missing delivery), got: {msg}"
    );
    assert!(
        !msg.contains("did not deliver one"),
        "the explicit-zero diagnosis must not use the absent-case text, \
         got: {msg}"
    );
}

/// `equipment_id` is reserved against user overrides: appearing in any of
/// the three override sources (the `all` and `*` wildcards, or a named
/// equipment override) is a hard, typed error naming the source.
#[test]
fn equipment_id_in_overrides_is_rejected_from_every_override_source() {
    let sources = [
        ("all", serde_json::json!({ "all": { "equipment_id": 3 } })),
        ("*", serde_json::json!({ "*": { "equipment_id": 3 } })),
        ("EV", serde_json::json!({ "EV": { "equipment_id": 3 } })),
    ];
    for (source, overrides) in sources {
        let (config, _tmp) = fixture_dwelling_config(&fixture_dir(), true, Some(overrides));
        let err = Dwelling::from_config(config)
            .err()
            .expect("the reserved-key override must fail the build");
        let msg = err.to_string();
        assert!(
            msg.contains("equipment_id"),
            "'{source}' override: the error must name the reserved field, got: {msg}"
        );
        assert!(
            msg.contains(source),
            "'{source}' override: the error must name the override source, got: {msg}"
        );
    }
}

/// Checkpoint round-trip: save → rebuild from the same config → restore →
/// step — the id-keyed core-output map still addresses every equipment
/// individually and the name→id mapping is unchanged. The ids are
/// deterministic from spec order, so the rebuilt dwelling re-derives the
/// same ids the checkpointed state was written under.
#[test]
fn checkpoint_round_trip_preserves_equipment_identity() {
    let mut dwelling = build_fixture_dwelling(&fixture_dir(), true, None);
    for _ in 0..2 {
        dwelling.step().expect("pre-save steps succeed");
    }
    let pre_save_map = id_map(&dwelling);
    let checkpoint = dwelling.save_checkpoint().expect("save checkpoint");

    let mut restored = build_fixture_dwelling(&fixture_dir(), true, None);
    restored.load_checkpoint(checkpoint).expect("restore");
    assert_eq!(
        id_map(&restored),
        pre_save_map,
        "the rebuilt dwelling must re-derive the same name→id mapping the checkpoint was written under"
    );
    restored.step().expect("first post-restore step succeeds");
    assert_eq!(
        restored.latest_env().equipment_core.len(),
        restored.equipment().len(),
        "after the round-trip, equipment_core must still hold one entry per equipment — ids and actor bindings survive the restore"
    );
}

/// Injects a battery (13.5 kWh / 5 kW, the resolve_der reference shape) into
/// the fixture's `<Systems>` element so the dwelling auto-registers its
/// BatteryManagementActor.
fn inject_battery(hpxml: &str) -> String {
    let battery = concat!(
        "<Batteries>",
        "<Battery>",
        "<SystemIdentifier id=\"Battery\"/>",
        "<RatedPowerOutput>5000</RatedPowerOutput>",
        "<NominalCapacity><Units>kWh</Units><Value>13.5</Value></NominalCapacity>",
        "</Battery>",
        "</Batteries>"
    );
    let marker = "</Systems>";
    assert!(hpxml.contains(marker), "fixture must contain </Systems>");
    hpxml.replacen(marker, &format!("{battery}{marker}"), 1)
}

/// The RCA's second misbound observation actor: the auto-registered BMS
/// reads its battery's SOC through the id-keyed `equipment_core` map. Under
/// the identity collapse it read whichever equipment was inserted last (no
/// SOC) and its `soc` telemetry channel never left the 0.0 init sentinel
/// (`idle:no_soc`, never dispatching). Through real assembly with a battery
/// injected, the channel must report the battery's real SOC.
#[test]
fn bms_actor_observes_its_own_battery_through_real_assembly() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let src = std::fs::read_to_string(fixture_dir().join("home.xml")).expect("home.xml");
    let hpxml = tmp.path().join("fixture_battery.xml");
    std::fs::write(&hpxml, inject_battery(&src)).expect("write battery-injected HPXML");

    let output_path = tmp.path().join("battery_observation.csv");
    // Self-consumption BMS mode (the battery's default is Manual, which
    // seeds no actor): the override reaches the typed config through the
    // named-equipment override channel, the same path the Python API uses.
    let bms_mode = serde_json::json!({"SelfConsumption": {
        "min_soc": 0.1,
        "max_soc": 1.0,
        "solar_only_charging": false,
        "surplus_deadband_kw": 0.0,
    }});
    let overrides = serde_json::json!({
        "Battery": { "bms_mode": bms_mode.to_string() }
    });
    let config = DwellingConfig {
        hpxml_path: hpxml,
        schedule_path: fixture_dir().join("in.schedules.csv"),
        weather_path: weather_for(&fixture_dir()),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: sim_config(Duration::days(2), Some(output_path.clone()), 5),
        overrides: Some(overrides),
        bldg_id: 100,
        initialization_duration: None,
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    };
    let result = SimulationEngine::new().run(config).expect("engine.run");
    assert!(
        !matches!(result.status, SimStatus::Failed(_)),
        "simulation failed: {:?}",
        result.status
    );

    let contents = std::fs::read_to_string(&output_path).expect("read output CSV");
    let header: Vec<&str> = contents
        .lines()
        .next()
        .expect("header")
        .split(',')
        .collect();
    let soc_col = header
        .iter()
        .position(|c| *c == "actor:BatteryManagementActor:Battery:soc")
        .unwrap_or_else(|| panic!("BMS soc column missing; header: {header:?}"));
    let max_observed_soc = contents
        .lines()
        .skip(1)
        .filter_map(|l| l.split(',').nth(soc_col))
        .filter_map(|v| v.trim().parse::<f64>().ok())
        .fold(0.0_f64, f64::max);
    assert!(
        max_observed_soc > 0.1,
        "the BMS must observe its own battery's SOC (default initial SOC 0.5) — \
         a channel pinned at the 0.0 init sentinel means the BMS is reading the \
         wrong equipment's core output again, got max soc = {max_observed_soc}"
    );
}

/// The range-anxiety override's raise side through real assembly: with a
/// genuinely low pack and a strategy whose own target sits *below* the
/// anxiety band (Immediate 0.1), the override must raise the dispatched
/// target to the band before each departure — observable as zero cancelled
/// trips and a pack peak well above the configured 0.1 target. Without the
/// raise, the pack would sit at 0.1 and every ~9.8 kWh trip would cancel.
#[test]
fn low_target_strategy_pack_is_raised_to_the_anxiety_band_before_departure() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let hpxml = write_fixture_hpxml(tmp.path());
    let output_path = tmp.path().join("ev_low_target_14day.csv");

    let low_target = serde_json::json!({"Immediate": { "target_soc": 0.1 }});
    let overrides = serde_json::json!({
        "EV": { "charging_strategy": low_target.to_string() }
    });
    let config = dwelling_config(
        &hpxml,
        sim_config(Duration::days(14), Some(output_path.clone()), 5),
        Some(overrides),
    );
    let result = SimulationEngine::new()
        .run(config)
        .expect("engine.run should succeed");
    assert!(
        !matches!(result.status, SimStatus::Failed(_)),
        "simulation failed: {:?}",
        result.status
    );

    let contents = std::fs::read_to_string(&output_path).expect("read output CSV");
    let header: Vec<&str> = contents
        .lines()
        .next()
        .expect("header")
        .split(',')
        .collect();
    let soc_col = header
        .iter()
        .position(|c| *c == "EV SOC (-)")
        .unwrap_or_else(|| panic!("EV SOC column missing; header: {header:?}"));
    let cancelled_col = header
        .iter()
        .position(|c| *c == "actor:EvDriver:EV:drive_cancelled")
        .expect("drive_cancelled column");
    let mut max_soc = 0.0_f64;
    let mut cancelled_max = 0.0_f64;
    for line in contents.lines().skip(1) {
        let fields: Vec<&str> = line.split(',').collect();
        if let Ok(soc) = fields[soc_col].trim().parse::<f64>() {
            max_soc = max_soc.max(soc);
        }
        if let Ok(c) = fields[cancelled_col].trim().parse::<f64>() {
            cancelled_max = cancelled_max.max(c);
        }
    }
    assert_eq!(
        cancelled_max, 0.0,
        "the anxiety band (trip + reserve) covers every trip by construction — \
         no departure may be cancelled when the override raises the pack to it"
    );
    assert!(
        max_soc > 0.15,
        "the override must raise the pack above the configured 0.1 target \
         toward the anxiety band (~0.22); peak SOC was {max_soc}"
    );
}

/// Behavioral breadth: the Nightly contract holds on a second fixture with
/// a different equipment mix — the mechanism is fixture-independent.
#[test]
fn nightly_strategy_keeps_ev_charged_on_a_second_fixture() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let src = std::fs::read_to_string(second_fixture_dir().join("home.xml")).expect("home.xml");
    let hpxml = tmp.path().join("second_fixture_ev.xml");
    std::fs::write(&hpxml, inject_ev(&src)).expect("write injected HPXML");
    let output_path = tmp.path().join("ev_nightly_45day_second_fixture.csv");

    let nightly = serde_json::json!({"Nightly": {
        "off_peak_start_hour": 22.0,
        "off_peak_end_hour": 6.0,
        "target_soc": 0.90,
    }});
    let overrides = serde_json::json!({
        "EV": { "charging_strategy": nightly.to_string() }
    });
    let config = DwellingConfig {
        hpxml_path: hpxml,
        schedule_path: second_fixture_dir().join("in.schedules.csv"),
        weather_path: weather_for(&second_fixture_dir()),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: sim_config(Duration::days(45), Some(output_path.clone()), 5),
        overrides: Some(overrides),
        bldg_id: 100,
        initialization_duration: None,
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    };
    let result = SimulationEngine::new()
        .run(config)
        .expect("engine.run should succeed");
    assert!(
        !matches!(result.status, SimStatus::Failed(_)),
        "simulation failed: {:?}",
        result.status
    );

    let (charge_days, cancelled_max) = summarize_ev_run(&output_path);
    assert!(
        charge_days >= 30,
        "Nightly(22:00-06:00, target 0.90) must top the pack up on most of 45 \
         days on the second fixture too, got {charge_days} charge-days"
    );
    assert_eq!(
        cancelled_max, 0.0,
        "no trip should be cancelled with a nightly-topped pack on the second \
         fixture, drive_cancelled peaked at {cancelled_max}"
    );
}

/// Checkpoint restore is identity-keyed, not positional: a spec reorder
/// between save and restore changes the order-derived equipment ids, and
/// the restore must reject the mismatch with a typed error naming the
/// equipment and both ids — never silently load one equipment's state into
/// another. Built through real paths on both sides (blueprint → assembly →
/// step → save; reordered blueprint → assembly → restore).
#[test]
fn checkpoint_restore_rejects_spec_reorder_with_named_error() {
    let (config, _tmp) = fixture_dwelling_config(&fixture_dir(), true, None);

    let mut dwelling_a = hares_core::dwelling::DwellingBlueprint::from_config(config.clone())
        .expect("blueprint A")
        .build()
        .expect("build dwelling A");
    for _ in 0..2 {
        dwelling_a.step().expect("pre-save steps succeed");
    }
    let checkpoint = dwelling_a.save_checkpoint().expect("save checkpoint");

    // Same config, same equipment set, rotated spec order: the id-injection
    // pass assigns ids from spec order, so every equipment's id shifts.
    let mut blueprint_b =
        hares_core::dwelling::DwellingBlueprint::from_config(config).expect("blueprint B");
    blueprint_b.equipment_specs.rotate_left(1);
    let mut dwelling_b = blueprint_b.build().expect("build dwelling B");

    let err = dwelling_b
        .load_checkpoint(checkpoint)
        .expect_err("a reordered equipment set must fail the restore");
    let msg = err.to_string();
    assert!(
        msg.contains("equipment id mismatch"),
        "the rejection must name the id-mismatch violation, got: {msg}"
    );
    assert!(
        msg.contains("order changed between save and restore"),
        "the rejection must state the cause (reorder), got: {msg}"
    );
}

/// Equipment added after construction gains port-slot coverage for its
/// declared ports while no step has run: the probe declares (and step-writes)
/// a thermal port for an environment zone the assembly table never
/// allocated (a real env zone with no equipment declarant, so the thermal
/// solver's wiring knows it) — before the fix, the table was never rebuilt
/// after construction, so the first stepped contribution failed with
/// "undeclared thermal zone"; after it, the step succeeds and the
/// accumulator exists.
#[test]
fn added_equipment_gains_port_slot_coverage_before_first_step() {
    let mut dwelling = build_fixture_dwelling(&fixture_dir(), true, None);
    let zone = dwelling
        .latest_env()
        .zones
        .iter()
        .map(|z| z.id)
        .find(|z| !dwelling.ports.thermal.iter().any(|t| t.zone == *z))
        .expect(
            "fixture must have an env zone with no thermal declarant \
                 (the coverage gap this test pins)",
        );
    dwelling
        .add_equipment(Box::new(
            IdentityProbeEquipment::new("ProbeThermal", 0).with_thermal_port(zone),
        ))
        .expect("add equipment before the first step");
    assert!(
        dwelling.ports.thermal.iter().any(|t| t.zone == zone),
        "the pre-step refresh must rebuild the slot table so the added \
         equipment's declared zone {zone:?} has an accumulator"
    );
    dwelling.step().expect(
        "the probe's thermal contribution must land, not fail as an \
                 undeclared zone",
    );
}

/// A declared zone that does not exist in the environment model is rejected
/// at the entrance at any time — the thermal solver's wiring is built from
/// env zones, so a contribution to an unknown zone would be silently
/// dropped (a debug-build assert catches it; release builds drop it
/// quietly). Mirrors the assembly boundary's `validate_equipment_zones`.
#[test]
fn add_equipment_rejects_unknown_zone_declaration() {
    let mut dwelling = build_fixture_dwelling(&fixture_dir(), true, None);
    let err = dwelling
        .add_equipment(Box::new(
            IdentityProbeEquipment::new("ProbeUnknownZone", 0)
                .with_thermal_port(hares_types::ZoneId(99)),
        ))
        .expect_err("a declaration for a non-env zone must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("does not exist in the environment model"),
        "the rejection must name the unknown-zone violation, got: {msg}"
    );
    assert!(
        !dwelling
            .equipment()
            .iter()
            .any(|eq| eq.descriptor().name == "ProbeUnknownZone"),
        "the rejected equipment must not join the vector"
    );
}

/// Mid-run equipment changes keep the output columns aligned: the
/// per-equipment column map is positional, and `record_step` zips it with
/// the live equipment vector. Before the fix, a mid-run removal left the
/// map desynced (it was rebuilt only while no rows had been recorded), so
/// every surviving equipment after the removed position had its values
/// written into the *predecessor's* columns — silently misattributed
/// output. After a mid-run removal the removed equipment's columns go
/// quiet and a constant-load survivor (MELs, ~90 W always-on plug load)
/// keeps reporting its own energy in its own column.
#[test]
fn midrun_equipment_removal_keeps_output_columns_aligned() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let hpxml = write_fixture_hpxml(tmp.path());
    let output_path = tmp.path().join("midrun_removal.csv");

    let config = dwelling_config(
        &hpxml,
        sim_config(Duration::days(2), Some(output_path.clone()), 1),
        None,
    );
    let mut dwelling = Dwelling::from_config(config).expect("dwelling builds");

    // Day 1 with the full equipment set (rows get recorded).
    for _ in 0..96 {
        dwelling.step().expect("day-1 step");
    }
    // Remove an equipment that sits EARLY in the vector, so every later
    // position shifts — the exact desync face.
    dwelling
        .remove_equipment("Clothes Washer")
        .expect("remove Clothes Washer mid-run");
    // Day 2 with the shifted vector; `simulate` runs the remaining horizon
    // and flushes/closes the output recorder.
    dwelling.simulate().expect("day-2 simulate");

    let contents = std::fs::read_to_string(&output_path).expect("read output CSV");
    let header: Vec<&str> = contents
        .lines()
        .next()
        .expect("header")
        .split(',')
        .collect();
    let mels_col = header
        .iter()
        .position(|c| *c == "MELs Electric Power (kW)")
        .unwrap_or_else(|| panic!("MELs column missing; header: {header:?}"));
    let washer_col = header
        .iter()
        .position(|c| *c == "Clothes Washer Electric Power (kW)")
        .expect("Clothes Washer column");

    let mut mels_day2_kwh = 0.0;
    let mut washer_day2_kwh = 0.0;
    for (i, line) in contents.lines().skip(1).enumerate() {
        if i < 96 {
            continue; // day 2 only
        }
        let fields: Vec<&str> = line.split(',').collect();
        mels_day2_kwh += fields[mels_col].trim().parse::<f64>().unwrap_or(0.0) * 0.25;
        washer_day2_kwh += fields[washer_col].trim().parse::<f64>().unwrap_or(0.0) * 0.25;
    }
    assert_eq!(
        washer_day2_kwh, 0.0,
        "the removed equipment's columns must go quiet after the removal — \
         nonzero values there mean another equipment's output is being written \
         into them (positional column-map desync)"
    );
    assert!(
        mels_day2_kwh > 1.0,
        "the always-on MELs plug load (~90 W → ≈2 kWh/day) must keep reporting \
         its own energy in its own column after the removal — a collapsed or \
         misattributed value means the column map desynced; got {mels_day2_kwh} kWh"
    );
}

/// A mid-run equipment add with output recording active must not demand
/// columns the frozen schema never allocated: the added equipment gets an
/// empty column map (its values read as missing — the documented contract),
/// the dwelling keeps stepping, and the schema-known equipment keep their
/// own columns. Before the `in_schema` predicate, `build_equipment_column_map`
/// demanded every applicable column for EVERY equipment and panicked in
/// debug builds the moment a post-recording add re-derived the map — the
/// CI python-test failure this pins (add_ev mid-simulation).
#[test]
fn midrun_equipment_add_with_recording_gets_empty_map_not_panic() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let hpxml = write_fixture_hpxml(tmp.path());
    let output_path = tmp.path().join("midrun_add.csv");

    let config = dwelling_config(
        &hpxml,
        sim_config(Duration::days(2), Some(output_path.clone()), 1),
        None,
    );
    let mut dwelling = Dwelling::from_config(config).expect("dwelling builds");

    // Day 1 with the full set (rows get recorded; the schema freezes).
    for _ in 0..96 {
        dwelling.step().expect("day-1 step");
    }
    // Add equipment mid-run, with output recording active — the exact
    // `add_ev` mid-simulation flow. Its declarations are electrical-only
    // (the default probe carries no ports beyond that), so the frozen-table
    // guard passes; the column-map re-derivation must not demand columns.
    dwelling
        .add_equipment(Box::new(IdentityProbeEquipment::new("ProbeLate", 0)))
        .expect("mid-run add with recording active must succeed");
    // Day 2 must run to completion — no panic, no failed step.
    dwelling.simulate().expect("day-2 simulate");

    // And the added equipment reads as missing values, never misattributed:
    // its own column would not exist, so assert the observable contract —
    // the run completed and the constant MELs survivor still reports its
    // own energy in its own column on day 2.
    let contents = std::fs::read_to_string(&output_path).expect("read output CSV");
    let header: Vec<&str> = contents
        .lines()
        .next()
        .expect("header")
        .split(',')
        .collect();
    let mels_col = header
        .iter()
        .position(|c| *c == "MELs Electric Power (kW)")
        .unwrap_or_else(|| panic!("MELs column missing; header: {header:?}"));
    let mels_day2_kwh: f64 = contents
        .lines()
        .skip(1 + 96)
        .filter_map(|l| l.split(',').nth(mels_col))
        .filter_map(|v| v.trim().parse::<f64>().ok())
        .map(|kw| kw * 0.25)
        .sum();
    assert!(
        mels_day2_kwh > 1.0,
        "the always-on MELs load must keep reporting its own energy after the \
         mid-run add; got {mels_day2_kwh} kWh"
    );
    assert!(
        !header.iter().any(|c| c.starts_with("ProbeLate")),
        "the frozen schema must not grow columns for the mid-run add"
    );
}

/// The same mid-run add as
/// `midrun_equipment_add_with_recording_gets_empty_map_not_panic`, but at
/// verbosity 7 where the global "HVAC Duct Losses (W)" column exists. The
/// column-map builder scopes the `expected` predicate to schema-known
/// equipment (so the added equipment's map is entirely empty), but
/// `record_step`'s debug invariant `if v >= 5 { debug_assert!(cols.duct_losses.is_some()) }`
/// is NOT scoped that way: it demands the duct-losses column for every
/// equipment, schema-known or not. A mid-run add at verbosity >= 5 should
/// therefore panic the debug build on the first recorded step after the add.
#[test]
fn midrun_equipment_add_with_recording_at_high_verbosity_does_not_panic() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let hpxml = write_fixture_hpxml(tmp.path());
    let output_path = tmp.path().join("midrun_add_v7.csv");

    let config = dwelling_config(
        &hpxml,
        sim_config(Duration::days(2), Some(output_path.clone()), 7),
        None,
    );
    let mut dwelling = Dwelling::from_config(config).expect("dwelling builds");

    for _ in 0..96 {
        dwelling.step().expect("day-1 step");
    }
    dwelling
        .add_equipment(Box::new(IdentityProbeEquipment::new("ProbeLate", 0)))
        .expect("mid-run add with recording active must succeed");
    dwelling.simulate().expect("day-2 simulate");
}

/// The same mid-run add, but the added equipment's NAME matches an HVAC
/// column predicate ("... Baseboard" satisfies `is_hvac_or_wh`), so
/// record_step's type-gated verbosity-7 debug invariants (`fan_power`,
/// `runtime_fraction`, and for heat-pump/cooling names `defrost_state`,
/// `er_power`, `shr`, `latent_gains`) apply to it. Those columns resolve
/// through the `in_schema` gate — a mid-run add after the schema froze is
/// schema-unknown, so its map is empty by design — and the invariants must
/// not demand per-equipment columns the schema never emitted: the
/// established contract for schema-unknown equipment is missing values,
/// never misattributed ones. The global `HVAC Duct Losses (W)` aggregate is
/// the one column such equipment still resolves (it accumulates every
/// equipment's contribution), pinned by the sibling test above.
#[test]
fn midrun_add_of_hvac_named_equipment_at_high_verbosity_does_not_panic() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let hpxml = write_fixture_hpxml(tmp.path());
    let output_path = tmp.path().join("midrun_add_hvac_v7.csv");

    let config = dwelling_config(
        &hpxml,
        sim_config(Duration::days(2), Some(output_path.clone()), 7),
        None,
    );
    let mut dwelling = Dwelling::from_config(config).expect("dwelling builds");

    for _ in 0..96 {
        dwelling.step().expect("day-1 step");
    }
    dwelling
        .add_equipment(Box::new(IdentityProbeEquipment::new(
            "ProbeLate Baseboard",
            0,
        )))
        .expect("mid-run add with recording active must succeed");
    dwelling.simulate().expect("day-2 simulate");
}

/// A mid-run equipment remove must keep the EndUse aggregate columns
/// attributed correctly. `end_use_aggregate_indices` is positional —
/// record_step zips it against the live equipment vector — so a remove
/// that shifts the vector without re-deriving the map silently writes
/// each surviving equipment's power into its removed neighbour's end-use
/// aggregate (the same positional-desync class the frozen-branch
/// re-derivation exists to prevent for the per-equipment and actor
/// maps). After the remove, every end-use aggregate column must equal the
/// sum of its surviving members' per-equipment electric-power columns,
/// step for step.
#[test]
fn end_use_aggregates_match_per_equipment_columns_after_midrun_remove() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let hpxml = write_fixture_hpxml(tmp.path());
    let output_path = tmp.path().join("end_use_desync.csv");

    let config = dwelling_config(
        &hpxml,
        sim_config(Duration::days(2), Some(output_path.clone()), 1),
        None,
    );
    let mut dwelling = Dwelling::from_config(config).expect("dwelling builds");

    // Pick the remove victim so the shift crosses at least one end-use
    // boundary: the first equipment whose successor belongs to a different
    // end-use. Removing it shifts every later equipment one slot down, into
    // end-use aggregate slots that are not theirs.
    let names: Vec<String> = dwelling
        .equipment()
        .iter()
        .map(|eq| eq.descriptor().name.clone())
        .collect();
    let victim = names
        .iter()
        .enumerate()
        .find(|(i, name)| {
            names.get(i + 1).is_some_and(|next| {
                hares_io::equipment_name_to_end_use(name)
                    != hares_io::equipment_name_to_end_use(next)
            })
        })
        .map(|(_, name)| name.clone())
        .expect("fixture must contain two adjacent equipment of different end-uses");

    // Day 1 with the full set (rows get recorded; the schema freezes).
    for _ in 0..96 {
        dwelling.step().expect("day-1 step");
    }
    dwelling
        .remove_equipment(&victim)
        .expect("mid-run remove must succeed");
    dwelling.simulate().expect("day-2 simulate");

    // Group the surviving equipment by end-use and check every aggregate
    // column against the sum of its members' own columns, step for step.
    let contents = std::fs::read_to_string(&output_path).expect("read output CSV");
    let mut lines = contents.lines();
    let header: Vec<&str> = lines.next().expect("header").split(',').collect();
    let day2: Vec<Vec<f64>> = lines
        .skip(96)
        .map(|l| {
            l.split(',')
                .map(|v| v.trim().parse::<f64>().unwrap_or(f64::NAN))
                .collect()
        })
        .collect();
    assert!(
        day2.len() >= 95,
        "day 2 must have recorded ~96 rows, got {}",
        day2.len()
    );

    let col = |name: &str| {
        header
            .iter()
            .position(|c| *c == name)
            .unwrap_or_else(|| panic!("column '{name}' missing; header: {header:?}"))
    };
    use std::collections::HashMap;
    let mut by_end_use: HashMap<hares_types::EndUse, Vec<&str>> = HashMap::new();
    for name in &names {
        if name != &victim {
            by_end_use
                .entry(hares_io::equipment_name_to_end_use(name))
                .or_default()
                .push(name);
        }
    }
    for (end_use, members) in &by_end_use {
        let agg_name = hares_io::end_use_electric_power_column(end_use);
        // The frozen schema emits aggregates for the end-uses present at
        // freeze time; a survivor's end-use was present then too (only
        // equipment was removed), so the column must exist.
        let agg_idx = col(&agg_name);
        let member_idx: Vec<usize> = members
            .iter()
            .map(|name| col(&format!("{name} Electric Power (kW)")))
            .collect();
        for (step, row) in day2.iter().enumerate() {
            let agg = row[agg_idx];
            let sum: f64 = member_idx.iter().map(|&i| row[i]).sum();
            assert!(
                (agg - sum).abs() < 1e-6,
                "end-use aggregate '{agg_name}' desynced from its members on \
                 day-2 step {step}: aggregate = {agg}, member sum = {sum} \
                 (members: {members:?}) — the positional aggregate map was \
                 not re-derived after the mid-run remove of '{victim}'"
            );
        }
    }
}

/// The aggregate map's mid-run **add** face: `end_use_aggregate_indices` is
/// positional and `record_step` zips it against the live equipment vector,
/// so an equipment added mid-run was silently TRUNCATED out of the
/// end-use aggregates (its energy vanished from them) before the
/// re-derivation. A live 0.5 kW OTHER probe joining after a recorded day
/// must raise its end-use aggregate by exactly its load, step for step —
/// the add-side twin of the remove test above.
#[test]
fn midrun_added_equipment_contributes_to_its_end_use_aggregate() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let hpxml = write_fixture_hpxml(tmp.path());
    let output_path = tmp.path().join("end_use_add.csv");

    let config = dwelling_config(
        &hpxml,
        sim_config(Duration::days(2), Some(output_path.clone()), 1),
        None,
    );
    let mut dwelling = Dwelling::from_config(config).expect("dwelling builds");

    // Day 1 with the fixture's set (rows recorded; the schema freezes).
    for _ in 0..96 {
        dwelling.step().expect("day-1 step");
    }
    let other_members: Vec<String> = dwelling
        .equipment()
        .iter()
        .map(|eq| eq.descriptor().name.clone())
        .filter(|name| hares_io::equipment_name_to_end_use(name) == hares_types::EndUse::OTHER)
        .collect();
    assert!(
        !other_members.is_empty(),
        "precondition: the fixture must carry OTHER end-use equipment"
    );

    // The live OTHER probe joins mid-run: schema-unknown (no per-equipment
    // column of its own), but its 0.5 kW must reach the end-use aggregate.
    dwelling
        .add_equipment(Box::new(
            IdentityProbeEquipment::new("ProbeAgg", 0).with_electric_load(0.5),
        ))
        .expect("mid-run add must succeed");
    dwelling.simulate().expect("day-2 simulate");

    let contents = std::fs::read_to_string(&output_path).expect("read output CSV");
    let mut lines = contents.lines();
    let header: Vec<&str> = lines.next().expect("header").split(',').collect();
    let day2: Vec<Vec<f64>> = lines
        .skip(96)
        .map(|l| {
            l.split(',')
                .map(|v| v.trim().parse::<f64>().unwrap_or(f64::NAN))
                .collect()
        })
        .collect();
    assert!(
        day2.len() >= 95,
        "day 2 must have recorded ~96 rows, got {}",
        day2.len()
    );

    let col = |name: &str| {
        header
            .iter()
            .position(|c| *c == name)
            .unwrap_or_else(|| panic!("column '{name}' missing; header: {header:?}"))
    };
    let agg_idx = col("Other End Use Electric Power (kW)");
    let member_idx: Vec<usize> = other_members
        .iter()
        .map(|name| col(&format!("{name} Electric Power (kW)")))
        .collect();
    for (step, row) in day2.iter().enumerate() {
        let agg = row[agg_idx];
        let sum: f64 = member_idx.iter().map(|&i| row[i]).sum();
        assert!(
            (agg - sum - 0.5).abs() < 1e-6,
            "the mid-run-added probe's 0.5 kW is missing from the OTHER \
             end-use aggregate on day-2 step {step}: aggregate = {agg}, \
             day-1 member sum = {sum} — the positional aggregate map \
             truncated the added equipment out of the aggregates"
        );
    }
}

/// Once stepping has begun the slot table is frozen, so equipment whose
/// declarations the table cannot satisfy is rejected loudly at the entrance
/// — its contributions through the orphaned port would otherwise be
/// silently miswired or dropped.
#[test]
fn add_equipment_after_stepping_rejects_undeclared_port() {
    let mut dwelling = build_fixture_dwelling(&fixture_dir(), true, None);
    dwelling.step().expect("first step succeeds");
    // A real env zone the frozen table never allocated (no declarant at
    // assembly): valid for the environment, unsatisfiable for the frozen
    // table.
    let zone = dwelling
        .latest_env()
        .zones
        .iter()
        .map(|z| z.id)
        .find(|z| !dwelling.ports.thermal.iter().any(|t| t.zone == *z))
        .expect("fixture must have an env zone with no thermal declarant");
    let err = dwelling
        .add_equipment(Box::new(
            IdentityProbeEquipment::new("ProbeLateThermal", 0).with_thermal_port(zone),
        ))
        .expect_err("a post-step add with an unsatisfiable declaration must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("ProbeLateThermal"),
        "the rejection must name the equipment, got: {msg}"
    );
    assert!(
        msg.contains("no accumulator in the dwelling's port-slot table"),
        "the rejection must name the orphaned declaration, got: {msg}"
    );
    assert!(
        !dwelling
            .equipment()
            .iter()
            .any(|eq| eq.descriptor().name == "ProbeLateThermal"),
        "the rejected equipment must not join the vector"
    );
}

// ===========================================================================
// The replacement entrance, actor-column attribution, checkpoint count
// ===========================================================================

// The remaining entrances and maps of the identity change that the tests
// above do not reach:
//
// - `Dwelling::replace_equipment` — the third equipment entrance
//   (Python-reachable via `py_dwelling.rs`), which gained the same identity
//   contract as `add_equipment`: auto-assignment, collision policy that
//   excludes the evictee, the landed-write postcondition, and registration
//   guarding. The `add_equipment` tests pin the shared
//   `assign_equipment_identity` helper's arms; these pin that the
//   replacement entrance actually routes through it and adds the
//   replacement-specific policy (the evictee's id leaves with it and is
//   transferable to the replacement).
// - The actor half of the mid-run output-column fix: removing equipment
//   evicts the actors targeting it, which shifts the actor vector exactly
//   like the equipment vector. The equipment-column test above pins that
//   map; the actor column map is re-derived by the same refresh and is
//   pinned here through the observable CSV columns.
// - The checkpoint count guard: the identity-keyed restore loop iterates the
//   *dwelling's* equipment, so a checkpoint with more equipment states than
//   the dwelling would silently drop the extras' state if the count check
//   did not fire first.

/// The replacement entrance assigns identity like `add_equipment`: an
/// unassigned replacement receives a fresh never-reused id, the evictee
/// leaves the vector, and the rebuilt identity maps keep every equipment
/// individually addressable.
#[test]
fn replace_equipment_assigns_a_fresh_id_and_keeps_equipment_addressable() {
    let mut dwelling = build_fixture_dwelling(&fixture_dir(), true, None);
    let ids_before: Vec<u32> = dwelling
        .equipment()
        .iter()
        .map(|eq| eq.descriptor().id.0)
        .collect();
    let washer_id = id_map(&dwelling)["Clothes Washer"];

    let old = dwelling
        .replace_equipment(
            "Clothes Washer",
            Box::new(IdentityProbeEquipment::new("ProbeSwapIn", 0)),
        )
        .expect("an unassigned replacement must be auto-assigned and accepted");
    assert_eq!(
        old.descriptor().name,
        "Clothes Washer",
        "the evictee must be returned to the caller"
    );

    let map = id_map(&dwelling);
    let new_id = map["ProbeSwapIn"];
    assert!(
        new_id != 0,
        "the unassigned sentinel must not survive the replacement entrance, got {map:?}"
    );
    assert_ne!(
        new_id, washer_id,
        "the never-reused id counter must not reissue the evictee's id"
    );
    assert!(
        !ids_before.contains(&new_id),
        "the auto-assigned id must be fresh — never held by any equipment this \
         dwelling has assembled or admitted, got {new_id} vs {ids_before:?}"
    );
    let mut ids: Vec<u32> = map.values().copied().collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(
        ids.len(),
        map.len(),
        "after the replacement every equipment must stay uniquely keyed, got {map:?}"
    );

    dwelling
        .step()
        .expect("the first post-replacement step must succeed");
    assert_eq!(
        dwelling.latest_env().equipment_core.len(),
        dwelling.equipment().len(),
        "after the replacement, equipment_core must hold one entry per equipment — \
         the rebuilt name→id map must address the replacement individually"
    );
}

/// The replacement entrance's collision policy: an explicit id colliding
/// with any *surviving* equipment is rejected naming both (the vector stays
/// untouched), while the evictee's own id is accepted — the ordinary
/// same-identity swap (e.g. a Python caller swapping an EV for an upgraded
/// EV). Replacing a missing name is an error.
#[test]
fn replace_equipment_rejects_collision_with_survivors_but_accepts_the_evictees_id() {
    let mut dwelling = build_fixture_dwelling(&fixture_dir(), true, None);
    let ev_id = id_map(&dwelling)["EV"];
    let washer_id = id_map(&dwelling)["Clothes Washer"];

    // A missing name is an error, not a silent append.
    let err = dwelling
        .replace_equipment(
            "Does Not Exist",
            Box::new(IdentityProbeEquipment::new("ProbeGhost", 0)),
        )
        .map(|_| ())
        .expect_err("replacing a missing equipment must fail");
    assert!(
        err.to_string().contains("not found"),
        "the error must name the missing equipment, got: {err}"
    );

    // An explicit id colliding with a surviving equipment is rejected.
    let err = dwelling
        .replace_equipment(
            "Clothes Washer",
            Box::new(IdentityProbeEquipment::new("ProbeCollide", ev_id)),
        )
        .map(|_| ())
        .expect_err("a replacement id colliding with a survivor must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("duplicate equipment id"),
        "the rejection must name the violation, got: {msg}"
    );
    assert!(
        msg.contains("ProbeCollide") && msg.contains("EV"),
        "the collision error must name both equipment, got: {msg}"
    );
    assert!(
        dwelling
            .equipment()
            .iter()
            .any(|eq| eq.descriptor().name == "Clothes Washer"),
        "a rejected replacement must leave the original equipment in place"
    );

    // The evictee's own id leaves with it and is transferable to the
    // replacement — the same-identity swap must not spuriously collide.
    dwelling
        .replace_equipment(
            "Clothes Washer",
            Box::new(IdentityProbeEquipment::new("ProbeSwap", washer_id)),
        )
        .expect("the evictee's own id must be acceptable for the replacement");
    assert_eq!(
        id_map(&dwelling)["ProbeSwap"],
        washer_id,
        "the evictee's id must land on the replacement"
    );
    assert!(
        !dwelling
            .equipment()
            .iter()
            .any(|eq| eq.descriptor().name == "Clothes Washer"),
        "the evictee must have left the vector"
    );
}

/// The replacement entrance registers the newcomer like any other equipment:
/// a guard-tracking type (EV is one of the two production flag-trackers,
/// with Battery) must report `is_initialized` after the swap. An unguarded
/// replacement would accept post-registration LUT/identity mutation through
/// the trait — the registration gap this entrance used to have.
#[test]
fn replace_equipment_marks_guard_tracking_replacements_initialized() {
    let mut dwelling = build_fixture_dwelling(&fixture_dir(), true, None);
    let ev_id = id_map(&dwelling)["EV"];

    let mut data = HashMap::new();
    data.insert(
        "equipment_id".to_string(),
        ConfigValue::Float(f64::from(ev_id)),
    );
    let config = EquipmentConfig::raw("EV".to_string(), "EV".to_string(), data);
    let new_ev = EquipmentRegistry::new()
        .create("EV", config)
        .unwrap_or_else(|e| panic!("registry must create the replacement EV: {e}"));
    assert!(
        !new_ev.is_initialized(),
        "precondition: a registry-constructed EV is not yet registration-guarded"
    );

    let old = dwelling
        .replace_equipment("EV", new_ev)
        .expect("the same-identity EV swap must be accepted");
    assert_eq!(old.descriptor().name, "EV");

    let replacement = dwelling
        .equipment()
        .iter()
        .find(|eq| eq.descriptor().name == "EV")
        .expect("the replacement must be in the vector");
    assert_eq!(
        replacement.descriptor().id.0,
        ev_id,
        "the replacement must carry the transferred id"
    );
    assert!(
        replacement.is_initialized(),
        "the replacement must be registration-guarded on entry — an unguarded \
         EV accepts post-registration LUT/identity mutation through the trait"
    );
}

/// The actor half of the mid-run output-column alignment: removing equipment
/// evicts the actors targeting it, shifting the actor vector. The actor
/// column map is positional (`record_step` zips it with the live actor
/// vector), so a removal that shifts a surviving telemetry-bearing actor
/// would write that actor's values into the evicted actor's columns and leave
/// its own quiet — silently misattributed telemetry. After a mid-run removal
/// the evicted actor's columns must go quiet and the survivor must keep
/// reporting its own telemetry in its own column.
///
/// Both observation actors are provisioned (EV driver + BMS via the
/// SelfConsumption override) so whichever is evicted, the survivor has a live
/// soc channel. The *earlier* of the two equipment is removed, so the
/// survivor's actor always shifts position — the exact face of the desync.
#[test]
fn midrun_equipment_removal_keeps_actor_columns_attributed() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let src = std::fs::read_to_string(fixture_dir().join("home.xml")).expect("home.xml");
    let hpxml = tmp.path().join("fixture_ev_battery.xml");
    std::fs::write(&hpxml, inject_battery(&inject_ev(&src))).expect("write injected HPXML");
    let output_path = tmp.path().join("actor_columns_midrun.csv");

    let nightly = serde_json::json!({"Nightly": {
        "off_peak_start_hour": 22.0,
        "off_peak_end_hour": 6.0,
        "target_soc": 0.90,
    }});
    let bms_mode = serde_json::json!({"SelfConsumption": {
        "min_soc": 0.1,
        "max_soc": 1.0,
        "solar_only_charging": false,
        "surplus_deadband_kw": 0.0,
    }});
    let overrides = serde_json::json!({
        "EV": { "charging_strategy": nightly.to_string() },
        "Battery": { "bms_mode": bms_mode.to_string() },
    });
    let config = DwellingConfig {
        hpxml_path: hpxml,
        schedule_path: fixture_dir().join("in.schedules.csv"),
        weather_path: weather_for(&fixture_dir()),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: sim_config(Duration::days(2), Some(output_path.clone()), 5),
        overrides: Some(overrides),
        bldg_id: 100,
        initialization_duration: None,
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    };
    let mut dwelling = Dwelling::from_config(config).expect("dwelling builds");

    // Both observation actors are live; remove the earlier of the two
    // equipment so the survivor's actor shifts position in the vector.
    let names: Vec<String> = dwelling
        .equipment()
        .iter()
        .map(|eq| eq.descriptor().name.clone())
        .collect();
    let ev_pos = names
        .iter()
        .position(|n| n == "EV")
        .expect("EV injected into the fixture");
    let battery_pos = names
        .iter()
        .position(|n| n == "Battery")
        .expect("Battery injected into the fixture");
    let (victim, victim_col, survivor_col) = if ev_pos < battery_pos {
        (
            "EV",
            "actor:EvDriver:EV:soc",
            "actor:BatteryManagementActor:Battery:soc",
        )
    } else {
        (
            "Battery",
            "actor:BatteryManagementActor:Battery:soc",
            "actor:EvDriver:EV:soc",
        )
    };

    // Day 1 with both actors (rows get recorded).
    for _ in 0..STEPS_PER_DAY {
        dwelling.step().expect("day-1 step");
    }
    dwelling
        .remove_equipment(victim)
        .expect("remove the victim equipment mid-run");
    // Day 2 with the shifted actor vector.
    dwelling.simulate().expect("day-2 simulate");

    let contents = std::fs::read_to_string(&output_path).expect("read output CSV");
    let header: Vec<&str> = contents
        .lines()
        .next()
        .expect("header")
        .split(',')
        .collect();
    let victim_idx = header
        .iter()
        .position(|c| *c == victim_col)
        .unwrap_or_else(|| panic!("'{victim_col}' column missing; header: {header:?}"));
    let survivor_idx = header
        .iter()
        .position(|c| *c == survivor_col)
        .unwrap_or_else(|| panic!("'{survivor_col}' column missing; header: {header:?}"));

    let mut victim_day = [0.0_f64; 2];
    let mut survivor_day = [0.0_f64; 2];
    for (i, line) in contents.lines().skip(1).enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').collect();
        let day = if i < STEPS_PER_DAY { 0 } else { 1 };
        victim_day[day] =
            victim_day[day].max(fields[victim_idx].trim().parse::<f64>().unwrap_or(0.0));
        survivor_day[day] =
            survivor_day[day].max(fields[survivor_idx].trim().parse::<f64>().unwrap_or(0.0));
    }

    assert!(
        victim_day[0] > 0.05 && survivor_day[0] > 0.05,
        "precondition: both actors' soc channels must be live before the removal, \
         got victim={:?} survivor={:?}",
        victim_day,
        survivor_day
    );
    assert_eq!(
        victim_day[1], 0.0,
        "the evicted actor's soc column must go quiet after the removal — nonzero \
         values there mean another actor's telemetry is written into it (positional \
         actor-column desync), got {}",
        victim_day[1]
    );
    assert!(
        survivor_day[1] > 0.05,
        "the surviving actor must keep reporting its own soc in its own column after \
         the removal — a quiet column means its telemetry was misattributed into the \
         evicted actor's columns, got {}",
        survivor_day[1]
    );
}

/// The checkpoint count guard: the identity-keyed restore loop iterates the
/// dwelling's equipment, so when the checkpoint holds *more* equipment states
/// than the dwelling (an equipment was removed between save and restore),
/// every surviving name still resolves in the checkpoint map and the extra
/// state would be silently dropped. The count check must fire first, naming
/// both counts.
#[test]
fn checkpoint_restore_rejects_a_shrunk_equipment_set() {
    let (config, _tmp) = fixture_dwelling_config(&fixture_dir(), true, None);

    let mut dwelling_a = hares_core::dwelling::DwellingBlueprint::from_config(config.clone())
        .expect("blueprint A")
        .build()
        .expect("build dwelling A");
    for _ in 0..2 {
        dwelling_a.step().expect("pre-save steps succeed");
    }
    let a_count = dwelling_a.equipment().len();
    let checkpoint = dwelling_a.save_checkpoint().expect("save checkpoint");

    // Same config minus the injected EV spec (proven to assemble — the
    // structural tests above read its equipment): the dwelling is smaller
    // than the checkpoint.
    let mut blueprint_b =
        hares_core::dwelling::DwellingBlueprint::from_config(config).expect("blueprint B");
    blueprint_b
        .equipment_specs
        .retain(|s| s.instance_name.clone().unwrap_or_else(|| s.name.clone()) != "EV");
    let mut dwelling_b = blueprint_b.build().expect("build dwelling B");
    assert!(
        !dwelling_b
            .equipment()
            .iter()
            .any(|eq| eq.descriptor().name == "EV"),
        "precondition: the removed spec must be the one that assembled the EV"
    );
    assert_eq!(
        dwelling_b.equipment().len() + 1,
        a_count,
        "precondition: the spec removal must shrink the equipment set by exactly one"
    );

    let err = dwelling_b
        .load_checkpoint(checkpoint)
        .expect_err("a shrunk equipment set must fail the restore");
    let msg = err.to_string();
    assert!(
        msg.contains("count mismatch"),
        "the rejection must name the count mismatch — without it the restore would \
         silently drop the removed equipment's state, got: {msg}"
    );
}

/// The replacement entrance validates declared zones against the environment
/// model exactly like `add_equipment`: a replacement declaring a port for a
/// zone that does not exist would have its contributions silently dropped
/// (the thermal solver's wiring is built from env zones), so it is rejected
/// at the entrance and the original equipment stays in place.
#[test]
fn replace_equipment_rejects_unknown_zone_declaration() {
    let mut dwelling = build_fixture_dwelling(&fixture_dir(), true, None);
    let err = dwelling
        .replace_equipment(
            "Clothes Washer",
            Box::new(
                IdentityProbeEquipment::new("ProbeSwapZone", 0)
                    .with_thermal_port(hares_types::ZoneId(99)),
            ),
        )
        .map(|_| ())
        .expect_err("a replacement declaring a non-env zone must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("does not exist in the environment model"),
        "the rejection must name the unknown-zone violation, got: {msg}"
    );
    assert!(
        dwelling
            .equipment()
            .iter()
            .any(|eq| eq.descriptor().name == "Clothes Washer"),
        "a rejected replacement must leave the original equipment in place"
    );
    assert!(
        !dwelling
            .equipment()
            .iter()
            .any(|eq| eq.descriptor().name == "ProbeSwapZone"),
        "the rejected replacement must not join the vector"
    );
}

/// Once stepping has begun the port-slot table is frozen, so a replacement
/// whose declarations the frozen table cannot satisfy is rejected at the
/// entrance — its contributions through the orphaned port would otherwise
/// be silently miswired or dropped. The same rule as `add_equipment`, wired
/// at the replacement entrance.
#[test]
fn replace_equipment_after_stepping_rejects_undeclared_port() {
    let mut dwelling = build_fixture_dwelling(&fixture_dir(), true, None);
    dwelling.step().expect("first step succeeds");
    // A real env zone the frozen table never allocated (no declarant at
    // assembly): valid for the environment, unsatisfiable for the frozen
    // table.
    let zone = dwelling
        .latest_env()
        .zones
        .iter()
        .map(|z| z.id)
        .find(|z| !dwelling.ports.thermal.iter().any(|t| t.zone == *z))
        .expect("fixture must have an env zone with no thermal declarant");
    let err = dwelling
        .replace_equipment(
            "Clothes Washer",
            Box::new(IdentityProbeEquipment::new("ProbeSwapLate", 0).with_thermal_port(zone)),
        )
        .map(|_| ())
        .expect_err("a post-step replacement with an unsatisfiable declaration must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("ProbeSwapLate"),
        "the rejection must name the equipment, got: {msg}"
    );
    assert!(
        msg.contains("no accumulator in the dwelling's port-slot table"),
        "the rejection must name the orphaned declaration, got: {msg}"
    );
    assert!(
        dwelling
            .equipment()
            .iter()
            .any(|eq| eq.descriptor().name == "Clothes Washer"),
        "a rejected replacement must leave the original equipment in place"
    );
}

/// `clear_equipment` empties the dwelling but must never lower the
/// never-reused id counter: equipment added after a clear receives ids past
/// every id the dwelling has ever issued. A counter reset on clear would
/// reissue an id the pre-clear equipment held — and anything keyed under it
/// (a checkpoint saved before the clear, output columns) would silently
/// alias the new equipment.
#[test]
fn clear_equipment_never_reissues_prior_ids() {
    let mut dwelling = build_fixture_dwelling(&fixture_dir(), true, None);
    let assembly_max = dwelling
        .equipment()
        .iter()
        .map(|eq| eq.descriptor().id.0)
        .max()
        .expect("fixture has equipment");

    dwelling.clear_equipment();
    assert!(
        dwelling.equipment().is_empty(),
        "clear must empty the equipment vector"
    );

    dwelling
        .add_equipment(Box::new(IdentityProbeEquipment::new("ProbeAfterClear", 0)))
        .expect("adding equipment after a clear must succeed");
    let new_id = id_map(&dwelling)["ProbeAfterClear"];
    assert!(
        new_id > assembly_max,
        "the id counter must survive the clear — reissuing id {new_id} \
         (≤ assembly max {assembly_max}) would alias a pre-clear equipment's identity"
    );
}

/// The `replace_equipment` entrance — the third equipment-push path, which
/// the fix plan missed — carries the same identity contract as
/// `add_equipment`: an unassigned replacement is auto-assigned a fresh
/// never-reused id, the evictee's own id is freed by the swap and may be
/// reused for the replacement, an explicit id colliding with any *other*
/// surviving equipment is rejected naming both, and a broken identity
/// write fails the swap.
#[test]
fn replace_equipment_assigns_fresh_identity_and_validates_collisions() {
    let mut dwelling = build_fixture_dwelling(&fixture_dir(), true, None);
    let ev_id = id_map(&dwelling)["EV"];
    let (other_name, other_id) = dwelling
        .equipment()
        .iter()
        .map(|eq| (eq.descriptor().name.clone(), eq.descriptor().id.0))
        .find(|(name, _)| name != "EV")
        .expect("fixture has more than one equipment");

    // Unassigned replacement: auto-assigned a fresh id, old id leaves with
    // the evictee.
    let old = dwelling
        .replace_equipment("EV", Box::new(IdentityProbeEquipment::new("EV", 0)))
        .expect("replace with an unassigned replacement must succeed");
    assert_eq!(old.descriptor().id.0, ev_id, "the evictee is returned");
    let new_id = id_map(&dwelling)["EV"];
    assert!(
        new_id != 0 && new_id != ev_id,
        "the replacement must receive a fresh never-reused id, got {new_id} (evictee had {ev_id})"
    );

    // The evictee's id is freed by the swap: reusing it for the replacement
    // is allowed (no collision — the old equipment left).
    dwelling
        .replace_equipment("EV", Box::new(IdentityProbeEquipment::new("EV", new_id)))
        .expect("reusing the just-freed id must be allowed");

    // An explicit id colliding with a *different* surviving equipment is
    // rejected, naming both.
    let err = dwelling
        .replace_equipment("EV", Box::new(IdentityProbeEquipment::new("EV", other_id)))
        .err()
        .expect("a collision with another surviving equipment must fail the swap");
    let msg = err.to_string();
    assert!(
        msg.contains("EV") && msg.contains(&other_name),
        "the collision error must name both equipment, got: {msg}"
    );

    // A broken identity write fails the swap with the postcondition
    // diagnostic pointing at the setter implementation.
    let err = dwelling
        .replace_equipment(
            "EV",
            Box::new(IdentityProbeEquipment::new("EV", 0).with_sabotaged_setter()),
        )
        .err()
        .expect("a no-op identity write must fail the swap");
    assert!(
        err.to_string().contains("set_equipment_id"),
        "the error must point at the setter implementation, got: {err}"
    );
    // The failed swaps must not have replaced anything: the live EV still
    // carries the last successful replacement's identity.
    assert_eq!(id_map(&dwelling)["EV"], new_id);
}

/// An actor's equipment binding must follow a replacement: eviction is
/// name-based and only fires on removal, so the actor targeting "Battery"
/// survives a `replace_equipment` — and the replacement receives a new
/// never-reused id. Without re-resolving the binding on the identity
/// refresh, the BMS kept reading the evictee's (now absent) id and its
/// `soc` channel pinned at the 0.0 no-observation sentinel — the exact
/// observation-misbinding class of the original bug, reintroduced through
/// the replace entrance. End to end: real assembly, battery-injected
/// fixture, BMS auto-registered, mid-run replacement, output CSV.
#[test]
fn actor_rebinds_to_replacement_equipment_after_replace() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let src = std::fs::read_to_string(fixture_dir().join("home.xml")).expect("home.xml");
    let hpxml = tmp.path().join("fixture_battery_replace.xml");
    std::fs::write(&hpxml, inject_battery(&src)).expect("write battery-injected HPXML");

    let output_path = tmp.path().join("battery_replace.csv");
    let bms_mode = serde_json::json!({"SelfConsumption": {
        "min_soc": 0.1,
        "max_soc": 1.0,
        "solar_only_charging": false,
        "surplus_deadband_kw": 0.0,
    }});
    let overrides = serde_json::json!({
        "Battery": { "bms_mode": bms_mode.to_string() }
    });
    let config = DwellingConfig {
        hpxml_path: hpxml,
        schedule_path: fixture_dir().join("in.schedules.csv"),
        weather_path: weather_for(&fixture_dir()),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: sim_config(Duration::days(2), Some(output_path.clone()), 5),
        overrides: Some(overrides),
        bldg_id: 100,
        initialization_duration: None,
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    };
    let mut dwelling = Dwelling::from_config(config).expect("dwelling builds");

    // Day 1 with the original battery (rows recorded; the BMS observes it).
    for _ in 0..96 {
        dwelling.step().expect("day-1 step");
    }

    // Replace the battery mid-run with a fresh, unassigned instance — the
    // replacement is auto-assigned a new never-reused id.
    let battery_cfg: hares_equipment::BatteryConfig = serde_json::from_value(serde_json::json!({
        "capacity_kwh": 13.5,
        "max_charge_kw": 5.0,
        "max_discharge_kw": 5.0,
    }))
    .expect("minimal BatteryConfig");
    let eq_config =
        EquipmentConfig::from_typed("Battery".to_string(), "Battery".to_string(), battery_cfg)
            .expect("typed battery config");
    let registry = hares_equipment::EquipmentRegistry::new();
    let mut replacement = registry
        .create("Battery", eq_config.clone())
        .expect("create replacement battery");
    replacement
        .init(&eq_config, dwelling.latest_env())
        .expect("init replacement battery");
    assert_eq!(
        replacement.descriptor().id,
        hares_types::EquipmentId(0),
        "precondition: the replacement arrives unassigned"
    );
    dwelling
        .replace_equipment("Battery", replacement)
        .expect("replace the battery mid-run");

    // Day 2: the BMS must observe the *replacement* battery's SOC (default
    // initial 0.5), not the evictee's absent id.
    dwelling.simulate().expect("day-2 simulate");

    let contents = std::fs::read_to_string(&output_path).expect("read output CSV");
    let header: Vec<&str> = contents
        .lines()
        .next()
        .expect("header")
        .split(',')
        .collect();
    let soc_col = header
        .iter()
        .position(|c| *c == "actor:BatteryManagementActor:Battery:soc")
        .unwrap_or_else(|| panic!("BMS soc column missing; header: {header:?}"));
    let mut post_replace_soc_max = 0.0_f64;
    let mut post_replace_soc_last = 0.0_f64;
    for line in contents.lines().skip(1 + 96) {
        // day 2 only
        if let Some(v) = line.split(',').nth(soc_col) {
            if let Ok(soc) = v.trim().parse::<f64>() {
                post_replace_soc_max = post_replace_soc_max.max(soc);
                post_replace_soc_last = soc;
            }
        }
    }
    assert!(
        post_replace_soc_max > 0.1,
        "the BMS must re-bind to the replacement battery and observe its SOC \
         (default initial 0.5) — a channel pinned at 0.0 after the replace means \
         the actor is still reading the evictee's id; got max soc = {post_replace_soc_max}"
    );
    // The sustained value, not a single transitional row: the first
    // post-replace step can still see the evictee's snapshot entry before
    // the end-of-step retain drops its id, so only the final row proves the
    // binding followed the replacement.
    assert!(
        post_replace_soc_last > 0.1,
        "the FINAL row's soc must reflect the replacement battery — a stale \
         binding reads the evictee's (retained-away) id and decays to the 0.0 \
         no-observation sentinel; got last soc = {post_replace_soc_last}"
    );
}

// ===========================================================================
// Identity-refresh observability: seeding and actor-binding resolution
// ===========================================================================
//
// The identity refresh (`refresh_equipment_caches`) is the shared mechanism
// behind two contracts the rebind test above does not pin:
//
// - **Seeding**: equipment entering mid-run (add, replace) must be
//   observable in `equipment_core` *immediately* — before its first step
//   completes — so actors reading its output on the joining step see real
//   state instead of the no-observation sentinel. The rebind test's
//   final-row assertion cannot catch a missing seed: one blind step at the
//   join decays into identical later rows.
// - **Actor-binding resolution for user-added actors**: the resolve loop
//   runs for *every* actor on *every* refresh. Before the loop existed,
//   only the auto-seed path resolved bindings — a manually added actor
//   never resolved at all, and an actor registered before its equipment
//   existed never recovered. Both user-reachable arms are pinned here
//   through the public `add_actor` / `add_equipment` entrances and the
//   public `Dwelling::telemetry()` actor channels.

/// Equipment joining a dwelling mid-run is observable in `equipment_core`
/// on the joining step itself — the identity refresh seeds each entering
/// equipment's current core output, which is the truthful value the
/// end-of-step snapshot will write from its first step onward.
#[test]
fn midrun_added_equipment_is_observable_before_its_first_step() {
    let mut dwelling = build_fixture_dwelling(&fixture_dir(), true, None);
    dwelling.step().expect("first step succeeds");
    let pre_add_entries = dwelling.latest_env().equipment_core.len();

    dwelling
        .add_equipment(Box::new(IdentityProbeEquipment::new("ProbeJoinObs", 0)))
        .expect("mid-run add must succeed");
    let new_id = EquipmentId(id_map(&dwelling)["ProbeJoinObs"]);
    assert_eq!(
        dwelling.latest_env().equipment_core.len(),
        pre_add_entries + 1,
        "the identity refresh must seed an equipment_core entry for the \
         joining equipment — actors reading its output on the joining step \
         otherwise get the no-observation sentinel"
    );
    let probe = dwelling
        .equipment()
        .iter()
        .find(|eq| eq.descriptor().name == "ProbeJoinObs")
        .expect("probe joined the vector");
    assert_eq!(
        dwelling.latest_env().equipment_core.get(&new_id),
        Some(probe.core_output()),
        "the seeded entry must be the equipment's own current core output — \
         the value the end-of-step snapshot will commit"
    );
}

/// A manually constructed BMS for the shared resolve loop's user path. The
/// battery's default Manual mode seeds no auto-actor, so a successful
/// `add_actor` also proves no auto-registered BMS exists (the call rejects
/// duplicate actor names).
fn user_bms_actor() -> Box<dyn hares_core::actor::Actor> {
    Box::new(BatteryManagementActor::new(
        "Battery",
        BmsMode::SelfConsumption {
            min_soc: 0.1,
            max_soc: 1.0,
            solar_only_charging: false,
            surplus_deadband_kw: 0.0,
        },
        GridExportRule::Unrestricted,
        5.0,
        5.0,
        None,
        96,
        0,
    ))
}

/// The max observed soc of the BMS's telemetry channel over the run so
/// far — the observable for binding resolution (a resolved binding reads
/// the battery's real SOC, default initial 0.5; an unresolved one pins at
/// the 0.0 no-observation sentinel).
fn bms_observed_soc(dwelling: &Dwelling) -> f64 {
    dwelling
        .telemetry()
        .actor_telemetry
        .get("BatteryManagementActor:Battery")
        .and_then(|channels| channels.get("soc"))
        .copied()
        .unwrap_or(0.0)
}

/// A user-added actor resolves its equipment binding at registration.
/// Before the resolve loop ran for every actor, only the auto-seed path
/// resolved — a manually added BMS never resolved and its soc channel
/// stayed at the 0.0 sentinel for the whole run (the observation-misbinding
/// class through the user-actor door).
#[test]
fn user_added_actor_resolves_its_equipment_binding_on_add() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let src = std::fs::read_to_string(fixture_dir().join("home.xml")).expect("home.xml");
    let hpxml = tmp.path().join("fixture_battery_user_actor.xml");
    std::fs::write(&hpxml, inject_battery(&src)).expect("write battery-injected HPXML");

    let config = DwellingConfig {
        hpxml_path: hpxml,
        schedule_path: fixture_dir().join("in.schedules.csv"),
        weather_path: weather_for(&fixture_dir()),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: sim_config(Duration::hours(6), None, 0),
        overrides: None,
        bldg_id: 100,
        initialization_duration: None,
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    };
    let mut dwelling = Dwelling::from_config(config).expect("dwelling builds");

    dwelling.add_actor(user_bms_actor()).expect(
        "manual BMS add must succeed — a duplicate-name error would \
             mean the fixture auto-registered a BMS actor",
    );

    for _ in 0..4 {
        dwelling.step().expect("step");
    }
    let soc = bms_observed_soc(&dwelling);
    assert!(
        soc > 0.1,
        "a user-added BMS must resolve its battery binding at registration and \
         observe the battery's SOC (default initial 0.5) — a channel pinned at \
         0.0 means the binding was never resolved, got soc = {soc}"
    );
}

/// The resolve contract's recovery arm: an actor registered *before* its
/// equipment exists resolves to the documented "operate without SOC
/// feedback" mode, and the equipment joining later (here through the
/// `add_equipment` user entrance) must recover the binding on the identity
/// refresh — the actor observes the newly arrived battery from its first
/// step, rather than staying blind forever.
#[test]
fn actor_binding_recovers_when_its_equipment_arrives_later() {
    // The EV fixture carries no battery: the actor is added while its
    // equipment does not exist. Two hours of horizon: one blind step before
    // the battery joins, then the recovery steps.
    let (mut config, _tmp) = fixture_dwelling_config(&fixture_dir(), true, None);
    config.sim_config.duration = Duration::hours(2);
    let mut dwelling = Dwelling::from_config(config).expect("dwelling builds");
    dwelling
        .add_actor(user_bms_actor())
        .expect("an actor added before its equipment must be accepted");

    // The documented intermediate state: with no equipment to bind, the
    // actor operates without SOC feedback — the run must proceed (the
    // actor is accepted, not an error) and its soc channel reads the 0.0
    // no-observation sentinel rather than a fabricated value.
    dwelling
        .step()
        .expect("a step with an unbound actor must succeed (the no-feedback mode)");
    let blind_soc = bms_observed_soc(&dwelling);
    assert_eq!(
        blind_soc, 0.0,
        "an unbound actor must report the no-observation sentinel, not a \
             fabricated SOC, while its equipment is absent"
    );

    // The battery joins through the user entrance — registry-built and
    // initialized, mirroring the Python adapter's add path.
    let battery_cfg: hares_equipment::BatteryConfig = serde_json::from_value(serde_json::json!({
        "capacity_kwh": 13.5,
        "max_charge_kw": 5.0,
        "max_discharge_kw": 5.0,
    }))
    .expect("minimal BatteryConfig");
    let eq_config =
        EquipmentConfig::from_typed("Battery".to_string(), "Battery".to_string(), battery_cfg)
            .expect("typed battery config");
    let registry = EquipmentRegistry::new();
    let mut battery = registry
        .create("Battery", eq_config.clone())
        .expect("create battery");
    battery
        .init(&eq_config, dwelling.latest_env())
        .expect("init battery");
    dwelling
        .add_equipment(battery)
        .expect("battery joins the dwelling");

    for _ in 0..4 {
        dwelling.step().expect("step");
    }
    let soc = bms_observed_soc(&dwelling);
    assert!(
        soc > 0.1,
        "the binding must recover when the equipment arrives — an actor \
             registered before its equipment would otherwise stay in the \
             no-SOC-feedback mode forever (the pre-fix user-actor gap), \
             got soc = {soc}"
    );
}

/// A dwelling-level safety net must fail loudly when its observation channel
/// is absent, not silently skip the check: `ev_capacity_degraded` reads four
/// static keys every real EV publishes at construction and every step, so an
/// EV-end-use equipment missing them is a wiring defect — the same
/// monitor-blindness class (I-06) the other regressions in this file pin
/// from the misattributed-reading end. Pre-fix, the gate silently skipped
/// the check whenever any key was missing, so a misbound EV passed
/// invariants unobserved.
#[test]
fn ev_capacity_check_fails_loudly_when_ev_telemetry_keys_are_absent() {
    let mut dwelling = build_fixture_dwelling(&fixture_dir(), false, None);
    // An EV-end-use probe whose telemetry carries none of the four static
    // keys — reachable only through a wiring defect, never through the real
    // EV constructor (which publishes them at construction, `Ev::new`).
    dwelling
        .add_equipment(Box::new(
            IdentityProbeEquipment::new("ProbeEvAbsentTelemetry", 0)
                .with_end_use(EndUse::EV)
                .with_published_soc(0.5),
        ))
        .expect("probe must register");
    let err = dwelling
        .step()
        .expect_err("absent static EV telemetry must fail the step loudly");
    let msg = err.to_string();
    assert!(
        msg.contains("ev_capacity_degraded") && msg.contains("ProbeEvAbsentTelemetry"),
        "the failure must name the check and the equipment, got: {msg}"
    );
}

/// The same monitor-blindness class, one gate up in the same invariant
/// loop: `check_soc` silently skipped when an EV-end-use equipment's
/// core-output SOC was absent — reachable only through a wiring defect
/// (every real EV and Battery publishes SOC at every step, and
/// `Soc::try_from(..).ok()` additionally nulls it on a non-finite
/// internal SOC), so the SOC-bounds monitor was blind exactly on the
/// observation failure it existed to catch — the anti-pattern the
/// adjacent `ev_capacity_degraded` gate was made loud against. The four
/// static capacity keys are present and consistent so that adjacent gate
/// passes, isolating the `soc_bounds` behavior under test.
///
/// The gate is now loud (the fix landed: the `let-else` at the top of the
/// loop fails the step naming the check and the equipment); this gate
/// was the round-9 routed finding's must_fail form and flipped green
/// with the fix, per the constitution's self-destructing-annotation
/// discipline. It asserts the full loud-error contract — the failure
/// names the check (`soc_bounds`) and the offending equipment, mirroring
/// the sibling `ev_capacity_degraded` gate's message assertion — so a
/// regression to a generic error message goes red here too.
#[test]
fn ev_soc_check_fails_loudly_when_core_output_soc_is_absent() {
    let mut dwelling = build_fixture_dwelling(&fixture_dir(), false, None);
    let mut probe = IdentityProbeEquipment::new("ProbeEvAbsentSoc", 0).with_end_use(EndUse::EV);
    // Consistent values — usable = rated × (1 − fade) × derate(25 °C), the
    // exact pass condition of `check_ev_capacity_degraded` — so the
    // adjacent gate stays green and the only behavior under test is the
    // `soc_bounds` loud failure on the absent SOC.
    let derate = hares_equipment::battery::CapacityDerateModel::default().evaluate(25.0);
    probe.telemetry.insert("capacity_kwh", 60.0 * derate);
    probe.telemetry.insert("capacity_kwh_rated", 60.0);
    probe.telemetry.insert("capacity_fade_pct", 0.0);
    probe.telemetry.insert("battery_temp_c", 25.0);
    dwelling
        .add_equipment(Box::new(probe))
        .expect("probe must register");
    let err = dwelling
        .step()
        .expect_err("absent core-output SOC must fail the step loudly");
    let msg = err.to_string();
    assert!(
        msg.contains("soc_bounds") && msg.contains("ProbeEvAbsentSoc"),
        "the failure must name the check and the equipment, got: {msg}"
    );
}
