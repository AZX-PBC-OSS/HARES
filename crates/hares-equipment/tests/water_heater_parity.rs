//! Water heater equipment step correctness tests.
//!
//! Tests physics correctness against hand-calculated OCHRE reference values.
//! Documents divergences where HARES intentionally differs from OCHRE.
//!
//! Reference: vendors/OCHRE/ochre/Equipment/WaterHeater.py
//!            vendors/OCHRE/ochre/Models/Water.py

use std::time::Duration;

use chrono::{FixedOffset, TimeZone};
use hares_equipment::water_heater::gas::GasWH;
use hares_equipment::water_heater::resistance::ResistanceWH;
use hares_equipment::{
    ElectricResistanceWaterHeaterConfig, Equipment, EquipmentConfig, GasWaterHeaterConfig,
    HeatPumpWaterHeaterConfig,
};
use hares_types::{
    EnvironmentState, FuelType, GridState, PortSlots, ThermalCategory, WeatherState, ZoneId,
    ZoneState, telemetry_keys,
};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn make_env(zone_temp_c: f64) -> EnvironmentState {
    EnvironmentState {
        zones: vec![ZoneState {
            id: ZoneId(1),
            temperature_c: zone_temp_c,
            humidity_ratio: 0.008,
            volume_m3: 200.0,
        }],
        weather: WeatherState {
            outdoor_temp_c: zone_temp_c,
            outdoor_humidity_ratio: 0.005,
            outdoor_wet_bulb_c: zone_temp_c - 5.0,
            outdoor_enthalpy_j_kg: 0.0,
            wind_speed_m_s: 1.5,
            wind_dir_deg: 0.0,
            ground_temp_c: zone_temp_c - 2.0,
            sky_temp_c: zone_temp_c - 10.0,
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
            ground_t_mean_c: 10.0,
            ground_t_amplitude_c: 0.0,
            ground_phase_day: 35.0,
            day_of_year: 1.0,
        },
        grid: GridState {
            voltage_pu: 1.0,
            frequency_hz: 60.0,
            island_bus_voltage_pu: None,
        },
        custom_domains: vec![],
        equipment_telemetry: std::collections::HashMap::new(),
        equipment_core: Default::default(),
        current_time: FixedOffset::east_opt(0)
            .expect("UTC offset")
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .expect("valid"),
        time_res: chrono::Duration::seconds(60),
        price_signal: Default::default(),
        electrical: Default::default(),
    }
}

fn make_env_at_minute(zone_temp_c: f64, minute: i64) -> EnvironmentState {
    let mut env = make_env(zone_temp_c);
    env.current_time += chrono::Duration::minutes(minute);
    env
}

