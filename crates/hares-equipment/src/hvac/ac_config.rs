//! Air conditioner, heat pump, and dehumidifier typed configuration structs.

use serde::{Deserialize, Serialize};

use hares_types::{HaresError, Telemetry, TelemetryField, parse_trimmed_f64, telemetry_keys as tk};

use super::core_config::parse_biquadratic_list;
use super::heating_config::DuctConfig;
use crate::EquipmentConfig;
use crate::config::EquipmentTypedConfig;

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

fn default_one() -> u8 {
    1
}

// =============================================================================
// Typed configuration structs for cooling equipment
// =============================================================================

/// Typed configuration for central air conditioners.
///
/// Resolves the SEER key mismatch (CW-005): the resolver writes `"efficiency_seer"`
/// or `"cooling_efficiency"` with `"cooling_efficiency_units" = "SEER"`, but the
/// raw config reads `"seer"` / `"SEER"`. This struct uses the canonical `seer` field.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CentralAirConditionerConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    /// Rated cooling capacity in watts.
    pub capacity_w: f64,
    /// Seasonal Energy Efficiency Ratio (BTU/Wh). EIR = 3.412 / SEER.
    pub seer: f64,
    /// Sensible heat ratio at rated conditions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shr: Option<f64>,
    /// Number of compressor speeds (1, 2, or 4 for variable-speed).
    #[serde(default = "default_one")]
    pub number_of_speeds: u8,
    /// Per-stage cooling capacities [W].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_capacities_w: Option<Vec<f64>>,
    /// Per-stage energy input ratios (EIR = 1/COP).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_eirs: Option<Vec<f64>>,
    /// Per-stage sensible heat ratios — was discarded before (CW-008).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_shrs: Option<Vec<f64>>,
    /// Fan power in watts (constant).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w: Option<f64>,
    /// Fan power per CFM of airflow (W/CFM).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w_per_cfm: Option<f64>,
    /// Fraction of zone load served by this equipment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction_load_served: Option<f64>,
    /// Duct configuration (distribution system efficiency).
    #[serde(flatten)]
    pub duct: DuctConfig,
    /// System type string (e.g., "split", "packaged").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_type: Option<String>,
    /// Startup capacity degradation coefficient (Cd).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_cd: Option<f64>,
}

impl EquipmentTypedConfig for CentralAirConditionerConfig {
    fn equipment_type_name() -> &'static str {
        "Central AC"
    }
}

/// Typed configuration for room air conditioners (window/through-wall units).
///
/// Uses EER instead of SEER for efficiency rating (CW-006).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomAcConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    /// Rated cooling capacity in watts.
    pub capacity_w: f64,
    /// Energy Efficiency Ratio (BTU/Wh). EIR = 3.412 / EER.
    pub eer: f64,
}

impl EquipmentTypedConfig for RoomAcConfig {
    fn equipment_type_name() -> &'static str {
        "Room AC"
    }
}

