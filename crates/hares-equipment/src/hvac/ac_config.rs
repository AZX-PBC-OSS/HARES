//! Air conditioner curve helpers and telemetry -- typed config structs live in cooling_config.

use hares_types::{HaresError, Telemetry, TelemetryField, telemetry_keys as tk};

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
    let mut telemetry = Telemetry::with_capacity(37);
    telemetry.insert(tk::ELECTRIC_KW, 0.0);
    telemetry.insert(tk::REACTIVE_POWER_KVAR, 0.0);
    telemetry.insert(tk::SENSIBLE_COOLING_W, 0.0);
    telemetry.insert(tk::LATENT_COOLING_W, 0.0);
    telemetry.insert(tk::COIL_SENSIBLE_COOLING_W, 0.0);
    telemetry.insert(tk::COIL_LATENT_COOLING_W, 0.0);
    telemetry.insert(tk::LATENT_GAINS_W, 0.0);
    telemetry.insert(tk::FAN_HEAT_W, 0.0);
    telemetry.insert(tk::SHR, 1.0);
    telemetry.insert(tk::OPERATING_MODE, 0.0);
    telemetry.insert(tk::SPEED_INDEX, 0.0);
    telemetry.insert(tk::COP, 0.0);
    telemetry.insert(tk::RUNTIME_FRACTION, 0.0);
    telemetry.insert(tk::COMPRESSOR_KW, 0.0);
    telemetry.insert(tk::FAN_KW, 0.0);
    telemetry.insert(tk::CRANKCASE_KW, 0.0);
    // OCHRE HVAC.py:575: main_power = total_input - fan.
    telemetry.insert(tk::MAIN_POWER_KW, 0.0);
    // ASHRAE 152: duct_loss = gross_capacity * (1 - dse).
    telemetry.insert(tk::DUCT_LOSS_W, 0.0);
    telemetry.insert(tk::SUPPLY_TEMP_C, 0.0);
    telemetry.insert(tk::APPARATUS_DEW_POINT_C, 0.0);
    telemetry.insert(tk::BYPASS_FACTOR, 0.0);
    telemetry.insert(tk::MAX_CAPACITY_FRACTION, 1.0);
    telemetry.insert(tk::HEATING_SETPOINT_C, 0.0);
    telemetry.insert(tk::COOLING_SETPOINT_C, 0.0);
    telemetry.insert(tk::SCHEDULE_HEATING_SETPOINT_C, 0.0);
    telemetry.insert(tk::SCHEDULE_COOLING_SETPOINT_C, 0.0);
    telemetry.insert(tk::RUNTIME_HEATING_SETPOINT_C, 0.0);
    telemetry.insert(tk::RUNTIME_COOLING_SETPOINT_C, 0.0);
    telemetry.insert(tk::SPEED_FRAC, 0.0);
    telemetry.insert(tk::HIGH_SIDE_CURVE_CLAMPED_SPEED_FRAC, 0.0);
    telemetry.insert(tk::PART_LOAD_RATIO, 0.0);
    telemetry.insert(tk::PART_LOAD_FACTOR, 0.0);
    telemetry.insert(tk::STARTUP_MULTIPLIER, 0.0);
    telemetry.insert(tk::DUTY_CYCLE, 0.0);
    telemetry.insert(tk::TIME_AT_CURRENT_SPEED_S, 0.0);
    telemetry.insert(tk::MODE_DURATION_S, 0.0);
    telemetry.insert(tk::MIN_ON_TIME_S, 0.0);
    telemetry.insert(tk::MIN_OFF_TIME_S, 0.0);
    telemetry.insert(tk::PUMP_POWER_KW, 0.0);
    telemetry.insert(tk::COOLING_OAT_LOCKOUT, 0.0);
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
            name: tk::REACTIVE_POWER_KVAR.to_string(),
            unit: "kVAR".to_string(),
            description: "Reactive power (positive = inductive/lagging), per component: compressor pf 0.96, fan pf 0.87, crankcase resistive Q=0"
                .to_string(),
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
            name: tk::LATENT_GAINS_W.to_string(),
            unit: "W".to_string(),
            description: "Pre-DSE gross latent cooling scaled by space_fraction; matches OCHRE HVAC.py:595 Latent Gains column".to_string(),
        },
        TelemetryField {
            name: tk::FAN_HEAT_W.to_string(),
            unit: "W".to_string(),
            description: "Supply fan waste heat added to the zone (positive); per E+ I/O Ref: fan motor heat in the supply air stream".to_string(),
        },
        TelemetryField {
            name: hares_types::telemetry_keys::EBM_EFFICIENCY.to_string(),
            unit: "-".to_string(),
            description: "EBM efficiency (COP = 1/EIR)".to_string(),
        },
        TelemetryField {
            name: hares_types::telemetry_keys::EBM_BASELINE_POWER_KW.to_string(),
            unit: "kW".to_string(),
            description: "EBM baseline power to hold setpoint".to_string(),
        },
        TelemetryField {
            name: hares_types::telemetry_keys::EBM_ENERGY_KWH.to_string(),
            unit: "kWh".to_string(),
            description: "EBM current energy state".to_string(),
        },
        TelemetryField {
            name: hares_types::telemetry_keys::EBM_MIN_ENERGY_KWH.to_string(),
            unit: "kWh".to_string(),
            description: "EBM minimum energy at turn-on threshold".to_string(),
        },
        TelemetryField {
            name: hares_types::telemetry_keys::EBM_MAX_ENERGY_KWH.to_string(),
            unit: "kWh".to_string(),
            description: "EBM maximum energy at turn-off threshold".to_string(),
        },
        TelemetryField {
            name: hares_types::telemetry_keys::EBM_MAX_POWER_KW.to_string(),
            unit: "kW".to_string(),
            description: "EBM maximum electrical power".to_string(),
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
            description: "Compressor-only electric power (space_fraction-scaled)".to_string(),
        },
        TelemetryField {
            name: tk::FAN_KW.to_string(),
            unit: "kW".to_string(),
            description: "Supply fan electric power (space_fraction-scaled)".to_string(),
        },
        TelemetryField {
            name: tk::CRANKCASE_KW.to_string(),
            unit: "kW".to_string(),
            description: "Crankcase heater electric power (space_fraction-scaled, resistive Q=0)"
                .to_string(),
        },
        TelemetryField {
            name: tk::MAIN_POWER_KW.to_string(),
            unit: "kW".to_string(),
            description: "Main (compressor-only) power per OCHRE HVAC.py:575: total_input - fan".to_string(),
        },
        TelemetryField {
            name: tk::DUCT_LOSS_W.to_string(),
            unit: "W".to_string(),
            description: "Duct distribution losses per ASHRAE 152: gross_capacity * (1 - dse)".to_string(),
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
            name: tk::SPEED_FRAC.to_string(),
            unit: "-".to_string(),
            description: "Interpolation weight between speed stages [0..1]".to_string(),
        },
        TelemetryField {
            name: tk::HIGH_SIDE_CURVE_CLAMPED_SPEED_FRAC.to_string(),
            unit: "-".to_string(),
            description: "speed_frac recorded when high-side curve index clamped to last valid stage; 0 means no clamping this step"
                .to_string(),
        },
        TelemetryField {
            name: tk::PART_LOAD_RATIO.to_string(),
            unit: "-".to_string(),
            description: "Cycling fraction at lowest speed stage [0..1]".to_string(),
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
        TelemetryField {
            name: tk::COOLING_OAT_LOCKOUT.to_string(),
            unit: "-".to_string(),
            description: "1.0 when cooling compressor is locked out by minimum outdoor temperature; 0.0 otherwise".to_string(),
        },
        TelemetryField {
            name: tk::PUMP_POWER_KW.to_string(),
            unit: "kW".to_string(),
            description: "Ground-loop circulation pump electrical power (GSHP only; zero for air-source)".to_string(),
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
        if !cap_curves.is_empty() && !eir_curves.is_empty() && cap_curves.len() != eir_curves.len()
        {
            return Err(HaresError::Equipment(format!(
                "capacity_biquadratic_coeffs has {} speed(s) but eir_biquadratic_coeffs has {} speed(s); per-speed cap/EIR curve counts must match",
                cap_curves.len(),
                eir_curves.len()
            )));
        }
        if cap_curves.is_empty() {
            return Err(HaresError::Equipment(format!(
                "eir_biquadratic_coeffs has {} speed(s) but no capacity_biquadratic_coeffs provided; per-speed cap/EIR curves must both be present or both absent",
                eir_curves.len()
            )));
        }
        if eir_curves.is_empty() {
            return Err(HaresError::Equipment(format!(
                "capacity_biquadratic_coeffs has {} speed(s) but no eir_biquadratic_coeffs provided; per-speed cap/EIR curves must both be present or both absent",
                cap_curves.len()
            )));
        }
        let n_stages = cap_curves.len();
        let mut interleaved = Vec::with_capacity(n_stages * 2);
        for i in 0..n_stages {
            interleaved.push(cap_curves[i]);
            interleaved.push(eir_curves[i]);
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

// ---------------------------------------------------------------------------
// tests for load_curve_pair split-key validation
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    fn config_with(key: &str, value: &str) -> EquipmentConfig {
        let mut cfg = EquipmentConfig::default();
        cfg.test_extras_mut().insert(key.to_string(), value.into());
        cfg
    }

    /// Both split keys provided with matching counts: interleaves correctly.
    #[test]
    fn matching_split_keys_interleaved() {
        let mut cfg = EquipmentConfig::default();
        cfg.test_extras_mut().insert(
            "capacity_biquadratic_coeffs".to_string(),
            "[[2.0,0,0,0,0,0],[3.0,0,0,0,0,0]]".into(),
        );
        cfg.test_extras_mut().insert(
            "eir_biquadratic_coeffs".to_string(),
            "[[0.5,0,0,0,0,0],[0.6,0,0,0,0,0]]".into(),
        );
        let result = load_curve_pair(&cfg, false).unwrap();
        assert_eq!(result.len(), 4, "interleaved: 2 cap + 2 eir = 4 entries");
        assert_eq!(result[0][0], 2.0); // cap[0] a-coeff
        assert_eq!(result[1][0], 0.5); // eir[0] a-coeff
        assert_eq!(result[2][0], 3.0); // cap[1] a-coeff
        assert_eq!(result[3][0], 0.6); // eir[1] a-coeff
    }

    /// Both split keys non-empty but counts differ → rejected.
    #[test]
    fn mismatched_split_key_counts_error() {
        let mut cfg = EquipmentConfig::default();
        cfg.test_extras_mut().insert(
            "capacity_biquadratic_coeffs".to_string(),
            "[[2.0,0,0,0,0,0],[3.0,0,0,0,0,0]]".into(),
        );
        cfg.test_extras_mut().insert(
            "eir_biquadratic_coeffs".to_string(),
            "[[0.5,0,0,0,0,0]]".into(),
        );
        let result = load_curve_pair(&cfg, false);
        assert!(result.is_err(), "mismatched counts must be an error");
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("per-speed cap/EIR curve counts must match"),
            "got: {msg}"
        );
    }

    /// Capacity split key without EIR split key → rejected.
    #[test]
    fn cap_split_key_without_eir_error() {
        let cfg = config_with("capacity_biquadratic_coeffs", "[[2.0,0,0,0,0,0]]");
        let result = load_curve_pair(&cfg, false);
        assert!(result.is_err(), "cap without eir must be an error");
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("no eir_biquadratic_coeffs provided"),
            "got: {msg}"
        );
    }

    /// EIR split key without capacity split key → rejected.
    #[test]
    fn eir_split_key_without_cap_error() {
        let cfg = config_with("eir_biquadratic_coeffs", "[[0.5,0,0,0,0,0]]");
        let result = load_curve_pair(&cfg, false);
        assert!(result.is_err(), "eir without cap must be an error");
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("no capacity_biquadratic_coeffs provided"),
            "got: {msg}"
        );
    }

    /// No split keys, no biquadratic_coeffs → falls back to equipment-type defaults.
    #[test]
    fn empty_config_produces_defaults() {
        let cfg = EquipmentConfig::default();
        let result = load_curve_pair(&cfg, false).unwrap();
        assert_eq!(result.len(), 2, "one default cap + one default eir");
        assert_eq!(result[0], DEFAULT_AC_CAPACITY_CURVE);
        assert_eq!(result[1], DEFAULT_AC_EIR_CURVE);
    }
}
