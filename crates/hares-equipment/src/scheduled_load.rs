//! Scheduled (deterministic) load equipment model.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use chrono::Datelike;
use hares_physics::constants::GAS_THERMS_PER_HOUR_TO_W;
use hares_types::{
    BoundaryPolicy, ControlCapabilities, ControlSignal, EndUse, EnvironmentState,
    EquipmentDescriptor, EquipmentId, ExecutionStage, FuelType, HaresError, OperatingMode,
    PortContribution, PortDeclaration, PortSlots, ScheduleSource, Telemetry, TelemetryField,
    ThermalCategory, ZoneId,
};
use serde::{Deserialize, Serialize};

use crate::schedule_helpers::{
    ScheduleSourceState, capture_schedule_source_state, parse_u32, parse_usize, parse_zone_id,
    restore_schedule_source_state,
};
use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

use crate::config::KEY_EQUIPMENT_ID;
const KEY_SENSIBLE_GAIN_FRACTION: &str = "sensible_gain_fraction";
// Reserved for future radiant/convective split (OCHRE ScheduledLoad heat gain decomposition).
#[allow(dead_code)]
const KEY_CONVECTIVE_GAIN_FRACTION: &str = "convective_gain_fraction";
#[allow(dead_code)]
const KEY_RADIATIVE_GAIN_FRACTION: &str = "radiative_gain_fraction";
const KEY_LATENT_GAIN_FRACTION: &str = "latent_gain_fraction";
const KEY_ZIP_Z: &str = "zip_z";
const KEY_ZIP_I: &str = "zip_i";
const KEY_ZIP_P: &str = "zip_p";
const KEY_ZIP_ZQ: &str = "zip_zq";
const KEY_ZIP_IQ: &str = "zip_iq";
const KEY_ZIP_PQ: &str = "zip_pq";
const KEY_ZIP_PF: &str = "zip_pf";
// Reserved for configurable reference voltage in the ZIP model.
#[allow(dead_code)]
const KEY_ZIP_V0: &str = "zip_v0";
const KEY_GAS_SCHEDULE_IS_W: &str = "gas_schedule_is_w";
const KEY_POWER_SCHEDULE_SOURCE: &str = "power_schedule_source";
const KEY_POWER_SCHEDULE_COL: &str = "power_schedule_col";
const KEY_POWER_PROFILE_MAX_KW: &str = "power_profile_max_kw";
const KEY_POWER_PROFILE_WEEKDAY: &str = "power_profile_weekday";
const KEY_POWER_PROFILE_WEEKEND: &str = "power_profile_weekend";
const KEY_POWER_PROFILE_MONTH: &str = "power_profile_month";
const KEY_POWER_CONSTANT_KW: &str = "power_constant_kw";
const KEY_GAS_SCHEDULE_SOURCE: &str = "gas_schedule_source";
const KEY_GAS_SCHEDULE_COL: &str = "gas_schedule_col";
const KEY_GAS_PROFILE_MAX: &str = "gas_profile_max";
const KEY_GAS_PROFILE_WEEKDAY: &str = "gas_profile_weekday";
const KEY_GAS_PROFILE_WEEKEND: &str = "gas_profile_weekend";
const KEY_GAS_PROFILE_MONTH: &str = "gas_profile_month";
const KEY_GAS_CONSTANT: &str = "gas_constant";
// Reserved for seasonal scaling of scheduled loads.
#[allow(dead_code)]
const KEY_MONTH_MULTIPLIER_PREFIX: &str = "month_multiplier_";
const ZIP_SUM_TARGET: f64 = 1.0;
const ZIP_SUM_TOLERANCE: f64 = 1e-9;

#[derive(Clone, Copy, Debug, PartialEq)]
struct ZipCoefficients {
    /// Real-power impedance fraction.
    z: f64,
    /// Real-power current fraction.
    i: f64,
    /// Real-power constant-power fraction.
    p_coeff: f64,
    /// Reference voltage for ZIP normalization (per-unit). Default 1.0 means no
    /// normalisation relative to rated conditions.
    /// OCHRE Equipment.py: v0 is the voltage at which the ZIP model was calibrated.
    v0: f64,
    /// Reactive-power impedance fraction (Zq in OCHRE ZIP Parameters.csv).
    zq: f64,
    /// Reactive-power current fraction (Iq).
    iq: f64,
    /// Reactive-power constant fraction (Pq).
    pq: f64,
    /// Power-factor multiplier: reactive_power = real_power * pf * (Zq*v² + Iq*v + Pq).
    pf: f64,
}

impl Default for ZipCoefficients {
    fn default() -> Self {
        Self {
            z: 0.0,
            i: 0.0,
            p_coeff: 1.0,
            v0: 1.0,
            // Default reactive coefficients produce zero reactive power (pf=0).
            zq: 0.0,
            iq: 0.0,
            pq: 1.0,
            pf: 0.0,
        }
    }
}

/// Zone ID assigned by convention to the garage zone (sort key 2 in building.rs).
const GARAGE_ZONE_ID: ZoneId = ZoneId(2);
/// Zone ID assigned by convention to the foundation/basement zone (sort key 3 in building.rs).
const FOUNDATION_ZONE_ID: ZoneId = ZoneId(3);

#[derive(Clone, Copy, Debug)]
enum GasScheduleUnit {
    Watts,
    ThermsPerHour,
}

impl GasScheduleUnit {
    fn consumption_w(self, value: f64) -> f64 {
        match self {
            Self::Watts => value,
            Self::ThermsPerHour => value * GAS_THERMS_PER_HOUR_TO_W,
        }
    }
}

fn slice_to_24(src: &[f64]) -> [f64; 24] {
    let mut arr = [0.0; 24];
    let n = src.len().min(24);
    arr[..n].copy_from_slice(&src[..n]);
    arr
}

