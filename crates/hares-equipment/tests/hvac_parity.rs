//! HVAC equipment step correctness tests.
//!
//! Tests physics correctness against hand-calculated OCHRE reference values.
//! Where HARES intentionally uses better physics than OCHRE, divergences are
//! documented inline. Tests that require OCHRE output data files are marked
//! `#[ignore]`.
//!
//! Reference: vendors/OCHRE/ochre/Equipment/HVAC.py

use std::time::Duration;

use chrono::{FixedOffset, TimeZone};
use hares_equipment::{
    CentralAirConditionerConfig, DuctConfig, ElectricBaseboardConfig, EquipmentConfig,
    EquipmentRegistry, GasFurnaceConfig, HeatPumpCoolerConfig, HeatPumpHeaterConfig,
};
use hares_types::{
    ControlSignal, EnvironmentState, FuelType, GridState, OperatingMode, PortSlots,
    ThermalAccumulator, WeatherState, ZoneId, ZoneState,
};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn make_env(zone_temp_c: f64, outdoor_temp_c: f64, zone_wb_c: f64) -> EnvironmentState {
    EnvironmentState {
        zones: vec![ZoneState {
            id: ZoneId(1),
            temperature_c: zone_temp_c,
            humidity_ratio: 0.010,
            relative_humidity: 0.50,
            wet_bulb_c: zone_wb_c,
            volume_m3: 200.0,
        }],
        weather: WeatherState {
            outdoor_temp_c,
            outdoor_humidity_ratio: 0.010,
            outdoor_wet_bulb_c: outdoor_temp_c - 3.0,
            outdoor_enthalpy_j_kg: 30_000.0,
            wind_speed_m_s: 2.0,
            wind_dir_deg: 0.0,
            ground_temp_c: 12.0,
            sky_temp_c: outdoor_temp_c - 8.0,
            pressure_kpa: 101.325,
            solar_irradiance: vec![],
            ghi_w_m2: 0.0,
            dni_w_m2: 0.0,
            dhi_w_m2: 0.0,
            solar_altitude_deg: 0.0,
            solar_azimuth_deg: 180.0,
            mains_temp_c: 15.0,
            rainfall_m: 0.0,
            ground_albedo: 0.2,
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
            .with_ymd_and_hms(2026, 7, 15, 14, 0, 0)
            .single()
            .expect("valid"),
        time_res: chrono::Duration::seconds(60),
        price_signal: Default::default(),
        electrical: Default::default(),
    }
}

fn make_env_at_minute(
    zone_temp_c: f64,
    outdoor_temp_c: f64,
    zone_wb_c: f64,
    minute: i64,
) -> EnvironmentState {
    let mut env = make_env(zone_temp_c, outdoor_temp_c, zone_wb_c);
    env.current_time += chrono::Duration::minutes(minute);
    env
}

fn make_ports() -> PortSlots {
    PortSlots {
        thermal: vec![ThermalAccumulator::new(ZoneId(1))],
        ..PortSlots::default()
    }
}

fn cfg(name: &str, class: &str, pairs: &[(&str, f64)]) -> EquipmentConfig {
    let get = |key: &str| -> Option<f64> { pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| *v) };

    match class {
        "Air Conditioner" => EquipmentConfig::from_typed(
            name.to_string(),
            class.to_string(),
            CentralAirConditionerConfig {
                equipment_id: None,
                zone_id: get("zone_id").map(|v| v as u16),
                capacity_w: get("capacity_w").unwrap_or(10_000.0),
                eir: 0.33,
                shr: Some(0.75),
                number_of_speeds: 1,
                stage_capacities_w: None,
                stage_eirs: None,
                stage_shrs: None,
                fan_power_w: None,
                fan_power_w_per_cfm: None,
                cooling_setpoint_c: get("cooling_setpoint_c"),
                heating_setpoint_c: get("heating_setpoint_c"),
                hysteresis_c: Some(1.0),
                heating_setpoint_source: None,
                cooling_setpoint_source: None,
                airflow_m3_s_per_w: None,
                fraction_load_served: None,
                crankcase_heater_kw: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                duct: DuctConfig::default(),
                system_type: None,
                startup_cd: get("startup_cd"),
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
            },
        ),
        "ASHP Heater" => EquipmentConfig::from_typed(
            name.to_string(),
            class.to_string(),
            HeatPumpHeaterConfig {
                equipment_id: None,
                zone_id: get("zone_id").map(|v| v as u16),
                heating_capacity_w: Some(8_000.0),
                heating_eir: Some(3.412_141_633 / 9.0),
                stage_heating_capacities_w: None,
                stage_heating_eirs: None,
                backup_fuel: None,
                backup_capacity_w: get("backup_capacity_w"),
                backup_eir: None,
                fraction_heating_load_served: Some(1.0),
                cooling_capacity_w: Some(8_000.0),
                cooling_eir: Some(3.412_141_633 / 14.0),
                stage_cooling_capacities_w: None,
                stage_cooling_eirs: None,
                stage_shrs: None,
                fraction_cooling_load_served: Some(1.0),
                number_of_speeds: 1,
                is_mini_split: false,
                shr: Some(0.75),
                fan_power_w: Some(0.0),
                fan_power_w_per_cfm: None,
                airflow_m3_s_per_w: None,
                heating_setpoint_c: get("heating_setpoint_c"),
                cooling_setpoint_c: get("cooling_setpoint_c"),
                hysteresis_c: Some(1.0),
                heating_setpoint_source: None,
                cooling_setpoint_source: None,
                hp_lockout_temp_c: None,
                er_lockout_temp_c: None,
                max_oat_supplemental_c: None,
                er_setpoint_offset_c: None,
                er_hard_lockout_time_s: None,
                duct: DuctConfig::default(),
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
            },
        ),
        _ => panic!("unsupported class in hvac_parity cfg helper: {class}"),
    }
}

fn hp_cooler_cfg(
    name: &str,
    number_of_speeds: u8,
    is_mini_split: bool,
    stage_capacities_w: Option<Vec<f64>>,
    stage_eirs: Option<Vec<f64>>,
) -> EquipmentConfig {
    EquipmentConfig::from_typed(
        name.to_string(),
        if is_mini_split {
            "MSHP Cooler".to_string()
        } else {
            "ASHP Cooler".to_string()
        },
        HeatPumpCoolerConfig {
            equipment_id: None,
            zone_id: Some(1),
            heating_capacity_w: None,
            heating_eir: None,
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            backup_fuel: None,
            backup_capacity_w: None,
            backup_eir: None,
            fraction_heating_load_served: None,
            cooling_capacity_w: Some(8_000.0),
            cooling_eir: Some(0.33),
            stage_cooling_capacities_w: stage_capacities_w,
            stage_cooling_eirs: stage_eirs,
            stage_shrs: None,
            fraction_cooling_load_served: Some(1.0),
            number_of_speeds,
            is_mini_split,
            shr: Some(0.75),
            fan_power_w: None,
            fan_power_w_per_cfm: None,
            airflow_m3_s_per_w: None,
            heating_setpoint_c: Some(18.0),
            cooling_setpoint_c: Some(24.0),
            hysteresis_c: Some(0.0),
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
            duct: DuctConfig::default(),
            biquadratic_x1_min: None,
            biquadratic_x1_max: None,
            biquadratic_x2_min: None,
            biquadratic_x2_max: None,
            ff_min: None,
            ff_max: None,
            plf_min: None,
            plf_max: None,
        },
    )
}

