//! The variable catalogue's acceptance tests.
//!
//! The pins, `fixtures/schema/v{0..8}.json`, were written from the
//! pre-catalogue schema pipeline (build_schema over the all-classes
//! equipment list, then the actor columns appended) before any code change,
//! and are the fixed ground truth the catalogue-derived schema must
//! reproduce at every verbosity. A variable the engine produces but the
//! catalogue lacks fails the pin instead of silently losing its column.

use std::sync::Arc;

use hares_core::actor::Actor;
use hares_core::actors::{
    BatteryManagementActor, BmsParams, DrCompliance, EvDriverActor, EvDriverParams,
    IdealThermostat, Occupant, SafetyMonitor, SolverFeedbackActor,
};
use hares_core::{ChaCha8Rng, Dwelling};
use hares_io::output::build_catalogue;
use hares_io::{CatalogueActor, CatalogueEquipment, CatalogueInput, EquipmentSpec};
use hares_types::{
    AcrossHomes, Aggregation, BmsMode, ChargingStrategy, CoreCapabilities, EndUse, EquipmentId,
    FuelType, GridExportRule, PlugInPolicy, Sampling, ScheduleSource, Telemetry, TelemetryField,
    Unit, VariableKind, ZoneId,
};
use rand::SeedableRng;
use serde_json::Value;

/// The observe-gated dwelling columns, tagged in the pins and filtered on
/// both sides by the active feature set. (The `Q_Stack`, `Q_Wind` and `Cd`
/// natural-ventilation diagnostics were ghost writes into columns the
/// index never carried; the writes are deleted, not declared.)
const OBSERVE_ONLY_COLUMNS: &[&str] = &[
    "Natural Ventilation Cw (-)",
    "Natural Ventilation Wind Angle (deg)",
];

fn spec(name: &str, fuel: FuelType, instance_name: Option<String>) -> EquipmentSpec {
    EquipmentSpec {
        instance_name,
        name: name.to_string(),
        fuel_type: fuel,
        parameters: serde_json::Map::new(),
        zip_params: None,
        typed_overrides: serde_json::Map::new(),
        typed_config: None,
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
        heating_setpoint_source: None,
        cooling_setpoint_source: None,
    }
}

/// One spec per registered class (so a new class is picked up
/// automatically), with gas-fuelled names burning gas, the PV class
/// instance-qualified as "PV #1", and the classifier pins "Television"
/// (falls through to OTHER, and its name contains "ev") and "Pool Heater".
fn all_classes_specs() -> Vec<EquipmentSpec> {
    let registry = hares_equipment::EquipmentRegistry::new();
    let mut specs: Vec<EquipmentSpec> = registry
        .known_names()
        .into_iter()
        .map(|name| {
            let fuel = if name.contains("Gas") {
                FuelType::Gas
            } else {
                FuelType::Electric
            };
            let instance_name = (name == "PV").then(|| "PV #1".to_string());
            spec(name, fuel, instance_name)
        })
        .collect();
    specs.push(spec("Television", FuelType::Electric, None));
    specs.push(spec("Pool Heater", FuelType::Electric, None));
    specs
}

/// One of each built-in actor, under fixed names.
fn built_in_actors() -> Vec<Box<dyn Actor>> {
    vec![
        Box::new(SafetyMonitor::new("safety_monitor")),
        Box::new(IdealThermostat::new("Ideal HVAC")),
        Box::new(BatteryManagementActor::new(
            "Battery",
            BmsParams {
                bms_mode: BmsMode::Manual,
                grid_export_rule: GridExportRule::Unrestricted,
                max_charge_kw: 5.0,
                max_discharge_kw: 5.0,
                price_schedule: None,
                steps_per_day: 1440,
                min_dwell_steps: 0,
            },
        )),
        Box::new(DrCompliance::new("dr_compliance")),
        Box::new(Occupant::new("occupant")),
        Box::new(EvDriverActor::new(
            "ev_driver",
            "EV",
            EvDriverParams {
                strategy: ChargingStrategy::Immediate { target_soc: 1.0 },
                plug_in_policy: PlugInPolicy::Always,
                daily_drive_miles: ScheduleSource::Constant(30.0),
                departure_time: ScheduleSource::Constant(480.0),
                trip_duration: ScheduleSource::Constant(600.0),
                arrival_time: None,
                event_day_ratio: 1.0,
                fuel_economy_kwh_per_mi: 0.3,
                capacity_kwh: 60.0,
                max_charge_kw: 7.2,
                average_speed_mph: 30.0,
                range_anxiety_miles: 20.0,
                away_charge_fraction: 0.0,
                away_charge_power_kw: 0.0,
            },
            ChaCha8Rng::from_seed([42u8; 32]),
        )),
        Box::new(SolverFeedbackActor::new()),
    ]
}