fn slice_to_12(src: &[f64]) -> [f64; 12] {
    let mut arr = [1.0; 12];
    let n = src.len().min(12);
    arr[..n].copy_from_slice(&src[..n]);
    arr
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ScheduledLoadState {
    last_non_zero_power_kw: f64,
    last_non_zero_gas_w: f64,
    load_fraction: f64,
    mode_override: Option<OperatingMode>,
    power_source_state: ScheduleSourceState,
    gas_source_state: Option<ScheduleSourceState>,
}

/// Deterministic load driven by a time-indexed schedule.
pub struct ScheduledLoad {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    gas_source: Option<ScheduleSource>,
    gas_schedule_unit: GasScheduleUnit,
    sensible_gain_fraction: f64,
    latent_gain_fraction: f64,
    zip: ZipCoefficients,
    /// Per-month scale factors [0..11] applied after load_fraction.
    // OCHRE ScheduledLoad.py:38-41: month_multipliers zeros schedule in specified months.
    // Used for seasonal equipment like ceiling fans (zero in winter months).
    month_multipliers: Option<[f64; 12]>,
    last_non_zero_power_kw: f64,
    last_non_zero_gas_w: f64,
    load_fraction: f64,
    mode_override: Option<OperatingMode>,
    /// Per-step absolute power override set by `PowerSetpoint`. Cleared after each step.
    power_setpoint_override: Option<f64>,
    power_source: ScheduleSource,
}

impl ScheduledLoad {
    #[must_use]
    pub fn new(config: EquipmentConfig, end_use: EndUse, equipment_type: &'static str) -> Self {
        // OCHRE convention: equipment whose name contains "Exterior" or "Outdoor"
        // has no zone assignment — its heat gain goes to the outdoor environment,
        // not the building envelope. "Garage" and "Basement" equipment auto-routes
        // to the respective zone when no explicit zone_id is provided.
        let name_lower = config.name.to_ascii_lowercase();
        let zone = if name_lower.contains("exterior") || name_lower.contains("outdoor") {
            None
        } else if let Some(explicit) = parse_zone_id(&config.raw_config) {
            Some(explicit)
        } else if name_lower.contains("garage") {
            Some(GARAGE_ZONE_ID)
        } else if name_lower.contains("basement") {
            Some(FOUNDATION_ZONE_ID)
        } else {
            // Indoor equipment defaults to the primary conditioned zone.
            Some(ZoneId(1))
        };
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(parse_u32(&config.raw_config, KEY_EQUIPMENT_ID).unwrap_or_default()),
            name: config.name,
            end_use,
            equipment_type: Cow::Borrowed(equipment_type),
            zone,
            fuel: FuelType::Electric,
            stage: ExecutionStage::Independent,
            control_capabilities: ControlCapabilities::LOAD_FRACTION
                | ControlCapabilities::MODE_OVERRIDE
                | ControlCapabilities::POWER_SETPOINT,
            telemetry_fields: scheduled_load_telemetry_fields(),
        };
        Self {
            descriptor,
            ports: vec![PortDeclaration::electrical()],
            telemetry: default_telemetry(),
            gas_source: None,
            gas_schedule_unit: GasScheduleUnit::ThermsPerHour,
            sensible_gain_fraction: 0.0,
            latent_gain_fraction: 0.0,
            zip: ZipCoefficients::default(),
            month_multipliers: None,
            last_non_zero_power_kw: 0.0,
            last_non_zero_gas_w: 0.0,
            load_fraction: 1.0,
            mode_override: None,
            power_setpoint_override: None,
            power_source: ScheduleSource::Constant(0.0),
        }
    }

    fn update_ports(&mut self) {
        self.ports.clear();
        self.ports.push(PortDeclaration::electrical());
        if let Some(zone) = self.descriptor.zone {
            self.ports.push(PortDeclaration::thermal(zone));
        }
        if self.gas_source.is_some() {
            self.ports.push(PortDeclaration::fuel());
        }
    }

    fn init_from_config(
        &mut self,
        config: &EquipmentConfig,
        _env: &EnvironmentState,
    ) -> crate::Result<()> {
        self.power_source = parse_power_schedule_source(config)?;
        let (gas_source, gas_unit) = parse_optional_gas_schedule_source(config)?;
        self.gas_source = gas_source;
        self.gas_schedule_unit = if parse_bool(config, KEY_GAS_SCHEDULE_IS_W)?.unwrap_or(false) {
            GasScheduleUnit::Watts
        } else {
            gas_unit
        };
        // OCHRE Equipment.py:83-86: sensible gain = convective + radiative fractions.
        // "frac_sensible" is the HPXML-parsed alias for sensible_gain_fraction.
        self.sensible_gain_fraction = config
            .get_f64(KEY_SENSIBLE_GAIN_FRACTION)
            .or_else(|| config.get_f64("frac_sensible"))
            .or_else(|| {
                let conv = config.get_f64(KEY_CONVECTIVE_GAIN_FRACTION).unwrap_or(0.0);
                let rad = config.get_f64(KEY_RADIATIVE_GAIN_FRACTION).unwrap_or(0.0);
                if conv > 0.0 || rad > 0.0 {
                    Some(conv + rad)
                } else {
                    None
                }
            })
            .unwrap_or_else(|| {
                tracing::warn!(
                    "sensible_gain_fraction missing for '{}'; using 0.5",
                    self.descriptor.name
                );
                0.5
            });
        // "frac_latent" is the HPXML-parsed alias for latent_gain_fraction.
        self.latent_gain_fraction = config
            .get_f64(KEY_LATENT_GAIN_FRACTION)
            .or_else(|| config.get_f64("frac_latent"))
            .unwrap_or(0.0);
        if self.sensible_gain_fraction + self.latent_gain_fraction > 1.0 + 1e-9 {
            return Err(HaresError::Equipment(format!(
                "sensible_gain_fraction ({}) + latent_gain_fraction ({}) must not exceed 1.0",
                self.sensible_gain_fraction, self.latent_gain_fraction
            )));
        }
        self.zip = parse_zip_coefficients(config)?;
        self.month_multipliers = parse_month_multipliers(config);
        if matches!(self.power_source, ScheduleSource::DailyProfile { .. }) {
            // Month multipliers are already baked into the DailyProfile evaluation,
            // so clear runtime month multipliers to avoid double-scaling.
            self.month_multipliers = None;
        }
        let usage_multiplier = config.get_f64("usage_multiplier").unwrap_or(1.0);
        if usage_multiplier != 1.0 {
            scale_schedule_source(&mut self.power_source, usage_multiplier);
            if let Some(gas_source) = &mut self.gas_source {
                scale_schedule_source(gas_source, usage_multiplier);
            }
        }
        self.last_non_zero_power_kw = 0.0;
        self.last_non_zero_gas_w = 0.0;
        self.load_fraction = 1.0;
        self.mode_override = None;
        self.power_setpoint_override = None;
        // Update primary fuel type: if a gas schedule exists and the electric
        // schedule is all zeros, this is a gas-only load. Otherwise we keep
        // Electric as the primary fuel (gas consumption is tracked via the Fuel
        // port regardless).
        if self.gas_source.is_some() && is_schedule_source_zero(&self.power_source) {
            self.descriptor.fuel = FuelType::Gas;
        }
        self.telemetry = default_telemetry();
        self.update_ports();
        Ok(())
    }
}

