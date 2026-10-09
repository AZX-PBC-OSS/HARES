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
    telemetry.insert(tk::ZONE_RISING, 0.0);
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
    telemetry.insert(tk::HIGH_SIDE_CURVE_CLAMPED_SPEED_FRAC, 0.0);
    // Observe-gated diagnostics: an unknown telemetry key latches a typed
    // error surfaced at the end of the step, so every observe-only set site
    // must have its key pre-populated here.
    telemetry.insert(tk::PART_LOAD_RATIO, 0.0);
    telemetry.insert(tk::PART_LOAD_FACTOR, 0.0);
    telemetry.insert(tk::STARTUP_MULTIPLIER, 0.0);
    telemetry.insert(tk::TIME_SINCE_START_MIN, 0.0);
    telemetry.insert(tk::STARTUP_TIMER_RESET_COUNT, 0.0);
    telemetry.insert(tk::DUTY_CYCLE, 0.0);
    telemetry.insert(tk::TIME_AT_CURRENT_SPEED_S, 0.0);
    telemetry.insert(tk::MODE_DURATION_S, 0.0);
    telemetry.insert(tk::MIN_ON_TIME_S, 0.0);
    telemetry.insert(tk::MIN_OFF_TIME_S, 0.0);
    telemetry
}

pub(super) fn heater_telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField::power_kw(tk::ELECTRIC_KW, "Total heater electric power"),
        TelemetryField::power_kvar(
            tk::REACTIVE_POWER_KVAR,
            "Reactive power (positive = inductive/lagging), per component: compressor pf 0.84, fan pf 0.87, pump pf 0.84, ER/pan resistive Q=0",
        ),
        TelemetryField::power_w(tk::THERMAL_OUTPUT_W, "Delivered sensible heating"),
        TelemetryField::code_code(
            tk::OPERATING_MODE,
            hares_types::MODE_LABELS,
            "Operating mode code (OperatingMode::as_code): 0=Off, \
                          7=HeatingHP, 8=HeatingER, 9=HeatingHPAndER",
        ),
        TelemetryField::code_code(
            tk::SPEED_INDEX,
            &["0", "1", "2", "3"],
            "Selected compressor speed stage",
        ),
        TelemetryField::boolean(tk::DEFROST_ACTIVE, "1 when defrost correction is active"),
        TelemetryField::ratio(
            tk::COP,
            "COP per AHRI convention: gross thermal output / compressor-only electric input",
        ),
        TelemetryField::ratio(
            tk::RUNTIME_FRACTION,
            "Compressor runtime fraction (part-load ratio) this timestep",
        ),
        TelemetryField::power_kw(tk::COMPRESSOR_KW, "Compressor-only electric power"),
        TelemetryField::power_kw(
            tk::MAIN_POWER_KW,
            "Main power (compressor-only) per OCHRE HVAC.py:1464-1467",
        ),
        TelemetryField::power_w(
            tk::DUCT_LOSS_W,
            "Duct distribution losses per ASHRAE 152: gross_capacity * (1 - dse)",
        ),
        TelemetryField::ratio(
            tk::DEFROST_TIME_FRACTION,
            "Fraction of timestep in defrost mode [0..1]",
        ),
        TelemetryField::power_w(
            tk::DEFROST_EXTRA_POWER_W,
            "Additional electric power consumed during defrost",
        ),
        TelemetryField::power_w(
            tk::DEFROST_Q_W,
            "Heating capacity lost to defrost energy [W]",
        ),
        TelemetryField::ratio(
            tk::DEFROST_CAPACITY_MULTIPLIER,
            "Capacity multiplier during defrost [0..1]",
        ),
        TelemetryField::temp_c(
            tk::HEATING_SETPOINT_C,
            "Active heating setpoint used by the heater control logic",
        ),
        TelemetryField::temp_c(
            tk::COOLING_SETPOINT_C,
            "Active cooling setpoint from shared thermostat state",
        ),
        TelemetryField::temp_c(
            tk::SCHEDULE_HEATING_SETPOINT_C,
            "Schedule-stage heating setpoint (before runtime override)",
        ),
        TelemetryField::temp_c(
            tk::SCHEDULE_COOLING_SETPOINT_C,
            "Schedule-stage cooling setpoint (before runtime override)",
        ),
        TelemetryField::temp_c(
            tk::RUNTIME_HEATING_SETPOINT_C,
            "Runtime override heating setpoint (0.0 when no override active)",
        ),
        TelemetryField::temp_c(
            tk::RUNTIME_COOLING_SETPOINT_C,
            "Runtime override cooling setpoint (0.0 when no override active)",
        ),
        TelemetryField::power_kw(tk::FAN_KW, "Fan/blower electric power"),
        TelemetryField::power_kw(tk::BACKUP_ER_KW, "Electric resistance backup heater power"),
        TelemetryField::power_kw(tk::PAN_HEATER_KW, "Minisplit pan/crankcase heater power"),
        TelemetryField::power_kw(
            tk::PUMP_POWER_KW,
            "Ground-loop circulation pump electrical power (GSHP only; zero for air-source)",
        ),
        TelemetryField::power_w(
            tk::HP_CAPACITY_W,
            "Heat pump thermal output before duct losses",
        ),
        TelemetryField::power_w(
            tk::ER_CAPACITY_W,
            "Electric resistance backup thermal output",
        ),
        TelemetryField::temp_c(
            tk::HP_LOCKOUT_TEMP_C,
            "Outdoor temperature below which the heat pump is locked out",
        ),
        TelemetryField::temp_c(
            tk::ER_LOCKOUT_TEMP_C,
            "Outdoor temperature above which ER backup is locked out",
        ),
        TelemetryField::temp_c(
            tk::ER_SETPOINT_OFFSET_C,
            "Setpoint offset above which ER backup may activate",
        ),
        TelemetryField::timer_s(
            tk::ER_HARD_LOCKOUT_TIME_S,
            "Hard lockout duration after setpoint increase [s]; 0 = disabled",
        ),
        TelemetryField::power_w(tk::BACKUP_CAPACITY_W, "Rated backup ER heating capacity"),
        TelemetryField::ratio(tk::BACKUP_EIR, "Backup heater energy input ratio"),
        TelemetryField::stock_count(
            tk::ER_STAGES_ON,
            "Number of ER backup heating stages currently active (0 = off)",
        ),
        TelemetryField::ratio(
            tk::MIN_COMPRESSOR_FRACTION,
            "Minimum compressor speed as a fraction of rated capacity (MSHP only)",
        ),
        TelemetryField::power_w(
            tk::FUEL_INPUT_W,
            "Fuel backup heater combustion input power",
        ),
        TelemetryField::ratio(
            tk::MAX_CAPACITY_FRACTION,
            "External max-capacity fraction control [0..1]",
        ),
        TelemetryField::ratio(
            tk::CAP_RATIO,
            "Biquadratic capacity correction ratio at current conditions",
        ),
        TelemetryField::ratio(
            tk::CAP_RATIO_RAW,
            "Raw biquadratic capacity output before non-negative clamp",
        ),
        TelemetryField::ratio(
            tk::EIR_RATIO,
            "Biquadratic EIR correction ratio at current conditions (pre-PLF)",
        ),
        TelemetryField::code_code(
            tk::BIQUADRATIC_CURVE_SOURCE,
            &["identity", "equipment", "user"],
            "Biquadratic curve provenance: 0=identity, 1=equipment-type default, 2=user-supplied",
        ),
        TelemetryField::code_enum(
            tk::DEFROST_CYCLE_STATE,
            &["Accumulating", "Defrosting"],
            "Defrost cycle state: 0=Accumulating, 1=Defrosting",
        ),
        TelemetryField::timer_s(
            tk::DEFROST_ACCUMULATED_FROST_S,
            "Accumulated frost proxy time since last defrost cycle [s]",
        ),
        TelemetryField::timer_s(
            tk::DEFROST_ELAPSED_S,
            "Elapsed time in current defrost cycle [s]",
        ),
        TelemetryField::power_w(
            tk::HEATING_LATENT_W,
            "Heating-side latent gain to zone; non-zero only during reverse-cycle defrost with heating_shr < 1.0",
        ),
        TelemetryField::ratio(
            tk::SPEED_FRAC,
            "Interpolation weight between speed stages [0..1]",
        ),
        TelemetryField::ratio(
            tk::HIGH_SIDE_CURVE_CLAMPED_SPEED_FRAC,
            "speed_frac recorded when high-side curve index clamped to last valid stage; 0 means no clamping this step",
        ),
        TelemetryField::ratio(
            tk::PART_LOAD_RATIO,
            "Heating load fraction at lowest speed stage [0..1]",
        ),
        TelemetryField::ratio(
            tk::PART_LOAD_FACTOR,
            "EIR degradation correction from cycling (PLF)",
        ),
        TelemetryField::ratio(
            tk::STARTUP_MULTIPLIER,
            "Capacity ramp multiplier on compressor restart (Winkler 2009)",
        ),
        TelemetryField::timer_min(
            tk::TIME_SINCE_START_MIN,
            "Elapsed time since compressor startup; preserved across off-steps, reset on off→on transition",
        ),
        TelemetryField::running_total(
            tk::STARTUP_TIMER_RESET_COUNT,
            hares_types::Unit::Count,
            "Cumulative startup-timer resets (off→on transitions) since init; difference consecutive rows for resets per hour",
        ),
        TelemetryField::ratio(
            tk::DUTY_CYCLE,
            "Thermostat on/off fraction this timestep [0..1]",
        ),
        TelemetryField::timer_s(
            tk::TIME_AT_CURRENT_SPEED_S,
            "Seconds since the last speed-stage change",
        ),
        TelemetryField::timer_s(
            tk::MODE_DURATION_S,
            "Seconds since the last thermostat mode change",
        ),
        TelemetryField::constant_mean(
            tk::MIN_ON_TIME_S,
            hares_types::Unit::Second,
            "Minimum compressor on-time for short-cycle protection",
        ),
        TelemetryField::constant_mean(
            tk::MIN_OFF_TIME_S,
            hares_types::Unit::Second,
            "Minimum compressor off-time for short-cycle protection",
        ),
        TelemetryField::boolean(
            tk::ZONE_RISING,
            "1 when zone temperature has not yet declined after ER hard lockout expired (soft lockout active); 0 otherwise",
        ),
        TelemetryField::ratio(tk::EBM_EFFICIENCY, "EBM efficiency (COP = 1/EIR)"),
        TelemetryField::power_kw(
            tk::EBM_BASELINE_POWER_KW,
            "EBM baseline power to hold setpoint",
        ),
        TelemetryField::stock_kwh(tk::EBM_ENERGY_KWH, "EBM current energy state"),
        TelemetryField::stock_kwh(
            tk::EBM_MIN_ENERGY_KWH,
            "EBM minimum energy at turn-on threshold",
        ),
        TelemetryField::stock_kwh(
            tk::EBM_MAX_ENERGY_KWH,
            "EBM maximum energy at turn-off threshold",
        ),
        TelemetryField::power_kw(tk::EBM_MAX_POWER_KW, "EBM maximum electrical power"),
    ]
}
