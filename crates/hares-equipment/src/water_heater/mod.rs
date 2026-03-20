//! Water heater equipment models.

pub mod gas;
pub mod heat_pump_wh;
pub mod resistance;
pub mod tank;
pub mod tankless;

pub use tank::{DrawResult, StratifiedTank, StratifiedTankConfig, TemperedDrawConfig};

use hares_types::{BoundaryPolicy, DomainId, EnvironmentState, ScheduleSource};

use crate::EquipmentRegistry;

pub fn register_with_registry(registry: &mut EquipmentRegistry) {
    resistance::register_with_registry(registry);
    gas::register_with_registry(registry);
    heat_pump_wh::register_with_registry(registry);
    tankless::register_with_registry(registry);
}

/// Domain ID used by `hares-core::EnvironmentManager` to publish per-step mains
/// water temperature [°C] in `EnvironmentState.custom_domains`.
const MAINS_WATER_DOMAIN_ID: DomainId = DomainId(u16::MAX - 1);

/// Resolve per-step storage-water-heater inputs `(mains_temp_c, draw_rate_kg_s)`.
///
/// Priority:
/// 1. Schedule-column values (when configured and finite)
/// 2. Runtime mains domain value for mains temperature
/// 3. Config fallback values from equipment init
pub(super) fn resolve_storage_step_inputs(
    env: &EnvironmentState,
    fallback_mains_temp_c: f64,
    fallback_draw_rate_kg_s: f64,
    draw_l_per_min_source: Option<&mut ScheduleSource>,
    mains_temp_c_source: Option<&mut ScheduleSource>,
) -> (f64, f64) {
    let mains_temp_c = mains_temp_c_source
        .and_then(|source| source.value_at(env).ok())
        .filter(|v| v.is_finite())
        .unwrap_or_else(|| resolve_mains_temp_c(env, fallback_mains_temp_c));

    // Schedule draw is interpreted as L/min; convert to kg/s (water ≈ 1 kg/L).
    let draw_rate_kg_s = draw_l_per_min_source
        .and_then(|source| source.value_at(env).ok())
        .filter(|v| v.is_finite())
        .map(|draw_l_per_min| (draw_l_per_min / 60.0).max(0.0))
        .unwrap_or(fallback_draw_rate_kg_s);

    (mains_temp_c, draw_rate_kg_s)
}

fn resolve_schedule_col(config: &crate::EquipmentConfig, keys: &[&str]) -> Option<usize> {
    keys.iter()
        .find_map(|&key| parse_usize(config.get_f64(key)))
}

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
/// The runtime environment value takes priority; `fallback_mains_temp_c` preserves
/// compatibility when running against older environment producers.
pub(super) fn resolve_mains_temp_c(env: &EnvironmentState, fallback_mains_temp_c: f64) -> f64 {
    env.custom_domains
        .iter()
        .find(|d| d.domain_id == MAINS_WATER_DOMAIN_ID)
        .and_then(|d| d.custom_payload.as_ref())
        .and_then(|payload| payload.first().copied())
        .filter(|value| value.is_finite())
        .unwrap_or(fallback_mains_temp_c)
}

