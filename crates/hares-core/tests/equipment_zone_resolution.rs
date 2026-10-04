//! Water heater and HVAC zone resolution at the dwelling boundary.
//!
//! Every water heater and HVAC unit runs in a zone it can name: an explicit
//! `zone_id`, the conditioned zone for HVAC, or (for water heaters) an HPXML
//! location with no modeled zone. A unit that names none fails `init`
//! instead of landing on the first zone, and a water heater in a location
//! with no modeled zone can join a dwelling that has already stepped.

use std::path::PathBuf;

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
use hares_equipment::{
    ConfigPayload, DHW_DEMAND_LOOP, ElectricResistanceWaterHeaterConfig, Equipment,
    EquipmentConfig, EquipmentRegistry,
};
use hares_io::OutputFormat;
use hares_io::defaults::DefaultsStore;
use hares_io::hpxml::building::parse_building;
use hares_io::hpxml::equipment::resolve_equipment;
use hares_types::{EndUse, EnvironmentState, LoopId, PortType};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn bldg0000007_config() -> DwellingConfig {
    let fixture_dir = project_root().join("tests/fixtures/resstock/2025.1/bldg0000007");
    DwellingConfig {
        hpxml_path: fixture_dir.join("home.xml"),
        schedule_path: Some(fixture_dir.join("in.schedules.csv")),
        weather_path: project_root()
            .join("tests/fixtures/resstock/2025.1/weather/G1500030_2018.csv"),
        defaults_path: Some(project_root().join("defaults")),
        sim_config: SimulationConfig {
            start_time: FixedOffset::west_opt(10 * 3600)
                .expect("UTC-10 offset is valid")
                .with_ymd_and_hms(2018, 1, 1, 0, 0, 0)
                .unwrap(),
            duration: Duration::hours(2),
            time_res: Duration::seconds(900),
            output_verbosity: 0,
            write_output: false,
            output_path: None,
            output_format: OutputFormat::Csv,
            output_chunk_size: 1024,
            setpoint_deadband_c: None,
            master_seed: 0,
            civil_timezone: None,
            site_location: hares_io::SiteLocationOverride::default(),
            retain_batches: false,
            rotation: hares_io::RotationPolicy::None,
            max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
        },
        overrides: None,
        bldg_id: 100,
        initialization_duration: None,
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    }
}

/// The supply loop of the fixture's own water heater, so a probe heater can
/// join a frozen port table that already carries that loop's accumulator.
fn fixture_water_heater_loop(dwelling: &Dwelling) -> LoopId {
    dwelling
        .equipment()
        .iter()
        .filter(|eq| eq.descriptor().end_use == EndUse::WATER_HEATING)
        .flat_map(|eq| eq.ports().iter())
        .filter(|port| port.port_type == PortType::Fluid)
        .find_map(|port| port.loop_id.filter(|id| *id != DHW_DEMAND_LOOP))
        .expect("the fixture carries a storage water heater with a supply loop")
}

fn probe_water_heater(
    name: &str,
    loop_id: LoopId,
    zone_type: Option<&str>,
    env: &EnvironmentState,
) -> hares_types::Result<Box<dyn Equipment>> {
    let typed = ElectricResistanceWaterHeaterConfig {
        equipment_id: None,
        zone_id: None,
        loop_id: Some(loop_id.0),
        tank_volume_m3: None,
        tank_height_m: None,
        energy_factor: None,
        uniform_energy_factor: None,
        heating_capacity_w: Some(4_500.0),
        ua_w_per_k: None,
        setpoint_c: Some(52.0),
        deadband_c: None,
        max_tank_temp_c: None,
        initial_tank_temp_c: None,
        tank_nodes: None,
        avg_water_draw_l_per_day: None,
        draw_flow_rate_kg_s: None,
        draw_flow_rate_source: None,
        mains_temp_c_source: None,
        performance_adjustment: None,
        zone_type: zone_type.map(str::to_string),
        first_hour_rating_m3: None,
        element_power_w: None,
        max_setpoint_ramp_rate_c_per_min: None,
        element_priority_mode: None,
        jacket_r_value_m2_k_w: None,
        max_combined_power_w: None,
        fixture_delivery_temp_c: None,
        hot_draw_temp_c: None,
    };
    let cfg = EquipmentConfig::from_typed(
        name.to_string(),
        "Resistance Water Heater".to_string(),
        typed,
    )?;
    let mut heater =
        EquipmentRegistry::new().create("Electric Resistance Water Heater", cfg.clone())?;
    heater.init(&cfg, env)?;
    Ok(heater)
}