fn resistance_cfg(
    setpoint_c: f64,
    deadband_c: f64,
    initial_tank_temp_c: f64,
    draw_kg_s: f64,
    ua_w_per_k: f64,
) -> EquipmentConfig {
    EquipmentConfig::from_typed(
        "RWH".to_string(),
        "Resistance Water Heater".to_string(),
        ElectricResistanceWaterHeaterConfig {
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            energy_factor: None,
            uniform_energy_factor: None,
            heating_capacity_w: None,
            ua_w_per_k: Some(ua_w_per_k),
            setpoint_c: Some(setpoint_c),
            deadband_c: Some(deadband_c),
            max_tank_temp_c: Some(300.0),
            initial_tank_temp_c: Some(initial_tank_temp_c),
            tank_nodes: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: Some(draw_kg_s),
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
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
    .unwrap()
}

fn step_wh(wh: &mut dyn hares_equipment::Equipment, env: &EnvironmentState, ports: &mut PortSlots) {
    ports.electrical = Default::default();
    ports.fuel = Default::default();
    ports.thermal.iter_mut().for_each(|t| t.zero());
    ports.fluid.iter_mut().for_each(|f| f.zero());
    wh.step(env, Duration::from_secs(60), ports).unwrap();
}

#[derive(Debug, Clone, Copy)]
struct HpwhStepSnapshot {
    mode: hares_types::OperatingMode,
    compressor_power_w: f64,
    backup_power_w: f64,
}

fn hpwh_step_snapshot(
    wh: &mut dyn hares_equipment::Equipment,
    env: &EnvironmentState,
    ports: &mut PortSlots,
) -> HpwhStepSnapshot {
    let mode = wh.update_control(env);
    step_wh(wh, env, ports);
    HpwhStepSnapshot {
        mode,
        compressor_power_w: wh.telemetry().get("compressor_power_w").unwrap_or(0.0),
        backup_power_w: wh.telemetry().get("backup_element_power_w").unwrap_or(0.0),
    }
}

fn hpwh_cfg_with(
    initial_tank_temp_c: f64,
    deadband_c: f64,
    backup_enable_offset_c: f64,
    min_on_time_s: f64,
    min_off_time_s: f64,
    hp_only_mode: bool,
    element_hp_control_mode: Option<&str>,
) -> EquipmentConfig {
    EquipmentConfig::from_typed(
        "HPWH".to_string(),
        "Heat Pump Water Heater".to_string(),
        HeatPumpWaterHeaterConfig {
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            cop: Some(2.5),
            backup_element_power_w: Some(4_500.0),
            ua_w_per_k: Some(2.0),
            setpoint_c: Some(52.0),
            deadband_c: Some(deadband_c),
            max_tank_temp_c: Some(300.0),
            initial_tank_temp_c: Some(initial_tank_temp_c),
            tank_nodes: None,
            tempering_valve_setpoint_c: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: Some(0.0),
            compressor_power_w: Some(1_200.0),
            backup_enable_offset_c: Some(backup_enable_offset_c),
            min_ambient_temp_c: None,
            max_ambient_temp_c: None,
            min_on_time_s: Some(min_on_time_s),
            min_off_time_s: Some(min_off_time_s),
            hp_only_mode: Some(hp_only_mode),
            element_hp_control_mode: element_hp_control_mode.map(str::to_string),
            fan_power_w: Some(0.0),
            parasitic_power_w: Some(0.0),
            backup_efficiency: Some(1.0),
            shr: None,
            lost_heat_fraction: None,
            wall_heat_fraction: None,
            capacity_biquadratic_coeffs: None,
            cop_biquadratic_coeffs: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            fixture_delivery_temp_c: None,
            low_power_hpwh: None,
            uniform_energy_factor: None,
        },
    )
    .unwrap()
}

// ---------------------------------------------------------------------------
// 1. Standby loss over 24 hours
//
// OCHRE WaterHeater.py: standby loss = UA × (T_tank - T_ambient) per step.
// At equilibrium (no draws, element off), tank temperature decays exponentially.
//
// Setup: 50-gal tank at 51.7°C (125°F), ambient 20°C, UA=2.0 W/K, no draws.
// Setpoint set well below initial temperature so element never fires.
//
// Expected result: tank temperature must drop (standby losses are non-zero).
// The temperature drop over 24 hours with UA=2.0 W/K, ΔT≈31.7 K gives
// ~63 W average standby loss → ΔE = 63 × 86400 = 5.45 MJ over 24 hours.
// With tank thermal mass ≈ 189.3 kg × 4183 J/(kg·K) = 791.9 kJ/K, the
// drop ΔT ≈ 5.45 MJ / 791.9 kJ/K ≈ 6.9°C (exact answer is slightly less
// due to exponential decay reducing ΔT as temperature drops).
// ---------------------------------------------------------------------------
#[test]
fn standby_loss_over_24h() {
    let env = make_env(20.0);
    // Setpoint below initial: element never fires → only standby losses
    let cfg = resistance_cfg(40.0, 2.0, 51.7, 0.0, 2.0);

    let mut wh = ResistanceWH::new(cfg.clone());
    wh.init(&cfg, &env).unwrap();
    let mut ports = PortSlots::from_declarations(wh.ports());

    // Telemetry is populated after the first step, so use the known initial temp from config.
    let initial_temp = 51.7_f64;

    // 24 hours = 1440 steps at 60 s each
    for _ in 0..1440 {
        step_wh(&mut wh, &env, &mut ports);
    }

    let final_temp = wh
        .telemetry()
        .get("tank_avg_temp_c")
        .expect("tank_avg_temp_c must exist");
    let power = wh
        .telemetry()
        .get("electric_kw")
        .expect("electric_power_w must exist");

    // Element must be off (setpoint is 40°C < current tank temp for most of the run)
    // It might fire briefly near 40°C, but long-run the tank settles far above ambient.
    eprintln!(
        "[wh_parity] standby_loss: initial={initial_temp:.3}°C final={final_temp:.3}°C \
         drop={:.3}°C element_power={power:.1} W",
        initial_temp - final_temp
    );

    // Tank must have cooled measurably (any non-trivial UA loss is detectable over 24h)
    assert!(
        final_temp < initial_temp - 1.0,
        "tank must cool by > 1°C in 24h with UA=2.0 W/K; \
         dropped only {:.3}°C (initial={initial_temp:.3}, final={final_temp:.3})",
        initial_temp - final_temp
    );

    // Upper bound: tank cannot cool to ambient in 24h (UA too small)
    assert!(
        final_temp > env.zones[0].temperature_c + 5.0,
        "tank should not reach ambient in 24h with small UA; \
         final={final_temp:.3}°C, ambient={:.1}°C",
        env.zones[0].temperature_c
    );
}

// ---------------------------------------------------------------------------
// 2. Element cycling: deadband thresholds match OCHRE default
//
// OCHRE WaterHeater.py default deadband = 10°F = 5.556°C.
// HARES resistance.rs DEFAULT_DEADBAND_C = 5.555_555_556 (10°F).
//
// DIVERGENCE NOTE: An earlier audit claimed a mismatch (OCHRE 5.56°C vs
// HARES 2.0°C). Code inspection confirms HARES already uses 5.556°C as the
// DEFAULT. Tests that pass explicit deadband_c=2.0 use a non-default value;
// the default behaviour matches OCHRE.
//
// This test verifies:
//   - Element turns on at (setpoint - deadband) = 48.9 - 5.556 = 43.34°C
//   - Element turns off at setpoint = 48.9°C
// ---------------------------------------------------------------------------
#[test]
fn element_cycling_deadband_matches_ochre_default() {
    let env = make_env(20.0);
    let setpoint_c = 48.9_f64;
    let cfg = EquipmentConfig::from_typed(
        "RWH".to_string(),
        "Resistance Water Heater".to_string(),
        ElectricResistanceWaterHeaterConfig {
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            energy_factor: None,
            uniform_energy_factor: None,
            heating_capacity_w: None,
            ua_w_per_k: Some(0.01),
            setpoint_c: Some(setpoint_c),
            deadband_c: None,
            max_tank_temp_c: Some(300.0),
            initial_tank_temp_c: Some(40.0),
            tank_nodes: Some(1),
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: Some(0.0),
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
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
    .unwrap();

    let mut wh = ResistanceWH::new(cfg.clone());
    wh.init(&cfg, &env).unwrap();
    let mut ports = PortSlots::from_declarations(wh.ports());

    // Tank starts at 40°C, below deadband floor (43.34°C) → element should fire immediately
    step_wh(&mut wh, &env, &mut ports);
    let power_step1 = wh
        .telemetry()
        .get("electric_kw")
        .expect("electric_power_w must exist");
    assert!(
        power_step1 > 0.0,
        "element must fire when tank (40°C) is below deadband floor (~43.3°C); got {power_step1:.2} W"
    );

    // Run until element turns off (setpoint reached)
    let mut turned_off = false;
    let mut steps = 0;
    while steps < 200 {
        steps += 1;
        step_wh(&mut wh, &env, &mut ports);
        let power = wh
            .telemetry()
            .get("electric_kw")
            .expect("electric_power_w must exist");
        let temp = wh
            .telemetry()
            .get("tank_avg_temp_c")
            .expect("tank_avg_temp_c must exist");
        if power < 1.0 {
            turned_off = true;
            // Verify temperature at shutoff: must be at or above setpoint
            assert!(
                temp >= setpoint_c - 0.5,
                "element must turn off at setpoint ({setpoint_c}°C); off at {temp:.3}°C"
            );
            eprintln!(
                "[wh_parity] cycling: element off at {temp:.3}°C after {steps} steps \
                 (setpoint={setpoint_c}°C, OCHRE_default_deadband=5.556°C)"
            );
            break;
        }
    }
    assert!(
        turned_off,
        "element must turn off after reaching setpoint within 200 steps"
    );
}

// ---------------------------------------------------------------------------
// 3. Gas WH: fuel consumption, no electricity when no fan
//
// OCHRE WaterHeater.py GasWaterHeater: burner is fueled by gas, no electric
// (unless a blower is configured). With pilot_power_w=0, a cold tank must
// report positive gas and zero electricity.
// ---------------------------------------------------------------------------
#[test]
fn gas_wh_fuel_not_electricity() {
    let env = make_env(21.0);
    let cfg = EquipmentConfig::from_typed(
        "GWH".to_string(),
        "Gas Water Heater".to_string(),
        GasWaterHeaterConfig {
            fan_power_w: None,
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            fuel_type: FuelType::Gas,
            tank_volume_m3: None,
            tank_height_m: None,
            energy_factor: None,
            uniform_energy_factor: None,
            heating_capacity_w: None,
            ua_w_per_k: Some(2.0),
            setpoint_c: Some(52.0),
            deadband_c: Some(5.556),
            max_tank_temp_c: Some(300.0),
            initial_tank_temp_c: Some(40.0),
            tank_nodes: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: Some(0.0),
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            pilot_power_w: Some(0.0),
            flue_loss_fraction: None,
            skin_loss_fraction: None,
            ignition_type: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            conversion_efficiency: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
            pilot_fraction_to_tank: None,
        },
    )
    .unwrap();

    let mut wh = GasWH::new(cfg.clone());
    wh.init(&cfg, &env).unwrap();
    let mut ports = PortSlots::from_declarations(wh.ports());

    step_wh(&mut wh, &env, &mut ports);

    let gas_w = ports.fuel.get(FuelType::Gas);
    let elec_kw = ports.electrical.load_power_w;

    assert!(
        gas_w > 0.0,
        "gas WH must consume gas when cold (40°C < 49.2°C floor); got {gas_w:.2} W"
    );
    assert_eq!(
        elec_kw, 0.0,
        "gas WH with no fan must draw zero electricity; got {elec_kw:.6} kW"
    );

    // Telemetry must be consistent
    let tel_gas = wh
        .telemetry()
        .get("fuel_input_w")
        .expect("fuel_input_w must exist");
    assert!(
        (tel_gas - gas_w).abs() < 1e-6,
        "telemetry fuel_input_w ({tel_gas:.2}) must match port ({gas_w:.2})"
    );
}

// ---------------------------------------------------------------------------
// 4. Tank temperature never goes below mains temperature during heavy draw
//
// Physics: incoming mains water sets the lower bound on tank temperature.
// A tank cannot be cooler than the inlet water it receives.
// ---------------------------------------------------------------------------
#[test]
fn tank_temp_never_below_mains_during_draw() {
    let env = make_env(20.0);
    // Fast draw, low setpoint so element stays off
    let cfg = resistance_cfg(40.0, 2.0, 50.0, 0.10, 0.01);

    let mut wh = ResistanceWH::new(cfg.clone());
    wh.init(&cfg, &env).unwrap();
    let mut ports = PortSlots::from_declarations(wh.ports());

    let mains_temp_c = env.weather.mains_temp_c;

    for step in 0..60 {
        step_wh(&mut wh, &env, &mut ports);
        let temp = wh
            .telemetry()
            .get("tank_avg_temp_c")
            .expect("tank_avg_temp_c must exist");
        assert!(
            temp >= mains_temp_c - 0.1,
            "tank ({temp:.4}°C) must not drop below mains ({mains_temp_c}°C) at step {step}"
        );
    }
}

// ---------------------------------------------------------------------------
// 5. Standby loss magnitude: UA × ΔT gives expected power
//
// OCHRE WaterHeater.py: skin_loss = UA × (T_tank - T_ambient)
// With UA=5.0 W/K and ΔT=30 K → expected ~150 W standby loss.
// We cannot observe the skin loss directly on the thermal port, but we can
// verify the temperature drop rate
// is consistent with the expected UA×ΔT.
//
// Check: over one 60-second step, ΔT ≈ UA×ΔT_ambient / (m×Cp) = 5.0×30 / 791_900 ≈ 1.9 mK.
// Single-step forward-Euler integration is exact to machine precision for a
// linear ODE (no nonlinear draw or switching); 1% tolerance is appropriate.
// ---------------------------------------------------------------------------
#[test]
fn standby_loss_ua_magnitude() {
    use uom::si::f64::Volume;
    use uom::si::volume::gallon;

    let env = make_env(20.0);
    let ua_w_per_k = 5.0_f64;
    let tank_temp_c = 50.0_f64;
    let ambient_c = 20.0_f64;
    let dt_s = 60.0_f64;

    let cfg = EquipmentConfig::from_typed(
        "RWH".to_string(),
        "Resistance Water Heater".to_string(),
        ElectricResistanceWaterHeaterConfig {
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            energy_factor: None,
            uniform_energy_factor: None,
            heating_capacity_w: None,
            ua_w_per_k: Some(ua_w_per_k),
            setpoint_c: Some(40.0),
            deadband_c: Some(2.0),
            max_tank_temp_c: Some(300.0),
            initial_tank_temp_c: Some(tank_temp_c),
            tank_nodes: Some(1),
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: Some(0.0),
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
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
    .unwrap();

    let mut wh = ResistanceWH::new(cfg.clone());
    wh.init(&cfg, &env).unwrap();
    let mut ports = PortSlots::from_declarations(wh.ports());

    step_wh(&mut wh, &env, &mut ports);

    let temp_after = wh
        .telemetry()
        .get("tank_avg_temp_c")
        .expect("tank_avg_temp_c must exist");
    let tank_volume_l = Volume::new::<gallon>(50.0).get::<uom::si::volume::liter>();
    let tank_mass_kg = tank_volume_l;
    let actual_loss_w = (tank_temp_c - temp_after) * (tank_mass_kg * 4183.0) / dt_s;

    // Typed storage-heater configs do not expose the tank-model end-cap override.
    // For a single-node tank the model adds one extra 10% end-cap UA in parallel.
    let effective_ua_w_per_k = ua_w_per_k * 1.1;
    let expected_loss_w = effective_ua_w_per_k * (tank_temp_c - ambient_c);

    eprintln!(
        "[wh_parity] ua_magnitude: expected UA×ΔT={expected_loss_w:.1} W, \
         actual from temp drop={actual_loss_w:.1} W, \
         tank after={temp_after:.4}°C"
    );

    // Forward-Euler is exact for a linear ODE, but the thermal-mass estimate
    // (50 gal × 3.78541 kg/gal × Cp) may differ from the actual tank mass used
    // internally (default volume, node discretization). 10% tolerance covers this.
    let tol = expected_loss_w * 0.10;
    assert!(
        (actual_loss_w - expected_loss_w).abs() < tol,
        "standby loss must match effective UA×ΔT={expected_loss_w:.1} W within 10%; \
         actual={actual_loss_w:.1} W"
    );
}

// ---------------------------------------------------------------------------
// HPWH COP at multiple ambient temperatures.
//
// Initialize a HPWH and verify it computes a reasonable COP across a range
// of ambient temperatures.
// ---------------------------------------------------------------------------
#[test]
fn hpwh_cop_at_multiple_ambient_temps() {
    use hares_equipment::water_heater::heat_pump_wh::HeatPumpWH;

    let setpoint_c = 51.7_f64;
    // Tank must start below the compressor turn-on threshold
    // (setpoint - deadband = 51.7 - 5.556 = 46.14°C) to trigger heating.
    let tank_temp_c = 40.0_f64;

    for ambient_c in [10.0, 20.0, 30.0, 40.0_f64] {
        let env = make_env(ambient_c);
        let cfg = EquipmentConfig::from_typed(
            "HPWH".to_string(),
            "Heat Pump Water Heater".to_string(),
            HeatPumpWaterHeaterConfig {
                equipment_id: None,
                zone_id: None,
                loop_id: None,
                tank_volume_m3: None,
                tank_height_m: None,
                cop: Some(3.45),
                backup_element_power_w: None,
                ua_w_per_k: Some(2.0),
                setpoint_c: Some(setpoint_c),
                deadband_c: Some(5.556),
                max_tank_temp_c: Some(300.0),
                initial_tank_temp_c: Some(tank_temp_c),
                tank_nodes: None,
                tempering_valve_setpoint_c: None,
                avg_water_draw_l_per_day: None,
                draw_flow_rate_kg_s: Some(0.0),
                compressor_power_w: None,
                backup_enable_offset_c: None,
                min_ambient_temp_c: None,
                max_ambient_temp_c: None,
                min_on_time_s: None,
                min_off_time_s: None,
                hp_only_mode: None,
                element_hp_control_mode: None,
                fan_power_w: None,
                parasitic_power_w: None,
                backup_efficiency: None,
                shr: None,
                lost_heat_fraction: None,
                wall_heat_fraction: None,
                capacity_biquadratic_coeffs: None,
                cop_biquadratic_coeffs: None,
                performance_adjustment: None,
                zone_type: None,
                first_hour_rating_m3: None,
                jacket_r_value_m2_k_w: None,
                fixture_delivery_temp_c: None,
                low_power_hpwh: None,
                uniform_energy_factor: None,
            },
        )
        .unwrap();

        let mut wh = HeatPumpWH::new(cfg.clone());
        wh.init(&cfg, &env).unwrap();
        let mut ports = PortSlots::from_declarations(wh.ports());

        // Force tank cold to trigger compressor
        step_wh(&mut wh, &env, &mut ports);

        let cop = wh
            .telemetry()
            .get("cop")
            .expect("cop must exist after step");
        let elec_w = wh
            .telemetry()
            .get("electric_kw")
            .expect("electric_power_w must exist");

        eprintln!("[wh_parity] hpwh_cop: ambient={ambient_c}°C cop={cop:.3} elec={elec_w:.1} W");

        // HPWH COP should be above 1.0 (heat pump thermodynamics)
        assert!(
            cop > 1.0,
            "HPWH COP must exceed 1.0 at ambient {ambient_c}°C; got {cop:.3}"
        );
        // At 20°C ambient, OCHRE reference HPWH gives COP ≈ 3.0–4.0
        // At 40°C ambient, COP ≈ 4.5–5.5 (better source of heat)
        assert!(
            cop < 7.0,
            "HPWH COP {cop:.3} unrealistically high at {ambient_c}°C"
        );
    }
}

// ---------------------------------------------------------------------------
// 7. Storage water-heater deadband matrix uses the physical hysteresis edges
//
// Source-backed thermostat rule:
//   stay off when tank <= setpoint - deadband and the heater was previously off
//   turn on only when tank < setpoint - deadband
//
// This test checks both electric-resistance and gas storage heaters at the
// exact deadband floor and just below it.
// ---------------------------------------------------------------------------
#[test]
fn storage_water_heater_deadband_matrix_matches_boundary_rule() {
    let env = make_env(21.0);
    let cases = [
        (
            "Resistance Water Heater",
            resistance_cfg(52.0, 2.0, 50.0, 0.0, 0.1),
            FuelType::Electric,
        ),
        (
            "Gas Water Heater",
            EquipmentConfig::from_typed(
                "GWH".to_string(),
                "Gas Water Heater".to_string(),
                GasWaterHeaterConfig {
                    fan_power_w: None,
                    equipment_id: None,
                    zone_id: None,
                    loop_id: None,
                    fuel_type: FuelType::Gas,
                    tank_volume_m3: None,
                    tank_height_m: None,
                    energy_factor: None,
                    uniform_energy_factor: None,
                    heating_capacity_w: Some(12_000.0),
                    ua_w_per_k: Some(0.1),
                    setpoint_c: Some(52.0),
                    deadband_c: Some(2.0),
                    max_tank_temp_c: Some(300.0),
                    initial_tank_temp_c: Some(50.0),
                    tank_nodes: None,
                    avg_water_draw_l_per_day: None,
                    draw_flow_rate_kg_s: Some(0.0),
                    draw_flow_rate_source: None,
                    mains_temp_c_source: None,
                    pilot_power_w: Some(0.0),
                    flue_loss_fraction: None,
                    skin_loss_fraction: None,
                    ignition_type: None,
                    performance_adjustment: None,
                    zone_type: None,
                    first_hour_rating_m3: None,
                    jacket_r_value_m2_k_w: None,
                    conversion_efficiency: None,
                    fixture_delivery_temp_c: None,
                    hot_draw_temp_c: None,
                    pilot_fraction_to_tank: None,
                },
            )
            .unwrap(),
            FuelType::Gas,
        ),
    ];

    for (label, at_floor_cfg, fuel_type) in cases {
        let mut ports;
        let on_at_floor = if fuel_type == FuelType::Electric {
            let mut wh = ResistanceWH::new(at_floor_cfg.clone());
            wh.init(&at_floor_cfg, &env).unwrap();
            ports = PortSlots::from_declarations(wh.ports());
            step_wh(&mut wh, &env, &mut ports);
            ports.electrical.load_power_w > 0.0
        } else {
            let mut wh = GasWH::new(at_floor_cfg.clone());
            wh.init(&at_floor_cfg, &env).unwrap();
            ports = PortSlots::from_declarations(wh.ports());
            step_wh(&mut wh, &env, &mut ports);
            ports.fuel.get(FuelType::Gas) > 0.0
        };
        assert!(
            !on_at_floor,
            "{label} must remain off at the exact deadband floor (setpoint-deadband = 50 C)"
        );

        let below_floor_cfg = if fuel_type == FuelType::Electric {
            resistance_cfg(52.0, 2.0, 49.9, 0.0, 0.1)
        } else {
            EquipmentConfig::from_typed(
                "GWH".to_string(),
                "Gas Water Heater".to_string(),
                GasWaterHeaterConfig {
                    fan_power_w: None,
                    equipment_id: None,
                    zone_id: None,
                    loop_id: None,
                    fuel_type: FuelType::Gas,
                    tank_volume_m3: None,
                    tank_height_m: None,
                    energy_factor: None,
                    uniform_energy_factor: None,
                    heating_capacity_w: Some(12_000.0),
                    ua_w_per_k: Some(0.1),
                    setpoint_c: Some(52.0),
                    deadband_c: Some(2.0),
                    max_tank_temp_c: Some(300.0),
                    initial_tank_temp_c: Some(49.9),
                    tank_nodes: None,
                    avg_water_draw_l_per_day: None,
                    draw_flow_rate_kg_s: Some(0.0),
                    draw_flow_rate_source: None,
                    mains_temp_c_source: None,
                    pilot_power_w: Some(0.0),
                    flue_loss_fraction: None,
                    skin_loss_fraction: None,
                    ignition_type: None,
                    performance_adjustment: None,
                    zone_type: None,
                    first_hour_rating_m3: None,
                    jacket_r_value_m2_k_w: None,
                    conversion_efficiency: None,
                    fixture_delivery_temp_c: None,
                    hot_draw_temp_c: None,
                    pilot_fraction_to_tank: None,
                },
            )
            .unwrap()
        };

        let on_below_floor = if fuel_type == FuelType::Electric {
            let mut wh = ResistanceWH::new(below_floor_cfg.clone());
            wh.init(&below_floor_cfg, &env).unwrap();
            ports = PortSlots::from_declarations(wh.ports());
            step_wh(&mut wh, &env, &mut ports);
            ports.electrical.load_power_w > 0.0
        } else {
            let mut wh = GasWH::new(below_floor_cfg.clone());
            wh.init(&below_floor_cfg, &env).unwrap();
            ports = PortSlots::from_declarations(wh.ports());
            step_wh(&mut wh, &env, &mut ports);
            ports.fuel.get(FuelType::Gas) > 0.0
        };
        assert!(
            on_below_floor,
            "{label} must turn on when the tank is just below the deadband floor and previously idle"
        );
    }
}

// ---------------------------------------------------------------------------
// 8. HPWH compressor lockout matrix enforces minimum on-time and off-time
//
// Real compressor protection requires:
//   - once started, the compressor cannot stop before min_on_time_s elapses
//   - once stopped, it cannot restart before min_off_time_s elapses
//
// The test uses hp_only_mode to isolate the compressor from backup-element
// assistance so the state transitions are unambiguous at the trait boundary.
// ---------------------------------------------------------------------------
#[test]
fn hpwh_compressor_lockout_matrix_respects_min_on_and_min_off_times() {
    use hares_equipment::water_heater::heat_pump_wh::HeatPumpWH;

    let cold_cfg = hpwh_cfg_with(40.0, 2.0, 30.0, 180.0, 120.0, true, Some("Simultaneous"));
    let mut wh = HeatPumpWH::new(cold_cfg.clone());
    let mut ports = PortSlots::from_declarations(wh.ports());

    let env0 = make_env_at_minute(21.0, 0);
    wh.init(&cold_cfg, &env0).unwrap();
    let start = hpwh_step_snapshot(&mut wh, &env0, &mut ports);
    assert_eq!(
        start.mode,
        hares_types::OperatingMode::HeatPumpWH,
        "cold tank should start the compressor"
    );
    assert!(
        start.compressor_power_w > 0.0,
        "compressor power must be positive after startup"
    );

    wh.apply_control(&hares_types::ControlSignal::ThermalSetpoint {
        heating_setpoint_c: Some(30.0),
        cooling_setpoint_c: None,
        deadband_c: None,
    })
    .unwrap();

    for minute in 1..=2 {
        let env = make_env_at_minute(21.0, minute);
        let snap = hpwh_step_snapshot(&mut wh, &env, &mut ports);
        assert_eq!(
            snap.mode,
            hares_types::OperatingMode::HeatPumpWH,
            "compressor must remain on before the 180 s min_on_time expires (minute {minute})"
        );
        assert!(
            snap.compressor_power_w > 0.0,
            "compressor power must remain positive before min_on_time expires (minute {minute})"
        );
    }

    let env3 = make_env_at_minute(21.0, 3);
    let off = hpwh_step_snapshot(&mut wh, &env3, &mut ports);
    assert_eq!(
        off.mode,
        hares_types::OperatingMode::Off,
        "compressor should be allowed to stop once min_on_time_s has elapsed"
    );
    assert!(
        off.compressor_power_w.abs() < 1e-9,
        "compressor power must drop to zero once the call clears after min_on_time"
    );

    wh.apply_control(&hares_types::ControlSignal::ThermalSetpoint {
        heating_setpoint_c: Some(52.0),
        cooling_setpoint_c: None,
        deadband_c: None,
    })
    .unwrap();

    let env4 = make_env_at_minute(21.0, 4);
    let locked_out = hpwh_step_snapshot(&mut wh, &env4, &mut ports);
    assert_eq!(
        locked_out.mode,
        hares_types::OperatingMode::Off,
        "compressor must stay off while only 60 s of the 120 s min_off_time has elapsed"
    );
    assert!(
        locked_out.compressor_power_w.abs() < 1e-9,
        "compressor power must remain zero during min_off_time lockout"
    );

    let env5 = make_env_at_minute(21.0, 5);
    let restarted = hpwh_step_snapshot(&mut wh, &env5, &mut ports);
    assert_eq!(
        restarted.mode,
        hares_types::OperatingMode::HeatPumpWH,
        "compressor must restart once min_off_time_s has elapsed"
    );
    assert!(
        restarted.compressor_power_w > 0.0,
        "compressor power must be positive after the lockout expires"
    );
}

// ---------------------------------------------------------------------------
// 9. HPWH control matrix covers compressor/backup coordination modes
//
// For a very cold tank below the backup threshold:
//   - MutuallyExclusive gives compressor priority
//   - Simultaneous allows both compressor and backup
//   - hp_only_mode suppresses backup even if Simultaneous is configured
// ---------------------------------------------------------------------------
#[test]
fn hpwh_control_mode_matrix_matches_configured_coordination_rules() {
    use hares_equipment::water_heater::heat_pump_wh::HeatPumpWH;

    let cases = [
        (
            "mutually-exclusive",
            None,
            false,
            hares_types::OperatingMode::HeatPumpWH,
            false,
        ),
        (
            "simultaneous",
            Some("Simultaneous"),
            false,
            hares_types::OperatingMode::HeatingHPAndER,
            true,
        ),
        (
            "hp-only",
            Some("Simultaneous"),
            true,
            hares_types::OperatingMode::HeatPumpWH,
            false,
        ),
    ];

    for (label, mode_str, hp_only_mode, expected_mode, expect_backup) in cases {
        let cfg = hpwh_cfg_with(40.0, 2.0, 8.0, 0.0, 0.0, hp_only_mode, mode_str);
        let env = make_env(21.0);
        let mut wh = HeatPumpWH::new(cfg.clone());
        wh.init(&cfg, &env).unwrap();
        let mut ports = PortSlots::from_declarations(wh.ports());
        let snap = hpwh_step_snapshot(&mut wh, &env, &mut ports);

        assert_eq!(snap.mode, expected_mode, "{label} mode mismatch");
        assert!(
            snap.compressor_power_w > 0.0,
            "{label} should keep the compressor active for a cold tank"
        );
        assert_eq!(
            snap.backup_power_w > 0.0,
            expect_backup,
            "{label} backup-element expectation mismatch"
        );
    }
}

#[test]
fn hpwh_wall_heat_fraction_splits_sensible_gain_by_category() {
    use hares_equipment::water_heater::heat_pump_wh::HeatPumpWH;

    let env = make_env(24.0);
    let base = HeatPumpWaterHeaterConfig {
        equipment_id: None,
        zone_id: None,
        loop_id: None,
        tank_volume_m3: None,
        tank_height_m: None,
        cop: Some(3.45),
        backup_element_power_w: None,
        ua_w_per_k: Some(2.0),
        setpoint_c: Some(51.7),
        deadband_c: Some(5.556),
        max_tank_temp_c: Some(300.0),
        initial_tank_temp_c: Some(40.0),
        tank_nodes: None,
        tempering_valve_setpoint_c: None,
        avg_water_draw_l_per_day: None,
        draw_flow_rate_kg_s: Some(0.0),
        compressor_power_w: None,
        backup_enable_offset_c: None,
        min_ambient_temp_c: None,
        max_ambient_temp_c: None,
        min_on_time_s: None,
        min_off_time_s: None,
        hp_only_mode: None,
        element_hp_control_mode: None,
        fan_power_w: None,
        parasitic_power_w: None,
        backup_efficiency: None,
        shr: None,
        lost_heat_fraction: None,
        wall_heat_fraction: None,
        capacity_biquadratic_coeffs: None,
        cop_biquadratic_coeffs: None,
        performance_adjustment: None,
        zone_type: None,
        first_hour_rating_m3: None,
        jacket_r_value_m2_k_w: None,
        fixture_delivery_temp_c: None,
        low_power_hpwh: None,
        uniform_energy_factor: None,
    };
    let cfg0 = EquipmentConfig::from_typed(
        "HPWH0".to_string(),
        "Heat Pump Water Heater".to_string(),
        HeatPumpWaterHeaterConfig {
            wall_heat_fraction: Some(0.0),
            ..base.clone()
        },
    )
    .unwrap();
    let cfg50 = EquipmentConfig::from_typed(
        "HPWH50".to_string(),
        "Heat Pump Water Heater".to_string(),
        HeatPumpWaterHeaterConfig {
            wall_heat_fraction: Some(0.5),
            ..base
        },
    )
    .unwrap();

    let mut wh0 = HeatPumpWH::new(cfg0.clone());
    wh0.init(&cfg0, &env).unwrap();
    let mut p0 = PortSlots::from_declarations(wh0.ports());
    step_wh(&mut wh0, &env, &mut p0);

    let mut wh50 = HeatPumpWH::new(cfg50.clone());
    wh50.init(&cfg50, &env).unwrap();
    let mut p50 = PortSlots::from_declarations(wh50.ports());
    step_wh(&mut wh50, &env, &mut p50);

    let sens0 = p0.thermal[0].sensible_gain_w;
    let sens50 = p50.thermal[0].sensible_gain_w;
    let internal50 = p50.thermal[0].sensible_for_category(ThermalCategory::InternalGain);
    let dehumid50 = p50.thermal[0].sensible_for_category(ThermalCategory::HvacDehumidification);
    let jacket50 = p50.thermal[0].sensible_for_category(ThermalCategory::JacketLoss);
    let wall_w = wh50
        .telemetry()
        .get("wall_sensible_gain_w")
        .expect("wall_sensible_gain_w must exist");
    let skin_loss_w = wh50
        .telemetry()
        .get(telemetry_keys::SKIN_LOSS_W)
        .expect("skin_loss_w must exist");

    assert!(
        (sens0 - sens50).abs() < 1e-6,
        "wall fraction must preserve total sensible gain; baseline={sens0:.3}, split={sens50:.3}"
    );
    assert!(
        internal50.abs() < 1e-6,
        "InternalGain must be zero when HP is running; HvacDehumidification covers compressor zone heat; got {internal50:.3} W"
    );
    assert!(
        (dehumid50 - wall_w).abs() < 1.0,
        "HvacDehumidification must equal wall share of HP waste heat (zone-half at wf=0.5); got {dehumid50:.3} vs {wall_w:.3}"
    );
    assert!(
        (jacket50 - wall_w - skin_loss_w).abs() < 1.0,
        "jacket loss must equal wall share plus skin loss; got {jacket50:.3} vs wall={wall_w:.3} + skin={skin_loss_w:.3}"
    );
    assert!(
        (wall_w - (sens50 - skin_loss_w) * 0.5).abs() < 1.0,
        "wall_sensible_gain_w telemetry must be half of HP waste heat; got {wall_w:.3} vs {:.3}",
        (sens50 - skin_loss_w) * 0.5
    );
}

// ---------------------------------------------------------------------------
// Regression: HPWH compressor zone heat must NOT be reported as InternalGain
//
// Fixed by reclassifying heat_pump_wh.rs compressor zone sensible/latent
// from ThermalCategory::InternalGain to HvacDehumidification. HP-cycle
// effects (evaporator extraction, compressor waste heat) are mechanical
// equipment interactions, not passive occupant/appliance gains.
// EnergyPlus Engineering Reference "Heat Pump Water Heater": the HP
// evaporator extracts sensible + latent heat from zone air — same physics
// as a standalone dehumidifier.
// ---------------------------------------------------------------------------
#[test]
fn hpwh_compressor_zone_heat_not_reported_as_internal_gain() {
    use hares_equipment::water_heater::heat_pump_wh::HeatPumpWH;

    // Cold tank below deadband floor to guarantee compressor runs this step.
    let env = make_env(21.0);
    let cfg = EquipmentConfig::from_typed(
        "HPWH".to_string(),
        "Heat Pump Water Heater".to_string(),
        HeatPumpWaterHeaterConfig {
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            cop: Some(3.45),
            backup_element_power_w: None,
            ua_w_per_k: Some(2.0),
            setpoint_c: Some(51.7),
            deadband_c: Some(5.556),
            max_tank_temp_c: Some(300.0),
            initial_tank_temp_c: Some(40.0),
            tank_nodes: None,
            tempering_valve_setpoint_c: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: Some(0.0),
            compressor_power_w: Some(1_200.0),
            backup_enable_offset_c: None,
            min_ambient_temp_c: None,
            max_ambient_temp_c: None,
            min_on_time_s: None,
            min_off_time_s: None,
            hp_only_mode: Some(true),
            element_hp_control_mode: None,
            fan_power_w: Some(0.0),
            parasitic_power_w: Some(0.0),
            backup_efficiency: None,
            shr: None,
            lost_heat_fraction: Some(0.0),
            wall_heat_fraction: Some(0.0),
            capacity_biquadratic_coeffs: None,
            cop_biquadratic_coeffs: None,
            performance_adjustment: None,
            zone_type: Some("conditioned".to_string()),
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            fixture_delivery_temp_c: None,
            low_power_hpwh: None,
            uniform_energy_factor: None,
        },
    )
    .unwrap();

    let mut wh = HeatPumpWH::new(cfg.clone());
    wh.init(&cfg, &env).unwrap();
    let mut ports = PortSlots::from_declarations(wh.ports());
    step_wh(&mut wh, &env, &mut ports);

    let compressor_power_w = wh.telemetry().get("compressor_power_w").unwrap_or(0.0);
    assert!(
        compressor_power_w > 0.0,
        "compressor must be running for this test to be meaningful"
    );

    let internal_gain_w = ports.thermal[0].sensible_for_category(ThermalCategory::InternalGain);
    let dehumid_w = ports.thermal[0].sensible_for_category(ThermalCategory::HvacDehumidification);
    assert!(
        internal_gain_w.abs() < 1e-6,
        "HPWH compressor zone heat must not appear in InternalGain (got {internal_gain_w:.2} W)"
    );
    // Sensible sign can be positive (compressor waste heat dominates) or negative
    // (evaporator extraction dominates). Both are mechanical equipment effects, not
    // passive internal gains — HvacDehumidification is the correct category.
    assert!(
        dehumid_w.abs() > 1.0,
        "HvacDehumidification must carry zone sensible heat when HP is running (got {dehumid_w:.2} W)"
    );
}
