//! Air conditioner curve helpers and telemetry — typed config structs live in cooling_config.

use hares_types::{HaresError, Telemetry, TelemetryField, parse_trimmed_f64, telemetry_keys as tk};

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
    let mut telemetry = Telemetry::with_capacity(12);
    telemetry.insert(tk::ELECTRIC_KW, 0.0);
    telemetry.insert(tk::SENSIBLE_COOLING_W, 0.0);
    telemetry.insert(tk::LATENT_COOLING_W, 0.0);
    telemetry.insert(tk::SHR, 1.0);
    telemetry.insert(tk::OPERATING_MODE, 0.0);
    telemetry.insert(tk::COP, 0.0);
    telemetry.insert(tk::RUNTIME_FRACTION, 0.0);
    telemetry.insert(tk::COMPRESSOR_KW, 0.0);
    telemetry.insert(tk::FAN_KW, 0.0);
    telemetry.insert(tk::SUPPLY_TEMP_C, 0.0);
    telemetry.insert(tk::APPARATUS_DEW_POINT_C, 0.0);
    telemetry.insert(tk::BYPASS_FACTOR, 0.0);
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

    if let Some(cap) = parse_single_coeff_array(config.get_str("capacity_biquadratic_coeffs"))? {
        if curves.is_empty() {
            curves.push(cap);
        } else {
            curves[0] = cap;
        }
    }

    if let Some(eir) = parse_single_coeff_array(config.get_str("eir_biquadratic_coeffs"))? {
        if curves.len() < 2 {
            curves.resize(2, eir);
        }
        curves[1] = eir;
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

fn parse_single_coeff_array(raw: Option<&str>) -> crate::Result<Option<[f64; 6]>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let curves = parse_biquadratic_list(raw)?;
    match curves.len() {
        0 => Ok(None),
        1 => Ok(Some(curves[0])),
        n => Err(HaresError::Equipment(format!(
            "expected exactly 6 biquadratic coefficients, got {}",
            n * 6
        ))),
    }
}

/// Parse optional crankcase heater capacity curve coefficients `[c0, c1, c2]`
/// from a config string such as `"[1.0, -0.02, 0.0]"`.
/// effective_capacity = rated * (c0 + c1*T + c2*T^2), clamped to >= 0.
pub(super) fn parse_crankcase_capacity_curve(raw: Option<&str>) -> crate::Result<Option<[f64; 3]>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let values: Vec<f64> = raw
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .filter_map(parse_trimmed_f64)
        .collect();
    if values.len() != 3 {
        return Err(HaresError::Equipment(format!(
            "crankcase_capacity_curve_coeffs must contain exactly 3 values [c0, c1, c2], \
             got {}",
            values.len()
        )));
    }
    Ok(Some([values[0], values[1], values[2]]))
}
