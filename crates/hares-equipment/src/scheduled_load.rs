//! Scheduled (deterministic) load equipment model.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use chrono::Datelike;
use hares_physics::constants::GAS_THERMS_PER_HOUR_TO_W;
use hares_physics::units::power_kw_to_w;
use hares_types::{
    BoundaryPolicy, ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput,
    CorePerformance, CoreState, ElectricPower, EndUse, EnvironmentState, EquipmentDescriptor,
    EquipmentId, ExecutionStage, FuelPower, FuelType, HaresError, OperatingMode, PortContribution,
    PortDeclaration, PortSlots, ScheduleSource, Telemetry, TelemetryField, telemetry_keys as tk,
};
use serde::{Deserialize, Serialize};

use crate::config::constructor_equipment_id;
use crate::gain_fractions::{GainFractions, accumulate_zone_gain};
use crate::load_zone::resolve_load_zone;
use crate::raw_params::{ParamKind, RawParam};
use crate::schedule_helpers::{
    ScheduleSourceState, capture_schedule_source_state, parse_month_multipliers, parse_usize,
    parse_zone_id, restore_schedule_source_state,
};
use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_versioned, try_save_versioned};
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

/// The schedule parameters a scheduled load reads, beside the ones every
/// raw-parameter load reads ([`crate::raw_params::SCHEDULED_LOAD`]).
pub(crate) const SCHEDULE_PARAMS: &[RawParam] = &[
    RawParam::key(KEY_GAS_SCHEDULE_IS_W, ParamKind::Bool),
    RawParam::key(KEY_POWER_SCHEDULE_SOURCE, ParamKind::Text),
    RawParam::key(KEY_POWER_SCHEDULE_COL, ParamKind::Number),
    RawParam::key(KEY_POWER_PROFILE_MAX_KW, ParamKind::Number),
    RawParam::key(KEY_POWER_PROFILE_WEEKDAY, ParamKind::NumberList),
    RawParam::key(KEY_POWER_PROFILE_WEEKEND, ParamKind::NumberList),
    RawParam::key(KEY_POWER_PROFILE_MONTH, ParamKind::NumberList),
    RawParam::key(KEY_POWER_CONSTANT_KW, ParamKind::Number),
    RawParam::key(KEY_GAS_SCHEDULE_SOURCE, ParamKind::Text),
    RawParam::key(KEY_GAS_SCHEDULE_COL, ParamKind::Number),
    RawParam::key(KEY_GAS_PROFILE_MAX, ParamKind::Number),
    RawParam::key(KEY_GAS_PROFILE_WEEKDAY, ParamKind::NumberList),
    RawParam::key(KEY_GAS_PROFILE_WEEKEND, ParamKind::NumberList),
    RawParam::key(KEY_GAS_PROFILE_MONTH, ParamKind::NumberList),
    RawParam::key(KEY_GAS_CONSTANT, ParamKind::Number),
];

