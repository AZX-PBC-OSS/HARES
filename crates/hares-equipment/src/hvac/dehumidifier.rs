//! Standalone dehumidifier model.

use std::borrow::Cow;
use std::time::Duration;

use hares_physics::biquadratic::BiquadraticCurve;
use hares_types::{
    ControlCapabilities, ControlSignal, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FuelType, HaresError, OperatingMode, PortContribution, PortDeclaration,
    PortSlots, Telemetry, TelemetryField, ThermalCategory, ZoneId,
};
use serde::{Deserialize, Serialize};

use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

use super::helpers::{equipment_id_from_config, first_f64, zone_id_from_config};

const KG_PER_LITER_WATER: f64 = 1.0;
const HOURS_PER_DAY: f64 = 24.0;
const MINUTES_PER_HOUR: f64 = 60.0;
const SECONDS_PER_MINUTE: f64 = 60.0;
const SECONDS_PER_HOUR: f64 = MINUTES_PER_HOUR * SECONDS_PER_MINUTE;
const SECONDS_PER_DAY: f64 = HOURS_PER_DAY * SECONDS_PER_HOUR;
const DEFAULT_RATED_WATER_REMOVAL_L_DAY: f64 = 30.0;
const DEFAULT_TARGET_RH_FRACTION: f64 = 0.50;
const DEFAULT_DEADBAND_HALF_WIDTH_RH_FRACTION: f64 = 0.025;
const RH_MIN_FRACTION: f64 = 0.0;
const RH_MAX_FRACTION: f64 = 1.0;
const DEFAULT_DB_BOUNDS_C: (f64, f64) = (10.0, 40.0);
const DEFAULT_RH_BOUNDS: (f64, f64) = (RH_MIN_FRACTION, RH_MAX_FRACTION);
const RATED_DRY_BULB_C: f64 = 26.666_666_666_7;
const RATED_RH_FRACTION: f64 = 0.60;
const LATENT_HEAT_VAPORIZATION_J_KG: f64 = 2_454_000.0;
const WATTS_PER_KILOWATT: f64 = 1_000.0;
const WATTS_PER_KILOWATT_HOUR: f64 = 3_600_000.0;
const DEFAULT_NORMALIZED_CURVE: [f64; 6] = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];

use crate::config::KEY_EQUIPMENT_ID;
const KEY_RATED_CAPACITY_PINTS_DAY: &str = "Capacity";
const KEY_RATED_CAPACITY_PINTS_DAY_ALT: &str = "capacity_pints_day";
const KEY_RATED_CAPACITY_L_DAY: &str = "capacity_l_day";
const KEY_INTEGRATED_ENERGY_FACTOR: &str = "IntegratedEnergyFactor";
const KEY_INTEGRATED_ENERGY_FACTOR_ALT: &str = "integrated_energy_factor";
const KEY_ENERGY_FACTOR: &str = "EnergyFactor";
const KEY_ENERGY_FACTOR_ALT: &str = "energy_factor";
const KEY_DEHUMIDISTAT_SETPOINT: &str = "DehumidistatSetpoint";
const KEY_DEHUMIDISTAT_SETPOINT_ALT: &str = "dehumidistat_setpoint";
const KEY_TARGET_RH: &str = "target_rh";
const KEY_MIN_RH: &str = "min_rh";
const KEY_MAX_RH: &str = "max_rh";
const KEY_FRACTION_LOAD_SERVED: &str = "FractionDehumidificationLoadServed";
const KEY_FRACTION_LOAD_SERVED_ALT: &str = "fraction_dehumidification_load_served";
const KEY_WR_CURVE: &str = "water_removal_biquadratic_coeffs";
const KEY_EF_CURVE: &str = "energy_factor_biquadratic_coeffs";
const KEY_WR_PREFIX: &str = "water_removal_curve";
const KEY_EF_PREFIX: &str = "energy_factor_curve";
const KEY_WR_T_MIN_C: &str = "water_removal_t_min_c";
const KEY_WR_T_MAX_C: &str = "water_removal_t_max_c";
const KEY_WR_RH_MIN: &str = "water_removal_rh_min";
const KEY_WR_RH_MAX: &str = "water_removal_rh_max";
const KEY_EF_T_MIN_C: &str = "energy_factor_t_min_c";
const KEY_EF_T_MAX_C: &str = "energy_factor_t_max_c";
const KEY_EF_RH_MIN: &str = "energy_factor_rh_min";
const KEY_EF_RH_MAX: &str = "energy_factor_rh_max";

