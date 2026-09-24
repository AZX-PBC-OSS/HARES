//! Equipment identity — misattribution and diagnosis attacks at the
//! identity-refresh and entrance boundaries.
//!
//! These tests attack the identity contract's boundaries through the public
//! API: an equipment whose name collides with an aggregate output column
//! (the exact-membership `in_schema` predicate's blind spot), and the
//! diagnostic content of the malformed-`equipment_id` rejection.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::{Dwelling, DwellingConfig};
use hares_equipment::Equipment;
use hares_io::OutputFormat;
use hares_types::{
    ControlCapabilities, CoreCapabilities, CoreOutput, EndUse, EnvironmentState,
    EquipmentDescriptor, EquipmentId, FuelType, HaresError, OperatingMode, PortSlots, Telemetry,
    TelemetryField,
};

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

/// Minimal inert equipment probe: no ports, draws no power, no control
/// capabilities — the cheapest equipment that can legally join a dwelling.
struct InertProbe {
    descriptor: EquipmentDescriptor,
    telemetry: Telemetry,
    core_output: CoreOutput,
}

impl InertProbe {
    fn new(name: &str, id: u32) -> Self {
        Self {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(id),
                name: name.to_string(),
                end_use: EndUse::OTHER,
                equipment_type: Cow::Borrowed("InertProbe"),
                zone: None,
                fuel: FuelType::Electric,
                stage: hares_types::ExecutionStage::Independent,
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
        }
    }
}

impl Equipment for InertProbe {
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

/// Reads the mean and max of a CSV column over `take_rows` rows after
/// skipping `skip_rows` lines (header first, then day windows).
fn column_stats(csv: &Path, column: &str, skip_rows: usize, take_rows: usize) -> (f64, f64, usize) {
    let contents = std::fs::read_to_string(csv).expect("read output CSV");
    let mut lines = contents.lines();
    let header: Vec<&str> = lines.next().expect("header").split(',').collect();
    let idx = header
        .iter()
        .position(|c| *c == column)
        .unwrap_or_else(|| panic!("column '{column}' missing; header: {header:?}"));
    let mut n = 0usize;
    let mut sum = 0.0f64;
    let mut max = f64::NEG_INFINITY;
    for line in lines.skip(skip_rows).take(take_rows) {
        if let Ok(v) = line.split(',').nth(idx).unwrap_or("").trim().parse::<f64>() {
            sum += v;
            max = max.max(v);
            n += 1;
        }
    }
    (if n == 0 { f64::NAN } else { sum / n as f64 }, max, n)
}

/// An equipment named after an aggregate output column ("Total") must never
/// claim that column. The schema's verbosity-0 base column
/// "Total Electric Power (kW)" is emitted at every verbosity, and the
/// column-map re-derivation's exact-membership check
/// (`contains_key("{name} Electric Power (kW)")`) matches it for an
/// equipment literally named "Total" — so `record_step` writes the
/// equipment's own power into the aggregate index *after* the true total,
/// silently replacing the dwelling's headline total with one equipment's
/// value from the add onward. Output must keep "missing values, never
/// misattributed ones".
#[test]
fn midrun_added_equipment_cannot_claim_the_aggregate_total_column() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let hpxml = write_fixture_hpxml(tmp.path());
    let output_path = tmp.path().join("aggregate_claim.csv");

    let mut dwelling = Dwelling::from_config(dwelling_config(
        &hpxml,
        sim_config(Duration::days(2), Some(output_path.clone()), 1),
    ))
    .expect("dwelling builds");

    // Day 1 with the full set: rows recorded, schema frozen.
    for _ in 0..96 {
        dwelling.step().expect("day-1 step");
    }

    // A unique, legal equipment name that exact-matches the aggregate
    // column's "{name} Electric Power (kW)" form.
    dwelling
        .add_equipment(Box::new(InertProbe::new("Total", 0)))
        .expect("mid-run add of an equipment named 'Total' must be accepted");
    dwelling.simulate().expect("day-2 simulate");

