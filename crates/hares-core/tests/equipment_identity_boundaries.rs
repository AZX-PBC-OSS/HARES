//! Equipment identity boundary attacks — dwelling-level, through the real
//! assembly and `add_equipment` entrances.
//!
//! These tests attack the identity contract at its boundaries rather than
//! restating its happy path: a mid-run add whose name collides by prefix
//! with another equipment's output columns, and the never-reused id
//! counter's behaviour at the `u32::MAX` boundary.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::{Dwelling, DwellingConfig};
use hares_equipment::Equipment;
use hares_io::OutputFormat;
use hares_types::{
    ControlCapabilities, CoreCapabilities, CoreOutput, ElectricPower, EndUse, EnvironmentState,
    EquipmentDescriptor, EquipmentId, FuelType, HaresError, OperatingMode, PortContribution,
    PortSlots, Telemetry, TelemetryField,
};
use serde_json::json;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// ResStock bldg0000007 fixture directory.
fn fixture_dir() -> PathBuf {
    project_root().join("tests/fixtures/resstock/2025.1/bldg0000007")
}

fn write_fixture_hpxml(dir: &Path) -> PathBuf {
    let src = std::fs::read_to_string(fixture_dir().join("home.xml")).expect("fixture home.xml");
    let path = dir.join("bldg0000007.xml");
    std::fs::write(&path, src).expect("write fixture HPXML");
    path
}