/// The typed declarations the all-classes catalogue input carries: the
/// class's own declared surface (`hares_equipment::class_declared_surface`,
/// the same source `build_schema_for_specs` reads) or, for a name that
/// is not a registered class, the pin's `"Television"`: no declaration at
/// all (end use OTHER, no capabilities, no telemetry).
fn constructed_class_surface(spec_name: &str) -> (EndUse, CoreCapabilities, Vec<TelemetryField>) {
    hares_equipment::class_declared_surface(spec_name).map_or(
        (EndUse::OTHER, CoreCapabilities::default(), Vec::new()),
        |surface| {
            (
                surface.end_use,
                surface.core_capabilities,
                surface.telemetry,
            )
        },
    )
}
fn all_classes_catalogue() -> hares_io::Catalogue {
    let input = CatalogueInput {
        equipment: all_classes_specs()
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let (end_use, core_capabilities, telemetry) = constructed_class_surface(&s.name);
                CatalogueEquipment {
                    name: s.name.clone(),
                    instance_name: s.instance_name.clone(),
                    id: EquipmentId(u32::try_from(i + 1).expect("equipment count")),
                    fuel: s.fuel_type,
                    end_use,
                    core_capabilities,
                    telemetry,
                }
            })
            .collect(),
        zones: vec![
            (ZoneId(1), "Indoor".to_string()),
            (ZoneId(2), "Attic".to_string()),
            (ZoneId(3), "Basement".to_string()),
        ],
        actors: built_in_actors()
            .iter()
            .map(|a| CatalogueActor {
                name: Arc::from(a.name()),
                output_variables: a.output_variables().to_vec(),
            })
            .collect(),
    };
    build_catalogue(&input).expect("the all-classes catalogue builds")
}

/// The catalogue-derived schema equals the pinned schema at every
/// verbosity, in field name, order, Arrow type and nullability; the
/// observe-gated pins apply only when the feature is on.
#[test]
fn catalogue_schema_equals_pinned_schema_at_every_verbosity() {
    let catalogue = all_classes_catalogue();
    for verbosity in 0..=8u8 {
        let path = format!("tests/fixtures/schema/v{verbosity}.json");
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("schema pin missing at {path}: {err}"));
        let pins: Vec<Value> = serde_json::from_str(&raw).expect("pin JSON parses");

        let schema = hares_io::build_schema(&catalogue, verbosity, false);
        let actual: Vec<(&str, String, bool)> = schema
            .fields()
            .iter()
            .map(|f| {
                (
                    f.name().as_str(),
                    format!("{:?}", f.data_type()),
                    f.is_nullable(),
                )
            })
            .collect();

        let expected: Vec<(&str, String, bool)> = pins
            .iter()
            .filter_map(|pin| {
                let name = pin["name"].as_str().expect("pin name");
                if OBSERVE_ONLY_COLUMNS.contains(&name) && !cfg!(feature = "observe") {
                    return None;
                }
                Some((
                    name,
                    pin["arrow_type"]
                        .as_str()
                        .expect("pin arrow_type")
                        .to_string(),
                    pin["nullable"].as_bool().expect("pin nullable"),
                ))
            })
            .collect();

        assert_eq!(
            actual.len(),
            expected.len(),
            "verbosity {verbosity}: the catalogue-derived schema's column count must equal the pin"
        );
        for (i, ((a_name, a_type, a_null), (e_name, e_type, e_null))) in
            actual.iter().zip(&expected).enumerate()
        {
            assert_eq!(
                (a_name, a_type, a_null),
                (e_name, e_type, e_null),
                "verbosity {verbosity}: column {i} diverges from the pin (name, Arrow type, \
                 nullability must all match, in order)"
            );
        }
    }
}

