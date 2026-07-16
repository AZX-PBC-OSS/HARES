//! Core HVAC equipment wrapper with thermostat state, step logic, and helpers.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use chrono::Duration as ChronoDuration;
use hares_physics::biquadratic::BiquadraticCurve;
use hares_physics::constants::CFM_PER_M3_S;
use hares_types::{ControlSignal, EnvironmentState, HaresError, ScheduleSource, ZoneId};

use crate::EquipmentConfig;

use super::core_config::{
    build_setpoint_source, extract_bool, extract_numeric, extract_text, load_biquadratic_coeffs,
    load_bounds_pair, load_plr_coefficients, parse_biquadratic_list, parse_f64_array_3,
    parse_speed_control_mode,
};
use super::default_curves::{BiquadraticCurveSource, maybe_substitute_defaults};
use super::helpers::validate_zone_id;
use super::speed_control::{SpeedControlMode, StartupConfig};
use super::staging::{DEFAULT_LOW_SPEED_CAPACITY_FRACTION, DEFAULT_PLF_DEGRADATION_COEFF};
use super::thermostat::{
    ScheduleSetpoints, ThermalSetpoints, ThermostatConfig, ThermostatFsm, ThermostatMode,
    lookup_zone_temp,
};

pub const IDEAL_CAPACITY_TIME_RES_THRESHOLD_S: i64 = 300;
pub(super) const DEFAULT_BIQUADRATIC_COEFFS: [f64; 6] = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];

/// Default fan power: 0.365 W/CFM.
/// Source: ANSI/RESNET/ICC 301-2019 §4.2.2(1) Table 4.2.2(1),
/// "Default Heating and Cooling Systems" -- supply fan power for
/// forced-air systems.
const DEFAULT_FAN_POWER_W_PER_CFM: f64 = 0.365;
const DEFAULT_FAN_POWER_W_PER_M3_S: f64 = DEFAULT_FAN_POWER_W_PER_CFM * CFM_PER_M3_S;

/// Default outdoor temperature for supply-air initialization [C] (47 F).
/// AHRI 210/240 H1 heating test condition.
const DEFAULT_INIT_OUTDOOR_TEMP_C: f64 = 8.3;

/// Default thermostat cutout ratio when not specified in config.
const DEFAULT_CUTOUT_RATIO: f64 = 0.25;

/// Default minimum on/off cycle lockout [s] when not specified in config.
const DEFAULT_MIN_CYCLE_TIME_S: f64 = 60.0;

/// Default biquadratic x1 (indoor) input bounds [°C].
/// DB-based curves (heating): covers indoor DB −10 to +50°C.
/// WB-based curves (cooling): caller should supply tighter per-curve bounds
/// (AHRI 210/240-2023: indoor WB 19.4–26.7°C).
/// Fallback only; per-curve explicit bounds from equipment CSV/specs must be preferred.
/// AHRI 210/240-2023 rating envelopes plus generous margin.
const DEFAULT_BIQUADRATIC_X1_BOUNDS: (f64, f64) = (-10.0, 50.0);
/// Default biquadratic x2 (outdoor DB) input bounds [°C].
/// Covers global residential outdoor conditions: −50°C (polar extreme) to
/// +60°C (desert extreme). EnergyPlus I/O Reference (Curve:Biquadratic) expects
/// bounded min/max fields per axis; ±100°C is not physically meaningful for
/// residential HVAC and allows unconstrained polynomial extrapolation.
/// Fallback only; per-curve explicit bounds from equipment CSV/specs must be preferred.
const DEFAULT_BIQUADRATIC_X2_BOUNDS: (f64, f64) = (-50.0, 60.0);

/// Maximum safe zone temperature [°C] for conditioned zones with active heating.
///
/// Above this threshold, heating equipment must be forced Off to prevent
/// simulation runaway where zone temperatures reach physically impossible levels
/// (e.g. 49.5 °C indoors in January). Attics and garages can legitimately reach
/// higher temperatures, but heating equipment's served zone (`config.zone_id`)
/// is always a conditioned space.
pub(crate) const MAX_CONDITIONED_ZONE_TEMP_C: f64 = 35.0;

/// Maximum number of compressor speed stages supported by any HVAC equipment type.
/// Covers 4-stage central AC/ASHP and 8-stage MSHP (the highest stage count in
/// the OCHRE/HARES default performance curve set). Used for stack-allocated
/// fixed-size arrays in `HvacControlState::disabled_speeds` to eliminate
/// per-timestep heap allocation on the control-signal hot path.
pub const MAX_SPEEDS: usize = 8;

/// Equipment category for HVAC defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HvacEquipmentType {
    GasFurnace,
    ElectricFurnace,
    AshpHeatPumpOnly,
    AshpHeatPumpAux,
    MiniSplitHeat,
    /// Central AC or ASHP cooling coil. Uses the central-cooling airflow baseline.
    AcCooler,
    /// MSHP cooling coil. Uses the ductless cooling airflow baseline; Cd=0 (no cycling penalty).
    MiniSplitCool,
    /// Ground-source heat pump heating coil. Source-side temperature is entering water/ground
    /// temperature rather than outdoor air. No defrost. Supply air temp follows ASHP HP-only profile
    /// (the coil operates at similar discharge temperatures regardless of source medium).
    GshpHeatPumpHeating,
    /// Ground-source heat pump cooling coil. Source-side temperature is entering water/ground
    /// temperature. No crankcase heater (compressor is indoors). Uses central-cooling airflow baseline.
    GshpHeatPumpCooling,
    /// Water-source heat pump heating coil. Source-side temperature is entering water from a
    /// water loop (boiler/condenser loop) or groundwater/surface water. No defrost.
    /// Supply air temp follows ASHP HP-only profile: the water-to-air coil produces similar
    /// discharge temperatures to air-source DX regardless of source medium. ASHRAE HoF 2021
    /// Ch. 34 (Geothermal Energy): water-to-air heat pumps typically deliver 32-49°C supply air.
    WshpHeatPumpHeating,
    /// Water-source heat pump cooling coil. Source-side temperature is entering water from a
    /// water loop or groundwater. No crankcase heater (compressor is indoors). Uses central-cooling
    /// airflow baseline.
    WshpHeatPumpCooling,
    Baseboard,
    Other,
}

/// Default airflow in SI units [m3/s/W] by equipment category.
///
/// Source provenance:
/// - Central AC/ASHP cooling baseline corresponds to RESNET HERS Addendum 82
///   and OpenStudio-HPXML residential defaults.
/// - MSHP cooling baseline follows ductless split assumptions used in OCHRE.
/// - Heating-side baseline follows OCHRE/ResStock residential conventions.
///
/// Stored in SI only for internal physics/state updates.
pub const AIRFLOW_HEATING_M3_S_PER_W: f64 = 4.696_858_666_481_194_7e-5;
pub const AIRFLOW_CENTRAL_AC_M3_S_PER_W: f64 = 5.367_838_475_978_508_5e-5;
pub const AIRFLOW_MSHP_COOLING_M3_S_PER_W: f64 = 4.186_914_011_263_237e-5;
pub const AIRFLOW_ROOM_AC_M3_S_PER_W: f64 = 4.294_270_780_782_807e-5;

/// EnergyPlus auto-sizing SHR coefficient [W·s/m³].
///
/// EnergyPlus `CoolingSHRSizing.cc:87–93`:
///   SHR = 0.431 + 6086.0 × (VolFlow / Capacity)
/// where VolFlow is rated volumetric airflow in m³/s and Capacity is
/// rated net cooling capacity in W. With VolFlow = airflow_m3_s_per_w ×
/// Capacity the expression simplifies to SHR = 0.431 + 6086.0 ×
/// airflow_m3_s_per_w. At 320 CFM/ton (room AC) this yields SHR ≈ 0.69;
/// at 400 CFM/ton (central AC) it yields SHR ≈ 0.76.
pub const SHR_AUTOSIZE_COEFFICIENT: f64 = 6086.0;

/// Default SHR for room AC equipment (≈320 CFM/ton).
///
/// Fallback used when airflow is unavailable at init.
/// Derived from EnergyPlus `CoolingSHRSizing.cc:87–93` at 320 CFM/ton.
pub const DEFAULT_SHR_ROOM_AC: f64 = 0.70;

/// Default SHR for central AC equipment (≈400 CFM/ton).
///
/// Fallback used when airflow is unavailable at init.
/// Derived from EnergyPlus `CoolingSHRSizing.cc:87–93` at 400 CFM/ton.
pub const DEFAULT_SHR_CENTRAL_AC: f64 = 0.75;

/// Derives default SHR from equipment airflow using EnergyPlus auto-sizing.
///
/// EnergyPlus `CoolingSHRSizing.cc:87–93`:
///   SHR = 0.431 + 6086.0 × (VolFlow / Capacity)
///
/// With VolFlow = airflow_m3_s_per_w × capacity_w, the formula simplifies:
///   SHR = 0.431 + 6086.0 × airflow_m3_s_per_w
///
/// Result is clamped to [0.5, 1.0].
///
/// When airflow is not available or non-positive, falls back to
/// hardcoded defaults: 0.70 for room AC, 0.75 for central AC.
pub fn derive_default_shr(
    airflow_m3_s_per_w: Option<f64>,
    capacity_w: Option<f64>,
    is_room_ac: bool,
) -> f64 {
    let shr = if let (Some(af), Some(_cap)) = (airflow_m3_s_per_w.filter(|&a| a > 0.0), capacity_w)
    {
        // EnergyPlus CoolingSHRSizing.cc:87–93.
        0.431 + SHR_AUTOSIZE_COEFFICIENT * af
    } else if is_room_ac {
        DEFAULT_SHR_ROOM_AC
    } else {
        DEFAULT_SHR_CENTRAL_AC
    };
    shr.clamp(0.5, 1.0)
}

/// Refrigerant charge defect capacity correction coefficient (Cc).
///
/// Capacity multiplier = 1.0 + CC_CHARGE_DEFECT * r
/// where r = (InstalledCharge - DesignCharge) / DesignCharge.
///
/// CC_CHARGE_DEFECT = +0.9 so that undercharge (r < 0) reduces capacity:
///   r = −0.10 → 1.0 + 0.9 × (−0.10) = 0.91 ✓ (capacity falls 9%)
///
/// Source: ANSI/RESNET/ACCA 310-2020 (paywalled; magnitude unverified from
/// public sources — the standard is not publicly accessible for direct citation).
/// Sign and approximate magnitude are consistent with OpenStudio-HPXML (NREL)
/// hvac_sizing.rb: `cool_qgr_values[0] = −9.46e-1` drives a ~0.9× capacity
/// multiplier at rated conditions for a 10% charge defect.
pub const CC_CHARGE_DEFECT: f64 = 0.9;

/// Refrigerant charge defect EIR correction coefficient (Ce).
///
/// EIR multiplier = 1.0 + CE_CHARGE_DEFECT * r
/// where r = (InstalledCharge - DesignCharge) / DesignCharge.
///
/// For cooling DX coils: ANSI/RESNET/ACCA 310-2020.
///
/// Ce is **negative** so that undercharge (r < 0) raises EIR (degrades efficiency):
///   r = −0.10 → 1.0 + (−0.9)(−0.10) = 1.09 — EIR increases 9%, COP falls 9%.
///
/// Physical motivation: an undercharged system moves less refrigerant, the
/// compressor works harder per unit of cooling, and COP degrades.
/// This is consistent with the OpenStudio-HPXML simulation EMS model
/// (NREL hvac.rb `add_installation_quality_ems_program_equations`) where
/// `Y_CH_COP = Y_CH_Q / (1 + P_terms * F_CH)` and the p_values produce a
/// net power correction such that COP falls for undercharge, meaning EIR rises.
///
/// The verification audit was unable to confirm the exact magnitude from
/// publicly available sources of ANSI/RESNET/ACCA 310-2020 (paywalled).
/// The magnitude 0.9 is a linearization of the OS-HPXML polynomial at rated
/// conditions; the sign (negative) is physically and numerically motivated.
pub const CE_CHARGE_DEFECT: f64 = -0.9;

/// Apply refrigerant charge defect correction to rated capacity and EIR values.
///
/// For a charge defect ratio r = (InstalledCharge − DesignCharge) / DesignCharge:
///
/// - Capacity multiplier = 1.0 + CC_CHARGE_DEFECT × r  (CC = +0.9)
///   Undercharge (r < 0) → multiplier < 1.0: capacity falls. ✓
/// - EIR multiplier     = 1.0 + CE_CHARGE_DEFECT × r  (CE = −0.9)
///   Undercharge (r < 0) → multiplier > 1.0: EIR rises, efficiency degrades. ✓
///
/// Example r = −0.10: capacity × 0.91, EIR × 1.09.
/// Example r =  0.0:  no correction.
pub fn apply_charge_defect_correction(
    capacities: &mut [f64],
    eirs: &mut [f64],
    charge_defect_ratio: f64,
) {
    let cap_corr = 1.0 + CC_CHARGE_DEFECT * charge_defect_ratio;
    let eir_corr = 1.0 + CE_CHARGE_DEFECT * charge_defect_ratio;
    for cap in capacities.iter_mut() {
        *cap *= cap_corr;
    }
    for eir in eirs.iter_mut() {
        *eir *= eir_corr;
    }
}

impl HvacEquipmentType {
    pub fn default_supply_air_temp_c(self, outdoor_temp_c: f64) -> f64 {
        match self {
            Self::GasFurnace => 54.4,
            Self::ElectricFurnace => 48.9,
            Self::AshpHeatPumpOnly => 32.2 + 0.15 * (outdoor_temp_c - 8.3),
            Self::AshpHeatPumpAux => 40.6,
            Self::MiniSplitHeat => 43.3,
            Self::AcCooler | Self::MiniSplitCool => 40.6,
            // GSHP supply air temp follows ASHP HP-only profile: the water-to-air coil
            // produces similar discharge temperatures to air-source DX regardless of
            // source medium. ASHRAE HoF 2021 Ch. 34 (Geothermal Energy): water-to-air
            // heat pumps typically deliver 32–49 °C supply air.
            Self::GshpHeatPumpHeating | Self::WshpHeatPumpHeating => {
                32.2 + 0.15 * (outdoor_temp_c - 8.3)
            }
            Self::GshpHeatPumpCooling | Self::WshpHeatPumpCooling => 40.6,
            Self::Baseboard => 0.0,
            Self::Other => 40.6,
        }
    }

    pub fn default_airflow_m3_s_per_w(self) -> f64 {
        match self {
            Self::AcCooler | Self::GshpHeatPumpCooling | Self::WshpHeatPumpCooling => {
                AIRFLOW_CENTRAL_AC_M3_S_PER_W
            }
            Self::MiniSplitCool => AIRFLOW_MSHP_COOLING_M3_S_PER_W,
            Self::GasFurnace
            | Self::ElectricFurnace
            | Self::AshpHeatPumpOnly
            | Self::AshpHeatPumpAux
            | Self::MiniSplitHeat
            | Self::GshpHeatPumpHeating
            | Self::WshpHeatPumpHeating
            | Self::Baseboard
            | Self::Other => AIRFLOW_HEATING_M3_S_PER_W,
        }
    }

    /// True for heating-side equipment types.
    pub fn is_heating(self) -> bool {
        matches!(
            self,
            Self::GasFurnace
                | Self::ElectricFurnace
                | Self::AshpHeatPumpOnly
                | Self::AshpHeatPumpAux
                | Self::MiniSplitHeat
                | Self::GshpHeatPumpHeating
                | Self::WshpHeatPumpHeating
                | Self::Baseboard
        )
    }
}

/// Per-equipment state for biquadratic curve-index clamping diagnostics.
///
/// Wraps atomic counters so `HvacConfig` can derive `Clone`/`PartialEq`/`Debug`
/// while supporting lock-free per-timestep increment and read-and-reset.
#[derive(Debug)]
pub struct ClampState {
    was_clamped: AtomicBool,
    clamp_count: AtomicU64,
}

impl Clone for ClampState {
    fn clone(&self) -> Self {
        Self {
            was_clamped: AtomicBool::new(self.was_clamped.load(Ordering::Relaxed)),
            clamp_count: AtomicU64::new(self.clamp_count.load(Ordering::Relaxed)),
        }
    }
}

impl PartialEq for ClampState {
    fn eq(&self, other: &Self) -> bool {
        self.was_clamped.load(Ordering::Relaxed) == other.was_clamped.load(Ordering::Relaxed)
            && self.clamp_count.load(Ordering::Relaxed) == other.clamp_count.load(Ordering::Relaxed)
    }
}

/// Equipment configuration set once at initialization and never modified at runtime.
#[derive(Clone, Debug, PartialEq)]
pub struct HvacConfig {
    pub equipment_type: HvacEquipmentType,
    pub zone_id: ZoneId,
    pub shr: f64,
    pub fan_power_w_per_m3_s: f64,
    pub duct_dse: f64,
    /// Zone where duct losses are deposited (e.g. attic, garage, basement).
    /// `None` means duct losses are unrecoverable (lost to outdoors).
    /// OCHRE HVAC.py: `self.duct_zone`
    pub duct_zone_id: Option<ZoneId>,
    /// Fraction of delivered heat (post-DSE) routed to the basement zone [0, 1].
    ///
    /// When non-zero a `basement_zone_id` must also be set. The conditioned zone
    /// receives `dse * (1 - basement_heat_frac)`, the basement zone receives
    /// `dse * basement_heat_frac`, and the duct zone still gets `1 - dse`.
    ///
    /// Default is 0.0 (no basement routing).  OCHRE HVAC.py: `basement_frac`.
    pub basement_heat_frac: f64,
    /// Zone corresponding to the basement (destination for `basement_heat_frac`).
    /// Only meaningful when `basement_heat_frac > 0`.
    pub basement_zone_id: Option<ZoneId>,
    pub supply_air_temp_c: f64,
    pub airflow_m3_s_per_w: f64,
    /// Absolute heat fraction per zone. Each entry is `(zone_id, fraction)` where
    /// `fraction` is a direct multiplier on gross capacity (not normalized).
    /// Fractions sum to `<= 1.0`; the remainder is unrecoverable duct loss.
    ///
    /// Set via `update_zone_heat_fractions()` after configuring `duct_dse` and
    /// `duct_zone_id`. OCHRE HVAC.py: `self.zone_fractions`
    pub zone_heat_fractions: Vec<(ZoneId, f64)>,
    pub biquadratic_coeffs: Vec<[f64; 6]>,
    pub biquadratic_x1_bounds: (f64, f64),
    pub biquadratic_x2_bounds: (f64, f64),
    pub speed_control_mode: SpeedControlMode,
    pub low_speed_capacity_fraction: f64,
    pub space_fraction: f64,
    pub cap_ff_coeffs: [f64; 3],
    pub eir_ff_coeffs: [f64; 3],
    pub ff_bounds: (f64, f64),
    pub plf_min: f64,
    pub heating_capacities_w: Vec<f64>,
    pub cooling_capacities_w: Vec<f64>,
    pub eir_by_stage: Vec<f64>,
    /// Per-speed EIR part-load ratio (PLR) quadratic coefficients `[a, b, c]`.
    ///
    /// OCHRE HVAC.py lines 823, 841–844: each speed stage has its own `eir_plr`
    /// quadratic that computes PLF = a + b·PLR + c·PLR². When `Some`, this
    /// replaces the simplified `1 − Cd·(1−PLR)` formula in `part_load_factor`.
    /// The PLF is clamped to `[min_plf, 1.0]` per OCHRE convention (min_plf=0.7).
    ///
    /// Indexed by speed stage. If a stage index is out of range, the last entry
    /// is used (same fallback pattern as `biquadratic_coeffs`).
    pub eir_plr_coefficients: Option<Vec<[f64; 3]>>,
    pub min_time_per_speed_s: f64,
    pub biquadratic_curve_source: BiquadraticCurveSource,
    /// Rate-limiting and per-timestep counter for `evaluate_biquadratic`
    /// curve-index clamping diagnostics. Supports lock-free increment under
    /// `&self` so the hot path stays allocation-free.
    pub biquadratic_clamp: ClampState,
    /// Zone thermal capacitance [kWh/K] for the equivalent battery model.
    /// Populated at init from `EquipmentConfig.zone_capacitance_kwh_per_k`;
    /// 0.0 means EBM is disabled (no telemetry output).
    pub zone_capacitance_kwh_per_k: f64,
}

