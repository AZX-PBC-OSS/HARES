//! Electric vehicle charging equipment model.

use std::borrow::Cow;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, Timelike};
use hares_types::telemetry_keys as tk;
use hares_types::{
    BatteryChemistry, ChargingLevel, ChargingStrategy, ControlCapabilities, ControlSignal,
    CoreCapabilities, CoreFlows, CoreOutput, CoreState, ElectricPower, EndUse, EnvironmentState,
    EquipmentDescriptor, EquipmentId, EvConnectionState, ExecutionStage, FuelType, HaresError,
    OperatingMode, PlugInPolicy, PortContribution, PortDeclaration, PortSlots, Soc, Telemetry,
};

use crate::battery::ocv::{OcvTable, UNegTable};
use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

pub mod catalog;
mod charging_curve;
mod checkpoint;
mod config;
mod telemetry;

pub use charging_curve::{ChargingCurveLut, parse_pybamm_lut_csv};
pub use config::EvConfig;

use checkpoint::EvCheckpoint;
use config::*;
use telemetry::{default_telemetry, telemetry_fields};

fn charging_level_from_config(config: &EquipmentConfig) -> ChargingLevel {
    let level = config
        .get_str(KEY_CHARGING_LEVEL)
        .or_else(|| config.get_str(KEY_CHARGING_LEVEL_HPXML))
        .unwrap_or("L2")
        .trim()
        .replace(' ', "")
        .to_ascii_lowercase();

    match level.as_str() {
        "l1" | "level1" => ChargingLevel::L1,
        _ => ChargingLevel::L2,
    }
}

fn telemetry_code(level: ChargingLevel) -> f64 {
    match level {
        ChargingLevel::L1 => 1.0,
        ChargingLevel::L2 => 2.0,
    }
}

/// CC-CV margin: constant-power formula underestimates charge time because
/// CC-CV taper reduces power at high SOC. 0.85 accounts for ~15% longer
/// charge time in the CV region.
const CC_CV_MARGIN: f64 = 0.85;

pub struct Ev {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    core_output: CoreOutput,

    battery_capacity_kwh: f64,
    charging_level: ChargingLevel,
    rated_power_kw: f64,
    charging_efficiency: f64,
    l1_current_a: Option<f64>,
    l1_voltage_v: f64,
    soc_max: f64,
    charging_curve_lut: Option<crate::ndinterp::RegularGridInterpolator>,
    min_charge_temp_c: f64,
    full_power_temp_c: f64,
    heater_power_w: f64,
    heater_threshold_c: f64,
    thermal_mass_j_per_k: f64,
    ua_w_per_k: f64,
    v2l_enabled: bool,
    v2l_soc_reserve: f64,
    v2l_max_discharge_kw: f64,
    v2g_enabled: bool,
    v2g_soc_reserve: f64,
    v2g_max_discharge_kw: f64,

    soc: f64,
    battery_temp_c: f64,
    heater_active: bool,
    connection_state: EvConnectionState,
    away_charger_power_kw: f64,
    away_charge_actual_kw: f64,
    active_power_kw: f64,

    ready_soc: f64,
    ready_by_hour: Option<f64>,
    ready_by_soc: Option<f64>,

    chemistry: BatteryChemistry,
    fuel_economy_kwh_per_mi: f64,
    custom_ocv: bool,
    custom_u_neg: bool,

    // Degradation tracking (Smith 2017, shared with Battery)
    degradation: crate::battery::degradation::DegradationState,
    rainflow: crate::battery::degradation::RainflowCounter,
    ocv_table: OcvTable,
    u_neg_table: UNegTable,
    last_daily_update_day: i32,

    v2l_active: bool,
    v2l_power_kw: f64,

    charging_strategy: ChargingStrategy,
    plug_in_policy: PlugInPolicy,

    power_limit_kw: Option<f64>,
    power_setpoint_kw: Option<f64>,
    soc_target: Option<f64>,
    soc_target_min: Option<f64>,
    soc_target_max: Option<f64>,
}

