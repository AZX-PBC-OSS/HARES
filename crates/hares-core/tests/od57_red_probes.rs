//! RED probes for the dwelling-assembly and stepping silent-substitution
//! sites: each test asserts the strict contract (a typed error naming the
//! input). Run against the pre-fix sources, every probe's call succeeded
//! silently instead (the sites' silent substitutions: the water heaters'
//! `unwrap_or_default()` loop ids, the tankless' `value_at().ok()` init
//! read, and the parser's dangling subsurface wall reference).

use std::path::PathBuf;

use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
use hares_equipment::water_heater::gas::GasWH;
use hares_equipment::water_heater::indirect_tank::IndirectTank;
use hares_equipment::water_heater::resistance::ResistanceWH;
use hares_equipment::water_heater::tankless::TanklessWH;
use hares_equipment::{
    ElectricResistanceWaterHeaterConfig, Equipment, EquipmentConfig, GasWaterHeaterConfig,
    HeatPumpWaterHeaterConfig, IndirectTankConfig, TanklessWaterHeaterConfig,
};
use hares_io::hpxml::parse_hpxml_str;
use hares_types::{
    BoundaryPolicy, DomainUpdate, EnvironmentState, FuelType, GridState, LoopId,
    ScheduleSourceConfig, WeatherState, ZoneId, ZoneState,
};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// An environment whose schedule domain carries `len` values.
fn env(len: usize) -> EnvironmentState {
    let mut env = EnvironmentState {
        ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
        zones: vec![ZoneState {
            id: ZoneId(1),
            temperature_c: 21.0,
            humidity_ratio: 0.008,
            volume_m3: 200.0,
        }],
        weather: WeatherState {
            outdoor_temp_c: 21.0,
            outdoor_humidity_ratio: 0.005,
            wind_speed_m_s: 1.5,
            wind_dir_deg: 0.0,
            ground_temp_c: 21.0,
            mains_temp_c: 15.0,
            sky_temp_c: 16.0,
            pressure_kpa: 101.325,
            solar_irradiance: vec![],
            ghi_w_m2: 0.0,
            dni_w_m2: 0.0,
            dhi_w_m2: 0.0,
            solar_altitude_deg: 0.0,
            ..Default::default()
        },
        grid: GridState {
            voltage_pu: 1.0,
            frequency_hz: 60.0,
            island_bus_voltage_pu: None,
        },
        schedule_row: None,
        domains: hares_types::DomainSlots::default(),
        equipment_telemetry: std::collections::HashMap::new(),
        equipment_core: Default::default(),
        current_time: FixedOffset::east_opt(0)
            .expect("UTC offset")
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .expect("valid timestamp"),
        time_res: ChronoDuration::seconds(60),
        price_signal: Default::default(),
        electrical: Default::default(),
    };
    let mut update = DomainUpdate::empty(hares_types::SCHEDULE_DOMAIN_ID);
    update.custom_payload = Some(vec![0.5; len]);
    env.domains.schedule.set_from(&update);
    env
}

fn gas_config(loop_id: Option<u16>) -> EquipmentConfig {
    EquipmentConfig::from_typed(
        "GWH".to_string(),
        "Gas Water Heater".to_string(),
        GasWaterHeaterConfig {
            equipment_id: None,
            zone_id: Some(1),
            loop_id,
            fuel_type: FuelType::Gas,
            tank_volume_m3: None,
            tank_height_m: None,
            energy_factor: None,
            uniform_energy_factor: None,
            heating_capacity_w: None,
            ua_w_per_k: None,
            setpoint_c: Some(52.0),
            deadband_c: Some(2.0),
            max_tank_temp_c: Some(300.0),
            initial_tank_temp_c: Some(45.0),
            tank_nodes: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: Some(0.0),
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            appliance_draw_flow_rate_source: None,
            pilot_power_w: None,
            flue_loss_fraction: None,
            skin_loss_fraction: None,
            ignition_type: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            // The burner efficiency is required at init (no default may
            // stand in for it); the probe's subject is the loop id.
            conversion_efficiency: Some(0.78),
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
            pilot_fraction_to_tank: None,
            fan_power_w: None,
        },
    )
    .expect("the config builds")
}

