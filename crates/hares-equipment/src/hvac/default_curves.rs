//! Equipment-type-aware default biquadratic performance curves.
//!
//! When no user-supplied curves are provided (via HPXML, config, or test
//! extras), the equipment init path substitutes these per-type defaults instead
//! of the identity placeholder `[1,0,0,0,0,0]`.
//!
//! Coefficients are sourced from OCHRE defaults CSVs:
//!   - `defaults/hvac_heating/ASHP Heater.csv` column `Single_1`
//!   - `defaults/hvac_heating/MSHP Heater.csv` column `Variable_1`
//!
//! Embedded as `const` arrays to avoid runtime file I/O in the constructor.
//! Multi-speed defaults are loaded via `DefaultsStore` in the HPXML resolver
//! (`apply_multispeed_parameters`); this module covers the single-speed gap.

use super::hvac_core::{DEFAULT_BIQUADRATIC_COEFFS, HvacEquipmentType};

/// ASHP single-speed heating capacity curve (OCHRE `ASHP Heater.csv` column `Single_1`, row `a_cap_t`–`f_cap_t`).
///
/// Verification at AHRI 210/240-2023 conditions:
///   - H1 (21.1°C indoor, 8.3°C outdoor): cap_ratio = 0.9951 ≈ 1.0 ✓
///   - H3 (21.1°C indoor, −8.3°C outdoor): cap_ratio = 0.6311 ✓
const ASHP_SINGLE_HEATING_CAPACITY: [f64; 6] = [
    0.878143655,
    -0.002914855,
    -0.00003337,
    0.022386661,
    0.000163944,
    -0.00002187,
];

/// ASHP single-speed heating EIR curve (OCHRE `ASHP Heater.csv` column `Single_1`, row `a_eir_t`–`f_eir_t`).
///
/// Verification at AHRI 210/240-2023 conditions:
///   - H1: eir_ratio = 0.9939
///   - H3: eir_ratio = 1.3459 (worse efficiency at low OAT ✓)
const ASHP_SINGLE_HEATING_EIR: [f64; 6] = [
    0.716518071,
    0.010275901,
    0.000460734,
    -0.006480365,
    0.000456354,
    -0.00069764,
];

/// MSHP variable-speed heating capacity curve (OCHRE `MSHP Heater.csv` column `Variable_1`, row `a_cap_t`–`f_cap_t`).
///
/// Verification at AHRI 210/240-2023 conditions:
///   - H1: cap_ratio = 0.9993 ≈ 1.0 ✓
///   - H3: cap_ratio = 0.5683 ✓
const MSHP_VARIABLE_HEATING_CAPACITY: [f64; 6] =
    [1.002928121, -0.010386676, 0.0, 0.025961538, 0.0, 0.0];

/// MSHP variable-speed heating EIR curve (OCHRE `MSHP Heater.csv` column `Variable_1`, row `a_eir_t`–`f_eir_t`).
const MSHP_VARIABLE_HEATING_EIR: [f64; 6] = [
    0.966475473,
    0.00591495,
    0.000191202,
    -0.012965668,
    0.00004225,
    -0.000524003,
];

/// Provenance of the biquadratic curve set currently in use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BiquadraticCurveSource {
    /// Identity `[1,0,0,0,0,0]` — no temperature-dependent correction.
    /// Only valid for equipment types that do not use biquadratic curves
    /// (furnace, baseboard, etc.) or for tests that explicitly set identity.
    Identity = 0,
    /// Equipment-type defaults substituted for identity placeholder.
    Default = 1,
    /// Curves explicitly provided by user config / HPXML.
    User = 2,
}

impl BiquadraticCurveSource {
    pub fn telemetry_value(self) -> f64 {
        self as i32 as f64
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Default => "default",
            Self::Identity => "identity",
        }
    }
}

