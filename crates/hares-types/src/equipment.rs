//! Equipment descriptor and identifier types.
//!
//! `EquipmentId`, `EquipmentDescriptor`, and capability flags that identify
//! equipment instances across crate boundaries.

use std::borrow::Cow;
use std::fmt;

use bitflags::bitflags;
use serde::{Deserialize, Serialize};

use crate::{ControlCapabilities, DayFilter, HaresError, ZoneId};

/// Stable equipment instance identifier.
#[derive(
    Hash, Eq, PartialEq, Copy, Clone, Debug, Default, Ord, PartialOrd, Serialize, Deserialize,
)]
pub struct EquipmentId(pub u32);

/// High-level end-use categories used for routing and reporting.
///
/// Extensible string-based type supporting both standard predefined categories
/// and custom user-defined end uses. Use the constants for standard types
/// (e.g., `EndUse::HVAC_HEATING`) or create custom ones with `EndUse::custom()`.
///
/// # Examples
///
/// ```
/// use hares_types::EndUse;
///
/// // Standard predefined end use
/// let heating = EndUse::HVAC_HEATING;
/// assert_eq!(heating.as_str(), "hvac_heating");
///
/// // Custom user-defined end use
/// let custom = EndUse::custom("heat_pump_water_heater");
/// assert_eq!(custom.as_str(), "heat_pump_water_heater");
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EndUse(Cow<'static, str>);

impl EndUse {
    /// Standard HVAC heating end use.
    pub const HVAC_HEATING: Self = Self::new("hvac_heating");
    /// Standard HVAC cooling end use.
    pub const HVAC_COOLING: Self = Self::new("hvac_cooling");
    /// Standard water heating end use.
    pub const WATER_HEATING: Self = Self::new("water_heating");
    /// Standard lighting end use.
    pub const LIGHTING: Self = Self::new("lighting");
    /// Standard plug loads end use.
    pub const PLUG_LOADS: Self = Self::new("plug_loads");
    /// Standard refrigeration end use.
    pub const REFRIGERATION: Self = Self::new("refrigeration");
    /// Standard ventilation end use.
    pub const VENTILATION: Self = Self::new("ventilation");
    /// Standard battery storage end use.
    pub const BATTERY: Self = Self::new("battery");
    /// Standard photovoltaic generation end use.
    pub const PV: Self = Self::new("pv");
    /// Standard electric vehicle end use.
    pub const EV: Self = Self::new("ev");
    /// Standard backup generator end use.
    pub const GENERATOR: Self = Self::new("generator");
    /// Returns true if this end use represents HVAC equipment.
    ///
    /// HVAC end uses cover space heating, cooling, and dehumidification —
    /// equipment that directly affects zone air temperature and humidity.
    pub fn is_hvac(&self) -> bool {
        *self == Self::HVAC_HEATING || *self == Self::HVAC_COOLING || *self == Self::DEHUMIDIFIER
    }

    /// Standard dehumidification end use.
    pub const DEHUMIDIFIER: Self = Self::new("dehumidifier");
    /// Default fallback/other end use.
    pub const OTHER: Self = Self::new("other");

    /// Creates a new end use from a string.
    #[inline]
    pub const fn new(s: &'static str) -> Self {
        Self(Cow::Borrowed(s))
    }

    /// Creates a custom end use from any string.
    ///
    /// This allows equipment and actors to define their own end use categories
    /// without modifying the core library.
    ///
    /// # Examples
    ///
    /// ```
    /// use hares_types::EndUse;
    ///
    /// let hpwh = EndUse::custom("heat_pump_water_heater");
    /// let ice_storage = EndUse::custom("ice_storage");
    /// ```
    #[inline]
    pub fn custom<S: Into<Cow<'static, str>>>(s: S) -> Self {
        Self(s.into())
    }

    /// Returns the string representation of this end use.
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Checks if this is a standard predefined end use.
    pub fn is_standard(&self) -> bool {
        matches!(
            self.0.as_ref(),
            "hvac_heating"
                | "hvac_cooling"
                | "water_heating"
                | "lighting"
                | "plug_loads"
                | "refrigeration"
                | "ventilation"
                | "battery"
                | "pv"
                | "ev"
                | "generator"
                | "dehumidifier"
                | "other"
        )
    }
}

impl From<&'static str> for EndUse {
    fn from(s: &'static str) -> Self {
        Self::new(s)
    }
}

impl From<String> for EndUse {
    fn from(s: String) -> Self {
        Self::custom(s)
    }
}

/// Ideal capacity mode for HVAC equipment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IdealCapacityMode {
    /// Use ideal capacity when time_res >= 5 min or variable-speed equipment (OCHRE rule).
    #[default]
    Auto,
    /// Force ideal capacity mode regardless of timestep/speed.
    On,
    /// Force dynamic on/off thermostat cycling (duty_cycle = 1.0 when on).
    Off,
}

/// Fuel type consumed by an equipment device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FuelType {
    Electric,
    Gas,
    Propane,
    Oil,
    Wood,
    Coal,
    WoodPellet,
    #[default]
    None,
}

/// Fixed execution stage ordering for equipment updates.
///
/// Stages 1-3 are for equipment; stage 4 (EnvelopeResolution) runs after all
/// equipment and is used by domain solvers (thermal, humidity, electrical, fluid).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ExecutionStage {
    #[default]
    Independent,
    Electrical,
    Thermal,
    EnvelopeResolution,
}

/// Runtime operating mode reported by equipment.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OperatingMode {
    #[default]
    Off = 0,
    Heating = 1,
    Cooling = 2,
    Defrost = 3,
    Standby = 4,
    Charging = 5,
    Discharging = 6,
    HeatingHP = 7,
    HeatingER = 8,
    HeatingHPAndER = 9,
    HeatPumpWH = 10,
    BackupElement = 11,
    On = 12,
}

impl OperatingMode {
    pub fn as_code(&self) -> f64 {
        *self as u8 as f64
    }
}

impl TryFrom<u8> for OperatingMode {
    type Error = HaresError;

    fn try_from(v: u8) -> Result<Self, Self::Error> {
        match v {
            0 => Ok(Self::Off),
            1 => Ok(Self::Heating),
            2 => Ok(Self::Cooling),
            3 => Ok(Self::Defrost),
            4 => Ok(Self::Standby),
            5 => Ok(Self::Charging),
            6 => Ok(Self::Discharging),
            7 => Ok(Self::HeatingHP),
            8 => Ok(Self::HeatingER),
            9 => Ok(Self::HeatingHPAndER),
            10 => Ok(Self::HeatPumpWH),
            11 => Ok(Self::BackupElement),
            _ => Err(HaresError::Equipment(format!(
                "unknown OperatingMode discriminant: {v}"
            ))),
        }
    }
}

/// Custom domain identifier for extension points in the solver.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct DomainId(pub u16);

/// Identifier for a fluid loop.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct LoopId(pub u16);

/// Identifier for protocol-specific integrations.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ProtocolId(pub u16);

/// Working fluid categories used by fluid ports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FluidType {
    Water,
    Glycol,
    Refrigerant,
}

/// Lookup table type for battery/EV LUT injection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BatteryLutType {
    ChargingCurve,
    Ocv,
    UNeg,
}

/// Battery cell chemistry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BatteryChemistry {
    Nmc,
    Lfp,
    Nca,
    Lto,
}

impl BatteryChemistry {
    /// Returns a lowercase config key for the equipment config system.
    pub fn as_config_str(&self) -> &'static str {
        match self {
            Self::Nmc => "nmc",
            Self::Lfp => "lfp",
            Self::Nca => "nca",
            Self::Lto => "lto",
        }
    }
}

impl std::str::FromStr for BatteryChemistry {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "nmc" => Ok(Self::Nmc),
            "lfp" => Ok(Self::Lfp),
            "nca" => Ok(Self::Nca),
            "lto" => Ok(Self::Lto),
            _ => Err(format!("invalid BatteryChemistry: {s}")),
        }
    }
}

impl fmt::Display for BatteryChemistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Nmc => "NMC",
            Self::Lfp => "LFP",
            Self::Nca => "NCA",
            Self::Lto => "LTO",
        })
    }
}

/// EV charging level (residential L1/L2 only).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChargingLevel {
    L1,
    L2,
}

impl std::str::FromStr for ChargingLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "l1" => Ok(Self::L1),
            "l2" => Ok(Self::L2),
            _ => Err(format!("invalid ChargingLevel: {s}")),
        }
    }
}

impl fmt::Display for ChargingLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::L1 => "L1",
            Self::L2 => "L2",
        })
    }
}

/// Electric vehicle powertrain type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VehicleType {
    Bev,
    Phev,
}

impl std::str::FromStr for VehicleType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "bev" => Ok(Self::Bev),
            "phev" => Ok(Self::Phev),
            _ => Err(format!("invalid VehicleType: {s}")),
        }
    }
}

impl fmt::Display for VehicleType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Bev => "BEV",
            Self::Phev => "PHEV",
        })
    }
}

/// EV connection state: home charging, away charging, or disconnected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EvConnectionState {
    #[default]
    HomePluggedIn,
    AwayPluggedIn,
    Disconnected,
}

impl std::str::FromStr for EvConnectionState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().replace('_', "").as_str() {
            "homepluggedin" => Ok(Self::HomePluggedIn),
            "awaypluggedin" => Ok(Self::AwayPluggedIn),
            "disconnected" => Ok(Self::Disconnected),
            _ => Err(format!("invalid EvConnectionState: {s}")),
        }
    }
}

impl fmt::Display for EvConnectionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::HomePluggedIn => "HomePluggedIn",
            Self::AwayPluggedIn => "AwayPluggedIn",
            Self::Disconnected => "Disconnected",
        })
    }
}

/// When the EV should plug in at home.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PlugInPolicy {
    Always,
    LowSoc { threshold: f64 },
}

