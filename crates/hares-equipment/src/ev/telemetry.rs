use hares_types::telemetry_keys as tk;
use hares_types::{ChargingLevel, Telemetry, TelemetryField};

use super::config::{DEFAULT_CAPACITY_KWH, DEFAULT_FUEL_ECONOMY_KWH_PER_MI, DEFAULT_SOC};
use super::telemetry_code;

pub(super) fn default_telemetry(charging_level: ChargingLevel) -> Telemetry {
    let mut t = Telemetry::with_capacity(14);
    t.insert(tk::SOC, DEFAULT_SOC);
    t.insert(tk::ACTIVE_POWER_KW, 0.0);
    t.insert(tk::ELECTRIC_KW, 0.0);
    t.insert(tk::CONNECTION_STATE, 0.0);
    t.insert(tk::CHARGING_LEVEL, telemetry_code(charging_level));
    t.insert(tk::BATTERY_TEMP_C, 20.0);
    t.insert(tk::HEATER_POWER_W, 0.0);
    t.insert(tk::CHARGE_DERATE, 1.0);
    t.insert(tk::V2L_ACTIVE, 0.0);
    t.insert(tk::V2L_POWER_KW, 0.0);
    t.insert(tk::CAPACITY_FADE_PCT, 0.0);
    t.insert(tk::AWAY_CHARGE_POWER_KW, 0.0);
    t.insert(tk::CAPACITY_KWH, DEFAULT_CAPACITY_KWH);
    t.insert(tk::FUEL_ECONOMY_KWH_PER_MI, DEFAULT_FUEL_ECONOMY_KWH_PER_MI);
    t
}

pub(super) fn telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: tk::SOC.to_string(),
            unit: "-".to_string(),
            description: "EV battery state of charge [0..1]".to_string(),
        },
        TelemetryField {
            name: tk::ACTIVE_POWER_KW.to_string(),
            unit: "kW".to_string(),
            description: "Residential grid-side EV charging power (positive = load)".to_string(),
        },
        TelemetryField {
            name: tk::ELECTRIC_KW.to_string(),
            unit: "kW".to_string(),
            description: "Grid-boundary electrical power".to_string(),
        },
        TelemetryField {
            name: tk::CONNECTION_STATE.to_string(),
            unit: "code".to_string(),
            description: "Connection state (0=HomePluggedIn, 1=AwayPluggedIn, 2=Disconnected)"
                .to_string(),
        },
        TelemetryField {
            name: tk::CHARGING_LEVEL.to_string(),
            unit: "code".to_string(),
            description: "Charging level code (1=L1, 2=L2)".to_string(),
        },
        TelemetryField {
            name: tk::BATTERY_TEMP_C.to_string(),
            unit: "C".to_string(),
            description: "EV pack temperature used for cold-charge derating".to_string(),
        },
        TelemetryField {
            name: tk::HEATER_POWER_W.to_string(),
            unit: "W".to_string(),
            description: "Battery heater power draw when active".to_string(),
        },
        TelemetryField {
            name: tk::CHARGE_DERATE.to_string(),
            unit: "-".to_string(),
            description: "Temperature-based charging derate factor [0..1]".to_string(),
        },
        TelemetryField {
            name: tk::V2L_ACTIVE.to_string(),
            unit: "-".to_string(),
            description: "V2L discharge active (1 discharging, 0 not)".to_string(),
        },
        TelemetryField {
            name: tk::V2L_POWER_KW.to_string(),
            unit: "kW".to_string(),
            description: "V2L discharge power magnitude".to_string(),
        },
        TelemetryField {
            name: tk::CAPACITY_FADE_PCT.to_string(),
            unit: "%".to_string(),
            description: "Cumulative capacity fade from degradation".to_string(),
        },
        TelemetryField {
            name: tk::AWAY_CHARGE_POWER_KW.to_string(),
            unit: "kW".to_string(),
            description: "Away charging actual power intake (positive = charging, non-residential)"
                .to_string(),
        },
        TelemetryField {
            name: tk::CAPACITY_KWH.to_string(),
            unit: "kWh".to_string(),
            description: "EV battery capacity".to_string(),
        },
        TelemetryField {
            name: tk::FUEL_ECONOMY_KWH_PER_MI.to_string(),
            unit: "kWh/mi".to_string(),
            description: "EV fuel economy for actor trip planning".to_string(),
        },
    ]
}