const PINTS_TO_LITERS: f64 = 0.473_176_473;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct DehumidifierState {
    is_on: bool,
    accumulated_water_removal_l: f64,
    target_rh: f64,
    min_rh: f64,
    max_rh: f64,
    mode_override: Option<OperatingMode>,
}

#[derive(Clone, Copy, Debug)]
struct PerformanceSnapshot {
    water_removal_l_day: f64,
    electric_power_w: f64,
    latent_removal_w: f64,
    sensible_gain_w: f64,
}

pub struct Dehumidifier {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    zone_id: ZoneId,
    operating_mode: OperatingMode,
    is_on: bool,
    mode_override: Option<OperatingMode>,
    rated_water_removal_l_day: f64,
    rated_energy_factor_l_kwh: f64,
    fraction_load_served: f64,
    target_rh: f64,
    min_rh: f64,
    max_rh: f64,
    water_removal_curve: BiquadraticCurve,
    energy_factor_curve: BiquadraticCurve,
    water_removal_curve_rated_value: f64,
    energy_factor_curve_rated_value: f64,
    accumulated_water_removal_l: f64,
}

impl Dehumidifier {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let zone = zone_id_from_config(&config).unwrap_or(ZoneId(1));
        Self {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(parse_u32_from_f64(config.get_f64(KEY_EQUIPMENT_ID)).unwrap_or(0)),
                name: config.name,
                end_use: EndUse::DEHUMIDIFIER,
                equipment_type: Cow::Borrowed("Dehumidifier"),
                zone: Some(zone),
                fuel: FuelType::Electric,
                stage: ExecutionStage::Thermal,
                control_capabilities: ControlCapabilities::HUMIDITY_SETPOINT
                    | ControlCapabilities::MODE_OVERRIDE,
                telemetry_fields: telemetry_fields(),
            },
            ports: vec![
                PortDeclaration::electrical(),
                PortDeclaration::thermal(zone),
            ],
            telemetry: default_telemetry(),
            zone_id: zone,
            operating_mode: OperatingMode::Off,
            is_on: false,
            mode_override: None,
            rated_water_removal_l_day: DEFAULT_RATED_WATER_REMOVAL_L_DAY,
            rated_energy_factor_l_kwh: 1.8,
            fraction_load_served: 1.0,
            target_rh: DEFAULT_TARGET_RH_FRACTION,
            min_rh: DEFAULT_TARGET_RH_FRACTION - DEFAULT_DEADBAND_HALF_WIDTH_RH_FRACTION,
            max_rh: DEFAULT_TARGET_RH_FRACTION + DEFAULT_DEADBAND_HALF_WIDTH_RH_FRACTION,
            water_removal_curve: BiquadraticCurve {
                coeffs: DEFAULT_NORMALIZED_CURVE,
                x1_bounds: DEFAULT_DB_BOUNDS_C,
                x2_bounds: DEFAULT_RH_BOUNDS,
            },
            energy_factor_curve: BiquadraticCurve {
                coeffs: DEFAULT_NORMALIZED_CURVE,
                x1_bounds: DEFAULT_DB_BOUNDS_C,
                x2_bounds: DEFAULT_RH_BOUNDS,
            },
            water_removal_curve_rated_value: 1.0,
            energy_factor_curve_rated_value: 1.0,
            accumulated_water_removal_l: 0.0,
        }
    }

    fn init_setpoints(&mut self, config: &EquipmentConfig) -> crate::Result<()> {
        let target_rh_raw = first_f64(
            config,
            &[
                KEY_TARGET_RH,
                KEY_DEHUMIDISTAT_SETPOINT,
                KEY_DEHUMIDISTAT_SETPOINT_ALT,
            ],
        )
        .unwrap_or(DEFAULT_TARGET_RH_FRACTION);
        let target_rh = parse_rh_fraction(target_rh_raw, "target_rh")?;
        let min_rh = config
            .get_f64(KEY_MIN_RH)
            .map(|v| parse_rh_fraction(v, KEY_MIN_RH))
            .transpose()?;
        let max_rh = config
            .get_f64(KEY_MAX_RH)
            .map(|v| parse_rh_fraction(v, KEY_MAX_RH))
            .transpose()?;
        let (min_rh, max_rh) = resolve_rh_band(target_rh, min_rh, max_rh)?;
        self.target_rh = target_rh;
        self.min_rh = min_rh;
        self.max_rh = max_rh;
        Ok(())
    }

    fn update_is_on(&mut self, current_rh: f64) {
        let maybe_override = self.mode_override;
        self.is_on = match maybe_override {
            Some(OperatingMode::Off) => false,
            Some(OperatingMode::Cooling) | Some(OperatingMode::Standby) => true,
            Some(_) | None => {
                if self.is_on {
                    current_rh >= self.min_rh
                } else {
                    current_rh > self.max_rh
                }
            }
        };
        self.operating_mode = if self.is_on {
            OperatingMode::Cooling
        } else {
            OperatingMode::Off
        };
    }

    fn performance_snapshot(&self, zone_temp_c: f64, zone_rh: f64) -> PerformanceSnapshot {
        if !self.is_on {
            return PerformanceSnapshot {
                water_removal_l_day: 0.0,
                electric_power_w: 0.0,
                latent_removal_w: 0.0,
                sensible_gain_w: 0.0,
            };
        }

        let wr_multiplier = evaluate_normalized_curve(
            &self.water_removal_curve,
            self.water_removal_curve_rated_value,
            zone_temp_c,
            zone_rh,
        );
        let ef_multiplier = evaluate_normalized_curve(
            &self.energy_factor_curve,
            self.energy_factor_curve_rated_value,
            zone_temp_c,
            zone_rh,
        );

        let water_removal_l_day =
            (self.rated_water_removal_l_day * self.fraction_load_served * wr_multiplier).max(0.0);
        let energy_factor_l_kwh = (self.rated_energy_factor_l_kwh * ef_multiplier).max(0.0);
        let water_removal_kg_s = water_removal_l_day * KG_PER_LITER_WATER / SECONDS_PER_DAY;
        let electric_power_w = if energy_factor_l_kwh > 0.0 {
            water_removal_kg_s * WATTS_PER_KILOWATT_HOUR / energy_factor_l_kwh
        } else {
            0.0
        };
        let latent_removal_w = water_removal_kg_s * LATENT_HEAT_VAPORIZATION_J_KG;
        let sensible_gain_w = latent_removal_w + electric_power_w;

        PerformanceSnapshot {
            water_removal_l_day,
            electric_power_w,
            latent_removal_w,
            sensible_gain_w,
        }
    }

    fn write_step_telemetry(&mut self, snapshot: PerformanceSnapshot) {
        self.telemetry
            .set("water_removal_l_day", snapshot.water_removal_l_day);
        self.telemetry
            .set("electric_power_w", snapshot.electric_power_w);
        self.telemetry
            .set("latent_removal_w", snapshot.latent_removal_w);
        self.telemetry
            .set("sensible_gain_w", snapshot.sensible_gain_w);
        self.telemetry.set("target_rh", self.target_rh);
        self.telemetry.set("min_rh", self.min_rh);
        self.telemetry.set("max_rh", self.max_rh);
        self.telemetry
            .set("is_on", if self.is_on { 1.0 } else { 0.0 });
    }
}

