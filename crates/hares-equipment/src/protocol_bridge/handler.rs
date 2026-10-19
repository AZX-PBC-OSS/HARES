//! Protocol-specific handler trait and built-in implementations.
//!
//! Each `ProtocolHandler` converts a raw binary payload from a specific
//! protocol into typed `EquipmentCommand`s that the dwelling dispatch loop
//! routes to target equipment within the same timestep.
//!
//! Handlers are registered with a `ProtocolBridge` at configuration time.
//! When a `ProtocolNative` signal arrives, the bridge looks up the handler
//! matching `signal.protocol` and delegates parsing.

use hares_types::removed_keys::{KeyScope, removed_key};
use hares_types::{ControlSignal, ProtocolId};
use serde::Deserialize;

/// A command parsed from a protocol payload, ready for dispatch.
///
/// Each `EquipmentCommand` names a target equipment instance and carries a
/// typed `ControlSignal` to apply to that equipment. The dwelling loop
/// collects these via `drain_command_signals()` and dispatches them
/// through the standard `ControlDispatcher` — preserving priority ordering
/// and capability gating.
#[derive(Clone, Debug, PartialEq)]
pub struct EquipmentCommand {
    pub target_name: String,
    pub signal: ControlSignal,
}

impl EquipmentCommand {
    #[must_use]
    pub fn new(target_name: impl Into<String>, signal: ControlSignal) -> Self {
        Self {
            target_name: target_name.into(),
            signal,
        }
    }
}

/// Protocol-specific handler that parses raw binary payloads into
/// typed equipment commands.
///
/// Implementations must be `Send + Sync` so they can be shared across
/// threads in fleet-mode simulations. The `Debug` bound provides usable
/// diagnostics when handler lookup fails.
pub trait ProtocolHandler: Send + Sync + std::fmt::Debug {
    /// The protocol ID this handler accepts. The bridge uses this to
    /// match incoming `ProtocolNative { protocol, .. }` signals to
    /// the correct handler.
    fn protocol_id(&self) -> ProtocolId;

    /// Parse a raw protocol payload into zero or more equipment commands.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the payload is malformed or cannot be interpreted
    /// under this protocol. Failed parses are recorded in telemetry but
    /// do not abort the simulation — the bridge rejects the signal and
    /// continues.
    fn parse(&self, payload: &[u8]) -> crate::Result<Vec<EquipmentCommand>>;
}

// ---------------------------------------------------------------------------
// JSON payload handler — "made-up" protocol for testing and demonstration
// ---------------------------------------------------------------------------

/// JSON-serialised `[{"target": "Battery-1", "signal": {"PowerSetpoint": {"active_power_w": 5000.0}}}]`
/// payload format.
#[derive(Deserialize)]
struct JsonCommand {
    target: String,
    signal: serde_json::Value,
}

/// A protocol handler that parses JSON payloads into equipment commands.
///
/// Payload format (UTF-8 JSON):
/// ```json
/// [
///   {
///     "target": "Battery-1",
///     "signal": {
///       "PowerSetpoint": {"active_power_w": 5000.0, "reactive_power_var": null}
///     }
///   },
///   {
///     "target": "HVAC-1",
///     "signal": {
///       "ModeOverride": {"mode": "Off"}
///     }
///   }
/// ]
/// ```
///
/// The `"signal"` field must deserialise to a `ControlSignal` variant via
/// serde. Any deserialisation error for an individual command is reported
/// as an `Err` — the bridge rejects the entire payload rather than
/// partially applying commands.
///
/// This handler provides a machine-readable JSON payload format for
/// integration testing and external controller communication. Additional
/// protocol-specific handlers are added by implementing `ProtocolHandler`
/// and registering them in `ProtocolBridgeConfig::handlers`.
#[derive(Debug, Default)]
pub struct JsonHandler {
    protocol_id: ProtocolId,
}

impl JsonHandler {
    #[must_use]
    pub fn new(protocol_id: ProtocolId) -> Self {
        Self { protocol_id }
    }
}

impl ProtocolHandler for JsonHandler {
    fn protocol_id(&self) -> ProtocolId {
        self.protocol_id
    }

    fn parse(&self, payload: &[u8]) -> crate::Result<Vec<EquipmentCommand>> {
        let raw: Vec<JsonCommand> = serde_json::from_slice(payload).map_err(|e| {
            hares_types::HaresError::Control(format!(
                "JSON protocol handler (id={}) failed to parse payload: {e}",
                self.protocol_id.0
            ))
        })?;

        let mut commands = Vec::with_capacity(raw.len());
        for cmd in raw {
            // Each command's signal keys are checked against the
            // removed-keys table before serde deserialises the signal, so
            // an old payload names its replacement instead of failing as a
            // missing field.
            if let Some(replacement) = removed_signal_key(&cmd.signal) {
                return Err(hares_types::HaresError::Control(format!(
                    "JSON protocol handler (id={}) rejected signal for target '{}': {}",
                    self.protocol_id.0, cmd.target, replacement
                )));
            }
            let signal: ControlSignal = serde_json::from_value(cmd.signal).map_err(|e| {
                hares_types::HaresError::Control(format!(
                    "JSON protocol handler (id={}) failed to deserialise signal for target '{}': {e}",
                    self.protocol_id.0, cmd.target
                ))
            })?;
            commands.push(EquipmentCommand {
                target_name: cmd.target,
                signal,
            });
        }
        Ok(commands)
    }
}

