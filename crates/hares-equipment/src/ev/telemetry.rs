use hares_types::telemetry_keys as tk;
use hares_types::{
    ChargingLevel, EV_CHARGING_LEVEL_LABELS, EV_CONNECTION_STATE_LABELS, Telemetry, TelemetryField,
};

use super::config::{DEFAULT_CAPACITY_J, DEFAULT_FUEL_ECONOMY_J_PER_M, DEFAULT_SOC};
use super::telemetry_code;

pub(super) fn default_telemetry(charging_level: ChargingLevel) -> Telemetry {
    let mut t = Telemetry::with_capacity(21);
    t.insert(tk::SOC, DEFAULT_SOC);
    t.insert(tk::ACTIVE_POWER_KW, 0.0);
    t.insert(tk::CONNECTION_STATE, 0.0);
    t.insert(tk::CHARGING_LEVEL, telemetry_code(charging_level));
    t.insert(tk::BATTERY_TEMP_C, 20.0);
    t.insert(tk::HEATER_POWER_W, 0.0);
    t.insert(tk::OHMIC_LOSS_W, 0.0);
    t.insert(tk::CHARGE_DERATE, 1.0);
    t.insert(tk::DISCHARGE_CEILING_W, 0.0);
    t.insert(tk::CC_CV_DERATE, 1.0);
    t.insert(tk::V2L_ACTIVE, 0.0);
    t.insert(tk::V2L_POWER_KW, 0.0);
    t.insert(tk::CAPACITY_FADE_PCT, 0.0);
    t.insert(tk::AWAY_CHARGE_POWER_KW, 0.0);
    t.insert(tk::CAPACITY_KWH, DEFAULT_CAPACITY_J);
    t.insert(tk::CAPACITY_KWH_RATED, DEFAULT_CAPACITY_J);
    t.insert(tk::FUEL_ECONOMY_KWH_PER_MI, DEFAULT_FUEL_ECONOMY_J_PER_M);
    t.insert(tk::DRIVE_SHORTFALL_KWH, 0.0);
    t.insert(tk::DR_POWER_FRACTION, 1.0);
    t.insert(tk::DR_LEVEL, 0.0);
    t.insert(tk::REACTIVE_POWER_KVAR, 0.0);
    t.insert(tk::DER1547_TRIPPED, 0.0);
    t.insert(tk::DER1547_TRIP_REASON, 0.0);
    t.insert(tk::DER1547_RIDE_THROUGH_S, 0.0);
    t.insert(tk::DER1547_ENTER_SERVICE_S, 0.0);
    t.insert(tk::DER1547_FW_ADJ_PU, 0.0);
    t.insert(tk::DER1547_UNCONFIGURED_EXCURSIONS, 0.0);
    t
}

