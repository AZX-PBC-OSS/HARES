//! The ASHRAE 140 reference bands the BESTEST cases validate against.
//!
//! Edition: ANSI/ASHRAE 140-2023, the current edition (the 140-2020 to
//! 140-2023 revision is editorial renumbering; the fabric tests are
//! Section 7 in this edition, Section 5.2 in 140-2017 and 140-2020). The
//! nine bands are the current edition's Annex B8 example results, by
//! table: B8-1 annual heating loads, B8-2 annual sensible cooling loads,
//! B8-5 free-float temperature output. The values are converted from the
//! report's MWh to kWh.
//!
//! Provenance: the public example-results report Std140_TF_Results.pdf
//! (134 pages, "ASHRAE Standard 140-2023 Test Results Comparison for
//! Section 7", Thermal Energy System Specialists, 19-Aug-2024), listed on
//! ASHRAE's Standard 140 Section 7 accompanying-files page
//! (<https://data.ashrae.org/standard140/AccompanyingSection7.html>) and
//! hosted in full by the TRNSYS project
//! (<https://trnsys.org/ASHRAE140/18_06_0002/Std140_TF_Results.pdf>),
//! sha256 ebebff90bba76ee4971eb40e32fc0cc22a858abb311000ea833dc1dfdcde510e.
//! Per data.ashrae.org, the 140-2023 file content is unchanged from
//! 140-2020 beyond folder and file names, section numbering and headers.
//!
//! The standard's normative acceptance ranges (Normative Annex A3,
//! "Software Acceptance Criteria", added by 140-2020 Addendum b) are
//! UNVERIFIABLE-WEB until the standard's pages are supplied. Until then
//! these informative Annex B8 example-result bands are the pinned
//! contract, and the upgrade path (pinning the A3 ranges) stays open.
//!
//! The bands are the target, not the tests' whole pass condition:
//! out-of-band metrics are drift-locked at measured baselines
//! (`super::drift_baseline`), and the signed distance to the band prints
//! on every run.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BestestMetric {
    PeakZoneTempC,
    MinZoneTempC,
    AnnualHeatingLoadKwh,
    AnnualCoolingLoadKwh,
    AnnualHeatingEnergyKwh,
}

impl fmt::Display for BestestMetric {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PeakZoneTempC => write!(f, "peak_zone_temp_c"),
            Self::MinZoneTempC => write!(f, "min_zone_temp_c"),
            Self::AnnualHeatingLoadKwh => write!(f, "annual_heating_load_kwh"),
            Self::AnnualCoolingLoadKwh => write!(f, "annual_cooling_load_kwh"),
            Self::AnnualHeatingEnergyKwh => write!(f, "annual_heating_energy_kwh"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReferenceBand {
    pub case_id: &'static str,
    pub metric: BestestMetric,
    pub min: f64,
    pub max: f64,
}

impl ReferenceBand {
    pub fn contains(&self, value: f64) -> bool {
        value >= self.min && value <= self.max
    }
}

pub fn core_reference_bands(case_id: &str) -> Vec<ReferenceBand> {
    match case_id {
        // ANSI/ASHRAE 140-2023 Annex B8, Tables B8-1 and B8-2: Case 600
        // annual heating and annual sensible cooling loads (kWh, the
        // report's MWh converted).
        "600" => vec![
            ReferenceBand {
                case_id: "600",
                metric: BestestMetric::AnnualHeatingLoadKwh,
                min: 3993.0,
                max: 4504.0,
            },
            ReferenceBand {
                case_id: "600",
                metric: BestestMetric::AnnualCoolingLoadKwh,
                min: 5432.0,
                max: 6162.0,
            },
        ],
        // ANSI/ASHRAE 140-2023 Annex B8, Tables B8-1 and B8-2: Case 900
        // annual heating and annual sensible cooling loads (kWh, the
        // report's MWh converted).
        "900" => vec![
            ReferenceBand {
                case_id: "900",
                metric: BestestMetric::AnnualHeatingLoadKwh,
                min: 1379.0,
                max: 1814.0,
            },
            ReferenceBand {
                case_id: "900",
                metric: BestestMetric::AnnualCoolingLoadKwh,
                min: 2267.0,
                max: 2714.0,
            },
        ],
        // ANSI/ASHRAE 140-2023 Annex B8 Table B8-5: Case 600FF free-float
        // temperatures (°C).
        // Peak: reference-tool range 62.4–68.4°C.
        // Min: reference-tool range -13.8 to -9.9°C, the report's own Min
        // and Max (the 140-2017 pin's 0°C upper bound was a HARES
        // relaxation this band does not carry).
        "600FF" => vec![
            ReferenceBand {
                case_id: "600FF",
                metric: BestestMetric::PeakZoneTempC,
                min: 62.4,
                max: 68.4,
            },
            ReferenceBand {
                case_id: "600FF",
                metric: BestestMetric::MinZoneTempC,
                min: -13.8,
                max: -9.9,
            },
        ],
        // ANSI/ASHRAE 140-2023 Annex B8 Table B8-5: Case 900FF free-float
        // temperatures (°C).
        "900FF" => vec![
            ReferenceBand {
                case_id: "900FF",
                metric: BestestMetric::PeakZoneTempC,
                min: 43.3,
                max: 46.0,
            },
            ReferenceBand {
                case_id: "900FF",
                metric: BestestMetric::MinZoneTempC,
                min: 0.6,
                max: 2.2,
            },
        ],
        // ANSI/ASHRAE 140-2023 Annex B8 Table B8-1: Case 640 annual
        // heating load (kWh, the report's MWh converted; the metric name
        // keeps the earlier pin's "energy" wording, the band is the
        // current table's annual heating quantity).
        "640" => vec![ReferenceBand {
            case_id: "640",
            metric: BestestMetric::AnnualHeatingEnergyKwh,
            min: 2403.0,
            max: 2682.0,
        }],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::core_reference_bands;

    #[test]
    fn reference_bands_have_case_ids_and_contains_is_exercised() {
        for band in core_reference_bands("600") {
            assert!(!band.case_id.is_empty());
            let mid = (band.min + band.max) * 0.5;
            assert!(band.contains(mid));
        }
    }
}
