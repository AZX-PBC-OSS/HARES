//! Electric vehicle charging equipment model.

use std::borrow::Cow;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, Timelike};
use hares_physics::units::{power_kw_to_w, power_w_to_kw};
use hares_types::telemetry_keys as tk;
use hares_types::zip::{ResolvedZip, ZipLoad};
use hares_types::{
    BatteryChemistry, ChargingLevel, ChargingPriority, ChargingStrategy, ControlCapabilities,
    ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CorePerformance, CoreState, DRLevel,
    ElectricPower, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId, EvConnectionState,
    ExecutionStage, FuelType, HaresError, OperatingMode, PlugInPolicy, PortContribution,
    PortDeclaration, PortSlots, Soc, Telemetry,
};

use crate::battery::ocv::{OcvTable, UNegTable};
use crate::config::constructor_equipment_id;
use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_versioned, try_save_versioned};

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

/// Keep the placeholder value while deferring the parse error to `init()`:
/// the registry factory must construct, but an invalid raw value must not
/// silently become the placeholder. The first deferred error wins; later
/// ones are dropped (init surfaces one cause, not a list).
fn defer_strict<T>(
    result: Result<T, HaresError>,
    fallback: T,
    deferred: &mut Option<HaresError>,
) -> T {
    match result {
        Ok(value) => value,
        Err(err) => {
            if deferred.is_none() {
                *deferred = Some(err);
            }
            fallback
        }
    }
}

fn telemetry_code(level: ChargingLevel) -> f64 {
    match level {
        ChargingLevel::L1 => 1.0,
        ChargingLevel::L2 => 2.0,
    }
}

fn dr_level_code(level: DRLevel) -> f64 {
    match level {
        DRLevel::Normal => 0.0,
        DRLevel::Moderate => 1.0,
        DRLevel::High => 2.0,
        DRLevel::Critical => 3.0,
        DRLevel::GridEmergency => 4.0,
    }
}

/// Minimum CC-CV power multiplier at SOC = 1.0.
/// When no LUT is present and SOC is at or above the transition point,
/// effective charging power is scaled linearly from 1.0 at the transition
/// SOC down to this value at 100% SOC.
const CC_CV_MIN_MULTIPLIER: f64 = 0.3;

pub struct Ev {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    core_output: CoreOutput,

    /// Current usable pack capacity [kWh], degraded from rated by state of
    /// health. Updated at each day boundary in `update_degradation` as
    /// `battery_capacity_kwh_rated * SOH`. This is the divisor used for all
    /// SOC arithmetic (driving, charging, V2L/V2G discharge) so the runtime
    /// SOC, range, and charging duration reflect the aged pack — mirroring
    /// the Battery model's `capacity_kwh_nominal` (battery/mod.rs).
    battery_capacity_kwh: f64,
    /// Rated (beginning-of-life) pack capacity [kWh], held constant from
    /// initialization. Mirrors the Battery model's `capacity_kwh_rated`.
    /// Not stored in the checkpoint: `init` sets it from config, and
    /// `load_state` recomputes `battery_capacity_kwh` from it and the
    /// restored SOH.
    battery_capacity_kwh_rated: f64,
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
    // Pack electrical topology — the I²R heating and terminal-voltage
    // solve share the stationary Battery's `pack_electrical` home.
    n_series: u32,
    n_parallel: u32,
    cell_resistance_ohm: f64,
    v2l_enabled: bool,
    v2l_soc_reserve: f64,
    v2l_max_discharge_kw: f64,
    v2g_enabled: bool,
    v2g_soc_reserve: f64,
    v2g_max_discharge_kw: f64,
    discharge_respects_deadline: bool,
    /// Reversible temperature-capacity derate (the NREL SSC d0 Arrhenius —
    /// the same model the stationary Battery applies): a cold pack holds
    /// less charge. Applied in `refresh_usable_capacity` on top of the
    /// temperature-independent degradation SOH, exactly once.
    capacity_derate_model: crate::battery::CapacityDerateModel,

    soc: f64,
    battery_temp_c: f64,
    heater_active: bool,
    /// Post-cap actual heater draw [W] on the last step — the pack-side DC
    /// draw actually delivered, used identically by the thermal equation,
    /// the pack netting, and the `HEATER_POWER_W` telemetry (the Battery's
    /// computed-value pattern). `heater_active` is true exactly when this
    /// is > 0, so "active but drawing nothing" is unrepresentable.
    heater_draw_w: f64,
    /// I²R ohmic heating [W] in the cell resistance on the last step —
    /// the only pack heating the electrical model produces (charger
    /// conversion losses dissipate in the charger, not the cells).
    ohmic_loss_w: f64,
    /// Pack-side net charge rate [kW] on the last step (positive = cells
    /// gaining energy through their terminals). Operating mode keys on
    /// this, never on the port total: a preconditioning pack (charge leg
    /// zero, heater drawing, charger import raised to the heater's
    /// AC-equivalent) nets exactly zero and reports `Heating`, not
    /// `Charging`.
    pack_net_charge_kw: f64,
    /// Drive energy [kWh] dispatched since the last step, awaiting its
    /// thermal application: the SOC debit happens at signal time; the I²R
    /// heat at the trip's equivalent discharge current is applied over the
    /// next step (which knows the timestep), then the pending clears.
    pending_drive_kwh: f64,
    /// Operating mode computed on the last step (or restored from a
    /// checkpoint). `update_control` reports it — mode is a step outcome,
    /// not something re-derivable from port power mid-control-phase.
    last_mode: OperatingMode,
    connection_state: EvConnectionState,
    away_charger_power_kw: f64,
    away_charge_actual_kw: f64,
    active_power_kw: f64,
    /// Cumulative drive energy [kWh] dispatched by the driver actor but not
    /// deliverable by the pack (a trip longer than the vehicle's remaining
    /// range). Published as `DRIVE_SHORTFALL_KWH` so a drive shortfall is
    /// observable output state, never silently dropped mobility.
    drive_shortfall_kwh: f64,

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
    /// External active power setpoint [kW]. Positive = charging command,
    /// negative = V2G/V2L discharge command. When set, the interaction
    /// with Ready‑By deadline enforcement depends on `charging_priority`:
    /// `DeadlineGuarantee` treats this as a soft floor and raises power
    /// above it when the deadline is urgent; `ExternalAuthority` bypasses
    /// deadline enforcement entirely.
    power_setpoint_kw: Option<f64>,
    /// How the EV resolves conflicts between an external PowerSetpoint and
    /// the internal Ready‑By departure deadline. See [`ChargingPriority`].
    charging_priority: ChargingPriority,
    /// min_soc carried by the last PowerSetpoint signal. Used to enforce
    /// a SOC floor during discharge (OCHRE EV.py:298).
    power_setpoint_min_soc: Option<f64>,
    /// max_soc carried by the last PowerSetpoint signal. Used as a charge
    /// ceiling — when charging, the EV will not exceed this SOC even if
    /// the power setpoint would otherwise allow it. Symmetric with
    /// power_setpoint_min_soc which acts as a discharge floor.
    power_setpoint_max_soc: Option<f64>,
    /// Demand response severity level (e.g. shed load, curtailment).
    dr_level: DRLevel,
    dr_duration_remaining_s: Option<f64>,
    soc_target: Option<f64>,
    soc_target_min: Option<f64>,
    soc_target_max: Option<f64>,

    cc_cv_transition_soc: f64,
    cc_cv_derating: f64,

    // Reactive power / smart-inverter control (V2G/V2L inverter-coupled DER,
    // IEEE 1547-2018 / SAE J3072 require reactive capability).
    /// Reactive-power override [kVAR]. `None` = no override (baseline
    /// power-factor path); `Some(0.0)` is a commanded zero and forces Q = 0.
    q_setpoint_kvar: Option<f64>,
    power_factor: f64,
    charger_capacity_kva: f64,
    /// Reactive power emitted on the last step [kVAR] — same signed value on
    /// port, CoreOutput, and telemetry. Positive = absorbing, negative =
    /// supplying.
    reactive_power_kvar: f64,

    /// Guards against post-registration LUT mutation via the `Equipment` trait
    /// setters. Set to `true` by `Dwelling::add_equipment` → `mark_initialized()`.
    initialized: bool,

    /// Deferred raw-config parse error, surfaced by `init()` before the typed
    /// parse runs (the registry factory cannot fail, so construction-time
    /// strictness is stored here — the same channel PV uses).
    init_error: Option<hares_types::HaresError>,
}

/// The discharge leg a negative `PowerSetpoint` resolves to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DischargeLeg {
    /// Vehicle-to-grid (export to the home bus/grid).
    V2G,
    /// Vehicle-to-load (backup of dedicated loads).
    V2L,
}

/// Resolved power for one charging-physics step, as seen by the port,
/// the pack, and the thermal model. Three named quantities, kept
/// distinct because they diverge exactly when the heater runs:
///
/// - the **charge leg** (AC, charging conversion only) — what the SOC
///   credit and the mode classifier key on;
/// - the **heater draw** (pack-side DC) — fed by the charger's raised
///   import while connected (netting the pack to zero when the charge
///   leg is zero), or by the pack itself during V2L/V2G discharge;
/// - the **export** (AC, negative) — the port power while discharging.
#[derive(Clone, Copy, Debug, Default)]
struct ChargeStepPower {
    /// Charge-leg AC power [kW] — the charging conversion only, never
    /// the heater's AC-equivalent.
    charge_leg_ac_kw: f64,
    /// Actual post-cap heater draw [W], pack-side DC.
    heater_draw_w: f64,
    /// Exported AC power [kW] (negative); zero unless discharging.
    export_ac_kw: f64,
    /// True while resolving a V2L/V2G discharge.
    is_discharge: bool,
}

