//! Water heater equipment models.

pub mod gas;
pub mod heat_pump_wh;
pub(crate) mod hpwh_compressor;
pub mod indirect_tank;
pub mod resistance;
pub mod tank;
pub mod tankless;
pub mod wh_config;

pub use tank::{DrawResult, StratifiedTank, StratifiedTankConfig, TemperedDrawConfig};

/// Tracks hourly and daily draw volumes for observer capture and invariant checks.
///
/// The observer capture (`observe` feature) logs the hourly draw volume distribution
/// each simulated hour so that the active schedule's temporal pattern can be verified.
///
/// The invariant check (`check_invariants`) validates that overnight draw (0:00–5:00)
/// is less than 20% of the total daily draw — the expected residential pattern.
/// A violation indicates a misconfiguration such as the DOE UEF test schedule being
/// used for a residential simulation.
#[derive(Debug, Clone)]
pub(crate) struct DrawVolumeTracker {
    /// Accumulated draw volume for the current hour [L].
    current_hour_volume_l: f64,
    /// Hour index (0-23) of the most recent observation.
    last_hour: Option<u32>,
    /// Total draw volume accumulated today [L].
    daily_total_l: f64,
    /// Draw volume accumulated between 0:00 and 5:00 today [L].
    overnight_volume_l: f64,
}

impl DrawVolumeTracker {
    pub(crate) const fn new() -> Self {
        Self {
            current_hour_volume_l: 0.0,
            last_hour: None,
            daily_total_l: 0.0,
            overnight_volume_l: 0.0,
        }
    }

    /// Register `draw_volume_l` for the given simulation hour (0-23).
    ///
    /// At hour boundaries:
    /// - Observed feature: logs the completed hour's draw volume.
    /// - Checkpoint: resets `current_hour_volume_l`.
    ///
    /// At midnight (hour 0): resets daily accumulators and runs the invariant check.
    pub(crate) fn accumulate(&mut self, draw_volume_l: f64, hour: u32) {
        if self.last_hour != Some(hour) {
            // Hour boundary: flush the completed hour.
            if let Some(completed_hour) = self.last_hour {
                #[cfg(feature = "observe")]
                {
                    let vol = self.current_hour_volume_l;
                    tracing::info!(
                        hour = completed_hour,
                        draw_volume_l = vol,
                        "hourly hot water draw volume"
                    );
                }

                // At midnight (hour 0 is the new hour, so last_hour was 23):
                // run the overnight-draw invariant check.
                if completed_hour == 23 {
                    #[cfg(any(debug_assertions, feature = "check_invariants"))]
                    {
                        if self.daily_total_l > 0.0 {
                            let overnight_pct =
                                100.0 * self.overnight_volume_l / self.daily_total_l;
                            if overnight_pct > 20.0 {
                                tracing::error!(
                                    overnight_volume_l = self.overnight_volume_l,
                                    daily_total_l = self.daily_total_l,
                                    overnight_pct,
                                    "overnight hot water draw (0:00–5:00) is \
                                     {overnight_pct:.1}% of daily total — exceeds \
                                     20% threshold; the active draw schedule may be a \
                                     DOE UEF test-standard profile rather than a \
                                     residential occupancy profile"
                                );
                            }
                        }
                        self.daily_total_l = 0.0;
                        self.overnight_volume_l = 0.0;
                    }
                    #[cfg(not(any(debug_assertions, feature = "check_invariants")))]
                    {
                        self.daily_total_l = 0.0;
                        self.overnight_volume_l = 0.0;
                    }
                }
            }
            self.current_hour_volume_l = 0.0;
            self.last_hour = Some(hour);
        }

        self.current_hour_volume_l += draw_volume_l;
        self.daily_total_l += draw_volume_l;
        if hour < 5 {
            self.overnight_volume_l += draw_volume_l;
        }
    }
}

// Shared physical constants and defaults used by multiple water heater types.
pub(crate) const DEFAULT_SETPOINT_C: f64 = 51.666_666_7;
pub(crate) const DEFAULT_UA_W_PER_K: f64 = 2.0;
pub(crate) const DEFAULT_TANK_HEIGHT_M: f64 = 1.2;
pub(crate) const DEFAULT_TANK_DIAMETER_M: f64 = 0.5;
pub(crate) const DEFAULT_CONDUCTIVITY_W_M_K: f64 = 0.6;
/// 50 US gallons in m³ (50 × 0.003785411784).
pub(crate) const DEFAULT_TANK_VOLUME_M3: f64 = 0.189_270_589_2;
pub(crate) const DEFAULT_MAX_TANK_TEMP_C: f64 = 60.0;

