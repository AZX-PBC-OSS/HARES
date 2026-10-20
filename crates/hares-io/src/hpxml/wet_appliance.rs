//! Wet-appliance annual energy and daily hot-water volume.
//!
//! Implements OpenStudio-HPXML v1.12.0's
//! `calc_clothes_washer_energy_gpd` and `calc_dishwasher_energy_gpd`
//! (`HPXMLtoOpenStudio/resources/hotwater_appliances.rb:824-888` and
//! `:607-675`, fetched from the v1.12.0 tag) across the ERI versions the
//! reference branches on (its `Constants::ERIVersions`,
//! `resources/constants.rb:117-119`):
//!
//! - `NumberofResidents` given and ERI 2019A or later: the operational
//!   cycles form (`scy = 123 + 61 n_occ` washer, `scy = 91 + 30 n_occ`
//!   dishwasher, FSEC PF-464-15 Eqs. 1 and 3).
//! - `NumberofResidents` absent, ERI `latest`: the RESNET HERS Addendum 81
//!   asset form (Eq. 4.2-34 and Eq. 4.2-36a, split by residential facility
//!   type).
//! - `NumberofResidents` absent, ERI 2019A through 2022CE: the older asset
//!   form (`scy = 164 + 46.5 nbeds` washer, `scy = 88.4 + 34.9 nbeds`
//!   dishwasher).
//! - ERI before 2019A: the pre-2019 label algebra (washer) and the
//!   energy-factor form (dishwasher), with the pre-2014A volume
//!   adjustments each branch carries.
//!
//! `UsageMultiplier` scales the annual energy and the daily volume alike;
//! a negative result is clamped at 0; a home with `NumberofResidents` 0
//! has no appliance energy or hot water (every version).
//!
//! The formulas run in their native units (kWh per year, gal per day); the
//! caller converts the volume to SI.

use crate::hpxml::HpxmlError;

/// The ANSI/RESNET/ICC 301 version the cycles equations branch on, from the
/// HPXML `SoftwareInfo/extension/ERICalculation/Version` string (OpenStudio-HPXML
/// `Constants::ERIVersions`, `resources/constants.rb:117-119`; the
/// comparisons are its `index(eri_version) >= index('2019A' | 'latest')`
/// and `< '2014A'` rank comparisons).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EriVersion {
    /// Before 2014A: the pre-2014A volume adjustments apply.
    Pre2014,
    /// 2014A through 2019: the pre-2019 label algebra, no pre-2014A
    /// adjustments.
    Y2014To2019,
    /// 2019A through 2022CE: the modern formulas with the older asset
    /// cycles when `NumberofResidents` is absent.
    Y2019To2022,
    /// `latest` (the default when the HPXML names no version): the modern
    /// formulas with the Addendum 81 asset cycles.
    Latest,
}

/// The versions' rank order, OpenStudio-HPXML's `Constants::ERIVersions`.
const ERI_VERSION_RANKS: &[&str] = &[
    "2014", "2014A", "2014AE", "2014AEG", "2019", "2019A", "2019AB", "2019ABC", "2019ABCD", "2022",
    "2022C", "2022CE", "latest",
];

impl EriVersion {
    /// The version a raw HPXML value names; an unknown spelling is a
    /// validation error naming it.
    pub(super) fn parse(raw: &str) -> Result<Self, HpxmlError> {
        let rank = ERI_VERSION_RANKS
            .iter()
            .position(|v| *v == raw)
            .ok_or_else(|| HpxmlError::InvalidField {
                path: "SoftwareInfo/extension/ERICalculation/Version",
                system_kind: "ERI Calculation",
                system_id: raw.to_string(),
                value_received: raw.to_string(),
                reason: "the version is not one the reference knows: 2014, 2014A, 2014AE, \
                         2014AEG, 2019, 2019A, 2019AB, 2019ABC, 2019ABCD, 2022, 2022C, \
                         2022CE, latest",
            })?;
        let latest = ERI_VERSION_RANKS.len() - 1;
        let rank_2019a = ERI_VERSION_RANKS
            .iter()
            .position(|v| *v == "2019A")
            .expect("2019A is in the version list");
        let rank_2014a = ERI_VERSION_RANKS
            .iter()
            .position(|v| *v == "2014A")
            .expect("2014A is in the version list");
        Ok(if rank >= latest {
            Self::Latest
        } else if rank >= rank_2019a {
            Self::Y2019To2022
        } else if rank >= rank_2014a {
            Self::Y2014To2019
        } else {
            Self::Pre2014
        })
    }

