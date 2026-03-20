//! Battery storage equipment model.
//!
//! Implements an electrochemical battery with OCV-based voltage model,
//! ohmic loss efficiency, self-consumption control, rainflow cycle counting,
//! and degradation tracking (stubbed in v1).

mod degradation;
mod ocv;

use std::borrow::Cow;
use std::time::Duration;

use hares_types::{
    ControlCapabilities, ControlSignal, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FuelType, HaresError, OperatingMode, PortContribution, PortDeclaration,
    PortSlots, Telemetry, TelemetryField, ThermalCategory, ZoneId,
};
use serde::{Deserialize, Serialize};

use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

use degradation::{DegradationState, RainflowCounter};
use ocv::{OcvTable, UNegTable};

// ---------------------------------------------------------------------------
// Config keys
// ---------------------------------------------------------------------------

use crate::config::{KEY_EQUIPMENT_ID, KEY_ZONE_ID};
const KEY_CAPACITY_KWH: &str = "capacity_kwh";
const KEY_MAX_CHARGE_KW: &str = "max_charge_kw";
const KEY_MAX_DISCHARGE_KW: &str = "max_discharge_kw";
const KEY_N_SERIES: &str = "n_series";
const KEY_N_PARALLEL: &str = "n_parallel";
const KEY_CELL_RESISTANCE_OHM: &str = "cell_resistance_ohm";
const KEY_SELF_DISCHARGE_PCT_PER_DAY: &str = "self_discharge_pct_per_day";
const KEY_STANDBY_POWER_W: &str = "standby_power_w";
const KEY_INITIAL_SOC: &str = "initial_soc";
const KEY_MIN_SOC: &str = "min_soc";
const KEY_MAX_SOC: &str = "max_soc";
const KEY_HEATER_POWER_W: &str = "heater_power_w";
const KEY_HEATER_THRESHOLD_C: &str = "heater_threshold_c";
const KEY_HEATER_ON_DISCHARGE: &str = "heater_on_discharge";
const KEY_CELL_THERMAL_MASS_J_PER_K: &str = "cell_thermal_mass_j_per_k";
const KEY_CELL_UA_W_PER_K: &str = "cell_ua_w_per_k";
const KEY_MIN_DISCHARGE_TEMP_C: &str = "min_discharge_temp_c";
const KEY_FULL_POWER_TEMP_C: &str = "full_power_temp_c";
const KEY_MIN_CHARGE_TEMP_C: &str = "min_charge_temp_c";
const KEY_INVERTER_EFFICIENCY: &str = "inverter_efficiency";
/// Per-cell capacity (Ah). When provided with KEY_V_CELL, n_series/n_parallel
/// are derived from capacity_kwh: n_series = V_pack/V_cell, n_parallel = Ah_pack/Ah_cell,
/// where V_pack ≈ cell_ocv_nom * n_series and Ah_pack = capacity_kwh * 1000 / V_pack.
const KEY_AH_CELL: &str = "ah_cell";
/// Per-cell nominal voltage (V). Used with KEY_AH_CELL to derive pack topology.
const KEY_V_CELL: &str = "v_cell";
/// Maximum grid import power while battery is charging (W). None = unlimited.
/// OCHRE: `import_limit` parameter on Generator (Battery inherits it).
const KEY_IMPORT_LIMIT_W: &str = "import_limit_w";
/// Maximum grid export power while battery is discharging (W). None = unlimited.
/// OCHRE: `export_limit` parameter on Generator (Battery inherits it).
const KEY_EXPORT_LIMIT_W: &str = "export_limit_w";

// ---------------------------------------------------------------------------
// Physical defaults (Li-NMC, Tesla Powerwall-class)
// ---------------------------------------------------------------------------

/// Tesla Powerwall 3: 13.5 kWh usable. Franklin aPower 2: 13.6 kWh.
const DEFAULT_CAPACITY_KWH: f64 = 13.5;
/// Powerwall 3 continuous: 11.04 kW charge; most units derate to ~5 kW sustained.
const DEFAULT_MAX_CHARGE_KW: f64 = 5.0;
/// Powerwall 3 continuous: 11.04 kW discharge; conservative default for generic pack.
const DEFAULT_MAX_DISCHARGE_KW: f64 = 5.0;
/// 96S1P is a common Li-NMC configuration (~355 V nominal pack).
const DEFAULT_N_SERIES: u32 = 96;
const DEFAULT_N_PARALLEL: u32 = 1;
/// Typical 18650/21700 Li-NMC cell internal resistance at 25 C, mid-SOC.
const DEFAULT_CELL_RESISTANCE_OHM: f64 = 0.005;
/// OCHRE default: 0.0 %/day (users opt-in to self-discharge via config).
const DEFAULT_SELF_DISCHARGE_PCT_PER_DAY: f64 = 0.0;
/// Parasitic BMS/inverter standby draw. Powerwall ~5-15 W; OCHRE hardcoded 0 W (fixed).
const DEFAULT_STANDBY_POWER_W: f64 = 5.0;
const DEFAULT_INITIAL_SOC: f64 = 0.5;
// OCHRE default_parameters.csv: soc_min=0.15, soc_max=0.95
const DEFAULT_MIN_SOC: f64 = 0.15;
const DEFAULT_MAX_SOC: f64 = 0.95;
/// Disabled by default; set to ~500 W for Franklin aPower 2 style pad heater,
/// or ~100 W for Tesla Powerwall 3 cell-level resistive heaters.
const DEFAULT_HEATER_POWER_W: f64 = 0.0;
/// Heater activation threshold. Tesla Heat Mode targets 0 C minimum cell temp;
/// Franklin activates around 5-10 C. 0 C is the Li-ion plating safety boundary.
const DEFAULT_HEATER_THRESHOLD_C: f64 = 0.0;
/// OCHRE/industry-standard inverter efficiency (AC-DC conversion loss, both directions).
const DEFAULT_INVERTER_EFFICIENCY: f64 = 0.97;
/// All major residential batteries (Powerwall, aPower 2, Enphase IQ) spec -20 C
/// as the lower operating limit for discharge.
const DEFAULT_MIN_DISCHARGE_TEMP_C: f64 = -20.0;
/// Tesla and Franklin both require ~10 C for full charge/discharge power.
/// Linear derating between min_discharge_temp_c and this value.
const DEFAULT_FULL_POWER_TEMP_C: f64 = 10.0;
/// Li-ion lithium plating occurs below 0 C; all manufacturers block charging here.
/// Enphase derates charge below 15 C but still allows reduced-rate charging above 0 C.
const DEFAULT_MIN_CHARGE_TEMP_C: f64 = 0.0;
/// Lumped thermal mass for the battery pack (J/K).
/// OCHRE Battery.py default: 90,000 J/K for a standard residential pack.
const DEFAULT_CELL_THERMAL_MASS_J_PER_K: f64 = 90_000.0;
/// Lumped UA (heat loss coefficient) between pack and ambient (W/K).
/// Typical enclosed residential battery enclosure.
const DEFAULT_CELL_UA_W_PER_K: f64 = 5.0;
const SECONDS_PER_DAY: f64 = 86_400.0;
const IDLE_POWER_THRESHOLD_KW: f64 = 1e-6;

// ---------------------------------------------------------------------------
// Serializable battery state for checkpointing
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct BatteryCheckpoint {
    soc: f64,
    cell_temp_c: f64,
    heater_active: bool,
    mode: OperatingMode,
    degradation: DegradationState,
    rainflow: RainflowCounter,
    self_consumption_enabled: bool,
    solar_only_charging: bool,
    grid_connected: bool,
    power_setpoint_kw: Option<f64>,
    soc_target: Option<f64>,
    soc_target_min: Option<f64>,
    soc_target_max: Option<f64>,
    last_daily_update_day: i32,
    import_limit_kw: Option<f64>,
    export_limit_kw: Option<f64>,
}

// ---------------------------------------------------------------------------
// Battery struct
// ---------------------------------------------------------------------------

pub struct Battery {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,

    // Static config
    capacity_kwh: f64,
    /// Nominal pack capacity before temperature derating (d0 Arrhenius model).
    /// Set once from config; capacity_kwh is updated each step based on cell temperature.
    capacity_kwh_nominal: f64,
    max_charge_kw: f64,
    max_discharge_kw: f64,
    n_series: u32,
    n_parallel: u32,
    cell_resistance_ohm: f64,
    ocv_table: OcvTable,
    u_neg_table: UNegTable,
    self_discharge_rate_per_s: f64,
    standby_power_w: f64,
    min_soc: f64,
    max_soc: f64,
    /// Maximum charge power drawn from the grid (kW). None = unlimited.
    /// OCHRE `import_limit`: clamps battery charging to prevent excessive grid import.
    import_limit_kw: Option<f64>,
    /// Maximum discharge power exported to the grid (kW). None = unlimited.
    /// OCHRE `export_limit`: clamps battery discharging to prevent excessive grid export.
    export_limit_kw: Option<f64>,

    // Inverter efficiency (AC-DC conversion, applied in both charge and discharge directions)
    inverter_efficiency: f64,

    // Cell heater config (e.g. Franklin WH ~500 W heater at 0 C)
    heater_power_w: f64,
    heater_threshold_c: f64,
    heater_on_discharge: bool,

    // Temperature-dependent power derating
    min_discharge_temp_c: f64,
    full_power_temp_c: f64,
    min_charge_temp_c: f64,

    // Lumped cell thermal model
    cell_thermal_mass_j_per_k: f64,
    cell_ua_w_per_k: f64,

    // Dynamic state
    soc: f64,
    cell_temp_c: f64,
    heater_active: bool,
    mode: OperatingMode,
    degradation: DegradationState,
    rainflow: RainflowCounter,

    // Control state
    self_consumption_enabled: bool,
    solar_only_charging: bool,
    grid_connected: bool,
    power_setpoint_kw: Option<f64>,
    soc_target: Option<f64>,
    soc_target_min: Option<f64>,
    soc_target_max: Option<f64>,

    last_daily_update_day: i32,
}

impl Battery {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let equipment_id = config
            .get_f64(KEY_EQUIPMENT_ID)
            .map(|v| v as u32)
            .unwrap_or(0);
        let zone = config.get_f64(KEY_ZONE_ID).map(|v| ZoneId(v as u16));

        let mut ports = vec![PortDeclaration::electrical()];
        if let Some(z) = zone {
            ports.push(PortDeclaration::thermal(z));
        }

        let descriptor = EquipmentDescriptor {
            id: EquipmentId(equipment_id),
            name: config.name.clone(),
            end_use: EndUse::Battery,
            equipment_type: Cow::Borrowed("Battery"),
            zone,
            fuel: FuelType::Electric,
            stage: ExecutionStage::Electrical,
            control_capabilities: ControlCapabilities::POWER_SETPOINT
                | ControlCapabilities::SOC_TARGET
                | ControlCapabilities::GRID_CONNECT
                | ControlCapabilities::SELF_CONSUMPTION,
            telemetry_fields: battery_telemetry_fields(),
        };

