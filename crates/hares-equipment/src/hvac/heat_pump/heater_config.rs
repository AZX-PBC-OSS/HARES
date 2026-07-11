//! Configuration parsing and initialization helpers for heat-pump heaters.

use hares_types::{OperatingMode, Telemetry, TelemetryField, telemetry_keys as tk};

use super::constants::HEATER_TELEMETRY_CAPACITY;

pub(super) fn default_heater_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(HEATER_TELEMETRY_CAPACITY);
    telemetry.insert(tk::ELECTRIC_KW, 0.0);
    telemetry.insert(tk::REACTIVE_POWER_KVAR, 0.0);
    telemetry.insert(tk::THERMAL_OUTPUT_W, 0.0);
    telemetry.insert(tk::OPERATING_MODE, OperatingMode::Off.as_code());
    telemetry.insert(tk::SPEED_INDEX, 0.0);
    telemetry.insert(tk::DEFROST_ACTIVE, 0.0);
    telemetry.insert(tk::COP, 0.0);
    telemetry.insert(tk::RUNTIME_FRACTION, 0.0);
    telemetry.insert(tk::COMPRESSOR_KW, 0.0);
    telemetry.insert(tk::MAIN_POWER_KW, 0.0);
    telemetry.insert(tk::DUCT_LOSS_W, 0.0);
    telemetry.insert(tk::DEFROST_TIME_FRACTION, 0.0);
    telemetry.insert(tk::DEFROST_EXTRA_POWER_W, 0.0);
    telemetry.insert(tk::DEFROST_Q_W, 0.0);
    telemetry.insert(tk::DEFROST_CAPACITY_MULTIPLIER, 1.0);
    telemetry.insert(tk::HEATING_SETPOINT_C, 0.0);
    telemetry.insert(tk::COOLING_SETPOINT_C, 0.0);
    telemetry.insert(tk::SCHEDULE_HEATING_SETPOINT_C, 0.0);
    telemetry.insert(tk::SCHEDULE_COOLING_SETPOINT_C, 0.0);
    telemetry.insert(tk::RUNTIME_HEATING_SETPOINT_C, 0.0);
    telemetry.insert(tk::RUNTIME_COOLING_SETPOINT_C, 0.0);
    telemetry.insert(tk::FAN_KW, 0.0);
    telemetry.insert(tk::BACKUP_ER_KW, 0.0);
    telemetry.insert(tk::PAN_HEATER_KW, 0.0);
    telemetry.insert(tk::PUMP_POWER_KW, 0.0);
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
    telemetry.insert(tk::CAP_RATIO_RAW, 0.0);
    telemetry.insert(tk::EIR_RATIO, 0.0);
    telemetry.insert(tk::BIQUADRATIC_CURVE_SOURCE, 0.0);
    telemetry.insert(tk::HEATING_LATENT_W, 0.0);
    telemetry.insert(tk::DEFROST_CYCLE_STATE, 0.0);
    telemetry.insert(tk::DEFROST_ACCUMULATED_FROST_S, 0.0);
    telemetry.insert(tk::DEFROST_ELAPSED_S, 0.0);
    telemetry.insert(tk::SPEED_FRAC, 0.0);
    telemetry.insert(tk::PART_LOAD_RATIO, 0.0);
    telemetry.insert(tk::PART_LOAD_FACTOR, 0.0);
    telemetry.insert(tk::STARTUP_MULTIPLIER, 0.0);
    telemetry.insert(tk::DUTY_CYCLE, 0.0);
    telemetry.insert(tk::TIME_AT_CURRENT_SPEED_S, 0.0);
    telemetry.insert(tk::MODE_DURATION_S, 0.0);
    telemetry.insert(tk::MIN_ON_TIME_S, 0.0);
    telemetry.insert(tk::MIN_OFF_TIME_S, 0.0);
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
            name: tk::REACTIVE_POWER_KVAR.to_string(),
            unit: "kVAR".to_string(),
            description: "Reactive power (positive = inductive/lagging), per component: compressor pf 0.84, fan pf 0.87, pump pf 0.84, ER/pan resistive Q=0"
                .to_string(),
        },
        TelemetryField {
            name: tk::THERMAL_OUTPUT_W.to_string(),
            unit: "W".to_string(),
            description: "Delivered sensible heating".to_string(),
        },
        TelemetryField {
            name: tk::OPERATING_MODE.to_string(),
            unit: "enum".to_string(),
            description: "Operating mode code (OperatingMode::as_code): 0=Off, \
                          7=HeatingHP, 8=HeatingER, 9=HeatingHPAndER"
                .to_string(),
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
            name: tk::MAIN_POWER_KW.to_string(),
            unit: "kW".to_string(),
            description: "Main power (compressor-only) per OCHRE HVAC.py:1464-1467".to_string(),
        },
        TelemetryField {
            name: tk::DUCT_LOSS_W.to_string(),
            unit: "W".to_string(),
            description: "Duct distribution losses per ASHRAE 152: gross_capacity * (1 - dse)".to_string(),
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
            name: tk::SCHEDULE_HEATING_SETPOINT_C.to_string(),
            unit: "C".to_string(),
            description: "Schedule-stage heating setpoint (before runtime override)".to_string(),
        },
        TelemetryField {
            name: tk::SCHEDULE_COOLING_SETPOINT_C.to_string(),
            unit: "C".to_string(),
            description: "Schedule-stage cooling setpoint (before runtime override)".to_string(),
        },
        TelemetryField {
            name: tk::RUNTIME_HEATING_SETPOINT_C.to_string(),
            unit: "C".to_string(),
            description: "Runtime override heating setpoint (0.0 when no override active)".to_string(),
        },
        TelemetryField {
            name: tk::RUNTIME_COOLING_SETPOINT_C.to_string(),
            unit: "C".to_string(),
            description: "Runtime override cooling setpoint (0.0 when no override active)".to_string(),
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
            name: tk::PUMP_POWER_KW.to_string(),
            unit: "kW".to_string(),
            description:
                "Ground-loop circulation pump electrical power (GSHP only; zero for air-source)"
                    .to_string(),
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
            name: tk::CAP_RATIO_RAW.to_string(),
            unit: "-".to_string(),
            description:
                "Raw biquadratic capacity output before non-negative clamp"
                    .to_string(),
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
        TelemetryField {
            name: tk::SPEED_FRAC.to_string(),
            unit: "-".to_string(),
            description: "Interpolation weight between speed stages [0..1]".to_string(),
        },
        TelemetryField {
            name: tk::PART_LOAD_RATIO.to_string(),
            unit: "-".to_string(),
            description: "Heating load fraction at lowest speed stage [0..1]".to_string(),
        },
        TelemetryField {
            name: tk::PART_LOAD_FACTOR.to_string(),
            unit: "-".to_string(),
            description: "EIR degradation correction from cycling (PLF)".to_string(),
        },
        TelemetryField {
            name: tk::STARTUP_MULTIPLIER.to_string(),
            unit: "-".to_string(),
            description: "Capacity ramp multiplier on compressor restart (Winkler 2009)".to_string(),
        },
        TelemetryField {
            name: tk::DUTY_CYCLE.to_string(),
            unit: "-".to_string(),
            description: "Thermostat on/off fraction this timestep [0..1]".to_string(),
        },
        TelemetryField {
            name: tk::TIME_AT_CURRENT_SPEED_S.to_string(),
            unit: "s".to_string(),
            description: "Seconds since the last speed-stage change".to_string(),
        },
        TelemetryField {
            name: tk::MODE_DURATION_S.to_string(),
            unit: "s".to_string(),
            description: "Seconds since the last thermostat mode change".to_string(),
        },
        TelemetryField {
            name: tk::MIN_ON_TIME_S.to_string(),
            unit: "s".to_string(),
            description: "Minimum compressor on-time for short-cycle protection".to_string(),
        },
        TelemetryField {
            name: tk::MIN_OFF_TIME_S.to_string(),
            unit: "s".to_string(),
            description: "Minimum compressor off-time for short-cycle protection".to_string(),
        },
    ]
}