impl Equipment for ScheduledLoad {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.init_from_config(config, env)
    }

    fn update_control(&mut self, _env: &EnvironmentState) -> OperatingMode {
        if self.last_non_zero_power_kw > 0.0 || self.last_non_zero_gas_w > 0.0 {
            OperatingMode::Standby
        } else {
            OperatingMode::Off
        }
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        _dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        // Grid outage: all outputs are zero.
        if env.grid.voltage_pu == 0.0 {
            self.last_non_zero_power_kw = 0.0;
            self.last_non_zero_gas_w = 0.0;
            self.telemetry.set("electric_kw", 0.0);
            self.telemetry.set("sensible_gain_w", 0.0);
            self.telemetry.set("latent_gain_w", 0.0);
            self.telemetry.set("fuel_input_w", 0.0);
            return Ok(());
        }

        // Mode override Off forces zero output regardless of schedule.
        if self.mode_override == Some(OperatingMode::Off) {
            self.last_non_zero_power_kw = 0.0;
            self.last_non_zero_gas_w = 0.0;
            self.telemetry.set("electric_kw", 0.0);
            self.telemetry.set("sensible_gain_w", 0.0);
            self.telemetry.set("latent_gain_w", 0.0);
            self.telemetry.set("fuel_input_w", 0.0);
            return Ok(());
        }

        // PowerSetpoint overrides the schedule entirely for this step. The override
        // is cleared at the end of this step (per-step, not persistent).
        let (electric_power_kw, reactive_power_kvar, gas_consumption_w) =
            if let Some(override_kw) = self.power_setpoint_override.take() {
                (override_kw, 0.0, 0.0)
            } else {
                // Apply month multiplier (OCHRE ScheduledLoad.py:38-41). Negative schedule
                // values are silently clamped to zero — OCHRE treats them as "no load" rather
                // than generation; this is intentional for e.g. CSV schedules with placeholder
                // fill values.
                let month_idx = env.current_time.month0() as usize;
                let month_scale = self.month_multipliers.map(|m| m[month_idx]).unwrap_or(1.0);
                let raw_schedule_kw = self.power_source.value_at(env)?;

                let scheduled_power_kw = if raw_schedule_kw > 0.0 {
                    raw_schedule_kw * self.load_fraction * month_scale
                } else {
                    0.0
                };

                let gas_schedule_value = if let Some(gas_source) = &mut self.gas_source {
                    gas_source.value_at(env)?
                } else {
                    0.0
                };
                let gas_w = if gas_schedule_value > 0.0 {
                    // OCHRE ScheduledLoad.py:54-55: Load Fraction scales both electric AND gas.
                    self.gas_schedule_unit
                        .consumption_w(gas_schedule_value * self.load_fraction * month_scale)
                } else {
                    0.0
                };

                let (real_kw, reactive_kvar) = if scheduled_power_kw > 0.0 {
                    let v_norm = env.grid.voltage_pu / self.zip.v0;
                    let zip_multiplier =
                        self.zip.z * v_norm * v_norm + self.zip.i * v_norm + self.zip.p_coeff;
                    let real_kw = scheduled_power_kw * zip_multiplier;
                    let reactive_base =
                        self.zip.zq * v_norm * v_norm + self.zip.iq * v_norm + self.zip.pq;
                    let reactive_kvar = real_kw * self.zip.pf * reactive_base;
                    (real_kw, reactive_kvar)
                } else {
                    (0.0, 0.0)
                };
                (real_kw, reactive_kvar, gas_w)
            };

        if electric_power_kw > 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_kw: electric_power_kw,
                reactive_power_kvar,
            })?;
            self.last_non_zero_power_kw = electric_power_kw;
        } else {
            self.last_non_zero_power_kw = 0.0;
        }

        if gas_consumption_w > 0.0 {
            ports.accumulate(&PortContribution::Fuel {
                fuel_type: FuelType::Gas,
                consumption_w: gas_consumption_w,
            })?;
            self.last_non_zero_gas_w = gas_consumption_w;
        } else {
            self.last_non_zero_gas_w = 0.0;
        }

        let total_gain_source_w = electric_power_kw * 1_000.0 + gas_consumption_w;
        let sensible_gain_w = total_gain_source_w * self.sensible_gain_fraction;
        let latent_gain_w = total_gain_source_w * self.latent_gain_fraction;
        if let Some(zone) = self.descriptor.zone {
            if sensible_gain_w != 0.0 || latent_gain_w != 0.0 {
                ports.accumulate(&PortContribution::Thermal {
                    zone,
                    sensible_gain_w,
                    latent_gain_w,
                    category: ThermalCategory::InternalGain,
                })?;
            }
        }

        self.telemetry.set("electric_kw", electric_power_kw);
        self.telemetry.set("sensible_gain_w", sensible_gain_w);
        self.telemetry.set("latent_gain_w", latent_gain_w);
        self.telemetry.set("fuel_input_w", gas_consumption_w);
        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&ScheduledLoadState {
            last_non_zero_power_kw: self.last_non_zero_power_kw,
            last_non_zero_gas_w: self.last_non_zero_gas_w,
            load_fraction: self.load_fraction,
            mode_override: self.mode_override,
            power_source_state: capture_schedule_source_state(&self.power_source),
            gas_source_state: self.gas_source.as_ref().map(capture_schedule_source_state),
        })
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: ScheduledLoadState = load_postcard(state)?;
        self.last_non_zero_power_kw = decoded.last_non_zero_power_kw;
        self.last_non_zero_gas_w = decoded.last_non_zero_gas_w;
        self.load_fraction = decoded.load_fraction;
        self.mode_override = decoded.mode_override;
        restore_schedule_source_state(&mut self.power_source, &decoded.power_source_state)?;
        match (&mut self.gas_source, decoded.gas_source_state.as_ref()) {
            (Some(source), Some(saved)) => restore_schedule_source_state(source, saved)?,
            (None, Some(_)) => {
                return Err(HaresError::Equipment(
                    "checkpoint has gas schedule state but equipment has no gas schedule source"
                        .to_string(),
                ));
            }
            (Some(source), None) => match source {
                ScheduleSource::Constant(_)
                | ScheduleSource::DailyProfile { .. }
                | ScheduleSource::ColumnRef { .. }
                | ScheduleSource::SolarAware { .. }
                | ScheduleSource::TimeWindows { .. } => {}
                _ => {
                    return Err(HaresError::Equipment(
                            "checkpoint is missing gas schedule state for a stateful gas schedule source".to_string(),
                        ));
                }
            },
            (None, None) => {}
        }
        self.telemetry
            .insert("electric_kw", self.last_non_zero_power_kw);
        // last_non_zero_power_kw already holds the ZIP-adjusted kW value (set in step()),
        // so the thermal gain reconstruction below is accurate — it does not need a
        // separate voltage correction. The same ZIP-adjusted wattage drives sensible/latent
        // heat gains regardless of what the voltage was at the time of last non-zero output.
        let total_gain_source_w = self.last_non_zero_power_kw * 1_000.0 + self.last_non_zero_gas_w;
        self.telemetry.insert(
            "sensible_gain_w",
            total_gain_source_w * self.sensible_gain_fraction,
        );
        self.telemetry.insert(
            "latent_gain_w",
            total_gain_source_w * self.latent_gain_fraction,
        );
        self.telemetry
            .insert("fuel_input_w", self.last_non_zero_gas_w);
        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        match signal {
            ControlSignal::LoadFraction { fraction } => {
                self.load_fraction = *fraction;
                Ok(())
            }
            ControlSignal::ModeOverride { mode } => {
                self.mode_override = Some(*mode);
                Ok(())
            }
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                if !active_power_kw.is_finite() {
                    return Err(HaresError::Control(
                        "ScheduledLoad PowerSetpoint active_power_kw must be finite".to_string(),
                    ));
                }
                self.power_setpoint_override = Some(active_power_kw.max(0.0));
                Ok(())
            }
            _ => Err(HaresError::Control(format!(
                "ScheduledLoad does not accept control signal: {signal:?}"
            ))),
        }
    }
}

pub fn register_with_registry(registry: &mut EquipmentRegistry) {
    registry.register(
        "Lighting",
        Box::new(|config| Box::new(ScheduledLoad::new(config, EndUse::LIGHTING, "Lighting"))),
    );
    registry.register(
        "Plug Loads",
        Box::new(|config| Box::new(ScheduledLoad::new(config, EndUse::PLUG_LOADS, "Plug Loads"))),
    );
    registry.register(
        "Other",
        Box::new(|config| Box::new(ScheduledLoad::new(config, EndUse::OTHER, "Other"))),
    );

    // Appliance loads
    registry.register(
        "Refrigerator",
        Box::new(|config| {
            Box::new(ScheduledLoad::new(
                config,
                EndUse::REFRIGERATION,
                "Refrigerator",
            ))
        }),
    );
    registry.register(
        "Freezer",
        Box::new(|config| Box::new(ScheduledLoad::new(config, EndUse::REFRIGERATION, "Freezer"))),
    );
    registry.register(
        "MELs",
        Box::new(|config| Box::new(ScheduledLoad::new(config, EndUse::PLUG_LOADS, "MELs"))),
    );
    registry.register(
        "TV",
        Box::new(|config| Box::new(ScheduledLoad::new(config, EndUse::PLUG_LOADS, "TV"))),
    );

    // Pumps
    registry.register(
        "Well Pump",
        Box::new(|config| Box::new(ScheduledLoad::new(config, EndUse::OTHER, "Well Pump"))),
    );
    registry.register(
        "Pool Pump",
        Box::new(|config| Box::new(ScheduledLoad::new(config, EndUse::OTHER, "Pool Pump"))),
    );
    registry.register(
        "Pool Heater",
        Box::new(|config| Box::new(ScheduledLoad::new(config, EndUse::OTHER, "Pool Heater"))),
    );
    registry.register(
        "Spa Pump",
        Box::new(|config| Box::new(ScheduledLoad::new(config, EndUse::OTHER, "Spa Pump"))),
    );
    registry.register(
        "Spa Heater",
        Box::new(|config| Box::new(ScheduledLoad::new(config, EndUse::OTHER, "Spa Heater"))),
    );

    // Gas appliances
    registry.register(
        "Gas Grill",
        Box::new(|config| Box::new(ScheduledLoad::new(config, EndUse::OTHER, "Gas Grill"))),
    );
    registry.register(
        "Gas Fireplace",
        Box::new(|config| Box::new(ScheduledLoad::new(config, EndUse::OTHER, "Gas Fireplace"))),
    );
    registry.register(
        "Gas Lighting",
        Box::new(|config| Box::new(ScheduledLoad::new(config, EndUse::LIGHTING, "Gas Lighting"))),
    );

    // Fans
    registry.register(
        "Ceiling Fan",
        Box::new(|config| {
            Box::new(ScheduledLoad::new(
                config,
                EndUse::VENTILATION,
                "Ceiling Fan",
            ))
        }),
    );
    registry.register(
        "Ventilation Fan",
        Box::new(|config| {
            Box::new(ScheduledLoad::new(
                config,
                EndUse::VENTILATION,
                "Ventilation Fan",
            ))
        }),
    );

    // Lighting variants
    registry.register(
        "Indoor Lighting",
        Box::new(|config| {
            Box::new(ScheduledLoad::new(
                config,
                EndUse::LIGHTING,
                "Indoor Lighting",
            ))
        }),
    );
    registry.register(
        "Exterior Lighting",
        Box::new(|config| {
            Box::new(ScheduledLoad::new(
                config,
                EndUse::LIGHTING,
                "Exterior Lighting",
            ))
        }),
    );
    registry.register(
        "Basement Lighting",
        Box::new(|config| {
            Box::new(ScheduledLoad::new(
                config,
                EndUse::LIGHTING,
                "Basement Lighting",
            ))
        }),
    );
    registry.register(
        "Garage Lighting",
        Box::new(|config| {
            Box::new(ScheduledLoad::new(
                config,
                EndUse::LIGHTING,
                "Garage Lighting",
            ))
        }),
    );
}