#[cfg(test)]
use hares_types::BoundaryPolicy;
use hares_types::{EnvironmentState, HaresError, LoopId, PortSlots, ScheduleSource};

use crate::EquipmentRegistry;

/// Well-known LoopId for domestic hot-water demand from wet appliances.
/// Wet appliances write demand flow rate here; water heaters read and add
/// to their schedule-based draw.
pub const DHW_DEMAND_LOOP: LoopId = LoopId(u16::MAX - 1);

/// Read accumulated DHW demand [kg/s] from wet appliance fluid contributions.
pub(crate) fn read_dhw_demand_kg_s(ports: &PortSlots) -> f64 {
    ports
        .fluid
        .iter()
        .find(|acc| acc.loop_id == DHW_DEMAND_LOOP)
        .map(|acc| acc.total_flow_kg_s.max(0.0))
        .unwrap_or(0.0)
}

pub fn register_with_registry(registry: &mut EquipmentRegistry) {
    resistance::register_with_registry(registry);
    gas::register_with_registry(registry);
    heat_pump_wh::register_with_registry(registry);
    tankless::register_with_registry(registry);
    indirect_tank::register_with_registry(registry);

    // "Water Heating" is ambiguous -- the resolver must specify the fuel type.
    registry.register_error(
        "Water Heating",
        "ambiguous water heater class 'Water Heating': \
         the HPXML resolver must emit a fuel-specific class name \
         (e.g. 'Gas Water Heater', 'Electric Resistance Water Heater', \
         'Heat Pump Water Heater')",
    );
}

/// Resolve per-step storage-water-heater inputs `(mains_temp_c, draw_rate_kg_s)`.
///
/// Priority:
/// 1. Canonical weather mains temperature
/// 2. Schedule-column mains temperature when weather is unavailable
/// 3. Config default from equipment init
pub(super) fn resolve_storage_step_inputs(
    env: &EnvironmentState,
    default_mains_temp_c: f64,
    default_draw_rate_kg_s: f64,
    draw_rate_kg_s_source: Option<&mut ScheduleSource>,
    mains_temp_c_source: Option<&mut ScheduleSource>,
) -> (f64, f64) {
    let mains_temp_c = if env.weather.mains_temp_c.is_finite() {
        env.weather.mains_temp_c
    } else {
        mains_temp_c_source
            .and_then(|source| source.value_at(env).ok())
            .filter(|v| v.is_finite())
            .unwrap_or_else(|| resolve_mains_temp_c(env, default_mains_temp_c))
    };

    // Schedule draw is interpreted as SI mass flow [kg/s].
    let draw_rate_kg_s = draw_rate_kg_s_source
        .and_then(|source| source.value_at(env).ok())
        .filter(|v| v.is_finite())
        .map(|draw_kg_s| draw_kg_s.max(0.0))
        .unwrap_or(default_draw_rate_kg_s);

    (mains_temp_c, draw_rate_kg_s)
}

#[cfg(test)]
fn resolve_schedule_col(config: &crate::EquipmentConfig, keys: &[&str]) -> Option<usize> {
    keys.iter()
        .find_map(|&key| parse_usize(config.get_f64(key)))
}

#[cfg(test)]
pub(super) fn draw_schedule_source(config: &crate::EquipmentConfig) -> Option<ScheduleSource> {
    resolve_schedule_col(
        config,
        &[
            "draw_flow_rate_schedule_col",
            "draw_rate_schedule_col",
            "draw_schedule_col",
            "hot_water_draw_schedule_col",
            "hot_water_delivered_schedule_col",
        ],
    )
    .map(|col_idx| ScheduleSource::ColumnRef {
        col_idx,
        boundary: BoundaryPolicy::Clamp,
    })
}

#[cfg(test)]
pub(super) fn mains_temp_schedule_source(
    config: &crate::EquipmentConfig,
) -> Option<ScheduleSource> {
    resolve_schedule_col(
        config,
        &[
            "mains_temp_schedule_col",
            "inlet_temp_schedule_col",
            "water_mains_temp_schedule_col",
        ],
    )
    .map(|col_idx| ScheduleSource::ColumnRef {
        col_idx,
        boundary: BoundaryPolicy::Clamp,
    })
}