/// The classes registered as scheduled loads, each with its end use.
pub(crate) const CLASSES: &[(&str, EndUse)] = &[
    ("Lighting", EndUse::LIGHTING),
    ("Plug Loads", EndUse::PLUG_LOADS),
    ("Other", EndUse::OTHER),
    ("Refrigerator", EndUse::REFRIGERATION),
    ("Freezer", EndUse::REFRIGERATION),
    ("MELs", EndUse::PLUG_LOADS),
    ("TV", EndUse::PLUG_LOADS),
    ("Well Pump", EndUse::OTHER),
    ("Pool Pump", EndUse::POOL_PUMP),
    ("Pool Heater", EndUse::POOL_HEATER),
    ("Spa Pump", EndUse::SPA_PUMP),
    ("Spa Heater", EndUse::SPA_HEATER),
    ("Gas Grill", EndUse::COOKING),
    // Gas Fireplace has no direct HPXML EndUse equivalent: it is neither
    // cooking, heating (it is decorative) nor an HPXML 4.2 §3 appliance
    // category, so it stays OTHER until the data dictionary gains a
    // fireplace end use.
    ("Gas Fireplace", EndUse::OTHER),
    ("Gas Lighting", EndUse::LIGHTING),
    ("Ceiling Fan", EndUse::CEILING_FAN),
    ("Indoor Lighting", EndUse::LIGHTING),
    ("Exterior Lighting", EndUse::LIGHTING),
    ("Basement Lighting", EndUse::LIGHTING),
    ("Garage Lighting", EndUse::LIGHTING),
];

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
    gains: GainFractions,
    /// Full ZIP model (real + reactive) resolved via
    /// [`crate::config::resolve_zip`]; scheduled loads keep the full
    /// voltage-dependent real-power polynomial (OCHRE parity), so the
    /// resolved regime is governing.
    zip: hares_types::zip::ResolvedZip,
    /// Per-month scale factors [0..11] applied after load_fraction, e.g. a
    /// ceiling fan off in winter (divergence D-013).
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
        // `init` resolves the zone; until then only an explicit one is known.
        let zone = if end_use == EndUse::EV {
            None
        } else {
            parse_zone_id(&config)
        };

        let descriptor = EquipmentDescriptor {
            id: EquipmentId(constructor_equipment_id(&config)),
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
            zone_type: None,
        };
        Self {
            descriptor,
            ports: vec![PortDeclaration::electrical()],
            telemetry: default_telemetry(),
            core_output: CoreOutput::default(),
            gas_source: None,
            gas_schedule_unit: GasScheduleUnit::ThermsPerHour,
            gains: GainFractions::default(),
            zip: hares_types::zip::ResolvedZip::governing(
                hares_types::zip::zip_defaults_for_class(equipment_type)
                    .unwrap_or_else(hares_types::zip::ZipLoad::constant_power),
            ),
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
        env: &EnvironmentState,
    ) -> crate::Result<()> {
        self.power_source = parse_power_schedule_source(config)?;
        let (gas_source, gas_unit) = parse_optional_gas_schedule_source(config)?;
        self.gas_source = gas_source;
        self.gas_schedule_unit = if config.get_bool(KEY_GAS_SCHEDULE_IS_W).unwrap_or(false) {
            GasScheduleUnit::Watts
        } else {
            gas_unit
        };
        self.gains = GainFractions::from_config(config, &self.descriptor.name)?;
        self.zip = crate::config::resolve_zip(config);
        crate::config::validate_zip_sums(&self.zip, &self.descriptor.name)?;
        #[cfg(feature = "observe")]
        tracing::debug!(
            equipment_type = %self.descriptor.equipment_type,
            instance = %self.descriptor.name,
            zp = self.zip.zp,
            ip = self.zip.ip,
            pp = self.zip.pp,
            zq = self.zip.zq,
            iq = self.zip.iq,
            pq = self.zip.pq,
            pf = self.zip.pf,
            "resolved ZIP coefficients",
        );
        // A daily profile's own month factors are its shape; the load's month
        // multipliers scale whatever schedule it has on top, the profile
        // included. OCHRE only zeroes a month whose multiplier is 0
        // (divergence D-013).
        self.month_multipliers = parse_month_multipliers(config)?;
        let usage_multiplier = config
            .get_f64(crate::config::KEY_USAGE_MULTIPLIER)
            .unwrap_or(1.0);
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
        // pf = 0.0 is the "no reactive ZIP configured" sentinel; any nonzero
        // pf (including 1.0, which yields Q = 0 exactly) declares REACTIVE.
        let reactive_supported = self.zip.pf != 0.0;
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
        self.descriptor.zone = resolve_load_zone(
            config,
            &self.descriptor.end_use,
            self.gains.gives_zone_heat(),
            env,
        )?;
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

    fn rename(&mut self, name: String) {
        self.descriptor.name = name;
    }

    fn set_equipment_id(&mut self, id: EquipmentId) -> crate::Result<()> {
        crate::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
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
        // Grid outage (de-energized bus): all outputs are zero. Gas scheduled
        // loads are also zeroed — modern gas appliances (ranges, dryers,
        // fireplaces with electronic ignition) need electricity to operate.
        // Islanded (battery/generator-backed) homes keep an energized bus and
        // are not affected. See docs/outage-behavior.md.
        if !env.grid.bus_energized() {
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
                // Apply the month multiplier (D-013). Negative schedule
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
                    self.zip
                        .apply(scheduled_power_kw, env.grid.bus_voltage_pu())
                } else {
                    (0.0, 0.0)
                };
                (real_kw, reactive_kvar, gas_w)
            };

        #[cfg(feature = "observe")]
        {
            let tan_phi = self.zip.tan_phi();
            tracing::debug!(
                scheduled_load_reactive_kvar = reactive_power_kvar,
                scheduled_load_real_kw = electric_power_kw,
                scheduled_load_pf = self.zip.pf,
                scheduled_load_tan_phi = tan_phi,
                "scheduled load ZIP reactive power diagnostic",
            );
        }

        if electric_power_kw > 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_w: power_kw_to_w(electric_power_kw),
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

        let total_gain_source_w = power_kw_to_w(electric_power_kw) + gas_consumption_w;
        let gain = self.gains.of(total_gain_source_w);
        let total_sensible_w = total_gain_source_w * self.gains.sensible;
        let latent_gain_w = gain.latent_w;
        accumulate_zone_gain(ports, self.descriptor.zone, gain)?;

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

    fn resolved_zip(&self) -> Option<hares_types::zip::ResolvedZip> {
        Some(self.zip)
    }

    fn expected_mean_power_kw(&self) -> Option<crate::ExpectedMeanPower> {
        // Gas-only loads draw no real electric power (the electric schedule
        // is all zeros), so their expected electric draw is exactly zero
        // rather than schedule-deferred.
        if self.descriptor.fuel == FuelType::Gas {
            return Some(crate::ExpectedMeanPower::Kw(0.0));
        }
        match &self.power_source {
            ScheduleSource::ColumnRef { col_idx, .. } => {
                // `step()` applies the runtime month multipliers to column
                // draws identically to constant draws, so the published
                // expectation must not contradict them. Production column
                // sources carry no runtime multipliers — the resolve layer
                // bakes monthly scaling into the generated column values
                // — so the deferral is correct as-is. With runtime
                // multipliers present: all-zeroed draws nothing year-round
                // (zero); identity scaling changes nothing (deferral); any
                // other configuration (uniform non-identity, partial,
                // non-uniform) is honest over-degradation to `None` — the
                // exact value (column mean × scale, or month-weighted for
                // partial) is computable but not expressible in
                // `ExpectedMeanPower`, and an unscaled deferral would
                // weight the premise aggregate by a draw the equipment
                // cannot take.
                match self.month_multipliers {
                    None => Some(crate::ExpectedMeanPower::ScheduleColumn(*col_idx)),
                    Some(m) => {
                        let all_zero = m.iter().all(|v| *v == 0.0);
                        let all_one = m.iter().all(|v| *v == 1.0);
                        if all_zero {
                            Some(crate::ExpectedMeanPower::Kw(0.0))
                        } else if all_one {
                            Some(crate::ExpectedMeanPower::ScheduleColumn(*col_idx))
                        } else {
                            None
                        }
                    }
                }
            }
            source => {
                // `step()` scales every draw by the runtime month
                // multipliers (`raw * load_fraction * month_scale`), so the
                // published expectation must describe the same load: the
                // unscaled source mean would overstate a seasonal load's
                // premise weight by the inverse of the mean multiplier.
                // Equal-weight month averaging is the same convention
                // `ScheduleSource::mean()` uses for DailyProfile months; a
                // daily profile's own month factors take the multipliers
                // month by month, so the mean is that of their product.
                let mean = match (source, self.month_multipliers) {
                    (
                        ScheduleSource::DailyProfile {
                            weekday,
                            weekend,
                            month_multipliers,
                            max_value,
                        },
                        Some(scale),
                    ) => ScheduleSource::DailyProfile {
                        weekday: *weekday,
                        weekend: *weekend,
                        month_multipliers: std::array::from_fn(|m| month_multipliers[m] * scale[m]),
                        max_value: *max_value,
                    }
                    .mean(),
                    (source, scale) => {
                        source.mean() * scale.map_or(1.0, |m| m.iter().sum::<f64>() / 12.0)
                    }
                };
                // `ScheduleSource::mean()` has no finiteness guard (it is
                // used for planning hints, not stepping); a non-finite mean
                // must not be published as a weight-looking `Some` value.
                if mean.is_finite() {
                    Some(crate::ExpectedMeanPower::Kw(mean))
                } else {
                    None
                }
            }
        }
    }

    fn save_state(&self) -> crate::Result<Vec<u8>> {
        try_save_versioned(
            &ScheduledLoadState {
                last_non_zero_power_kw: self.last_non_zero_power_kw,
                last_non_zero_gas_w: self.last_non_zero_gas_w,
                last_reactive_power_kvar: self
                    .telemetry
                    .get(tk::REACTIVE_POWER_KVAR)
                    .unwrap_or(0.0),
                load_fraction: self.load_fraction,
                mode_override: self.mode_override,
                power_source_state: capture_schedule_source_state(&self.power_source),
                gas_source_state: self.gas_source.as_ref().map(capture_schedule_source_state),
            },
            Self::checkpoint_version(),
            "ScheduledLoad",
        )
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: ScheduledLoadState = load_versioned(
            state,
            Self::checkpoint_version(),
            "ScheduledLoad",
            self.descriptor().id,
        )?;
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
        let total_gain_source_w =
            power_kw_to_w(self.last_non_zero_power_kw) + self.last_non_zero_gas_w;
        self.telemetry.insert(
            tk::TOTAL_SENSIBLE_GAIN_W,
            total_gain_source_w * self.gains.sensible,
        );
        self.telemetry
            .insert(tk::LATENT_GAIN_W, total_gain_source_w * self.gains.latent);
        self.telemetry
            .insert(tk::FUEL_INPUT_W, self.last_non_zero_gas_w);
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(
                    self.last_non_zero_power_kw.max(0.0),
                )),
                reactive_power_kvar: self
                    .descriptor
                    .core_capabilities
                    .contains(CoreCapabilities::REACTIVE)
                    .then_some(decoded.last_reactive_power_kvar),
                fuel_w: self
                    .descriptor
                    .core_capabilities
                    .contains(CoreCapabilities::FUEL)
                    .then_some(FuelPower {
                        fuel_type: FuelType::Gas,
                        consumption_w: self.last_non_zero_gas_w.max(0.0),
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

    fn apply_signal(&mut self, signal: &ControlSignal) -> crate::Result<()> {
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
    for (class, end_use) in CLASSES {
        registry.register(
            *class,
            Box::new(move |config| Box::new(ScheduledLoad::new(config, end_use.clone(), class))),
        );
    }
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
                if let (Some(lo), Some(hi)) = (&mut w.min_value, &mut w.max_value)
                    && *lo > *hi
                {
                    std::mem::swap(lo, hi);
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

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc, time::Duration};

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        BoundaryPolicy, ControlSignal, DomainUpdate, EnvironmentState, FuelType, GridState,
        PortSlots, SCHEDULE_DOMAIN_ID, ScheduleSource, TelemetryField, WeatherState, ZoneId,
        ZoneMap, ZoneRole, ZoneState,
    };
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    use hares_types::telemetry_keys as tk;

    use hares_types::zip::{ResolvedZip, ZipLoad};

    use super::{
        GAS_THERMS_PER_HOUR_TO_W, KEY_GAS_CONSTANT, KEY_GAS_SCHEDULE_IS_W, KEY_GAS_SCHEDULE_SOURCE,
        KEY_POWER_CONSTANT_KW, KEY_POWER_SCHEDULE_COL, KEY_POWER_SCHEDULE_SOURCE, ScheduledLoad,
    };
    use crate::gain_fractions::{
        KEY_LATENT as KEY_LATENT_GAIN_FRACTION, KEY_RADIANT as KEY_RADIATIVE_GAIN_FRACTION,
        KEY_SENSIBLE as KEY_SENSIBLE_GAIN_FRACTION,
    };

    use crate::schedule_helpers::KEY_MONTH_MULTIPLIER_PREFIX;
    use crate::{Equipment, EquipmentConfig, EquipmentRegistry};

    fn base_env() -> EnvironmentState {
        EnvironmentState {
            ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
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
                island_bus_voltage_pu: None,
            },
            schedule_row: None,
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

    /// A ZIP sidecar with the given real-power polynomial, no reactive
    /// component (pf = 0 sentinel), and unity reactive base.
    fn real_only_zip(zp: f64, ip: f64, pp: f64) -> ZipLoad {
        ZipLoad {
            zp,
            ip,
            pp,
            ..ZipLoad::constant_power()
        }
    }

    #[test]
    fn init_rejects_invalid_zip_sum() {
        let mut config = config_with_extras(
            "s",
            "Lighting",
            &[1.0],
            &[(KEY_SENSIBLE_GAIN_FRACTION, 0.0.into())],
        );
        config.zip = Some(real_only_zip(0.2, 0.2, 0.2)); // sum = 0.6
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let err = eq.init(&config, &base_env()).unwrap_err();
        assert!(err.to_string().contains("invalid ZIP coefficients"));
    }

    #[test]
    fn zip_voltage_uses_env_grid_voltage() {
        let mut config = config_with_extras(
            "s",
            "Lighting",
            &[2.0],
            &[(KEY_SENSIBLE_GAIN_FRACTION, 0.0.into())],
        );
        config.zip = Some(real_only_zip(0.2, 0.3, 0.5));
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
        let expected_w = 2000.0 * expected_multiplier;
        assert!((ports.electrical.net_active_w() - expected_w).abs() < 1.0);
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
        assert_eq!(ports.electrical.net_active_w(), 0.0);
        assert_eq!(ports.thermal[0].sensible_gain_w, 0.0);

        env.current_time += ChronoDuration::minutes(15);
        ports.zero();
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert_eq!(ports.electrical.net_active_w(), 0.0);
    }

    #[test]
    fn load_fraction_rejects_out_of_domain_on_unchecked_path() {
        // The arm clamps only at zero (`fraction.max(0.0)`): a fraction
        // above 1 is stored raw and multiplies the schedule power directly
        // (5.0 → five times the load), while a negative or NaN fraction is
        // silently resolved to 0.0 (f64::max ignores NaN) — a load that
        // quietly turns off. The central validator rejects all of these on
        // the checked path; the arm must not silently rewrite them.
        let config = config_with_schedule("s", "Lighting", &[3.0, 3.0]);
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let env = base_env();
        eq.init(&config, &env).unwrap();

        for bad in [5.0, -0.5, f64::NAN] {
            let err = eq
                .apply_control_unchecked(&ControlSignal::LoadFraction { fraction: bad })
                .expect_err("out-of-domain LoadFraction must be rejected");
            assert!(
                format!("{err:?}").to_lowercase().contains("fraction"),
                "error must name the signal for {bad}, got {err:?}"
            );
        }
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
            min_soc: None,
            max_soc: None,
        })
        .unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert!(
            (ports.electrical.net_active_w() - 5000.0).abs() < 10.0,
            "step 1 should use setpoint 5.0"
        );

        // No new PowerSetpoint issued -- next step reverts to schedule value.
        env.current_time += ChronoDuration::minutes(15);
        ports.zero();
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert!(
            (ports.electrical.net_active_w() - 3000.0).abs() < 10.0,
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
        assert!((ports.electrical.net_active_w() - 1000.0).abs() < 10.0);
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
        assert_eq!(ports.electrical.net_active_w(), 0.0);
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
        assert_eq!(ports.electrical.net_active_w(), 0.0);
        assert_eq!(ports.thermal[0].sensible_gain_w, 0.0);
    }

    /// An islanded home (utility out, backup source holding the bus at
    /// nominal) keeps its scheduled loads running at nominal power.
    #[test]
    fn islanded_home_keeps_scheduled_load_running() {
        let config = config_with_schedule("s", "Lighting", &[3.0]);
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let mut env = base_env();
        eq.init(&config, &env).unwrap();
        env.grid.voltage_pu = 0.0;
        env.grid.island_bus_voltage_pu = Some(1.0);

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert!(
            (ports.electrical.net_active_w() - 3_000.0).abs() < 10.0,
            "islanded bus at nominal voltage powers the load at schedule power, got {}",
            ports.electrical.net_active_w()
        );
    }

    #[test]
    fn load_fraction_state_round_trip() {
        let config = config_with_schedule("s", "Lighting", &[2.0]);
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let env = base_env();
        eq.init(&config, &env).unwrap();
        eq.apply_control(&ControlSignal::LoadFraction { fraction: 0.7 })
            .unwrap();
        let state = eq.save_state().unwrap();

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
        assert!((ports.electrical.net_active_w() - 1400.0).abs() < 10.0);
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
        assert!(ports.electrical.net_active_w() > 0.0);
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
                (KEY_GAS_SCHEDULE_IS_W, true.into()),
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
        let state = eq.save_state().unwrap();

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
        assert!((ports.electrical.net_active_w() - 7250.0).abs() < 10.0);
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
        let checkpoint = a.save_state().unwrap();

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
                (ports_a.electrical.net_active_w() - ports_b.electrical.net_active_w()).abs()
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

    /// Over the static registry: every template with a dedicated standard
    /// end-use constant must not register as `EndUse::OTHER`, so energy
    /// disaggregates by HPXML-aligned end use. This replaced the gated
    /// construction warn (`check_other_end_use_for_known_template`): the
    /// registry is static, so the property is a unit test. The documented
    /// OTHER users are pinned: the generic "Other" template, "Well Pump"
    /// (no dedicated constant) and "Gas Fireplace" (no HPXML end use).
    #[test]
    fn registry_templates_with_dedicated_end_use_do_not_use_other() {
        use crate::config::ConfigValue;
        use hares_types::EndUse as Eu;

        let registry = EquipmentRegistry::new();
        let config = |name: &str| {
            let mut raw: HashMap<String, ConfigValue> = HashMap::new();
            raw.insert(KEY_POWER_SCHEDULE_SOURCE.to_string(), "constant".into());
            raw.insert(KEY_POWER_CONSTANT_KW.to_string(), 1.0.into());
            raw.insert(KEY_SENSIBLE_GAIN_FRACTION.to_string(), 1.0.into());
            EquipmentConfig::raw(name.to_string(), name.to_string(), raw)
        };
        let end_use_of = |name: &str| {
            let factory = registry.get(name).expect(name);
            let eq = factory(config(name));
            eq.descriptor().end_use.clone()
        };

        assert_ne!(end_use_of("Refrigerator"), Eu::OTHER);
        assert_ne!(end_use_of("Freezer"), Eu::OTHER);
        assert_ne!(end_use_of("MELs"), Eu::OTHER);
        assert_ne!(end_use_of("TV"), Eu::OTHER);
        assert_ne!(end_use_of("Pool Pump"), Eu::OTHER);
        assert_ne!(end_use_of("Pool Heater"), Eu::OTHER);
        assert_ne!(end_use_of("Spa Pump"), Eu::OTHER);
        assert_ne!(end_use_of("Spa Heater"), Eu::OTHER);
        assert_ne!(end_use_of("Gas Grill"), Eu::OTHER);
        assert_ne!(end_use_of("Ceiling Fan"), Eu::OTHER);
        assert_ne!(end_use_of("Gas Lighting"), Eu::OTHER);
        assert_ne!(end_use_of("Lighting"), Eu::OTHER);
        assert_ne!(end_use_of("Plug Loads"), Eu::OTHER);

        assert_eq!(end_use_of("Other"), Eu::OTHER);
        assert_eq!(end_use_of("Well Pump"), Eu::OTHER);
        assert_eq!(end_use_of("Gas Fireplace"), Eu::OTHER);
    }

    #[test]
    fn pool_pump_template_uses_pool_pump_end_use() {
        let config = config_with_extras("pp", "Pool Pump", &[1.0], &[]);
        let eq = ScheduledLoad::new(config, hares_types::EndUse::POOL_PUMP, "Pool Pump");
        assert_eq!(eq.descriptor().end_use, hares_types::EndUse::POOL_PUMP);
        assert_ne!(eq.descriptor().end_use, hares_types::EndUse::OTHER);
    }

    #[test]
    fn pool_heater_template_uses_pool_heater_end_use() {
        let config = config_with_extras("ph", "Pool Heater", &[1.0], &[]);
        let eq = ScheduledLoad::new(config, hares_types::EndUse::POOL_HEATER, "Pool Heater");
        assert_eq!(eq.descriptor().end_use, hares_types::EndUse::POOL_HEATER);
        assert_ne!(eq.descriptor().end_use, hares_types::EndUse::OTHER);
    }

    #[test]
    fn spa_pump_template_uses_spa_pump_end_use() {
        let config = config_with_extras("sp", "Spa Pump", &[1.0], &[]);
        let eq = ScheduledLoad::new(config, hares_types::EndUse::SPA_PUMP, "Spa Pump");
        assert_eq!(eq.descriptor().end_use, hares_types::EndUse::SPA_PUMP);
        assert_ne!(eq.descriptor().end_use, hares_types::EndUse::OTHER);
    }

    #[test]
    fn spa_heater_template_uses_spa_heater_end_use() {
        let config = config_with_extras("sh", "Spa Heater", &[1.0], &[]);
        let eq = ScheduledLoad::new(config, hares_types::EndUse::SPA_HEATER, "Spa Heater");
        assert_eq!(eq.descriptor().end_use, hares_types::EndUse::SPA_HEATER);
        assert_ne!(eq.descriptor().end_use, hares_types::EndUse::OTHER);
    }

    #[test]
    fn gas_grill_template_uses_cooking_end_use() {
        let config = config_with_extras("gg", "Gas Grill", &[1.0], &[]);
        let eq = ScheduledLoad::new(config, hares_types::EndUse::COOKING, "Gas Grill");
        assert_eq!(eq.descriptor().end_use, hares_types::EndUse::COOKING);
        assert_ne!(eq.descriptor().end_use, hares_types::EndUse::OTHER);
    }

    #[test]
    fn ceiling_fan_template_uses_ceiling_fan_end_use() {
        let config = config_with_extras("cf", "Ceiling Fan", &[1.0], &[]);
        let eq = ScheduledLoad::new(config, hares_types::EndUse::CEILING_FAN, "Ceiling Fan");
        assert_eq!(eq.descriptor().end_use, hares_types::EndUse::CEILING_FAN);
        assert_ne!(eq.descriptor().end_use, hares_types::EndUse::VENTILATION);
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
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::COOKING, "Gas Grill");
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
            hares_types::EndUse::CEILING_FAN,
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
            ports.electrical.net_active_w(),
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
            ports.electrical.net_active_w() > 0.0,
            "March output should be non-zero"
        );
    }

    #[test]
    fn an_absolute_radiative_fraction_leaves_the_rest_of_sensible_convective() {
        let config = config_with_extras(
            "s",
            "Lighting",
            &[1.0],
            &[
                (KEY_SENSIBLE_GAIN_FRACTION, 0.5.into()),
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
        let mut config = config_with_extras(
            "s",
            "Lighting",
            &[2.0],
            &[(KEY_SENSIBLE_GAIN_FRACTION, 0.0.into())],
        );
        config.zip = Some(ZipLoad {
            v0: 0.95,
            ..real_only_zip(0.2, 0.3, 0.5)
        });
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
        assert!((ports.electrical.net_active_w() - 2000.0).abs() < 10.0);
    }

    #[test]
    fn exterior_equipment_has_no_zone_assignment() {
        let config = config_no_zone("Exterior Lighting", "Exterior Lighting", &[1.0]);
        let eq = ScheduledLoad::new(
            config.clone(),
            hares_types::EndUse::LIGHTING,
            "Exterior Lighting",
        );
        assert!(
            eq.descriptor().zone.is_none(),
            "Exterior equipment without explicit zone_id should have no zone"
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
        // pf=0.8 → tan(acos(0.8)) = 0.75; reactive = real_kw * 0.75 * 1.0.
        let mut config = config_with_extras(
            "s",
            "Lighting",
            &[2.0],
            &[(KEY_SENSIBLE_GAIN_FRACTION, 0.0.into())],
        );
        config.zip = Some(ZipLoad::reactive_only(0.0, 0.0, 1.0, 0.8));
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let env = base_env();
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        // real_kw = 2.0 (all-P load at nominal voltage); reactive = 2.0 * 0.75 * 1.0 = 1.5
        assert!((ports.electrical.reactive_power_kvar - 1.5).abs() < 1e-12);
    }

    #[test]
    fn reactive_zip_default_produces_zero_kvar() {
        // A class with no ZIP defaults falls back to constant_power()
        // (pf=0.0 sentinel), so reactive_power=0. "TV" used to be the
        // example here, but it now has a class-table row (MELs values), so
        // use a name that is genuinely absent from the table.
        let config = config_with_extras(
            "s",
            "Custom Bench Load",
            &[3.0],
            &[(KEY_SENSIBLE_GAIN_FRACTION, 0.0.into())],
        );
        let mut eq = ScheduledLoad::new(
            config.clone(),
            hares_types::EndUse::PLUG_LOADS,
            "Custom Bench Load",
        );
        let env = base_env();
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots::default();
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert_eq!(ports.electrical.reactive_power_kvar, 0.0);
    }

    #[test]
    fn tv_class_defaults_produce_nonzero_reactive() {
        // "TV" (HPXML PlugLoadType="TV other", split out of plug loads) has
        // a class-table row inheriting the MELs values (pf 0.8), so a
        // running TV must emit reactive power — regression test for the
        // gap where TV fell to constant_power() and Q ≡ 0 despite a
        // continuous draw.
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
        // At nominal voltage the MELs reactive base sums to ~1, so
        // Q ≈ P · tan(acos(0.8)).
        let expected = 3.0 * 0.8_f64.acos().tan();
        assert!(
            (ports.electrical.reactive_power_kvar - expected).abs() < 1e-9,
            "TV reactive = {}, expected ≈ {expected}",
            ports.electrical.reactive_power_kvar
        );
    }

    #[test]
    fn reactive_zip_voltage_sensitivity() {
        // zq=1, iq=0, pq=0 → reactive_base = v_norm².
        // pf=0.9 → tan(acos(0.9)) ≈ 0.4843; reactive = real_kw * 0.4843 * v_norm².
        // Pure-P real ZIP (zp=0, ip=0, pp=1) keeps real_kw independent of voltage.
        let mut config = config_with_extras(
            "s",
            "Lighting",
            &[1.0],
            &[(KEY_SENSIBLE_GAIN_FRACTION, 0.0.into())],
        );
        config.zip = Some(ZipLoad::reactive_only(1.0, 0.0, 0.0, 0.9));
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let mut env = base_env();
        env.grid.voltage_pu = 0.9;
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots::default();
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        let v_norm = 0.9_f64;
        // real_kw = 1.0 (pure-P real ZIP); reactive = 1.0 * tan(acos(0.9)) * v_norm²
        let tan_phi = 0.9_f64.clamp(-1.0, 1.0).acos().tan();
        let expected_kvar = tan_phi * v_norm * v_norm;
        assert!(
            (ports.electrical.reactive_power_kvar - expected_kvar).abs() < 1e-12,
            "reactive={} expected={expected_kvar}",
            ports.electrical.reactive_power_kvar
        );
    }

    #[test]
    fn pf_1_0_produces_zero_reactive_power_at_nominal_voltage() {
        // pf=1.0 → tan(acos(1.0)) = 0.0 → reactive power must be zero.
        // Regression: before the fix, pf was treated as a raw multiplier,
        // so pf=1.0 produced reactive = real * 1.0 instead of 0.0.
        let mut config = config_with_extras(
            "s",
            "Lighting",
            &[5.0],
            &[(KEY_SENSIBLE_GAIN_FRACTION, 0.0.into())],
        );
        config.zip = Some(ZipLoad::reactive_only(0.0, 0.0, 1.0, 1.0));
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let env = base_env();
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots::default();
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert!(
            ports.electrical.reactive_power_kvar.abs() < 1e-12,
            "pf=1.0 should produce zero reactive power, got {}",
            ports.electrical.reactive_power_kvar
        );
    }

    #[test]
    fn pf_0_99_produces_correct_reactive_power_ratio() {
        // Regression: before the fix, pf=0.99 produced reactive ≈ 0.99× real
        // (a 7× overestimate). The correct ratio is tan(acos(0.99)) ≈ 0.1425.
        let zip = ZipLoad::reactive_only(0.0, 0.0, 1.0, 0.99);
        let p_kw = 4.0;
        let (_real_kw, reactive_kvar) = zip.apply(p_kw, 1.0);
        let tan_phi = 0.99_f64.clamp(-1.0, 1.0).acos().tan();
        let expected_reactive = p_kw * tan_phi;
        assert!(
            (reactive_kvar - expected_reactive).abs() < 1e-12,
            "pf=0.99: expected reactive {expected_reactive} ≈ 0.1425× real, got {reactive_kvar}"
        );
        // Confirm the bug is fixed: reactive should be ~14% of real, not ~99%.
        let ratio = reactive_kvar / p_kw;
        assert!(
            ratio < 0.20,
            "pf=0.99: reactive/real ratio {ratio} should be < 0.20 (was 0.99 before fix)"
        );
    }

    #[test]
    fn pf_1_0_produces_zero_reactive_at_any_voltage() {
        // pf=1.0 → tan(acos(1.0)) = 0.0 → reactive zero regardless of voltage.
        for voltage_pu in [0.8, 0.9, 0.95, 1.0, 1.05] {
            let zip = ZipLoad {
                zp: 0.2,
                ip: 0.3,
                pp: 0.5,
                v0: 1.0,
                zq: 0.15,
                iq: 0.35,
                pq: 0.5,
                pf: 1.0,
            };
            let (_real_kw, reactive_kvar) = zip.apply(3.0, voltage_pu);
            assert!(
                reactive_kvar.abs() < 1e-12,
                "pf=1.0 at v={voltage_pu}: reactive should be 0, got {reactive_kvar}"
            );
        }
    }

    #[test]
    fn reactive_zip_invalid_sum_is_rejected() {
        let mut config = config_with_extras(
            "s",
            "Lighting",
            &[1.0],
            &[(KEY_SENSIBLE_GAIN_FRACTION, 0.0.into())],
        );
        config.zip = Some(ZipLoad::reactive_only(0.3, 0.3, 0.3, 0.9)); // reactive sum = 0.9
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let err = eq.init(&config, &base_env()).unwrap_err();
        assert!(
            err.to_string().contains("reactive ZIP coefficients"),
            "expected reactive ZIP validation error, got: {err}"
        );
    }

    /// Bit-identity regression for the ZIP migration: stepping a
    /// ScheduledLoad through the canonical `resolve_zip` path must produce
    /// (P, Q) bit-identical to the legacy `ZipCoefficients::apply`
    /// arithmetic, whose table constants are pinned literally here.
    #[test]
    fn migrated_zip_path_is_bit_identical_to_legacy_class_table() {
        // (class, z, i, p, zq, iq, pq, pf) rows copied verbatim from the
        // deleted `zip_coefficients_from_class` table.
        let legacy_rows: &[(&str, [f64; 7])] = &[
            ("Lighting", [0.54, 0.5, -0.04, 0.46, 0.51, 0.03, 1.0]),
            (
                "Refrigerator",
                [5.03, -8.48, 4.45, 17.44, -28.62, 12.18, 0.8],
            ),
            (
                "MELs",
                [
                    0.361_47, -0.085_83, 0.724_36, 8.399_85, -14.167_7, 6.767_85, 0.8,
                ],
            ),
            ("Well Pump", [0.72, -0.98, 1.26, 14.78, -23.71, 9.93, 0.84]),
            ("Ceiling Fan", [0.26, 0.9, -0.16, 0.5, 0.62, -0.12, 0.87]),
        ];
        let schedule_kw = 2.375;
        for (class, row) in legacy_rows {
            for voltage_pu in [0.9, 1.0, 1.1] {
                let config = config_with_extras(
                    "s",
                    class,
                    &[schedule_kw],
                    &[(KEY_SENSIBLE_GAIN_FRACTION, 0.0.into())],
                );
                let mut eq =
                    ScheduledLoad::new(config.clone(), hares_types::EndUse::OTHER, "Other");
                let mut env = base_env();
                env.grid.voltage_pu = voltage_pu;
                eq.init(&config, &env).unwrap();

                let mut ports = PortSlots {
                    thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
                    ..PortSlots::default()
                };
                eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();

                // Legacy ZipCoefficients::apply arithmetic, expression for
                // expression (scheduled_load.rs pre-migration).
                let [z, i, p_coeff, zq, iq, pq, pf] = *row;
                let v_norm = voltage_pu / 1.0;
                let zip_multiplier = z * v_norm * v_norm + i * v_norm + p_coeff;
                let expected_real_kw = schedule_kw * zip_multiplier;
                let reactive_base = zq * v_norm * v_norm + iq * v_norm + pq;
                let expected_reactive = if pf.abs() < 1e-9 {
                    0.0
                } else {
                    let tan_phi = pf.clamp(-1.0, 1.0).acos().tan();
                    expected_real_kw * tan_phi * reactive_base
                };
                let expected_active_w = hares_physics::units::power_kw_to_w(expected_real_kw);

                assert_eq!(
                    ports.electrical.load_power_w.to_bits(),
                    expected_active_w.to_bits(),
                    "{class} at v={voltage_pu}: active power diverged from legacy \
                     ({} vs {expected_active_w})",
                    ports.electrical.load_power_w
                );
                assert_eq!(
                    ports.electrical.reactive_power_kvar.to_bits(),
                    expected_reactive.to_bits(),
                    "{class} at v={voltage_pu}: reactive power diverged from legacy \
                     ({} vs {expected_reactive})",
                    ports.electrical.reactive_power_kvar
                );
            }
        }
    }

    /// A load that gives heat to a zone must have that zone: garage
    /// lighting in a building with no garage zone fails init naming the
    /// load and the missing role, instead of dropping its heat.
    #[test]
    fn heat_giving_load_errors_when_its_zone_role_is_missing() {
        let mut config = base_config_for_zone_test("Garage Lighting", &[1.0]);
        config
            .test_extras_mut()
            .insert(KEY_SENSIBLE_GAIN_FRACTION.to_string(), 0.5.into());
        let mut zone_map = ZoneMap::new();
        zone_map.insert(ZoneRole::Indoor, ZoneId(1));
        config.zone_map = Some(zone_map);
        let mut eq = ScheduledLoad::new(
            config.clone(),
            hares_types::EndUse::LIGHTING,
            "Garage Lighting",
        );
        let err = eq
            .init(&config, &base_env())
            .expect_err("garage lighting with gains and no garage zone must fail init");
        let message = err.to_string();
        assert!(
            message.contains("Garage Lighting") && message.contains("Garage"),
            "the error must name the load and its zone role, got: {message}"
        );
    }

    /// With no zone_id and no dwelling zone map a heat-giving load has no
    /// zone to heat.
    #[test]
    fn heat_giving_load_errors_without_a_zone_or_zone_map() {
        let mut config = base_config_for_zone_test("Refrigerator", &[1.0]);
        config
            .test_extras_mut()
            .insert(KEY_SENSIBLE_GAIN_FRACTION.to_string(), 0.5.into());
        let mut eq = ScheduledLoad::new(
            config.clone(),
            hares_types::EndUse::REFRIGERATION,
            "Refrigerator",
        );
        eq.init(&config, &base_env())
            .expect_err("a heat-giving load with no zone must fail init");
    }

    /// A load that gives no heat to any zone needs none.
    #[test]
    fn load_without_zone_gains_needs_no_zone() {
        let mut config = base_config_for_zone_test("Garage Lighting", &[1.0]);
        config
            .test_extras_mut()
            .insert(KEY_SENSIBLE_GAIN_FRACTION.to_string(), 0.0.into());
        let mut zone_map = ZoneMap::new();
        zone_map.insert(ZoneRole::Indoor, ZoneId(1));
        config.zone_map = Some(zone_map);
        let mut eq = ScheduledLoad::new(
            config.clone(),
            hares_types::EndUse::LIGHTING,
            "Garage Lighting",
        );
        eq.init(&config, &base_env()).unwrap();
        assert_eq!(eq.descriptor().zone, None);
    }

    #[test]
    fn garage_name_auto_routes_to_garage_zone() {
        let mut config = base_config_for_zone_test("Garage Lighting", &[1.0]);
        config
            .test_extras_mut()
            .insert(KEY_SENSIBLE_GAIN_FRACTION.to_string(), 0.0.into());
        let mut zone_map = ZoneMap::new();
        zone_map.insert(ZoneRole::Indoor, ZoneId(1));
        zone_map.insert(ZoneRole::Garage, ZoneId(3));
        config.zone_map = Some(zone_map);
        let mut eq = ScheduledLoad::new(
            config.clone(),
            hares_types::EndUse::LIGHTING,
            "Garage Lighting",
        );
        eq.init(&config, &env_with_zone(ZoneId(3))).unwrap();
        assert_eq!(
            eq.descriptor().zone,
            Some(ZoneId(3)),
            "Garage equipment should auto-route via ZoneMap to ZoneId(3)"
        );
    }

    #[test]
    fn basement_name_auto_routes_to_basement_zone() {
        let mut config = base_config_for_zone_test("Basement Lighting", &[1.0]);
        config
            .test_extras_mut()
            .insert(KEY_SENSIBLE_GAIN_FRACTION.to_string(), 0.0.into());
        let mut zone_map = ZoneMap::new();
        zone_map.insert(ZoneRole::Indoor, ZoneId(1));
        zone_map.insert(ZoneRole::Basement, ZoneId(4));
        zone_map.insert(ZoneRole::Crawlspace, ZoneId(4));
        config.zone_map = Some(zone_map);
        let mut eq = ScheduledLoad::new(
            config.clone(),
            hares_types::EndUse::LIGHTING,
            "Basement Lighting",
        );
        eq.init(&config, &env_with_zone(ZoneId(4))).unwrap();
        assert_eq!(
            eq.descriptor().zone,
            Some(ZoneId(4)),
            "Basement equipment should auto-route via ZoneMap to ZoneId(4)"
        );
    }

    #[test]
    fn indoor_equipment_defaults_to_indoor_zone_via_zone_map() {
        let mut config = base_config_for_zone_test("Refrigerator", &[1.0]);
        config
            .test_extras_mut()
            .insert(KEY_SENSIBLE_GAIN_FRACTION.to_string(), 0.0.into());
        let mut zone_map = ZoneMap::new();
        zone_map.insert(ZoneRole::Indoor, ZoneId(5));
        config.zone_map = Some(zone_map);
        let mut eq = ScheduledLoad::new(
            config.clone(),
            hares_types::EndUse::REFRIGERATION,
            "Refrigerator",
        );
        eq.init(&config, &env_with_zone(ZoneId(5))).unwrap();
        assert_eq!(
            eq.descriptor().zone,
            Some(ZoneId(5)),
            "Indoor equipment should resolve to ZoneMap Indoor role (ZoneId(5))"
        );
    }

    /// A zone map entry the environment does not hold would drop the load's
    /// heat every step: init fails naming the load.
    #[test]
    fn zone_map_zone_missing_from_the_environment_fails_init() {
        let mut config = base_config_for_zone_test("Refrigerator", &[1.0]);
        config
            .test_extras_mut()
            .insert(KEY_SENSIBLE_GAIN_FRACTION.to_string(), 1.0.into());
        let mut zone_map = ZoneMap::new();
        zone_map.insert(ZoneRole::Indoor, ZoneId(5));
        config.zone_map = Some(zone_map);
        let mut eq = ScheduledLoad::new(
            config.clone(),
            hares_types::EndUse::REFRIGERATION,
            "Refrigerator",
        );
        let err = eq
            .init(&config, &base_env())
            .expect_err("a zone the environment lacks must fail init");
        assert!(err.to_string().contains("Refrigerator"), "got: {err}");
    }

    fn env_with_zone(zone: ZoneId) -> EnvironmentState {
        let mut env = base_env();
        let mut extra = env.zones[0].clone();
        extra.id = zone;
        env.zones.push(extra);
        env
    }

    #[test]
    fn zone_map_missing_role_leaves_zone_none() {
        let mut config = base_config_for_zone_test("Garage Lighting", &[1.0]);
        config
            .test_extras_mut()
            .insert(KEY_SENSIBLE_GAIN_FRACTION.to_string(), 0.0.into());
        let mut zone_map = ZoneMap::new();
        zone_map.insert(ZoneRole::Indoor, ZoneId(1));
        // No Garage entry — zone should remain None.
        config.zone_map = Some(zone_map);
        let mut eq = ScheduledLoad::new(
            config.clone(),
            hares_types::EndUse::LIGHTING,
            "Garage Lighting",
        );
        eq.init(&config, &base_env()).unwrap();
        assert!(
            eq.descriptor().zone.is_none(),
            "Equipment should have no zone when ZoneMap lacks the matching role"
        );
    }

    /// Regression test for Outdoor/Exterior equipment routed via ZoneMap.
    /// Without the outdoor/exclusion guard in resolve_load_zone(), outdoor-named
    /// equipment would be routed to the indoor zone because it doesn't match any
    /// specific role keyword.
    #[test]
    fn outdoor_equipment_ignored_by_zone_map() {
        let mut config = base_config_for_zone_test("Exterior Lighting", &[1.0]);
        config
            .test_extras_mut()
            .insert(KEY_SENSIBLE_GAIN_FRACTION.to_string(), 0.0.into());
        let mut zone_map = ZoneMap::new();
        zone_map.insert(ZoneRole::Indoor, ZoneId(1));
        config.zone_map = Some(zone_map);
        let mut eq = ScheduledLoad::new(
            config.clone(),
            hares_types::EndUse::LIGHTING,
            "Exterior Lighting",
        );
        eq.init(&config, &base_env()).unwrap();
        assert!(
            eq.descriptor().zone.is_none(),
            "Exterior equipment should have no zone even when ZoneMap is present"
        );
    }

    /// Regression test: EV equipment must not receive a zone from ZoneMap.
    /// EV charging occurs outside the building envelope; the end-use-based
    /// exclusion in resolve_zone_from_map() must prevent ZoneMap routing.
    #[test]
    fn ev_equipment_ignored_by_zone_map() {
        let mut config = base_config_for_zone_test("Scheduled EV", &[3.5]);
        config
            .test_extras_mut()
            .insert(KEY_SENSIBLE_GAIN_FRACTION.to_string(), 0.0.into());
        let mut zone_map = ZoneMap::new();
        zone_map.insert(ZoneRole::Indoor, ZoneId(1));
        config.zone_map = Some(zone_map);
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::EV, "Scheduled EV");
        eq.init(&config, &base_env()).unwrap();
        assert!(
            eq.descriptor().zone.is_none(),
            "EV equipment must have no zone even when ZoneMap is present"
        );
    }

    /// Creates a minimal `EquipmentConfig` without a `zone_id` key for
    /// ZoneMap-based routing tests.
    fn base_config_for_zone_test(name: &str, schedule: &[f64]) -> EquipmentConfig {
        let mut raw: HashMap<String, crate::config::ConfigValue> = HashMap::new();
        raw.insert(KEY_POWER_SCHEDULE_SOURCE.to_string(), "constant".into());
        raw.insert(
            KEY_POWER_CONSTANT_KW.to_string(),
            schedule.first().copied().unwrap_or(0.0).into(),
        );
        EquipmentConfig::raw(name.to_string(), name.to_string(), raw)
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
    fn explicit_zone_id_overrides_outdoor_name_routing() {
        let config = config_with_schedule("Outdoor Sauna Heater", "Other", &[1.0]);
        let eq = ScheduledLoad::new(config, hares_types::EndUse::OTHER, "Other");
        assert_eq!(
            eq.descriptor().zone,
            Some(ZoneId(1)),
            "Explicit zone_id should override 'outdoor' name-based routing"
        );
    }

    #[test]
    fn outdoor_name_without_explicit_zone_has_no_zone() {
        let config = config_no_zone("Outdoor Light", "Other", &[1.0]);
        let eq = ScheduledLoad::new(config, hares_types::EndUse::OTHER, "Other");
        assert!(
            eq.descriptor().zone.is_none(),
            "Outdoor name without explicit zone_id should produce zone=None"
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

    // --- Reactive power telemetry tests ---

    #[test]
    fn zip_reactive_power_emitted_to_telemetry() {
        // Iq=0.8 (pure current reactive term), pq=0.2, zq=0, pf=0.9 at nominal voltage v=1.0.
        // reactive_base = zq*v² + iq*v + pq = 0.0 + 0.8*1.0 + 0.2 = 1.0
        // pf=0.9 → tan(acos(0.9)) ≈ 0.4843; reactive_kvar = real_kw * 0.4843 * reactive_base
        let mut config = config_with_extras(
            "s",
            "Lighting",
            &[2.0],
            &[(KEY_SENSIBLE_GAIN_FRACTION, 0.0.into())],
        );
        config.zip = Some(ZipLoad::reactive_only(0.0, 0.8, 0.2, 0.9));
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let env = base_env();
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots::default();
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();

        let tan_phi = 0.9_f64.clamp(-1.0, 1.0).acos().tan();
        let expected_kvar = 2.0_f64 * tan_phi * (0.0 + 0.8 * 1.0 + 0.2);
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
        // A class absent from the ZIP table falls back to constant_power()
        // (pf=0.0 sentinel), so reactive power is always zero. ("TV" used to
        // be the example, but it now has a class-table row.)
        let config = config_with_schedule("s", "Custom Bench Load", &[5.0]);
        let mut eq = ScheduledLoad::new(
            config.clone(),
            hares_types::EndUse::PLUG_LOADS,
            "Custom Bench Load",
        );
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
    fn nan_schedule_value_must_not_degrade_to_silent_zero_draw() {
        // A NaN schedule value is concretely reachable: the CSV loader
        // (`hares-io/src/schedule.rs`) parses with `str::parse::<f64>()`,
        // which accepts "nan", with no is_nan/is_finite check at that
        // boundary. At the consumption site below, `raw_schedule_kw > 0.0`
        // is false for NaN, so corrupted data silently becomes "no load" —
        // the dwelling under-reports consumption with no signal to the
        // caller, the same failure class the premise-ZIP NaN guard was
        // added to eliminate. The honest degradation is an error or an
        // explicit signal, not 0.0 kW indistinguishable from a real zero.
        let config = config_with_schedule("s", "Lighting", &[f64::NAN]);
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let env = base_env();
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let result = eq.step(&env, Duration::from_secs(900), &mut ports);
        assert!(
            result.is_err(),
            "a NaN schedule value must surface as an error, not a silent 0.0 kW draw"
        );
    }

    #[test]
    fn column_sourced_load_with_zeroed_months_must_not_publish_unscaled_column_weight() {
        // `step()` scales every draw by the runtime month multipliers
        // (`raw * load_fraction * month_scale`) regardless of whether the
        // power source is a constant or a schedule column. The constant
        // arm of `expected_mean_power_kw` scales its published expectation
        // by the mean multiplier; the column arm defers to
        // `ExpectedMeanPower::ScheduleColumn`, whose premise weight is the
        // *unscaled* column mean — the same published-expectation vs step
        // contradiction on the other arm of the same method. With all
        // twelve months zeroed the equipment draws nothing year-round, so
        // the honest publication is zero or an explicit "not computable",
        // never a deferral the premise aggregation weights by a draw the
        // equipment can never take.
        let mut extras: Vec<(String, crate::config::ConfigValue)> = vec![
            (KEY_POWER_SCHEDULE_SOURCE.to_string(), "column".into()),
            (KEY_POWER_SCHEDULE_COL.to_string(), 1.0.into()),
            (KEY_SENSIBLE_GAIN_FRACTION.to_string(), 0.0.into()),
        ];
        for month in 0..12 {
            extras.push((format!("month_multiplier_{month}"), 0.0.into()));
        }
        let extras_ref: Vec<(&str, crate::config::ConfigValue)> = extras
            .iter()
            .map(|(k, v)| (k.as_str(), v.clone()))
            .collect();
        let config = config_with_extras("Seasonal Pump", "Seasonal Pump", &[0.0], &extras_ref);
        let mut eq =
            ScheduledLoad::new(config.clone(), hares_types::EndUse::OTHER, "Seasonal Pump");
        let mut env = base_env();
        env.custom_domains.push(DomainUpdate {
            domain_id: SCHEDULE_DOMAIN_ID,
            zone_temperatures_c: vec![],
            custom_payload: Some(vec![2.5, 7.25]),
        });
        eq.init(&config, &env).unwrap();

        // Ground the equipment's own model: month zeroing applies to the
        // column draw exactly as it does to a constant.
        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert_eq!(
            ports.electrical.net_active_w(),
            0.0,
            "every month is zeroed; the column draw must be suppressed"
        );

        match eq.expected_mean_power_kw() {
            None => {}
            Some(crate::ExpectedMeanPower::Kw(kw)) => assert!(
                kw.abs() < 1e-12,
                "a load zeroed in every month has zero expected draw, got {kw}"
            ),
            other => panic!(
                "a zeroed column-sourced load must not defer its unscaled \
                 column mean to the premise aggregate, got {other:?}"
            ),
        }
    }

    #[test]
    fn column_sourced_load_with_identity_month_multipliers_keeps_column_deferral() {
        // Identity multipliers change nothing at step, so the column
        // deferral remains the honest expectation — the premise aggregate
        // weights the load by the column mean, which is what it draws.
        let mut extras: Vec<(String, crate::config::ConfigValue)> = vec![
            (KEY_POWER_SCHEDULE_SOURCE.to_string(), "column".into()),
            (KEY_POWER_SCHEDULE_COL.to_string(), 1.0.into()),
            (KEY_SENSIBLE_GAIN_FRACTION.to_string(), 0.0.into()),
        ];
        for month in 0..12 {
            extras.push((format!("month_multiplier_{month}"), 1.0.into()));
        }
        let extras_ref: Vec<(&str, crate::config::ConfigValue)> = extras
            .iter()
            .map(|(k, v)| (k.as_str(), v.clone()))
            .collect();
        let config = config_with_extras("Lighting", "Lighting", &[0.0], &extras_ref);
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let env = base_env();
        eq.init(&config, &env).unwrap();

        match eq.expected_mean_power_kw() {
            Some(crate::ExpectedMeanPower::ScheduleColumn(1)) => {}
            other => panic!(
                "identity multipliers change no draw; the column deferral \
                 must be preserved, got {other:?}"
            ),
        }
    }

    #[test]
    fn column_sourced_load_with_partial_month_multipliers_is_not_computable() {
        // With multipliers that vary by month, the expected mean depends on
        // which month each column step falls in — not derivable without
        // simulation. The honest answer is `None`, never an unscaled
        // deferral the premise aggregate would weight by a draw the
        // equipment does not take at that weight.
        let mut extras: Vec<(String, crate::config::ConfigValue)> = vec![
            (KEY_POWER_SCHEDULE_SOURCE.to_string(), "column".into()),
            (KEY_POWER_SCHEDULE_COL.to_string(), 1.0.into()),
            (KEY_SENSIBLE_GAIN_FRACTION.to_string(), 0.0.into()),
        ];
        for month in 0..12 {
            let multiplier = if month == 11 { 1.0 } else { 0.0 };
            extras.push((format!("month_multiplier_{month}"), multiplier.into()));
        }
        let extras_ref: Vec<(&str, crate::config::ConfigValue)> = extras
            .iter()
            .map(|(k, v)| (k.as_str(), v.clone()))
            .collect();
        let config = config_with_extras("Shoulder Pump", "Seasonal Pump", &[0.0], &extras_ref);
        let mut eq =
            ScheduledLoad::new(config.clone(), hares_types::EndUse::OTHER, "Shoulder Pump");
        let env = base_env();
        eq.init(&config, &env).unwrap();

        assert_eq!(
            eq.expected_mean_power_kw(),
            None,
            "partial month multipliers make a column load's expectation \
             unknowable without simulation; it must be None"
        );
    }

    #[test]
    fn load_fraction_negative_is_clamped_to_zero() {
        let config = config_with_schedule("s", "Lighting", &[2.0]);
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let env = base_env();
        eq.init(&config, &env).unwrap();
        // Negative load fractions are rejected centrally; 0.0 means "off" and
        // is the correct way to zero the load through the control interface.
        eq.apply_control(&ControlSignal::LoadFraction { fraction: 0.0 })
            .unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert_eq!(ports.electrical.net_active_w(), 0.0);
    }

    #[test]
    fn non_finite_month_multiplier_fails_loudly_at_init() {
        let mut extras: Vec<(String, crate::config::ConfigValue)> =
            vec![(KEY_SENSIBLE_GAIN_FRACTION.to_string(), 0.0.into())];
        extras.push((
            "month_multiplier_3".to_string(),
            crate::config::ConfigValue::Float(f64::NAN),
        ));
        let extras_ref: Vec<(&str, crate::config::ConfigValue)> = extras
            .iter()
            .map(|(k, v)| (k.as_str(), v.clone()))
            .collect();
        let config = config_with_extras("Seasonal Pump", "Seasonal Pump", &[2.0], &extras_ref);
        let mut eq =
            ScheduledLoad::new(config.clone(), hares_types::EndUse::OTHER, "Seasonal Pump");
        let env = base_env();
        let err = eq
            .init(&config, &env)
            .expect_err("a non-finite month multiplier must fail at init");
        assert!(
            matches!(
                &err,
                hares_types::HaresError::InvalidEquipmentParameter { key, .. }
                    if key == "month_multiplier_3"
            ),
            "the rejection must name the offending key, got: {err}"
        );
    }

    #[test]
    fn expected_mean_power_accounts_for_month_multipliers() {
        // A load configured with runtime month multipliers draws
        // `constant_kw * load_fraction * month_scale` each step (the
        // scaling applied in `step()`), so its expected mean draw over a
        // uniform year is the constant scaled by the mean of the twelve
        // multipliers — the same convention `ScheduleSource::mean()` uses
        // for DailyProfile month averaging. The premise-level aggregation
        // weights equipment by `expected_mean_power_kw()`: publishing the
        // unscaled constant overstates a seasonal load's weight by the
        // inverse of the mean multiplier (12x here), silently skewing the
        // published premise ZIP toward that equipment's coefficients.
        let mut extras: Vec<(String, crate::config::ConfigValue)> =
            vec![(KEY_SENSIBLE_GAIN_FRACTION.to_string(), 0.0.into())];
        for month in 0..12 {
            let multiplier = if month == 11 { 1.0 } else { 0.0 };
            extras.push((format!("month_multiplier_{month}"), multiplier.into()));
        }
        let extras_ref: Vec<(&str, crate::config::ConfigValue)> = extras
            .iter()
            .map(|(k, v)| (k.as_str(), v.clone()))
            .collect();
        let config = config_with_extras("Seasonal Pump", "Seasonal Pump", &[2.0], &extras_ref);
        let mut eq =
            ScheduledLoad::new(config.clone(), hares_types::EndUse::OTHER, "Seasonal Pump");
        let mut env = base_env();
        eq.init(&config, &env).unwrap();

        // Ground the equipment's own model: the same 2 kW constant load
        // draws nothing in March (month_multiplier_2 = 0) and 2 kW in
        // December (month_multiplier_11 = 1).
        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert_eq!(
            ports.electrical.net_active_w(),
            0.0,
            "March is zeroed by month_multiplier_2 = 0"
        );
        env.current_time = FixedOffset::east_opt(0)
            .expect("UTC offset")
            .with_ymd_and_hms(2026, 12, 18, 0, 0, 0)
            .single()
            .expect("valid UTC timestamp");
        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert_eq!(ports.electrical.net_active_w(), 2000.0);

        // The published expectation must describe that same load: 2 kW in
        // one month of twelve is 2/12 kW on average, not 2 kW.
        match eq.expected_mean_power_kw() {
            Some(crate::ExpectedMeanPower::Kw(kw)) => assert!(
                (kw - 2.0 / 12.0).abs() < 1e-12,
                "expected mean must be the month-scaled average 2/12 kW, got {kw}"
            ),
            other => panic!(
                "a month-scaled constant load has a computable expected draw, \
                 got {other:?}"
            ),
        }
    }

    /// A daily profile's month factors are its shape; the load's month
    /// multipliers scale it on top, in its draws and its expected mean.
    #[test]
    fn month_multipliers_scale_a_daily_profile() {
        use super::{
            KEY_POWER_PROFILE_MAX_KW, KEY_POWER_PROFILE_MONTH, KEY_POWER_PROFILE_WEEKDAY,
            KEY_POWER_PROFILE_WEEKEND,
        };
        use hares_types::EndUse;
        let mut raw: HashMap<String, crate::config::ConfigValue> = HashMap::new();
        raw.insert("zone_id".to_string(), 1.0.into());
        raw.insert(KEY_SENSIBLE_GAIN_FRACTION.to_string(), 0.0.into());
        raw.insert(
            KEY_POWER_SCHEDULE_SOURCE.to_string(),
            "daily_profile".into(),
        );
        raw.insert(KEY_POWER_PROFILE_MAX_KW.to_string(), 2.0.into());
        for key in [KEY_POWER_PROFILE_WEEKDAY, KEY_POWER_PROFILE_WEEKEND] {
            raw.insert(
                key.to_string(),
                crate::config::ConfigValue::FloatArray(vec![1.0; 24]),
            );
        }
        let mut profile_months = vec![1.0; 12];
        profile_months[2] = 0.5;
        raw.insert(
            KEY_POWER_PROFILE_MONTH.to_string(),
            crate::config::ConfigValue::FloatArray(profile_months),
        );
        raw.insert("month_multiplier_2".to_string(), 0.5.into());
        let config = EquipmentConfig::raw("Profile".to_string(), "Lighting".to_string(), raw);
        let mut eq = ScheduledLoad::new(config.clone(), EndUse::LIGHTING, "Lighting");
        let env = base_env();
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert_eq!(
            ports.electrical.net_active_w(),
            500.0,
            "March: 2 kW, times the profile's 0.5, times the multiplier's 0.5"
        );
        match eq.expected_mean_power_kw() {
            Some(crate::ExpectedMeanPower::Kw(kw)) => assert!(
                (kw - 2.0 * 11.25 / 12.0).abs() < 1e-12,
                "eleven months at 2 kW and March at 0.5 kW, got {kw}"
            ),
            other => panic!("a daily profile has a computable mean, got {other:?}"),
        }
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
    fn zip_sidecar_overrides_type_specific_default() {
        let mut config = config_with_extras(
            "s",
            "Lighting",
            &[2.0],
            &[(KEY_SENSIBLE_GAIN_FRACTION, 0.0.into())],
        );
        config.zip = Some(real_only_zip(0.3, 0.3, 0.4));
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
        let expected_w = 2000.0 * expected_multiplier;
        assert!(
            (ports.electrical.net_active_w() - expected_w).abs() < 10.0,
            "sidecar zp=0.3,ip=0.3,pp=0.4 should override type-specific Lighting defaults"
        );
        assert_eq!(eq.zip.zp, 0.3);
        assert_eq!(eq.zip.ip, 0.3);
        assert_eq!(eq.zip.pp, 0.4);
    }

    #[test]
    fn unrecognized_class_falls_back_to_zip_default() {
        let config = config_with_schedule("s", "Custom Bench Load", &[1.0]);
        let eq = ScheduledLoad::new(
            config.clone(),
            hares_types::EndUse::PLUG_LOADS,
            "Custom Bench Load",
        );
        assert_eq!(eq.zip, ResolvedZip::governing(ZipLoad::constant_power()));
    }

    #[test]
    fn type_specific_zip_produces_voltage_dependent_load() {
        // Lighting defaults: z=0.54, i=0.5, p_coeff=-0.04, pf=1.0
        // At 0.9 pu: multiplier = 0.54*0.81 + 0.5*0.9 + (-0.04) = 0.8474
        // Default (unrecognized class): multiplier = 0*0.81 + 0*0.9 + 1.0 = 1.0
        let lighting_config = config_with_schedule("s", "Lighting", &[1.0]);
        let mut lighting_eq = ScheduledLoad::new(
            lighting_config.clone(),
            hares_types::EndUse::LIGHTING,
            "Lighting",
        );
        let mut env = base_env();
        lighting_eq.init(&lighting_config, &env).unwrap();

        let tv_config = config_with_schedule("s", "Custom Bench Load", &[1.0]);
        let mut tv_eq = ScheduledLoad::new(
            tv_config.clone(),
            hares_types::EndUse::PLUG_LOADS,
            "Custom Bench Load",
        );
        // Requires explicit sensible gain fraction
        let tv_config_full = config_with_extras(
            "s",
            "Custom Bench Load",
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
            (ports_lighting.electrical.net_active_w() - 1000.0).abs() < 10.0,
            "at nominal voltage Lighting ZIP multiplier should equal 1.0"
        );
        assert!(
            (ports_tv.electrical.net_active_w() - 1000.0).abs() < 10.0,
            "at nominal voltage the constant-power default should produce 1.0 kW"
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

        let lighting_w = ports_lighting.electrical.net_active_w();
        let tv_w = ports_tv.electrical.net_active_w();
        let expected_lighting_mult = 0.54 * 0.9_f64.powi(2) + 0.5 * 0.9 - 0.04;
        assert!(
            (lighting_w - 1000.0 * expected_lighting_mult).abs() < 10.0,
            "Lighting at 0.9 pu: expected {}, got {lighting_w}",
            1000.0 * expected_lighting_mult
        );
        assert!(
            (tv_w - 1000.0).abs() < 10.0,
            "constant-power default at 0.9 pu: should still be 1.0 kW, got {tv_w}"
        );
        assert!(
            lighting_w < tv_w,
            "Lighting (voltage-sensitive) should draw less than TV (constant-power) at reduced voltage"
        );
    }

    #[test]
    fn lighting_zip_produces_reactive_power_at_nominal_voltage() {
        // Lighting with type-specific defaults: pf=1.0, zq=0.46, iq=0.51, pq=0.03
        // At 1.0 pu: reactive_base = 0.46+0.51+0.03 = 1.0
        // pf=1.0 → tan(acos(1.0)) = 0.0 → reactive power must be zero.
        let config = config_with_schedule("s", "Lighting", &[2.0]);
        let mut eq = ScheduledLoad::new(config.clone(), hares_types::EndUse::LIGHTING, "Lighting");
        let env = base_env();
        eq.init(&config, &env).unwrap();

        let mut ports = PortSlots::default();
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();
        assert!((ports.electrical.net_active_w() - 2000.0).abs() < 10.0);
        assert!(
            ports.electrical.reactive_power_kvar.abs() < 1e-12,
            "Lighting with pf=1.0 should produce zero reactive power, got {}",
            ports.electrical.reactive_power_kvar
        );
    }
}