fn resistance_config(loop_id: Option<u16>) -> EquipmentConfig {
    EquipmentConfig::from_typed(
        "RWH".to_string(),
        "Resistance Water Heater".to_string(),
        ElectricResistanceWaterHeaterConfig {
            equipment_id: None,
            zone_id: Some(1),
            loop_id,
            tank_volume_m3: None,
            tank_height_m: None,
            energy_factor: None,
            uniform_energy_factor: None,
            heating_capacity_w: None,
            ua_w_per_k: None,
            setpoint_c: Some(52.0),
            deadband_c: Some(2.0),
            max_tank_temp_c: Some(300.0),
            initial_tank_temp_c: Some(45.0),
            tank_nodes: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: Some(0.0),
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            appliance_draw_flow_rate_source: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            element_power_w: None,
            max_setpoint_ramp_rate_c_per_min: None,
            element_priority_mode: None,
            jacket_r_value_m2_k_w: None,
            max_combined_power_w: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
        },
    )
    .expect("the config builds")
}

fn hpwh_config(loop_id: Option<u16>) -> EquipmentConfig {
    EquipmentConfig::from_typed(
        "HPWH".to_string(),
        "Heat Pump Water Heater".to_string(),
        HeatPumpWaterHeaterConfig {
            equipment_id: None,
            zone_id: Some(1),
            loop_id,
            tank_volume_m3: None,
            tank_height_m: None,
            cop: Some(2.5),
            backup_element_power_w: Some(4_500.0),
            ua_w_per_k: None,
            setpoint_c: Some(52.0),
            deadband_c: Some(2.0),
            max_tank_temp_c: Some(300.0),
            initial_tank_temp_c: Some(45.0),
            tank_nodes: None,
            tempering_valve_setpoint_c: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: Some(0.0),
            appliance_draw_flow_rate_source: None,
            compressor_power_w: Some(1_200.0),
            backup_enable_offset_c: Some(3.0),
            min_ambient_temp_c: None,
            max_ambient_temp_c: None,
            min_on_time_s: Some(0.0),
            min_off_time_s: Some(0.0),
            hp_only_mode: Some(false),
            element_hp_control_mode: None,
            fan_power_w: Some(35.0),
            parasitic_power_w: Some(1.0),
            backup_efficiency: Some(1.0),
            shr: Some(0.88),
            lost_heat_fraction: Some(0.0),
            wall_heat_fraction: Some(0.0),
            capacity_biquadratic_coeffs: None,
            cop_biquadratic_coeffs: None,
            cop_curve_is_normalized: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            fixture_delivery_temp_c: None,
            low_power_hpwh: None,
            uniform_energy_factor: None,
        },
    )
    .expect("the config builds")
}

fn indirect_config(boiler_loop_id: Option<u16>) -> EquipmentConfig {
    EquipmentConfig::from_typed(
        "IHWH".to_string(),
        "Indirect Tank".to_string(),
        IndirectTankConfig {
            equipment_id: None,
            zone_id: Some(1),
            boiler_loop_id,
            tank_volume_m3: None,
            tank_height_m: None,
            ua_w_per_k: None,
            hx_ua_w_per_k: Some(150.0),
            setpoint_c: Some(52.0),
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            draw_flow_rate_kg_s: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            appliance_draw_flow_rate_source: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
            boiler_loop_flow_rate_kg_s: None,
        },
    )
    .expect("the config builds")
}

/// water_heater/gas.rs: a config with no loop id fails init naming the
/// equipment. The pre-fix code accepted it silently
/// (`unwrap_or_default()`: the unassigned sentinel 0 stood in for the loop).
#[test]
fn red_probe_gas_wh_init_without_loop_id() {
    let env = env(0);
    let cfg = gas_config(None);
    let mut wh = GasWH::new(cfg.clone());
    let err = wh
        .init(&cfg, &env)
        .expect_err("a gas WH with no loop id must fail init");
    let shown = err.to_string();
    assert!(
        shown.contains("Gas Water Heater") && shown.contains("no fluid loop id"),
        "{shown}"
    );
}