/// A departure deadline with day-of-week filter and required SoC.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DepartureConstraint {
    pub day_filter: DayFilter,
    /// Minute of day the vehicle must depart; valid range [0, 1439].
    pub departure_minute: u32,
    pub target_soc: f64,
}

impl DepartureConstraint {
    pub fn validate(&self) -> Result<(), crate::HaresError> {
        if self.departure_minute >= 1440 {
            return Err(crate::HaresError::Equipment(format!(
                "departure_minute must be < 1440, got {}",
                self.departure_minute
            )));
        }
        validate_fraction("target_soc", self.target_soc)
    }
}

fn default_charge_buffer_hours() -> f64 {
    2.0
}

/// Charging strategy governing when and how fast to charge.
///
/// `TouAware` references the TOU rate schedule from the environment/simulation
/// config -- the EV just knows "be TOU-aware" and reads peak periods externally.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ChargingStrategy {
    Immediate {
        target_soc: f64,
    },
    Nightly {
        off_peak_start_hour: f64,
        off_peak_end_hour: f64,
        target_soc: f64,
    },
    LowSoc {
        threshold: f64,
        target_soc: f64,
    },
    QuickThenWait {
        partial_soc: f64,
    },
    PreDeparture {
        target_soc: f64,
        #[serde(default)]
        departure_schedule: Vec<DepartureConstraint>,
    },
    TouAware {
        target_soc: f64,
        #[serde(default)]
        departure_schedule: Vec<DepartureConstraint>,
        #[serde(default = "default_charge_buffer_hours")]
        charge_buffer_hours: f64,
    },
    SolarSurplus {
        min_charge_rate_kw: f64,
        #[serde(default)]
        departure_schedule: Vec<DepartureConstraint>,
    },
    V2H {
        discharge_threshold_soc: f64,
        min_soc: f64,
    },
    V2G {
        min_soc: f64,
        max_export_kw: f64,
        price_threshold: f64,
    },
}

