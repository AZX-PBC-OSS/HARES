//! Scheduled (deterministic) load equipment model.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use chrono::Datelike;
use hares_physics::constants::GAS_THERMS_PER_HOUR_TO_W;
use hares_types::{
    BoundaryPolicy, ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput,
    CorePerformance, CoreState, ElectricPower, EndUse, EnvironmentState, EquipmentDescriptor,
    EquipmentId, ExecutionStage, FuelPower, FuelType, HaresError, OperatingMode, PortContribution,
    PortDeclaration, PortSlots, ScheduleSource, Telemetry, TelemetryField, ThermalCategory, ZoneId,
    telemetry_keys as tk,
};
use serde::{Deserialize, Serialize};

use crate::schedule_helpers::{
    ScheduleSourceState, capture_schedule_source_state, parse_month_multipliers, parse_u32,
    parse_usize, parse_zone_id, restore_schedule_source_state,
};
use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

use crate::config::KEY_EQUIPMENT_ID;
const KEY_SENSIBLE_GAIN_FRACTION: &str = "sensible_gain_fraction";
const KEY_CONVECTIVE_GAIN_FRACTION: &str = "convective_gain_fraction";
const KEY_RADIATIVE_GAIN_FRACTION: &str = "radiative_gain_fraction";
const KEY_LATENT_GAIN_FRACTION: &str = "latent_gain_fraction";
pub(crate) const KEY_ZIP_Z: &str = "zip_z";
pub(crate) const KEY_ZIP_I: &str = "zip_i";
pub(crate) const KEY_ZIP_P: &str = "zip_p";
pub(crate) const KEY_ZIP_ZQ: &str = "zip_zq";
pub(crate) const KEY_ZIP_IQ: &str = "zip_iq";
pub(crate) const KEY_ZIP_PQ: &str = "zip_pq";
pub(crate) const KEY_ZIP_PF: &str = "zip_pf";
// Reserved for configurable reference voltage in the ZIP model.
pub(crate) const KEY_ZIP_V0: &str = "zip_v0";
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
pub(crate) const ZIP_SUM_TARGET: f64 = 1.0;
pub(crate) const ZIP_SUM_TOLERANCE: f64 = 1e-9;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ZipCoefficients {
    pub(crate) z: f64,
    pub(crate) i: f64,
    pub(crate) p_coeff: f64,
    pub(crate) v0: f64,
    pub(crate) zq: f64,
    pub(crate) iq: f64,
    pub(crate) pq: f64,
    pub(crate) pf: f64,
}

