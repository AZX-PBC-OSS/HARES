//! Equipment-type-aware default biquadratic performance curves.
//!
//! When no user-supplied curves are provided (via HPXML, config, or test
//! extras), the equipment init path substitutes these per-type defaults instead
//! of the identity placeholder `[1,0,0,0,0,0]`.
//!
//! Coefficients are sourced from OCHRE defaults CSVs:
//!   - `defaults/HVAC Heating/Biquadratic ASHP Heater.csv` — columns
//!     `Single_1`, `Double_{1,2}`, `Variable_{1..4}` for per-speed ASHP
//!     heating curves.
//!   - `defaults/HVAC Heating/Biquadratic MSHP Heater.csv` — column
//!     `Variable_1` (all Variable_* columns are numerically identical).
//!
//! Embedded as `const` arrays to avoid runtime file I/O in the constructor.
//! Multi-speed defaults are loaded via `DefaultsStore` in the HPXML resolver
//! (`apply_multispeed_parameters`); this module covers the identity-substitution
//! gap for equipment that arrives with no explicit biquadratic curves.

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

/// ASHP two-speed heating capacity curves (OCHRE `ASHP Heater.csv` columns `Double_1`, `Double_2`,
/// rows `a_cap_t`–`f_cap_t`).
///
/// Verification at AHRI 210/240-2023 H1 (21.1 °C indoor, 8.3 °C outdoor):
///   - Double_1 cap_ratio ≈ 0.997 ✓
///   - Double_2 cap_ratio ≈ 1.009 ✓
///
/// H3 (21.1 °C indoor, −8.3 °C outdoor):
///   - Double_1 cap_ratio ≈ 0.62 < 0.8 ✓
///   - Double_2 cap_ratio ≈ 0.62 < 0.8 ✓
const ASHP_TWO_SPEED_HEATING_CAPACITY: [[f64; 6]; 2] = [
    [
        0.84077409,
        -0.001433659,
        -0.00015034,
        0.029628603,
        0.000161676,
        -0.00002349,
    ],
    [
        0.831506971,
        0.001839217,
        -0.0001876,
        0.026600206,
        0.000191484,
        -0.00006577,
    ],
];

/// ASHP two-speed heating EIR curves (OCHRE `ASHP Heater.csv` columns `Double_1`, `Double_2`,
/// rows `a_eir_t`–`f_eir_t`).
///
/// Verification at AHRI 210/240-2023 H3:
///   - Double_1 EIR ≈ 1.76 > 1.0 ✓
///   - Double_2 EIR ≈ 1.73 > 1.0 ✓
const ASHP_TWO_SPEED_HEATING_EIR: [[f64; 6]; 2] = [
    [
        0.539472334,
        0.016510315,
        0.000838745,
        -0.00403234,
        0.001424042,
        -0.002118063,
    ],
    [
        0.787746797,
        -0.000652315,
        0.000788668,
        -0.002320906,
        0.000747604,
        -0.001091731,
    ],
];

/// ASHP variable-speed heating capacity curves (OCHRE `ASHP Heater.csv` columns
/// `Variable_1`–`Variable_4`, rows `a_cap_t`–`f_cap_t`).
///
/// Verification at AHRI 210/240-2023 H1 (21.1 °C indoor, 8.3 °C outdoor):
///   - Variable_1 cap_ratio ≈ 0.993 ✓
///   - Variable_2 cap_ratio ≈ 1.000 ✓
///   - Variable_3 cap_ratio ≈ 0.996 ✓
///   - Variable_4 cap_ratio ≈ 1.000 ✓
const ASHP_VARIABLE_HEATING_CAPACITY: [[f64; 6]; 4] = [
    [
        0.893321032,
        -0.009733743,
        0.00006364,
        0.039113052,
        -0.000002508,
        -0.00027259,
    ],
    [
        0.923734534,
        -0.005970776,
        0.0,
        0.027816729,
        0.000065917,
        -0.00018925,
    ],
    [
        0.96205422,
        -0.009492778,
        0.00010921,
        0.024707831,
        0.000034225,
        -0.0001257,
    ],
    [
        0.936079154,
        -0.005481564,
        -0.00000859,
        0.024910532,
        0.000053087,
        -0.00015575,
    ],
];