impl ChargingStrategy {
    /// Validate all fraction/SoC fields and nested departure constraints.
    pub fn validate(&self) -> Result<(), crate::HaresError> {
        match self {
            Self::Immediate { target_soc }
            | Self::QuickThenWait {
                partial_soc: target_soc,
            } => validate_fraction("target_soc", *target_soc),
            Self::Nightly {
                off_peak_start_hour,
                off_peak_end_hour,
                target_soc,
            } => {
                if !off_peak_start_hour.is_finite()
                    || !off_peak_end_hour.is_finite()
                    || *off_peak_start_hour < 0.0
                    || *off_peak_start_hour >= 24.0
                    || *off_peak_end_hour < 0.0
                    || *off_peak_end_hour >= 24.0
                {
                    return Err(crate::HaresError::Equipment(
                        "off_peak hours must be finite and in [0.0, 24.0)".into(),
                    ));
                }
                validate_fraction("target_soc", *target_soc)
            }
            Self::LowSoc {
                threshold,
                target_soc,
            } => {
                validate_fraction("threshold", *threshold)?;
                validate_fraction("target_soc", *target_soc)
            }
            Self::PreDeparture {
                target_soc,
                departure_schedule,
            } => {
                validate_fraction("target_soc", *target_soc)?;
                for dc in departure_schedule {
                    dc.validate()?;
                }
                Ok(())
            }
            Self::TouAware {
                target_soc,
                departure_schedule,
                charge_buffer_hours,
            } => {
                validate_fraction("target_soc", *target_soc)?;
                if !charge_buffer_hours.is_finite() || *charge_buffer_hours < 0.0 {
                    return Err(crate::HaresError::Equipment(format!(
                        "charge_buffer_hours must be finite and >= 0, got {charge_buffer_hours}"
                    )));
                }
                for dc in departure_schedule {
                    dc.validate()?;
                }
                Ok(())
            }
            Self::SolarSurplus {
                min_charge_rate_kw,
                departure_schedule,
            } => {
                if !min_charge_rate_kw.is_finite() || *min_charge_rate_kw < 0.0 {
                    return Err(crate::HaresError::Equipment(format!(
                        "min_charge_rate_kw must be finite and >= 0, got {min_charge_rate_kw}"
                    )));
                }
                for dc in departure_schedule {
                    dc.validate()?;
                }
                Ok(())
            }
            Self::V2H {
                discharge_threshold_soc,
                min_soc,
            } => {
                validate_fraction("discharge_threshold_soc", *discharge_threshold_soc)?;
                validate_fraction("min_soc", *min_soc)?;
                if min_soc > discharge_threshold_soc {
                    return Err(crate::HaresError::Equipment(
                        "min_soc must be <= discharge_threshold_soc".into(),
                    ));
                }
                Ok(())
            }
            Self::V2G {
                min_soc,
                max_export_kw,
                price_threshold,
            } => {
                validate_fraction("min_soc", *min_soc)?;
                if !max_export_kw.is_finite() || *max_export_kw < 0.0 {
                    return Err(crate::HaresError::Equipment(format!(
                        "max_export_kw must be finite and >= 0, got {max_export_kw}"
                    )));
                }
                if !price_threshold.is_finite() {
                    return Err(crate::HaresError::Equipment(
                        "price_threshold must be finite".into(),
                    ));
                }
                Ok(())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum GridExportRule {
    SolarOnly,
    #[default]
    Unrestricted,
    Disabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum StormWatchTrigger {
    ManualEnable,
    WeatherSignal { wind_speed_threshold_m_s: f64 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum BmsAction {
    Charge { rate_fraction: f64 },
    Discharge { rate_fraction: f64 },
    Idle,
    Hold { target_soc: f64 },
}

impl BmsAction {
    pub fn validate(&self) -> Result<(), crate::HaresError> {
        match self {
            Self::Charge { rate_fraction } | Self::Discharge { rate_fraction } => {
                validate_fraction("rate_fraction", *rate_fraction)
            }
            Self::Hold { target_soc } => validate_fraction("target_soc", *target_soc),
            Self::Idle => Ok(()),
        }
    }
}

/// Day/time range for BMS scheduling (no value/noise payload).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BmsTimeWindow {
    pub day: DayFilter,
    pub start_minute: u16,
    pub end_minute: u16,
}

impl BmsTimeWindow {
    /// Does this window contain the given day and minute-of-day?
    /// Same semantics as `TimeWindow::contains()`: half-open `[start, end)`,
    /// wraps across midnight when `start > end`.
    pub fn contains(&self, weekday: chrono::Weekday, minute_of_day: u16) -> bool {
        if !self.day.matches(weekday) {
            return false;
        }
        if self.start_minute < self.end_minute {
            minute_of_day >= self.start_minute && minute_of_day < self.end_minute
        } else {
            minute_of_day >= self.start_minute || minute_of_day < self.end_minute
        }
    }

    pub fn validate(&self) -> Result<(), crate::HaresError> {
        if self.start_minute >= 1440 {
            return Err(crate::HaresError::Equipment(format!(
                "start_minute must be < 1440, got {}",
                self.start_minute
            )));
        }
        if self.end_minute > 1440 {
            return Err(crate::HaresError::Equipment(format!(
                "end_minute must be <= 1440, got {}",
                self.end_minute
            )));
        }
        if self.start_minute == self.end_minute {
            return Err(crate::HaresError::Equipment(
                "zero-width window (start_minute == end_minute) is invalid".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BmsScheduleWindow {
    pub time_window: BmsTimeWindow,
    pub action: BmsAction,
}

/// Battery management system operating mode.
///
/// Configures how the `BatteryManagementActor` dispatches charge/discharge
/// control signals relative to PV production, grid prices, and backup needs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub enum BmsMode {
    SelfConsumption {
        min_soc: f64,
        max_soc: f64,
        solar_only_charging: bool,
    },
    TimeOfUseOptimization {
        reserve_soc: f64,
        charge_threshold_percentile: f64,
        discharge_threshold_percentile: f64,
        solar_only_charging: bool,
    },
    BackupReserve {
        target_soc: f64,
        charge_from_grid: bool,
        charge_rate_fraction: f64,
    },
    DemandResponse {
        base_mode: Box<BmsMode>,
        dr_discharge_rate: f64,
        min_soc_during_dr: f64,
    },
    Scheduled {
        windows: Vec<BmsScheduleWindow>,
    },
    StormWatch {
        target_soc: f64,
        trigger: StormWatchTrigger,
        base_mode: Box<BmsMode>,
    },
    #[default]
    Manual,
}

impl BmsMode {
    /// Validate all fraction/SoC fields are finite and in `[0.0, 1.0]`.
    pub fn validate(&self) -> Result<(), crate::HaresError> {
        match self {
            Self::SelfConsumption {
                min_soc, max_soc, ..
            } => {
                validate_fraction("min_soc", *min_soc)?;
                validate_fraction("max_soc", *max_soc)?;
                if min_soc > max_soc {
                    return Err(crate::HaresError::Equipment(
                        "min_soc must be <= max_soc".into(),
                    ));
                }
                Ok(())
            }
            Self::TimeOfUseOptimization {
                reserve_soc,
                charge_threshold_percentile,
                discharge_threshold_percentile,
                ..
            } => {
                validate_fraction("reserve_soc", *reserve_soc)?;
                validate_fraction("charge_threshold_percentile", *charge_threshold_percentile)?;
                validate_fraction(
                    "discharge_threshold_percentile",
                    *discharge_threshold_percentile,
                )
            }
            Self::BackupReserve {
                target_soc,
                charge_rate_fraction,
                ..
            } => {
                validate_fraction("target_soc", *target_soc)?;
                validate_fraction("charge_rate_fraction", *charge_rate_fraction)
            }
            Self::DemandResponse {
                base_mode,
                dr_discharge_rate,
                min_soc_during_dr,
            } => {
                validate_fraction("dr_discharge_rate", *dr_discharge_rate)?;
                validate_fraction("min_soc_during_dr", *min_soc_during_dr)?;
                base_mode.validate()
            }
            Self::Scheduled { windows } => {
                for w in windows {
                    w.time_window.validate()?;
                    w.action.validate()?;
                }
                Ok(())
            }
            Self::StormWatch {
                target_soc,
                base_mode,
                ..
            } => {
                validate_fraction("target_soc", *target_soc)?;
                base_mode.validate()
            }
            Self::Manual => Ok(()),
        }
    }
}

fn validate_fraction(name: &str, v: f64) -> Result<(), crate::HaresError> {
    if !v.is_finite() || !(0.0..=1.0).contains(&v) {
        return Err(crate::HaresError::Equipment(format!(
            "{name} must be finite and in [0.0, 1.0], got {v}"
        )));
    }
    Ok(())
}

/// Electrical power measurement for equipment output.
///
/// All values must be finite and non-negative for Consumption and Generation.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ElectricPower {
    /// Net power consumed from the grid (kW). Must be >= 0.
    Consumption(f64),
    /// Net power generated/exported to the grid (kW). Must be >= 0.
    Generation(f64),
    /// Bidirectional power (kW). Positive = consuming, negative = generating.
    Bidirectional(f64),
}

impl ElectricPower {
    pub fn consumption(kw: f64) -> Result<Self, HaresError> {
        if !kw.is_finite() || kw < 0.0 {
            return Err(HaresError::Equipment(format!(
                "ElectricPower::Consumption requires finite non-negative value, got {kw}"
            )));
        }
        Ok(Self::Consumption(kw))
    }

    pub fn generation(kw: f64) -> Result<Self, HaresError> {
        if !kw.is_finite() || kw < 0.0 {
            return Err(HaresError::Equipment(format!(
                "ElectricPower::Generation requires finite non-negative value, got {kw}"
            )));
        }
        Ok(Self::Generation(kw))
    }

    pub fn bidirectional(kw: f64) -> Result<Self, HaresError> {
        if !kw.is_finite() {
            return Err(HaresError::Equipment(format!(
                "ElectricPower::Bidirectional requires finite value, got {kw}"
            )));
        }
        Ok(Self::Bidirectional(kw))
    }

    /// Net consumption in kW. Generation is negative, bidirectional follows sign.
    pub fn net_consumption_kw(&self) -> f64 {
        match self {
            Self::Consumption(v) => *v,
            Self::Generation(v) => -*v,
            Self::Bidirectional(v) => *v,
        }
    }

    /// Equivalent to [`net_consumption_kw()`](Self::net_consumption_kw): positive = consuming, negative = generating.
    pub fn signed_kw(&self) -> f64 {
        self.net_consumption_kw()
    }
}

/// State of charge, constrained to [0.0, 1.0].
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Soc(f64);

impl Default for Soc {
    fn default() -> Self {
        Self(0.0)
    }
}

impl Soc {
    pub fn get(&self) -> f64 {
        self.0
    }
}

impl TryFrom<f64> for Soc {
    type Error = HaresError;

    fn try_from(v: f64) -> Result<Self, Self::Error> {
        if !v.is_finite() || !(0.0..=1.0).contains(&v) {
            return Err(HaresError::Equipment(format!(
                "Soc must be in [0.0, 1.0], got {v}"
            )));
        }
        Ok(Self(v))
    }
}

/// Fuel consumption for thermal/combustion equipment.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FuelPower {
    pub fuel_type: FuelType,
    /// Fuel consumption rate in watts.
    pub consumption_w: f64,
}

impl FuelPower {
    pub fn new(fuel_type: FuelType, consumption_w: f64) -> Result<Self, HaresError> {
        if !consumption_w.is_finite() || consumption_w < 0.0 {
            return Err(HaresError::Equipment(format!(
                "FuelPower consumption_w must be finite and non-negative, got {consumption_w}"
            )));
        }
        Ok(Self {
            fuel_type,
            consumption_w,
        })
    }
}

/// Energy flow outputs from one equipment step.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CoreFlows {
    pub electric_kw: Option<ElectricPower>,
    /// Reactive power in kvar. Positive = inductive/lagging (IEEE 1547).
    pub reactive_power_kvar: Option<f64>,
    pub fuel_w: Option<FuelPower>,
    /// Net delivered thermal output in watts (positive = heating, negative = cooling),
    /// post-DSE. None for non-thermal equipment.
    pub thermal_output_w: Option<f64>,
    /// Delivered sensible cooling in watts (negative or zero). Post-DSE.
    /// None for non-cooling equipment.
    pub sensible_cooling_w: Option<f64>,
    /// Delivered latent cooling in watts (negative or zero). Post-DSE.
    /// None for non-cooling equipment.
    pub latent_cooling_w: Option<f64>,
}

/// Discrete/continuous state outputs from one equipment step.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CoreState {
    pub operating_mode: Option<OperatingMode>,
    pub soc: Option<Soc>,
    /// Active compressor/heat-pump speed level (0-based). None when equipment
    /// has no discrete speeds or is off.
    pub speed_index: Option<u8>,
    /// Active thermal setpoint in °C: heating setpoint when heating, cooling
    /// setpoint when cooling, None when off or in deadband.
    pub setpoint_c: Option<f64>,
}

/// Equipment performance metrics from one step.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CorePerformance {
    /// Coefficient of performance. For cooling equipment this is gross cooling
    /// (pre-DSE) over compressor-only electric input, per AHRI/SEER convention.
    /// None for non-compressor equipment.
    pub cop: Option<f64>,
    /// Main (compressor or primary heat-source) power in kW, excluding fan.
    /// OCHRE HVAC.py:575: main_power = total_input_kw - fan_kw.
    pub main_power_kw: Option<f64>,
}

/// Typed output from one equipment simulation step.
///
/// Not persisted in checkpoints -- reconstructed each step.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CoreOutput {
    pub flows: CoreFlows,
    pub state: CoreState,
    pub performance: CorePerformance,
}

/// Validates that CoreOutput and declared capabilities agree in both directions.
///
/// Declared capabilities must be populated, undeclared capabilities must remain
/// absent, and reactive power is only valid when real electric power is also
/// present.
pub fn validate_core_contract(
    desc: &EquipmentDescriptor,
    co: &CoreOutput,
) -> Result<(), HaresError> {
    let caps = desc.core_capabilities;

    if caps.contains(CoreCapabilities::REACTIVE) && !caps.contains(CoreCapabilities::ELECTRIC) {
        return Err(HaresError::Equipment(format!(
            "core_output contract violation for '{}' ({:?}): REACTIVE requires ELECTRIC",
            desc.name, caps
        )));
    }
    if co.flows.reactive_power_kvar.is_some() && co.flows.electric_kw.is_none() {
        return Err(HaresError::Equipment(format!(
            "core_output contract violation for '{}' ({:?}): flows.reactive_power_kvar requires flows.electric_kw",
            desc.name, caps
        )));
    }

    // Count missing/undeclared-populated fields without allocating.
    let mut missing_bits = 0u16;
    let mut unexpected_bits = 0u16;
    if caps.contains(CoreCapabilities::ELECTRIC) && co.flows.electric_kw.is_none() {
        missing_bits |= 1;
    }
    if !caps.contains(CoreCapabilities::ELECTRIC) && co.flows.electric_kw.is_some() {
        unexpected_bits |= 1;
    }
    if caps.contains(CoreCapabilities::REACTIVE) && co.flows.reactive_power_kvar.is_none() {
        missing_bits |= 2;
    }
    if !caps.contains(CoreCapabilities::REACTIVE) && co.flows.reactive_power_kvar.is_some() {
        unexpected_bits |= 2;
    }
    if caps.contains(CoreCapabilities::FUEL) && co.flows.fuel_w.is_none() {
        missing_bits |= 4;
    }
    if !caps.contains(CoreCapabilities::FUEL) && co.flows.fuel_w.is_some() {
        unexpected_bits |= 4;
    }
    if caps.contains(CoreCapabilities::HAS_SOC) && co.state.soc.is_none() {
        missing_bits |= 8;
    }
    if !caps.contains(CoreCapabilities::HAS_SOC) && co.state.soc.is_some() {
        unexpected_bits |= 8;
    }
    if caps.contains(CoreCapabilities::HAS_MODE) && co.state.operating_mode.is_none() {
        missing_bits |= 16;
    }
    if !caps.contains(CoreCapabilities::HAS_MODE) && co.state.operating_mode.is_some() {
        unexpected_bits |= 16;
    }
    if caps.contains(CoreCapabilities::THERMAL) && co.flows.thermal_output_w.is_none() {
        missing_bits |= 32;
    }
    if !caps.contains(CoreCapabilities::THERMAL) && co.flows.thermal_output_w.is_some() {
        unexpected_bits |= 32;
    }
    if caps.contains(CoreCapabilities::HAS_SPEED) && co.state.speed_index.is_none() {
        missing_bits |= 64;
    }
    if !caps.contains(CoreCapabilities::HAS_SPEED) && co.state.speed_index.is_some() {
        unexpected_bits |= 64;
    }
    if caps.contains(CoreCapabilities::HAS_SETPOINT) && co.state.setpoint_c.is_none() {
        missing_bits |= 128;
    }
    if !caps.contains(CoreCapabilities::HAS_SETPOINT) && co.state.setpoint_c.is_some() {
        unexpected_bits |= 128;
    }
    if caps.contains(CoreCapabilities::HAS_COP) && co.performance.cop.is_none() {
        missing_bits |= 256;
    }
    if !caps.contains(CoreCapabilities::HAS_COP) && co.performance.cop.is_some() {
        unexpected_bits |= 256;
    }

    if missing_bits == 0 && unexpected_bits == 0 {
        return Ok(());
    }

    // Only build the error string on the failure path.
    static NAMES: [(u16, &str); 9] = [
        (1, "flows.electric_kw"),
        (2, "flows.reactive_power_kvar"),
        (4, "flows.fuel_w"),
        (8, "state.soc"),
        (16, "state.operating_mode"),
        (32, "flows.thermal_output_w"),
        (64, "state.speed_index"),
        (128, "state.setpoint_c"),
        (256, "performance.cop"),
    ];
    let missing: String = NAMES
        .iter()
        .filter(|(bit, _)| missing_bits & bit != 0)
        .map(|(_, name)| *name)
        .collect::<Vec<_>>()
        .join(", ");
    let unexpected: String = NAMES
        .iter()
        .filter(|(bit, _)| unexpected_bits & bit != 0)
        .map(|(_, name)| *name)
        .collect::<Vec<_>>()
        .join(", ");
    let mut details = Vec::new();
    if !missing.is_empty() {
        details.push(format!("missing {missing}"));
    }
    if !unexpected.is_empty() {
        details.push(format!("undeclared fields populated: {unexpected}"));
    }

    Err(HaresError::Equipment(format!(
        "core_output contract violation for '{}' ({:?}): {}",
        desc.name,
        caps,
        details.join("; "),
    )))
}

bitflags! {
    /// Capabilities declared by equipment for CoreOutput validation.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct CoreCapabilities: u16 {
        const ELECTRIC     = 0b0000_0000_0000_0001;
        const REACTIVE     = 0b0000_0000_0000_0010;
        const FUEL         = 0b0000_0000_0000_0100;
        const HAS_SOC      = 0b0000_0000_0000_1000;
        const HAS_MODE     = 0b0000_0000_0001_0000;
        const THERMAL      = 0b0000_0000_0010_0000;
        const HAS_SPEED    = 0b0000_0000_0100_0000;
        const HAS_SETPOINT = 0b0000_0000_1000_0000;
        const HAS_COP      = 0b0000_0001_0000_0000;
    }
}

/// Describes one telemetry channel exposed by an equipment model.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TelemetryField {
    pub name: String,
    pub unit: String,
    pub description: String,
}

macro_rules! impl_id_type {
    ($name:ident, $inner:ty) => {
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl From<$inner> for $name {
            fn from(v: $inner) -> Self {
                Self(v)
            }
        }

        impl From<$name> for $inner {
            fn from(id: $name) -> Self {
                id.0
            }
        }
    };
}

impl_id_type!(EquipmentId, u32);
impl_id_type!(DomainId, u16);
impl_id_type!(LoopId, u16);
impl_id_type!(ProtocolId, u16);

/// Static metadata advertised by each equipment model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EquipmentDescriptor {
    pub id: EquipmentId,
    pub name: String,
    pub end_use: EndUse,
    /// Human-readable equipment type string.
    ///
    /// Use `Cow::Borrowed("Literal")` for compile-time constants (zero allocation).
    /// Use `Cow::Owned(string)` for runtime-constructed names (e.g., Python adapters
    /// or config-driven equipment).
    pub equipment_type: Cow<'static, str>,
    pub zone: Option<ZoneId>,
    pub fuel: FuelType,
    pub stage: ExecutionStage,
    pub control_capabilities: ControlCapabilities,
    pub core_capabilities: CoreCapabilities,
    pub telemetry_fields: Vec<TelemetryField>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ZoneId;

    #[test]
    fn equipment_descriptor_round_trips_through_json() {
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(42),
            name: "Battery #1".to_string(),
            end_use: EndUse::OTHER,
            equipment_type: Cow::Borrowed("Battery"),
            zone: Some(ZoneId(1)),
            fuel: FuelType::Electric,
            stage: ExecutionStage::Electrical,
            control_capabilities: ControlCapabilities::POWER_SETPOINT
                | ControlCapabilities::THERMAL_SETPOINT,
            core_capabilities: CoreCapabilities::ELECTRIC | CoreCapabilities::HAS_SOC,
            telemetry_fields: vec![TelemetryField {
                name: "electric_kw".to_string(),
                unit: "kW".to_string(),
                description: "Active electrical power".to_string(),
            }],
        };

        let json = serde_json::to_string(&descriptor).expect("serialize descriptor");
        let decoded: EquipmentDescriptor =
            serde_json::from_str(&json).expect("deserialize descriptor");
        assert_eq!(decoded, descriptor);
    }