impl Equipment for Dehumidifier {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, _env: &EnvironmentState) -> crate::Result<()> {
        let equipment_id = equipment_id_from_config(config)?;
        self.descriptor.id = EquipmentId(equipment_id);
        self.zone_id = zone_id_from_config(config).unwrap_or(self.zone_id);
        self.descriptor.zone = Some(self.zone_id);
        self.ports = vec![
            PortDeclaration::electrical(),
            PortDeclaration::thermal(self.zone_id),
        ];

        self.rated_water_removal_l_day = first_f64(
            config,
            &[
                KEY_RATED_CAPACITY_L_DAY,
                KEY_RATED_CAPACITY_PINTS_DAY,
                KEY_RATED_CAPACITY_PINTS_DAY_ALT,
            ],
        )
        .map(|value| {
            if config.get_f64(KEY_RATED_CAPACITY_L_DAY).is_some() {
                value
            } else {
                value * PINTS_TO_LITERS
            }
        })
        .unwrap_or(DEFAULT_RATED_WATER_REMOVAL_L_DAY);
        if !self.rated_water_removal_l_day.is_finite() || self.rated_water_removal_l_day <= 0.0 {
            return Err(HaresError::Equipment(format!(
                "invalid dehumidifier rated capacity (L/day): {}",
                self.rated_water_removal_l_day
            )));
        }

