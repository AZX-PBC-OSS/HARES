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
    telemetry.insert(tk::DEFROST_EXTRA_POWER_W, 0.0);
    telemetry.insert(tk::DEFROST_Q_W, 0.0);
    telemetry.insert(tk::DEFROST_CAPACITY_MULTIPLIER, 1.0);
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
    telemetry.insert(tk::ER_STAGES_ON, 0.0);
    telemetry.insert(tk::MIN_COMPRESSOR_FRACTION, 0.25);
    telemetry.insert(tk::FUEL_INPUT_W, 0.0);
    telemetry.insert(tk::MAX_CAPACITY_FRACTION, 1.0);
    telemetry.insert(tk::CAP_RATIO, 0.0);
    telemetry.insert(tk::EIR_RATIO, 0.0);
    telemetry.insert(tk::BIQUADRATIC_CURVE_SOURCE, 0.0);
    telemetry.insert(tk::HEATING_LATENT_W, 0.0);
    telemetry.insert(tk::DEFROST_CYCLE_STATE, 0.0);
    telemetry.insert(tk::DEFROST_ACCUMULATED_FROST_S, 0.0);
    telemetry.insert(tk::DEFROST_ELAPSED_S, 0.0);
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
            name: tk::DEFROST_EXTRA_POWER_W.to_string(),
            unit: "W".to_string(),
            description: "Additional electric power consumed during defrost".to_string(),
        },
        TelemetryField {
            name: tk::DEFROST_Q_W.to_string(),
            unit: "W".to_string(),
            description: "Heating capacity lost to defrost energy [W]".to_string(),
        },
        TelemetryField {
            name: tk::DEFROST_CAPACITY_MULTIPLIER.to_string(),
            unit: "-".to_string(),
            description: "Capacity multiplier during defrost [0..1]".to_string(),
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
            description: "Hard lockout duration after setpoint increase [s]; 0 = disabled"
                .to_string(),
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
        TelemetryField {
            name: tk::ER_STAGES_ON.to_string(),
            unit: "count".to_string(),
            description: "Number of ER backup heating stages currently active (0 = off)".to_string(),
        },
        TelemetryField {
            name: tk::MIN_COMPRESSOR_FRACTION.to_string(),
            unit: "-".to_string(),
            description: "Minimum compressor speed as a fraction of rated capacity (MSHP only)".to_string(),
        },
        TelemetryField {
            name: tk::FUEL_INPUT_W.to_string(),
            unit: "W".to_string(),
            description: "Fuel backup heater combustion input power".to_string(),
        },
        TelemetryField {
            name: tk::MAX_CAPACITY_FRACTION.to_string(),
            unit: "-".to_string(),
            description: "External max-capacity fraction control [0..1]".to_string(),
        },
        TelemetryField {
            name: tk::CAP_RATIO.to_string(),
            unit: "-".to_string(),
            description: "Biquadratic capacity correction ratio at current conditions".to_string(),
        },
        TelemetryField {
            name: tk::EIR_RATIO.to_string(),
            unit: "-".to_string(),
            description: "Biquadratic EIR correction ratio at current conditions (pre-PLF)".to_string(),
        },
        TelemetryField {
            name: tk::BIQUADRATIC_CURVE_SOURCE.to_string(),
            unit: "enum".to_string(),
            description: "Biquadratic curve provenance: 0=identity, 1=equipment-type default, 2=user-supplied".to_string(),
        },
        TelemetryField {
            name: tk::DEFROST_CYCLE_STATE.to_string(),
            unit: "enum".to_string(),
            description: "Defrost cycle state: 0=Accumulating, 1=Defrosting".to_string(),
        },
        TelemetryField {
            name: tk::DEFROST_ACCUMULATED_FROST_S.to_string(),
            unit: "s".to_string(),
            description: "Accumulated frost proxy time since last defrost cycle [s]".to_string(),
        },
        TelemetryField {
            name: tk::DEFROST_ELAPSED_S.to_string(),
            unit: "s".to_string(),
            description: "Elapsed time in current defrost cycle [s]".to_string(),
        },
        TelemetryField {
            name: tk::HEATING_LATENT_W.to_string(),
            unit: "W".to_string(),
            description: "Heating-side latent gain to zone; non-zero only during reverse-cycle defrost with heating_shr < 1.0".to_string(),
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
