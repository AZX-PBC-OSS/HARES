//! Control signal enum and associated types.
//!
//! Typed setpoints, power targets, SOC targets, and mode overrides that
//! flow from the control layer to equipment.

use bitflags::bitflags;
use serde::{Deserialize, Serialize};

use crate::{EvConnectionState, HaresError, IdealCapacityMode, OperatingMode, ProtocolId};

/// Target component for split duty cycle control (HPWH compressor vs backup element).
///
/// When `None`, the duty cycle applies to the entire equipment (default, backward-compatible).
/// When `Some(Compressor)` or `Some(BackupElement)`, the duty cycle targets only that component,
/// enabling fine-grained demand response (e.g., curtail compressor during peak but allow backup
/// for freeze protection).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DutyCycleComponent {
    /// Heat pump compressor (HPWH, ASHP).
    Compressor,
    /// Backup electric resistance element.
    BackupElement,
}

/// Demand response severity levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum DRLevel {
    Normal = 0,
    Moderate = 1,
    High = 2,
    Critical = 3,
    GridEmergency = 4,
}

/// Typed external control signals consumed by equipment models.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ControlSignal {
    ThermalSetpoint {
        heating_setpoint_c: Option<f64>,
        cooling_setpoint_c: Option<f64>,
        deadband_c: Option<f64>,
    },
    HumiditySetpoint {
        target_rh: f64,
        min_rh: Option<f64>,
        max_rh: Option<f64>,
    },
    PowerSetpoint {
        active_power_kw: f64,
        reactive_power_kvar: Option<f64>,
        /// Minimum SOC floor constraint (e.g. from V2G/V2H discharge strategies).
        /// When set, equipment BMS should clamp discharge to respect this floor.
        #[serde(default)]
        min_soc: Option<f64>,
        /// Maximum SOC ceiling constraint.
        #[serde(default)]
        max_soc: Option<f64>,
    },
    PowerLimit {
        max_power_kw: f64,
        ramp_rate_kw_per_s: Option<f64>,
    },
    SOCTarget {
        target_soc: f64,
        min_soc: Option<f64>,
        max_soc: Option<f64>,
    },
    ModeOverride {
        mode: OperatingMode,
    },
    DutyCycle {
        on_fraction: f64,
        period_s: Option<f64>,
        /// Optional component target for split duty cycle control.
        /// `None` = apply to entire equipment (default). `Some(Compressor)` or
        /// `Some(BackupElement)` targets a single component (HPWH/ASHP).
        #[serde(default)]
        component: Option<DutyCycleComponent>,
    },
    LoadFraction {
        fraction: f64,
    },
    GridConnect {
        connected: bool,
    },
    SelfConsumption {
        enabled: bool,
        solar_only_charging: bool,
    },
    DemandResponse {
        level: DRLevel,
        duration_s: Option<f64>,
    },
    ProtocolNative {
        protocol: ProtocolId,
        payload: Vec<u8>,
    },
    CurtailmentPercent {
        percent: f64,
    },
    ReactiveSetpoint {
        kvar: f64,
    },
    PowerFactorSetpoint {
        power_factor: f64,
    },
    InverterPriorityMode {
        priority: InverterPriority,
    },
    IdealCapacity {
        capacity_w: f64,
        /// True when the ideal capacity value is a degraded fallback
        /// (last-good capacity used after consecutive solver failures).
        /// False for normal, converged solver output.
        #[serde(default)]
        degraded: bool,
    },
    /// Relative setpoint adjustment applied on top of the equipment's current
    /// effective setpoints. Positive `heating_delta_c` raises the heating
    /// setpoint; positive `cooling_delta_c` raises the cooling setpoint.
    ThermalSetpointDelta {
        heating_delta_c: Option<f64>,
        cooling_delta_c: Option<f64>,
    },
    /// Override the ideal capacity mode at runtime (Auto/On/Off).
    IdealCapacityModeOverride {
        mode: IdealCapacityMode,
    },
    /// Set EV connection state (HomePluggedIn, AwayPluggedIn, Disconnected).
    EvPlugIn {
        state: EvConnectionState,
    },
    /// Deduct driving energy from EV SOC. Only valid when Disconnected.
    EvDrive {
        kwh: f64,
    },
    /// Set external charger power for away charging. Only valid when AwayPluggedIn.
    EvAwayCharge {
        power_kw: f64,
    },
    /// Tell EV BMS to be at target_soc by departure_hour.
    EvSetReadyBy {
        departure_hour: f64,
        target_soc: f64,
    },
    /// Defer the next event/cycle start by `delay_s` seconds. Cannot be applied
    /// to an already-active event.
    EventDelay {
        delay_s: f64,
    },
    /// Limit maximum capacity to a fraction of rated capacity [0, 1].
    /// OCHRE HVAC.py: `ext_capacity_frac` -- clips ideal capacity output to
    /// `capacity_max * fraction`. Only meaningful for ideal-capacity equipment.
    MaxCapacityFraction {
        fraction: f64,
    },
}

/// Inverter priority mode for smart inverter Watt/Var/CPF dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InverterPriority {
    Watt,
    Var,
    Cpf,
}

bitflags! {
    /// Equipment-declared set of supported control signal families.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct ControlCapabilities: u32 {
        const POWER_SETPOINT = 1 << 0;
        const SOC_TARGET = 1 << 1;
        const THERMAL_SETPOINT = 1 << 2;
        const POWER_LIMIT = 1 << 3;
        const MODE_OVERRIDE = 1 << 4;
        const DUTY_CYCLE = 1 << 5;
        const LOAD_FRACTION = 1 << 6;
        const GRID_CONNECT = 1 << 7;
        const SELF_CONSUMPTION = 1 << 8;
        const DEMAND_RESPONSE = 1 << 9;
        const PROTOCOL_NATIVE = 1 << 10;
        const HUMIDITY_SETPOINT = 1 << 11;
        const CURTAILMENT_PERCENT = 1 << 12;
        const REACTIVE_SETPOINT = 1 << 13;
        const POWER_FACTOR_SETPOINT = 1 << 14;
        const INVERTER_PRIORITY_MODE = 1 << 15;
        const IDEAL_CAPACITY = 1 << 16;
        const THERMAL_SETPOINT_DELTA = 1 << 17;
        const IDEAL_CAPACITY_MODE_OVERRIDE = 1 << 18;
        const EV_PLUG_IN = 1 << 19;
        const EV_DRIVE = 1 << 20;
        const EV_AWAY_CHARGE = 1 << 21;
        const EV_SET_READY_BY = 1 << 22;
        const EVENT_DELAY = 1 << 23;
        const MAX_CAPACITY_FRACTION = 1 << 24;
    }
}

