//! DOE 10 CFR 430 standby-loss test UA derivation for storage water heaters.
//!
//! Converts HPXML EnergyFactor (EF) or UniformEnergyFactor (UEF) to a tank
//! standby-loss UA (W/K) using the Burch & Erickson (2004) methodology for
//! EF, and the Maguire & Roberts (2020) methodology for UEF.
//!
//! Reference: OCHRE `ochre/utils/hpxml.py` `parse_water_heater()`.

/// DOE 24-hour test: lb/gal density of water.
const DENSITY_LB_PER_GAL: f64 = 8.2938;
/// DOE test: specific heat of water, Btu/(lb·°F).
const CP_BTU_LB_F: f64 = 1.0007;
/// DOE EF test: mains inlet temperature, °F.
const T_IN_EF_F: f64 = 58.0;
/// DOE UEF test: mains inlet temperature, °F.
const T_IN_UEF_F: f64 = 58.0;
/// DOE EF test: ambient temperature, °F.
const T_ENV_F: f64 = 67.5;
/// DOE EF test: setpoint temperature, °F.
const T_SETPOINT_EF_F: f64 = 135.0;
/// DOE UEF test: setpoint temperature, °F.
const T_SETPOINT_UEF_F: f64 = 125.0;
/// DOE EF test: daily draw volume, gal.
const VOLUME_DRAWN_EF_GAL: f64 = 64.3;
use hares_physics::constants::{UEF_TO_EF_GAS_INTERCEPT, UEF_TO_EF_GAS_SLOPE};
use hares_physics::units as conv;

/// Water heater type and fuel discriminant, sufficient for UA routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhCategory {
    StorageElectric,
    StorageGas,
    HeatPump,
    Instantaneous,
}

/// Inputs required to compute tank UA from HPXML energy-factor fields.
#[derive(Debug, Clone)]
pub struct UaInputs {
    pub category: WhCategory,
    pub energy_factor: Option<f64>,
    pub uniform_energy_factor: Option<f64>,
    /// Nominal rated volume in gallons (before the OCHRE volume-reduction factor).
    pub tank_volume_rated_gal: Option<f64>,
    /// Burner/element recovery efficiency, if explicitly provided in HPXML.
    pub recovery_efficiency: Option<f64>,
    /// Rated heating capacity, Btu/hr.  Required for gas storage EF path.
    pub heating_capacity_btu_hr: Option<f64>,
    /// UEF first-hour rating in gallons (selects draw bin for UEF path).
    pub first_hour_rating_gal: Option<f64>,
}

/// Result of the UA derivation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UaResult {
    /// Standby-loss UA, W/K.
    pub ua_w_per_k: f64,
    /// Conversion efficiency (η_c) of the heat source.  For electric storage
    /// and HPWH this is 1.0.  For gas storage it is > RE and ≤ 1.0.
    pub conversion_efficiency: f64,
    /// Equivalent EF (derived from UEF when EF was not supplied).
    pub energy_factor: Option<f64>,
}

/// Compute UA (W/K) and conversion efficiency from HPXML EF/UEF fields.
///
/// Returns `None` when neither `energy_factor` nor `uniform_energy_factor`
/// is present in `inputs`, or when the category does not produce a tank UA
/// (i.e. `Instantaneous`).
///
/// # Errors
///
/// Returns an error string when the computed UA is negative (physically
/// impossible given the inputs) or the conversion efficiency exceeds 1.0.
pub fn ua_from_energy_factor(inputs: &UaInputs) -> Result<Option<UaResult>, String> {
    match inputs.category {
        WhCategory::Instantaneous => {
            // Tankless: no standby loss UA; the caller can set ua = 0.
            Ok(None)
        }
        WhCategory::HeatPump => ua_for_hpwh(inputs).map(Some),
        WhCategory::StorageElectric => ua_for_electric_storage(inputs).map(Some),
        WhCategory::StorageGas => ua_for_gas_storage(inputs).map(Some),
    }
}

// ---------------------------------------------------------------------------
// Per-type implementations
// ---------------------------------------------------------------------------