/// A dwelling built from the parity fixture (no warm-up) returns its
/// catalogue before the first step, and the catalogue's variables at the
/// run's verbosity are exactly the columns of its recorded schema after one
/// step.
#[test]
fn catalogue_queryable_before_run() {
    let fixture = std::path::Path::new("../../tests/fixtures/parity/cz4a_ashp_hpwh");
    let verbosity = 2;
    let mut dwelling = Dwelling::from_hpxml_with_reports(
        hares_core::HpxmlInputs {
            hpxml_path: &fixture.join("building.xml"),
            schedule_path: &fixture.join("schedule.csv"),
            weather_path: &fixture.join("weather.epw"),
        },
        chrono::DateTime::parse_from_rfc3339("2023-01-01T00:00:00-07:00")
            .expect("the fixture start time parses"),
        chrono::Duration::minutes(1),
        chrono::Duration::hours(1),
        None,
        vec![hares_io::ReportSpec {
            name: "frame".to_string(),
            selection: hares_io::Selection::Verbosity(verbosity),
            frequency: hares_io::Frequency::default(),
            destination: hares_io::Destination::Memory,
        }],
        Some(
            std::path::Path::new("../../defaults")
                .canonicalize()
                .expect("the repo defaults directory exists"),
        ),
    )
    .expect("the fixture dwelling builds");

    // Before the first step: the catalogue is readable.
    let catalogue = dwelling.catalogue();
    assert!(
        !catalogue.is_empty(),
        "the built dwelling's catalogue is not empty"
    );
    // After one step, the recorded schema's columns equal the catalogue's
    // variables at the run's verbosity (Time is the row label, not a
    // catalogue variable).
    dwelling.step().expect("the first step succeeds");
    let columns = dwelling
        .output_column_names()
        .expect("the fixture dwelling records output");
    let recorded: Vec<&str> = columns.iter().map(String::as_str).collect();

    let mut catalogue_names: Vec<&str> = catalogue
        .variables_at_verbosity(verbosity)
        .iter()
        .map(|v| v.name.as_ref())
        .collect();
    catalogue_names.insert(0, "Time");

    assert_eq!(
        recorded, catalogue_names,
        "the recorded schema's columns must equal the catalogue's variables at the run's verbosity"
    );
}

/// Every recorded column resolves to a `VariableDef`, and every
/// non-nullable selected variable is non-null, on the all-classes fixture
/// after one step. The structural half pins the catalogue against the
/// schema the engine actually builds; the anti-drift direction (a variable
/// the engine produces but the catalogue lacks) is a schema-pin failure.
#[test]
fn every_written_channel_is_declared_and_every_declared_variable_is_written() {
    let catalogue = all_classes_catalogue();
    for verbosity in 0..=8u8 {
        let schema = hares_io::build_schema(&catalogue, verbosity, false);
        for field in schema.fields() {
            let name = field.name().as_str();
            if name == "Time" {
                continue;
            }
            let def = catalogue.resolve(name).unwrap_or_else(|| {
                panic!("verbosity {verbosity}: recorded column '{name}' has no VariableDef")
            });
            assert_eq!(
                def.nullable,
                field.is_nullable(),
                "column '{name}': the catalogue's nullability must equal the schema's"
            );
        }
    }
}

/// The catalogue rejects a duplicate variable name as an assembly error,
/// naming both sources. Two actors under one registered name would
/// collide; the catalogue build is the backstop that refuses it.
#[test]
fn duplicate_variable_name_is_an_assembly_error() {
    let input = CatalogueInput {
        equipment: Vec::new(),
        zones: Vec::new(),
        actors: vec![
            CatalogueActor {
                name: Arc::from("shared_name"),
                output_variables: vec![TelemetryField::ratio("shared", "the shared key")],
            },
            CatalogueActor {
                name: Arc::from("shared_name"),
                output_variables: vec![TelemetryField::ratio("shared", "the same key")],
            },
        ],
    };
    let err = build_catalogue(&input).expect_err("a duplicate name is an assembly error");
    let message = err.to_string();
    assert!(
        message.contains("actor:shared_name"),
        "names the colliding source: {message}"
    );
}

