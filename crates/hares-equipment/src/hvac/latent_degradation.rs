//! Henderson-Rengarajan latent degradation model and coil Ao computation.
//!
//! Extracted from `air_conditioner.rs` — pure structural refactor, no logic changes.

use hares_physics::psychrometrics::humidity_ratio_from_twb;
use hares_types::HaresError;

use super::coil_physics::coil_ao_factor;

pub(super) const AHRI_RATED_INDOOR_DB_C: f64 = 26.666_666_666_7;
pub(super) const AHRI_RATED_INDOOR_WB_C: f64 = 19.444_444_444_4;
pub(super) const AHRI_RATED_OUTDOOR_DB_C: f64 = 35.0;
pub(super) const RATED_PRESSURE_KPA: f64 = 101.3;

/// Compute coil Ao factors per speed stage using the rated SHR at the AHRI
/// 80 degF/67 degF indoor / 95 degF outdoor test point.
pub(super) fn compute_coil_ao_by_stage(
    cooling_capacities_w: &[f64],
    airflow_m3_s_per_w: f64,
    rated_shr: f64,
) -> crate::Result<Vec<f64>> {
    let rated_w = humidity_ratio_from_twb(
        AHRI_RATED_INDOOR_DB_C,
        AHRI_RATED_INDOOR_WB_C,
        RATED_PRESSURE_KPA * 1000.0,
    );

    let mut ao = Vec::with_capacity(cooling_capacities_w.len().max(1));
    for (idx, cap_w) in cooling_capacities_w.iter().copied().enumerate() {
        let flow_m3_s = cap_w.max(0.0) * airflow_m3_s_per_w;
        let shr = rated_shr.clamp(0.0, 1.0);
        let ao_i = coil_ao_factor(
            AHRI_RATED_INDOOR_DB_C,
            rated_w,
            RATED_PRESSURE_KPA,
            (cap_w / 1000.0).max(0.0),
            flow_m3_s,
            shr,
        )
        .map_err(|err| {
            HaresError::Equipment(format!(
                "failed to compute coil Ao for stage {} at {} C ambient: {}",
                idx + 1,
                AHRI_RATED_OUTDOOR_DB_C,
                err
            ))
        })?;
        ao.push(ao_i);
    }
    if ao.is_empty() {
        ao.push(10.0);
    }
    Ok(ao)
}