fn ua_for_hpwh(inputs: &UaInputs) -> Result<UaResult, String> {
    // OCHRE / ResStock: UA is driven solely by tank volume (three bins).
    // EF/UEF are used downstream for COP derivation, not for the UA formula.
    // Source: ResStock waterheater.rb L765.
    let volume_gal = inputs
        .tank_volume_rated_gal
        .ok_or_else(|| "HPWH UA requires TankVolume".to_string())?;

    // OCHRE applies a 0.9× factor to rated electric volume.
    let volume_actual_gal = volume_gal * 0.9;

    let ua_btu_hr_f = if volume_actual_gal <= 58.0 {
        3.6_f64
    } else if volume_actual_gal <= 73.0 {
        4.0_f64
    } else {
        4.7_f64
    };

    let ua_w_per_k = conv::btu_hr_per_f_to_w_per_k(ua_btu_hr_f);
    Ok(UaResult {
        ua_w_per_k,
        conversion_efficiency: 1.0,
        energy_factor: inputs.energy_factor.or(inputs.uniform_energy_factor),
    })
}

fn ua_for_electric_storage(inputs: &UaInputs) -> Result<UaResult, String> {
    let volume_gal = inputs
        .tank_volume_rated_gal
        .ok_or_else(|| "Electric storage WH UA requires TankVolume".to_string())?;

    let _volume_actual_gal = volume_gal * 0.9; // OCHRE reduction; kept for reference.

    let q_load = daily_load_btu(inputs)?;

    let (ua_btu_hr_f, ef_out) = if let Some(ef) = inputs.energy_factor {
        // EF path -- Burch & Erickson 2004, §3.
        let ua = q_load * (1.0 / ef - 1.0) / ((T_SETPOINT_EF_F - T_ENV_F) * 24.0);
        (ua, ef)
    } else if let Some(uef) = inputs.uniform_energy_factor {
        // UEF path -- Maguire & Roberts 2020.
        let t = T_SETPOINT_UEF_F;
        let t_in = T_IN_UEF_F;
        let denominator = (24.0 * (t - T_ENV_F)) * (0.8 + 0.2 * ((t_in - T_ENV_F) / (t - T_ENV_F)));
        let ua = q_load * (1.0 / uef - 1.0) / denominator;
        // RESNET EF←UEF regression.
        let ef_equiv = 2.4029 * uef - 1.2844;
        (ua, ef_equiv)
    } else {
        return Err("Electric storage WH requires EnergyFactor or UniformEnergyFactor".to_string());
    };

    validate_ua(ua_btu_hr_f)?;
    Ok(UaResult {
        ua_w_per_k: conv::btu_hr_per_f_to_w_per_k(ua_btu_hr_f),
        conversion_efficiency: 1.0,
        energy_factor: Some(ef_out),
    })
}