/// Apply insulation jacket R-value correction to tank UA.
/// Shared by resistance, gas, and heat-pump water heater modules.
pub(super) fn apply_jacket_r_value(
    ua_base: f64,
    height_m: f64,
    diameter_m: f64,
    config: &crate::EquipmentConfig,
) -> f64 {
    use crate::hvac::helpers::first_f64;
    let Some(jacket_r) = first_f64(config, &["jacket_r_value_m2_k_w"]) else {
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
/// We use `<=` for the off→on transition (turn on when at or below deadband floor)
/// and `<` for the on→off transition (stay on until setpoint is strictly reached).
/// OCHRE uses `<` for both; our choice keeps the element off when temperature is
/// exactly at `setpoint_c - deadband_c`, which matches physical thermostat behavior
/// and avoids chatter at the boundary.
pub(super) fn hysteresis_call(
    sensor_temp_c: f64,
    setpoint_c: f64,
    deadband_c: f64,
    currently_on: bool,
) -> bool {
    if currently_on {
        sensor_temp_c < setpoint_c
    } else {
        // off→on: engage at or below the deadband floor
        sensor_temp_c <= setpoint_c - deadband_c
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

/// Parse an `f64` config value as a `usize` node index.
/// Returns `None` for non-finite, negative, or fractional values.
pub(super) fn parse_usize(raw: Option<f64>) -> Option<usize> {
    let value = raw?;
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 {
        return None;
    }
    Some(value as usize)
}

/// ZIP load model coefficients for voltage-dependent power scaling.
///
/// Implements the OCHRE ZIP model (Equipment.py `run_zip`, lines 200-218):
/// - `P_actual = P_rated * (z * V² + i * V + p)` where V = v / v0 (per-unit)
/// - `Q_actual = P_actual * pf * (zq * V² + iq * V + pq)`
///
/// Default is constant-power load (z=0, i=0, p=1, pf=0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct WaterHeaterZip {
    /// Real-power impedance fraction (Z term).
    pub z: f64,
    /// Real-power current fraction (I term).
    pub i: f64,
    /// Real-power constant-power fraction (P term).
    pub p: f64,
    /// Reference voltage for ZIP normalization [per-unit]. Default 1.0.
    pub v0: f64,
    /// Reactive-power impedance fraction.
    pub zq: f64,
    /// Reactive-power current fraction.
    pub iq: f64,
    /// Reactive-power constant fraction.
    pub pq: f64,
    /// Power factor magnitude for reactive power calculation.
    pub pf: f64,
}

impl Default for WaterHeaterZip {
    fn default() -> Self {
        Self {
            z: 0.0,
            i: 0.0,
            p: 1.0,
            v0: 1.0,
            zq: 0.0,
            iq: 0.0,
            pq: 1.0,
            pf: 0.0,
        }
    }
}

impl WaterHeaterZip {
    /// Parse ZIP coefficients from equipment config.
    ///
    /// Keys: `zip_z`, `zip_i`, `zip_p`, `zip_zq`, `zip_iq`, `zip_pq`, `zip_pf`.
    /// Falls back to constant-power defaults (0, 0, 1) if keys are absent.
    pub(super) fn from_config(config: &crate::EquipmentConfig) -> Self {
        let z = config.get_f64("zip_z").unwrap_or(0.0);
        let i = config.get_f64("zip_i").unwrap_or(0.0);
        let p = config.get_f64("zip_p").unwrap_or(1.0);
        let zq = config.get_f64("zip_zq").unwrap_or(0.0);
        let iq = config.get_f64("zip_iq").unwrap_or(0.0);
        let pq = config.get_f64("zip_pq").unwrap_or(1.0);

        debug_assert!(
            (z + i + p - 1.0).abs() < 0.01,
            "ZIP z+i+p must sum to 1.0, got z={z} i={i} p={p} sum={}",
            z + i + p
        );
        let has_reactive = zq != 0.0 || iq != 0.0 || pq != 0.0;
        if has_reactive {
            debug_assert!(
                (zq + iq + pq - 1.0).abs() < 0.01,
                "ZIP zq+iq+pq must sum to 1.0, got zq={zq} iq={iq} pq={pq} sum={}",
                zq + iq + pq
            );
        }

        Self {
            z,
            i,
            p,
            v0: config.get_f64("zip_v0").unwrap_or(1.0),
            zq,
            iq,
            pq,
            pf: config.get_f64("zip_pf").unwrap_or(0.0),
        }
    }

    /// Apply ZIP voltage scaling to a rated real power [W].
    ///
    /// Returns `(active_power_w, reactive_power_kvar)`.
    /// Returns `(0.0, 0.0)` when `rated_w` is zero or voltage is zero (grid outage).
    pub(super) fn apply(&self, rated_w: f64, voltage_pu: f64) -> (f64, f64) {
        if rated_w == 0.0 || voltage_pu == 0.0 {
            return (0.0, 0.0);
        }
        let v_norm = voltage_pu / self.v0;
        let real_mult = self.z * v_norm * v_norm + self.i * v_norm + self.p;
        let actual_w = rated_w * real_mult;
        let reactive_base = self.zq * v_norm * v_norm + self.iq * v_norm + self.pq;
        // Q = P × tan(acos(pf)) × reactive_base
        // tan(acos(pf)) converts from power factor to reactive/active power ratio.
        let tan_phi = self.pf.clamp(-1.0, 1.0).acos().tan();
        let reactive_kvar = actual_w / 1_000.0 * tan_phi * reactive_base;
        (actual_w, reactive_kvar)
    }
}

/// Compute the draw flow rate [kg/s] for a water heater from config.
///
/// If `draw_profile_fraction` AND `avg_water_draw_l_per_day` are both present,
/// the fraction is normalized using the OCHRE draw normalization:
///   `draw_kg_s = fraction * (avg_l_per_day / 1440) / mean_fraction / 60`
/// where the division by 60 converts L/min to L/s (≈ kg/s for water).
///
/// If only `draw_flow_rate_kg_s` or `draw_rate_kg_s` is present, it is used directly.
/// Returns 0.0 if neither key is present.
pub(super) fn resolve_draw_rate_kg_s(config: &crate::EquipmentConfig) -> f64 {
    // Try direct physical value first.
    if let Some(direct) = config
        .get_f64("draw_flow_rate_kg_s")
        .or_else(|| config.get_f64("draw_rate_kg_s"))
    {
        return direct.max(0.0);
    }

    // Try normalized profile fraction.
    let fraction = config.get_f64("draw_profile_fraction");
    let avg_daily_l = config.get_f64("avg_water_draw_l_per_day");

    match (fraction, avg_daily_l) {
        (Some(f), Some(avg_l)) if f > 0.0 && avg_l > 0.0 => {
            // OCHRE convert_water_column: scale = (avg_l / 1440) / profile_mean
            // Here we have a single fraction value (timestep value), not a full profile.
            // For a single-fraction config, we treat the fraction as already the mean,
            // giving: draw = fraction * (avg_l / 1440) / fraction / 60 = avg_l / (1440 * 60)
            // More generally: draw_kg_s = f * (avg_l / 1440) / profile_mean / 60
            // When only one value is given, profile_mean == f, so it simplifies to avg_l / 86400.
            // This matches OCHRE's "average draw rate" used when no schedule is present.
            avg_l / 86_400.0
        }
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::{Duration as ChronoDuration, TimeZone, Utc};
    use hares_types::{
        BoundaryPolicy, DomainUpdate, EnvironmentState, GridState, ScheduleSource, WeatherState,
        schedule_domain_id,
    };

    use super::{
        apply_jacket_r_value, draw_schedule_source, hysteresis_call, mains_temp_schedule_source,
        resolve_storage_step_inputs,
    };
    use crate::EquipmentConfig;

    /// Document our `<=` vs `<` boundary choice: at exactly `setpoint - deadband`,
    /// an inactive heater should turn on (we use `<=`), whereas OCHRE uses `<`.
    #[test]
    fn hysteresis_boundary_exact_deadband_floor_turns_on() {
        let setpoint = 52.0_f64;
        let deadband = 2.0_f64;
        let boundary = setpoint - deadband; // 50.0

        // Exactly at boundary: off → on (our choice: <=)
        assert!(
            hysteresis_call(boundary, setpoint, deadband, false),
            "at exactly setpoint-deadband, inactive heater should turn on"
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
        EquipmentConfig {
            name: "WH".to_string(),
            ochre_class: "Resistance Water Heater".to_string(),
            raw_config: HashMap::new(),
        }
    }

    fn env_with_payloads(
        schedule_payload: Option<Vec<f64>>,
        mains_payload: Option<Vec<f64>>,
    ) -> EnvironmentState {
        let mut custom_domains = Vec::new();
        if let Some(payload) = schedule_payload {
            custom_domains.push(DomainUpdate {
                domain_id: schedule_domain_id(),
                zone_temperatures_c: Vec::new(),
                custom_payload: Some(payload),
            });
        }
        if let Some(payload) = mains_payload {
            custom_domains.push(DomainUpdate {
                domain_id: super::MAINS_WATER_DOMAIN_ID,
                zone_temperatures_c: Vec::new(),
                custom_payload: Some(payload),
            });
        }

        EnvironmentState {
            zones: Vec::new(),
            weather: WeatherState::default(),
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains,
            current_time: Utc
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .single()
                .expect("valid timestamp"),
            time_res: ChronoDuration::seconds(60),
        }
    }

    #[test]
    fn jacket_r_value_reduces_effective_ua() {
        let ua_base = 2.0_f64;
        let height_m = 1.5_f64;
        let diameter_m = 0.5_f64;
        let jacket_r_m2_k_w = 1.761_101_84_f64; // 10 hr·ft²·°F/BTU

        let mut cfg = base_config();
        cfg.raw_config
            .insert("jacket_r_value_m2_k_w".to_string(), jacket_r_m2_k_w.into());

        let ua_adjusted = apply_jacket_r_value(ua_base, height_m, diameter_m, &cfg);
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
        let cfg = base_config();

        // Missing jacket config: no adjustment.
        assert_eq!(
            apply_jacket_r_value(ua_base, height_m, diameter_m, &cfg),
            ua_base
        );

        // Non-positive jacket R-value: no adjustment.
        let mut cfg_zero = base_config();
        cfg_zero
            .raw_config
            .insert("jacket_r_value_m2_k_w".to_string(), 0.0.into());
        assert_eq!(
            apply_jacket_r_value(ua_base, height_m, diameter_m, &cfg_zero),
            ua_base
        );
    }

    #[test]
    fn storage_step_inputs_use_schedule_when_configured() {
        let env = env_with_payloads(Some(vec![6.0, 10.0, 999.0]), Some(vec![13.0]));
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
        assert!((draw_rate_kg_s - (10.0 / 60.0)).abs() < 1e-12);
    }

    #[test]
    fn storage_step_inputs_fall_back_when_schedule_missing_or_invalid() {
        let env = env_with_payloads(Some(vec![f64::NAN]), Some(vec![12.5]));
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
    fn schedule_sources_parse_supported_aliases() {
        let mut cfg = base_config();
        cfg.raw_config
            .insert("draw_flow_rate_schedule_col".to_string(), 7.0.into());
        cfg.raw_config
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
}
