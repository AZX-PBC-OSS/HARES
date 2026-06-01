//! 24-hour analytical oracle tests for 8 equipment types.
//!
//! Each test constructs equipment with known parameters, steps 1440 x 60 s
//! (= 24 h), and compares cumulative results against analytically computed
//! expected values derived from first principles.

use std::time::Duration;

use arrow::array::Float64Array;
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
use hares_equipment::battery::Battery;
use hares_equipment::battery::config::BatteryConfig;
use hares_equipment::generator::{Generator, GeneratorKind};
use hares_equipment::hvac::baseboard::ElectricBaseboard;
use hares_equipment::hvac::furnace::GasFurnace;
use hares_equipment::hvac::ideal_hvac::IdealHvac;
use hares_equipment::pv::PV;
use hares_equipment::water_heater::gas::GasWH;
use hares_equipment::water_heater::tankless::TanklessWH;
use hares_equipment::{
    DuctConfig, ElectricBaseboardConfig, Equipment, EquipmentConfig, GasFurnaceConfig,
    GasWaterHeaterConfig, GeneratorConfig, HvacSetpointConfig, IdealHvacConfig, PvConfig,
    TanklessWaterHeaterConfig,
};
use hares_types::{
    ControlSignal, EnvironmentState, FuelType, GridState, PortSlots, SurfaceIrradiance,
    ThermalAccumulator, WeatherState, ZoneId, ZoneState, telemetry_keys as tk,
};
use parquet::arrow::ArrowWriter;
use parquet::file::metadata::KeyValue;
use parquet::file::properties::WriterProperties;

const STEPS: usize = 1440;
const DT_S: u64 = 60;
const DT: Duration = Duration::from_secs(DT_S);
const HOURS_24: f64 = 24.0;

fn base_env() -> EnvironmentState {
    EnvironmentState {
        zones: vec![ZoneState {
            id: ZoneId(1),
            temperature_c: 18.0,
            humidity_ratio: 0.008,
            volume_m3: 200.0,
        }],
        weather: WeatherState {
            outdoor_temp_c: 5.0,
            outdoor_humidity_ratio: 0.003,
            outdoor_wet_bulb_c: 2.0,
            outdoor_enthalpy_j_kg: 15_000.0,
            wind_speed_m_s: 1.0,
            wind_dir_deg: 0.0,
            ground_temp_c: 10.0,
            sky_temp_c: 0.0,
            pressure_kpa: 101.325,
            solar_irradiance: vec![],
            ghi_w_m2: 0.0,
            dni_w_m2: 0.0,
            dhi_w_m2: 0.0,
            solar_altitude_deg: 45.0,
            solar_azimuth_deg: 180.0,
            mains_temp_c: 10.0,
            rainfall_m: 0.0,
            ground_albedo: 0.2,
            ground_t_mean_c: 10.0,
            ground_t_amplitude_c: 0.0,
            ground_phase_day: 35.0,
            day_of_year: 1.0,
        },
        grid: GridState {
            voltage_pu: 1.0,
            frequency_hz: 60.0,
        },
        custom_domains: vec![],
        equipment_telemetry: std::collections::HashMap::new(),
        equipment_core: Default::default(),
        current_time: FixedOffset::east_opt(0)
            .expect("UTC offset")
            .with_ymd_and_hms(2026, 1, 15, 0, 0, 0)
            .single()
            .expect("valid timestamp"),
        time_res: ChronoDuration::seconds(DT_S as i64),
        price_signal: Default::default(),
        electrical: Default::default(),
    }
}

fn default_ports() -> PortSlots {
    PortSlots {
        thermal: vec![ThermalAccumulator::new(ZoneId(1))],
        ..PortSlots::default()
    }
}

