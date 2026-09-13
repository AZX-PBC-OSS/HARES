//! Equipment descriptor and identifier types.
//!
//! `EquipmentId`, `EquipmentDescriptor`, and capability flags that identify
//! equipment instances across crate boundaries.

use std::borrow::Cow;
use std::fmt;

use bitflags::bitflags;
use serde::{Deserialize, Serialize};

use crate::ports::ElectricalAccumulator;
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
    /// Standard cooking end use (gas range, gas grill, stove).
    /// HPXML: Cooking Range (non-ventilation cooking loads).
    pub const COOKING: Self = Self::new("cooking");
    /// Standard laundry end use (clothes washer + clothes dryer).
    /// HPXML: Clothes Washer, Clothes Dryer.
    pub const LAUNDRY: Self = Self::new("laundry");
    /// Standard dishwasher end use.
    /// HPXML: Dishwasher.
    pub const DISHWASHER: Self = Self::new("dishwasher");
    /// Standard pool pump end use.
    /// HPXML: Pool Pump.
    pub const POOL_PUMP: Self = Self::new("pool_pump");
    /// Standard pool heater end use.
    /// HPXML: Pool Heater.
    pub const POOL_HEATER: Self = Self::new("pool_heater");
    /// Standard spa/hot-tub pump end use.
    /// HPXML: Hot Tub Pump (spa pump).
    pub const SPA_PUMP: Self = Self::new("spa_pump");
    /// Standard spa/hot-tub heater end use.
    /// HPXML: Hot Tub Heater (spa heater).
    pub const SPA_HEATER: Self = Self::new("spa_heater");
    /// Standard ceiling fan end use.
    /// HPXML: Ceiling Fan (fans distinct from whole-house ventilation).
    pub const CEILING_FAN: Self = Self::new("ceiling_fan");
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
                | "cooking"
                | "laundry"
                | "dishwasher"
                | "pool_pump"
                | "pool_heater"
                | "spa_pump"
                | "spa_heater"
                | "ceiling_fan"
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
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, strum::EnumIter,
)]
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

    /// Active modes represent equipment that is running — drawing power,
    /// producing thermal output, or moving energy. Off and Standby are excluded.
    pub fn is_active(&self) -> bool {
        !matches!(self, Self::Off | Self::Standby)
    }

    /// Heating variants deliver positive (into-zone) thermal power.
    pub fn is_heating_variant(&self) -> bool {
        matches!(
            self,
            Self::Heating
                | Self::HeatingHP
                | Self::HeatingER
                | Self::HeatingHPAndER
                | Self::HeatPumpWH
                | Self::BackupElement
        )
    }

    /// Cooling variants deliver negative (out-of-zone) thermal power.
    pub fn is_cooling_variant(&self) -> bool {
        matches!(self, Self::Cooling)
    }

    /// Resolve mode ambiguity at the contract boundary for equipment that is
    /// idle or has parasitic loads, using available flow data to pick the
    /// closest semantically-correct variant.
    ///
    /// - `Off` with non-zero flow → best match among `On`, `Heating`, `Cooling`
    ///   depending on the sign and magnitude of `thermal_output_w`.
    /// - Active mode (Heating, Cooling, etc.) with zero flow → `Standby`.
    /// - Otherwise returns `self` unchanged.
    pub fn resolve_idle(self, has_nonzero_flow: bool, thermal_output_w: Option<f64>) -> Self {
        match self {
            Self::Off if has_nonzero_flow => match thermal_output_w {
                Some(t) if t > 0.0 => Self::Heating,
                Some(t) if t < 0.0 => Self::Cooling,
                _ => Self::On,
            },
            m if m.is_active() && !has_nonzero_flow => Self::Standby,
            other => other,
        }
    }
}

impl TryFrom<u8> for OperatingMode {
    type Error = HaresError;

    // Every enum variant must have a corresponding arm in this match.
    // When a new variant is added, update this impl and verify with
    // `all_variants_round_trip_through_tryfrom` in core_output_invariants.rs.
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
            12 => Ok(Self::On),
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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
///
/// Parsed from override JSON: unknown fields are rejected so a mistyped
/// key (e.g. `threshod`) cannot silently drop a configured constraint.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum PlugInPolicy {
    Always,
    LowSoc { threshold: f64 },
}

impl PlugInPolicy {
    /// Validate the `LowSoc` threshold; `Always` carries no values.
    pub fn validate(&self) -> Result<(), crate::HaresError> {
        match self {
            Self::Always => Ok(()),
            Self::LowSoc { threshold } => validate_fraction("LowSoc threshold", *threshold),
        }
    }
}

/// A departure deadline with day-of-week filter and required SoC.
///
/// Deserialized from override JSON nested inside `ChargingStrategy`
/// variants: unknown fields are rejected so a mistyped key cannot silently
/// drop a configured deadline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

/// How the EV resolves conflicts between an external PowerSetpoint and the
/// internal Ready‑By departure deadline.
///
/// Real EVs and smart EVSEs (Tesla scheduled departure, FordPass, Wallbox)
/// default to deadline guarantee: cost optimization yields to departure
/// readiness. VPP and grid‑service deployments may require external authority,
/// where the utility dispatch has absolute control including the risk of a
/// missed departure SOC.
///
/// This is a per‑equipment config‑time setting. It determines the equipment's
/// contract with external controllers (RL agents, HEMS, VPP aggregators,
/// HELICS co‑simulation federates): the equipment advertises whether it will
/// guarantee the departure SOC regardless of external commands, or whether it
/// cedes authority to the external controller.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default, strum::EnumIter,
)]
pub enum ChargingPriority {
    /// BMS deadline enforcement always runs, even when an external
    /// PowerSetpoint is active. When the deadline is urgent, actual power =
    /// `max(bms_required_power, external_setpoint)` — the external setpoint
    /// acts as a soft floor during urgent deadlines, not a hard ceiling.
    /// `PowerLimit` is still applied as a final cap after the max operation.
    ///
    /// This mirrors real‑world smart EVSE behaviour: TOU optimization and
    /// price‑responsive dispatch are honoured when there is ample time, but
    /// the equipment will override cost signals to meet the departure SOC.
    #[default]
    DeadlineGuarantee,

    /// The external controller has absolute authority. The BMS deadline logic
    /// is completely bypassed when a PowerSetpoint is active — the controller
    /// bears sole responsibility for meeting the departure SOC.
    ///
    /// Use this for VPP / grid‑service deployments where the user has
    /// explicitly enrolled in a program that gives the utility dispatch
    /// authority over charging, or for HELICS co‑simulations where an external
    /// federate is the sole charging controller.
    ExternalAuthority,
}

/// Charging strategy governing when and how fast to charge.
///
/// `TouAware` references the TOU rate schedule from the environment/simulation
/// config -- the EV just knows "be TOU-aware" and reads peak periods externally.
/// Parsed from override JSON (`charging_strategy`): unknown fields are
/// rejected so a mistyped key cannot silently drop a configured constraint
/// — several strategies would otherwise silently never charge.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

/// Parsed from override JSON (`grid_export_rule`): unknown fields are
/// rejected so a mistyped key cannot silently drop a configured constraint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub enum GridExportRule {
    SolarOnly,
    #[default]
    Unrestricted,
    Disabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum StormWatchTrigger {
    ManualEnable,
    WeatherSignal {
        wind_speed_threshold_m_s: f64,
        /// Deactivation threshold; defaults to `wind_speed_threshold_m_s * 0.9`
        /// when zero (backward-compatible). Provides Schmitt-trigger hysteresis
        /// to prevent single-step toggling on noisy wind-speed signals.
        #[serde(default)]
        wind_speed_deactivation_threshold_m_s: f64,
    },
}

