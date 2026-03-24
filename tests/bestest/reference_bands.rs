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
        // ASHRAE 140-2017 Table B8-2: Case 600 annual heating/cooling loads (kWh).
        "600" => vec![
            ReferenceBand {
                case_id: "600",
                metric: BestestMetric::AnnualHeatingLoadKwh,
                min: 4296.0,
                max: 5709.0,
            },
            ReferenceBand {
                case_id: "600",
                metric: BestestMetric::AnnualCoolingLoadKwh,
                min: 6137.0,
                max: 7964.0,
            },
        ],
        // ASHRAE 140-2017 Table B8-2: Case 900 annual heating/cooling loads (kWh).
        "900" => vec![
            ReferenceBand {
                case_id: "900",
                metric: BestestMetric::AnnualHeatingLoadKwh,
                min: 1170.0,
                max: 2041.0,
            },
            ReferenceBand {
                case_id: "900",
                metric: BestestMetric::AnnualCoolingLoadKwh,
                min: 2132.0,
                max: 3415.0,
            },
        ],
        // ASHRAE 140-2017 Table B8-3a: Case 600FF free-float temperatures (°C).
        // Peak: reference tool range 64.9–69.5°C.
        // Min: reference value -18.8°C — must not drop below this; upper bound of 0°C
        // is generous given Denver winter outdoor lows.
        "600FF" => vec![
            ReferenceBand {
                case_id: "600FF",
                metric: BestestMetric::PeakZoneTempC,
                min: 64.9,
                max: 69.5,
            },
            ReferenceBand {
                case_id: "600FF",
                metric: BestestMetric::MinZoneTempC,
                min: -18.8,
                max: 0.0,
            },
        ],
        // ASHRAE 140-2017 Table B8-3a: Case 900FF free-float temperatures (°C).
        "900FF" => vec![
            ReferenceBand {
                case_id: "900FF",
                metric: BestestMetric::PeakZoneTempC,
                min: 41.6,
                max: 44.8,
            },
            ReferenceBand {
                case_id: "900FF",
                metric: BestestMetric::MinZoneTempC,
                min: -6.4,
                max: -1.6,
            },
        ],
        // ASHRAE 140-2017 Table B8-2: Case 640 annual heating energy (kWh).
        "640" => vec![ReferenceBand {
            case_id: "640",
            metric: BestestMetric::AnnualHeatingEnergyKwh,
            min: 2751.0,
            max: 3803.0,
        }],
        _ => Vec::new(),
    }
}