impl Ev {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(constructor_equipment_id(&config)),
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
                | ControlCapabilities::EV_SET_READY_BY
                | ControlCapabilities::DEMAND_RESPONSE
                | ControlCapabilities::REACTIVE_SETPOINT
                | ControlCapabilities::POWER_FACTOR_SETPOINT,
            core_capabilities: CoreCapabilities::ELECTRIC
                | CoreCapabilities::REACTIVE
                | CoreCapabilities::HAS_SOC
                | CoreCapabilities::HAS_MODE,
            telemetry_fields: telemetry_fields(),
            zone_type: None,
        };

        // Raw-config parsing on this path is strict: an invalid value defers
        // an error that init() surfaces before init_typed runs (the registry
        // factory contract is infallible, so construction cannot fail — the
        // same deferred-error channel PV uses). The placeholder keeps the
        // struct constructible until init rejects it. Production paths supply
        // a typed config, where init_typed applies the same strict parses.
        let mut init_error = None;
        let charging_level = defer_strict(
            config
                .get_str(KEY_CHARGING_LEVEL)
                .or_else(|| config.get_str(KEY_CHARGING_LEVEL_HPXML))
                .map_or(Ok(ChargingLevel::L2), parse_charging_level),
            ChargingLevel::L2,
            &mut init_error,
        );
        let battery_capacity_kwh = resolve_capacity_kwh(&config).unwrap_or(DEFAULT_CAPACITY_KWH);
        let rated_power_kw = resolve_rated_power_kw(&config, charging_level, battery_capacity_kwh)
            .unwrap_or_else(|| default_max_power_kw(&config, charging_level, battery_capacity_kwh));
        let soc_max = config.get_f64(KEY_SOC_MAX).unwrap_or(DEFAULT_SOC_MAX);

        let initial_connection_state = defer_strict(
            config.get_str(KEY_INITIAL_CONNECTION_STATE).map_or(
                Ok(EvConnectionState::HomePluggedIn),
                |s| {
                    s.parse::<EvConnectionState>().map_err(|e| {
                        HaresError::Equipment(format!("invalid initial_connection_state: {e}"))
                    })
                },
            ),
            EvConnectionState::HomePluggedIn,
            &mut init_error,
        );

        let chemistry = defer_strict(
            config
                .get_str(KEY_CHEMISTRY)
                .map_or(Ok(BatteryChemistry::Nmc), |s| {
                    s.parse::<BatteryChemistry>()
                        .map_err(|e| HaresError::Equipment(format!("invalid chemistry: {e}")))
                }),
            BatteryChemistry::Nmc,
            &mut init_error,
        );

        let fuel_economy_kwh_per_mi = config
            .get_f64(KEY_FUEL_ECONOMY_KWH_PER_MI)
            .unwrap_or(DEFAULT_FUEL_ECONOMY_KWH_PER_MI);

        let charging_strategy = defer_strict(
            config.get_str(KEY_CHARGING_STRATEGY).map_or(
                Ok(ChargingStrategy::Immediate { target_soc: 1.0 }),
                parse_charging_strategy,
            ),
            ChargingStrategy::Immediate { target_soc: 1.0 },
            &mut init_error,
        );

        let plug_in_policy = defer_strict(
            config
                .get_str(KEY_PLUG_IN_POLICY)
                .map_or(Ok(PlugInPolicy::Always), parse_plug_in_policy),
            PlugInPolicy::Always,
            &mut init_error,
        );

        let charging_priority = defer_strict(
            config
                .get_str(KEY_CHARGING_PRIORITY)
                .map_or(Ok(ChargingPriority::default()), parse_charging_priority),
            ChargingPriority::default(),
            &mut init_error,
        );

        let ev = Self {
            descriptor,
            ports: vec![PortDeclaration::electrical()],
            telemetry: default_telemetry(charging_level),
            core_output: CoreOutput::default(),
            battery_capacity_kwh,
            battery_capacity_kwh_rated: battery_capacity_kwh,
            charging_level,
            rated_power_kw,
            charging_efficiency: config.get_f64(KEY_EFFICIENCY).unwrap_or(DEFAULT_EFFICIENCY),
            l1_current_a: config.get_f64(KEY_L1_CURRENT_A),
            l1_voltage_v: config
                .get_f64(KEY_L1_VOLTAGE_V)
                .unwrap_or(DEFAULT_L1_VOLTAGE_V),
            soc_max,
            charging_curve_lut: None,
            min_charge_temp_c: config
                .get_f64(KEY_MIN_CHARGE_TEMP_C)
                .unwrap_or(DEFAULT_MIN_CHARGE_TEMP_C),
            full_power_temp_c: config
                .get_f64(KEY_FULL_POWER_TEMP_C)
                .unwrap_or(DEFAULT_FULL_POWER_TEMP_C),
            heater_power_w: config
                .get_f64(KEY_HEATER_POWER_W)
                .unwrap_or(DEFAULT_HEATER_POWER_W),
            heater_threshold_c: config
                .get_f64(KEY_HEATER_THRESHOLD_C)
                .unwrap_or(DEFAULT_HEATER_THRESHOLD_C),
            thermal_mass_j_per_k: config
                .get_f64(KEY_THERMAL_MASS_J_PER_K)
                .unwrap_or_else(|| default_thermal_mass_j_per_k(battery_capacity_kwh)),
            ua_w_per_k: config
                .get_f64(KEY_UA_W_PER_K)
                .unwrap_or_else(|| default_ua_w_per_k(battery_capacity_kwh)),
            n_series: config
                .get_f64(KEY_N_SERIES)
                .map(|v| v as u32)
                .unwrap_or(DEFAULT_N_SERIES),
            n_parallel: config
                .get_f64(KEY_N_PARALLEL)
                .map(|v| v as u32)
                .unwrap_or_else(|| default_n_parallel(battery_capacity_kwh)),
            cell_resistance_ohm: config
                .get_f64(KEY_CELL_RESISTANCE_OHM)
                .unwrap_or(DEFAULT_CELL_RESISTANCE_OHM),
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
            discharge_respects_deadline: true,
            capacity_derate_model: crate::battery::CapacityDerateModel::default(),
            soc: config
                .get_f64(KEY_INITIAL_SOC)
                .unwrap_or(DEFAULT_SOC)
                .clamp(0.0, 1.0),
            battery_temp_c: config
                .get_f64(KEY_BATTERY_TEMP_C)
                .unwrap_or(DEFAULT_BATTERY_TEMP_C),
            heater_active: false,
            heater_draw_w: 0.0,
            ohmic_loss_w: 0.0,
            pack_net_charge_kw: 0.0,
            pending_drive_kwh: 0.0,
            last_mode: OperatingMode::Off,
            connection_state: initial_connection_state,
            away_charger_power_kw: 0.0,
            away_charge_actual_kw: 0.0,
            active_power_kw: 0.0,
            drive_shortfall_kwh: 0.0,
            ready_soc: config.get_f64(KEY_READY_SOC).unwrap_or(soc_max),
            ready_by_hour: None,
            ready_by_soc: None,
            chemistry,
            fuel_economy_kwh_per_mi,
            custom_ocv: false,
            custom_u_neg: false,
            v2l_active: false,
            v2l_power_kw: 0.0,
            charging_strategy,
            plug_in_policy,
            power_limit_kw: config.get_f64(KEY_POWER_LIMIT_KW),
            power_setpoint_kw: None,
            charging_priority,
            power_setpoint_min_soc: None,
            power_setpoint_max_soc: None,
            dr_level: DRLevel::Normal,
            dr_duration_remaining_s: None,
            soc_target: None,
            soc_target_min: None,
            soc_target_max: None,
            cc_cv_transition_soc: config
                .get_f64(KEY_CC_CV_TRANSITION_SOC)
                .unwrap_or(DEFAULT_CC_CV_TRANSITION_SOC),
            cc_cv_derating: 1.0,
            q_setpoint_kvar: None,
            power_factor: config.get_f64(KEY_POWER_FACTOR).unwrap_or(1.0),
            charger_capacity_kva: config.get_f64(KEY_CHARGER_CAPACITY_KVA).unwrap_or_else(|| {
                rated_power_kw
                    .max(config.get_f64(KEY_V2G_MAX_DISCHARGE_KW).unwrap_or(0.0))
                    .max(config.get_f64(KEY_V2L_MAX_DISCHARGE_KW).unwrap_or(0.0))
            }),
            reactive_power_kvar: 0.0,
            initialized: false,
            init_error,
            degradation: crate::battery::degradation::DegradationState::default(),
            rainflow: crate::battery::degradation::RainflowCounter::default(),
            ocv_table: OcvTable::for_chemistry(chemistry),
            u_neg_table: UNegTable::for_chemistry(chemistry),
            last_daily_update_day: 0,
        };
        tracing::info!(
            battery_temp_c = ev.battery_temp_c,
            "EV constructed (battery_temp_c is the pre-init placeholder; init resolves it by cascade: explicit config, else outdoor ambient)"
        );
        ev
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
        self.battery_capacity_kwh_rated = c.capacity_kwh;

        self.charging_level = match c.charging_level.as_deref() {
            None => ChargingLevel::L2,
            Some(s) => config::parse_charging_level(s)?,
        };

        self.charging_efficiency = c.charging_efficiency.unwrap_or(DEFAULT_EFFICIENCY);
        self.l1_current_a = c.l1_current_a;
        self.l1_voltage_v = c.l1_voltage_v.unwrap_or(DEFAULT_L1_VOLTAGE_V);
        self.soc_max = c.soc_max.unwrap_or(DEFAULT_SOC_MAX);

        // The charging level caps the EVSE hardware: an L1 coupler cannot
        // deliver L2 power. The clamp is correct physics; the warn makes the
        // adjustment observable so a contradictory config is never silent.
        let (level_label, (min_kw, max_kw)) = match self.charging_level {
            ChargingLevel::L1 => ("L1", (L1_MIN_POWER_KW, L1_MAX_POWER_KW)),
            ChargingLevel::L2 => ("L2", (L2_MIN_POWER_KW, L2_MAX_POWER_KW)),
        };
        let clamped = c.max_charging_power_kw.clamp(min_kw, max_kw);
        if clamped != c.max_charging_power_kw {
            tracing::warn!(
                equipment = %self.descriptor.name,
                charging_level = level_label,
                requested_kw = c.max_charging_power_kw,
                applied_kw = clamped,
                bounds = ?[min_kw, max_kw],
                "max_charging_power_kw adjusted to charging-level hardware bounds"
            );
        }
        self.rated_power_kw = clamped;

        self.min_charge_temp_c = c.min_charge_temp_c.unwrap_or(DEFAULT_MIN_CHARGE_TEMP_C);
        self.full_power_temp_c = c.full_power_temp_c.unwrap_or(DEFAULT_FULL_POWER_TEMP_C);
        self.heater_power_w = c.heater_power_w.unwrap_or(DEFAULT_HEATER_POWER_W);
        self.heater_threshold_c = c.heater_threshold_c.unwrap_or(DEFAULT_HEATER_THRESHOLD_C);
        self.thermal_mass_j_per_k = c
            .thermal_mass_j_per_k
            .unwrap_or_else(|| default_thermal_mass_j_per_k(self.battery_capacity_kwh_rated));
        self.ua_w_per_k = c
            .ua_w_per_k
            .unwrap_or_else(|| default_ua_w_per_k(self.battery_capacity_kwh_rated));
        self.n_series = c.n_series.unwrap_or(DEFAULT_N_SERIES);
        self.n_parallel = c
            .n_parallel
            .unwrap_or_else(|| default_n_parallel(self.battery_capacity_kwh_rated));
        self.cell_resistance_ohm = c.cell_resistance_ohm.unwrap_or(DEFAULT_CELL_RESISTANCE_OHM);

        self.cc_cv_transition_soc = c
            .cc_cv_transition_soc
            .unwrap_or(DEFAULT_CC_CV_TRANSITION_SOC);

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

        self.discharge_respects_deadline = c.discharge_respects_deadline;
        self.capacity_derate_model = crate::battery::CapacityDerateModel::default();

        self.chemistry = match c.chemistry.as_deref() {
            Some(s) => s
                .parse::<BatteryChemistry>()
                .map_err(|e| HaresError::Equipment(format!("invalid chemistry: {e}")))?,
            None => BatteryChemistry::Nmc,
        };
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
        self.charging_priority = c.charging_priority.unwrap_or_default();

        self.power_factor = c.power_factor.unwrap_or(1.0);
        self.charger_capacity_kva = c.charger_capacity_kva.unwrap_or_else(|| {
            self.rated_power_kw
                .max(self.v2g_max_discharge_kw)
                .max(self.v2l_max_discharge_kw)
        });
        self.q_setpoint_kvar = None;

        self.connection_state = match c.initial_connection_state.as_deref() {
            Some(s) => s.parse::<EvConnectionState>().map_err(|e| {
                HaresError::Equipment(format!("invalid initial_connection_state: {e}"))
            })?,
            None => EvConnectionState::HomePluggedIn,
        };

        let initial_soc = c.initial_soc.unwrap_or(DEFAULT_SOC);
        self.soc = initial_soc.clamp(0.0, 1.0);

        // Degradation tracking starts fresh at init, with the day boundary
        // anchored to the environment's current day — the Battery's init
        // contract (battery/mod.rs init_typed). Without the anchor, the
        // first step would fire a spurious day boundary carrying a
        // one-step "day" integral; without the reset, a re-init would
        // carry stale degradation state.
        self.degradation = crate::battery::degradation::DegradationState::default();
        self.rainflow = crate::battery::degradation::RainflowCounter::default();
        self.last_daily_update_day = {
            use chrono::Datelike;
            env.current_time.date_naive().num_days_from_ce()
        };

        // Pack temperature initialization — the Battery's cascade
        // (battery/mod.rs init_typed): explicit config wins, else the
        // zone's temperature, else outdoor ambient. The EV carries no zone
        // (`zone: None` on the descriptor — pack sits outdoors/garage, no
        // zone thermal coupling by design), so the cascade here is config,
        // else outdoor ambient. `DEFAULT_BATTERY_TEMP_C` is only the
        // pre-init construction placeholder: with the day-scale post-fix
        // thermal mass (τ ≈ 8.5 h at 75 kWh) a wrong initial temperature
        // would persist past a day, so `init` binds real weather — the
        // dwelling calls it with `initial_env` from
        // `environment.update(&clock, &[])`, never a zeroed state.
        self.battery_temp_c = c.battery_temp_c.unwrap_or(env.weather.outdoor_temp_c);
        tracing::info!(
            battery_temp_c = self.battery_temp_c,
            source = if c.battery_temp_c.is_some() {
                "config"
            } else {
                "outdoor_ambient"
            },
            "EV pack temperature initialized"
        );

        if let Some(strat_str) = c.charging_strategy.as_deref() {
            self.charging_strategy = parse_charging_strategy(strat_str)?;
        } else {
            self.charging_strategy = ChargingStrategy::Immediate { target_soc: 1.0 };
        }

        if let Some(policy_str) = c.plug_in_policy.as_deref() {
            self.plug_in_policy = parse_plug_in_policy(policy_str)?;
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
        self.heater_draw_w = 0.0;
        self.ohmic_loss_w = 0.0;
        self.pack_net_charge_kw = 0.0;
        self.pending_drive_kwh = 0.0;
        self.last_mode = OperatingMode::Off;
        self.active_power_kw = 0.0;
        self.v2l_active = false;
        self.v2l_power_kw = 0.0;
        self.power_setpoint_kw = None;
        self.power_setpoint_min_soc = None;
        self.power_setpoint_max_soc = None;
        self.soc_target = None;
        self.soc_target_min = None;
        self.soc_target_max = None;
        self.cc_cv_derating = 1.0;
        self.reactive_power_kvar = 0.0;
        // The usable capacity is temperature-dependent (the reversible
        // derate) and the pack temperature was just resolved by the
        // cascade above — refresh before the first telemetry so the
        // init-time reading (and any signal that lands before the first
        // step, e.g. an EvDrive) uses the thermally-scaled divisor.
        self.refresh_usable_capacity();
        self.telemetry = default_telemetry(self.charging_level);
        self.core_output = CoreOutput::default();
        self.write_telemetry();

        Ok(())
    }

    /// Compute the charge-leg demand [kW AC] for this timestep: the power
    /// the BMS wants to charge at, after capability (rated power, charging
    /// level, temperature derate / LUT), the headroom-to-target taper, the
    /// commanded `power_setpoint_kw` (a *total*-draw bound with the
    /// heater's AC-equivalent carved out — see `heater_ac_eq_kw`), and the
    /// Ready-By deadline logic. Supply-cap allocation between the charge
    /// leg and the heater, and the DR multiplier, happen in
    /// [`run_charging_physics`] — one allocator, one priority rule.
    ///
    /// Returns `(demand_kw, deadline_raise_active)`: the flag is true when
    /// a `DeadlineGuarantee` ready-by deadline raised the demand above what
    /// the commanded setpoint alone would allow — the documented exception
    /// under which the allocator lets the total draw exceed the command
    /// (the setpoint is a soft floor under that priority; without the flag
    /// the allocator could not distinguish a BMS raise from an
    /// setpoint-derived demand and would either defeat the deadline
    /// contract or let the heater ignore dispatched commands).
    ///
    /// `charge_derate` is applied before the taper limit so that the taper
    /// correctly prevents SOC overshoot even under cold-temperature derating.
    /// `max_power_kw` is the charger's rated power (home rated or away charger).
    fn compute_charging_power_kw(
        &mut self,
        now: DateTime<FixedOffset>,
        dt: Duration,
        charge_derate: f64,
        max_power_kw: f64,
        heater_ac_eq_kw: f64,
    ) -> crate::Result<(f64, bool)> {
        // `min_charge_temp_c` safety cutoff, unconditional: charging Li-ion
        // at or below the plating boundary is blocked under every
        // configuration, including a charging-curve LUT whose temperature
        // axis was fitted only above the cutoff (the normal measurement
        // regime) — a curve cannot grant permission the BMS physics denies.
        if self.battery_temp_c <= self.min_charge_temp_c {
            return Ok((0.0, false));
        }

        // A controller-set `soc_target` and a latched ready-by deadline
        // target are two different contracts — "charge toward this
        // ceiling" and "reach this SOC by departure_hour" — and which one
        // wins when both are set is exactly what the configured
        // `ChargingPriority` decides:
        //
        // - `DeadlineGuarantee` (the default): the deadline is the hard
        //   contract — deadline enforcement deliberately outranks soft
        //   controller limits (the same contract that lets an urgent
        //   deadline charge through a latched zero-power hold). The
        //   effective destination is the max of the two: a controller
        //   target above the deadline's raises the destination, while one
        //   below it (e.g. the driver actor's range-anxiety minimal top-up
        //   at the anxiety band) cannot silently veto the deadline. With
        //   a plain `.or()` here, a latched band target both shrank
        //   `bms_ready_by_power`'s deficit basis (a maximally urgent
        //   deadline recomputed its deficit against the band, concluded
        //   "plenty of time", and commanded zero) and capped the early
        //   return below the departure target — nothing charged while the
        //   deadline went unmet.
        //
        // - `ExternalAuthority`: the external controller bears sole
        //   responsibility for the departure SOC — "BMS deadline logic is
        //   not applied" (the priority arm's own contract). The
        //   controller's commanded destination therefore governs: it
        //   shadows a deadline target latched earlier in the session,
        //   exactly as its setpoint shadows the BMS's pacing, so a stale
        //   higher deadline target must not silently raise where the
        //   controller told the pack to stop.
        //
        // With no controller target set there is nothing to compete with
        // the deadline's own target, which is the destination under both
        // priorities.
        let target = match (self.soc_target, self.ready_by_soc) {
            (Some(controller), Some(deadline)) => match self.charging_priority {
                ChargingPriority::DeadlineGuarantee => controller.max(deadline),
                ChargingPriority::ExternalAuthority => controller,
            },
            (Some(controller), None) => controller,
            (None, Some(deadline)) => deadline,
            (None, None) => self.ready_soc,
        };
        let soc_limit = {
            let mut limit = target;
            if let Some(min) = self.soc_target_min {
                limit = limit.max(min);
            }
            if let Some(max) = self.soc_target_max {
                limit = limit.min(max);
            }
            if let Some(max) = self.power_setpoint_max_soc {
                limit = limit.min(max);
            }
            limit.clamp(0.0, self.soc_max)
        };

        if self.soc >= soc_limit {
            return Ok((0.0, false));
        }

        let dt_hours = (dt.as_secs_f64() / SECONDS_PER_HOUR).max(MIN_TIMESTEP_HOURS);
        let rated = match self.charging_level {
            ChargingLevel::L1 => self.l1_power_kw().min(max_power_kw),
            ChargingLevel::L2 => self.rated_power_kw.min(max_power_kw),
        };
        // The shared c-rate rule (pack_electrical): power over the
        // degradation-adjusted rating, never the live usable capacity —
        // the LUT's own [soc, temp, c_rate, soh] axes already carry the
        // temperature and degradation effects, so a temperature-derated
        // divisor would apply the temperature twice (the c-rate inflated
        // ≈1.3× at 0 °C, bin-shifting the lookup) and diverge from the
        // Battery sibling (whose `capacity_kwh_nominal` is the same
        // rated × SOH rating). Hoisted above the `as_mut` borrow: the
        // method reads all of `self`, which the LUT borrow excludes.
        let lut_divisor_kwh = self.degradation_adjusted_capacity_kwh();
        let curve_limited_rated = if let Some(lut) = self.charging_curve_lut.as_mut() {
            let soh = 1.0 - self.degradation.capacity_fade_fraction();
            let c_rate = crate::pack_electrical::charging_lut_c_rate(rated, lut_divisor_kwh);
            rated * lut.interpolate(&[self.soc, self.battery_temp_c, c_rate, soh])? as f64
        } else {
            rated
        };
        // One physical effect, applied once: a configured charging-curve
        // LUT's temperature axis IS the temperature-dependent charge
        // capability — a measured curve already contains the
        // manufacturer's low-temperature derate — so the linear BMS ramp's
        // derate is NOT multiplied on top of it. Without a LUT the linear
        // ramp (`min_charge_temp_c` → `full_power_temp_c`) is the model.
        // The plating cutoff above applies either way.
        let derated_rated = if self.charging_curve_lut.is_some() {
            curve_limited_rated
        } else {
            curve_limited_rated * charge_derate
        };

        // A commanded `power_setpoint_kw` is a total-draw bound: the
        // heater's AC-equivalent is carved out within the command and the
        // charge leg receives the remainder, so the vehicle never draws
        // more than its dispatch commands. With no heater running this is
        // the plain setpoint bound.
        let setpoint_charge_bound_kw = match self.power_setpoint_kw {
            Some(sp) => (sp - heater_ac_eq_kw).max(0.0).min(derated_rated),
            None => derated_rated,
        };
        let mut requested = self
            .power_setpoint_kw
            .map_or(derated_rated, |_| setpoint_charge_bound_kw)
            .max(0.0)
            .min(derated_rated);

        let mut cc_cv_mult = 1.0_f64;

        // Diagnostic captures for the deadline‑vs‑setpoint interaction.
        #[cfg(feature = "observe")]
        let mut ev_ready_by_bypassed = false;
        #[cfg(feature = "observe")]
        let mut ev_ready_by_power_before_setpoint: Option<f64> = None;
        #[cfg(feature = "observe")]
        let mut ev_ready_by_power_after_setpoint: Option<f64> = None;

        // Deadline‑vs‑setpoint resolution. The BMS Ready‑By logic always
        // runs when a deadline is set; how its result interacts with an
        // external PowerSetpoint depends on `charging_priority`.
        if self.ready_by_hour.is_some() {
            let bms_power = self.bms_ready_by_power(now, derated_rated, soc_limit);
            cc_cv_mult = if self.charging_curve_lut.is_some() {
                1.0
            } else {
                Self::cc_cv_taper_multiplier(self.soc, self.cc_cv_transition_soc)
            };

            match self.charging_priority {
                ChargingPriority::DeadlineGuarantee => {
                    if self.power_setpoint_kw.is_some() {
                        // External setpoint is a soft floor: BMS deadline
                        // enforcement raises power above the setpoint when
                        // the deadline is urgent. When the BMS reports no
                        // urgency (returns 0.0), the external setpoint is
                        // honoured unchanged. PowerLimit is still applied
                        // as a final cap after this max operation.
                        #[cfg(feature = "observe")]
                        {
                            ev_ready_by_power_before_setpoint = Some(requested);
                        }
                        requested = bms_power.max(requested);
                        #[cfg(feature = "observe")]
                        {
                            ev_ready_by_power_after_setpoint = Some(requested);
                        }
                    } else {
                        // No external setpoint: BMS has full control.
                        requested = bms_power;
                    }
                }
                ChargingPriority::ExternalAuthority => {
                    if self.power_setpoint_kw.is_some() {
                        // External controller bears sole responsibility;
                        // BMS deadline logic is not applied. CC‑CV
                        // tapering is a BMS‑level mechanism; since the
                        // external setpoint is used verbatim, the
                        // telemetry must report no CC‑CV derating here.
                        cc_cv_mult = 1.0;
                        #[cfg(feature = "observe")]
                        {
                            ev_ready_by_bypassed = true;
                        }
                        #[cfg(any(debug_assertions, feature = "check_invariants"))]
                        {
                            let ready_by_hour = self.ready_by_hour.unwrap_or(0.0);
                            let current_hour = now.hour() as f64
                                + now.minute() as f64 / 60.0
                                + now.second() as f64 / 3600.0;
                            let hours_remaining = if ready_by_hour > current_hour {
                                ready_by_hour - current_hour
                            } else {
                                24.0 - current_hour + ready_by_hour
                            };
                            tracing::warn!(
                                soc = self.soc,
                                target_soc = soc_limit,
                                hours_remaining,
                                power_setpoint_kw = self.power_setpoint_kw,
                                "EV Ready‑By deadline enforcement bypassed by external \
                                 PowerSetpoint (charging_priority = ExternalAuthority): \
                                 external controller bears sole responsibility for \
                                 departure SOC"
                            );
                        }
                    } else {
                        requested = bms_power;
                    }
                }
            }
        }

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            if let (Some(_ready_hour), Some(sp_kw)) = (self.ready_by_hour, self.power_setpoint_kw) {
                if sp_kw > 0.0 {
                    // When both Ready-By and PowerSetpoint are active, the
                    // setpoint originates from the scheduler's urgency power
                    // (ctx.max_charge_kw). It must not exceed the equipment's
                    // rated power by more than a small tolerance — divergence
                    // beyond 110% indicates the schedule and equipment
                    // configuration disagree about the EV's capabilities.
                    let max_allowed = self.rated_power_kw * 1.1;
                    if sp_kw > max_allowed {
                        tracing::warn!(
                            ready_by_hour = self.ready_by_hour,
                            power_setpoint_kw = sp_kw,
                            rated_power_kw = self.rated_power_kw,
                            "EV: PowerSetpoint exceeds rated power while Ready-By active — \
                             scheduler urgency power inconsistent with equipment capabilities"
                        );
                    }
                }
            }
        }

        self.cc_cv_derating = cc_cv_mult;

        #[cfg(feature = "observe")]
        {
            let lut_active = self.charging_curve_lut.is_some();
            let lut_derate = if derated_rated > 0.0 {
                curve_limited_rated / rated
            } else {
                1.0
            };
            let eff_before = derated_rated * self.charging_efficiency;
            let eff_after = eff_before * cc_cv_mult;
            tracing::debug!(
                lut_active,
                lut_derate,
                eff_power_before_cc_cv = eff_before,
                eff_power_after_cc_cv = eff_after,
                ev_cc_cv_derating = cc_cv_mult,
                ev_ready_by_bypassed,
                ev_ready_by_power_before_setpoint,
                ev_ready_by_power_after_setpoint,
                soc = self.soc,
                "compute_charging_power_kw: LUT active={lut_active}, CC‑CV derating multiplier={cc_cv_mult}",
            );
        }

        // Taper bound on the charge leg: the AC power that lands SOC exactly
        // on the target in one step. With the heater diverting DC, the
        // charger's raised import covers the diversion, so the charge-leg
        // bound is unchanged (`headroom/η`) — mathematically the same rule
        // as bounding the *total* import at `(headroom_dc + heater_dc)/η`
        // with the heater carved out; either form lands the pack exactly on
        // target, never above it.
        let taper_limit = (soc_limit - self.soc).max(0.0) * self.battery_capacity_kwh
            / dt_hours
            / self.charging_efficiency;

        let power = requested.min(derated_rated).min(taper_limit).max(0.0);
        // A DeadlineGuarantee ready-by raise pushed the demand above what
        // the commanded setpoint alone would allow — the documented
        // exception the allocator honors (see the method doc).
        let deadline_raise_active = self
            .power_setpoint_kw
            .is_some_and(|_| power > setpoint_charge_bound_kw + 1e-12);
        Ok((power, deadline_raise_active))
    }
    /// Returns the CC‑CV tapering multiplier for a given SOC.
    ///
    /// When `soc < transition_soc`: multiplier = 1.0 (constant-power CC region).
    /// When `soc >= transition_soc`: linear taper from 1.0 at `transition_soc`
    /// to `CC_CV_MIN_MULTIPLIER` at SOC = 1.0 (CV taper region).
    fn cc_cv_taper_multiplier(soc: f64, transition_soc: f64) -> f64 {
        if soc < transition_soc {
            return 1.0;
        }
        let range = 1.0 - transition_soc;
        if range <= 0.0 {
            return 1.0;
        }
        if soc >= 1.0 {
            return CC_CV_MIN_MULTIPLIER;
        }
        let t = (soc - transition_soc) / range;
        (1.0 - (1.0 - CC_CV_MIN_MULTIPLIER) * t).max(CC_CV_MIN_MULTIPLIER)
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

        let cc_cv_mult = if self.charging_curve_lut.is_some() {
            1.0
        } else {
            Self::cc_cv_taper_multiplier(self.soc, self.cc_cv_transition_soc)
        };

        let eff_power = derated_rated * self.charging_efficiency * cc_cv_mult;
        let hours_needed = if eff_power > 0.0 {
            soc_deficit * self.battery_capacity_kwh / eff_power
        } else {
            return derated_rated;
        };

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            if self.charging_curve_lut.is_some() {
                assert!(
                    cc_cv_mult == 1.0,
                    "CC‑CV margin must not be applied when a charging‑curve LUT is present"
                );
            }
        }

        #[cfg(feature = "observe")]
        if self.charging_curve_lut.is_none() && self.soc >= self.cc_cv_transition_soc {
            tracing::debug!(
                soc = self.soc,
                transition_soc = self.cc_cv_transition_soc,
                cc_cv_taper_mult = cc_cv_mult,
                hours_needed,
                "bms_ready_by_power: SOC‑based CC‑CV taper applied in no‑LUT path",
            );
        }

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

    /// Effective discharge floor: `max(reserve, commanded min_soc,
    /// ready_by_soc)` — the SOC the pack never lands below while
    /// discharging. Shared by both discharge legs (they differ only in
    /// reserve and power ceiling); the pre-fix V2L/V2G twins were
    /// byte-identical apart from those two constants.
    fn effective_discharge_floor(&self, leg: DischargeLeg) -> f64 {
        let reserve = match leg {
            DischargeLeg::V2G => self.v2g_soc_reserve,
            DischargeLeg::V2L => self.v2l_soc_reserve,
        };
        let reserve_floor = self
            .power_setpoint_min_soc
            .map_or(reserve, |ms| reserve.max(ms));

        let effective_floor = if self.discharge_respects_deadline
            && let Some(ready_by_soc) = self.ready_by_soc
        {
            reserve_floor.max(ready_by_soc)
        } else {
            reserve_floor
        };

        #[cfg(feature = "observe")]
        tracing::debug!(
            leg = ?leg,
            ev_discharge_floor_effective = effective_floor,
            ev_discharge_deadline_interlocked = (effective_floor > reserve_floor),
            soc = self.soc,
            "effective discharge floor = {effective_floor}, interlocked = {}",
            effective_floor > reserve_floor,
        );

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            let expected_min = if let (true, Some(ready_by_soc)) =
                (self.discharge_respects_deadline, self.ready_by_soc)
            {
                let signal_floor = self
                    .power_setpoint_min_soc
                    .map_or(reserve, |ms| reserve.max(ms));
                signal_floor.max(ready_by_soc)
            } else {
                reserve_floor
            };
            assert!(
                effective_floor >= expected_min,
                "{leg:?} effective discharge floor {effective_floor} less than required minimum \
                 {expected_min} (reserve={reserve}, ready_by_soc={:?}, respects_deadline={})",
                self.ready_by_soc,
                self.discharge_respects_deadline,
            );
        }

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            let interlock_prevented = effective_floor > reserve_floor
                && self.soc <= effective_floor
                && self.soc > reserve_floor;
            if interlock_prevented {
                tracing::warn!(
                    leg = ?leg,
                    soc = self.soc,
                    reserve,
                    ready_by_soc = self.ready_by_soc,
                    effective_floor,
                    "discharge prevented by Ready‑By deadline interlock: \
                     SOC {:.4} below effective floor {:.4} (reserve={:.4})",
                    self.soc,
                    effective_floor,
                    reserve,
                );
            }
        }

        effective_floor
    }
    /// Resolve a discharge step: the exported AC power (negative, DR-scaled)
    /// and the actual heater draw [kW DC] the pack supplies alongside it.
    ///
    /// The discharge budget covers the **total pack-side draw** — the export
    /// converted at `charging_efficiency` plus the heater's unconverted DC
    /// draw — so the pack never lands below the effective floor: not past
    /// the reserve, not past a commanded `min_soc`, not past `ready_by_soc`.
    /// At the floor both the export and the heater stop (preconditioning
    /// protects future mobility; draining the pack below the departure
    /// floor to warm it destroys the very mobility it protects).
    ///
    /// Priority below the chargeable temperature mirrors the charge-side
    /// rule: the heater takes headroom priority (preconditioning is the
    /// precondition for any future recharge); above it the export draws
    /// first and the heater takes the remainder.
    ///
    /// DR composition is unchanged from the pre-fix code: the fraction is
    /// a multiplier on the exported power; the heater's pack-side draw is
    /// bounded by the floor invariant, not by the DR fraction.
    fn compute_discharge(
        &self,
        leg: DischargeLeg,
        dt: Duration,
        heater_desired_dc_kw: f64,
    ) -> (f64, f64) {
        let effective_floor = self.effective_discharge_floor(leg);
        if self.soc <= effective_floor {
            // At the floor both the export and the heater stop.
            return (0.0, 0.0);
        }
        let setpoint_magnitude = self.power_setpoint_kw.unwrap_or(0.0).abs();
        let max_discharge_kw = match leg {
            DischargeLeg::V2G => self.v2g_max_discharge_kw,
            DischargeLeg::V2L => self.v2l_max_discharge_kw,
        };
        let dt_hours = (dt.as_secs_f64() / SECONDS_PER_HOUR).max(MIN_TIMESTEP_HOURS);
        let eta = self.charging_efficiency.max(0.01);
        // Budget the cap in the DC domain the pack actually draws from: the
        // pack delivers the exported AC power at 1/charging_efficiency in
        // DC, so the sustainable AC power for the available DC energy is
        // that energy times the efficiency, spread over the step — or every
        // floor landing overshoots by one step's (1/efficiency − 1).
        let available_dc_kw = (self.soc - effective_floor) * self.battery_capacity_kwh / dt_hours;
        let commanded_ac = setpoint_magnitude.min(max_discharge_kw).max(0.0);

        let (heater_dc_kw, export_ac_kw) = if self.battery_temp_c <= self.min_charge_temp_c {
            // Heater priority below the chargeable temperature.
            let heater = heater_desired_dc_kw.min(available_dc_kw).max(0.0);
            let export = commanded_ac.min((available_dc_kw - heater).max(0.0) * eta);
            (heater, export)
        } else {
            // Export priority; the heater takes the remaining DC headroom.
            let export = commanded_ac.min(available_dc_kw * eta);
            let heater = heater_desired_dc_kw
                .min((available_dc_kw - export / eta).max(0.0))
                .max(0.0);
            (heater, export)
        };

        (-(export_ac_kw * self.dr_power_fraction()), heater_dc_kw)
    }

    fn charge_derate_factor(&self) -> f64 {
        crate::linear_temp_derate(
            self.battery_temp_c,
            self.min_charge_temp_c,
            self.full_power_temp_c,
        )
    }

    fn dr_power_fraction(&self) -> f64 {
        match self.dr_level {
            DRLevel::Normal => 1.0,
            DRLevel::Moderate => 0.8,
            DRLevel::High => 0.5,
            DRLevel::Critical => 0.25,
            DRLevel::GridEmergency => 0.0,
        }
    }

    fn l1_power_kw(&self) -> f64 {
        match self.l1_current_a {
            Some(current_a) => power_w_to_kw(current_a * self.l1_voltage_v).max(0.0),
            None => self.rated_power_kw,
        }
    }

    /// Compute reactive power [kVAR] for the given grid-side active power.
    ///
    /// Precedence (mirrors battery/mod.rs exactly):
    /// 1. `q_setpoint_kvar` (from `ReactiveSetpoint` or
    ///    `PowerSetpoint.reactive_power_kvar`) — absolute override, passes
    ///    through as-commanded.
    /// 2. Else `power_factor` baseline: `Q = P · tan(acos(pf))` via
    ///    `ZipLoad::reactive_only` (no inline formula). Baseline sign follows
    ///    var flow: charging P>0 → Q>0 absorbing; V2G/V2L discharge P<0 →
    ///    Q<0 supplying.
    ///
    /// kVA clamp: `|Q| ≤ sqrt(max(0, S² − P²))` with
    /// `S = charger_capacity_kva` — active-power priority (P never
    /// curtailed by Q).
    fn compute_reactive_kvar(&self, active_power_kw: f64) -> f64 {
        // A `Some` q_setpoint (including a commanded 0.0) is an absolute
        // override; only `None` falls through to the power-factor baseline.
        let q = match self.q_setpoint_kvar {
            Some(q) => q,
            None if self.power_factor < 1.0 => {
                let zip = ZipLoad::reactive_only(0.0, 0.0, 1.0, self.power_factor);
                active_power_kw * zip.tan_phi()
            }
            None => 0.0,
        };
        let s = self.charger_capacity_kva;
        let p2 = active_power_kw * active_power_kw;
        let q_max = (s * s - p2).max(0.0).sqrt();
        q.clamp(-q_max, q_max)
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
        // Value-carrying: the actual post-cap heater draw (the Battery's
        // computed-value pattern), not the nameplate gated on a bool — a
        // throttled or DR-scaled heater reports its true draw, including
        // the commanded-zero case (0 W).
        self.telemetry.set(tk::HEATER_POWER_W, self.heater_draw_w);
        // I²R cell heating on the last step — the only pack heating the
        // electrical model produces.
        self.telemetry.set(tk::OHMIC_LOSS_W, self.ohmic_loss_w);
        self.telemetry
            .set(tk::CHARGE_DERATE, self.charge_derate_factor());
        self.telemetry.set(tk::CC_CV_DERATE, self.cc_cv_derating);
        self.telemetry
            .set(tk::V2L_ACTIVE, if self.v2l_active { 1.0 } else { 0.0 });
        self.telemetry.set(tk::V2L_POWER_KW, self.v2l_power_kw);
        self.telemetry.set(
            tk::CAPACITY_FADE_PCT,
            self.degradation.capacity_fade_fraction() * 100.0,
        );
        self.telemetry
            .set(tk::AWAY_CHARGE_POWER_KW, self.away_charge_actual_kw);
        self.telemetry
            .set(tk::CAPACITY_KWH, self.battery_capacity_kwh);
        self.telemetry
            .set(tk::CAPACITY_KWH_RATED, self.battery_capacity_kwh_rated);
        self.telemetry
            .set(tk::FUEL_ECONOMY_KWH_PER_MI, self.fuel_economy_kwh_per_mi);
        self.telemetry
            .set(tk::DRIVE_SHORTFALL_KWH, self.drive_shortfall_kwh);
        self.telemetry
            .set(tk::DR_POWER_FRACTION, self.dr_power_fraction());
        self.telemetry
            .set(tk::DR_LEVEL, dr_level_code(self.dr_level));
        self.telemetry
            .set(tk::REACTIVE_POWER_KVAR, self.reactive_power_kvar);

        #[cfg(feature = "observe")]
        tracing::debug!(
            effective_wall_kwh_per_mi = self.fuel_economy_kwh_per_mi,
            battery_capacity_kwh = self.battery_capacity_kwh,
            battery_capacity_kwh_rated = self.battery_capacity_kwh_rated,
            capacity_fade_pct = self.degradation.capacity_fade_fraction() * 100.0,
            charging_efficiency = self.charging_efficiency,
            "EV runtime: effective wall-to-wheels fuel economy diagnostic",
        );
    }

    /// Usable pack capacity [kWh]: rated × SOH × the reversible
    /// temperature derate — the same factored layering as the stationary
    /// Battery (`capacity_kwh = capacity_kwh_nominal × derate`,
    /// battery/mod.rs): the degradation fade stays temperature-independent
    /// (see the `deg_const` notes on why both branches' reversible
    /// temperature scaling lives at the equipment layer), and the
    /// temperature-capacity contraction — a cold pack holds less charge,
    /// Smith 2017 Eq. 3's d0 model, the NREL SSC d0 Arrhenius the
    /// Battery's default derate carries — applies exactly once.
    ///
    /// Called at each step (the pack temperature moves every step) and at
    /// the degradation day boundary (the SOH moves daily). SOC is
    /// stored-energy over usable capacity, so the BMS reading of a
    /// warming/cooling pack moves with the divisor — the physical
    /// behavior a real BMS reports.
    fn refresh_usable_capacity(&mut self) {
        let soh = 1.0 - self.degradation.capacity_fade_fraction();
        let derate = self.capacity_derate_model.evaluate(self.battery_temp_c);
        self.battery_capacity_kwh = self.battery_capacity_kwh_rated * soh * derate;
    }

    /// Degradation-adjusted pack capacity [kWh] — `rated × SOH`, the
    /// temperature-independent rating (the EV's counterpart of the
    /// Battery's `capacity_kwh_nominal`). Boundaries that consume a rating
    /// — the driver's belief model (`actor_seed`) and the charging-LUT
    /// c-rate divisor — use this, never the live usable capacity: the
    /// usable capacity moves with the pack temperature (the reversible
    /// derate) and with every degradation update, so seeding it into a
    /// fixed one-shot value would freeze one step's weather into the
    /// whole run (a −7 °C init would shrink the driver's believed pack
    /// by roughly a third and raise every range-anxiety threshold with
    /// it).
    fn degradation_adjusted_capacity_kwh(&self) -> f64 {
        self.battery_capacity_kwh_rated * (1.0 - self.degradation.capacity_fade_fraction())
    }

    fn update_degradation(&mut self, env: &EnvironmentState, dt_s: f64) -> crate::Result<()> {
        // OCHRE Battery.py:315-346: calculate_degradation() runs *before*
        // degradation_data.append() so the midnight timestep belongs to the
        // *next* day's degradation window.  HARES mirrors this ordering:
        // the day-boundary check and update_daily() run *before* the current
        // step's rainflow.push() and degradation.accumulate().
        let current_day = {
            use chrono::Datelike;
            env.current_time.date_naive().num_days_from_ce()
        };
        if current_day != self.last_daily_update_day {
            self.degradation
                .update_daily(&self.u_neg_table, &self.rainflow);

            // Feed the aged state of health back into the usable pack
            // capacity so runtime SOC arithmetic (driving, charging, V2L/V2G)
            // reflects the degraded pack. Mirrors the Battery model's daily
            // update `capacity_kwh_nominal = capacity_kwh_rated * SOH`
            // (battery/mod.rs). Without this the EV would move SOC using the
            // undegraded divisor, understating range loss and charge duration.
            self.refresh_usable_capacity();
            let soh = 1.0 - self.degradation.capacity_fade_fraction();
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            {
                if !(self.battery_capacity_kwh > 0.0 || soh <= 0.0) {
                    return Err(HaresError::InvariantViolation {
                        check_name: "ev_battery_capacity_kwh_underflow".to_string(),
                        value: self.battery_capacity_kwh,
                        tolerance: 0.0,
                    });
                }
            }

            self.degradation.reset_day_tracking(self.soc);
            self.rainflow.reset_daily();
            self.last_daily_update_day = current_day;
        }

        let cell_temp_k = self.battery_temp_c + 273.15;
        let v_oc = self.ocv_table.voltage_at_soc(self.soc);
        self.rainflow.push(self.soc);
        self.degradation
            .accumulate(dt_s, cell_temp_k, v_oc, self.soc)?;
        Ok(())
    }

    /// The shared pack-electrical view of this EV's topology.
    fn pack_electrical(&self) -> crate::pack_electrical::PackElectrical {
        crate::pack_electrical::PackElectrical {
            n_series: self.n_series,
            n_parallel: self.n_parallel,
            cell_resistance_ohm: self.cell_resistance_ohm,
        }
    }

    /// Resolve one step of charging physics: heater gate, charge demand,
    /// discharge export, supply-cap allocation with the priority rule,
    /// and the DR multiplier on the allocated AC total.
    ///
    /// Alignment with the stationary Battery's architecture:
    ///
    /// - **Heater gate** — activates on pack temperature alone whenever the
    ///   vehicle is connected with an energized supply: charging, idle, or
    ///   discharging (Battery precedent: the heater protects cells from
    ///   freezing regardless of charge/discharge demand — Tesla PW3 Heat
    ///   Mode practice). The pre-fix gate required charge demand, so a
    ///   pack whose charging was derated to zero by the plating cutoff
    ///   could not be preconditioned by any configuration while the heater
    ///   still billed energy — dead exactly when needed. The discharge-side
    ///   floor bound is enforced inside `compute_discharge` (at the floor
    ///   both the export and the heater stop). `Disconnected` stays
    ///   drift-only; a de-energized home bus and an uncommanded away
    ///   charger both remove the supply.
    /// - **Heater placement** — a pack-side DC load (HV PTC, as in real
    ///   EVs), not an AC-side pad heater like the stationary Battery's.
    ///   While connected, the charger's import covers it (the BMS raise),
    ///   so the pack nets zero in the heater-only state and the port
    ///   carries the charger's AC input alone in every state — import when
    ///   charging or covering the heater, export when discharging. The
    ///   heater never discharges the pack while plugged in.
    /// - **Supply cap & priority** — the bound on the charger's AC input is
    ///   `min(rating, dispatched power_limit)` at home (the away branch
    ///   uses the away charger's commanded rating), covering charging plus
    ///   the heater's AC-equivalent. The priority rule swaps exactly at
    ///   `min_charge_temp_c`: below it charging is physically zero and
    ///   warming the pack is the only path to ever charging, so the heater
    ///   takes the bound; above it charging draws first and the heater
    ///   takes the remainder — the heater never consumes dispatch budget
    ///   that charging could have used, so a dispatched limit can never
    ///   produce a heater-monopolized blackout. A *commanded*
    ///   `power_setpoint_kw` is a total-draw bound: the heater is carved
    ///   out within the command and the charge leg receives the remainder,
    ///   so the vehicle never draws more than its dispatch commands — with
    ///   one documented exception: a `DeadlineGuarantee` ready-by deadline
    ///   may raise the charge demand above the command (the setpoint is a
    ///   soft floor under that priority), and then the supply-bound rule
    ///   governs the allocation.
    /// - **DR** — the fraction is a multiplier on the allocated AC total
    ///   (charge leg + heater AC-equivalent), exactly as the code has
    ///   always applied it to charge power; never a term inside the `min`
    ///   (the withdrawn ceiling form agrees with the multiplier only at
    ///   full demand and would silently weaken dispatch obedience in the
    ///   derated regime). A commanded zero (`GridEmergency`, or
    ///   `power_limit_kw = 0`) zeroes the port.
    /// - **Explicit-Euler overshoot note** — the heater's per-step
    ///   temperature rise is bounded by `P·dt/C` (≈9 K at the 5 kW default,
    ///   480 kJ/K mass, 900 s steps): a thermostat duty cycle around the
    ///   threshold, bounded and stable at the simulation's design
    ///   resolution; coarser steps coarsen it the same way they coarsen
    ///   the ambient relaxation.
    fn run_charging_physics(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
    ) -> crate::Result<ChargeStepPower> {
        let charge_derate = self.charge_derate_factor();

        // Discharge leg: a negative setpoint at home with V2G/V2L enabled.
        let discharge_leg = match self.connection_state {
            EvConnectionState::HomePluggedIn
                if self.power_setpoint_kw.is_some_and(|sp| sp < 0.0) =>
            {
                if self.v2g_enabled {
                    Some(DischargeLeg::V2G)
                } else if self.v2l_enabled {
                    Some(DischargeLeg::V2L)
                } else {
                    None
                }
            }
            _ => None,
        };

        // Heater gate (see the method doc).
        let heater_eligible = self.heater_power_w > 0.0
            && self.battery_temp_c <= self.heater_threshold_c
            && match self.connection_state {
                // At home the EVSE supply is the bus: charging, idle, or
                // discharging (the vehicle itself keeps an islanded bus
                // energized). A de-energized bus removes the supply.
                EvConnectionState::HomePluggedIn => {
                    discharge_leg.is_some() || env.grid.bus_energized()
                }
                // Away: the only modeled supply is the commanded away
                // charger.
                EvConnectionState::AwayPluggedIn => self.away_charger_power_kw > 0.0,
                // Contact open: drift only.
                EvConnectionState::Disconnected => false,
            };
        let heater_desired_dc_kw = if heater_eligible {
            power_w_to_kw(self.heater_power_w)
        } else {
            0.0
        };
        let heater_ac_eq_kw = heater_desired_dc_kw / self.charging_efficiency.max(0.01);

        if let Some(leg) = discharge_leg {
            let (export_ac_kw, heater_dc_kw) =
                self.compute_discharge(leg, dt, heater_desired_dc_kw);
            return Ok(ChargeStepPower {
                charge_leg_ac_kw: 0.0,
                heater_draw_w: power_kw_to_w(heater_dc_kw),
                export_ac_kw,
                is_discharge: true,
            });
        }

        // Charger capability bound: the EVSE rating for the configured
        // level (home rated power, or the away charger's commanded rating).
        let max_power_kw = match self.connection_state {
            EvConnectionState::HomePluggedIn => self.rated_power_kw,
            EvConnectionState::AwayPluggedIn => self.away_charger_power_kw,
            EvConnectionState::Disconnected => 0.0,
        };
        let evse_rating_kw = match self.charging_level {
            ChargingLevel::L1 => self.l1_power_kw(),
            ChargingLevel::L2 => self.rated_power_kw,
        }
        .min(max_power_kw);

        let (charge_demand_kw, deadline_raise_active) = self.compute_charging_power_kw(
            env.current_time,
            dt,
            charge_derate,
            max_power_kw,
            heater_ac_eq_kw,
        )?;

        // Supply bound on the charger's AC input: `min(rating, dispatched
        // power_limit)` at home; the away charger's rating away (the away
        // site's EVSE is not dispatched through the home's power_limit).
        let supply_bound_kw = match self.connection_state {
            EvConnectionState::HomePluggedIn => self
                .power_limit_kw
                .map(|lim| evse_rating_kw.min(lim.max(0.0)))
                .unwrap_or(evse_rating_kw),
            _ => evse_rating_kw,
        };
        // A commanded `power_setpoint_kw` (including a commanded 0.0) is a
        // total-draw bound on charge + heater AC-equivalent — UNLESS a
        // `DeadlineGuarantee` ready-by deadline raised the charge demand
        // above the command (the setpoint is a soft floor under that
        // priority, a pinned contract): then the supply-bound priority
        // rule governs and the total may legitimately exceed the command.
        let commanded_total_kw = self
            .power_setpoint_kw
            .filter(|sp| *sp >= 0.0)
            .filter(|_| !deadline_raise_active)
            .map(|sp| supply_bound_kw.min(sp.max(0.0)));
        let total_bound_kw = commanded_total_kw.unwrap_or(supply_bound_kw);
        // Within a binding command the heater is carved out first and the
        // charge leg receives the remainder (the vehicle never draws more
        // than its dispatch commands); within the supply bound charging
        // draws first and the heater takes the remainder.
        let setpoint_binding = commanded_total_kw.is_some_and(|c| c < supply_bound_kw);

        let (charge_ac_kw, heater_ac_kw) = if self.battery_temp_c <= self.min_charge_temp_c {
            // Below the plating cutoff charging is physically zero; warming
            // the pack is the only path to ever charging.
            let h = heater_ac_eq_kw.min(total_bound_kw);
            (0.0, h)
        } else if setpoint_binding {
            let h = heater_ac_eq_kw.min(total_bound_kw);
            let c = charge_demand_kw.min((total_bound_kw - h).max(0.0));
            (c, h)
        } else {
            // Supply-bound: charging draws first, the heater takes the
            // remainder — the heater never takes budget charging could use.
            let c = charge_demand_kw.min(total_bound_kw);
            let h = heater_ac_eq_kw.min((total_bound_kw - c).max(0.0));
            (c, h)
        };

        // DR multiplier on the allocated AC total.
        let dr = self.dr_power_fraction();
        Ok(ChargeStepPower {
            charge_leg_ac_kw: charge_ac_kw * dr,
            heater_draw_w: power_kw_to_w(heater_ac_kw * dr * self.charging_efficiency.max(0.01)),
            export_ac_kw: 0.0,
            is_discharge: false,
        })
    }

    /// Apply the resolved power to SOC and pack temperature — one unified
    /// equation for every connection state:
    ///
    /// `dT = (Q_I2R + Q_heater − UA·(T − T_ambient)) · dt / C`
    ///
    /// Pack heating is I²R through the cell resistance only (the shared
    /// `pack_electrical` solve). A model that injects the charger's AC→DC
    /// conversion loss `(1−η)·P` into the pack — ~1.15 kW at Level 2
    /// against a ~480 kJ/K mass — drives packs to 165–281 °C and the
    /// Arrhenius degradation fit out of its domain; conversion losses
    /// dissipate in the charger's power electronics, not the cells.
    /// The Bernardi reversible-entropic term `I·T·(dU/dT)` is omitted: at
    /// Level 2 currents it is ~1.7 W against ~10 W ohmic (both dwarfed by
    /// ~110 W of UA loss and the kilowatt-class heater in the regime this
    /// model matters for) — a documented modeling boundary, not a
    /// silently-held approximation. Drive energy heats the pack at its
    /// equivalent discharge current through the same solve — the
    /// discharge-side instance of the same loss-attribution rule.
    fn apply_soc_and_thermal(&mut self, dt: Duration, step: &ChargeStepPower, ambient_c: f64) {
        let dt_s = dt.as_secs_f64();
        let dt_hours = (dt_s / SECONDS_PER_HOUR).max(MIN_TIMESTEP_HOURS);
        let eta = self.charging_efficiency.max(0.01);
        let heater_dc_kw = power_w_to_kw(step.heater_draw_w);

        // Pack-side net rate — what the cells actually gain or lose
        // through their terminals. While connected the charger's raised
        // import covers the heater, so the net rate is exactly the charge
        // leg's DC credit — the heater never slows SOC gain while plugged
        // in (the additional-load identity); during discharge the pack
        // supplies the export conversion plus the heater's unconverted
        // DC draw.
        //
        // The same quantity is the I²R basis. The heater current crosses
        // the cell internal resistance only on discharge, where the pack
        // is its source; while connected the heater is a parallel DC load
        // on the charger bus and its current never passes through the
        // cells, so it must not enter the ohmic solve — its heat reaches
        // the pack through the thermal equation instead. Feeding the
        // heater-inclusive bus draw to the solve would count the heater's
        // current as cell heating while SOC nets it out of the pack: two
        // pictures of the same state that cannot both be true (the
        // D2 loss-attribution class — one effect, attributed once).
        let net_dc_kw = if step.is_discharge {
            let export_dc_kw = -step.export_ac_kw / eta;
            -(export_dc_kw + heater_dc_kw)
        } else {
            step.charge_leg_ac_kw * eta
        };

        // SOC via the pack-side net rate. A fully-degraded pack (SOH ≤ 0,
        // usable capacity 0) cannot store energy — the same guard the
        // stationary Battery applies — and degradation tracking still
        // continues (it updates SOH and must not be skipped).
        if self.battery_capacity_kwh > 0.0 {
            self.soc =
                (self.soc + net_dc_kw * dt_hours / self.battery_capacity_kwh).clamp(0.0, 1.0);
        }
        self.pack_net_charge_kw = net_dc_kw;

        // Drive energy dispatched since the last step: the SOC debit
        // already happened at signal time; apply the I²R heat at the trip's
        // equivalent discharge current over this step (the driver actor
        // dispatches `EvDrive` per step, so the equivalent power is
        // `pending/dt_hours`).
        let drive_dc_kw = if self.pending_drive_kwh > 0.0 {
            let p = self.pending_drive_kwh / dt_hours;
            self.pending_drive_kwh = 0.0;
            p
        } else {
            0.0
        };

        // I²R through the shared solve on the cells' net DC power (the
        // drive draw is a terminal current like the charge leg — drives
        // happen while disconnected, so the two never co-occur).
        let cell_ocv = self.ocv_table.voltage_at_soc(self.soc);
        let solved = self
            .pack_electrical()
            .solve(cell_ocv, power_kw_to_w(net_dc_kw - drive_dc_kw));
        self.ohmic_loss_w = solved.ohmic_loss_w;

        // Unified lumped thermal equation.
        let q_in_w = self.ohmic_loss_w + step.heater_draw_w;
        let q_loss_w = self.ua_w_per_k * (self.battery_temp_c - ambient_c);
        self.battery_temp_c += (q_in_w - q_loss_w) * dt_s / self.thermal_mass_j_per_k;
    }

    /// Operating mode from the pack-side net charge rate and the export
    /// leg — never the port total alone — keyed on **exact signs**, the
    /// same exactness the mode-flow guard's zero test enforces (`is_zero()`
    /// is an exact match on 0.0, `hares-types/src/equipment.rs`). A
    /// "treat-as-nothing" tolerance here would classify sub-threshold
    /// power as `Off` while the published electric flow carries the
    /// actual value, and Rule 2 (Off forbids non-zero flows) would fail
    /// the dwelling step — the taper limit is a continuum
    /// `(soc_limit − soc) · capacity / dt / η`, so a landing that rounds
    /// even one ULP short of the target produces a nonzero charge leg
    /// inside (0, 1e-9] kW, reachable from ordinary charging.
    ///
    /// A charger-fed preconditioning pack (charge leg zero, heater
    /// drawing, charger import raised to the heater's AC-equivalent)
    /// nets exactly zero and reports `Heating`; a pack-fed
    /// preconditioning pack (a commanded V2L/V2G export curtailed to
    /// zero — e.g. a GridEmergency DR event — while the discharge-side
    /// heater draws from the pack) nets negative with a zero port and
    /// reports `Standby`: the vehicle is energized and connected but
    /// exchanges nothing at the port, the same label the stationary
    /// Battery reports for a commanded-zero discharge — the only
    /// guard-valid label, because the pack-side draw is port-invisible
    /// by design (a pack-side DC load, never a port flow), so no active
    /// mode's nonzero-flow requirement can be satisfied honestly.
    /// `Discharging` keys on the export leg, not the pack net: a negative
    /// net from the heater alone is internal preconditioning, not a
    /// grid-facing discharge. An EV commanded to vars at zero real power
    /// is genuinely active (the inverter is exchanging reactive power —
    /// standby var support, a normal smart-inverter mode) and reports
    /// `On`; all-zero flows report `Off`.
    ///
    /// `Heating` reuse boundary: the variant's into-zone meaning applies to
    /// zoned equipment; the EV carries `zone: None` (pack outdoors/garage,
    /// no zone coupling) and produces thermal output at the equipment level
    /// only — the dwelling-level zone classifiers are zone-guarded and
    /// never see it. Giving the EV a zone would put a preconditioning pack
    /// into those classifiers while emitting no zone thermal power; that
    /// invariant is documented here so the change is not made casually.
    fn classify_mode(
        net_charge_kw: f64,
        export_ac_kw: f64,
        heater_draw_w: f64,
        reactive_kvar: f64,
    ) -> OperatingMode {
        if net_charge_kw > 0.0 {
            OperatingMode::Charging
        } else if export_ac_kw < 0.0 {
            OperatingMode::Discharging
        } else if heater_draw_w > 0.0 {
            if net_charge_kw >= 0.0 {
                OperatingMode::Heating
            } else {
                OperatingMode::Standby
            }
        } else if reactive_kvar != 0.0 {
            OperatingMode::On
        } else {
            OperatingMode::Off
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
        // Surface a deferred raw-config parse error before the typed parse:
        // an invalid raw value must not silently stand in as its placeholder.
        if let Some(e) = self.init_error.take() {
            return Err(e);
        }
        self.init_typed(config, env)
    }

    fn island_source_available(&self) -> bool {
        // An EV plugged in at home and actively discharging (V2L/V2G) is a
        // source that can hold the home bus energized during a utility
        // outage. A merely plugged-in EV is NOT counted — most EVSEs cannot
        // island a home, and HARES only dispatches EV discharge on explicit
        // (negative) setpoints. Uses the previous step's discharge state, so
        // EV-driven islanding takes effect one step after discharge begins.
        matches!(self.connection_state, EvConnectionState::HomePluggedIn)
            && self.v2l_active
            && self.soc > 0.0
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        if let Some(remaining) = self.dr_duration_remaining_s.as_mut() {
            *remaining -= env.time_res.num_seconds() as f64;
            if *remaining <= 0.0 {
                self.dr_level = DRLevel::Normal;
                self.dr_duration_remaining_s = None;
            }
        }
        // Mode is a step outcome, keyed on the pack-side net charge rate
        // (see `classify_mode`): the control phase runs between steps, so
        // the last computed mode is the honest report — re-deriving it
        // from port power here would misclassify a preconditioning pack
        // (charge leg zero, heater drawing) as Charging.
        self.last_mode
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        let ambient_c = env.weather.outdoor_temp_c;
        // The usable capacity moves with the pack temperature every step
        // (the reversible derate) and with SOH at day boundaries.
        self.refresh_usable_capacity();
        let mut power = self.run_charging_physics(env, dt)?;

        // Grid outage (de-energized home bus): the EVSE has no supply, so
        // charging and battery preconditioning stop — gated at the control
        // decision (WH precedent; the heater gate already excludes a dead
        // bus, this re-zeroing keeps the charge leg honest too). V2L/V2G
        // *discharge* is NOT gated: the EV is then a source (it can island
        // the home — see `island_source_available`). Islanded homes keep an
        // energized bus, so charging from on-site backup remains possible.
        // See docs/outage-behavior.md.
        if self.connection_state == EvConnectionState::HomePluggedIn
            && !power.is_discharge
            && !env.grid.bus_energized()
        {
            power.charge_leg_ac_kw = 0.0;
            power.heater_draw_w = 0.0;
        }

        match self.connection_state {
            EvConnectionState::HomePluggedIn => {
                self.v2l_active = power.is_discharge;
                self.v2l_power_kw = if power.is_discharge {
                    -power.export_ac_kw
                } else {
                    0.0
                };

                // Billing rule: the port carries the charger's AC input
                // alone in every state — import when charging or covering
                // the heater (the BMS raise), export when discharging. The
                // pack-side heater is not an AC-side load, so it adds no
                // port power of its own.
                let inverter_leg_kw = if power.is_discharge {
                    power.export_ac_kw
                } else {
                    power.charge_leg_ac_kw
                };
                self.active_power_kw = if power.is_discharge {
                    power.export_ac_kw
                } else {
                    power.charge_leg_ac_kw
                        + power_w_to_kw(power.heater_draw_w) / self.charging_efficiency.max(0.01)
                };

                self.away_charge_actual_kw = 0.0;

                // Reactive power: smart-inverter var control (IEEE 1547-2018 /
                // SAE J3072). Both the power-factor baseline and the kVA
                // headroom clamp key on the *inverter leg* — the charge/
                // discharge conversion — never the heater-inclusive port
                // total: a PTC pack heater is resistive and DC-fed, not
                // inverter-coupled, so it produces no vars and consumes no
                // inverter kVA headroom (the same inverter-leg keying the
                // stationary Battery uses, where its port P adds the
                // standby and cell-heater loads on top). A de-energized bus
                // produces no vars either — a commanded q-setpoint cannot
                // be served by a dead EVSE.
                let q_kvar = if power.is_discharge || env.grid.bus_energized() {
                    self.compute_reactive_kvar(inverter_leg_kw)
                } else {
                    0.0
                };
                self.reactive_power_kvar = q_kvar;

                ports.accumulate(&PortContribution::Electrical {
                    active_power_w: power_kw_to_w(self.active_power_kw),
                    reactive_power_kvar: q_kvar,
                })?;
            }
            EvConnectionState::AwayPluggedIn => {
                // Away charging: no V2L/V2G, no residential port contribution
                // (an office car park must not appear on the home meter). The
                // away charger supplies both the charge leg and the heater
                // off-site; the heater's energy appears in no dwelling
                // energy balance — the load genuinely is off-site.
                // `away_charge_actual_kw` keeps its documented meaning:
                // the actual charge intake after taper, never the rating,
                // and never overloaded with heater draw — the heater draw
                // is observable through `HEATER_POWER_W`.
                self.v2l_active = false;
                self.v2l_power_kw = 0.0;
                self.active_power_kw = 0.0;
                self.reactive_power_kvar = 0.0;
                self.away_charge_actual_kw = power.charge_leg_ac_kw;
            }
            EvConnectionState::Disconnected => {
                // Contact open: no grid connection, Q must be 0, no supply
                // for the heater (thermal drift + drive I²R only, applied
                // by the unified equation below).
                self.active_power_kw = 0.0;
                self.away_charge_actual_kw = 0.0;
                self.v2l_active = false;
                self.v2l_power_kw = 0.0;
                self.reactive_power_kvar = 0.0;
            }
        }

        // The heater ran exactly when it drew power — "active but drawing
        // nothing" is unrepresentable.
        self.heater_active = power.heater_draw_w > 0.0;
        // The actual post-cap draw is step state: telemetry, the pack
        // netting (already applied via `power`), and checkpointing all read
        // this field.
        self.heater_draw_w = power.heater_draw_w;

        self.apply_soc_and_thermal(dt, &power, ambient_c);
        // Re-refresh after the thermal update: the published pair
        // (`CAPACITY_KWH`, `BATTERY_TEMP_C`) must be self-consistent — the
        // capacity the telemetry reports must be the one the reported
        // temperature implies. The SOC arithmetic above correctly used the
        // step-start divisor (the energy moved under the conditions that
        // held when it moved); this refresh is for publication and the
        // next step's arithmetic.
        self.refresh_usable_capacity();
        self.update_degradation(env, dt.as_secs_f64())?;
        self.write_telemetry();
        self.last_mode = match self.connection_state {
            // The away arm contributes nothing to the dwelling: zero port
            // contribution and zero CoreOutput flows — an off-site load must
            // not enter the dwelling's electrical summary (which feeds BMS
            // dispatch through `EnvironmentState.electrical`). The mode/flow
            // guard requires the mode to agree with those flows, so the
            // away mode is `Off`, the dwelling-side truth (the pre-fix
            // output column behaved identically). Away charging remains
            // observable through `AWAY_CHARGE_POWER_KW` and the heater
            // through `HEATER_POWER_W`.
            EvConnectionState::AwayPluggedIn => OperatingMode::Off,
            _ => Self::classify_mode(
                self.pack_net_charge_kw,
                power.export_ac_kw,
                power.heater_draw_w,
                self.reactive_power_kvar,
            ),
        };
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Bidirectional(self.active_power_kw)),
                reactive_power_kvar: Some(self.reactive_power_kvar),
                fuel_w: None,
                thermal_output_w: None,
                sensible_cooling_w: None,
                latent_cooling_w: None,
            },
            state: CoreState {
                operating_mode: Some(self.last_mode),
                soc: Soc::try_from(self.soc).ok(),
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

    fn resolved_zip(&self) -> Option<ResolvedZip> {
        // Live runtime state: constant-power reactive-only ZIP carrying the
        // current effective power factor (config baseline, later mutated by
        // PowerFactorSetpoint) — mirrors `compute_reactive_kvar`'s baseline.
        // Real power is charging-strategy-controlled, never ZIP-scaled, so
        // the resolved regime is reactive-only.
        Some(ResolvedZip::reactive_only(ZipLoad::reactive_only(
            0.0,
            0.0,
            1.0,
            self.power_factor,
        )))
    }

    fn actor_seed(&self) -> Option<crate::ActorSeed> {
        // The driver actor is the vehicle-use simulation (departures, trips,
        // SOC depletion), not just charging-schedule logic: every strategy,
        // `Immediate` included, needs it or the EV never leaves home, never
        // discharges, and so never has anything to charge back.
        Some(crate::ActorSeed::Ev {
            strategy: self.charging_strategy.clone(),
            plug_in_policy: self.plug_in_policy.clone(),
            // The degradation-adjusted rating, never the live usable
            // capacity: the actor absorbs this once and never reassigns it,
            // so a temperature-scaled value would freeze the init-time
            // weather into the run's range-anxiety and needed-hours
            // arithmetic (see `degradation_adjusted_capacity_kwh`).
            capacity_kwh: self.degradation_adjusted_capacity_kwh(),
            max_charge_kw: self.rated_power_kw,
            fuel_economy_kwh_per_mi: self.fuel_economy_kwh_per_mi,
        })
    }

    fn save_state(&self) -> crate::Result<Vec<u8>> {
        try_save_versioned(
            &EvCheckpoint {
                soc: self.soc,
                connection_state: self.connection_state,
                away_charger_power_kw: self.away_charger_power_kw,
                active_power_kw: self.active_power_kw,
                power_limit_kw: self.power_limit_kw,
                power_setpoint_kw: self.power_setpoint_kw,
                power_setpoint_min_soc: self.power_setpoint_min_soc,
                power_setpoint_max_soc: self.power_setpoint_max_soc,
                dr_level: self.dr_level,
                dr_duration_remaining_s: self.dr_duration_remaining_s,
                soc_target: self.soc_target,
                soc_target_min: self.soc_target_min,
                soc_target_max: self.soc_target_max,
                battery_temp_c: self.battery_temp_c,
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
                q_setpoint_kvar: self.q_setpoint_kvar,
                power_factor: self.power_factor,
                drive_shortfall_kwh: self.drive_shortfall_kwh,
                pending_drive_kwh: self.pending_drive_kwh,
                reactive_power_kvar: self.reactive_power_kvar,
                heater_draw_w: self.heater_draw_w,
                v2l_active: self.v2l_active,
                v2l_power_kw: self.v2l_power_kw,
                away_charge_actual_kw: self.away_charge_actual_kw,
                last_mode: self.last_mode,
            },
            Self::checkpoint_version(),
            "Ev",
        )
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let cp: EvCheckpoint = load_versioned(
            state,
            Self::checkpoint_version(),
            "Ev",
            self.descriptor().id,
        )?;
        self.soc = cp.soc;
        self.connection_state = cp.connection_state;
        self.away_charger_power_kw = cp.away_charger_power_kw;
        self.active_power_kw = cp.active_power_kw;
        self.power_limit_kw = cp.power_limit_kw;
        self.power_setpoint_kw = cp.power_setpoint_kw;
        self.power_setpoint_min_soc = cp.power_setpoint_min_soc;
        self.power_setpoint_max_soc = cp.power_setpoint_max_soc;
        self.dr_level = cp.dr_level;
        self.dr_duration_remaining_s = cp.dr_duration_remaining_s;
        self.soc_target = cp.soc_target;
        self.soc_target_min = cp.soc_target_min;
        self.soc_target_max = cp.soc_target_max;
        self.battery_temp_c = cp.battery_temp_c;
        // The heater draw is checkpointed and restored verbatim (the same
        // lossless pattern the reactive flow uses): its basis — the
        // charge-leg/heater split of the port total — is transient step
        // state, not derivable at restore, and zeroing it would pair the
        // restored `Heating` mode (which `classify_mode` only ever
        // produces with a nonzero draw) with a zero heater column while
        // the port total still carries the heater's AC-equivalent — a
        // state no live step publishes. `heater_active` is derived from
        // the restored draw (draw > 0) — the pairing the step's own
        // value-carrying telemetry contract makes canonical (active
        // exactly when drawing).
        self.heater_draw_w = cp.heater_draw_w;
        self.heater_active = cp.heater_draw_w > 0.0;
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
        self.q_setpoint_kvar = cp.q_setpoint_kvar;
        self.power_factor = cp.power_factor;
        self.drive_shortfall_kwh = cp.drive_shortfall_kwh;
        self.pending_drive_kwh = cp.pending_drive_kwh;
        self.last_mode = cp.last_mode;

        // `battery_capacity_kwh_rated` is static config set by `init`, not
        // stored in the checkpoint (the caller must call `init` before
        // `load_state`). Recompute the usable capacity from the rated
        // capacity, the restored SOH, and the restored pack temperature's
        // derate so SOC arithmetic resumes with the aged, thermally-scaled
        // divisor. Mirrors Battery::load_state (battery/mod.rs).
        self.refresh_usable_capacity();

        // The V2L columns are checkpointed and restored verbatim (the
        // same lossless pattern as the heater draw and reactive flow): the
        // step keys `v2l_active` on dispatch **presence** (a negative
        // setpoint latched with V2L/V2G enabled), not on the computed
        // export — so it stays true at the reserve floor and under export
        // DR while the export computes to exactly zero, and the mode is
        // `Off` in those corners (zero pack net rate). Deriving either
        // column from checkpointed state — `last_mode`, the port sign —
        // therefore diverges from the saved state exactly where
        // `island_source_available` (the dwelling's islanding reads it)
        // needs the dispatch-active answer; this corner is what proved
        // the earlier `last_mode` derivation unfaithful.
        self.v2l_active = cp.v2l_active;
        self.v2l_power_kw = cp.v2l_power_kw;
        // The away session's actual intake is checkpointed and restored
        // verbatim (the same lossless pattern): it is the charge-side
        // counterpart of `v2l_power_kw` — the documented way away charging
        // is observed (`AWAY_CHARGE_POWER_KW`) — and its basis (the away
        // charge leg) is not derivable at restore (the away state
        // publishes zero port power, so no checkpointed column
        // reconstructs it). Leaving it zeroed would misreport an active
        // away session as drawing nothing until the next step recomputes
        // the leg — the same restored-state-disagrees-with-saved-state
        // family the heater draw, reactive flow, and V2L dispatch state
        // fixes closed.
        self.away_charge_actual_kw = cp.away_charge_actual_kw;
        // The ohmic loss and pack-side net rate are step outcomes the
        // next step recomputes from live state; the heater draw and the
        // reactive flow are step-published values restored verbatim (the
        // lossless pattern — see the comments at their restore sites).
        // The step keys the reactive computation on the **inverter leg** —
        // the charge/export conversion only, because the DC-fed pack
        // heater is resistive and produces no vars and consumes no kVA
        // headroom — and that basis is not derivable at restore (the
        // heater draw that splits the port total into leg + heater
        // AC-equivalent is transient step state). Recomputing over the
        // restored port total — the heater-inclusive basis — would
        // attribute the heater's watts to the inverter: inflating the
        // power-factor baseline in idle preconditioning (vars from a
        // resistive load) and zeroing the kVA headroom a commanded
        // q-setpoint needs at a bound-binding session. The verbatim
        // restore keeps the published (mode, flows) pair both
        // guard-valid and faithful to the step-published values.
        self.reactive_power_kvar = cp.reactive_power_kvar;
        self.ohmic_loss_w = 0.0;
        self.pack_net_charge_kw = 0.0;

        self.write_telemetry();
        self.core_output = {
            CoreOutput {
                flows: CoreFlows {
                    electric_kw: Some(ElectricPower::Bidirectional(self.active_power_kw)),
                    reactive_power_kvar: Some(self.reactive_power_kvar),
                    fuel_w: None,
                    thermal_output_w: None,
                    sensible_cooling_w: None,
                    latent_cooling_w: None,
                },
                state: CoreState {
                    operating_mode: Some(self.last_mode),
                    soc: Soc::try_from(self.soc).ok(),
                    speed_index: None,
                    setpoint_c: None,
                },
                performance: CorePerformance::default(),
            }
        };
        Ok(())
    }

    fn checkpoint_version() -> u32 {
        // v2: EvCheckpoint gained the reactive-control fields.
        // v3: `q_setpoint_kvar` became Option<f64> (None = no var override;
        //     Some(0.0) is a commanded zero).
        // v4: gained `drive_shortfall_kwh` (cumulative drive-energy
        //     shortfall accounting; see the EvDrive arm in apply_signal).
        // v5: gained `pending_drive_kwh` (drive energy awaiting its I²R
        //     thermal application on the next step) and `last_mode` (the
        //     step-outcome operating mode `update_control` reports); dropped
        //     `heater_active` (derived state — true exactly when the actual
        //     draw > 0, recomputed every step).
        // v6: gained `reactive_power_kvar` (the step-published reactive
        //     flow, restored verbatim — the inverter-leg basis it is
        //     computed on is not derivable from the checkpoint).
        // v7: gained `heater_draw_w` (the step-published heater draw,
        //     restored verbatim — the charge-leg/heater split of the port
        //     total is transient step state, not derivable at restore);
        //     `heater_active` is derived from the restored draw (draw > 0).
        // v8: gained `v2l_active`/`v2l_power_kw` (the step-published V2L
        //     dispatch state and export, restored verbatim — the step keys
        //     the dispatch on presence, not the computed export, so a
        //     floor-held or DR-zeroed dispatch publishes dispatch-active
        //     with zero export and mode `Off`, which no checkpointed basis
        //     can derive; the floor-held corner proved the v7-era
        //     `last_mode` derivation unfaithful there).
        // v9: gained `away_charge_actual_kw` (the step-published away
        //     session intake, restored verbatim — its basis, the away
        //     charge leg, is not derivable at restore because the away
        //     state publishes zero port power; the charge-side
        //     counterpart of `v2l_power_kw`, completing the family).
        9
    }

    fn validate_signal(&self, signal: &hares_types::ControlSignal) -> crate::Result<()> {
        use hares_types::ensure_signal_supported;
        ensure_signal_supported(self.descriptor().control_capabilities, signal)?;
        match signal {
            hares_types::ControlSignal::EvDrive { .. }
                if self.connection_state != hares_types::EvConnectionState::Disconnected =>
            {
                return Err(hares_types::HaresError::Control(
                    "EvDrive rejected: EV must be Disconnected to drive".to_string(),
                ));
            }
            hares_types::ControlSignal::EvAwayCharge { .. }
                if self.connection_state != hares_types::EvConnectionState::AwayPluggedIn =>
            {
                return Err(hares_types::HaresError::Control(
                    "EvAwayCharge rejected: EV must be AwayPluggedIn".to_string(),
                ));
            }
            _ => {}
        }
        Ok(())
    }

    fn apply_signal(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        match signal {
            ControlSignal::PowerSetpoint {
                active_power_kw,
                reactive_power_kvar,
                min_soc,
                max_soc,
            } => {
                // Validate the ENTIRE signal before mutating any state so a
                // rejected setpoint leaves no partial effect (e.g. an armed
                // q_setpoint from a signal whose active power was refused).
                if let Some(q) = reactive_power_kvar {
                    if !q.is_finite() {
                        return Err(HaresError::Control(
                            "EV PowerSetpoint reactive_power_kvar must be finite".to_string(),
                        ));
                    }
                }
                // Self-contained guard (see EvDrive): a non-finite active
                // setpoint would silently disarm charging (`f64::max`
                // swallows NaN) or saturate v2g/v2l discharge to the
                // hardware maximum — battery, PV, and scheduled load all
                // guard this value at the arm level too.
                if !active_power_kw.is_finite() {
                    return Err(HaresError::Control(format!(
                        "EV PowerSetpoint active_power_kw must be finite, got {active_power_kw}"
                    )));
                }
                // A non-finite SOC window would be silently substituted with
                // defaults downstream (`max(NaN)` returns the reserve, the
                // `min(NaN)` cap no-ops) — a requested constraint that
                // quietly never applies.
                if let Some(m) = min_soc
                    && !m.is_finite()
                {
                    return Err(HaresError::Control(format!(
                        "EV PowerSetpoint min_soc must be finite, got {m}"
                    )));
                }
                if let Some(m) = max_soc
                    && !m.is_finite()
                {
                    return Err(HaresError::Control(format!(
                        "EV PowerSetpoint max_soc must be finite, got {m}"
                    )));
                }
                if *active_power_kw < 0.0 && !self.v2l_enabled && !self.v2g_enabled {
                    return Err(HaresError::Control(
                        "negative PowerSetpoint requires v2l_enabled or v2g_enabled".to_string(),
                    ));
                }
                if let Some(q) = reactive_power_kvar {
                    self.q_setpoint_kvar = Some(*q);
                }
                self.power_setpoint_kw = Some(*active_power_kw);
                self.power_setpoint_min_soc = *min_soc;
                self.power_setpoint_max_soc = *max_soc;
            }
            ControlSignal::PowerLimit { max_power_kw, .. } => {
                self.power_limit_kw = Some((*max_power_kw).max(0.0));
            }
            ControlSignal::SOCTarget {
                target_soc,
                min_soc,
                max_soc,
            } => {
                // Self-contained guard (see EvDrive): `f64::clamp` propagates
                // NaN, which makes `soc >= soc_limit` compare false forever —
                // full-power charging toward an unreachable target; ±∞
                // would silently stand in as 1.0/0.0. The battery arm
                // rejects non-finite targets after clamping; match it before
                // storing.
                if !target_soc.is_finite() {
                    return Err(HaresError::Control(format!(
                        "EV SOCTarget target_soc must be finite, got {target_soc}"
                    )));
                }
                if let Some(m) = min_soc
                    && !m.is_finite()
                {
                    return Err(HaresError::Control(format!(
                        "EV SOCTarget min_soc must be finite, got {m}"
                    )));
                }
                if let Some(m) = max_soc
                    && !m.is_finite()
                {
                    return Err(HaresError::Control(format!(
                        "EV SOCTarget max_soc must be finite, got {m}"
                    )));
                }
                self.soc_target = Some((*target_soc).clamp(0.0, 1.0));
                self.soc_target_min = *min_soc;
                self.soc_target_max = *max_soc;
                // A fresh charge target supersedes any zero-power hold
                // dispatched during a prior idle window: without this clear,
                // `compute_charging_power_kw` would keep consulting the
                // latched `power_setpoint_kw` first and silently veto the
                // new target ("never charges" instead of "always charges").
                self.power_setpoint_kw = None;
                self.power_setpoint_min_soc = None;
                self.power_setpoint_max_soc = None;
            }
            ControlSignal::ReactiveSetpoint { kvar } => {
                if !kvar.is_finite() {
                    return Err(HaresError::Control(
                        "EV ReactiveSetpoint kvar must be finite".to_string(),
                    ));
                }
                self.q_setpoint_kvar = Some(*kvar);
            }
            ControlSignal::PowerFactorSetpoint { power_factor } => {
                if !power_factor.is_finite() || *power_factor <= 0.0 || *power_factor > 1.0 {
                    return Err(HaresError::Control(
                        "EV PowerFactorSetpoint must be in (0, 1]".to_string(),
                    ));
                }
                self.power_factor = *power_factor;
                self.q_setpoint_kvar = None;
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
                        // Same state -- no-op
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
                    // Disconnect ends the actor's charging session: drop any
                    // latched setpoint and target so the next plug-in — home
                    // or away — starts from the equipment's own configured
                    // defaults instead of whatever the previous session left
                    // behind. A latched zero-power hold would silently zero
                    // away charging (the away arm consults the same
                    // `power_setpoint_kw`); a stale home-side `soc_target`
                    // would govern away charging by accident of whatever the
                    // home strategy last set, not by design. No caller or
                    // test relies on these surviving a plug cycle.
                    self.power_setpoint_kw = None;
                    self.power_setpoint_min_soc = None;
                    self.power_setpoint_max_soc = None;
                    self.soc_target = None;
                    self.soc_target_min = None;
                    self.soc_target_max = None;
                }
            }
            ControlSignal::EvDrive { kwh } => {
                if self.connection_state != EvConnectionState::Disconnected {
                    return Err(HaresError::Control(
                        "EvDrive rejected: EV must be Disconnected to drive".to_string(),
                    ));
                }
                // Self-contained guard: `apply_control_unchecked` bypasses the
                // central signal validation, and a non-finite or negative
                // energy would create SOC from nowhere (or poison every
                // downstream clamp).
                if !kwh.is_finite() || *kwh < 0.0 {
                    return Err(HaresError::Control(format!(
                        "EvDrive kwh must be finite and >= 0, got {kwh}"
                    )));
                }
                let available_kwh = self.battery_capacity_kwh * self.soc;
                if *kwh > available_kwh {
                    // A finite drive exceeding the pack's remaining energy is
                    // a physical situation — a trip longer than the
                    // vehicle's range — not an invalid value: deliver what
                    // the pack holds (SOC lands exactly at 0, never below)
                    // and account the undeliverable remainder as observable
                    // state (`DRIVE_SHORTFALL_KWH`), never as silently
                    // dropped mobility. The driver actor clamps its trip to
                    // the observed pack energy, so this path carries only
                    // the residual — a mid-trip capacity-fade step, or a
                    // driver with no live equipment observation. Invalid
                    // values (non-finite, negative) are still rejected
                    // loudly above; a rejected step would instead vanish
                    // into a dwelling warning string while the day's
                    // profile kept reporting the dispatched energy.
                    self.drive_shortfall_kwh += *kwh - available_kwh;
                    self.soc = 0.0;
                    // Only the delivered energy drives the pack — the I²R
                    // heat of a trip is proportional to the current that
                    // actually flowed.
                    self.pending_drive_kwh += available_kwh;
                } else {
                    self.soc = (self.soc - *kwh / self.battery_capacity_kwh).clamp(0.0, 1.0);
                    // The SOC debit happens here; the trip's I²R heat is
                    // applied over the next step at the equivalent
                    // discharge current (`pending_drive_kwh / dt_hours`),
                    // because the signal carries energy, not duration —
                    // and the driver dispatches one EvDrive per step.
                    self.pending_drive_kwh += *kwh;
                }
            }
            ControlSignal::EvAwayCharge { power_kw } => {
                if self.connection_state != EvConnectionState::AwayPluggedIn {
                    return Err(HaresError::Control(
                        "EvAwayCharge rejected: EV must be AwayPluggedIn".to_string(),
                    ));
                }
                // Self-contained guard (see EvDrive): a non-finite or negative
                // charge power would silently no-op (`> 0.0` compares false)
                // instead of being reported.
                if !power_kw.is_finite() || *power_kw < 0.0 {
                    return Err(HaresError::Control(format!(
                        "EvAwayCharge power_kw must be finite and >= 0, got {power_kw}"
                    )));
                }
                self.away_charger_power_kw = *power_kw;
            }
            ControlSignal::EvSetReadyBy {
                departure_hour,
                target_soc,
            } => {
                // Self-contained guard (see EvDrive): these feed the charging
                // target and the deadline pacing — NaN or out-of-range values
                // would silently disable or never satisfy charging.
                if !departure_hour.is_finite() || *departure_hour < 0.0 || *departure_hour > 24.0 {
                    return Err(HaresError::Control(format!(
                        "EvSetReadyBy departure_hour must be finite and in [0, 24], got {departure_hour}"
                    )));
                }
                if !target_soc.is_finite() || *target_soc < 0.0 || *target_soc > 1.0 {
                    return Err(HaresError::Control(format!(
                        "EvSetReadyBy target_soc must be finite and in [0, 1], got {target_soc}"
                    )));
                }
                self.ready_by_hour = Some(*departure_hour);
                self.ready_by_soc = Some(*target_soc);
                // Same hold-clearing rule as `SOCTarget`: a ready-by target
                // means "charge toward this by departure", which a latched
                // zero-power hold from a prior idle window would veto.
                self.power_setpoint_kw = None;
                self.power_setpoint_min_soc = None;
                self.power_setpoint_max_soc = None;
            }
            ControlSignal::DemandResponse { level, duration_s } => {
                self.dr_level = *level;
                self.dr_duration_remaining_s = *duration_s;
            }
            _ => {
                return Err(HaresError::Control(format!(
                    "EV does not handle control signal: {signal:?}"
                )));
            }
        }

        Ok(())
    }

    fn is_initialized(&self) -> bool {
        self.initialized
    }

    fn mark_initialized(&mut self) {
        self.initialized = true;
    }

    fn unmark_initialized(&mut self) {
        self.initialized = false;
    }

    fn set_charging_curve_lut(
        &mut self,
        lut: Option<crate::ndinterp::RegularGridInterpolator>,
    ) -> crate::Result<()> {
        if self.initialized {
            return Err(HaresError::InvalidState(format!(
                "equipment '{}' is already initialized; cannot set charging curve LUT",
                self.descriptor().name
            )));
        }
        self.charging_curve_lut = lut;
        Ok(())
    }

    fn has_charging_curve_lut(&self) -> bool {
        self.charging_curve_lut.is_some()
    }

    fn set_ocv_table(&mut self, table: OcvTable) -> crate::Result<()> {
        if self.initialized {
            return Err(HaresError::InvalidState(format!(
                "equipment '{}' is already initialized; cannot set OCV table",
                self.descriptor().name
            )));
        }
        self.ocv_table = table;
        self.custom_ocv = true;
        Ok(())
    }

    fn set_u_neg_table(&mut self, table: UNegTable) -> crate::Result<()> {
        if self.initialized {
            return Err(HaresError::InvalidState(format!(
                "equipment '{}' is already initialized; cannot set UNeg table",
                self.descriptor().name
            )));
        }
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

    fn ocv_source(&self) -> Option<&str> {
        Some(self.ocv_table.ocv_source.as_str())
    }

    fn rename(&mut self, name: String) {
        self.descriptor.name = name;
    }

    fn set_equipment_id(&mut self, id: EquipmentId) -> crate::Result<()> {
        crate::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
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