    /// The version when the HPXML names none: OpenStudio-HPXML's default
    /// (`defaults.rb:31`, `eri_version = 'latest'`).
    pub(super) fn latest() -> Self {
        Self::Latest
    }
}

/// The occupancy inputs the cycles-per-year equations read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct OccupancyInputs {
    /// `NumberofResidents` when given; the operational branch (2019A and
    /// later) uses it directly and `Some(0.0)` zeroes both appliances.
    pub n_occ: Option<f64>,
    /// The raw `NumberofBedrooms` (the asset branch's input), with the
    /// parser's fallback applied.
    pub nbeds: f64,
    /// `ResidentialFacilityType` is `apartment unit`
    /// (HPXML `ResidentialTypeApartment`).
    pub is_apartment: bool,
    /// The ERI version the cycles equations branch on.
    pub eri_version: EriVersion,
}

/// Clothes washer inputs, with the OpenStudio-HPXML defaults
/// (`resources/defaults.rb:5586-5594` `get_clothes_washer_values`) applied by
/// the caller for every field the HPXML omits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ClothesWasherInputs {
    pub rated_annual_kwh: f64,
    pub capacity_ft3: f64,
    pub label_usage: f64,
    pub label_electric_rate: f64,
    pub label_gas_rate: f64,
    pub label_annual_gas_cost: f64,
    pub usage_multiplier: f64,
    pub occupancy: OccupancyInputs,
}

/// Dishwasher inputs, with the OpenStudio-HPXML defaults
/// (`resources/defaults.rb:5523-5530` `get_dishwasher_values`) applied by
/// the caller for every field the HPXML omits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct DishwasherInputs {
    pub rated_annual_kwh: f64,
    pub place_setting_capacity: f64,
    /// The HPXML `EnergyFactor`: the pre-2019A branch's input, falling back
    /// to 215 / rated annual kWh when absent.
    pub energy_factor: Option<f64>,
    pub label_usage: f64,
    pub label_electric_rate: f64,
    pub label_gas_rate: f64,
    pub label_annual_gas_cost: f64,
    pub usage_multiplier: f64,
    pub occupancy: OccupancyInputs,
}

/// The washer's annual energy [kWh/yr] and daily hot water [gal/day].
///
/// `adjusted_annual_kwh` is the HARES-internal `extension/AdjustedAnnualkWh`
/// (or an explicit `Load` energy): not an OpenStudio-HPXML input, whose
/// parity claim does not extend to it. The reference always runs the label
/// algebra; HARES uses the adjusted value where its own pipeline has
/// already resolved the appliance's energy, tested in
/// `wet_appliance_hot_water.rs`. The adjusted annual energy already
/// carries the usage adjustment, so it is the result, never the rated
/// input of a second adjustment. The hot-water formula needs the
/// label-cycle energy the adjustment algebra would have produced, so it is
/// back-derived from the adjusted value (`cw_appl = adjusted * lcy / acy`)
/// and the rated value stays the rated one.
pub(super) fn calc_clothes_washer_energy_gpd(
    inputs: &ClothesWasherInputs,
    adjusted_annual_kwh: Option<f64>,
) -> (f64, f64) {
    let occupancy = inputs.occupancy;
    if occupancy.n_occ == Some(0.0) {
        // Operational calculation with zero occupants: zero out energy use.
        return (0.0, 0.0);
    }
    let modern = matches!(
        occupancy.eri_version,
        EriVersion::Y2019To2022 | EriVersion::Latest
    );
    let (annual_kwh, gpd, scy, acy) = if modern {
        let gas_h20 = 0.3914; // (gal/cyc) per (therm/y)
        let elec_h20 = 0.0178; // (gal/cyc) per (kWh/y)
        let lcy = inputs.label_usage * 52.0; // label cycles per year
        let scy = washer_cycles_per_year(occupancy);
        let acy = scy * ((3.0 * 2.08 + 1.59) / (inputs.capacity_ft3 * 2.08 + 1.59));
        let cw_appl_algebraic = (inputs.label_annual_gas_cost * gas_h20 / inputs.label_gas_rate
            - (inputs.rated_annual_kwh * inputs.label_electric_rate) * elec_h20
                / inputs.label_electric_rate)
            / (inputs.label_electric_rate * gas_h20 / inputs.label_gas_rate - elec_h20);
        let (annual_kwh, cw_appl) = match adjusted_annual_kwh {
            Some(adjusted) => (adjusted, adjusted * lcy / acy),
            None => (cw_appl_algebraic / lcy * acy, cw_appl_algebraic),
        };
        let gpd = (inputs.rated_annual_kwh - cw_appl) * elec_h20 * acy / 365.0;
        (annual_kwh, gpd, scy, acy)
    } else {
        // Pre-2019A (hotwater_appliances.rb:859-866): the pre-2019 label
        // algebra; `NumberofResidents` plays no part, and the pre-2014A
        // versions subtract the 3.97 gal/day hot-water reduction
        // (ANSI/RESNET 301 Section 4.2.2.5.2.10).
        let ncy = (3.0 / 2.874) * (164.0 + occupancy.nbeds * 46.5);
        let acy = ncy * ((3.0 * 2.08 + 1.59) / (inputs.capacity_ft3 * 2.08 + 1.59));
        let water_term = (inputs.rated_annual_kwh * inputs.label_electric_rate
            - inputs.label_annual_gas_cost)
            / (21.9825 * inputs.label_electric_rate - inputs.label_gas_rate);
        let annual_kwh = (inputs.rated_annual_kwh / 392.0 - water_term / 392.0 * 21.9825) * acy;
        let mut gpd = 60.0 * (water_term / 392.0) * acy / 365.0;
        if matches!(occupancy.eri_version, EriVersion::Pre2014) {
            gpd -= 3.97;
        }
        // The pre-2019 form has no resident-count cycles: the logged
        // cycles are the adjusted-cycles base at the reference's default
        // capacity.
        (annual_kwh, gpd, ncy, acy)
    };
    let annual_kwh = (annual_kwh * inputs.usage_multiplier).max(0.0);
    let gpd = (gpd * inputs.usage_multiplier).max(0.0);
    tracing::debug!(
        appliance = "Clothes Washer",
        rated_annual_kwh = inputs.rated_annual_kwh,
        capacity_ft3 = inputs.capacity_ft3,
        label_usage = inputs.label_usage,
        label_electric_rate = inputs.label_electric_rate,
        label_gas_rate = inputs.label_gas_rate,
        label_annual_gas_cost = inputs.label_annual_gas_cost,
        usage_multiplier = inputs.usage_multiplier,
        adjusted_annual_kwh = adjusted_annual_kwh,
        n_occupants = occupancy.n_occ,
        n_bedrooms = occupancy.nbeds,
        is_apartment = occupancy.is_apartment,
        standard_cycles_per_year = scy,
        annual_cycles = acy,
        annual_electric_kwh = annual_kwh,
        daily_hot_water_gal = gpd,
        "wet appliance energy and hot water resolved by the OpenStudio-HPXML cycles rule",
    );
    (annual_kwh, gpd)
}