/// A water heater in a location with no modeled zone declares no zone
/// port, so it joins a dwelling whose port table froze at its first step
/// and steps with it.
#[test]
fn ambient_water_heater_joins_after_stepping_has_begun() {
    for location in ["other heated space", "other housing unit"] {
        let mut dwelling = Dwelling::from_config(bldg0000007_config()).expect("dwelling builds");
        for _ in 0..3 {
            dwelling.step().expect("fixture steps");
        }
        let loop_id = fixture_water_heater_loop(&dwelling);
        let heater = probe_water_heater("Probe WH", loop_id, Some(location), dwelling.latest_env())
            .unwrap_or_else(|err| panic!("{location}: probe heater inits: {err}"));
        dwelling
            .add_equipment(heater)
            .unwrap_or_else(|err| panic!("{location}: add after step 3 must succeed: {err}"));
        dwelling.step().unwrap_or_else(|err| {
            panic!("{location}: the dwelling steps with the added heater: {err}")
        });
    }
}

/// A water heater that names neither a zone nor a location has no place to
/// run: init fails naming it, rather than placing it in the first zone.
#[test]
fn water_heater_without_zone_or_location_fails_init() {
    let dwelling = Dwelling::from_config(bldg0000007_config()).expect("dwelling builds");
    let loop_id = fixture_water_heater_loop(&dwelling);
    let err = probe_water_heater("Unplaced WH", loop_id, None, dwelling.latest_env())
        .err()
        .expect("no zone_id and no location must fail init");
    assert!(
        err.to_string().contains("Unplaced WH"),
        "the error must name the water heater, got: {err}"
    );
}

/// OS-HPXML samples covering every HPXML-reachable HVAC and water heater
/// class.
const ZONE_SWEEP_SAMPLES: &[&str] = &[
    "base.xml",
    "base-hvac-elec-resistance-only.xml",
    "base-hvac-boiler-gas-only.xml",
    "base-hvac-boiler-elec-only.xml",
    "base-hvac-furnace-elec-only.xml",
    "base-hvac-room-ac-only.xml",
    "base-hvac-air-to-air-heat-pump-1-speed.xml",
    "base-hvac-mini-split-heat-pump-ducted.xml",
    "base-hvac-autosize-ground-to-air-heat-pump-cooling-only.xml",
    "base-hvac-autosize-ground-to-air-heat-pump-heating-only.xml",
    "base-dhw-tank-gas.xml",
    "base-dhw-tank-heat-pump.xml",
    "base-dhw-tankless-electric.xml",
    "base-dhw-indirect.xml",
];

fn is_zoned_class(spec: &hares_io::EquipmentSpec) -> bool {
    let name = spec.name.as_str();
    name.contains("Furnace")
        || name.contains("Baseboard")
        || name.contains("Boiler")
        || name.contains("Heater")
        || name.contains("Cooler")
        || name.contains("Air Conditioner")
        || name == "Room AC"
        || name == "Indirect Tank"
}