fn ua_for_gas_storage(inputs: &UaInputs) -> Result<UaResult, String> {
    let q_load = daily_load_btu(inputs)?;

    // Default recovery efficiency for gas storage.
    let re = inputs.recovery_efficiency.unwrap_or(0.78);

    let heating_capacity = inputs
        .heating_capacity_btu_hr
        .ok_or_else(|| "Gas storage WH EF→UA requires HeatingCapacity (Btu/hr)".to_string())?;

    let (ua_btu_hr_f, eta_c, ef_out) = if let Some(ef) = inputs.energy_factor {
        // EF path -- OCHRE gas formula.
        let t = T_SETPOINT_EF_F;
        let ua =
            (re / ef - 1.0) / ((t - T_ENV_F) * (24.0 / q_load - 1.0 / (heating_capacity * ef)));
        let eta_c = re + ua * (t - T_ENV_F) / heating_capacity;
        (ua, eta_c, ef)
    } else if let Some(uef) = inputs.uniform_energy_factor {
        // UEF path.
        let t = T_SETPOINT_UEF_F;
        let ua = ((re / uef) - 1.0)
            / ((t - T_ENV_F) * (24.0 / q_load) - ((t - T_ENV_F) / (heating_capacity * uef)));
        let eta_c = re + (ua * (t - T_ENV_F)) / heating_capacity;
        let ef_equiv = UEF_TO_EF_GAS_SLOPE * uef + UEF_TO_EF_GAS_INTERCEPT;
        (ua, eta_c, ef_equiv)
    } else {
        return Err("Gas storage WH requires EnergyFactor or UniformEnergyFactor".to_string());
    };

    validate_ua(ua_btu_hr_f)?;
    if eta_c > 1.0 {
        return Err(format!(
            "Gas WH conversion efficiency {eta_c:.4} > 1.0; check EF/RE/capacity inputs"
        ));
    }

    Ok(UaResult {
        ua_w_per_k: conv::btu_hr_per_f_to_w_per_k(ua_btu_hr_f),
        conversion_efficiency: eta_c,
        energy_factor: Some(ef_out),
    })
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Compute the DOE daily load (Btu/day) from the draw volume bin.
///
/// For EF tests the draw volume is fixed at 64.3 gal/day; for UEF tests
/// it depends on the first-hour rating.
fn daily_load_btu(inputs: &UaInputs) -> Result<f64, String> {
    let (volume_drawn, t_setpoint) = if inputs.energy_factor.is_some() {
        (VOLUME_DRAWN_EF_GAL, T_SETPOINT_EF_F)
    } else {
        // UEF draw bin (DOE 10 CFR 430 Appendix E).
        let fhr = inputs.first_hour_rating_gal.unwrap_or(55.0);
        let bin_gal = if fhr < 18.0 {
            10.0_f64
        } else if fhr < 51.0 {
            38.0_f64
        } else if fhr < 75.0 {
            55.0_f64
        } else {
            84.0_f64
        };
        (bin_gal, T_SETPOINT_UEF_F)
    };

    let t_in = T_IN_EF_F; // inlet is the same for both test procedures.
    let draw_mass_lb = volume_drawn * DENSITY_LB_PER_GAL;
    let q_load = draw_mass_lb * CP_BTU_LB_F * (t_setpoint - t_in);
    Ok(q_load)
}

fn validate_ua(ua_btu_hr_f: f64) -> Result<(), String> {
    if ua_btu_hr_f < 0.0 {
        return Err(format!(
            "Computed water heater UA is negative ({ua_btu_hr_f:.4} Btu/hr·°F); \
             check EF/UEF and capacity inputs"
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const TOLERANCE: f64 = 0.01; // W/K, ~0.5% relative for typical UA values

    fn electric_inputs(ef: Option<f64>, uef: Option<f64>, vol_gal: f64) -> UaInputs {
        UaInputs {
            category: WhCategory::StorageElectric,
            energy_factor: ef,
            uniform_energy_factor: uef,
            tank_volume_rated_gal: Some(vol_gal),
            recovery_efficiency: None,
            heating_capacity_btu_hr: None,
            first_hour_rating_gal: None,
        }
    }

    fn gas_inputs(
        ef: Option<f64>,
        uef: Option<f64>,
        vol_gal: f64,
        cap_btu_hr: f64,
        re: f64,
    ) -> UaInputs {
        UaInputs {
            category: WhCategory::StorageGas,
            energy_factor: ef,
            uniform_energy_factor: uef,
            tank_volume_rated_gal: Some(vol_gal),
            recovery_efficiency: Some(re),
            heating_capacity_btu_hr: Some(cap_btu_hr),
            first_hour_rating_gal: None,
        }
    }

    fn hpwh_inputs(uef: f64, vol_gal: f64) -> UaInputs {
        UaInputs {
            category: WhCategory::HeatPump,
            energy_factor: None,
            uniform_energy_factor: Some(uef),
            tank_volume_rated_gal: Some(vol_gal),
            recovery_efficiency: None,
            heating_capacity_btu_hr: None,
            first_hour_rating_gal: Some(55.0),
        }
    }

    /// Electric 50-gal, EF=0.92 -- matches OCHRE `parse_water_heater` output.
    ///
    /// OCHRE gives ua = 2.2057 Btu/hr·°F → 1.1636 W/K.
    #[test]
    fn electric_ef_50gal_matches_ochre() {
        let result = ua_from_energy_factor(&electric_inputs(Some(0.92), None, 50.0))
            .expect("no error")
            .expect("some result");

        // Expected: q_load = 64.3 gal × 8.2938 lb/gal × 1.0007 Btu/lb·°F × (135-58) °F
        //                   = 41092.2 Btu/day
        //           ua = 41092.2 × (1/0.92 - 1) / ((135-67.5) × 24)
        //              ≈ 2.2057 Btu/hr·°F → 1.1636 W/K
        let expected_ua = 1.163_568;
        assert!(
            (result.ua_w_per_k - expected_ua).abs() < TOLERANCE,
            "50-gal electric EF=0.92: ua={:.4} W/K, expected≈{expected_ua:.4}",
            result.ua_w_per_k
        );
        assert_eq!(result.conversion_efficiency, 1.0);
        assert_eq!(result.energy_factor, Some(0.92));
    }

    /// Electric 40-gal, EF=0.95 -- higher EF → lower standby UA.
    #[test]
    fn electric_ef_40gal_higher_ef_gives_lower_ua() {
        let r50 = ua_from_energy_factor(&electric_inputs(Some(0.90), None, 50.0))
            .unwrap()
            .unwrap();
        let r40 = ua_from_energy_factor(&electric_inputs(Some(0.95), None, 40.0))
            .unwrap()
            .unwrap();
        assert!(
            r40.ua_w_per_k < r50.ua_w_per_k,
            "Higher EF must produce lower UA (better insulation): EF=0.95 ua={:.4} vs EF=0.90 ua={:.4}",
            r40.ua_w_per_k,
            r50.ua_w_per_k
        );
    }

    /// Gas 50-gal, EF=0.59, RE=0.78, cap=40 000 Btu/hr -- matches OCHRE output.
    ///
    /// OCHRE gives ua ≈ 8.8076 Btu/hr·°F → 4.6462 W/K.
    #[test]
    fn gas_ef_50gal_matches_ochre() {
        let result = ua_from_energy_factor(&gas_inputs(Some(0.59), None, 50.0, 40_000.0, 0.78))
            .expect("no error")
            .expect("some result");

        let expected_ua = 4.646_228;
        assert!(
            (result.ua_w_per_k - expected_ua).abs() < TOLERANCE,
            "50-gal gas EF=0.59 RE=0.78 40kBtu/hr: ua={:.4} W/K, expected≈{expected_ua:.4}",
            result.ua_w_per_k
        );
        assert!(result.conversion_efficiency > 0.78, "eta_c must be > RE");
        assert!(result.conversion_efficiency <= 1.0, "eta_c must be ≤ 1.0");
    }

    /// HPWH, UEF=3.45, 50-gal -- volume bin ≤58 gal → ua = 3.6 Btu/hr·°F.
    #[test]
    fn hpwh_uef_50gal_volume_bin() {
        let result = ua_from_energy_factor(&hpwh_inputs(3.45, 50.0))
            .expect("no error")
            .expect("some result");

        // 50 gal × 0.9 = 45 gal actual → bin ≤ 58 gal → ua = 3.6 Btu/hr·°F
        let expected_ua = conv::btu_hr_per_f_to_w_per_k(3.6);
        assert!(
            (result.ua_w_per_k - expected_ua).abs() < 1e-9,
            "HPWH 50-gal UEF=3.45: ua={:.4} W/K, expected {expected_ua:.4}",
            result.ua_w_per_k
        );
        assert!(
            result.ua_w_per_k > 0.5,
            "HPWH UA must be positive and physically reasonable"
        );
    }

    /// HPWH, large tank (85 gal rated) → actual 76.5 gal → highest UA bin.
    ///
    /// 85 gal × 0.9 = 76.5 gal actual.  76.5 > 73 → bin = 4.7 Btu/hr·°F.
    #[test]
    fn hpwh_large_tank_highest_bin() {
        let result = ua_from_energy_factor(&hpwh_inputs(3.45, 85.0))
            .unwrap()
            .unwrap();
        let expected = conv::btu_hr_per_f_to_w_per_k(4.7);
        assert!(
            (result.ua_w_per_k - expected).abs() < 1e-9,
            "85-gal HPWH (76.5 gal actual) ua={:.4}, expected {expected:.4}",
            result.ua_w_per_k
        );
    }

    /// Instantaneous water heater returns None (no tank).
    #[test]
    fn instantaneous_returns_none() {
        let inputs = UaInputs {
            category: WhCategory::Instantaneous,
            energy_factor: Some(0.82),
            uniform_energy_factor: None,
            tank_volume_rated_gal: None,
            recovery_efficiency: None,
            heating_capacity_btu_hr: None,
            first_hour_rating_gal: None,
        };
        assert!(
            ua_from_energy_factor(&inputs).unwrap().is_none(),
            "Instantaneous WH must return None UA"
        );
    }

    /// Missing EF and UEF for storage WH returns an error.
    #[test]
    fn missing_ef_and_uef_errors() {
        let inputs = electric_inputs(None, None, 50.0);
        assert!(ua_from_energy_factor(&inputs).is_err());
    }

    /// Negative UA is rejected.
    #[test]
    fn negative_ua_is_rejected() {
        // EF > 1.0 would produce a negative UA for electric storage.
        let inputs = electric_inputs(Some(1.5), None, 50.0);
        assert!(
            ua_from_energy_factor(&inputs).is_err(),
            "EF > 1.0 must produce a negative UA error"
        );
    }
}
