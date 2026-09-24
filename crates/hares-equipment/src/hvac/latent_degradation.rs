//! Henderson-Rengarajan latent degradation model and coil Ao computation.
//!
//! Extracted from `air_conditioner.rs` -- pure structural refactor, no logic changes.

use hares_physics::psychrometrics::humidity_ratio_from_twb;
use hares_types::HaresError;

use super::coil_physics::coil_ao_factor;

pub(super) const AHRI_RATED_INDOOR_DB_C: f64 = 26.666_666_666_7;
pub(super) const AHRI_RATED_INDOOR_WB_C: f64 = 19.444_444_444_4;
pub(super) const AHRI_RATED_OUTDOOR_DB_C: f64 = 35.0;
pub(super) const RATED_PRESSURE_KPA: f64 = 101.3;

/// Compute coil Ao factors per speed stage at the AHRI 80 degF/67 degF
/// indoor / 95 degF outdoor test point.
///
/// When `stage_shrs` is non-empty, each stage uses its own SHR for Ao
/// computation, matching OCHRE `HVAC.py:207-211` where per-speed SHR and
/// capacity are zipped together.  When `stage_shrs` is empty (single-stage
/// or legacy equipment), `rated_shr` is used for every stage.
pub(super) fn compute_coil_ao_by_stage(
    cooling_capacities_w: &[f64],
    airflow_m3_s_per_w: f64,
    rated_shr: f64,
    stage_shrs: &[f64],
) -> crate::Result<Vec<f64>> {
    let rated_w = humidity_ratio_from_twb(
        AHRI_RATED_INDOOR_DB_C,
        AHRI_RATED_INDOOR_WB_C,
        RATED_PRESSURE_KPA * 1000.0,
    );

    let mut ao = Vec::with_capacity(cooling_capacities_w.len().max(1));
    for (idx, cap_w) in cooling_capacities_w.iter().copied().enumerate() {
        let flow_m3_s = cap_w.max(0.0) * airflow_m3_s_per_w;
        let shr = stage_shrs
            .get(idx)
            .copied()
            .unwrap_or(rated_shr)
            .clamp(0.0, 1.0);
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

    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    check_coil_ao_invariant(cooling_capacities_w.len().max(1), stage_shrs, &ao)?;

    Ok(ao)
}

/// Verify the per-stage coil Ao invariant: when the supplied per-stage SHR
/// values differ, the computed Ao factors must differ too.  Identical Ao
/// across stages while SHR differs means the per-stage SHR was silently
/// ignored -- the exact defect this fix guards against reintroducing.
///
/// Returns `Err(HaresError::InvariantViolation)` rather than using
/// `debug_assert!`.  `debug_assert!` is gated on Rust's own `debug_assertions`
/// cfg, which is *off* in release builds, so it compiles to nothing under CI's
/// `cargo nextest run --workspace --release -F check_invariants` job -- the one
/// build specifically configured to run this check
/// (docs/invariants-and-observability.md: `--release -F check_invariants` =>
/// "Checks active? Yes").  A returned typed error keeps the gate live in that
/// build, matching the established pattern in `hvac/dehumidifier.rs:411-428`
/// and `pv/mod.rs:979-991`.
#[cfg(any(debug_assertions, feature = "check_invariants"))]
fn check_coil_ao_invariant(
    expected_stage_count: usize,
    stage_shrs: &[f64],
    ao: &[f64],
) -> crate::Result<()> {
    const SPREAD_EPS: f64 = 1e-9;

    if ao.len() != expected_stage_count {
        return Err(HaresError::InvariantViolation {
            check_name: "compute_coil_ao_by_stage_count".to_string(),
            value: ao.len() as f64,
            tolerance: expected_stage_count as f64,
        });
    }

    if stage_shrs.len() > 1 {
        let shr_spread = stage_shrs
            .windows(2)
            .map(|w| (w[0] - w[1]).abs())
            .fold(0.0_f64, f64::max);
        if shr_spread > SPREAD_EPS {
            let ao_spread = ao
                .windows(2)
                .map(|w| (w[0] - w[1]).abs())
                .fold(0.0_f64, f64::max);
            if ao_spread <= SPREAD_EPS {
                return Err(HaresError::InvariantViolation {
                    check_name: "per_stage_shr_differs_but_coil_ao_identical".to_string(),
                    value: ao_spread,
                    tolerance: SPREAD_EPS,
                });
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_stage_ao_differs_with_per_stage_shr() {
        let capacities = vec![8_000.0, 12_000.0];
        let airflow = 0.000_054;
        let rated_shr = 0.75;
        let stage_shrs = vec![0.80, 0.70];

        let ao = compute_coil_ao_by_stage(&capacities, airflow, rated_shr, &stage_shrs)
            .expect("Ao computation should succeed");

        assert_eq!(ao.len(), 2);
        assert!(
            (ao[0] - ao[1]).abs() > 1e-6,
            "Ao should differ per stage when per-stage SHR differs, got {ao:?}"
        );
    }

    #[test]
    fn empty_stage_shrs_falls_back_to_rated_shr() {
        let capacities = vec![8_000.0, 8_000.0];
        let airflow = 0.000_054;
        let rated_shr = 0.75;
        let stage_shrs: Vec<f64> = vec![];

        let ao = compute_coil_ao_by_stage(&capacities, airflow, rated_shr, &stage_shrs)
            .expect("Ao computation should succeed");

        assert_eq!(ao.len(), 2);
        assert!(
            (ao[0] - ao[1]).abs() < 1e-9,
            "Ao should be identical for same capacity and same SHR, got {ao:?}"
        );
    }

    #[test]
    fn single_stage_with_stage_shrs_uses_per_stage_value() {
        let capacities = vec![8_000.0];
        let airflow = 0.000_054;
        let rated_shr = 0.75;
        let stage_shrs = vec![0.82];

        let ao_per_stage = compute_coil_ao_by_stage(&capacities, airflow, rated_shr, &stage_shrs)
            .expect("Ao computation should succeed");

        let ao_fallback = compute_coil_ao_by_stage(&capacities, airflow, rated_shr, &[])
            .expect("Ao computation should succeed");

        assert_eq!(ao_per_stage.len(), 1);
        assert_eq!(ao_fallback.len(), 1);
        assert!(
            (ao_per_stage[0] - ao_fallback[0]).abs() > 1e-6,
            "Ao should differ between per-stage SHR and rated SHR fallback"
        );
    }

    // The invariant must fire on the original bug's signature -- per-stage SHR
    // values differ but Ao is identical across stages -- and it must do so via
    // a returned typed error, not a `debug_assert!`, so it survives the
    // release-mode `check_invariants` CI build.  This test compiles under both
    // the default (debug) test run and `--release -F check_invariants`.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    #[test]
    fn coil_ao_invariant_fires_when_shr_differs_but_ao_identical() {
        use hares_types::HaresError;

        let stage_shrs = [0.80, 0.70];
        let identical_ao = [1.5, 1.5];

        let err = check_coil_ao_invariant(stage_shrs.len(), &stage_shrs, &identical_ao)
            .expect_err("differing SHR with identical Ao must violate the invariant");

        match err {
            HaresError::InvariantViolation { check_name, .. } => {
                assert_eq!(check_name, "per_stage_shr_differs_but_coil_ao_identical");
            }
            other => panic!("expected InvariantViolation, got {other:?}"),
        }
    }

    // Consistent per-stage Ao (differing SHR that legitimately maps to
    // differing Ao) must pass the invariant.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    #[test]
    fn coil_ao_invariant_passes_when_ao_tracks_shr() {
        let stage_shrs = [0.80, 0.70];
        let differing_ao = [1.2, 1.5];

        check_coil_ao_invariant(stage_shrs.len(), &stage_shrs, &differing_ao)
            .expect("differing SHR with differing Ao must satisfy the invariant");
    }
}