        let maybe_ief = first_f64(
            config,
            &[
                KEY_INTEGRATED_ENERGY_FACTOR,
                KEY_INTEGRATED_ENERGY_FACTOR_ALT,
            ],
        );
        let maybe_ef = first_f64(config, &[KEY_ENERGY_FACTOR, KEY_ENERGY_FACTOR_ALT]);
        self.rated_energy_factor_l_kwh = maybe_ief.or(maybe_ef).unwrap_or(1.8);
        if maybe_ief.is_some() {
            tracing::warn!(
                "IntegratedEnergyFactor is used with EF-era curves; part-load predictions may deviate"
            );
        }
        if !self.rated_energy_factor_l_kwh.is_finite() || self.rated_energy_factor_l_kwh <= 0.0 {
            return Err(HaresError::Equipment(format!(
                "invalid dehumidifier rated energy factor (L/kWh): {}",
                self.rated_energy_factor_l_kwh
            )));
        }

        self.fraction_load_served = first_f64(
            config,
            &[KEY_FRACTION_LOAD_SERVED, KEY_FRACTION_LOAD_SERVED_ALT],
        )
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);

        self.init_setpoints(config)?;

        self.water_removal_curve = parse_curve(
            config,
            KEY_WR_CURVE,
            KEY_WR_PREFIX,
            DEFAULT_NORMALIZED_CURVE,
            [KEY_WR_T_MIN_C, KEY_WR_T_MAX_C],
            [KEY_WR_RH_MIN, KEY_WR_RH_MAX],
        )?;
        self.energy_factor_curve = parse_curve(
            config,
            KEY_EF_CURVE,
            KEY_EF_PREFIX,
            DEFAULT_NORMALIZED_CURVE,
            [KEY_EF_T_MIN_C, KEY_EF_T_MAX_C],
            [KEY_EF_RH_MIN, KEY_EF_RH_MAX],
        )?;
        self.water_removal_curve_rated_value = self
            .water_removal_curve
            .evaluate(RATED_DRY_BULB_C, RATED_RH_FRACTION);
        self.energy_factor_curve_rated_value = self
            .energy_factor_curve
            .evaluate(RATED_DRY_BULB_C, RATED_RH_FRACTION);
        if self.water_removal_curve_rated_value <= 0.0
            || !self.water_removal_curve_rated_value.is_finite()
        {
            return Err(HaresError::Equipment(
                "water removal curve rated-condition value must be finite and positive".to_string(),
            ));
        }
        if self.energy_factor_curve_rated_value <= 0.0
            || !self.energy_factor_curve_rated_value.is_finite()
        {
            return Err(HaresError::Equipment(
                "energy factor curve rated-condition value must be finite and positive".to_string(),
            ));
        }

        self.operating_mode = OperatingMode::Off;
        self.is_on = false;
        self.mode_override = None;
        self.accumulated_water_removal_l = 0.0;
        self.telemetry = default_telemetry();
        self.write_step_telemetry(PerformanceSnapshot {
            water_removal_l_day: 0.0,
            electric_power_w: 0.0,
            latent_removal_w: 0.0,
            sensible_gain_w: 0.0,
        });
        Ok(())
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        let zone = env.zones.iter().find(|z| z.id == self.zone_id);
        if let Some(zone_state) = zone {
            self.update_is_on(
                zone_state
                    .relative_humidity
                    .clamp(RH_MIN_FRACTION, RH_MAX_FRACTION),
            );
        } else {
            self.is_on = false;
            self.operating_mode = OperatingMode::Off;
        }
        self.operating_mode
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        self.update_control(env);
        let zone = env
            .zones
            .iter()
            .find(|z| z.id == self.zone_id)
            .ok_or_else(|| HaresError::Equipment(format!("zone {} not found", self.zone_id.0)))?;

        let snapshot = self.performance_snapshot(
            zone.temperature_c,
            zone.relative_humidity
                .clamp(RH_MIN_FRACTION, RH_MAX_FRACTION),
        );
        if snapshot.electric_power_w > 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_kw: snapshot.electric_power_w / WATTS_PER_KILOWATT,
                reactive_power_kvar: 0.0,
            })?;
        }
        if snapshot.sensible_gain_w != 0.0 || snapshot.latent_removal_w != 0.0 {
            ports.accumulate(&PortContribution::Thermal {
                zone: self.zone_id,
                sensible_gain_w: snapshot.sensible_gain_w,
                latent_gain_w: -snapshot.latent_removal_w,
                category: ThermalCategory::InternalGain,
            })?;
        }

        let water_removed_l = snapshot.water_removal_l_day * dt.as_secs_f64() / SECONDS_PER_DAY;
        self.accumulated_water_removal_l += water_removed_l.max(0.0);
        self.write_step_telemetry(snapshot);
        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&DehumidifierState {
            is_on: self.is_on,
            accumulated_water_removal_l: self.accumulated_water_removal_l,
            target_rh: self.target_rh,
            min_rh: self.min_rh,
            max_rh: self.max_rh,
            mode_override: self.mode_override,
        })
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: DehumidifierState = load_postcard(state)?;
        self.is_on = decoded.is_on;
        self.accumulated_water_removal_l = decoded.accumulated_water_removal_l;
        self.target_rh = decoded.target_rh;
        self.min_rh = decoded.min_rh;
        self.max_rh = decoded.max_rh;
        self.mode_override = decoded.mode_override;
        self.operating_mode = if self.is_on {
            OperatingMode::Cooling
        } else {
            OperatingMode::Off
        };
        self.write_step_telemetry(PerformanceSnapshot {
            water_removal_l_day: self.telemetry.get("water_removal_l_day").unwrap_or(0.0),
            electric_power_w: self.telemetry.get("electric_power_w").unwrap_or(0.0),
            latent_removal_w: self.telemetry.get("latent_removal_w").unwrap_or(0.0),
            sensible_gain_w: self.telemetry.get("sensible_gain_w").unwrap_or(0.0),
        });
        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        match signal {
            ControlSignal::HumiditySetpoint {
                target_rh,
                min_rh,
                max_rh,
            } => {
                let target = parse_rh_fraction(*target_rh, "target_rh")?;
                let min = (*min_rh)
                    .map(|v| parse_rh_fraction(v, KEY_MIN_RH))
                    .transpose()?;
                let max = (*max_rh)
                    .map(|v| parse_rh_fraction(v, KEY_MAX_RH))
                    .transpose()?;
                let (min, max) = resolve_rh_band(target, min, max)?;
                self.target_rh = target;
                self.min_rh = min;
                self.max_rh = max;
                self.telemetry.set("target_rh", self.target_rh);
                self.telemetry.set("min_rh", self.min_rh);
                self.telemetry.set("max_rh", self.max_rh);
            }
            ControlSignal::ModeOverride { mode } => {
                self.mode_override = Some(*mode);
            }
            _ => {}
        }
        Ok(())
    }
}