fn sim_config(
    duration: Duration,
    output_path: Option<PathBuf>,
    verbosity: u8,
) -> hares_core::SimulationConfig {
    hares_core::SimulationConfig {
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

fn dwelling_config(hpxml: &Path, sim: hares_core::SimulationConfig) -> DwellingConfig {
    DwellingConfig {
        hpxml_path: hpxml.to_path_buf(),
        schedule_path: fixture_dir().join("in.schedules.csv"),
        weather_path: project_root()
            .join("tests/fixtures/resstock/2025.1/weather/G1500030_2018.csv"),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: sim,
        overrides: None,
        bldg_id: 100,
        initialization_duration: None,
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    }
}

/// Minimal inert equipment probe: no ports, electric, no control
/// capabilities — the cheapest equipment that can legally join a dwelling.
struct BoundaryProbe {
    descriptor: EquipmentDescriptor,
    telemetry: Telemetry,
    core_output: CoreOutput,
    electric_load_kw: f64,
}

impl BoundaryProbe {
    fn new(name: &str, id: u32) -> Self {
        Self {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(id),
                name: name.to_string(),
                end_use: EndUse::OTHER,
                equipment_type: Cow::Borrowed("BoundaryProbe"),
                zone: None,
                fuel: FuelType::Electric,
                stage: hares_types::ExecutionStage::Independent,
                control_capabilities: ControlCapabilities::empty(),
                core_capabilities: CoreCapabilities::empty(),
                telemetry_fields: vec![TelemetryField {
                    name: "probe".to_string(),
                    unit: "-".to_string(),
                    description: "identity boundary probe".to_string(),
                }],
                zone_type: None,
            },
            telemetry: Telemetry::with_capacity(1),
            core_output: CoreOutput::default(),
            electric_load_kw: 0.0,
        }
    }

    /// A constant electric load: the core output claims it (with the
    /// matching core capability declared, the core-contract validator's
    /// requirement) and `step` deposits the port contribution, so the debug
    /// core/port electrical-consistency validator sees agreement.
    fn with_electric_load(mut self, kw: f64) -> Self {
        self.electric_load_kw = kw;
        self.descriptor.core_capabilities = CoreCapabilities::ELECTRIC;
        self.core_output.flows.electric_kw = Some(ElectricPower::Consumption(kw));
        self
    }
}

impl Equipment for BoundaryProbe {
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

    fn init(
        &mut self,
        _: &hares_equipment::EquipmentConfig,
        _: &EnvironmentState,
    ) -> Result<(), HaresError> {
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
        if self.electric_load_kw > 0.0 {
            ports.accumulate(&PortContribution::Electrical {
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

/// A mid-run add whose name is a word-boundary prefix of another equipment's
/// name ("Clothes" vs "Clothes Washer") must not panic. The column-map
/// re-derivation's `in_schema` check matches any frozen column by `"{name} "`
/// prefix, so the prefix-colliding name is misclassified as schema-known and
/// its (nonexistent) columns are demanded — a debug panic on a legitimate
/// mid-run add, the exact failure the scoping was introduced to prevent.
#[test]
fn midrun_add_whose_name_prefix_matches_another_equipment_does_not_panic() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let hpxml = write_fixture_hpxml(tmp.path());
    let output_path = tmp.path().join("prefix_collision.csv");

    let mut dwelling = Dwelling::from_config(dwelling_config(
        &hpxml,
        sim_config(Duration::days(2), Some(output_path), 1),
    ))
    .expect("dwelling builds");

    // Precondition: the fixture carries a multi-word equipment name whose
    // first word is free — the word-boundary prefix collision this test
    // attacks ("Clothes" against "Clothes Washer ...", etc.).
    let colliding_name = dwelling
        .equipment()
        .iter()
        .map(|eq| eq.descriptor().name.clone())
        .filter(|name| name.contains(' '))
        .map(|name| name.split(' ').next().expect("non-empty").to_string())
        .find(|first_word| {
            !dwelling
                .equipment()
                .iter()
                .any(|eq| eq.descriptor().name == *first_word)
        })
        .expect("fixture precondition: a multi-word equipment name with a free first word");

    // One day with the full set: rows are recorded, the schema freezes.
    for _ in 0..96 {
        dwelling.step().expect("day-1 step");
    }

    // The prefix-colliding add is legitimate (the name is unique and the
    // probe declares no ports) — it must succeed without panicking.
    dwelling
        .add_equipment(Box::new(BoundaryProbe::new(&colliding_name, 0)))
        .unwrap_or_else(|e| {
            panic!(
                "mid-run add of '{colliding_name}' (word-boundary prefix of an existing \
                 equipment's name) must succeed: {e}"
            )
        });
    // Day 2 must complete: the re-derived column maps must not demand
    // columns the frozen schema never allocated.
    dwelling.simulate().expect("day-2 simulate");
}

/// The never-reused id counter must never hand out an id already in the
/// equipment vector — including at the `u32::MAX` boundary. An explicit
/// `EquipmentId(u32::MAX)` advances the counter with `saturating_add`, so a
/// saturated counter re-issues the in-use MAX id to a later auto-assign:
/// a silent duplicate, the exact identity collapse this initiative's
/// contract exists to make impossible.
#[test]
fn auto_assignment_never_reissues_an_in_use_id_at_the_u32_max_boundary() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let hpxml = write_fixture_hpxml(tmp.path());

    let mut dwelling = Dwelling::from_config(dwelling_config(
        &hpxml,
        sim_config(Duration::days(1), None, 0),
    ))
    .expect("dwelling builds");

    dwelling
        .add_equipment(Box::new(BoundaryProbe::new("ProbeMax", u32::MAX)))
        .expect("explicit u32::MAX id must be accepted (unique in the vector)");
    dwelling
        .add_equipment(Box::new(BoundaryProbe::new("ProbeAuto", 0)))
        .expect("unassigned probe must be auto-assigned");

    let max_users: Vec<&str> = dwelling
        .equipment()
        .iter()
        .filter(|eq| eq.descriptor().id == EquipmentId(u32::MAX))
        .map(|eq| eq.descriptor().name.as_str())
        .collect();
    assert_eq!(
        max_users.len(),
        1,
        "auto-assignment re-issued the in-use id u32::MAX to '{}' — the id \
         counter saturated and a silent identity duplicate entered the \
         dwelling (every EquipmentId-keyed map collapses onto it)",
        max_users.join("', '")
    );
}

/// The other edge of the `u32::MAX` boundary: when the counter is pinned at
/// MAX and MAX itself is in use (an explicit MAX-1 advances the counter to
/// MAX; an explicit MAX cannot advance it past itself), auto-assignment has
/// no free id left and must fail with the loud typed exhaustion error —
/// never a silent duplicate.
#[test]
fn auto_assignment_at_an_exhausted_id_counter_fails_loudly() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let hpxml = write_fixture_hpxml(tmp.path());

    let mut dwelling = Dwelling::from_config(dwelling_config(
        &hpxml,
        sim_config(Duration::days(1), None, 0),
    ))
    .expect("dwelling builds");

    // MAX-1 advances the counter to MAX (checked add of MAX-1 + 1).
    dwelling
        .add_equipment(Box::new(BoundaryProbe::new(
            "ProbeMaxMinusOne",
            u32::MAX - 1,
        )))
        .expect("explicit u32::MAX - 1 must be accepted (unique)");
    // MAX is accepted (unique in the vector) but cannot advance the counter
    // past itself — the counter stays pinned at the in-use MAX.
    dwelling
        .add_equipment(Box::new(BoundaryProbe::new("ProbeMax", u32::MAX)))
        .expect("explicit u32::MAX must be accepted (unique)");

    let err = dwelling
        .add_equipment(Box::new(BoundaryProbe::new("ProbeExhausted", 0)))
        .map(|_| ())
        .expect_err("auto-assignment with every remaining id in use must fail loudly");
    let msg = err.to_string();
    assert!(
        msg.contains("ProbeExhausted"),
        "the exhaustion error must name the equipment, got: {msg}"
    );
    assert!(
        msg.contains("no free equipment id") && msg.contains("u32::MAX"),
        "the exhaustion error must name the violation and the boundary, got: {msg}"
    );
    assert!(
        !dwelling
            .equipment()
            .iter()
            .any(|eq| eq.descriptor().name == "ProbeExhausted"),
        "the rejected equipment must not join the vector"
    );
    // The two explicit boundary ids remain the only holders of their ids.
    for (name, id) in [("ProbeMaxMinusOne", u32::MAX - 1), ("ProbeMax", u32::MAX)] {
        let holders: Vec<&str> = dwelling
            .equipment()
            .iter()
            .filter(|eq| eq.descriptor().id.0 == id)
            .map(|eq| eq.descriptor().name.as_str())
            .collect();
        assert_eq!(holders, [name], "id {id} must have exactly one holder");
    }
}

/// A malformed explicit `equipment_id` must fail the dwelling build loudly.
/// The assignment pass leaves the malformed value untouched (its contract),
/// no valid id reaches the constructor, and the assembly entrance must
/// reject the unassigned sentinel instead of the pre-fix behaviour — the
/// pass silently overwriting the malformed value with an assigned id and
/// the build succeeding. A hand-built typed Dehumidifier spec (all-optional
/// config, so it inits cleanly) carrying a negative id in its raw
/// parameters: the channel the pass reads first.
#[test]
fn malformed_explicit_equipment_id_fails_the_build_loudly() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let hpxml = write_fixture_hpxml(tmp.path());
    let config = dwelling_config(&hpxml, sim_config(Duration::days(1), None, 0));

    let mut blueprint =
        hares_core::dwelling::DwellingBlueprint::from_config(config).expect("blueprint");
    let typed = hares_equipment::EquipmentConfig::from_typed(
        "Dehumidifier".to_string(),
        "Dehumidifier".to_string(),
        serde_json::from_value::<hares_equipment::DehumidifierConfig>(json!({}))
            .expect("minimal all-optional DehumidifierConfig"),
    )
    .expect("typed config");
    let mut spec = hares_io::EquipmentSpec {
        name: "Dehumidifier".to_string(),
        instance_name: None,
        fuel_type: FuelType::Electric,
        parameters: serde_json::Map::new(),
        zip_params: None,
        typed_config: Some(typed),
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
    };
    spec.parameters
        .insert("equipment_id".to_string(), json!(-1));
    blueprint.add_equipment_spec(spec).expect("add spec");

    let err = blueprint
        .build()
        .map(|_| ())
        .expect_err("a malformed explicit equipment_id must fail the build");
    let msg = err.to_string();
    assert!(
        msg.contains("Dehumidifier"),
        "the rejection must name the equipment, got: {msg}"
    );
    assert!(
        msg.contains("unassigned equipment id"),
        "the rejection must be the unassigned-id chain the pass's contract \
         promises (malformed left untouched → no valid id delivered → the \
         entrance rejects the sentinel), got: {msg}"
    );
}

/// The `in_schema` membership predicate's true branch at the mid-run
/// boundary: an equipment re-added under a name the frozen schema knows
/// (remove "Clothes Washer" after a recorded day, re-add a live load under
/// the same name) is schema-known, its columns resolve, and its values land
/// in its own name-keyed columns — the affirmative side of the exact-membership
/// fix. An overcorrection that classifies every mid-run join as
/// schema-unknown would silently drop its output (missing values) instead.
#[test]
fn midrun_readd_of_a_removed_name_reuses_its_frozen_columns() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let hpxml = write_fixture_hpxml(tmp.path());
    let output_path = tmp.path().join("readd_columns.csv");

    let mut dwelling = Dwelling::from_config(dwelling_config(
        &hpxml,
        sim_config(Duration::days(2), Some(output_path.clone()), 1),
    ))
    .expect("dwelling builds");

    // Day 1 with the full set (rows recorded; the schema freezes).
    for _ in 0..96 {
        dwelling.step().expect("day-1 step");
    }
    dwelling
        .remove_equipment("Clothes Washer")
        .expect("remove the washer mid-run");
    // Re-add a live 0.5 kW load under the removed name: the name is free
    // again, and the frozen schema still carries its columns.
    dwelling
        .add_equipment(Box::new(
            BoundaryProbe::new("Clothes Washer", 0).with_electric_load(0.5),
        ))
        .expect("re-adding the removed name mid-run must succeed");
    // Day 2 completes: the schema-known name's applicable columns resolve
    // (no panic), and its values land in its own columns.
    dwelling.simulate().expect("day-2 simulate");

    let contents = std::fs::read_to_string(&output_path).expect("read output CSV");
    let header: Vec<&str> = contents
        .lines()
        .next()
        .expect("header")
        .split(',')
        .collect();
    let washer_col = header
        .iter()
        .position(|c| *c == "Clothes Washer Electric Power (kW)")
        .unwrap_or_else(|| panic!("washer column missing; header: {header:?}"));
    let day2_kwh: f64 = contents
        .lines()
        .skip(1 + 96)
        .filter_map(|l| l.split(',').nth(washer_col))
        .filter_map(|v| v.trim().parse::<f64>().ok())
        .map(|kw| kw * 0.25)
        .sum();
    assert!(
        day2_kwh > 1.0,
        "the re-added name's frozen columns must carry its output (0.5 kW → \
         ≈12 kWh/day) — a quiet column means the re-add was misclassified as \
         schema-unknown and its values silently dropped; got {day2_kwh} kWh"
    );
}

/// The malformed-id loud-rejection chain through the *typed* channel,
/// end-to-end: the pass leaves the typed payload's malformed value untouched,
/// and the dwelling build must fail loudly — whichever boundary catches it
/// (constructor deserialization, init, or the entrance's unassigned-id
/// rejection). The raw-channel chain is pinned above; this pins that the
/// second config channel cannot reach a silently-built dwelling either —
/// including the silent-skip face, where a non-critical equipment whose init
/// fails is dropped with only a warning and the build *succeeds*.
#[test]
fn malformed_equipment_id_in_the_typed_payload_fails_the_build_loudly() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let hpxml = write_fixture_hpxml(tmp.path());
    let config = dwelling_config(&hpxml, sim_config(Duration::days(1), None, 0));

    let mut blueprint =
        hares_core::dwelling::DwellingBlueprint::from_config(config).expect("blueprint");
    let typed = hares_equipment::EquipmentConfig::with_payload(
        "Dehumidifier".to_string(),
        "Dehumidifier".to_string(),
        hares_equipment::ConfigPayload::Typed {
            type_name: "Dehumidifier".to_string(),
            version: 1,
            data: json!({ "equipment_id": -1 }),
        },
    );
    let spec = hares_io::EquipmentSpec {
        name: "Dehumidifier".to_string(),
        instance_name: None,
        fuel_type: FuelType::Electric,
        parameters: serde_json::Map::new(),
        zip_params: None,
        typed_config: Some(typed),
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
    };
    blueprint.add_equipment_spec(spec).expect("add spec");

    // GREEN since the fix that moved the entrance-1 id-0 rejection ahead
    // of `init` (and therefore ahead of the non-critical init-failure
    // skip): pre-fix, the malformed typed id failed the dehumidifier's
    // init and the skip silently dropped the equipment — the build
    // returned `Ok` with the dehumidifier absent. The assertion pins the
    // post-fix contract: the build fails loudly with the same
    // unassigned-id error as the raw channel (both channels construct
    // with the sentinel; the pre-init check rejects it through the shared
    // error helper).
    let err = blueprint.build().map(|_| ()).expect_err(
        "a malformed equipment_id in the typed payload must fail the build \
         loudly — never a silently-built dwelling missing the equipment (the \
         raw channel already fails loudly with the entrance's unassigned-id \
         rejection; the typed channel must not silently drop the equipment \
         through the non-critical init-failure skip)",
    );
    let msg = err.to_string();
    assert!(
        msg.contains("Dehumidifier"),
        "the rejection must name the equipment, got: {msg}"
    );
    assert!(
        msg.contains("unassigned equipment id"),
        "both config channels must produce the same unassigned-id rejection \
         (the shared error helper), got: {msg}"
    );
    assert!(
        msg.contains("-1"),
        "the typed channel's malformed-value diagnosis must also name the \
         received value (the same four-way diagnostic as the raw channel — \
         the classifier reads both channels), got: {msg}"
    );
}