impl StormWatchTrigger {
    pub fn validate(&self) -> Result<(), crate::HaresError> {
        match self {
            Self::ManualEnable => Ok(()),
            Self::WeatherSignal {
                wind_speed_threshold_m_s,
                wind_speed_deactivation_threshold_m_s,
            } => {
                if !wind_speed_threshold_m_s.is_finite() || *wind_speed_threshold_m_s < 0.0 {
                    return Err(crate::HaresError::Equipment(format!(
                        "wind_speed_threshold_m_s must be finite and >= 0.0, got {wind_speed_threshold_m_s}"
                    )));
                }
                if !wind_speed_deactivation_threshold_m_s.is_finite()
                    || *wind_speed_deactivation_threshold_m_s < 0.0
                {
                    return Err(crate::HaresError::Equipment(format!(
                        "wind_speed_deactivation_threshold_m_s must be finite and >= 0.0, got {wind_speed_deactivation_threshold_m_s}"
                    )));
                }
                Ok(())
            }
        }
    }
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
/// Configures how the `BatteryManagementActor` dispatches charge/discharge
/// control signals relative to PV production, grid prices, and backup needs.
///
/// Parsed from override JSON (`bms_mode`): unknown fields are rejected so a
/// mistyped key cannot silently drop a configured constraint.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub enum BmsMode {
    SelfConsumption {
        min_soc: f64,
        max_soc: f64,
        solar_only_charging: bool,
        /// Deadband (kW) around surplus=0 to prevent charge/discharge toggling.
        /// Surplus must exceed +deadband to enter charge and drop below -deadband
        /// to enter discharge. Default 0.0 preserves prior behaviour.
        #[serde(default)]
        surplus_deadband_kw: f64,
    },
    TimeOfUseOptimization {
        reserve_soc: f64,
        charge_threshold_percentile: f64,
        discharge_threshold_percentile: f64,
        solar_only_charging: bool,
        /// Deadband applied around charge/discharge price thresholds to prevent
        /// toggling when the price signal oscillates. Default 0.0.
        #[serde(default)]
        price_deadband: f64,
        /// Minimum number of steps the TOU charge or discharge action must
        /// persist before changing. `None` (or missing in serialisation) falls
        /// back to the actor-level `min_dwell_steps`.
        #[serde(default)]
        min_duration_steps: Option<usize>,
    },
    BackupReserve {
        target_soc: f64,
        charge_from_grid: bool,
        charge_rate_fraction: f64,
        /// Deadband around `target_soc`. Charging starts when `soc < target_soc - deadband`
        /// and stops when `soc > target_soc + deadband`. Default 0.0.
        #[serde(default)]
        soc_deadband: f64,
    },
    DemandResponse {
        base_mode: Box<BmsMode>,
        dr_discharge_rate: f64,
        min_soc_during_dr: f64,
        /// Deactivation multiplier: when DR is active, it stays active until
        /// `current_price < multiplier * daily_avg_price`. Default 0.0 uses the
        /// activation threshold (2.0 * daily_avg_price) for backward compatibility.
        /// Values like 1.8 provide hysteresis: price must rise above 2.0× avg to
        /// activate, then fall below 1.8× avg to deactivate.
        #[serde(default)]
        dr_deactivation_multiplier: f64,
        /// Minimum number of steps the DR discharge action must persist once
        /// activated before deactivating. `None` (or missing in serialisation)
        /// falls back to the actor-level `min_dwell_steps`.
        #[serde(default)]
        min_duration_steps: Option<usize>,
    },
    Scheduled {
        windows: Vec<BmsScheduleWindow>,
    },
    StormWatch {
        target_soc: f64,
        trigger: StormWatchTrigger,
        base_mode: Box<BmsMode>,
        /// Minimum number of steps the storm watch must persist once activated
        /// before deactivating. `None` (or missing in serialisation) falls back
        /// to the actor-level `min_dwell_steps`.
        #[serde(default)]
        min_duration_steps: Option<usize>,
    },
    #[default]
    Manual,
}