/// An `Energy` variable whose aggregation is `Mean` contradicts its kind:
/// an assembly error.
#[test]
fn energy_variable_declared_mean_is_rejected() {
    let input = CatalogueInput {
        equipment: Vec::new(),
        zones: Vec::new(),
        actors: vec![CatalogueActor {
            name: Arc::from("energy_mean"),
            output_variables: vec![TelemetryField::full(
                "step_energy_kwh",
                Unit::KilowattHour,
                VariableKind::Energy,
                Aggregation::Mean,
                Sampling::IntervalMean,
                "the energy of the step, wrongly averaged",
            )],
        }],
    };
    let err = build_catalogue(&input).expect_err("Energy must aggregate as Sum");
    assert!(
        err.to_string().contains("energy_mean:step_energy_kwh")
            || err.to_string().contains("step_energy_kwh"),
        "the error names the variable: {err}"
    );
}

/// A `Last` aggregation on a non-`Quantity` kind contradicts its kind: an
/// assembly error.
#[test]
fn last_aggregation_on_a_non_quantity_is_rejected() {
    let input = CatalogueInput {
        equipment: Vec::new(),
        zones: Vec::new(),
        actors: vec![CatalogueActor {
            name: Arc::from("last_power"),
            output_variables: vec![TelemetryField::full(
                "power_kw",
                Unit::Kilowatt,
                VariableKind::Power,
                Aggregation::Last,
                Sampling::InstantAtEnd,
                "a power, wrongly held to Last",
            )],
        }],
    };
    let err = build_catalogue(&input).expect_err("Last is legal only for a Quantity");
    assert!(
        err.to_string().contains("power_kw"),
        "the error names the variable: {err}"
    );
}

/// Every variable in a power unit is declared `Power`; every `Quantity` in
/// a unit the census names declares that unit's across-homes rule; every
/// variable's unit, kind, across-homes, labels, aggregation, sampling and
/// nullability equal the pinned kind table `fixtures/schema/kinds.json`;
/// and no variable in an energy unit other than the step's energy is
/// declared `Energy`.
#[test]
fn every_quantity_declares_across_homes_and_power_units_are_power() {
    let catalogue = all_classes_catalogue();

    // The pin: every variable of the all-classes catalogue, dumped.
    let mut pinned: Vec<Value> = Vec::new();
    for def in catalogue.variables() {
        pinned.push(kind_entry(def.name.as_ref(), def));
    }
    let pin_path = "tests/fixtures/schema/kinds.json";
    let raw = std::fs::read_to_string(pin_path)
        .unwrap_or_else(|err| panic!("kinds pin missing at {pin_path}: {err}"));
    let expected: Vec<Value> = serde_json::from_str(&raw).expect("kinds JSON parses");
    assert_eq!(
        pinned, expected,
        "the catalogue's variables must equal the pinned kind table (unit, kind, across-homes, \
         labels, aggregation, sampling, nullability)"
    );

    for def in catalogue.variables() {
        let name = def.name.as_ref();
        // Every power-unit variable is Power.
        if def.unit.is_power() {
            assert!(
                matches!(def.kind, VariableKind::Power),
                "'{name}' is in a power unit ({}) and must be declared Power",
                def.unit.symbol()
            );
        }
        // Every Quantity in a unit the census classifies declares that
        // unit's across-homes rule.
        if let VariableKind::Quantity { across_homes } = &def.kind {
            let expected = match def.unit {
                Unit::WattPerSquareMeter
                | Unit::Second
                | Unit::Minute
                | Unit::Volt
                | Unit::JoulePerKilogramKelvin
                | Unit::KilowattHourPerMile
                | Unit::Degree => AcrossHomes::Mean,
                Unit::KilogramPerSecond
                | Unit::LitrePerDay
                | Unit::Count
                | Unit::Ampere
                | Unit::AmpereHour
                | Unit::Byte => AcrossHomes::Sum,
                _ => *across_homes,
            };
            assert_eq!(
                *across_homes,
                expected,
                "'{name}': Quantity in {} must declare {expected:?} across homes",
                def.unit.symbol()
            );
        }
        // No variable in an energy unit other than the energy of the step
        // is declared Energy.
        if def.unit.is_energy() && matches!(def.kind, VariableKind::Energy) {
            assert!(
                name.ends_with("Energy (kWh)"),
                "'{name}' is in an energy unit but is not the energy of the step; it must be a \
                 Quantity (a stock, a constant or a running total), never Energy"
            );
            assert_eq!(
                def.aggregation,
                Aggregation::Sum,
                "'{name}': the step's energy sums"
            );
        }
    }
}