impl ControlSignal {
    /// Returns `true` for signals whose only effect is updating connection or
    /// mode state (idempotent assignments). These can be applied eagerly before
    /// queuing so that subsequent signals in the same timestep see the updated
    /// state when validated.
    pub fn is_immediate_state_update(&self) -> bool {
        matches!(self, Self::EvPlugIn { .. })
    }

    /// Validate numeric bounds for every `ControlSignal` variant.
    ///
    /// Per-variant range checks reject physically impossible or numerically
    /// dangerous values (NaN, ±∞, out-of-range) that would corrupt simulation
    /// state. The trait boundary (`Equipment::apply_control`) calls this
    /// before dispatching to equipment-specific logic.
    ///
    /// Variants with no numeric fields (mode/boolean/enum signals) trivially
    /// return `Ok(())`.
    pub fn validate_numeric_bounds(&self) -> Result<(), HaresError> {
        match self {
            Self::ThermalSetpoint {
                heating_setpoint_c,
                cooling_setpoint_c,
                deadband_c,
            } => {
                if let Some(h) = heating_setpoint_c {
                    if *h < -50.0 || *h > 100.0 || !h.is_finite() {
                        return Err(HaresError::Control(format!(
                            "ThermalSetpoint heating_setpoint_c invalid: {h}, expected [-50, 100] °C"
                        )));
                    }
                }
                if let Some(c) = cooling_setpoint_c {
                    if *c < 0.0 || *c > 60.0 || !c.is_finite() {
                        return Err(HaresError::Control(format!(
                            "ThermalSetpoint cooling_setpoint_c invalid: {c}, expected [0, 60] °C"
                        )));
                    }
                }
                if let Some(db) = deadband_c {
                    if *db < 0.0 || *db > 5.0 || !db.is_finite() {
                        return Err(HaresError::Control(format!(
                            "ThermalSetpoint deadband_c invalid: {db}, expected [0, 5] °C"
                        )));
                    }
                }
                if let (Some(h), Some(c), Some(db)) =
                    (heating_setpoint_c, cooling_setpoint_c, deadband_c)
                {
                    if h + db >= *c {
                        return Err(HaresError::Control(format!(
                            "ThermalSetpoint: heating ({h}) + deadband ({db}) = {} not < cooling ({c})",
                            h + db
                        )));
                    }
                }
            }
            Self::HumiditySetpoint {
                target_rh,
                min_rh,
                max_rh,
            } => {
                if *target_rh < 0.0 || *target_rh > 1.0 || !target_rh.is_finite() {
                    return Err(HaresError::Control(format!(
                        "HumiditySetpoint target_rh invalid: {target_rh}, expected [0, 1]"
                    )));
                }
                if let Some(r) = min_rh {
                    if *r < 0.0 || *r > 1.0 || !r.is_finite() {
                        return Err(HaresError::Control(format!(
                            "HumiditySetpoint min_rh invalid: {r}, expected [0, 1]"
                        )));
                    }
                    if *r >= *target_rh {
                        return Err(HaresError::Control(format!(
                            "HumiditySetpoint min_rh ({r}) must be < target_rh ({target_rh})"
                        )));
                    }
                }
                if let Some(r) = max_rh {
                    if *r < 0.0 || *r > 1.0 || !r.is_finite() {
                        return Err(HaresError::Control(format!(
                            "HumiditySetpoint max_rh invalid: {r}, expected [0, 1]"
                        )));
                    }
                    if *r <= *target_rh {
                        return Err(HaresError::Control(format!(
                            "HumiditySetpoint max_rh ({r}) must be > target_rh ({target_rh})"
                        )));
                    }
                }
            }
            Self::PowerSetpoint {
                active_power_kw,
                reactive_power_kvar,
                min_soc,
                max_soc,
            } => {
                if !active_power_kw.is_finite() {
                    return Err(HaresError::Control(format!(
                        "PowerSetpoint active_power_kw must be finite, got {active_power_kw}"
                    )));
                }
                if let Some(r) = reactive_power_kvar {
                    if !r.is_finite() {
                        return Err(HaresError::Control(format!(
                            "PowerSetpoint reactive_power_kvar must be finite, got {r}"
                        )));
                    }
                }
                // The SOC window carries the same semantics as SOCTarget's
                // window (a discharge floor / charge ceiling); validate it
                // with the same rules so a garbage window cannot reach any
                // equipment arm and be silently substituted downstream.
                if let Some(m) = min_soc {
                    if *m < 0.0 || *m > 1.0 || !m.is_finite() {
                        return Err(HaresError::Control(format!(
                            "PowerSetpoint min_soc invalid: {m}, expected [0, 1]"
                        )));
                    }
                }
                if let Some(m) = max_soc {
                    if *m < 0.0 || *m > 1.0 || !m.is_finite() {
                        return Err(HaresError::Control(format!(
                            "PowerSetpoint max_soc invalid: {m}, expected [0, 1]"
                        )));
                    }
                }
                if let (Some(min), Some(max)) = (min_soc, max_soc)
                    && min >= max
                {
                    return Err(HaresError::Control(format!(
                        "PowerSetpoint min_soc ({min}) must be < max_soc ({max})"
                    )));
                }
            }
            Self::PowerLimit {
                max_power_kw,
                ramp_rate_kw_per_s,
            } => {
                if !max_power_kw.is_finite() || *max_power_kw < 0.0 {
                    return Err(HaresError::Control(format!(
                        "PowerLimit max_power_kw must be finite and >= 0, got {max_power_kw}"
                    )));
                }
                if let Some(r) = ramp_rate_kw_per_s {
                    if !r.is_finite() || *r < 0.0 {
                        return Err(HaresError::Control(format!(
                            "PowerLimit ramp_rate_kw_per_s must be finite and >= 0, got {r}"
                        )));
                    }
                }
            }
            Self::SOCTarget {
                target_soc,
                min_soc,
                max_soc,
            } => {
                if *target_soc < 0.0 || *target_soc > 1.0 || !target_soc.is_finite() {
                    return Err(HaresError::Control(format!(
                        "SOCTarget target_soc invalid: {target_soc}, expected [0, 1]"
                    )));
                }
                if let Some(m) = min_soc {
                    if *m < 0.0 || *m > 1.0 || !m.is_finite() {
                        return Err(HaresError::Control(format!(
                            "SOCTarget min_soc invalid: {m}, expected [0, 1]"
                        )));
                    }
                }
                if let Some(m) = max_soc {
                    if *m < 0.0 || *m > 1.0 || !m.is_finite() {
                        return Err(HaresError::Control(format!(
                            "SOCTarget max_soc invalid: {m}, expected [0, 1]"
                        )));
                    }
                }
                if let (Some(min), Some(max)) = (min_soc, max_soc) {
                    if *min >= *max {
                        return Err(HaresError::Control(format!(
                            "SOCTarget min_soc ({min}) must be < max_soc ({max})"
                        )));
                    }
                    if *target_soc <= *min || *target_soc >= *max {
                        return Err(HaresError::Control(format!(
                            "SOCTarget target_soc ({target_soc}) must be in ({min}, {max})"
                        )));
                    }
                }
            }
            Self::ModeOverride { .. } => {}
            Self::DutyCycle {
                on_fraction,
                period_s,
                ..
            } => {
                if *on_fraction < 0.0 || *on_fraction > 1.0 || !on_fraction.is_finite() {
                    return Err(HaresError::Control(format!(
                        "DutyCycle on_fraction invalid: {on_fraction}, expected [0, 1]"
                    )));
                }
                if let Some(p) = period_s {
                    if !p.is_finite() || *p < 0.0 {
                        return Err(HaresError::Control(format!(
                            "DutyCycle period_s must be finite and >= 0, got {p}"
                        )));
                    }
                }
            }
            Self::LoadFraction { fraction } => {
                if *fraction < 0.0 || *fraction > 1.0 || !fraction.is_finite() {
                    return Err(HaresError::Control(format!(
                        "LoadFraction fraction invalid: {fraction}, expected [0, 1]"
                    )));
                }
            }
            Self::GridConnect { .. } => {}
            Self::SelfConsumption { .. } => {}
            Self::DemandResponse {
                level: _,
                duration_s,
            } => {
                if let Some(d) = duration_s {
                    if !d.is_finite() || *d < 0.0 {
                        return Err(HaresError::Control(format!(
                            "DemandResponse duration_s must be finite and >= 0, got {d}"
                        )));
                    }
                }
            }
            Self::ProtocolNative { .. } => {}
            Self::CurtailmentPercent { percent } => {
                if *percent < 0.0 || *percent > 100.0 || !percent.is_finite() {
                    return Err(HaresError::Control(format!(
                        "CurtailmentPercent percent invalid: {percent}, expected [0, 100]"
                    )));
                }
            }
            Self::ReactiveSetpoint { kvar } => {
                if !kvar.is_finite() {
                    return Err(HaresError::Control(format!(
                        "ReactiveSetpoint kvar must be finite, got {kvar}"
                    )));
                }
            }
            Self::PowerFactorSetpoint { power_factor } => {
                if *power_factor < 0.0 || *power_factor > 1.0 || !power_factor.is_finite() {
                    return Err(HaresError::Control(format!(
                        "PowerFactorSetpoint power_factor invalid: {power_factor}, expected [0, 1]"
                    )));
                }
            }
            Self::InverterPriorityMode { .. } => {}
            Self::IdealCapacity {
                capacity_w,
                degraded: _,
            } => {
                if !capacity_w.is_finite() {
                    return Err(HaresError::Control(format!(
                        "IdealCapacity capacity_w must be finite, got {capacity_w}"
                    )));
                }
            }
            Self::ThermalSetpointDelta {
                heating_delta_c,
                cooling_delta_c,
            } => {
                if let Some(d) = heating_delta_c {
                    if !d.is_finite() || *d < -20.0 || *d > 20.0 {
                        return Err(HaresError::Control(format!(
                            "ThermalSetpointDelta heating_delta_c invalid: {d}, expected ±20 °C"
                        )));
                    }
                }
                if let Some(d) = cooling_delta_c {
                    if !d.is_finite() || *d < -20.0 || *d > 20.0 {
                        return Err(HaresError::Control(format!(
                            "ThermalSetpointDelta cooling_delta_c invalid: {d}, expected ±20 °C"
                        )));
                    }
                }
            }
            Self::IdealCapacityModeOverride { .. } => {}
            Self::EvPlugIn { .. } => {}
            Self::EvDrive { kwh } => {
                if !kwh.is_finite() || *kwh < 0.0 {
                    return Err(HaresError::Control(format!(
                        "EvDrive kwh must be finite and >= 0, got {kwh}"
                    )));
                }
            }
            Self::EvAwayCharge { power_kw } => {
                if !power_kw.is_finite() || *power_kw < 0.0 {
                    return Err(HaresError::Control(format!(
                        "EvAwayCharge power_kw must be finite and >= 0, got {power_kw}"
                    )));
                }
            }
            Self::EvSetReadyBy {
                departure_hour,
                target_soc,
            } => {
                if *departure_hour < 0.0 || *departure_hour > 24.0 || !departure_hour.is_finite() {
                    return Err(HaresError::Control(format!(
                        "EvSetReadyBy departure_hour invalid: {departure_hour}, expected [0, 24]"
                    )));
                }
                if *target_soc < 0.0 || *target_soc > 1.0 || !target_soc.is_finite() {
                    return Err(HaresError::Control(format!(
                        "EvSetReadyBy target_soc invalid: {target_soc}, expected [0, 1]"
                    )));
                }
            }
            Self::EventDelay { delay_s } => {
                if !delay_s.is_finite() || *delay_s < 0.0 {
                    return Err(HaresError::Control(format!(
                        "EventDelay delay_s must be finite and >= 0, got {delay_s}"
                    )));
                }
            }
            Self::MaxCapacityFraction { fraction } => {
                if *fraction < 0.0 || *fraction > 1.0 || !fraction.is_finite() {
                    return Err(HaresError::Control(format!(
                        "MaxCapacityFraction fraction invalid: {fraction}, expected [0, 1]"
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn required_capability(&self) -> ControlCapabilities {
        match self {
            Self::ThermalSetpoint { .. } => ControlCapabilities::THERMAL_SETPOINT,
            Self::HumiditySetpoint { .. } => ControlCapabilities::HUMIDITY_SETPOINT,
            Self::PowerSetpoint { .. } => ControlCapabilities::POWER_SETPOINT,
            Self::PowerLimit { .. } => ControlCapabilities::POWER_LIMIT,
            Self::SOCTarget { .. } => ControlCapabilities::SOC_TARGET,
            Self::ModeOverride { .. } => ControlCapabilities::MODE_OVERRIDE,
            Self::DutyCycle { .. } => ControlCapabilities::DUTY_CYCLE,
            Self::LoadFraction { .. } => ControlCapabilities::LOAD_FRACTION,
            Self::GridConnect { .. } => ControlCapabilities::GRID_CONNECT,
            Self::SelfConsumption { .. } => ControlCapabilities::SELF_CONSUMPTION,
            Self::DemandResponse { .. } => ControlCapabilities::DEMAND_RESPONSE,
            Self::ProtocolNative { .. } => ControlCapabilities::PROTOCOL_NATIVE,
            Self::CurtailmentPercent { .. } => ControlCapabilities::CURTAILMENT_PERCENT,
            Self::ReactiveSetpoint { .. } => ControlCapabilities::REACTIVE_SETPOINT,
            Self::PowerFactorSetpoint { .. } => ControlCapabilities::POWER_FACTOR_SETPOINT,
            Self::InverterPriorityMode { .. } => ControlCapabilities::INVERTER_PRIORITY_MODE,
            Self::IdealCapacity { .. } => ControlCapabilities::IDEAL_CAPACITY,
            Self::ThermalSetpointDelta { .. } => ControlCapabilities::THERMAL_SETPOINT_DELTA,
            Self::IdealCapacityModeOverride { .. } => {
                ControlCapabilities::IDEAL_CAPACITY_MODE_OVERRIDE
            }
            Self::EvPlugIn { .. } => ControlCapabilities::EV_PLUG_IN,
            Self::EvDrive { .. } => ControlCapabilities::EV_DRIVE,
            Self::EvAwayCharge { .. } => ControlCapabilities::EV_AWAY_CHARGE,
            Self::EvSetReadyBy { .. } => ControlCapabilities::EV_SET_READY_BY,
            Self::EventDelay { .. } => ControlCapabilities::EVENT_DELAY,
            Self::MaxCapacityFraction { .. } => ControlCapabilities::MAX_CAPACITY_FRACTION,
        }
    }
}

pub fn ensure_signal_supported(
    capabilities: ControlCapabilities,
    signal: &ControlSignal,
) -> Result<(), HaresError> {
    let required = signal.required_capability();
    if capabilities.contains(required) {
        Ok(())
    } else {
        Err(HaresError::Control(format!(
            "unsupported control signal: requires {:?}, available {:?}",
            required, capabilities
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_compose_and_contains_work() {
        let caps = ControlCapabilities::POWER_SETPOINT
            | ControlCapabilities::SOC_TARGET
            | ControlCapabilities::GRID_CONNECT;
        assert!(caps.contains(ControlCapabilities::POWER_SETPOINT));
        assert!(caps.contains(ControlCapabilities::SOC_TARGET));
        assert!(caps.contains(ControlCapabilities::GRID_CONNECT));
        assert!(!caps.contains(ControlCapabilities::MODE_OVERRIDE));
    }

    #[test]
    fn control_signal_variants_round_trip_through_json() {
        let signals = vec![
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(20.0),
                cooling_setpoint_c: Some(24.0),
                deadband_c: Some(1.0),
            },
            ControlSignal::HumiditySetpoint {
                target_rh: 0.45,
                min_rh: Some(0.30),
                max_rh: Some(0.60),
            },
            ControlSignal::PowerSetpoint {
                active_power_kw: 4.2,
                reactive_power_kvar: Some(0.5),
                min_soc: None,
                max_soc: None,
            },
            ControlSignal::PowerLimit {
                max_power_kw: 5.0,
                ramp_rate_kw_per_s: Some(0.2),
            },
            ControlSignal::SOCTarget {
                target_soc: 0.7,
                min_soc: Some(0.2),
                max_soc: Some(0.9),
            },
            ControlSignal::ModeOverride {
                mode: OperatingMode::Standby,
            },
            ControlSignal::DutyCycle {
                on_fraction: 0.5,
                period_s: Some(900.0),
                component: None,
            },
            ControlSignal::LoadFraction { fraction: 0.8 },
            ControlSignal::GridConnect { connected: true },
            ControlSignal::SelfConsumption {
                enabled: true,
                solar_only_charging: false,
            },
            ControlSignal::DemandResponse {
                level: DRLevel::High,
                duration_s: Some(3600.0),
            },
            ControlSignal::ProtocolNative {
                protocol: ProtocolId(17),
                payload: vec![1, 2, 3, 4, 5],
            },
            ControlSignal::IdealCapacity {
                capacity_w: 3500.0,
                degraded: false,
            },
            ControlSignal::ThermalSetpointDelta {
                heating_delta_c: Some(2.0),
                cooling_delta_c: Some(-2.0),
            },
            ControlSignal::IdealCapacityModeOverride {
                mode: crate::IdealCapacityMode::On,
            },
            ControlSignal::EvPlugIn {
                state: EvConnectionState::HomePluggedIn,
            },
            ControlSignal::EvDrive { kwh: 5.0 },
            ControlSignal::EvAwayCharge { power_kw: 11.5 },
            ControlSignal::EvSetReadyBy {
                departure_hour: 7.0,
                target_soc: 0.8,
            },
            ControlSignal::EventDelay { delay_s: 300.0 },
            ControlSignal::MaxCapacityFraction { fraction: 0.5 },
        ];

        for signal in signals {
            let json = serde_json::to_string(&signal).expect("serialize signal");
            let decoded: ControlSignal = serde_json::from_str(&json).expect("deserialize signal");
            assert_eq!(decoded, signal);
        }
    }

    #[test]
    fn control_capabilities_round_trip_through_json() {
        let caps = ControlCapabilities::POWER_SETPOINT
            | ControlCapabilities::SOC_TARGET
            | ControlCapabilities::PROTOCOL_NATIVE;
        let json = serde_json::to_string(&caps).expect("serialize capabilities");
        let decoded: ControlCapabilities =
            serde_json::from_str(&json).expect("deserialize capabilities");
        assert_eq!(decoded, caps);
    }

    #[test]
    fn control_capabilities_with_ideal_capacity_round_trip_through_json() {
        let caps = ControlCapabilities::IDEAL_CAPACITY | ControlCapabilities::THERMAL_SETPOINT;
        let json = serde_json::to_string(&caps).expect("serialize capabilities");
        let decoded: ControlCapabilities =
            serde_json::from_str(&json).expect("deserialize capabilities");
        assert_eq!(decoded, caps);
        assert!(decoded.contains(ControlCapabilities::IDEAL_CAPACITY));
        assert!(decoded.contains(ControlCapabilities::THERMAL_SETPOINT));
    }

    #[test]
    fn unsupported_signal_returns_error() {
        let capabilities = ControlCapabilities::POWER_SETPOINT;
        let signal = ControlSignal::SOCTarget {
            target_soc: 0.6,
            min_soc: None,
            max_soc: None,
        };
        let result = ensure_signal_supported(capabilities, &signal);
        assert!(result.is_err());
    }

    #[test]
    fn supported_signal_succeeds() {
        let capabilities = ControlCapabilities::SOC_TARGET;
        let signal = ControlSignal::SOCTarget {
            target_soc: 0.6,
            min_soc: None,
            max_soc: None,
        };
        let result = ensure_signal_supported(capabilities, &signal);
        assert!(result.is_ok());
    }

    #[test]
    fn ideal_capacity_variant_constructs_and_matches() {
        let signal = ControlSignal::IdealCapacity {
            capacity_w: 1000.0,
            degraded: false,
        };
        match signal {
            ControlSignal::IdealCapacity { capacity_w, .. } => assert_eq!(capacity_w, 1000.0),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn ideal_capacity_capability_flag_is_valid() {
        let caps = ControlCapabilities::IDEAL_CAPACITY;
        assert!(caps.contains(ControlCapabilities::IDEAL_CAPACITY));
        assert!(!caps.contains(ControlCapabilities::POWER_SETPOINT));
    }

    #[test]
    fn ideal_capacity_signal_requires_ideal_capacity_capability() {
        let signal = ControlSignal::IdealCapacity {
            capacity_w: 500.0,
            degraded: false,
        };
        assert_eq!(
            signal.required_capability(),
            ControlCapabilities::IDEAL_CAPACITY
        );
    }

    #[test]
    fn ideal_capacity_signal_rejected_without_capability() {
        let capabilities = ControlCapabilities::POWER_SETPOINT;
        let signal = ControlSignal::IdealCapacity {
            capacity_w: 500.0,
            degraded: false,
        };
        let result = ensure_signal_supported(capabilities, &signal);
        assert!(result.is_err());
    }

    #[test]
    fn ideal_capacity_signal_accepted_with_capability() {
        let capabilities = ControlCapabilities::IDEAL_CAPACITY;
        let signal = ControlSignal::IdealCapacity {
            capacity_w: 500.0,
            degraded: false,
        };
        let result = ensure_signal_supported(capabilities, &signal);
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------
    // validate_numeric_bounds tests
    // -----------------------------------------------------------------

    fn assert_ok(signal: &ControlSignal) {
        assert!(
            signal.validate_numeric_bounds().is_ok(),
            "expected Ok for {signal:?}"
        );
    }

    fn assert_err(signal: &ControlSignal) {
        assert!(
            signal.validate_numeric_bounds().is_err(),
            "expected Err for {signal:?}"
        );
    }

    #[test]
    fn thermal_setpoint_valid() {
        assert_ok(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(20.0),
            cooling_setpoint_c: Some(24.0),
            deadband_c: Some(1.0),
        });
    }

    #[test]
    fn thermal_setpoint_all_none_is_valid() {
        assert_ok(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: None,
            cooling_setpoint_c: None,
            deadband_c: None,
        });
    }

    #[test]
    fn thermal_setpoint_heating_out_of_range() {
        assert_err(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(-51.0),
            cooling_setpoint_c: None,
            deadband_c: None,
        });
        assert_err(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(101.0),
            cooling_setpoint_c: None,
            deadband_c: None,
        });
    }

    #[test]
    fn thermal_setpoint_cooling_out_of_range() {
        assert_err(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: None,
            cooling_setpoint_c: Some(-1.0),
            deadband_c: None,
        });
        assert_err(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: None,
            cooling_setpoint_c: Some(61.0),
            deadband_c: None,
        });
    }

    #[test]
    fn thermal_setpoint_deadband_out_of_range() {
        assert_err(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: None,
            cooling_setpoint_c: None,
            deadband_c: Some(-0.1),
        });
        assert_err(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: None,
            cooling_setpoint_c: None,
            deadband_c: Some(6.0),
        });
    }

    #[test]
    fn thermal_setpoint_deadband_collision() {
        assert_err(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(22.0),
            cooling_setpoint_c: Some(23.0),
            deadband_c: Some(2.0),
        });
    }

    #[test]
    fn thermal_setpoint_nan_rejected() {
        assert_err(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(f64::NAN),
            cooling_setpoint_c: None,
            deadband_c: None,
        });
        assert_err(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: None,
            cooling_setpoint_c: Some(f64::NAN),
            deadband_c: None,
        });
        assert_err(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: None,
            cooling_setpoint_c: None,
            deadband_c: Some(f64::NAN),
        });
    }

    #[test]
    fn humidity_setpoint_valid() {
        assert_ok(&ControlSignal::HumiditySetpoint {
            target_rh: 0.5,
            min_rh: None,
            max_rh: None,
        });
    }

    #[test]
    fn humidity_setpoint_target_out_of_range() {
        assert_err(&ControlSignal::HumiditySetpoint {
            target_rh: -0.1,
            min_rh: None,
            max_rh: None,
        });
        assert_err(&ControlSignal::HumiditySetpoint {
            target_rh: 1.5,
            min_rh: None,
            max_rh: None,
        });
        assert_err(&ControlSignal::HumiditySetpoint {
            target_rh: f64::NAN,
            min_rh: None,
            max_rh: None,
        });
    }

    #[test]
    fn humidity_setpoint_min_max_ordering() {
        assert_err(&ControlSignal::HumiditySetpoint {
            target_rh: 0.5,
            min_rh: Some(0.6),
            max_rh: None,
        });
        assert_err(&ControlSignal::HumiditySetpoint {
            target_rh: 0.5,
            min_rh: None,
            max_rh: Some(0.4),
        });
    }

    #[test]
    fn power_setpoint_valid() {
        assert_ok(&ControlSignal::PowerSetpoint {
            active_power_kw: 5.0,
            reactive_power_kvar: Some(1.0),
            min_soc: None,
            max_soc: None,
        });
        assert_ok(&ControlSignal::PowerSetpoint {
            active_power_kw: -3.0,
            reactive_power_kvar: None,
            min_soc: None,
            max_soc: None,
        });
    }

    #[test]
    fn power_setpoint_nan_inf_rejected() {
        assert_err(&ControlSignal::PowerSetpoint {
            active_power_kw: f64::NAN,
            reactive_power_kvar: None,
            min_soc: None,
            max_soc: None,
        });
        assert_err(&ControlSignal::PowerSetpoint {
            active_power_kw: f64::INFINITY,
            reactive_power_kvar: None,
            min_soc: None,
            max_soc: None,
        });
        assert_err(&ControlSignal::PowerSetpoint {
            active_power_kw: 0.0,
            reactive_power_kvar: Some(f64::NAN),
            min_soc: None,
            max_soc: None,
        });
    }

    #[test]
    fn power_setpoint_soc_window_valid() {
        assert_ok(&ControlSignal::PowerSetpoint {
            active_power_kw: 5.0,
            reactive_power_kvar: None,
            min_soc: Some(0.2),
            max_soc: Some(0.9),
        });
        // Bounds are inclusive; a single-sided window is valid.
        assert_ok(&ControlSignal::PowerSetpoint {
            active_power_kw: 5.0,
            reactive_power_kvar: None,
            min_soc: Some(0.0),
            max_soc: Some(1.0),
        });
        assert_ok(&ControlSignal::PowerSetpoint {
            active_power_kw: 5.0,
            reactive_power_kvar: None,
            min_soc: Some(0.5),
            max_soc: None,
        });
    }

    #[test]
    fn power_setpoint_soc_window_out_of_range() {
        for (min_soc, max_soc) in [
            (Some(-0.1), None),
            (Some(1.5), None),
            (Some(f64::NAN), None),
            (None, Some(1.1)),
            (None, Some(f64::NEG_INFINITY)),
        ] {
            assert_err(&ControlSignal::PowerSetpoint {
                active_power_kw: 5.0,
                reactive_power_kvar: None,
                min_soc,
                max_soc,
            });
        }
    }

    #[test]
    fn power_setpoint_soc_window_inverted_or_empty_rejected() {
        // An inverted or empty window would silently substitute downstream:
        // every equipment arm treats the pair as a discharge floor / charge
        // ceiling and none re-checks the ordering.
        assert_err(&ControlSignal::PowerSetpoint {
            active_power_kw: 5.0,
            reactive_power_kvar: None,
            min_soc: Some(0.8),
            max_soc: Some(0.2),
        });
        assert_err(&ControlSignal::PowerSetpoint {
            active_power_kw: 5.0,
            reactive_power_kvar: None,
            min_soc: Some(0.5),
            max_soc: Some(0.5),
        });
    }

    #[test]
    fn power_limit_valid() {
        assert_ok(&ControlSignal::PowerLimit {
            max_power_kw: 5.0,
            ramp_rate_kw_per_s: Some(0.5),
        });
        assert_ok(&ControlSignal::PowerLimit {
            max_power_kw: 0.0,
            ramp_rate_kw_per_s: None,
        });
    }

    #[test]
    fn power_limit_negative_rejected() {
        assert_err(&ControlSignal::PowerLimit {
            max_power_kw: -1.0,
            ramp_rate_kw_per_s: None,
        });
        assert_err(&ControlSignal::PowerLimit {
            max_power_kw: 5.0,
            ramp_rate_kw_per_s: Some(-0.1),
        });
    }

    #[test]
    fn power_limit_nan_inf_rejected() {
        assert_err(&ControlSignal::PowerLimit {
            max_power_kw: f64::NAN,
            ramp_rate_kw_per_s: None,
        });
        assert_err(&ControlSignal::PowerLimit {
            max_power_kw: f64::INFINITY,
            ramp_rate_kw_per_s: Some(0.5),
        });
    }

    #[test]
    fn soc_target_valid() {
        assert_ok(&ControlSignal::SOCTarget {
            target_soc: 0.5,
            min_soc: None,
            max_soc: None,
        });
        assert_ok(&ControlSignal::SOCTarget {
            target_soc: 0.5,
            min_soc: Some(0.2),
            max_soc: Some(0.8),
        });
    }

    #[test]
    fn soc_target_out_of_range() {
        assert_err(&ControlSignal::SOCTarget {
            target_soc: -0.1,
            min_soc: None,
            max_soc: None,
        });
        assert_err(&ControlSignal::SOCTarget {
            target_soc: 1.5,
            min_soc: None,
            max_soc: None,
        });
        assert_err(&ControlSignal::SOCTarget {
            target_soc: f64::NAN,
            min_soc: None,
            max_soc: None,
        });
    }

    #[test]
    fn soc_target_ordering() {
        assert_err(&ControlSignal::SOCTarget {
            target_soc: 0.5,
            min_soc: Some(0.8),
            max_soc: Some(0.2),
        });
        assert_err(&ControlSignal::SOCTarget {
            target_soc: 0.1,
            min_soc: Some(0.2),
            max_soc: Some(0.8),
        });
        assert_err(&ControlSignal::SOCTarget {
            target_soc: 0.9,
            min_soc: Some(0.2),
            max_soc: Some(0.8),
        });
    }

    #[test]
    fn load_fraction_valid() {
        assert_ok(&ControlSignal::LoadFraction { fraction: 0.5 });
        assert_ok(&ControlSignal::LoadFraction { fraction: 0.01 });
        assert_ok(&ControlSignal::LoadFraction { fraction: 0.99 });
    }

    #[test]
    fn load_fraction_invalid() {
        assert_err(&ControlSignal::LoadFraction { fraction: -0.1 });
        assert_err(&ControlSignal::LoadFraction { fraction: 1.1 });
        assert_err(&ControlSignal::LoadFraction { fraction: 5.0 });
        assert_err(&ControlSignal::LoadFraction { fraction: f64::NAN });
    }

    #[test]
    fn duty_cycle_valid() {
        assert_ok(&ControlSignal::DutyCycle {
            on_fraction: 0.5,
            period_s: Some(900.0),
            component: None,
        });
        assert_ok(&ControlSignal::DutyCycle {
            on_fraction: 0.01,
            period_s: None,
            component: None,
        });
    }

    #[test]
    fn duty_cycle_invalid() {
        assert_err(&ControlSignal::DutyCycle {
            on_fraction: -0.1,
            period_s: None,
            component: None,
        });
        assert_err(&ControlSignal::DutyCycle {
            on_fraction: 1.1,
            period_s: None,
            component: None,
        });
        assert_err(&ControlSignal::DutyCycle {
            on_fraction: 2.0,
            period_s: None,
            component: None,
        });
        assert_err(&ControlSignal::DutyCycle {
            on_fraction: f64::NAN,
            period_s: None,
            component: None,
        });
        assert_err(&ControlSignal::DutyCycle {
            on_fraction: 0.5,
            period_s: Some(-1.0),
            component: None,
        });
        assert_err(&ControlSignal::DutyCycle {
            on_fraction: 0.5,
            period_s: Some(f64::NAN),
            component: None,
        });
    }

    #[test]
    fn max_capacity_fraction_valid() {
        assert_ok(&ControlSignal::MaxCapacityFraction { fraction: 0.5 });
        assert_ok(&ControlSignal::MaxCapacityFraction { fraction: 0.01 });
    }

    #[test]
    fn max_capacity_fraction_invalid() {
        assert_err(&ControlSignal::MaxCapacityFraction { fraction: -0.1 });
        assert_err(&ControlSignal::MaxCapacityFraction { fraction: 1.1 });
        assert_err(&ControlSignal::MaxCapacityFraction { fraction: 5.0 });
        assert_err(&ControlSignal::MaxCapacityFraction { fraction: f64::NAN });
    }

    #[test]
    fn curtailment_percent_valid() {
        assert_ok(&ControlSignal::CurtailmentPercent { percent: 50.0 });
        assert_ok(&ControlSignal::CurtailmentPercent { percent: 0.0 });
        assert_ok(&ControlSignal::CurtailmentPercent { percent: 100.0 });
    }

    #[test]
    fn curtailment_percent_invalid() {
        assert_err(&ControlSignal::CurtailmentPercent { percent: -5.0 });
        assert_err(&ControlSignal::CurtailmentPercent { percent: 150.0 });
        assert_err(&ControlSignal::CurtailmentPercent { percent: f64::NAN });
    }

    #[test]
    fn power_factor_setpoint_valid() {
        assert_ok(&ControlSignal::PowerFactorSetpoint { power_factor: 0.95 });
        assert_ok(&ControlSignal::PowerFactorSetpoint { power_factor: 0.0 });
        assert_ok(&ControlSignal::PowerFactorSetpoint { power_factor: 1.0 });
    }

    #[test]
    fn power_factor_setpoint_invalid() {
        assert_err(&ControlSignal::PowerFactorSetpoint { power_factor: -0.1 });
        assert_err(&ControlSignal::PowerFactorSetpoint { power_factor: 1.1 });
        assert_err(&ControlSignal::PowerFactorSetpoint {
            power_factor: f64::NAN,
        });
    }

    #[test]
    fn ev_drive_valid() {
        assert_ok(&ControlSignal::EvDrive { kwh: 5.0 });
        assert_ok(&ControlSignal::EvDrive { kwh: 0.0 });
    }

    #[test]
    fn ev_drive_invalid() {
        assert_err(&ControlSignal::EvDrive { kwh: -1.0 });
        assert_err(&ControlSignal::EvDrive { kwh: f64::NAN });
        assert_err(&ControlSignal::EvDrive { kwh: f64::INFINITY });
    }

    #[test]
    fn ev_away_charge_valid() {
        assert_ok(&ControlSignal::EvAwayCharge { power_kw: 7.2 });
        assert_ok(&ControlSignal::EvAwayCharge { power_kw: 0.0 });
    }

    #[test]
    fn ev_away_charge_invalid() {
        assert_err(&ControlSignal::EvAwayCharge { power_kw: -1.0 });
        assert_err(&ControlSignal::EvAwayCharge { power_kw: f64::NAN });
    }

    #[test]
    fn ev_set_ready_by_valid() {
        assert_ok(&ControlSignal::EvSetReadyBy {
            departure_hour: 7.0,
            target_soc: 0.8,
        });
        assert_ok(&ControlSignal::EvSetReadyBy {
            departure_hour: 0.0,
            target_soc: 0.0,
        });
        assert_ok(&ControlSignal::EvSetReadyBy {
            departure_hour: 24.0,
            target_soc: 1.0,
        });
    }

    #[test]
    fn ev_set_ready_by_invalid() {
        assert_err(&ControlSignal::EvSetReadyBy {
            departure_hour: -1.0,
            target_soc: 0.8,
        });
        assert_err(&ControlSignal::EvSetReadyBy {
            departure_hour: 25.0,
            target_soc: 0.8,
        });
        assert_err(&ControlSignal::EvSetReadyBy {
            departure_hour: 7.0,
            target_soc: -0.1,
        });
        assert_err(&ControlSignal::EvSetReadyBy {
            departure_hour: 7.0,
            target_soc: 1.5,
        });
        assert_err(&ControlSignal::EvSetReadyBy {
            departure_hour: f64::NAN,
            target_soc: 0.8,
        });
        assert_err(&ControlSignal::EvSetReadyBy {
            departure_hour: 7.0,
            target_soc: f64::NAN,
        });
    }

    #[test]
    fn event_delay_valid() {
        assert_ok(&ControlSignal::EventDelay { delay_s: 300.0 });
        assert_ok(&ControlSignal::EventDelay { delay_s: 0.0 });
    }

    #[test]
    fn event_delay_invalid() {
        assert_err(&ControlSignal::EventDelay { delay_s: -1.0 });
        assert_err(&ControlSignal::EventDelay { delay_s: f64::NAN });
    }

    #[test]
    fn ideal_capacity_valid() {
        assert_ok(&ControlSignal::IdealCapacity {
            capacity_w: 3500.0,
            degraded: false,
        });
        assert_ok(&ControlSignal::IdealCapacity {
            capacity_w: 0.0,
            degraded: true,
        });
    }

    #[test]
    fn ideal_capacity_invalid() {
        assert_err(&ControlSignal::IdealCapacity {
            capacity_w: f64::NAN,
            degraded: false,
        });
        assert_err(&ControlSignal::IdealCapacity {
            capacity_w: f64::INFINITY,
            degraded: false,
        });
    }

    #[test]
    fn thermal_setpoint_delta_valid() {
        assert_ok(&ControlSignal::ThermalSetpointDelta {
            heating_delta_c: Some(2.0),
            cooling_delta_c: Some(-2.0),
        });
        assert_ok(&ControlSignal::ThermalSetpointDelta {
            heating_delta_c: Some(-20.0),
            cooling_delta_c: Some(20.0),
        });
        assert_ok(&ControlSignal::ThermalSetpointDelta {
            heating_delta_c: None,
            cooling_delta_c: None,
        });
    }

    #[test]
    fn thermal_setpoint_delta_out_of_range() {
        assert_err(&ControlSignal::ThermalSetpointDelta {
            heating_delta_c: Some(25.0),
            cooling_delta_c: None,
        });
        assert_err(&ControlSignal::ThermalSetpointDelta {
            heating_delta_c: Some(-25.0),
            cooling_delta_c: None,
        });
        assert_err(&ControlSignal::ThermalSetpointDelta {
            heating_delta_c: None,
            cooling_delta_c: Some(30.0),
        });
        assert_err(&ControlSignal::ThermalSetpointDelta {
            heating_delta_c: Some(f64::NAN),
            cooling_delta_c: None,
        });
    }

    #[test]
    fn reactive_setpoint_nan_rejected() {
        assert_err(&ControlSignal::ReactiveSetpoint { kvar: f64::NAN });
        assert_err(&ControlSignal::ReactiveSetpoint {
            kvar: f64::INFINITY,
        });
    }

    #[test]
    fn reactive_setpoint_valid() {
        assert_ok(&ControlSignal::ReactiveSetpoint { kvar: 5.0 });
        assert_ok(&ControlSignal::ReactiveSetpoint { kvar: -3.0 });
    }

    #[test]
    fn demand_response_duration_invalid() {
        assert_err(&ControlSignal::DemandResponse {
            level: DRLevel::Moderate,
            duration_s: Some(-1.0),
        });
        assert_err(&ControlSignal::DemandResponse {
            level: DRLevel::Moderate,
            duration_s: Some(f64::NAN),
        });
    }

    #[test]
    fn demand_response_valid() {
        assert_ok(&ControlSignal::DemandResponse {
            level: DRLevel::High,
            duration_s: Some(3600.0),
        });
        assert_ok(&ControlSignal::DemandResponse {
            level: DRLevel::Normal,
            duration_s: None,
        });
    }

    #[test]
    fn non_numeric_signals_always_valid() {
        assert_ok(&ControlSignal::ModeOverride {
            mode: OperatingMode::Off,
        });
        assert_ok(&ControlSignal::GridConnect { connected: true });
        assert_ok(&ControlSignal::SelfConsumption {
            enabled: true,
            solar_only_charging: false,
        });
        assert_ok(&ControlSignal::InverterPriorityMode {
            priority: InverterPriority::Watt,
        });
        assert_ok(&ControlSignal::IdealCapacityModeOverride {
            mode: IdealCapacityMode::On,
        });
        assert_ok(&ControlSignal::EvPlugIn {
            state: EvConnectionState::HomePluggedIn,
        });
        assert_ok(&ControlSignal::ProtocolNative {
            protocol: ProtocolId(1),
            payload: vec![1, 2, 3],
        });
    }
}