    #[test]
    fn value_types_round_trip_through_json() {
        let id_json = serde_json::to_string(&EquipmentId(7)).expect("serialize equipment id");
        let id: EquipmentId = serde_json::from_str(&id_json).expect("deserialize equipment id");
        assert_eq!(id, EquipmentId(7));

        let domain_json = serde_json::to_string(&DomainId(1)).expect("serialize domain id");
        let domain: DomainId = serde_json::from_str(&domain_json).expect("deserialize domain id");
        assert_eq!(domain, DomainId(1));

        let loop_json = serde_json::to_string(&LoopId(2)).expect("serialize loop id");
        let loop_id: LoopId = serde_json::from_str(&loop_json).expect("deserialize loop id");
        assert_eq!(loop_id, LoopId(2));

        let protocol_json = serde_json::to_string(&ProtocolId(3)).expect("serialize protocol id");
        let protocol: ProtocolId =
            serde_json::from_str(&protocol_json).expect("deserialize protocol id");
        assert_eq!(protocol, ProtocolId(3));

        let end_use_json = serde_json::to_string(&EndUse::VENTILATION).expect("serialize end use");
        let end_use: EndUse = serde_json::from_str(&end_use_json).expect("deserialize end use");
        assert_eq!(end_use, EndUse::VENTILATION);

        let fuel_json = serde_json::to_string(&FuelType::None).expect("serialize fuel type");
        let fuel: FuelType = serde_json::from_str(&fuel_json).expect("deserialize fuel type");
        assert_eq!(fuel, FuelType::None);

        let stage_json =
            serde_json::to_string(&ExecutionStage::Thermal).expect("serialize execution stage");
        let stage: ExecutionStage =
            serde_json::from_str(&stage_json).expect("deserialize execution stage");
        assert_eq!(stage, ExecutionStage::Thermal);

        let mode_json = serde_json::to_string(&OperatingMode::Defrost).expect("serialize mode");
        let mode: OperatingMode = serde_json::from_str(&mode_json).expect("deserialize mode");
        assert_eq!(mode, OperatingMode::Defrost);

        let fluid_json = serde_json::to_string(&FluidType::Glycol).expect("serialize fluid type");
        let fluid: FluidType = serde_json::from_str(&fluid_json).expect("deserialize fluid type");
        assert_eq!(fluid, FluidType::Glycol);

        let lut_json =
            serde_json::to_string(&BatteryLutType::ChargingCurve).expect("serialize lut type");
        let lut: BatteryLutType = serde_json::from_str(&lut_json).expect("deserialize lut type");
        assert_eq!(lut, BatteryLutType::ChargingCurve);
    }

    #[test]
    fn new_operating_mode_variants_round_trip_through_json() {
        let modes = vec![
            OperatingMode::Charging,
            OperatingMode::Discharging,
            OperatingMode::HeatingHP,
            OperatingMode::HeatingER,
            OperatingMode::HeatingHPAndER,
            OperatingMode::HeatPumpWH,
            OperatingMode::BackupElement,
        ];
        for mode in modes {
            let json = serde_json::to_string(&mode).expect("serialize mode");
            let decoded: OperatingMode = serde_json::from_str(&json).expect("deserialize mode");
            assert_eq!(decoded, mode);
        }
    }