impl BmsMode {
    /// Validate all fraction/SoC fields are finite and in `[0.0, 1.0]`.
    pub fn validate(&self) -> Result<(), crate::HaresError> {
        match self {
            Self::SelfConsumption {
                min_soc,
                max_soc,
                surplus_deadband_kw,
                ..
            } => {
                validate_fraction("min_soc", *min_soc)?;
                validate_fraction("max_soc", *max_soc)?;
                if min_soc > max_soc {
                    return Err(crate::HaresError::Equipment(
                        "min_soc must be <= max_soc".into(),
                    ));
                }
                validate_finite_non_negative("surplus_deadband_kw", *surplus_deadband_kw)?;
                Ok(())
            }
            Self::TimeOfUseOptimization {
                reserve_soc,
                charge_threshold_percentile,
                discharge_threshold_percentile,
                price_deadband,
                min_duration_steps,
                ..
            } => {
                validate_fraction("reserve_soc", *reserve_soc)?;
                validate_fraction("charge_threshold_percentile", *charge_threshold_percentile)?;
                validate_fraction(
                    "discharge_threshold_percentile",
                    *discharge_threshold_percentile,
                )?;
                validate_finite_non_negative("price_deadband", *price_deadband)?;
                validate_positive_if_present("min_duration_steps", *min_duration_steps)
            }
            Self::BackupReserve {
                target_soc,
                charge_rate_fraction,
                soc_deadband,
                ..
            } => {
                validate_fraction("target_soc", *target_soc)?;
                validate_fraction("charge_rate_fraction", *charge_rate_fraction)?;
                validate_finite_non_negative("soc_deadband", *soc_deadband)
            }
            Self::DemandResponse {
                base_mode,
                dr_discharge_rate,
                min_soc_during_dr,
                dr_deactivation_multiplier,
                min_duration_steps,
            } => {
                validate_fraction("dr_discharge_rate", *dr_discharge_rate)?;
                validate_fraction("min_soc_during_dr", *min_soc_during_dr)?;
                validate_finite_non_negative(
                    "dr_deactivation_multiplier",
                    *dr_deactivation_multiplier,
                )?;
                validate_positive_if_present("min_duration_steps", *min_duration_steps)?;
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
                trigger,
                base_mode,
                min_duration_steps,
            } => {
                validate_fraction("target_soc", *target_soc)?;
                trigger.validate()?;
                validate_positive_if_present("min_duration_steps", *min_duration_steps)?;
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

fn validate_finite_non_negative(name: &str, v: f64) -> Result<(), crate::HaresError> {
    if !v.is_finite() || v < 0.0 {
        return Err(crate::HaresError::Equipment(format!(
            "{name} must be finite and >= 0.0, got {v}"
        )));
    }
    Ok(())
}

fn validate_positive_if_present(name: &str, v: Option<usize>) -> Result<(), crate::HaresError> {
    if let Some(n) = v {
        if n == 0 {
            return Err(crate::HaresError::Equipment(format!(
                "{name} must be > 0 when set, got 0; use None to disable"
            )));
        }
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

    /// True when the net power magnitude is zero — no load and no generation.
    pub fn is_zero(&self) -> bool {
        matches!(
            self,
            Self::Consumption(0.0) | Self::Generation(0.0) | Self::Bidirectional(0.0)
        )
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

// Range-validation observability — zero-cost when `observe` is disabled.
//
// Two layers, mirroring `mode_flow_guard`:
// - Atomic counters give cheap lifetime totals ("how many").
// - A fixed-capacity ring of `RangeViolationEvent`s answers "which field,
//   what value, which equipment" for every rejection and warning. The ring
//   is a const-initialised static array of `Copy` events, so recording never
//   heap-allocates; when full, the oldest event is overwritten because
//   diagnostics favour recency over completeness (the counters still hold
//   the lifetime totals).
#[cfg(feature = "observe")]
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(feature = "observe")]
static RANGE_REJECTION_COUNT: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "observe")]
static RANGE_WARNING_COUNT: AtomicU64 = AtomicU64::new(0);

/// Severity of a recorded physical-range violation.
#[cfg(feature = "observe")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeViolationSeverity {
    /// Physically impossible value; [`validate_core_contract`] returned `Err`.
    Rejection,
    /// Implausible but physically possible value; validation passed with a
    /// `tracing::warn!`.
    Warning,
}

/// One recorded physical-range violation from [`validate_core_contract`].
#[cfg(feature = "observe")]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RangeViolationEvent {
    /// `CoreOutput` field path, e.g. `"performance.cop"`.
    pub field: &'static str,
    /// The offending value.
    pub value: f64,
    /// [`EquipmentId`] inner value of the equipment that produced the value.
    pub equipment_id: u32,
    pub severity: RangeViolationSeverity,
}

/// Ring capacity. 256 comfortably exceeds any plausible violation burst
/// between diagnostic polls while keeping the static footprint small
/// (256 × 32 B = 8 KiB); fixed so recording never allocates.
#[cfg(feature = "observe")]
const RANGE_EVENT_CAPACITY: usize = 256;

#[cfg(feature = "observe")]
struct RangeEventRing {
    events: [Option<RangeViolationEvent>; RANGE_EVENT_CAPACITY],
    /// Index of the next write slot.
    next: usize,
    /// Number of valid events; saturates at capacity.
    len: usize,
}

#[cfg(feature = "observe")]
impl RangeEventRing {
    const fn new() -> Self {
        Self {
            events: [None; RANGE_EVENT_CAPACITY],
            next: 0,
            len: 0,
        }
    }

    fn push(&mut self, event: RangeViolationEvent) {
        self.events[self.next] = Some(event);
        self.next = (self.next + 1) % RANGE_EVENT_CAPACITY;
        if self.len < RANGE_EVENT_CAPACITY {
            self.len += 1;
        }
    }

    fn snapshot(&self) -> Vec<RangeViolationEvent> {
        // Oldest-first: once the ring has wrapped, the oldest entry sits at
        // `next` (the slot about to be overwritten); before wrapping it is 0.
        let start = if self.len == RANGE_EVENT_CAPACITY {
            self.next
        } else {
            0
        };
        (0..self.len)
            .filter_map(|i| self.events[(start + i) % RANGE_EVENT_CAPACITY])
            .collect()
    }
}

#[cfg(feature = "observe")]
static RANGE_EVENTS: std::sync::Mutex<RangeEventRing> =
    std::sync::Mutex::new(RangeEventRing::new());

#[cfg(feature = "observe")]
fn lock_range_events() -> std::sync::MutexGuard<'static, RangeEventRing> {
    // A poisoned lock only means another thread panicked mid-push; the ring
    // holds Copy data and stays structurally valid, so recover the guard
    // rather than propagating the poison.
    RANGE_EVENTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(feature = "observe")]
fn record_range_rejection(field: &'static str, value: f64, equipment_id: u32) {
    RANGE_REJECTION_COUNT.fetch_add(1, Ordering::Relaxed);
    lock_range_events().push(RangeViolationEvent {
        field,
        value,
        equipment_id,
        severity: RangeViolationSeverity::Rejection,
    });
}

#[cfg(not(feature = "observe"))]
fn record_range_rejection(_field: &'static str, _value: f64, _equipment_id: u32) {}

#[cfg(feature = "observe")]
fn record_range_warning(field: &'static str, value: f64, equipment_id: u32) {
    RANGE_WARNING_COUNT.fetch_add(1, Ordering::Relaxed);
    lock_range_events().push(RangeViolationEvent {
        field,
        value,
        equipment_id,
        severity: RangeViolationSeverity::Warning,
    });
}

#[cfg(not(feature = "observe"))]
fn record_range_warning(_field: &'static str, _value: f64, _equipment_id: u32) {}

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
        let checks: [(&str, Option<f64>); 7] = [
            ("flows.reactive_power_kvar", co.flows.reactive_power_kvar),
            ("flows.thermal_output_w", co.flows.thermal_output_w),
            ("flows.sensible_cooling_w", co.flows.sensible_cooling_w),
            ("flows.latent_cooling_w", co.flows.latent_cooling_w),
            ("state.setpoint_c", co.state.setpoint_c),
            ("performance.cop", co.performance.cop),
            ("performance.main_power_kw", co.performance.main_power_kw),
        ];
        // Runtime finiteness guard — always present. Returns Err for
        // non-finite values (NaN, ±∞) that would silently corrupt
        // downstream telemetry, ports, and checkpoint serialization.
        for (name, value) in &checks {
            if let Some(v) = value {
                if !v.is_finite() {
                    return Err(HaresError::Equipment(format!(
                        "core_output contract violation for '{}' ({:?}): non-finite value in {}={}",
                        desc.name, caps, name, v,
                    )));
                }
            }
        }

        // --- physical-range validation ---
        // Error for physically impossible values; warn for implausible-but-possible.

        // COP must be > 0: negative implies a degenerate performance curve.
        // COP = Q_thermal / W_electric, both ≥ 0, so negative COP is physically
        // impossible. COP == 0 is valid for equipment that's off or in transition
        // (zero useful output, zero work input — ratio undefined, reported as 0).
        // AHRI 210/240-2023 §6.2: COP is total heating capacity / effective power
        // input, both positive quantities when operating.
        if let Some(c) = co.performance.cop {
            if c < 0.0 {
                record_range_rejection("performance.cop", c, desc.id.0);
                return Err(HaresError::Equipment(format!(
                    "core_output contract violation for '{}' ({:?}): \
                     performance.cop={c} must be >= 0.0; \
                     negative COP is physically impossible \
                     (COP = Q_thermal / W_electric, both ≥ 0 per AHRI 210/240-2023 §6.2)",
                    desc.name, caps
                )));
            }
            if c == 0.0 {
                record_range_warning("performance.cop", c, desc.id.0);
                tracing::warn!(
                    equipment = %desc.name,
                    equipment_id = desc.id.0,
                    cop = c,
                    "performance.cop=0: equipment may be off or in transition; \
                     consider reporting cop=None when no performance metric is available"
                );
            }
            // Upper plausibility bound. HARES equipment models clamp reported
            // COP to [0, 8] (air conditioner, AHRI 210/240-2023 rated cooling
            // COP ≈ 2.3–4.1) and [0, 10] (ASHP heater), and the Carnot limit
            // (ASHRAE HoF 2021 Ch.2: COP_max = T_h / (T_h − T_c)) is ≈ 8–15 at
            // typical residential heating lifts of 20–40 K. COP > 20 therefore
            // indicates a bug (inverted EIR, percent-as-ratio, unit mix-up)
            // rather than real equipment — but it is not physically impossible
            // (Carnot COP diverges as the lift approaches zero), so warn
            // rather than reject.
            if c > 20.0 {
                record_range_warning("performance.cop", c, desc.id.0);
                tracing::warn!(
                    equipment = %desc.name,
                    equipment_id = desc.id.0,
                    cop = c,
                    "performance.cop={c} exceeds the plausible bound of 20.0 for \
                     residential vapor-compression equipment; may indicate an \
                     inverted EIR curve or a percent-as-ratio bug"
                );
            }
        }

        // Main power consumed cannot be negative.
        if let Some(p) = co.performance.main_power_kw {
            if p < 0.0 {
                record_range_rejection("performance.main_power_kw", p, desc.id.0);
                return Err(HaresError::Equipment(format!(
                    "core_output contract violation for '{}' ({:?}): \
                     performance.main_power_kw={p} must be >= 0.0; \
                     negative main power is physically impossible",
                    desc.name, caps
                )));
            }
        }

        // Sensible cooling by documented convention is ≤ 0 (heat removed from zone).
        if let Some(sc) = co.flows.sensible_cooling_w {
            if sc > 0.0 {
                record_range_rejection("flows.sensible_cooling_w", sc, desc.id.0);
                return Err(HaresError::Equipment(format!(
                    "core_output contract violation for '{}' ({:?}): \
                     flows.sensible_cooling_w={sc} must be <= 0.0; \
                     positive sensible cooling violates the sign convention \
                     (negative = heat removed from zone)",
                    desc.name, caps
                )));
            }
        }

        // Latent cooling by documented convention is ≤ 0 (moisture condensed from zone air).
        if let Some(lc) = co.flows.latent_cooling_w {
            if lc > 0.0 {
                record_range_rejection("flows.latent_cooling_w", lc, desc.id.0);
                return Err(HaresError::Equipment(format!(
                    "core_output contract violation for '{}' ({:?}): \
                     flows.latent_cooling_w={lc} must be <= 0.0; \
                     positive latent cooling violates the sign convention \
                     (negative = moisture condensed from zone)",
                    desc.name, caps
                )));
            }
        }

        // setpoint_c: [-50, 80] °C is the plausible residential HVAC range.
        // Values outside this are physically possible (extreme arctic climates,
        // industrial process heat) but rare — warn rather than reject.
        // ASHRAE HoF 2021 Ch.18: residential heating setpoints rarely below 15°C;
        // ASHRAE 55-2020 §5.3: typical occupied range 20-30°C.
        if let Some(sp) = co.state.setpoint_c {
            if !(-50.0..=80.0).contains(&sp) {
                record_range_warning("state.setpoint_c", sp, desc.id.0);
                if sp > 100.0 {
                    tracing::warn!(
                        equipment = %desc.name,
                        equipment_id = desc.id.0,
                        setpoint_c = sp,
                        "setpoint_c={sp} °C outside plausible residential HVAC range [-50, 80] \
                         and may indicate a Fahrenheit-to-Celsius conversion bug"
                    );
                } else {
                    tracing::warn!(
                        equipment = %desc.name,
                        equipment_id = desc.id.0,
                        setpoint_c = sp,
                        "setpoint_c={sp} °C outside plausible residential HVAC range [-50, 80]"
                    );
                }
            }
        }

        // thermal_output_w: [-100 kW, 100 kW] soft plausibility range, warn-only.
        // The largest residential heat pumps and furnaces deliver ≈ 35–40 kW
        // (120–140 kBtu/h); 100 kW leaves ~2.5× headroom for central
        // multi-family plant while still catching W-vs-kW unit bugs (×1000)
        // and performance-curve blow-ups. HARES's scope is residential load
        // simulation — commercial HVAC equipment that could legitimately
        // exceed this range is out of scope by design (constitution, "Scope:
        // residential load profile simulation"), so no per-equipment-class
        // allowlist is needed; any value past the bound is a diagnostic
        // signal, never a block.
        if let Some(t) = co.flows.thermal_output_w {
            if !(-100_000.0..=100_000.0).contains(&t) {
                record_range_warning("flows.thermal_output_w", t, desc.id.0);
                tracing::warn!(
                    equipment = %desc.name,
                    equipment_id = desc.id.0,
                    thermal_output_w = t,
                    "thermal_output_w={t} W outside residential plausibility range \
                     [-100, 100] kW; may indicate a W-vs-kW unit bug or a \
                     performance-curve blow-up"
                );
            }
        }

        // --- mode-vs-flows consistency checks ---
        crate::mode_flow_guard::check_mode_flow_consistency(desc, co)
            .map_err(|v| HaresError::Equipment(v.message))?;
        crate::mode_flow_guard::invariant_recheck_mode_flow_consistency(desc, co);
        invariant_recheck_range_consistency(desc, co);

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

/// Invariant re-check for physical-range validation.
///
/// Re-runs the same range constraints independently and logs a warning if a
/// violation is found. Called after the production check has passed — a
/// warning here means the production path has a logic bug.
///
/// Present only in debug or `check_invariants` builds.
#[cfg(any(debug_assertions, feature = "check_invariants"))]
fn invariant_recheck_range_consistency(desc: &EquipmentDescriptor, co: &CoreOutput) {
    if let Some(c) = co.performance.cop {
        if c < 0.0 {
            tracing::warn!(
                equipment = %desc.name,
                cop = c,
                "invariant violation: negative COP ({c}) passed production check"
            );
        }
    }
    if let Some(p) = co.performance.main_power_kw {
        if p < 0.0 {
            tracing::warn!(
                equipment = %desc.name,
                main_power_kw = p,
                "invariant violation: negative main_power_kw ({p}) passed production check"
            );
        }
    }
    if let Some(sc) = co.flows.sensible_cooling_w {
        if sc > 0.0 {
            tracing::warn!(
                equipment = %desc.name,
                sensible_cooling_w = sc,
                "invariant violation: positive sensible_cooling_w ({sc}) passed production check"
            );
        }
    }
    if let Some(lc) = co.flows.latent_cooling_w {
        if lc > 0.0 {
            tracing::warn!(
                equipment = %desc.name,
                latent_cooling_w = lc,
                "invariant violation: positive latent_cooling_w ({lc}) passed production check"
            );
        }
    }
}

#[cfg(not(any(debug_assertions, feature = "check_invariants")))]
fn invariant_recheck_range_consistency(_desc: &EquipmentDescriptor, _co: &CoreOutput) {}

/// Returns the total number of physical-range rejections in this process.
#[cfg(feature = "observe")]
pub fn range_rejection_counter() -> u64 {
    RANGE_REJECTION_COUNT.load(Ordering::Relaxed)
}

/// Returns the total number of physical-range warnings in this process.
#[cfg(feature = "observe")]
pub fn range_warning_counter() -> u64 {
    RANGE_WARNING_COUNT.load(Ordering::Relaxed)
}

/// Returns every recorded range-violation diagnostic event, oldest first.
///
/// The backing ring holds the most recent 256 events; older events are
/// overwritten. Use [`range_rejection_counter`] / [`range_warning_counter`]
/// for lifetime totals.
#[cfg(feature = "observe")]
pub fn range_violation_events() -> Vec<RangeViolationEvent> {
    lock_range_events().snapshot()
}

/// Clears recorded range-violation events. The lifetime counters are
/// unaffected.
#[cfg(feature = "observe")]
pub fn clear_range_violation_events() {
    *lock_range_events() = RangeEventRing::new();
}

/// Tolerance for [`validate_port_core_electrical_consistency`]: ~1e-6
/// relative for large magnitudes, 1e-6 absolute near zero. Wide enough to
/// absorb kW<->W round-trip float error, tight enough to catch sign flips,
/// unit mistakes, and dropped contributions.
const PORT_CORE_ELECTRICAL_TOL: f64 = 1e-6;

/// True when `expected` and `actual` differ by more than the combined
/// absolute/relative tolerance [`PORT_CORE_ELECTRICAL_TOL`]. A non-finite
/// difference (NaN/Inf leaking through the port or CoreOutput) is always a
/// mismatch.
fn port_core_mismatch(expected: f64, actual: f64) -> bool {
    let diff = (expected - actual).abs();
    !diff.is_finite() || diff > PORT_CORE_ELECTRICAL_TOL * expected.abs().max(actual.abs()).max(1.0)
}

/// Validates that the electrical port contributions an equipment deposited
/// during one `step()` agree with the [`CoreOutput`] it reported for that
/// same step.
///
/// `pre` is a snapshot of the shared bus accumulator taken immediately
/// before the equipment stepped ([`ElectricalAccumulator`] is `Copy`);
/// `post` is the accumulator after the step, so the deltas are exactly this
/// equipment's contribution.
///
/// Checks (tolerance ~1e-6, relative for large magnitudes):
/// - `flows.electric_kw == Some(Consumption(kw))` requires a port load delta
///   of `kw * 1000` W and a zero generation delta;
/// - `Some(Generation(kw))` requires a port generation delta of `-kw * 1000`
///   W and a zero load delta (catches sign-flipped generation);
/// - `Some(Bidirectional(kw))` requires a net active delta of `kw * 1000` W
///   (positive = consuming, negative = generating, per [`ElectricPower`]);
/// - `None` requires zero active-power contribution;
/// - the reactive port delta must equal
///   `flows.reactive_power_kvar.unwrap_or(0.0)` (signed, positive =
///   inductive/absorbing): equipment reporting `None` must contribute
///   exactly zero reactive power at the port.
pub fn validate_port_core_electrical_consistency(
    desc: &EquipmentDescriptor,
    co: &CoreOutput,
    pre: ElectricalAccumulator,
    post: &ElectricalAccumulator,
) -> Result<(), HaresError> {
    let load_delta_w = post.load_power_w - pre.load_power_w;
    let generation_delta_w = post.generation_power_w - pre.generation_power_w;
    let reactive_delta_kvar = post.reactive_power_kvar - pre.reactive_power_kvar;

    let fail = |detail: String| -> Result<(), HaresError> {
        Err(HaresError::Equipment(format!(
            "port/core electrical consistency violation for '{}': {detail} \
             (port deltas this step: load {load_delta_w} W, generation \
             {generation_delta_w} W, reactive {reactive_delta_kvar} kvar)",
            desc.name,
        )))
    };

    match co.flows.electric_kw {
        None => {
            if port_core_mismatch(0.0, load_delta_w) || port_core_mismatch(0.0, generation_delta_w)
            {
                return fail(
                    "flows.electric_kw is None but the electrical port received an \
                     active-power contribution"
                        .to_string(),
                );
            }
        }
        Some(ElectricPower::Consumption(kw)) => {
            let expected_w = kw * 1000.0;
            if port_core_mismatch(expected_w, load_delta_w) {
                return fail(format!(
                    "flows.electric_kw is Consumption({kw} kW) but the port load delta \
                     is {load_delta_w} W (expected {expected_w} W)"
                ));
            }
            if port_core_mismatch(0.0, generation_delta_w) {
                return fail(format!(
                    "flows.electric_kw is Consumption({kw} kW) but the port received a \
                     generation contribution of {generation_delta_w} W (expected 0 W)"
                ));
            }
        }
        Some(ElectricPower::Generation(kw)) => {
            let expected_w = -kw * 1000.0;
            if port_core_mismatch(expected_w, generation_delta_w) {
                return fail(format!(
                    "flows.electric_kw is Generation({kw} kW) but the port generation \
                     delta is {generation_delta_w} W (expected {expected_w} W)"
                ));
            }
            if port_core_mismatch(0.0, load_delta_w) {
                return fail(format!(
                    "flows.electric_kw is Generation({kw} kW) but the port received a \
                     load contribution of {load_delta_w} W (expected 0 W)"
                ));
            }
        }
        Some(ElectricPower::Bidirectional(kw)) => {
            let expected_w = kw * 1000.0;
            let net_delta_w = load_delta_w + generation_delta_w;
            if port_core_mismatch(expected_w, net_delta_w) {
                return fail(format!(
                    "flows.electric_kw is Bidirectional({kw} kW) but the net port \
                     active delta is {net_delta_w} W (expected {expected_w} W)"
                ));
            }
        }
    }

    let expected_q_kvar = co.flows.reactive_power_kvar.unwrap_or(0.0);
    if port_core_mismatch(expected_q_kvar, reactive_delta_kvar) {
        return fail(format!(
            "flows.reactive_power_kvar is {:?} but the port reactive delta is \
             {reactive_delta_kvar} kvar (expected {expected_q_kvar} kvar)",
            co.flows.reactive_power_kvar,
        ));
    }

    Ok(())
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
    /// HPXML-resolved zone type label (e.g. "conditioned", "garage").
    /// Set by water heater constructors when a `<Location>` was parsed;
    /// `None` for equipment types that do not have a zone location concept.
    pub zone_type: Option<String>,
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
            zone_type: None,
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
            OperatingMode::On,
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
            zone_type: None,
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
        assert_eq!(EndUse::COOKING.as_str(), "cooking");
        assert_eq!(EndUse::LAUNDRY.as_str(), "laundry");
        assert_eq!(EndUse::DISHWASHER.as_str(), "dishwasher");
        assert_eq!(EndUse::POOL_PUMP.as_str(), "pool_pump");
        assert_eq!(EndUse::POOL_HEATER.as_str(), "pool_heater");
        assert_eq!(EndUse::SPA_PUMP.as_str(), "spa_pump");
        assert_eq!(EndUse::SPA_HEATER.as_str(), "spa_heater");
        assert_eq!(EndUse::CEILING_FAN.as_str(), "ceiling_fan");
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
        assert!(EndUse::COOKING.is_standard());
        assert!(EndUse::LAUNDRY.is_standard());
        assert!(EndUse::DISHWASHER.is_standard());
        assert!(EndUse::POOL_PUMP.is_standard());
        assert!(EndUse::POOL_HEATER.is_standard());
        assert!(EndUse::SPA_PUMP.is_standard());
        assert!(EndUse::SPA_HEATER.is_standard());
        assert!(EndUse::CEILING_FAN.is_standard());
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
            zone_type: None,
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
            surplus_deadband_kw: 0.0,
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
            price_deadband: 0.0,
            min_duration_steps: None,
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
            soc_deadband: 0.0,
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
                surplus_deadband_kw: 0.0,
            }),
            dr_discharge_rate: 0.8,
            min_soc_during_dr: 0.15,
            dr_deactivation_multiplier: 0.0,
            min_duration_steps: None,
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
                wind_speed_deactivation_threshold_m_s: 0.0,
            },
            base_mode: Box::new(BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.95,
                solar_only_charging: true,
                surplus_deadband_kw: 0.0,
            }),
            min_duration_steps: None,
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
                    surplus_deadband_kw: 0.0,
                }),
                dr_discharge_rate: 0.7,
                min_soc_during_dr: 0.2,
                dr_deactivation_multiplier: 0.0,
                min_duration_steps: None,
            }),
            min_duration_steps: None,
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
            surplus_deadband_kw: 0.0,
        };
        assert!(bad_soc.validate().is_err());

        let nan_rate = BmsMode::BackupReserve {
            target_soc: 0.8,
            charge_from_grid: true,
            charge_rate_fraction: f64::NAN,
            soc_deadband: 0.0,
        };
        assert!(nan_rate.validate().is_err());

        let over_one = BmsMode::SelfConsumption {
            min_soc: 0.1,
            max_soc: 1.5,
            solar_only_charging: false,
            surplus_deadband_kw: 0.0,
        };
        assert!(over_one.validate().is_err());
    }

    #[test]
    fn bms_mode_validate_rejects_inverted_soc() {
        let inverted = BmsMode::SelfConsumption {
            min_soc: 0.9,
            max_soc: 0.1,
            solar_only_charging: false,
            surplus_deadband_kw: 0.0,
        };
        assert!(inverted.validate().is_err());
    }

    #[test]
    fn bms_mode_validate_accepts_valid() {
        let mode = BmsMode::SelfConsumption {
            min_soc: 0.1,
            max_soc: 0.9,
            solar_only_charging: false,
            surplus_deadband_kw: 0.0,
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
        assert_eq!(OperatingMode::On as u8, 12);
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
            zone_type: None,
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
            zone_type: None,
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
            zone_type: None,
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
            zone_type: None,
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
            zone_type: None,
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
    fn validate_core_contract_rejects_nan() {
        let desc = EquipmentDescriptor {
            id: EquipmentId(6),
            name: "Validator NaN".to_string(),
            end_use: EndUse::OTHER,
            equipment_type: Cow::Borrowed("Test"),
            zone: None,
            fuel: FuelType::Electric,
            stage: ExecutionStage::Independent,
            control_capabilities: ControlCapabilities::empty(),
            core_capabilities: CoreCapabilities::THERMAL,
            telemetry_fields: vec![],
            zone_type: None,
        };
        let out = CoreOutput {
            flows: CoreFlows {
                thermal_output_w: Some(f64::NAN),
                ..Default::default()
            },
            ..Default::default()
        };
        let err =
            validate_core_contract(&desc, &out).expect_err("NaN in thermal_output_w must error");
        let msg = err.to_string();
        assert!(
            msg.contains("non-finite value in flows.thermal_output_w"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn validate_core_contract_rejects_infinity() {
        let desc = EquipmentDescriptor {
            id: EquipmentId(7),
            name: "Validator Infinity".to_string(),
            end_use: EndUse::OTHER,
            equipment_type: Cow::Borrowed("Test"),
            zone: None,
            fuel: FuelType::Electric,
            stage: ExecutionStage::Independent,
            control_capabilities: ControlCapabilities::empty(),
            core_capabilities: CoreCapabilities::THERMAL | CoreCapabilities::HAS_SETPOINT,
            telemetry_fields: vec![],
            zone_type: None,
        };

        let out_inf = CoreOutput {
            flows: CoreFlows {
                thermal_output_w: Some(f64::INFINITY),
                ..Default::default()
            },
            state: CoreState {
                setpoint_c: Some(20.0),
                ..Default::default()
            },
            ..Default::default()
        };
        let err = validate_core_contract(&desc, &out_inf)
            .expect_err("INFINITY in thermal_output_w must error");
        let msg = err.to_string();
        assert!(
            msg.contains("non-finite value in flows.thermal_output_w"),
            "unexpected error: {msg}"
        );

        let out_neg_inf = CoreOutput {
            flows: CoreFlows {
                thermal_output_w: Some(1000.0),
                ..Default::default()
            },
            state: CoreState {
                setpoint_c: Some(f64::NEG_INFINITY),
                ..Default::default()
            },
            ..Default::default()
        };
        let err = validate_core_contract(&desc, &out_neg_inf)
            .expect_err("NEG_INFINITY in setpoint_c must error");
        let msg = err.to_string();
        assert!(
            msg.contains("non-finite value in state.setpoint_c"),
            "unexpected error: {msg}"
        );
    }

    fn mode_flow_test_descriptor(name: &str) -> EquipmentDescriptor {
        EquipmentDescriptor {
            id: EquipmentId(8),
            name: name.to_string(),
            end_use: EndUse::OTHER,
            equipment_type: Cow::Borrowed("Test"),
            zone: None,
            fuel: FuelType::Electric,
            stage: ExecutionStage::Independent,
            control_capabilities: ControlCapabilities::empty(),
            core_capabilities: CoreCapabilities::ELECTRIC
                | CoreCapabilities::THERMAL
                | CoreCapabilities::HAS_MODE,
            telemetry_fields: vec![],
            zone_type: None,
        }
    }

    fn battery_test_descriptor(name: &str) -> EquipmentDescriptor {
        EquipmentDescriptor {
            id: EquipmentId(9),
            name: name.to_string(),
            end_use: EndUse::OTHER,
            equipment_type: Cow::Borrowed("Test"),
            zone: None,
            fuel: FuelType::Electric,
            stage: ExecutionStage::Independent,
            control_capabilities: ControlCapabilities::empty(),
            core_capabilities: CoreCapabilities::ELECTRIC | CoreCapabilities::HAS_MODE,
            telemetry_fields: vec![],
            zone_type: None,
        }
    }

    #[test]
    fn validate_core_contract_rejects_active_mode_zero_power() {
        let desc = mode_flow_test_descriptor("Active Zero");
        let out = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(0.0)),
                thermal_output_w: Some(0.0),
                fuel_w: None,
                ..Default::default()
            },
            state: CoreState {
                operating_mode: Some(OperatingMode::Heating),
                ..Default::default()
            },
            ..Default::default()
        };
        let err = validate_core_contract(&desc, &out)
            .expect_err("active mode with zero flows must error");
        assert!(
            err.to_string().contains("active") && err.to_string().contains("zero flows"),
            "unexpected error: {err}",
        );
    }

    #[test]
    fn validate_core_contract_rejects_off_mode_with_power() {
        let desc = mode_flow_test_descriptor("Off With Power");
        let out = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(1.0)),
                thermal_output_w: Some(0.0),
                ..Default::default()
            },
            state: CoreState {
                operating_mode: Some(OperatingMode::Off),
                ..Default::default()
            },
            ..Default::default()
        };
        let err = validate_core_contract(&desc, &out)
            .expect_err("Off mode with non-zero electric must error");
        assert!(
            err.to_string().contains("Off") && err.to_string().contains("non-zero"),
            "unexpected error: {err}",
        );
    }

    #[test]
    fn validate_core_contract_rejects_cooling_with_positive_thermal() {
        let desc = mode_flow_test_descriptor("Cooling With Heat");
        let out = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(1.0)),
                thermal_output_w: Some(1000.0),
                ..Default::default()
            },
            state: CoreState {
                operating_mode: Some(OperatingMode::Cooling),
                ..Default::default()
            },
            ..Default::default()
        };
        let err = validate_core_contract(&desc, &out)
            .expect_err("Cooling mode with positive thermal must error");
        assert!(
            err.to_string().contains("Cooling") || err.to_string().contains("heating"),
            "unexpected error: {err}",
        );
    }

    #[test]
    fn validate_core_contract_allows_heating_with_positive_thermal() {
        let desc = mode_flow_test_descriptor("Heating OK");
        let out = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(1.0)),
                thermal_output_w: Some(1000.0),
                ..Default::default()
            },
            state: CoreState {
                operating_mode: Some(OperatingMode::Heating),
                ..Default::default()
            },
            ..Default::default()
        };
        validate_core_contract(&desc, &out).expect("heating with positive thermal must pass");
    }

    #[test]
    fn validate_core_contract_rejects_charging_with_generation() {
        let desc = battery_test_descriptor("Charge Gen Conflict");
        let out = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Generation(1.0)),
                ..Default::default()
            },
            state: CoreState {
                operating_mode: Some(OperatingMode::Charging),
                ..Default::default()
            },
            ..Default::default()
        };
        let err = validate_core_contract(&desc, &out)
            .expect_err("Charging mode with Generation must error");
        assert!(
            err.to_string().contains("Charging") && err.to_string().contains("generation"),
            "unexpected error: {err}",
        );
    }

    #[test]
    fn validate_core_contract_rejects_discharging_with_consumption() {
        let desc = battery_test_descriptor("Discharge Consume Conflict");
        let out = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(1.0)),
                ..Default::default()
            },
            state: CoreState {
                operating_mode: Some(OperatingMode::Discharging),
                ..Default::default()
            },
            ..Default::default()
        };
        let err = validate_core_contract(&desc, &out)
            .expect_err("Discharging mode with Consumption must error");
        assert!(
            err.to_string().contains("Discharging") && err.to_string().contains("consumption"),
            "unexpected error: {err}",
        );
    }

    #[test]
    fn validate_core_contract_allows_charging_with_consumption() {
        let desc = battery_test_descriptor("Charge OK");
        let out = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(1.0)),
                ..Default::default()
            },
            state: CoreState {
                operating_mode: Some(OperatingMode::Charging),
                ..Default::default()
            },
            ..Default::default()
        };
        validate_core_contract(&desc, &out).expect("Charging with Consumption must pass");
    }

    #[test]
    fn validate_core_contract_allows_discharging_with_generation() {
        let desc = battery_test_descriptor("Discharge OK");
        let out = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Generation(1.0)),
                ..Default::default()
            },
            state: CoreState {
                operating_mode: Some(OperatingMode::Discharging),
                ..Default::default()
            },
            ..Default::default()
        };
        validate_core_contract(&desc, &out).expect("Discharging with Generation must pass");
    }

    #[test]
    fn validate_core_contract_allows_standby_with_small_power() {
        let desc = battery_test_descriptor("Standby OK");
        let out = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(0.001)),
                ..Default::default()
            },
            state: CoreState {
                operating_mode: Some(OperatingMode::Standby),
                ..Default::default()
            },
            ..Default::default()
        };
        validate_core_contract(&desc, &out).expect("Standby with small electric draw must pass");
    }

    #[test]
    fn validate_core_contract_allows_defrost_with_positive_thermal() {
        let desc = mode_flow_test_descriptor("Defrost OK");
        let out = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(1.0)),
                thermal_output_w: Some(500.0),
                ..Default::default()
            },
            state: CoreState {
                operating_mode: Some(OperatingMode::Defrost),
                ..Default::default()
            },
            ..Default::default()
        };
        validate_core_contract(&desc, &out).expect("Defrost with positive thermal must pass");
    }

    #[test]
    fn validate_core_contract_allows_on_mode_with_positive_thermal() {
        let desc = mode_flow_test_descriptor("On OK");
        let out = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(1.0)),
                thermal_output_w: Some(800.0),
                ..Default::default()
            },
            state: CoreState {
                operating_mode: Some(OperatingMode::On),
                ..Default::default()
            },
            ..Default::default()
        };
        validate_core_contract(&desc, &out).expect("On mode with positive thermal must pass");
    }

    fn range_test_descriptor(name: &str) -> EquipmentDescriptor {
        EquipmentDescriptor {
            id: EquipmentId(8),
            name: name.to_string(),
            end_use: EndUse::OTHER,
            equipment_type: Cow::Borrowed("Test"),
            zone: None,
            fuel: FuelType::Electric,
            stage: ExecutionStage::Independent,
            control_capabilities: ControlCapabilities::empty(),
            core_capabilities: CoreCapabilities::ELECTRIC
                | CoreCapabilities::THERMAL
                | CoreCapabilities::HAS_SETPOINT
                | CoreCapabilities::HAS_COP,
            telemetry_fields: vec![],
            zone_type: None,
        }
    }

    #[test]
    fn validate_core_contract_rejects_negative_cop() {
        let desc = range_test_descriptor("Negative COP");
        let out = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(1.0)),
                thermal_output_w: Some(3000.0),
                ..Default::default()
            },
            state: CoreState {
                setpoint_c: Some(20.0),
                ..Default::default()
            },
            performance: CorePerformance {
                cop: Some(-2.3),
                ..Default::default()
            },
        };
        let err = validate_core_contract(&desc, &out).expect_err("negative COP must be rejected");
        assert!(
            err.to_string()
                .contains("performance.cop=-2.3 must be >= 0.0"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_core_contract_allows_zero_cop() {
        let desc = range_test_descriptor("Zero COP");
        let out = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(1.0)),
                thermal_output_w: Some(3000.0),
                ..Default::default()
            },
            state: CoreState {
                setpoint_c: Some(20.0),
                ..Default::default()
            },
            performance: CorePerformance {
                cop: Some(0.0),
                ..Default::default()
            },
        };
        validate_core_contract(&desc, &out)
            .expect("zero COP must be allowed (equipment off / in transition)");
    }

    #[test]
    fn validate_core_contract_rejects_negative_main_power() {
        let desc = range_test_descriptor("Negative Main Power");
        let out = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(1.0)),
                thermal_output_w: Some(3000.0),
                ..Default::default()
            },
            state: CoreState {
                setpoint_c: Some(20.0),
                ..Default::default()
            },
            performance: CorePerformance {
                cop: Some(3.0),
                main_power_kw: Some(-1.0),
            },
        };
        let err = validate_core_contract(&desc, &out)
            .expect_err("negative main_power_kw must be rejected");
        assert!(
            err.to_string()
                .contains("performance.main_power_kw=-1 must be >= 0.0"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_core_contract_rejects_positive_sensible_cooling() {
        let desc = range_test_descriptor("Positive Sensible");
        let out = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(1.0)),
                thermal_output_w: Some(-3000.0),
                sensible_cooling_w: Some(500.0),
                ..Default::default()
            },
            state: CoreState {
                setpoint_c: Some(20.0),
                ..Default::default()
            },
            performance: CorePerformance {
                cop: Some(3.0),
                ..Default::default()
            },
        };
        let err = validate_core_contract(&desc, &out)
            .expect_err("positive sensible_cooling_w must be rejected");
        assert!(
            err.to_string()
                .contains("flows.sensible_cooling_w=500 must be <= 0.0"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_core_contract_rejects_positive_latent_cooling() {
        let desc = range_test_descriptor("Positive Latent");
        let out = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(1.0)),
                thermal_output_w: Some(-3000.0),
                latent_cooling_w: Some(300.0),
                ..Default::default()
            },
            state: CoreState {
                setpoint_c: Some(20.0),
                ..Default::default()
            },
            performance: CorePerformance {
                cop: Some(3.0),
                ..Default::default()
            },
        };
        let err = validate_core_contract(&desc, &out)
            .expect_err("positive latent_cooling_w must be rejected");
        assert!(
            err.to_string()
                .contains("flows.latent_cooling_w=300 must be <= 0.0"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_core_contract_allows_valid_ranges_at_boundaries() {
        let desc = range_test_descriptor("Valid Boundaries");
        let out = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(1.0)),
                thermal_output_w: Some(100_000.0),
                sensible_cooling_w: Some(0.0),
                latent_cooling_w: Some(0.0),
                ..Default::default()
            },
            state: CoreState {
                setpoint_c: Some(80.0),
                ..Default::default()
            },
            performance: CorePerformance {
                // 20.0 is the inclusive upper edge of the COP plausibility band.
                cop: Some(20.0),
                main_power_kw: Some(0.0),
            },
        };
        validate_core_contract(&desc, &out).expect("valid ranges at boundaries must pass");
    }

    #[test]
    fn validate_core_contract_allows_implausibly_high_cop() {
        // COP > 20 is implausible (warn-level diagnostic) but not physically
        // impossible — validation must pass.
        let desc = range_test_descriptor("High COP");
        let out = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(1.0)),
                thermal_output_w: Some(3000.0),
                ..Default::default()
            },
            state: CoreState {
                setpoint_c: Some(20.0),
                ..Default::default()
            },
            performance: CorePerformance {
                cop: Some(25.0),
                ..Default::default()
            },
        };
        validate_core_contract(&desc, &out).expect("implausibly high COP must warn, not reject");
    }

    /// Diagnostic-event capture: only compiled with the `observe` feature,
    /// matching the CI pass `cargo nextest run --workspace -F observe`.
    /// nextest runs each test in its own process, so the process-global ring
    /// and counters are isolated per test; assertions are nevertheless written
    /// to hold even under a shared-process runner (filter by a unique
    /// equipment id, assert containment rather than exact buffer equality).
    #[cfg(feature = "observe")]
    mod observe_capture {
        use super::*;

        fn observed_descriptor(name: &str, id: u32) -> EquipmentDescriptor {
            let mut desc = range_test_descriptor(name);
            desc.id = EquipmentId(id);
            desc
        }

        fn output_with_cop(cop: f64) -> CoreOutput {
            CoreOutput {
                flows: CoreFlows {
                    electric_kw: Some(ElectricPower::Consumption(1.0)),
                    thermal_output_w: Some(3000.0),
                    ..Default::default()
                },
                state: CoreState {
                    setpoint_c: Some(20.0),
                    ..Default::default()
                },
                performance: CorePerformance {
                    cop: Some(cop),
                    ..Default::default()
                },
            }
        }

        #[test]
        fn rejection_records_event_with_field_value_and_equipment_id() {
            let desc = observed_descriptor("Observe Reject", 9001);
            let rejections_before = range_rejection_counter();
            validate_core_contract(&desc, &output_with_cop(-2.3))
                .expect_err("negative COP must be rejected");
            assert!(
                range_rejection_counter() > rejections_before,
                "rejection counter must increment"
            );
            let events = range_violation_events();
            assert!(
                events.contains(&RangeViolationEvent {
                    field: "performance.cop",
                    value: -2.3,
                    equipment_id: 9001,
                    severity: RangeViolationSeverity::Rejection,
                }),
                "expected a rejection event for performance.cop=-2.3, got {events:?}"
            );
        }

        #[test]
        fn warning_records_event_with_field_value_and_equipment_id() {
            let desc = observed_descriptor("Observe Warn", 9002);
            let mut out = output_with_cop(3.0);
            out.state.setpoint_c = Some(150.0);
            let warnings_before = range_warning_counter();
            validate_core_contract(&desc, &out)
                .expect("out-of-range setpoint is warn-only and must pass");
            assert!(
                range_warning_counter() > warnings_before,
                "warning counter must increment"
            );
            let events = range_violation_events();
            assert!(
                events.contains(&RangeViolationEvent {
                    field: "state.setpoint_c",
                    value: 150.0,
                    equipment_id: 9002,
                    severity: RangeViolationSeverity::Warning,
                }),
                "expected a warning event for state.setpoint_c=150, got {events:?}"
            );
        }

        #[test]
        fn event_ring_caps_length_and_evicts_oldest() {
            clear_range_violation_events();
            let desc = observed_descriptor("Observe Ring", 9003);
            // Push capacity + 10 rejections with distinguishable values.
            let total = 266usize;
            for i in 0..total {
                let value = -(1.0 + i as f64);
                validate_core_contract(&desc, &output_with_cop(value))
                    .expect_err("negative COP must be rejected");
            }
            let events = range_violation_events();
            // The ring never exceeds its fixed capacity (256)...
            assert!(
                events.len() <= 256,
                "ring must cap at capacity, got {} events",
                events.len()
            );
            // ...the newest event survives...
            assert!(
                events
                    .iter()
                    .any(|e| e.value == -(total as f64) && e.equipment_id == 9003),
                "newest event must be present"
            );
            // ...and the oldest was evicted (266 pushes > 256 slots; eviction
            // is monotonic, so interleaved events from other sources only
            // evict more, never less).
            assert!(
                !events
                    .iter()
                    .any(|e| e.value == -1.0 && e.equipment_id == 9003),
                "oldest event must have been evicted"
            );
        }
    }

    /// Minimal electric-equipment descriptor for port/core consistency tests.
    fn consistency_test_descriptor(name: &str) -> EquipmentDescriptor {
        EquipmentDescriptor {
            id: EquipmentId(9),
            name: name.to_string(),
            end_use: EndUse::OTHER,
            equipment_type: Cow::Borrowed("Test"),
            zone: None,
            fuel: FuelType::Electric,
            stage: ExecutionStage::Independent,
            control_capabilities: ControlCapabilities::empty(),
            core_capabilities: CoreCapabilities::ELECTRIC,
            telemetry_fields: vec![],
            zone_type: None,
        }
    }

    // The struct update is only "needless" without the `observe` feature,
    // which adds a contribution-count field to ElectricalAccumulator.
    #[allow(clippy::needless_update)]
    fn accumulator(load_w: f64, generation_w: f64, reactive_kvar: f64) -> ElectricalAccumulator {
        ElectricalAccumulator {
            load_power_w: load_w,
            generation_power_w: generation_w,
            reactive_power_kvar: reactive_kvar,
            ..Default::default()
        }
    }

    fn electric_core_output(electric_kw: ElectricPower, reactive_kvar: Option<f64>) -> CoreOutput {
        CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(electric_kw),
                reactive_power_kvar: reactive_kvar,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn port_core_consistency_accepts_agreeing_consumption_and_reactive() {
        let desc = consistency_test_descriptor("Agreeing Load");
        let co = electric_core_output(ElectricPower::Consumption(2.5), Some(0.75));
        // Pre-existing bus state from earlier equipment must not matter.
        let pre = accumulator(1200.0, -400.0, -0.1);
        let post = accumulator(1200.0 + 2500.0, -400.0, -0.1 + 0.75);
        assert!(validate_port_core_electrical_consistency(&desc, &co, pre, &post).is_ok());
    }

    #[test]
    fn port_core_consistency_accepts_agreeing_generation() {
        let desc = consistency_test_descriptor("Agreeing Source");
        let co = electric_core_output(ElectricPower::Generation(3.0), Some(-0.5));
        let pre = accumulator(500.0, 0.0, 0.2);
        let post = accumulator(500.0, -3000.0, 0.2 - 0.5);
        assert!(validate_port_core_electrical_consistency(&desc, &co, pre, &post).is_ok());
    }

    #[test]
    fn port_core_consistency_accepts_bidirectional_both_signs() {
        let desc = consistency_test_descriptor("Battery");
        // Charging: positive kW lands in the load accumulator.
        let co = electric_core_output(ElectricPower::Bidirectional(4.0), None);
        let pre = accumulator(0.0, 0.0, 0.0);
        let post = accumulator(4000.0, 0.0, 0.0);
        assert!(validate_port_core_electrical_consistency(&desc, &co, pre, &post).is_ok());
        // Discharging: negative kW lands in the generation accumulator.
        let co = electric_core_output(ElectricPower::Bidirectional(-4.0), None);
        let post = accumulator(0.0, -4000.0, 0.0);
        assert!(validate_port_core_electrical_consistency(&desc, &co, pre, &post).is_ok());
    }

    #[test]
    fn port_core_consistency_rejects_none_reactive_with_nonzero_port_reactive() {
        // The water-heater bug class: equipment pushes reactive power at the
        // port while reporting flows.reactive_power_kvar = None.
        let desc = consistency_test_descriptor("Divergent Water Heater");
        let co = electric_core_output(ElectricPower::Consumption(4.5), None);
        let pre = accumulator(0.0, 0.0, 0.0);
        let post = accumulator(4500.0, 0.0, 1.1);
        let err = validate_port_core_electrical_consistency(&desc, &co, pre, &post)
            .expect_err("nonzero port reactive with None CoreOutput must error");
        let msg = err.to_string();
        assert!(
            msg.contains("flows.reactive_power_kvar is None"),
            "unexpected error: {msg}"
        );
        assert!(
            msg.contains("Divergent Water Heater"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn port_core_consistency_rejects_reactive_value_disagreement() {
        let desc = consistency_test_descriptor("Sign-Flipped Q");
        // PV-style bug: CoreOutput reports +Q while the port received -Q.
        let co = electric_core_output(ElectricPower::Generation(3.0), Some(0.9));
        let pre = accumulator(0.0, 0.0, 0.0);
        let post = accumulator(0.0, -3000.0, -0.9);
        let err = validate_port_core_electrical_consistency(&desc, &co, pre, &post)
            .expect_err("sign-flipped reactive must error");
        assert!(
            err.to_string().contains("port reactive delta"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn port_core_consistency_rejects_sign_flipped_generation() {
        let desc = consistency_test_descriptor("Backwards PV");
        let co = electric_core_output(ElectricPower::Generation(2.0), None);
        // Bug: generation pushed with positive sign, so it landed in the load
        // accumulator instead of the generation accumulator.
        let pre = accumulator(0.0, 0.0, 0.0);
        let post = accumulator(2000.0, 0.0, 0.0);
        let err = validate_port_core_electrical_consistency(&desc, &co, pre, &post)
            .expect_err("sign-flipped generation must error");
        assert!(
            err.to_string().contains("Generation(2 kW)"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn port_core_consistency_rejects_consumption_missing_from_port() {
        let desc = consistency_test_descriptor("Ghost Load");
        let co = electric_core_output(ElectricPower::Consumption(1.5), None);
        let pre = accumulator(0.0, 0.0, 0.0);
        let post = accumulator(0.0, 0.0, 0.0);
        assert!(
            validate_port_core_electrical_consistency(&desc, &co, pre, &post).is_err(),
            "CoreOutput consumption with no port contribution must error"
        );
    }

    #[test]
    fn port_core_consistency_rejects_port_contribution_without_core_electric() {
        let desc = consistency_test_descriptor("Undeclared Load");
        let co = CoreOutput::default();
        let pre = accumulator(0.0, 0.0, 0.0);
        let post = accumulator(300.0, 0.0, 0.0);
        let err = validate_port_core_electrical_consistency(&desc, &co, pre, &post)
            .expect_err("port contribution with electric_kw None must error");
        assert!(
            err.to_string().contains("flows.electric_kw is None"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn port_core_consistency_tolerance_boundary() {
        let desc = consistency_test_descriptor("Tolerance Probe");
        let pre = accumulator(0.0, 0.0, 0.0);

        // 10 kW load: tolerance is 1e-6 relative -> 0.01 W. A 0.005 W skew
        // passes; a 0.05 W skew fails.
        let co = electric_core_output(ElectricPower::Consumption(10.0), None);
        let post = accumulator(10_000.0 + 0.005, 0.0, 0.0);
        assert!(validate_port_core_electrical_consistency(&desc, &co, pre, &post).is_ok());
        let post = accumulator(10_000.0 + 0.05, 0.0, 0.0);
        assert!(validate_port_core_electrical_consistency(&desc, &co, pre, &post).is_err());

        // Reactive near zero: tolerance floor is 1e-6 absolute.
        let co = electric_core_output(ElectricPower::Consumption(10.0), Some(0.0));
        let post = accumulator(10_000.0, 0.0, 5e-7);
        assert!(validate_port_core_electrical_consistency(&desc, &co, pre, &post).is_ok());
        let post = accumulator(10_000.0, 0.0, 5e-6);
        assert!(validate_port_core_electrical_consistency(&desc, &co, pre, &post).is_err());
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
        for v in 0u8..=12 {
            let mode = OperatingMode::try_from(v).expect("valid discriminant");
            assert_eq!(mode as u8, v);
        }
        assert!(OperatingMode::try_from(13).is_err());
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

    /// Rejection cases not covered by the variant-specific tests above:
    /// non-finite fractions, `QuickThenWait`'s `partial_soc` bounds, `LowSoc`'s
    /// `target_soc` bound, and an invalid constraint nested inside a
    /// `PreDeparture` schedule. A value that parses as JSON but lies outside
    /// its physical domain must not reach the simulation, where it would
    /// silently distort charge decisions.
    #[test]
    fn charging_strategy_validate_rejects_out_of_domain_values() {
        let bad: Vec<ChargingStrategy> = vec![
            ChargingStrategy::Immediate {
                target_soc: f64::NAN,
            },
            ChargingStrategy::QuickThenWait { partial_soc: -0.1 },
            ChargingStrategy::QuickThenWait { partial_soc: 1.5 },
            ChargingStrategy::Nightly {
                off_peak_start_hour: f64::NAN,
                off_peak_end_hour: 6.0,
                target_soc: 0.9,
            },
            ChargingStrategy::LowSoc {
                threshold: 0.3,
                target_soc: 1.1,
            },
            ChargingStrategy::PreDeparture {
                target_soc: 0.9,
                departure_schedule: vec![DepartureConstraint {
                    day_filter: crate::DayFilter::Any,
                    departure_minute: 480,
                    target_soc: 2.0,
                }],
            },
        ];

        for strategy in bad {
            assert!(
                strategy.validate().is_err(),
                "out-of-domain strategy must fail validation: {strategy:?}"
            );
        }
    }

    /// Boundary values are valid: 0.0 and 1.0 fractions, an hour just under
    /// 24, a zero charge buffer, and `V2H`'s `min_soc` exactly equal to its
    /// `discharge_threshold_soc` must all pass so validation cannot reject a
    /// legitimate configuration. The acceptance tests above use only
    /// mid-range values.
    #[test]
    fn charging_strategy_validate_accepts_boundary_valid_values() {
        let good: Vec<ChargingStrategy> = vec![
            ChargingStrategy::Immediate { target_soc: 0.0 },
            ChargingStrategy::Immediate { target_soc: 1.0 },
            ChargingStrategy::QuickThenWait { partial_soc: 1.0 },
            ChargingStrategy::Nightly {
                off_peak_start_hour: 23.999,
                off_peak_end_hour: 0.0,
                target_soc: 1.0,
            },
            ChargingStrategy::LowSoc {
                threshold: 0.0,
                target_soc: 1.0,
            },
            ChargingStrategy::TouAware {
                target_soc: 1.0,
                departure_schedule: vec![],
                charge_buffer_hours: 0.0,
            },
            ChargingStrategy::SolarSurplus {
                min_charge_rate_kw: 0.0,
                departure_schedule: vec![],
            },
            ChargingStrategy::V2H {
                discharge_threshold_soc: 0.5,
                min_soc: 0.5,
            },
            ChargingStrategy::V2G {
                min_soc: 1.0,
                max_export_kw: 0.0,
                price_threshold: 0.0,
            },
        ];

        for strategy in good {
            assert!(
                strategy.validate().is_ok(),
                "boundary-valid strategy must pass validation: {strategy:?}"
            );
        }
    }

    /// `PlugInPolicy::validate` has no direct test elsewhere (only indirect
    /// coverage through EV init): pin the threshold bounds and the
    /// non-finite rejection.
    #[test]
    fn plug_in_policy_validate_bounds_threshold() {
        assert!(PlugInPolicy::Always.validate().is_ok());
        assert!(PlugInPolicy::LowSoc { threshold: 0.0 }.validate().is_ok());
        assert!(PlugInPolicy::LowSoc { threshold: 1.0 }.validate().is_ok());
        assert!(PlugInPolicy::LowSoc { threshold: 1.5 }.validate().is_err());
        assert!(
            PlugInPolicy::LowSoc {
                threshold: f64::NAN
            }
            .validate()
            .is_err()
        );
    }

    // all_standard test deferred to T-1267: `EndUse::all_standard()` does not yet
    // exist. Uncomment when T-1267 lands and `all_standard()` is available.
    // #[test]
    // fn all_standard_includes_new_end_use_constants() {
    //     let all = EndUse::all_standard();
    //     assert!(all.contains(&EndUse::COOKING));
    //     assert!(all.contains(&EndUse::LAUNDRY));
    //     assert!(all.contains(&EndUse::DISHWASHER));
    //     assert!(all.contains(&EndUse::POOL_PUMP));
    //     assert!(all.contains(&EndUse::POOL_HEATER));
    //     assert!(all.contains(&EndUse::SPA_PUMP));
    //     assert!(all.contains(&EndUse::SPA_HEATER));
    //     assert!(all.contains(&EndUse::CEILING_FAN));
    // }
}