    // The streaming recorder flushes with the run; read both day windows
    // from the completed CSV (rows 1..96 = day 1, 97..192 = day 2).
    let (day1_mean, day1_max, day1_rows) =
        column_stats(&output_path, "Total Electric Power (kW)", 1, 96);
    assert_eq!(day1_rows, 96, "day 1 must have recorded 96 rows");
    assert!(
        day1_max > 0.1,
        "precondition: the aggregate total column must carry real data before \
         the add (day-1 mean {day1_mean}, max {day1_max})"
    );
    let (day2_mean, day2_max, day2_rows) =
        column_stats(&output_path, "Total Electric Power (kW)", 1 + 96, 96);
    assert!(
        day2_rows >= 95,
        "day 2 must have recorded ~96 parseable rows, got {day2_rows}"
    );
    assert!(
        day2_max > 0.1,
        "the aggregate 'Total Electric Power (kW)' column collapsed after an \
         equipment named 'Total' joined mid-run (day-2 mean {day2_mean}, max \
         {day2_max}): the column map's exact-membership check matched the \
         aggregate column as the equipment's own, and record_step overwrote \
         the true dwelling total with the equipment's value"
    );
}

/// The same collision through the OTHER path: an equipment named "Total"
/// added before any row is recorded. The schema rebuild (safe while
/// `total_rows == 0`) would emit the per-equipment column under the same
/// name as the level-0 aggregate — a duplicate field name that collapses
/// in the column index (last wins), so the aggregate column the header
/// advertises is never the one written. The aggregate must stay the true
/// dwelling total from the first row.
#[test]
fn prerecord_added_equipment_cannot_shadow_the_aggregate_total_column() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let hpxml = write_fixture_hpxml(tmp.path());
    let output_path = tmp.path().join("aggregate_shadow.csv");

    let mut dwelling = Dwelling::from_config(dwelling_config(
        &hpxml,
        sim_config(Duration::days(1), Some(output_path.clone()), 1),
    ))
    .expect("dwelling builds");

    // Before any recorded row: the schema rebuild incorporates the new
    // equipment — its per-equipment column name collides with the
    // level-0 aggregate.
    dwelling
        .add_equipment(Box::new(InertProbe::new("Total", 0)))
        .expect("add of an equipment named 'Total' before any recorded row must be accepted");
    dwelling.simulate().expect("day-1 simulate");

    let (mean, max, rows) = column_stats(&output_path, "Total Electric Power (kW)", 1, 96);
    assert!(
        rows >= 95,
        "day 1 must have recorded ~96 parseable rows, got {rows}"
    );
    assert!(
        max > 0.1,
        "the aggregate 'Total Electric Power (kW)' column must carry the true \
         dwelling total from the first row (mean {mean}, max {max}): the \
         per-equipment emission for an equipment named 'Total' duplicated the \
         aggregate's column name, and the collapsed index wrote the total \
         into the shadowed duplicate instead",
    );
}

