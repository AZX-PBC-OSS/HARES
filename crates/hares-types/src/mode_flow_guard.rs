//! Mode-versus-flows consistency validation for [`validate_core_contract`].
//!
//! Ensures equipment operating mode is physically consistent with reported
//! energy flows: active modes need non-zero power, Off mode forbids non-zero
//! flows, thermal sign matches heating/cooling variants, and charge/discharge
//! direction agrees with electric power sign.

use crate::equipment::{CoreOutput, EquipmentDescriptor, OperatingMode};

#[cfg(feature = "observe")]
use std::sync::atomic::{AtomicU64, Ordering};

/// Counter of mode/flow inconsistency rejections.
/// Incremented on every physical-implausibility rejection.
#[cfg(feature = "observe")]
static MODE_FLOW_INCONSISTENCY_COUNT: AtomicU64 = AtomicU64::new(0);

/// Returns the total number of mode/flow inconsistency rejections in this process.
#[cfg(feature = "observe")]
pub fn mode_flow_inconsistency_counter() -> u64 {
    MODE_FLOW_INCONSISTENCY_COUNT.load(Ordering::Relaxed)
}

#[cfg(feature = "observe")]
fn record_rejection() {
    MODE_FLOW_INCONSISTENCY_COUNT.fetch_add(1, Ordering::Relaxed);
}

#[cfg(not(feature = "observe"))]
fn record_rejection() {}

/// Reasons a mode/flow combination is physically inconsistent.
///
/// Returned by [`check_mode_flow_consistency`] as the first-found violation.
/// The caller converts the message into a [`crate::HaresError`] or logs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModeFlowViolation {
    pub message: String,
}

/// Checks that operating mode and reported flows are physically consistent.
///
/// Rules:
/// 1. Active-mode power guard — active mode (not Off/Standby) requires at
///    least one non-zero flow (electric, thermal, or fuel).
/// 2. Off-mode zero-flow guard — Off mode forbids non-zero flows.
/// 3. Thermal sign vs mode — positive thermal output requires a heating
///    variant; negative thermal output requires a cooling variant.
///    `On` and `Defrost` are exempt as generic/transitional modes.
/// 4. Charging/discharging direction — Charging must have net consumption;
///    Discharging must have net generation.
pub(crate) fn check_mode_flow_consistency(
    desc: &EquipmentDescriptor,
    co: &CoreOutput,
) -> Result<(), ModeFlowViolation> {
    let caps = desc.core_capabilities;

    if let Some(mode) = co.state.operating_mode {
        // Rule 1: active-mode power guard
        if mode.is_active() {
            let has_flow = co.flows.electric_kw.as_ref().is_some_and(|p| !p.is_zero())
                || co.flows.thermal_output_w.is_some_and(|v| v != 0.0)
                || co
                    .flows
                    .fuel_w
                    .as_ref()
                    .is_some_and(|f| f.consumption_w > 0.0);
            if !has_flow {
                record_rejection();
                return Err(ModeFlowViolation {
                    message: format!(
                        "core_output contract violation for '{}' ({:?}): \
                         operating_mode={mode:?} (active) with zero flows \
                         (electric={:?}, thermal={:?}, fuel={:?})",
                        desc.name,
                        caps,
                        co.flows.electric_kw,
                        co.flows.thermal_output_w,
                        co.flows.fuel_w,
                    ),
                });
            }
        }

        // Rule 2: Off-mode zero-flow guard
        if mode == OperatingMode::Off {
            let non_zero = co.flows.electric_kw.as_ref().is_some_and(|p| !p.is_zero())
                || co.flows.thermal_output_w.is_some_and(|v| v != 0.0)
                || co
                    .flows
                    .fuel_w
                    .as_ref()
                    .is_some_and(|f| f.consumption_w > 0.0)
                || co.flows.reactive_power_kvar.is_some_and(|v| v != 0.0);
            if non_zero {
                record_rejection();
                return Err(ModeFlowViolation {
                    message: format!(
                        "core_output contract violation for '{}' ({:?}): \
                         operating_mode=Off with non-zero flows \
                         (electric={:?}, thermal={:?}, fuel={:?}, reactive={:?})",
                        desc.name,
                        caps,
                        co.flows.electric_kw,
                        co.flows.thermal_output_w,
                        co.flows.fuel_w,
                        co.flows.reactive_power_kvar,
                    ),
                });
            }
        }
    }

    // Rule 3: thermal sign vs mode
    if let (Some(mode), Some(thermal)) = (co.state.operating_mode, co.flows.thermal_output_w) {
        if thermal > 0.0
            && !mode.is_heating_variant()
            && !matches!(mode, OperatingMode::On | OperatingMode::Defrost)
        {
            record_rejection();
            return Err(ModeFlowViolation {
                message: format!(
                    "core_output contract violation for '{}' ({:?}): \
                     thermal_output_w={thermal} > 0 (heating) but operating_mode={mode:?} \
                     is not a heating variant",
                    desc.name, caps,
                ),
            });
        }
        if thermal < 0.0
            && !mode.is_cooling_variant()
            && !matches!(mode, OperatingMode::On | OperatingMode::Defrost)
        {
            record_rejection();
            return Err(ModeFlowViolation {
                message: format!(
                    "core_output contract violation for '{}' ({:?}): \
                     thermal_output_w={thermal} < 0 (cooling) but operating_mode={mode:?} \
                     is not a cooling variant",
                    desc.name, caps,
                ),
            });
        }
    }

    // Rule 4: charging/discharging direction.
    // Only applies to unidirectional Consumption/Generation — Bidirectional
    // can flow either way at any moment (e.g., battery with standby load).
    if let (Some(mode), Some(electric)) = (co.state.operating_mode, &co.flows.electric_kw) {
        use crate::equipment::ElectricPower;
        let violation = match (mode, electric) {
            (OperatingMode::Charging, ElectricPower::Generation(kw)) if *kw > 0.0 => {
                Some("operating_mode=Charging with net generation")
            }
            (OperatingMode::Discharging, ElectricPower::Consumption(kw)) if *kw > 0.0 => {
                Some("operating_mode=Discharging with net consumption")
            }
            _ => None,
        };
        if let Some(reason) = violation {
            record_rejection();
            return Err(ModeFlowViolation {
                message: format!(
                    "core_output contract violation for '{}' ({:?}): \
                     {reason} ({electric:?})",
                    desc.name, caps,
                ),
            });
        }
    }

    Ok(())
}

/// Invariant re-verification: runs the same consistency checks independently
/// and logs a warning if a violation is found. Called AFTER the production check
/// has passed — a warning here means the production path has a logic bug.
///
/// Exists only in debug or `check_invariants` builds.
#[cfg(any(debug_assertions, feature = "check_invariants"))]
pub(crate) fn invariant_recheck_mode_flow_consistency(desc: &EquipmentDescriptor, co: &CoreOutput) {
    if let Err(violation) = check_mode_flow_consistency(desc, co) {
        tracing::warn!(
            equipment = %desc.name,
            operating_mode = ?co.state.operating_mode,
            electric_kw = ?co.flows.electric_kw,
            thermal_output_w = ?co.flows.thermal_output_w,
            fuel_w = ?co.flows.fuel_w,
            reactive_power_kvar = ?co.flows.reactive_power_kvar,
            "invariant violation: {} (production check should have caught this)",
            violation.message,
        );
    }
}

#[cfg(not(any(debug_assertions, feature = "check_invariants")))]
pub(crate) fn invariant_recheck_mode_flow_consistency(
    _desc: &EquipmentDescriptor,
    _co: &CoreOutput,
) {
}