fn default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(4);
    telemetry.insert("electric_kw", 0.0);
    telemetry.insert("sensible_gain_w", 0.0);
    telemetry.insert("latent_gain_w", 0.0);
    telemetry.insert("fuel_input_w", 0.0);
    telemetry
}

fn scheduled_load_telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: "electric_kw".to_string(),
            unit: "kW".to_string(),
            description: "ZIP-adjusted active electrical power draw".to_string(),
        },
        TelemetryField {
            name: "sensible_gain_w".to_string(),
            unit: "W".to_string(),
            description: "Sensible thermal gain to assigned zone".to_string(),
        },
        TelemetryField {
            name: "latent_gain_w".to_string(),
            unit: "W".to_string(),
            description: "Latent thermal gain to assigned zone".to_string(),
        },
        TelemetryField {
            name: "fuel_input_w".to_string(),
            unit: "W".to_string(),
            description: "Gas fuel consumption rate converted to watts".to_string(),
        },
    ]
}

fn parse_zip_coefficients(config: &EquipmentConfig) -> crate::Result<ZipCoefficients> {
    let z = config.get_f64(KEY_ZIP_Z);
    let i = config.get_f64(KEY_ZIP_I);
    let p = config.get_f64(KEY_ZIP_P);
    // v0 defaults to 1.0 pu (no normalisation); override to calibrate ZIP at a
    // non-nominal voltage, e.g. 0.95 pu for ANSI Range A lower boundary.
    let v0 = config.get_f64(KEY_ZIP_V0).unwrap_or(1.0);

    let zq = config.get_f64(KEY_ZIP_ZQ);
    let iq = config.get_f64(KEY_ZIP_IQ);
    let pq = config.get_f64(KEY_ZIP_PQ);
    let pf = config.get_f64(KEY_ZIP_PF);

    let zip = if z.is_none() && i.is_none() && p.is_none() {
        ZipCoefficients {
            v0,
            zq: zq.unwrap_or(0.0),
            iq: iq.unwrap_or(0.0),
            pq: pq.unwrap_or(1.0),
            pf: pf.unwrap_or(0.0),
            ..ZipCoefficients::default()
        }
    } else {
        ZipCoefficients {
            z: z.unwrap_or(0.0),
            i: i.unwrap_or(0.0),
            p_coeff: p.unwrap_or(0.0),
            v0,
            zq: zq.unwrap_or(0.0),
            iq: iq.unwrap_or(0.0),
            pq: pq.unwrap_or(1.0),
            pf: pf.unwrap_or(0.0),
        }
    };
    let sum = zip.z + zip.i + zip.p_coeff;
    if (sum - ZIP_SUM_TARGET).abs() > ZIP_SUM_TOLERANCE {
        return Err(HaresError::Equipment(format!(
            "invalid ZIP coefficients: z + i + p = {sum}, expected {ZIP_SUM_TARGET}"
        )));
    }
    // Validate reactive sum only when any reactive coefficient is explicitly set.
    if zq.is_some() || iq.is_some() || pq.is_some() {
        let reactive_sum = zip.zq + zip.iq + zip.pq;
        if (reactive_sum - ZIP_SUM_TARGET).abs() > ZIP_SUM_TOLERANCE {
            return Err(HaresError::Equipment(format!(
                "invalid reactive ZIP coefficients: zq + iq + pq = {reactive_sum}, expected {ZIP_SUM_TARGET}"
            )));
        }
    }
    Ok(zip)
}

fn parse_power_schedule_source(config: &EquipmentConfig) -> crate::Result<ScheduleSource> {
    let source = config
        .get_str(KEY_POWER_SCHEDULE_SOURCE)
        .ok_or_else(|| {
            HaresError::Equipment(format!(
                "missing required key `{KEY_POWER_SCHEDULE_SOURCE}`"
            ))
        })?
        .to_ascii_lowercase();

    match source.as_str() {
        "column" => {
            let Some(col_idx) = parse_usize(&config.raw_config, KEY_POWER_SCHEDULE_COL)? else {
                return Err(HaresError::Equipment(format!(
                    "missing required key `{KEY_POWER_SCHEDULE_COL}` for column power schedule"
                )));
            };
            Ok(ScheduleSource::ColumnRef {
                col_idx,
                boundary: BoundaryPolicy::Error,
            })
        }
        "daily_profile" => {
            let max_value = config.get_f64(KEY_POWER_PROFILE_MAX_KW).ok_or_else(|| {
                HaresError::Equipment(format!(
                    "missing required key `{KEY_POWER_PROFILE_MAX_KW}` for daily_profile power schedule"
                ))
            })?;
            let weekday = config
                .get_f64_array(KEY_POWER_PROFILE_WEEKDAY)
                .map(slice_to_24)
                .ok_or_else(|| {
                    HaresError::Equipment(format!(
                        "missing required key `{KEY_POWER_PROFILE_WEEKDAY}` for daily_profile power schedule"
                    ))
                })?;
            let weekend = config
                .get_f64_array(KEY_POWER_PROFILE_WEEKEND)
                .map(slice_to_24)
                .ok_or_else(|| {
                    HaresError::Equipment(format!(
                        "missing required key `{KEY_POWER_PROFILE_WEEKEND}` for daily_profile power schedule"
                    ))
                })?;
            let month_multipliers = config
                .get_f64_array(KEY_POWER_PROFILE_MONTH)
                .map(slice_to_12)
                .ok_or_else(|| {
                    HaresError::Equipment(format!(
                        "missing required key `{KEY_POWER_PROFILE_MONTH}` for daily_profile power schedule"
                    ))
                })?;
            Ok(ScheduleSource::DailyProfile {
                weekday,
                weekend,
                month_multipliers,
                max_value,
            })
        }
        "constant" => {
            let Some(v) = config.get_f64(KEY_POWER_CONSTANT_KW) else {
                return Err(HaresError::Equipment(format!(
                    "missing required key `{KEY_POWER_CONSTANT_KW}` for constant power schedule"
                )));
            };
            Ok(ScheduleSource::Constant(v))
        }
        _ => Err(HaresError::Equipment(format!(
            "unsupported {KEY_POWER_SCHEDULE_SOURCE} value `{source}` (expected `column`, `daily_profile`, or `constant`)"
        ))),
    }
}