/// The malformed-`equipment_id` rejection must diagnose the actual defect.
/// The pass leaves a malformed value untouched, the constructor's reader
/// maps it to the sentinel, and the entrance rejects id 0 with "its config
/// channel did not deliver one" — a false diagnosis when the channel
/// delivered a *malformed* value: the user is told to remove the field when
/// the real fix is to correct the value, and the received value is named
/// nowhere in the typed error (only in a tracing warning). The
/// constitution's loud-errors rule requires the error to name the field,
/// the value received, and the expected form.
#[test]
fn malformed_equipment_id_rejection_names_the_received_value() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let hpxml = write_fixture_hpxml(tmp.path());
    let config = dwelling_config(&hpxml, sim_config(Duration::days(1), None, 0));

    let mut blueprint =
        hares_core::dwelling::DwellingBlueprint::from_config(config).expect("blueprint");
    let mut spec = hares_io::EquipmentSpec {
        name: "Dehumidifier".to_string(),
        instance_name: None,
        fuel_type: FuelType::Electric,
        parameters: serde_json::Map::new(),
        zip_params: None,
        typed_config: None,
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
    };
    spec.parameters
        .insert("equipment_id".to_string(), serde_json::json!(-1));
    blueprint.add_equipment_spec(spec).expect("add spec");

    let err = hares_core::dwelling::DwellingBlueprint::build(blueprint)
        .map(|_| ())
        .expect_err("a malformed explicit equipment_id must fail the build");
    let msg = err.to_string();
    assert!(
        msg.contains("-1"),
        "the rejection must name the value received so the user can correct \
         it (the constitution's error-content rule), got: {msg}"
    );
    assert!(
        !msg.contains("did not deliver one"),
        "the rejection must not claim the config channel delivered nothing \
         when it delivered a malformed value — that misdirects the user to \
         delete the field instead of fixing the value, got: {msg}"
    );
}

/// The reserved-namespace protection must hold through the WHOLE output
/// chain, from birth: an equipment whose display name is "Total" (present
/// at initial schema build, not added mid-run) must not silence or corrupt
/// the aggregate "Total Electric Power (kW)" column over a real run. The
/// schema-emission level is pinned in `hares-io`'s own tests; this pins the
/// rest of the chain — name-keyed column index, the equipment column map's
/// reserved check, and `record_step`'s aggregate-then-per-equipment write
/// order — where a regression would silently replace the dwelling's
/// headline total with one equipment's value from row 1.
#[test]
fn equipment_named_total_at_build_time_never_silences_the_aggregate_total() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let hpxml = write_fixture_hpxml(tmp.path());
    let output_path = tmp.path().join("total_at_birth.csv");

    let mut blueprint = hares_core::dwelling::DwellingBlueprint::from_config(dwelling_config(
        &hpxml,
        sim_config(Duration::days(2), Some(output_path.clone()), 1),
    ))
    .expect("blueprint");
    let typed = hares_equipment::EquipmentConfig::with_payload(
        "Dehumidifier".to_string(),
        "Dehumidifier".to_string(),
        hares_equipment::ConfigPayload::Typed {
            type_name: "Dehumidifier".to_string(),
            version: 1,
            data: serde_json::json!({}),
        },
    );
    blueprint
        .add_equipment_spec(hares_io::EquipmentSpec {
            name: "Dehumidifier".to_string(),
            instance_name: Some("Total".to_string()),
            fuel_type: FuelType::Electric,
            parameters: serde_json::Map::new(),
            zip_params: None,
            typed_config: Some(typed),
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        })
        .expect("add spec");
    let mut dwelling = hares_core::dwelling::DwellingBlueprint::build(blueprint)
        .expect("a dwelling with an equipment named 'Total' must build");

    // Precondition: the shadowing equipment is really in the vector.
    assert!(
        dwelling
            .equipment()
            .iter()
            .any(|eq| eq.descriptor().name == "Total"),
        "precondition: the equipment named 'Total' must have assembled (a \
         non-critical init failure would drop it and make this test vacuous)"
    );

    dwelling.simulate().expect("2-day simulate");

    for (label, skip) in [("day 1", 1), ("day 2", 1 + 96)] {
        let (mean, max, rows) = column_stats(&output_path, "Total Electric Power (kW)", skip, 96);
        assert!(
            rows >= 95,
            "{label}: ~96 rows expected, got {rows} parseable"
        );
        assert!(
            max > 0.1,
            "{label}: the aggregate 'Total Electric Power (kW)' column is dead \
             (mean {mean}, max {max}) while an equipment named 'Total' is in the \
             dwelling — the reserved-namespace protection regressed somewhere in \
             the schema → column-index → column-map → record_step chain and the \
             equipment's own value is replacing the dwelling's total"
        );
    }
}