    #[test]
    fn equipment_descriptor_with_no_zone_round_trips() {
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(1),
            name: "Test".to_string(),
            end_use: EndUse::OTHER,
            equipment_type: Cow::Borrowed("Generic"),
            zone: None,
            fuel: FuelType::None,
            stage: ExecutionStage::Independent,
            control_capabilities: ControlCapabilities::empty(),
            core_capabilities: CoreCapabilities::empty(),
            telemetry_fields: vec![],
        };
        let json = serde_json::to_string(&descriptor).expect("serialize");
        let decoded: EquipmentDescriptor = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, descriptor);
    }

    #[test]
    fn end_use_as_str_returns_stable_identifiers() {
        assert_eq!(EndUse::HVAC_HEATING.as_str(), "hvac_heating");
        assert_eq!(EndUse::HVAC_COOLING.as_str(), "hvac_cooling");
        assert_eq!(EndUse::WATER_HEATING.as_str(), "water_heating");
        assert_eq!(EndUse::LIGHTING.as_str(), "lighting");
        assert_eq!(EndUse::PLUG_LOADS.as_str(), "plug_loads");
        assert_eq!(EndUse::REFRIGERATION.as_str(), "refrigeration");
        assert_eq!(EndUse::VENTILATION.as_str(), "ventilation");
        assert_eq!(EndUse::BATTERY.as_str(), "battery");
        assert_eq!(EndUse::PV.as_str(), "pv");
        assert_eq!(EndUse::EV.as_str(), "ev");
        assert_eq!(EndUse::GENERATOR.as_str(), "generator");
        assert_eq!(EndUse::DEHUMIDIFIER.as_str(), "dehumidifier");
        assert_eq!(EndUse::OTHER.as_str(), "other");
    }

    #[test]
    fn custom_end_use_creation_and_comparison() {
        let custom = EndUse::custom("heat_pump_water_heater");
        assert_eq!(custom.as_str(), "heat_pump_water_heater");
        assert!(!custom.is_standard());

        // Two custom end uses with same string are equal
        let custom2 = EndUse::custom("heat_pump_water_heater");
        assert_eq!(custom, custom2);

        // Different custom end uses are not equal
        let different = EndUse::custom("ice_storage");
        assert_ne!(custom, different);
    }

    #[test]
    fn custom_end_use_round_trips_through_json() {
        let custom = EndUse::custom("vehicle_to_grid_charger");
        let json = serde_json::to_string(&custom).expect("serialize custom end use");
        let decoded: EndUse = serde_json::from_str(&json).expect("deserialize custom end use");
        assert_eq!(decoded, custom);
        assert_eq!(decoded.as_str(), "vehicle_to_grid_charger");
    }

    #[test]
    fn standard_end_use_is_standard_returns_true() {
        assert!(EndUse::HVAC_HEATING.is_standard());
        assert!(EndUse::BATTERY.is_standard());
        assert!(EndUse::OTHER.is_standard());
    }

    #[test]
    fn custom_end_use_is_standard_returns_false() {
        let custom = EndUse::custom("novel_equipment_type");
        assert!(!custom.is_standard());
    }

    #[test]
    fn custom_end_use_in_equipment_descriptor_round_trips() {
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(42),
            name: "NovelDevice".to_string(),
            end_use: EndUse::custom("my_custom_category"),
            equipment_type: Cow::Borrowed("CustomType"),
            zone: Some(ZoneId(1)),
            fuel: FuelType::Electric,
            stage: ExecutionStage::Independent,
            control_capabilities: ControlCapabilities::POWER_SETPOINT,
            core_capabilities: CoreCapabilities::empty(),
            telemetry_fields: vec![],
        };

        let json =
            serde_json::to_string(&descriptor).expect("serialize descriptor with custom end use");
        let decoded: EquipmentDescriptor = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(decoded.end_use, EndUse::custom("my_custom_category"));
        assert!(!decoded.end_use.is_standard());
    }

    #[test]
    fn end_use_from_string_and_static_str() {
        let from_static: EndUse = "hvac_heating".into();
        assert_eq!(from_static, EndUse::HVAC_HEATING);

        let from_string: EndUse = "custom_from_string".to_string().into();
        assert_eq!(from_string.as_str(), "custom_from_string");
    }

    #[test]
    fn ideal_capacity_mode_default_is_auto() {
        let mode: IdealCapacityMode = Default::default();
        assert_eq!(mode, IdealCapacityMode::Auto);
    }

    #[test]
    fn ideal_capacity_mode_round_trips_through_json() {
        let modes = vec![
            IdealCapacityMode::Auto,
            IdealCapacityMode::On,
            IdealCapacityMode::Off,
        ];
        for mode in modes {
            let json = serde_json::to_string(&mode).expect("serialize mode");
            let decoded: IdealCapacityMode = serde_json::from_str(&json).expect("deserialize mode");
            assert_eq!(decoded, mode);
        }
    }

    #[test]
    fn battery_chemistry_round_trips_through_json() {
        for chem in [
            BatteryChemistry::Nmc,
            BatteryChemistry::Lfp,
            BatteryChemistry::Nca,
            BatteryChemistry::Lto,
        ] {
            let json = serde_json::to_string(&chem).expect("serialize");
            let decoded: BatteryChemistry = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(decoded, chem);
        }
    }

    #[test]
    fn battery_chemistry_from_str_case_insensitive() {
        assert_eq!(
            "nmc".parse::<BatteryChemistry>().unwrap(),
            BatteryChemistry::Nmc
        );
        assert_eq!(
            "NMC".parse::<BatteryChemistry>().unwrap(),
            BatteryChemistry::Nmc
        );
        assert_eq!(
            "Lfp".parse::<BatteryChemistry>().unwrap(),
            BatteryChemistry::Lfp
        );
        assert!("invalid".parse::<BatteryChemistry>().is_err());
    }

    #[test]
    fn battery_chemistry_display() {
        assert_eq!(BatteryChemistry::Nmc.to_string(), "NMC");
        assert_eq!(BatteryChemistry::Lfp.to_string(), "LFP");
        assert_eq!(BatteryChemistry::Nca.to_string(), "NCA");
        assert_eq!(BatteryChemistry::Lto.to_string(), "LTO");
    }

    #[test]
    fn battery_chemistry_config_str() {
        assert_eq!(BatteryChemistry::Nmc.as_config_str(), "nmc");
        assert_eq!(BatteryChemistry::Lfp.as_config_str(), "lfp");
        assert_eq!(BatteryChemistry::Nca.as_config_str(), "nca");
        assert_eq!(BatteryChemistry::Lto.as_config_str(), "lto");
    }

    #[test]
    fn charging_level_round_trips_through_json() {
        for level in [ChargingLevel::L1, ChargingLevel::L2] {
            let json = serde_json::to_string(&level).expect("serialize");
            let decoded: ChargingLevel = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(decoded, level);
        }
    }

    #[test]
    fn charging_level_from_str_and_display() {
        assert_eq!("l1".parse::<ChargingLevel>().unwrap(), ChargingLevel::L1);
        assert_eq!("L2".parse::<ChargingLevel>().unwrap(), ChargingLevel::L2);
        assert_eq!(ChargingLevel::L1.to_string(), "L1");
        assert_eq!(ChargingLevel::L2.to_string(), "L2");
    }

    #[test]
    fn vehicle_type_round_trips_through_json() {
        for vt in [VehicleType::Bev, VehicleType::Phev] {
            let json = serde_json::to_string(&vt).expect("serialize");
            let decoded: VehicleType = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(decoded, vt);
        }
    }

    #[test]
    fn vehicle_type_from_str_and_display() {
        assert_eq!("bev".parse::<VehicleType>().unwrap(), VehicleType::Bev);
        assert_eq!("PHEV".parse::<VehicleType>().unwrap(), VehicleType::Phev);
        assert_eq!(VehicleType::Bev.to_string(), "BEV");
        assert_eq!(VehicleType::Phev.to_string(), "PHEV");
    }

    #[test]
    fn ev_connection_state_default_is_home_plugged_in() {
        assert_eq!(
            EvConnectionState::default(),
            EvConnectionState::HomePluggedIn
        );
    }

    #[test]
    fn ev_connection_state_from_str_and_display() {
        for state in [
            EvConnectionState::HomePluggedIn,
            EvConnectionState::AwayPluggedIn,
            EvConnectionState::Disconnected,
        ] {
            let s = state.to_string();
            assert_eq!(s.parse::<EvConnectionState>().unwrap(), state);
        }
        assert!("invalid".parse::<EvConnectionState>().is_err());
    }

    #[test]
    fn ev_connection_state_round_trips_through_json() {
        for state in [
            EvConnectionState::HomePluggedIn,
            EvConnectionState::AwayPluggedIn,
            EvConnectionState::Disconnected,
        ] {
            let json = serde_json::to_string(&state).expect("serialize");
            let decoded: EvConnectionState = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(decoded, state);
        }
    }

    #[test]
    fn plug_in_policy_round_trips_through_json() {
        let policies = vec![
            PlugInPolicy::Always,
            PlugInPolicy::LowSoc { threshold: 0.2 },
        ];
        for policy in policies {
            let json = serde_json::to_string(&policy).expect("serialize");
            let decoded: PlugInPolicy = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(decoded, policy);
        }
    }

    #[test]
    fn charging_strategy_round_trips_through_json() {
        let strategies = vec![
            ChargingStrategy::Immediate { target_soc: 0.9 },
            ChargingStrategy::Nightly {
                off_peak_start_hour: 22.0,
                off_peak_end_hour: 6.0,
                target_soc: 0.9,
            },
            ChargingStrategy::LowSoc {
                threshold: 0.2,
                target_soc: 0.8,
            },
            ChargingStrategy::QuickThenWait { partial_soc: 0.5 },
            ChargingStrategy::PreDeparture {
                target_soc: 0.95,
                departure_schedule: vec![],
            },
            ChargingStrategy::TouAware {
                target_soc: 0.85,
                departure_schedule: vec![],
                charge_buffer_hours: 2.0,
            },
            ChargingStrategy::V2H {
                discharge_threshold_soc: 0.8,
                min_soc: 0.2,
            },
            ChargingStrategy::V2G {
                min_soc: 0.2,
                max_export_kw: 7.0,
                price_threshold: 0.15,
            },
            ChargingStrategy::SolarSurplus {
                min_charge_rate_kw: 1.4,
                departure_schedule: vec![],
            },
        ];
        for strategy in strategies {
            let json = serde_json::to_string(&strategy).expect("serialize");
            let decoded: ChargingStrategy = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(decoded, strategy);
        }
    }

    #[test]
    fn charging_strategy_serde_produces_expected_json() {
        let nightly = ChargingStrategy::Nightly {
            off_peak_start_hour: 22.0,
            off_peak_end_hour: 6.0,
            target_soc: 0.9,
        };
        let json = serde_json::to_string(&nightly).expect("serialize");
        let value: serde_json::Value = serde_json::from_str(&json).expect("parse");
        let inner = value.get("Nightly").expect("expected Nightly key");
        assert_eq!(inner["off_peak_start_hour"], 22.0);
        assert_eq!(inner["off_peak_end_hour"], 6.0);
        assert_eq!(inner["target_soc"], 0.9);

        let immediate = ChargingStrategy::Immediate { target_soc: 1.0 };
        let json = serde_json::to_string(&immediate).expect("serialize");
        let value: serde_json::Value = serde_json::from_str(&json).expect("parse");
        assert_eq!(value["Immediate"]["target_soc"], 1.0);
    }

    #[test]
    fn charging_strategy_cross_deserialize_from_raw_json() {
        let raw = r#"{"LowSoc":{"threshold":0.15,"target_soc":0.7}}"#;
        let decoded: ChargingStrategy = serde_json::from_str(raw).expect("deserialize raw JSON");
        assert_eq!(
            decoded,
            ChargingStrategy::LowSoc {
                threshold: 0.15,
                target_soc: 0.7
            }
        );
    }

    #[test]
    fn bms_mode_default_is_manual() {
        assert_eq!(BmsMode::default(), BmsMode::Manual);
    }

    #[test]
    fn bms_mode_self_consumption_serde() {
        let mode = BmsMode::SelfConsumption {
            min_soc: 0.1,
            max_soc: 0.95,
            solar_only_charging: true,
        };
        let json = serde_json::to_string(&mode).unwrap();
        let back: BmsMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, mode);
    }

    #[test]
    fn bms_mode_tou_optimization_serde() {
        let mode = BmsMode::TimeOfUseOptimization {
            reserve_soc: 0.2,
            charge_threshold_percentile: 0.25,
            discharge_threshold_percentile: 0.75,
            solar_only_charging: false,
        };
        let json = serde_json::to_string(&mode).unwrap();
        let back: BmsMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, mode);
    }

    #[test]
    fn bms_mode_backup_reserve_serde() {
        let mode = BmsMode::BackupReserve {
            target_soc: 0.8,
            charge_from_grid: true,
            charge_rate_fraction: 0.5,
        };
        let json = serde_json::to_string(&mode).unwrap();
        let back: BmsMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, mode);
    }

    #[test]
    fn bms_mode_demand_response_nested_serde() {
        let mode = BmsMode::DemandResponse {
            base_mode: Box::new(BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.9,
                solar_only_charging: false,
            }),
            dr_discharge_rate: 0.8,
            min_soc_during_dr: 0.15,
        };
        let json = serde_json::to_string(&mode).unwrap();
        let back: BmsMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, mode);
    }

    #[test]
    fn bms_mode_scheduled_serde() {
        let mode = BmsMode::Scheduled {
            windows: vec![BmsScheduleWindow {
                time_window: BmsTimeWindow {
                    day: crate::DayFilter::Weekdays,
                    start_minute: 0,
                    end_minute: 360,
                },
                action: BmsAction::Charge { rate_fraction: 1.0 },
            }],
        };
        let json = serde_json::to_string(&mode).unwrap();
        let back: BmsMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, mode);
    }

    #[test]
    fn bms_mode_storm_watch_nested_serde() {
        let mode = BmsMode::StormWatch {
            target_soc: 1.0,
            trigger: StormWatchTrigger::WeatherSignal {
                wind_speed_threshold_m_s: 25.0,
            },
            base_mode: Box::new(BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.95,
                solar_only_charging: true,
            }),
        };
        let json = serde_json::to_string(&mode).unwrap();
        let back: BmsMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, mode);
    }

    #[test]
    fn bms_mode_deep_nesting() {
        let mode = BmsMode::StormWatch {
            target_soc: 1.0,
            trigger: StormWatchTrigger::ManualEnable,
            base_mode: Box::new(BmsMode::DemandResponse {
                base_mode: Box::new(BmsMode::SelfConsumption {
                    min_soc: 0.1,
                    max_soc: 0.9,
                    solar_only_charging: false,
                }),
                dr_discharge_rate: 0.7,
                min_soc_during_dr: 0.2,
            }),
        };
        let json = serde_json::to_string(&mode).unwrap();
        let back: BmsMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, mode);
    }

    #[test]
    fn grid_export_rule_default_is_unrestricted() {
        assert_eq!(GridExportRule::default(), GridExportRule::Unrestricted);
    }

    #[test]
    fn bms_mode_manual_serde() {
        let mode = BmsMode::Manual;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, r#""Manual""#);
        let back: BmsMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, mode);
    }

    #[test]
    fn grid_export_rule_serde() {
        for rule in [
            GridExportRule::SolarOnly,
            GridExportRule::Unrestricted,
            GridExportRule::Disabled,
        ] {
            let json = serde_json::to_string(&rule).unwrap();
            let back: GridExportRule = serde_json::from_str(&json).unwrap();
            assert_eq!(back, rule);
        }
    }

    #[test]
    fn bms_mode_validate_rejects_invalid_fractions() {
        let bad_soc = BmsMode::SelfConsumption {
            min_soc: -0.1,
            max_soc: 0.9,
            solar_only_charging: false,
        };
        assert!(bad_soc.validate().is_err());

        let nan_rate = BmsMode::BackupReserve {
            target_soc: 0.8,
            charge_from_grid: true,
            charge_rate_fraction: f64::NAN,
        };
        assert!(nan_rate.validate().is_err());

        let over_one = BmsMode::SelfConsumption {
            min_soc: 0.1,
            max_soc: 1.5,
            solar_only_charging: false,
        };
        assert!(over_one.validate().is_err());
    }

    #[test]
    fn bms_mode_validate_rejects_inverted_soc() {
        let inverted = BmsMode::SelfConsumption {
            min_soc: 0.9,
            max_soc: 0.1,
            solar_only_charging: false,
        };
        assert!(inverted.validate().is_err());
    }

    #[test]
    fn bms_mode_validate_accepts_valid() {
        let mode = BmsMode::SelfConsumption {
            min_soc: 0.1,
            max_soc: 0.9,
            solar_only_charging: false,
        };
        assert!(mode.validate().is_ok());

        assert!(BmsMode::Manual.validate().is_ok());
    }

    #[test]
    fn bms_action_validate_rejects_invalid() {
        assert!(
            BmsAction::Charge {
                rate_fraction: -0.1
            }
            .validate()
            .is_err()
        );
        assert!(
            BmsAction::Discharge { rate_fraction: 1.1 }
                .validate()
                .is_err()
        );
        assert!(
            BmsAction::Hold {
                target_soc: f64::INFINITY
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn bms_action_validate_accepts_valid() {
        assert!(BmsAction::Charge { rate_fraction: 0.5 }.validate().is_ok());
        assert!(BmsAction::Idle.validate().is_ok());
    }

    #[test]
    fn bms_time_window_validate_rejects_invalid() {
        let out_of_range_start = BmsTimeWindow {
            day: crate::DayFilter::Any,
            start_minute: 1440,
            end_minute: 1440,
        };
        assert!(out_of_range_start.validate().is_err());

        let out_of_range_end = BmsTimeWindow {
            day: crate::DayFilter::Any,
            start_minute: 0,
            end_minute: 1441,
        };
        assert!(out_of_range_end.validate().is_err());

        let zero_width = BmsTimeWindow {
            day: crate::DayFilter::Any,
            start_minute: 300,
            end_minute: 300,
        };
        assert!(zero_width.validate().is_err());
    }

    #[test]
    fn bms_time_window_validate_accepts_valid() {
        let normal = BmsTimeWindow {
            day: crate::DayFilter::Weekdays,
            start_minute: 0,
            end_minute: 360,
        };
        assert!(normal.validate().is_ok());

        let full_day = BmsTimeWindow {
            day: crate::DayFilter::Any,
            start_minute: 0,
            end_minute: 1440,
        };
        assert!(full_day.validate().is_ok());

        let wrapping = BmsTimeWindow {
            day: crate::DayFilter::Any,
            start_minute: 1320,
            end_minute: 360,
        };
        assert!(wrapping.validate().is_ok());
    }

    #[test]
    fn bms_mode_scheduled_validate_checks_time_windows() {
        let mode = BmsMode::Scheduled {
            windows: vec![BmsScheduleWindow {
                time_window: BmsTimeWindow {
                    day: crate::DayFilter::Any,
                    start_minute: 1500,
                    end_minute: 360,
                },
                action: BmsAction::Idle,
            }],
        };
        assert!(mode.validate().is_err());
    }

    // ── TARIFF-003: ChargingStrategy extensions ─────────────────────

    #[test]
    fn charging_strategy_v2h_serde() {
        let s = ChargingStrategy::V2H {
            discharge_threshold_soc: 0.8,
            min_soc: 0.2,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: ChargingStrategy = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn charging_strategy_v2g_serde() {
        let s = ChargingStrategy::V2G {
            min_soc: 0.2,
            max_export_kw: 7.6,
            price_threshold: 0.15,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: ChargingStrategy = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn charging_strategy_solar_surplus_serde() {
        let s = ChargingStrategy::SolarSurplus {
            min_charge_rate_kw: 1.4,
            departure_schedule: vec![
                DepartureConstraint {
                    day_filter: crate::DayFilter::Weekdays,
                    departure_minute: 450,
                    target_soc: 0.9,
                },
                DepartureConstraint {
                    day_filter: crate::DayFilter::Weekends,
                    departure_minute: 600,
                    target_soc: 0.7,
                },
            ],
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: ChargingStrategy = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn charging_strategy_tou_aware_backward_compat() {
        let raw = r#"{"TouAware":{"target_soc":0.85}}"#;
        let decoded: ChargingStrategy = serde_json::from_str(raw).unwrap();
        assert_eq!(
            decoded,
            ChargingStrategy::TouAware {
                target_soc: 0.85,
                departure_schedule: vec![],
                charge_buffer_hours: 2.0,
            }
        );
    }

    #[test]
    fn charging_strategy_pre_departure_backward_compat() {
        let raw = r#"{"PreDeparture":{"target_soc":0.95}}"#;
        let decoded: ChargingStrategy = serde_json::from_str(raw).unwrap();
        assert_eq!(
            decoded,
            ChargingStrategy::PreDeparture {
                target_soc: 0.95,
                departure_schedule: vec![],
            }
        );
    }

    #[test]
    fn departure_constraint_serde() {
        use chrono::Weekday;
        let constraints = vec![
            DepartureConstraint {
                day_filter: crate::DayFilter::Weekdays,
                departure_minute: 480,
                target_soc: 0.9,
            },
            DepartureConstraint {
                day_filter: crate::DayFilter::Day(Weekday::Sat),
                departure_minute: 600,
                target_soc: 0.8,
            },
            DepartureConstraint {
                day_filter: crate::DayFilter::Any,
                departure_minute: 0,
                target_soc: 1.0,
            },
        ];
        for dc in &constraints {
            let json = serde_json::to_string(dc).unwrap();
            let back: DepartureConstraint = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, dc);
        }
    }

    #[test]
    fn charging_strategy_tou_aware_with_departures() {
        let s = ChargingStrategy::TouAware {
            target_soc: 0.9,
            departure_schedule: vec![DepartureConstraint {
                day_filter: crate::DayFilter::Weekdays,
                departure_minute: 420,
                target_soc: 0.85,
            }],
            charge_buffer_hours: 3.0,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: ChargingStrategy = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn departure_constraint_validate_rejects_invalid() {
        let bad_minute = DepartureConstraint {
            day_filter: crate::DayFilter::Any,
            departure_minute: 1440,
            target_soc: 0.9,
        };
        assert!(bad_minute.validate().is_err());

        let bad_soc = DepartureConstraint {
            day_filter: crate::DayFilter::Any,
            departure_minute: 480,
            target_soc: 1.5,
        };
        assert!(bad_soc.validate().is_err());

        let nan_soc = DepartureConstraint {
            day_filter: crate::DayFilter::Any,
            departure_minute: 480,
            target_soc: f64::NAN,
        };
        assert!(nan_soc.validate().is_err());
    }

    #[test]
    fn departure_constraint_validate_accepts_valid() {
        let dc = DepartureConstraint {
            day_filter: crate::DayFilter::Weekdays,
            departure_minute: 420,
            target_soc: 0.9,
        };
        assert!(dc.validate().is_ok());

        let edge = DepartureConstraint {
            day_filter: crate::DayFilter::Any,
            departure_minute: 1439,
            target_soc: 1.0,
        };
        assert!(edge.validate().is_ok());
    }

    #[test]
    fn charging_strategy_validate_v2h_rejects_inverted_soc() {
        let s = ChargingStrategy::V2H {
            discharge_threshold_soc: 0.2,
            min_soc: 0.8,
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn charging_strategy_validate_v2h_accepts_valid() {
        let s = ChargingStrategy::V2H {
            discharge_threshold_soc: 0.8,
            min_soc: 0.2,
        };
        assert!(s.validate().is_ok());
    }

    #[test]
    fn charging_strategy_validate_v2g_rejects_invalid() {
        let bad_kw = ChargingStrategy::V2G {
            min_soc: 0.2,
            max_export_kw: -1.0,
            price_threshold: 0.15,
        };
        assert!(bad_kw.validate().is_err());

        let bad_price = ChargingStrategy::V2G {
            min_soc: 0.2,
            max_export_kw: 7.0,
            price_threshold: f64::NAN,
        };
        assert!(bad_price.validate().is_err());
    }

    #[test]
    fn charging_strategy_validate_v2g_accepts_valid() {
        let s = ChargingStrategy::V2G {
            min_soc: 0.2,
            max_export_kw: 7.6,
            price_threshold: 0.15,
        };
        assert!(s.validate().is_ok());
    }

    #[test]
    fn charging_strategy_validate_solar_surplus_rejects_invalid() {
        let bad = ChargingStrategy::SolarSurplus {
            min_charge_rate_kw: -1.0,
            departure_schedule: vec![],
        };
        assert!(bad.validate().is_err());

        let bad_nested = ChargingStrategy::SolarSurplus {
            min_charge_rate_kw: 1.4,
            departure_schedule: vec![DepartureConstraint {
                day_filter: crate::DayFilter::Any,
                departure_minute: 1440,
                target_soc: 0.9,
            }],
        };
        assert!(bad_nested.validate().is_err());
    }

    #[test]
    fn charging_strategy_validate_solar_surplus_accepts_valid() {
        let s = ChargingStrategy::SolarSurplus {
            min_charge_rate_kw: 1.4,
            departure_schedule: vec![DepartureConstraint {
                day_filter: crate::DayFilter::Weekdays,
                departure_minute: 420,
                target_soc: 0.9,
            }],
        };
        assert!(s.validate().is_ok());
    }

    #[test]
    fn charging_strategy_validate_tou_aware_rejects_invalid_buffer() {
        let s = ChargingStrategy::TouAware {
            target_soc: 0.9,
            departure_schedule: vec![],
            charge_buffer_hours: -1.0,
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn charging_strategy_validate_nightly_rejects_invalid_hours() {
        let s = ChargingStrategy::Nightly {
            off_peak_start_hour: 25.0,
            off_peak_end_hour: 6.0,
            target_soc: 0.9,
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn charging_strategy_validate_accepts_all_variants() {
        let valid = vec![
            ChargingStrategy::Immediate { target_soc: 0.9 },
            ChargingStrategy::Nightly {
                off_peak_start_hour: 22.0,
                off_peak_end_hour: 6.0,
                target_soc: 0.9,
            },
            ChargingStrategy::LowSoc {
                threshold: 0.2,
                target_soc: 0.8,
            },
            ChargingStrategy::QuickThenWait { partial_soc: 0.5 },
            ChargingStrategy::PreDeparture {
                target_soc: 0.95,
                departure_schedule: vec![],
            },
            ChargingStrategy::TouAware {
                target_soc: 0.85,
                departure_schedule: vec![],
                charge_buffer_hours: 2.0,
            },
            ChargingStrategy::V2H {
                discharge_threshold_soc: 0.8,
                min_soc: 0.2,
            },
            ChargingStrategy::V2G {
                min_soc: 0.2,
                max_export_kw: 7.6,
                price_threshold: 0.15,
            },
            ChargingStrategy::SolarSurplus {
                min_charge_rate_kw: 1.4,
                departure_schedule: vec![],
            },
        ];
        for s in &valid {
            assert!(s.validate().is_ok(), "expected valid: {s:?}");
        }
    }

    #[test]
    fn charging_strategy_v2g_accepts_negative_price_threshold() {
        let s = ChargingStrategy::V2G {
            min_soc: 0.2,
            max_export_kw: 7.0,
            price_threshold: -0.05,
        };
        assert!(s.validate().is_ok());
    }

    #[test]
    fn charging_strategy_solar_surplus_backward_compat() {
        let raw = r#"{"SolarSurplus":{"min_charge_rate_kw":1.4}}"#;
        let decoded: ChargingStrategy = serde_json::from_str(raw).unwrap();
        assert_eq!(
            decoded,
            ChargingStrategy::SolarSurplus {
                min_charge_rate_kw: 1.4,
                departure_schedule: vec![],
            }
        );
    }

    #[test]
    fn operating_mode_numeric_codes_are_stable() {
        assert_eq!(OperatingMode::Off as u8, 0);
        assert_eq!(OperatingMode::Heating as u8, 1);
        assert_eq!(OperatingMode::Cooling as u8, 2);
        assert_eq!(OperatingMode::Defrost as u8, 3);
        assert_eq!(OperatingMode::Standby as u8, 4);
        assert_eq!(OperatingMode::Charging as u8, 5);
        assert_eq!(OperatingMode::Discharging as u8, 6);
        assert_eq!(OperatingMode::HeatingHP as u8, 7);
        assert_eq!(OperatingMode::HeatingER as u8, 8);
        assert_eq!(OperatingMode::HeatingHPAndER as u8, 9);
        assert_eq!(OperatingMode::HeatPumpWH as u8, 10);
        assert_eq!(OperatingMode::BackupElement as u8, 11);
    }

    #[test]
    fn operating_mode_as_code_returns_f64_discriminant() {
        assert_eq!(OperatingMode::Off.as_code(), 0.0);
        assert_eq!(OperatingMode::Heating.as_code(), 1.0);
        assert_eq!(OperatingMode::BackupElement.as_code(), 11.0);
    }

    #[test]
    fn electric_power_consumption_rejects_negative() {
        assert!(ElectricPower::consumption(-1.0).is_err());
        assert!(ElectricPower::consumption(f64::NAN).is_err());
        assert!(ElectricPower::consumption(f64::INFINITY).is_err());
        assert!(ElectricPower::consumption(0.0).is_ok());
        assert!(ElectricPower::consumption(5.0).is_ok());
    }

    #[test]
    fn electric_power_generation_rejects_negative() {
        assert!(ElectricPower::generation(-0.001).is_err());
        assert!(ElectricPower::generation(0.0).is_ok());
    }

    #[test]
    fn electric_power_bidirectional_rejects_non_finite() {
        assert!(ElectricPower::bidirectional(f64::NAN).is_err());
        assert!(ElectricPower::bidirectional(-5.0).is_ok());
        assert!(ElectricPower::bidirectional(5.0).is_ok());
    }

    #[test]
    fn electric_power_net_consumption_kw_signs() {
        assert_eq!(ElectricPower::Consumption(3.0).net_consumption_kw(), 3.0);
        assert_eq!(ElectricPower::Generation(3.0).net_consumption_kw(), -3.0);
        assert_eq!(
            ElectricPower::Bidirectional(-2.0).net_consumption_kw(),
            -2.0
        );
    }

    #[test]
    fn soc_try_from_rejects_out_of_range() {
        assert!(Soc::try_from(1.5).is_err());
        assert!(Soc::try_from(-0.1).is_err());
        assert!(Soc::try_from(f64::NAN).is_err());
        assert!(Soc::try_from(0.0).is_ok());
        assert!(Soc::try_from(1.0).is_ok());
        assert!(Soc::try_from(0.5).is_ok());
    }

    #[test]
    fn soc_get_round_trips() {
        let soc = Soc::try_from(0.75).unwrap();
        assert_eq!(soc.get(), 0.75);
    }

    #[test]
    fn core_capabilities_round_trips_through_json() {
        let caps = CoreCapabilities::ELECTRIC | CoreCapabilities::HAS_SOC;
        let json = serde_json::to_string(&caps).expect("serialize CoreCapabilities");
        let decoded: CoreCapabilities =
            serde_json::from_str(&json).expect("deserialize CoreCapabilities");
        assert_eq!(decoded, caps);
    }

    #[test]
    fn core_output_default_has_no_fields_set() {
        let out = CoreOutput::default();
        assert!(out.flows.electric_kw.is_none());
        assert!(out.flows.fuel_w.is_none());
        assert!(out.flows.reactive_power_kvar.is_none());
        assert!(out.flows.thermal_output_w.is_none());
        assert!(out.flows.sensible_cooling_w.is_none());
        assert!(out.flows.latent_cooling_w.is_none());
        assert!(out.state.operating_mode.is_none());
        assert!(out.state.soc.is_none());
        assert!(out.state.speed_index.is_none());
        assert!(out.state.setpoint_c.is_none());
        assert!(out.performance.cop.is_none());
        assert!(out.performance.main_power_kw.is_none());
    }

    #[test]
    fn core_output_round_trips_through_json() {
        let out = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(3.5)),
                reactive_power_kvar: Some(0.2),
                fuel_w: None,
                thermal_output_w: Some(8500.0),
                sensible_cooling_w: Some(-6000.0),
                latent_cooling_w: Some(-2500.0),
            },
            state: CoreState {
                operating_mode: Some(OperatingMode::Heating),
                soc: Some(Soc::try_from(0.8).unwrap()),
                speed_index: Some(2),
                setpoint_c: Some(21.0),
            },
            performance: CorePerformance {
                cop: Some(3.5),
                main_power_kw: Some(2.1),
            },
        };
        let json = serde_json::to_string(&out).expect("serialize");
        let decoded: CoreOutput = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, out);
    }

    #[test]
    fn soc_rejects_nan() {
        assert!(Soc::try_from(f64::NAN).is_err());
    }

    #[test]
    fn soc_rejects_infinity() {
        assert!(Soc::try_from(f64::INFINITY).is_err());
    }

    #[test]
    fn electric_power_consumption_rejects_nan() {
        assert!(ElectricPower::consumption(f64::NAN).is_err());
    }

    #[test]
    fn electric_power_consumption_rejects_infinity() {
        assert!(ElectricPower::consumption(f64::INFINITY).is_err());
    }

    #[test]
    fn electric_power_generation_rejects_nan() {
        assert!(ElectricPower::generation(f64::NAN).is_err());
    }

    #[test]
    fn core_capabilities_bit_values_are_stable() {
        assert_eq!(CoreCapabilities::ELECTRIC.bits(), 1u16);
        assert_eq!(CoreCapabilities::REACTIVE.bits(), 2u16);
        assert_eq!(CoreCapabilities::FUEL.bits(), 4u16);
        assert_eq!(CoreCapabilities::HAS_SOC.bits(), 8u16);
        assert_eq!(CoreCapabilities::HAS_MODE.bits(), 16u16);
        assert_eq!(CoreCapabilities::THERMAL.bits(), 32u16);
        assert_eq!(CoreCapabilities::HAS_SPEED.bits(), 64u16);
        assert_eq!(CoreCapabilities::HAS_SETPOINT.bits(), 128u16);
        assert_eq!(CoreCapabilities::HAS_COP.bits(), 256u16);
    }

    #[test]
    fn validate_core_contract_accepts_matching_capabilities() {
        let desc = EquipmentDescriptor {
            id: EquipmentId(1),
            name: "Validator Happy Path".to_string(),
            end_use: EndUse::OTHER,
            equipment_type: Cow::Borrowed("Test"),
            zone: None,
            fuel: FuelType::Electric,
            stage: ExecutionStage::Independent,
            control_capabilities: ControlCapabilities::empty(),
            core_capabilities: CoreCapabilities::ELECTRIC
                | CoreCapabilities::REACTIVE
                | CoreCapabilities::FUEL
                | CoreCapabilities::HAS_SOC
                | CoreCapabilities::HAS_MODE,
            telemetry_fields: vec![],
        };
        let out = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(1.0)),
                reactive_power_kvar: Some(-0.5),
                fuel_w: Some(FuelPower {
                    fuel_type: FuelType::Gas,
                    consumption_w: 500.0,
                }),
                thermal_output_w: None,
                sensible_cooling_w: None,
                latent_cooling_w: None,
            },
            state: CoreState {
                operating_mode: Some(OperatingMode::Standby),
                soc: Some(Soc::try_from(0.5).expect("valid SOC")),
                speed_index: None,
                setpoint_c: None,
            },
            performance: CorePerformance::default(),
        };
        assert!(validate_core_contract(&desc, &out).is_ok());
    }

    #[test]
    fn validate_core_contract_rejects_missing_declared_fields() {
        let desc = EquipmentDescriptor {
            id: EquipmentId(2),
            name: "Validator Failure".to_string(),
            end_use: EndUse::OTHER,
            equipment_type: Cow::Borrowed("Test"),
            zone: None,
            fuel: FuelType::Electric,
            stage: ExecutionStage::Independent,
            control_capabilities: ControlCapabilities::empty(),
            core_capabilities: CoreCapabilities::ELECTRIC
                | CoreCapabilities::REACTIVE
                | CoreCapabilities::HAS_MODE,
            telemetry_fields: vec![],
        };
        let out = CoreOutput::default();
        let err = validate_core_contract(&desc, &out).expect_err("missing fields must error");
        let msg = err.to_string();
        assert!(msg.contains("flows.electric_kw"));
        assert!(msg.contains("flows.reactive_power_kvar"));
        assert!(msg.contains("state.operating_mode"));
    }

    #[test]
    fn validate_core_contract_rejects_undeclared_populated_fields() {
        let desc = EquipmentDescriptor {
            id: EquipmentId(3),
            name: "Validator Unexpected Fields".to_string(),
            end_use: EndUse::OTHER,
            equipment_type: Cow::Borrowed("Test"),
            zone: None,
            fuel: FuelType::Electric,
            stage: ExecutionStage::Independent,
            control_capabilities: ControlCapabilities::empty(),
            core_capabilities: CoreCapabilities::ELECTRIC,
            telemetry_fields: vec![],
        };
        let out = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(0.5)),
                reactive_power_kvar: Some(0.2),
                fuel_w: None,
                thermal_output_w: None,
                sensible_cooling_w: None,
                latent_cooling_w: None,
            },
            state: CoreState {
                operating_mode: Some(OperatingMode::Standby),
                soc: None,
                speed_index: None,
                setpoint_c: None,
            },
            performance: CorePerformance::default(),
        };

        let err = validate_core_contract(&desc, &out)
            .expect_err("undeclared but populated fields must error");
        let msg = err.to_string();
        assert!(msg.contains("flows.reactive_power_kvar"));
        assert!(msg.contains("state.operating_mode"));
    }

    #[test]
    fn validate_core_contract_rejects_reactive_without_electric() {
        let desc = EquipmentDescriptor {
            id: EquipmentId(4),
            name: "Validator Reactive Only".to_string(),
            end_use: EndUse::OTHER,
            equipment_type: Cow::Borrowed("Test"),
            zone: None,
            fuel: FuelType::Electric,
            stage: ExecutionStage::Independent,
            control_capabilities: ControlCapabilities::empty(),
            core_capabilities: CoreCapabilities::REACTIVE,
            telemetry_fields: vec![],
        };
        let out = CoreOutput {
            flows: CoreFlows {
                electric_kw: None,
                reactive_power_kvar: Some(0.1),
                fuel_w: None,
                thermal_output_w: None,
                sensible_cooling_w: None,
                latent_cooling_w: None,
            },
            state: CoreState::default(),
            performance: CorePerformance::default(),
        };

        let err = validate_core_contract(&desc, &out)
            .expect_err("reactive capability without electric must error");
        assert!(
            err.to_string().contains("REACTIVE requires ELECTRIC"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_core_contract_rejects_reactive_flow_without_electric_flow() {
        let desc = EquipmentDescriptor {
            id: EquipmentId(5),
            name: "Validator Reactive Missing Electric Flow".to_string(),
            end_use: EndUse::OTHER,
            equipment_type: Cow::Borrowed("Test"),
            zone: None,
            fuel: FuelType::Electric,
            stage: ExecutionStage::Independent,
            control_capabilities: ControlCapabilities::empty(),
            core_capabilities: CoreCapabilities::ELECTRIC | CoreCapabilities::REACTIVE,
            telemetry_fields: vec![],
        };
        let out = CoreOutput {
            flows: CoreFlows {
                electric_kw: None,
                reactive_power_kvar: Some(0.1),
                fuel_w: None,
                thermal_output_w: None,
                sensible_cooling_w: None,
                latent_cooling_w: None,
            },
            state: CoreState::default(),
            performance: CorePerformance::default(),
        };

        let err = validate_core_contract(&desc, &out)
            .expect_err("reactive flow without electric flow must error");
        assert!(
            err.to_string()
                .contains("flows.reactive_power_kvar requires flows.electric_kw"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn core_capabilities_serde_format_is_pinned() {
        assert_eq!(
            serde_json::to_string(&CoreCapabilities::ELECTRIC).unwrap(),
            "\"ELECTRIC\""
        );
    }

    #[test]
    fn operating_mode_try_from_u8_round_trip() {
        for v in 0u8..=11 {
            let mode = OperatingMode::try_from(v).expect("valid discriminant");
            assert_eq!(mode as u8, v);
        }
        assert!(OperatingMode::try_from(12).is_err());
        assert!(OperatingMode::try_from(255).is_err());
    }

    #[test]
    fn soc_default_is_zero() {
        assert_eq!(Soc::default().get(), 0.0);
    }

    #[test]
    fn fuel_power_new_validates() {
        assert!(FuelPower::new(FuelType::Gas, 100.0).is_ok());
        assert!(FuelPower::new(FuelType::Gas, 0.0).is_ok());
        assert!(FuelPower::new(FuelType::Gas, -1.0).is_err());
        assert!(FuelPower::new(FuelType::Gas, f64::NAN).is_err());
        assert!(FuelPower::new(FuelType::Gas, f64::INFINITY).is_err());
    }
}