/// Every HVAC and water heater class, configured with no zone_id and no
/// location and initialised outside a dwelling (no zone map), fails init
/// with an error naming the unit and its missing zone.
#[test]
fn hvac_and_water_heaters_without_a_zone_fail_init() {
    let env = Dwelling::from_config(bldg0000007_config())
        .expect("dwelling builds")
        .latest_env()
        .clone();
    let registry = EquipmentRegistry::new();
    let defaults = DefaultsStore::load(&project_root().join("defaults")).expect("defaults load");
    let mut classes = std::collections::BTreeSet::new();
    for sample in ZONE_SWEEP_SAMPLES {
        let path = project_root()
            .join("vendors/OCHRE/test/OS-HPXML Sample Files")
            .join(sample);
        let xml = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("{sample} readable: {err}"))
            .replace(
                "<StateCode>CO</StateCode>",
                "<StateCode>CO</StateCode><Latitude>39.7</Latitude><Longitude>-105.0</Longitude>",
            );
        let building =
            parse_building(&xml).unwrap_or_else(|err| panic!("{sample} parses: {err:?}"));
        let specs = resolve_equipment(&building, &defaults, None, &mut Vec::new())
            .unwrap_or_else(|err| panic!("{sample} resolves: {err:?}"));
        for spec in specs.iter().filter(|spec| is_zoned_class(spec)) {
            let mut cfg = spec
                .typed_config
                .clone()
                .unwrap_or_else(|| panic!("{sample}: {} carries a typed config", spec.name));
            if let ConfigPayload::Typed { data, .. } = &mut cfg.payload {
                data["zone_id"] = serde_json::Value::Null;
                if data.get("zone_type").is_some() {
                    data["zone_type"] = serde_json::Value::Null;
                }
                // The dwelling's loop allocator assigns every resolved
                // boiler's loop id before init; mirror it so the failure
                // under test is the missing zone, not the missing id.
                if data.get("loop_id").is_some() {
                    data["loop_id"] = serde_json::Value::Number(1.into());
                }
            }
            let mut eq = registry
                .create(&cfg.ochre_class, cfg.clone())
                .unwrap_or_else(|err| panic!("{sample}: {} constructs: {err}", spec.name));
            let err = eq
                .init(&cfg, &env)
                .err()
                .unwrap_or_else(|| panic!("{sample}: {} with no zone must fail init", spec.name));
            let message = err.to_string();
            assert!(
                message.contains(&cfg.name) && message.contains("zone"),
                "{sample}: {} must fail naming itself and its zone, got: {message}",
                spec.name
            );
            classes.insert(spec.name.clone());
        }
    }
    for class in [
        "Gas Furnace",
        "Electric Furnace",
        "Electric Baseboard",
        "Gas Boiler",
        "Electric Boiler",
        "Air Conditioner",
        "Room AC",
        "ASHP Heater",
        "ASHP Cooler",
        "MSHP Heater",
        "MSHP Cooler",
        "GSHP Heater",
        "GSHP Cooler",
        "Gas Water Heater",
        "Heat Pump Water Heater",
        "Tankless Water Heater",
        "Indirect Tank",
    ] {
        assert!(
            classes.contains(class),
            "the sweep must cover {class}; covered: {classes:?}"
        );
    }
}