// ---------------------------------------------------------------------------
// 1. Gas furnace: energy balance verification
//
// OCHRE reference (HVAC.py GasFurnace.calculate_power_and_heat):
//   capacity = rated * duty_cycle
//   fuel_input = capacity / AFUE
//   fan_kw = fan_power_w * duty_cycle / 1000
//
// Setup: capacity=15000 W, AFUE=0.80, fan=400 W, zone=19°C, setpoint=21°C.
// Expect: element runs at full duty → fuel_input=18750 W, fan=0.4 kW,
//         thermal_output=15400 W (gross heat + fan heat, DSE=1.0).
// ---------------------------------------------------------------------------
#[test]
fn gas_furnace_energy_balance() {
    const RATED_CAPACITY_W: f64 = 15_000.0;
    const FUEL_EFFICIENCY: f64 = 0.80;
    const FAN_POWER_W: f64 = 400.0;
    const EXPECTED_FAN_KW: f64 = FAN_POWER_W / 1_000.0;

    let cfg = EquipmentConfig::from_typed(
        "furnace".to_string(),
        "Gas Furnace".to_string(),
        GasFurnaceConfig {
            equipment_id: None,
            zone_id: Some(1),
            afue: FUEL_EFFICIENCY,
            capacity_w: RATED_CAPACITY_W,
            number_of_speeds: 1,
            fan_power_w: Some(FAN_POWER_W),
            ducts: DuctConfig::default(),
        },
    );
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Gas Furnace", cfg.clone()).unwrap();
    let env = make_env(19.0, -5.0, 13.0);
    eq.init(&cfg, &env).unwrap();
    eq.update_control(&env);

    let mut ports = make_ports();
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let fuel_w = ports.fuel.get(FuelType::Gas);
    let fan_kw = ports.electrical.net_active_kw();
    let thermal_w = ports.thermal[0].sensible_gain_w;

    // AFUE relationship: fuel = capacity / AFUE = 15000 / 0.80 = 18750 W
    let expected_fuel_w = RATED_CAPACITY_W / FUEL_EFFICIENCY;
    assert!(
        (fuel_w - expected_fuel_w).abs() < expected_fuel_w * 0.01,
        "fuel_input must equal capacity/AFUE={expected_fuel_w:.0} W ±1%; got {fuel_w:.1} W"
    );

    // Fan is electric-only: 400 W = 0.4 kW
    assert!(
        (fan_kw - EXPECTED_FAN_KW).abs() < 0.01,
        "fan electric must be {EXPECTED_FAN_KW:.1} kW ±10 W; got {fan_kw:.4} kW"
    );

    // Thermal output includes gross heating + fan heat (no duct losses configured).
    let expected_thermal_w = RATED_CAPACITY_W + FAN_POWER_W;
    assert!(
        (thermal_w - expected_thermal_w).abs() < 150.0,
        "thermal_output must be ~{expected_thermal_w:.0} W; got {thermal_w:.1} W"
    );

    // Effective COP here is delivered thermal (gross + fan heat) / fuel.
    let tel = eq.telemetry();
    let fuel_input_w = tel.get("fuel_input_w").expect("fuel_input_w must exist");
    let thermal_output_w = tel
        .get("thermal_output_w")
        .expect("thermal_output_w must exist");
    assert!(
        (thermal_output_w - thermal_w).abs() < 1.0,
        "telemetry thermal_output_w ({thermal_output_w:.1} W) must match thermal port ({thermal_w:.1} W)"
    );
    let gas_cop = thermal_output_w / fuel_input_w;
    let expected_effective_cop = expected_thermal_w / expected_fuel_w;
    assert!(
        (gas_cop - expected_effective_cop).abs() < 0.01,
        "effective furnace COP must be ~{expected_effective_cop:.4}; got {gas_cop:.4} \
         (thermal={thermal_output_w:.1} W, fuel={fuel_input_w:.1} W)"
    );
}

// ---------------------------------------------------------------------------
// 2. Gas furnace: fuel is independent of DSE
//
// OCHRE HVAC.py: fuel is computed from rated capacity regardless of duct losses.
// Only the delivered heat to the zone is reduced by DSE.
// ---------------------------------------------------------------------------
#[test]
fn gas_furnace_fuel_independent_of_duct_dse() {
    let make = |dse: f64| {
        let c = EquipmentConfig::from_typed(
            "furnace".to_string(),
            "Gas Furnace".to_string(),
            GasFurnaceConfig {
                equipment_id: None,
                zone_id: Some(1),
                afue: 0.80,
                capacity_w: 10_000.0,
                number_of_speeds: 1,
                fan_power_w: Some(0.0),
                ducts: DuctConfig {
                    dse_heat: Some(dse),
                    ..DuctConfig::default()
                },
            },
        );
        let registry = EquipmentRegistry::new();
        let mut eq = registry.create("Gas Furnace", c.clone()).unwrap();
        let env = make_env(18.0, -5.0, 12.0);
        eq.init(&c, &env).unwrap();
        eq.update_control(&env);
        let mut ports = make_ports();
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        let fuel_w = ports.fuel.get(FuelType::Gas);
        let thermal_w = ports.thermal[0].sensible_gain_w;
        (fuel_w, thermal_w)
    };

    let (fuel_perfect, thermal_perfect) = make(1.0);
    let (fuel_lossy, thermal_lossy) = make(0.80);

    // Fuel must be identical: both cases burn the same gas for the same capacity.
    assert!(
        (fuel_perfect - fuel_lossy).abs() < 1.0,
        "fuel_input must be DSE-independent: perfect={fuel_perfect:.1}, lossy={fuel_lossy:.1}"
    );

    // Thermal delivery IS reduced by DSE.
    assert!(
        thermal_lossy < thermal_perfect - 100.0,
        "thermal with DSE=0.80 ({thermal_lossy:.1} W) must be < perfect ({thermal_perfect:.1} W)"
    );

    // Exact ratio: thermal_lossy / thermal_perfect ≈ 0.80
    let actual_dse = thermal_lossy / thermal_perfect;
    assert!(
        (actual_dse - 0.80).abs() < 0.01,
        "effective DSE must be ~0.80; got {actual_dse:.4}"
    );
}

// ---------------------------------------------------------------------------
// 3. Electric baseboard: resistive COP = 1.0
//
// For electric resistance heating, every watt of electrical input becomes
// one watt of delivered heat. COP ≡ 1.0 by definition (no biquadratic curves).
// ---------------------------------------------------------------------------
#[test]
fn electric_baseboard_cop_is_unity() {
    let c = EquipmentConfig::from_typed(
        "bb".to_string(),
        "Electric Baseboard".to_string(),
        ElectricBaseboardConfig {
            equipment_id: None,
            zone_id: Some(1),
            capacity_w: 3_000.0,
            eir: 1.0,
        },
    );
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Electric Baseboard", c.clone()).unwrap();
    let env = make_env(18.0, -5.0, 12.0);
    eq.init(&c, &env).unwrap();
    eq.update_control(&env);

    let mut ports = make_ports();
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let thermal_w = ports.thermal[0].sensible_gain_w;
    let electric_w = ports.electrical.net_active_kw() * 1_000.0;

    assert!(
        thermal_w > 0.0,
        "baseboard must deliver heat; got {thermal_w:.3} W"
    );
    assert!(
        electric_w > 0.0,
        "baseboard must draw electricity; got {electric_w:.3} W"
    );

    let cop = thermal_w / electric_w;
    // OCHRE uses space_fraction=1.0 by default → full heat delivered to zone.
    assert!(
        (cop - 1.0).abs() < 0.02,
        "electric baseboard COP must be ~1.0 (resistive); got {cop:.4} \
         (thermal={thermal_w:.1} W, electric={electric_w:.1} W)"
    );
}