fn parse_optional_gas_schedule_source(
    config: &EquipmentConfig,
) -> crate::Result<(Option<ScheduleSource>, GasScheduleUnit)> {
    let Some(source) = config.get_str(KEY_GAS_SCHEDULE_SOURCE) else {
        return Ok((None, GasScheduleUnit::ThermsPerHour));
    };
    let source = source.to_ascii_lowercase();
    let gas_source = match source.as_str() {
        "column" => {
            let Some(col_idx) = parse_usize(&config.raw_config, KEY_GAS_SCHEDULE_COL)? else {
                return Err(HaresError::Equipment(format!(
                    "missing required key `{KEY_GAS_SCHEDULE_COL}` for column gas schedule"
                )));
            };
            ScheduleSource::ColumnRef {
                col_idx,
                boundary: BoundaryPolicy::Error,
            }
        }
        "daily_profile" => {
            let max_value = config.get_f64(KEY_GAS_PROFILE_MAX).ok_or_else(|| {
                HaresError::Equipment(format!(
                    "missing required key `{KEY_GAS_PROFILE_MAX}` for daily_profile gas schedule"
                ))
            })?;
            let weekday = config
                .get_f64_array(KEY_GAS_PROFILE_WEEKDAY)
                .map(slice_to_24)
                .ok_or_else(|| {
                    HaresError::Equipment(format!(
                        "missing required key `{KEY_GAS_PROFILE_WEEKDAY}` for daily_profile gas schedule"
                    ))
                })?;
            let weekend = config
                .get_f64_array(KEY_GAS_PROFILE_WEEKEND)
                .map(slice_to_24)
                .ok_or_else(|| {
                    HaresError::Equipment(format!(
                        "missing required key `{KEY_GAS_PROFILE_WEEKEND}` for daily_profile gas schedule"
                    ))
                })?;
            let month_multipliers = config
                .get_f64_array(KEY_GAS_PROFILE_MONTH)
                .map(slice_to_12)
                .ok_or_else(|| {
                    HaresError::Equipment(format!(
                        "missing required key `{KEY_GAS_PROFILE_MONTH}` for daily_profile gas schedule"
                    ))
                })?;
            ScheduleSource::DailyProfile {
                weekday,
                weekend,
                month_multipliers,
                max_value,
            }
        }
        "constant" => {
            let Some(v) = config.get_f64(KEY_GAS_CONSTANT) else {
                return Err(HaresError::Equipment(format!(
                    "missing required key `{KEY_GAS_CONSTANT}` for constant gas schedule"
                )));
            };
            ScheduleSource::Constant(v)
        }
        _ => {
            return Err(HaresError::Equipment(format!(
                "unsupported {KEY_GAS_SCHEDULE_SOURCE} value `{source}`"
            )));
        }
    };
    Ok((Some(gas_source), GasScheduleUnit::ThermsPerHour))
}

fn scale_schedule_source(source: &mut ScheduleSource, scale: f64) {
    match source {
        ScheduleSource::Constant(v) => *v *= scale,
        ScheduleSource::DailyProfile { max_value, .. } => *max_value *= scale,
        ScheduleSource::Stochastic { kind, .. } => match kind {
            hares_types::DistributionKind::Gaussian { mean, std_dev } => {
                *mean *= scale;
                *std_dev *= scale.abs();
            }
            hares_types::DistributionKind::Uniform { low, high } => {
                *low *= scale;
                *high *= scale;
                // Ensure low ≤ high after scaling (negative scale inverts).
                if *low > *high {
                    std::mem::swap(low, high);
                }
            }
            hares_types::DistributionKind::Exponential { lambda } => {
                // E[Exp(λ)] = 1/λ; scaling output by `scale` ⟹ λ′ = λ/scale.
                *lambda /= scale;
                *lambda = lambda.abs(); // Guard against negative scale.
            }
            hares_types::DistributionKind::Poisson { lambda } => {
                *lambda *= scale;
                *lambda = lambda.abs(); // Guard against negative scale.
            }
            // LogNormal: scaling mu/sigma doesn't scale output linearly.
            // Bernoulli: scaling a probability is not meaningful.
            hares_types::DistributionKind::LogNormal { .. }
            | hares_types::DistributionKind::Bernoulli { .. } => {
                tracing::warn!(
                    scale,
                    "usage_multiplier ignored for LogNormal/Bernoulli distribution"
                );
            }
        },
        ScheduleSource::Shared { data, .. } => {
            let scaled: Vec<f64> = data.iter().map(|v| v * scale).collect();
            *data = Arc::from(scaled);
        }
        ScheduleSource::ColumnRef { .. } | ScheduleSource::SolarAware { .. } => {}
        // Wildcard required: ScheduleSource is #[non_exhaustive].
        // New variants must be handled explicitly here.
        _ => {}
    }
}

fn is_schedule_source_zero(source: &ScheduleSource) -> bool {
    match source {
        ScheduleSource::Constant(v) => *v == 0.0,
        ScheduleSource::DailyProfile { max_value, .. } => *max_value == 0.0,
        ScheduleSource::Shared { data, .. } => data.iter().all(|v| *v == 0.0),
        ScheduleSource::ColumnRef { .. }
        | ScheduleSource::SolarAware { .. }
        | ScheduleSource::Stochastic { .. } => false,
        // Wildcard required: ScheduleSource is #[non_exhaustive].
        // New variants must be handled explicitly here.
        _ => false,
    }
}

/// Parse per-month scale factors from config keys `month_multiplier_0` through
/// `month_multiplier_11`. Returns `None` when no multiplier keys are present.
fn parse_month_multipliers(config: &EquipmentConfig) -> Option<[f64; 12]> {
    let mut found_any = false;
    let mut multipliers = [1.0_f64; 12];
    for (month, slot) in multipliers.iter_mut().enumerate() {
        let key = format!("{KEY_MONTH_MULTIPLIER_PREFIX}{month}");
        if let Some(val) = config.get_f64(&key) {
            *slot = val.max(0.0);
            found_any = true;
        }
    }
    found_any.then_some(multipliers)
}