/// Each HVAC unit's equivalent-battery maximum energy over the run, by unit
/// name, with the HVAC zone_ids cleared from the resolved specs when
/// `clear_zone_ids` is set, and the building's temperature capacitance
/// multiplier replaced by `multiplier` when given.
fn hvac_ebm_max_energy(clear_zone_ids: bool, multiplier: Option<f64>) -> Vec<(String, Vec<f64>)> {
    // A Connecticut home with a gas boiler, in January: the boiler heats.
    let fixture_dir = project_root().join("tests/fixtures/resstock/2025.1/bldg0000002");
    let mut config = bldg0000007_config();
    config.hpxml_path = fixture_dir.join("home.xml");
    config.schedule_path = Some(fixture_dir.join("in.schedules.csv"));
    config.weather_path =
        project_root().join("tests/fixtures/resstock/2025.1/weather/G0900090_2018.csv");
    config.sim_config.start_time = FixedOffset::west_opt(5 * 3600)
        .expect("UTC-5 offset is valid")
        .with_ymd_and_hms(2018, 1, 15, 0, 0, 0)
        .unwrap();
    // One-minute steps run the thermostat's on/off cycling rather than the
    // ideal-capacity hold of coarse steps, so the boiler enters its heating
    // mode, where the equivalent battery reports the zone's energy bounds.
    config.sim_config.duration = Duration::hours(2);
    config.sim_config.time_res = Duration::seconds(60);
    // The fixture states OS-HPXML's 7.0; a replaced multiplier goes into a
    // copy of its HPXML.
    let edited = tempfile::tempdir().expect("temp dir");
    if let Some(multiplier) = multiplier {
        let xml = std::fs::read_to_string(&config.hpxml_path).expect("HPXML readable");
        let stated = "<TemperatureCapacitanceMultiplier>7.0</TemperatureCapacitanceMultiplier>";
        assert!(xml.contains(stated), "the fixture states its multiplier");
        config.hpxml_path = edited.path().join("home.xml");
        std::fs::write(
            &config.hpxml_path,
            xml.replacen(
                stated,
                &format!("<TemperatureCapacitanceMultiplier>{multiplier}</TemperatureCapacitanceMultiplier>"),
                1,
            ),
        )
        .expect("write HPXML");
    }
    let mut blueprint =
        hares_core::dwelling::DwellingBlueprint::from_config(config).expect("blueprint");
    let mut cleared = 0;
    for spec in &mut blueprint.equipment_specs {
        let Some(typed) = spec.typed_config.as_mut() else {
            continue;
        };
        let is_hvac = [
            "Heater",
            "Cooler",
            "Air Conditioner",
            "Room AC",
            "Furnace",
            "Boiler",
            "Baseboard",
        ]
        .iter()
        .any(|class| spec.name.contains(class))
            && !spec.name.contains("Water Heater");
        if clear_zone_ids
            && is_hvac
            && let ConfigPayload::Typed { data, .. } = &mut typed.payload
        {
            data["zone_id"] = serde_json::Value::Null;
            spec.parameters.remove("zone_id");
            cleared += 1;
        }
    }
    assert!(
        !clear_zone_ids || cleared > 0,
        "the fixture must carry HVAC to clear"
    );
    let mut dwelling = blueprint.build().expect("dwelling builds");
    let units: Vec<String> = dwelling
        .equipment()
        .iter()
        .filter(|eq| {
            let end_use = &eq.descriptor().end_use;
            *end_use == EndUse::HVAC_HEATING || *end_use == EndUse::HVAC_COOLING
        })
        .map(|eq| eq.descriptor().name.clone())
        .collect();
    let mut series: Vec<(String, Vec<f64>)> = units
        .iter()
        .map(|name| (name.clone(), Vec::new()))
        .collect();
    for _ in 0..120 {
        dwelling.step().expect("fixture steps");
        for (name, values) in &mut series {
            let eq = dwelling
                .equipment()
                .iter()
                .find(|eq| eq.descriptor().name == *name)
                .expect("unit stays in the dwelling");
            values.push(
                eq.telemetry()
                    .get(hares_types::telemetry_keys::EBM_MAX_ENERGY_KWH)
                    .expect("HVAC registers the equivalent-battery keys"),
            );
        }
    }
    series
}

/// An HVAC unit with no zone_id serves the conditioned zone, so the
/// dwelling hands it that zone's capacitance: its equivalent-battery energy
/// bounds equal those of the same unit naming the zone explicitly, and are
/// nonzero (a missing capacitance leaves them at 0).
#[test]
fn hvac_without_a_zone_id_takes_the_conditioned_zone_capacitance() {
    let explicit = hvac_ebm_max_energy(false, None);
    let implicit = hvac_ebm_max_energy(true, None);
    assert!(!explicit.is_empty(), "the fixture must carry HVAC");
    assert!(
        explicit
            .iter()
            .any(|(_, values)| values.iter().any(|v| *v > 0.0)),
        "a unit must reach a heating or cooling mode in the run, got {explicit:?}"
    );
    assert_eq!(
        implicit, explicit,
        "a unit with no zone_id must carry the conditioned zone's capacitance"
    );
}