/// Typed configuration for heat pumps (both heating and cooling).
///
/// Combines heating and cooling parameters into a single struct with optional
/// fields for each side. Resolves:
/// - CW-005: SEER key mismatch for cooling
/// - CW-008: Per-stage SHR not propagated
/// - CW-009: Mini-split speed count forced to 4
/// - CW-010: Combined HP cooling configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeatPumpConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    // --- Heating parameters ---
    /// Rated heating capacity in watts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heating_capacity_w: Option<f64>,
    /// Heating Seasonal Performance Factor (BTU/Wh).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hspf: Option<f64>,
    /// Per-stage heating capacities [W].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_heating_capacities_w: Option<Vec<f64>>,
    /// Per-stage heating EIRs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_heating_eirs: Option<Vec<f64>>,
    /// Backup heating fuel type (e.g., "electric", "gas").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_fuel: Option<String>,
    /// Backup heating capacity in watts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_capacity_w: Option<f64>,
    /// Backup heating EIR.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_eir: Option<f64>,
    /// Fraction of heating load served.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction_heating_load_served: Option<f64>,
    // --- Cooling parameters ---
    /// Rated cooling capacity in watts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooling_capacity_w: Option<f64>,
    /// Seasonal Energy Efficiency Ratio (BTU/Wh).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seer: Option<f64>,
    /// Per-stage cooling capacities [W].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_cooling_capacities_w: Option<Vec<f64>>,
    /// Per-stage cooling EIRs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_cooling_eirs: Option<Vec<f64>>,
    /// Per-stage sensible heat ratios — was discarded before (CW-008).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_shrs: Option<Vec<f64>>,
    /// Fraction of cooling load served.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction_cooling_load_served: Option<f64>,
    // --- Common parameters ---
    /// Number of compressor speeds.
    /// For mini-splits, this is forced to 4 regardless of config (CW-009).
    #[serde(default = "default_one")]
    pub number_of_speeds: u8,
    /// Whether this is a mini-split heat pump.
    /// When true, `number_of_speeds` is forced to 4.
    #[serde(default)]
    pub is_mini_split: bool,
    /// Sensible heat ratio at rated conditions (for single-speed or default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shr: Option<f64>,
    /// Fan power in watts (constant).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w: Option<f64>,
    /// Fan power per CFM of airflow (W/CFM).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w_per_cfm: Option<f64>,
    /// Duct configuration.
    #[serde(flatten)]
    pub duct: DuctConfig,
}

impl EquipmentTypedConfig for HeatPumpConfig {
    fn equipment_type_name() -> &'static str {
        "Heat Pump"
    }
}

/// Typed configuration for dehumidifiers (CW-019).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DehumidifierConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    /// Rated water removal capacity in pints per day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_pints_per_day: Option<f64>,
    /// Energy factor in L/kWh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub energy_factor: Option<f64>,
    /// Integrated energy factor in L/kWh (newer rating).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrated_energy_factor: Option<f64>,
    /// Fraction of dehumidification load served.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction_served: Option<f64>,
    /// Target relative humidity setpoint (fraction 0-1 or percent 0-100).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_rh: Option<f64>,
}

impl EquipmentTypedConfig for DehumidifierConfig {
    fn equipment_type_name() -> &'static str {
        "Dehumidifier"
    }
}

#[cfg(test)]
mod typed_config_tests {
    use super::*;
    use crate::config::{ConfigPayload, EquipmentConfig};