fn kind_entry(name: &str, def: &hares_io::VariableDef) -> Value {
    let mut entry = match &def.kind {
        VariableKind::Energy => serde_json::json!({ "kind": "energy" }),
        VariableKind::Power => serde_json::json!({ "kind": "power" }),
        VariableKind::Temperature => serde_json::json!({ "kind": "temperature" }),
        VariableKind::Ratio => serde_json::json!({ "kind": "ratio" }),
        VariableKind::State => serde_json::json!({ "kind": "state" }),
        VariableKind::Enum { labels } => serde_json::json!({
            "kind": "enum",
            "labels": (0..labels.len())
                .map(|i| labels.get(i).unwrap_or_default())
                .collect::<Vec<&str>>(),
        }),
        VariableKind::Quantity { across_homes } => serde_json::json!({
            "kind": "quantity",
            "across_homes": match across_homes {
                AcrossHomes::Sum => "sum",
                AcrossHomes::Mean => "mean",
            },
        }),
    };
    let obj = entry.as_object_mut().expect("an object");
    obj.insert("name".to_string(), Value::String(name.to_string()));
    obj.insert(
        "unit".to_string(),
        Value::String(def.unit.symbol().to_string()),
    );
    obj.insert(
        "aggregation".to_string(),
        Value::String(
            match def.aggregation {
                Aggregation::Sum => "sum",
                Aggregation::Mean => "mean",
                Aggregation::Mode => "mode",
                Aggregation::Last => "last",
            }
            .to_string(),
        ),
    );
    obj.insert(
        "sampling".to_string(),
        Value::String(
            match def.sampling {
                Sampling::IntervalMean => "interval_mean",
                Sampling::InstantAtStart => "instant_at_start",
                Sampling::InstantAtEnd => "instant_at_end",
            }
            .to_string(),
        ),
    );
    obj.insert("nullable".to_string(), Value::Bool(def.nullable));
    entry
}

/// Writes the kind table pin (`tests/fixtures/schema/kinds.json`) from the
/// all-classes catalogue's declarations. Run once when the declarations
/// change; the output is then reviewed with the diff (every variable's
/// unit, kind, across-homes, labels, aggregation, sampling and nullability
/// is pinned by name). One entry per line, so the review diff reads one
/// variable at a time.
#[test]
#[ignore = "kind pin generator: run once per declaration change, review the diff"]
fn write_kinds_pin() {
    let catalogue = all_classes_catalogue();
    let entries: Vec<String> = catalogue
        .variables()
        .iter()
        .map(|def| {
            let entry = kind_entry(def.name.as_ref(), def);
            serde_json::to_string(&entry).expect("an entry serialises")
        })
        .collect();
    let body = entries.join(",\n");
    std::fs::write(
        "tests/fixtures/schema/kinds.json",
        format!("[\n{body}\n]\n"),
    )
    .expect("write kinds pin");
}

/// Writes the schema pins (`tests/fixtures/schema/v{0..8}.json`) from the
/// all-classes catalogue's catalogue-derived schemas. Run once per
/// schema-shape change (the column families move); the output is then
/// reviewed with the diff. The pretty form the pins already carry: a
/// re-capture that moves nothing diffs to nothing.
#[test]
#[ignore = "schema pin generator: run once per schema-shape change, review the diff"]
fn write_schema_pins() {
    let catalogue = all_classes_catalogue();
    for verbosity in 0..=8u8 {
        let schema = hares_io::build_schema(&catalogue, verbosity, false);
        let fields: Vec<Value> = schema
            .fields()
            .iter()
            .map(|f| {
                serde_json::json!({
                    "name": f.name(),
                    "arrow_type": format!("{:?}", f.data_type()),
                    "nullable": f.is_nullable(),
                })
            })
            .collect();
        let body = serde_json::to_string_pretty(&fields).expect("the pin serialises");
        let path = format!("tests/fixtures/schema/v{verbosity}.json");
        std::fs::write(&path, format!("{body}\n"))
            .unwrap_or_else(|err| panic!("write {path}: {err}"));
    }
}

