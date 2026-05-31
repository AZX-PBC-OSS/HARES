//! Per-timestep numerical invariant checks for the dwelling simulation loop.
//!
//! Checks are compiled and executed when either:
//! - The `check_invariants` Cargo feature is enabled, or
//! - The build has `debug_assertions` enabled (i.e., `cargo build` / `cargo test`
//!   without `--release`).
//!
//! In production release builds without the feature flag all public functions
//! compile to nothing -- the compiler eliminates the bodies entirely.

use hares_types::{ControlCapabilities, HaresError};

/// Entrypoint for per-timestep numerical invariant validation.
///
/// Construct once per dwelling; call the `check_*` methods each timestep
/// inside a `cfg(any(debug_assertions, feature = "check_invariants"))` block.
/// All methods return `Ok(())` in unchecked builds.
pub struct InvariantChecker;

impl InvariantChecker {
    pub fn new() -> Self {
        Self
    }
}

impl Default for InvariantChecker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(debug_assertions, feature = "check_invariants"))]
impl InvariantChecker {
    /// Verifies energy conservation across the thermal domain for one zone.
    ///
    /// The check asserts:
    /// `|Σ(Q_gain) − ΔE_storage − Q_loss_envelope| < max(1.0, 1e-6 · |Σ Q_gain|)`
    ///
    /// All values in watts [W].
    pub fn check_thermal(
        &self,
        q_gains: &[f64],
        delta_e_storage: f64,
        q_loss: f64,
    ) -> Result<(), HaresError> {
        let q_sum: f64 = q_gains.iter().sum();
        let residual = (q_sum - delta_e_storage - q_loss).abs();
        // Use gross flux (sum of absolute values) for relative tolerance,
        // not net sum -- a balanced system with large opposed fluxes still has
        // floating-point accumulation error proportional to gross magnitude.
        let gross_flux: f64 = q_gains.iter().map(|q| q.abs()).sum();
        let tolerance = f64::max(1.0, 1e-6 * gross_flux);
        if residual >= tolerance {
            return Err(HaresError::InvariantViolation {
                check_name: "thermal_balance".to_string(),
                value: residual,
                tolerance,
            });
        }
        Ok(())
    }

    /// Verifies electrical power balance across the bus.
    ///
    /// The check asserts:
    /// `|P_grid + Σ P_equipment_ports| < 0.001` [kW]
    pub fn check_electrical(
        &self,
        p_grid: f64,
        p_equipment_ports: &[f64],
    ) -> Result<(), HaresError> {
        let p_sum: f64 = p_equipment_ports.iter().sum();
        let residual = (p_grid + p_sum).abs();
        const TOLERANCE: f64 = 0.001;
        if residual >= TOLERANCE {
            return Err(HaresError::InvariantViolation {
                check_name: "electrical_balance".to_string(),
                value: residual,
                tolerance: TOLERANCE,
            });
        }
        Ok(())
    }

    /// Verifies moisture mass conservation.
    ///
    /// Each term is `(Q_latent_i [W], dt_s [s])`.
    /// `h_fg = 2_501_000 J/kg` (latent heat of vaporisation at 0°C).
    ///
    /// The check asserts:
    /// `|Δm_water − Σ(Q_latent_i · dt / h_fg)| < 1e-6` [kg]
    pub fn check_moisture(
        &self,
        delta_m_water: f64,
        q_latent_terms: &[(f64, f64)],
    ) -> Result<(), HaresError> {
        const H_FG_J_KG: f64 = 2_501_000.0;
        const TOLERANCE: f64 = 1e-6;
        let m_from_latent: f64 = q_latent_terms
            .iter()
            .map(|&(q_w, dt_s)| q_w * dt_s / H_FG_J_KG)
            .sum();
        let residual = (delta_m_water - m_from_latent).abs();
        if residual >= TOLERANCE {
            return Err(HaresError::InvariantViolation {
                check_name: "moisture_balance".to_string(),
                value: residual,
                tolerance: TOLERANCE,
            });
        }
        Ok(())
    }

    /// Validates state-of-charge is within `[0.0, 1.0]` and warns if accumulated
    /// integration error exceeds threshold.
    ///
    /// Returns `Ok(())` always -- SoC out-of-bounds is clamped (not a fatal error).
    /// Emits `tracing::warn!` when `accumulated_error.abs() > 0.001`.
    pub fn check_soc(&self, soc: f64, accumulated_error: f64) -> Result<(), HaresError> {
        let clamped = soc.clamp(0.0, 1.0);
        if (clamped - soc).abs() > f64::EPSILON {
            tracing::warn!(
                soc = soc,
                clamped = clamped,
                "SoC out of [0, 1] range; clamped"
            );
        }
        if accumulated_error.abs() > 0.001 {
            tracing::warn!(
                accumulated_error = accumulated_error,
                "SoC accumulated integration error exceeds 0.001"
            );
        }
        Ok(())
    }