/// The equivalent battery holds the conditioned zone's capacitance with its
/// temperature capacitance multiplier, the capacitance the thermal solver
/// integrates (OCHRE's reads the same, `ochre/Equipment/HVAC.py:625-640`):
/// a unit's largest maximum energy over the run at OS-HPXML's default 7 is
/// seven times its value at 1.
#[test]
fn the_equivalent_battery_holds_the_multiplied_zone_capacitance() {
    let peak = |series: &[(String, Vec<f64>)]| -> f64 {
        series
            .iter()
            .flat_map(|(_, values)| values.iter().copied())
            .fold(0.0, f64::max)
    };
    let at_one = peak(&hvac_ebm_max_energy(false, Some(1.0)));
    let at_default = peak(&hvac_ebm_max_energy(false, None));
    assert!(at_one > 0.0, "a unit must reach a heating or cooling mode");
    assert!(
        (at_default / at_one - 7.0).abs() < 1e-9,
        "{at_default} kWh at the default against {at_one} kWh at 1"
    );
}

/// A home whose HPXML has neither a ClimateZoneIECC nor a water heater
/// location takes its IECC zone from the weather station, as OS-HPXML
/// does, and the water heater default follows that zone: `base.xml` has a
/// conditioned basement, which IECC 5B (Denver) picks, while IECC 1A
/// (Miami) has only garage and conditioned space in its hierarchy.
#[test]
fn water_heater_default_follows_the_weather_station_climate_zone() {
    let sample = project_root().join("vendors/OCHRE/test/OS-HPXML Sample Files/base.xml");
    let xml = std::fs::read_to_string(&sample).expect("base.xml readable");
    let without = |xml: &str, open: &str, close: &str| {
        let start = xml.find(open).expect("element present");
        let end = start + xml[start..].find(close).expect("element closed") + close.len();
        format!("{}{}", &xml[..start], &xml[end..])
    };
    let xml = without(&xml, "<ClimateZoneIECC>", "</ClimateZoneIECC>");
    let system = xml
        .find("<WaterHeatingSystem>")
        .expect("base.xml has a water heater");
    let xml = format!(
        "{}{}",
        &xml[..system],
        without(&xml[system..], "<Location>", "</Location>")
    );
    let dir = tempfile::tempdir().expect("temp dir");
    let hpxml_path = dir.path().join("home.xml");
    std::fs::write(&hpxml_path, xml).expect("write edited HPXML");

    for (weather, zone, expected) in [
        (
            "data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw",
            "5B",
            "basement - conditioned",
        ),
        (
            "tests/fixtures/resstock/2024.2/weather/G1200860.epw",
            "1A",
            "conditioned space",
        ),
    ] {
        let mut config = bldg0000007_config();
        config.hpxml_path = hpxml_path.clone();
        config.schedule_path = None;
        config.weather_path = project_root().join(weather);
        let blueprint = hares_core::dwelling::DwellingBlueprint::from_config(config)
            .unwrap_or_else(|err| panic!("{weather}: blueprint builds: {err}"));
        let water_heater = blueprint
            .equipment_specs
            .iter()
            .find(|spec| spec.name.contains("Water Heater"))
            .expect("base.xml resolves a water heater");
        assert_eq!(
            water_heater
                .parameters
                .get("zone_type")
                .and_then(|v| v.as_str()),
            Some(expected),
            "{weather}: defaulted water heater location"
        );
        let warnings = blueprint
            .build()
            .unwrap_or_else(|err| panic!("{weather}: dwelling builds: {err}"))
            .take_warnings();
        assert!(
            warnings
                .iter()
                .any(|w| w.contains(&format!("IECC zone {zone} derived from weather station"))),
            "{weather}: the derived zone must be a warning, got {warnings:?}"
        );
    }
}