pub fn register_with_registry(registry: &mut EquipmentRegistry) {
    registry.register(
        "Dehumidifier",
        Box::new(|config| Box::new(Dehumidifier::new(config))),
    );
}

fn parse_u32_from_f64(raw: Option<f64>) -> Option<u32> {
    let value = raw?;
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value > u32::MAX as f64 {
        return None;
    }
    Some(value as u32)
}

fn parse_rh_fraction(value: f64, field: &str) -> crate::Result<f64> {
    if !value.is_finite() {
        return Err(HaresError::Equipment(format!("{field} must be finite")));
    }
    let normalized = if value > RH_MAX_FRACTION && value <= 100.0 {
        value / 100.0
    } else {
        value
    };
    if !(RH_MIN_FRACTION..=RH_MAX_FRACTION).contains(&normalized) {
        return Err(HaresError::Equipment(format!(
            "{field} must be in [0,1] fraction (or [0,100] percent), got {value}"
        )));
    }
    Ok(normalized)
}

fn resolve_rh_band(
    target_rh: f64,
    min_rh: Option<f64>,
    max_rh: Option<f64>,
) -> crate::Result<(f64, f64)> {
    let min = min_rh.unwrap_or(target_rh - DEFAULT_DEADBAND_HALF_WIDTH_RH_FRACTION);
    let max = max_rh.unwrap_or(target_rh + DEFAULT_DEADBAND_HALF_WIDTH_RH_FRACTION);
    if !min.is_finite() || !max.is_finite() {
        return Err(HaresError::Equipment(
            "min_rh and max_rh must be finite".to_string(),
        ));
    }
    if min >= max {
        return Err(HaresError::Equipment(format!(
            "invalid RH deadband: min_rh ({min}) must be < max_rh ({max})"
        )));
    }
    Ok((
        min.clamp(RH_MIN_FRACTION, RH_MAX_FRACTION),
        max.clamp(RH_MIN_FRACTION, RH_MAX_FRACTION),
    ))
}

fn evaluate_normalized_curve(
    curve: &BiquadraticCurve,
    rated_value: f64,
    dry_bulb_c: f64,
    rh_fraction: f64,
) -> f64 {
    let value = curve.evaluate(dry_bulb_c, rh_fraction);
    if !value.is_finite() || rated_value <= 0.0 || !rated_value.is_finite() {
        return 0.0;
    }
    (value / rated_value).max(0.0)
}