/// Runtime state updated every simulation timestep.
#[derive(Clone, Debug, PartialEq)]
pub struct HvacRuntimeState {
    pub duty_cycle: f64,
    /// Borderline config/runtime: set from config at init but also overridden at
    /// runtime by equipment-type Cd defaults and by CoolingCore. Grouped with
    /// runtime because it can change; if future work makes it truly immutable,
    /// it can move to `HvacConfig`.
    pub plf_cooling_degradation_coeff: f64,
    pub plf_state: f64,
    pub startup: StartupConfig,
    pub last_speed_index: usize,
    pub last_speed_frac: f64,
    /// Seconds the unit has been running at the current speed stage.
    /// OCHRE HVAC.py: min_time_in_speed prevents rapid speed hunting by
    /// locking the current stage for at least `min_time_per_speed_s` seconds.
    pub time_at_current_speed_s: f64,
    /// Previous zone temperature [°C], used by `TwoSpeedTime` mode to detect
    /// whether the temperature is still moving away from setpoint.
    /// Updated each timestep by `update_prev_zone_temp`.
    pub prev_zone_temp_c: Option<f64>,
    /// DemandResponse heating setpoint offset [°C]. Set by
    /// `apply_simple_mode_override_in_control` each timestep; cleared
    /// when DR reverts to Normal. Applied by `update_heating_control`
    /// as a temporary override on top of the thermostat's effective
    /// setpoints without mutating `runtime_setpoints` (which is reserved
    /// for `ThermalSetpoint`/`ThermalSetpointDelta` signals).
    pub dr_setpoint_offset_c: f64,
}

/// Control-signal state modified by external control signals.
#[derive(Clone, Debug, PartialEq)]
pub struct HvacControlState {
    /// External max-capacity fraction [0, 1]. Clips ideal capacity output to
    /// `rated_max * max_capacity_fraction`. Set via `MaxCapacityFraction` control signal.
    /// OCHRE HVAC.py: `self.ext_capacity_frac`.
    pub max_capacity_fraction: f64,
    /// Per-speed disable flags. Only indices `0..speed_count` are meaningful.
    /// OCHRE HVAC.py lines 754, 853–857: `disable_speeds[i]` marks speed stage `i`
    /// as unavailable. When a desired speed is disabled, equipment routes to the
    /// highest allowed (non-disabled) speed. Set via `set_disabled_speeds`.
    pub disabled_speeds: [bool; MAX_SPEEDS],
    /// Number of active speed stages (mirrors the length of `heating_capacities_w`).
    pub speed_count: u8,
    /// Cached index of the highest non-disabled speed stage.
    /// Recomputed on every `set_disabled_speeds` call.
    /// OCHRE HVAC.py: `_max_enabled_speed`
    pub max_enabled_speed: usize,
}

/// Shared HVAC wrapper with thermostat state and base helper parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct HvacEquipment {
    pub config: HvacConfig,
    pub thermostat_fsm: ThermostatFsm,
    pub runtime: HvacRuntimeState,
    pub control: HvacControlState,
}

impl HvacEquipment {
    pub fn new(equipment_type: HvacEquipmentType, zone_id: ZoneId) -> Self {
        let default_cd = match equipment_type {
            HvacEquipmentType::MiniSplitHeat | HvacEquipmentType::MiniSplitCool => 0.0,
            _ => DEFAULT_PLF_DEGRADATION_COEFF,
        };
        Self {
            config: HvacConfig {
                equipment_type,
                zone_id,
                shr: 1.0,
                fan_power_w_per_m3_s: DEFAULT_FAN_POWER_W_PER_M3_S,
                duct_dse: 1.0,
                duct_zone_id: None,
                basement_heat_frac: 0.0,
                basement_zone_id: None,
                supply_air_temp_c: equipment_type
                    .default_supply_air_temp_c(DEFAULT_INIT_OUTDOOR_TEMP_C),
                airflow_m3_s_per_w: match equipment_type {
                    HvacEquipmentType::AcCooler
                    | HvacEquipmentType::GshpHeatPumpCooling
                    | HvacEquipmentType::WshpHeatPumpCooling => AIRFLOW_CENTRAL_AC_M3_S_PER_W,
                    HvacEquipmentType::MiniSplitCool => AIRFLOW_MSHP_COOLING_M3_S_PER_W,
                    HvacEquipmentType::GasFurnace
                    | HvacEquipmentType::ElectricFurnace
                    | HvacEquipmentType::AshpHeatPumpOnly
                    | HvacEquipmentType::AshpHeatPumpAux
                    | HvacEquipmentType::MiniSplitHeat
                    | HvacEquipmentType::GshpHeatPumpHeating
                    | HvacEquipmentType::WshpHeatPumpHeating
                    | HvacEquipmentType::Baseboard
                    | HvacEquipmentType::Other => AIRFLOW_HEATING_M3_S_PER_W,
                },
                zone_heat_fractions: vec![(zone_id, 1.0)],
                biquadratic_coeffs: vec![DEFAULT_BIQUADRATIC_COEFFS],
                biquadratic_x1_bounds: DEFAULT_BIQUADRATIC_X1_BOUNDS,
                biquadratic_x2_bounds: DEFAULT_BIQUADRATIC_X2_BOUNDS,
                speed_control_mode: SpeedControlMode::SingleSpeed,
                low_speed_capacity_fraction: DEFAULT_LOW_SPEED_CAPACITY_FRACTION,
                space_fraction: 1.0,
                cap_ff_coeffs: [1.0, 0.0, 0.0],
                eir_ff_coeffs: [1.0, 0.0, 0.0],
                ff_bounds: (0.0, f64::INFINITY),
                plf_min: 0.7,
                heating_capacities_w: vec![],
                cooling_capacities_w: vec![],
                eir_by_stage: vec![],
                eir_plr_coefficients: None,
                min_time_per_speed_s: 300.0,
                biquadratic_curve_source: BiquadraticCurveSource::Identity,
                biquadratic_clamp: ClampState {
                    was_clamped: AtomicBool::new(false),
                    clamp_count: AtomicU64::new(0),
                },
                zone_capacitance_kwh_per_k: 0.0,
            },
            thermostat_fsm: ThermostatFsm::new(ThermalSetpoints {
                heating_c: 20.0,
                cooling_c: 24.0,
            }),
            runtime: HvacRuntimeState {
                duty_cycle: 0.0,
                plf_cooling_degradation_coeff: default_cd,
                plf_state: 1.0,
                startup: StartupConfig::default(),
                last_speed_index: 0,
                last_speed_frac: 0.0,
                time_at_current_speed_s: 0.0,
                prev_zone_temp_c: None,
                dr_setpoint_offset_c: 0.0,
            },
            control: HvacControlState {
                max_capacity_fraction: 1.0,
                disabled_speeds: [false; MAX_SPEEDS],
                speed_count: 0,
                max_enabled_speed: 0,
            },
        }
    }

    pub fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.thermostat_fsm.thermostat = ThermostatConfig {
            hysteresis_c: extract_numeric(config, "hysteresis_c").unwrap_or(1.0),
            cutout_ratio: extract_numeric(config, "cutout_ratio").unwrap_or(DEFAULT_CUTOUT_RATIO),
            min_cycle_time_s: extract_numeric(config, "min_cycle_time_s")
                .unwrap_or(DEFAULT_MIN_CYCLE_TIME_S),
            use_ideal_capacity: extract_bool(config, "use_ideal_capacity").unwrap_or(false),
            deadband_offset: extract_numeric(config, "deadband_offset").unwrap_or(0.2),
        };
        self.thermostat_fsm.thermostat.validate(env)?;

        if let Some(value) = extract_numeric(config, "heating_setpoint_c") {
            self.thermostat_fsm.static_setpoints.heating_c = value;
        }
        if let Some(value) = extract_numeric(config, "cooling_setpoint_c") {
            self.thermostat_fsm.static_setpoints.cooling_c = value;
        }

        // Build setpoint sources: CSV column > daily profile > constant (static).
        self.thermostat_fsm.heating_setpoint_source = build_setpoint_source(config, "heating");
        self.thermostat_fsm.cooling_setpoint_source = build_setpoint_source(config, "cooling");

        // Seed static setpoints from the source so the initial deadband check
        // is reasonable before the first update_mode call.
        if let Some(ScheduleSource::DailyProfile { weekday, .. }) =
            &self.thermostat_fsm.heating_setpoint_source
        {
            self.thermostat_fsm.static_setpoints.heating_c = weekday[0];
        }
        if let Some(ScheduleSource::DailyProfile { weekday, .. }) =
            &self.thermostat_fsm.cooling_setpoint_source
        {
            self.thermostat_fsm.static_setpoints.cooling_c = weekday[0];
        }

        self.thermostat_fsm.static_setpoints = self
            .thermostat_fsm
            .static_setpoints
            .reconcile_for_deadband(self.thermostat_fsm.thermostat.hysteresis_c);

        let explicit_airflow_m3_s_per_w = extract_numeric(config, "airflow_m3_s_per_w")
            .or_else(|| extract_numeric(config, "duct_airflow_m3_s_per_w"));
        let mut airflow_m3_s_per_w = explicit_airflow_m3_s_per_w
            .unwrap_or_else(|| self.config.equipment_type.default_airflow_m3_s_per_w());
        if explicit_airflow_m3_s_per_w.is_none() {
            // HPXML installation quality uses defect deltas where 0.0 means
            // no defect and -0.25 means a 25% airflow reduction.
            let airflow_defect_ratio = extract_numeric(config, "AirflowDefectRatio")
                .or_else(|| extract_numeric(config, "airflow_defect_ratio"))
                .unwrap_or(0.0);
            airflow_m3_s_per_w *= 1.0 + airflow_defect_ratio;
        }
        if !airflow_m3_s_per_w.is_finite() || airflow_m3_s_per_w <= 0.0 {
            return Err(HaresError::Equipment(format!(
                "airflow_m3_s_per_w must be finite and > 0 after defect application, got {}",
                airflow_m3_s_per_w
            )));
        }
        self.config.airflow_m3_s_per_w = airflow_m3_s_per_w;
        if let Some(w_per_m3_s) = extract_numeric(config, "fan_power_w_per_m3_s") {
            self.config.fan_power_w_per_m3_s = w_per_m3_s.max(0.0);
        } else if let Some(w_per_cfm) = extract_numeric(config, "fan_power_w_per_cfm") {
            self.config.fan_power_w_per_m3_s = w_per_cfm.max(0.0) * CFM_PER_M3_S;
        }

        self.config.supply_air_temp_c = extract_numeric(config, "supply_air_temp_c")
            .unwrap_or_else(|| {
                self.config
                    .equipment_type
                    .default_supply_air_temp_c(env.weather.outdoor_temp_c)
            });

        self.config.speed_control_mode = parse_speed_control_mode(config)?;
        self.config.low_speed_capacity_fraction =
            extract_numeric(config, "low_speed_capacity_fraction")
                .unwrap_or(DEFAULT_LOW_SPEED_CAPACITY_FRACTION);
        let cd = super::core_config::resolve_cd(
            config,
            self.config.speed_control_mode,
            extract_numeric(config, "rated_seer"),
            extract_numeric(config, "rated_hspf"),
            DEFAULT_PLF_DEGRADATION_COEFF,
        );
        self.runtime.plf_cooling_degradation_coeff = cd;
        self.runtime.startup = StartupConfig {
            c_d: cd,
            time_since_start_min: 0.0,
        };
        self.runtime.startup.validate()?;
        self.config.biquadratic_coeffs = load_biquadratic_coeffs(config, "biquadratic_coeffs")?;
        // Also honour the split capacity/EIR keys that the HPXML resolver writes
        // (capacity_biquadratic_coeffs / eir_biquadratic_coeffs). These are the
        // same keys that ac_config::load_curve_pair handles for the AC path; the
        // HP heater goes through this generic init, so we replicate the handling
        // here. Per-stage curves are interleaved: [cap_0, eir_0, cap_1, eir_1, ...].
        //
        // When split keys are present they override any `biquadratic_coeffs` loaded
        // above. Both keys must be provided with the same number of per-speed curves;
        // a count mismatch or providing only one side is a configuration error. This
        // is consistent with OCHRE (raises an exception) and EnergyPlus
        // (required-field per speed stage). There is no silent fill — missing or
        // mismatched entries produce an Err rather than substituting identity
        // coefficients.
        {
            let cap_curves = extract_text(config, "capacity_biquadratic_coeffs")
                .map(parse_biquadratic_list)
                .transpose()?
                .unwrap_or_default();
            let eir_curves = extract_text(config, "eir_biquadratic_coeffs")
                .map(parse_biquadratic_list)
                .transpose()?
                .unwrap_or_default();
            if !cap_curves.is_empty() || !eir_curves.is_empty() {
                if !cap_curves.is_empty()
                    && !eir_curves.is_empty()
                    && cap_curves.len() != eir_curves.len()
                {
                    return Err(HaresError::Equipment(format!(
                        "capacity_biquadratic_coeffs has {} speed(s) but eir_biquadratic_coeffs has {} speed(s); per-speed cap/EIR curve counts must match",
                        cap_curves.len(),
                        eir_curves.len()
                    )));
                }
                if cap_curves.is_empty() {
                    return Err(HaresError::Equipment(format!(
                        "eir_biquadratic_coeffs has {} speed(s) but no capacity_biquadratic_coeffs provided; per-speed cap/EIR curves must both be present or both absent",
                        eir_curves.len()
                    )));
                }
                if eir_curves.is_empty() {
                    return Err(HaresError::Equipment(format!(
                        "capacity_biquadratic_coeffs has {} speed(s) but no eir_biquadratic_coeffs provided; per-speed cap/EIR curves must both be present or both absent",
                        cap_curves.len()
                    )));
                }
                let n_stages = cap_curves.len();
                let mut interleaved = Vec::with_capacity(n_stages * 2);
                for i in 0..n_stages {
                    interleaved.push(cap_curves[i]);
                    interleaved.push(eir_curves[i]);
                }
                self.config.biquadratic_coeffs = interleaved;
            }
        }
        // If the loaded coefficients are still identity and the equipment type
        // has physically-correct defaults available, substitute them.
        self.config.biquadratic_curve_source = maybe_substitute_defaults(
            &mut self.config.biquadratic_coeffs,
            self.config.equipment_type,
        );
        // Per-stage biquadratic curve-count validation is deferred to
        // equipment-specific init (heater.rs, air_conditioner.rs, etc.)
        // because heating_capacities_w / cooling_capacities_w are set after
        // hvac_core::init() returns. See Known Limitations in T-0481.
        // OCHRE HVAC.py: biquadratic CSV files specify `min_Twb`, `max_Twb`,
        // `min_Tdb`, `max_Tdb` bounds. Load from config if provided.
        self.config.biquadratic_x1_bounds = load_bounds_pair(
            config,
            "biquadratic_x1_min",
            "biquadratic_x1_max",
            DEFAULT_BIQUADRATIC_X1_BOUNDS,
        );
        self.config.biquadratic_x2_bounds = load_bounds_pair(
            config,
            "biquadratic_x2_min",
            "biquadratic_x2_max",
            DEFAULT_BIQUADRATIC_X2_BOUNDS,
        );
        self.config.min_time_per_speed_s =
            extract_numeric(config, "min_time_per_speed_s").unwrap_or(300.0);
        // 0.0 = disabled (no compressor-level on/off hold). Configure via
        // "min_on_time_s" / "min_off_time_s" keys.
        self.thermostat_fsm.min_on_time_s = extract_numeric(config, "min_on_time_s").unwrap_or(0.0);
        self.thermostat_fsm.min_off_time_s =
            extract_numeric(config, "min_off_time_s").unwrap_or(0.0);

        // Flow-fraction quadratic coefficients: OCHRE HVAC.py `cap_ff` / `eir_ff`.
        if let Some(raw) = extract_text(config, "cap_ff_coeffs") {
            if let Ok(arr) = parse_f64_array_3(raw) {
                self.config.cap_ff_coeffs = arr;
            }
        }
        if let Some(raw) = extract_text(config, "eir_ff_coeffs") {
            if let Ok(arr) = parse_f64_array_3(raw) {
                self.config.eir_ff_coeffs = arr;
            }
        }

        // Flow-fraction clamping bounds: applied before ff quadratic evaluation.
        if let Some(ff_min) = extract_numeric(config, "ff_min") {
            self.config.ff_bounds.0 = ff_min;
        }
        if let Some(ff_max) = extract_numeric(config, "ff_max") {
            self.config.ff_bounds.1 = ff_max;
        }

        // PLF floor: AHRI 210/240 default 0.7; MSHP CSV-derived configs may lower it.
        if let Some(plf_min) = extract_numeric(config, "plf_min") {
            self.config.plf_min = plf_min;
        }

        self.config.eir_plr_coefficients = load_plr_coefficients(config, "eir_plr_coefficients")?;

        // space_fraction: OCHRE HVAC.py:104. Heating/cooling each reads their
        // specific fraction key, falling back to the generic key.
        let frac = if self.config.equipment_type.is_heating() {
            extract_numeric(config, "fraction_heating_load_served")
                .or_else(|| extract_numeric(config, "fraction_load_served"))
        } else {
            extract_numeric(config, "fraction_cooling_load_served")
                .or_else(|| extract_numeric(config, "fraction_load_served"))
        };
        if let Some(f) = frac {
            if !(0.0..=1.0).contains(&f) {
                tracing::warn!(
                    "space_fraction {f} out of range [0,1] for {:?}, clamping",
                    self.config.equipment_type
                );
            }
            self.config.space_fraction = f.clamp(0.0, 1.0);
        }

        if let Some(raw) = extract_numeric(config, "basement_zone_id") {
            if raw == 0.0 {
                tracing::warn!(
                    basement_zone_id = raw,
                    "basement_zone_id=0 is not a valid thermal zone; zones are 1-indexed, rejecting"
                );
            }
            if validate_zone_id(raw) {
                self.config.basement_zone_id = Some(ZoneId(raw as u16));
            }
        }
        self.config.basement_heat_frac =
            extract_numeric(config, "basement_airflow_ratio").unwrap_or(0.0);

        // Evaluate initial thermostat mode from zone temperature so the
        // FSM doesn't start stuck in Deadband when the zone is already
        // outside the comfort band (cold-start fix).
        if let Ok(zone_temp) = lookup_zone_temp(env, self.config.zone_id) {
            let sp = self.thermostat_fsm.effective_setpoints();
            let hysteresis = self.thermostat_fsm.thermostat.hysteresis_c;
            let offset = self
                .thermostat_fsm
                .thermostat
                .deadband_offset
                .clamp(0.0, 1.0);
            let heat_turn_on = sp.heating_c - hysteresis * (1.0 - offset);
            let cool_turn_on = sp.cooling_c + hysteresis * (1.0 - offset);
            if zone_temp < heat_turn_on {
                self.thermostat_fsm.mode = ThermostatMode::Heating;
            } else if zone_temp > cool_turn_on {
                self.thermostat_fsm.mode = ThermostatMode::Cooling;
            }
        }