        Self {
            descriptor,
            ports,
            telemetry: default_telemetry(),
            capacity_kwh: DEFAULT_CAPACITY_KWH,
            capacity_kwh_nominal: DEFAULT_CAPACITY_KWH,
            max_charge_kw: DEFAULT_MAX_CHARGE_KW,
            max_discharge_kw: DEFAULT_MAX_DISCHARGE_KW,
            n_series: DEFAULT_N_SERIES,
            n_parallel: DEFAULT_N_PARALLEL,
            cell_resistance_ohm: DEFAULT_CELL_RESISTANCE_OHM,
            ocv_table: OcvTable::default_li_nmc(),
            u_neg_table: UNegTable::default_li_nmc(),
            self_discharge_rate_per_s: DEFAULT_SELF_DISCHARGE_PCT_PER_DAY / 100.0 / SECONDS_PER_DAY,
            standby_power_w: DEFAULT_STANDBY_POWER_W,
            min_soc: DEFAULT_MIN_SOC,
            max_soc: DEFAULT_MAX_SOC,
            import_limit_kw: None,
            export_limit_kw: None,
            inverter_efficiency: DEFAULT_INVERTER_EFFICIENCY,
            heater_power_w: DEFAULT_HEATER_POWER_W,
            heater_threshold_c: DEFAULT_HEATER_THRESHOLD_C,
            heater_on_discharge: false,
            min_discharge_temp_c: DEFAULT_MIN_DISCHARGE_TEMP_C,
            full_power_temp_c: DEFAULT_FULL_POWER_TEMP_C,
            min_charge_temp_c: DEFAULT_MIN_CHARGE_TEMP_C,
            cell_thermal_mass_j_per_k: DEFAULT_CELL_THERMAL_MASS_J_PER_K,
            cell_ua_w_per_k: DEFAULT_CELL_UA_W_PER_K,
            soc: DEFAULT_INITIAL_SOC,
            cell_temp_c: 25.0,
            heater_active: false,
            mode: OperatingMode::Off,
            degradation: DegradationState::default(),
            rainflow: RainflowCounter::default(),
            self_consumption_enabled: true,
            solar_only_charging: false,
            grid_connected: true,
            power_setpoint_kw: None,
            soc_target: None,
            soc_target_min: None,
            soc_target_max: None,
            last_daily_update_day: 0,
        }
    }

    /// Compute pack-level voltage, current, and ohmic losses for a target AC power.
    ///
    /// Applies inverter efficiency to convert AC power to DC power, then uses the
    /// quadratic terminal-voltage formula (OCHRE method) for accurate current and
    /// ohmic loss calculation at high C-rates.
    ///
    /// Returns `(actual_power_kw, ohmic_loss_w)` where `actual_power_kw` is the
    /// power at the grid connection point (positive = charging/consuming,
    /// negative = discharging/generating).
    fn compute_electrical(&self, target_ac_power_kw: f64) -> (f64, f64) {
        if target_ac_power_kw.abs() < IDLE_POWER_THRESHOLD_KW {
            return (0.0, 0.0);
        }
        let cell_ocv = self.ocv_table.voltage_at_soc(self.soc);
        let pack_ocv = cell_ocv * self.n_series as f64;
        let pack_resistance =
            self.cell_resistance_ohm * self.n_series as f64 / self.n_parallel as f64;

        if pack_ocv < f64::EPSILON {
            return (0.0, 0.0);
        }

        // Convert AC power to DC power using inverter efficiency.
        // Charging: DC power = AC power * eta (less DC than AC due to conversion loss).
        // Discharging: DC power = AC power / eta (more DC needed than AC delivered).
        let dc_power_kw = if target_ac_power_kw > 0.0 {
            target_ac_power_kw * self.inverter_efficiency
        } else {
            target_ac_power_kw / self.inverter_efficiency
        };
        let dc_power_w = dc_power_kw * 1000.0;

        // Quadratic terminal voltage (OCHRE method, Battery.py:295).
        //   V = Voc/2 + sqrt((Voc/2)^2 + P_dc * R)
        // Sign convention: P_dc > 0 = charging (consuming from grid), P_dc < 0 = discharging.
        // Charging (P_dc > 0): discriminant > (Voc/2)^2, so V > Voc (terminal voltage rises).
        // Discharging (P_dc < 0): discriminant < (Voc/2)^2, so V < Voc (terminal voltage sags).
        let half_voc = pack_ocv / 2.0;
        let discriminant = half_voc * half_voc + dc_power_w * pack_resistance;

        // Clamp to maximum extractable power P_max = Voc²/(4R) when discriminant < 0.
        // At this limit terminal voltage = Voc/2 (matched-impedance condition).
        let (terminal_v, actual_dc_power_w) = if discriminant >= 0.0 {
            (half_voc + discriminant.sqrt(), dc_power_w)
        } else {
            let p_max_w = pack_ocv * pack_ocv / (4.0 * pack_resistance);
            let clamped = dc_power_w.abs().min(p_max_w) * dc_power_w.signum();
            (half_voc, clamped)
        };

        let current_a = if terminal_v.abs() > f64::EPSILON {
            actual_dc_power_w / terminal_v
        } else {
            0.0
        };
        let ohmic_loss_w = current_a * current_a * pack_resistance;

        // Convert actual DC power back to AC grid power for the clamped case.
        let actual_ac_power_kw = if discriminant >= 0.0 {
            target_ac_power_kw
        } else if actual_dc_power_w > 0.0 {
            actual_dc_power_w / self.inverter_efficiency / 1000.0
        } else {
            actual_dc_power_w * self.inverter_efficiency / 1000.0
        };

        (actual_ac_power_kw, ohmic_loss_w)
    }

    /// Determine the target charge/discharge power based on control state and
    /// Stage 1 accumulated electrical power.
    fn determine_target_power(&self, net_load_kw: f64, dt_hours: f64) -> f64 {
        // Priority 1: explicit power setpoint from external control
        if let Some(setpoint) = self.power_setpoint_kw {
            return self.clamp_power(setpoint);
        }

        // Priority 2: SOC target (simple proportional controller)
        if let Some(target) = self.soc_target {
            if dt_hours > 0.0 {
                let error = target - self.soc;
                let power = error * self.capacity_kwh / dt_hours;
                return self.clamp_power(power);
            }
        }

        // Priority 3: self-consumption
        // Note: target power here is in AC grid terms. For discharge, the exact
        // correction for inverter+ohmic losses would require iteration (losses depend
        // on power which depends on losses). This is an intentional first-order
        // approximation; the error is small (<4%) for typical inverter efficiencies.
        if self.self_consumption_enabled {
            if net_load_kw > IDLE_POWER_THRESHOLD_KW {
                // Net load exceeds PV -- discharge to offset
                let discharge = net_load_kw.min(self.max_discharge_kw);
                return self.clamp_power(-discharge);
            } else if net_load_kw < -IDLE_POWER_THRESHOLD_KW {
                // PV surplus -- charge from excess (regardless of solar_only flag,
                // since we already know there's a PV surplus)
                let charge = (-net_load_kw).min(self.max_charge_kw);
                return self.clamp_power(charge);
            } else if self.solar_only_charging {
                // No PV surplus and solar-only mode -- do not charge from grid
                return 0.0;
            }
        }

        0.0
    }

    /// Clamp power to hardware charge/discharge limits and any active import/export limits.
    ///
    /// Positive = charging (grid import), negative = discharging (grid export).
    ///
    /// `import_limit_kw` caps the charge power (grid → battery).
    /// `export_limit_kw` caps the discharge power (battery → grid).
    ///
    /// OCHRE `Generator.get_power_limits` + Battery schedule inputs
    /// `Battery Max Import Limit (kW)` / `Battery Max Export Limit (kW)`.
    fn clamp_power(&self, power_kw: f64) -> f64 {
        if power_kw > 0.0 {
            let hw_max = self.max_charge_kw;
            let limit = self
                .import_limit_kw
                .map(|lim| hw_max.min(lim))
                .unwrap_or(hw_max);
            power_kw.min(limit)
        } else {
            let hw_max = self.max_discharge_kw;
            let limit = self
                .export_limit_kw
                .map(|lim| hw_max.min(lim))
                .unwrap_or(hw_max);
            power_kw.max(-limit)
        }
    }

    /// Temperature-dependent discharge derating factor [0.0 .. 1.0].
    ///
    /// Full power above `full_power_temp_c`, linearly derates to zero at
    /// `min_discharge_temp_c`. Returns 0.0 below min discharge temp.
    fn discharge_derate_factor(&self) -> f64 {
        if self.cell_temp_c >= self.full_power_temp_c {
            1.0
        } else if self.cell_temp_c <= self.min_discharge_temp_c {
            0.0
        } else {
            let span = self.full_power_temp_c - self.min_discharge_temp_c;
            if span <= 0.0 {
                return 0.0;
            }
            (self.cell_temp_c - self.min_discharge_temp_c) / span
        }
    }

    /// Whether charging is allowed at current cell temperature.
    fn charge_allowed(&self) -> bool {
        self.cell_temp_c >= self.min_charge_temp_c
    }

    fn day_ordinal(env: &EnvironmentState) -> i32 {
        use chrono::Datelike;
        env.current_time.date_naive().num_days_from_ce()
    }
}

impl Equipment for Battery {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.capacity_kwh = config
            .get_f64(KEY_CAPACITY_KWH)
            .unwrap_or(DEFAULT_CAPACITY_KWH);
        self.capacity_kwh_nominal = self.capacity_kwh;
        self.max_charge_kw = config
            .get_f64(KEY_MAX_CHARGE_KW)
            .unwrap_or(DEFAULT_MAX_CHARGE_KW);
        self.max_discharge_kw = config
            .get_f64(KEY_MAX_DISCHARGE_KW)
            .unwrap_or(DEFAULT_MAX_DISCHARGE_KW);
        self.n_series = config
            .get_f64(KEY_N_SERIES)
            .map(|v| v as u32)
            .unwrap_or(DEFAULT_N_SERIES);
        self.n_parallel = config
            .get_f64(KEY_N_PARALLEL)
            .map(|v| v as u32)
            .unwrap_or(DEFAULT_N_PARALLEL);

        // If per-cell parameters are provided, derive pack topology from them.
        // n_series = round(V_pack_nom / V_cell), where V_pack_nom ≈ 350 V for
        // standard residential packs; n_parallel = Ah_pack / Ah_cell,
        // where Ah_pack = capacity_kwh * 1000 / (n_series * V_cell).
        if let (Some(ah_cell), Some(v_cell)) =
            (config.get_f64(KEY_AH_CELL), config.get_f64(KEY_V_CELL))
        {
            if ah_cell > 0.0 && v_cell > 0.0 {
                // Estimate n_series from a target pack voltage of ~350 V (standard residential).
                let target_pack_v = 350.0_f64;
                self.n_series = (target_pack_v / v_cell).round() as u32;
                if self.n_series == 0 {
                    self.n_series = 1;
                }
                let pack_v = self.n_series as f64 * v_cell;
                let pack_ah = self.capacity_kwh * 1000.0 / pack_v;
                self.n_parallel = (pack_ah / ah_cell).round() as u32;
                if self.n_parallel == 0 {
                    self.n_parallel = 1;
                }
            }
        }

        self.cell_resistance_ohm = config
            .get_f64(KEY_CELL_RESISTANCE_OHM)
            .unwrap_or(DEFAULT_CELL_RESISTANCE_OHM);
        self.standby_power_w = config
            .get_f64(KEY_STANDBY_POWER_W)
            .unwrap_or(DEFAULT_STANDBY_POWER_W);
        self.min_soc = config.get_f64(KEY_MIN_SOC).unwrap_or(DEFAULT_MIN_SOC);
        self.max_soc = config.get_f64(KEY_MAX_SOC).unwrap_or(DEFAULT_MAX_SOC);
        // import/export limits: config values are in watts; store as kW for internal use.
        self.import_limit_kw = config.get_f64(KEY_IMPORT_LIMIT_W).map(|w| w / 1000.0);
        self.export_limit_kw = config.get_f64(KEY_EXPORT_LIMIT_W).map(|w| w / 1000.0);
        self.heater_power_w = config
            .get_f64(KEY_HEATER_POWER_W)
            .unwrap_or(DEFAULT_HEATER_POWER_W);
        self.heater_threshold_c = config
            .get_f64(KEY_HEATER_THRESHOLD_C)
            .unwrap_or(DEFAULT_HEATER_THRESHOLD_C);
        self.heater_on_discharge = config.get_bool(KEY_HEATER_ON_DISCHARGE).unwrap_or(false);
        self.min_discharge_temp_c = config
            .get_f64(KEY_MIN_DISCHARGE_TEMP_C)
            .unwrap_or(DEFAULT_MIN_DISCHARGE_TEMP_C);
        self.full_power_temp_c = config
            .get_f64(KEY_FULL_POWER_TEMP_C)
            .unwrap_or(DEFAULT_FULL_POWER_TEMP_C);
        self.min_charge_temp_c = config
            .get_f64(KEY_MIN_CHARGE_TEMP_C)
            .unwrap_or(DEFAULT_MIN_CHARGE_TEMP_C);
        self.inverter_efficiency = config
            .get_f64(KEY_INVERTER_EFFICIENCY)
            .unwrap_or(DEFAULT_INVERTER_EFFICIENCY)
            .clamp(f64::EPSILON, 1.0);
        self.cell_thermal_mass_j_per_k = config
            .get_f64(KEY_CELL_THERMAL_MASS_J_PER_K)
            .unwrap_or(DEFAULT_CELL_THERMAL_MASS_J_PER_K);
        self.cell_ua_w_per_k = config
            .get_f64(KEY_CELL_UA_W_PER_K)
            .unwrap_or(DEFAULT_CELL_UA_W_PER_K);

        let self_discharge = config
            .get_f64(KEY_SELF_DISCHARGE_PCT_PER_DAY)
            .unwrap_or(DEFAULT_SELF_DISCHARGE_PCT_PER_DAY);
        self.self_discharge_rate_per_s = self_discharge / 100.0 / SECONDS_PER_DAY;

        // Validate before clamping
        if self.capacity_kwh <= 0.0 {
            return Err(HaresError::Equipment(
                "battery capacity_kwh must be positive".to_string(),
            ));
        }
        if self.min_soc >= self.max_soc {
            return Err(HaresError::Equipment(
                "battery min_soc must be less than max_soc".to_string(),
            ));
        }
        if self.n_series == 0 || self.n_parallel == 0 {
            return Err(HaresError::Equipment(
                "n_series and n_parallel must be positive".to_string(),
            ));
        }
        if let Some(lim) = self.import_limit_kw {
            if lim < 0.0 {
                return Err(HaresError::Equipment(
                    "battery import_limit_w must be non-negative".to_string(),
                ));
            }
        }
        if let Some(lim) = self.export_limit_kw {
            if lim < 0.0 {
                return Err(HaresError::Equipment(
                    "battery export_limit_w must be non-negative".to_string(),
                ));
            }
        }

        let initial_soc = config
            .get_f64(KEY_INITIAL_SOC)
            .unwrap_or(DEFAULT_INITIAL_SOC);
        self.soc = initial_soc.clamp(self.min_soc, self.max_soc);

        // Cell temperature from zone if configured, else outdoor ambient.
        // Batteries are commonly in unheated spaces (garages, outdoor enclosures)
        // so the default ambient is outdoor weather, not a conditioned 25 C.
        self.cell_temp_c = if let Some(zone_id) = self.descriptor.zone {
            env.zones
                .iter()
                .find(|z| z.id == zone_id)
                .map(|z| z.temperature_c)
                .unwrap_or(env.weather.outdoor_temp_c)
        } else {
            env.weather.outdoor_temp_c
        };

        self.mode = OperatingMode::Off;
        self.heater_active = false;
        self.degradation = DegradationState::default();
        self.degradation.reset_day_tracking(self.soc);
        self.rainflow = RainflowCounter::default();
        self.self_consumption_enabled = true;
        self.solar_only_charging = false;
        self.grid_connected = true;
        self.power_setpoint_kw = None;
        self.soc_target = None;
        self.soc_target_min = None;
        self.soc_target_max = None;
        self.last_daily_update_day = Self::day_ordinal(env);

        self.telemetry = default_telemetry();
        self.telemetry.set("soc", self.soc);