/// The tolerance design the identity checks narrowly scope around: a
/// NON-identity init failure on non-critical equipment is still skipped
/// with a warning — the build succeeds with the equipment absent, not
/// fatally rejected. The pre-init identity check must not overcorrect into
/// rejecting valid ids (this spec carries a valid explicit id 7, which
/// must pass it), and the skip must not have been broken or made fatal by
/// the check's insertion directly above it.
#[test]
fn noncritical_init_failure_is_skipped_not_fatal_after_the_identity_checks() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let hpxml = write_fixture_hpxml(tmp.path());
    let config = dwelling_config(&hpxml, sim_config(Duration::days(1), None, 0));

    let mut blueprint =
        hares_core::dwelling::DwellingBlueprint::from_config(config).expect("blueprint");
    // A valid explicit id (passes the pre-init identity check) plus a
    // non-identity field init cannot accept (a string where an f64 is
    // required): construction defers the parse error to init, init fails,
    // and the dehumidifier is non-critical — skipped with a warning.
    let typed = hares_equipment::EquipmentConfig::with_payload(
        "Dehumidifier".to_string(),
        "Dehumidifier".to_string(),
        hares_equipment::ConfigPayload::Typed {
            type_name: "Dehumidifier".to_string(),
            version: 1,
            data: json!({ "equipment_id": 7, "capacity_liters_per_day": "not a number" }),
        },
    );
    let spec = hares_io::EquipmentSpec {
        name: "Dehumidifier".to_string(),
        instance_name: None,
        fuel_type: FuelType::Electric,
        parameters: serde_json::Map::new(),
        zip_params: None,
        typed_config: Some(typed),
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
    };
    blueprint.add_equipment_spec(spec).expect("add spec");

    let dwelling = blueprint.build().expect(
        "a non-identity init failure on non-critical equipment must be \
             skipped, not fatal",
    );
    assert!(
        !dwelling
            .equipment()
            .iter()
            .any(|eq| eq.descriptor().name == "Dehumidifier"),
        "the init-failing non-critical equipment must be absent from the \
             built dwelling (skipped with a warning)"
    );
    assert!(
        dwelling.equipment().len() > 1,
        "the rest of the dwelling must have assembled normally"
    );
}