/// Resolve mains water temperature for this timestep.
///
/// Priority:
/// 1. Canonical weather field (`env.weather.mains_temp_c`)
/// 2. Config default (`default_mains_temp_c`)
pub(super) fn resolve_mains_temp_c(env: &EnvironmentState, default_mains_temp_c: f64) -> f64 {
    if env.weather.mains_temp_c.is_finite() {
        return env.weather.mains_temp_c;
    }
    default_mains_temp_c
}

pub(super) fn require_mains_temp_c(
    env: &EnvironmentState,
    equipment_name: &str,
) -> crate::Result<f64> {
    let mains_temp_c = env.weather.mains_temp_c;
    if mains_temp_c.is_finite() {
        Ok(mains_temp_c)
    } else {
        Err(HaresError::Equipment(format!(
            "{equipment_name} requires finite env.weather.mains_temp_c at init"
        )))
    }
}

/// Apply insulation jacket R-value correction to tank UA.
/// Shared by resistance, gas, and heat-pump water heater modules.
pub(super) fn apply_jacket_r_value(
    ua_base: f64,
    height_m: f64,
    diameter_m: f64,
    jacket_r_m2_k_w: Option<f64>,
) -> f64 {
    let Some(jacket_r) = jacket_r_m2_k_w else {
        return ua_base;
    };
    if jacket_r <= 0.0 || ua_base <= 0.0 {
        return ua_base;
    }
    let lateral_area_m2 = std::f64::consts::PI * diameter_m * height_m;
    let r_total = 1.0 / ua_base + jacket_r / lateral_area_m2;
    1.0 / r_total
}

/// Thermostat hysteresis logic shared by all storage water heater types.
///
/// We use `<` for both transitions to match OCHRE's strict deadband floor.
/// The thermostat stays off when temperature is exactly at `setpoint_c - deadband_c`.
pub(super) fn hysteresis_call(
    sensor_temp_c: f64,
    setpoint_c: f64,
    deadband_c: f64,
    currently_on: bool,
) -> bool {
    sensor_temp_c
        < if currently_on {
            setpoint_c
        } else {
            setpoint_c - deadband_c
        }
}

/// Volume-weighted average temperature across all tank nodes.
pub(super) fn weighted_average_tank_temp(node_temps: &[f64], node_volumes_m3: &[f64]) -> f64 {
    let total_volume = node_volumes_m3.iter().sum::<f64>();
    if total_volume <= 0.0 {
        return 0.0;
    }
    node_temps
        .iter()
        .zip(node_volumes_m3.iter())
        .map(|(temp, vol)| temp * vol)
        .sum::<f64>()
        / total_volume
}

/// Rate-limit a setpoint change to model real thermostat motor slew.
///
/// Returns `target_c` clamped so that the absolute change from `current_c`
/// does not exceed `max_rate_c_per_s * dt_s`.
pub(super) fn ramp_limited_setpoint(
    current_c: f64,
    target_c: f64,
    max_rate_c_per_s: f64,
    dt_s: f64,
) -> f64 {
    let max_delta = max_rate_c_per_s * dt_s;
    target_c.clamp(current_c - max_delta, current_c + max_delta)
}

/// Parse an `f64` config value as a `usize` node index.
/// Returns `None` for non-finite, negative, or fractional values.
pub(super) fn parse_usize(raw: Option<f64>) -> Option<usize> {
    let value = raw?;
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 {
        return None;
    }
    Some(value as usize)
}