/// The dishwasher's annual energy [kWh/yr] and daily hot water [gal/day].
///
/// `adjusted_annual_kwh` is the HARES-internal `extension/AdjustedAnnualkWh`
/// (or an explicit `Load` energy): not an OpenStudio-HPXML input, whose
/// parity claim does not extend to it. The reference always runs the label
/// algebra; HARES uses the adjusted value where its own pipeline has
/// already resolved the appliance's energy, tested in
/// `wet_appliance_hot_water.rs`. The adjusted annual energy already
/// carries the usage adjustment, so it is the result, never the rated
/// input of a second adjustment. The hot-water formula needs the
/// per-cycle energy the adjustment algebra would have produced, so it is
/// back-derived from the adjusted value (`kwh_per_cyc = adjusted / dwcpy`)
/// and the rated value stays the rated one.
pub(super) fn calc_dishwasher_energy_gpd(
    inputs: &DishwasherInputs,
    adjusted_annual_kwh: Option<f64>,
) -> (f64, f64) {
    let occupancy = inputs.occupancy;
    if occupancy.n_occ == Some(0.0) {
        // Operational calculation with zero occupants: zero out energy use.
        return (0.0, 0.0);
    }
    let modern = matches!(
        occupancy.eri_version,
        EriVersion::Y2019To2022 | EriVersion::Latest
    );
    let (annual_kwh, gpd, scy, dwcpy) = if modern {
        let lcy = inputs.label_usage * 52.0;
        let kwh_per_cyc_algebraic = ((inputs.label_annual_gas_cost * 0.5497
            / inputs.label_gas_rate
            - inputs.rated_annual_kwh * inputs.label_electric_rate * 0.02504
                / inputs.label_electric_rate)
            / (inputs.label_electric_rate * 0.5497 / inputs.label_gas_rate - 0.02504))
            / lcy;
        let scy = dishwasher_cycles_per_year(occupancy);
        let dwcpy = scy * (12.0 / inputs.place_setting_capacity);
        let (annual_kwh, kwh_per_cyc) = match adjusted_annual_kwh {
            Some(adjusted) => (adjusted, adjusted / dwcpy),
            None => (kwh_per_cyc_algebraic * dwcpy, kwh_per_cyc_algebraic),
        };
        let gpd = (inputs.rated_annual_kwh - kwh_per_cyc * lcy) * 0.02504 * dwcpy / 365.0;
        (annual_kwh, gpd, scy, dwcpy)
    } else {
        // Pre-2019A (hotwater_appliances.rb:651-664): the energy-factor
        // form; the energy factor falls back to 215 / rated annual kWh
        // (`calc_dishwasher_ef_from_annual_kwh`), and the pre-2014A
        // versions use the 8.16 form.
        let ef = inputs
            .energy_factor
            .unwrap_or(215.0 / inputs.rated_annual_kwh);
        let scy = 88.4 + 34.9 * occupancy.nbeds;
        let dwcpy = scy * (12.0 / inputs.place_setting_capacity);
        let annual_kwh = ((86.3 + 47.73 / ef) / 215.0) * dwcpy;
        let gpd = if matches!(occupancy.eri_version, EriVersion::Pre2014) {
            ((88.4 + 34.9 * occupancy.nbeds) * 8.16
                - (88.4 + 34.9 * occupancy.nbeds) * 12.0 / inputs.place_setting_capacity
                    * (4.6415 * (1.0 / ef) - 1.9295))
                / 365.0
        } else {
            dwcpy * (4.6415 * (1.0 / ef) - 1.9295) / 365.0
        };
        (annual_kwh, gpd, scy, dwcpy)
    };
    let annual_kwh = (annual_kwh * inputs.usage_multiplier).max(0.0);
    let gpd = (gpd * inputs.usage_multiplier).max(0.0);
    tracing::debug!(
        appliance = "Dishwasher",
        rated_annual_kwh = inputs.rated_annual_kwh,
        place_setting_capacity = inputs.place_setting_capacity,
        label_usage = inputs.label_usage,
        label_electric_rate = inputs.label_electric_rate,
        label_gas_rate = inputs.label_gas_rate,
        label_annual_gas_cost = inputs.label_annual_gas_cost,
        usage_multiplier = inputs.usage_multiplier,
        adjusted_annual_kwh = adjusted_annual_kwh,
        n_occupants = occupancy.n_occ,
        n_bedrooms = occupancy.nbeds,
        is_apartment = occupancy.is_apartment,
        standard_cycles_per_year = scy,
        annual_cycles = dwcpy,
        annual_electric_kwh = annual_kwh,
        daily_hot_water_gal = gpd,
        "wet appliance energy and hot water resolved by the OpenStudio-HPXML cycles rule",
    );
    (annual_kwh, gpd)
}