fn assert_within_pct(actual: f64, expected: f64, pct: f64, label: &str) {
    let rel_err = if expected.abs() < 1e-12 {
        actual.abs()
    } else {
        ((actual - expected) / expected).abs() * 100.0
    };
    assert!(
        rel_err <= pct,
        "{label}: actual={actual:.6}, expected={expected:.6}, error={rel_err:.3}% (tolerance {pct}%)"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 1. GasFurnace -- 24h constant heating
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn oracle_gas_furnace_24h_constant_heating() {
    let capacity_w = 15_000.0;
    let afue = 0.92;

    let cfg = EquipmentConfig::from_typed(
        "TestFurnace".to_string(),
        "Gas Furnace".to_string(),
        GasFurnaceConfig {
            equipment_id: None,
            zone_id: Some(1),
            capacity_w,
            afue,
            fan_power_w: Some(0.0),
            number_of_speeds: 1,
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            setpoint: HvacSetpointConfig::default(),
            ducts: DuctConfig {
                dse_heat: Some(1.0),
                ..Default::default()
            },
        },
    );

    let mut eq = GasFurnace::new(cfg.clone());
    let env = base_env();
    eq.init(&cfg, &env).unwrap();

    eq.apply_control(&ControlSignal::ThermalSetpoint {
        heating_setpoint_c: Some(22.0),
        cooling_setpoint_c: None,
        deadband_c: None,
    })
    .unwrap();
    eq.apply_control(&ControlSignal::IdealCapacity { capacity_w })
        .unwrap();

    let mut total_fuel_w_s = 0.0;
    let mut total_heat_w_s = 0.0;

    for _ in 0..STEPS {
        let mut ports = default_ports();
        eq.update_control(&env);
        eq.step(&env, DT, &mut ports).unwrap();

        total_fuel_w_s += ports.fuel.get(FuelType::Gas) * DT_S as f64;
        total_heat_w_s += ports.thermal[0].sensible_gain_w * DT_S as f64;
    }

    let total_fuel_kwh = total_fuel_w_s / 3_600_000.0;
    let total_heat_kwh = total_heat_w_s / 3_600_000.0;

    // Analytical: fuel = capacity / afue * 24h, heat = capacity * 24h
    let expected_fuel_kwh = capacity_w / afue * HOURS_24 / 1_000.0;
    let expected_heat_kwh = capacity_w * HOURS_24 / 1_000.0;

    assert_within_pct(total_fuel_kwh, expected_fuel_kwh, 1.0, "gas furnace fuel");
    assert_within_pct(total_heat_kwh, expected_heat_kwh, 1.0, "gas furnace heat");
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. ElectricBaseboard -- trivial COP=1
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn oracle_electric_baseboard_24h_cop1() {
    let capacity_w = 3_000.0;

    let cfg = EquipmentConfig::from_typed(
        "TestBaseboard".to_string(),
        "Electric Baseboard".to_string(),
        ElectricBaseboardConfig {
            equipment_id: None,
            zone_id: Some(1),
            capacity_w,
            eir: 1.0,
            setpoint: HvacSetpointConfig::default(),
        },
    );

    let mut eq = ElectricBaseboard::new(cfg.clone());
    let env = base_env();
    eq.init(&cfg, &env).unwrap();

    eq.apply_control(&ControlSignal::ThermalSetpoint {
        heating_setpoint_c: Some(22.0),
        cooling_setpoint_c: None,
        deadband_c: None,
    })
    .unwrap();
    eq.apply_control(&ControlSignal::IdealCapacity { capacity_w })
        .unwrap();

    let mut total_electric_kw_s = 0.0;
    let mut total_heat_w_s = 0.0;

    for _ in 0..STEPS {
        let mut ports = default_ports();
        eq.update_control(&env);
        eq.step(&env, DT, &mut ports).unwrap();

        total_electric_kw_s += ports.electrical.net_active_w() * DT_S as f64 / 1_000.0;
        total_heat_w_s += ports.thermal[0].sensible_gain_w * DT_S as f64;
    }

    let total_electric_kwh = total_electric_kw_s / 3_600.0;
    let total_heat_kwh = total_heat_w_s / 3_600_000.0;

    let expected_kwh = capacity_w * HOURS_24 / 1_000.0;

    assert_within_pct(total_electric_kwh, expected_kwh, 1.0, "baseboard electric");
    assert_within_pct(total_heat_kwh, expected_kwh, 1.0, "baseboard heat");
    assert_within_pct(
        total_electric_kwh,
        total_heat_kwh,
        0.01,
        "baseboard COP=1 identity",
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. IdealHvac -- exact capacity delivery at 50% load
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn oracle_ideal_hvac_24h_50pct_load() {
    let rated_w = 10_000.0;
    let load_fraction = 0.5;

    let cfg = EquipmentConfig::from_typed(
        "TestIdeal".to_string(),
        "Ideal HVAC".to_string(),
        IdealHvacConfig {
            equipment_id: None,
            zone_id: Some(1),
            setpoint: HvacSetpointConfig {
                heating_setpoint_c: Some(22.0),
                cooling_setpoint_c: Some(28.0),
                ..Default::default()
            },
            deadband_c: None,
            n_speeds: None,
            ideal_capacity_mode: Some(
                hares_equipment::hvac::heating_config::IdealCapacityModeConfig::Off,
            ),
            heating_capacity_w: Some(rated_w),
            cooling_capacity_w: Some(rated_w),
            shr: None,
            fraction_heating_load_served: Some(load_fraction),
            fraction_cooling_load_served: None,
            rated_fan_power_w: None,
            rated_eir: None,
            capacity_min_w: None,
            fuel_type: None,
            capacity_biquadratic_coeffs: None,
            eir_biquadratic_coeffs: None,
        },
    );

    let mut eq = IdealHvac::new(cfg.clone());
    let env = base_env();
    eq.init(&cfg, &env).unwrap();

    let mut total_heat_w_s = 0.0;

    for _ in 0..STEPS {
        let mut ports = default_ports();
        eq.update_control(&env);
        eq.step(&env, DT, &mut ports).unwrap();

        total_heat_w_s += ports.thermal[0].sensible_gain_w * DT_S as f64;
    }

    let total_heat_kwh = total_heat_w_s / 3_600_000.0;
    let expected_kwh = rated_w * load_fraction * HOURS_24 / 1_000.0;

    assert_within_pct(total_heat_kwh, expected_kwh, 1.0, "ideal HVAC heat");
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. PV -- constant irradiance
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn oracle_pv_24h_constant_irradiance() {
    let capacity_kw = 5.0;
    let system_losses = 0.14;
    let inverter_eff = 0.96;
    let ghi_w_m2 = 800.0;
    let ambient_c = 25.0;
    let wind_m_s = 1.0;

    let noct_c = 47.0;
    let t_ref_c = 25.0;
    let gamma_per_c = -0.0047;

    let cfg = EquipmentConfig::from_typed(
        "TestPV".to_string(),
        "PV".to_string(),
        PvConfig {
            equipment_id: None,
            zone_id: None,
            capacity_kw,
            tilt_deg: Some(30.0),
            azimuth_deg: Some(180.0),
            module_type: Some("Standard".to_string()),
            noct_c: Some(noct_c),
            array_type: None,
            system_losses_fraction: Some(system_losses),
            inverter_efficiency: Some(inverter_eff),
            inverter_capacity_kw: None,
            power_factor: None,
            surface_resolution_deg: None,
            sam_lut_path: None,
            arrays: None,
        },
    );

    let surface_id = hares_equipment::pv::surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();

    let mut env = base_env();
    env.weather.outdoor_temp_c = ambient_c;
    env.weather.wind_speed_m_s = wind_m_s;
    env.weather.ghi_w_m2 = ghi_w_m2;
    env.weather.solar_altitude_deg = 45.0;
    env.weather.solar_irradiance = vec![SurfaceIrradiance {
        surface_id,
        direct_w_m2: ghi_w_m2 * 0.6,
        diffuse_w_m2: ghi_w_m2 * 0.3,
        reflected_w_m2: ghi_w_m2 * 0.1,
        angle_of_incidence_rad: 0.0,
    }];

    let mut eq = PV::new(cfg.clone());
    eq.init(&cfg, &env).unwrap();

    let mut total_gen_kw_s = 0.0;

    for _ in 0..STEPS {
        let mut ports = default_ports();
        eq.step(&env, DT, &mut ports).unwrap();

        total_gen_kw_s += (-ports.electrical.net_active_w()) * DT_S as f64 / 1_000.0;
    }

    let total_gen_kwh = total_gen_kw_s / 3_600.0;

    // Hand-calculate: irradiance on array = direct + diffuse + reflected = ghi_w_m2
    let poa_w_m2 = ghi_w_m2;
    // Cell temp (SAM-NOCT with wind correction):
    let noct_factor = (noct_c - 20.0) / 800.0;
    let wind_correction = 9.5 / (5.7 + 3.8 * wind_m_s);
    let cell_temp_c = ambient_c + poa_w_m2 * noct_factor * wind_correction;
    let temp_derate = (1.0 + gamma_per_c * (cell_temp_c - t_ref_c)).max(0.0);
    let dc_kw = capacity_kw * (poa_w_m2 / 1000.0) * temp_derate * (1.0 - system_losses);
    let ac_kw = dc_kw * inverter_eff;
    let expected_kwh = ac_kw * HOURS_24;

    assert_within_pct(total_gen_kwh, expected_kwh, 1.0, "PV generation");
}

/// PV LUT path oracle: verifies that the LUT path produces equivalent 24h
/// cumulative generation when the LUT encodes the analytically expected
/// AC power and SAM metadata matches HARES configuration.
///
/// Uses a synthetic single-entry Parquet LUT with embedded SAM metadata
/// (inv_eff=0.96, losses=0.14). HARES is configured with the same values,
/// making the LUT correction an identity (no-op). The cumulative AC
/// generation must match the hand-calculated expected value.
#[test]
fn oracle_pv_24h_lut_parity() {
    let capacity_kw: f64 = 5.0;
    let system_losses: f64 = 0.14;
    let inverter_eff: f64 = 0.96;
    let ghi_w_m2: f64 = 800.0;
    let ambient_c: f64 = 25.0;
    let wind_m_s: f64 = 1.0;

    let noct_c: f64 = 47.0;
    let t_ref_c: f64 = 25.0;
    let gamma_per_c: f64 = -0.0047;

    // Compute the analytically expected AC power per timestep (same as
    // the non-LUT oracle).
    let poa_w_m2 = ghi_w_m2;
    let noct_factor = (noct_c - 20.0) / 800.0;
    let wind_correction = 9.5 / (5.7 + 3.8 * wind_m_s);
    let cell_temp_c = ambient_c + poa_w_m2 * noct_factor * wind_correction;
    let temp_derate = (1.0 + gamma_per_c * (cell_temp_c - t_ref_c)).max(0.0);
    let dc_kw = capacity_kw * (poa_w_m2 / 1000.0) * temp_derate * (1.0 - system_losses);
    let expected_ac_kw_per_step = dc_kw * inverter_eff;
    let expected_kwh = expected_ac_kw_per_step * HOURS_24;

    // Build a synthetic Parquet LUT with one entry matching the weather
    // conditions. The LUT's AC output is set to the analytically expected
    // AC power so the LUT correction is identity (SAM and HARES match).
    let lut_path =
        std::env::temp_dir().join(format!("pv_lut_oracle_{}.parquet", std::process::id()));
    {
        let schema = Schema::new(vec![
            Field::new("solar_zenith_deg", DataType::Float64, false),
            Field::new("solar_azimuth_deg", DataType::Float64, false),
            Field::new("ghi", DataType::Float64, false),
            Field::new("dni", DataType::Float64, false),
            Field::new("dhi", DataType::Float64, false),
            Field::new("temp_c", DataType::Float64, false),
            Field::new("ac_power_kw", DataType::Float64, false),
        ]);
        // LUT entry: zenith=45° (altitude=45° → zenith=90-45=45°),
        // azimuth=180°, GHI=800, DNI=600, DHI=200, temp=25°C.
        let zenith_deg = 90.0 - 45.0; // from solar_altitude_deg = 45.0
        let batch = RecordBatch::try_new(
            std::sync::Arc::new(schema.clone()),
            vec![
                std::sync::Arc::new(Float64Array::from(vec![zenith_deg])),
                std::sync::Arc::new(Float64Array::from(vec![180.0])),
                std::sync::Arc::new(Float64Array::from(vec![ghi_w_m2])),
                std::sync::Arc::new(Float64Array::from(vec![600.0])),
                std::sync::Arc::new(Float64Array::from(vec![200.0])),
                std::sync::Arc::new(Float64Array::from(vec![ambient_c])),
                std::sync::Arc::new(Float64Array::from(vec![expected_ac_kw_per_step])),
            ],
        )
        .expect("record batch");
        let file = std::fs::File::create(&lut_path).expect("create lut file");
        let props = WriterProperties::builder()
            .set_key_value_metadata(Some(vec![
                KeyValue::new(
                    "harvest_lut_sam_inv_eff".to_string(),
                    format!("{inverter_eff}"),
                ),
                KeyValue::new(
                    "harvest_lut_sam_losses".to_string(),
                    format!("{system_losses}"),
                ),
            ]))
            .build();
        let mut writer = ArrowWriter::try_new(file, std::sync::Arc::new(schema), Some(props))
            .expect("arrow writer");
        writer.write(&batch).expect("write batch");
        writer.close().expect("close writer");
    }

    let surface_id = hares_equipment::pv::surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();

    let mut env = base_env();
    env.weather.outdoor_temp_c = ambient_c;
    env.weather.wind_speed_m_s = wind_m_s;
    env.weather.ghi_w_m2 = ghi_w_m2;
    env.weather.dni_w_m2 = 600.0;
    env.weather.dhi_w_m2 = 200.0;
    env.weather.solar_altitude_deg = 45.0;
    env.weather.solar_azimuth_deg = 180.0;
    env.weather.solar_irradiance = vec![SurfaceIrradiance {
        surface_id,
        direct_w_m2: 0.0,
        diffuse_w_m2: 0.0,
        reflected_w_m2: 0.0,
        angle_of_incidence_rad: 0.0,
    }];

    let cfg = EquipmentConfig::from_typed(
        "TestPVLUT".to_string(),
        "PV".to_string(),
        PvConfig {
            equipment_id: None,
            zone_id: None,
            capacity_kw,
            tilt_deg: Some(30.0),
            azimuth_deg: Some(180.0),
            module_type: Some("Standard".to_string()),
            noct_c: Some(noct_c),
            array_type: None,
            system_losses_fraction: Some(system_losses),
            inverter_efficiency: Some(inverter_eff),
            inverter_capacity_kw: None,
            power_factor: None,
            surface_resolution_deg: None,
            sam_lut_path: Some(lut_path.to_string_lossy().into_owned()),
            arrays: None,
        },
    );

    let mut eq = PV::new(cfg.clone());
    eq.init(&cfg, &env).unwrap();

    let mut total_gen_kw_s = 0.0;

    for _ in 0..STEPS {
        let mut ports = default_ports();
        eq.step(&env, DT, &mut ports).unwrap();

        total_gen_kw_s += (-ports.electrical.net_active_w()) * DT_S as f64 / 1_000.0;
    }

    let total_gen_kwh = total_gen_kw_s / 3_600.0;

    assert_within_pct(total_gen_kwh, expected_kwh, 1.0, "PV LUT generation");
}

// ═══════════════════════════════════════════════════════════════════════════
// 5. Battery -- self-discharge analytical formula
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn oracle_battery_self_discharge_24h() {
    let daily_rate_pct = 2.0;
    let initial_soc = 0.80;

    let cfg = EquipmentConfig::from_typed(
        "TestBattery".to_string(),
        "Battery".to_string(),
        BatteryConfig {
            equipment_id: None,
            zone_id: None,
            capacity_kwh: 13.5,
            max_charge_kw: 5.0,
            max_discharge_kw: 5.0,
            n_series: None,
            n_parallel: None,
            ah_cell: None,
            v_cell: None,
            cell_resistance_ohm: Some(0.0),
            pack_voltage_v: None,
            chemistry: None,
            standby_power_w: Some(0.0),
            self_discharge_pct_per_day: Some(daily_rate_pct),
            min_soc: Some(0.0),
            max_soc: Some(1.0),
            initial_soc: Some(initial_soc),
            initial_cell_temp_c: None,
            import_limit_w: None,
            export_limit_w: None,
            heater_power_w: Some(0.0),
            heater_threshold_c: None,
            heater_on_discharge: None,
            min_discharge_temp_c: None,
            full_power_temp_c: None,
            min_charge_temp_c: None,
            cell_thermal_mass_j_per_k: None,
            cell_ua_w_per_k: Some(0.0),
            inverter_efficiency: None,
            charge_efficiency: Some(1.0),
            discharge_efficiency: Some(1.0),
            bms_mode: None,
            grid_export_rule: None,
        },
    );

    let mut bat = Battery::new(cfg.clone());
    let env = base_env();
    bat.init(&cfg, &env).unwrap();

    // Disable self-consumption so battery is idle
    bat.apply_control(&ControlSignal::SelfConsumption {
        enabled: false,
        solar_only_charging: false,
    })
    .unwrap();

    for _ in 0..STEPS {
        let mut ports = default_ports();
        bat.step(&env, DT, &mut ports).unwrap();
    }

    // OCHRE uses absolute (linear) self-discharge:
    // soc_loss = rate_per_s * total_seconds
    let rate_per_s = daily_rate_pct / 100.0 / 86_400.0;
    let total_s = STEPS as f64 * DT_S as f64;
    let expected_soc = initial_soc - rate_per_s * total_s;

    let actual_soc = bat.telemetry().get(tk::SOC).expect("SOC telemetry");
    assert_within_pct(actual_soc, expected_soc, 0.5, "battery self-discharge SOC");
}

// ═══════════════════════════════════════════════════════════════════════════
// 6. Generator -- constant load fuel consumption
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn oracle_generator_24h_constant_load() {
    let rated_kw = 10.0;
    let setpoint_kw = 5.0;
    let eta_electric = 0.30;

    let cfg = EquipmentConfig::from_typed(
        "TestGen".to_string(),
        "Gas Generator".to_string(),
        GeneratorConfig {
            equipment_id: None,
            zone_id: None,
            fuel_type: None,
            rated_power_kw: rated_kw,
            eta_electric: Some(eta_electric),
            eta_thermal: Some(0.0),
            eta_jacket_water: None,
            eta_lube_oil: None,
            eta_exhaust: None,
            efficiency_type: Some("constant".to_string()),
            efficiency_curve_points: None,
            delta_kw_per_s: Some(100.0),
            capacity_min_kw: None,
            grid_import_limit_kw: None,
            export_limit_kw: None,
            loop_id: None,
            flow_rate_kg_s: None,
            supply_temp_c: None,
            return_temp_c: None,
            inverter_efficiency: None,
            stack_temp_c: None,
            stack_cooler_r0: None,
            stack_cooler_r1: None,
            stack_cooler_r2: None,
            stack_cooler_r3: None,
            stack_nominal_temp_c: None,
            heat_rec_max_temp_c: None,
        },
    );

    let mut generator = Generator::new(cfg.clone(), GeneratorKind::GasGenerator);
    let env = base_env();
    generator.init(&cfg, &env).unwrap();

    generator
        .apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: setpoint_kw,
            reactive_power_kvar: None,
            min_soc: None,
            max_soc: None,
        })
        .unwrap();

    let mut total_fuel_w_s = 0.0;
    let mut total_gen_kw_s = 0.0;

    for _ in 0..STEPS {
        let mut ports = default_ports();
        generator.update_control(&env);
        generator.step(&env, DT, &mut ports).unwrap();

        total_fuel_w_s += ports.fuel.get(FuelType::Gas) * DT_S as f64;
        total_gen_kw_s += (-ports.electrical.net_active_w()) * DT_S as f64 / 1_000.0;
    }

    let total_fuel_kwh = total_fuel_w_s / 3_600_000.0;
    let total_gen_kwh = total_gen_kw_s / 3_600.0;

    // Analytical: gen = power * 24h, fuel = power_W / efficiency * 24h
    let expected_gen_kwh = setpoint_kw * HOURS_24;
    let expected_fuel_kwh = (setpoint_kw * 1_000.0) / eta_electric * HOURS_24 / 1_000.0;

    assert_within_pct(total_gen_kwh, expected_gen_kwh, 1.0, "generator output");
    assert_within_pct(total_fuel_kwh, expected_fuel_kwh, 1.0, "generator fuel");
}

// ═══════════════════════════════════════════════════════════════════════════
// 7. GasWH -- standby UA decay (no draw)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn oracle_gas_wh_standby_ua_decay_24h() {
    let initial_temp_c = 60.0;
    let ambient_c = 20.0;
    let ua_w_per_k = 2.0;
    let setpoint_c = 40.0; // below initial so burner never fires
    let tank_volume_m3 = 0.15;

    let cfg = EquipmentConfig::from_typed(
        "TestGasWH".to_string(),
        "Gas Water Heater".to_string(),
        GasWaterHeaterConfig {
            equipment_id: None,
            zone_id: Some(1),
            loop_id: None,
            fuel_type: FuelType::Gas,
            tank_volume_m3: Some(tank_volume_m3),
            tank_height_m: None,
            energy_factor: None,
            uniform_energy_factor: None,
            heating_capacity_w: Some(11_000.0),
            ua_w_per_k: Some(ua_w_per_k),
            setpoint_c: Some(setpoint_c),
            deadband_c: Some(1.0),
            max_tank_temp_c: Some(300.0),
            initial_tank_temp_c: Some(initial_temp_c),
            tank_nodes: Some(1),
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: Some(0.0),
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            pilot_power_w: Some(0.0),
            flue_loss_fraction: Some(0.0),
            skin_loss_fraction: Some(0.0),
            ignition_type: Some("ElectronicIgnition".to_string()),
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            conversion_efficiency: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
            pilot_fraction_to_tank: None,
        },
    );

    let mut wh = GasWH::new(cfg.clone());
    let mut env = base_env();
    env.zones[0].temperature_c = ambient_c;
    env.weather.outdoor_temp_c = ambient_c;
    wh.init(&cfg, &env).unwrap();

    for _ in 0..STEPS {
        let mut ports = default_ports();
        wh.update_control(&env);
        wh.step(&env, DT, &mut ports).unwrap();
    }

    // Exponential decay: T(t) = T_amb + (T_init - T_amb) * exp(-UA/mc * t)
    let water_density_kg_m3 = 998.0;
    let cp_j_kg_k = 4183.0;
    let mass_kg = tank_volume_m3 * water_density_kg_m3;
    let mc = mass_kg * cp_j_kg_k;
    let total_s = STEPS as f64 * DT_S as f64;
    let expected_temp_c =
        ambient_c + (initial_temp_c - ambient_c) * (-ua_w_per_k / mc * total_s).exp();

    let final_temp = wh
        .telemetry()
        .get(tk::TANK_AVG_TEMP_C)
        .expect("tank avg temp telemetry");

    assert_within_pct(
        final_temp,
        expected_temp_c,
        2.0,
        "gas WH standby decay temp",
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 8. TanklessWH -- constant draw energy balance
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn oracle_tankless_wh_24h_constant_draw() {
    let draw_kg_s = 2.0 / 60.0 * 0.998; // ~2 L/min
    let inlet_temp_c = 10.0;
    let setpoint_c = 50.0;
    let efficiency = 0.90;
    let rated_w = 20_000.0;

    let cfg = EquipmentConfig::from_typed(
        "TestTankless".to_string(),
        "Tankless Water Heater".to_string(),
        TanklessWaterHeaterConfig {
            equipment_id: None,
            zone_id: Some(1),
            loop_id: None,
            fuel_type: FuelType::Gas,
            energy_factor: Some(efficiency),
            uniform_energy_factor: None,
            heating_capacity_w: Some(rated_w),
            setpoint_c: Some(setpoint_c),
            parasitic_power_w: Some(0.0),
            performance_adjustment: None,
            inlet_temp_c: Some(inlet_temp_c),
            draw_flow_rate_kg_s: Some(draw_kg_s),
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            avg_water_draw_l_per_day: None,
            zone_type: None,
        },
    );

    let mut wh = TanklessWH::new(cfg.clone());
    let env = base_env();
    wh.init(&cfg, &env).unwrap();

    let mut total_fuel_w_s = 0.0;

    for _ in 0..STEPS {
        let mut ports = default_ports();
        wh.update_control(&env);
        wh.step(&env, DT, &mut ports).unwrap();

        total_fuel_w_s += ports.fuel.get(FuelType::Gas) * DT_S as f64;
    }

    let total_fuel_kwh = total_fuel_w_s / 3_600_000.0;

    // Analytical: thermal = m_dot * cp * dT, fuel = thermal / efficiency
    let cp_j_kg_k = 4183.0;
    let delta_t = setpoint_c - inlet_temp_c;
    let thermal_demand_w = draw_kg_s * cp_j_kg_k * delta_t;
    let fuel_demand_w = thermal_demand_w / efficiency;
    let expected_fuel_kwh = fuel_demand_w * HOURS_24 / 1_000.0;

    assert_within_pct(total_fuel_kwh, expected_fuel_kwh, 2.0, "tankless WH fuel");
}