// ---------------------------------------------------------------------------
// 4. ASHP heating: COP > 1.0 at AHRI H1 conditions (7°C outdoor)
//
// OCHRE HVAC.py: HeatPumpHeater.calculate_power_and_heat applies biquadratic
// capacity and EIR curves with clamping. At 7°C outdoor and reasonable EIR,
// COP should be well above 1.0.
//
// AHRI 210/240-2023: minimum heating COP at 47°F (8.3°C) is 2.0 for Tier 1.
// ---------------------------------------------------------------------------
#[test]
fn ashp_heating_cop_above_unity_at_ahri_h1() {
    let c = cfg(
        "ashp",
        "ASHP Heater",
        &[
            ("zone_id", 1.0),
            ("heating_setpoint_c", 21.0),
            ("cooling_setpoint_c", 27.0),
            ("backup_capacity_w", 0.0),
        ],
    );
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("ASHP Heater", c.clone()).unwrap();
    // AHRI H1 test point: 47°F (8.3°C) outdoor, 70°F (21.1°C) indoor
    let env = make_env(20.0, 7.0, 14.0);
    eq.init(&c, &env).unwrap();
    eq.update_control(&env);

    let mut ports = make_ports();
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let thermal_w = ports.thermal[0].sensible_gain_w;
    let electric_kw = ports.electrical.net_active_kw();
    let electric_w = electric_kw * 1_000.0;

    assert!(
        thermal_w > 0.0,
        "ASHP must deliver positive heat; got {thermal_w:.3} W"
    );
    assert!(
        electric_w > 0.0,
        "ASHP must draw electricity; got {electric_w:.3} W"
    );

    let cop = thermal_w / electric_w;
    // Telemetry COP should match the computed ratio within rounding
    let tel_cop = eq
        .telemetry()
        .get("cop")
        .expect("cop must exist in telemetry");

    assert!(
        cop > 2.0,
        "ASHP COP must exceed AHRI 210/240-2023 minimum of 2.0 at 7°C outdoor; \
         got port-derived COP={cop:.3}, telemetry COP={tel_cop:.3} \
         (thermal={thermal_w:.1} W, electric={electric_w:.1} W)"
    );
    assert!(
        cop < 6.0,
        "ASHP COP {cop:.3} unrealistically high (expected < 6.0 at 7°C)"
    );

    // Note: telemetry COP uses compressor-only power (AHRI/SEER convention),
    // while port-derived COP uses total electrical including fan. They legitimately
    // differ when fan power is non-negligible. Both are valid definitions; HARES
    // reports the AHRI convention in telemetry for consistency with rating databases.
    eprintln!(
        "[hvac_parity] h1_cop: port_cop={cop:.3}, telemetry_cop={tel_cop:.3} \
         (difference expected due to AHRI compressor-only convention)"
    );
}