/// water_heater/resistance.rs: the same rule for the resistance WH.
#[test]
fn red_probe_resistance_wh_init_without_loop_id() {
    let env = env(0);
    let cfg = resistance_config(None);
    let mut wh = ResistanceWH::new(cfg.clone());
    let err = wh
        .init(&cfg, &env)
        .expect_err("a resistance WH with no loop id must fail init");
    assert!(
        err.to_string().contains("Electric Resistance Water Heater"),
        "{err}"
    );
}

/// water_heater/heat_pump_wh.rs: the same rule for the heat pump WH.
#[test]
fn red_probe_hpwh_init_without_loop_id() {
    let env = env(0);
    let cfg = hpwh_config(None);
    let mut wh = hares_equipment::water_heater::heat_pump_wh::HeatPumpWH::new(cfg.clone());
    let err = wh
        .init(&cfg, &env)
        .expect_err("an HPWH with no loop id must fail init");
    assert!(err.to_string().contains("Heat Pump Water Heater"), "{err}");
}

/// water_heater/indirect_tank.rs: the same rule for the indirect tank.
#[test]
fn red_probe_indirect_tank_init_without_loop_id() {
    let env = env(0);
    let cfg = indirect_config(None);
    let mut wh = IndirectTank::new(cfg.clone());
    let err = wh
        .init(&cfg, &env)
        .expect_err("an indirect tank with no loop id must fail init");
    assert!(err.to_string().contains("Indirect Tank"), "{err}");
}

/// A bound loop id inits and the WH declares its supply loop's port: the
/// rule errors on the missing id, never on a stated one.
#[test]
fn the_bound_loop_id_inits_and_declares_its_port() {
    let env = env(0);
    let cfg = gas_config(Some(1));
    let mut wh = GasWH::new(cfg.clone());
    wh.init(&cfg, &env).expect("a stated loop id inits");
    assert!(
        wh.ports().iter().any(|p| p.loop_id == Some(LoopId(1))),
        "the WH declares its supply loop's fluid port"
    );
}

/// water_heater/tankless.rs: a mains-temperature source whose read fails at
/// init fails construction naming the heater. The pre-fix code swallowed
/// the read's error (`value_at().ok()`) and silently substituted the
/// weather's mains temperature.
#[test]
fn red_probe_tankless_init_source_failure() {
    // No schedule payload in the environment, so a column source cannot read.
    let env = env(0);
    let typed = TanklessWaterHeaterConfig {
        inlet_temp_c: None,
        mains_temp_c_source: Some(ScheduleSourceConfig::ColumnRef {
            col_idx: 0,
            boundary: BoundaryPolicy::Error,
        }),
        ..tankless_config(Some(1))
    };
    let cfg = EquipmentConfig::from_typed(
        "TWH".to_string(),
        "Tankless Water Heater".to_string(),
        typed,
    )
    .expect("the source-driven config builds");
    let mut twh = TanklessWH::new(cfg.clone());
    let err = twh
        .init(&cfg, &env)
        .expect_err("a failing source read must fail init");
    assert!(err.to_string().contains("Tankless Water Heater"), "{err}");
}

fn tankless_config(loop_id: Option<u16>) -> TanklessWaterHeaterConfig {
    TanklessWaterHeaterConfig {
        equipment_id: None,
        zone_id: Some(1),
        loop_id,
        fuel_type: FuelType::Gas,
        energy_factor: Some(1.0),
        uniform_energy_factor: None,
        heating_capacity_w: Some(20_000.0),
        setpoint_c: Some(50.0),
        parasitic_power_w: Some(0.0),
        performance_adjustment: Some(1.0),
        inlet_temp_c: Some(15.0),
        draw_flow_rate_kg_s: Some(0.0),
        draw_flow_rate_source: None,
        mains_temp_c_source: None,
        appliance_draw_flow_rate_source: None,
        avg_water_draw_l_per_day: None,
        zone_type: None,
        min_flow_kg_s: None,
        min_flow_gpm: None,
    }
}

