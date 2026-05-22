//! Air conditioner curve helpers and telemetry -- typed config structs live in cooling_config.

use hares_types::{Telemetry, TelemetryField, telemetry_keys as tk};

use super::core_config::parse_biquadratic_list;
use crate::EquipmentConfig;

pub(super) use super::cooling_config::{
    CentralAirConditionerConfig, DehumidifierConfig, RoomAcConfig,
};
pub(super) use super::heat_pump_config::{HeatPumpCoolerConfig, HeatPumpHeaterConfig};

pub(super) const DEFAULT_AC_CAPACITY_CURVE: [f64; 6] =
    [1.5509, -0.07505, 0.0031, 0.0024, -0.00005, -0.00043];
pub(super) const DEFAULT_AC_EIR_CURVE: [f64; 6] =
    [-0.30428, 0.11805, -0.00342, -0.00626, 0.0007, -0.00047];
pub(super) const DEFAULT_ROOM_AC_CAPACITY_CURVE: [f64; 6] =
    [0.6405, 0.01568, 0.0004531, 0.001615, -0.0001825, 0.00006614];
pub(super) const DEFAULT_ROOM_AC_EIR_CURVE: [f64; 6] =
    [2.287, -0.1732, 0.004745, 0.01662, 0.000484, -0.001306];

pub(super) fn default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(19);
    telemetry.insert(tk::ELECTRIC_KW, 0.0);
    telemetry.insert(tk::SENSIBLE_COOLING_W, 0.0);
    telemetry.insert(tk::LATENT_COOLING_W, 0.0);
    telemetry.insert(tk::COIL_SENSIBLE_COOLING_W, 0.0);
    telemetry.insert(tk::COIL_LATENT_COOLING_W, 0.0);
    telemetry.insert(tk::FAN_HEAT_W, 0.0);
    telemetry.insert(tk::SHR, 1.0);
    telemetry.insert(tk::OPERATING_MODE, 0.0);
    telemetry.insert(tk::SPEED_INDEX, 0.0);
    telemetry.insert(tk::COP, 0.0);
    telemetry.insert(tk::RUNTIME_FRACTION, 0.0);
    telemetry.insert(tk::COMPRESSOR_KW, 0.0);
    telemetry.insert(tk::FAN_KW, 0.0);
    telemetry.insert(tk::SUPPLY_TEMP_C, 0.0);
    telemetry.insert(tk::APPARATUS_DEW_POINT_C, 0.0);
    telemetry.insert(tk::BYPASS_FACTOR, 0.0);
    telemetry.insert(tk::MAX_CAPACITY_FRACTION, 1.0);
    telemetry.insert(tk::HEATING_SETPOINT_C, 0.0);
    telemetry.insert(tk::COOLING_SETPOINT_C, 0.0);
    telemetry
}