fn parse_curve(
    config: &EquipmentConfig,
    list_key: &str,
    prefix: &str,
    default_coeffs: [f64; 6],
    t_bound_keys: [&str; 2],
    rh_bound_keys: [&str; 2],
) -> crate::Result<BiquadraticCurve> {
    let coeffs = if let Some(raw) = config.get_str(list_key) {
        parse_coefficients(raw)?
    } else if let Some(coeffs) = parse_coefficients_from_prefix(config, prefix) {
        coeffs
    } else {
        default_coeffs
    };

    let t_min = config
        .get_f64(t_bound_keys[0])
        .unwrap_or(DEFAULT_DB_BOUNDS_C.0);
    let t_max = config
        .get_f64(t_bound_keys[1])
        .unwrap_or(DEFAULT_DB_BOUNDS_C.1);
    let rh_min_raw = config
        .get_f64(rh_bound_keys[0])
        .unwrap_or(DEFAULT_RH_BOUNDS.0);
    let rh_max_raw = config
        .get_f64(rh_bound_keys[1])
        .unwrap_or(DEFAULT_RH_BOUNDS.1);
    let rh_min = parse_rh_fraction(rh_min_raw, rh_bound_keys[0])?;
    let rh_max = parse_rh_fraction(rh_max_raw, rh_bound_keys[1])?;
    if !t_min.is_finite() || !t_max.is_finite() || t_min >= t_max {
        return Err(HaresError::Equipment(format!(
            "invalid temperature bounds for {prefix}: [{t_min}, {t_max}]"
        )));
    }
    if rh_min >= rh_max {
        return Err(HaresError::Equipment(format!(
            "invalid RH bounds for {prefix}: [{rh_min}, {rh_max}]"
        )));
    }

    Ok(BiquadraticCurve {
        coeffs,
        x1_bounds: (t_min, t_max),
        x2_bounds: (rh_min, rh_max),
    })
}

fn parse_coefficients(raw: &str) -> crate::Result<[f64; 6]> {
    let mut coeffs = [0.0; 6];
    let parts: Vec<&str> = raw.split(',').map(str::trim).collect();
    if parts.len() != coeffs.len() {
        return Err(HaresError::Equipment(format!(
            "expected 6 biquadratic coefficients, got {} in '{raw}'",
            parts.len()
        )));
    }
    for (idx, part) in parts.iter().enumerate() {
        coeffs[idx] = part.parse::<f64>().map_err(|error| {
            HaresError::Equipment(format!(
                "failed to parse biquadratic coefficient index {idx} ('{part}'): {error}"
            ))
        })?;
    }
    Ok(coeffs)
}

fn parse_coefficients_from_prefix(config: &EquipmentConfig, prefix: &str) -> Option<[f64; 6]> {
    let keys = [
        format!("{prefix}_a"),
        format!("{prefix}_b"),
        format!("{prefix}_c"),
        format!("{prefix}_d"),
        format!("{prefix}_e"),
        format!("{prefix}_f"),
    ];
    let mut coeffs = [0.0; 6];
    for (idx, key) in keys.iter().enumerate() {
        let value = config.get_f64(key)?;
        coeffs[idx] = value;
    }
    Some(coeffs)
}

fn default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(8);
    telemetry.insert("water_removal_l_day", 0.0);
    telemetry.insert("electric_power_w", 0.0);
    telemetry.insert("latent_removal_w", 0.0);
    telemetry.insert("sensible_gain_w", 0.0);
    telemetry.insert("target_rh", DEFAULT_TARGET_RH_FRACTION);
    telemetry.insert(
        "min_rh",
        DEFAULT_TARGET_RH_FRACTION - DEFAULT_DEADBAND_HALF_WIDTH_RH_FRACTION,
    );
    telemetry.insert(
        "max_rh",
        DEFAULT_TARGET_RH_FRACTION + DEFAULT_DEADBAND_HALF_WIDTH_RH_FRACTION,
    );
    telemetry.insert("is_on", 0.0);
    telemetry
}