    fn typed_config<T: EquipmentTypedConfig>(config: T) -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "test".to_string(),
            T::equipment_type_name().to_string(),
            config,
        )
        .unwrap()
    }

    #[test]
    fn central_ac_config_round_trips() {
        let cfg = CentralAirConditionerConfig {
            equipment_id: Some(1),
            zone_id: Some(1),
            capacity_w: 12_000.0,
            seer: 16.0,
            shr: Some(0.75),
            number_of_speeds: 2,
            stage_capacities_w: Some(vec![6_000.0, 12_000.0]),
            stage_eirs: Some(vec![0.25, 0.22]),
            stage_shrs: Some(vec![0.78, 0.72]),
            fan_power_w: Some(300.0),
            fan_power_w_per_cfm: None,
            fraction_load_served: Some(1.0),
            duct: DuctConfig {
                dse_heat: Some(0.8),
                dse_cool: Some(0.85),
            },
            system_type: Some("split".to_string()),
            startup_cd: None,
        };
        let ec = typed_config(cfg.clone());
        assert!(ec.is_typed());
        let recovered: CentralAirConditionerConfig = ec.typed().unwrap();
        assert!((recovered.seer - cfg.seer).abs() < 1e-12);
        assert!((recovered.capacity_w - cfg.capacity_w).abs() < 1e-12);
        assert_eq!(recovered.stage_shrs, cfg.stage_shrs);
    }

    #[test]
    fn room_ac_config_round_trips() {
        let cfg = RoomAcConfig {
            equipment_id: Some(2),
            zone_id: Some(1),
            capacity_w: 3_500.0,
            eer: 10.0,
        };
        let ec = typed_config(cfg.clone());
        let recovered: RoomAcConfig = ec.typed().unwrap();
        assert!((recovered.eer - cfg.eer).abs() < 1e-12);
    }

    #[test]
    fn heat_pump_config_round_trips() {
        let cfg = HeatPumpConfig {
            equipment_id: Some(1),
            zone_id: Some(1),
            heating_capacity_w: Some(10_000.0),
            hspf: Some(9.0),
            stage_heating_capacities_w: Some(vec![5_000.0, 10_000.0]),
            stage_heating_eirs: Some(vec![0.28, 0.25]),
            backup_fuel: Some("electric".to_string()),
            backup_capacity_w: Some(5_000.0),
            backup_eir: Some(1.0),
            fraction_heating_load_served: Some(1.0),
            cooling_capacity_w: Some(12_000.0),
            seer: Some(16.0),
            stage_cooling_capacities_w: Some(vec![6_000.0, 12_000.0]),
            stage_cooling_eirs: Some(vec![0.25, 0.22]),
            stage_shrs: Some(vec![0.78, 0.72]),
            fraction_cooling_load_served: Some(1.0),
            number_of_speeds: 1,
            is_mini_split: true,
            shr: Some(0.75),
            fan_power_w: Some(300.0),
            fan_power_w_per_cfm: None,
            duct: DuctConfig::default(),
        };
        let ec = typed_config(cfg.clone());
        let recovered: HeatPumpConfig = ec.typed().unwrap();
        assert!((recovered.seer.unwrap() - cfg.seer.unwrap()).abs() < 1e-12);
        assert!(recovered.is_mini_split);
        assert_eq!(recovered.stage_shrs, cfg.stage_shrs);
    }

    #[test]
    fn dehumidifier_config_round_trips() {
        let cfg = DehumidifierConfig {
            equipment_id: Some(1),
            zone_id: Some(1),
            capacity_pints_per_day: Some(70.0),
            energy_factor: Some(2.0),
            integrated_energy_factor: None,
            fraction_served: Some(1.0),
            target_rh: Some(0.50),
        };
        let ec = typed_config(cfg.clone());
        let recovered: DehumidifierConfig = ec.typed().unwrap();
        assert!(
            (recovered.capacity_pints_per_day.unwrap() - cfg.capacity_pints_per_day.unwrap()).abs()
                < 1e-12
        );
    }

    #[test]
    fn central_ac_rejects_unknown_fields() {
        let data = serde_json::json!({
            "capacity_w": 12000.0,
            "seer": 16.0,
            "unknown_key": true,
        });
        let ec = EquipmentConfig {
            name: "test".to_string(),
            ochre_class: "Central AC".to_string(),
            payload: ConfigPayload::Typed {
                type_name: "Central AC".to_string(),
                version: 1,
                data,
            },
        };
        let result: crate::Result<CentralAirConditionerConfig> = ec.typed();
        assert!(result.is_err());
    }

    #[test]
    fn heat_pump_number_of_speeds_defaults_to_one() {
        let json = serde_json::json!({
            "cooling_capacity_w": 12000.0,
            "seer": 16.0,
        });
        let cfg: HeatPumpConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.number_of_speeds, 1);
    }

    #[test]
    fn seer_16_produces_correct_eir() {
        const BTU_PER_HR_PER_W: f64 = 3.412_141_633;
        let seer = 16.0;
        let eir = BTU_PER_HR_PER_W / seer;
        let expected_eir = 1.0 / (seer / BTU_PER_HR_PER_W);
        assert!((eir - expected_eir).abs() < 1e-12);
        assert!((eir - 0.21325).abs() < 1e-4);
    }
}