pub(super) fn telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: tk::ELECTRIC_KW.to_string(),
            unit: "kW".to_string(),
            description: "Total cooling electric power: compressor + fan + crankcase".to_string(),
        },
        TelemetryField {
            name: tk::SENSIBLE_COOLING_W.to_string(),
            unit: "W".to_string(),
            description: "Delivered sensible cooling magnitude".to_string(),
        },
        TelemetryField {
            name: tk::LATENT_COOLING_W.to_string(),
            unit: "W".to_string(),
            description: "Delivered latent cooling magnitude".to_string(),
        },
        TelemetryField {
            name: tk::COIL_SENSIBLE_COOLING_W.to_string(),
            unit: "W".to_string(),
            description: "Gross sensible cooling at the coil (positive magnitude, pre-DSE); distinguishes coil output from fan waste heat".to_string(),
        },
        TelemetryField {
            name: tk::COIL_LATENT_COOLING_W.to_string(),
            unit: "W".to_string(),
            description: "Gross latent cooling at the coil (pre-DSE)".to_string(),
        },
        TelemetryField {
            name: tk::FAN_HEAT_W.to_string(),
            unit: "W".to_string(),
            description: "Supply fan waste heat added to the zone (positive); per E+ I/O Ref: fan motor heat in the supply air stream".to_string(),
        },
        TelemetryField {
            name: tk::SHR.to_string(),
            unit: "-".to_string(),
            description: "Sensible heat ratio".to_string(),
        },
        TelemetryField {
            name: tk::OPERATING_MODE.to_string(),
            unit: "enum".to_string(),
            description: "Operating mode code: 0=Off, 2=Cooling".to_string(),
        },
        TelemetryField {
            name: tk::SPEED_INDEX.to_string(),
            unit: "-".to_string(),
            description: "Active compressor speed stage index (0-based)".to_string(),
        },
        TelemetryField {
            name: tk::COP.to_string(),
            unit: "-".to_string(),
            description: "Coefficient of performance (AHRI: excludes fan)".to_string(),
        },
        TelemetryField {
            name: tk::RUNTIME_FRACTION.to_string(),
            unit: "-".to_string(),
            description: "Runtime fraction (PLR/PLF) [0..1]".to_string(),
        },
        TelemetryField {
            name: tk::COMPRESSOR_KW.to_string(),
            unit: "kW".to_string(),
            description: "Compressor-only electric power".to_string(),
        },
        TelemetryField {
            name: tk::FAN_KW.to_string(),
            unit: "kW".to_string(),
            description: "Supply fan electric power".to_string(),
        },
        TelemetryField {
            name: tk::SUPPLY_TEMP_C.to_string(),
            unit: "C".to_string(),
            description: "Supply air temperature leaving the coil".to_string(),
        },
        TelemetryField {
            name: tk::APPARATUS_DEW_POINT_C.to_string(),
            unit: "C".to_string(),
            description: "Apparatus dew point temperature at the coil".to_string(),
        },
        TelemetryField {
            name: tk::BYPASS_FACTOR.to_string(),
            unit: "-".to_string(),
            description: "Coil bypass factor (fraction of air bypassing the coil)".to_string(),
        },
        TelemetryField {
            name: tk::MAX_CAPACITY_FRACTION.to_string(),
            unit: "-".to_string(),
            description: "External max-capacity fraction control [0..1]".to_string(),
        },
        TelemetryField {
            name: tk::HEATING_SETPOINT_C.to_string(),
            unit: "C".to_string(),
            description: "Active heating setpoint temperature".to_string(),
        },
        TelemetryField {
            name: tk::COOLING_SETPOINT_C.to_string(),
            unit: "C".to_string(),
            description: "Active cooling setpoint temperature".to_string(),
        },
    ]
}

pub(super) fn load_curve_pair(
    config: &EquipmentConfig,
    is_room_ac: bool,
) -> crate::Result<Vec<[f64; 6]>> {
    let mut curves = if let Some(raw) = config.get_str("biquadratic_coeffs") {
        parse_biquadratic_list(raw)?
    } else {
        Vec::new()
    };

    // Per-stage cap/eir keys: may contain multiple 6-element chunks.
    // Interleave them: [cap_0, eir_0, cap_1, eir_1, ...].
    let cap_curves = config
        .get_str("capacity_biquadratic_coeffs")
        .map(parse_biquadratic_list)
        .transpose()?
        .unwrap_or_default();
    let eir_curves = config
        .get_str("eir_biquadratic_coeffs")
        .map(parse_biquadratic_list)
        .transpose()?
        .unwrap_or_default();

    if !cap_curves.is_empty() || !eir_curves.is_empty() {
        let n_stages = cap_curves.len().max(eir_curves.len());
        let default_cap = if is_room_ac {
            DEFAULT_ROOM_AC_CAPACITY_CURVE
        } else {
            DEFAULT_AC_CAPACITY_CURVE
        };
        let default_eir = if is_room_ac {
            DEFAULT_ROOM_AC_EIR_CURVE
        } else {
            DEFAULT_AC_EIR_CURVE
        };
        let mut interleaved = Vec::with_capacity(n_stages * 2);
        for i in 0..n_stages {
            let cap = cap_curves.get(i).copied().unwrap_or(default_cap);
            let eir = eir_curves.get(i).copied().unwrap_or(default_eir);
            interleaved.push(cap);
            interleaved.push(eir);
        }
        curves = interleaved;
    }

    if curves.is_empty() {
        curves.push(if is_room_ac {
            DEFAULT_ROOM_AC_CAPACITY_CURVE
        } else {
            DEFAULT_AC_CAPACITY_CURVE
        });
        curves.push(if is_room_ac {
            DEFAULT_ROOM_AC_EIR_CURVE
        } else {
            DEFAULT_AC_EIR_CURVE
        });
    } else if curves.len() == 1 {
        tracing::warn!(
            "Only one biquadratic curve provided; duplicating for both capacity and EIR. \
             This is likely incorrect."
        );
        curves.push(curves[0]);
    }

    Ok(curves)
}