/// Return the default interleaved `[cap_0, eir_0]` biquadratic coefficient
/// vector for the given `equipment_type`, or `None` if that type does not
/// use biquadratic curves in the heating path (furnace, baseboard, etc.).
///
/// Returns `Some` only for heat-pump types that need temperature-dependent
/// capacity and EIR corrections. Non-HP types continue to use identity.
pub(super) fn default_biquadratic_coeffs(
    equipment_type: HvacEquipmentType,
) -> Option<Vec<[f64; 6]>> {
    match equipment_type {
        HvacEquipmentType::AshpHeatPumpOnly | HvacEquipmentType::AshpHeatPumpAux => {
            Some(vec![ASHP_SINGLE_HEATING_CAPACITY, ASHP_SINGLE_HEATING_EIR])
        }
        HvacEquipmentType::MiniSplitHeat => Some(vec![
            MSHP_VARIABLE_HEATING_CAPACITY,
            MSHP_VARIABLE_HEATING_EIR,
        ]),
        HvacEquipmentType::GasFurnace
        | HvacEquipmentType::ElectricFurnace
        | HvacEquipmentType::AcCooler
        | HvacEquipmentType::MiniSplitCool
        | HvacEquipmentType::Baseboard
        | HvacEquipmentType::Other => None,
    }
}

/// Determine whether a coefficient vector represents the identity placeholder.
pub(super) fn is_identity(coeffs: &[[f64; 6]]) -> bool {
    coeffs.len() == 1 && coeffs[0] == DEFAULT_BIQUADRATIC_COEFFS
}