        // Initialize stage-disable state with all speeds enabled so
        // max_enabled_speed is valid before any DR signal is applied.
        self.set_disabled_speeds(&[]);

        self.config.zone_capacitance_kwh_per_k = config.zone_capacitance_kwh_per_k;

        Ok(())
    }

    /// Set which speed stages are disabled for demand-response control.
    ///
    /// OCHRE HVAC.py lines 853–857: `disable_speeds` is updated from the external
    /// control signal. The highest non-disabled speed is cached as `max_enabled_speed`.
    pub fn apply_control_signal(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        if self.thermostat_fsm.apply_thermal_setpoint_signal(signal)? {
            return Ok(());
        }
        if let ControlSignal::MaxCapacityFraction { fraction } = signal {
            self.control.max_capacity_fraction = *fraction;
        }
        Ok(())
    }

    pub fn set_schedule_setpoints(&mut self, schedule: ScheduleSetpoints) {
        self.thermostat_fsm.schedule_setpoints = Some(schedule);
    }

    pub fn clear_schedule_setpoints(&mut self) {
        self.thermostat_fsm.schedule_setpoints = None;
    }

    pub fn effective_setpoints(&self) -> ThermalSetpoints {
        self.thermostat_fsm.effective_setpoints()
    }

    pub fn update_mode(&mut self, env: &EnvironmentState) -> crate::Result<ThermostatMode> {
        self.thermostat_fsm.update_mode(env, self.config.zone_id)
    }

    pub fn use_ideal_capacity(&self, env: &EnvironmentState) -> bool {
        let variable_speed_mode = matches!(
            self.config.speed_control_mode,
            SpeedControlMode::VariableSpeedIdeal
        );
        let has_four_plus_stages = self
            .config
            .heating_capacities_w
            .len()
            .max(self.config.cooling_capacities_w.len())
            >= 4;
        let coarse_auto =
            env.time_res >= ChronoDuration::seconds(IDEAL_CAPACITY_TIME_RES_THRESHOLD_S);
        // Coarse-timestep auto-ideal is only valid for equipment paths that
        // implement ideal-capacity signal handling.
        let supports_auto_ideal = matches!(
            self.config.equipment_type,
            HvacEquipmentType::AcCooler
                | HvacEquipmentType::MiniSplitCool
                | HvacEquipmentType::AshpHeatPumpOnly
                | HvacEquipmentType::AshpHeatPumpAux
                | HvacEquipmentType::MiniSplitHeat
                | HvacEquipmentType::GasFurnace
                | HvacEquipmentType::ElectricFurnace
                | HvacEquipmentType::Baseboard
                | HvacEquipmentType::Other
        );
        let auto_ideal =
            (coarse_auto || variable_speed_mode || has_four_plus_stages) && supports_auto_ideal;
        self.thermostat_fsm.thermostat.use_ideal_capacity || auto_ideal
    }

    /// Compute the clamped same-type index for a biquadratic curve lookup.
    ///
    /// When `curve_index` is out of bounds against a vector of length `n`,
    /// clamps to the largest same-type index:
    /// - Even indices (capacity curves) → last even index in [0..n)
    /// - Odd indices (EIR curves) → last odd index in [0..n)
    ///
    /// Returns `None` when `n == 0`. Returns `Some(curve_index)` when in bounds.
    /// This prevents cross-type substitution where an EIR curve would be
    /// silently used as a capacity curve (or vice versa). OCHRE HVAC.py:814-818
    /// raises an exception for this condition at init; the hot-path clamp is a
    /// defence-in-depth fallback.
    pub(crate) fn clamp_biquadratic_index(curve_index: usize, n: usize) -> Option<usize> {
        if n == 0 {
            return None;
        }
        if curve_index < n {
            return Some(curve_index);
        }
        let clamped = if curve_index.is_multiple_of(2) {
            if n >= 2 {
                if n.is_multiple_of(2) { n - 2 } else { n - 1 }
            } else {
                0
            }
        } else {
            if n >= 2 {
                if n.is_multiple_of(2) { n - 1 } else { n - 2 }
            } else {
                0
            }
        };
        Some(clamped)
    }

    /// Validate that interleaved biquadratic curve count matches speed stage count.
    ///
    /// A single identity curve (`n_coeffs == 1`) or a single cap+EIR pair shared
    /// across all stages (`n_coeffs == 2`) are legitimate regardless of `n_speeds`.
    /// When multiple pairs are provided, they must cover every speed stage —
    /// partial per-stage curves are a configuration error.
    ///
    /// Rejects `2 < n_coeffs < n_speeds * 2` with a descriptive `HaresError::Equipment`.
    /// OCHRE HVAC.py:814-818 raises an exception for this condition at init.
    pub(crate) fn validate_biquadratic_curve_count(
        n_coeffs: usize,
        n_speeds: usize,
    ) -> crate::Result<()> {
        if n_coeffs > 2 && n_coeffs < n_speeds * 2 {
            return Err(HaresError::Equipment(format!(
                "biquadratic_coeffs has {} entries ({} cap+EIR pair(s)) but \
                 equipment has {} speed stage(s); when providing per-stage \
                 curves, each speed stage needs one cap+EIR pair, or provide \
                 a single pair (2 entries) to share across all stages",
                n_coeffs,
                n_coeffs / 2,
                n_speeds,
            )));
        }
        Ok(())
    }

    pub fn evaluate_biquadratic(
        &self,
        curve_index: usize,
        t_indoor_c: f64,
        t_outdoor_c: f64,
    ) -> f64 {
        let coeffs = self
            .config
            .biquadratic_coeffs
            .get(curve_index)
            .copied()
            .unwrap_or_else(|| {
                let coeffs = &self.config.biquadratic_coeffs;
                let n = coeffs.len();
                let clamped = match Self::clamp_biquadratic_index(curve_index, n) {
                    Some(idx) => idx,
                    None => return DEFAULT_BIQUADRATIC_COEFFS,
                };
                if clamped != curve_index {
                    if !self
                        .config
                        .biquadratic_clamp
                        .was_clamped
                        .load(Ordering::Relaxed)
                    {
                        self.config
                            .biquadratic_clamp
                            .was_clamped
                            .store(true, Ordering::Relaxed);
                        let curve_type = if curve_index.is_multiple_of(2) {
                            "capacity"
                        } else {
                            "EIR"
                        };
                        tracing::warn!(
                            curve_index,
                            clamped_index = clamped,
                            curve_type,
                            coeff_count = n,
                            "biquadratic curve index {} out of bounds ({curve_type} curve), \
                             clamping to index {} (last same-type curve); \
                             init-time validation should have caught this configuration mismatch",
                            curve_index,
                            clamped,
                        );
                    }
                    self.config
                        .biquadratic_clamp
                        .clamp_count
                        .fetch_add(1, Ordering::Relaxed);
                }
                coeffs[clamped]
            });
        // Capacity curves (even indices) in real curve sets (len > 1 means
        // cap+EIR pairs are present, not a test fixture or identity fallback)
        // use a non-negative output clamp so that cold-climate simulation
        // never produces negative capacity ratios.
        // EnergyPlus CurveManager.cc:282–287 optional output limits for
        // biquadratic curves — HARES mirrors this at the curve-evaluation level.
        let (output_min, output_max) =
            if curve_index.is_multiple_of(2) && self.config.biquadratic_coeffs.len() > 1 {
                (Some(0.0_f64), None)
            } else {
                (None, None)
            };
        let curve = BiquadraticCurve {
            coeffs,
            x1_bounds: self.config.biquadratic_x1_bounds,
            x2_bounds: self.config.biquadratic_x2_bounds,
            warn_on_clamp: false,
            output_min,
            output_max,
        };
        curve.evaluate(t_indoor_c, t_outdoor_c)
    }

    /// Read and reset the per-timestep biquadratic curve-index clamping counter.
    ///
    /// Returns the number of times `evaluate_biquadratic` clamped an out-of-bounds
    /// curve index during the current timestep, then resets to zero. Callers should
    /// invoke this once per timestep (after all curve evaluations) to feed the
    /// `biquadratic_index_clamped` observe counter.
    pub fn take_biquadratic_clamp_count(&self) -> u64 {
        self.config
            .biquadratic_clamp
            .clamp_count
            .swap(0, Ordering::Relaxed)
    }

    /// Evaluate the flow-fraction quadratic: `c[0] + c[1]*ff + c[2]*ff^2`.
    /// OCHRE HVAC.py `_biquadratic`: `ff_ratio = coeffs_ff[0] + coeffs_ff[1]*ff + coeffs_ff[2]*ff*ff`.
    pub fn evaluate_ff_quadratic(coeffs: &[f64; 3], ff: f64) -> f64 {
        coeffs[0] + coeffs[1] * ff + coeffs[2] * ff * ff
    }

    /// Evaluate a biquadratic curve and apply the flow-fraction correction.
    ///
    /// Returns `(raw, flow_adjusted)` where:
    /// - `raw` is the direct curve output
    /// - `flow_adjusted` is `raw * ff_ratio` where `ff_ratio` is evaluated from the
    ///   flow-fraction quadratic coefficients (`cap_ff_coeffs` for even curve indices,
    ///   `eir_ff_coeffs` for odd curve indices). When the ff coefficients are the
    ///   default `[1.0, 0.0, 0.0]`, this degenerates to `raw * flow_fraction`.
    ///
    /// PLF is intentionally NOT applied here. Callers must apply it themselves:
    /// - Capacity curves: PLF does not apply.
    /// - EIR curves: divide `flow_adjusted` by PLF (efficiency penalty for cycling).
    pub fn evaluate_biquadratic_with_flow(
        &self,
        curve_index: usize,
        t_indoor_c: f64,
        t_outdoor_c: f64,
        flow_fraction: f64,
    ) -> (f64, f64) {
        let raw = self.evaluate_biquadratic(curve_index, t_indoor_c, t_outdoor_c);
        let ff_clamped = flow_fraction.clamp(self.config.ff_bounds.0, self.config.ff_bounds.1);
        // Use cap_ff for capacity curves (even index), eir_ff for EIR curves (odd index).
        let ff_coeffs = if curve_index.is_multiple_of(2) {
            &self.config.cap_ff_coeffs
        } else {
            &self.config.eir_ff_coeffs
        };
        let ff_ratio = Self::evaluate_ff_quadratic(ff_coeffs, ff_clamped);
        let adjusted = raw * ff_ratio;
        (raw, adjusted)
    }

    pub fn update_supply_air_temp(&mut self, env: &EnvironmentState) {
        if self.config.equipment_type == HvacEquipmentType::AshpHeatPumpOnly {
            self.config.supply_air_temp_c = self
                .config
                .equipment_type
                .default_supply_air_temp_c(env.weather.outdoor_temp_c);
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        BoundaryPolicy, ControlSignal, EnvironmentState, GridState, PortSlots, ScheduleSource,
        SurfaceIrradiance, ThermalAccumulator, ThermalCategory, WeatherState, ZoneState,
    };

    use super::super::thermostat::{
        COOLING_DISABLED_SETPOINT_C, HEATING_DISABLED_SETPOINT_C, ScheduleSetpoints,
    };
    use super::*;
    use crate::EquipmentConfig;

    fn env(zone_temp_c: f64, time_res_s: i64, second: i64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 8.3,
                outdoor_humidity_ratio: 0.005,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 10.0,
                sky_temp_c: 5.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![SurfaceIrradiance {
                    surface_id: 1,
                    direct_w_m2: 0.0,
                    diffuse_w_m2: 0.0,
                    reflected_w_m2: 0.0,
                    angle_of_incidence_rad: 0.0,
                }],
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
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .expect("UTC offset")
                .with_ymd_and_hms(2026, 3, 18, 0, 0, 0)
                .single()
                .expect("valid")
                + ChronoDuration::seconds(second),
            time_res: ChronoDuration::seconds(time_res_s),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    #[test]
    fn thermostat_cutout_ratio_is_validated() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .test_extras_mut()
            .insert("cutout_ratio".to_string(), 1.2.into());
        let err = hvac
            .init(&config, &env(20.0, 60, 0))
            .expect_err("must fail");
        assert!(err.to_string().contains("cutout_ratio"));
    }

    #[test]
    fn thermostat_defaults_cutout_ratio_when_unspecified() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        let config = EquipmentConfig::default();
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert!((hvac.thermostat_fsm.thermostat.cutout_ratio - DEFAULT_CUTOUT_RATIO).abs() < 1e-12);
    }

    #[test]
    fn thermostat_defaults_min_cycle_time_when_unspecified() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        let config = EquipmentConfig::default();
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert!(
            (hvac.thermostat_fsm.thermostat.min_cycle_time_s - DEFAULT_MIN_CYCLE_TIME_S).abs()
                < 1e-12
        );
    }

    #[test]
    fn thermostat_uses_explicit_cutout_ratio_override() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .test_extras_mut()
            .insert("cutout_ratio".to_string(), 0.1.into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert!((hvac.thermostat_fsm.thermostat.cutout_ratio - 0.1).abs() < 1e-12);
    }

    #[test]
    fn default_min_cycle_time_blocks_turnoff_before_60s() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        let config = EquipmentConfig::default();
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        hvac.thermostat_fsm.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 25.0,
        };

        let mode = hvac.update_mode(&env(18.0, 60, 0)).expect("turns on");
        assert_eq!(mode, ThermostatMode::Heating);

        let mode = hvac.update_mode(&env(22.0, 60, 30)).expect("still locked");
        assert_eq!(mode, ThermostatMode::Heating);

        let mode = hvac.update_mode(&env(22.0, 60, 61)).expect("lock expired");
        assert_eq!(mode, ThermostatMode::Deadband);
    }

    #[test]
    fn thermostat_setpoint_deadband_reconciled_when_too_close() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .test_extras_mut()
            .insert("hysteresis_c".to_string(), 1.0.into());
        config
            .test_extras_mut()
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        config
            .test_extras_mut()
            .insert("cooling_setpoint_c".to_string(), 22.0.into());
        hvac.init(&config, &env(20.0, 60, 0))
            .expect("init should succeed with reconciliation");
        let gap = hvac.thermostat_fsm.static_setpoints.cooling_c
            - hvac.thermostat_fsm.static_setpoints.heating_c;
        assert!(gap >= 2.0, "reconciled gap must be >= 2.0 C, got {gap:.3}");
    }

    #[test]
    fn thermostat_fsm_heating_cycle_turns_on_and_off() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.thermostat_fsm.thermostat.hysteresis_c = 1.0;
        hvac.thermostat_fsm.thermostat.cutout_ratio = 0.5;
        hvac.thermostat_fsm.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 25.0,
        };

        let mode = hvac.update_mode(&env(18.9, 60, 0)).expect("mode updates");
        assert_eq!(mode, ThermostatMode::Heating);

        let mode = hvac.update_mode(&env(20.6, 60, 61)).expect("mode updates");
        assert_eq!(mode, ThermostatMode::Deadband);
    }

    #[test]
    fn min_cycle_time_holds_mode_until_elapsed() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.thermostat_fsm.thermostat.min_cycle_time_s = 300.0;
        hvac.thermostat_fsm.thermostat.hysteresis_c = 1.0;
        hvac.thermostat_fsm.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 25.0,
        };

        let mode = hvac.update_mode(&env(18.0, 60, 0)).expect("mode updates");
        assert_eq!(mode, ThermostatMode::Heating);

        let mode = hvac.update_mode(&env(22.0, 60, 120)).expect("locked");
        assert_eq!(mode, ThermostatMode::Heating);

        let mode = hvac.update_mode(&env(22.0, 60, 301)).expect("unlocked");
        assert_eq!(mode, ThermostatMode::Deadband);
    }

    #[test]
    fn control_signal_thermal_setpoint_has_top_priority() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.thermostat_fsm.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 24.0,
        };
        hvac.set_schedule_setpoints(ScheduleSetpoints {
            heating_c: Some(19.0),
            cooling_c: Some(25.0),
            ..ScheduleSetpoints::default()
        });
        hvac.apply_control_signal(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(18.0),
            cooling_setpoint_c: Some(26.0),
            deadband_c: None,
        })
        .unwrap();

        let setpoints = hvac.effective_setpoints();
        assert_eq!(setpoints.heating_c, 18.0);
        assert_eq!(setpoints.cooling_c, 26.0);
    }

    #[test]
    fn no_space_heating_schedule_disables_heating_call() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.thermostat_fsm.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 26.0,
        };
        hvac.set_schedule_setpoints(ScheduleSetpoints {
            no_space_heating: true,
            ..ScheduleSetpoints::default()
        });
        let mode = hvac.update_mode(&env(-20.0, 60, 0)).expect("mode updates");
        assert_eq!(mode, ThermostatMode::Deadband);
        assert_eq!(
            hvac.effective_setpoints().heating_c,
            HEATING_DISABLED_SETPOINT_C
        );
    }

    #[test]
    fn no_space_cooling_schedule_disables_cooling_call() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.thermostat_fsm.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 24.0,
        };
        hvac.set_schedule_setpoints(ScheduleSetpoints {
            no_space_cooling: true,
            ..ScheduleSetpoints::default()
        });
        let mode = hvac.update_mode(&env(45.0, 60, 0)).expect("mode updates");
        assert_eq!(mode, ThermostatMode::Deadband);
        assert_eq!(
            hvac.effective_setpoints().cooling_c,
            COOLING_DISABLED_SETPOINT_C
        );
    }

    #[test]
    fn dse_reduces_effective_capacity() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.config.duct_dse = 0.8;
        assert_eq!(hvac.apply_duct_dse(10_000.0), 8_000.0);
    }

    #[test]
    fn supply_air_defaults_match_equipment_tables() {
        assert!((HvacEquipmentType::GasFurnace.default_supply_air_temp_c(8.3) - 54.4).abs() < 1e-9);
        assert!(
            (HvacEquipmentType::ElectricFurnace.default_supply_air_temp_c(8.3) - 48.9).abs() < 1e-9
        );
        assert!(
            (HvacEquipmentType::AshpHeatPumpOnly.default_supply_air_temp_c(8.3) - 32.2).abs()
                < 1e-9
        );
        assert!(
            (HvacEquipmentType::AshpHeatPumpAux.default_supply_air_temp_c(8.3) - 40.6).abs() < 1e-9
        );
        assert!(
            (HvacEquipmentType::MiniSplitHeat.default_supply_air_temp_c(8.3) - 43.3).abs() < 1e-9
        );
        // Cooling variants use 40.6 as a construction-time placeholder;
        // overwritten by step() before any real use.
        assert!((HvacEquipmentType::AcCooler.default_supply_air_temp_c(8.3) - 40.6).abs() < 1e-9);
        assert!(
            (HvacEquipmentType::MiniSplitCool.default_supply_air_temp_c(8.3) - 40.6).abs() < 1e-9
        );
    }

    #[test]
    fn airflow_heating_defaults_and_scales_by_defect_delta() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .test_extras_mut()
            .insert("AirflowDefectRatio".to_string(), (-0.2).into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        let expected = AIRFLOW_HEATING_M3_S_PER_W * 0.8;
        assert!((hvac.config.airflow_m3_s_per_w - expected).abs() < 1e-12);
    }

    #[test]
    fn airflow_cooling_defaults_and_scales_by_defect_delta() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::AcCooler, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .test_extras_mut()
            .insert("AirflowDefectRatio".to_string(), (-0.2).into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        let expected = AIRFLOW_CENTRAL_AC_M3_S_PER_W * 0.8;
        assert!((hvac.config.airflow_m3_s_per_w - expected).abs() < 1e-12);
    }

    #[test]
    fn airflow_defect_zero_keeps_default_airflow() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::AcCooler, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .test_extras_mut()
            .insert("AirflowDefectRatio".to_string(), 0.0.into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert!((hvac.config.airflow_m3_s_per_w - AIRFLOW_CENTRAL_AC_M3_S_PER_W).abs() < 1e-12);
    }

    #[test]
    fn explicit_airflow_is_not_rescaled_by_defect_ratio() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::AcCooler, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .test_extras_mut()
            .insert("airflow_m3_s_per_w".to_string(), 4.2e-5.into());
        config
            .test_extras_mut()
            .insert("AirflowDefectRatio".to_string(), (-0.5).into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert!((hvac.config.airflow_m3_s_per_w - 4.2e-5).abs() < 1e-12);
    }

    #[test]
    fn two_hvac_instances_same_zone_accumulate_thermal_ports() {
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let hvac_a = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        let hvac_b = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));

        hvac_a
            .write_zone_thermal_contributions(&mut ports, 1000.0, 0.0, ThermalCategory::HvacHeating)
            .expect("write a");
        hvac_b
            .write_zone_thermal_contributions(&mut ports, 1500.0, 0.0, ThermalCategory::HvacHeating)
            .expect("write b");

        assert_eq!(ports.thermal[0].sensible_gain_w, 2500.0);
    }

    #[test]
    fn biquadratic_evaluation_matches_reference_value() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.biquadratic_coeffs = vec![[1.0, 0.2, 0.01, -0.1, 0.005, 0.02]];
        let x = 20.0;
        let y = 30.0;
        let expected = 1.0 + 0.2 * x + 0.01 * x * x - 0.1 * y + 0.005 * y * y + 0.02 * x * y;
        let got = hvac.evaluate_biquadratic(0, x, y);
        assert!((got - expected).abs() < 1e-12);
    }

    #[test]
    fn flow_fraction_quadratic_applied_in_evaluate_biquadratic_with_flow() {
        // PLF is NOT applied by evaluate_biquadratic_with_flow; callers handle
        // PLF separately: capacity does not use PLF, EIR divides by PLF.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.biquadratic_coeffs = vec![[1.0, 0.0, 0.0, 0.0, 0.0, 0.0]];
        // Default ff coeffs [1, 0, 0]: ff_ratio = 1.0 regardless of ff input.
        let (raw, adjusted) = hvac.evaluate_biquadratic_with_flow(0, 19.0, 35.0, 0.8);
        assert!((raw - 1.0).abs() < 1e-12);
        assert!((adjusted - 1.0).abs() < 1e-12);

        // Non-trivial ff quadratic: ff_ratio = 0.7 + 0.4*ff - 0.1*ff^2
        // At ff=0.8: 0.7 + 0.32 - 0.064 = 0.956
        hvac.config.cap_ff_coeffs = [0.7, 0.4, -0.1];
        let (raw2, adjusted2) = hvac.evaluate_biquadratic_with_flow(0, 19.0, 35.0, 0.8);
        assert!((raw2 - 1.0).abs() < 1e-12);
        let expected_ff = 0.7 + 0.4 * 0.8 + (-0.1) * 0.8 * 0.8;
        assert!(
            (adjusted2 - expected_ff).abs() < 1e-12,
            "adjusted2={adjusted2}, expected={expected_ff}"
        );
    }

    #[test]
    fn eir_ff_quadratic_used_for_odd_curve_index() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.biquadratic_coeffs = vec![
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        ];
        hvac.config.eir_ff_coeffs = [1.3, -0.5, 0.2];
        // curve_index=1 (odd) → uses eir_ff_coeffs
        // At ff=0.8: 1.3 + (-0.5)*0.8 + 0.2*0.64 = 1.3 - 0.4 + 0.128 = 1.028
        let (raw, adjusted) = hvac.evaluate_biquadratic_with_flow(1, 19.0, 35.0, 0.8);
        assert!((raw - 1.0).abs() < 1e-12);
        let expected = 1.3 + (-0.5) * 0.8 + 0.2 * 0.64;
        assert!(
            (adjusted - expected).abs() < 1e-12,
            "adjusted={adjusted}, expected={expected}"
        );
    }

    #[test]
    fn part_load_factor_formula_correct() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.runtime.plf_cooling_degradation_coeff = 0.25;
        let plf = hvac.part_load_factor(0.5);
        assert!((plf - 0.875).abs() < 1e-12);
    }

    #[test]
    fn two_speed_setpoint_control_switches_at_threshold() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.speed_control_mode = SpeedControlMode::TwoSpeedSetpoint;
        hvac.config.low_speed_capacity_fraction = 0.6;
        // Disable minimum dwell time so threshold-crossing tests are not blocked.
        hvac.config.min_time_per_speed_s = 0.0;
        let low = hvac.select_speed(0.4);
        let high = hvac.select_speed(0.8);
        assert_eq!(low.speed_index, 0);
        assert_eq!(high.speed_index, 1);
    }

    #[test]
    fn two_speed_min_time_locks_stage_during_dwell_period() {
        // OCHRE HVAC.py: min_time_in_speed prevents rapid hunting between low and
        // high compressor stages. With a 600 s lockout and 60 s steps, the stage
        // must remain locked for 10 consecutive steps after selection.
        //
        // Simulation order matches the actual control/step cycle:
        //   update_control() → select_speed()  (speed decision first)
        //   step()           → advance_speed_timer()  (then accumulate dwell time)
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.speed_control_mode = SpeedControlMode::TwoSpeedSetpoint;
        hvac.config.low_speed_capacity_fraction = 0.5;
        hvac.config.min_time_per_speed_s = 600.0;

        // Start at low speed with a low load.
        let sel = hvac.select_speed(0.3);
        assert_eq!(sel.speed_index, 0, "initial selection must be low stage");
        hvac.advance_speed_timer(60.0); // first step's dwell at low speed

        // Now jump to a high load -- should be blocked by min-time.
        // Timer starts at 60 s; after 9 more select+advance cycles it will be 600 s.
        for step in 1..=9 {
            let sel = hvac.select_speed(0.9);
            assert_eq!(
                sel.speed_index, 0,
                "speed change must be blocked at step {step} while min_time_per_speed_s not elapsed"
            );
            hvac.advance_speed_timer(60.0);
        }
        // Timer is now 600 s = min_time_per_speed_s.

        // After 600 s the lock expires (OCHRE uses strict <); the next
        // select_speed with high load switches to stage 1.
        let sel = hvac.select_speed(0.9);
        assert_eq!(
            sel.speed_index, 1,
            "speed change must be allowed after min-time elapses"
        );
    }

    /// Helper: create an HvacEquipment in MultiSpeedInterpolated mode with 4 heating stages.
    fn make_msi_hvac() -> HvacEquipment {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.speed_control_mode = SpeedControlMode::MultiSpeedInterpolated;
        // Capacities: [4000, 6000, 8000, 10000] W → fractions [0.4, 0.6, 0.8, 1.0]
        hvac.config.heating_capacities_w = vec![4000.0, 6000.0, 8000.0, 10000.0];
        hvac
    }

    #[test]
    fn multi_speed_bracket_selection() {
        // load_fraction=0.5 is between cap_frac[0]=0.4 and cap_frac[1]=0.6
        // → speed_index=0, speed_frac = (0.5 - 0.4) / (0.6 - 0.4) = 0.5
        let mut hvac = make_msi_hvac();
        let sel = hvac.select_speed(0.5);
        assert_eq!(sel.speed_index, 0, "lower bracket index");
        assert!(
            (sel.speed_frac - 0.5).abs() < 1e-12,
            "speed_frac={}",
            sel.speed_frac
        );
        assert_eq!(
            sel.part_load_ratio, 1.0,
            "PLR=1 when interpolating between stages"
        );
    }

    #[test]
    fn multi_speed_plr_below_lowest_stage() {
        // load_fraction=0.30 < cap_frac[0]=0.40 → cycling at speed 0
        // PLR = 0.30 / 0.40 = 0.75
        let mut hvac = make_msi_hvac();
        let sel = hvac.select_speed(0.30);
        assert_eq!(sel.speed_index, 0);
        assert_eq!(sel.speed_frac, 0.0, "no interpolation at lowest stage");
        assert!(
            (sel.part_load_ratio - 0.75).abs() < 1e-12,
            "PLR={}",
            sel.part_load_ratio
        );
    }

    #[test]
    fn multi_speed_midpoint_interpolation() {
        // load_fraction=0.7 is between cap_frac[1]=0.6 and cap_frac[2]=0.8
        // → speed_index=1, speed_frac = (0.7 - 0.6) / (0.8 - 0.6) = 0.5
        let mut hvac = make_msi_hvac();
        let sel = hvac.select_speed(0.7);
        assert_eq!(sel.speed_index, 1);
        assert!(
            (sel.speed_frac - 0.5).abs() < 1e-12,
            "speed_frac={}",
            sel.speed_frac
        );
        assert_eq!(sel.part_load_ratio, 1.0);
    }

    #[test]
    fn multi_speed_full_capacity_clamp() {
        // load_fraction >= 1.0 → last stage, speed_frac=0, PLR=1
        let mut hvac = make_msi_hvac();
        let sel = hvac.select_speed(1.0);
        assert_eq!(sel.speed_index, 3, "top stage index");
        assert_eq!(sel.speed_frac, 0.0);
        assert_eq!(sel.part_load_ratio, 1.0);
    }

    #[test]
    fn variable_speed_matches_requested_capacity_fraction() {
        // P2-K speed selection fixes changed the VariableSpeedIdeal path.
        // This test needs multi-stage capacities to exercise the interpolation.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.speed_control_mode = SpeedControlMode::VariableSpeedIdeal;
        hvac.config.heating_capacities_w = vec![3000.0, 6000.0, 9000.0, 12000.0];
        let sel = hvac.select_speed(0.63);
        assert!(sel.speed_index <= 3);
        assert!(sel.part_load_ratio >= 0.0 && sel.part_load_ratio <= 1.0);
    }

    #[test]
    fn ahri_210_240_plf_degradation_default() {
        // AHRI Standard 210/240-2023, S6.6.3
        // PLF = 1 - Cd * (1 - PLR), default Cd = 0.25
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.runtime.plf_cooling_degradation_coeff = super::DEFAULT_PLF_DEGRADATION_COEFF;

        // At PLR=0.5: PLF = 1 - 0.25*0.5 = 0.875
        let plf_half = hvac.part_load_factor(0.5);
        assert!(
            (plf_half - 0.875).abs() < 1e-12,
            "PLF at PLR=0.5: {plf_half}"
        );

        // At PLR=0.0: PLF = 1 - 0.25*1.0 = 0.75 (minimum)
        let plf_zero = hvac.part_load_factor(0.0);
        assert!(
            (plf_zero - 0.75).abs() < 1e-12,
            "PLF at PLR=0.0: {plf_zero}"
        );

        // At PLR=1.0: PLF = 1 - 0.25*0.0 = 1.0 (full load)
        let plf_full = hvac.part_load_factor(1.0);
        assert!((plf_full - 1.0).abs() < 1e-12, "PLF at PLR=1.0: {plf_full}");
    }

    #[test]
    fn startup_ramp_degrades_first_step_and_recovers_to_full() {
        // c_d=0.25 → t_full=5.4 min; with dt=1 min the ramp takes ~6 steps.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.runtime.startup.c_d = 0.25;
        hvac.runtime.duty_cycle = 1.0;

        // First on-step must be degraded (t = 0.5 min < t_full = 5.4 min).
        let step1 = hvac.apply_startup_capacity_degradation(10_000.0, 1.0);
        assert!(step1 < 10_000.0, "first startup step must degrade: {step1}");

        // After enough on-steps the ramp reaches full capacity.
        let mut last = step1;
        for _ in 0..10 {
            last = hvac.apply_startup_capacity_degradation(10_000.0, 1.0);
        }
        assert!(
            (last - 10_000.0).abs() < 1e-9,
            "must reach full after ramp: {last}"
        );

        // Turn off resets the ramp.
        hvac.runtime.duty_cycle = 0.0;
        let off = hvac.apply_startup_capacity_degradation(10_000.0, 1.0);
        assert!((off - 10_000.0).abs() < 1e-9);
        assert_eq!(hvac.runtime.startup.time_since_start_min, 0.0);

        // Restart must degrade again.
        hvac.runtime.duty_cycle = 1.0;
        let restart = hvac.apply_startup_capacity_degradation(10_000.0, 1.0);
        assert!(restart < 10_000.0, "restart must degrade again: {restart}");
    }

    #[test]
    fn mshp_plf_degradation_coeff_defaults_to_zero() {
        // MSHP selects discrete compressor stages rather than cycling, so the
        // AHRI cycling-degradation penalty (Cd) must be zero by default.
        for eq_type in [
            HvacEquipmentType::MiniSplitHeat,
            HvacEquipmentType::MiniSplitCool,
        ] {
            let hvac = HvacEquipment::new(eq_type, ZoneId(1));
            assert_eq!(
                hvac.runtime.plf_cooling_degradation_coeff, 0.0,
                "{eq_type:?} must default to Cd=0 (no cycling penalty)"
            );
        }
    }

    #[test]
    fn variable_speed_cooling_always_uses_ideal_capacity() {
        let env = env(26.0, 60, 0);

        let mut central = HvacEquipment::new(HvacEquipmentType::AcCooler, ZoneId(1));
        central.config.speed_control_mode = SpeedControlMode::VariableSpeedIdeal;
        assert!(
            central.use_ideal_capacity(&env),
            "4-speed central cooling must use ideal-capacity control"
        );

        let mut minisplit = HvacEquipment::new(HvacEquipmentType::MiniSplitCool, ZoneId(1));
        minisplit.config.speed_control_mode = SpeedControlMode::VariableSpeedIdeal;
        assert!(
            minisplit.use_ideal_capacity(&env),
            "4-speed mini-split cooling must use ideal-capacity control"
        );
    }

    #[test]
    fn four_stage_minisplit_heating_auto_enables_ideal_capacity() {
        let env = env(17.0, 60, 0);
        let mut minisplit_heat = HvacEquipment::new(HvacEquipmentType::MiniSplitHeat, ZoneId(1));
        minisplit_heat.config.speed_control_mode = SpeedControlMode::MultiSpeedInterpolated;
        minisplit_heat.config.heating_capacities_w = vec![2500.0, 5000.0, 7500.0, 10_000.0];
        assert!(
            minisplit_heat.use_ideal_capacity(&env),
            "4-stage mini-split heating must auto-enable ideal-capacity control"
        );
    }

    #[test]
    fn non_mshp_plf_degradation_coeff_defaults_to_ahri_standard() {
        for eq_type in [
            HvacEquipmentType::GasFurnace,
            HvacEquipmentType::ElectricFurnace,
            HvacEquipmentType::AshpHeatPumpOnly,
            HvacEquipmentType::AshpHeatPumpAux,
            HvacEquipmentType::AcCooler,
            HvacEquipmentType::Baseboard,
            HvacEquipmentType::Other,
        ] {
            let hvac = HvacEquipment::new(eq_type, ZoneId(1));
            assert_eq!(
                hvac.runtime.plf_cooling_degradation_coeff, DEFAULT_PLF_DEGRADATION_COEFF,
                "{eq_type:?} must default to AHRI standard Cd={DEFAULT_PLF_DEGRADATION_COEFF}"
            );
        }
    }

    #[test]
    fn biquadratic_clamps_extreme_inputs_to_default_bounds() {
        // Default bounds are (-10, 50) for x1 and (-50, 60) for x2;
        // inputs outside these ranges must be clamped.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        // Linear curve: f(x1, x2) = x1 (coefficient on x1 = 1, rest 0)
        hvac.config.biquadratic_coeffs = vec![[0.0, 1.0, 0.0, 0.0, 0.0, 0.0]];
        // With x1_bounds = (-10, 50): input -200°C must clamp to -10°C.
        let result = hvac.evaluate_biquadratic(0, -200.0, 30.0);
        assert!(
            (result - (-10.0)).abs() < 1e-9,
            "input -200°C must clamp to -10°C x1 lower bound; got {result}"
        );
    }

    #[test]
    fn biquadratic_x2_lower_bound_clamps_through_production_path() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.biquadratic_coeffs = vec![[0.0, 0.0, 0.0, 1.0, 0.0, 0.0]];
        let at_neg60 = hvac.evaluate_biquadratic(0, 20.0, -60.0);
        let at_neg50 = hvac.evaluate_biquadratic(0, 20.0, -50.0);
        assert_eq!(
            at_neg60, at_neg50,
            "DEFAULT_BIQUADRATIC_X2_BOUNDS lower bound -50°C must clamp x2=-60 to -50"
        );
    }

    #[test]
    fn biquadratic_x2_upper_bound_clamps_through_production_path() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.biquadratic_coeffs = vec![[0.0, 0.0, 0.0, 1.0, 0.0, 0.0]];
        let at_pos70 = hvac.evaluate_biquadratic(0, 20.0, 70.0);
        let at_pos60 = hvac.evaluate_biquadratic(0, 20.0, 60.0);
        assert_eq!(
            at_pos70, at_pos60,
            "DEFAULT_BIQUADRATIC_X2_BOUNDS upper bound +60°C must clamp x2=70 to +60"
        );
    }

    #[test]
    fn multi_speed_zero_load_returns_zero_plr() {
        let mut hvac = make_msi_hvac();
        let sel = hvac.select_speed(0.0);
        assert_eq!(sel.speed_index, 0);
        assert_eq!(sel.speed_frac, 0.0);
        assert_eq!(sel.part_load_ratio, 0.0, "zero load → zero PLR");
    }

    #[test]
    fn multi_speed_at_exact_stage_boundary() {
        // load_fraction = cap_frac[1] = 0.6 exactly.
        // partition_point(|&f| f < 0.6): cap_fracs[0]=0.4 < 0.6 (true), cap_fracs[1]=0.6 < 0.6 (false)
        // → hi=1, lo=0, frac = (0.6 - 0.4) / (0.6 - 0.4) = 1.0
        // This means "fully at the upper bracket" -- equivalent to being at speed_index=1.
        let mut hvac = make_msi_hvac();
        let sel = hvac.select_speed(0.6);
        assert_eq!(sel.speed_index, 0, "lower bracket index is 0");
        assert!(
            (sel.speed_frac - 1.0).abs() < 1e-12,
            "speed_frac=1.0 at exact upper boundary"
        );
        assert_eq!(sel.part_load_ratio, 1.0);
    }

    #[test]
    fn grid_emergency_off_blocked_until_min_on_time_elapses() {
        // Verifies interaction between min-on-time (feature 2) and a GridEmergency
        // signal (feature 11) that tries to force the compressor off before the
        // minimum hold has elapsed.
        //
        // Scenario:
        //   t=0s   Heating begins; min_on_time_s=120s.
        //   t=60s  GridEmergency arrives → thermostat would prefer Deadband, but
        //          min-on-time blocks the transition. Equipment stays Heating.
        //   t=120s min_on_time has elapsed → Deadband transition is now allowed.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.thermostat_fsm.thermostat.hysteresis_c = 1.0;
        hvac.thermostat_fsm.thermostat.cutout_ratio = 0.5;
        hvac.thermostat_fsm.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 25.0,
        };
        hvac.thermostat_fsm.min_on_time_s = 120.0;

        // t=0s: zone is cold → enters Heating.
        let mode = hvac.update_mode(&env(18.0, 60, 0)).expect("initial mode");
        assert_eq!(mode, ThermostatMode::Heating, "must enter Heating at t=0");

        // t=60s: DR signal forces setpoints so thermostat would cut out
        // (simulate GridEmergency by pushing the zone temp far above cutout).
        // In a full DR implementation the signal would also set a setpoint
        // override; here we drive the same code path by passing a hot zone temp
        // while min_on_time has not yet elapsed.
        let mode = hvac.update_mode(&env(30.0, 60, 60)).expect("mode at 60s");
        assert_eq!(
            mode,
            ThermostatMode::Heating,
            "min_on_time_s=120 must block Heating→Deadband at t=60s even under DR pressure"
        );

        // t=120s: min_on_time has elapsed → equipment shuts off.
        let mode = hvac.update_mode(&env(30.0, 60, 120)).expect("mode at 120s");
        assert_eq!(
            mode,
            ThermostatMode::Deadband,
            "min_on_time_s=120 must release at t=120s and allow Deadband"
        );
    }

    #[test]
    fn low_seer_single_speed_overrides_cd_on_init() {
        // SEER < 13 → Cd = 0.20 (applies to both PLF coeff and startup ramp).
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .test_extras_mut()
            .insert("rated_seer".to_string(), 10.0.into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert!(
            (hvac.runtime.plf_cooling_degradation_coeff - 0.20).abs() < 1e-12,
            "low-SEER Cd must be 0.20, got {}",
            hvac.runtime.plf_cooling_degradation_coeff
        );
        assert!(
            (hvac.runtime.startup.c_d - 0.20).abs() < 1e-12,
            "startup c_d must also be 0.20 for low-SEER: {}",
            hvac.runtime.startup.c_d
        );
    }

    #[test]
    fn high_seer_single_speed_overrides_cd_on_init() {
        // SEER >= 13 → Cd = 0.07.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .test_extras_mut()
            .insert("rated_seer".to_string(), 15.0.into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert!(
            (hvac.runtime.plf_cooling_degradation_coeff - 0.07).abs() < 1e-12,
            "high-SEER Cd must be 0.07, got {}",
            hvac.runtime.plf_cooling_degradation_coeff
        );
        assert!(
            (hvac.runtime.startup.c_d - 0.07).abs() < 1e-12,
            "startup c_d must also be 0.07: {}",
            hvac.runtime.startup.c_d
        );
    }

    #[test]
    fn low_hspf_single_speed_overrides_cd_on_init() {
        // HSPF < 7 → Cd = 0.20.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .test_extras_mut()
            .insert("rated_hspf".to_string(), 6.5.into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert!(
            (hvac.runtime.plf_cooling_degradation_coeff - 0.20).abs() < 1e-12,
            "low-HSPF Cd must be 0.20, got {}",
            hvac.runtime.plf_cooling_degradation_coeff
        );
    }

    #[test]
    fn high_hspf_single_speed_overrides_cd_on_init() {
        // HSPF >= 7 → Cd = 0.11 (applies to both PLF coeff and startup ramp).
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .test_extras_mut()
            .insert("rated_hspf".to_string(), 8.0.into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert!(
            (hvac.runtime.plf_cooling_degradation_coeff - 0.11).abs() < 1e-12,
            "high-HSPF Cd must be 0.11, got {}",
            hvac.runtime.plf_cooling_degradation_coeff
        );
        assert!(
            (hvac.runtime.startup.c_d - 0.11).abs() < 1e-12,
            "startup c_d must also be 0.11 for high-HSPF: {}",
            hvac.runtime.startup.c_d
        );
    }

    #[test]
    fn two_speed_defaults_cd_to_0_11_on_init() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .test_extras_mut()
            .insert("speed_control_mode".to_string(), "two_speed".into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert!(
            (hvac.runtime.plf_cooling_degradation_coeff - 0.11).abs() < 1e-12,
            "two-speed must default Cd to 0.11, got {}",
            hvac.runtime.plf_cooling_degradation_coeff
        );
        assert!(
            (hvac.runtime.startup.c_d - 0.11).abs() < 1e-12,
            "two-speed startup c_d must be 0.11: {}",
            hvac.runtime.startup.c_d
        );
    }

    #[test]
    fn variable_speed_defaults_cd_to_zero_on_init() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .test_extras_mut()
            .insert("speed_control_mode".to_string(), "variable".into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert_eq!(
            hvac.runtime.startup.c_d, 0.0,
            "variable-speed must default startup c_d to 0.0"
        );
    }

    #[test]
    fn explicit_cd_config_overrides_derived_defaults() {
        // When "cooling_cd" is explicitly set, rating-based overrides must not apply.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .test_extras_mut()
            .insert("cooling_cd".to_string(), 0.15.into());
        config
            .test_extras_mut()
            .insert("rated_seer".to_string(), 10.0.into()); // would give 0.20 if derived
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert!(
            (hvac.runtime.plf_cooling_degradation_coeff - 0.15).abs() < 1e-12,
            "explicit cooling_cd must not be overridden by SEER table"
        );
        assert!(
            (hvac.runtime.startup.c_d - 0.15).abs() < 1e-12,
            "startup c_d must also respect explicit cooling_cd"
        );
    }

    #[test]
    fn biquadratic_bounds_loaded_from_config_override_defaults() {
        // OCHRE HVAC.py: biquadratic CSV specifies min_Twb/max_Twb/min_Tdb/max_Tdb.
        // Verify config keys override the ±100°C fallback defaults.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .test_extras_mut()
            .insert("biquadratic_x1_min".to_string(), 12.0.into());
        config
            .test_extras_mut()
            .insert("biquadratic_x1_max".to_string(), 30.0.into());
        config
            .test_extras_mut()
            .insert("biquadratic_x2_min".to_string(), (-15.0).into());
        config
            .test_extras_mut()
            .insert("biquadratic_x2_max".to_string(), 55.0.into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");

        assert_eq!(hvac.config.biquadratic_x1_bounds, (12.0, 30.0));
        assert_eq!(hvac.config.biquadratic_x2_bounds, (-15.0, 55.0));

        // Evaluation must clamp: x1=5.0 → 12.0, x2=60.0 → 55.0
        hvac.config.biquadratic_coeffs = vec![[0.0, 1.0, 0.0, 1.0, 0.0, 0.0]];
        let result = hvac.evaluate_biquadratic(0, 5.0, 60.0);
        let expected = 12.0 + 55.0; // clamped x1 + clamped x2
        assert!(
            (result - expected).abs() < 1e-9,
            "config bounds must clamp inputs; expected {expected}, got {result}"
        );
    }

    #[test]
    fn plf_formula_applies_rating_dependent_cd() {
        // Verify the PLF = 1 - Cd * (1 - PLR) formula produces correct values
        // for the three rating-dependent Cd values:
        //   default Cd=0.25, low-SEER Cd=0.2, high-HSPF Cd=0.11
        let plf = |cd: f64, plr: f64| 1.0 - cd * (1.0 - plr);
        let plr = 0.6;

        // Default AHRI Cd = 0.25
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.runtime.plf_cooling_degradation_coeff = 0.25;
        assert!((hvac.part_load_factor(plr) - plf(0.25, plr)).abs() < 1e-12);

        // Low-SEER Cd = 0.2
        hvac.runtime.plf_cooling_degradation_coeff = 0.2;
        assert!((hvac.part_load_factor(plr) - plf(0.2, plr)).abs() < 1e-12);

        // High-HSPF Cd = 0.11
        hvac.runtime.plf_cooling_degradation_coeff = 0.11;
        assert!((hvac.part_load_factor(plr) - plf(0.11, plr)).abs() < 1e-12);
    }

    // --- Feature: PLF cycling degradation floor ---

    #[test]
    fn plf_floor_prevents_plf_below_0_7() {
        // EnergyPlus constraint: PLF >= 0.7 regardless of curve.
        // With Cd=1.0 (extreme) and PLR=0.0: PLF_raw = 0.0, must be floored to 0.7.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.runtime.plf_cooling_degradation_coeff = 1.0;
        let plf = hvac.part_load_factor(0.0);
        assert!(
            plf >= 0.7,
            "PLF must be >= 0.7 (floor); got {plf} with Cd=1.0, PLR=0.0"
        );
    }

    #[test]
    fn plf_floor_prevents_plf_below_plr() {
        // EnergyPlus constraint: PLF >= PLR to keep RTF = PLR/PLF <= 1.0.
        // With Cd=0.5 and PLR=0.8: PLF_raw = 1 - 0.5*0.2 = 0.9 >= PLR, no change.
        // With Cd=0.5 and PLR=0.75: PLF_raw = 1 - 0.5*0.25 = 0.875 >= PLR, no change.
        // Edge: PLR clamped to 1.0, PLF_raw = 1.0 by formula, max stays 1.0.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.runtime.plf_cooling_degradation_coeff = 0.25;
        for &plr in &[0.0, 0.3, 0.5, 0.7, 0.9, 1.0] {
            let plf = hvac.part_load_factor(plr);
            assert!(
                plf >= plr,
                "PLF ({plf:.4}) must be >= PLR ({plr:.4}); RTF = PLR/PLF must be <= 1.0"
            );
            let rtf = (plr / plf.max(f64::EPSILON)).min(1.0);
            assert!(rtf <= 1.0, "RTF ({rtf:.4}) must be <= 1.0 at PLR={plr:.4}");
        }
    }

    #[test]
    fn plf_normal_range_not_affected_by_floor() {
        // Standard AHRI Cd=0.25 produces PLF well above 0.7 at any PLR >= 0.
        // PLF_raw at PLR=0.0: 1 - 0.25 = 0.75 > 0.7 -- floor has no effect.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.runtime.plf_cooling_degradation_coeff = 0.25;
        let plf_at_zero = hvac.part_load_factor(0.0);
        assert!(
            (plf_at_zero - 0.75).abs() < 1e-12,
            "Cd=0.25 at PLR=0.0 must give PLF=0.75; got {plf_at_zero}"
        );
    }

    // --- Feature: compressor minimum on/off time ---

    #[test]
    fn can_transition_mode_allows_when_disabled() {
        // With defaults (min_on_time_s=0, min_off_time_s=0), all transitions
        // are always allowed regardless of elapsed time.
        let hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let t0 = FixedOffset::east_opt(0)
            .expect("UTC offset")
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .expect("valid");
        // No mode_start_at → always allowed
        assert!(
            hvac.thermostat_fsm
                .can_transition_mode(ThermostatMode::Heating, t0)
        );
        assert!(
            hvac.thermostat_fsm
                .can_transition_mode(ThermostatMode::Cooling, t0)
        );
        assert!(
            hvac.thermostat_fsm
                .can_transition_mode(ThermostatMode::Deadband, t0)
        );
    }

    #[test]
    fn min_on_time_blocks_early_shutdown() {
        // With min_on_time_s=120s: transition from Heating→Deadband is blocked
        // until 120 s have elapsed in Heating mode.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.thermostat_fsm.min_on_time_s = 120.0;

        let t0 = FixedOffset::east_opt(0)
            .expect("UTC offset")
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .expect("valid");
        // Simulate entering Heating mode at t0.
        hvac.thermostat_fsm.mode = ThermostatMode::Heating;
        hvac.thermostat_fsm.mode_start_at = Some(t0);

        // At 60 s: still blocked.
        let t60 = t0 + ChronoDuration::seconds(60);
        assert!(
            !hvac
                .thermostat_fsm
                .can_transition_mode(ThermostatMode::Deadband, t60),
            "min_on_time_s=120 should block Heating→Deadband at 60s"
        );

        // At exactly 120 s: allowed (elapsed >= min_s).
        let t120 = t0 + ChronoDuration::seconds(120);
        assert!(
            hvac.thermostat_fsm
                .can_transition_mode(ThermostatMode::Deadband, t120),
            "min_on_time_s=120 should allow Heating→Deadband at 120s"
        );
    }

    #[test]
    fn min_off_time_blocks_early_restart() {
        // With min_off_time_s=180s: transition from Deadband→Heating is blocked
        // until 180 s have elapsed in Deadband mode.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.thermostat_fsm.min_off_time_s = 180.0;

        let t0 = FixedOffset::east_opt(0)
            .expect("UTC offset")
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .expect("valid");
        hvac.thermostat_fsm.mode = ThermostatMode::Deadband;
        hvac.thermostat_fsm.mode_start_at = Some(t0);

        // At 90 s: blocked.
        let t90 = t0 + ChronoDuration::seconds(90);
        assert!(
            !hvac
                .thermostat_fsm
                .can_transition_mode(ThermostatMode::Heating, t90),
            "min_off_time_s=180 should block Deadband→Heating at 90s"
        );

        // At exactly 180 s: allowed.
        let t180 = t0 + ChronoDuration::seconds(180);
        assert!(
            hvac.thermostat_fsm
                .can_transition_mode(ThermostatMode::Heating, t180),
            "min_off_time_s=180 should allow Deadband→Heating at 180s"
        );
    }

    #[test]
    fn update_mode_respects_min_on_time_when_configured() {
        // Wire the compressor minimum on-time through the full update_mode path.
        // Equipment with min_on_time_s=120 must stay Heating even when zone is warm.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.thermostat_fsm.thermostat.hysteresis_c = 1.0;
        hvac.thermostat_fsm.thermostat.cutout_ratio = 0.5;
        hvac.thermostat_fsm.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 25.0,
        };
        hvac.thermostat_fsm.min_on_time_s = 120.0;

        // Step 1: cold zone → enters Heating mode.
        let mode = hvac.update_mode(&env(18.0, 60, 0)).expect("mode updates");
        assert_eq!(mode, ThermostatMode::Heating, "should enter Heating");

        // Step 2 at 60s: zone is warm (above cutout), thermostat wants Deadband,
        // but min_on_time_s=120 blocks the transition.
        let mode = hvac
            .update_mode(&env(22.0, 60, 60))
            .expect("locked by min_on_time");
        assert_eq!(
            mode,
            ThermostatMode::Heating,
            "min_on_time_s=120 must hold Heating at 60s"
        );

        // Step 3 at 120s: min_on_time has elapsed, transition is allowed.
        let mode = hvac.update_mode(&env(22.0, 60, 120)).expect("unlocked");
        assert_eq!(
            mode,
            ThermostatMode::Deadband,
            "min_on_time_s=120 must release Heating at 120s"
        );
    }

    #[test]
    fn same_mode_transition_always_allowed() {
        // can_transition_mode returns true when proposed == current (no transition).
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.thermostat_fsm.min_on_time_s = 9999.0;
        hvac.thermostat_fsm.mode = ThermostatMode::Heating;
        let t0 = FixedOffset::east_opt(0)
            .expect("UTC offset")
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .expect("valid");
        hvac.thermostat_fsm.mode_start_at = Some(t0);

        // Propose same mode -- no transition needed, always allowed.
        assert!(
            hvac.thermostat_fsm
                .can_transition_mode(ThermostatMode::Heating, t0),
            "same-mode 'transition' must always be allowed"
        );
    }

    #[test]
    fn biquadratic_parser_handles_negative_coefficients() {
        let input = "-1.0, -2.5, 3.0, -0.001, 0.5, -0.02";
        let result = super::super::core_config::parse_biquadratic_list(input)
            .expect("should parse successfully");
        assert_eq!(result.len(), 1, "should produce exactly one curve");
        let coeffs = result[0];
        let expected = [-1.0_f64, -2.5, 3.0, -0.001, 0.5, -0.02];
        for (i, (&got, &exp)) in coeffs.iter().zip(expected.iter()).enumerate() {
            assert!(
                (got - exp).abs() < 1e-9,
                "coefficient {i}: expected {exp}, got {got}"
            );
        }
    }

    #[test]
    fn parse_biquadratic_list_empty_input_is_error() {
        let result = super::super::core_config::parse_biquadratic_list("");
        assert!(result.is_err(), "empty string must be an error");
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("zero parseable coefficients"),
            "error message must mention zero coefficients, got: {msg}"
        );
    }

    #[test]
    fn parse_biquadratic_list_no_parseable_numbers_is_error() {
        let result = super::super::core_config::parse_biquadratic_list("abc, xyz, !@#");
        assert!(
            result.is_err(),
            "input with no parseable numbers must be an error, not silently return identity"
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("zero parseable coefficients"),
            "error message must mention zero coefficients, got: {msg}"
        );
    }

    // --- Feature: DSE routing via zone_heat_fractions ---

    #[test]
    fn update_zone_heat_fractions_dse_one_produces_single_full_fraction() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.config.duct_dse = 1.0;
        hvac.config.duct_zone_id = None;
        hvac.update_zone_heat_fractions();
        assert_eq!(hvac.config.zone_heat_fractions, vec![(ZoneId(1), 1.0)]);
    }

    #[test]
    fn update_zone_heat_fractions_dse_below_one_no_duct_zone() {
        // Without a duct zone the conditioned-zone fraction equals DSE; duct losses
        // are unrecoverable (lost to outdoors). OCHRE HVAC.py line 190-192.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.config.duct_dse = 0.7;
        hvac.config.duct_zone_id = None;
        hvac.update_zone_heat_fractions();
        assert_eq!(hvac.config.zone_heat_fractions, vec![(ZoneId(1), 0.7)]);
    }

    #[test]
    fn update_zone_heat_fractions_dse_below_one_with_duct_zone() {
        // With a duct zone specified: conditioned gets DSE, duct zone gets 1-DSE.
        // OCHRE HVAC.py lines 190-192: zone_fractions[duct_zone] = 1 - duct_dse.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.config.duct_dse = 0.7;
        hvac.config.duct_zone_id = Some(ZoneId(2));
        hvac.update_zone_heat_fractions();
        assert_eq!(hvac.config.zone_heat_fractions.len(), 2);
        assert!((hvac.config.zone_heat_fractions[0].1 - 0.7).abs() < 1e-12);
        assert!((hvac.config.zone_heat_fractions[1].1 - 0.3).abs() < 1e-12);
        assert_eq!(hvac.config.zone_heat_fractions[0].0, ZoneId(1));
        assert_eq!(hvac.config.zone_heat_fractions[1].0, ZoneId(2));
    }

    #[test]
    fn write_zone_thermal_contributions_routes_duct_loss_to_duct_zone() {
        // With DSE=0.7 and a duct zone: 10_000 W gross → 7_000 W conditioned,
        // 3_000 W to duct zone.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.config.duct_dse = 0.7;
        hvac.config.duct_zone_id = Some(ZoneId(2));
        hvac.update_zone_heat_fractions();

        let mut ports = PortSlots {
            thermal: vec![
                ThermalAccumulator::new(ZoneId(1)),
                ThermalAccumulator::new(ZoneId(2)),
            ],
            ..PortSlots::default()
        };
        hvac.write_zone_thermal_contributions(
            &mut ports,
            10_000.0,
            0.0,
            ThermalCategory::HvacHeating,
        )
        .expect("write ok");

        assert!(
            (ports.thermal[0].sensible_gain_w - 7_000.0).abs() < 1e-9,
            "conditioned zone must get gross * DSE = 7_000 W"
        );
        assert!(
            (ports.thermal[1].sensible_gain_w - 3_000.0).abs() < 1e-9,
            "duct zone must get gross * (1-DSE) = 3_000 W"
        );
        // Conditioned zone keeps the caller's category; duct zone gets DuctLoss.
        assert!(
            ports.thermal[0].sensible_for_category(ThermalCategory::HvacHeating) > 0.0,
            "conditioned zone must be tagged HvacHeating"
        );
        assert!(
            (ports.thermal[0].sensible_for_category(ThermalCategory::DuctLoss)).abs() < 1e-12,
            "conditioned zone must NOT have DuctLoss category"
        );
        assert!(
            (ports.thermal[1].sensible_for_category(ThermalCategory::DuctLoss) - 3_000.0).abs()
                < 1e-9,
            "duct zone must be tagged DuctLoss, got {}",
            ports.thermal[1].sensible_for_category(ThermalCategory::DuctLoss)
        );
        assert!(
            ports.thermal[1]
                .sensible_for_category(ThermalCategory::HvacHeating)
                .abs()
                < 1e-12,
            "duct zone must NOT have HvacHeating category"
        );
    }

    #[test]
    fn write_zone_thermal_contributions_discards_duct_loss_when_no_duct_zone() {
        // Without a duct zone, only the conditioned zone receives heat.
        // Duct loss (1-DSE) is discarded (unrecoverable).
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.config.duct_dse = 0.7;
        hvac.config.duct_zone_id = None;
        hvac.update_zone_heat_fractions();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hvac.write_zone_thermal_contributions(
            &mut ports,
            10_000.0,
            0.0,
            ThermalCategory::HvacHeating,
        )
        .expect("write ok");

        assert!(
            (ports.thermal[0].sensible_gain_w - 7_000.0).abs() < 1e-9,
            "conditioned zone must get gross * DSE = 7_000 W; duct loss discarded"
        );
    }

    #[test]
    fn write_zone_thermal_contributions_duct_zone_same_as_conditioned_merges() {
        // When duct_zone == zone_id the ducts are inside the conditioned space.
        // Duct losses loop back into the same zone, so effective DSE = 1.0 and
        // all gross capacity is delivered to the conditioned zone.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.config.duct_dse = 0.7;
        hvac.config.duct_zone_id = Some(ZoneId(1)); // same as conditioned zone
        hvac.update_zone_heat_fractions();
        // Only one entry since duct_zone == zone_id.
        assert_eq!(hvac.config.zone_heat_fractions.len(), 1);
        assert!(
            (hvac.config.zone_heat_fractions[0].1 - 1.0).abs() < 1e-12,
            "conditioned-zone fraction must be 1.0 when ducts are inside the conditioned space"
        );
    }

    #[test]
    fn update_zone_heat_fractions_basement_frac_splits_conditioned_and_basement() {
        // OCHRE HVAC.py lines 188-197: basement_heat_frac routes a fraction of
        // DSE-adjusted capacity to the basement zone.
        // DSE=0.8, basement_frac=0.2:
        //   conditioned = 0.8 * (1 - 0.2) = 0.64
        //   basement    = 0.8 * 0.2        = 0.16
        //   duct zone   = 1 - 0.8          = 0.20
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.config.duct_dse = 0.8;
        hvac.config.duct_zone_id = Some(ZoneId(3));
        hvac.config.basement_heat_frac = 0.2;
        hvac.config.basement_zone_id = Some(ZoneId(2));
        hvac.update_zone_heat_fractions();

        assert_eq!(hvac.config.zone_heat_fractions.len(), 3);
        let fracs: std::collections::HashMap<ZoneId, f64> =
            hvac.config.zone_heat_fractions.iter().copied().collect();
        assert!(
            (fracs[&ZoneId(1)] - 0.64).abs() < 1e-12,
            "conditioned zone expected 0.64, got {}",
            fracs[&ZoneId(1)]
        );
        assert!(
            (fracs[&ZoneId(2)] - 0.16).abs() < 1e-12,
            "basement zone expected 0.16, got {}",
            fracs[&ZoneId(2)]
        );
        assert!(
            (fracs[&ZoneId(3)] - 0.20).abs() < 1e-12,
            "duct zone expected 0.20, got {}",
            fracs[&ZoneId(3)]
        );
    }

    #[test]
    fn init_loads_basement_zone_id_and_airflow_ratio_from_config() {
        // basement_zone_id and basement_airflow_ratio config keys must be picked
        // up by HvacEquipment::init() so that subsequent update_zone_heat_fractions
        // calls route heat correctly without extra setup.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .test_extras_mut()
            .insert("basement_zone_id".to_string(), 4.0.into());
        config
            .test_extras_mut()
            .insert("basement_airflow_ratio".to_string(), 0.2.into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert_eq!(hvac.config.basement_zone_id, Some(ZoneId(4)));
        assert!((hvac.config.basement_heat_frac - 0.2).abs() < 1e-12);
    }

    // --- Change 1: TwoSpeedTime and TwoSpeedAlternating speed control ---

    #[test]
    fn two_speed_time_starts_low_and_escalates_on_continued_temperature_drop() {
        // OCHRE "Time" mode: start at low speed; switch to high if temperature
        // still moving away from setpoint after min_time_per_speed_s.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.speed_control_mode = SpeedControlMode::TwoSpeedTime;
        hvac.config.low_speed_capacity_fraction = 0.5;
        hvac.config.min_time_per_speed_s = 300.0;

        // Step 1: no prior temp → start at low speed.
        let sel = hvac.select_speed_with_zone_temp(0.8, Some(19.0), true);
        assert_eq!(sel.speed_index, 0, "first step must start at low speed");
        hvac.advance_speed_timer(300.0); // dwell min_time_per_speed_s at low

        // Step 2: temperature still dropping (19.0 → 18.5) → escalate to high.
        hvac.update_prev_zone_temp(Some(19.0));
        let sel = hvac.select_speed_with_zone_temp(0.8, Some(18.5), true);
        assert_eq!(
            sel.speed_index, 1,
            "temperature still dropping after min-time → must escalate to high speed"
        );
    }

    #[test]
    fn two_speed_time_does_not_escalate_if_temperature_recovering() {
        // If temperature is moving toward setpoint, keep current low speed.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.speed_control_mode = SpeedControlMode::TwoSpeedTime;
        hvac.config.low_speed_capacity_fraction = 0.5;
        hvac.config.min_time_per_speed_s = 300.0;
        hvac.update_prev_zone_temp(Some(19.0));

        // Dwell past min-time.
        hvac.advance_speed_timer(300.0);

        // Temperature rising (19.0 → 19.5): zone is recovering → stay at low.
        let sel = hvac.select_speed_with_zone_temp(0.8, Some(19.5), true);
        assert_eq!(
            sel.speed_index, 0,
            "temperature recovering → must stay at low speed"
        );
    }

    #[test]
    fn two_speed_alternating_always_selects_high_speed() {
        // OCHRE "Time2" mode: always run at high speed (index 1) when on.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.speed_control_mode = SpeedControlMode::TwoSpeedAlternating;
        hvac.config.low_speed_capacity_fraction = 0.5;

        let sel = hvac.select_speed(0.3);
        assert_eq!(
            sel.speed_index, 1,
            "TwoSpeedAlternating must always select high speed (index 1)"
        );
        let sel = hvac.select_speed(0.9);
        assert_eq!(
            sel.speed_index, 1,
            "TwoSpeedAlternating must always select high speed regardless of load"
        );
    }

    #[test]
    fn speed_control_mode_parses_time_and_alternating_from_config() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .test_extras_mut()
            .insert("speed_control_mode".to_string(), "time".into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert_eq!(
            hvac.config.speed_control_mode,
            SpeedControlMode::TwoSpeedTime
        );

        let mut config2 = EquipmentConfig::default();
        config2
            .test_extras_mut()
            .insert("speed_control_mode".to_string(), "time2".into());
        hvac.init(&config2, &env(20.0, 60, 0)).expect("init ok");
        assert_eq!(
            hvac.config.speed_control_mode,
            SpeedControlMode::TwoSpeedAlternating
        );
    }

    #[test]
    fn speed_control_mode_rejects_unrecognised_text_value() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .test_extras_mut()
            .insert("speed_control_mode".to_string(), "foobar".into());
        let err = hvac.init(&config, &env(20.0, 60, 0)).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("foobar"),
            "error must name the unrecognised value, got: {msg}"
        );
        assert!(
            msg.contains("unrecognised"),
            "error must indicate the value was unrecognised, got: {msg}"
        );
    }

    #[test]
    fn speed_control_mode_rejects_unrecognised_numeric_value() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .test_extras_mut()
            .insert("speed_control_mode".to_string(), 99.0_f64.into());
        let err = hvac.init(&config, &env(20.0, 60, 0)).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("99"),
            "error must name the unrecognised value, got: {msg}"
        );
        assert!(
            msg.contains("unrecognised"),
            "error must indicate the value was unrecognised, got: {msg}"
        );
    }

    #[test]
    fn speed_control_mode_defaults_to_single_speed_when_absent() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let config = EquipmentConfig::default();
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert_eq!(
            hvac.config.speed_control_mode,
            SpeedControlMode::SingleSpeed,
            "absent speed_control_mode must default to SingleSpeed"
        );
    }

    // --- Change 2: Per-speed biquadratic PLR curves ---

    #[test]
    fn eir_plr_curve_replaces_cd_formula_when_configured() {
        // PLR curve: PLF = 0.85 + 0.15 * PLR + 0.0 * PLR²
        // At PLR=0.6: PLF = 0.85 + 0.09 = 0.94
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.eir_plr_coefficients = Some(vec![[0.85, 0.15, 0.0]]);
        let plf = hvac.part_load_factor(0.6);
        assert!(
            (plf - 0.94).abs() < 1e-12,
            "per-speed PLR curve PLF must be 0.94 at PLR=0.6; got {plf}"
        );
    }

    #[test]
    fn eir_plr_falls_back_to_cd_when_not_configured() {
        // Without per-speed curves, PLF = 1 - Cd * (1 - PLR).
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.eir_plr_coefficients = None;
        hvac.runtime.plf_cooling_degradation_coeff = 0.25;
        let plf = hvac.part_load_factor(0.5);
        assert!(
            (plf - 0.875).abs() < 1e-12,
            "fallback Cd=0.25 at PLR=0.5 must give PLF=0.875; got {plf}"
        );
    }

    #[test]
    fn eir_plr_per_speed_selects_correct_curve_by_stage() {
        // Two curves: stage 0 = [0.9, 0.1, 0], stage 1 = [0.8, 0.2, 0].
        // At PLR=0.5: stage 0 → PLF=0.95, stage 1 → PLF=0.90.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.eir_plr_coefficients = Some(vec![[0.9, 0.1, 0.0], [0.8, 0.2, 0.0]]);

        hvac.runtime.last_speed_index = 0;
        let plf0 = hvac.part_load_factor(0.5);
        assert!(
            (plf0 - 0.95).abs() < 1e-12,
            "stage 0 PLF at PLR=0.5 must be 0.95; got {plf0}"
        );

        hvac.runtime.last_speed_index = 1;
        let plf1 = hvac.part_load_factor(0.5);
        assert!(
            (plf1 - 0.90).abs() < 1e-12,
            "stage 1 PLF at PLR=0.5 must be 0.90; got {plf1}"
        );
    }

    #[test]
    fn eir_plr_curve_clamped_to_min_0_7() {
        // Even with a curve that produces < 0.7, PLF must be floored to 0.7.
        // Curve: PLF = 0.5 + 0.1 * PLR at PLR=0 → 0.5 (below floor).
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.eir_plr_coefficients = Some(vec![[0.5, 0.1, 0.0]]);
        let plf = hvac.part_load_factor(0.0);
        assert!(
            plf >= 0.7,
            "PLF from curve must be clamped to at least 0.7; got {plf}"
        );
    }

    // --- Change 3: Speed disabling for demand response ---

    #[test]
    fn set_disabled_speeds_routes_to_max_enabled_when_desired_disabled() {
        // OCHRE HVAC.py lines 906–909: disabled speed → highest allowed speed.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.speed_control_mode = SpeedControlMode::TwoSpeedSetpoint;
        hvac.config.low_speed_capacity_fraction = 0.5;
        hvac.config.min_time_per_speed_s = 0.0;

        // Disable high speed (index 1).
        hvac.set_disabled_speeds(&[false, true]);
        assert_eq!(
            hvac.control.max_enabled_speed, 0,
            "highest non-disabled is index 0"
        );

        // Load fraction above threshold would normally select high speed.
        let sel = hvac.select_speed(0.8);
        assert_eq!(
            sel.speed_index, 0,
            "high speed disabled → must route to index 0 (max enabled)"
        );
    }

    #[test]
    fn set_disabled_speeds_empty_re_enables_all() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.speed_control_mode = SpeedControlMode::TwoSpeedSetpoint;
        hvac.config.low_speed_capacity_fraction = 0.5;
        hvac.config.min_time_per_speed_s = 0.0;

        hvac.set_disabled_speeds(&[false, true]);
        let sel = hvac.select_speed(0.8);
        assert_eq!(sel.speed_index, 0, "high speed disabled initially");

        // Re-enable all.
        hvac.set_disabled_speeds(&[]);
        let sel = hvac.select_speed(0.8);
        assert_eq!(
            sel.speed_index, 1,
            "after re-enabling, high speed must be selectable again"
        );
    }

    #[test]
    fn n_speed_stages_matches_mode() {
        for (mode, expected) in [
            (SpeedControlMode::SingleSpeed, 1),
            (SpeedControlMode::TwoSpeedSetpoint, 2),
            (SpeedControlMode::TwoSpeedTime, 2),
            (SpeedControlMode::TwoSpeedAlternating, 2),
            (SpeedControlMode::VariableSpeedIdeal, 1),
        ] {
            let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
            hvac.config.speed_control_mode = mode;
            assert_eq!(
                hvac.n_speed_stages(),
                expected,
                "{mode:?} must have {expected} speed stages"
            );
        }
        // MultiSpeedInterpolated derives stage count from capacities.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.speed_control_mode = SpeedControlMode::MultiSpeedInterpolated;
        hvac.config.heating_capacities_w = vec![4000.0, 6000.0, 8000.0, 10000.0];
        assert_eq!(
            hvac.n_speed_stages(),
            4,
            "MultiSpeedInterpolated with 4 stages"
        );
    }

    // --- Change 4: Deadband offset (asymmetric setpoint bands) ---

    #[test]
    fn deadband_offset_defaults_to_0_2() {
        let hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        assert!(
            (hvac.thermostat_fsm.thermostat.deadband_offset - 0.2).abs() < 1e-12,
            "default deadband_offset must be 0.2 (OCHRE default)"
        );
    }

    #[test]
    fn deadband_offset_loaded_from_config() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .test_extras_mut()
            .insert("deadband_offset".to_string(), 0.3.into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert!(
            (hvac.thermostat_fsm.thermostat.deadband_offset - 0.3).abs() < 1e-12,
            "deadband_offset must be loaded from config"
        );
    }

    #[test]
    fn deadband_offset_asymmetric_heating_turn_on_threshold() {
        // With setpoint=20°C, hysteresis=1°C, offset=0.2:
        //   turn_on  = 20 - 1×(1-0.2) = 20 - 0.8 = 19.2°C
        //   turn_off = 20 + 1×0.2     = 20.2°C
        // At 19.3°C the furnace must be in Deadband.
        // At 19.1°C it must enter Heating.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.thermostat_fsm.thermostat.hysteresis_c = 1.0;
        hvac.thermostat_fsm.thermostat.deadband_offset = 0.2;
        hvac.thermostat_fsm.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 26.0,
        };

        // 19.3°C: above turn_on threshold (19.2°C) → Deadband.
        let mode = hvac.update_mode(&env(19.3, 60, 0)).expect("mode");
        assert_eq!(
            mode,
            ThermostatMode::Deadband,
            "19.3°C is above turn-on 19.2°C → must stay Deadband"
        );

        // 19.1°C: below turn_on threshold → Heating.
        let mode = hvac.update_mode(&env(19.1, 60, 1)).expect("mode");
        assert_eq!(
            mode,
            ThermostatMode::Heating,
            "19.1°C is below turn-on 19.2°C → must enter Heating"
        );

        // Heating turn_off at 20.2°C.
        let mode = hvac.update_mode(&env(20.3, 60, 2)).expect("mode");
        assert_eq!(
            mode,
            ThermostatMode::Deadband,
            "20.3°C exceeds heating turn-off 20.2°C → must return to Deadband"
        );
    }

    #[test]
    fn deadband_offset_zero_uses_legacy_symmetric_hysteresis() {
        // With offset=0.0 the legacy symmetric path is used:
        //   turn_on  = setpoint - hysteresis
        //   turn_off = setpoint + hysteresis × cutout (cutout=0 → at setpoint)
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.thermostat_fsm.thermostat.hysteresis_c = 1.0;
        hvac.thermostat_fsm.thermostat.deadband_offset = 0.0;
        hvac.thermostat_fsm.thermostat.cutout_ratio = 0.0;
        hvac.thermostat_fsm.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 26.0,
        };

        // Below 19°C → Heating.
        let mode = hvac.update_mode(&env(18.9, 60, 0)).expect("mode");
        assert_eq!(mode, ThermostatMode::Heating, "18.9°C < 19°C → Heating");

        // Any temperature at or above setpoint and cutout=0 → Deadband immediately.
        let mode = hvac.update_mode(&env(20.1, 60, 1)).expect("mode");
        assert_eq!(
            mode,
            ThermostatMode::Deadband,
            "20.1°C > 20°C (cutout=0) → Deadband"
        );
    }

    /// `TwoSpeedTime` must restart at low speed (index 0) on a new heating cycle,
    /// even if the previous cycle ended at high speed.
    ///
    /// OCHRE HVAC.py "Time" mode: `prev_zone_temp_c = None` when the unit shuts off,
    /// so the next `select_speed_with_zone_temp` call takes the `else { 0 }` branch
    /// and returns index 0 regardless of `last_speed_index`.
    #[test]
    fn two_speed_time_restarts_at_low_speed_after_off_cycle() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.speed_control_mode = SpeedControlMode::TwoSpeedTime;
        hvac.config.low_speed_capacity_fraction = 0.5;
        hvac.config.min_time_per_speed_s = 300.0;

        // Simulate a completed cycle: HVAC reached high speed (index 1).
        hvac.update_prev_zone_temp(Some(19.0));
        hvac.advance_speed_timer(300.0);
        let sel = hvac.select_speed_with_zone_temp(0.8, Some(18.5), true);
        assert_eq!(
            sel.speed_index, 1,
            "precondition: cycle escalated to high speed"
        );

        // HVAC turns off: clear prev_zone_temp so the next on-cycle starts fresh.
        hvac.update_prev_zone_temp(None);

        // Re-enable: first speed selection with no prior temp must start at low speed,
        // regardless of last_speed_index still being 1.
        let sel = hvac.select_speed_with_zone_temp(0.8, Some(18.5), true);
        assert_eq!(
            sel.speed_index, 0,
            "TwoSpeedTime must start at low speed (0) on re-enable; got {}",
            sel.speed_index
        );
    }

    #[test]
    fn deadband_offset_validation_rejects_out_of_range() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .test_extras_mut()
            .insert("deadband_offset".to_string(), 1.5.into());
        let err = hvac
            .init(&config, &env(20.0, 60, 0))
            .expect_err("must fail for deadband_offset > 1.0");
        assert!(
            err.to_string().contains("deadband_offset"),
            "error must mention deadband_offset"
        );
    }

    /// Regression test for bug: HvacEquipment::init only read the combined
    /// "biquadratic_coeffs" key and ignored the split "capacity_biquadratic_coeffs"
    /// and "eir_biquadratic_coeffs" keys that the HPXML resolver writes.
    /// When only split keys were provided the HP heater got identity curves.
    #[test]
    fn split_biquadratic_keys_are_applied_during_init() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::AshpHeatPumpOnly, ZoneId(1));
        let mut config = EquipmentConfig::default();
        // Provide only the split keys, not the combined key.
        config.test_extras_mut().insert(
            "capacity_biquadratic_coeffs".to_string(),
            "2.0,0.0,0.0,0.0,0.0,0.0".into(),
        );
        config.test_extras_mut().insert(
            "eir_biquadratic_coeffs".to_string(),
            "3.0,0.0,0.0,0.0,0.0,0.0".into(),
        );
        hvac.init(&config, &env(20.0, 60, 0))
            .expect("init must succeed");
        assert!(
            hvac.config.biquadratic_coeffs.len() >= 2,
            "must have at least two curves after split-key init"
        );
        assert!(
            (hvac.config.biquadratic_coeffs[0][0] - 2.0).abs() < 1e-12,
            "capacity curve intercept must be 2.0, got {}",
            hvac.config.biquadratic_coeffs[0][0]
        );
        assert!(
            (hvac.config.biquadratic_coeffs[1][0] - 3.0).abs() < 1e-12,
            "EIR curve intercept must be 3.0, got {}",
            hvac.config.biquadratic_coeffs[1][0]
        );
    }

    #[test]
    fn mode_reversal_heating_to_cooling_via_deadband() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.thermostat_fsm.thermostat.hysteresis_c = 1.0;
        hvac.thermostat_fsm.thermostat.deadband_offset = 0.2;
        hvac.thermostat_fsm.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 25.0,
        };

        let mode = hvac.update_mode(&env(18.0, 60, 0)).expect("mode");
        assert_eq!(mode, ThermostatMode::Heating, "18°C < 19.2 → Heating");

        let mode = hvac.update_mode(&env(21.0, 60, 61)).expect("mode");
        assert_eq!(mode, ThermostatMode::Deadband, "21°C > 20.2 → Deadband");

        let mode = hvac.update_mode(&env(26.0, 60, 122)).expect("mode");
        assert_eq!(mode, ThermostatMode::Cooling, "26°C > 25.8 → Cooling");
    }

    #[test]
    fn mode_reversal_cooling_to_heating_via_deadband() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.thermostat_fsm.thermostat.hysteresis_c = 1.0;
        hvac.thermostat_fsm.thermostat.deadband_offset = 0.2;
        hvac.thermostat_fsm.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 25.0,
        };

        let mode = hvac.update_mode(&env(27.0, 60, 0)).expect("mode");
        assert_eq!(mode, ThermostatMode::Cooling, "27°C > 25.8 → Cooling");

        let mode = hvac.update_mode(&env(24.0, 60, 61)).expect("mode");
        assert_eq!(mode, ThermostatMode::Deadband, "24°C < 24.8 → Deadband");

        let mode = hvac.update_mode(&env(18.0, 60, 122)).expect("mode");
        assert_eq!(mode, ThermostatMode::Heating, "18°C < 19.2 → Heating");
    }

    #[test]
    fn min_on_and_off_time_stacking() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.thermostat_fsm.thermostat.hysteresis_c = 1.0;
        hvac.thermostat_fsm.thermostat.deadband_offset = 0.2;
        hvac.thermostat_fsm.thermostat.min_cycle_time_s = 0.0;
        hvac.thermostat_fsm.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 25.0,
        };
        hvac.thermostat_fsm.min_on_time_s = 120.0;
        hvac.thermostat_fsm.min_off_time_s = 180.0;

        let mode = hvac.update_mode(&env(18.0, 60, 0)).expect("mode");
        assert_eq!(mode, ThermostatMode::Heating, "cold zone → Heating");

        let mode = hvac.update_mode(&env(30.0, 60, 60)).expect("mode");
        assert_eq!(
            mode,
            ThermostatMode::Heating,
            "min_on=120 not elapsed → still Heating"
        );

        let mode = hvac.update_mode(&env(30.0, 60, 121)).expect("mode");
        assert_eq!(
            mode,
            ThermostatMode::Deadband,
            "min_on elapsed at 121s → Deadband"
        );

        let mode = hvac.update_mode(&env(15.0, 60, 150)).expect("mode");
        assert_eq!(
            mode,
            ThermostatMode::Deadband,
            "only 29s in Deadband → min_off blocks restart"
        );

        let mode = hvac.update_mode(&env(15.0, 60, 302)).expect("mode");
        assert_eq!(
            mode,
            ThermostatMode::Heating,
            "181s in Deadband > min_off=180 → Heating"
        );
    }

    #[test]
    fn two_speed_time_escalates_when_temp_moving_wrong_way() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.speed_control_mode = SpeedControlMode::TwoSpeedTime;
        hvac.config.low_speed_capacity_fraction = 0.5;
        hvac.config.min_time_per_speed_s = 60.0;

        let sel = hvac.select_speed_with_zone_temp(0.8, None, true);
        assert_eq!(sel.speed_index, 0, "fresh cycle → low speed");

        hvac.update_prev_zone_temp(Some(20.0));
        hvac.advance_speed_timer(61.0);

        let sel = hvac.select_speed_with_zone_temp(0.8, Some(19.5), true);
        assert_eq!(
            sel.speed_index, 1,
            "temp dropped during heating, timer≥60s → escalate to high"
        );
    }

    #[test]
    fn two_speed_time_stays_low_when_temp_moving_right_way() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.speed_control_mode = SpeedControlMode::TwoSpeedTime;
        hvac.config.low_speed_capacity_fraction = 0.5;
        hvac.config.min_time_per_speed_s = 60.0;

        hvac.update_prev_zone_temp(Some(20.0));
        hvac.advance_speed_timer(61.0);

        let sel = hvac.select_speed_with_zone_temp(0.8, Some(20.5), true);
        assert_eq!(
            sel.speed_index, 0,
            "temp rising during heating → stay at low speed"
        );
    }

    #[test]
    fn two_speed_alternating_always_high_speed() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.speed_control_mode = SpeedControlMode::TwoSpeedAlternating;

        assert_eq!(
            hvac.select_speed(0.3).speed_index,
            1,
            "low load → high speed"
        );
        assert_eq!(
            hvac.select_speed(0.9).speed_index,
            1,
            "high load → high speed"
        );
    }

    #[test]
    fn all_disabled_speeds_falls_back_to_last() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.speed_control_mode = SpeedControlMode::TwoSpeedSetpoint;
        hvac.config.low_speed_capacity_fraction = 0.5;
        hvac.config.min_time_per_speed_s = 0.0;

        hvac.set_disabled_speeds(&[true, true]);
        assert_eq!(
            hvac.control.max_enabled_speed, 1,
            "all disabled → fallback to last (index 1)"
        );

        let sel = hvac.select_speed(0.3);
        assert_eq!(
            sel.speed_index, 1,
            "all disabled → select max_enabled (index 1)"
        );
    }

    #[test]
    fn eir_plr_per_stage_with_custom_quad_curves() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.eir_plr_coefficients = Some(vec![[0.8, 0.3, -0.1], [0.9, 0.15, -0.05]]);

        let plf0 = hvac.part_load_factor_for_stage(0.5, 0);
        assert!(
            (plf0 - 0.925).abs() < 1e-12,
            "stage 0 at PLR=0.5: 0.8 + 0.3×0.5 + (-0.1)×0.25 = 0.925; got {plf0}"
        );

        let plf1 = hvac.part_load_factor_for_stage(0.5, 1);
        assert!(
            (plf1 - 0.9625).abs() < 1e-12,
            "stage 1 at PLR=0.5: 0.9 + 0.15×0.5 + (-0.05)×0.25 = 0.9625; got {plf1}"
        );

        let plf_oob = hvac.part_load_factor_for_stage(0.5, 5);
        assert!(
            (plf_oob - 0.9625).abs() < 1e-12,
            "out-of-range stage → uses last curve (0.9625); got {plf_oob}"
        );
    }

    #[test]
    fn deadband_offset_zero_thresholds() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.thermostat_fsm.thermostat.hysteresis_c = 1.0;
        hvac.thermostat_fsm.thermostat.deadband_offset = 0.0;
        hvac.thermostat_fsm.thermostat.cutout_ratio = 0.5;
        hvac.thermostat_fsm.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 25.0,
        };

        let mode = hvac.update_mode(&env(19.5, 60, 0)).expect("mode");
        assert_eq!(
            mode,
            ThermostatMode::Deadband,
            "19.5°C > turn-on 19.0 → Deadband"
        );

        let mode = hvac.update_mode(&env(18.5, 60, 1)).expect("mode");
        assert_eq!(
            mode,
            ThermostatMode::Heating,
            "18.5°C < turn-on 19.0 → Heating"
        );
    }

    #[test]
    fn deadband_offset_half_thresholds() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.thermostat_fsm.thermostat.hysteresis_c = 1.0;
        hvac.thermostat_fsm.thermostat.deadband_offset = 0.5;
        hvac.thermostat_fsm.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 25.0,
        };

        let mode = hvac.update_mode(&env(19.4, 60, 0)).expect("mode");
        assert_eq!(
            mode,
            ThermostatMode::Heating,
            "19.4°C < turn-on 19.5 → Heating"
        );

        let mode = hvac.update_mode(&env(20.6, 60, 61)).expect("mode");
        assert_eq!(
            mode,
            ThermostatMode::Deadband,
            "20.6°C > turn-off 20.5 → Deadband"
        );
    }

    #[test]
    fn deadband_offset_one_thresholds() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.thermostat_fsm.thermostat.hysteresis_c = 1.0;
        hvac.thermostat_fsm.thermostat.deadband_offset = 1.0;
        hvac.thermostat_fsm.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 25.0,
        };

        let mode = hvac.update_mode(&env(19.9, 60, 0)).expect("mode");
        assert_eq!(
            mode,
            ThermostatMode::Heating,
            "19.9°C < turn-on 20.0 → Heating"
        );

        let mode = hvac.update_mode(&env(21.1, 60, 61)).expect("mode");
        assert_eq!(
            mode,
            ThermostatMode::Deadband,
            "21.1°C > turn-off 21.0 → Deadband"
        );
    }

    #[test]
    fn shr_sensible_latent_split() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));

        hvac.config.shr = 0.5;
        let (s, l) = hvac.sensible_latent_from_shr(10_000.0);
        assert!(
            (s - 5_000.0).abs() < 1e-9,
            "shr=0.5 → sensible=5000; got {s}"
        );
        assert!((l - 5_000.0).abs() < 1e-9, "shr=0.5 → latent=5000; got {l}");

        hvac.config.shr = 1.0;
        let (s, l) = hvac.sensible_latent_from_shr(10_000.0);
        assert!(
            (s - 10_000.0).abs() < 1e-9,
            "shr=1.0 → sensible=10000; got {s}"
        );
        assert!((l - 0.0).abs() < 1e-9, "shr=1.0 → latent=0; got {l}");

        hvac.config.shr = 0.0;
        let (s, l) = hvac.sensible_latent_from_shr(10_000.0);
        assert!((s - 0.0).abs() < 1e-9, "shr=0.0 → sensible=0; got {s}");
        assert!(
            (l - 10_000.0).abs() < 1e-9,
            "shr=0.0 → latent=10000; got {l}"
        );

        hvac.config.shr = 1.5;
        let (s, l) = hvac.sensible_latent_from_shr(10_000.0);
        assert!(
            (s - 10_000.0).abs() < 1e-9,
            "shr=1.5 clamped to 1.0 → sensible=10000; got {s}"
        );
        assert!(
            (l - 0.0).abs() < 1e-9,
            "shr=1.5 clamped to 1.0 → latent=0; got {l}"
        );
    }

    #[test]
    fn schedule_setpoints_cleared_when_source_returns_none_after_valid_step() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.thermostat_fsm.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 25.0,
        };
        // One-element shared source: step 0 returns a value, step 1 errors → None.
        hvac.thermostat_fsm.heating_setpoint_source = Some(ScheduleSource::Shared {
            data: std::sync::Arc::from(vec![21.0]),
            cursor: 0,
            boundary: BoundaryPolicy::Error,
        });

        hvac.thermostat_fsm
            .resolve_profile_setpoints(&env(20.0, 60, 0));
        assert!(
            hvac.thermostat_fsm.schedule_setpoints.is_some(),
            "step 0: source returned a value, schedule_setpoints must be Some"
        );
        assert_eq!(
            hvac.thermostat_fsm.schedule_setpoints.unwrap().heating_c,
            Some(21.0)
        );

        hvac.thermostat_fsm
            .resolve_profile_setpoints(&env(20.0, 60, 60));
        assert!(
            hvac.thermostat_fsm.schedule_setpoints.is_none(),
            "step 1: source returned None (out of bounds), schedule_setpoints must be cleared"
        );
    }

    #[test]
    fn cutout_ratio_governs_turn_off_threshold_when_deadband_offset_is_zero() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.thermostat_fsm.thermostat.hysteresis_c = 2.0;
        hvac.thermostat_fsm.thermostat.cutout_ratio = 0.5;
        hvac.thermostat_fsm.thermostat.deadband_offset = 0.0;
        hvac.thermostat_fsm.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 28.0,
        };

        // Turn heating on (below heating_c - hysteresis).
        let mode = hvac.update_mode(&env(17.0, 60, 0)).expect("turns on");
        assert_eq!(mode, ThermostatMode::Heating);

        // At setpoint (20.0) heating should still be on because turn-off is
        // heating_c + hysteresis * cutout_ratio = 20.0 + 2.0 * 0.5 = 21.0.
        let mode = hvac.update_mode(&env(20.0, 60, 61)).expect("still heating");
        assert_eq!(
            mode,
            ThermostatMode::Heating,
            "must still be heating below cutout threshold"
        );

        // Just above the cutout threshold (21.0) heating should turn off.
        let mode = hvac.update_mode(&env(21.1, 60, 122)).expect("turns off");
        assert_eq!(
            mode,
            ThermostatMode::Deadband,
            "must turn off above cutout threshold"
        );
    }

    #[test]
    fn ff_bounds_clamp_flow_fraction_before_quadratic() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.biquadratic_coeffs = vec![[1.0, 0.0, 0.0, 0.0, 0.0, 0.0]];
        // ff quadratic: ratio = 0.5 + 0.5*ff  (linear in ff)
        hvac.config.cap_ff_coeffs = [0.5, 0.5, 0.0];
        hvac.config.ff_bounds = (0.7, 1.3);

        // ff=0.6 is below ff_min=0.7, should be clamped to 0.7
        let (_raw, adjusted) = hvac.evaluate_biquadratic_with_flow(0, 19.0, 35.0, 0.6);
        let expected = 0.5 + 0.5 * 0.7; // clamped to 0.7
        assert!(
            (adjusted - expected).abs() < 1e-12,
            "ff=0.6 with ff_min=0.7: expected {expected}, got {adjusted}"
        );

        // ff=0.8 is within bounds, no clamping
        let (_raw, adjusted) = hvac.evaluate_biquadratic_with_flow(0, 19.0, 35.0, 0.8);
        let expected = 0.5 + 0.5 * 0.8;
        assert!(
            (adjusted - expected).abs() < 1e-12,
            "ff=0.8 within bounds: expected {expected}, got {adjusted}"
        );

        // ff=1.5 is above ff_max=1.3, should be clamped to 1.3
        let (_raw, adjusted) = hvac.evaluate_biquadratic_with_flow(0, 19.0, 35.0, 1.5);
        let expected = 0.5 + 0.5 * 1.3;
        assert!(
            (adjusted - expected).abs() < 1e-12,
            "ff=1.5 with ff_max=1.3: expected {expected}, got {adjusted}"
        );
    }

    // --- Regression tests for Cd cascade ---
    //
    // The Cd cascade in init() is performed in three separate blocks (lines 396-398,
    // 399-406, 491-515).  This test pins the observable semantics so that the
    // refactoring to a single `resolve_cd` helper does not
    // silently change behaviour.

    /// When the user provides only `"cooling_cd"`, it sets `plf_cooling_degradation_coeff`
    /// but NOT `startup.c_d` (which falls through to `startup_cd` → `cooling_cd` → `cd`).
    /// In the current code `cooling_cd` DOES set both because the second block also reads it.
    /// This test documents the actual (current) behaviour: both fields receive the same value.
    #[test]
    fn cooling_cd_sets_both_plf_and_startup_cd() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::AcCooler, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .test_extras_mut()
            .insert("cooling_cd".to_string(), 0.15_f64.into());
        hvac.init(&config, &env(24.0, 60, 0))
            .expect("init must succeed");
        assert!(
            (hvac.runtime.plf_cooling_degradation_coeff - 0.15).abs() < 1e-12,
            "cooling_cd must set plf_cooling_degradation_coeff to 0.15, got {}",
            hvac.runtime.plf_cooling_degradation_coeff
        );
        assert!(
            (hvac.runtime.startup.c_d - 0.15).abs() < 1e-12,
            "cooling_cd must also set startup.c_d to 0.15, got {}",
            hvac.runtime.startup.c_d
        );
    }

    /// When the user provides `"startup_cd"`, `resolve_cd` applies it to both
    /// `plf_cooling_degradation_coeff` and `startup.c_d`. The old Cd cascade had an
    /// asymmetry where `startup_cd` only set `startup.c_d` while `plf_cooling_degradation_coeff`
    /// fell through to the default — this was a bug, now fixed by the unified `resolve_cd`.
    #[test]
    fn startup_cd_sets_both_plf_and_startup_cd() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::AcCooler, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .test_extras_mut()
            .insert("startup_cd".to_string(), 0.05_f64.into());
        hvac.init(&config, &env(24.0, 60, 0))
            .expect("init must succeed");
        assert!(
            (hvac.runtime.startup.c_d - 0.05).abs() < 1e-12,
            "startup_cd must set startup.c_d to 0.05, got {}",
            hvac.runtime.startup.c_d
        );
        assert!(
            (hvac.runtime.plf_cooling_degradation_coeff - 0.05).abs() < 1e-12,
            "startup_cd must also set plf_cooling_degradation_coeff to 0.05, got {}",
            hvac.runtime.plf_cooling_degradation_coeff
        );
    }

    /// When no Cd key is set and speed_control_mode is VariableSpeedIdeal,
    /// the derived path must set both plf Cd and startup Cd to 0.0.
    #[test]
    fn variable_speed_derived_cd_is_zero() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::AcCooler, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .test_extras_mut()
            .insert("speed_control_mode".to_string(), "variable".into());
        hvac.init(&config, &env(24.0, 60, 0))
            .expect("init must succeed");
        assert_eq!(
            hvac.config.speed_control_mode,
            SpeedControlMode::VariableSpeedIdeal,
            "speed_control_mode must be VariableSpeedIdeal"
        );
        assert!(
            hvac.runtime.plf_cooling_degradation_coeff.abs() < 1e-12,
            "variable-speed derived plf Cd must be 0.0, got {}",
            hvac.runtime.plf_cooling_degradation_coeff
        );
        assert!(
            hvac.runtime.startup.c_d.abs() < 1e-12,
            "variable-speed derived startup Cd must be 0.0, got {}",
            hvac.runtime.startup.c_d
        );
    }

    /// The key chain "startup_cd" → "cooling_cd" → "cd" is evaluated once by
    /// `resolve_cd`. When both `startup_cd` and `cooling_cd` are present,
    /// `startup_cd` wins for both `startup.c_d` and `plf_cooling_degradation_coeff`.
    #[test]
    fn cd_key_chain_priority() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::AcCooler, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .test_extras_mut()
            .insert("startup_cd".to_string(), 0.05_f64.into());
        config
            .test_extras_mut()
            .insert("cooling_cd".to_string(), 0.30_f64.into());
        hvac.init(&config, &env(24.0, 60, 0))
            .expect("init must succeed");
        // startup_cd (0.05) takes precedence over cooling_cd (0.30)
        assert!(
            (hvac.runtime.startup.c_d - 0.05).abs() < 1e-12,
            "startup_cd must take precedence over cooling_cd for startup.c_d, got {}",
            hvac.runtime.startup.c_d
        );
        assert!(
            (hvac.runtime.plf_cooling_degradation_coeff - 0.05).abs() < 1e-12,
            "startup_cd must also take precedence over cooling_cd for plf_cooling_degradation_coeff, got {}",
            hvac.runtime.plf_cooling_degradation_coeff
        );
    }

    // ---------------------------------------------------------------------------
    // Regression tests for default biquadratic performance curves
    //
    // These tests verify that equipment-type-aware default curves produce
    // physically correct temperature-dependent capacity and EIR ratios.
    //
    // Physics basis:
    //   AHRI 210/240-2023 H1 condition: OAT=8.3°C DB, indoor=21.1°C DB.
    //   A properly-fitted capacity curve must return cap_ratio ≈ 1.0 at H1.
    //   At H3 (OAT=-8.3°C), a real ASHP loses 30-50% capacity; cap_ratio < 0.8.
    //
    // Coefficients come from vendors/OCHRE/ochre/defaults/HVAC Heating/
    //   Biquadratic ASHP Heater.csv column Single_1 and
    //   Biquadratic MSHP Heater.csv column Variable_1.
    // ---------------------------------------------------------------------------

    #[test]
    fn ashp_default_cap_curve_unity_at_ahri_h1() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::AshpHeatPumpOnly, ZoneId(1));
        let config = EquipmentConfig::default();
        hvac.init(&config, &env(20.0, 60, 0)).unwrap();
        assert_ne!(
            hvac.config.biquadratic_coeffs[0], DEFAULT_BIQUADRATIC_COEFFS,
            "AshpHeatPumpOnly must not have identity biquadratic coefficients after init"
        );
        let cap_ratio_h1 = hvac.evaluate_biquadratic(0, 21.1, 8.3);
        assert!(
            (cap_ratio_h1 - 1.0).abs() < 0.05,
            "ASHP cap_ratio at AHRI H1 (21.1°C indoor, 8.3°C OAT) \
             must be 1.0 ± 5%; got {cap_ratio_h1:.6}"
        );
    }

    #[test]
    fn ashp_default_cap_curve_below_08_at_ahri_h3() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::AshpHeatPumpOnly, ZoneId(1));
        let config = EquipmentConfig::default();
        hvac.init(&config, &env(20.0, 60, 0)).unwrap();
        let cap_ratio_h3 = hvac.evaluate_biquadratic(0, 21.1, -8.3);
        assert!(
            cap_ratio_h3 < 0.8,
            "ASHP cap_ratio at AHRI H3 (OAT=-8.3°C) is {cap_ratio_h3:.6}; \
             must be < 0.8"
        );
    }

    #[test]
    fn ashp_default_eir_curve_increases_at_low_oat() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::AshpHeatPumpOnly, ZoneId(1));
        let config = EquipmentConfig::default();
        hvac.init(&config, &env(20.0, 60, 0)).unwrap();
        let eir_h1 = hvac.evaluate_biquadratic(1, 21.1, 8.3);
        let eir_h3 = hvac.evaluate_biquadratic(1, 21.1, -8.3);
        assert!(
            eir_h3 > eir_h1,
            "ASHP EIR at H3 ({eir_h3:.4}) must exceed EIR at H1 ({eir_h1:.4})"
        );
        assert!(
            eir_h3 > 1.0,
            "ASHP EIR at H3 must exceed 1.0 (worse than rated efficiency); \
             got {eir_h3:.4}"
        );
    }

    #[test]
    fn mshp_default_cap_curve_below_08_at_ahri_h3() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::MiniSplitHeat, ZoneId(1));
        let config = EquipmentConfig::default();
        hvac.init(&config, &env(20.0, 60, 0)).unwrap();
        let cap_ratio_h3 = hvac.evaluate_biquadratic(0, 21.1, -8.3);
        assert!(
            cap_ratio_h3 < 0.8,
            "MSHP cap_ratio at AHRI H3 (OAT=-8.3°C) is {cap_ratio_h3:.6}; \
             must be < 0.8"
        );
    }

    #[test]
    fn mshp_default_eir_curve_increases_at_low_oat() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::MiniSplitHeat, ZoneId(1));
        let config = EquipmentConfig::default();
        hvac.init(&config, &env(20.0, 60, 0)).unwrap();
        let eir_h1 = hvac.evaluate_biquadratic(1, 21.1, 8.3);
        let eir_h3 = hvac.evaluate_biquadratic(1, 21.1, -8.3);
        assert!(
            eir_h3 > eir_h1,
            "MSHP EIR at H3 ({eir_h3:.4}) must exceed EIR at H1 ({eir_h1:.4})"
        );
        assert!(
            eir_h3 > 1.0,
            "MSHP EIR at H3 must exceed 1.0 (worse than rated efficiency); \
             got {eir_h3:.4}"
        );
    }

    // ---------------------------------------------------------------------------
    // mismatched cap/EIR biquadratic curve counts should warn
    // ---------------------------------------------------------------------------

    /// Mismatched capacity/EIR biquadratic curve counts are rejected during
    /// init rather than silently padded with identity coefficients. OCHRE raises
    /// an exception for this, so HARES must fail loudly too.
    #[test]
    fn mismatched_curve_counts_rejected() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::AshpHeatPumpOnly, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config.test_extras_mut().insert(
            "capacity_biquadratic_coeffs".to_string(),
            "[[0.9,0.01,0,0.02,0,0],[0.8,0.02,0,0.015,0,0]]".into(),
        );
        config.test_extras_mut().insert(
            "eir_biquadratic_coeffs".to_string(),
            "[[1.1,0,0,0.03,0,0]]".into(),
        );
        let result = hvac.init(&config, &env(20.0, 60, 0));
        assert!(
            result.is_err(),
            "expected Err for mismatched curve counts, got Ok"
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("capacity_biquadratic_coeffs has 2 speed(s) but eir_biquadratic_coeffs has 1 speed(s)")
                || msg.contains("per-speed cap/EIR curve counts must match"),
            "error message should mention mismatched curve counts, got: {msg}"
        );
    }

    /// Converse direction: more EIR curves than capacity curves is also rejected.
    #[test]
    fn mismatched_curve_counts_eir_more_than_cap_rejected() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::AshpHeatPumpOnly, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config.test_extras_mut().insert(
            "capacity_biquadratic_coeffs".to_string(),
            "[[0.9,0.01,0,0.02,0,0]]".into(),
        );
        config.test_extras_mut().insert(
            "eir_biquadratic_coeffs".to_string(),
            "[[1.1,0,0,0.03,0,0],[1.2,0,0,0.04,0,0]]".into(),
        );
        let result = hvac.init(&config, &env(20.0, 60, 0));
        assert!(
            result.is_err(),
            "expected Err for mismatched curve counts (more EIR than cap), got Ok"
        );
    }

    /// Providing `capacity_biquadratic_coeffs` without `eir_biquadratic_coeffs` is
    /// rejected. A per-speed capacity curve without a corresponding EIR curve produces
    /// physically impossible constant-EIR behaviour.
    #[test]
    fn cap_curves_without_eir_curves_rejected() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::AshpHeatPumpOnly, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config.test_extras_mut().insert(
            "capacity_biquadratic_coeffs".to_string(),
            "[[0.9,0.01,0,0.02,0,0],[0.8,0.02,0,0.015,0,0]]".into(),
        );
        let result = hvac.init(&config, &env(20.0, 60, 0));
        assert!(
            result.is_err(),
            "expected Err when capacity_biquadratic_coeffs is provided without eir_biquadratic_coeffs"
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("no eir_biquadratic_coeffs provided"),
            "error should mention missing eir, got: {msg}"
        );
    }

    /// Providing `eir_biquadratic_coeffs` without `capacity_biquadratic_coeffs` is
    /// rejected (converse direction of the test above).
    #[test]
    fn eir_curves_without_cap_curves_rejected() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::AshpHeatPumpOnly, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config.test_extras_mut().insert(
            "eir_biquadratic_coeffs".to_string(),
            "[[1.1,0,0,0.03,0,0],[1.2,0,0,0.04,0,0]]".into(),
        );
        let result = hvac.init(&config, &env(20.0, 60, 0));
        assert!(
            result.is_err(),
            "expected Err when eir_biquadratic_coeffs is provided without capacity_biquadratic_coeffs"
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("no capacity_biquadratic_coeffs provided"),
            "error should mention missing cap, got: {msg}"
        );
    }

    #[test]
    fn biquadratic_curve_source_default_after_init_ashp() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::AshpHeatPumpOnly, ZoneId(1));
        let config = EquipmentConfig::default();
        hvac.init(&config, &env(20.0, 60, 0)).unwrap();
        assert_eq!(
            hvac.config.biquadratic_curve_source,
            BiquadraticCurveSource::Default,
            "AshpHeatPumpOnly with no user curves must report Default source"
        );
    }

    #[test]
    fn biquadratic_curve_source_user_when_explicit_curves_provided() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::AshpHeatPumpOnly, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config.test_extras_mut().insert(
            "biquadratic_coeffs".to_string(),
            "[[0.9,0.01,0,0.02,0,0],[1.1,0,0,0.03,0,0]]".into(),
        );
        hvac.init(&config, &env(20.0, 60, 0)).unwrap();
        assert_eq!(
            hvac.config.biquadratic_curve_source,
            BiquadraticCurveSource::User,
            "Explicitly provided curves must report User source"
        );
    }

    #[test]
    fn biquadratic_curve_source_identity_for_furnace() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        let config = EquipmentConfig::default();
        hvac.init(&config, &env(20.0, 60, 0)).unwrap();
        assert_eq!(
            hvac.config.biquadratic_curve_source,
            BiquadraticCurveSource::Identity,
            "GasFurnace has no biquadratic defaults; must report Identity"
        );
    }

    #[test]
    fn ashp_default_cap_curve_matches_ochre_csv_single_1() {
        use super::super::default_curves::default_biquadratic_coeffs;
        let defaults = default_biquadratic_coeffs(HvacEquipmentType::AshpHeatPumpOnly).unwrap();
        let expected_cap: [f64; 6] = [
            0.878143655,
            -0.002914855,
            -0.00003337,
            0.022386661,
            0.000163944,
            -0.00002187,
        ];
        let expected_eir: [f64; 6] = [
            0.716518071,
            0.010275901,
            0.000460734,
            -0.006480365,
            0.000456354,
            -0.00069764,
        ];
        assert_eq!(
            defaults[0], expected_cap,
            "ASHP capacity coefficients must match CSV"
        );
        assert_eq!(
            defaults[1], expected_eir,
            "ASHP EIR coefficients must match CSV"
        );
    }

    #[test]
    fn mshp_default_cap_curve_matches_ochre_csv_variable_1() {
        use super::super::default_curves::default_biquadratic_coeffs;
        let defaults = default_biquadratic_coeffs(HvacEquipmentType::MiniSplitHeat).unwrap();
        let expected_cap: [f64; 6] = [1.002928121, -0.010386676, 0.0, 0.025961538, 0.0, 0.0];
        let expected_eir: [f64; 6] = [
            0.966475473,
            0.00591495,
            0.000191202,
            -0.012965668,
            0.00004225,
            -0.000524003,
        ];
        assert_eq!(
            defaults[0], expected_cap,
            "MSHP capacity coefficients must match CSV"
        );
        assert_eq!(
            defaults[1], expected_eir,
            "MSHP EIR coefficients must match CSV"
        );
    }

    #[test]
    fn derive_default_shr_at_room_ac_airflow() {
        // Room AC airflow ≈ 320 CFM/ton → EnergyPlus auto-sizing predicts
        // SHR ≈ 0.431 + 6086 × 4.294e-5 ≈ 0.692.
        let shr = derive_default_shr(Some(AIRFLOW_ROOM_AC_M3_S_PER_W), Some(3_500.0), true);
        assert!(
            (shr - 0.692).abs() < 0.005,
            "Room AC derived SHR should be ~0.692, got {shr}"
        );
    }

    #[test]
    fn derive_default_shr_at_central_ac_airflow() {
        // Central AC airflow ≈ 400 CFM/ton → EnergyPlus auto-sizing predicts
        // SHR ≈ 0.431 + 6086 × 5.368e-5 ≈ 0.758.
        let shr = derive_default_shr(Some(AIRFLOW_CENTRAL_AC_M3_S_PER_W), Some(8_000.0), false);
        assert!(
            (shr - 0.758).abs() < 0.005,
            "Central AC derived SHR should be ~0.758, got {shr}"
        );
    }

    #[test]
    fn derive_default_shr_falls_back_when_airflow_missing() {
        let shr_room = derive_default_shr(None, Some(3_500.0), true);
        assert!(
            (shr_room - DEFAULT_SHR_ROOM_AC).abs() < 1e-12,
            "Room AC fallback should be {DEFAULT_SHR_ROOM_AC}, got {shr_room}"
        );

        let shr_central = derive_default_shr(None, Some(8_000.0), false);
        assert!(
            (shr_central - DEFAULT_SHR_CENTRAL_AC).abs() < 1e-12,
            "Central AC fallback should be {DEFAULT_SHR_CENTRAL_AC}, got {shr_central}"
        );
    }

    #[test]
    fn derive_default_shr_clamped_to_valid_range() {
        let shr_high = derive_default_shr(Some(0.001), Some(1_000.0), false);
        assert!(
            shr_high <= 1.0,
            "SHR must be clamped to 1.0, got {shr_high}"
        );

        let shr_low = derive_default_shr(Some(1e-8), Some(1_000.0), false);
        assert!(shr_low >= 0.5, "SHR must be clamped to 0.5, got {shr_low}");
    }

    #[test]
    fn biquadratic_oob_capacity_index_clamps_to_last_even_curve() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        // 6 curves = 3 speeds (cap+EIR pairs), indices 0,1,2,3,4,5.
        // Last even (capacity) index = 4. Last odd (EIR) index = 5.
        hvac.config.biquadratic_coeffs = vec![
            [0.8, 0.0, 0.0, 0.0, 0.0, 0.0], // cap speed 0
            [1.2, 0.0, 0.0, 0.0, 0.0, 0.0], // EIR speed 0
            [0.9, 0.0, 0.0, 0.0, 0.0, 0.0], // cap speed 1
            [1.1, 0.0, 0.0, 0.0, 0.0, 0.0], // EIR speed 1
            [2.0, 0.0, 0.0, 0.0, 0.0, 0.0], // cap speed 2 (last even)
            [3.0, 0.0, 0.0, 0.0, 0.0, 0.0], // EIR speed 2 (last odd)
        ];
        // Request capacity curve index 8 (OOB, even) → should clamp to 4 (last even, 2.0)
        let result = hvac.evaluate_biquadratic(8, 20.0, 30.0);
        assert!(
            (result - 2.0).abs() < 1e-12,
            "expected 2.0 from last even curve, got {result}"
        );
    }

    #[test]
    fn biquadratic_oob_eir_index_clamps_to_last_odd_curve() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        // Same setup as the capacity test: 6 curves, indices 0..5.
        hvac.config.biquadratic_coeffs = vec![
            [0.8, 0.0, 0.0, 0.0, 0.0, 0.0],
            [1.2, 0.0, 0.0, 0.0, 0.0, 0.0],
            [0.9, 0.0, 0.0, 0.0, 0.0, 0.0],
            [1.1, 0.0, 0.0, 0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [3.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        ];
        // Request EIR curve index 7 (OOB, odd) → should clamp to 5 (last odd, 3.0)
        let result = hvac.evaluate_biquadratic(7, 20.0, 30.0);
        assert!(
            (result - 3.0).abs() < 1e-12,
            "expected 3.0 from last odd curve, got {result}"
        );
    }

    #[test]
    fn biquadratic_oob_clamping_increments_counter() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.config.biquadratic_coeffs = vec![
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        ];
        // Evaluate OOB capacity index → should clamp and increment counter
        let _ = hvac.evaluate_biquadratic(4, 20.0, 30.0);
        let _ = hvac.evaluate_biquadratic(5, 20.0, 30.0);
        assert_eq!(
            hvac.take_biquadratic_clamp_count(),
            2,
            "two OOB calls → counter = 2"
        );
        // Counter resets after take
        assert_eq!(
            hvac.take_biquadratic_clamp_count(),
            0,
            "counter reset to 0 after take"
        );
    }

    #[test]
    fn biquadratic_oob_eir_clamps_to_zero_for_single_identity_curve() {
        let hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        assert_eq!(
            hvac.config.biquadratic_coeffs.len(),
            1,
            "default config has single identity curve"
        );
        let result = hvac.evaluate_biquadratic(1, 20.0, 30.0);
        assert!(
            result.is_finite(),
            "EIR curve index 1 on n=1 must yield finite value (clamped to index 0), got {result}"
        );
        assert!(
            (result - DEFAULT_BIQUADRATIC_COEFFS[0]).abs() < 1e-12,
            "EIR curve index 1 on n=1 must clamp to the single identity curve value {}, got {result}",
            DEFAULT_BIQUADRATIC_COEFFS[0],
        );
    }

    #[test]
    fn biquadratic_curve_count_rejects_partial_per_stage_coverage() {
        // 4 speed stages require 8 biquadratic entries (4 cap+EIR pairs).
        // 6 entries = 3 pairs → one pair short → rejected.
        let result = super::HvacEquipment::validate_biquadratic_curve_count(6, 4);
        assert!(
            result.is_err(),
            "6 coeffs for 4 speeds (3 pairs, need 4) must be rejected"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("biquadratic_coeffs has 6 entries"),
            "error must name the actual coeff count; got: {err}"
        );
        assert!(
            err.contains("speed stage"),
            "error must reference speed stages; got: {err}"
        );
    }

    #[test]
    fn biquadratic_curve_count_accepts_shared_pair_for_many_speeds() {
        // Minisplit fixture: 4 speed stages, 2 biquadratic entries (1 pair shared).
        super::HvacEquipment::validate_biquadratic_curve_count(2, 4).unwrap();
    }

    #[test]
    fn biquadratic_curve_count_accepts_identity_curve() {
        // 1 biquadratic entry (identity) for any number of speeds is valid.
        super::HvacEquipment::validate_biquadratic_curve_count(1, 4).unwrap();
        super::HvacEquipment::validate_biquadratic_curve_count(1, 1).unwrap();
        super::HvacEquipment::validate_biquadratic_curve_count(1, 0).unwrap();
    }

    #[test]
    fn biquadratic_curve_count_accepts_full_coverage() {
        // Exact match: n_speeds * 2 entries → n_speeds cap+EIR pairs.
        super::HvacEquipment::validate_biquadratic_curve_count(4, 2).unwrap();
        super::HvacEquipment::validate_biquadratic_curve_count(8, 4).unwrap();
        super::HvacEquipment::validate_biquadratic_curve_count(2, 1).unwrap();
    }

    #[test]
    fn biquadratic_curve_count_accepts_zero_speeds() {
        // No speed stages → nothing to validate against; always accepted.
        super::HvacEquipment::validate_biquadratic_curve_count(100, 0).unwrap();
        super::HvacEquipment::validate_biquadratic_curve_count(0, 0).unwrap();
    }

    #[test]
    fn biquadratic_curve_count_rejects_intermediate_counts() {
        // 3 coeffs for 2 speeds: 2 < 3 < 4 → rejected.
        assert!(super::HvacEquipment::validate_biquadratic_curve_count(3, 2).is_err());
        // 4 coeffs for 3 speeds: 2 < 4 < 6 → rejected.
        assert!(super::HvacEquipment::validate_biquadratic_curve_count(4, 3).is_err());
    }
}