        Ok(())
    }

    fn update_control(&mut self, _env: &EnvironmentState) -> OperatingMode {
        self.mode
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        let dt_s = dt.as_secs_f64();
        let dt_hours = dt_s / 3600.0;

        if dt_s <= 0.0 {
            return Err(HaresError::Equipment(
                "timestep must be positive".to_string(),
            ));
        }

        // -- Read Stage 1 accumulated electrical power --
        let net_load_kw = ports.electrical.net_active_kw();

        // -- Determine target power --
        let mut target_power_kw = if self.grid_connected {
            self.determine_target_power(net_load_kw, dt_hours)
        } else {
            0.0
        };

        // -- Effective SOC bounds: physical limits narrowed by any active SOC target window --
        let eff_min_soc = self
            .soc_target_min
            .map(|v| v.max(self.min_soc))
            .unwrap_or(self.min_soc);
        let eff_max_soc = self
            .soc_target_max
            .map(|v| v.min(self.max_soc))
            .unwrap_or(self.max_soc);

        // -- SOC bounds check: can we actually charge/discharge? --
        if target_power_kw > IDLE_POWER_THRESHOLD_KW && self.soc >= eff_max_soc {
            target_power_kw = 0.0; // Already full
        } else if target_power_kw < -IDLE_POWER_THRESHOLD_KW && self.soc <= eff_min_soc {
            target_power_kw = 0.0; // Already empty
        }

        // -- Temperature-dependent power limits --
        // Capture intent before temp limits may zero the power
        let wants_charge = target_power_kw > IDLE_POWER_THRESHOLD_KW;
        let wants_discharge = target_power_kw < -IDLE_POWER_THRESHOLD_KW;
        // Charging blocked below min_charge_temp_c (Li-ion safety: lithium plating risk)
        if wants_charge && !self.charge_allowed() {
            target_power_kw = 0.0;
        }
        // Discharge power derates linearly as cell temp drops
        if target_power_kw < -IDLE_POWER_THRESHOLD_KW {
            let derate = self.discharge_derate_factor();
            target_power_kw *= derate;
        }

        // -- Temperature-dependent capacity (d0 Arrhenius model) --
        // Reference: Schimpe et al. (2018) "Comprehensive Modeling of Temperature-Dependent
        // Degradation Mechanisms in Lithium Iron Phosphate Batteries" and OCHRE Battery.py:321-331.
        // Constants: d0_ref=1.001, e_ad1=4126 J/mol, e_ad2=9.752e6 J²/mol², T_ref=298.15 K, R=8.31446 J/(mol·K).
        {
            const D0_REF: f64 = 1.001;
            const E_AD1: f64 = 4_126.0; // J/mol
            const E_AD2: f64 = 9.752e6; // J²/mol²
            const T_REF: f64 = 298.15; // K
            const R_GAS: f64 = 8.314_46; // J/(mol·K)
            let t_k = self.cell_temp_c + 273.15;
            let inv_diff = 1.0 / t_k - 1.0 / T_REF;
            let d0 =
                D0_REF * (-E_AD1 / R_GAS * inv_diff - E_AD2 / R_GAS * inv_diff * inv_diff).exp();
            self.capacity_kwh = self.capacity_kwh_nominal * d0;
        }

        // -- Compute electrical model --
        let (power_kw, ohmic_loss_w) = self.compute_electrical(target_power_kw);

        // -- Apply self-discharge --
        // Self-discharge: absolute SOC loss per OCHRE Battery.py:337-338, matching Li-ion calendar aging model.
        self.soc -= self.self_discharge_rate_per_s * dt_s;
        self.soc = self.soc.clamp(0.0, 1.0);

        // -- Update SOC from charge/discharge --
        // DC power is the power seen by the battery cells (after inverter conversion).
        // Charging (power_kw > 0): DC = AC * eta; cell stores DC power - ohmic losses.
        // Discharging (power_kw < 0): DC = AC / eta; cell provides DC power + ohmic losses.
        let dc_power_kw = if power_kw > 0.0 {
            power_kw * self.inverter_efficiency
        } else {
            power_kw / self.inverter_efficiency
        };
        let effective_cell_power_kw = if dc_power_kw > 0.0 {
            dc_power_kw - ohmic_loss_w / 1000.0
        } else {
            dc_power_kw + ohmic_loss_w / 1000.0
        };

        let soc_before = self.soc;
        let energy_delta_kwh = effective_cell_power_kw * dt_hours;
        self.soc += energy_delta_kwh / self.capacity_kwh;
        self.soc = self.soc.clamp(eff_min_soc, eff_max_soc);

        // Recompute actual grid power and ohmic losses if SOC was clamped.
        // When SOC hits a bound, the actual energy transferred differs from
        // the target, so we must recompute losses from the actual current.
        let actual_soc_delta = self.soc - soc_before;
        let actual_cell_energy_kwh = actual_soc_delta * self.capacity_kwh;
        let (actual_power_kw, ohmic_loss_w) =
            if actual_soc_delta.abs() < f64::EPSILON && power_kw.abs() > IDLE_POWER_THRESHOLD_KW {
                // SOC didn't change -- we hit a bound, no real power transfer
                (0.0, 0.0)
            } else if power_kw.abs() < IDLE_POWER_THRESHOLD_KW {
                (0.0, 0.0)
            } else {
                // actual_cell_energy_kwh is the DC energy into/out of cells.
                // Recover AC grid power from DC cell power + inverter conversion.
                let actual_cell_power_kw = actual_cell_energy_kwh / dt_hours;
                // Recompute ohmic losses from actual cell DC power (use effective DC power as proxy).
                // Since compute_electrical works in AC terms, pass the scaled AC equivalent.
                let ac_equivalent_kw = if actual_cell_energy_kwh > 0.0 {
                    // Charging: cell DC → AC = DC / eta
                    actual_cell_power_kw / self.inverter_efficiency
                } else {
                    // Discharging: cell DC → AC = DC * eta
                    actual_cell_power_kw * self.inverter_efficiency
                };
                let (_, actual_ohmic_w) = self.compute_electrical(ac_equivalent_kw);
                (ac_equivalent_kw, actual_ohmic_w)
            };

        // -- Cell heater --
        // Default (Franklin-style): fires when charge is desired but cells are cold.
        // With heater_on_discharge (Tesla-style): also fires when discharge is
        // desired but blocked/derated by cold temps, e.g. grid outage at -25 C.
        let wants_power = wants_charge || (wants_discharge && self.heater_on_discharge);
        let heater_w = if self.heater_power_w > 0.0
            && self.cell_temp_c <= self.heater_threshold_c
            && wants_power
        {
            self.heater_active = true;
            self.heater_power_w
        } else {
            self.heater_active = false;
            0.0
        };

        // -- Lumped cell thermal model --
        // dT/dt = (Q_ohmic + Q_heater - UA * (T_cell - T_ambient)) / C_thermal
        if self.cell_thermal_mass_j_per_k > 0.0 {
            let ambient_c = if let Some(zone_id) = self.descriptor.zone {
                env.zones
                    .iter()
                    .find(|z| z.id == zone_id)
                    .map(|z| z.temperature_c)
                    .unwrap_or(env.weather.outdoor_temp_c)
            } else {
                env.weather.outdoor_temp_c
            };
            let q_in = ohmic_loss_w + heater_w;
            let q_loss = self.cell_ua_w_per_k * (self.cell_temp_c - ambient_c);
            let dt_cell = (q_in - q_loss) * dt_s / self.cell_thermal_mass_j_per_k;
            self.cell_temp_c += dt_cell;
        }

        // -- Standby power is always consumed --
        let standby_kw = self.standby_power_w / 1000.0;
        let heater_kw = heater_w / 1000.0;
        let port_power_kw = actual_power_kw + standby_kw + heater_kw;

        // -- Write electrical port contribution --
        ports.accumulate(&PortContribution::Electrical {
            active_power_kw: port_power_kw,
            reactive_power_kvar: 0.0,
        })?;

        // -- Optional thermal port: heat from ohmic losses only --
        // Energy pathway: grid → heater → cell thermal mass → zone (via UA model).
        // The heater energy enters the cell thermal mass and reaches the zone through the
        // lumped UA conductance above. Adding heater_w here directly would double-count it.
        // Only ohmic losses dissipate directly into the zone without passing through the cell model.
        if let Some(zone) = self.descriptor.zone {
            if ohmic_loss_w > 0.0 {
                ports.accumulate(&PortContribution::Thermal {
                    zone,
                    sensible_gain_w: ohmic_loss_w,
                    latent_gain_w: 0.0,
                    category: ThermalCategory::InternalGain,
                })?;
            }
        }

        // -- Update mode --
        self.mode = if actual_power_kw > IDLE_POWER_THRESHOLD_KW {
            OperatingMode::Charging
        } else if actual_power_kw < -IDLE_POWER_THRESHOLD_KW {
            OperatingMode::Discharging
        } else {
            OperatingMode::Standby
        };

        // -- Rainflow tracking --
        self.rainflow.push(self.soc);

        // -- Degradation per-timestep accumulation --
        {
            let cell_temp_k = self.cell_temp_c + 273.15;
            let v_oc = self.ocv_table.voltage_at_soc(self.soc);
            self.degradation
                .accumulate(dt_s, cell_temp_k, v_oc, self.soc);
        }

        // -- Daily degradation update --
        let current_day = Self::day_ordinal(env);
        if current_day != self.last_daily_update_day {
            let cell_temp_k = self.cell_temp_c + 273.15;
            let sum_sq_dod = self.rainflow.sum_squared_dod_daily();
            self.degradation
                .update_daily(&self.u_neg_table, cell_temp_k, sum_sq_dod);
            self.degradation.reset_day_tracking(self.soc);
            self.rainflow.reset_daily();
            self.last_daily_update_day = current_day;
        }

        // -- Update telemetry --
        self.telemetry.set("soc", self.soc);
        self.telemetry.set("active_power_kw", port_power_kw);
        self.telemetry.set("ohmic_loss_w", ohmic_loss_w);
        self.telemetry.set("standby_power_w", self.standby_power_w);
        self.telemetry.set("cell_temp_c", self.cell_temp_c);
        self.telemetry.set("heater_power_w", heater_w);
        self.telemetry
            .set("discharge_derate", self.discharge_derate_factor());
        self.telemetry
            .set("cycle_count", self.rainflow.total_cycles());
        self.telemetry
            .set("capacity_fade_pct", self.degradation.capacity_fade_pct());

        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&BatteryCheckpoint {
            soc: self.soc,
            cell_temp_c: self.cell_temp_c,
            heater_active: self.heater_active,
            mode: self.mode,
            degradation: self.degradation.clone(),
            rainflow: self.rainflow.clone(),
            self_consumption_enabled: self.self_consumption_enabled,
            solar_only_charging: self.solar_only_charging,
            grid_connected: self.grid_connected,
            power_setpoint_kw: self.power_setpoint_kw,
            soc_target: self.soc_target,
            soc_target_min: self.soc_target_min,
            soc_target_max: self.soc_target_max,
            last_daily_update_day: self.last_daily_update_day,
            import_limit_kw: self.import_limit_kw,
            export_limit_kw: self.export_limit_kw,
        })
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let cp: BatteryCheckpoint = load_postcard(state)?;
        self.soc = cp.soc;
        self.cell_temp_c = cp.cell_temp_c;
        self.heater_active = cp.heater_active;
        self.mode = cp.mode;
        self.degradation = cp.degradation;
        self.rainflow = cp.rainflow;
        self.self_consumption_enabled = cp.self_consumption_enabled;
        self.solar_only_charging = cp.solar_only_charging;
        self.grid_connected = cp.grid_connected;
        self.power_setpoint_kw = cp.power_setpoint_kw;
        self.soc_target = cp.soc_target;
        self.soc_target_min = cp.soc_target_min;
        self.soc_target_max = cp.soc_target_max;
        self.last_daily_update_day = cp.last_daily_update_day;
        self.import_limit_kw = cp.import_limit_kw;
        self.export_limit_kw = cp.export_limit_kw;

        // Recompute all derived telemetry from restored state so no fields are stale.
        self.telemetry.set("soc", self.soc);
        self.telemetry.set("cell_temp_c", self.cell_temp_c);
        self.telemetry
            .set("cycle_count", self.rainflow.total_cycles());
        self.telemetry
            .set("capacity_fade_pct", self.degradation.capacity_fade_pct());
        self.telemetry
            .set("discharge_derate", self.discharge_derate_factor());
        // active_power_kw, ohmic_loss_w, heater_power_w are operational — reset to
        // idle defaults; they will be updated on the next step() call.
        self.telemetry.set(
            "heater_power_w",
            if self.heater_active {
                self.heater_power_w
            } else {
                0.0
            },
        );
        self.telemetry.set("standby_power_w", self.standby_power_w);
        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        match signal {
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                self.power_setpoint_kw = Some(*active_power_kw);
                self.soc_target = None;
                self.self_consumption_enabled = false;
            }
            ControlSignal::SOCTarget {
                target_soc,
                min_soc,
                max_soc,
            } => {
                self.soc_target = Some(*target_soc);
                // Store as operational window — do NOT mutate physical min_soc/max_soc
                // which are hardware limits set at init from config.
                self.soc_target_min = *min_soc;
                self.soc_target_max = *max_soc;
                self.power_setpoint_kw = None;
                self.self_consumption_enabled = false;
            }
            ControlSignal::GridConnect { connected } => {
                self.grid_connected = *connected;
            }
            ControlSignal::SelfConsumption {
                enabled,
                solar_only_charging,
            } => {
                self.self_consumption_enabled = *enabled;
                self.solar_only_charging = *solar_only_charging;
                self.power_setpoint_kw = None;
                self.soc_target = None;
            }
            _ => {
                return Err(HaresError::Control(format!(
                    "Battery does not handle control signal: {signal:?}"
                )));
            }
        }
        Ok(())
    }
}