impl ZipCoefficients {
    pub(crate) fn apply(&self, p_kw: f64, voltage_pu: f64) -> (f64, f64) {
        let v_norm = voltage_pu / self.v0;
        let zip_multiplier = self.z * v_norm * v_norm + self.i * v_norm + self.p_coeff;
        let real_kw = p_kw * zip_multiplier;
        let reactive_base = self.zq * v_norm * v_norm + self.iq * v_norm + self.pq;
        let reactive_kvar = real_kw * self.pf * reactive_base;
        (real_kw, reactive_kvar)
    }
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

/// Look up literature-based ZIP coefficients from the OCHRE parameter table
/// (`vendors/OCHRE/ochre/defaults/ZIP Parameters.csv`). Returns `None` for
/// equipment classes not found in the table; callers fall back to `default()`.
///
/// Coefficients sourced from:
/// - Bokhari et al., IEEE Trans. Power Delivery 29(3), 2014
/// - Hajagos & Danai, IEEE Trans. Power Systems 13(2), 1998
/// - Lu et al., IEEE PESGM, 2008
/// - Arif et al., IEEE Trans. Smart Grid, 2013
pub(crate) fn zip_coefficients_from_class(class_name: &str) -> Option<ZipCoefficients> {
    // Lighting (Bokhari et al. 2014)
    const LIGHTING: ZipCoefficients = ZipCoefficients {
        z: 0.54,
        i: 0.5,
        p_coeff: -0.04,
        v0: 1.0,
        zq: 0.46,
        iq: 0.51,
        pq: 0.03,
        pf: 1.0,
    };
    // Refrigerator (Bokhari et al. 2014)
    const REFRIGERATOR: ZipCoefficients = ZipCoefficients {
        z: 5.03,
        i: -8.48,
        p_coeff: 4.45,
        v0: 1.0,
        zq: 17.44,
        iq: -28.62,
        pq: 12.18,
        pf: 0.8,
    };
    // MELs — miscellaneous electrical loads
    const MELS: ZipCoefficients = ZipCoefficients {
        z: 0.361_47,
        i: -0.085_83,
        p_coeff: 0.724_36,
        v0: 1.0,
        zq: 8.399_85,
        iq: -14.167_7,
        pq: 6.767_85,
        pf: 0.8,
    };
    // Pumps (Hajagos & Danai 1998); identical values shared with HVAC_HEAT_PUMP — updating
    // one requires updating the other.
    const PUMPS: ZipCoefficients = ZipCoefficients {
        z: 0.72,
        i: -0.98,
        p_coeff: 1.26,
        v0: 1.0,
        zq: 14.78,
        iq: -23.71,
        pq: 9.93,
        pf: 0.84,
    };
    // Resistance heater / baseboard (Bokhari et al. 2014)
    const RESISTANCE: ZipCoefficients = ZipCoefficients {
        z: 0.92,
        i: 0.1,
        p_coeff: -0.02,
        v0: 1.0,
        zq: 0.15,
        iq: 0.86,
        pq: -0.01,
        pf: 1.0,
    };
    // Fan coefficients from OCHRE `fans` row (no primary literature reference;
    // fan ZIP calibration is an open item in the CSV).
    const FAN: ZipCoefficients = ZipCoefficients {
        z: 0.26,
        i: 0.9,
        p_coeff: -0.16,
        v0: 1.0,
        zq: 0.5,
        iq: 0.62,
        pq: -0.12,
        pf: 0.87,
    };
    // HVAC heat pump / air conditioner (Hajagos & Danai 1998 — same source as PUMPS;
    // values are identical; updating one requires updating the other).
    const HVAC_HEAT_PUMP: ZipCoefficients = ZipCoefficients {
        z: 0.72,
        i: -0.98,
        p_coeff: 1.26,
        v0: 1.0,
        zq: 14.78,
        iq: -23.71,
        pq: 9.93,
        pf: 0.84,
    };
    const HVAC_COOLING: ZipCoefficients = ZipCoefficients {
        z: 1.6,
        i: -2.69,
        p_coeff: 2.09,
        v0: 1.0,
        zq: 12.53,
        iq: -21.11,
        pq: 9.58,
        pf: 0.96,
    };
    // Appliances
    const CLOTHES_WASHER: ZipCoefficients = ZipCoefficients {
        z: 0.05,
        i: 0.31,
        p_coeff: 0.64,
        v0: 1.0,
        zq: -0.56,
        iq: 2.2,
        pq: -0.64,
        pf: 0.65,
    };
    const CLOTHES_DRYER: ZipCoefficients = ZipCoefficients {
        z: 1.0,
        i: 0.0,
        p_coeff: 0.0,
        v0: 1.0,
        zq: 1.0,
        iq: 0.0,
        pq: 0.0,
        pf: 0.99,
    };
    const DISHWASHER: ZipCoefficients = ZipCoefficients {
        z: 1.0,
        i: 0.0,
        p_coeff: 0.0,
        v0: 1.0,
        zq: 0.0,
        iq: 0.0,
        pq: 1.0,
        pf: 0.99,
    };
    const RANGE: ZipCoefficients = ZipCoefficients {
        z: 1.0,
        i: 0.0,
        p_coeff: 0.0,
        v0: 1.0,
        zq: 1.0,
        iq: 0.0,
        pq: 0.0,
        pf: 1.0,
    };
    // Electric Resistance Water Heater — constant impedance (OCHRE CSV row 17)
    const RESISTANCE_WATER_HEATER: ZipCoefficients = ZipCoefficients {
        z: 1.0,
        i: 0.0,
        p_coeff: 0.0,
        v0: 1.0,
        zq: 1.0,
        iq: 0.0,
        pq: 0.0,
        pf: 1.0,
    };
    // Ideal loads
    const IDEAL: ZipCoefficients = ZipCoefficients {
        z: 0.0,
        i: 0.0,
        p_coeff: 1.0,
        v0: 1.0,
        zq: 0.0,
        iq: 0.0,
        pq: 1.0,
        pf: 1.0,
    };
    const HPWH: ZipCoefficients = ZipCoefficients {
        z: 0.825,
        i: -0.44,
        p_coeff: 0.615,
        v0: 1.0,
        zq: 7.465,
        iq: -11.425,
        pq: 4.96,
        pf: 0.97,
    };

    match class_name {
        "Lighting" | "Indoor Lighting" => Some(LIGHTING),
        "Exterior Lighting" => Some(LIGHTING),
        "Garage Lighting" => Some(LIGHTING),
        "Basement Lighting" => Some(LIGHTING),
        "Refrigerator" => Some(REFRIGERATOR),
        "Freezer" => Some(REFRIGERATOR),
        "MELs" | "Basement MELs" => Some(MELS),
        "Well Pump" | "Pool Pump" | "Spa Pump" => Some(PUMPS),
        "Pool Heater" | "Spa Heater" => Some(RESISTANCE),
        "Ceiling Fan" | "Ventilation Fan" => Some(FAN),
        "Clothes Washer" => Some(CLOTHES_WASHER),
        "Clothes Dryer" => Some(CLOTHES_DRYER),
        "Dishwasher" => Some(DISHWASHER),
        "Range" | "Cooking Range" => Some(RANGE),
        "ASHP Heater" | "MSHP Heater" => Some(HVAC_HEAT_PUMP),
        "Electric Baseboard" | "Electric Furnace" | "Electric Boiler" => Some(RESISTANCE),
        "Air Conditioner" | "ASHP Cooler" | "MSHP Cooler" | "Room AC" => Some(HVAC_COOLING),
        "Ideal Cooler" | "Ideal Heater" => Some(IDEAL),
        "Electric Resistance Water Heater" => Some(RESISTANCE_WATER_HEATER),
        "Heat Pump Water Heater" => Some(HPWH),
        _ => None,
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
    last_reactive_power_kvar: f64,
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
    core_output: CoreOutput,
    gas_source: Option<ScheduleSource>,
    gas_schedule_unit: GasScheduleUnit,
    sensible_gain_fraction: f64,
    radiant_gain_fraction: f64,
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
        // has no zone assignment -- its heat gain goes to the outdoor environment,
        // not the building envelope. "Garage" and "Basement" equipment auto-routes
        // to the respective zone when no explicit zone_id is provided.
        let name_lower = config.name.to_ascii_lowercase();
        let zone = if end_use == EndUse::EV {
            // EV charging occurs outside the building envelope.
            None
        } else if name_lower.contains("exterior") || name_lower.contains("outdoor") {
            None
        } else if let Some(explicit) = parse_zone_id(&config) {
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
            id: EquipmentId(parse_u32(&config, KEY_EQUIPMENT_ID).unwrap_or_default()),
            name: config.name,
            end_use,
            equipment_type: Cow::Borrowed(equipment_type),
            zone,
            fuel: FuelType::Electric,
            stage: ExecutionStage::Independent,
            control_capabilities: ControlCapabilities::LOAD_FRACTION
                | ControlCapabilities::MODE_OVERRIDE
                | ControlCapabilities::POWER_SETPOINT,
            core_capabilities: CoreCapabilities::ELECTRIC,
            telemetry_fields: scheduled_load_telemetry_fields(),
        };
        Self {
            descriptor,
            ports: vec![PortDeclaration::electrical()],
            telemetry: default_telemetry(),
            core_output: CoreOutput::default(),
            gas_source: None,
            gas_schedule_unit: GasScheduleUnit::ThermsPerHour,
            sensible_gain_fraction: 0.0,
            radiant_gain_fraction: 0.0,
            latent_gain_fraction: 0.0,
            zip: zip_coefficients_from_class(equipment_type).unwrap_or_default(),
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
        let radiative_frac = config.get_f64(KEY_RADIATIVE_GAIN_FRACTION).unwrap_or(0.0);
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
            .ok_or_else(|| {
                HaresError::Equipment(format!(
                    "sensible_gain_fraction missing for '{}'; must be specified explicitly",
                    self.descriptor.name
                ))
            })?;
        self.radiant_gain_fraction = radiative_frac;
        // "frac_latent" is the HPXML-parsed alias for latent_gain_fraction.
        self.latent_gain_fraction = config
            .get_f64(KEY_LATENT_GAIN_FRACTION)
            .or_else(|| config.get_f64("frac_latent"))
            .unwrap_or(0.0);
        if self.sensible_gain_fraction < 0.0 {
            return Err(HaresError::Equipment(format!(
                "sensible_gain_fraction ({}) must not be negative",
                self.sensible_gain_fraction
            )));
        }
        if self.sensible_gain_fraction > 1.0 + 1e-9 {
            return Err(HaresError::Equipment(format!(
                "sensible_gain_fraction ({}) must not exceed 1.0",
                self.sensible_gain_fraction
            )));
        }
        if self.latent_gain_fraction < 0.0 {
            return Err(HaresError::Equipment(format!(
                "latent_gain_fraction ({}) must not be negative",
                self.latent_gain_fraction
            )));
        }
        if self.radiant_gain_fraction < 0.0 {
            return Err(HaresError::Equipment(format!(
                "radiant_gain_fraction ({}) must not be negative",
                self.radiant_gain_fraction
            )));
        }
        if self.sensible_gain_fraction + self.latent_gain_fraction > 1.0 + 1e-9 {
            return Err(HaresError::Equipment(format!(
                "sensible_gain_fraction ({}) + latent_gain_fraction ({}) must not exceed 1.0",
                self.sensible_gain_fraction, self.latent_gain_fraction
            )));
        }
        if self.radiant_gain_fraction > self.sensible_gain_fraction + 1e-9 {
            return Err(HaresError::Equipment(format!(
                "radiant_gain_fraction ({}) must not exceed sensible_gain_fraction ({}) (convective gain would be negative)",
                self.radiant_gain_fraction, self.sensible_gain_fraction
            )));
        }
        let zip_base = self.zip;
        self.zip = parse_zip_coefficients(config, zip_base)?;
        #[cfg(feature = "observe")]
        tracing::debug!(
            equipment_type = %self.descriptor.equipment_type,
            instance = %self.descriptor.name,
            z = self.zip.z,
            i = self.zip.i,
            p_coeff = self.zip.p_coeff,
            zq = self.zip.zq,
            iq = self.zip.iq,
            pq = self.zip.pq,
            pf = self.zip.pf,
            "resolved ZIP coefficients",
        );
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
        let reactive_supported = self.zip.pf != 0.0
            || config.get_f64(KEY_ZIP_ZQ).is_some()
            || config.get_f64(KEY_ZIP_IQ).is_some()
            || config.get_f64(KEY_ZIP_PQ).is_some();
        self.descriptor.core_capabilities = CoreCapabilities::ELECTRIC
            | if reactive_supported {
                CoreCapabilities::REACTIVE
            } else {
                CoreCapabilities::empty()
            }
            | if self.gas_source.is_some() {
                CoreCapabilities::FUEL
            } else {
                CoreCapabilities::empty()
            };
        self.telemetry = default_telemetry();
        self.core_output = CoreOutput::default();
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
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            let real_sum = self.zip.z + self.zip.i + self.zip.p_coeff;
            assert!(
                (real_sum - ZIP_SUM_TARGET).abs() <= ZIP_SUM_TOLERANCE,
                "ScheduledLoad '{}': ZIP real-power coefficients do not sum to 1.0 \
                 (z={}, i={}, p_coeff={}, sum={})",
                self.descriptor.name,
                self.zip.z,
                self.zip.i,
                self.zip.p_coeff,
                real_sum,
            );
            if self.zip.pf != 0.0 {
                let reactive_sum = self.zip.zq + self.zip.iq + self.zip.pq;
                assert!(
                    (reactive_sum - ZIP_SUM_TARGET).abs() <= ZIP_SUM_TOLERANCE,
                    "ScheduledLoad '{}': ZIP reactive coefficients do not sum to 1.0 \
                     (zq={}, iq={}, pq={}, sum={})",
                    self.descriptor.name,
                    self.zip.zq,
                    self.zip.iq,
                    self.zip.pq,
                    reactive_sum,
                );
            }
        }

        // Grid outage: all outputs are zero.
        if env.grid.voltage_pu == 0.0 {
            self.last_non_zero_power_kw = 0.0;
            self.last_non_zero_gas_w = 0.0;
            self.telemetry.set(tk::ELECTRIC_KW, 0.0);
            self.telemetry.set(tk::REACTIVE_POWER_KVAR, 0.0);
            self.telemetry.set(tk::TOTAL_SENSIBLE_GAIN_W, 0.0);
            self.telemetry.set(tk::LATENT_GAIN_W, 0.0);
            self.telemetry.set(tk::FUEL_INPUT_W, 0.0);
            self.core_output = CoreOutput {
                flows: CoreFlows {
                    electric_kw: Some(ElectricPower::Consumption(0.0)),
                    reactive_power_kvar: self
                        .descriptor
                        .core_capabilities
                        .contains(CoreCapabilities::REACTIVE)
                        .then_some(0.0),
                    fuel_w: self
                        .descriptor
                        .core_capabilities
                        .contains(CoreCapabilities::FUEL)
                        .then_some(FuelPower {
                            fuel_type: FuelType::Gas,
                            consumption_w: 0.0,
                        }),
                    thermal_output_w: None,
                    sensible_cooling_w: None,
                    latent_cooling_w: None,
                },
                state: CoreState {
                    operating_mode: None,
                    soc: None,
                    speed_index: None,
                    setpoint_c: None,
                },
                performance: CorePerformance::default(),
            };
            return Ok(());
        }

        // Mode override Off forces zero output regardless of schedule.
        if self.mode_override == Some(OperatingMode::Off) {
            self.last_non_zero_power_kw = 0.0;
            self.last_non_zero_gas_w = 0.0;
            self.telemetry.set(tk::ELECTRIC_KW, 0.0);
            self.telemetry.set(tk::REACTIVE_POWER_KVAR, 0.0);
            self.telemetry.set(tk::TOTAL_SENSIBLE_GAIN_W, 0.0);
            self.telemetry.set(tk::LATENT_GAIN_W, 0.0);
            self.telemetry.set(tk::FUEL_INPUT_W, 0.0);
            self.core_output = CoreOutput {
                flows: CoreFlows {
                    electric_kw: Some(ElectricPower::Consumption(0.0)),
                    reactive_power_kvar: self
                        .descriptor
                        .core_capabilities
                        .contains(CoreCapabilities::REACTIVE)
                        .then_some(0.0),
                    fuel_w: self
                        .descriptor
                        .core_capabilities
                        .contains(CoreCapabilities::FUEL)
                        .then_some(FuelPower {
                            fuel_type: FuelType::Gas,
                            consumption_w: 0.0,
                        }),
                    thermal_output_w: None,
                    sensible_cooling_w: None,
                    latent_cooling_w: None,
                },
                state: CoreState {
                    operating_mode: None,
                    soc: None,
                    speed_index: None,
                    setpoint_c: None,
                },
                performance: CorePerformance::default(),
            };
            return Ok(());
        }

        // PowerSetpoint overrides the schedule entirely for this step. The override
        // is cleared at the end of this step (per-step, not persistent).
        let (electric_power_kw, reactive_power_kvar, gas_consumption_w) =
            if let Some(override_kw) = self.power_setpoint_override.take() {
                (override_kw, 0.0, 0.0)
            } else {
                // Apply month multiplier (OCHRE ScheduledLoad.py:38-41). Negative schedule
                // values are silently clamped to zero -- OCHRE treats them as "no load" rather
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
                    self.zip.apply(scheduled_power_kw, env.grid.voltage_pu)
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
        let total_sensible_w = total_gain_source_w * self.sensible_gain_fraction;
        let radiant_gain_w = total_gain_source_w * self.radiant_gain_fraction;
        let sensible_gain_w = total_sensible_w - radiant_gain_w;
        let latent_gain_w = total_gain_source_w * self.latent_gain_fraction;
        if let Some(zone) = self.descriptor.zone {
            if sensible_gain_w != 0.0 || radiant_gain_w != 0.0 || latent_gain_w != 0.0 {
                ports.accumulate(&PortContribution::Thermal {
                    zone,
                    sensible_gain_w,
                    radiant_gain_w,
                    latent_gain_w,
                    category: ThermalCategory::InternalGain,
                })?;
            }
        }

        self.telemetry.set(tk::ELECTRIC_KW, electric_power_kw);
        self.telemetry
            .set(tk::REACTIVE_POWER_KVAR, reactive_power_kvar);
        self.telemetry
            .set(tk::TOTAL_SENSIBLE_GAIN_W, total_sensible_w);
        self.telemetry.set(tk::LATENT_GAIN_W, latent_gain_w);
        self.telemetry.set(tk::FUEL_INPUT_W, gas_consumption_w);
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(electric_power_kw.max(0.0))),
                reactive_power_kvar: self
                    .descriptor
                    .core_capabilities
                    .contains(CoreCapabilities::REACTIVE)
                    .then_some(reactive_power_kvar),
                fuel_w: self
                    .descriptor
                    .core_capabilities
                    .contains(CoreCapabilities::FUEL)
                    .then_some(FuelPower {
                        fuel_type: FuelType::Gas,
                        consumption_w: gas_consumption_w.max(0.0),
                    }),
                thermal_output_w: None,
                sensible_cooling_w: None,
                latent_cooling_w: None,
            },
            state: CoreState {
                operating_mode: None,
                soc: None,
                speed_index: None,
                setpoint_c: None,
            },
            performance: CorePerformance::default(),
        };
        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core_output
    }

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&ScheduledLoadState {
            last_non_zero_power_kw: self.last_non_zero_power_kw,
            last_non_zero_gas_w: self.last_non_zero_gas_w,
            last_reactive_power_kvar: self.telemetry.get(tk::REACTIVE_POWER_KVAR).unwrap_or(0.0),
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
            .insert(tk::ELECTRIC_KW, self.last_non_zero_power_kw);
        self.telemetry
            .insert(tk::REACTIVE_POWER_KVAR, decoded.last_reactive_power_kvar);
        // last_non_zero_power_kw already holds the ZIP-adjusted kW value (set in step()),
        // so the thermal gain reconstruction below is accurate -- it does not need a
        // separate voltage correction. The same ZIP-adjusted wattage drives sensible/latent
        // heat gains regardless of what the voltage was at the time of last non-zero output.
        let total_gain_source_w = self.last_non_zero_power_kw * 1_000.0 + self.last_non_zero_gas_w;
        self.telemetry.insert(
            tk::TOTAL_SENSIBLE_GAIN_W,
            total_gain_source_w * self.sensible_gain_fraction,
        );
        self.telemetry.insert(
            tk::LATENT_GAIN_W,
            total_gain_source_w * self.latent_gain_fraction,
        );
        self.telemetry
            .insert(tk::FUEL_INPUT_W, self.last_non_zero_gas_w);
        self.core_output = CoreOutput::default();
        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        match signal {
            ControlSignal::LoadFraction { fraction } => {
                self.load_fraction = fraction.max(0.0);
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
    let mut telemetry = Telemetry::with_capacity(5);
    telemetry.insert(tk::ELECTRIC_KW, 0.0);
    telemetry.insert(tk::REACTIVE_POWER_KVAR, 0.0);
    telemetry.insert(tk::TOTAL_SENSIBLE_GAIN_W, 0.0);
    telemetry.insert(tk::LATENT_GAIN_W, 0.0);
    telemetry.insert(tk::FUEL_INPUT_W, 0.0);
    telemetry
}

fn scheduled_load_telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: tk::ELECTRIC_KW.to_string(),
            unit: "kW".to_string(),
            description: "ZIP-adjusted active electrical power draw".to_string(),
        },
        TelemetryField {
            name: tk::REACTIVE_POWER_KVAR.to_string(),
            unit: "kVAR".to_string(),
            description: "ZIP-adjusted reactive electrical power".to_string(),
        },
        TelemetryField {
            name: tk::TOTAL_SENSIBLE_GAIN_W.to_string(),
            unit: "W".to_string(),
            description: "Total sensible thermal gain (convective + radiant) to assigned zone"
                .to_string(),
        },
        TelemetryField {
            name: tk::LATENT_GAIN_W.to_string(),
            unit: "W".to_string(),
            description: "Latent thermal gain to assigned zone".to_string(),
        },
        TelemetryField {
            name: tk::FUEL_INPUT_W.to_string(),
            unit: "W".to_string(),
            description: "Gas fuel consumption rate converted to watts".to_string(),
        },
    ]
}