fn telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: "water_removal_l_day".to_string(),
            unit: "L/day".to_string(),
            description: "Water removed from air at current conditions".to_string(),
        },
        TelemetryField {
            name: "electric_power_w".to_string(),
            unit: "W".to_string(),
            description: "Compressor + fan electric power draw".to_string(),
        },
        TelemetryField {
            name: "latent_removal_w".to_string(),
            unit: "W".to_string(),
            description: "Latent cooling removed from zone air".to_string(),
        },
        TelemetryField {
            name: "sensible_gain_w".to_string(),
            unit: "W".to_string(),
            description: "Sensible heat gain dumped back to zone".to_string(),
        },
        TelemetryField {
            name: "target_rh".to_string(),
            unit: "fraction".to_string(),
            description: "Active RH setpoint target".to_string(),
        },
        TelemetryField {
            name: "min_rh".to_string(),
            unit: "fraction".to_string(),
            description: "Deadband lower bound".to_string(),
        },
        TelemetryField {
            name: "max_rh".to_string(),
            unit: "fraction".to_string(),
            description: "Deadband upper bound".to_string(),
        },
        TelemetryField {
            name: "is_on".to_string(),
            unit: "bool".to_string(),
            description: "1 when compressor/fan are on, else 0".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, time::Duration};

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, EnvironmentState, ExecutionStage, GridState, OperatingMode, PortSlots,
        ThermalAccumulator, WeatherState, ZoneId, ZoneState,
    };

    use super::{
        Dehumidifier, KEY_DEHUMIDISTAT_SETPOINT, KEY_ENERGY_FACTOR, KEY_EQUIPMENT_ID,
        KEY_RATED_CAPACITY_PINTS_DAY, LATENT_HEAT_VAPORIZATION_J_KG, SECONDS_PER_DAY,
        WATTS_PER_KILOWATT, register_with_registry,
    };
    use crate::{Equipment, EquipmentConfig, EquipmentRegistry};

    const TOLERANCE_REL: f64 = 1e-9;

    fn approx_eq(a: f64, b: f64) {
        let denom = b.abs().max(1e-12);
        let rel_err = (a - b).abs() / denom;
        assert!(
            rel_err <= TOLERANCE_REL,
            "values differ: left={a}, right={b}, rel_err={rel_err}"
        );
    }

    fn env(relative_humidity: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 26.666_666_666_7,
                humidity_ratio: 0.010,
                relative_humidity,
                wet_bulb_c: 20.0,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 30.0,
                outdoor_humidity_ratio: 0.012,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 20.0,
                sky_temp_c: 15.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![],
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                ..Default::default()
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            current_time: FixedOffset::east_opt(0)
                .expect("UTC offset")
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .unwrap(),
            time_res: ChronoDuration::seconds(60),
        }
    }

    fn config() -> EquipmentConfig {
        let mut raw_config = HashMap::new();
        raw_config.insert(KEY_EQUIPMENT_ID.to_string(), 9.0.into());
        raw_config.insert("zone_id".to_string(), 1.0.into());
        raw_config.insert(KEY_RATED_CAPACITY_PINTS_DAY.to_string(), 70.0.into());
        raw_config.insert(KEY_ENERGY_FACTOR.to_string(), 2.0.into());
        raw_config.insert(KEY_DEHUMIDISTAT_SETPOINT.to_string(), 50.0.into());
        EquipmentConfig {
            name: "Test Dehumidifier".to_string(),
            ochre_class: "Dehumidifier".to_string(),
            raw_config,
        }
    }

    fn ports() -> PortSlots {
        PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        }
    }

    #[test]
    fn off_outputs_are_zero() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.40)).unwrap();

        let mut slots = ports();
        eq.step(&env(0.40), Duration::from_secs(60), &mut slots)
            .unwrap();
        assert_eq!(eq.update_control(&env(0.40)), OperatingMode::Off);
        assert_eq!(eq.telemetry().get("water_removal_l_day"), Some(0.0));
        assert_eq!(eq.telemetry().get("electric_power_w"), Some(0.0));
        assert_eq!(eq.telemetry().get("latent_removal_w"), Some(0.0));
        assert_eq!(eq.telemetry().get("sensible_gain_w"), Some(0.0));
        assert_eq!(slots.electrical.load_power_kw, 0.0);
        assert_eq!(slots.thermal[0].sensible_gain_w, 0.0);
        assert_eq!(slots.thermal[0].latent_gain_w, 0.0);
    }

    #[test]
    fn deadband_turns_on_and_off_with_hold_behavior() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.40)).unwrap();

        // target=0.50 -> min=0.475, max=0.525
        assert_eq!(eq.update_control(&env(0.53)), OperatingMode::Cooling);
        assert_eq!(eq.update_control(&env(0.50)), OperatingMode::Cooling);
        assert_eq!(eq.update_control(&env(0.48)), OperatingMode::Cooling);
        assert_eq!(eq.update_control(&env(0.47)), OperatingMode::Off);
        assert_eq!(eq.update_control(&env(0.50)), OperatingMode::Off);
    }

    #[test]
    fn rated_condition_reproduces_rated_outputs() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.60)).unwrap();

        let mut slots = ports();
        eq.step(&env(0.60), Duration::from_secs(60), &mut slots)
            .unwrap();

        let water_l_day = eq.telemetry().get("water_removal_l_day").unwrap();
        let electric_power_w = eq.telemetry().get("electric_power_w").unwrap();
        let latent_removal_w = eq.telemetry().get("latent_removal_w").unwrap();

        // 70 pints/day -> liters/day
        let rated_l_day = 70.0 * 0.473_176_473;
        approx_eq(water_l_day, rated_l_day);

        let expected_electric_w = (rated_l_day / SECONDS_PER_DAY) * 3_600_000.0 / 2.0;
        approx_eq(electric_power_w, expected_electric_w);
        let expected_latent_w = (rated_l_day / SECONDS_PER_DAY) * LATENT_HEAT_VAPORIZATION_J_KG;
        approx_eq(latent_removal_w, expected_latent_w);
    }

    #[test]
    fn energy_balance_and_ports_are_consistent() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.62)).unwrap();

        let mut slots = ports();
        eq.step(&env(0.62), Duration::from_secs(60), &mut slots)
            .unwrap();

        let electric_power_w = eq.telemetry().get("electric_power_w").unwrap();
        let latent_removal_w = eq.telemetry().get("latent_removal_w").unwrap();
        let sensible_gain_w = eq.telemetry().get("sensible_gain_w").unwrap();

        approx_eq(sensible_gain_w, latent_removal_w + electric_power_w);
        approx_eq(
            slots.electrical.load_power_kw,
            electric_power_w / WATTS_PER_KILOWATT,
        );
        approx_eq(slots.thermal[0].sensible_gain_w, sensible_gain_w);
        approx_eq(slots.thermal[0].latent_gain_w, -latent_removal_w);
    }

    #[test]
    fn save_load_round_trip_preserves_state_and_subsequent_output() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.62)).unwrap();

        let mut slots_a = ports();
        eq.step(&env(0.62), Duration::from_secs(60), &mut slots_a)
            .unwrap();
        let state = eq.save_state();

        let mut restored = Dehumidifier::new(cfg.clone());
        restored.init(&cfg, &env(0.62)).unwrap();
        restored.load_state(&state).unwrap();
        let mut slots_b = ports();
        restored
            .step(&env(0.62), Duration::from_secs(60), &mut slots_b)
            .unwrap();

        approx_eq(
            restored.telemetry().get("water_removal_l_day").unwrap(),
            eq.telemetry().get("water_removal_l_day").unwrap(),
        );
        approx_eq(
            restored.telemetry().get("electric_power_w").unwrap(),
            eq.telemetry().get("electric_power_w").unwrap(),
        );
        approx_eq(
            restored.telemetry().get("latent_removal_w").unwrap(),
            eq.telemetry().get("latent_removal_w").unwrap(),
        );
        approx_eq(
            restored.telemetry().get("sensible_gain_w").unwrap(),
            eq.telemetry().get("sensible_gain_w").unwrap(),
        );
    }

    #[test]
    fn control_signal_applies_setpoint_and_mode_override() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.45)).unwrap();
        eq.apply_control(&ControlSignal::HumiditySetpoint {
            target_rh: 55.0,
            min_rh: None,
            max_rh: None,
        })
        .unwrap();
        approx_eq(eq.telemetry().get("target_rh").unwrap(), 0.55);
        approx_eq(eq.telemetry().get("min_rh").unwrap(), 0.525);
        approx_eq(eq.telemetry().get("max_rh").unwrap(), 0.575);
        assert_eq!(eq.update_control(&env(0.58)), OperatingMode::Cooling);

        eq.apply_control(&ControlSignal::ModeOverride {
            mode: OperatingMode::Off,
        })
        .unwrap();
        assert_eq!(eq.update_control(&env(0.90)), OperatingMode::Off);
    }

    #[test]
    fn registry_includes_dehumidifier_and_thermal_stage() {
        let mut registry = EquipmentRegistry::new();
        register_with_registry(&mut registry);
        let eq = registry.create("Dehumidifier", config()).unwrap();
        assert_eq!(eq.descriptor().stage, ExecutionStage::Thermal);
    }
}
