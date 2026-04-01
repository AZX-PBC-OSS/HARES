//! Configuration parsing and initialization helpers for heat-pump heaters.

use hares_types::{Telemetry, TelemetryField, telemetry_keys as tk};

use super::constants::HEATER_TELEMETRY_CAPACITY;

const OPERATING_MODE_CODE_OFF: f64 = 0.0;
const OPERATING_MODE_CODE_HP_ON: f64 = 3.0;
const OPERATING_MODE_CODE_HP_ER_ON: f64 = 4.0;
const OPERATING_MODE_CODE_ER_ON: f64 = 5.0;

pub(super) fn default_heater_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(HEATER_TELEMETRY_CAPACITY);
    telemetry.insert(tk::ELECTRIC_KW, 0.0);
    telemetry.insert(tk::THERMAL_OUTPUT_W, 0.0);
    telemetry.insert(tk::OPERATING_MODE, OPERATING_MODE_CODE_OFF);
    telemetry.insert(tk::SPEED_INDEX, 0.0);
    telemetry.insert(tk::DEFROST_ACTIVE, 0.0);
    telemetry.insert(tk::COP, 0.0);
    telemetry.insert(tk::RUNTIME_FRACTION, 0.0);
    telemetry.insert(tk::COMPRESSOR_KW, 0.0);
    telemetry.insert(tk::DEFROST_TIME_FRACTION, 0.0);
    telemetry.insert(tk::HEATING_SETPOINT_C, 0.0);
    telemetry.insert(tk::COOLING_SETPOINT_C, 0.0);
    telemetry.insert(tk::FAN_KW, 0.0);
    telemetry.insert(tk::BACKUP_ER_KW, 0.0);
    telemetry.insert(tk::PAN_HEATER_KW, 0.0);
    telemetry.insert(tk::HP_CAPACITY_W, 0.0);
    telemetry.insert(tk::ER_CAPACITY_W, 0.0);
    telemetry.insert(tk::HP_LOCKOUT_TEMP_C, 0.0);
    telemetry.insert(tk::ER_LOCKOUT_TEMP_C, 0.0);
    telemetry.insert(tk::ER_SETPOINT_OFFSET_C, 0.0);
    telemetry.insert(tk::ER_HARD_LOCKOUT_TIME_S, 0.0);
    telemetry.insert(tk::BACKUP_CAPACITY_W, 0.0);
    telemetry.insert(tk::BACKUP_EIR, 0.0);
    telemetry
}

pub(super) fn heater_telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: tk::ELECTRIC_KW.to_string(),
            unit: "kW".to_string(),
            description: "Total heater electric power".to_string(),
        },
        TelemetryField {
            name: tk::THERMAL_OUTPUT_W.to_string(),
            unit: "W".to_string(),
            description: "Delivered sensible heating".to_string(),
        },
        TelemetryField {
            name: tk::OPERATING_MODE.to_string(),
            unit: "enum".to_string(),
            description: "Operating mode code".to_string(),
        },
        TelemetryField {
            name: tk::SPEED_INDEX.to_string(),
            unit: "index".to_string(),
            description: "Selected compressor speed stage".to_string(),
        },
        TelemetryField {
            name: tk::DEFROST_ACTIVE.to_string(),
            unit: "bool".to_string(),
            description: "1 when defrost correction is active".to_string(),
        },
        TelemetryField {
            name: tk::COP.to_string(),
            unit: "-".to_string(),
            description:
                "COP per AHRI convention: gross thermal output / compressor-only electric input"
                    .to_string(),
        },
        TelemetryField {
            name: tk::RUNTIME_FRACTION.to_string(),
            unit: "-".to_string(),
            description: "Compressor runtime fraction (part-load ratio) this timestep".to_string(),
        },
        TelemetryField {
            name: tk::COMPRESSOR_KW.to_string(),
            unit: "kW".to_string(),
            description: "Compressor-only electric power".to_string(),
        },
        TelemetryField {
            name: tk::DEFROST_TIME_FRACTION.to_string(),
            unit: "-".to_string(),
            description: "Fraction of timestep in defrost mode [0..1]".to_string(),
        },
        TelemetryField {
            name: tk::HEATING_SETPOINT_C.to_string(),
            unit: "C".to_string(),
            description: "Active heating setpoint used by the heater control logic".to_string(),
        },
        TelemetryField {
            name: tk::COOLING_SETPOINT_C.to_string(),
            unit: "C".to_string(),
            description: "Active cooling setpoint from shared thermostat state".to_string(),
        },
        TelemetryField {
            name: tk::FAN_KW.to_string(),
            unit: "kW".to_string(),
            description: "Fan/blower electric power".to_string(),
        },
        TelemetryField {
            name: tk::BACKUP_ER_KW.to_string(),
            unit: "kW".to_string(),
            description: "Electric resistance backup heater power".to_string(),
        },
        TelemetryField {
            name: tk::PAN_HEATER_KW.to_string(),
            unit: "kW".to_string(),
            description: "Minisplit pan/crankcase heater power".to_string(),
        },
        TelemetryField {
            name: tk::HP_CAPACITY_W.to_string(),
            unit: "W".to_string(),
            description: "Heat pump thermal output before duct losses".to_string(),
        },
        TelemetryField {
            name: tk::ER_CAPACITY_W.to_string(),
            unit: "W".to_string(),
            description: "Electric resistance backup thermal output".to_string(),
        },
        TelemetryField {
            name: tk::HP_LOCKOUT_TEMP_C.to_string(),
            unit: "C".to_string(),
            description: "Outdoor temperature below which the heat pump is locked out".to_string(),
        },
        TelemetryField {
            name: tk::ER_LOCKOUT_TEMP_C.to_string(),
            unit: "C".to_string(),
            description: "Outdoor temperature above which ER backup is locked out".to_string(),
        },
        TelemetryField {
            name: tk::ER_SETPOINT_OFFSET_C.to_string(),
            unit: "C".to_string(),
            description: "Setpoint offset above which ER backup may activate".to_string(),
        },
        TelemetryField {
            name: tk::ER_HARD_LOCKOUT_TIME_S.to_string(),
            unit: "s".to_string(),
            description: "Hard lockout duration after setpoint increase [s]; 0 = disabled".to_string(),
        },
        TelemetryField {
            name: tk::BACKUP_CAPACITY_W.to_string(),
            unit: "W".to_string(),
            description: "Rated backup ER heating capacity".to_string(),
        },
        TelemetryField {
            name: tk::BACKUP_EIR.to_string(),
            unit: "-".to_string(),
            description: "Backup heater energy input ratio".to_string(),
        },
    ]
}

pub(super) fn operating_mode_code(mode: hares_types::OperatingMode) -> f64 {
    match mode {
        hares_types::OperatingMode::Off => OPERATING_MODE_CODE_OFF,
        hares_types::OperatingMode::HeatingHP => OPERATING_MODE_CODE_HP_ON,
        hares_types::OperatingMode::HeatingHPAndER => OPERATING_MODE_CODE_HP_ER_ON,
        hares_types::OperatingMode::HeatingER => OPERATING_MODE_CODE_ER_ON,
        _ => OPERATING_MODE_CODE_OFF,
    }
}
