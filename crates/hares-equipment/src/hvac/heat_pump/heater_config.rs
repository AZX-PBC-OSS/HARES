//! Configuration parsing and initialization helpers for heat-pump heaters.

use hares_physics::constants::BTU_PER_HR_PER_W;
use hares_types::{HaresError, Telemetry, TelemetryField};

use crate::EquipmentConfig;

use super::super::helpers::first_f64;
use super::constants::{
    DEFAULT_AC_SPEED_MAP_ERROR, DEFAULT_HEATING_CAPACITY_W, DEFAULT_MSHP_SPEED_MAP,
    HEATER_TELEMETRY_CAPACITY, MAX_MSHP_SPEED_INDEX,
};

const OPERATING_MODE_CODE_OFF: f64 = 0.0;
const OPERATING_MODE_CODE_HP_ON: f64 = 3.0;
const OPERATING_MODE_CODE_HP_ER_ON: f64 = 4.0;
const OPERATING_MODE_CODE_ER_ON: f64 = 5.0;

pub(super) fn default_heater_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(HEATER_TELEMETRY_CAPACITY);
    telemetry.insert("electric_kw", 0.0);
    telemetry.insert("thermal_output_w", 0.0);
    telemetry.insert("operating_mode", OPERATING_MODE_CODE_OFF);
    telemetry.insert("speed_index", 0.0);
    telemetry.insert("defrost_active", 0.0);
    telemetry.insert("cop", 0.0);
    telemetry.insert("runtime_fraction", 0.0);
    telemetry.insert("compressor_kw", 0.0);
    telemetry.insert("defrost_time_fraction", 0.0);
    telemetry
}

pub(super) fn heater_telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: "electric_kw".to_string(),
            unit: "kW".to_string(),
            description: "Total heater electric power".to_string(),
        },
        TelemetryField {
            name: "thermal_output_w".to_string(),
            unit: "W".to_string(),
            description: "Delivered sensible heating".to_string(),
        },
        TelemetryField {
            name: "operating_mode".to_string(),
            unit: "enum".to_string(),
            description: "Operating mode code".to_string(),
        },
        TelemetryField {
            name: "speed_index".to_string(),
            unit: "index".to_string(),
            description: "Selected compressor speed stage".to_string(),
        },
        TelemetryField {
            name: "defrost_active".to_string(),
            unit: "bool".to_string(),
            description: "1 when defrost correction is active".to_string(),
        },
        TelemetryField {
            name: "cop".to_string(),
            unit: "-".to_string(),
            description:
                "COP per AHRI convention: gross thermal output / compressor-only electric input"
                    .to_string(),
        },
        TelemetryField {
            name: "runtime_fraction".to_string(),
            unit: "-".to_string(),
            description: "Compressor runtime fraction (part-load ratio) this timestep".to_string(),
        },
        TelemetryField {
            name: "compressor_kw".to_string(),
            unit: "kW".to_string(),
            description: "Compressor-only electric power".to_string(),
        },
        TelemetryField {
            name: "defrost_time_fraction".to_string(),
            unit: "-".to_string(),
            description: "Fraction of timestep in defrost mode [0..1]".to_string(),
        },
    ]
}

pub(super) fn compute_eir_from_efficiency(config: &EquipmentConfig, default_eir: f64) -> f64 {
    if let Some(eir) = first_f64(
        config,
        &["eir", "heating_eir", "EIR", "HVAC Heating EIR (-)"],
    ) {
        return eir;
    }

    let Some(efficiency) = config.get_f64("heating_efficiency") else {
        return default_eir;
    };

    let units = config
        .get_str("heating_efficiency_units")
        .map(|s| s.to_ascii_uppercase())
        .unwrap_or_default();

    let cop = if matches!(
        units.as_str(),
        "HSPF" | "HSPF2" | "EER" | "EER2" | "SEER" | "SEER2"
    ) {
        efficiency / BTU_PER_HR_PER_W
    } else {
        efficiency
    };

    let eir = if cop > 1e-6 { 1.0 / cop } else { default_eir };
    let clamped_eir = eir.clamp(0.0, 1.0);
    if (eir - clamped_eir).abs() > 1e-9 {
        tracing::warn!(
            "Heating EIR {} clamped to {} (implies COP < 1, indicating malformed input)",
            eir,
            clamped_eir
        );
    }
    clamped_eir
}

pub(super) fn reconcile_stage_lengths(
    capacities: &mut Vec<f64>,
    eirs: &mut Vec<f64>,
    default_eir: f64,
) -> crate::Result<()> {
    if capacities.is_empty() {
        capacities.push(DEFAULT_HEATING_CAPACITY_W);
    }
    if eirs.is_empty() {
        eirs.push(default_eir);
    }

    if capacities.len() == eirs.len() {
        return Ok(());
    }

    if eirs.len() == 1 {
        *eirs = vec![eirs[0]; capacities.len()];
        return Ok(());
    }

    Err(HaresError::Equipment(
        "heating capacity and EIR stage counts must match".to_string(),
    ))
}

pub(super) fn remap_minisplit_stages(values: Vec<f64>, map: [u8; 4]) -> crate::Result<Vec<f64>> {
    if values.is_empty() {
        return Ok(values);
    }
    if values.len() < (MAX_MSHP_SPEED_INDEX as usize + 1) {
        return Ok(values);
    }

    let mut mapped = Vec::with_capacity(map.len());
    for &idx in &map {
        if idx > MAX_MSHP_SPEED_INDEX {
            return Err(HaresError::Equipment(
                DEFAULT_AC_SPEED_MAP_ERROR.to_string(),
            ));
        }
        mapped.push(values[idx as usize]);
    }
    Ok(mapped)
}

pub(super) fn parse_mshp_speed_map(config: &EquipmentConfig) -> crate::Result<[u8; 4]> {
    let Some(raw) = config.get_str("mshp_speed_map") else {
        return Ok(DEFAULT_MSHP_SPEED_MAP);
    };

    let parts: Vec<&str> = raw
        .split(|c: char| c == ',' || c == '[' || c == ']' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() != 4 {
        return Err(HaresError::Equipment(
            DEFAULT_AC_SPEED_MAP_ERROR.to_string(),
        ));
    }

    let mut values = [0u8; 4];
    for (i, part) in parts.into_iter().enumerate() {
        let parsed = part
            .parse::<u8>()
            .map_err(|_| HaresError::Equipment(DEFAULT_AC_SPEED_MAP_ERROR.to_string()))?;
        if parsed > MAX_MSHP_SPEED_INDEX {
            return Err(HaresError::Equipment(
                DEFAULT_AC_SPEED_MAP_ERROR.to_string(),
            ));
        }
        values[i] = parsed;
    }

    if values.windows(2).any(|w| w[0] >= w[1]) {
        return Err(HaresError::Equipment(
            "mshp_speed_map must be strictly increasing".to_string(),
        ));
    }

    Ok(values)
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