/// Resolve ZIP coefficients by merging type-specific defaults with user-supplied
/// config key overrides. Each config key (`zip_z`, `zip_i`, ...) overrides the
/// corresponding field in `base` only when explicitly present in the config map.
pub(crate) fn parse_zip_coefficients(
    config: &EquipmentConfig,
    base: ZipCoefficients,
) -> crate::Result<ZipCoefficients> {
    let z = config.get_f64(KEY_ZIP_Z).unwrap_or(base.z);
    let i = config.get_f64(KEY_ZIP_I).unwrap_or(base.i);
    let p_coeff = config.get_f64(KEY_ZIP_P).unwrap_or(base.p_coeff);
    // v0 defaults to 1.0 pu (no normalisation); override to calibrate ZIP at a
    // non-nominal voltage, e.g. 0.95 pu for ANSI Range A lower boundary.
    let v0 = config.get_f64(KEY_ZIP_V0).unwrap_or(base.v0);
    let zq = config.get_f64(KEY_ZIP_ZQ).unwrap_or(base.zq);
    let iq = config.get_f64(KEY_ZIP_IQ).unwrap_or(base.iq);
    let pq = config.get_f64(KEY_ZIP_PQ).unwrap_or(base.pq);
    let pf = config.get_f64(KEY_ZIP_PF).unwrap_or(base.pf);

    let zip = ZipCoefficients {
        z,
        i,
        p_coeff,
        v0,
        zq,
        iq,
        pq,
        pf,
    };

    let sum = zip.z + zip.i + zip.p_coeff;
    if (sum - ZIP_SUM_TARGET).abs() > ZIP_SUM_TOLERANCE {
        return Err(HaresError::Equipment(format!(
            "invalid ZIP coefficients: z + i + p = {sum}, expected {ZIP_SUM_TARGET}"
        )));
    }
    // Validate reactive sum when reactive power is active (pf != 0) and any
    // reactive coefficient was explicitly set by the user. When pf != 0 but no
    // explicit reactive keys are present, the type-specific defaults are trusted
    // (they are sourced from literature and validated by the per-step invariant check).
    if pf != 0.0
        && (config.get_f64(KEY_ZIP_ZQ).is_some()
            || config.get_f64(KEY_ZIP_IQ).is_some()
            || config.get_f64(KEY_ZIP_PQ).is_some())
    {
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
            let Some(col_idx) = parse_usize(config, KEY_POWER_SCHEDULE_COL)? else {
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
            let Some(col_idx) = parse_usize(config, KEY_GAS_SCHEDULE_COL)? else {
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
        ScheduleSource::TimeWindows { windows, .. } => {
            for w in windows.iter_mut() {
                w.value *= scale;
                if let Some(min) = &mut w.min_value {
                    *min *= scale;
                }
                if let Some(max) = &mut w.max_value {
                    *max *= scale;
                }
                // Ensure min ≤ max after scaling (negative scale inverts).
                if let (Some(lo), Some(hi)) = (&mut w.min_value, &mut w.max_value) {
                    if *lo > *hi {
                        std::mem::swap(lo, hi);
                    }
                }
            }
        }
        ScheduleSource::ColumnRef { .. } | ScheduleSource::SolarAware { .. } => {}
        // Wildcard required: ScheduleSource is #[non_exhaustive].
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
        | ScheduleSource::Stochastic { .. }
        | ScheduleSource::TimeWindows { .. } => false,
        // Wildcard required: ScheduleSource is #[non_exhaustive].
        // New variants must be handled explicitly here.
        _ => false,
    }
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
        PortSlots, SCHEDULE_DOMAIN_ID, ScheduleSource, TelemetryField, WeatherState, ZoneId,
        ZoneState,
    };
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    use hares_types::telemetry_keys as tk;

    use super::{
        GAS_THERMS_PER_HOUR_TO_W, KEY_CONVECTIVE_GAIN_FRACTION, KEY_GAS_CONSTANT,
        KEY_GAS_SCHEDULE_IS_W, KEY_GAS_SCHEDULE_SOURCE, KEY_LATENT_GAIN_FRACTION,
        KEY_POWER_CONSTANT_KW, KEY_POWER_SCHEDULE_COL, KEY_POWER_SCHEDULE_SOURCE,
        KEY_RADIATIVE_GAIN_FRACTION, KEY_SENSIBLE_GAIN_FRACTION, KEY_ZIP_I, KEY_ZIP_P, KEY_ZIP_V0,
        KEY_ZIP_Z, ScheduledLoad,
    };

    use crate::schedule_helpers::KEY_MONTH_MULTIPLIER_PREFIX;
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
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .expect("UTC offset")
                .with_ymd_and_hms(2026, 3, 18, 0, 0, 0)
                .single()
                .expect("valid UTC timestamp"),
            time_res: ChronoDuration::minutes(15),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    fn config_with_schedule(name: &str, ochre_class: &str, schedule: &[f64]) -> EquipmentConfig {
        let mut raw: HashMap<String, crate::config::ConfigValue> = HashMap::new();
        raw.insert("zone_id".to_string(), 1.0.into());
        raw.insert(KEY_POWER_SCHEDULE_SOURCE.to_string(), "constant".into());
        raw.insert(
            KEY_POWER_CONSTANT_KW.to_string(),
            schedule.first().copied().unwrap_or(0.0).into(),
        );
        raw.insert(KEY_SENSIBLE_GAIN_FRACTION.to_string(), 0.0.into());
        EquipmentConfig::raw(name.to_string(), ochre_class.to_string(), raw)
    }

    fn config_with_extras(
        name: &str,
        ochre_class: &str,
        schedule: &[f64],
        extras: &[(&str, crate::config::ConfigValue)],
    ) -> EquipmentConfig {
        let mut raw: HashMap<String, crate::config::ConfigValue> = HashMap::new();
        raw.insert("zone_id".to_string(), 1.0.into());
        raw.insert(KEY_POWER_SCHEDULE_SOURCE.to_string(), "constant".into());
        raw.insert(
            KEY_POWER_CONSTANT_KW.to_string(),
            schedule.first().copied().unwrap_or(0.0).into(),
        );
        for (k, v) in extras {
            raw.insert(k.to_string(), v.clone());
        }
        EquipmentConfig::raw(name.to_string(), ochre_class.to_string(), raw)
    }

    fn config_no_zone(name: &str, ochre_class: &str, schedule: &[f64]) -> EquipmentConfig {
        let mut raw: HashMap<String, crate::config::ConfigValue> = HashMap::new();
        raw.insert(KEY_POWER_SCHEDULE_SOURCE.to_string(), "constant".into());
        raw.insert(
            KEY_POWER_CONSTANT_KW.to_string(),
            schedule.first().copied().unwrap_or(0.0).into(),
        );
        EquipmentConfig::raw(name.to_string(), ochre_class.to_string(), raw)
    }

    #[test]
    fn init_rejects_invalid_zip_sum() {
        let config = config_with_extras(
            "s",
            "Lighting",
            &[1.0],
            &[
                (KEY_SENSIBLE_GAIN_FRACTION, 0.0.into()),
                (KEY_ZIP_Z, 0.2.into()),
                (KEY_ZIP_I, 0.2.into()),
                (KEY_ZIP_P, 0.2.into()),
            ],
        );
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let err = eq.init(&config, &base_env()).unwrap_err();
        assert!(err.to_string().contains("invalid ZIP coefficients"));
    }

    #[test]
    fn zip_voltage_uses_env_grid_voltage() {
        let config = config_with_extras(
            "s",
            "Lighting",
            &[2.0],
            &[
                (KEY_SENSIBLE_GAIN_FRACTION, 0.0.into()),
                (KEY_ZIP_Z, 0.2.into()),
                (KEY_ZIP_I, 0.3.into()),
                (KEY_ZIP_P, 0.5.into()),
            ],
        );
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
        let config = config_with_extras(
            "s",
            "Lighting",
            &[1.0],
            &[
                (KEY_SENSIBLE_GAIN_FRACTION, 0.5.into()),
                (KEY_LATENT_GAIN_FRACTION, 0.2.into()),
            ],
        );
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
    fn missing_sensible_gain_fraction_returns_err_on_init() {
        // Construct a config without any sensible_gain_fraction key.
        let mut raw: std::collections::HashMap<String, crate::config::ConfigValue> =
            std::collections::HashMap::new();
        raw.insert("zone_id".to_string(), 1.0.into());
        raw.insert(KEY_POWER_SCHEDULE_SOURCE.to_string(), "constant".into());
        raw.insert(KEY_POWER_CONSTANT_KW.to_string(), 1.0.into());
        // NOTE: KEY_SENSIBLE_GAIN_FRACTION deliberately omitted.
        let config = crate::EquipmentConfig::raw("s".to_string(), "Lighting".to_string(), raw);
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let err = eq.init(&config, &base_env()).unwrap_err();
        assert!(
            err.to_string().contains("sensible_gain_fraction missing"),
            "expected 'sensible_gain_fraction missing' error, got: {err}"
        );
    }

    #[test]
    fn out_of_range_sensible_gain_fraction_returns_err() {
        use crate::config::ConfigValue;
        let mut raw: std::collections::HashMap<String, ConfigValue> =
            std::collections::HashMap::new();
        raw.insert("zone_id".to_string(), 1.0.into());
        raw.insert(KEY_POWER_SCHEDULE_SOURCE.to_string(), "constant".into());
        raw.insert(KEY_POWER_CONSTANT_KW.to_string(), 1.0.into());
        raw.insert(KEY_SENSIBLE_GAIN_FRACTION.to_string(), 1.5.into());

        let config = crate::EquipmentConfig::raw("s".to_string(), "Lighting".to_string(), raw);
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let env = base_env();
        let result = eq.init(&config, &env);
        assert!(
            result.is_err(),
            "init() must return Err for sensible_gain_fraction=1.5 outside [0.0, 1.0]; \
             got Ok (upper-bound validation missing)"
        );
    }

    #[test]
    fn lighting_physics_uses_point_seven_sensible_fraction() {
        let config = config_with_extras(
            "s",
            "Indoor Lighting",
            &[1.0],
            &[(KEY_SENSIBLE_GAIN_FRACTION, 0.70.into())],
        );
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

        // No new PowerSetpoint issued -- next step reverts to schedule value.
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
        let config = config_with_extras(
            "s",
            "Lighting",
            &[1.0],
            &[
                (KEY_SENSIBLE_GAIN_FRACTION, 0.0.into()),
                (KEY_GAS_SCHEDULE_SOURCE, "constant".into()),
                (KEY_GAS_CONSTANT, 0.1.into()),
            ],
        );
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
        let config = config_with_extras(
            "s",
            "Lighting",
            &[1.0],
            &[
                (KEY_SENSIBLE_GAIN_FRACTION, 0.0.into()),
                (KEY_GAS_SCHEDULE_SOURCE, "constant".into()),
                (KEY_GAS_CONSTANT, 1000.0.into()),
                (KEY_GAS_SCHEDULE_IS_W, 1.0.into()),
            ],
        );
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
        let config = config_with_extras(
            "s",
            "Lighting",
            &[1.0],
            &[(KEY_SENSIBLE_GAIN_FRACTION, 0.0.into())],
        );
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
        assert_eq!(telemetry.get(tk::ELECTRIC_KW), Some(2.0));
    }

    #[test]
    fn column_ref_reads_from_schedule_custom_domain() {
        let config = config_with_extras(
            "s",
            "Lighting",
            &[0.0],
            &[
                (KEY_POWER_SCHEDULE_SOURCE, "column".into()),
                (KEY_POWER_SCHEDULE_COL, 1.0.into()),
                (KEY_SENSIBLE_GAIN_FRACTION, 0.0.into()),
            ],
        );

        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let mut env = base_env();
        env.custom_domains.push(DomainUpdate {
            domain_id: SCHEDULE_DOMAIN_ID,
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
        let config = config_with_extras(
            "s",
            "Lighting",
            &[0.0],
            &[(KEY_SENSIBLE_GAIN_FRACTION, 0.0.into())],
        );
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
        assert!(names.contains(&tk::ELECTRIC_KW));
        assert!(names.contains(&tk::TOTAL_SENSIBLE_GAIN_W));
        assert!(names.contains(&tk::LATENT_GAIN_W));
        assert!(names.contains(&tk::FUEL_INPUT_W));
    }

    #[test]
    fn registry_includes_scheduled_load_aliases() {
        let registry = EquipmentRegistry::new();
        assert!(registry.get("Lighting").is_some());
        assert!(registry.get("Plug Loads").is_some());
        assert!(registry.get("Other").is_some());
    }

    #[test]
    fn load_fraction_scales_gas_output() {
        let config = config_with_extras(
            "s",
            "Gas Grill",
            &[1.0],
            &[
                (KEY_SENSIBLE_GAIN_FRACTION, 0.0.into()),
                (KEY_GAS_SCHEDULE_SOURCE, "constant".into()),
                (KEY_GAS_CONSTANT, 0.2.into()),
            ],
        );
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
        let config = config_with_extras(
            "Ceiling Fan",
            "Ceiling Fan",
            &[2.0],
            &[
                (KEY_SENSIBLE_GAIN_FRACTION, 0.0.into()),
                (&format!("{KEY_MONTH_MULTIPLIER_PREFIX}0"), 0.0.into()),
            ],
        );
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
        let config = config_with_extras(
            "s",
            "Lighting",
            &[1.0],
            &[
                (KEY_CONVECTIVE_GAIN_FRACTION, 0.3.into()),
                (KEY_RADIATIVE_GAIN_FRACTION, 0.2.into()),
            ],
        );
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let env = base_env();
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        // 1 kW = 1000 W; sensible = conv(0.3) + rad(0.2) = 0.5 → total sensible = 500W
        // convective = total_sensible - radiant = 500 - 200 = 300W
        // radiant = 1000 * 0.2 = 200W
        assert!((ports.thermal[0].sensible_gain_w - 300.0).abs() < 1e-9);
        assert!((ports.thermal[0].radiant_gain_w - 200.0).abs() < 1e-9);
    }

    #[test]
    fn sensible_plus_latent_exceeding_one_is_rejected() {
        let config = config_with_extras(
            "s",
            "Lighting",
            &[1.0],
            &[
                (KEY_SENSIBLE_GAIN_FRACTION, 0.7.into()),
                (KEY_LATENT_GAIN_FRACTION, 0.5.into()),
            ],
        );
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
        let config = config_with_extras(
            "s",
            "Lighting",
            &[2.0],
            &[
                (KEY_SENSIBLE_GAIN_FRACTION, 0.0.into()),
                (KEY_ZIP_Z, 0.2.into()),
                (KEY_ZIP_I, 0.3.into()),
                (KEY_ZIP_P, 0.5.into()),
                (KEY_ZIP_V0, 0.95.into()),
            ],
        );
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
        let config = config_with_extras(
            "s",
            "Lighting",
            &[2.0],
            &[
                (KEY_SENSIBLE_GAIN_FRACTION, 0.0.into()),
                (super::KEY_ZIP_ZQ, 0.0.into()),
                (super::KEY_ZIP_IQ, 0.0.into()),
                (super::KEY_ZIP_PQ, 1.0.into()),
                (super::KEY_ZIP_PF, 0.8.into()),
            ],
        );
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
        // "TV" has no type-specific ZIP defaults, so pf=0.0, reactive_power=0.
        let config = config_with_extras(
            "s",
            "TV",
            &[3.0],
            &[(KEY_SENSIBLE_GAIN_FRACTION, 0.0.into())],
        );
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::PLUG_LOADS, "TV");
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
        // Explicit pure-P real ZIP (z=0, i=0, p=1) keeps real_kw independent of voltage.
        let config = config_with_extras(
            "s",
            "Lighting",
            &[1.0],
            &[
                (KEY_SENSIBLE_GAIN_FRACTION, 0.0.into()),
                (super::KEY_ZIP_ZQ, 1.0.into()),
                (super::KEY_ZIP_IQ, 0.0.into()),
                (super::KEY_ZIP_PQ, 0.0.into()),
                (super::KEY_ZIP_PF, 0.9.into()),
                (KEY_ZIP_Z, 0.0.into()),
                (KEY_ZIP_I, 0.0.into()),
                (KEY_ZIP_P, 1.0.into()),
            ],
        );
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
        let config = config_with_extras(
            "s",
            "Lighting",
            &[1.0],
            &[
                (KEY_SENSIBLE_GAIN_FRACTION, 0.0.into()),
                (super::KEY_ZIP_ZQ, 0.3.into()),
                (super::KEY_ZIP_IQ, 0.3.into()),
                (super::KEY_ZIP_PQ, 0.3.into()),
            ],
        );
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let err = eq.init(&config, &base_env()).unwrap_err();
        assert!(
            err.to_string().contains("reactive ZIP coefficients"),
            "expected reactive ZIP validation error, got: {err}"
        );
    }

    #[test]
    fn garage_name_auto_routes_to_garage_zone() {
        let config = config_no_zone("Garage Lighting", "Garage Lighting", &[1.0]);
        let eq = ScheduledLoad::new(config, hares_types::EndUse::LIGHTING, "Garage Lighting");
        assert_eq!(
            eq.descriptor().zone,
            Some(ZoneId(2)),
            "Garage equipment should auto-route to ZoneId(2)"
        );
    }

    #[test]
    fn basement_name_auto_routes_to_foundation_zone() {
        let config = config_no_zone("Basement Lighting", "Basement Lighting", &[1.0]);
        let eq = ScheduledLoad::new(config, hares_types::EndUse::LIGHTING, "Basement Lighting");
        assert_eq!(
            eq.descriptor().zone,
            Some(ZoneId(3)),
            "Basement equipment should auto-route to ZoneId(3)"
        );
    }

    #[test]
    fn explicit_zone_id_overrides_auto_routing() {
        let config = config_with_schedule("Garage Lighting", "Garage Lighting", &[1.0]);
        let eq = ScheduledLoad::new(config, hares_types::EndUse::LIGHTING, "Garage Lighting");
        assert_eq!(
            eq.descriptor().zone,
            Some(ZoneId(1)),
            "Explicit zone_id should take precedence over name-based auto-routing"
        );
    }

    #[test]
    fn outdoor_name_suppresses_auto_routing() {
        let config = config_no_zone("Outdoor Garage Fan", "Other", &[1.0]);
        let eq = ScheduledLoad::new(config, hares_types::EndUse::OTHER, "Other");
        assert!(
            eq.descriptor().zone.is_none(),
            "Outdoor prefix takes precedence and suppresses garage auto-routing"
        );
    }

    // --- Scheduled EV registration tests (Ticket 2) ---

    #[test]
    fn scheduled_ev_resolves_from_registry() {
        let registry = EquipmentRegistry::new();
        assert!(
            registry.get("Scheduled EV").is_some(),
            "Scheduled EV must be registered in the equipment registry"
        );
    }

    #[test]
    fn scheduled_ev_has_no_zone() {
        let registry = EquipmentRegistry::new();
        let config = config_with_schedule("Scheduled EV", "Scheduled EV", &[3.5]);
        let factory = registry
            .get("Scheduled EV")
            .expect("Scheduled EV registered");
        let eq = factory(config);
        assert!(
            eq.descriptor().zone.is_none(),
            "Scheduled EV must have zone=None (charging occurs outside building envelope)"
        );
    }

    #[test]
    fn scheduled_ev_has_ev_end_use() {
        let registry = EquipmentRegistry::new();
        let config = config_with_schedule("Scheduled EV", "Scheduled EV", &[3.5]);
        let factory = registry
            .get("Scheduled EV")
            .expect("Scheduled EV registered");
        let eq = factory(config);
        assert_eq!(
            eq.descriptor().end_use,
            hares_types::EndUse::EV,
            "Scheduled EV must carry EndUse::EV"
        );
    }

    // --- Reactive power telemetry tests (Ticket 5) ---

    #[test]
    fn zip_reactive_power_emitted_to_telemetry() {
        // Iq=0.8 (pure current reactive term), pq=0.2, zq=0, pf=0.9 at nominal voltage v=1.0.
        // reactive_base = zq*v² + iq*v + pq = 0.0 + 0.8*1.0 + 0.2 = 1.0
        // reactive_kvar = real_kw * pf * reactive_base = 2.0 * 0.9 * 1.0 = 1.8
        let config = config_with_extras(
            "s",
            "Lighting",
            &[2.0],
            &[
                (super::KEY_ZIP_ZQ, 0.0.into()),
                (super::KEY_ZIP_IQ, 0.8.into()),
                (super::KEY_ZIP_PQ, 0.2.into()),
                (super::KEY_ZIP_PF, 0.9.into()),
                (KEY_SENSIBLE_GAIN_FRACTION, 0.0.into()),
            ],
        );
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let env = base_env();
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots::default();
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();

        let expected_kvar = 2.0_f64 * 0.9 * (0.0 + 0.8 * 1.0 + 0.2);
        let telemetry_kvar = eq
            .telemetry()
            .get(tk::REACTIVE_POWER_KVAR)
            .expect("reactive_power_kvar must be present in telemetry");
        assert!(
            (telemetry_kvar - expected_kvar).abs() < 1e-12,
            "telemetry reactive_power_kvar={telemetry_kvar} expected={expected_kvar}"
        );
        // Confirm telemetry matches port accumulation.
        assert!(
            (telemetry_kvar - ports.electrical.reactive_power_kvar).abs() < 1e-12,
            "telemetry must match port accumulation: telemetry={telemetry_kvar} port={}",
            ports.electrical.reactive_power_kvar
        );
    }

    #[test]
    fn reactive_power_telemetry_zero_without_zip() {
        // "TV" has no type-specific ZIP defaults, so pf=0.0, reactive power is always zero.
        let config = config_with_schedule("s", "TV", &[5.0]);
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::PLUG_LOADS, "TV");
        let env = base_env();
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();

        let telemetry_kvar = eq
            .telemetry()
            .get(tk::REACTIVE_POWER_KVAR)
            .expect("reactive_power_kvar must be present in telemetry even when zero");
        assert_eq!(
            telemetry_kvar, 0.0,
            "reactive_power_kvar telemetry must be 0.0 when no ZIP coefficients configured"
        );
    }

    #[test]
    fn load_fraction_negative_is_clamped_to_zero() {
        let config = config_with_schedule("s", "Lighting", &[2.0]);
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let env = base_env();
        eq.init(&config, &env).unwrap();
        eq.apply_control(&ControlSignal::LoadFraction { fraction: -0.5 })
            .unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert_eq!(
            ports.electrical.net_active_kw(),
            0.0,
            "negative load fraction must be clamped to 0, producing zero power"
        );
    }

    #[test]
    fn radiant_gain_fraction_splits_sensible_gain() {
        let config = config_with_extras(
            "s",
            "Lighting",
            &[0.2],
            &[
                (KEY_SENSIBLE_GAIN_FRACTION, 1.0.into()),
                (KEY_RADIATIVE_GAIN_FRACTION, 0.3.into()),
            ],
        );
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let env = base_env();
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();

        let total_gain_w = 200.0;
        let expected_radiant = total_gain_w * 0.3;
        let expected_convective = total_gain_w * 1.0 - expected_radiant;
        assert!(
            (ports.thermal[0].sensible_gain_w - expected_convective).abs() < 1e-9,
            "convective sensible should be {:.3}, got {:.3}",
            expected_convective,
            ports.thermal[0].sensible_gain_w
        );
        assert!(
            (ports.thermal[0].radiant_gain_w - expected_radiant).abs() < 1e-9,
            "radiant gain should be {:.3}, got {:.3}",
            expected_radiant,
            ports.thermal[0].radiant_gain_w
        );
    }

    // --- Type-specific ZIP coefficient default tests (T-0006) ---

    #[test]
    fn user_zip_z_overrides_type_specific_default() {
        let config = config_with_extras(
            "s",
            "Lighting",
            &[2.0],
            &[
                (KEY_SENSIBLE_GAIN_FRACTION, 0.0.into()),
                (KEY_ZIP_Z, 0.3.into()),
                (KEY_ZIP_I, 0.3.into()),
                (KEY_ZIP_P, 0.4.into()),
            ],
        );
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let mut env = base_env();
        env.grid.voltage_pu = 0.95;
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        let expected_multiplier = 0.3 * 0.95_f64.powi(2) + 0.3 * 0.95 + 0.4;
        let expected_kw = 2.0 * expected_multiplier;
        assert!(
            (ports.electrical.net_active_kw() - expected_kw).abs() < 1e-12,
            "user override z=0.3,i=0.3,p=0.4 should override type-specific Lighting defaults"
        );
        assert_eq!(eq.zip.z, 0.3);
        assert_eq!(eq.zip.i, 0.3);
        assert_eq!(eq.zip.p_coeff, 0.4);
    }

    #[test]
    fn unrecognized_class_falls_back_to_zip_default() {
        let config = config_with_schedule("s", "TV", &[1.0]);
        let eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::PLUG_LOADS, "TV");
        assert_eq!(eq.zip.z, 0.0);
        assert_eq!(eq.zip.i, 0.0);
        assert_eq!(eq.zip.p_coeff, 1.0);
        assert_eq!(eq.zip.pf, 0.0);
        assert_eq!(eq.zip.zq, 0.0);
        assert_eq!(eq.zip.iq, 0.0);
        assert_eq!(eq.zip.pq, 1.0);
    }

    #[test]
    fn type_specific_zip_produces_voltage_dependent_load() {
        // Lighting defaults: z=0.54, i=0.5, p_coeff=-0.04, pf=1.0
        // At 0.9 pu: multiplier = 0.54*0.81 + 0.5*0.9 + (-0.04) = 0.8474
        // Default (TV): multiplier = 0*0.81 + 0*0.9 + 1.0 = 1.0
        let lighting_config = config_with_schedule("s", "Lighting", &[1.0]);
        let mut lighting_eq = ScheduledLoad::new(
            lighting_config.clone(),
            hares_types::EndUse::LIGHTING,
            "Lighting",
        );
        let mut env = base_env();
        lighting_eq.init(&lighting_config, &env).unwrap();

        let tv_config = config_with_schedule("s", "TV", &[1.0]);
        let mut tv_eq =
            ScheduledLoad::new(tv_config.clone(), hares_types::EndUse::PLUG_LOADS, "TV");
        // TV requires explicit sensible gain fraction
        let tv_config_full = config_with_extras(
            "s",
            "TV",
            &[1.0],
            &[(KEY_SENSIBLE_GAIN_FRACTION, 0.0.into())],
        );
        tv_eq.init(&tv_config_full, &env).unwrap();

        // At 1.0 pu both produce 1.0 kW (ZIP multiplier = 1.0)
        let mut ports_lighting = PortSlots::default();
        let mut ports_tv = PortSlots::default();
        lighting_eq
            .step(&env, Duration::from_secs(900), &mut ports_lighting)
            .unwrap();
        tv_eq
            .step(&env, Duration::from_secs(900), &mut ports_tv)
            .unwrap();
        assert!(
            (ports_lighting.electrical.net_active_kw() - 1.0).abs() < 1e-12,
            "at nominal voltage Lighting ZIP multiplier should equal 1.0"
        );
        assert!(
            (ports_tv.electrical.net_active_kw() - 1.0).abs() < 1e-12,
            "at nominal voltage TV (default) should produce 1.0 kW"
        );

        // At 0.9 pu: Lighting produces less (voltage-sensitive), TV stays the same
        env.grid.voltage_pu = 0.9;
        ports_lighting.zero();
        ports_tv.zero();
        lighting_eq
            .step(&env, Duration::from_secs(900), &mut ports_lighting)
            .unwrap();
        tv_eq
            .step(&env, Duration::from_secs(900), &mut ports_tv)
            .unwrap();

        let lighting_kw = ports_lighting.electrical.net_active_kw();
        let tv_kw = ports_tv.electrical.net_active_kw();
        let expected_lighting_mult = 0.54 * 0.9_f64.powi(2) + 0.5 * 0.9 - 0.04;
        assert!(
            (lighting_kw - expected_lighting_mult).abs() < 1e-12,
            "Lighting at 0.9 pu: expected {expected_lighting_mult}, got {lighting_kw}"
        );
        assert!(
            (tv_kw - 1.0).abs() < 1e-12,
            "TV (default) at 0.9 pu: should still be 1.0, got {tv_kw}"
        );
        assert!(
            lighting_kw < tv_kw,
            "Lighting (voltage-sensitive) should draw less than TV (constant-power) at reduced voltage"
        );
    }

    #[test]
    fn lighting_zip_produces_reactive_power_at_nominal_voltage() {
        // Lighting with type-specific defaults: pf=1.0, zq=0.46, iq=0.51, pq=0.03
        // At 1.0 pu: reactive_base = 0.46+0.51+0.03 = 1.0, reactive = real_kw * 1.0 * 1.0
        let config = config_with_schedule("s", "Lighting", &[2.0]);
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let env = base_env();
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots::default();
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert!((ports.electrical.net_active_kw() - 2.0).abs() < 1e-12);
        assert!(
            (ports.electrical.reactive_power_kvar - 2.0).abs() < 1e-12,
            "Lighting with pf=1.0 and reactive_base=1.0 should produce kvar == kW"
        );
    }
}
