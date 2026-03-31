//! HPWH compressor control logic, COP/capacity curve evaluation, and condenser heat distribution.

use hares_types::HaresError;
use serde::{Deserialize, Serialize};

use crate::EquipmentConfig;

/// Mutual exclusion mode between compressor and backup resistance elements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ElementHpControlMode {
    /// Compressor and backup elements cannot run simultaneously.
    /// When the compressor is on, the backup element is locked out, and vice versa.
    /// Compressor gets priority when neither is currently running.
    #[default]
    MutuallyExclusive,
    /// Both compressor and backup elements can operate at the same time.
    Simultaneous,
}

pub(super) const DEFAULT_DEADBAND_C: f64 = 8.166_666_667; // 14.7°F (OCHRE HPWH-specific default)
pub(super) const DEFAULT_COMPRESSOR_POWER_W: f64 = 1_200.0;
pub(super) const DEFAULT_BACKUP_ELEMENT_POWER_W: f64 = 4_500.0;
pub(super) const DEFAULT_BACKUP_ENABLE_OFFSET_C: f64 = 8.0;
/// Standard EnergyPlus/OCHRE HPWH COP biquadratic curve (GE GeoSpring class).
/// Inputs: wet-bulb temperature (°C), tank average temperature (°C).
/// Source: vendors/OCHRE/ochre/Equipment/WaterHeater.py lines 446-448.
pub(super) const DEFAULT_COP_CURVE: [f64; 6] =
    [1.0132, 0.0436, 0.0000117, -0.01113, 0.00003688, -0.000498];
/// Standard EnergyPlus/OCHRE HPWH capacity curve (GE GeoSpring / A.O. Smith class).
/// Inputs: wet-bulb temperature (°C), tank average temperature (°C).
pub(super) const DEFAULT_CAPACITY_CURVE: [f64; 6] =
    [0.563, 0.0437, 0.000039, 0.0055, -0.000148, -0.000145];
pub(super) const DEFAULT_ZONE_TEMP_BOUNDS_C: (f64, f64) = (5.0, 45.0);
pub(super) const DEFAULT_TANK_TEMP_BOUNDS_C: (f64, f64) = (20.0, 70.0);
/// OCHRE-compatible ambient temperature lockout range (°C) — standard HPWH.
/// 45°F = (45-32)×5/9 = 7.2̄°C; 110°F = (110-32)×5/9 = 43.3̄°C.
pub(super) const DEFAULT_MIN_AMBIENT_TEMP_C: f64 = 5.0 * (45.0 - 32.0) / 9.0; // 7.2222...
pub(super) const DEFAULT_MAX_AMBIENT_TEMP_C: f64 = 5.0 * (110.0 - 32.0) / 9.0; // 43.3333...
/// Low-power HPWH ambient lockout bounds (°C).
/// 37°F = 2.7̄°C; 145°F = 62.7̄°C.
pub(super) const LOW_POWER_MIN_AMBIENT_TEMP_C: f64 = 5.0 * (37.0 - 32.0) / 9.0; // 2.7777...
pub(super) const LOW_POWER_MAX_AMBIENT_TEMP_C: f64 = 5.0 * (145.0 - 32.0) / 9.0; // 62.7777...
/// Sensible heat ratio of zone-air cooling from evaporator (OCHRE WH.py:671-674).
pub(super) const DEFAULT_SHR: f64 = 0.88;
/// Fraction of compressor waste heat that exits the building envelope.
pub(super) const DEFAULT_LOST_HEAT_FRACTION: f64 = 0.0;
/// Evaporator fan power (W); added to electrical consumption when compressor runs.
pub(super) const DEFAULT_FAN_POWER_W: f64 = 35.0;
/// Standby parasitic power (W); drawn when compressor is off.
/// OCHRE WaterHeater.py:457: `HPWH Parasitics (W)` default 1 W.
#[allow(dead_code)] // Reserved for future parasitic standby loss implementation
pub(super) const DEFAULT_PARASITIC_POWER_W: f64 = 1.0;
/// Resistance backup element efficiency (fraction). Default 1.0 = 100% electric→heat.
pub(super) const DEFAULT_BACKUP_EFFICIENCY: f64 = 1.0;
/// Minimum compressor on-time (s) before an Off transition is allowed.
pub(super) const DEFAULT_MIN_ON_TIME_S: f64 = 600.0;
/// OCHRE condenser heat distribution weights for 12-node tanks.
/// Indices map to tank nodes 0–11 (top=0, bottom=11).
/// Values: [0, 0, 0, 0, 0, 5, 10, 15, 20, 25, 30, 5] / 110.
pub(super) const OCHRE_12NODE_CONDENSER_WEIGHTS: [f64; 12] = [
    0.0, 0.0, 0.0, 0.0, 0.0, 5.0, 10.0, 15.0, 20.0, 25.0, 30.0, 5.0,
];

/// Build a `(node, power_w)` injection list from condenser weights and total heat.
///
/// Weights need not be pre-normalized; they are normalized internally. If the
/// weight sum is zero the heat is concentrated at the bottom (last) node.
pub(super) fn build_heat_injections(
    weights: &[f64],
    q_w: f64,
    n_nodes: usize,
) -> Vec<(usize, f64)> {
    if q_w == 0.0 {
        return vec![];
    }
    let weight_sum: f64 = weights.iter().sum();
    if weight_sum <= 0.0 {
        return vec![(n_nodes.saturating_sub(1), q_w)];
    }
    weights
        .iter()
        .enumerate()
        .filter(|&(node, &w)| w > 0.0 && node < n_nodes)
        .map(|(node, &w)| (node, q_w * w / weight_sum))
        .collect()
}

/// Default condenser heat distribution weights for a tank with `n_nodes` nodes.
///
/// For 12-node tanks, returns the OCHRE-calibrated distribution (bottom-biased).
/// For other node counts, concentrates all heat at the bottom half of the tank
/// (nodes at or above index `n_nodes / 2`), matching OCHRE's general convention
/// that condenser heat enters the lower portion of the tank.
pub(super) fn default_condenser_weights(n_nodes: usize) -> Vec<f64> {
    if n_nodes == 12 {
        return OCHRE_12NODE_CONDENSER_WEIGHTS.to_vec();
    }
    // For generic node counts: place 100% weight on the condenser node (n_nodes / 2).
    let mut weights = vec![0.0_f64; n_nodes];
    if n_nodes > 0 {
        weights[n_nodes / 2] = 1.0;
    }
    weights
}

pub(super) fn parse_curve_coeffs(
    config: &EquipmentConfig,
    key: &str,
) -> crate::Result<Option<[f64; 6]>> {
    let Some(raw) = config.get_str(key) else {
        return Ok(None);
    };
    let parts: Vec<&str> = raw
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    if parts.len() != 6 {
        return Err(HaresError::Equipment(format!(
            "expected 6 coefficients in '{key}', got {}",
            parts.len()
        )));
    }

    let mut coeffs = [0.0_f64; 6];
    for (idx, part) in parts.into_iter().enumerate() {
        coeffs[idx] = part.parse::<f64>().map_err(|error| {
            HaresError::Equipment(format!(
                "failed to parse coefficient {idx} ('{part}') in '{key}': {error}"
            ))
        })?;
    }

    Ok(Some(coeffs))
}