impl Ev {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(config.get_f64(KEY_EQUIPMENT_ID).unwrap_or_default() as u32),
            name: config.name.clone(),
            end_use: EndUse::EV,
            equipment_type: Cow::Borrowed("EV"),
            zone: None,
            fuel: FuelType::Electric,
            stage: ExecutionStage::Electrical,
            control_capabilities: ControlCapabilities::POWER_SETPOINT
                | ControlCapabilities::SOC_TARGET
                | ControlCapabilities::POWER_LIMIT
                | ControlCapabilities::EV_PLUG_IN
                | ControlCapabilities::EV_DRIVE
                | ControlCapabilities::EV_AWAY_CHARGE
                | ControlCapabilities::EV_SET_READY_BY,
            core_capabilities: CoreCapabilities::ELECTRIC
                | CoreCapabilities::HAS_SOC
                | CoreCapabilities::HAS_MODE,
            telemetry_fields: telemetry_fields(),
        };

        let charging_level = charging_level_from_config(&config);
        let battery_capacity_kwh = resolve_capacity_kwh(&config).unwrap_or(DEFAULT_CAPACITY_KWH);
        let rated_power_kw = resolve_rated_power_kw(&config, charging_level, battery_capacity_kwh)
            .unwrap_or_else(|| default_max_power_kw(&config, charging_level, battery_capacity_kwh));
        let soc_max = config.get_f64(KEY_SOC_MAX).unwrap_or(DEFAULT_SOC_MAX);

        let initial_connection_state = config
            .get_str(KEY_INITIAL_CONNECTION_STATE)
            .and_then(|s| s.parse::<EvConnectionState>().ok())
            .unwrap_or(EvConnectionState::HomePluggedIn);

        let chemistry = config
            .get_str(KEY_CHEMISTRY)
            .and_then(|s| s.parse::<BatteryChemistry>().ok())
            .unwrap_or(BatteryChemistry::Nmc);

        let fuel_economy_kwh_per_mi = config
            .get_f64(KEY_FUEL_ECONOMY_KWH_PER_MI)
            .unwrap_or(DEFAULT_FUEL_ECONOMY_KWH_PER_MI);

        Self {
            descriptor,
            ports: vec![PortDeclaration::electrical()],
            telemetry: default_telemetry(charging_level),
            core_output: CoreOutput::default(),
            battery_capacity_kwh,
            charging_level,
            rated_power_kw,
            charging_efficiency: config.get_f64(KEY_EFFICIENCY).unwrap_or(DEFAULT_EFFICIENCY),
            l1_current_a: config.get_f64(KEY_L1_CURRENT_A),
            l1_voltage_v: config
                .get_f64(KEY_L1_VOLTAGE_V)
                .unwrap_or(DEFAULT_L1_VOLTAGE_V),
            soc_max,
            charging_curve_lut: None,
            min_charge_temp_c: DEFAULT_MIN_CHARGE_TEMP_C,
            full_power_temp_c: DEFAULT_FULL_POWER_TEMP_C,
            heater_power_w: DEFAULT_HEATER_POWER_W,
            heater_threshold_c: DEFAULT_HEATER_THRESHOLD_C,
            thermal_mass_j_per_k: DEFAULT_THERMAL_MASS_J_PER_K,
            ua_w_per_k: DEFAULT_UA_W_PER_K,
            v2l_enabled: config.get_bool(KEY_V2L_ENABLED).unwrap_or(false),
            v2l_soc_reserve: config
                .get_f64(KEY_V2L_SOC_RESERVE)
                .unwrap_or(DEFAULT_V2L_SOC_RESERVE),
            v2l_max_discharge_kw: config
                .get_f64(KEY_V2L_MAX_DISCHARGE_KW)
                .unwrap_or(DEFAULT_V2L_MAX_DISCHARGE_KW),
            v2g_enabled: config.get_bool(KEY_V2G_ENABLED).unwrap_or(false),
            v2g_soc_reserve: config
                .get_f64(KEY_V2G_SOC_RESERVE)
                .unwrap_or(DEFAULT_V2G_SOC_RESERVE),
            v2g_max_discharge_kw: config
                .get_f64(KEY_V2G_MAX_DISCHARGE_KW)
                .unwrap_or(DEFAULT_V2G_MAX_DISCHARGE_KW),
            soc: config
                .get_f64(KEY_INITIAL_SOC)
                .unwrap_or(DEFAULT_SOC)
                .clamp(0.0, 1.0),
            battery_temp_c: config.get_f64(KEY_BATTERY_TEMP_C).unwrap_or(20.0),
            heater_active: false,
            connection_state: initial_connection_state,
            away_charger_power_kw: 0.0,
            away_charge_actual_kw: 0.0,
            active_power_kw: 0.0,
            ready_soc: config.get_f64(KEY_READY_SOC).unwrap_or(soc_max),
            ready_by_hour: None,
            ready_by_soc: None,
            chemistry,
            fuel_economy_kwh_per_mi,
            custom_ocv: false,
            custom_u_neg: false,
            v2l_active: false,
            v2l_power_kw: 0.0,
            charging_strategy: ChargingStrategy::Immediate { target_soc: 1.0 },
            plug_in_policy: config
                .get_str(KEY_PLUG_IN_POLICY)
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or(PlugInPolicy::Always),
            power_limit_kw: config.get_f64(KEY_POWER_LIMIT_KW),
            power_setpoint_kw: None,
            soc_target: None,
            soc_target_min: None,
            soc_target_max: None,
            degradation: crate::battery::degradation::DegradationState::default(),
            rainflow: crate::battery::degradation::RainflowCounter::default(),
            ocv_table: OcvTable::for_chemistry(chemistry),
            u_neg_table: UNegTable::for_chemistry(chemistry),
            last_daily_update_day: 0,
        }
    }

    pub fn charging_strategy(&self) -> &ChargingStrategy {
        &self.charging_strategy
    }

    fn init_typed(
        &mut self,
        config: &EquipmentConfig,
        env: &EnvironmentState,
    ) -> crate::Result<()> {
        let c = config.require_typed::<EvConfig>("EV")?;
        c.validate()?;

        self.battery_capacity_kwh = c.capacity_kwh;

        let level_str = c.charging_level.as_deref().unwrap_or("L2");
        self.charging_level = match level_str
            .trim()
            .replace(' ', "")
            .to_ascii_lowercase()
            .as_str()
        {
            "l1" | "level1" => ChargingLevel::L1,
            _ => ChargingLevel::L2,
        };

        self.charging_efficiency = c.charging_efficiency.unwrap_or(DEFAULT_EFFICIENCY);
        self.l1_current_a = c.l1_current_a;
        self.l1_voltage_v = c.l1_voltage_v.unwrap_or(DEFAULT_L1_VOLTAGE_V);
        self.soc_max = c.soc_max.unwrap_or(DEFAULT_SOC_MAX);

        self.rated_power_kw = match self.charging_level {
            ChargingLevel::L1 => c
                .max_charging_power_kw
                .clamp(L1_MIN_POWER_KW, L1_MAX_POWER_KW),
            ChargingLevel::L2 => c
                .max_charging_power_kw
                .clamp(L2_MIN_POWER_KW, L2_MAX_POWER_KW),
        };

        self.min_charge_temp_c = c.min_charge_temp_c.unwrap_or(DEFAULT_MIN_CHARGE_TEMP_C);
        self.full_power_temp_c = c.full_power_temp_c.unwrap_or(DEFAULT_FULL_POWER_TEMP_C);
        self.heater_power_w = c.heater_power_w.unwrap_or(DEFAULT_HEATER_POWER_W);
        self.heater_threshold_c = c.heater_threshold_c.unwrap_or(DEFAULT_HEATER_THRESHOLD_C);
        self.thermal_mass_j_per_k = c
            .thermal_mass_j_per_k
            .unwrap_or(DEFAULT_THERMAL_MASS_J_PER_K);
        self.ua_w_per_k = c.ua_w_per_k.unwrap_or(DEFAULT_UA_W_PER_K);

        self.v2l_enabled = c.v2l_enabled.unwrap_or(false);
        self.v2l_soc_reserve = c.v2l_soc_reserve.unwrap_or(DEFAULT_V2L_SOC_RESERVE);
        self.v2l_max_discharge_kw = c
            .v2l_max_discharge_kw
            .unwrap_or(DEFAULT_V2L_MAX_DISCHARGE_KW);
        self.v2g_enabled = c.v2g_enabled.unwrap_or(false);
        self.v2g_soc_reserve = c.v2g_soc_reserve.unwrap_or(DEFAULT_V2G_SOC_RESERVE);
        self.v2g_max_discharge_kw = c
            .v2g_max_discharge_kw
            .unwrap_or(DEFAULT_V2G_MAX_DISCHARGE_KW);

        self.chemistry = c
            .chemistry
            .as_deref()
            .and_then(|s| s.parse::<BatteryChemistry>().ok())
            .unwrap_or(BatteryChemistry::Nmc);
        if !self.custom_ocv {
            self.ocv_table = OcvTable::for_chemistry(self.chemistry);
        }
        if !self.custom_u_neg {
            self.u_neg_table = UNegTable::for_chemistry(self.chemistry);
        }

        self.fuel_economy_kwh_per_mi = c
            .fuel_economy_kwh_per_mi
            .unwrap_or(DEFAULT_FUEL_ECONOMY_KWH_PER_MI);
        self.ready_soc = c.ready_soc.unwrap_or(self.soc_max);
        self.power_limit_kw = c.power_limit_kw;

        self.connection_state = c
            .initial_connection_state
            .as_deref()
            .and_then(|s| s.parse::<EvConnectionState>().ok())
            .unwrap_or(EvConnectionState::HomePluggedIn);

        let initial_soc = c.initial_soc.unwrap_or(DEFAULT_SOC);
        self.soc = initial_soc.clamp(0.0, 1.0);

        self.battery_temp_c = c.battery_temp_c.unwrap_or(env.weather.outdoor_temp_c);

        if let Some(strat_str) = c.charging_strategy.as_deref() {
            self.charging_strategy = serde_json::from_str(strat_str)
                .map_err(|e| HaresError::Equipment(format!("invalid charging_strategy: {e}")))?;
        } else {
            self.charging_strategy = ChargingStrategy::Immediate { target_soc: 1.0 };
        }

        if let Some(policy_str) = c.plug_in_policy.as_deref() {
            self.plug_in_policy = serde_json::from_str(policy_str)
                .map_err(|e| HaresError::Equipment(format!("invalid plug_in_policy: {e}")))?;
        } else {
            self.plug_in_policy = PlugInPolicy::Always;
        }

        // Reset transient state
        self.charging_curve_lut = None;
        self.ready_by_hour = None;
        self.ready_by_soc = None;
        self.away_charger_power_kw = 0.0;
        self.away_charge_actual_kw = 0.0;
        self.heater_active = false;
        self.active_power_kw = 0.0;
        self.v2l_active = false;
        self.v2l_power_kw = 0.0;
        self.power_setpoint_kw = None;
        self.soc_target = None;
        self.soc_target_min = None;
        self.soc_target_max = None;
        self.telemetry = default_telemetry(self.charging_level);
        self.core_output = CoreOutput::default();
        self.write_telemetry();

        Ok(())
    }

    fn effective_soc_limit(&self) -> f64 {
        let mut limit = self.soc_target.unwrap_or(self.soc_max);
        if let Some(min) = self.soc_target_min {
            limit = limit.max(min);
        }
        if let Some(max) = self.soc_target_max {
            limit = limit.min(max);
        }
        limit.clamp(0.0, self.soc_max)
    }

    /// Compute charging power for this timestep.
    ///
    /// `charge_derate` is applied before the taper limit so that the taper
    /// correctly prevents SOC overshoot even under cold-temperature derating.
    /// `max_power_kw` is the charger's rated power (home rated or away charger).
    fn compute_charging_power_kw(
        &self,
        now: DateTime<FixedOffset>,
        dt: Duration,
        charge_derate: f64,
        max_power_kw: f64,
    ) -> f64 {
        if self.connection_state == EvConnectionState::HomePluggedIn
            && let Some(setpoint) = self.power_setpoint_kw
            && setpoint < 0.0
        {
            if self.v2g_enabled {
                return self.compute_v2g_discharge(dt);
            } else if self.v2l_enabled {
                return self.compute_v2l_discharge(dt);
            }
        }

        let target = self
            .soc_target
            .or(self.ready_by_soc)
            .unwrap_or(self.ready_soc);
        let soc_limit = {
            let mut limit = target;
            if let Some(min) = self.soc_target_min {
                limit = limit.max(min);
            }
            if let Some(max) = self.soc_target_max {
                limit = limit.min(max);
            }
            limit.clamp(0.0, self.soc_max)
        };

        if self.soc >= soc_limit {
            return 0.0;
        }

        let dt_hours = (dt.as_secs_f64() / SECONDS_PER_HOUR).max(MIN_TIMESTEP_HOURS);
        let rated = match self.charging_level {
            ChargingLevel::L1 => self.l1_power_kw().min(max_power_kw),
            ChargingLevel::L2 => self.rated_power_kw.min(max_power_kw),
        };
        let curve_limited_rated = if let Some(lut) = &self.charging_curve_lut {
            let soh = 1.0 - self.degradation.capacity_fade_fraction();
            let effective_kwh = self.battery_capacity_kwh * soh;
            let c_rate = if effective_kwh > 0.0 {
                rated / effective_kwh
            } else {
                0.0
            };
            rated * lut.interpolate(&[self.soc, self.battery_temp_c, c_rate, soh]) as f64
        } else {
            rated
        };
        let derated_rated = curve_limited_rated * charge_derate;

        let mut requested = self
            .power_setpoint_kw
            .unwrap_or(derated_rated)
            .max(0.0)
            .min(derated_rated);

        if self.ready_by_hour.is_some() && self.power_setpoint_kw.is_none() {
            requested = self.bms_ready_by_power(now, derated_rated, soc_limit);
        }

        let taper_limit = (soc_limit - self.soc).max(0.0) * self.battery_capacity_kwh
            / dt_hours
            / self.charging_efficiency;

        let mut power = requested.min(derated_rated).min(taper_limit).max(0.0);
        if let Some(limit) = self.power_limit_kw
            && self.connection_state == EvConnectionState::HomePluggedIn
        {
            power = power.min(limit.max(0.0));
        }
        power
    }

    fn bms_ready_by_power(
        &self,
        now: DateTime<FixedOffset>,
        derated_rated: f64,
        soc_limit: f64,
    ) -> f64 {
        let ready_by_hour = match self.ready_by_hour {
            Some(h) => h,
            None => return derated_rated,
        };

        let soc_deficit = (soc_limit - self.soc).max(0.0);
        if soc_deficit <= 0.0 {
            return 0.0;
        }

        let eff_power = derated_rated * self.charging_efficiency * CC_CV_MARGIN;
        let hours_needed = if eff_power > 0.0 {
            soc_deficit * self.battery_capacity_kwh / eff_power
        } else {
            return derated_rated;
        };

        let current_hour =
            now.hour() as f64 + now.minute() as f64 / 60.0 + now.second() as f64 / 3600.0;

        let hours_until_deadline = if ready_by_hour > current_hour {
            ready_by_hour - current_hour
        } else if (current_hour - ready_by_hour).abs() < 1.0 {
            return derated_rated;
        } else {
            24.0 - current_hour + ready_by_hour
        };

        if hours_needed >= hours_until_deadline {
            return derated_rated;
        }

        0.0
    }

    fn compute_v2l_discharge(&self, dt: Duration) -> f64 {
        if self.soc <= self.v2l_soc_reserve {
            return 0.0;
        }
        let setpoint_magnitude = self.power_setpoint_kw.unwrap_or(0.0).abs();
        let capped = setpoint_magnitude.min(self.v2l_max_discharge_kw);
        let dt_hours = (dt.as_secs_f64() / SECONDS_PER_HOUR).max(MIN_TIMESTEP_HOURS);
        let available_kwh = (self.soc - self.v2l_soc_reserve) * self.battery_capacity_kwh;
        let max_discharge_kw = available_kwh / dt_hours;
        -(capped.min(max_discharge_kw).max(0.0))
    }

    fn compute_v2g_discharge(&self, dt: Duration) -> f64 {
        if self.soc <= self.v2g_soc_reserve {
            return 0.0;
        }
        let setpoint_magnitude = self.power_setpoint_kw.unwrap_or(0.0).abs();
        let capped = setpoint_magnitude.min(self.v2g_max_discharge_kw);
        let dt_hours = (dt.as_secs_f64() / SECONDS_PER_HOUR).max(MIN_TIMESTEP_HOURS);
        let available_kwh = (self.soc - self.v2g_soc_reserve) * self.battery_capacity_kwh;
        let max_discharge_kw = available_kwh / dt_hours;
        -(capped.min(max_discharge_kw).max(0.0))
    }

    fn charge_derate_factor(&self) -> f64 {
        crate::linear_temp_derate(
            self.battery_temp_c,
            self.min_charge_temp_c,
            self.full_power_temp_c,
        )
    }

    fn l1_power_kw(&self) -> f64 {
        match self.l1_current_a {
            Some(current_a) => (current_a * self.l1_voltage_v / 1_000.0).max(0.0),
            None => self.rated_power_kw,
        }
    }

    fn write_telemetry(&mut self) {
        self.telemetry.set(tk::SOC, self.soc);
        self.telemetry
            .set(tk::ACTIVE_POWER_KW, self.active_power_kw);
        self.telemetry.set(
            tk::CONNECTION_STATE,
            match self.connection_state {
                EvConnectionState::HomePluggedIn => 0.0,
                EvConnectionState::AwayPluggedIn => 1.0,
                EvConnectionState::Disconnected => 2.0,
            },
        );
        self.telemetry
            .set(tk::CHARGING_LEVEL, telemetry_code(self.charging_level));
        self.telemetry.set(tk::BATTERY_TEMP_C, self.battery_temp_c);
        self.telemetry.set(
            tk::HEATER_POWER_W,
            if self.heater_active {
                self.heater_power_w
            } else {
                0.0
            },
        );
        self.telemetry
            .set(tk::CHARGE_DERATE, self.charge_derate_factor());
        self.telemetry
            .set(tk::V2L_ACTIVE, if self.v2l_active { 1.0 } else { 0.0 });
        self.telemetry.set(tk::V2L_POWER_KW, self.v2l_power_kw);
        self.telemetry
            .set(tk::CAPACITY_FADE_PCT, self.degradation.capacity_fade_fraction() * 100.0);
        self.telemetry
            .set(tk::AWAY_CHARGE_POWER_KW, self.away_charge_actual_kw);
        self.telemetry
            .set(tk::CAPACITY_KWH, self.battery_capacity_kwh);
        self.telemetry
            .set(tk::FUEL_ECONOMY_KWH_PER_MI, self.fuel_economy_kwh_per_mi);
    }

    fn update_degradation(&mut self, env: &EnvironmentState, dt_s: f64) {
        let cell_temp_k = self.battery_temp_c + 273.15;
        let v_oc = self.ocv_table.voltage_at_soc(self.soc);
        self.rainflow.push(self.soc);
        self.degradation
            .accumulate(dt_s, cell_temp_k, v_oc, self.soc);

        let current_day = {
            use chrono::Datelike;
            env.current_time.date_naive().num_days_from_ce()
        };
        if current_day != self.last_daily_update_day {
            let sum_sq_dod = self.rainflow.sum_squared_dod_daily();
            self.degradation
                .update_daily(&self.u_neg_table, cell_temp_k, sum_sq_dod);
            self.degradation.reset_day_tracking(self.soc);
            self.rainflow.reset_daily();
            self.last_daily_update_day = current_day;
        }
    }

    fn run_charging_physics(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        max_power_kw: f64,
    ) -> (f64, f64, bool) {
        let charge_derate = self.charge_derate_factor();
        let charger_kw =
            self.compute_charging_power_kw(env.current_time, dt, charge_derate, max_power_kw);

        let is_v2l_discharge = charger_kw < 0.0;

        let would_charge_underated = !is_v2l_discharge && self.soc < self.effective_soc_limit();
        self.heater_active = self.heater_power_w > 0.0
            && would_charge_underated
            && self.battery_temp_c <= self.heater_threshold_c;
        let heater_kw = if self.heater_active {
            self.heater_power_w / 1000.0
        } else {
            0.0
        };

        (charger_kw, heater_kw, is_v2l_discharge)
    }

    fn apply_soc_and_thermal(
        &mut self,
        dt: Duration,
        charger_kw: f64,
        heater_kw: f64,
        is_v2l_discharge: bool,
        ambient_c: f64,
    ) {
        let dt_s = dt.as_secs_f64();

        let q_loss_w = self.ua_w_per_k * (self.battery_temp_c - ambient_c);
        self.battery_temp_c -= (q_loss_w * dt_s) / self.thermal_mass_j_per_k;

        if is_v2l_discharge {
            let dt_hours = (dt.as_secs_f64() / SECONDS_PER_HOUR).max(MIN_TIMESTEP_HOURS);
            let ac_discharge_kw = charger_kw.abs();
            let dc_discharge_kwh =
                (ac_discharge_kw / self.charging_efficiency.max(0.01)) * dt_hours;
            self.soc = (self.soc - dc_discharge_kwh / self.battery_capacity_kwh).clamp(0.0, 1.0);
        } else if charger_kw > 0.0 {
            let dt_hours = (dt.as_secs_f64() / SECONDS_PER_HOUR).max(MIN_TIMESTEP_HOURS);
            let net_charge_kw = (charger_kw - heater_kw).max(0.0);
            let dc_stored_kw = net_charge_kw * self.charging_efficiency;
            let dc_energy_kwh = dc_stored_kw * dt_hours;
            self.soc = (self.soc + dc_energy_kwh / self.battery_capacity_kwh).clamp(0.0, 1.0);

            let ohmic_like_heat_w = (net_charge_kw - dc_stored_kw) * 1000.0;
            let heater_w = heater_kw * 1000.0;
            self.battery_temp_c +=
                ((ohmic_like_heat_w + heater_w) * dt_s) / self.thermal_mass_j_per_k;
        }
    }
}