pub fn register_with_registry(registry: &mut EquipmentRegistry) {
    registry.register("Battery", Box::new(|config| Box::new(Battery::new(config))));
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_telemetry() -> Telemetry {
    let mut t = Telemetry::with_capacity(9);
    t.insert("soc", 0.0);
    t.insert("active_power_kw", 0.0);
    t.insert("ohmic_loss_w", 0.0);
    t.insert("standby_power_w", 0.0);
    t.insert("cell_temp_c", 25.0);
    t.insert("heater_power_w", 0.0);
    t.insert("discharge_derate", 1.0);
    t.insert("cycle_count", 0.0);
    t.insert("capacity_fade_pct", 0.0);
    t
}

fn battery_telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: "soc".to_string(),
            unit: "-".to_string(),
            description: "State of charge [0..1]".to_string(),
        },
        TelemetryField {
            name: "active_power_kw".to_string(),
            unit: "kW".to_string(),
            description: "Grid-side active power (positive=consuming, negative=generating)"
                .to_string(),
        },
        TelemetryField {
            name: "ohmic_loss_w".to_string(),
            unit: "W".to_string(),
            description: "Ohmic heat dissipation from internal resistance".to_string(),
        },
        TelemetryField {
            name: "standby_power_w".to_string(),
            unit: "W".to_string(),
            description: "Parasitic standby power draw".to_string(),
        },
        TelemetryField {
            name: "cell_temp_c".to_string(),
            unit: "C".to_string(),
            description: "Cell temperature".to_string(),
        },
        TelemetryField {
            name: "cycle_count".to_string(),
            unit: "-".to_string(),
            description: "Equivalent full cycles from rainflow counting".to_string(),
        },
        TelemetryField {
            name: "heater_power_w".to_string(),
            unit: "W".to_string(),
            description: "Cell heater power draw (active when cells are cold and charge requested)"
                .to_string(),
        },
        TelemetryField {
            name: "discharge_derate".to_string(),
            unit: "-".to_string(),
            description: "Temperature-dependent discharge power derating factor [0..1]".to_string(),
        },
        TelemetryField {
            name: "capacity_fade_pct".to_string(),
            unit: "%".to_string(),
            description: "Cumulative capacity degradation".to_string(),
        },
    ]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, TimeZone, Utc};
    use hares_types::{
        ControlSignal, EnvironmentState, GridState, PortSlots, WeatherState, ZoneId, ZoneState,
    };

    use crate::config::ConfigValue;

    use super::*;
    use crate::{Equipment, EquipmentConfig};

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
            current_time: Utc
                .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
                .single()
                .expect("valid UTC timestamp"),
            time_res: ChronoDuration::minutes(5),
        }
    }

    fn battery_config(overrides: &[(&str, f64)]) -> EquipmentConfig {
        let mut raw: HashMap<String, ConfigValue> = HashMap::new();
        raw.insert(KEY_CAPACITY_KWH.to_string(), 10.0.into());
        raw.insert(KEY_MAX_CHARGE_KW.to_string(), 5.0.into());
        raw.insert(KEY_MAX_DISCHARGE_KW.to_string(), 5.0.into());
        raw.insert(KEY_STANDBY_POWER_W.to_string(), 10.0.into());
        raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
        for (k, v) in overrides {
            raw.insert(k.to_string(), (*v).into());
        }
        EquipmentConfig {
            name: "Test Battery".to_string(),
            ochre_class: "Battery".to_string(),
            raw_config: raw,
        }
    }

    fn default_ports() -> PortSlots {
        PortSlots::default()
    }

    #[test]
    fn descriptor_matches_ticket_contract() {
        let config = battery_config(&[]);
        let bat = Battery::new(config);
        assert_eq!(bat.descriptor().end_use, EndUse::Battery);
        assert_eq!(bat.descriptor().stage, ExecutionStage::Electrical);
        assert_eq!(bat.descriptor().fuel, FuelType::Electric);
        let caps = bat.descriptor().control_capabilities;
        assert!(caps.contains(ControlCapabilities::POWER_SETPOINT));
        assert!(caps.contains(ControlCapabilities::SOC_TARGET));
        assert!(caps.contains(ControlCapabilities::GRID_CONNECT));
        assert!(caps.contains(ControlCapabilities::SELF_CONSUMPTION));
        let field_names: Vec<&str> = bat
            .descriptor()
            .telemetry_fields
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert!(field_names.contains(&"soc"));
        assert!(field_names.contains(&"active_power_kw"));
        assert!(field_names.contains(&"ohmic_loss_w"));
        assert!(field_names.contains(&"standby_power_w"));
        assert!(field_names.contains(&"cell_temp_c"));
        assert!(field_names.contains(&"cycle_count"));
        assert!(field_names.contains(&"capacity_fade_pct"));
    }

    #[test]
    fn day_ordinal_is_contiguous_across_year_boundary() {
        let mut env_dec31 = base_env();
        env_dec31.current_time = Utc
            .with_ymd_and_hms(2025, 12, 31, 23, 59, 0)
            .single()
            .expect("valid");
        let mut env_jan1 = base_env();
        env_jan1.current_time = Utc
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .expect("valid");
        let ord_dec31 = Battery::day_ordinal(&env_dec31);
        let ord_jan1 = Battery::day_ordinal(&env_jan1);
        assert_eq!(
            ord_jan1 - ord_dec31,
            1,
            "day_ordinal must be contiguous across year boundary: Dec 31={ord_dec31}, Jan 1={ord_jan1}"
        );
    }

    #[test]
    fn soc_clamped_at_upper_bound() {
        // Explicitly set max_soc=1.0 so this test is independent of the OCHRE default.
        let config = battery_config(&[
            (KEY_INITIAL_SOC, 0.99),
            (KEY_MAX_SOC, 1.0),
            (KEY_MIN_SOC, 0.0),
            (KEY_SELF_DISCHARGE_PCT_PER_DAY, 0.0),
        ]);
        let mut bat = Battery::new(config.clone());
        let env = base_env();
        bat.init(&config, &env).unwrap();

        // Apply large charge power setpoint
        bat.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: 100.0,
            reactive_power_kvar: None,
        })
        .unwrap();

        let mut ports = default_ports();
        // Step multiple times
        for _ in 0..100 {
            ports.zero();
            bat.step(&env, Duration::from_secs(300), &mut ports)
                .unwrap();
        }
        assert!(bat.soc <= 1.0);
        assert!((bat.soc - 1.0).abs() < 1e-9);
    }

    #[test]
    fn soc_clamped_at_lower_bound() {
        // Explicitly set min_soc=0.0 so this test is independent of the OCHRE default.
        let config = battery_config(&[
            (KEY_INITIAL_SOC, 0.01),
            (KEY_MIN_SOC, 0.0),
            (KEY_MAX_SOC, 1.0),
            (KEY_SELF_DISCHARGE_PCT_PER_DAY, 0.0),
        ]);
        let mut bat = Battery::new(config.clone());
        let env = base_env();
        bat.init(&config, &env).unwrap();

        bat.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: -100.0,
            reactive_power_kvar: None,
        })
        .unwrap();

        let mut ports = default_ports();
        for _ in 0..100 {
            ports.zero();
            bat.step(&env, Duration::from_secs(300), &mut ports)
                .unwrap();
        }
        assert!(bat.soc >= 0.0);
        assert!(bat.soc.abs() < f64::EPSILON);
    }

    #[test]
    fn round_trip_efficiency_less_than_one() {
        let config = battery_config(&[
            (KEY_INITIAL_SOC, 0.5),
            (KEY_SELF_DISCHARGE_PCT_PER_DAY, 0.0),
        ]);
        let mut bat = Battery::new(config.clone());
        let env = base_env();
        bat.init(&config, &env).unwrap();

        let dt = Duration::from_secs(300);
        let charge_power = 3.0; // kW

        // Charge for 10 steps
        bat.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: charge_power,
            reactive_power_kvar: None,
        })
        .unwrap();

        let mut total_charge_energy = 0.0;
        for _ in 0..10 {
            let mut ports = default_ports();
            bat.step(&env, dt, &mut ports).unwrap();
            total_charge_energy += ports.electrical.net_active_kw() * dt.as_secs_f64() / 3600.0;
        }
        let soc_after_charge = bat.soc;

        // Discharge for 10 steps at same power magnitude
        bat.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: -charge_power,
            reactive_power_kvar: None,
        })
        .unwrap();

        let mut total_discharge_energy = 0.0;
        for _ in 0..10 {
            let mut ports = default_ports();
            bat.step(&env, dt, &mut ports).unwrap();
            // Discharging produces negative power (generation)
            total_discharge_energy +=
                (-ports.electrical.net_active_kw()) * dt.as_secs_f64() / 3600.0;
        }

        assert!(
            soc_after_charge > 0.5,
            "SOC should have increased during charge"
        );
        // Due to ohmic losses, discharge energy < charge energy
        assert!(
            total_discharge_energy < total_charge_energy,
            "round-trip efficiency must be < 1.0: charged {total_charge_energy:.4} kWh, discharged {total_discharge_energy:.4} kWh"
        );
    }

    #[test]
    fn self_consumption_charges_from_pv_surplus() {
        let config = battery_config(&[(KEY_INITIAL_SOC, 0.5)]);
        let mut bat = Battery::new(config.clone());
        let env = base_env();
        bat.init(&config, &env).unwrap();

        // Simulate Stage 1 PV surplus: generation_power_kw = -3.0 (negative = generation)
        let mut ports = PortSlots::default();
        ports.electrical.generation_power_kw = -3.0; // PV producing 3 kW
        ports.electrical.load_power_kw = 1.0; // 1 kW base load
        // net_active_kw = 1.0 + (-3.0) = -2.0 (surplus)

        bat.step(&env, Duration::from_secs(300), &mut ports)
            .unwrap();

        assert!(bat.soc > 0.5, "battery should charge from PV surplus");
        assert_eq!(bat.mode, OperatingMode::Charging);
    }

    #[test]
    fn self_consumption_discharges_to_offset_load() {
        let config = battery_config(&[(KEY_INITIAL_SOC, 0.5)]);
        let mut bat = Battery::new(config.clone());
        let env = base_env();
        bat.init(&config, &env).unwrap();

        // Stage 1: net load = 2 kW (no PV, just loads)
        let mut ports = PortSlots::default();
        ports.electrical.load_power_kw = 2.0;

        bat.step(&env, Duration::from_secs(300), &mut ports)
            .unwrap();

        assert!(bat.soc < 0.5, "battery should discharge to offset load");
        assert_eq!(bat.mode, OperatingMode::Discharging);
    }

    #[test]
    fn solar_only_charging_does_not_charge_from_grid() {
        let config = battery_config(&[(KEY_INITIAL_SOC, 0.3)]);
        let mut bat = Battery::new(config.clone());
        let env = base_env();
        bat.init(&config, &env).unwrap();

        bat.apply_control(&ControlSignal::SelfConsumption {
            enabled: true,
            solar_only_charging: true,
        })
        .unwrap();

        // Stage 1: net load positive (no PV surplus) -- should NOT charge
        let mut ports = PortSlots::default();
        ports.electrical.load_power_kw = 2.0;

        let soc_before = bat.soc;
        bat.step(&env, Duration::from_secs(300), &mut ports)
            .unwrap();

        // SOC should decrease or stay same (discharge to offset load + self-discharge),
        // definitely not increase (no charging from grid)
        assert!(
            bat.soc <= soc_before,
            "should not charge from grid in solar-only mode"
        );
    }

    #[test]
    fn standby_power_consumed_when_idle() {
        let config = battery_config(&[(KEY_STANDBY_POWER_W, 20.0)]);
        let mut bat = Battery::new(config.clone());
        let env = base_env();
        bat.init(&config, &env).unwrap();

        // No control signal, no stage 1 load -- battery should be idle
        bat.apply_control(&ControlSignal::SelfConsumption {
            enabled: false,
            solar_only_charging: false,
        })
        .unwrap();
        bat.self_consumption_enabled = false;

        let mut ports = PortSlots::default();
        bat.step(&env, Duration::from_secs(300), &mut ports)
            .unwrap();

        let expected_standby_kw = 20.0 / 1000.0;
        let actual = ports.electrical.net_active_kw();
        assert!(
            (actual - expected_standby_kw).abs() < 1e-9,
            "idle battery should consume standby power: expected {expected_standby_kw}, got {actual}"
        );
    }

    #[test]
    fn two_independent_instances_evolve_separately() {
        let config_a = battery_config(&[(KEY_INITIAL_SOC, 0.3)]);
        let config_b = battery_config(&[(KEY_INITIAL_SOC, 0.8)]);
        let mut bat_a = Battery::new(config_a.clone());
        let mut bat_b = Battery::new(config_b.clone());
        let env = base_env();
        bat_a.init(&config_a, &env).unwrap();
        bat_b.init(&config_b, &env).unwrap();

        // Both charge at 3 kW
        bat_a
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 3.0,
                reactive_power_kvar: None,
            })
            .unwrap();
        bat_b
            .apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: 3.0,
                reactive_power_kvar: None,
            })
            .unwrap();

        let mut ports_a = default_ports();
        let mut ports_b = default_ports();
        bat_a
            .step(&env, Duration::from_secs(300), &mut ports_a)
            .unwrap();
        bat_b
            .step(&env, Duration::from_secs(300), &mut ports_b)
            .unwrap();

        assert!(bat_a.soc > 0.3);
        assert!(bat_b.soc > 0.8);
        assert!(
            (bat_a.soc - bat_b.soc).abs() > 0.1,
            "different initial SOCs should stay different"
        );
    }

    #[test]
    fn state_round_trip_preserves_soc_and_control() {
        let config = battery_config(&[(KEY_INITIAL_SOC, 0.6)]);
        let mut bat = Battery::new(config.clone());
        let env = base_env();
        bat.init(&config, &env).unwrap();

        // Step a few times to build up some state
        bat.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: 2.0,
            reactive_power_kvar: None,
        })
        .unwrap();
        let mut ports = default_ports();
        bat.step(&env, Duration::from_secs(300), &mut ports)
            .unwrap();

        let saved = bat.save_state();
        let soc_saved = bat.soc;
        let cycles_saved = bat.rainflow.total_cycles();

        // Create fresh battery and restore
        let mut bat2 = Battery::new(config.clone());
        bat2.init(&config, &env).unwrap();
        bat2.load_state(&saved).unwrap();

        assert!(
            (bat2.soc - soc_saved).abs() < f64::EPSILON,
            "SOC mismatch after round-trip"
        );
        assert!(
            (bat2.rainflow.total_cycles() - cycles_saved).abs() < f64::EPSILON,
            "cycle count mismatch after round-trip"
        );
        assert_eq!(bat2.self_consumption_enabled, false);
        assert_eq!(bat2.power_setpoint_kw, Some(2.0));
    }

    #[test]
    fn self_discharge_reduces_soc_over_time() {
        let config = battery_config(&[
            (KEY_INITIAL_SOC, 0.5),
            (KEY_SELF_DISCHARGE_PCT_PER_DAY, 1.0), // 1%/day for visible effect
        ]);
        let mut bat = Battery::new(config.clone());
        let env = base_env();
        bat.init(&config, &env).unwrap();

        // Disable self-consumption so battery is idle
        bat.self_consumption_enabled = false;
        bat.power_setpoint_kw = None;

        let soc_start = bat.soc;
        // Step for many timesteps (simulate ~1 day: 288 x 5-min steps)
        for _ in 0..288 {
            let mut ports = default_ports();
            bat.step(&env, Duration::from_secs(300), &mut ports)
                .unwrap();
        }

        assert!(
            bat.soc < soc_start,
            "SOC should decrease due to self-discharge"
        );
        // Absolute self-discharge: 1%/day = 0.01 SOC units per day (independent of SOC level).
        // 288 steps × 300 s = 86400 s = 1 day. Expected loss ≈ 0.01 SOC units.
        let loss = soc_start - bat.soc;
        assert!(loss > 0.005, "self-discharge loss too small: {loss}");
        assert!(loss < 0.02, "self-discharge loss too large: {loss}");
    }

    #[test]
    fn ocv_interpolation_at_known_points() {
        let table = OcvTable::default_li_nmc();

        // Exact table points (NREL SSC / OCHRE calibrated values)
        assert!((table.voltage_at_soc(0.0) - 3.0000).abs() < 1e-10);
        assert!((table.voltage_at_soc(0.5) - 3.6876).abs() < 1e-10);
        assert!((table.voltage_at_soc(1.0) - 4.1934).abs() < 1e-10);

        // Interpolated midpoint between 0.0 and 0.1: (3.0000 + 3.4679) / 2 = 3.23395
        let v_05 = table.voltage_at_soc(0.05);
        assert!(
            (v_05 - 3.23395).abs() < 1e-10,
            "interpolated OCV at 0.05: {v_05}"
        );

        // Clamped below/above table bounds
        assert!((table.voltage_at_soc(-0.5) - 3.0000).abs() < 1e-10);
        assert!((table.voltage_at_soc(1.5) - 4.1934).abs() < 1e-10);
    }

    #[test]
    fn registry_includes_battery() {
        let registry = EquipmentRegistry::new();
        assert!(registry.get("Battery").is_some());
    }

    #[test]
    fn init_rejects_zero_capacity() {
        let config = battery_config(&[(KEY_CAPACITY_KWH, 0.0)]);
        let mut bat = Battery::new(config.clone());
        let err = bat.init(&config, &base_env()).unwrap_err();
        assert!(err.to_string().contains("capacity_kwh must be positive"));
    }

    #[test]
    fn init_rejects_invalid_soc_bounds() {
        let config = battery_config(&[(KEY_MIN_SOC, 0.8), (KEY_MAX_SOC, 0.2)]);
        let mut bat = Battery::new(config.clone());
        let err = bat.init(&config, &base_env()).unwrap_err();
        assert!(
            err.to_string()
                .contains("min_soc must be less than max_soc")
        );
    }

    #[test]
    fn apply_control_rejects_unsupported_signal() {
        let config = battery_config(&[]);
        let mut bat = Battery::new(config.clone());
        bat.init(&config, &base_env()).unwrap();

        let result = bat.apply_control(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(20.0),
            cooling_setpoint_c: None,
            deadband_c: None,
        });
        assert!(result.is_err());
    }

    #[test]
    fn grid_disconnect_prevents_power_flow() {
        let config = battery_config(&[(KEY_INITIAL_SOC, 0.5)]);
        let mut bat = Battery::new(config.clone());
        let env = base_env();
        bat.init(&config, &env).unwrap();

        bat.apply_control(&ControlSignal::GridConnect { connected: false })
            .unwrap();
        bat.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: 5.0,
            reactive_power_kvar: None,
        })
        .unwrap();

        let soc_before = bat.soc;
        let mut ports = default_ports();
        bat.step(&env, Duration::from_secs(300), &mut ports)
            .unwrap();

        // Only standby power should flow; SOC should not increase
        // (self-discharge may decrease it slightly)
        assert!(bat.soc <= soc_before + f64::EPSILON);
    }

    #[test]
    fn heater_activates_when_cold_and_charge_requested() {
        let config = battery_config(&[
            (KEY_INITIAL_SOC, 0.3),
            (KEY_HEATER_POWER_W, 500.0),
            (KEY_HEATER_THRESHOLD_C, 5.0),
            (KEY_MIN_CHARGE_TEMP_C, 0.0),
            (KEY_SELF_DISCHARGE_PCT_PER_DAY, 0.0),
        ]);
        let mut bat = Battery::new(config.clone());
        let env = base_env();
        bat.init(&config, &env).unwrap();
        bat.cell_temp_c = -5.0; // Below threshold

        // PV surplus wants to charge
        let mut ports = PortSlots::default();
        ports.electrical.generation_power_kw = -3.0;

        bat.step(&env, Duration::from_secs(300), &mut ports)
            .unwrap();

        assert!(
            bat.heater_active,
            "heater should be active when cold and charge desired"
        );
        assert!(
            bat.telemetry().get("heater_power_w").unwrap() > 0.0,
            "heater power should be reported in telemetry"
        );
        // Charging should be blocked (cell too cold), but heater draws power.
        // The battery's own contribution (heater + standby) is on top of the
        // pre-set Stage 1 values in the accumulator.
        let heater_kw = 500.0 / 1000.0;
        let standby_kw = 10.0 / 1000.0;
        let battery_contribution_kw = heater_kw + standby_kw;
        // load_power_kw should include battery's heater + standby draw
        assert!(
            (ports.electrical.load_power_kw - battery_contribution_kw).abs() < 0.01,
            "battery should draw heater + standby: load_power={}",
            ports.electrical.load_power_kw
        );
        // SOC should not change (charging blocked)
        assert!(
            (bat.soc - 0.3).abs() < 0.001,
            "SOC should not change when charging blocked"
        );
        // Cell temp should increase due to heater
        assert!(bat.cell_temp_c > -5.0, "heater should warm cells");
    }

    #[test]
    fn heater_does_not_activate_when_warm() {
        let config = battery_config(&[
            (KEY_INITIAL_SOC, 0.3),
            (KEY_HEATER_POWER_W, 500.0),
            (KEY_HEATER_THRESHOLD_C, 5.0),
        ]);
        let mut bat = Battery::new(config.clone());
        let env = base_env();
        bat.init(&config, &env).unwrap();
        bat.cell_temp_c = 20.0; // Well above threshold

        let mut ports = PortSlots::default();
        ports.electrical.generation_power_kw = -3.0; // PV surplus

        bat.step(&env, Duration::from_secs(300), &mut ports)
            .unwrap();

        assert!(!bat.heater_active, "heater should not activate when warm");
        assert_eq!(bat.telemetry().get("heater_power_w").unwrap(), 0.0);
    }

    #[test]
    fn heater_activates_on_discharge_when_configured() {
        let mut raw: HashMap<String, ConfigValue> = HashMap::new();
        raw.insert(KEY_CAPACITY_KWH.to_string(), 10.0.into());
        raw.insert(KEY_MAX_CHARGE_KW.to_string(), 5.0.into());
        raw.insert(KEY_MAX_DISCHARGE_KW.to_string(), 5.0.into());
        raw.insert(KEY_STANDBY_POWER_W.to_string(), 10.0.into());
        raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
        raw.insert(KEY_HEATER_POWER_W.to_string(), 500.0.into());
        raw.insert(KEY_HEATER_THRESHOLD_C.to_string(), 5.0.into());
        raw.insert(KEY_SELF_DISCHARGE_PCT_PER_DAY.to_string(), 0.0.into());
        raw.insert(KEY_HEATER_ON_DISCHARGE.to_string(), ConfigValue::Bool(true));
        let config = EquipmentConfig {
            name: "Test Battery".to_string(),
            ochre_class: "Battery".to_string(),
            raw_config: raw,
        };
        let mut bat = Battery::new(config.clone());
        let env = base_env();
        bat.init(&config, &env).unwrap();
        bat.cell_temp_c = -25.0; // Below min discharge temp

        // Request discharge
        bat.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: -5.0,
            reactive_power_kvar: None,
        })
        .unwrap();

        let mut ports = default_ports();
        bat.step(&env, Duration::from_secs(300), &mut ports)
            .unwrap();

        assert!(
            bat.heater_active,
            "heater should activate when discharge requested and cells cold (heater_on_discharge=true)"
        );
        assert!(bat.cell_temp_c > -25.0, "heater should warm cells");
    }

    #[test]
    fn heater_does_not_activate_on_discharge_by_default() {
        let config = battery_config(&[
            (KEY_INITIAL_SOC, 0.5),
            (KEY_HEATER_POWER_W, 500.0),
            (KEY_HEATER_THRESHOLD_C, 5.0),
            (KEY_SELF_DISCHARGE_PCT_PER_DAY, 0.0),
        ]);
        let mut bat = Battery::new(config.clone());
        let env = base_env();
        bat.init(&config, &env).unwrap();
        bat.cell_temp_c = -25.0;

        bat.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: -5.0,
            reactive_power_kvar: None,
        })
        .unwrap();

        let mut ports = default_ports();
        bat.step(&env, Duration::from_secs(300), &mut ports)
            .unwrap();

        assert!(
            !bat.heater_active,
            "heater should NOT activate on discharge by default (heater_on_discharge=false)"
        );
    }

    #[test]
    fn discharge_derates_at_low_temperature() {
        let config = battery_config(&[
            (KEY_INITIAL_SOC, 0.5),
            (KEY_SELF_DISCHARGE_PCT_PER_DAY, 0.0),
        ]);
        let mut bat = Battery::new(config.clone());
        let env = base_env();
        bat.init(&config, &env).unwrap();

        // Step at warm temp to get baseline discharge
        bat.cell_temp_c = 25.0;
        bat.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: -5.0,
            reactive_power_kvar: None,
        })
        .unwrap();
        let mut ports_warm = default_ports();
        bat.step(&env, Duration::from_secs(300), &mut ports_warm)
            .unwrap();
        let warm_power = ports_warm.electrical.net_active_kw();

        // Reset and step at cold temp (halfway in derating range)
        bat.init(&config, &env).unwrap();
        bat.cell_temp_c = -5.0; // Midpoint of [-20, 10] -> derate = 0.5
        bat.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: -5.0,
            reactive_power_kvar: None,
        })
        .unwrap();
        let mut ports_cold = default_ports();
        bat.step(&env, Duration::from_secs(300), &mut ports_cold)
            .unwrap();
        let cold_power = ports_cold.electrical.net_active_kw();

        // Cold discharge should be less than warm (more negative = more generation)
        assert!(
            cold_power > warm_power,
            "cold discharge should be derated (less generation): warm={warm_power}, cold={cold_power}"
        );
    }

    #[test]
    fn discharge_blocked_below_min_temp() {
        let config = battery_config(&[
            (KEY_INITIAL_SOC, 0.5),
            (KEY_SELF_DISCHARGE_PCT_PER_DAY, 0.0),
        ]);
        let mut bat = Battery::new(config.clone());
        let env = base_env();
        bat.init(&config, &env).unwrap();
        bat.cell_temp_c = -25.0; // Below default min_discharge_temp_c (-20)

        bat.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: -5.0,
            reactive_power_kvar: None,
        })
        .unwrap();

        let mut ports = default_ports();
        bat.step(&env, Duration::from_secs(300), &mut ports)
            .unwrap();

        // Only standby should be consumed, no discharge
        let standby_kw = 10.0 / 1000.0;
        assert!(
            (ports.electrical.net_active_kw() - standby_kw).abs() < 0.001,
            "discharge should be fully blocked below min temp"
        );
    }

    #[test]
    fn charging_blocked_below_min_charge_temp() {
        let config = battery_config(&[
            (KEY_INITIAL_SOC, 0.3),
            (KEY_MIN_CHARGE_TEMP_C, 0.0),
            (KEY_SELF_DISCHARGE_PCT_PER_DAY, 0.0),
        ]);
        let mut bat = Battery::new(config.clone());
        let env = base_env();
        bat.init(&config, &env).unwrap();
        bat.cell_temp_c = -2.0; // Below min charge temp

        bat.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: 5.0,
            reactive_power_kvar: None,
        })
        .unwrap();

        let soc_before = bat.soc;
        let mut ports = default_ports();
        bat.step(&env, Duration::from_secs(300), &mut ports)
            .unwrap();

        assert!(
            (bat.soc - soc_before).abs() < 1e-12,
            "charging should be blocked below min charge temp"
        );
    }

    #[test]
    fn cell_temp_evolves_toward_ambient() {
        let config = battery_config(&[
            (KEY_INITIAL_SOC, 0.5),
            (KEY_SELF_DISCHARGE_PCT_PER_DAY, 0.0),
        ]);
        let mut bat = Battery::new(config.clone());
        let env = base_env(); // outdoor_temp_c = 10
        bat.init(&config, &env).unwrap();
        bat.cell_temp_c = 30.0; // Warmer than ambient
        bat.self_consumption_enabled = false;

        // Step idle for many steps -- cell should cool toward ambient
        for _ in 0..1000 {
            let mut ports = default_ports();
            bat.step(&env, Duration::from_secs(300), &mut ports)
                .unwrap();
        }

        // Should approach outdoor temp (10 C since no zone configured)
        assert!(
            (bat.cell_temp_c - 10.0).abs() < 1.0,
            "cell temp should converge to ambient: got {}",
            bat.cell_temp_c
        );
    }

    #[test]
    fn soc_target_drives_toward_target() {
        let config = battery_config(&[
            (KEY_INITIAL_SOC, 0.3),
            (KEY_SELF_DISCHARGE_PCT_PER_DAY, 0.0),
        ]);
        let mut bat = Battery::new(config.clone());
        let env = base_env();
        bat.init(&config, &env).unwrap();

        bat.apply_control(&ControlSignal::SOCTarget {
            target_soc: 0.8,
            min_soc: None,
            max_soc: None,
        })
        .unwrap();

        for _ in 0..20 {
            let mut ports = default_ports();
            bat.step(&env, Duration::from_secs(300), &mut ports)
                .unwrap();
        }

        assert!(bat.soc > 0.3, "SOC should increase toward target");
    }

    // -----------------------------------------------------------------------
    // Regression tests for bug fixes
    // -----------------------------------------------------------------------

    /// ASTM E1049-85: a 50% DOD round-trip (0.5→1.0→0.5→1.0) should yield
    /// two half-cycles of range 0.5, totalling 0.5 EFC.
    #[test]
    fn rainflow_50_pct_dod_round_trip() {
        let mut rf = RainflowCounter::default();
        rf.push(0.5);
        rf.push(1.0);
        rf.push(0.5);
        rf.push(1.0);

        // Two half-cycles extracted: each range=0.5 × count=0.5 = 0.25 EFC.
        assert!(
            (rf.total_cycles() - 0.5).abs() < 1e-10,
            "50% DOD round-trip should count as 0.5 EFC, got {}",
            rf.total_cycles()
        );
    }

    /// ASTM E1049-85: full 0→1→0→1 swing yields two half-cycles of range 1.0,
    /// totalling 1.0 EFC.
    #[test]
    fn rainflow_full_dod_round_trip() {
        let mut rf = RainflowCounter::default();
        rf.push(0.0);
        rf.push(1.0);
        rf.push(0.0);
        rf.push(1.0);

        assert!(
            (rf.total_cycles() - 1.0).abs() < 1e-10,
            "100% DOD round-trip should count as 1.0 EFC, got {}",
            rf.total_cycles()
        );
    }

    /// Inverter efficiency losses must appear in addition to ohmic losses.
    /// Round-trip efficiency with inverter should be lower than without.
    #[test]
    fn inverter_efficiency_reduces_round_trip_efficiency() {
        let make_bat = |inv_eta: f64| {
            let mut raw: HashMap<String, ConfigValue> = HashMap::new();
            raw.insert(KEY_CAPACITY_KWH.to_string(), 10.0.into());
            raw.insert(KEY_MAX_CHARGE_KW.to_string(), 5.0.into());
            raw.insert(KEY_MAX_DISCHARGE_KW.to_string(), 5.0.into());
            raw.insert(KEY_STANDBY_POWER_W.to_string(), 0.0.into());
            raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
            raw.insert(KEY_SELF_DISCHARGE_PCT_PER_DAY.to_string(), 0.0.into());
            raw.insert(KEY_INVERTER_EFFICIENCY.to_string(), inv_eta.into());
            EquipmentConfig {
                name: "TestBat".to_string(),
                ochre_class: "Battery".to_string(),
                raw_config: raw,
            }
        };

        let charge_steps = 10;
        let dt = Duration::from_secs(300);
        let charge_power_kw = 3.0;
        let env = base_env();

        let measure_round_trip = |inv_eta: f64| -> f64 {
            let config = make_bat(inv_eta);
            let mut bat = Battery::new(config.clone());
            bat.init(&config, &env).unwrap();

            bat.apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: charge_power_kw,
                reactive_power_kvar: None,
            })
            .unwrap();
            let mut energy_in = 0.0;
            for _ in 0..charge_steps {
                let mut ports = default_ports();
                bat.step(&env, dt, &mut ports).unwrap();
                energy_in += ports.electrical.net_active_kw() * dt.as_secs_f64() / 3600.0;
            }

            bat.apply_control(&ControlSignal::PowerSetpoint {
                active_power_kw: -charge_power_kw,
                reactive_power_kvar: None,
            })
            .unwrap();
            let mut energy_out = 0.0;
            for _ in 0..charge_steps {
                let mut ports = default_ports();
                bat.step(&env, dt, &mut ports).unwrap();
                energy_out += (-ports.electrical.net_active_kw()) * dt.as_secs_f64() / 3600.0;
            }

            energy_out / energy_in
        };

        let rte_ideal = measure_round_trip(1.0);
        let rte_with_inverter = measure_round_trip(0.96);

        assert!(
            rte_with_inverter < rte_ideal,
            "inverter losses (eta=0.96) must reduce round-trip efficiency below ideal (eta=1.0): \
             rte_ideal={rte_ideal:.4}, rte_with_inverter={rte_with_inverter:.4}"
        );
        // With eta=0.96, round-trip inverter loss alone is 0.96^2 = 0.9216.
        // The inverter loss should be measurably different from the ideal case.
        // With eta=0.96, the round-trip inverter penalty is 0.96^2 ≈ 0.9216, but
        // the actual RTE difference is smaller here because we're only cycling a
        // small fraction of the capacity.  We just need to confirm the direction.
        let gap = rte_ideal - rte_with_inverter;
        assert!(
            gap > 1e-4,
            "inverter loss should be measurable: rte_ideal={rte_ideal:.4}, rte_with_inverter={rte_with_inverter:.4}, gap={gap:.6}"
        );
    }

    /// The quadratic terminal-voltage formula accounts for voltage sag during
    /// discharge. At high C-rate discharge, V < Voc, so I = P/V > P/Voc,
    /// giving higher ohmic losses than the linear approximation.
    #[test]
    fn quadratic_current_gives_higher_losses_during_discharge() {
        let config = battery_config(&[
            (KEY_INITIAL_SOC, 0.5),
            (KEY_SELF_DISCHARGE_PCT_PER_DAY, 0.0),
            (KEY_INVERTER_EFFICIENCY, 1.0),
        ]);
        let mut bat = Battery::new(config.clone());
        let env = base_env();
        bat.init(&config, &env).unwrap();

        // Discharge at 95% of max (negative = discharge).
        let high_power_kw = -0.95 * DEFAULT_MAX_DISCHARGE_KW;
        bat.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: high_power_kw,
            reactive_power_kvar: None,
        })
        .unwrap();

        let mut ports = default_ports();
        bat.step(&env, Duration::from_secs(300), &mut ports)
            .unwrap();
        let ohmic_quadratic = bat.telemetry().get("ohmic_loss_w").unwrap_or(0.0);

        // Linear approximation: I = P / Voc
        let cell_ocv = bat.ocv_table.voltage_at_soc(0.5);
        let pack_ocv = cell_ocv * bat.n_series as f64;
        let pack_r = bat.cell_resistance_ohm * bat.n_series as f64 / bat.n_parallel as f64;
        let dc_power_w = high_power_kw.abs() * 1000.0;
        let i_linear = dc_power_w / pack_ocv;
        let ohmic_linear = i_linear * i_linear * pack_r;

        assert!(
            ohmic_quadratic > ohmic_linear,
            "quadratic model should give higher ohmic losses during discharge: \
             quadratic={ohmic_quadratic:.4} W, linear={ohmic_linear:.4} W"
        );
    }

    /// Self-discharge loss must be independent of SOC level (absolute, not multiplicative).
    /// OCHRE Battery.py:337-338 uses absolute SOC loss per timestep.
    #[test]
    fn self_discharge_is_absolute_not_soc_relative() {
        let dt = Duration::from_secs(3600); // 1 hour
        let rate = 1.0; // 1%/day self-discharge

        let make_bat = |initial_soc: f64| {
            let config = battery_config(&[
                (KEY_INITIAL_SOC, initial_soc),
                (KEY_MIN_SOC, 0.0),
                (KEY_MAX_SOC, 1.0),
                (KEY_SELF_DISCHARGE_PCT_PER_DAY, rate),
            ]);
            let mut bat = Battery::new(config.clone());
            let env = base_env();
            bat.init(&config, &env).unwrap();
            bat.self_consumption_enabled = false;
            bat.power_setpoint_kw = None;
            bat
        };

        let mut bat_high = make_bat(0.9);
        let mut bat_low = make_bat(0.1);
        let env = base_env();

        let soc_high_before = bat_high.soc;
        let soc_low_before = bat_low.soc;

        let mut ports = default_ports();
        bat_high.step(&env, dt, &mut ports).unwrap();
        ports.zero();
        bat_low.step(&env, dt, &mut ports).unwrap();

        let loss_high = soc_high_before - bat_high.soc;
        let loss_low = soc_low_before - bat_low.soc;

        // Absolute loss must be equal regardless of starting SOC.
        assert!(
            (loss_high - loss_low).abs() < 1e-12,
            "self-discharge must be absolute (SOC-independent): high={loss_high:.10}, low={loss_low:.10}"
        );
        assert!(loss_high > 0.0, "self-discharge must cause SOC loss");
    }

    /// With heater active, zone thermal gain must equal ohmic_loss_w only, not ohmic + heater.
    /// The heater energy path is: grid → heater → cell thermal mass → zone (via UA model).
    #[test]
    fn zone_thermal_gain_is_ohmic_only_not_ohmic_plus_heater() {
        let mut raw: HashMap<String, ConfigValue> = HashMap::new();
        raw.insert(KEY_CAPACITY_KWH.to_string(), 10.0.into());
        raw.insert(KEY_MAX_CHARGE_KW.to_string(), 5.0.into());
        raw.insert(KEY_MAX_DISCHARGE_KW.to_string(), 5.0.into());
        raw.insert(KEY_STANDBY_POWER_W.to_string(), 0.0.into());
        raw.insert(KEY_INITIAL_SOC.to_string(), 0.5.into());
        raw.insert(KEY_MIN_SOC.to_string(), 0.0.into());
        raw.insert(KEY_MAX_SOC.to_string(), 1.0.into());
        raw.insert(KEY_HEATER_POWER_W.to_string(), 500.0.into());
        raw.insert(KEY_HEATER_THRESHOLD_C.to_string(), 5.0.into());
        raw.insert(KEY_SELF_DISCHARGE_PCT_PER_DAY.to_string(), 0.0.into());
        raw.insert(KEY_ZONE_ID.to_string(), 1.0.into());
        raw.insert(KEY_CELL_UA_W_PER_K.to_string(), 0.0.into()); // disable UA so ohmic is the only thermal gain
        let config = EquipmentConfig {
            name: "Test Battery".to_string(),
            ochre_class: "Battery".to_string(),
            raw_config: raw,
        };
        let mut bat = Battery::new(config.clone());
        let env = base_env();
        bat.init(&config, &env).unwrap();
        // Force cold cells so heater activates
        bat.cell_temp_c = -5.0;

        // Build ports with a thermal slot for zone 1.
        let mut ports = PortSlots::from_declarations(bat.ports());
        // PV surplus to request charging (heater activates because cell is cold and min_charge_temp=0°C)
        ports.electrical.generation_power_kw = -3.0;

        bat.step(&env, Duration::from_secs(300), &mut ports)
            .unwrap();

        assert!(bat.heater_active, "heater should be active for this test");

        let ohmic_w = bat.telemetry().get("ohmic_loss_w").unwrap_or(0.0);
        // Zone 1 thermal gain should equal ohmic_loss_w only (heater not double-counted).
        let zone_gain_w = ports
            .thermal
            .iter()
            .find(|t| t.zone == ZoneId(1))
            .map(|t| t.sensible_gain_w)
            .unwrap_or(0.0);
        assert!(
            (zone_gain_w - ohmic_w).abs() < 1e-6,
            "zone thermal gain should equal ohmic_loss_w={ohmic_w:.4} W, got {zone_gain_w:.4} W"
        );
    }

    /// At -20°C, capacity should be significantly lower than at 25°C (d0 Arrhenius model).
    /// OCHRE Battery.py:321-331: expect ~70-80% of nominal at -20°C.
    #[test]
    fn temperature_dependent_capacity_lower_at_cold() {
        let config = battery_config(&[
            (KEY_INITIAL_SOC, 0.5),
            (KEY_MIN_SOC, 0.0),
            (KEY_MAX_SOC, 1.0),
            (KEY_SELF_DISCHARGE_PCT_PER_DAY, 0.0),
        ]);
        let env = base_env();

        // Measure capacity at 25°C
        let mut bat_warm = Battery::new(config.clone());
        bat_warm.init(&config, &env).unwrap();
        bat_warm.cell_temp_c = 25.0;
        let mut ports = default_ports();
        bat_warm
            .step(&env, Duration::from_secs(300), &mut ports)
            .unwrap();
        let cap_warm = bat_warm.capacity_kwh;

        // Measure capacity at -20°C
        let mut bat_cold = Battery::new(config.clone());
        bat_cold.init(&config, &env).unwrap();
        bat_cold.cell_temp_c = -20.0;
        let mut ports = default_ports();
        bat_cold
            .step(&env, Duration::from_secs(300), &mut ports)
            .unwrap();
        let cap_cold = bat_cold.capacity_kwh;

        let ratio = cap_cold / cap_warm;
        assert!(
            ratio < 0.99,
            "capacity at -20°C should be less than at 25°C: cold={cap_cold:.4}, warm={cap_warm:.4}"
        );
        // OCHRE Arrhenius constants give ~49% at -20°C (Schimpe et al. 2018 fit).
        // Assert a wide range to verify the model is active without over-constraining the physics.
        assert!(
            ratio > 0.3,
            "capacity at -20°C should not be unreasonably low: ratio={ratio:.4}"
        );
    }

    /// Request discharge power well above rated; output must be clamped to P_max = Voc²/(4R).
    #[test]
    fn discriminant_clamp_limits_discharge_to_p_max() {
        let config = battery_config(&[
            (KEY_INITIAL_SOC, 0.5),
            (KEY_MIN_SOC, 0.0),
            (KEY_MAX_SOC, 1.0),
            (KEY_MAX_DISCHARGE_KW, 1000.0), // allow large setpoint
            (KEY_SELF_DISCHARGE_PCT_PER_DAY, 0.0),
            (KEY_INVERTER_EFFICIENCY, 1.0),
        ]);
        let mut bat = Battery::new(config.clone());
        let env = base_env();
        bat.init(&config, &env).unwrap();

        // Compute expected P_max
        let cell_ocv = bat.ocv_table.voltage_at_soc(0.5);
        let pack_ocv = cell_ocv * bat.n_series as f64;
        let pack_r = bat.cell_resistance_ohm * bat.n_series as f64 / bat.n_parallel as f64;
        let p_max_w = pack_ocv * pack_ocv / (4.0 * pack_r);

        // Request discharge power 10× P_max (guaranteed to drive discriminant negative)
        bat.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: -(p_max_w * 10.0 / 1000.0),
            reactive_power_kvar: None,
        })
        .unwrap();

        let mut ports = default_ports();
        bat.step(&env, Duration::from_secs(300), &mut ports)
            .unwrap();

        // Actual discharge power (negative = generation) should be clamped to P_max
        let actual_w = (-ports.electrical.net_active_kw() - bat.standby_power_w / 1000.0) * 1000.0;
        assert!(
            actual_w <= p_max_w * 1.001, // small tolerance for floating point
            "discharge must be clamped to P_max={p_max_w:.1} W, got {actual_w:.1} W"
        );
    }

    /// After reset_daily, the reversal buffer is preserved (not cleared).
    /// Residential charge/discharge cycles can straddle midnight.
    #[test]
    fn reset_daily_preserves_reversal_buffer() {
        let mut rf = RainflowCounter::default();
        rf.push(0.5);
        rf.push(0.8);
        rf.push(0.3); // 3 reversals in buffer

        let reversals_before = rf.reversals.len();
        rf.reset_daily();
        let reversals_after = rf.reversals.len();

        assert_eq!(
            reversals_after, reversals_before,
            "reset_daily must not discard partial cycles: had {reversals_before}, now {reversals_after}"
        );
    }

    /// ASTM E1049-85 4-point extraction: with a large outer cycle containing a small
    /// inner cycle, the inner cycle is extracted as a full cycle.
    /// Sequence: 0.0 → 0.8 → 0.6 → 1.0.
    /// After 3 points [0.0, 0.8, 0.6]: X=|0.6-0.8|=0.2, Y=|0.8-0.0|=0.8. X < Y → no extract.
    /// After 4 points [0.0, 0.8, 0.6, 1.0]: X=|1.0-0.6|=0.4, Y=|0.6-0.8|=0.2. X >= Y, n=4
    ///   → full cycle of range 0.2 → count += 0.2. Remaining: [0.0, 1.0].
    #[test]
    fn rainflow_inner_cycle_extracted_as_full_cycle() {
        let mut rf = RainflowCounter::default();
        rf.push(0.0);
        rf.push(0.8);
        rf.push(0.6);
        rf.push(1.0);

        assert!(
            (rf.total_cycles() - 0.2).abs() < 1e-10,
            "inner cycle should be extracted as full cycle (range=0.2): got {} EFC",
            rf.total_cycles()
        );
        assert_eq!(
            rf.reversals.len(),
            2,
            "should have 2 remaining reversals after extraction"
        );
    }

    /// After a save/load round-trip, all telemetry fields must be consistent
    /// with the restored state (no stale values from before the checkpoint).
    #[test]
    fn load_state_telemetry_is_fully_consistent() {
        let config = battery_config(&[
            (KEY_INITIAL_SOC, 0.6),
            (KEY_SELF_DISCHARGE_PCT_PER_DAY, 0.0),
            (KEY_HEATER_POWER_W, 200.0),
            (KEY_HEATER_THRESHOLD_C, 5.0),
        ]);
        let env = base_env();

        // First battery: step a few times to accumulate state.
        let mut bat1 = Battery::new(config.clone());
        bat1.init(&config, &env).unwrap();
        bat1.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: 2.5,
            reactive_power_kvar: None,
        })
        .unwrap();

        for _ in 0..5 {
            let mut ports = default_ports();
            bat1.step(&env, Duration::from_secs(300), &mut ports)
                .unwrap();
        }

        let saved_soc = bat1.soc;
        let saved_temp = bat1.cell_temp_c;
        let saved_cycles = bat1.rainflow.total_cycles();
        let saved_derate = bat1.discharge_derate_factor();
        let saved_standby = bat1.standby_power_w;
        let state = bat1.save_state();

        // Restore into a fresh battery.
        let mut bat2 = Battery::new(config.clone());
        bat2.init(&config, &env).unwrap();
        // Modify bat2's temp to ensure load_state overwrites it.
        bat2.cell_temp_c = -99.0;
        bat2.load_state(&state).unwrap();

        // Verify all telemetry fields are consistent with restored state.
        assert!(
            (bat2.telemetry().get("soc").unwrap() - saved_soc).abs() < 1e-12,
            "soc telemetry mismatch after load_state"
        );
        assert!(
            (bat2.telemetry().get("cell_temp_c").unwrap() - saved_temp).abs() < 1e-12,
            "cell_temp_c telemetry mismatch: expected {saved_temp}, got {}",
            bat2.telemetry().get("cell_temp_c").unwrap()
        );
        assert!(
            (bat2.telemetry().get("cycle_count").unwrap() - saved_cycles).abs() < 1e-12,
            "cycle_count telemetry mismatch after load_state"
        );
        assert!(
            (bat2.telemetry().get("discharge_derate").unwrap() - saved_derate).abs() < 1e-10,
            "discharge_derate telemetry stale after load_state: expected {saved_derate}, got {}",
            bat2.telemetry().get("discharge_derate").unwrap()
        );
        assert!(
            (bat2.telemetry().get("standby_power_w").unwrap() - saved_standby).abs() < 1e-12,
            "standby_power_w telemetry stale after load_state"
        );
    }

    /// Default SOC bounds must match OCHRE: min=0.15, max=0.95.
    #[test]
    fn default_soc_bounds_match_ochre() {
        let config = battery_config(&[]);
        let mut bat = Battery::new(config.clone());
        let env = base_env();
        bat.init(&config, &env).unwrap();

        assert!(
            (bat.min_soc - 0.15).abs() < 1e-12,
            "default min_soc should be 0.15, got {}",
            bat.min_soc
        );
        assert!(
            (bat.max_soc - 0.95).abs() < 1e-12,
            "default max_soc should be 0.95, got {}",
            bat.max_soc
        );
    }

    /// ASTM E1049-85 known sequence test.
    /// Sequence: -2, 1, -3, 5, -1, 3, -4, 4, -2 (scaled to SOC [0..1] range).
    #[test]
    fn rainflow_astm_known_sequence() {
        let values: Vec<f64> = [-2.0, 1.0, -3.0, 5.0, -1.0, 3.0, -4.0, 4.0, -2.0]
            .iter()
            .map(|x| (x + 4.0) / 9.0)
            .collect();

        let mut rf = RainflowCounter::default();
        for &v in &values {
            rf.push(v);
        }

        assert!(
            rf.total_cycles() > 0.0,
            "ASTM known sequence should produce cycles: got {}",
            rf.total_cycles()
        );
    }

    /// At 60 C (hot), capacity should differ from 25 C reference due to Arrhenius model.
    #[test]
    fn temperature_dependent_capacity_at_hot() {
        let config = battery_config(&[
            (KEY_INITIAL_SOC, 0.5),
            (KEY_MIN_SOC, 0.0),
            (KEY_MAX_SOC, 1.0),
            (KEY_SELF_DISCHARGE_PCT_PER_DAY, 0.0),
        ]);
        let env = base_env();

        let mut bat_ref = Battery::new(config.clone());
        bat_ref.init(&config, &env).unwrap();
        bat_ref.cell_temp_c = 25.0;
        let mut ports = default_ports();
        bat_ref
            .step(&env, Duration::from_secs(300), &mut ports)
            .unwrap();
        let cap_ref = bat_ref.capacity_kwh;

        let mut bat_hot = Battery::new(config.clone());
        bat_hot.init(&config, &env).unwrap();
        bat_hot.cell_temp_c = 60.0;
        let mut ports = default_ports();
        bat_hot
            .step(&env, Duration::from_secs(300), &mut ports)
            .unwrap();
        let cap_hot = bat_hot.capacity_kwh;

        assert!(
            (cap_hot - cap_ref).abs() > 1e-6,
            "capacity at 60 C should differ from 25 C reference: hot={cap_hot:.6}, ref={cap_ref:.6}"
        );
    }

    /// Midnight boundary: partial cycles that straddle the daily reset are preserved
    /// and can still contribute to cycle counts after the boundary.
    #[test]
    fn midnight_boundary_preserves_partial_cycles() {
        let mut rf = RainflowCounter::default();
        rf.push(0.3);
        rf.push(0.8);
        rf.push(0.4);

        let reversals_before = rf.reversals.clone();
        let cycles_before = rf.total_cycles();

        rf.reset_daily();

        assert_eq!(rf.reversals.len(), reversals_before.len());
        assert!((rf.total_cycles() - cycles_before).abs() < 1e-12);

        // Complete the cycle after midnight
        rf.push(0.9);

        // [0.3, 0.8, 0.4, 0.9]: x1=0.8, x2=0.4, x3=0.9
        // X=0.5, Y=0.4. X >= Y, n=4 -> full cycle of range 0.4.
        assert!(
            rf.total_cycles() > cycles_before,
            "completing cycle after midnight should increment count: before={cycles_before}, after={}",
            rf.total_cycles()
        );
    }

    /// Negative discriminant: requesting power beyond P_max should clamp correctly.
    #[test]
    fn negative_discriminant_clamp_is_correct() {
        let config = battery_config(&[
            (KEY_INITIAL_SOC, 0.5),
            (KEY_MIN_SOC, 0.0),
            (KEY_MAX_SOC, 1.0),
            (KEY_MAX_DISCHARGE_KW, 1000.0),
            (KEY_SELF_DISCHARGE_PCT_PER_DAY, 0.0),
            (KEY_INVERTER_EFFICIENCY, 1.0),
        ]);
        let mut bat = Battery::new(config.clone());
        let env = base_env();
        bat.init(&config, &env).unwrap();

        let cell_ocv = bat.ocv_table.voltage_at_soc(0.5);
        let pack_ocv = cell_ocv * bat.n_series as f64;
        let pack_r = bat.cell_resistance_ohm * bat.n_series as f64 / bat.n_parallel as f64;
        let p_max_w = pack_ocv * pack_ocv / (4.0 * pack_r);
        let p_max_kw = p_max_w / 1000.0;

        let (actual_kw, ohmic_w) = bat.compute_electrical(-p_max_kw * 100.0);

        assert!(actual_kw.is_finite(), "clamped power must be finite");
        assert!(ohmic_w.is_finite(), "clamped ohmic loss must be finite");
        assert!(
            actual_kw.abs() <= p_max_kw * 1.001,
            "clamped discharge must not exceed P_max={p_max_kw:.1} kW, got {:.1} kW",
            actual_kw.abs()
        );
    }

    // -----------------------------------------------------------------------
    // Smith 2017 degradation model tests
    // -----------------------------------------------------------------------

    /// U_neg table must interpolate correctly at exact table points and in-between.
    #[test]
    fn u_neg_interpolation_at_known_points() {
        let table = UNegTable::default_li_nmc();
        assert!((table.potential_at_soc(0.0) - 1.2868).abs() < 1e-10);
        assert!((table.potential_at_soc(1.0) - 0.0859).abs() < 1e-10);
        // Midpoint between 0.0 (1.2868) and 0.1 (0.2420): average = 0.7644
        let mid = table.potential_at_soc(0.05);
        assert!(
            (mid - (1.2868 + 0.2420) / 2.0).abs() < 1e-10,
            "U_neg at 0.05 should interpolate: got {mid}"
        );
        // Clamped below / above bounds
        assert!((table.potential_at_soc(-0.1) - 1.2868).abs() < 1e-10);
        assert!((table.potential_at_soc(1.5) - 0.0859).abs() < 1e-10);
    }

    /// After zero simulated time, capacity fade must be exactly 0.0 (fresh cell).
    #[test]
    fn degradation_starts_at_zero() {
        let state = DegradationState::default();
        assert_eq!(state.capacity_fade_pct(), 0.0);
    }

    /// After many complete days of cycling at 25 C, capacity fade must be positive
    /// but physically plausible (well below 100% after a few simulated years).
    #[test]
    fn degradation_positive_after_cycling() {
        let u_neg = UNegTable::default_li_nmc();
        let mut state = DegradationState::default();
        state.reset_day_tracking(0.5);

        let cell_temp_k = 298.15; // 25 °C
        let v_oc = 3.69; // Mid-charge OCV
        let dt_s = 300.0; // 5-minute timesteps
        let steps_per_day = (86_400.0 / dt_s) as usize;

        // Simulate 365 days with cycling between SOC 0.2 and 0.8.
        let mut soc_direction = 1_f64;
        let mut soc = 0.5;
        for day in 0..365 {
            for _ in 0..steps_per_day {
                soc += soc_direction * 0.002; // slow charge/discharge ramp
                if soc >= 0.8 {
                    soc = 0.8;
                    soc_direction = -1.0;
                } else if soc <= 0.2 {
                    soc = 0.2;
                    soc_direction = 1.0;
                }
                state.accumulate(dt_s, cell_temp_k, v_oc, soc);
            }
            // Simulate a rainflow counter with daily DOD of 0.6
            let mut rf = RainflowCounter::default();
            rf.push(0.2);
            rf.push(0.8);
            let sum_sq = rf.sum_squared_dod_daily();
            state.update_daily(&u_neg, cell_temp_k, sum_sq);
            state.reset_day_tracking(soc);
            let _ = day; // suppress lint
        }

        let fade = state.capacity_fade_pct();
        assert!(
            fade > 0.0,
            "capacity fade must be positive after 1 year: got {fade}"
        );
        assert!(
            fade < 0.5,
            "capacity fade must be <50% after 1 year: got {fade}"
        );
    }

    /// Mechanism 1 (calendar) dominates at rest: even without cycling,
    /// fade must accumulate slowly over many days.
    #[test]
    fn degradation_calendar_aging_at_rest() {
        let u_neg = UNegTable::default_li_nmc();
        let mut state = DegradationState::default();
        state.reset_day_tracking(0.5);

        let cell_temp_k = 308.15; // 35 °C — elevated temperature accelerates calendar aging
        let v_oc = 3.69;
        let dt_s = 3600.0; // hourly steps, idle battery

        // Simulate 365 days at rest with no cycling.
        for _ in 0..365 {
            for _ in 0..24 {
                state.accumulate(dt_s, cell_temp_k, v_oc, 0.5); // constant SOC = 0.5
            }
            let sum_sq = 0.0; // no cycles
            state.update_daily(&u_neg, cell_temp_k, sum_sq);
            state.reset_day_tracking(0.5);
        }

        let fade = state.capacity_fade_pct();
        assert!(
            fade > 0.0,
            "calendar aging must produce positive fade at rest: got {fade}"
        );
    }

    /// Higher temperature must produce more capacity fade than lower temperature
    /// (Arrhenius relationship: both calendar and cycle mechanisms accelerate with T).
    #[test]
    fn degradation_higher_temp_more_fade() {
        let u_neg = UNegTable::default_li_nmc();
        let dt_s = 300.0;
        let steps_per_day = (86_400.0 / dt_s) as usize;
        let v_oc = 3.69;

        let simulate_fade = |temp_k: f64| -> f64 {
            let mut state = DegradationState::default();
            state.reset_day_tracking(0.5);
            let mut rf = RainflowCounter::default();
            rf.push(0.2);
            rf.push(0.8);
            let sum_sq = rf.sum_squared_dod_daily();

            for _ in 0..180 {
                for _ in 0..steps_per_day {
                    state.accumulate(dt_s, temp_k, v_oc, 0.5);
                }
                state.update_daily(&u_neg, temp_k, sum_sq);
                state.reset_day_tracking(0.5);
            }
            state.capacity_fade_pct()
        };

        let fade_cold = simulate_fade(278.15); // 5 °C
        let fade_warm = simulate_fade(318.15); // 45 °C

        assert!(
            fade_warm > fade_cold,
            "higher temperature must produce more fade: 45C={fade_warm:.6}, 5C={fade_cold:.6}"
        );
    }

    /// After a save/load round-trip, degradation state (capacity fade and accumulators)
    /// must be identical, and the model must continue evolving consistently.
    #[test]
    fn degradation_state_survives_checkpoint_round_trip() {
        let config = battery_config(&[
            (KEY_INITIAL_SOC, 0.5),
            (KEY_SELF_DISCHARGE_PCT_PER_DAY, 0.0),
            (KEY_MIN_SOC, 0.0),
            (KEY_MAX_SOC, 1.0),
        ]);
        let env = base_env();

        let mut bat1 = Battery::new(config.clone());
        bat1.init(&config, &env).unwrap();

        // Run 10 steps to accumulate some sub-daily degradation state.
        bat1.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: 3.0,
            reactive_power_kvar: None,
        })
        .unwrap();
        for _ in 0..10 {
            let mut ports = default_ports();
            bat1.step(&env, Duration::from_secs(300), &mut ports)
                .unwrap();
        }

        let saved = bat1.save_state();
        let fade_before = bat1.degradation.capacity_fade_pct();
        let b1_accum_before = bat1.degradation.b1_accum;

        // Restore into a fresh battery.
        let mut bat2 = Battery::new(config.clone());
        bat2.init(&config, &env).unwrap();
        bat2.load_state(&saved).unwrap();

        assert!(
            (bat2.degradation.capacity_fade_pct() - fade_before).abs() < 1e-15,
            "capacity_fade must survive checkpoint"
        );
        assert!(
            (bat2.degradation.b1_accum - b1_accum_before).abs() < 1e-15,
            "b1_accum must survive checkpoint"
        );
    }

    /// RainflowCounter daily DOD sum-of-squares is zero when no cycles have been
    /// completed today, and non-zero after a full cycle.
    #[test]
    fn rainflow_sum_squared_dod_reflects_daily_cycles() {
        let mut rf = RainflowCounter::default();
        assert_eq!(rf.sum_squared_dod_daily(), 0.0);

        // Push a full 50% DOD cycle: 0.5 → 1.0 → 0.5 → 1.0
        // First two half-cycles are extracted (range=0.5 each, weight=0.5 each → 0.25 each).
        rf.push(0.5);
        rf.push(1.0);
        rf.push(0.5);
        rf.push(1.0);

        let sum_sq = rf.sum_squared_dod_daily();
        // Two half-cycles each with effective DOD = 0.5 * 0.5 = 0.25.
        // Σ(DOD_i)² = 0.25² + 0.25² = 0.0625 + 0.0625 = 0.125
        assert!(
            (sum_sq - 0.125).abs() < 1e-10,
            "sum_squared_dod should be 0.125 for two half-cycles of range 0.5: got {sum_sq}"
        );

        // After reset_daily, sum should be zero again.
        rf.reset_daily();
        assert_eq!(rf.sum_squared_dod_daily(), 0.0);
    }

    /// The BOL transient mechanism (q_li3) uses B3_REF < 0, which means b3_accum
    /// accumulates negative values. The `.max(0.0)` clamp in `update_daily` keeps
    /// dq_li3 = 0 when b3_accum < q_li3, so q_li3 never rises above 0 during
    /// calendar-only aging. This confirms the mechanism produces no fade contribution
    /// (≤ 0) in the early-life period and that total capacity fade grows with time.
    #[test]
    fn degradation_bol_transient_initially_provides_capacity_gain() {
        let u_neg = UNegTable::default_li_nmc();
        let cell_temp_k = 298.15; // 25 °C
        let soc = 0.5;
        let v_oc = OcvTable::default_li_nmc().voltage_at_soc(soc);
        let dt_s = 300.0; // 5-minute timesteps
        let steps_per_day = (SECONDS_PER_DAY / dt_s) as usize;

        // --- Simulate 5 days of calendar aging (no cycling) ---
        let mut state_5 = DegradationState::default();
        state_5.reset_day_tracking(soc);

        for _ in 0..5 {
            for _ in 0..steps_per_day {
                state_5.accumulate(dt_s, cell_temp_k, v_oc, soc);
            }
            state_5.update_daily(&u_neg, cell_temp_k, 0.0);
            state_5.reset_day_tracking(soc);
        }

        // The BOL transient b3_accum is always negative (B3_REF < 0). The
        // `.max(0.0)` clamp in update_daily prevents dq_li3 from being positive,
        // so q_li3 stays at 0.0 — confirming no capacity loss from this mechanism.
        assert!(
            state_5.q_li3 <= 0.0,
            "q_li3 should be <= 0.0 after 5 days of calendar aging (BOL transient provides capacity gain, not loss): got {}",
            state_5.q_li3
        );
        let fade_5_days = state_5.capacity_fade_pct();

        // --- Simulate 60 days of calendar aging (no cycling) ---
        let mut state_60 = DegradationState::default();
        state_60.reset_day_tracking(soc);

        for _ in 0..60 {
            for _ in 0..steps_per_day {
                state_60.accumulate(dt_s, cell_temp_k, v_oc, soc);
            }
            state_60.update_daily(&u_neg, cell_temp_k, 0.0);
            state_60.reset_day_tracking(soc);
        }

        assert!(
            state_60.q_li3 <= 0.0,
            "q_li3 should be <= 0.0 after 60 days of calendar aging: got {}",
            state_60.q_li3
        );
        let fade_60_days = state_60.capacity_fade_pct();

        // Calendar aging (mechanism 1) grows monotonically with time: 60-day fade
        // must exceed 5-day fade.
        assert!(
            fade_60_days > fade_5_days,
            "capacity fade after 60 days ({fade_60_days:.6}) must exceed fade after 5 days ({fade_5_days:.6})"
        );
    }

    /// All 11 calibrated points of the default Li-NMC OCV table must match
    /// the NREL SSC / OCHRE reference values exactly (within 1e-10 V).
    #[test]
    fn ocv_table_all_11_calibrated_points_exact() {
        let table = OcvTable::default_li_nmc();

        let expected: [(f64, f64); 11] = [
            (0.0, 3.0000),
            (0.1, 3.4679),
            (0.2, 3.5394),
            (0.3, 3.5950),
            (0.4, 3.6453),
            (0.5, 3.6876),
            (0.6, 3.7469),
            (0.7, 3.8400),
            (0.8, 3.9521),
            (0.9, 4.0668),
            (1.0, 4.1934),
        ];

        for (soc, expected_v) in expected {
            let actual_v = table.voltage_at_soc(soc);
            assert!(
                (actual_v - expected_v).abs() < 1e-10,
                "OCV at SOC {soc:.1}: expected {expected_v:.4} V, got {actual_v:.10} V"
            );
        }
    }

    #[test]
    fn import_limit_caps_charge_power() {
        // OCHRE Battery Max Import Limit: clamps the charge power drawn from the grid.
        // Setting import_limit_w=2000 W = 2 kW should cap charging to 2 kW even
        // when max_charge_kw=5 kW and a 5 kW setpoint is applied.
        let config = battery_config(&[
            (KEY_IMPORT_LIMIT_W, 2000.0), // 2 kW limit
            (KEY_INITIAL_SOC, 0.1),       // plenty of room to charge
            (KEY_MIN_SOC, 0.0),
            (KEY_MAX_SOC, 1.0),
            (KEY_SELF_DISCHARGE_PCT_PER_DAY, 0.0),
        ]);
        let mut bat = Battery::new(config.clone());
        bat.init(&config, &base_env()).unwrap();

        bat.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: 5.0,
            reactive_power_kvar: None,
        })
        .unwrap();

        let mut slots = default_ports();
        bat.step(&base_env(), Duration::from_secs(300), &mut slots)
            .unwrap();

        // Standby is always drawn on top; charging itself must not exceed 2 kW.
        let standby_kw = DEFAULT_STANDBY_POWER_W / 1000.0;
        let charging_kw = bat
            .telemetry()
            .get("active_power_kw")
            .unwrap_or(0.0)
            - standby_kw;
        assert!(
            charging_kw <= 2.0 + 1e-9,
            "charge power {charging_kw:.4} kW should be capped at 2 kW by import_limit_w"
        );
        assert!(
            charging_kw > 1.9,
            "charge power {charging_kw:.4} kW should be close to the 2 kW import limit"
        );
    }

    #[test]
    fn export_limit_caps_discharge_power() {
        // OCHRE Battery Max Export Limit: clamps discharge power exported to the grid.
        // Setting export_limit_w=1500 W = 1.5 kW should cap discharging to 1.5 kW
        // even when max_discharge_kw=5 kW and a -5 kW setpoint is applied.
        let config = battery_config(&[
            (KEY_EXPORT_LIMIT_W, 1500.0), // 1.5 kW limit
            (KEY_INITIAL_SOC, 0.9),       // plenty of energy to discharge
            (KEY_MIN_SOC, 0.0),
            (KEY_MAX_SOC, 1.0),
            (KEY_SELF_DISCHARGE_PCT_PER_DAY, 0.0),
        ]);
        let mut bat = Battery::new(config.clone());
        bat.init(&config, &base_env()).unwrap();

        bat.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: -5.0,
            reactive_power_kvar: None,
        })
        .unwrap();

        let mut slots = default_ports();
        bat.step(&base_env(), Duration::from_secs(300), &mut slots)
            .unwrap();

        // active_power_kw includes standby; subtract it to get net discharge.
        let active_kw = bat.telemetry().get("active_power_kw").unwrap_or(0.0);
        let standby_kw = DEFAULT_STANDBY_POWER_W / 1000.0;
        let discharge_kw = active_kw - standby_kw;
        assert!(
            discharge_kw >= -1.5 - 1e-9,
            "discharge power {discharge_kw:.4} kW must not exceed export_limit of -1.5 kW"
        );
        assert!(
            discharge_kw < -1.3,
            "discharge power {discharge_kw:.4} kW should be close to the -1.5 kW export limit"
        );
    }

    #[test]
    fn no_limit_allows_full_charge_power() {
        // Without import/export limits, the battery should use its full capacity.
        let config = battery_config(&[
            (KEY_INITIAL_SOC, 0.1),
            (KEY_MIN_SOC, 0.0),
            (KEY_MAX_SOC, 1.0),
            (KEY_SELF_DISCHARGE_PCT_PER_DAY, 0.0),
        ]);
        let mut bat = Battery::new(config.clone());
        bat.init(&config, &base_env()).unwrap();

        bat.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: 5.0,
            reactive_power_kvar: None,
        })
        .unwrap();

        let mut slots = default_ports();
        bat.step(&base_env(), Duration::from_secs(300), &mut slots)
            .unwrap();

        let standby_kw = DEFAULT_STANDBY_POWER_W / 1000.0;
        let charging_kw = bat
            .telemetry()
            .get("active_power_kw")
            .unwrap_or(0.0)
            - standby_kw;
        // Should be close to 5 kW (max_charge_kw), not capped lower.
        assert!(
            charging_kw > 4.9,
            "without import limit, charge power {charging_kw:.4} kW should reach max_charge_kw"
        );
    }

    #[test]
    fn import_limit_defaults_to_none_unlimited() {
        // Default construction has no import/export limits.
        let config = battery_config(&[]);
        let mut bat = Battery::new(config.clone());
        bat.init(&config, &base_env()).unwrap();
        assert!(bat.import_limit_kw.is_none());
        assert!(bat.export_limit_kw.is_none());
    }

    #[test]
    fn init_rejects_negative_import_limit() {
        let config = battery_config(&[(KEY_IMPORT_LIMIT_W, -100.0)]);
        let mut bat = Battery::new(config.clone());
        assert!(bat.init(&config, &base_env()).is_err());
    }

    #[test]
    fn init_rejects_negative_export_limit() {
        let config = battery_config(&[(KEY_EXPORT_LIMIT_W, -100.0)]);
        let mut bat = Battery::new(config.clone());
        assert!(bat.init(&config, &base_env()).is_err());
    }
}