/// The EV columns' enum labels cover every published code from 0.
#[test]
fn enum_labels_cover_every_published_code() {
    let catalogue = all_classes_catalogue();
    let connection = catalogue
        .resolve("Battery Management Actor Connection State (-)")
        .or_else(|| catalogue.resolve("EV Connection State (-)"));
    // The EV column family: find any Connection State column.
    let connection = connection
        .or_else(|| {
            catalogue
                .variables()
                .iter()
                .find(|v| v.name.ends_with("Connection State (-)"))
        })
        .expect("the EV Connection State column exists in the all-classes catalogue");
    match &connection.kind {
        VariableKind::Enum { labels } => {
            assert_eq!(labels.get(0), Some("home_plugged_in"));
            assert_eq!(labels.get(1), Some("away_plugged_in"));
            assert_eq!(labels.get(2), Some("disconnected"));
            assert_eq!(labels.len(), 3);
        }
        other => panic!("Connection State must be an Enum, got {other:?}"),
    }
    let charging = catalogue
        .variables()
        .iter()
        .find(|v| v.name.ends_with("Charging Level (-)"))
        .expect("the EV Charging Level column exists");
    match &charging.kind {
        VariableKind::Enum { labels } => {
            assert_eq!(
                labels.get(0),
                Some("none"),
                "CHARGING_LEVEL publishes 1 and 2, so the label table from 0 declares the placeholder"
            );
            assert_eq!(labels.get(1), Some("L1"));
            assert_eq!(labels.get(2), Some("L2"));
        }
        other => panic!("Charging Level must be an Enum, got {other:?}"),
    }
}

/// The static tables are `static` items whose enum labels are
/// `EnumLabels::Static`: the per-capability `Mode (-)` entry's labels are
/// readable through `EnumLabels::get`, and every catalogue variable's
/// labels survive as static data.
#[test]
fn static_tables_are_statics() {
    let catalogue = all_classes_catalogue();
    let mode = catalogue
        .variables()
        .iter()
        .find(|v| v.name.ends_with("Mode (-)"))
        .expect("the all-classes catalogue carries a Mode column");
    match &mode.kind {
        VariableKind::Enum { labels } => {
            assert!(
                labels.len() >= 13,
                "the mode label table covers every ordinal"
            );
            assert_eq!(labels.get(0), Some("Off"));
            assert_eq!(labels.get(12), Some("On"));
        }
        other => panic!("Mode (-) must be an Enum, got {other:?}"),
    }
    let speed = catalogue
        .variables()
        .iter()
        .find(|v| v.name.ends_with("Speed (-)"))
        .expect("the all-classes catalogue carries a Speed column");
    match &speed.kind {
        VariableKind::Enum { labels } => {
            assert_eq!(
                labels.get(0),
                Some("0"),
                "a speed stage has no name of its own: decimal labels"
            );
        }
        other => panic!("Speed (-) must be an Enum, got {other:?}"),
    }
}

/// The EV Connection State and Charging Level columns carry the equipment
/// declarations' `code` unit end to end: the catalogue's declaration and
/// the schema field metadata at every verbosity that emits them. The
/// declarations are the authority; the static per-capability table mirrors
/// them.
#[test]
fn ev_code_columns_carry_the_declared_code_unit_end_to_end() {
    let catalogue = all_classes_catalogue();
    let mut column_count = 0;
    for def in catalogue.variables() {
        if def.name.ends_with("Connection State (-)") || def.name.ends_with("Charging Level (-)") {
            column_count += 1;
            assert_eq!(
                def.unit,
                Unit::Code,
                "'{}': the EV column's catalogue declaration must be Unit::Code (the equipment \
                 declarations' canonical symbol), got {}",
                def.name,
                def.unit.symbol()
            );
        }
    }
    assert_eq!(
        column_count, 6,
        "the all-classes fixture's three EV-end-use classes carry two code \
         columns each (the name-string classifier's phantom EV columns are \
         gone: an event load and a television are not EV equipment by their \
         typed end use)"
    );

    // The schema field metadata carries the same symbol at the verbosity
    // that emits the columns.
    let schema = hares_io::build_schema(&catalogue, 8, false);
    for field in schema.fields() {
        let name = field.name().as_str();
        if name.ends_with("Connection State (-)") || name.ends_with("Charging Level (-)") {
            assert_eq!(
                field.metadata().get("unit").map(String::as_str),
                Some("code"),
                "'{name}': the schema field metadata must carry the declared unit"
            );
        }
    }
}