    /// Validates that zone and tank temperatures are within physically plausible bounds.
    ///
    /// - Conditioned zone temperatures: `[-50, 80]` °C
    /// - Unconditioned zone temperatures: `[-50, 120]` °C (attics under solar load
    ///   routinely exceed 80 °C in hot climates)
    /// - Tank temperatures: `[0, 100]` °C
    ///
    /// Returns the first violation found, or `Ok(())`.
    pub fn check_temperatures(
        &self,
        conditioned_zone_temps_c: &[f64],
        unconditioned_zone_temps_c: &[f64],
        tank_temps_c: &[f64],
    ) -> Result<(), HaresError> {
        const ZONE_MIN_C: f64 = -50.0;
        const CONDITIONED_MAX_C: f64 = 80.0;
        const UNCONDITIONED_MAX_C: f64 = 120.0;
        const TANK_MIN_C: f64 = 0.0;
        const TANK_MAX_C: f64 = 100.0;

        for &t in conditioned_zone_temps_c {
            if !t.is_finite() || !(ZONE_MIN_C..=CONDITIONED_MAX_C).contains(&t) {
                return Err(HaresError::InvariantViolation {
                    check_name: "zone_temperature_bounds".to_string(),
                    value: t,
                    tolerance: 0.0,
                });
            }
        }
        for &t in unconditioned_zone_temps_c {
            if !t.is_finite() || !(ZONE_MIN_C..=UNCONDITIONED_MAX_C).contains(&t) {
                return Err(HaresError::InvariantViolation {
                    check_name: "unconditioned_zone_temperature_bounds".to_string(),
                    value: t,
                    tolerance: 0.0,
                });
            }
        }
        for &t in tank_temps_c {
            if !t.is_finite() || !(TANK_MIN_C..=TANK_MAX_C).contains(&t) {
                return Err(HaresError::InvariantViolation {
                    check_name: "tank_temperature_bounds".to_string(),
                    value: t,
                    tolerance: 0.0,
                });
            }
        }
        Ok(())
    }

    /// Verifies that at least one equipment in the dwelling declares the
    /// `PROTOCOL_NATIVE` capability, so a `ProtocolNative` dispatch has a
    /// consumer. Without a registered consumer the dispatch is silently
    /// rejected by the capability gate and the signal never reaches any
    /// equipment.
    ///
    /// Called before dispatching `ProtocolNative` signals in the dwelling's
    /// Step 2b derived-signal cascade. An empty `capability_sets` — a dwelling
    /// with no equipment at all — always fails this check.
    pub fn check_protocol_native_registration(
        &self,
        capability_sets: &[ControlCapabilities],
    ) -> Result<(), HaresError> {
        let has_consumer = capability_sets
            .iter()
            .any(|caps| caps.contains(ControlCapabilities::PROTOCOL_NATIVE));
        if !has_consumer {
            return Err(HaresError::InvariantViolation {
                check_name: "protocol_native_registration".to_string(),
                value: capability_sets.len() as f64,
                tolerance: 0.0,
            });
        }
        Ok(())
    }

    /// Verifies that the fuel accumulator contains no electric contributions.
    ///
    /// Electric power must route through `ElectricalAccumulator` — never through
    /// the fuel accumulator. A non-zero electric slot in the fuel accumulator
    /// indicates equipment is mistakenly treating electricity as a fuel.
    pub fn check_fuel_electric_absent(&self, electric_fuel_w: f64) -> Result<(), HaresError> {
        if electric_fuel_w > 0.0 {
            return Err(HaresError::InvariantViolation {
                check_name: "electric_fuel_in_fuel_accumulator".to_string(),
                value: electric_fuel_w,
                tolerance: 0.0,
            });
        }
        Ok(())
    }
}

#[cfg(not(any(debug_assertions, feature = "check_invariants")))]
impl InvariantChecker {
    pub fn check_thermal(&self, _: &[f64], _: f64, _: f64) -> Result<(), HaresError> {
        Ok(())
    }

    pub fn check_electrical(&self, _: f64, _: &[f64]) -> Result<(), HaresError> {
        Ok(())
    }

    pub fn check_moisture(&self, _: f64, _: &[(f64, f64)]) -> Result<(), HaresError> {
        Ok(())
    }

    pub fn check_soc(&self, _: f64, _: f64) -> Result<(), HaresError> {
        Ok(())
    }