/// The washer's standard cycles per year: the operational form
/// (`123 + 61 n_occ`, FSEC PF-464-15 Eq. 1) when `NumberofResidents` is
/// given; the asset form otherwise, split by ERI version: the RESNET HERS
/// Addendum 81 Eq. 4.2-34 cycles on `latest` (split by residential
/// facility type) and `scy = 164 + 46.5 nbeds` on 2019A through 2022CE
/// (`hotwater_appliances.rb:836-850`).
fn washer_cycles_per_year(occupancy: OccupancyInputs) -> f64 {
    match occupancy.n_occ {
        Some(n_occ) => 123.0 + 61.0 * n_occ,
        None if occupancy.eri_version == EriVersion::Latest => {
            if occupancy.is_apartment {
                213.9 + 27.5 * occupancy.nbeds
            } else {
                189.5 + 32.9 * occupancy.nbeds
            }
        }
        None => 164.0 + occupancy.nbeds * 46.5,
    }
}

/// The dishwasher's standard cycles per year: the operational form
/// (`91 + 30 n_occ`, FSEC PF-464-15 Eq. 3) when `NumberofResidents` is
/// given; the asset form otherwise, split by ERI version: the RESNET HERS
/// Addendum 81 Eq. 4.2-36a cycles on `latest` (split by residential
/// facility type) and `scy = 88.4 + 34.9 nbeds` on 2019A through 2022CE
/// (`hotwater_appliances.rb:626-641`).
fn dishwasher_cycles_per_year(occupancy: OccupancyInputs) -> f64 {
    match occupancy.n_occ {
        Some(n_occ) => 91.0 + 30.0 * n_occ,
        None if occupancy.eri_version == EriVersion::Latest => {
            if occupancy.is_apartment {
                135.7 + 13.5 * occupancy.nbeds
            } else {
                123.7 + 16.2 * occupancy.nbeds
            }
        }
        None => 88.4 + 34.9 * occupancy.nbeds,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn occupancy(n_occ: Option<f64>) -> OccupancyInputs {
        OccupancyInputs {
            n_occ,
            nbeds: 3.0,
            is_apartment: false,
            eri_version: EriVersion::latest(),
        }
    }

    fn washer_inputs(n_occ: Option<f64>) -> ClothesWasherInputs {
        ClothesWasherInputs {
            rated_annual_kwh: 284.0,
            capacity_ft3: 4.2,
            label_usage: 6.0,
            label_electric_rate: 0.12,
            label_gas_rate: 1.09,
            label_annual_gas_cost: 18.0,
            usage_multiplier: 1.0,
            occupancy: occupancy(n_occ),
        }
    }

    fn dishwasher_inputs(n_occ: Option<f64>) -> DishwasherInputs {
        DishwasherInputs {
            rated_annual_kwh: 307.0,
            place_setting_capacity: 12.0,
            energy_factor: None,
            label_usage: 4.0,
            label_electric_rate: 0.12,
            label_gas_rate: 1.09,
            label_annual_gas_cost: 22.32,
            usage_multiplier: 1.0,
            occupancy: occupancy(n_occ),
        }
    }

    #[test]
    fn washer_operational_branch_uses_the_resident_cycles() {
        // The cz4a parity building's inputs with 3 residents.
        let (annual_kwh, gpd) = calc_clothes_washer_energy_gpd(&washer_inputs(Some(3.0)), None);
        assert!((annual_kwh - 41.413_330_225_392_19).abs() < 1e-9);
        assert!((gpd - 2.583_516_976_044_368_4).abs() < 1e-12);
        // The cycles rule is the resident form: 123 + 61 * n_occ, so a second
        // resident adds 61 adjusted cycles, not the asset form's slope.
        let (annual_4, _) = calc_clothes_washer_energy_gpd(&washer_inputs(Some(4.0)), None);
        let scy3 = 123.0 + 61.0 * 3.0;
        let acy3 = scy3 * ((3.0 * 2.08 + 1.59) / (4.2 * 2.08 + 1.59));
        let scy4 = 123.0 + 61.0 * 4.0;
        let acy4 = scy4 * ((3.0 * 2.08 + 1.59) / (4.2 * 2.08 + 1.59));
        assert!((annual_4 / annual_kwh - acy4 / acy3).abs() < 1e-12);
    }

    #[test]
    fn washer_asset_branch_uses_the_addendum_81_equations() {
        let mut inputs = washer_inputs(None);
        inputs.occupancy = OccupancyInputs {
            n_occ: None,
            nbeds: 3.0,
            is_apartment: false,
            eri_version: EriVersion::latest(),
        };
        let (_, gpd_detached) = calc_clothes_washer_energy_gpd(&inputs, None);
        inputs.occupancy.is_apartment = true;
        let (_, gpd_apartment) = calc_clothes_washer_energy_gpd(&inputs, None);
        // Addendum 81 Eq. 4.2-34: apartment 213.9 + 27.5 nbeds, detached
        // 189.5 + 32.9 nbeds.
        let ratio = (213.9 + 27.5 * 3.0) / (189.5 + 32.9 * 3.0);
        assert!((gpd_apartment / gpd_detached - ratio).abs() < 1e-12);
    }

    #[test]
    fn washer_asset_branch_uses_the_older_cycles_before_latest() {
        // ERI 2019A through 2022CE without NumberofResidents:
        // scy = 164 + 46.5 nbeds (hotwater_appliances.rb:846-847).
        let mut inputs = washer_inputs(None);
        inputs.occupancy.eri_version = EriVersion::Y2019To2022;
        let (annual, gpd) = calc_clothes_washer_energy_gpd(&inputs, None);
        let scy = 164.0 + 3.0 * 46.5;
        let acy = scy * ((3.0 * 2.08 + 1.59) / (4.2 * 2.08 + 1.59));
        let lcy = 6.0 * 52.0;
        let cw_appl = (inputs.label_annual_gas_cost * 0.3914 / inputs.label_gas_rate
            - inputs.rated_annual_kwh * 0.0178)
            / (inputs.label_electric_rate * 0.3914 / inputs.label_gas_rate - 0.0178);
        assert!(
            (annual - (cw_appl / lcy * acy)).abs() < 1e-9,
            "got {annual}"
        );
        assert!((gpd - (inputs.rated_annual_kwh - cw_appl) * 0.0178 * acy / 365.0).abs() < 1e-12);
        // The Addendum 81 cycles differ: the same inputs on latest are not
        // the older branch.
        let mut latest = washer_inputs(None);
        latest.occupancy.eri_version = EriVersion::Latest;
        let (latest_annual, _) = calc_clothes_washer_energy_gpd(&latest, None);
        assert!((latest_annual - annual).abs() > 1.0);
    }

    #[test]
    fn washer_pre_2019_branch_uses_the_pre_2019_label_algebra() {
        // ERI 2014A without NumberofResidents: the pre-2019 algebra, where
        // the resident count plays no part (hotwater_appliances.rb:859-866).
        let mut inputs = washer_inputs(None);
        inputs.occupancy.eri_version = EriVersion::Y2014To2019;
        let (annual, gpd) = calc_clothes_washer_energy_gpd(&inputs, None);
        let ncy = (3.0 / 2.874) * (164.0 + 3.0 * 46.5);
        let acy = ncy * ((3.0 * 2.08 + 1.59) / (4.2 * 2.08 + 1.59));
        let water_term = (inputs.rated_annual_kwh * inputs.label_electric_rate
            - inputs.label_annual_gas_cost)
            / (21.9825 * inputs.label_electric_rate - inputs.label_gas_rate);
        assert!(
            (annual - (inputs.rated_annual_kwh / 392.0 - water_term / 392.0 * 21.9825) * acy).abs()
                < 1e-9,
            "got {annual}"
        );
        assert!(
            (gpd - 60.0 * (water_term / 392.0) * acy / 365.0).abs() < 1e-12,
            "got {gpd}"
        );
        // The same inputs with residents are the same result: n_occ plays
        // no part pre-2019.
        let mut with_residents = washer_inputs(Some(4.0));
        with_residents.occupancy.eri_version = EriVersion::Y2014To2019;
        let (annual_with, gpd_with) = calc_clothes_washer_energy_gpd(&with_residents, None);
        assert_eq!(annual, annual_with);
        assert_eq!(gpd, gpd_with);
    }

    #[test]
    fn washer_pre_2014_branch_subtracts_the_3_97_reduction() {
        // ANSI/RESNET 301 Section 4.2.2.5.2.10: gpd -= 3.97 before the
        // usage multiplier (hotwater_appliances.rb:868-870). Inputs whose
        // pre-2019 volume clears 3.97 so the subtraction does not clamp.
        let rated_larger = |version: EriVersion| {
            let mut inputs = washer_inputs(None);
            inputs.rated_annual_kwh = 800.0;
            inputs.label_annual_gas_cost = 0.0;
            inputs.occupancy.eri_version = version;
            inputs
        };
        let (_, gpd_pre_2014) =
            calc_clothes_washer_energy_gpd(&rated_larger(EriVersion::Pre2014), None);
        let (_, gpd_2014) =
            calc_clothes_washer_energy_gpd(&rated_larger(EriVersion::Y2014To2019), None);
        assert!((gpd_pre_2014 - (gpd_2014 - 3.97)).abs() < 1e-12);
    }

    #[test]
    fn dishwasher_operational_branch_uses_the_resident_cycles() {
        let (annual_kwh, gpd) = calc_dishwasher_energy_gpd(&dishwasher_inputs(Some(3.0)), None);
        assert!((annual_kwh - 87.539_693_548_156_11).abs() < 1e-9);
        assert!((gpd - 2.562_912_173_422_650_8).abs() < 1e-12);
    }

    #[test]
    fn dishwasher_asset_branch_uses_the_addendum_81_equations() {
        let mut inputs = dishwasher_inputs(None);
        inputs.occupancy = OccupancyInputs {
            n_occ: None,
            nbeds: 3.0,
            is_apartment: false,
            eri_version: EriVersion::latest(),
        };
        let (_, gpd_detached) = calc_dishwasher_energy_gpd(&inputs, None);
        inputs.occupancy.is_apartment = true;
        let (_, gpd_apartment) = calc_dishwasher_energy_gpd(&inputs, None);
        // Addendum 81 Eq. 4.2-36a: apartment 135.7 + 13.5 nbeds, detached
        // 123.7 + 16.2 nbeds.
        let ratio = (135.7 + 13.5 * 3.0) / (123.7 + 16.2 * 3.0);
        assert!((gpd_apartment / gpd_detached - ratio).abs() < 1e-12);
    }

    #[test]
    fn dishwasher_asset_branch_uses_the_older_cycles_before_latest() {
        // ERI 2019A through 2022CE without NumberofResidents:
        // scy = 88.4 + 34.9 nbeds (hotwater_appliances.rb:635-636).
        let mut inputs = dishwasher_inputs(None);
        inputs.occupancy.eri_version = EriVersion::Y2019To2022;
        let (annual, gpd) = calc_dishwasher_energy_gpd(&inputs, None);
        let dwcpy = (88.4 + 34.9 * 3.0) * (12.0 / 12.0);
        let lcy = 4.0 * 52.0;
        let kwh_per_cyc = (inputs.label_annual_gas_cost * 0.5497 / inputs.label_gas_rate
            - inputs.rated_annual_kwh * 0.02504)
            / (inputs.label_electric_rate * 0.5497 / inputs.label_gas_rate - 0.02504)
            / lcy;
        assert!((annual - kwh_per_cyc * dwcpy).abs() < 1e-9, "got {annual}");
        assert!(
            (gpd - (inputs.rated_annual_kwh - kwh_per_cyc * lcy) * 0.02504 * dwcpy / 365.0).abs()
                < 1e-12
        );
        // The Addendum 81 cycles differ: the same inputs on latest are not
        // the older branch.
        let mut latest = dishwasher_inputs(None);
        latest.occupancy.eri_version = EriVersion::Latest;
        let (latest_annual, _) = calc_dishwasher_energy_gpd(&latest, None);
        assert!((latest_annual - annual).abs() > 1.0);
    }

    #[test]
    fn dishwasher_pre_2019_branch_uses_the_energy_factor_form() {
        // ERI 2014A: the energy-factor form, the resident count playing no
        // part (hotwater_appliances.rb:651-664).
        let mut inputs = dishwasher_inputs(None);
        inputs.occupancy.eri_version = EriVersion::Y2014To2019;
        inputs.energy_factor = Some(0.5);
        let (annual, gpd) = calc_dishwasher_energy_gpd(&inputs, None);
        let dwcpy = (88.4 + 34.9 * 3.0) * (12.0 / 12.0);
        assert!(
            ((86.3 + 47.73 / 0.5) / 215.0 * dwcpy - annual).abs() < 1e-9,
            "got {annual}"
        );
        assert!(
            (dwcpy * (4.6415 * 2.0 - 1.9295) / 365.0 - gpd).abs() < 1e-12,
            "got {gpd}"
        );
        // With no EnergyFactor the fallback is 215 / rated annual kWh.
        let mut no_ef = dishwasher_inputs(None);
        no_ef.occupancy.eri_version = EriVersion::Y2014To2019;
        let (annual_no_ef, _) = calc_dishwasher_energy_gpd(&no_ef, None);
        let ef_fallback = 215.0 / no_ef.rated_annual_kwh;
        let dwcpy_no_ef = (88.4 + 34.9 * 3.0) * (12.0 / 12.0);
        assert!(((86.3 + 47.73 / ef_fallback) / 215.0 * dwcpy_no_ef - annual_no_ef).abs() < 1e-9);
    }

    #[test]
    fn dishwasher_pre_2014_branch_uses_the_8_16_form() {
        let mut inputs = dishwasher_inputs(None);
        inputs.occupancy.eri_version = EriVersion::Pre2014;
        inputs.energy_factor = Some(0.5);
        let (_, gpd) = calc_dishwasher_energy_gpd(&inputs, None);
        let cycles_base = 88.4 + 34.9 * 3.0;
        let expected =
            (cycles_base * 8.16 - cycles_base * (12.0 / 12.0) * (4.6415 * 2.0 - 1.9295)) / 365.0;
        assert!((gpd - expected).abs() < 1e-12, "got {gpd}");
    }

    #[test]
    fn usage_multiplier_scales_energy_and_volume() {
        let mut inputs = washer_inputs(Some(3.0));
        inputs.usage_multiplier = 1.2;
        let (annual, gpd) = calc_clothes_washer_energy_gpd(&inputs, None);
        let mut base = washer_inputs(Some(3.0));
        base.usage_multiplier = 1.0;
        let (annual_base, gpd_base) = calc_clothes_washer_energy_gpd(&base, None);
        assert!((annual / annual_base - 1.2).abs() < 1e-12);
        assert!((gpd / gpd_base - 1.2).abs() < 1e-12);

        let mut dw = dishwasher_inputs(Some(3.0));
        dw.usage_multiplier = 1.2;
        let (_, dw_gpd) = calc_dishwasher_energy_gpd(&dw, None);
        let mut dw_base = dishwasher_inputs(Some(3.0));
        dw_base.usage_multiplier = 1.0;
        let (_, dw_gpd_base) = calc_dishwasher_energy_gpd(&dw_base, None);
        assert!((dw_gpd / dw_gpd_base - 1.2).abs() < 1e-12);
    }

    #[test]
    fn zero_occupants_zero_both_appliances() {
        let (annual, gpd) = calc_clothes_washer_energy_gpd(&washer_inputs(Some(0.0)), None);
        assert_eq!(annual, 0.0);
        assert_eq!(gpd, 0.0);
        let (annual, gpd) = calc_dishwasher_energy_gpd(&dishwasher_inputs(Some(0.0)), None);
        assert_eq!(annual, 0.0);
        assert_eq!(gpd, 0.0);
    }

    #[test]
    fn negative_results_clamp_at_zero() {
        // A label gas cost that drives the algebra's appliance energy far
        // above the rated value makes the water term negative.
        let mut inputs = washer_inputs(Some(3.0));
        inputs.label_annual_gas_cost = 1000.0;
        inputs.rated_annual_kwh = 100.0;
        let (annual, gpd) = calc_clothes_washer_energy_gpd(&inputs, None);
        assert!(
            annual > 0.0,
            "the energy term alone stays positive: {annual}"
        );
        assert_eq!(gpd, 0.0);

        let mut dw = dishwasher_inputs(Some(3.0));
        dw.label_annual_gas_cost = 1000.0;
        dw.rated_annual_kwh = 100.0;
        let (dw_annual, dw_gpd) = calc_dishwasher_energy_gpd(&dw, None);
        assert!(
            dw_annual > 0.0,
            "the energy term alone stays positive: {dw_annual}"
        );
        assert_eq!(dw_gpd, 0.0);
    }

    #[test]
    fn reference_defaults_produce_the_reference_daily_volumes() {
        // The OpenStudio-HPXML defaults with 3 residents: the recorded
        // reference values for the two formula functions.
        let inputs = ClothesWasherInputs {
            rated_annual_kwh: 400.0,
            capacity_ft3: 3.0,
            label_usage: 6.0,
            label_electric_rate: 0.12,
            label_gas_rate: 1.09,
            label_annual_gas_cost: 27.0,
            usage_multiplier: 1.0,
            occupancy: occupancy(Some(3.0)),
        };
        let (_, gpd) = calc_clothes_washer_energy_gpd(&inputs, None);
        assert!((gpd - 4.449_538_086_922_645_5).abs() < 1e-12, "got {gpd}");

        let dw = DishwasherInputs {
            rated_annual_kwh: 467.0,
            place_setting_capacity: 12.0,
            energy_factor: None,
            label_usage: 4.0,
            label_electric_rate: 0.12,
            label_gas_rate: 1.09,
            label_annual_gas_cost: 33.12,
            usage_multiplier: 1.0,
            occupancy: occupancy(Some(3.0)),
        };
        let (_, dw_gpd) = calc_dishwasher_energy_gpd(&dw, None);
        assert!(
            (dw_gpd - 4.045_588_637_386_169).abs() < 1e-12,
            "got {dw_gpd}"
        );
    }

    #[test]
    fn adjusted_annual_kwh_is_the_energy_and_scales_the_water_term() {
        // The adjusted value is the result: the energy equals it, and the
        // water term back-derives the label-cycle energy from it.
        let base = washer_inputs(Some(3.0));
        let (annual, gpd) = calc_clothes_washer_energy_gpd(&base, Some(100.0));
        assert_eq!(annual, 100.0);
        let lcy = base.label_usage * 52.0;
        let scy = 123.0 + 61.0 * 3.0;
        let acy = scy * ((3.0 * 2.08 + 1.59) / (base.capacity_ft3 * 2.08 + 1.59));
        let cw_appl = 100.0 * lcy / acy;
        let expected_gpd = (base.rated_annual_kwh - cw_appl) * 0.0178 * acy / 365.0;
        assert!(
            (gpd - expected_gpd).abs() < 1e-12,
            "got {gpd}, want {expected_gpd}"
        );

        let dw_base = dishwasher_inputs(Some(3.0));
        let (dw_annual, dw_gpd) = calc_dishwasher_energy_gpd(&dw_base, Some(100.0));
        assert_eq!(dw_annual, 100.0);
        let dwcpy = (91.0 + 30.0 * 3.0) * (12.0 / dw_base.place_setting_capacity);
        let kwh_per_cyc = 100.0 / dwcpy;
        let expected_dw_gpd =
            (dw_base.rated_annual_kwh - kwh_per_cyc * lcy_of(&dw_base)) * 0.02504 * dwcpy / 365.0;
        assert!(
            (dw_gpd - expected_dw_gpd).abs() < 1e-12,
            "got {dw_gpd}, want {expected_dw_gpd}"
        );
    }

    fn lcy_of(inputs: &DishwasherInputs) -> f64 {
        inputs.label_usage * 52.0
    }
}