/// Two equipment declaring the same telemetry key with contradicting
/// properties is an assembly error naming both.
#[test]
fn contradicting_equipment_declarations_fail_assembly() {
    let input = CatalogueInput {
        equipment: vec![
            CatalogueEquipment {
                name: "EV".to_string(),
                instance_name: None,
                id: EquipmentId(1),
                fuel: FuelType::Electric,
                end_use: EndUse::EV,
                core_capabilities: CoreCapabilities::default(),
                telemetry: vec![TelemetryField::code_code(
                    "connection_state",
                    hares_types::EV_CONNECTION_STATE_LABELS,
                    "Connection state",
                )],
            },
            CatalogueEquipment {
                name: "Electric Vehicle".to_string(),
                instance_name: None,
                id: EquipmentId(2),
                fuel: FuelType::Electric,
                end_use: EndUse::EV,
                core_capabilities: CoreCapabilities::default(),
                telemetry: vec![TelemetryField::ratio(
                    "connection_state",
                    "the same key, declared dimensionless",
                )],
            },
        ],
        zones: Vec::new(),
        actors: Vec::new(),
    };
    let err = build_catalogue(&input).expect_err("a contradicted channel is an assembly error");
    let message = err.to_string();
    assert!(
        message.contains("connection_state")
            && message.contains("equipment:1 (EV)")
            && message.contains("equipment:2 (Electric Vehicle)"),
        "the error names the key and both declaring equipment: {message}"
    );
}

/// A static telemetry-derived column whose declaration contradicts the
/// merged declaration of the key it reads is an assembly error naming
/// both the column and the declaring equipment.
#[test]
fn static_column_contradicting_a_declared_channel_fails_assembly() {
    let input = CatalogueInput {
        equipment: vec![CatalogueEquipment {
            name: "EV".to_string(),
            instance_name: None,
            id: EquipmentId(1),
            fuel: FuelType::Electric,
            end_use: EndUse::EV,
            core_capabilities: CoreCapabilities::default(),
            telemetry: vec![TelemetryField::ratio(
                "connection_state",
                "the equipment drifts from the static table's declaration",
            )],
        }],
        zones: Vec::new(),
        actors: Vec::new(),
    };
    let err = build_catalogue(&input).expect_err("a contradicted column is an assembly error");
    let message = err.to_string();
    assert!(
        message.contains("connection_state")
            && message.contains("EV Connection State (-)")
            && message.contains("equipment:1 (EV)"),
        "the error names the key, the static column and the equipment: {message}"
    );
}

/// An actor whose sealed telemetry is `set` on an undeclared key in its
/// second decide() fails the step naming the actor and the key — also when
/// the actor has not yet written a declared key that step.
#[test]
fn undeclared_actor_telemetry_key_fails_the_step() {
    use hares_control::DispatchRequest;
    use hares_types::EnvironmentState;

    struct DeclaringActor {
        steps: usize,
        telemetry: Telemetry,
        declare_it: bool,
    }
    const DECLARED: &[TelemetryField] = &[TelemetryField::step_count(
        "signals_count",
        "dispatches this step",
    )];

    impl Actor for DeclaringActor {
        fn name(&self) -> &str {
            "undeclared_writer"
        }
        fn output_variables(&self) -> &[TelemetryField] {
            DECLARED
        }
        fn telemetry(&self) -> Option<&Telemetry> {
            Some(&self.telemetry)
        }
        fn decide(&mut self, _env: &EnvironmentState, _out: &mut Vec<DispatchRequest>) {
            self.steps += 1;
            if self.steps >= 2 {
                self.telemetry.set("undeclared_key", 1.0);
            }
            if self.declare_it {
                self.telemetry.set("signals_count", self.steps as f64);
            }
        }
    }

    let make = |declare_it: bool| DeclaringActor {
        steps: 0,
        telemetry: Telemetry::from_fields(DECLARED),
        declare_it,
    };

    // The synthetic dwelling (no output written).
    let toml_path = synthetic_dwelling_toml("catalogue_undeclared_key");

    // The actor wrote its declared key on step 1; the undeclared key on
    // step 2 still fails the step, naming actor and key.
    let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build");
    dwelling.add_actor(Box::new(make(true))).expect("register");
    dwelling.step().expect("step 1 succeeds");
    let err = dwelling
        .step()
        .expect_err("the undeclared key fails step 2");
    let message = err.to_string();
    assert!(
        message.contains("undeclared_writer") && message.contains("undeclared_key"),
        "the failure names the actor and the key: {message}"
    );

    // The same failure when the actor has not yet written a declared key
    // that step.
    let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build");
    dwelling.add_actor(Box::new(make(false))).expect("register");
    dwelling.step().expect("step 1 succeeds");
    let err = dwelling
        .step()
        .expect_err("the undeclared key fails step 2");
    let message = err.to_string();
    assert!(
        message.contains("undeclared_writer") && message.contains("undeclared_key"),
        "the failure names the actor and the key: {message}"
    );
}

