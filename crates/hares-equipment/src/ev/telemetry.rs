use hares_types::{ChargingLevel, Telemetry, TelemetryField};

use super::config::{DEFAULT_CAPACITY_KWH, DEFAULT_FUEL_ECONOMY_KWH_PER_MI, DEFAULT_SOC};
use super::telemetry_code;

pub(super) fn default_telemetry(charging_level: ChargingLevel) -> Telemetry {
    let mut t = Telemetry::with_capacity(14);
    t.insert("soc", DEFAULT_SOC);
    t.insert("active_power_kw", 0.0);
    t.insert("connection_state", 0.0);
    t.insert("charging_level", telemetry_code(charging_level));
    t.insert("battery_temp_c", 20.0);
    t.insert("heater_power_w", 0.0);
    t.insert("charge_derate", 1.0);
    t.insert("v2l_active", 0.0);
    t.insert("v2l_power_kw", 0.0);
    t.insert("capacity_fade_pct", 0.0);
    t.insert("away_charge_power_kw", 0.0);
    t.insert("capacity_kwh", DEFAULT_CAPACITY_KWH);
    t.insert("fuel_economy_kwh_per_mi", DEFAULT_FUEL_ECONOMY_KWH_PER_MI);
    t
}

pub(super) fn telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: "soc".to_string(),
            unit: "-".to_string(),
            description: "EV battery state of charge [0..1]".to_string(),
        },
        TelemetryField {
            name: "active_power_kw".to_string(),
            unit: "kW".to_string(),
            description: "Residential grid-side EV charging power (positive = load)".to_string(),
        },
        TelemetryField {
            name: "connection_state".to_string(),
            unit: "code".to_string(),
            description: "Connection state (0=HomePluggedIn, 1=AwayPluggedIn, 2=Disconnected)"
                .to_string(),
        },
        TelemetryField {
            name: "charging_level".to_string(),
            unit: "code".to_string(),
            description: "Charging level code (1=L1, 2=L2)".to_string(),
        },
        TelemetryField {
            name: "battery_temp_c".to_string(),
            unit: "C".to_string(),
            description: "EV pack temperature used for cold-charge derating".to_string(),
        },
        TelemetryField {
            name: "heater_power_w".to_string(),
            unit: "W".to_string(),
            description: "Battery heater power draw when active".to_string(),
        },
        TelemetryField {
            name: "charge_derate".to_string(),
            unit: "-".to_string(),
            description: "Temperature-based charging derate factor [0..1]".to_string(),
        },
        TelemetryField {
            name: "v2l_active".to_string(),
            unit: "-".to_string(),
            description: "V2L discharge active (1 discharging, 0 not)".to_string(),
        },
        TelemetryField {
            name: "v2l_power_kw".to_string(),
            unit: "kW".to_string(),
            description: "V2L discharge power magnitude".to_string(),
        },
        TelemetryField {
            name: "capacity_fade_pct".to_string(),
            unit: "%".to_string(),
            description: "Cumulative capacity fade from degradation".to_string(),
        },
        TelemetryField {
            name: "away_charge_power_kw".to_string(),
            unit: "kW".to_string(),
            description: "Away charger rated power (non-residential)".to_string(),
        },
        TelemetryField {
            name: "capacity_kwh".to_string(),
            unit: "kWh".to_string(),
            description: "EV battery capacity".to_string(),
        },
        TelemetryField {
            name: "fuel_economy_kwh_per_mi".to_string(),
            unit: "kWh/mi".to_string(),
            description: "EV fuel economy for actor trip planning".to_string(),
        },
    ]
}