    pub fn check_temperatures(&self, _: &[f64], _: &[f64], _: &[f64]) -> Result<(), HaresError> {
        Ok(())
    }

    pub fn check_protocol_native_registration(
        &self,
        _: &[ControlCapabilities],
    ) -> Result<(), HaresError> {
        Ok(())
    }

    pub fn check_fuel_electric_absent(&self, _: f64) -> Result<(), HaresError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker() -> InvariantChecker {
        InvariantChecker::new()
    }

    // ── thermal_balance ───────────────────────────────────────────────────────

    #[test]
    fn thermal_balance_passes_when_balanced() {
        // 1000 W gain, 200 W stored, 800 W lost → residual = 0
        let result = checker().check_thermal(&[1000.0], 200.0, 800.0);
        assert!(result.is_ok());
    }

    #[test]
    fn thermal_balance_fails_on_sign_flipped_gain() {
        // Deliberately broken: gain reported as -1000 W instead of +1000 W.
        // Σ = -1000, storage = 200, loss = 800 → residual = |-1000 - 200 - 800| = 2000.
        // tolerance = max(1.0, 1e-6 * 1000) = 1.0 → should fail.
        let result = checker().check_thermal(&[-1000.0], 200.0, 800.0);
        assert!(
            result.is_err(),
            "sign-flipped gain must trigger thermal invariant"
        );
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::InvariantViolation { check_name, .. } if check_name == "thermal_balance"
        ));
    }

    #[test]
    fn thermal_balance_uses_relative_tolerance_for_large_gains() {
        // With Q_sum = 1e8 W, tolerance = 1e-6 * 1e8 = 100 W.
        // Residual = 50 W → passes.
        let result = checker().check_thermal(&[1e8], 0.0, 1e8 - 50.0);
        assert!(result.is_ok());

        // Residual = 200 W → fails (> 100 W tolerance).
        let result = checker().check_thermal(&[1e8], 0.0, 1e8 - 200.0);
        assert!(result.is_err());
    }

    // ── electrical_balance ────────────────────────────────────────────────────

    #[test]
    fn electrical_balance_passes_when_balanced() {
        // Grid supplies 3 kW, equipment draws 3 kW → net = 0.
        let result = checker().check_electrical(3.0, &[-3.0]);
        assert!(result.is_ok());
    }

    #[test]
    fn electrical_balance_fails_when_imbalanced() {
        // Grid 3 kW, equipment 2 kW → residual = 1 kW > 0.001 kW.
        let result = checker().check_electrical(3.0, &[-2.0]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::InvariantViolation { check_name, .. } if check_name == "electrical_balance"
        ));
    }

    #[test]
    fn electrical_balance_passes_with_zip_adjusted_port_load() {
        // IEEE residential ZIP: Z=0.2, I=0.2, P=0.6 at V=0.95 pu.
        // scale = 0.2·0.95² + 0.2·0.95 + 0.6 = 0.2·0.9025 + 0.19 + 0.6 = 0.9705
        // p_load = 10 kW, p_gen = 0 kW.
        let scale = 0.2_f64 * 0.95_f64.powi(2) + 0.2 * 0.95 + 0.6;
        let p_load = 10.0_f64;
        let p_gen = 0.0_f64;
        let p_grid = p_load * scale + p_gen; // ZIP-adjusted solver output
        let port_net = p_load * scale + p_gen; // ZIP-adjusted port accumulation
        // When both sides use the same scaling, the residual is near-zero.
        let result = checker().check_electrical(p_grid, &[-port_net]);
        assert!(
            result.is_ok(),
            "ZIP-adjusted comparison should pass; residual should be < 0.001 kW"
        );
    }

    #[test]
    fn electrical_balance_fails_with_mismatched_zip_scaling() {
        // Demonstrates the bug: comparing ZIP-adjusted grid against raw ports
        // produces a false positive when scale ≠ 1.0.
        let scale = 0.2_f64 * 0.95_f64.powi(2) + 0.2 * 0.95 + 0.6;
        let p_load = 10.0_f64;
        let p_gen = 0.0_f64;
        let p_grid = p_load * scale + p_gen; // ZIP-adjusted solver output
        let port_net_raw = p_load + p_gen; // raw (unadjusted) port
        let result = checker().check_electrical(p_grid, &[-port_net_raw]);
        assert!(
            result.is_err(),
            "Comparing ZIP-adjusted grid vs raw ports with scale={scale} should produce false positive"
        );
    }

    // ── moisture_balance ──────────────────────────────────────────────────────

    #[test]
    fn moisture_balance_passes_when_balanced() {
        // 100 W latent for 600 s → 100 * 600 / 2_501_000 ≈ 2.399e-5 kg
        let dt_s = 600.0_f64;
        let q_latent_w = 100.0_f64;
        let delta_m = q_latent_w * dt_s / 2_501_000.0;
        let result = checker().check_moisture(delta_m, &[(q_latent_w, dt_s)]);
        assert!(result.is_ok());
    }

    #[test]
    fn moisture_balance_fails_on_large_discrepancy() {
        // 0.01 kg claimed change vs 0 latent → residual = 0.01 >> 1e-6.
        let result = checker().check_moisture(0.01, &[]);
        assert!(result.is_err());
    }

    // ── temperature bounds ────────────────────────────────────────────────────

    #[test]
    fn conditioned_zone_temperature_within_bounds_passes() {
        let result = checker().check_temperatures(&[20.0, -10.0], &[], &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn unconditioned_zone_temperature_within_bounds_passes() {
        // 95°C is valid for an attic under solar load.
        let result = checker().check_temperatures(&[], &[95.0, 119.0], &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn conditioned_zone_temperature_below_minimum_fails() {
        let result = checker().check_temperatures(&[-51.0], &[], &[]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::InvariantViolation { check_name, .. } if check_name == "zone_temperature_bounds"
        ));
    }

    #[test]
    fn conditioned_zone_temperature_above_maximum_fails() {
        // 80.1°C should fail for a conditioned zone.
        let result = checker().check_temperatures(&[80.1], &[], &[]);
        assert!(result.is_err());
    }

    #[test]
    fn unconditioned_zone_temperature_above_maximum_fails() {
        // 120.1°C should fail even for an unconditioned zone.
        let result = checker().check_temperatures(&[], &[120.1], &[]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::InvariantViolation { check_name, .. } if check_name == "unconditioned_zone_temperature_bounds"
        ));
    }

    #[test]
    fn zone_temperature_nan_fails() {
        let result = checker().check_temperatures(&[f64::NAN], &[], &[]);
        assert!(result.is_err());
    }

    #[test]
    fn tank_temperature_within_bounds_passes() {
        let result = checker().check_temperatures(&[], &[], &[50.0, 99.9]);
        assert!(result.is_ok());
    }

    #[test]
    fn tank_temperature_above_maximum_fails() {
        let result = checker().check_temperatures(&[], &[], &[100.1]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::InvariantViolation { check_name, .. } if check_name == "tank_temperature_bounds"
        ));
    }

    #[test]
    fn tank_temperature_below_minimum_fails() {
        let result = checker().check_temperatures(&[], &[], &[-0.1]);
        assert!(result.is_err());
    }

    // ── soc ──────────────────────────────────────────────────────────────────

    #[test]
    fn soc_within_range_returns_ok() {
        let result = checker().check_soc(0.5, 0.0);
        assert!(result.is_ok());
    }

    #[test]
    fn soc_out_of_range_returns_ok_with_warning() {
        // SoC violations warn but do not return Err.
        let result = checker().check_soc(1.5, 0.002);
        assert!(result.is_ok());
    }

    // ── protocol_native_registration ────────────────────────────────────────

    #[test]
    fn protocol_native_registration_passes_with_consumer() {
        let caps = [
            ControlCapabilities::POWER_SETPOINT,
            ControlCapabilities::PROTOCOL_NATIVE,
        ];
        let result = checker().check_protocol_native_registration(&caps);
        assert!(result.is_ok());
    }

    #[test]
    fn protocol_native_registration_fails_without_consumer() {
        let caps = [
            ControlCapabilities::POWER_SETPOINT,
            ControlCapabilities::THERMAL_SETPOINT,
        ];
        let result = checker().check_protocol_native_registration(&caps);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::InvariantViolation { check_name, .. } if check_name == "protocol_native_registration"
        ));
    }

    #[test]
    fn protocol_native_registration_fails_empty() {
        let caps: [ControlCapabilities; 0] = [];
        let result = checker().check_protocol_native_registration(&caps);
        assert!(result.is_err());
    }

    // ── check_fuel_electric_absent ───────────────────────────────────────────

    #[test]
    fn fuel_electric_absent_passes_when_zero() {
        let result = checker().check_fuel_electric_absent(0.0);
        assert!(result.is_ok());
    }

    #[test]
    fn fuel_electric_absent_fails_when_positive() {
        let result = checker().check_fuel_electric_absent(100.0);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::InvariantViolation { check_name, .. } if check_name == "electric_fuel_in_fuel_accumulator"
        ));
    }

    #[test]
    fn fuel_electric_absent_passes_when_negative() {
        // Negative contributions shouldn't appear for fuel but the check
        // only flags positive values to avoid false alarms.
        let result = checker().check_fuel_electric_absent(-50.0);
        assert!(result.is_ok());
    }
}