// ---------------------------------------------------------------------------
// 5. ASHP heating: mode transitions with thermostat deadband
//
// OCHRE HVAC.py: thermostat uses a deadband; on transition is at
// (setpoint - deadband/2) for most modes. Tests that the unit is off
// when the zone is above setpoint+deadband and heating when below.
// ---------------------------------------------------------------------------
#[test]
fn ashp_thermostat_deadband_transitions() {
    let c = cfg(
        "ashp",
        "ASHP Heater",
        &[
            ("zone_id", 1.0),
            ("heating_setpoint_c", 21.0),
            ("cooling_setpoint_c", 27.0),
            ("backup_capacity_w", 0.0),
        ],
    );
    let registry = EquipmentRegistry::new();

    // Zone well below setpoint → must be in a heating mode (HP, ER, or combined)
    {
        let mut eq = registry.create("ASHP Heater", c.clone()).unwrap();
        let env = make_env(17.0, 5.0, 11.0);
        eq.init(&c, &env).unwrap();
        let mode = eq.update_control(&env);
        let is_heating = matches!(
            mode,
            OperatingMode::Heating
                | OperatingMode::HeatingHP
                | OperatingMode::HeatingER
                | OperatingMode::HeatingHPAndER
        );
        assert!(
            is_heating,
            "ASHP must demand heating when zone (17°C) is well below setpoint (21°C); got {mode:?}"
        );
    }

    // Zone well above setpoint → must be off (not any heating mode)
    {
        let mut eq = registry.create("ASHP Heater", c.clone()).unwrap();
        let env = make_env(24.0, 5.0, 18.0);
        eq.init(&c, &env).unwrap();
        let mode = eq.update_control(&env);
        let is_heating = matches!(
            mode,
            OperatingMode::Heating
                | OperatingMode::HeatingHP
                | OperatingMode::HeatingER
                | OperatingMode::HeatingHPAndER
        );
        assert!(
            !is_heating,
            "ASHP must NOT heat when zone (24°C) is well above setpoint (21°C); got {mode:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 6. Air conditioner: sign conventions and SHR split
//
// OCHRE AirConditioner.calculate_power_and_heat:
//   total_cooling_w = capacity * duty_cycle
//   sensible_w = total_cooling_w * SHR  (negative sign → removes heat)
//   latent_w   = total_cooling_w * (1 - SHR)
//
// Setup: cooling setpoint=24°C, zone=28°C, outdoor=35°C → cooling demanded.
// Verify: thermal port is negative, electric draw is positive.
// ---------------------------------------------------------------------------
#[test]
fn air_conditioner_sign_convention_and_shr_split() {
    let c = cfg(
        "ac",
        "Air Conditioner",
        &[
            ("zone_id", 1.0),
            ("capacity_w", 10_000.0),
            ("heating_setpoint_c", 20.0),
            ("cooling_setpoint_c", 24.0),
        ],
    );
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Air Conditioner", c.clone()).unwrap();
    // Hot summer day: zone above cooling setpoint, high outdoor temp
    let env = make_env(28.0, 35.0, 19.0);
    eq.init(&c, &env).unwrap();
    eq.update_control(&env);

    let mut ports = make_ports();
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let thermal_w = ports.thermal[0].sensible_gain_w;
    let electric_kw = ports.electrical.net_active_kw();

    // Cooling removes heat → thermal contribution must be negative
    assert!(
        thermal_w < -100.0,
        "AC must produce negative sensible thermal when cooling (zone=28°C > setpoint=24°C); \
         got {thermal_w:.3} W"
    );

    // Compressor draws positive electricity
    assert!(
        electric_kw > 0.0,
        "AC must draw positive electricity when cooling; got {electric_kw:.4} kW"
    );

    // COP for a cooling unit: coil_cooling / compressor_electric.
    // Port sensible includes fan heat offset, so use telemetry coil output.
    let coil_cooling_w = eq.telemetry().get("sensible_cooling_w").unwrap_or(0.0)
        + eq.telemetry().get("latent_cooling_w").unwrap_or(0.0);
    let compressor_kw = eq.telemetry().get("compressor_kw").unwrap_or(0.0);
    let cooling_cop = coil_cooling_w / (compressor_kw * 1_000.0).max(f64::MIN_POSITIVE);
    // EER ≥ 10 BTU/(Wh) corresponds to COP ≥ 2.93; SEER 14 minimum corresponds to COP ≈ 4.1.
    assert!(
        cooling_cop > 2.0 && cooling_cop < 8.0,
        "AC cooling COP must be in [2.0, 8.0] at rated conditions; got {cooling_cop:.3} \
         (coil={coil_cooling_w:.1} W, compressor={compressor_kw:.4} kW)"
    );
}

// ---------------------------------------------------------------------------
// 7. Air conditioner: setpoint override raises cooling threshold
//
// OCHRE HVAC.py: ThermalSetpoint signal updates the active cooling setpoint
// for the current control cycle.
// ---------------------------------------------------------------------------
#[test]
fn air_conditioner_setpoint_override() {
    let c = cfg(
        "ac",
        "Air Conditioner",
        &[
            ("zone_id", 1.0),
            ("capacity_w", 10_000.0),
            ("heating_setpoint_c", 20.0),
            ("cooling_setpoint_c", 24.0),
        ],
    );
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Air Conditioner", c.clone()).unwrap();
    // Zone at 25°C — just above original cooling setpoint (24°C)
    let env = make_env(25.0, 30.0, 18.0);
    eq.init(&c, &env).unwrap();

    // With original setpoint 24°C, zone at 25°C → cooling should be active
    let mode_before = eq.update_control(&env);
    assert_eq!(
        mode_before,
        OperatingMode::Cooling,
        "AC must cool at zone=25°C > setpoint=24°C"
    );

    // Raise cooling setpoint to 28°C — zone at 25°C is now below setpoint
    eq.apply_control(&ControlSignal::ThermalSetpoint {
        heating_setpoint_c: None,
        cooling_setpoint_c: Some(28.0),
        deadband_c: None,
    })
    .unwrap();

    let mode_after = eq.update_control(&env);
    assert_ne!(
        mode_after,
        OperatingMode::Cooling,
        "AC must stop cooling when setpoint raised to 28°C (zone=25°C < 28°C)"
    );
}

#[test]
fn ashp_cooler_two_speed_runtime_is_partial_near_setpoint() {
    let cfg = hp_cooler_cfg(
        "ashp-cooler",
        2,
        false,
        Some(vec![4_000.0, 8_000.0]),
        Some(vec![0.33, 0.33]),
    );
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("ASHP Cooler", cfg.clone()).unwrap();
    let env = make_env(24.1, 35.0, 18.0);
    eq.init(&cfg, &env).unwrap();
    eq.update_control(&env);

    let mut ports = make_ports();
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let rtf = eq
        .telemetry()
        .get("runtime_fraction")
        .expect("runtime_fraction must exist");
    let electric_kw = eq.telemetry().get("electric_kw").expect("electric_kw");

    assert!(
        rtf > 0.0 && rtf < 0.8,
        "two-speed ASHP cooler near setpoint must run at partial runtime, got rtf={rtf:.3}"
    );
    assert!(
        electric_kw > 0.0,
        "two-speed ASHP cooler must still draw power under partial runtime; got {electric_kw:.4} kW"
    );
}

#[test]
fn minisplit_cooling_has_no_first_step_startup_penalty() {
    let cfg = hp_cooler_cfg("mshp-cooler", 1, true, None, None);
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("MSHP Cooler", cfg.clone()).unwrap();
    let env = make_env(24.2, 35.0, 18.0);
    eq.init(&cfg, &env).unwrap();

    let mut ports_first = make_ports();
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports_first)
        .unwrap();
    let kw_first = eq.telemetry().get("electric_kw").expect("electric_kw");
    let rtf_first = eq
        .telemetry()
        .get("runtime_fraction")
        .expect("runtime_fraction");

    let mut ports_second = make_ports();
    eq.update_control(&env);
    eq.step(&env, Duration::from_secs(60), &mut ports_second)
        .unwrap();
    let kw_second = eq.telemetry().get("electric_kw").expect("electric_kw");
    let rtf_second = eq
        .telemetry()
        .get("runtime_fraction")
        .expect("runtime_fraction");

    assert!(
        (kw_first - kw_second).abs() < 0.02,
        "variable-speed minisplit cooling should not ramp between first and second step; first={kw_first:.4} kW second={kw_second:.4} kW"
    );
    assert!(
        (rtf_first - rtf_second).abs() < 0.02,
        "variable-speed minisplit runtime should be stable across repeated near-setpoint steps; first={rtf_first:.3} second={rtf_second:.3}"
    );
}

#[test]
fn minisplit_cooling_stage_match_runs_continuously() {
    let cfg = hp_cooler_cfg(
        "mshp-stage-match",
        4,
        true,
        Some(vec![2_000.0, 4_000.0, 6_000.0, 8_000.0]),
        Some(vec![0.20, 0.25, 0.30, 0.35]),
    );
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("MSHP Cooler", cfg.clone()).unwrap();
    let env = make_env(24.5, 35.0, 18.0);
    eq.init(&cfg, &env).unwrap();
    eq.update_control(&env);

    let mut ports = make_ports();
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let rtf = eq
        .telemetry()
        .get("runtime_fraction")
        .expect("runtime_fraction must exist");
    let electric_kw = eq.telemetry().get("electric_kw").expect("electric_kw");

    assert!(
        (rtf - 1.0).abs() < 1e-9,
        "an exact 4-speed mini-split stage match must run continuously, got rtf={rtf:.3}"
    );
    assert!(
        electric_kw > 0.0,
        "variable-speed mini-split stage selection must still draw power, got {electric_kw:.4} kW"
    );
}

// ---------------------------------------------------------------------------
// 8. ASHP defrost: OnDemand defrost engages at sub-freezing outdoor temp
//
// OCHRE HVAC.py ASHPHeater.update_capacity (lines ~1176-1231):
//   defrost_factor = 0.875 * (1 - defrost_time_fraction)
//   effective_capacity = rated_capacity * defrost_factor  (during defrost)
//
// HARES uses the EnergyPlus OnDemand humidity-based defrost model, which
// computes time_fraction from outdoor coil moisture accumulation. The model
// also applies extra_power_w from the defrost EIR modifier (DEFROST_EIR_TEMP_MODIFIER,
// a dimensionless 0.1528 scalar producing watts from watts — NOT kW).
//
// This test verifies that defrost engages at 0°C (below max_oat_defrost_c),
// that the unit continues to deliver heat, and that the port-derived COP
// stays above 0.5 — confirming the extra_power_w is correctly sized (~90-200 W
// of overhead for a 10 kW unit, not 1000× inflated).
// ---------------------------------------------------------------------------
#[test]
fn ashp_defrost_at_sub_freezing_outdoor_temp() {
    let c = cfg(
        "ashp",
        "ASHP Heater",
        &[
            ("zone_id", 1.0),
            ("heating_setpoint_c", 21.0),
            ("cooling_setpoint_c", 27.0),
            ("backup_capacity_w", 0.0),
        ],
    );
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("ASHP Heater", c.clone()).unwrap();
    // Outdoor at 0°C — at the defrost activation threshold
    let env = make_env(18.0, 0.0, 12.0);
    eq.init(&c, &env).unwrap();

    // Run 30 minutes at sub-freezing to allow defrost logic to accumulate
    for _ in 0..30 {
        let mut ports = make_ports();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
    }

    let tel = eq.telemetry();
    let defrost_active = tel
        .get("defrost_active")
        .expect("defrost_active must exist in telemetry");
    let thermal_output_w = tel
        .get("thermal_output_w")
        .expect("thermal_output_w must exist in telemetry");
    let electric_kw = tel
        .get("electric_kw")
        .expect("electric_kw must exist in telemetry");

    eprintln!(
        "[hvac_parity] defrost_at_0C: defrost_active={defrost_active:.0}, \
         thermal_output={thermal_output_w:.1} W, electric={electric_kw:.4} kW"
    );

    // Defrost must be active at 0°C — the OnDemand model triggers below max_oat_defrost_c
    // (typically 5°C), and 0°C is well within that range.
    assert!(
        defrost_active > 0.0,
        "ASHP defrost must be active at 0°C outdoor (below max_oat_defrost_c); \
         got defrost_active={defrost_active:.0}"
    );

    // The unit must deliver positive heat and draw positive electricity.
    assert!(
        thermal_output_w > 0.0,
        "ASHP must deliver positive thermal output at 0°C during defrost; \
         got {thermal_output_w:.1} W"
    );
    assert!(
        electric_kw > 0.0,
        "ASHP must draw positive electricity at 0°C; got {electric_kw:.4} kW"
    );

    // COP must be physically plausible. DEFROST_EIR_TEMP_MODIFIER is dimensionless
    // (0.1528 × capacity_W / 1.01667 → extra_power in W). With correct units, the
    // defrost overhead for a 10 kW unit is ~150 W, not 150 kW, so COP stays above 0.5.
    let cop = thermal_output_w / (electric_kw * 1_000.0);
    assert!(
        cop > 0.5,
        "ASHP COP during defrost at 0°C must be > 0.5 (heat pump, not resistance heater); \
         got COP={cop:.3} (thermal={thermal_output_w:.1} W, electric={electric_kw:.4} kW)"
    );
}

// ---------------------------------------------------------------------------
// 9. Furnace thermostat deadband: heating turns off above setpoint
//
// OCHRE HVAC.py Heater.update_external_control: unit stays on while zone is
// below (setpoint + deadband/2) and turns off when zone reaches setpoint.
// We verify the off→on and on→off transitions are sensible.
// ---------------------------------------------------------------------------
#[test]
fn furnace_thermostat_off_above_setpoint() {
    let c = EquipmentConfig::from_typed(
        "furnace".to_string(),
        "Gas Furnace".to_string(),
        GasFurnaceConfig {
            equipment_id: None,
            zone_id: Some(1),
            afue: 0.80,
            capacity_w: 10_000.0,
            number_of_speeds: 1,
            fan_power_w: Some(0.0),
            ducts: DuctConfig::default(),
        },
    );
    let registry = EquipmentRegistry::new();

    // 1. Zone well below setpoint → heating
    {
        let mut eq = registry.create("Gas Furnace", c.clone()).unwrap();
        let env = make_env(18.0, -5.0, 12.0);
        eq.init(&c, &env).unwrap();
        let mode = eq.update_control(&env);
        let mut ports = make_ports();
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        assert_eq!(
            mode,
            OperatingMode::Heating,
            "must heat at 18°C with 21°C setpoint"
        );
        assert!(
            ports.fuel.get(FuelType::Gas) > 0.0,
            "gas must flow when heating at 18°C"
        );
    }

    // 2. Zone above setpoint → off
    {
        let mut eq = registry.create("Gas Furnace", c.clone()).unwrap();
        let env = make_env(23.0, -5.0, 17.0);
        eq.init(&c, &env).unwrap();
        let mode = eq.update_control(&env);
        let mut ports = make_ports();
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        assert_ne!(
            mode,
            OperatingMode::Heating,
            "must not heat at 23°C with 21°C setpoint"
        );
        assert!(
            ports.fuel.get(FuelType::Gas) < 1e-6,
            "no gas when unit is off at 23°C"
        );
    }
}

fn make_ports_two_zones() -> PortSlots {
    PortSlots {
        thermal: vec![
            ThermalAccumulator::new(ZoneId(1)),
            ThermalAccumulator::new(ZoneId(2)),
        ],
        ..PortSlots::default()
    }
}

#[derive(Debug, Clone, Copy)]
struct HvacStepSnapshot {
    mode: OperatingMode,
    electric_kw: f64,
    sensible_w: f64,
    runtime_fraction: f64,
}

fn hvac_step_snapshot(
    eq: &mut dyn hares_equipment::Equipment,
    env: &EnvironmentState,
) -> HvacStepSnapshot {
    let mode = eq.update_control(env);
    let mut ports = make_ports();
    eq.step(env, Duration::from_secs(60), &mut ports).unwrap();
    HvacStepSnapshot {
        mode,
        electric_kw: ports.electrical.net_active_kw(),
        sensible_w: ports.thermal[0].sensible_gain_w,
        runtime_fraction: eq.telemetry().get("runtime_fraction").unwrap_or(0.0),
    }
}

fn two_speed_ac_config() -> EquipmentConfig {
    EquipmentConfig::from_typed(
        "two-speed-ac".to_string(),
        "Air Conditioner".to_string(),
        CentralAirConditionerConfig {
            equipment_id: None,
            zone_id: Some(1),
            capacity_w: 8_000.0,
            eir: 0.33,
            shr: Some(0.75),
            number_of_speeds: 2,
            stage_capacities_w: Some(vec![4_000.0, 8_000.0]),
            stage_eirs: Some(vec![0.33, 0.33]),
            stage_shrs: Some(vec![0.75, 0.75]),
            fan_power_w: Some(0.0),
            fan_power_w_per_cfm: None,
            cooling_setpoint_c: Some(24.0),
            heating_setpoint_c: Some(20.0),
            hysteresis_c: Some(1.0),
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
            airflow_m3_s_per_w: None,
            fraction_load_served: Some(1.0),
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            duct: DuctConfig::default(),
            system_type: None,
            startup_cd: Some(0.0),
            biquadratic_x1_min: None,
            biquadratic_x1_max: None,
            biquadratic_x2_min: None,
            biquadratic_x2_max: None,
            ff_min: None,
            ff_max: None,
            plf_min: None,
            plf_max: None,
        },
    )
}

// ---------------------------------------------------------------------------
// 10. Two-speed ASHP: heating at low outdoor temp falls back to backup
//
// OCHRE HVAC.py: when outdoor temp drops below hp_lockout_temp_c, the heat
// pump compressor is locked out and ER backup takes over.
// This tests that the equipment does not crash and produces positive thermal
// output even at extreme cold (-20°C), whether via HP or backup resistance.
// ---------------------------------------------------------------------------
#[test]
fn ashp_heating_at_extreme_cold() {
    let c = cfg(
        "ashp",
        "ASHP Heater",
        &[
            ("zone_id", 1.0),
            ("heating_setpoint_c", 21.0),
            ("cooling_setpoint_c", 27.0),
            ("backup_capacity_w", 5_000.0),
        ],
    );
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("ASHP Heater", c.clone()).unwrap();
    let env = make_env(15.0, -20.0, 9.0);
    eq.init(&c, &env).unwrap();
    eq.update_control(&env);

    let mut ports = make_ports();
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let thermal_w = ports.thermal[0].sensible_gain_w;
    let electric_kw = ports.electrical.net_active_kw();

    // Either the HP or backup resistance must deliver positive heat
    assert!(
        thermal_w > 0.0,
        "ASHP (with backup) must deliver positive heat at -20°C; got {thermal_w:.3} W"
    );
    assert!(
        electric_kw > 0.0,
        "ASHP (with backup) must draw electricity at -20°C; got {electric_kw:.4} kW"
    );

    eprintln!(
        "[hvac_parity] extreme_cold: thermal={thermal_w:.1} W, electric={electric_kw:.4} kW, \
         COP={:.3}",
        thermal_w / (electric_kw * 1_000.0)
    );
}

// ---------------------------------------------------------------------------
// 11. AC deadband: zero output when zone is below cooling setpoint
//
// An AC is a one-way cooling device. When zone temp is below the cooling
// setpoint minus the deadband half-width, the thermostat is satisfied and
// the unit must be fully off.
//
// Setup: cooling_setpoint=24°C, zone=22°C, outdoor=20°C.
// Deadband default is ~1°C, so the lower edge is ~23.5°C; zone at 22°C is
// comfortably inside the satisfied region.
// Expected: mode != Cooling, thermal == 0.0 W, electric == 0.0 kW.
// ---------------------------------------------------------------------------
#[test]
fn ac_deadband_no_output_when_zone_below_cooling_setpoint() {
    let c = cfg(
        "ac",
        "Air Conditioner",
        &[
            ("zone_id", 1.0),
            ("capacity_w", 10_000.0),
            ("heating_setpoint_c", 20.0),
            ("cooling_setpoint_c", 24.0),
        ],
    );
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Air Conditioner", c.clone()).unwrap();
    let env = make_env(22.0, 20.0, 15.0);
    eq.init(&c, &env).unwrap();
    let mode = eq.update_control(&env);

    let mut ports = make_ports();
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let thermal_w = ports.thermal[0].sensible_gain_w;
    let electric_kw = ports.electrical.net_active_kw();

    assert_ne!(
        mode,
        OperatingMode::Cooling,
        "AC must not be in Cooling mode when zone (22°C) is below setpoint (24°C); got {mode:?}"
    );
    assert!(
        thermal_w.abs() < 1.0,
        "AC must deliver zero thermal output when off; got {thermal_w:.4} W"
    );
    assert!(
        electric_kw.abs() < 0.001,
        "AC must draw zero electricity when off; got {electric_kw:.6} kW"
    );
}

// ---------------------------------------------------------------------------
// 12. ASHP COP drops when defrost activates
//
// Carnot COP degrades as outdoor temperature falls. For HARES with identity
// biquadratic curves ([1,0,0,0,0,0]), the compressor performance is identical
// at 15°C and 7°C. The observable COP drop occurs when the OnDemand defrost
// model activates below max_oat_defrost_c (default ~5°C). Defrost diverts
// compressor capacity and adds extra resistive power, lowering net COP.
//
// Expected:
//   COP at 7°C (no defrost) >= COP at 0°C (defrost active)
//   COP at 0°C > COP at -10°C (deeper defrost penalty)
//   All COPs positive (heat is delivered at all three outdoor temperatures)
// ---------------------------------------------------------------------------
#[test]
fn ashp_cop_drops_with_defrost() {
    let make_ashp_cop = |outdoor_c: f64| {
        let c = cfg(
            "ashp",
            "ASHP Heater",
            &[
                ("zone_id", 1.0),
                ("heating_setpoint_c", 21.0),
                ("cooling_setpoint_c", 27.0),
                ("backup_capacity_w", 0.0),
            ],
        );
        let registry = EquipmentRegistry::new();
        let mut eq = registry.create("ASHP Heater", c.clone()).unwrap();
        let env = make_env(19.0, outdoor_c, outdoor_c - 2.0);
        eq.init(&c, &env).unwrap();
        // Run 30 steps to allow defrost accumulation to reach steady state.
        for _ in 0..30 {
            eq.update_control(&env);
            let mut ports = make_ports();
            eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        }
        let tel = eq.telemetry();
        let thermal_w = tel.get("thermal_output_w").expect("thermal_output_w");
        let electric_kw = tel.get("electric_kw").expect("electric_kw");
        let cop = thermal_w / (electric_kw * 1_000.0);
        (cop, thermal_w, electric_kw)
    };

    let (cop_7, thermal_7, elec_7) = make_ashp_cop(7.0);
    let (cop_0, thermal_0, elec_0) = make_ashp_cop(0.0);
    let (cop_m10, thermal_m10, elec_m10) = make_ashp_cop(-10.0);

    assert!(cop_7 > 0.0, "COP must be positive at 7°C; got {cop_7:.3}");
    assert!(cop_0 > 0.0, "COP must be positive at 0°C; got {cop_0:.3}");
    assert!(
        cop_m10 > 0.0,
        "COP must be positive at -10°C; got {cop_m10:.3}"
    );

    // At 0°C and -10°C defrost is active; at 7°C it is not.
    // COP with defrost must be lower than COP without defrost.
    assert!(
        cop_7 >= cop_0,
        "COP at 7°C ({cop_7:.3}) must be >= COP at 0°C ({cop_0:.3}) \
         (defrost active at 0°C; outdoor_temp < max_oat_defrost_c)"
    );
    assert!(
        cop_0 > cop_m10,
        "COP at 0°C ({cop_0:.3}) must exceed COP at -10°C ({cop_m10:.3}) \
         (deeper defrost penalty at -10°C)"
    );

    eprintln!(
        "[hvac_parity] ashp_cop_vs_outdoor: \
         7°C: cop={cop_7:.3} (thermal={thermal_7:.1} W, elec={elec_7:.4} kW) | \
         0°C: cop={cop_0:.3} (thermal={thermal_0:.1} W, elec={elec_0:.4} kW) | \
         -10°C: cop={cop_m10:.3} (thermal={thermal_m10:.1} W, elec={elec_m10:.4} kW)"
    );
}

// ---------------------------------------------------------------------------
// 13. AC first law: sensible + latent = total cooling
//
// When an AC dehumidifies air, total heat removed equals sensible heat
// removed plus latent heat of condensation. This is a direct consequence
// of the first law applied to the moist-air coil model.
//
// Setup: zone=28°C, outdoor=35°C, wet_bulb=19°C → cooling active.
// Expected:
//   sensible_cooling_w + latent_cooling_w ≈ |sensible_gain_w| + |latent_gain_w|
//   Both components negative in the port (heat removed from zone).
//   Tolerance: 1.0 W (floating-point rounding in DSE application).
// ---------------------------------------------------------------------------
#[test]
fn ac_sensible_plus_latent_equals_total() {
    let c = cfg(
        "ac",
        "Air Conditioner",
        &[
            ("zone_id", 1.0),
            ("capacity_w", 10_000.0),
            ("heating_setpoint_c", 20.0),
            ("cooling_setpoint_c", 24.0),
            // Disable startup ramp so first-step output reflects full capacity.
            ("startup_cd", 0.0),
        ],
    );
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Air Conditioner", c.clone()).unwrap();
    let env = make_env(28.0, 35.0, 19.0);
    eq.init(&c, &env).unwrap();
    eq.update_control(&env);

    let mut ports = make_ports();
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let sensible_port_w = ports.thermal[0].sensible_gain_w;
    let latent_port_w = ports.thermal[0].latent_gain_w;

    // Ports accumulate cooling as negative values (heat removed from zone).
    assert!(
        sensible_port_w < -100.0,
        "sensible_gain_w must be negative (heat removed); got {sensible_port_w:.3} W"
    );
    assert!(
        latent_port_w <= 0.0,
        "latent_gain_w must be <= 0 (moisture removed); got {latent_port_w:.3} W"
    );

    // Fan heat partially offsets cooling at the port level:
    // port_sensible = -(sensible_cooling - fan_heat)
    let fan_kw = eq.telemetry().get("fan_kw").unwrap_or(0.0);
    let fan_heat_w = fan_kw * 1000.0;
    let total_port_w = sensible_port_w.abs() + latent_port_w.abs();

    // Telemetry reports pre-fan-offset cooling (coil output only).
    let sens_tel = eq
        .telemetry()
        .get("sensible_cooling_w")
        .expect("sensible_cooling_w");
    let lat_tel = eq
        .telemetry()
        .get("latent_cooling_w")
        .expect("latent_cooling_w");

    // First law: coil cooling - fan heat = net port cooling
    let net_cooling_tel = sens_tel + lat_tel - fan_heat_w;
    assert!(
        (net_cooling_tel - total_port_w).abs() < 1.0,
        "coil_total ({:.3} W) - fan_heat ({fan_heat_w:.3} W) = net {net_cooling_tel:.3} W \
         must equal port total {total_port_w:.3} W (tolerance 1.0 W)",
        sens_tel + lat_tel,
    );
}

// ---------------------------------------------------------------------------
// 14. Gas furnace first law: thermal output < fuel input
//
// For any combustion device with AFUE < 1.0, delivered thermal energy must
// be strictly less than the chemical energy consumed. The ratio equals AFUE.
//
// Setup: capacity=15000 W, AFUE=0.80, fan_power=0 W, zone=18°C → heating.
// Expected:
//   thermal_output_w < fuel_input_w (first law, AFUE < 1.0)
//   thermal_output_w / fuel_input_w ≈ 0.80 within 1%
//   thermal_output_w > 0 (heating is active)
// ---------------------------------------------------------------------------
#[test]
fn gas_furnace_first_law_thermal_less_than_fuel() {
    let c = EquipmentConfig::from_typed(
        "furnace".to_string(),
        "Gas Furnace".to_string(),
        GasFurnaceConfig {
            equipment_id: None,
            zone_id: Some(1),
            afue: 0.80,
            capacity_w: 15_000.0,
            number_of_speeds: 1,
            fan_power_w: Some(0.0),
            ducts: DuctConfig::default(),
        },
    );
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Gas Furnace", c.clone()).unwrap();
    let env = make_env(18.0, -5.0, 12.0);
    eq.init(&c, &env).unwrap();
    eq.update_control(&env);

    let mut ports = make_ports();
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let tel = eq.telemetry();
    let thermal_w = tel.get("thermal_output_w").expect("thermal_output_w");
    let fuel_w = tel.get("fuel_input_w").expect("fuel_input_w");

    assert!(
        thermal_w > 0.0,
        "furnace must deliver positive heat at 18°C; got {thermal_w:.1} W"
    );
    assert!(
        thermal_w < fuel_w,
        "first law: thermal_output ({thermal_w:.1} W) must be < fuel_input ({fuel_w:.1} W)"
    );

    // AFUE = thermal / fuel. Hand calculation: 15000 / (15000 / 0.80) = 0.80.
    let effective_afue = thermal_w / fuel_w;
    assert!(
        (effective_afue - 0.80).abs() < 0.01,
        "effective AFUE must be ~0.80; got {effective_afue:.4} \
         (thermal={thermal_w:.1} W, fuel={fuel_w:.1} W)"
    );
}

// ---------------------------------------------------------------------------
// 15. Gas furnace DSE multi-zone energy conservation
//
// With duct_dse < 1.0 and an explicit duct_zone_id, the furnace must route
// the gross capacity to both zones such that:
//   zone_1 + zone_2 = rated_capacity (energy conservation)
//   zone_1 = rated * DSE
//   zone_2 = rated * (1 - DSE)
//
// This exercises the full equipment pipeline: config parsing, zone_heat_fractions
// initialisation, and write_zone_thermal_contributions at step time.
//
// Setup: capacity=10000 W, DSE=0.80, duct_zone_id=2, fan_power=0 W.
// Expected (tolerance 10 W for floating-point):
//   zone_1_thermal ≈ 8000 W
//   zone_2_thermal ≈ 2000 W
//   zone_1 + zone_2 ≈ 10000 W
// ---------------------------------------------------------------------------
#[test]
fn gas_furnace_dse_multi_zone_energy_conservation() {
    let c = EquipmentConfig::from_typed(
        "furnace".to_string(),
        "Gas Furnace".to_string(),
        GasFurnaceConfig {
            equipment_id: None,
            zone_id: Some(1),
            afue: 0.80,
            capacity_w: 10_000.0,
            number_of_speeds: 1,
            fan_power_w: Some(0.0),
            ducts: DuctConfig {
                dse_heat: Some(0.80),
                duct_zone_id: Some(2),
                ..DuctConfig::default()
            },
        },
    );
    let registry = EquipmentRegistry::new();
    let mut eq = registry.create("Gas Furnace", c.clone()).unwrap();
    let env = make_env(18.0, -5.0, 12.0);
    eq.init(&c, &env).unwrap();
    eq.update_control(&env);

    let mut ports = make_ports_two_zones();
    eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

    let zone1_w = ports.thermal[0].sensible_gain_w;
    let zone2_w = ports.thermal[1].sensible_gain_w;
    let total_w = zone1_w + zone2_w;

    // zone_1 = 10000 * 0.80 = 8000 W
    assert!(
        (zone1_w - 8_000.0).abs() < 10.0,
        "conditioned zone must receive capacity * DSE = 8000 W; got {zone1_w:.2} W"
    );
    // zone_2 = 10000 * (1 - 0.80) = 2000 W
    assert!(
        (zone2_w - 2_000.0).abs() < 10.0,
        "duct zone must receive capacity * (1-DSE) = 2000 W; got {zone2_w:.2} W"
    );
    // Conservation: zone_1 + zone_2 = rated capacity
    assert!(
        (total_w - 10_000.0).abs() < 10.0,
        "total delivered heat must conserve energy: zone1 + zone2 = 10000 W; \
         got zone1={zone1_w:.2} W + zone2={zone2_w:.2} W = {total_w:.2} W"
    );
}

// ---------------------------------------------------------------------------
// 16. Default asymmetric deadband matches documented turn-on / turn-off edges
//
// OCHRE-style thermostat thresholds (deadband_offset = 0.2, hysteresis = 1.0 C):
//   Heating turn_on  = setpoint - 0.8 C
//   Heating turn_off = setpoint + 0.2 C
//   Cooling turn_on  = setpoint + 0.8 C
//   Cooling turn_off = setpoint - 0.2 C
//
// This regression checks both the activation edge and the hysteresis hold region
// through the public Equipment API for a heater and a cooler.
// ---------------------------------------------------------------------------
#[test]
fn hvac_default_deadband_matrix_matches_ochre_thresholds() {
    let registry = EquipmentRegistry::new();

    let furnace_cfg = EquipmentConfig::from_typed(
        "furnace".to_string(),
        "Gas Furnace".to_string(),
        GasFurnaceConfig {
            equipment_id: None,
            zone_id: Some(1),
            afue: 0.80,
            capacity_w: 10_000.0,
            number_of_speeds: 1,
            fan_power_w: Some(0.0),
            ducts: DuctConfig::default(),
        },
    );
    let mut furnace = registry.create("Gas Furnace", furnace_cfg.clone()).unwrap();
    furnace
        .init(&furnace_cfg, &make_env_at_minute(19.0, 0.0, 13.0, 0))
        .unwrap();
    furnace
        .apply_control(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(21.0),
            cooling_setpoint_c: None,
            deadband_c: None,
        })
        .unwrap();

    let heating_on = hvac_step_snapshot(furnace.as_mut(), &make_env_at_minute(20.15, 0.0, 13.0, 0));
    let heating_hold =
        hvac_step_snapshot(furnace.as_mut(), &make_env_at_minute(21.15, 0.0, 13.0, 1));
    let heating_off =
        hvac_step_snapshot(furnace.as_mut(), &make_env_at_minute(21.25, 0.0, 13.0, 2));

    assert_eq!(
        heating_on.mode,
        OperatingMode::Heating,
        "heating must engage below 21.0-0.8=20.2 C; got {:?}",
        heating_on.mode
    );
    assert_eq!(
        heating_hold.mode,
        OperatingMode::Heating,
        "heating must stay latched below 21.0+0.2=21.2 C; got {:?}",
        heating_hold.mode
    );
    assert_ne!(
        heating_off.mode,
        OperatingMode::Heating,
        "heating must release above 21.0+0.2=21.2 C; got {:?}",
        heating_off.mode
    );

    let ac_cfg = cfg(
        "ac",
        "Air Conditioner",
        &[
            ("zone_id", 1.0),
            ("capacity_w", 10_000.0),
            ("heating_setpoint_c", 20.0),
            ("cooling_setpoint_c", 24.0),
        ],
    );
    let mut ac = registry.create("Air Conditioner", ac_cfg.clone()).unwrap();
    ac.init(&ac_cfg, &make_env_at_minute(25.0, 32.0, 18.0, 0))
        .unwrap();

    let cooling_on = hvac_step_snapshot(ac.as_mut(), &make_env_at_minute(24.85, 32.0, 18.0, 0));
    let cooling_hold = hvac_step_snapshot(ac.as_mut(), &make_env_at_minute(23.85, 32.0, 18.0, 1));
    let cooling_off = hvac_step_snapshot(ac.as_mut(), &make_env_at_minute(23.75, 32.0, 18.0, 2));

    assert_eq!(
        cooling_on.mode,
        OperatingMode::Cooling,
        "cooling must engage above 24.0+0.8=24.8 C; got {:?}",
        cooling_on.mode
    );
    assert_eq!(
        cooling_hold.mode,
        OperatingMode::Cooling,
        "cooling must stay latched above 24.0-0.2=23.8 C; got {:?}",
        cooling_hold.mode
    );
    assert_ne!(
        cooling_off.mode,
        OperatingMode::Cooling,
        "cooling must release below 24.0-0.2=23.8 C; got {:?}",
        cooling_off.mode
    );
}

// ---------------------------------------------------------------------------
// 17. Two-speed setpoint control drops from high stage to low-stage cycling
//
// Staging rule: when the thermostat is already calling, load_fraction below the
// low-speed capacity fraction should fall back to stage 0 and cycle there.
// With low_speed_capacity_fraction = 0.5 and load_fraction = 0.4, the expected
// low-stage PLR is 0.4 / 0.5 = 0.8.
// ---------------------------------------------------------------------------
#[test]
fn two_speed_ac_setpoint_matrix_transitions_from_high_to_low_stage() {
    let registry = EquipmentRegistry::new();
    let cfg = two_speed_ac_config();
    let mut eq = registry.create("Air Conditioner", cfg.clone()).unwrap();
    eq.init(&cfg, &make_env_at_minute(27.0, 35.0, 20.0, 0))
        .unwrap();

    let mut high = None;
    for minute in 0..=5 {
        high = Some(hvac_step_snapshot(
            eq.as_mut(),
            &make_env_at_minute(27.0, 35.0, 20.0, minute),
        ));
    }
    let high = high.expect("high-stage snapshot after dwell");

    let mut low = None;
    for minute in 6..=11 {
        low = Some(hvac_step_snapshot(
            eq.as_mut(),
            &make_env_at_minute(24.4, 35.0, 18.0, minute),
        ));
    }
    let low = low.expect("low-stage snapshot after stage-down dwell");

    assert_eq!(high.mode, OperatingMode::Cooling);
    assert_eq!(
        low.mode,
        OperatingMode::Cooling,
        "cooling call should persist inside the hysteresis hold region"
    );
    assert!(
        (high.runtime_fraction - 1.0).abs() < 1e-9,
        "high-load two-speed call should run full runtime fraction; got {}",
        high.runtime_fraction
    );
    assert!(
        (low.runtime_fraction - 0.8).abs() < 1e-9,
        "load_fraction 0.4 with low stage fraction 0.5 should cycle stage 0 at PLR=0.8; got {}",
        low.runtime_fraction
    );
    assert!(
        low.electric_kw < high.electric_kw,
        "falling back to low stage must reduce electric draw: high={} kW low={} kW",
        high.electric_kw,
        low.electric_kw
    );
    assert!(
        low.sensible_w.abs() < high.sensible_w.abs(),
        "low-stage cycling must remove less sensible heat: high={} W low={} W",
        high.sensible_w,
        low.sensible_w
    );
}

// ---------------------------------------------------------------------------
// 18. Two-speed setpoint staging honors the 300 s dwell before escalating
//
// The typed two-speed AC defaults to setpoint-based staging with a 300 s
// minimum time per speed. Under sustained high load it should stay at low
// stage for the first five 1-minute steps, then escalate to high stage once
// the dwell has elapsed.
// ---------------------------------------------------------------------------
#[test]
fn two_speed_ac_stage_lockout_holds_low_speed_until_dwell_elapses() {
    let registry = EquipmentRegistry::new();
    let cfg = two_speed_ac_config();
    let mut eq = registry.create("Air Conditioner", cfg.clone()).unwrap();
    eq.init(&cfg, &make_env_at_minute(27.0, 35.0, 20.0, 0))
        .unwrap();

    let mut snapshots = Vec::new();
    for minute in 0..=5 {
        snapshots.push(hvac_step_snapshot(
            eq.as_mut(),
            &make_env_at_minute(27.0, 35.0, 20.0, minute),
        ));
    }

    for (idx, snap) in snapshots.iter().enumerate().take(5) {
        assert_eq!(
            snap.mode,
            OperatingMode::Cooling,
            "two-speed AC must stay cooling during pre-escalation step {idx}"
        );
        assert!(
            (snap.runtime_fraction - 1.0).abs() < 1e-9,
            "low stage is saturated at full runtime under the large load before dwell expiry; got {} at step {idx}",
            snap.runtime_fraction
        );
    }

    let low_stage_kw = snapshots[0].electric_kw;
    let high_stage_kw = snapshots[5].electric_kw;
    assert!(
        snapshots[..5]
            .iter()
            .all(|snap| (snap.electric_kw - low_stage_kw).abs() < 1e-9),
        "electric draw must stay locked to the low stage before the 300 s dwell expires"
    );
    assert!(
        high_stage_kw > low_stage_kw * 1.5,
        "after 300 s of sustained high load the controller must escalate to high speed; low={low_stage_kw:.4} kW high={high_stage_kw:.4} kW"
    );
    assert!(
        snapshots[5].sensible_w.abs() > snapshots[0].sensible_w.abs() * 1.5,
        "high stage after dwell expiry must remove materially more heat"
    );
}

// ---------------------------------------------------------------------------
// 19. Heat-pump heating lockout matrix switches among HP, ER, and Off
//
// With hp_lockout_temp_c = 10 C and er_lockout_temp_c = 5 C:
//   outdoor > 10 C  -> compressor heating is available, ER locked out
//   5 C < outdoor <= 10 C -> HP locked out and ER still blocked -> no heat
//   outdoor <= 5 C -> HP locked out and ER permitted
// ---------------------------------------------------------------------------
#[test]
fn ashp_lockout_matrix_matches_outdoor_thresholds() {
    let registry = EquipmentRegistry::new();
    let cfg = EquipmentConfig::from_typed(
        "ashp".to_string(),
        "ASHP Heater".to_string(),
        HeatPumpHeaterConfig {
            equipment_id: None,
            zone_id: Some(1),
            heating_capacity_w: Some(8_000.0),
            heating_eir: Some(3.412_141_633 / 9.0),
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            backup_fuel: None,
            backup_capacity_w: Some(5_000.0),
            backup_eir: None,
            fraction_heating_load_served: Some(1.0),
            cooling_capacity_w: Some(8_000.0),
            cooling_eir: Some(3.412_141_633 / 14.0),
            stage_cooling_capacities_w: None,
            stage_cooling_eirs: None,
            stage_shrs: None,
            fraction_cooling_load_served: Some(1.0),
            number_of_speeds: 1,
            is_mini_split: false,
            shr: Some(0.75),
            fan_power_w: Some(0.0),
            fan_power_w_per_cfm: None,
            airflow_m3_s_per_w: None,
            heating_setpoint_c: Some(21.0),
            cooling_setpoint_c: Some(27.0),
            hysteresis_c: Some(1.0),
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
            hp_lockout_temp_c: Some(10.0),
            er_lockout_temp_c: Some(5.0),
            max_oat_supplemental_c: Some(50.0),
            er_setpoint_offset_c: None,
            er_hard_lockout_time_s: None,
            duct: DuctConfig::default(),
            biquadratic_x1_min: None,
            biquadratic_x1_max: None,
            biquadratic_x2_min: None,
            biquadratic_x2_max: None,
            ff_min: None,
            ff_max: None,
            plf_min: None,
            plf_max: None,
        },
    );

    let mut mild = registry.create("ASHP Heater", cfg.clone()).unwrap();
    mild.init(&cfg, &make_env_at_minute(17.0, 12.0, 11.0, 0))
        .unwrap();
    let mild_step = hvac_step_snapshot(mild.as_mut(), &make_env_at_minute(17.0, 12.0, 11.0, 0));
    assert_eq!(
        mild_step.mode,
        OperatingMode::HeatingHP,
        "outdoor air above the HP lockout should allow compressor-only heating"
    );
    assert!(mild_step.sensible_w > 0.0);

    let mut shoulder = registry.create("ASHP Heater", cfg.clone()).unwrap();
    shoulder
        .init(&cfg, &make_env_at_minute(17.0, 7.0, 11.0, 0))
        .unwrap();
    let shoulder_step =
        hvac_step_snapshot(shoulder.as_mut(), &make_env_at_minute(17.0, 7.0, 11.0, 0));
    assert_eq!(
        shoulder_step.mode,
        OperatingMode::Off,
        "between the ER and HP lockouts neither heat source should be available"
    );
    assert!(shoulder_step.electric_kw.abs() < 1e-6);
    assert!(shoulder_step.sensible_w.abs() < 1e-6);

    let mut cold = registry.create("ASHP Heater", cfg.clone()).unwrap();
    cold.init(&cfg, &make_env_at_minute(17.0, 0.0, 11.0, 0))
        .unwrap();
    let cold_step = hvac_step_snapshot(cold.as_mut(), &make_env_at_minute(17.0, 0.0, 11.0, 0));
    assert_eq!(
        cold_step.mode,
        OperatingMode::HeatingER,
        "below the ER lockout the backup element should carry the heating call"
    );
    assert!(cold_step.sensible_w > 0.0);
}