#[cfg(test)]
mod tests {
    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        BoundaryPolicy, DomainUpdate, EnvironmentState, GridState, SCHEDULE_DOMAIN_ID,
        ScheduleSource, WeatherState,
    };

    use super::{
        apply_jacket_r_value, draw_schedule_source, hysteresis_call, mains_temp_schedule_source,
        resolve_storage_step_inputs,
    };
    use crate::EquipmentConfig;
    use crate::config::resolve_reactive_zip;

    /// Document the shared hysteresis floor: at exactly `setpoint - deadband`,
    /// an inactive heater remains off, matching OCHRE's strict boundary.
    #[test]
    fn hysteresis_boundary_exact_deadband_floor_remains_off() {
        let setpoint = 52.0_f64;
        let deadband = 2.0_f64;
        let boundary = setpoint - deadband; // 50.0

        // Exactly at boundary: off → off (strict `<` threshold)
        assert!(
            !hysteresis_call(boundary, setpoint, deadband, false),
            "at exactly setpoint-deadband, inactive heater should remain off"
        );
        // One epsilon above boundary: off → off
        assert!(
            !hysteresis_call(boundary + 1e-9, setpoint, deadband, false),
            "just above setpoint-deadband, inactive heater stays off"
        );
        // Below boundary: off → on
        assert!(hysteresis_call(boundary - 0.5, setpoint, deadband, false));

        // Active heater at boundary: on → on (< setpoint)
        assert!(hysteresis_call(boundary, setpoint, deadband, true));
        // Active heater exactly at setpoint: on → off (not < setpoint)
        assert!(!hysteresis_call(setpoint, setpoint, deadband, true));
    }

    fn base_config() -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "WH".to_string(),
            "Resistance Water Heater".to_string(),
            crate::water_heater::wh_config::ElectricResistanceWaterHeaterConfig {
                equipment_id: None,
                zone_id: None,
                loop_id: None,
                tank_volume_m3: None,
                tank_height_m: None,
                energy_factor: None,
                uniform_energy_factor: None,
                heating_capacity_w: None,
                ua_w_per_k: None,
                setpoint_c: None,
                deadband_c: None,
                max_tank_temp_c: None,
                initial_tank_temp_c: None,
                tank_nodes: None,
                avg_water_draw_l_per_day: None,
                draw_flow_rate_kg_s: None,
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

    fn env_with_payloads(schedule_payload: Option<Vec<f64>>) -> EnvironmentState {
        let mut custom_domains = Vec::new();
        if let Some(payload) = schedule_payload {
            custom_domains.push(DomainUpdate {
                domain_id: SCHEDULE_DOMAIN_ID,
                zone_temperatures_c: Vec::new(),
                custom_payload: Some(payload),
            });
        }

        // Use NaN to exercise the non-finite mains-temperature path in tests.
        let weather = WeatherState {
            mains_temp_c: f64::NAN,
            ..Default::default()
        };

        EnvironmentState {
            zones: Vec::new(),
            weather,
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
                island_bus_voltage_pu: None,
            },
            custom_domains,
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
        }
    }

    #[test]
    fn jacket_r_value_reduces_effective_ua() {
        let ua_base = 2.0_f64;
        let height_m = 1.5_f64;
        let diameter_m = 0.5_f64;
        let jacket_r_m2_k_w = 1.761_101_84_f64; // 10 hr·ft²·°F/BTU

        let ua_adjusted =
            apply_jacket_r_value(ua_base, height_m, diameter_m, Some(jacket_r_m2_k_w));
        let lateral_area_m2 = std::f64::consts::PI * diameter_m * height_m;
        let ua_expected = 1.0 / (1.0 / ua_base + jacket_r_m2_k_w / lateral_area_m2);

        assert!(
            ua_adjusted < ua_base,
            "jacket insulation should reduce tank UA: {ua_adjusted} < {ua_base}"
        );
        assert!(
            (ua_adjusted - ua_expected).abs() < 1e-12,
            "expected UA {ua_expected}, got {ua_adjusted}"
        );
    }

    #[test]
    fn jacket_r_value_guard_conditions_keep_base_ua() {
        let ua_base = 2.0_f64;
        let height_m = 1.4_f64;
        let diameter_m = 0.55_f64;

        // Missing jacket config: no adjustment.
        assert_eq!(
            apply_jacket_r_value(ua_base, height_m, diameter_m, None),
            ua_base
        );

        // Non-positive jacket R-value: no adjustment.
        assert_eq!(
            apply_jacket_r_value(ua_base, height_m, diameter_m, Some(0.0)),
            ua_base
        );
    }

    #[test]
    fn storage_step_inputs_use_schedule_when_configured() {
        let mut env = env_with_payloads(Some(vec![6.0, 10.0, 999.0]));
        env.weather.mains_temp_c = f64::NAN;
        let mut draw_source = ScheduleSource::ColumnRef {
            col_idx: 1,
            boundary: BoundaryPolicy::Clamp,
        };
        let mut mains_source = ScheduleSource::ColumnRef {
            col_idx: 0,
            boundary: BoundaryPolicy::Clamp,
        };
        let (mains_temp_c, draw_rate_kg_s) = resolve_storage_step_inputs(
            &env,
            15.0,
            0.05,
            Some(&mut draw_source),
            Some(&mut mains_source),
        );

        assert!((mains_temp_c - 6.0).abs() < 1e-12);
        assert!((draw_rate_kg_s - 10.0).abs() < 1e-12);
    }

    #[test]
    fn storage_step_inputs_use_weather_when_schedule_missing_or_invalid() {
        let mut env = env_with_payloads(Some(vec![f64::NAN]));
        env.weather.mains_temp_c = 12.5;
        let mut draw_source = ScheduleSource::ColumnRef {
            col_idx: 5,
            boundary: BoundaryPolicy::Clamp,
        };
        let mut mains_source = ScheduleSource::ColumnRef {
            col_idx: 0,
            boundary: BoundaryPolicy::Clamp,
        };
        let (mains_temp_c, draw_rate_kg_s) = resolve_storage_step_inputs(
            &env,
            15.0,
            0.08,
            Some(&mut draw_source),
            Some(&mut mains_source),
        );

        assert!((mains_temp_c - 12.5).abs() < 1e-12);
        assert!((draw_rate_kg_s - 0.08).abs() < 1e-12);
    }

    #[test]
    fn storage_step_inputs_prefer_canonical_weather_mains_temp() {
        let mut env = env_with_payloads(Some(vec![f64::NAN]));
        env.weather.mains_temp_c = 14.25;

        let (mains_temp_c, draw_rate_kg_s) =
            resolve_storage_step_inputs(&env, 15.0, 0.08, None, None);

        assert!((mains_temp_c - 14.25).abs() < 1e-12);
        assert!((draw_rate_kg_s - 0.08).abs() < 1e-12);
    }

    #[test]
    fn schedule_sources_parse_supported_aliases() {
        let mut cfg = base_config();
        cfg.test_extras_mut()
            .insert("draw_flow_rate_schedule_col".to_string(), 7.0.into());
        cfg.test_extras_mut()
            .insert("mains_temp_schedule_col".to_string(), 2.0.into());

        assert_eq!(
            draw_schedule_source(&cfg),
            Some(ScheduleSource::ColumnRef {
                col_idx: 7,
                boundary: BoundaryPolicy::Clamp,
            })
        );
        assert_eq!(
            mains_temp_schedule_source(&cfg),
            Some(ScheduleSource::ColumnRef {
                col_idx: 2,
                boundary: BoundaryPolicy::Clamp,
            })
        );
    }

    #[test]
    fn resolve_reactive_zip_rejects_invalid_reactive_sum() {
        // Migrated from the deleted WaterHeaterZip::from_config validation:
        // a reactive coefficient set that does not sum to 1.0 (with a
        // nonzero pf) must be rejected at init.
        let mut cfg = base_config();
        cfg.zip = Some(hares_types::zip::ZipLoad::reactive_only(0.5, 0.3, 0.3, 0.9)); // reactive sum = 1.1
        let result = resolve_reactive_zip(&cfg);
        assert!(result.is_err(), "reactive zq+iq+pq=1.1 must be rejected");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("reactive ZIP coefficients"),
            "error message should mention the reactive sum, got: {err_msg}"
        );
    }

    #[test]
    fn resolve_reactive_zip_forces_constant_real_power() {
        // Rule R1: whatever the source ZIP says about the real side, water
        // heaters must never scale their real power through the polynomial.
        let mut cfg = base_config();
        cfg.zip = Some(hares_types::zip::ZipLoad {
            zp: 0.825,
            ip: -0.44,
            pp: 0.615,
            zq: 0.0,
            iq: 0.0,
            pq: 1.0,
            pf: 0.97,
            v0: 1.0,
        });
        let zip = resolve_reactive_zip(&cfg).expect("valid reactive side");
        assert_eq!((zip.zp, zip.ip, zip.pp), (0.0, 0.0, 1.0));
        assert_eq!(zip.pf, 0.97);
        for v in [0.9, 1.0, 1.1] {
            let p_kw = 2.4;
            assert_eq!(zip.apply(p_kw, v).0.to_bits(), p_kw.to_bits());
        }
    }

    #[test]
    fn resolve_reactive_zip_falls_back_to_class_defaults() {
        // base_config() carries no sidecar; "Resistance Water Heater" is not
        // a class-table name, so the resolver lands on constant power
        // (pf = 0 sentinel, zero reactive).
        let cfg = base_config();
        let zip = resolve_reactive_zip(&cfg).expect("constant power fallback");
        assert_eq!(zip, hares_types::zip::ZipLoad::constant_power());

        // A production class name resolves the literature row.
        let mut cfg = base_config();
        cfg.ochre_class = "Heat Pump Water Heater".to_string();
        let zip = resolve_reactive_zip(&cfg).expect("HPWH class row");
        assert_eq!(zip.pf, 0.97);
        assert_eq!((zip.zp, zip.ip, zip.pp), (0.0, 0.0, 1.0));
        let class_row =
            hares_types::zip::zip_defaults_for_class("Heat Pump Water Heater").expect("row");
        assert_eq!(
            (zip.zq, zip.iq, zip.pq),
            (class_row.zq, class_row.iq, class_row.pq)
        );
    }
}