/// Substitute equipment-type-aware defaults for the identity placeholder.
///
/// Call this after `HvacEquipment::init()` has loaded curves from config.
/// If the loaded coefficients are still identity and the equipment type has
/// default curves available, this replaces them and returns `Default`.
/// If the coefficients are identity but the type has no defaults, returns
/// `Identity`. If the coefficients are non-identity (user-supplied), returns
/// `User`.
pub(super) fn maybe_substitute_defaults(
    biquadratic_coeffs: &mut Vec<[f64; 6]>,
    equipment_type: HvacEquipmentType,
) -> BiquadraticCurveSource {
    if !is_identity(biquadratic_coeffs) {
        return BiquadraticCurveSource::User;
    }
    if let Some(defaults) = default_biquadratic_coeffs(equipment_type) {
        tracing::info!(
            equipment_type = ?equipment_type,
            "substituting default biquadratic curves; original was identity"
        );
        *biquadratic_coeffs = defaults;
        BiquadraticCurveSource::Default
    } else {
        BiquadraticCurveSource::Identity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ashp_single_capacity_at_ahri_h1_approximately_unity() {
        let [c0, c1, c2, c3, c4, c5] = ASHP_SINGLE_HEATING_CAPACITY;
        let x1 = 21.1_f64;
        let x2 = 8.3_f64;
        let cap_ratio = c0 + c1 * x1 + c2 * x1 * x1 + c3 * x2 + c4 * x2 * x2 + c5 * x1 * x2;
        assert!(
            (cap_ratio - 1.0).abs() < 0.05,
            "ASHP Single_1 cap_ratio at AHRI H1 must be 1.0 ± 5%; got {cap_ratio:.6}"
        );
    }

    #[test]
    fn ashp_single_capacity_at_ahri_h3_below_08() {
        let [c0, c1, c2, c3, c4, c5] = ASHP_SINGLE_HEATING_CAPACITY;
        let x1 = 21.1_f64;
        let x2 = -8.3_f64;
        let cap_ratio = c0 + c1 * x1 + c2 * x1 * x1 + c3 * x2 + c4 * x2 * x2 + c5 * x1 * x2;
        assert!(
            cap_ratio < 0.8,
            "ASHP Single_1 cap_ratio at AHRI H3 must be < 0.8; got {cap_ratio:.6}"
        );
    }

    #[test]
    fn ashp_single_eir_at_h3_exceeds_h1() {
        let [c0, c1, c2, c3, c4, c5] = ASHP_SINGLE_HEATING_EIR;
        let eir = |x2: f64| -> f64 {
            let x1 = 21.1_f64;
            c0 + c1 * x1 + c2 * x1 * x1 + c3 * x2 + c4 * x2 * x2 + c5 * x1 * x2
        };
        let eir_h1 = eir(8.3);
        let eir_h3 = eir(-8.3);
        assert!(
            eir_h3 > eir_h1,
            "ASHP EIR at H3 ({eir_h3:.4}) must exceed H1 ({eir_h1:.4})"
        );
        assert!(
            eir_h3 > 1.0,
            "ASHP EIR at H3 must exceed 1.0; got {eir_h3:.4}"
        );
    }

    #[test]
    fn mshp_variable_eir_at_h3_exceeds_h1() {
        let [c0, c1, c2, c3, c4, c5] = MSHP_VARIABLE_HEATING_EIR;
        let eir = |x2: f64| -> f64 {
            let x1 = 21.1_f64;
            c0 + c1 * x1 + c2 * x1 * x1 + c3 * x2 + c4 * x2 * x2 + c5 * x1 * x2
        };
        let eir_h1 = eir(8.3);
        let eir_h3 = eir(-8.3);
        assert!(
            eir_h3 > eir_h1,
            "MSHP EIR at H3 ({eir_h3:.4}) must exceed H1 ({eir_h1:.4})"
        );
        assert!(
            eir_h3 > 1.0,
            "MSHP EIR at H3 must exceed 1.0; got {eir_h3:.4}"
        );
    }

    #[test]
    fn mshp_variable_capacity_at_ahri_h3_below_08() {
        let [c0, c1, c2, c3, c4, c5] = MSHP_VARIABLE_HEATING_CAPACITY;
        let x1 = 21.1_f64;
        let x2 = -8.3_f64;
        let cap_ratio = c0 + c1 * x1 + c2 * x1 * x1 + c3 * x2 + c4 * x2 * x2 + c5 * x1 * x2;
        assert!(
            cap_ratio < 0.8,
            "MSHP Variable_1 cap_ratio at AHRI H3 must be < 0.8; got {cap_ratio:.6}"
        );
    }

    #[test]
    fn identity_detection_single_identity_entry() {
        assert!(is_identity(&[DEFAULT_BIQUADRATIC_COEFFS]));
    }

    #[test]
    fn identity_detection_rejects_non_identity() {
        assert!(!is_identity(&[[0.9, 0.01, 0.0, 0.02, 0.0, 0.0]]));
    }

    #[test]
    fn identity_detection_rejects_multi_entry() {
        assert!(!is_identity(&[
            DEFAULT_BIQUADRATIC_COEFFS,
            DEFAULT_BIQUADRATIC_COEFFS,
        ]));
    }

    #[test]
    fn substitute_defaults_ashp_replaces_identity() {
        let mut coeffs = vec![DEFAULT_BIQUADRATIC_COEFFS];
        let source = maybe_substitute_defaults(&mut coeffs, HvacEquipmentType::AshpHeatPumpOnly);
        assert_eq!(source, BiquadraticCurveSource::Default);
        assert_eq!(coeffs.len(), 2);
        assert_eq!(coeffs[0], ASHP_SINGLE_HEATING_CAPACITY);
        assert_eq!(coeffs[1], ASHP_SINGLE_HEATING_EIR);
    }

    #[test]
    fn substitute_defaults_preserves_user_curves() {
        let user = [[0.9, 0.01, 0.0, 0.02, 0.0, 0.0]];
        let mut coeffs = user.to_vec();
        let source = maybe_substitute_defaults(&mut coeffs, HvacEquipmentType::AshpHeatPumpOnly);
        assert_eq!(source, BiquadraticCurveSource::User);
        assert_eq!(coeffs, user);
    }

    #[test]
    fn substitute_defaults_furnace_stays_identity() {
        let mut coeffs = vec![DEFAULT_BIQUADRATIC_COEFFS];
        let source = maybe_substitute_defaults(&mut coeffs, HvacEquipmentType::GasFurnace);
        assert_eq!(source, BiquadraticCurveSource::Identity);
        assert_eq!(coeffs, vec![DEFAULT_BIQUADRATIC_COEFFS]);
    }

    #[test]
    fn curve_source_telemetry_values() {
        assert_eq!(BiquadraticCurveSource::Identity.telemetry_value(), 0.0);
        assert_eq!(BiquadraticCurveSource::Default.telemetry_value(), 1.0);
        assert_eq!(BiquadraticCurveSource::User.telemetry_value(), 2.0);
    }

    #[test]
    fn curve_source_labels() {
        assert_eq!(BiquadraticCurveSource::Identity.label(), "identity");
        assert_eq!(BiquadraticCurveSource::Default.label(), "default");
        assert_eq!(BiquadraticCurveSource::User.label(), "user");
    }
}