/// ASHP variable-speed heating EIR curves (OCHRE `ASHP Heater.csv` columns
/// `Variable_1`–`Variable_4`, rows `a_eir_t`–`f_eir_t`).
///
/// Verification at AHRI 210/240-2023 H3 (−8.3 °C outdoor):
///   - Variable_1 EIR ≈ 1.59 > 1.0 ✓
///   - Variable_2 EIR ≈ 1.72 > 1.0 ✓
///   - Variable_3 EIR ≈ 1.71 > 1.0 ✓
///   - Variable_4 EIR ≈ 1.65 > 1.0 ✓
const ASHP_VARIABLE_HEATING_EIR: [[f64; 6]; 4] = [
    [
        0.466648487,
        0.020263329,
        0.001268392,
        -0.017016133,
        0.003174996,
        -0.003496096,
    ],
    [
        0.450656859,
        0.029290264,
        0.000393145,
        -0.009789518,
        0.000539369,
        -0.001180883,
    ],
    [
        0.572518011,
        0.022896249,
        0.000266019,
        -0.010667543,
        0.000490922,
        -0.000681369,
    ],
    [
        0.668195855,
        0.014671955,
        0.000445963,
        -0.011439229,
        0.000497103,
        -0.000690956,
    ],
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

/// GSHP single-speed heating capacity curve.
///
/// Biquadratic in entering air dry-bulb temperature (°C, x1) and entering
/// water temperature (°C, x2).  Coefficients derived from ASHRAE
/// Handbook of Fundamentals 2021 Ch.34 (Geothermal Energy Systems)
/// Fig.10 water-to-air heat pump performance data, cross-checked
/// against ClimateMaster Tranquility 22 (TCH072) manufacturer data
/// published in EnergyPlus dataset `WaterToAirHeatPumps.idf`.
///
/// Rated at ISO 13256-1 GLHP conditions: 21.1 °C (70 °F) entering air
/// dry-bulb, 10 °C (50 °F) entering water.
///
/// Verification at rated: cap_ratio = 1.000.
///   - 0 °C EWT, 21.1 °C air: cap_ratio ≈ 0.825 (17.5 % capacity loss)
///   - 21.1 °C EWT, 21.1 °C air: cap_ratio ≈ 1.183 (18 % capacity gain)
const GSHP_HEATING_CAPACITY: [f64; 6] = [0.9016563, -0.003, -0.00003, 0.018, -0.00005, 0.0];

/// GSHP single-speed heating EIR curve.
///
/// Biquadratic in entering air dry-bulb temperature (°C, x1) and entering
/// water temperature (°C, x2).  Same sources as `GSHP_HEATING_CAPACITY`.
///
/// Rated at ISO 13256-1 GLHP conditions (21.1 °C air, 10 °C water).
/// EIR increases (efficiency worsens) as entering water temperature drops.
///
/// Verification at rated: eir_ratio = 1.000.
///   - 0 °C EWT, 21.1 °C air: eir_ratio ≈ 1.173 (17 % worse COP)
///   - 21.1 °C EWT, 21.1 °C air: eir_ratio ≈ 0.815 (18 % better COP)
const GSHP_HEATING_EIR: [f64; 6] = [1.1472279, 0.001, 0.00001, -0.018, 0.00003, 0.00002];

/// GSHP single-speed cooling capacity curve.
///
/// Biquadratic in entering air **wet-bulb** temperature (°C, x1) and entering
/// water temperature (°C, x2).  Fitted for wet-bulb x1 to match the shared
/// cooling path in `air_conditioner.rs`, which passes `coil_entering_wb_c` as
/// x1 for all cooling equipment types (consistent with ASHP cooling curve
/// convention).  Same source references as heating.
///
/// Rated at ISO 13256-1 GLHP conditions: 27 °C (80.6 °F) entering air
/// dry-bulb, 19.4 °C entering air wet-bulb (~50 % RH), 15 °C (59 °F)
/// entering water (with 15 % methanol antifreeze).
///
/// Verification at rated: cap_ratio = 1.000.
///   - 5 °C EWT, 19.4 °C WB: cap_ratio ≈ 1.222 (22 % capacity gain)
///   - 25 °C EWT, 19.4 °C WB: cap_ratio ≈ 0.762 (24 % capacity loss)
const GSHP_COOLING_CAPACITY: [f64; 6] = [1.412307, -0.004, -0.00002, -0.021, -0.00008, 0.00002];

/// GSHP single-speed cooling EIR curve.
///
/// Biquadratic in entering air **wet-bulb** temperature (°C, x1) and entering
/// water temperature (°C, x2).  Fitted for wet-bulb x1 to match the shared
/// cooling path in `air_conditioner.rs` (see `GSHP_COOLING_CAPACITY`).
/// Same source references as heating.
///
/// Rated at ISO 13256-1 GLHP conditions (19.4 °C WB, 15 °C water).
/// EIR increases (efficiency worsens) as entering water temperature rises.
///
/// Verification at rated: eir_ratio = 1.000.
///   - 5 °C EWT, 19.4 °C WB: eir_ratio ≈ 0.774 (22 % better EER)
///   - 25 °C EWT, 19.4 °C WB: eir_ratio ≈ 1.236 (23 % worse EER)
const GSHP_COOLING_EIR: [f64; 6] = [0.602606, 0.003, 0.00001, 0.022, 0.00005, -0.00002];

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

/// Return the default interleaved `[cap_0, eir_0, cap_1, eir_1, ...]` biquadratic
/// coefficient vector for the given `equipment_type` and `speed_count`, or `None`
/// if that type does not use biquadratic curves in the heating path (furnace,
/// baseboard, etc.).
///
/// ## Per-equipment-type behaviour
///
/// - **ASHP (heating):** For `speed_count` ∈ {1, 2, 4}, distinct per-stage
///   coefficients are sourced from the OCHRE default CSVs
///   (`vendors/OCHRE/ochre/defaults/HVAC Heating/Biquadratic ASHP Heater.csv`).
///   For any other `speed_count`, the single-speed pair is replicated (with a
///   `tracing::warn!`) — this is a genuine limitation because OCHRE provides
///   only columns for `Single_1`, `Double_{1,2}`, and `Variable_{1..4}`.
/// - **MSHP (heating):** All speed stages share identical capacity and EIR
///   coefficients in the vendored OCHRE CSV
///   (`vendors/OCHRE/ochre/defaults/HVAC Heating/Biquadratic MSHP Heater.csv` —
///   `Variable_1` through `Variable_4` are byte-identical columns). Replication
///   of the single-speed pair is therefore numerically correct.
/// - **GSHP/WSHP (heating & cooling):** No per-speed OCHRE defaults exist.
///   The single-speed pair is replicated with a `tracing::warn!` for
///   `speed_count > 1`.
///
/// Returns `Some` only for heat-pump types that need temperature-dependent
/// capacity and EIR corrections. Non-HP types continue to use identity.
pub(super) fn default_biquadratic_coeffs(
    equipment_type: HvacEquipmentType,
    speed_count: usize,
) -> Option<Vec<[f64; 6]>> {
    let count = speed_count.max(1);

    match equipment_type {
        HvacEquipmentType::AshpHeatPumpOnly | HvacEquipmentType::AshpHeatPumpAux => {
            Some(match count {
                1 => {
                    vec![ASHP_SINGLE_HEATING_CAPACITY, ASHP_SINGLE_HEATING_EIR]
                }
                2 => interleave_per_speed(
                    &ASHP_TWO_SPEED_HEATING_CAPACITY,
                    &ASHP_TWO_SPEED_HEATING_EIR,
                ),
                4 => interleave_per_speed(
                    &ASHP_VARIABLE_HEATING_CAPACITY,
                    &ASHP_VARIABLE_HEATING_EIR,
                ),
                _ => {
                    tracing::warn!(
                        equipment_type = ?equipment_type,
                        speed_count = count,
                        "ASHP with {} speeds has no per-stage default curves in \
                         the OCHRE CSV; replicating single-speed defaults. Per-stage \
                         COP correction will be identical across speeds.",
                        count,
                    );
                    replicate(ASHP_SINGLE_HEATING_CAPACITY, ASHP_SINGLE_HEATING_EIR, count)
                }
            })
        }
        HvacEquipmentType::MiniSplitHeat => {
            // MSHP Variable_1–4 columns are numerically identical; replication is correct.
            let coeffs = replicate(
                MSHP_VARIABLE_HEATING_CAPACITY,
                MSHP_VARIABLE_HEATING_EIR,
                count,
            );
            Some(coeffs)
        }
        HvacEquipmentType::GshpHeatPumpHeating | HvacEquipmentType::WshpHeatPumpHeating => {
            let coeffs = replicate(GSHP_HEATING_CAPACITY, GSHP_HEATING_EIR, count);
            if count > 1 {
                tracing::warn!(
                    equipment_type = ?equipment_type,
                    speed_count = count,
                    "multi-speed equipment ({speed_count} speeds) using replicated single-speed \
                     biquadratic curves; per-stage COP correction will be identical across speeds",
                );
            }
            Some(coeffs)
        }
        HvacEquipmentType::GshpHeatPumpCooling | HvacEquipmentType::WshpHeatPumpCooling => {
            let coeffs = replicate(GSHP_COOLING_CAPACITY, GSHP_COOLING_EIR, count);
            if count > 1 {
                tracing::warn!(
                    equipment_type = ?equipment_type,
                    speed_count = count,
                    "multi-speed equipment ({speed_count} speeds) using replicated single-speed \
                     biquadratic curves; per-stage COP correction will be identical across speeds",
                );
            }
            Some(coeffs)
        }
        HvacEquipmentType::GasFurnace
        | HvacEquipmentType::ElectricFurnace
        | HvacEquipmentType::AcCooler
        | HvacEquipmentType::AshpHeatPumpCooling
        | HvacEquipmentType::MiniSplitCool
        | HvacEquipmentType::Baseboard
        | HvacEquipmentType::Other => None,
    }
}

/// Interleave per-speed cap/EIR coefficient arrays: `[cap[0], eir[0], cap[1], eir[1], ...]`.
fn interleave_per_speed(caps: &[[f64; 6]], eirs: &[[f64; 6]]) -> Vec<[f64; 6]> {
    assert_eq!(
        caps.len(),
        eirs.len(),
        "cap and EIR per-speed arrays must have the same length"
    );
    let n = caps.len();
    let mut coeffs = Vec::with_capacity(n * 2);
    for i in 0..n {
        coeffs.push(caps[i]);
        coeffs.push(eirs[i]);
    }
    coeffs
}

/// Build a replicated interleaved curve vector: `n` copies of `(cap, eir)`.
fn replicate(cap: [f64; 6], eir: [f64; 6], n: usize) -> Vec<[f64; 6]> {
    let mut coeffs = Vec::with_capacity(n * 2);
    for _ in 0..n {
        coeffs.push(cap);
        coeffs.push(eir);
    }
    coeffs
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
///
/// Warnings about replicated defaults for multi-speed equipment are emitted
/// by `default_biquadratic_coeffs` when per-stage data is unavailable.
pub(super) fn maybe_substitute_defaults(
    biquadratic_coeffs: &mut Vec<[f64; 6]>,
    equipment_type: HvacEquipmentType,
    speed_count: usize,
) -> BiquadraticCurveSource {
    if !is_identity(biquadratic_coeffs) {
        return BiquadraticCurveSource::User;
    }
    if let Some(defaults) = default_biquadratic_coeffs(equipment_type, speed_count) {
        if speed_count == 1 {
            tracing::info!(
                equipment_type = ?equipment_type,
                "substituting default biquadratic curves; original was identity"
            );
        }
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
        let source = maybe_substitute_defaults(&mut coeffs, HvacEquipmentType::AshpHeatPumpOnly, 1);
        assert_eq!(source, BiquadraticCurveSource::Default);
        assert_eq!(coeffs.len(), 2);
        assert_eq!(coeffs[0], ASHP_SINGLE_HEATING_CAPACITY);
        assert_eq!(coeffs[1], ASHP_SINGLE_HEATING_EIR);
    }

    #[test]
    fn substitute_defaults_preserves_user_curves() {
        let user = [[0.9, 0.01, 0.0, 0.02, 0.0, 0.0]];
        let mut coeffs = user.to_vec();
        let source = maybe_substitute_defaults(&mut coeffs, HvacEquipmentType::AshpHeatPumpOnly, 1);
        assert_eq!(source, BiquadraticCurveSource::User);
        assert_eq!(coeffs, user);
    }

    #[test]
    fn substitute_defaults_furnace_stays_identity() {
        let mut coeffs = vec![DEFAULT_BIQUADRATIC_COEFFS];
        let source = maybe_substitute_defaults(&mut coeffs, HvacEquipmentType::GasFurnace, 1);
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

    /// Helper: evaluate biquadratic at (x1, x2).
    fn eval_biquadratic(coeffs: &[f64; 6], x1: f64, x2: f64) -> f64 {
        coeffs[0]
            + coeffs[1] * x1
            + coeffs[2] * x1 * x1
            + coeffs[3] * x2
            + coeffs[4] * x2 * x2
            + coeffs[5] * x1 * x2
    }

    #[test]
    fn gshp_heating_capacity_at_rated_approximately_unity() {
        let cap = eval_biquadratic(&GSHP_HEATING_CAPACITY, 21.1, 10.0);
        assert!(
            (cap - 1.0).abs() < 0.02,
            "GSHP heating cap_ratio at rated (21.1 °C air, 10 °C water) must be 1.0 ± 2 %; got {cap:.6}"
        );
    }

    #[test]
    fn gshp_heating_capacity_drops_at_freezing_ewt() {
        let cap_rated = eval_biquadratic(&GSHP_HEATING_CAPACITY, 21.1, 10.0);
        let cap_cold = eval_biquadratic(&GSHP_HEATING_CAPACITY, 21.1, 0.0);
        assert!(
            cap_cold < cap_rated,
            "GSHP heating capacity at 0 °C EWT ({cap_cold:.4}) must be below rated ({cap_rated:.4})"
        );
        assert!(
            cap_cold > 0.7,
            "GSHP heating capacity at 0 °C EWT ({cap_cold:.4}) must stay above 0.7"
        );
    }

    #[test]
    fn gshp_heating_eir_worse_at_freezing_ewt() {
        let eir_rated = eval_biquadratic(&GSHP_HEATING_EIR, 21.1, 10.0);
        let eir_cold = eval_biquadratic(&GSHP_HEATING_EIR, 21.1, 0.0);
        assert!(
            eir_cold > eir_rated,
            "GSHP heating EIR at 0 °C EWT ({eir_cold:.4}) must exceed rated ({eir_rated:.4})"
        );
        assert!(
            eir_cold > 1.0,
            "GSHP heating EIR at 0 °C EWT must exceed 1.0; got {eir_cold:.4}"
        );
    }

    #[test]
    fn gshp_heating_eir_better_at_warm_ewt() {
        let eir_rated = eval_biquadratic(&GSHP_HEATING_EIR, 21.1, 10.0);
        let eir_warm = eval_biquadratic(&GSHP_HEATING_EIR, 21.1, 21.1);
        assert!(
            eir_warm < eir_rated,
            "GSHP heating EIR at 21.1 °C EWT ({eir_warm:.4}) must be below rated ({eir_rated:.4})"
        );
    }

    #[test]
    fn gshp_cooling_capacity_at_rated_approximately_unity() {
        let cap = eval_biquadratic(&GSHP_COOLING_CAPACITY, 19.4, 15.0);
        assert!(
            (cap - 1.0).abs() < 0.02,
            "GSHP cooling cap_ratio at rated (19.4 °C WB air, 15 °C water) must be 1.0 ± 2 %; got {cap:.6}"
        );
    }

    #[test]
    fn gshp_cooling_capacity_higher_at_cold_ewt() {
        let cap_rated = eval_biquadratic(&GSHP_COOLING_CAPACITY, 19.4, 15.0);
        let cap_cold = eval_biquadratic(&GSHP_COOLING_CAPACITY, 19.4, 5.0);
        assert!(
            cap_cold > cap_rated,
            "GSHP cooling capacity at 5 °C EWT ({cap_cold:.4}) must exceed rated ({cap_rated:.4})"
        );
        assert!(
            cap_cold > 1.0,
            "GSHP cooling capacity at 5 °C EWT must exceed 1.0; got {cap_cold:.4}"
        );
    }

    #[test]
    fn gshp_cooling_capacity_lower_at_warm_ewt() {
        let cap_rated = eval_biquadratic(&GSHP_COOLING_CAPACITY, 19.4, 15.0);
        let cap_warm = eval_biquadratic(&GSHP_COOLING_CAPACITY, 19.4, 25.0);
        assert!(
            cap_warm < cap_rated,
            "GSHP cooling capacity at 25 °C EWT ({cap_warm:.4}) must be below rated ({cap_rated:.4})"
        );
        assert!(
            cap_warm > 0.5,
            "GSHP cooling capacity at 25 °C EWT ({cap_warm:.4}) must stay above 0.5"
        );
    }

    #[test]
    fn gshp_cooling_eir_better_at_cold_ewt() {
        let eir_rated = eval_biquadratic(&GSHP_COOLING_EIR, 19.4, 15.0);
        let eir_cold = eval_biquadratic(&GSHP_COOLING_EIR, 19.4, 5.0);
        assert!(
            eir_cold < eir_rated,
            "GSHP cooling EIR at 5 °C EWT ({eir_cold:.4}) must be below rated ({eir_rated:.4})"
        );
        assert!(
            eir_cold > 0.5,
            "GSHP cooling EIR at 5 °C EWT ({eir_cold:.4}) must stay above 0.5"
        );
    }

    #[test]
    fn gshp_cooling_eir_worse_at_warm_ewt() {
        let eir_rated = eval_biquadratic(&GSHP_COOLING_EIR, 19.4, 15.0);
        let eir_warm = eval_biquadratic(&GSHP_COOLING_EIR, 19.4, 25.0);
        assert!(
            eir_warm > eir_rated,
            "GSHP cooling EIR at 25 °C EWT ({eir_warm:.4}) must exceed rated ({eir_rated:.4})"
        );
        assert!(
            eir_warm > 1.0,
            "GSHP cooling EIR at 25 °C EWT must exceed 1.0; got {eir_warm:.4}"
        );
    }

    #[test]
    fn gshp_heating_capacity_all_positive_in_operating_range() {
        // Typical GSHP operating range: 16–27 °C indoor air, −1–33 °C EWT
        for ta in [16.0, 18.0, 20.0, 22.0, 24.0, 27.0] {
            for tw in [-1.0, 0.0, 5.0, 10.0, 15.0, 20.0, 25.0, 32.2] {
                let cap = eval_biquadratic(&GSHP_HEATING_CAPACITY, ta, tw);
                assert!(
                    cap > 0.0,
                    "GSHP heating capacity must be > 0 at T_air={ta} °C, T_water={tw} °C; got {cap:.4}"
                );
            }
        }
    }

    #[test]
    fn gshp_cooling_capacity_all_positive_in_operating_range() {
        // Ground loop cooling wet-bulb range: 13–24 °C indoor WB, −5–35 °C EWT
        for ta in [13.0, 15.0, 17.0, 19.4, 22.0, 24.0] {
            for tw in [-5.0, 0.0, 5.0, 10.0, 15.0, 20.0, 25.0, 30.0, 35.0] {
                let cap = eval_biquadratic(&GSHP_COOLING_CAPACITY, ta, tw);
                assert!(
                    cap > 0.0,
                    "GSHP cooling capacity must be > 0 at T_air={ta} °C WB, T_water={tw} °C; got {cap:.4}"
                );
            }
        }
    }

    #[test]
    fn substitute_defaults_gshp_heating_replaces_identity() {
        let mut coeffs = vec![DEFAULT_BIQUADRATIC_COEFFS];
        let source =
            maybe_substitute_defaults(&mut coeffs, HvacEquipmentType::GshpHeatPumpHeating, 1);
        assert_eq!(source, BiquadraticCurveSource::Default);
        assert_eq!(coeffs.len(), 2);
        assert_eq!(coeffs[0], GSHP_HEATING_CAPACITY);
        assert_eq!(coeffs[1], GSHP_HEATING_EIR);
    }

    #[test]
    fn substitute_defaults_gshp_cooling_replaces_identity() {
        let mut coeffs = vec![DEFAULT_BIQUADRATIC_COEFFS];
        let source =
            maybe_substitute_defaults(&mut coeffs, HvacEquipmentType::GshpHeatPumpCooling, 1);
        assert_eq!(source, BiquadraticCurveSource::Default);
        assert_eq!(coeffs.len(), 2);
        assert_eq!(coeffs[0], GSHP_COOLING_CAPACITY);
        assert_eq!(coeffs[1], GSHP_COOLING_EIR);
    }

    #[test]
    fn gshp_heating_eir_all_positive_in_operating_range() {
        for ta in [16.0, 18.0, 20.0, 22.0, 24.0, 27.0] {
            for tw in [-1.0, 0.0, 5.0, 10.0, 15.0, 20.0, 25.0, 32.2] {
                let eir = eval_biquadratic(&GSHP_HEATING_EIR, ta, tw);
                assert!(
                    eir > 0.0,
                    "GSHP heating EIR must be > 0 at T_air={ta} °C, T_water={tw} °C; got {eir:.4}"
                );
            }
        }
    }

    #[test]
    fn gshp_cooling_eir_all_positive_in_operating_range() {
        for ta in [13.0, 15.0, 17.0, 19.4, 22.0, 24.0] {
            for tw in [-5.0, 0.0, 5.0, 10.0, 15.0, 20.0, 25.0, 30.0, 35.0] {
                let eir = eval_biquadratic(&GSHP_COOLING_EIR, ta, tw);
                assert!(
                    eir > 0.0,
                    "GSHP cooling EIR must be > 0 at T_air={ta} °C WB, T_water={tw} °C; got {eir:.4}"
                );
            }
        }
    }

    /// Regression: MSHP variable-speed heating capacity curve must evaluate to
    /// ≥ 0.0 at −50 °C outdoor / 21.1 °C indoor when the non-negative output
    /// clamp is applied.  Before the fix the unclamped biquadratic evaluated
    /// to −0.5143 at this condition.
    #[test]
    fn mshp_heating_capacity_non_negative_at_extreme_cold() {
        use hares_physics::biquadratic::BiquadraticCurve;

        let curve = BiquadraticCurve {
            coeffs: MSHP_VARIABLE_HEATING_CAPACITY,
            x1_bounds: (-10.0, 50.0),
            x2_bounds: (-50.0, 60.0),
            warn_on_clamp: false,
            output_min: Some(0.0),
            output_max: None,
        };
        let result = curve.evaluate(21.1, -50.0);
        assert!(
            result >= 0.0,
            "MSHP capacity at T_indoor=21.1°C, T_outdoor=-50°C must be ≥ 0.0 with output_min=0.0; \
             got {result}"
        );
        let result_h1 = curve.evaluate(21.1, 8.3);
        assert!(
            result_h1 > 0.9,
            "MSHP capacity at AHRI H1 (21.1°C / 8.3°C) must still be near 1.0; got {result_h1}"
        );
    }

    #[test]
    fn default_biquadratic_coeffs_speed_4_ashp_produces_8_interleaved_entries() {
        let coeffs = default_biquadratic_coeffs(HvacEquipmentType::AshpHeatPumpOnly, 4).unwrap();
        // 4 speeds × 2 (cap + EIR) = 8 interleaved entries.
        assert_eq!(coeffs.len(), 8, "speed_count=4 must produce 8 entries");

        // Each pair must match the corresponding variable-speed ASHP constants
        // from the OCHRE CSV (Variable_1–4), not replicated single-speed values.
        for speed in 0..4 {
            assert_eq!(
                coeffs[speed * 2],
                ASHP_VARIABLE_HEATING_CAPACITY[speed],
                "speed {speed} capacity curve must match ASHP Variable_{} default",
                speed + 1,
            );
            assert_eq!(
                coeffs[speed * 2 + 1],
                ASHP_VARIABLE_HEATING_EIR[speed],
                "speed {speed} EIR curve must match ASHP Variable_{} default",
                speed + 1,
            );
        }
    }

    #[test]
    fn default_biquadratic_coeffs_speed_1_returns_single_pair() {
        let coeffs = default_biquadratic_coeffs(HvacEquipmentType::AshpHeatPumpOnly, 1).unwrap();
        assert_eq!(coeffs.len(), 2, "speed_count=1 must produce 2 entries");
        assert_eq!(coeffs[0], ASHP_SINGLE_HEATING_CAPACITY);
        assert_eq!(coeffs[1], ASHP_SINGLE_HEATING_EIR);
    }

    #[test]
    fn default_biquadratic_coeffs_speed_2_ashp_produces_distinct_two_speed_pairs() {
        let coeffs = default_biquadratic_coeffs(HvacEquipmentType::AshpHeatPumpOnly, 2).unwrap();
        assert_eq!(coeffs.len(), 4, "speed_count=2 must produce 4 entries");
        // Two distinct cap/EIR pairs from the Double_1 / Double_2 columns.
        for speed in 0..2 {
            assert_eq!(
                coeffs[speed * 2],
                ASHP_TWO_SPEED_HEATING_CAPACITY[speed],
                "speed {speed} capacity curve must match ASHP Double_{} default",
                speed + 1,
            );
            assert_eq!(
                coeffs[speed * 2 + 1],
                ASHP_TWO_SPEED_HEATING_EIR[speed],
                "speed {speed} EIR curve must match ASHP Double_{} default",
                speed + 1,
            );
        }
        // The two speed stages must have genuinely different coefficients.
        assert_ne!(
            coeffs[0], coeffs[2],
            "Double_1 and Double_2 capacity curves must differ"
        );
        assert_ne!(
            coeffs[1], coeffs[3],
            "Double_1 and Double_2 EIR curves must differ"
        );
    }

    #[test]
    fn ashp_variable_speed_capacity_h1_approximately_unity() {
        for (i, coeffs) in ASHP_VARIABLE_HEATING_CAPACITY.iter().enumerate() {
            let [c0, c1, c2, c3, c4, c5] = *coeffs;
            let x1 = 21.1_f64;
            let x2 = 8.3_f64;
            let cap_ratio = c0 + c1 * x1 + c2 * x1 * x1 + c3 * x2 + c4 * x2 * x2 + c5 * x1 * x2;
            assert!(
                (cap_ratio - 1.0).abs() < 0.05,
                "ASHP Variable_{} cap_ratio at AHRI H1 must be 1.0 ± 5%; got {cap_ratio:.6}",
                i + 1,
            );
        }
    }

    #[test]
    fn ashp_variable_speed_capacity_h3_below_08() {
        for (i, coeffs) in ASHP_VARIABLE_HEATING_CAPACITY.iter().enumerate() {
            let [c0, c1, c2, c3, c4, c5] = *coeffs;
            let x1 = 21.1_f64;
            let x2 = -8.3_f64;
            let cap_ratio = c0 + c1 * x1 + c2 * x1 * x1 + c3 * x2 + c4 * x2 * x2 + c5 * x1 * x2;
            assert!(
                cap_ratio < 0.8,
                "ASHP Variable_{} cap_ratio at AHRI H3 must be < 0.8; got {cap_ratio:.6}",
                i + 1,
            );
        }
    }

    #[test]
    fn ashp_variable_speed_eir_h3_exceeds_h1() {
        for (i, coeffs) in ASHP_VARIABLE_HEATING_EIR.iter().enumerate() {
            let [c0, c1, c2, c3, c4, c5] = *coeffs;
            let eir = |x2: f64| -> f64 {
                let x1 = 21.1_f64;
                c0 + c1 * x1 + c2 * x1 * x1 + c3 * x2 + c4 * x2 * x2 + c5 * x1 * x2
            };
            let eir_h1 = eir(8.3);
            let eir_h3 = eir(-8.3);
            assert!(
                eir_h3 > eir_h1,
                "ASHP Variable_{} EIR at H3 ({eir_h3:.4}) must exceed H1 ({eir_h1:.4})",
                i + 1,
            );
            assert!(
                eir_h3 > 1.0,
                "ASHP Variable_{} EIR at H3 must exceed 1.0; got {eir_h3:.4}",
                i + 1,
            );
        }
    }

    #[test]
    fn ashp_two_speed_capacity_h1_approximately_unity() {
        for (i, coeffs) in ASHP_TWO_SPEED_HEATING_CAPACITY.iter().enumerate() {
            let [c0, c1, c2, c3, c4, c5] = *coeffs;
            let x1 = 21.1_f64;
            let x2 = 8.3_f64;
            let cap_ratio = c0 + c1 * x1 + c2 * x1 * x1 + c3 * x2 + c4 * x2 * x2 + c5 * x1 * x2;
            assert!(
                (cap_ratio - 1.0).abs() < 0.05,
                "ASHP Double_{} cap_ratio at AHRI H1 must be 1.0 ± 5%; got {cap_ratio:.6}",
                i + 1,
            );
        }
    }

    #[test]
    fn ashp_two_speed_eir_h3_exceeds_h1() {
        for (i, coeffs) in ASHP_TWO_SPEED_HEATING_EIR.iter().enumerate() {
            let [c0, c1, c2, c3, c4, c5] = *coeffs;
            let eir = |x2: f64| -> f64 {
                let x1 = 21.1_f64;
                c0 + c1 * x1 + c2 * x1 * x1 + c3 * x2 + c4 * x2 * x2 + c5 * x1 * x2
            };
            let eir_h1 = eir(8.3);
            let eir_h3 = eir(-8.3);
            assert!(
                eir_h3 > eir_h1,
                "ASHP Double_{} EIR at H3 ({eir_h3:.4}) must exceed H1 ({eir_h1:.4})",
                i + 1,
            );
            assert!(
                eir_h3 > 1.0,
                "ASHP Double_{} EIR at H3 must exceed 1.0; got {eir_h3:.4}",
                i + 1,
            );
        }
    }

    #[test]
    fn ashp_variable_speed_pairs_are_distinct_per_stage() {
        // The four variable-speed pairs must genuinely differ from each other —
        // not replicated single-speed values. Compare each pair against every
        // other pair.
        for i in 0..4 {
            for j in (i + 1)..4 {
                assert_ne!(
                    ASHP_VARIABLE_HEATING_CAPACITY[i],
                    ASHP_VARIABLE_HEATING_CAPACITY[j],
                    "ASHP Variable_{} and Variable_{} capacity curves must differ",
                    i + 1,
                    j + 1,
                );
                assert_ne!(
                    ASHP_VARIABLE_HEATING_EIR[i],
                    ASHP_VARIABLE_HEATING_EIR[j],
                    "ASHP Variable_{} and Variable_{} EIR curves must differ",
                    i + 1,
                    j + 1,
                );
            }
        }
    }

    #[test]
    fn default_biquadratic_coeffs_speed_0_returns_single_pair() {
        let coeffs = default_biquadratic_coeffs(HvacEquipmentType::AshpHeatPumpOnly, 0).unwrap();
        assert_eq!(
            coeffs.len(),
            2,
            "speed_count=0 must produce 2 entries (floor at 1)"
        );
    }

    #[test]
    fn maybe_substitute_defaults_multi_speed_replicates_mshp_curves() {
        let mut coeffs = vec![DEFAULT_BIQUADRATIC_COEFFS];
        let source = maybe_substitute_defaults(&mut coeffs, HvacEquipmentType::MiniSplitHeat, 4);
        assert_eq!(source, BiquadraticCurveSource::Default);
        assert_eq!(coeffs.len(), 8, "MSHP speed_count=4 must produce 8 entries");
        for cap_idx in [0, 2, 4, 6] {
            assert_eq!(coeffs[cap_idx], MSHP_VARIABLE_HEATING_CAPACITY);
        }
        for eir_idx in [1, 3, 5, 7] {
            assert_eq!(coeffs[eir_idx], MSHP_VARIABLE_HEATING_EIR);
        }
    }

    #[test]
    fn maybe_substitute_defaults_multi_speed_preserves_user_curves() {
        let user = [
            [0.9, 0.01, 0.0, 0.02, 0.0, 0.0],
            [0.8, 0.01, 0.0, 0.02, 0.0, 0.0],
            [0.7, 0.01, 0.0, 0.02, 0.0, 0.0],
            [0.6, 0.01, 0.0, 0.02, 0.0, 0.0],
        ];
        let mut coeffs = user.to_vec();
        let source = maybe_substitute_defaults(&mut coeffs, HvacEquipmentType::AshpHeatPumpOnly, 4);
        assert_eq!(source, BiquadraticCurveSource::User);
        assert_eq!(coeffs, user);
    }
}
