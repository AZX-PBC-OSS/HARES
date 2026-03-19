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
        "600FF" => vec![
            ReferenceBand {
                case_id: "600FF",
                metric: BestestMetric::PeakZoneTempC,
                min: -20.0,
                max: 80.0,
            },
            ReferenceBand {
                case_id: "600FF",
                metric: BestestMetric::MinZoneTempC,
                min: -40.0,
                max: 50.0,
            },
            ReferenceBand {
                case_id: "600FF",
                metric: BestestMetric::AnnualHeatingLoadKwh,
                min: 0.0,
                max: 5_000.0,
            },
            ReferenceBand {
                case_id: "600FF",
                metric: BestestMetric::AnnualCoolingLoadKwh,
                min: 0.0,
                max: 5_000.0,
            },
        ],
        "900FF" => vec![
            ReferenceBand {
                case_id: "900FF",
                metric: BestestMetric::PeakZoneTempC,
                min: -20.0,
                max: 80.0,
            },
            ReferenceBand {
                case_id: "900FF",
                metric: BestestMetric::MinZoneTempC,
                min: -40.0,
                max: 50.0,
            },
            ReferenceBand {
                case_id: "900FF",
                metric: BestestMetric::AnnualHeatingLoadKwh,
                min: 0.0,
                max: 5_000.0,
            },
            ReferenceBand {
                case_id: "900FF",
                metric: BestestMetric::AnnualCoolingLoadKwh,
                min: 0.0,
                max: 5_000.0,
            },
        ],
        "640" => vec![ReferenceBand {
            case_id: "640",
            metric: BestestMetric::AnnualHeatingEnergyKwh,
            min: 0.0,
            max: 5_000.0,
        }],
        _ => Vec::new(),
    }
}