/// The removed-keys row a signal JSON value's key names hit, if any: the
/// variant's field keys are checked against the `ControlSignal` scope of
/// the removed-keys table (the same rows every Python signal surface
/// consults), so a pre-rename key names its replacement. The signal JSON
/// is externally tagged - `{"PowerSetpoint": {"active_power_w": 1.0}}` -
/// so the check descends into the variant's field map.
fn removed_signal_key(signal: &serde_json::Value) -> Option<String> {
    let outer = signal.as_object()?;
    for (_variant, fields) in outer {
        let Some(fields) = fields.as_object() else {
            continue;
        };
        for key in fields.keys() {
            if let Some(replacement) = removed_key(KeyScope::ControlSignal, key) {
                return Some(format!("signal key '{key}' was removed: {replacement}"));
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use hares_types::OperatingMode;

    fn handler() -> JsonHandler {
        JsonHandler::new(ProtocolId(1))
    }

    #[test]
    fn parses_power_setpoint_from_json() {
        let payload = br#"[
            {"target": "Battery-1", "signal": {"PowerSetpoint": {"active_power_w": 5000.0, "reactive_power_var": null}}}
        ]"#;

        let commands = handler().parse(payload).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].target_name, "Battery-1");
        assert_eq!(
            commands[0].signal,
            ControlSignal::PowerSetpoint {
                active_power_w: 5000.0,
                reactive_power_var: None,
                min_soc: None,
                max_soc: None,
            }
        );
    }

    #[test]
    fn parses_mode_override_from_json() {
        let payload = br#"[
            {"target": "HVAC-1", "signal": {"ModeOverride": {"mode": "Off"}}}
        ]"#;

        let commands = handler().parse(payload).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].target_name, "HVAC-1");
        assert_eq!(
            commands[0].signal,
            ControlSignal::ModeOverride {
                mode: OperatingMode::Off,
            }
        );
    }

    #[test]
    fn parses_multiple_commands_from_json() {
        let payload = br#"[
            {"target": "Battery-1", "signal": {"PowerSetpoint": {"active_power_w": 3000.0, "reactive_power_var": null}}},
            {"target": "HVAC-1", "signal": {"ThermalSetpoint": {"heating_setpoint_c": 20.0, "cooling_setpoint_c": 24.0, "deadband_c": null}}}
        ]"#;

        let commands = handler().parse(payload).unwrap();
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].target_name, "Battery-1");
        assert_eq!(commands[1].target_name, "HVAC-1");

        assert_eq!(
            commands[0].signal,
            ControlSignal::PowerSetpoint {
                active_power_w: 3000.0,
                reactive_power_var: None,
                min_soc: None,
                max_soc: None,
            }
        );
        assert_eq!(
            commands[1].signal,
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(20.0),
                cooling_setpoint_c: Some(24.0),
                deadband_c: None,
            }
        );
    }

    #[test]
    fn empty_payload_returns_empty_vec() {
        let payload = b"[]";
        let commands = handler().parse(payload).unwrap();
        assert!(commands.is_empty());
    }

    #[test]
    fn invalid_json_returns_err() {
        let payload = b"not json";
        let result = handler().parse(payload);
        assert!(result.is_err());
    }

    #[test]
    fn unknown_signal_variant_returns_err() {
        let payload = br#"[
            {"target": "Bad", "signal": {"Nonexistent": {}}}
        ]"#;
        let result = handler().parse(payload);
        assert!(result.is_err());
    }

    #[test]
    fn handler_reports_protocol_id() {
        let h = JsonHandler::new(ProtocolId(7));
        assert_eq!(h.protocol_id().0, 7);
    }

    /// `protocol_bridge_old_signal_key_names_its_replacement`: the command
    /// `{"target": "Battery", "signal": {"PowerSetpoint":
    /// {"active_power_kw": 1.0}}}` fails naming `PowerSetpoint.active_power_w
    /// (W)` - the signal keys are checked against the removed-keys table
    /// before serde deserialises the signal, so an old payload names its
    /// replacement instead of failing as a missing field - and the same
    /// command with `active_power_w` applies 1.0 W.
    #[test]
    fn protocol_bridge_old_signal_key_names_its_replacement() {
        let payload = br#"[
            {"target": "Battery", "signal": {"PowerSetpoint": {"active_power_kw": 1.0}}}
        ]"#;
        let err = handler()
            .parse(payload)
            .expect_err("the pre-rename signal key is rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("PowerSetpoint.active_power_w (W)"),
            "the error must name the replacement PowerSetpoint.active_power_w (W): {msg}"
        );
        assert!(
            !msg.contains("missing field"),
            "the old key names its replacement, not a missing field: {msg}"
        );

        let payload = br#"[
            {"target": "Battery", "signal": {"PowerSetpoint": {"active_power_w": 1.0}}}
        ]"#;
        let commands = handler().parse(payload).expect("the SI key applies");
        assert_eq!(commands[0].target_name, "Battery");
        let debug = format!("{:?}", commands[0].signal);
        assert!(
            debug.contains("active_power_w: 1.0"),
            "the applied setpoint holds 1.0 W under active_power_w: {debug}"
        );
    }
}