/// hpxml/building.rs: a window whose AttachedToWall matches no parsed
/// boundary fails the parse naming both ids (its interior zone cannot be
/// derived, and no zone is substituted). The pre-fix code parsed it with
/// the window's interior zone left None and fell back to zone 0 at assembly.
#[test]
fn red_probe_window_dangling_attached_wall() {
    let base =
        std::fs::read_to_string(project_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"))
            .expect("the base fixture reads");
    let dangling = base.replace(
        "<AttachedToWall idref='Wall1'/>\n          </Window>",
        "<AttachedToWall idref='WallN'/>\n          </Window>",
    );
    let err =
        parse_hpxml_str(&dangling).expect_err("a dangling AttachedToWall must fail the parse");
    let shown = err.to_string();
    assert!(
        shown.contains("Window1") && shown.contains("WallN"),
        "{shown}"
    );
}

/// An HPXML whose construction names no NumberofBedrooms fails at the
/// resolve naming the field: the appliance energy formulas and the water
/// heater's tank volume and first-hour rating are sized by it, and OCHRE
/// reads the field with a direct index and fails on absence (hpxml.py:263).
/// Before the strictness the extract silently substituted 2.0 and the
/// appliance side 3.0, so the autosizer's own missing-count error was
/// unreachable.
#[test]
fn red_probe_bedroom_count_without_numberofbedrooms() {
    use hares_io::defaults::DefaultsStore;

    let base =
        std::fs::read_to_string(project_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"))
            .expect("the base fixture reads");
    let absent = base.replace("<NumberofBedrooms>3</NumberofBedrooms>\n          ", "");
    assert_ne!(
        absent, base,
        "the fixture's NumberofBedrooms line must match"
    );
    let building = parse_hpxml_str(&absent).expect("the parse itself succeeds");

    // The water-heater autosize feed errors naming the field.
    let err = hares_io::hpxml::extract_bedroom_count(&building, None)
        .expect_err("an absent NumberofBedrooms must fail the extract");
    assert!(
        err.to_string().contains("NumberofBedrooms"),
        "the error names the required field: {err}"
    );

    // The appliance resolve errors the same way.
    let mut warnings = Vec::new();
    let err = hares_io::hpxml::resolve_equipment(
        &building,
        building.climate_zone_iecc.as_deref(),
        None,
        &DefaultsStore::empty(),
        None,
        &mut warnings,
    )
    .expect_err("an absent NumberofBedrooms must fail the appliance resolve");
    assert!(
        err.to_string().contains("NumberofBedrooms"),
        "the error names the required field: {err}"
    );
}

/// A blueprint-only build (a storage water heater, no Appliances section)
/// with no NumberofBedrooms fails the equipment resolve naming the field:
/// the water-heater resolve sizes the hot-water draw and the tank by the
/// bedroom count, and OCHRE reads the field with a direct index and fails
/// on absence (hpxml.py:263; no OS-HPXML default exists either,
/// defaults.rb:3303). Before the strictness the water-heater side
/// substituted 2.0 whenever no appliance ran the same read first, so the
/// resolve "succeeded" with the substituted sizing.
#[test]
fn red_probe_blueprint_only_bedroom_count_without_numberofbedrooms() {
    use hares_io::defaults::DefaultsStore;

    let base =
        std::fs::read_to_string(project_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"))
            .expect("the base fixture reads");
    let no_bedrooms = base.replace("<NumberofBedrooms>3</NumberofBedrooms>\n          ", "");
    assert_ne!(
        no_bedrooms, base,
        "the fixture's NumberofBedrooms line must match"
    );
    let start = no_bedrooms
        .find("      <Appliances>")
        .expect("the fixture has Appliances");
    let end = no_bedrooms
        .find("      </Appliances>")
        .expect("the fixture closes Appliances")
        + "      </Appliances>".len();
    let blueprint_only = format!("{}{}", &no_bedrooms[..start], &no_bedrooms[end..]);
    assert!(
        !blueprint_only.contains("<Appliances>"),
        "the probe's fixture variant must carry no Appliances"
    );
    let building = parse_hpxml_str(&blueprint_only).expect("the parse itself succeeds");

    let mut warnings = Vec::new();
    let err = hares_io::hpxml::resolve_equipment(
        &building,
        building.climate_zone_iecc.as_deref(),
        None,
        &DefaultsStore::empty(),
        None,
        &mut warnings,
    )
    .expect_err("a blueprint-only build without NumberofBedrooms must fail the resolve");
    assert!(
        err.to_string().contains("NumberofBedrooms"),
        "the error names the required field: {err}"
    );
}