impl Equipment for Ev {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.init_typed(config, env)
    }

    fn update_control(&mut self, _env: &EnvironmentState) -> OperatingMode {
        match self.connection_state {
            EvConnectionState::HomePluggedIn if self.active_power_kw > 0.0 => {
                OperatingMode::Charging
            }
            EvConnectionState::AwayPluggedIn if self.away_charger_power_kw > 0.0 => {
                OperatingMode::Charging
            }
            _ => OperatingMode::Off,
        }
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        let ambient_c = env.weather.outdoor_temp_c;

        match self.connection_state {
            EvConnectionState::HomePluggedIn => {
                let (charger_kw, heater_kw, is_v2l_discharge) =
                    self.run_charging_physics(env, dt, self.rated_power_kw);

                self.v2l_active = is_v2l_discharge;
                self.v2l_power_kw = if is_v2l_discharge {
                    charger_kw.abs()
                } else {
                    0.0
                };

                self.active_power_kw = if is_v2l_discharge {
                    charger_kw
                } else {
                    charger_kw + heater_kw
                };

                self.away_charge_actual_kw = 0.0;

                if is_v2l_discharge || self.active_power_kw > 0.0 {
                    ports.accumulate(&PortContribution::Electrical {
                        active_power_kw: self.active_power_kw,
                        reactive_power_kvar: 0.0,
                    })?;
                }

                self.apply_soc_and_thermal(dt, charger_kw, heater_kw, is_v2l_discharge, ambient_c);
            }
            EvConnectionState::AwayPluggedIn => {
                let (charger_kw, heater_kw, _is_v2l_discharge) =
                    self.run_charging_physics(env, dt, self.away_charger_power_kw);

                // Away charging: no V2L/V2G, no port contribution
                self.v2l_active = false;
                self.v2l_power_kw = 0.0;
                self.active_power_kw = 0.0;
                self.away_charge_actual_kw = charger_kw;

                self.apply_soc_and_thermal(dt, charger_kw, heater_kw, false, ambient_c);
            }
            EvConnectionState::Disconnected => {
                // Thermal drift only, calendar degradation, zero power
                self.active_power_kw = 0.0;
                self.away_charge_actual_kw = 0.0;
                self.heater_active = false;
                self.v2l_active = false;
                self.v2l_power_kw = 0.0;

                let dt_s = dt.as_secs_f64();
                let q_loss_w = self.ua_w_per_k * (self.battery_temp_c - ambient_c);
                self.battery_temp_c -= (q_loss_w * dt_s) / self.thermal_mass_j_per_k;
            }
        }

        self.update_degradation(env, dt.as_secs_f64());
        self.write_telemetry();
        let mode = if self.active_power_kw > 1e-9 {
            OperatingMode::Charging
        } else if self.active_power_kw < -1e-9 {
            OperatingMode::Discharging
        } else {
            OperatingMode::Off
        };
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Bidirectional(self.active_power_kw)),
                reactive_power_kvar: None,
                fuel_w: None,
            },
            state: CoreState {
                operating_mode: Some(mode),
                soc: Soc::try_from(self.soc).ok(),
            },
        };
        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core_output
    }

    fn actor_seed(&self) -> Option<crate::ActorSeed> {
        if matches!(self.charging_strategy, ChargingStrategy::Immediate { .. }) {
            return None;
        }
        Some(crate::ActorSeed::Ev {
            strategy: self.charging_strategy.clone(),
            plug_in_policy: self.plug_in_policy.clone(),
            capacity_kwh: self.battery_capacity_kwh,
            max_charge_kw: self.rated_power_kw,
            fuel_economy_kwh_per_mi: self.fuel_economy_kwh_per_mi,
        })
    }

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&EvCheckpoint {
            soc: self.soc,
            connection_state: self.connection_state,
            away_charger_power_kw: self.away_charger_power_kw,
            active_power_kw: self.active_power_kw,
            power_limit_kw: self.power_limit_kw,
            power_setpoint_kw: self.power_setpoint_kw,
            soc_target: self.soc_target,
            soc_target_min: self.soc_target_min,
            soc_target_max: self.soc_target_max,
            battery_temp_c: self.battery_temp_c,
            heater_active: self.heater_active,
            ready_soc: self.ready_soc,
            ready_by_hour: self.ready_by_hour,
            ready_by_soc: self.ready_by_soc,
            v2l_enabled: self.v2l_enabled,
            v2l_soc_reserve: self.v2l_soc_reserve,
            v2l_max_discharge_kw: self.v2l_max_discharge_kw,
            v2g_enabled: self.v2g_enabled,
            v2g_soc_reserve: self.v2g_soc_reserve,
            v2g_max_discharge_kw: self.v2g_max_discharge_kw,
            degradation: self.degradation.clone(),
            rainflow: self.rainflow.clone(),
            last_daily_update_day: self.last_daily_update_day,
        })
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let cp: EvCheckpoint = load_postcard(state)?;
        self.soc = cp.soc;
        self.connection_state = cp.connection_state;
        self.away_charger_power_kw = cp.away_charger_power_kw;
        self.active_power_kw = cp.active_power_kw;
        self.power_limit_kw = cp.power_limit_kw;
        self.power_setpoint_kw = cp.power_setpoint_kw;
        self.soc_target = cp.soc_target;
        self.soc_target_min = cp.soc_target_min;
        self.soc_target_max = cp.soc_target_max;
        self.battery_temp_c = cp.battery_temp_c;
        self.heater_active = cp.heater_active;
        self.ready_soc = cp.ready_soc;
        self.ready_by_hour = cp.ready_by_hour;
        self.ready_by_soc = cp.ready_by_soc;
        self.v2l_enabled = cp.v2l_enabled;
        self.v2l_soc_reserve = cp.v2l_soc_reserve;
        self.v2l_max_discharge_kw = cp.v2l_max_discharge_kw;
        self.v2g_enabled = cp.v2g_enabled;
        self.v2g_soc_reserve = cp.v2g_soc_reserve;
        self.v2g_max_discharge_kw = cp.v2g_max_discharge_kw;
        self.degradation = cp.degradation;
        self.rainflow = cp.rainflow;
        self.last_daily_update_day = cp.last_daily_update_day;

        self.v2l_active = false;
        self.v2l_power_kw = 0.0;

        self.write_telemetry();
        self.core_output = CoreOutput::default();
        Ok(())
    }

    fn validate_signal(&self, signal: &hares_types::ControlSignal) -> crate::Result<()> {
        use hares_types::ensure_signal_supported;
        ensure_signal_supported(self.descriptor().control_capabilities, signal)?;
        match signal {
            hares_types::ControlSignal::EvDrive { .. } => {
                if self.connection_state != hares_types::EvConnectionState::Disconnected {
                    return Err(hares_types::HaresError::Control(
                        "EvDrive rejected: EV must be Disconnected to drive".to_string(),
                    ));
                }
            }
            hares_types::ControlSignal::EvAwayCharge { .. } => {
                if self.connection_state != hares_types::EvConnectionState::AwayPluggedIn {
                    return Err(hares_types::HaresError::Control(
                        "EvAwayCharge rejected: EV must be AwayPluggedIn".to_string(),
                    ));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        match signal {
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                if *active_power_kw < 0.0 && !self.v2l_enabled && !self.v2g_enabled {
                    return Err(HaresError::Control(
                        "negative PowerSetpoint requires v2l_enabled or v2g_enabled".to_string(),
                    ));
                }
                if !active_power_kw.is_finite() {
                    return Err(HaresError::Control(
                        "EV PowerSetpoint active_power_kw must be finite".to_string(),
                    ));
                }
                self.power_setpoint_kw = Some(*active_power_kw);
            }
            ControlSignal::PowerLimit { max_power_kw, .. } => {
                if !max_power_kw.is_finite() {
                    return Err(HaresError::Control(
                        "EV PowerLimit max_power_kw must be finite".to_string(),
                    ));
                }
                self.power_limit_kw = Some((*max_power_kw).max(0.0));
            }
            ControlSignal::SOCTarget {
                target_soc,
                min_soc,
                max_soc,
            } => {
                if !target_soc.is_finite() {
                    return Err(HaresError::Control(
                        "EV SOCTarget target_soc must be finite".to_string(),
                    ));
                }
                if let Some(min) = min_soc
                    && (!min.is_finite() || !(0.0..=1.0).contains(min))
                {
                    return Err(HaresError::Control(
                        "EV SOCTarget min_soc must be finite and within [0, 1]".to_string(),
                    ));
                }
                if let Some(max) = max_soc
                    && (!max.is_finite() || !(0.0..=1.0).contains(max))
                {
                    return Err(HaresError::Control(
                        "EV SOCTarget max_soc must be finite and within [0, 1]".to_string(),
                    ));
                }
                self.soc_target = Some((*target_soc).clamp(0.0, 1.0));
                self.soc_target_min = *min_soc;
                self.soc_target_max = *max_soc;
            }
            ControlSignal::EvPlugIn { state } => {
                // Validate transitions: no direct home<->away
                match (self.connection_state, state) {
                    (EvConnectionState::HomePluggedIn, EvConnectionState::AwayPluggedIn)
                    | (EvConnectionState::AwayPluggedIn, EvConnectionState::HomePluggedIn) => {
                        return Err(HaresError::Control(
                            "EV cannot transition directly between HomePluggedIn and AwayPluggedIn; must disconnect first".to_string(),
                        ));
                    }
                    (from, to) if from == *to => {
                        // Same state — no-op
                        return Ok(());
                    }
                    _ => {}
                }
                self.connection_state = *state;
                self.away_charger_power_kw = 0.0;
                self.away_charge_actual_kw = 0.0;
                if *state == EvConnectionState::Disconnected {
                    self.ready_by_hour = None;
                    self.ready_by_soc = None;
                }
            }
            ControlSignal::EvDrive { kwh } => {
                if self.connection_state != EvConnectionState::Disconnected {
                    return Err(HaresError::Control(
                        "EvDrive rejected: EV must be Disconnected to drive".to_string(),
                    ));
                }
                if !kwh.is_finite() || *kwh < 0.0 {
                    return Err(HaresError::Control(
                        "EvDrive kwh must be finite and >= 0".to_string(),
                    ));
                }
                let available_kwh = self.battery_capacity_kwh * self.soc;
                if *kwh > available_kwh {
                    return Err(HaresError::Control(format!(
                        "EvDrive kwh ({kwh}) exceeds available energy ({available_kwh:.2} kWh)"
                    )));
                }
                self.soc = (self.soc - kwh / self.battery_capacity_kwh).clamp(0.0, 1.0);
            }
            ControlSignal::EvAwayCharge { power_kw } => {
                if self.connection_state != EvConnectionState::AwayPluggedIn {
                    return Err(HaresError::Control(
                        "EvAwayCharge rejected: EV must be AwayPluggedIn".to_string(),
                    ));
                }
                if !power_kw.is_finite() || *power_kw < 0.0 {
                    return Err(HaresError::Control(
                        "EvAwayCharge power_kw must be finite and >= 0".to_string(),
                    ));
                }
                self.away_charger_power_kw = *power_kw;
            }
            ControlSignal::EvSetReadyBy {
                departure_hour,
                target_soc,
            } => {
                validate_optional_hour("ready_by_hour", Some(*departure_hour))?;
                if !target_soc.is_finite() || !(0.0..=1.0).contains(target_soc) {
                    return Err(HaresError::Control(
                        "EvSetReadyBy target_soc must be finite and within [0, 1]".to_string(),
                    ));
                }
                self.ready_by_hour = Some(*departure_hour);
                self.ready_by_soc = Some(*target_soc);
            }
            _ => {
                return Err(HaresError::Control(format!(
                    "EV does not handle control signal: {signal:?}"
                )));
            }
        }

        Ok(())
    }

    fn set_charging_curve_lut(
        &mut self,
        lut: Option<crate::ndinterp::RegularGridInterpolator>,
    ) -> crate::Result<()> {
        self.charging_curve_lut = lut;
        Ok(())
    }

    fn has_charging_curve_lut(&self) -> bool {
        self.charging_curve_lut.is_some()
    }

    fn set_ocv_table(&mut self, table: OcvTable) -> crate::Result<()> {
        self.ocv_table = table;
        self.custom_ocv = true;
        Ok(())
    }

    fn set_u_neg_table(&mut self, table: UNegTable) -> crate::Result<()> {
        self.u_neg_table = table;
        self.custom_u_neg = true;
        Ok(())
    }

    fn reset_ocv_table(&mut self) -> crate::Result<()> {
        self.ocv_table = OcvTable::for_chemistry(self.chemistry);
        self.custom_ocv = false;
        Ok(())
    }

    fn reset_u_neg_table(&mut self) -> crate::Result<()> {
        self.u_neg_table = UNegTable::for_chemistry(self.chemistry);
        self.custom_u_neg = false;
        Ok(())
    }

    fn has_custom_ocv_table(&self) -> bool {
        self.custom_ocv
    }

    fn has_custom_u_neg_table(&self) -> bool {
        self.custom_u_neg
    }
}

pub fn register_with_registry(registry: &mut EquipmentRegistry) {
    registry.register("EV", Box::new(|config| Box::new(Ev::new(config))));
    registry.register(
        "Electric Vehicle",
        Box::new(|config| Box::new(Ev::new(config))),
    );
    registry.register(
        "Scheduled EV",
        Box::new(|config| {
            Box::new(crate::scheduled_load::ScheduledLoad::new(
                config,
                hares_types::EndUse::EV,
                "Scheduled EV",
            ))
        }),
    );
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