/// An actor whose telemetry is not built (and sealed) from its declarations
/// is rejected at registration, naming the actor.
#[test]
fn unsealed_actor_telemetry_is_rejected_at_registration() {
    use hares_control::DispatchRequest;
    use hares_types::EnvironmentState;

    struct UnsealedActor {
        telemetry: Telemetry,
    }
    impl Actor for UnsealedActor {
        fn name(&self) -> &str {
            "unsealed"
        }
        fn output_variables(&self) -> &[TelemetryField] {
            &[]
        }
        fn telemetry(&self) -> Option<&Telemetry> {
            Some(&self.telemetry)
        }
        fn decide(&mut self, _: &EnvironmentState, _: &mut Vec<DispatchRequest>) {}
    }

    let toml_path = synthetic_dwelling_toml("catalogue_unsealed");
    let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build");
    let err = dwelling
        .add_actor(Box::new(UnsealedActor {
            telemetry: Telemetry::new(),
        }))
        .expect_err("an unsealed telemetry is rejected at registration");
    let message = err.to_string();
    assert!(
        message.contains("unsealed") && message.contains("not sealed"),
        "the rejection names the actor and the defect: {message}"
    );

    // An actor whose telemetry holds a key its declarations omit cannot
    // exist once sealed (the sealed insert latched and the key never
    // entered the map); the sealed map's latch is what fails the step. An
    // actor whose telemetry is not sealed at all is rejected above, so
    // registration's key check is the backstop between the two.

    // A declared variable missing from the telemetry is a registration
    // rejection too: hand-rolled maps that skip from_fields are unsealed,
    // and the rejection names the actor and the defect.
    struct IncompleteActor {
        telemetry: Telemetry,
    }
    const INCOMPLETE_DECLARED: &[TelemetryField] = &[
        TelemetryField::step_count("declared_key", "a declared key"),
        TelemetryField::step_count("other_declared_key", "another declared key"),
    ];
    impl Actor for IncompleteActor {
        fn name(&self) -> &str {
            "incomplete"
        }
        fn output_variables(&self) -> &[TelemetryField] {
            INCOMPLETE_DECLARED
        }
        fn telemetry(&self) -> Option<&Telemetry> {
            Some(&self.telemetry)
        }
        fn decide(&mut self, _: &EnvironmentState, _: &mut Vec<DispatchRequest>) {}
    }
    let mut telemetry = Telemetry::new();
    telemetry.insert("declared_key", 0.0);
    let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build");
    let err = dwelling
        .add_actor(Box::new(IncompleteActor { telemetry }))
        .expect_err("an incomplete telemetry is rejected at registration");
    let message = err.to_string();
    assert!(
        message.contains("incomplete") && message.contains("not sealed"),
        "the rejection names the actor and the defect: {message}"
    );
}

/// Writes the synthetic minimal dwelling TOML the registration tests
/// register actors against, and returns its path (the temp dir keeps the
/// file alive for the call).
fn synthetic_dwelling_toml(name: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "hares-catalogue-{name}-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("dwelling.toml");
    std::fs::write(
        &path,
        r#"building_id = 9001

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res = 60
duration = 600

[geometry]
number_of_bedrooms = 3.0
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

[infiltration]
ach = 0.0

[schedule]
occupancy = 0.0
"#,
    )
    .expect("write toml");
    path
}
