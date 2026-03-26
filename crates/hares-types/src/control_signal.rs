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
    }
}

impl ControlSignal {
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
            ControlSignal::IdealCapacity { capacity_w: 3500.0 },
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
        let signal = ControlSignal::IdealCapacity { capacity_w: 1000.0 };
        match signal {
            ControlSignal::IdealCapacity { capacity_w } => assert_eq!(capacity_w, 1000.0),
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
        let signal = ControlSignal::IdealCapacity { capacity_w: 500.0 };
        assert_eq!(
            signal.required_capability(),
            ControlCapabilities::IDEAL_CAPACITY
        );
    }

    #[test]
    fn ideal_capacity_signal_rejected_without_capability() {
        let capabilities = ControlCapabilities::POWER_SETPOINT;
        let signal = ControlSignal::IdealCapacity { capacity_w: 500.0 };
        let result = ensure_signal_supported(capabilities, &signal);
        assert!(result.is_err());
    }

    #[test]
    fn ideal_capacity_signal_accepted_with_capability() {
        let capabilities = ControlCapabilities::IDEAL_CAPACITY;
        let signal = ControlSignal::IdealCapacity { capacity_w: 500.0 };
        let result = ensure_signal_supported(capabilities, &signal);
        assert!(result.is_ok());
    }
}