#[cfg(test)]
mod dhw_integration_tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        DomainUpdate, EnvironmentState, FluidNodeId, FluidType, GridState, HeatTransferDirection,
        PortContribution, PortSlots, SCHEDULE_DOMAIN_ID, WeatherState, ZoneId, ZoneState,
    };

    use super::DHW_DEMAND_LOOP;
    use crate::event_load::WetAppliance;
    use crate::water_heater::resistance::ResistanceWH;
    use crate::{Equipment, EquipmentConfig};

    fn base_env() -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 21.0,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 10.0,
                outdoor_humidity_ratio: 0.005,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 12.0,
                sky_temp_c: 8.0,
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
            custom_domains: vec![DomainUpdate {
                domain_id: SCHEDULE_DOMAIN_ID,
                zone_temperatures_c: Vec::new(),
                custom_payload: Some(vec![1.0, 1.0]),
            }],
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .expect("UTC offset")
                .with_ymd_and_hms(2026, 3, 18, 0, 0, 0)
                .single()
                .expect("valid UTC timestamp"),
            time_res: ChronoDuration::minutes(1),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    fn washer_config_with_draw(draw_volume_l: f64) -> EquipmentConfig {
        let mut raw: HashMap<String, crate::config::ConfigValue> = HashMap::new();
        raw.insert("zone_id".to_string(), 1.0.into());
        raw.insert("event_window_schedule_col".to_string(), 0.0.into());
        raw.insert("event_probability_schedule_col".to_string(), 1.0.into());
        raw.insert("phase_len".to_string(), 1.0.into());
        raw.insert("phase_0_power_kw".to_string(), 0.5.into());
        raw.insert("phase_0_duration_s".to_string(), 1800.0.into()); // 30 min
        raw.insert("n_units".to_string(), 1.0.into());
        raw.insert("sensible_gain_fraction".to_string(), 0.0.into());
        raw.insert("building_id".to_string(), 11.0.into());
        raw.insert("master_seed".to_string(), 987.0.into());
        raw.insert("hot_water_draw_volume_l".to_string(), draw_volume_l.into());
        EquipmentConfig::raw("washer".to_string(), "Clothes Washer".to_string(), raw)
    }

    fn wh_config() -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "WH".to_string(),
            "Resistance Water Heater".to_string(),
            crate::water_heater::wh_config::ElectricResistanceWaterHeaterConfig {
                equipment_id: None,
                zone_id: None,
                loop_id: None,
                tank_volume_m3: None,
                tank_height_m: None,
                energy_factor: None,
                uniform_energy_factor: None,
                heating_capacity_w: None,
                ua_w_per_k: None,
                setpoint_c: Some(52.0),
                deadband_c: Some(2.0),
                max_tank_temp_c: Some(300.0),
                initial_tank_temp_c: Some(52.0),
                tank_nodes: None,
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
        .unwrap()
    }

    /// Merge port declarations from multiple equipment into shared PortSlots,
    /// mimicking what the dwelling orchestrator does.
    fn shared_ports(equipment: &[&dyn Equipment]) -> PortSlots {
        let mut all_decls = Vec::new();
        for eq in equipment {
            all_decls.extend_from_slice(eq.ports());
        }
        PortSlots::from_declarations(&all_decls)
    }

    // ===================================================================
    // WetAppliance DHW demand emission tests
    // ===================================================================

    #[test]
    fn wet_appliance_with_draw_emits_fluid_demand_when_active() {
        let env = base_env();
        let config = washer_config_with_draw(15.0);
        let mut washer = WetAppliance::new(config.clone(), "Clothes Washer");
        washer.init(&config, &env).unwrap();

        // Verify port declarations include a Fluid port on DHW_DEMAND_LOOP.
        assert!(
            washer
                .ports()
                .iter()
                .any(|p| p.loop_id == Some(DHW_DEMAND_LOOP)),
            "washer with draw must declare a DHW_DEMAND_LOOP fluid port"
        );

        let mut slots = PortSlots::from_declarations(washer.ports());
        washer
            .step(&env, Duration::from_secs(60), &mut slots)
            .unwrap();

        let dhw = slots
            .fluid
            .iter()
            .find(|a| a.loop_id == DHW_DEMAND_LOOP)
            .expect("DHW_DEMAND_LOOP accumulator must exist");
        assert!(
            dhw.total_flow_kg_s > 0.0,
            "active washer with draw must emit positive fluid demand, got {}",
            dhw.total_flow_kg_s,
        );
    }

    #[test]
    fn wet_appliance_with_zero_draw_emits_no_fluid() {
        let env = base_env();
        let config = washer_config_with_draw(0.0);
        let mut dryer = WetAppliance::new(config.clone(), "Clothes Dryer");
        dryer.init(&config, &env).unwrap();

        // No DHW_DEMAND_LOOP port should be declared.
        assert!(
            !dryer
                .ports()
                .iter()
                .any(|p| p.loop_id == Some(DHW_DEMAND_LOOP)),
            "dryer with zero draw must NOT declare a DHW_DEMAND_LOOP fluid port"
        );
    }

    #[test]
    fn draw_rate_equals_volume_over_total_cycle_duration() {
        let env = base_env();
        let draw_volume_l = 15.0;
        let phase_duration_s = 1800.0; // single phase, 30 min
        let config = washer_config_with_draw(draw_volume_l);
        let mut washer = WetAppliance::new(config.clone(), "Clothes Washer");
        washer.init(&config, &env).unwrap();

        let mut slots = PortSlots::from_declarations(washer.ports());
        washer
            .step(&env, Duration::from_secs(60), &mut slots)
            .unwrap();

        let dhw = slots
            .fluid
            .iter()
            .find(|a| a.loop_id == DHW_DEMAND_LOOP)
            .unwrap();
        let expected_kg_s = draw_volume_l / phase_duration_s;
        assert!(
            (dhw.total_flow_kg_s - expected_kg_s).abs() < 1e-9,
            "draw rate must be volume/duration = {expected_kg_s}, got {}",
            dhw.total_flow_kg_s,
        );
    }

    // ===================================================================
    // Cross-equipment wiring: wet appliance demand → water heater
    // ===================================================================

    /// Run a resistance water heater for one step with the given pre-populated
    /// DHW demand on the shared PortSlots. Returns the average tank temperature
    /// after the step.
    fn wh_step_with_demand(demand_kg_s: f64) -> f64 {
        let env = base_env();
        let cfg = wh_config();
        let mut wh = ResistanceWH::new(cfg.clone());
        wh.init(&cfg, &env).unwrap();

        // Build PortSlots that include the DHW_DEMAND_LOOP accumulator
        // (as the dwelling would when both appliance and WH are present).
        let mut slots = PortSlots::from_declarations(wh.ports());

        // Pre-populate the demand accumulator as if a wet appliance had stepped.
        if demand_kg_s > 0.0 {
            slots
                .accumulate(&PortContribution::Fluid {
                    loop_id: DHW_DEMAND_LOOP,
                    flow_rate_kg_s: demand_kg_s,
                    supply_temp_c: 0.0,
                    return_temp_c: 0.0,
                    fluid_type: FluidType::Water,
                    thermal_power_w: None,
                    node_id: FluidNodeId(0),
                    direction: HeatTransferDirection::Source,
                })
                .unwrap();
        }

        wh.step(&env, Duration::from_secs(60), &mut slots).unwrap();

        wh.telemetry().get("tank_avg_temp_c").unwrap()
    }

    #[test]
    fn water_heater_with_appliance_demand_draws_more_than_without() {
        let temp_no_demand = wh_step_with_demand(0.0);
        // 0.1 kg/s is a substantial draw (6 L/min).
        let temp_with_demand = wh_step_with_demand(0.1);

        assert!(
            temp_with_demand < temp_no_demand,
            "tank temp with appliance demand ({temp_with_demand:.4} C) must be \
             lower than without ({temp_no_demand:.4} C)"
        );
    }

    #[test]
    fn water_heater_draw_increases_with_larger_appliance_demand() {
        let temp_small = wh_step_with_demand(0.01);
        let temp_large = wh_step_with_demand(0.1);

        assert!(
            temp_large < temp_small,
            "larger appliance demand ({temp_large:.4} C) must cool the tank \
             more than smaller demand ({temp_small:.4} C)"
        );
    }

    /// Full end-to-end: step a WetAppliance, then step a ResistanceWH on the
    /// same shared PortSlots. Verify the water heater's reported draw_flow_rate
    /// includes the appliance demand.
    #[test]
    fn end_to_end_wet_appliance_demand_reaches_water_heater() {
        let env = base_env();

        // Set up washer with 15 L draw over a 1800 s cycle → 0.00833 kg/s.
        let washer_cfg = washer_config_with_draw(15.0);
        let mut washer = WetAppliance::new(washer_cfg.clone(), "Clothes Washer");
        washer.init(&washer_cfg, &env).unwrap();

        // Set up water heater with zero schedule draw.
        let wh_cfg = wh_config();
        let mut wh = ResistanceWH::new(wh_cfg.clone());
        wh.init(&wh_cfg, &env).unwrap();

        // Build shared PortSlots from both equipment (like the dwelling does).
        let mut slots = shared_ports(&[&washer as &dyn Equipment, &wh as &dyn Equipment]);

        // Step 1: washer runs (Independent stage, rank 0).
        washer
            .step(&env, Duration::from_secs(60), &mut slots)
            .unwrap();

        // Verify the washer emitted demand.
        let demand_after_washer = slots
            .fluid
            .iter()
            .find(|a| a.loop_id == DHW_DEMAND_LOOP)
            .map(|a| a.total_flow_kg_s)
            .unwrap_or(0.0);
        assert!(
            demand_after_washer > 0.0,
            "washer must have emitted DHW demand, got {demand_after_washer}"
        );

        // Step 2: water heater runs (Thermal stage, rank 2) on same slots.
        wh.step(&env, Duration::from_secs(60), &mut slots).unwrap();

        // The water heater's telemetry draw_flow_rate_kg_s must include the
        // appliance demand (WH has zero schedule draw, so all draw comes from
        // the washer).
        let wh_draw = wh.telemetry().get("draw_flow_rate_kg_s").unwrap_or(0.0);
        let expected_draw = 15.0 / 1800.0; // volume_l / cycle_duration_s
        assert!(
            (wh_draw - expected_draw).abs() < 1e-6,
            "water heater draw must equal washer demand ({expected_draw:.6}), got {wh_draw:.6}"
        );

        // Tank must have cooled from 52°C due to the cold-water draw.
        let tank_temp = wh.telemetry().get("tank_avg_temp_c").unwrap();
        assert!(
            tank_temp < 52.0,
            "tank must cool below initial 52°C with draw, got {tank_temp:.4} C"
        );
    }

    /// Water heater schedule draw + appliance demand are additive.
    #[test]
    fn schedule_draw_and_appliance_demand_are_additive() {
        let env = base_env();

        // WH with schedule draw of 0.05 kg/s.
        let base_cfg = wh_config();
        let mut typed: crate::water_heater::wh_config::ElectricResistanceWaterHeaterConfig =
            base_cfg.typed().expect("typed resistance config");
        typed.draw_flow_rate_kg_s = Some(0.05);
        let wh_cfg = EquipmentConfig::from_typed(
            "WH".to_string(),
            "Resistance Water Heater".to_string(),
            typed,
        )
        .unwrap();

        let mut wh = ResistanceWH::new(wh_cfg.clone());
        wh.init(&wh_cfg, &env).unwrap();

        let mut slots = PortSlots::from_declarations(wh.ports());

        // Pre-populate 0.01 kg/s of appliance demand.
        slots
            .accumulate(&PortContribution::Fluid {
                loop_id: DHW_DEMAND_LOOP,
                flow_rate_kg_s: 0.01,
                supply_temp_c: 0.0,
                return_temp_c: 0.0,
                fluid_type: FluidType::Water,
                thermal_power_w: None,
                node_id: FluidNodeId(0),
                direction: HeatTransferDirection::Source,
            })
            .unwrap();

        wh.step(&env, Duration::from_secs(60), &mut slots).unwrap();

        let wh_draw = wh.telemetry().get("draw_flow_rate_kg_s").unwrap_or(0.0);
        let expected = 0.05 + 0.01;
        assert!(
            (wh_draw - expected).abs() < 1e-6,
            "total draw must be schedule ({}) + appliance ({}) = {expected}, got {wh_draw}",
            0.05,
            0.01,
        );
    }
}