pub(super) fn telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField::ratio(tk::SOC, "EV battery state of charge [0..1]"),
        TelemetryField::power_kw(
            tk::ACTIVE_POWER_KW,
            "Residential grid-side EV charging power (positive = load)",
        ),
        TelemetryField::code_code(
            tk::CONNECTION_STATE,
            EV_CONNECTION_STATE_LABELS,
            "Connection state (0=HomePluggedIn, 1=AwayPluggedIn, 2=Disconnected)",
        ),
        TelemetryField::code_code(
            tk::CHARGING_LEVEL,
            EV_CHARGING_LEVEL_LABELS,
            "Charging level code (1=L1, 2=L2)",
        ),
        TelemetryField::temp_c(
            tk::BATTERY_TEMP_C,
            "EV pack temperature used for cold-charge derating",
        ),
        TelemetryField::power_w(
            tk::HEATER_POWER_W,
            "Actual battery heater draw (pack-side DC, post-cap)",
        ),
        TelemetryField::power_w(
            tk::OHMIC_LOSS_W,
            "I2R ohmic heating in the cell resistance during \
                         charge/discharge/drive (the only pack heating the \
                         electrical model produces)",
        ),
        TelemetryField::ratio(
            tk::CHARGE_DERATE,
            "Linear BMS temperature derate factor [0..1] — the \
                         applied capability when no charging-curve LUT is \
                         configured; with a LUT, the LUT's temperature axis is \
                         the capability and this reports the ramp only",
        ),
        TelemetryField::power_w(
            tk::DISCHARGE_CEILING_W,
            "The pack's live AC discharge ceiling - the tighter of each \
                         enabled leg's rating and its energy above the \
                         effective discharge floor over the step; 0 when \
                         neither discharge leg is enabled",
        ),
        TelemetryField::ratio(
            tk::CC_CV_DERATE,
            "CC-CV charging taper multiplier [0..1] (1.0=no taper)",
        ),
        TelemetryField::state_flag(
            tk::V2L_ACTIVE,
            "V2L discharge active (1 discharging, 0 not)",
        ),
        TelemetryField::power_kw(tk::V2L_POWER_KW, "V2L discharge power magnitude"),
        TelemetryField::ratio_percent(
            tk::CAPACITY_FADE_PCT,
            "Cumulative capacity fade from degradation",
        ),
        TelemetryField::power_kw(
            tk::AWAY_CHARGE_POWER_KW,
            "Away charging actual power intake (positive = charging, non-residential)",
        ),
        TelemetryField::stock_kwh(tk::CAPACITY_KWH, "EV battery capacity"),
        TelemetryField::constant_sum(
            tk::CAPACITY_KWH_RATED,
            hares_types::Unit::KilowattHour,
            "EV rated (beginning-of-life) battery capacity",
        ),
        TelemetryField::constant_mean(
            tk::FUEL_ECONOMY_KWH_PER_MI,
            hares_types::Unit::KilowattHourPerMile,
            "EV fuel economy for actor trip planning",
        ),
        TelemetryField::running_total(
            tk::DRIVE_SHORTFALL_KWH,
            hares_types::Unit::KilowattHour,
            "Cumulative drive energy dispatched but not deliverable by the pack \
                         (trip longer than remaining range); accounted, never silently dropped",
        ),
        TelemetryField::ratio(
            tk::DR_POWER_FRACTION,
            "Demand response power scaling factor [0..1]",
        ),
        TelemetryField::code_code(
            tk::DR_LEVEL,
            &["Normal", "Moderate", "High", "Critical", "GridEmergency"],
            "Demand response level (0=Normal, 1=Moderate, 2=High, 3=Critical, 4=GridEmergency)",
        ),
        TelemetryField::power_kvar(
            tk::REACTIVE_POWER_KVAR,
            "Reactive power (positive=absorbing vars, negative=supplying)",
        ),
        TelemetryField::code_code(
            tk::DER1547_TRIPPED,
            &["online", "tripped"],
            "IEEE 1547 export-inverter trip state: 0.0 = online, 1.0 = \
                 tripped (cease to energize). Always 0.0 when no `der_1547` \
                 block is configured.",
        ),
        TelemetryField::code_code(
            tk::DER1547_TRIP_REASON,
            crate::der1547::trip_reason::LABELS.as_slice(),
            "The IEEE 1547 trip condition that fired: 0.0 = none, 1.0 = uv2, \
                 2.0 = uv1, 3.0 = ov1, 4.0 = ov2, 5.0 = uf2, 6.0 = uf1, \
                 7.0 = of1, 8.0 = of2",
        ),
        TelemetryField::flow_mean(
            tk::DER1547_RIDE_THROUGH_S,
            hares_types::Unit::Second,
            "Seconds accumulated in the governing IEEE 1547 trip band this \
                 disturbance (the clearing-time window's position)",
        ),
        TelemetryField::flow_mean(
            tk::DER1547_ENTER_SERVICE_S,
            hares_types::Unit::Second,
            "Seconds accumulated inside the IEEE 1547 enter-service windows \
                 while tripped (the return-to-service delay's position)",
        ),
        TelemetryField::ratio(
            tk::DER1547_FW_ADJ_PU,
            "The IEEE 1547 frequency-droop adjustment, per unit of the \
                 export inverter's active-power rating (negative = curtailed)",
        ),
        TelemetryField::running_total(
            tk::DER1547_UNCONFIGURED_EXCURSIONS,
            hares_types::Unit::Count,
            "Grid excursions seen while no der_1547 tables were configured",
        ),
    ]
}