fn parse_bool(config: &EquipmentConfig, key: &str) -> crate::Result<Option<bool>> {
    if let Some(b) = config.get_bool(key) {
        return Ok(Some(b));
    }
    let value = match config.get_f64(key) {
        Some(value) => value,
        None => return Ok(None),
    };
    if value == 0.0 {
        return Ok(Some(false));
    }
    if value == 1.0 {
        return Ok(Some(true));
    }
    Err(HaresError::Equipment(format!(
        "invalid boolean for key {key}: expected 0.0 or 1.0, got {value}"
    )))
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc, time::Duration};

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        BoundaryPolicy, ControlSignal, DomainUpdate, EnvironmentState, FuelType, GridState,
        PortSlots, ScheduleSource, TelemetryField, WeatherState, ZoneId, ZoneState,
        schedule_domain_id,
    };
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    use super::{
        GAS_THERMS_PER_HOUR_TO_W, KEY_CONVECTIVE_GAIN_FRACTION, KEY_GAS_CONSTANT,
        KEY_GAS_SCHEDULE_IS_W, KEY_GAS_SCHEDULE_SOURCE, KEY_LATENT_GAIN_FRACTION,
        KEY_MONTH_MULTIPLIER_PREFIX, KEY_POWER_CONSTANT_KW, KEY_POWER_SCHEDULE_COL,
        KEY_POWER_SCHEDULE_SOURCE, KEY_RADIATIVE_GAIN_FRACTION, KEY_SENSIBLE_GAIN_FRACTION,
        KEY_ZIP_I, KEY_ZIP_P, KEY_ZIP_V0, KEY_ZIP_Z, ScheduledLoad, register_with_registry,
    };
    use crate::{Equipment, EquipmentConfig, EquipmentRegistry};

    fn base_env() -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 21.0,
                humidity_ratio: 0.008,
                relative_humidity: 0.5,
                wet_bulb_c: 15.0,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 10.0,
                outdoor_humidity_ratio: 0.005,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 12.0,
                sky_temp_c: 7.0,
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
                .with_ymd_and_hms(2026, 3, 18, 0, 0, 0)
                .single()
                .expect("valid UTC timestamp"),
            time_res: ChronoDuration::minutes(15),
        }
    }

    fn config_with_schedule(name: &str, ochre_class: &str, schedule: &[f64]) -> EquipmentConfig {
        let mut raw_config: HashMap<String, crate::config::ConfigValue> = HashMap::new();
        raw_config.insert("zone_id".to_string(), 1.0.into());
        raw_config.insert(KEY_POWER_SCHEDULE_SOURCE.to_string(), "constant".into());
        raw_config.insert(
            KEY_POWER_CONSTANT_KW.to_string(),
            schedule.first().copied().unwrap_or(0.0).into(),
        );
        EquipmentConfig {
            name: name.to_string(),
            ochre_class: ochre_class.to_string(),
            raw_config,
        }
    }

    #[test]
    fn init_rejects_invalid_zip_sum() {
        let mut config = config_with_schedule("s", "Lighting", &[1.0]);
        config.raw_config.insert(KEY_ZIP_Z.to_string(), 0.2.into());
        config.raw_config.insert(KEY_ZIP_I.to_string(), 0.2.into());
        config.raw_config.insert(KEY_ZIP_P.to_string(), 0.2.into());
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let err = eq.init(&config, &base_env()).unwrap_err();
        assert!(err.to_string().contains("invalid ZIP coefficients"));
    }

    #[test]
    fn zip_voltage_uses_env_grid_voltage() {
        let mut config = config_with_schedule("s", "Lighting", &[2.0]);
        config.raw_config.insert(KEY_ZIP_Z.to_string(), 0.2.into());
        config.raw_config.insert(KEY_ZIP_I.to_string(), 0.3.into());
        config.raw_config.insert(KEY_ZIP_P.to_string(), 0.5.into());
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let mut env = base_env();
        env.grid.voltage_pu = 0.95;
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        let expected_multiplier = 0.2 * 0.95 * 0.95 + 0.3 * 0.95 + 0.5;
        let expected_kw = 2.0 * expected_multiplier;
        assert!((ports.electrical.net_active_kw() - expected_kw).abs() < 1e-12);
    }

    #[test]
    fn thermal_gains_follow_config_fractions() {
        let mut config = config_with_schedule("s", "Lighting", &[1.0]);
        config
            .raw_config
            .insert(KEY_SENSIBLE_GAIN_FRACTION.to_string(), 0.5.into());
        config
            .raw_config
            .insert(KEY_LATENT_GAIN_FRACTION.to_string(), 0.2.into());
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let env = base_env();
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert!((ports.thermal[0].sensible_gain_w - 500.0).abs() < 1e-12);
        assert!((ports.thermal[0].latent_gain_w - 200.0).abs() < 1e-12);
    }

    #[test]
    fn missing_sensible_gain_fraction_uses_conservative_half() {
        let config = config_with_schedule("s", "Lighting", &[1.0]);
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let env = base_env();
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert!((ports.thermal[0].sensible_gain_w - 500.0).abs() < 1e-12);
    }

    #[test]
    fn lighting_physics_uses_point_seven_sensible_fraction() {
        let mut config = config_with_schedule("s", "Indoor Lighting", &[1.0]);
        config
            .raw_config
            .insert(KEY_SENSIBLE_GAIN_FRACTION.to_string(), 0.70.into());
        let mut eq = ScheduledLoad::new(
            config.clone(),
            hares_types::EndUse::LIGHTING,
            "Indoor Lighting",
        );
        let env = base_env();
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert!((ports.thermal[0].sensible_gain_w - 700.0).abs() < 1e-12);
    }

    #[test]
    fn zero_or_negative_schedule_value_writes_no_power() {
        let config = config_with_schedule("s", "Lighting", &[0.0, -2.0]);
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let mut env = base_env();
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert_eq!(ports.electrical.net_active_kw(), 0.0);
        assert_eq!(ports.thermal[0].sensible_gain_w, 0.0);

        env.current_time += ChronoDuration::minutes(15);
        ports.zero();
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert_eq!(ports.electrical.net_active_kw(), 0.0);
    }

    #[test]
    fn power_setpoint_overrides_schedule_for_one_step() {
        // Schedule produces 3.0 kW; PowerSetpoint should override it with 5.0 kW for one step,
        // then revert to the schedule on the next step.
        let config = config_with_schedule("s", "Lighting", &[3.0, 3.0]);
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let mut env = base_env();
        eq.init(&config, &env).unwrap();
        eq.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: 5.0,
            reactive_power_kvar: None,
        })
        .unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert!(
            (ports.electrical.net_active_kw() - 5.0).abs() < 1e-12,
            "step 1 should use setpoint 5.0"
        );

        // No new PowerSetpoint issued — next step reverts to schedule value.
        env.current_time += ChronoDuration::minutes(15);
        ports.zero();
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert!(
            (ports.electrical.net_active_kw() - 3.0).abs() < 1e-12,
            "step 2 should revert to schedule 3.0"
        );
    }

    #[test]
    fn unsupported_control_signal_is_rejected() {
        let config = config_with_schedule("s", "Lighting", &[1.0]);
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        eq.init(&config, &base_env()).unwrap();
        let err = eq
            .apply_control(&ControlSignal::SOCTarget {
                target_soc: 0.8,
                min_soc: None,
                max_soc: None,
            })
            .unwrap_err();
        assert!(err.to_string().contains("unsupported control signal"));
    }

    #[test]
    fn load_fraction_scales_electric_output() {
        let config = config_with_schedule("s", "Lighting", &[2.0]);
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let env = base_env();
        eq.init(&config, &env).unwrap();
        eq.apply_control(&ControlSignal::LoadFraction { fraction: 0.5 })
            .unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert!((ports.electrical.net_active_kw() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn mode_override_off_forces_zero_power() {
        let config = config_with_schedule("s", "Lighting", &[5.0]);
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let env = base_env();
        eq.init(&config, &env).unwrap();
        eq.apply_control(&ControlSignal::ModeOverride {
            mode: hares_types::OperatingMode::Off,
        })
        .unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert_eq!(ports.electrical.net_active_kw(), 0.0);
    }

    #[test]
    fn voltage_outage_forces_zero_output() {
        let config = config_with_schedule("s", "Lighting", &[3.0]);
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let mut env = base_env();
        eq.init(&config, &env).unwrap();
        env.grid.voltage_pu = 0.0;

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert_eq!(ports.electrical.net_active_kw(), 0.0);
        assert_eq!(ports.thermal[0].sensible_gain_w, 0.0);
    }

    #[test]
    fn load_fraction_state_round_trip() {
        let config = config_with_schedule("s", "Lighting", &[2.0]);
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let env = base_env();
        eq.init(&config, &env).unwrap();
        eq.apply_control(&ControlSignal::LoadFraction { fraction: 0.7 })
            .unwrap();
        let state = eq.save_state();

        let mut restored =
            ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        restored.init(&config, &env).unwrap();
        restored.load_state(&state).unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        restored
            .step(&env, Duration::from_secs(900), &mut ports)
            .unwrap();
        assert!((ports.electrical.net_active_kw() - 1.4).abs() < 1e-12);
    }

    #[test]
    fn gas_schedule_writes_fuel_contribution() {
        let mut config = config_with_schedule("s", "Lighting", &[1.0]);
        config
            .raw_config
            .insert(KEY_GAS_SCHEDULE_SOURCE.to_string(), "constant".into());
        config
            .raw_config
            .insert(KEY_GAS_CONSTANT.to_string(), 0.1.into());
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let env = base_env();
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert_eq!(
            ports.fuel.get(FuelType::Gas),
            0.1 * GAS_THERMS_PER_HOUR_TO_W
        );
        assert!(ports.electrical.net_active_kw() > 0.0);
    }

    #[test]
    fn gas_schedule_in_watts_is_supported() {
        let mut config = config_with_schedule("s", "Lighting", &[1.0]);
        config
            .raw_config
            .insert(KEY_GAS_SCHEDULE_SOURCE.to_string(), "constant".into());
        config
            .raw_config
            .insert(KEY_GAS_CONSTANT.to_string(), 1000.0.into());
        config
            .raw_config
            .insert(KEY_GAS_SCHEDULE_IS_W.to_string(), 1.0.into());
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let env = base_env();
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert_eq!(ports.fuel.get(FuelType::Gas), 1000.0);
    }

    #[test]
    fn stepping_past_schedule_end_returns_error() {
        let mut config = config_with_schedule("s", "Lighting", &[1.0]);
        config
            .raw_config
            .insert(KEY_SENSIBLE_GAIN_FRACTION.to_string(), 0.0.into());
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let mut env = base_env();
        eq.init(&config, &env).unwrap();
        eq.power_source = ScheduleSource::Shared {
            data: Arc::from(vec![1.0]),
            cursor: 0,
            boundary: BoundaryPolicy::Error,
        };
        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        env.current_time += ChronoDuration::minutes(15);
        ports.zero();
        let err = eq
            .step(&env, Duration::from_secs(900), &mut ports)
            .unwrap_err();
        assert!(err.to_string().contains("out of bounds"));
    }

    #[test]
    fn state_round_trip_preserves_source_state_and_last_values() {
        let config = config_with_schedule("s", "Lighting", &[1.0]);
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let mut env = base_env();
        eq.init(&config, &env).unwrap();
        eq.power_source = ScheduleSource::Shared {
            data: Arc::from(vec![1.0, 2.0]),
            cursor: 0,
            boundary: BoundaryPolicy::Error,
        };
        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        env.current_time += ChronoDuration::minutes(15);
        ports.zero();
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        let state = eq.save_state();

        let mut restored =
            ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        restored.init(&config, &base_env()).unwrap();
        restored.power_source = ScheduleSource::Shared {
            data: Arc::from(vec![1.0, 2.0]),
            cursor: 0,
            boundary: BoundaryPolicy::Error,
        };
        restored.load_state(&state).unwrap();
        let telemetry = restored.telemetry();
        assert_eq!(telemetry.get("electric_kw"), Some(2.0));
    }

    #[test]
    fn column_ref_reads_from_schedule_custom_domain() {
        let mut config = config_with_schedule("s", "Lighting", &[0.0]);
        config
            .raw_config
            .insert(KEY_POWER_SCHEDULE_SOURCE.to_string(), "column".into());
        config
            .raw_config
            .insert(KEY_POWER_SCHEDULE_COL.to_string(), 1.0.into());
        config
            .raw_config
            .insert(KEY_SENSIBLE_GAIN_FRACTION.to_string(), 0.0.into());

        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let mut env = base_env();
        env.custom_domains.push(DomainUpdate {
            domain_id: schedule_domain_id(),
            zone_temperatures_c: vec![],
            custom_payload: Some(vec![2.5, 7.25]),
        });
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert!((ports.electrical.net_active_kw() - 7.25).abs() < 1e-12);
    }

    #[test]
    fn stochastic_source_is_deterministic_across_save_restore() {
        let mut config = config_with_schedule("s", "Lighting", &[0.0]);
        config
            .raw_config
            .insert(KEY_SENSIBLE_GAIN_FRACTION.to_string(), 0.0.into());
        let mut env = base_env();
        let mut a = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        a.init(&config, &env).unwrap();
        a.power_source = ScheduleSource::Stochastic {
            kind: hares_types::DistributionKind::Gaussian {
                mean: 1.0,
                std_dev: 0.2,
            },
            seed: [9_u8; 32],
            draw_count: 0,
            rng: ChaCha8Rng::from_seed([9_u8; 32]),
            clamp_min: None,
            clamp_max: None,
        };

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        a.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        env.current_time += ChronoDuration::minutes(15);
        ports.zero();
        a.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        let checkpoint = a.save_state();

        let mut b = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        b.init(&config, &env).unwrap();
        b.power_source = ScheduleSource::Stochastic {
            kind: hares_types::DistributionKind::Gaussian {
                mean: 1.0,
                std_dev: 0.2,
            },
            seed: [9_u8; 32],
            draw_count: 0,
            rng: ChaCha8Rng::from_seed([9_u8; 32]),
            clamp_min: None,
            clamp_max: None,
        };
        b.load_state(&checkpoint).unwrap();

        let mut ports_a = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let mut ports_b = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        for _ in 0..4 {
            env.current_time += ChronoDuration::minutes(15);
            ports_a.zero();
            ports_b.zero();
            a.step(&env, Duration::from_secs(900), &mut ports_a)
                .unwrap();
            b.step(&env, Duration::from_secs(900), &mut ports_b)
                .unwrap();
            assert!(
                (ports_a.electrical.net_active_kw() - ports_b.electrical.net_active_kw()).abs()
                    < 1e-12
            );
        }
    }

    #[test]
    fn descriptor_stage_capabilities_and_telemetry_fields_match_contract() {
        let config = config_with_schedule("s", "Lighting", &[1.0]);
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        eq.init(&config, &base_env()).unwrap();
        assert_eq!(
            eq.descriptor().stage,
            hares_types::ExecutionStage::Independent
        );
        assert!(
            eq.descriptor()
                .control_capabilities
                .contains(hares_types::ControlCapabilities::LOAD_FRACTION)
        );
        assert!(
            eq.descriptor()
                .control_capabilities
                .contains(hares_types::ControlCapabilities::MODE_OVERRIDE)
        );
        assert!(
            eq.descriptor()
                .control_capabilities
                .contains(hares_types::ControlCapabilities::POWER_SETPOINT)
        );
        let names: Vec<&str> = eq
            .descriptor()
            .telemetry_fields
            .iter()
            .map(TelemetryField::name_as_str)
            .collect();
        assert!(names.contains(&"electric_kw"));
        assert!(names.contains(&"sensible_gain_w"));
        assert!(names.contains(&"latent_gain_w"));
        assert!(names.contains(&"fuel_input_w"));
    }

    #[test]
    fn registry_includes_scheduled_load_aliases() {
        let mut registry = EquipmentRegistry::new();
        register_with_registry(&mut registry);
        assert!(registry.get("Lighting").is_some());
        assert!(registry.get("Plug Loads").is_some());
        assert!(registry.get("Other").is_some());
    }

    #[test]
    fn load_fraction_scales_gas_output() {
        let mut config = config_with_schedule("s", "Gas Grill", &[1.0]);
        config
            .raw_config
            .insert(KEY_GAS_SCHEDULE_SOURCE.to_string(), "constant".into());
        config
            .raw_config
            .insert(KEY_GAS_CONSTANT.to_string(), 0.2.into());
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::OTHER, "Gas Grill");
        let env = base_env();
        eq.init(&config, &env).unwrap();
        eq.apply_control(&ControlSignal::LoadFraction { fraction: 0.5 })
            .unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        let expected_gas_w = 0.2 * 0.5 * GAS_THERMS_PER_HOUR_TO_W;
        assert!(
            (ports.fuel.get(FuelType::Gas) - expected_gas_w).abs() < 1e-6,
            "gas_w={} expected={expected_gas_w}",
            ports.fuel.get(FuelType::Gas)
        );
    }

    #[test]
    fn month_multiplier_zeroes_schedule_in_target_month() {
        let mut config = config_with_schedule("Ceiling Fan", "Ceiling Fan", &[2.0]);
        // month_multiplier_0 = January (0-based) → zero all output in January
        config
            .raw_config
            .insert(format!("{KEY_MONTH_MULTIPLIER_PREFIX}0"), 0.0.into());
        let mut eq = ScheduledLoad::new(
            config.clone(),
            hares_types::EndUse::VENTILATION,
            "Ceiling Fan",
        );
        let mut env = base_env();
        // Step in January
        env.current_time = FixedOffset::east_opt(0)
            .expect("UTC offset")
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .expect("valid UTC timestamp");
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert_eq!(
            ports.electrical.net_active_kw(),
            0.0,
            "January output should be zero"
        );

        // Confirm non-January still produces output
        let mut env_march = base_env();
        env_march.current_time = FixedOffset::east_opt(0)
            .expect("UTC offset")
            .with_ymd_and_hms(2026, 3, 1, 0, 0, 0)
            .single()
            .expect("valid UTC timestamp");
        eq.init(&config, &env_march).unwrap();
        ports.zero();
        eq.step(&env_march, Duration::from_secs(900), &mut ports)
            .unwrap();
        assert!(
            ports.electrical.net_active_kw() > 0.0,
            "March output should be non-zero"
        );
    }

    #[test]
    fn convective_and_radiative_fractions_sum_to_sensible() {
        let mut config = config_with_schedule("s", "Lighting", &[1.0]);
        config
            .raw_config
            .insert(KEY_CONVECTIVE_GAIN_FRACTION.to_string(), 0.3.into());
        config
            .raw_config
            .insert(KEY_RADIATIVE_GAIN_FRACTION.to_string(), 0.2.into());
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let env = base_env();
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        // 1 kW = 1000 W; sensible fraction = 0.3 + 0.2 = 0.5
        assert!((ports.thermal[0].sensible_gain_w - 500.0).abs() < 1e-9);
    }

    #[test]
    fn sensible_plus_latent_exceeding_one_is_rejected() {
        let mut config = config_with_schedule("s", "Lighting", &[1.0]);
        config
            .raw_config
            .insert(KEY_SENSIBLE_GAIN_FRACTION.to_string(), 0.7.into());
        config
            .raw_config
            .insert(KEY_LATENT_GAIN_FRACTION.to_string(), 0.5.into());
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let err = eq.init(&config, &base_env()).unwrap_err();
        assert!(
            err.to_string().contains("must not exceed 1.0"),
            "expected validation error, got: {err}"
        );
    }

    #[test]
    fn zip_v0_normalizes_voltage() {
        // With v0=1.0, v=0.95: v_norm=0.95. With v0=0.95, v=0.95: v_norm=1.0 → pure P load.
        let mut config = config_with_schedule("s", "Lighting", &[2.0]);
        config.raw_config.insert(KEY_ZIP_Z.to_string(), 0.2.into());
        config.raw_config.insert(KEY_ZIP_I.to_string(), 0.3.into());
        config.raw_config.insert(KEY_ZIP_P.to_string(), 0.5.into());
        config
            .raw_config
            .insert(KEY_ZIP_V0.to_string(), 0.95.into());
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let mut env = base_env();
        env.grid.voltage_pu = 0.95;
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        // v_norm = 0.95 / 0.95 = 1.0; zip_multiplier = 0.2*1 + 0.3*1 + 0.5 = 1.0
        assert!((ports.electrical.net_active_kw() - 2.0).abs() < 1e-12);
    }

    #[test]
    fn exterior_equipment_has_no_zone_assignment() {
        let config = config_with_schedule("Exterior Lighting", "Exterior Lighting", &[1.0]);
        let eq = ScheduledLoad::new(
            config.clone(),
            hares_types::EndUse::LIGHTING,
            "Exterior Lighting",
        );
        assert!(
            eq.descriptor().zone.is_none(),
            "Exterior equipment should have no zone"
        );
    }

    /// GAS_THERMS_PER_HOUR_TO_W must be ~29,307 W/therm/hr.
    /// 1 US therm = 100,000 BTU_IT; 1 BTU_IT = 1055.05585262 J.
    /// 100_000 * 1055.05585262 / 3600 = 29_307.107... W.
    #[test]
    fn gas_therms_per_hour_to_w_matches_nist_definition() {
        let btu_it_joules = 1_055.055_852_62;
        let expected = 100_000.0 * btu_it_joules / 3600.0;
        assert!(
            (GAS_THERMS_PER_HOUR_TO_W - expected).abs() < 0.01,
            "GAS_THERMS_PER_HOUR_TO_W={GAS_THERMS_PER_HOUR_TO_W} should be ~{expected}"
        );
    }

    trait TelemetryFieldExt {
        fn name_as_str(&self) -> &str;
    }

    impl TelemetryFieldExt for TelemetryField {
        fn name_as_str(&self) -> &str {
            &self.name
        }
    }

    #[test]
    fn reactive_zip_coefficients_produce_correct_kvar() {
        // zq=0, iq=0, pq=1 → reactive_base = 1.0 at any voltage.
        // reactive_kvar = real_kw * pf * 1.0.
        let mut config = config_with_schedule("s", "Lighting", &[2.0]);
        config
            .raw_config
            .insert(super::KEY_ZIP_ZQ.to_string(), 0.0.into());
        config
            .raw_config
            .insert(super::KEY_ZIP_IQ.to_string(), 0.0.into());
        config
            .raw_config
            .insert(super::KEY_ZIP_PQ.to_string(), 1.0.into());
        config
            .raw_config
            .insert(super::KEY_ZIP_PF.to_string(), 0.8.into());
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let env = base_env();
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        // real_kw = 2.0 (all-P load at nominal voltage); reactive = 2.0 * 0.8 * 1.0 = 1.6
        assert!((ports.electrical.reactive_power_kvar - 1.6).abs() < 1e-12);
    }

    #[test]
    fn reactive_zip_default_produces_zero_kvar() {
        let mut config = config_with_schedule("s", "Lighting", &[3.0]);
        config
            .raw_config
            .insert(KEY_SENSIBLE_GAIN_FRACTION.to_string(), 0.0.into());
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let env = base_env();
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots::default();
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert_eq!(ports.electrical.reactive_power_kvar, 0.0);
    }

    #[test]
    fn reactive_zip_voltage_sensitivity() {
        // zq=1, iq=0, pq=0 → reactive_base = v_norm².
        // At v=0.9, v_norm=0.9; reactive = real_kw * 0.9 * 0.81.
        let mut config = config_with_schedule("s", "Lighting", &[1.0]);
        config
            .raw_config
            .insert(KEY_SENSIBLE_GAIN_FRACTION.to_string(), 0.0.into());
        config
            .raw_config
            .insert(super::KEY_ZIP_ZQ.to_string(), 1.0.into());
        config
            .raw_config
            .insert(super::KEY_ZIP_IQ.to_string(), 0.0.into());
        config
            .raw_config
            .insert(super::KEY_ZIP_PQ.to_string(), 0.0.into());
        config
            .raw_config
            .insert(super::KEY_ZIP_PF.to_string(), 0.9.into());
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let mut env = base_env();
        env.grid.voltage_pu = 0.9;
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots::default();
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        let v_norm = 0.9_f64;
        // real_kw = 1.0 (pure-P real ZIP); reactive = 1.0 * 0.9 * v_norm²
        let expected_kvar = 0.9 * v_norm * v_norm;
        assert!(
            (ports.electrical.reactive_power_kvar - expected_kvar).abs() < 1e-12,
            "reactive={} expected={expected_kvar}",
            ports.electrical.reactive_power_kvar
        );
    }

    #[test]
    fn reactive_zip_invalid_sum_is_rejected() {
        let mut config = config_with_schedule("s", "Lighting", &[1.0]);
        config
            .raw_config
            .insert(super::KEY_ZIP_ZQ.to_string(), 0.3.into());
        config
            .raw_config
            .insert(super::KEY_ZIP_IQ.to_string(), 0.3.into());
        config
            .raw_config
            .insert(super::KEY_ZIP_PQ.to_string(), 0.3.into());
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let err = eq.init(&config, &base_env()).unwrap_err();
        assert!(
            err.to_string().contains("reactive ZIP coefficients"),
            "expected reactive ZIP validation error, got: {err}"
        );
    }

    #[test]
    fn garage_name_auto_routes_to_garage_zone() {
        let mut config = config_with_schedule("Garage Lighting", "Garage Lighting", &[1.0]);
        config.raw_config.remove("zone_id");
        let eq = ScheduledLoad::new(config, hares_types::EndUse::LIGHTING, "Garage Lighting");
        assert_eq!(
            eq.descriptor().zone,
            Some(ZoneId(2)),
            "Garage equipment should auto-route to ZoneId(2)"
        );
    }

    #[test]
    fn basement_name_auto_routes_to_foundation_zone() {
        let mut config = config_with_schedule("Basement Lighting", "Basement Lighting", &[1.0]);
        config.raw_config.remove("zone_id");
        let eq = ScheduledLoad::new(config, hares_types::EndUse::LIGHTING, "Basement Lighting");
        assert_eq!(
            eq.descriptor().zone,
            Some(ZoneId(3)),
            "Basement equipment should auto-route to ZoneId(3)"
        );
    }

    #[test]
    fn explicit_zone_id_overrides_auto_routing() {
        let mut config = config_with_schedule("Garage Lighting", "Garage Lighting", &[1.0]);
        config.raw_config.insert("zone_id".to_string(), 1.0.into());
        let eq = ScheduledLoad::new(config, hares_types::EndUse::LIGHTING, "Garage Lighting");
        assert_eq!(
            eq.descriptor().zone,
            Some(ZoneId(1)),
            "Explicit zone_id should take precedence over name-based auto-routing"
        );
    }

    #[test]
    fn outdoor_name_suppresses_auto_routing() {
        let mut config = config_with_schedule("Outdoor Garage Fan", "Other", &[1.0]);
        config.raw_config.remove("zone_id");
        let eq = ScheduledLoad::new(config, hares_types::EndUse::OTHER, "Other");
        assert!(
            eq.descriptor().zone.is_none(),
            "Outdoor prefix takes precedence and suppresses garage auto-routing"
        );
    }
}
