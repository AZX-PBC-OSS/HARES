use hares_types::{BillingCycle, HaresError, SeasonFilter, SeasonalSplit, TouPeriod};
use serde::{Deserialize, Serialize};

/// Per-kWh energy rate for a TOU period and season.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnergyRate {
    pub period_name: String,
    pub season: SeasonFilter,
    pub rate_per_kwh: f64,
}

impl EnergyRate {
    pub fn validate(&self) -> Result<(), HaresError> {
        if !self.rate_per_kwh.is_finite() || self.rate_per_kwh < 0.0 {
            return Err(HaresError::Tariff(format!(
                "energy rate '{}' rate_per_kwh must be finite and >= 0, got {}",
                self.period_name, self.rate_per_kwh
            )));
        }
        Ok(())
    }
}

/// Demand ratchet: bill at least `minimum_fraction` of the highest peak
/// seen in the past `lookback_months`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RatchetConfig {
    pub lookback_months: u8,
    pub minimum_fraction: f64,
}

impl RatchetConfig {
    pub fn validate(&self) -> Result<(), HaresError> {
        if self.lookback_months == 0 {
            return Err(HaresError::Tariff("lookback_months must be > 0".into()));
        }
        if !self.minimum_fraction.is_finite() || self.minimum_fraction < 0.0 {
            return Err(HaresError::Tariff(format!(
                "minimum_fraction must be finite and >= 0, got {}",
                self.minimum_fraction
            )));
        }
        Ok(())
    }
}

/// Per-kW demand charge for a TOU period and season.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DemandRate {
    /// `None` = coincident peak (system-wide).
    pub period_name: Option<String>,
    pub season: SeasonFilter,
    pub rate_per_kw: f64,
    pub ratchet: Option<RatchetConfig>,
}

impl DemandRate {
    pub fn validate(&self) -> Result<(), HaresError> {
        if !self.rate_per_kw.is_finite() || self.rate_per_kw < 0.0 {
            return Err(HaresError::Tariff(format!(
                "rate_per_kw must be finite and >= 0, got {}",
                self.rate_per_kw
            )));
        }
        if let Some(r) = &self.ratchet {
            r.validate()?;
        }
        Ok(())
    }
}

fn validate_tiered(
    thresholds: &[f64],
    rates: &[f64],
    threshold_name: &str,
    rate_name: &str,
) -> Result<(), HaresError> {
    if rates.len() != thresholds.len() + 1 {
        return Err(HaresError::Tariff(format!(
            "{rate_name}.len() ({}) must be {threshold_name}.len() + 1 ({})",
            rates.len(),
            thresholds.len() + 1
        )));
    }
    for (i, t) in thresholds.iter().enumerate() {
        if !t.is_finite() || *t < 0.0 {
            return Err(HaresError::Tariff(format!(
                "{threshold_name}[{i}] must be finite and >= 0, got {t}"
            )));
        }
        if i > 0 && *t <= thresholds[i - 1] {
            return Err(HaresError::Tariff(format!(
                "{threshold_name} must be strictly ascending: [{}] = {} <= [{}] = {}",
                i - 1,
                thresholds[i - 1],
                i,
                t
            )));
        }
    }
    for (i, r) in rates.iter().enumerate() {
        if !r.is_finite() || *r < 0.0 {
            return Err(HaresError::Tariff(format!(
                "{rate_name}[{i}] must be finite and >= 0, got {r}"
            )));
        }
    }
    Ok(())
}

/// Inclining/declining block rate for a season.
///
/// `rates_per_kwh.len()` must equal `thresholds_kwh.len() + 1`:
/// the first rate applies to usage below the first threshold,
/// each subsequent rate applies between consecutive thresholds,
/// and the last rate applies to all usage above the final threshold.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TieredBlock {
    pub season: SeasonFilter,
    /// Cumulative upper bounds per tier (kWh). Must be strictly ascending.
    pub thresholds_kwh: Vec<f64>,
    /// Rate for each tier. Length must be `thresholds_kwh.len() + 1`.
    pub rates_per_kwh: Vec<f64>,
}

impl TieredBlock {
    pub fn validate(&self) -> Result<(), HaresError> {
        validate_tiered(
            &self.thresholds_kwh,
            &self.rates_per_kwh,
            "thresholds_kwh",
            "rates_per_kwh",
        )
    }
}

/// How grid exports are compensated.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub enum ExportMode {
    NetMetering,
    NetBilling,
    /// 8760 hourly export prices ($/kWh) indexed by hour-of-year (0-8759).
    /// Used for CPUC-mandated Avoided Cost Calculator (ACC) profiles
    /// (e.g., California NEM 3.0) and other time-varying avoided-cost models.
    /// CPUC Decision 22-12-056 (December 2022): NEM 3.0 hourly avoided cost methodology.
    HourlySchedule(Vec<f64>),
    FlatRate(f64),
    #[default]
    None,
}

/// Export compensation configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportRate {
    pub mode: ExportMode,
    pub tou_credits: Vec<EnergyRate>,
}

impl Default for ExportRate {
    fn default() -> Self {
        Self {
            mode: ExportMode::None,
            tou_credits: Vec::new(),
        }
    }
}

impl ExportRate {
    pub fn validate(&self) -> Result<(), HaresError> {
        match &self.mode {
            ExportMode::FlatRate(r) if !r.is_finite() || *r < 0.0 => {
                return Err(HaresError::Tariff(format!(
                    "FlatRate must be finite and >= 0, got {r}"
                )));
            }
            ExportMode::HourlySchedule(schedule) => {
                if schedule.len() != 8760 {
                    return Err(HaresError::Tariff(format!(
                        "HourlySchedule must have exactly 8760 entries, got {}",
                        schedule.len()
                    )));
                }
                for (i, price) in schedule.iter().enumerate() {
                    if !price.is_finite() || *price < 0.0 {
                        return Err(HaresError::Tariff(format!(
                            "HourlySchedule[{i}] must be finite and >= 0, got {price}"
                        )));
                    }
                }
            }
            _ => {}
        }
        for er in &self.tou_credits {
            er.validate()?;
        }
        Ok(())
    }
}

/// Fixed monthly and daily charges.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct FixedCharges {
    pub monthly_usd: f64,
    pub daily_usd: f64,
}

impl FixedCharges {
    pub fn validate(&self) -> Result<(), HaresError> {
        if !self.monthly_usd.is_finite() || self.monthly_usd < 0.0 {
            return Err(HaresError::Tariff(format!(
                "monthly_usd must be finite and >= 0, got {}",
                self.monthly_usd
            )));
        }
        if !self.daily_usd.is_finite() || self.daily_usd < 0.0 {
            return Err(HaresError::Tariff(format!(
                "daily_usd must be finite and >= 0, got {}",
                self.daily_usd
            )));
        }
        Ok(())
    }
}

/// Critical peak pricing configuration.
///
/// CPP applies an elevated rate during a limited number of event hours per year.
/// EnergyPlus supports this via `CriticalPeakSchedule` and dedicated CPP rate
/// fields (`vendors/EnergyPlus/src/EnergyPlus/EconomicTariff.cc:768-930`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CppConfig {
    /// Rate per kWh during CPP event hours ($/kWh).
    pub event_rate_per_kwh: f64,
    /// Maximum number of CPP event hours allowed per year.
    pub event_count_limit: u32,
    /// 8760-length vector where non-zero entries indicate CPP event hours.
    /// Index is hour-of-year (0-8759) in local civil time. The evaluator
    /// applies the event rate to the first `event_count_limit` non-zero
    /// entries encountered during the simulation; subsequent event hours
    /// (even if the schedule says they are events) use the standard rate.
    pub event_schedule: Vec<i32>,
}

impl CppConfig {
    pub fn validate(&self) -> Result<(), HaresError> {
        if !self.event_rate_per_kwh.is_finite() || self.event_rate_per_kwh < 0.0 {
            return Err(HaresError::Tariff(format!(
                "cpp event_rate_per_kwh must be finite and >= 0, got {}",
                self.event_rate_per_kwh
            )));
        }
        if self.event_schedule.len() != 8760 {
            return Err(HaresError::Tariff(format!(
                "cpp event_schedule must have 8760 entries, got {}",
                self.event_schedule.len()
            )));
        }
        Ok(())
    }
}

/// Complete electric utility tariff.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ElectricTariff {
    pub name: Option<String>,
    pub tou_schedule: Vec<TouPeriod>,
    /// Demand-specific TOU schedule. When present, demand periods are resolved
    /// independently of energy periods (URDB `demandweekdayschedule`).
    pub demand_tou_schedule: Vec<TouPeriod>,
    pub energy_rates: Vec<EnergyRate>,
    pub demand_rates: Vec<DemandRate>,
    pub tiered_rates: Vec<TieredBlock>,
    pub export_rate: ExportRate,
    pub fixed_charges: FixedCharges,
    /// Minimum monthly charge ($/month floor).
    pub minimum_charge: Option<f64>,
    /// When `true` (default), the minimum charge floor is applied to metered
    /// charges (energy + demand + fixed) *before* subtracting export credit:
    ///   `net = max(metered, min_charge) - export_credit`
    /// When `false`, the minimum charge floor is applied to the net bill
    /// *after* subtracting export credit:
    ///   `net = max(metered - export_credit, min_charge)`
    #[serde(default = "default_true")]
    pub minimum_charge_excludes_export: bool,
    pub billing_cycle: BillingCycle,
    pub seasonal_split: Option<SeasonalSplit>,
    /// Demand averaging window in minutes. Defaults to 15 (standard US FERC/NERC).
    /// Some utilities use 30 (LADWP commercial) or 5.
    #[serde(default = "default_demand_window_minutes")]
    pub demand_window_minutes: u32,
    /// Real-time pricing: 8760 hourly prices ($/kWh) indexed by hour-of-year.
    /// When present, overrides static `energy_rates` and `tou_schedule` for
    /// energy charges. Export price under `NetMetering` also uses the RTP price.
    /// EnergyPlus: `RealTimePriceSchedule` (EconomicTariff.cc:2539-2541).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtp_schedule: Option<Vec<f64>>,
    /// Critical peak pricing configuration. When present, CPP event hours
    /// use `event_rate_per_kwh` instead of the standard energy rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpp_config: Option<CppConfig>,
    /// When `Some`, the evaluator applies the energy rate for this TOU period
    /// to the EV-charging portion of the load (passed via `ev_power_kw`)
    /// instead of the standard import rate. The EV rate is resolved from
    /// `energy_rates` by matching `period_name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ev_tou_period_name: Option<String>,
}

fn default_demand_window_minutes() -> u32 {
    15
}

fn default_true() -> bool {
    true
}

impl Default for ElectricTariff {
    fn default() -> Self {
        Self {
            name: None,
            tou_schedule: Vec::new(),
            demand_tou_schedule: Vec::new(),
            energy_rates: Vec::new(),
            demand_rates: Vec::new(),
            tiered_rates: Vec::new(),
            export_rate: ExportRate::default(),
            fixed_charges: FixedCharges::default(),
            minimum_charge: None,
            minimum_charge_excludes_export: true,
            billing_cycle: BillingCycle::default(),
            seasonal_split: None,
            demand_window_minutes: 15,
            rtp_schedule: None,
            cpp_config: None,
            ev_tou_period_name: None,
        }
    }
}

impl ElectricTariff {
    pub fn validate(&self) -> Result<(), HaresError> {
        for tp in &self.tou_schedule {
            tp.validate()?;
        }
        for tp in &self.demand_tou_schedule {
            tp.validate()?;
        }
        for dr in &self.demand_rates {
            dr.validate()?;
        }
        for tb in &self.tiered_rates {
            tb.validate()?;
        }
        self.fixed_charges.validate()?;
        self.export_rate.validate()?;
        if let Some(mc) = self.minimum_charge {
            if !mc.is_finite() || mc < 0.0 {
                return Err(HaresError::Tariff(format!(
                    "minimum_charge must be finite and >= 0, got {mc}"
                )));
            }
        }
        for er in &self.energy_rates {
            er.validate()?;
        }
        if let BillingCycle::Custom(days) = self.billing_cycle {
            if days == 0 {
                return Err(HaresError::Tariff(
                    "BillingCycle::Custom days must be > 0".into(),
                ));
            }
        }
        if let Some(ss) = &self.seasonal_split {
            ss.validate()?;
        }
        if self.demand_window_minutes < 5 || self.demand_window_minutes > 60 {
            return Err(HaresError::Tariff(format!(
                "demand_window_minutes must be in [5, 60], got {}",
                self.demand_window_minutes
            )));
        }
        let tou_names: Vec<&str> = self.tou_schedule.iter().map(|p| p.name.as_str()).collect();
        for er in &self.energy_rates {
            if !er.period_name.is_empty() && !tou_names.contains(&er.period_name.as_str()) {
                return Err(HaresError::Tariff(format!(
                    "energy rate references unknown TOU period '{}'",
                    er.period_name
                )));
            }
        }
        // Cross-check demand rate period names against demand (or energy) TOU schedule.
        let demand_schedule = if self.demand_tou_schedule.is_empty() {
            &self.tou_schedule
        } else {
            &self.demand_tou_schedule
        };
        let demand_tou_names: Vec<&str> = demand_schedule.iter().map(|p| p.name.as_str()).collect();
        for dr in &self.demand_rates {
            if let Some(name) = &dr.period_name {
                if !demand_tou_names.contains(&name.as_str()) {
                    return Err(HaresError::Tariff(format!(
                        "demand rate references unknown TOU period '{name}'"
                    )));
                }
            }
        }
        if let Some(ref rtp) = self.rtp_schedule {
            if rtp.len() < 8760 {
                return Err(HaresError::Tariff(format!(
                    "rtp_schedule must have at least 8760 entries, got {}",
                    rtp.len()
                )));
            }
            for (i, price) in rtp.iter().enumerate() {
                if !price.is_finite() || *price < 0.0 {
                    return Err(HaresError::Tariff(format!(
                        "rtp_schedule[{i}] must be finite and >= 0, got {price}"
                    )));
                }
            }
        }
        if let Some(ref cpp) = self.cpp_config {
            cpp.validate()?;
        }
        if let Some(ref ev_name) = self.ev_tou_period_name {
            if !ev_name.is_empty() && !tou_names.contains(&ev_name.as_str()) {
                return Err(HaresError::Tariff(format!(
                    "ev_tou_period_name '{ev_name}' references unknown TOU period"
                )));
            }
        }
        Ok(())
    }
}

/// Gas tiered block rate for a season.
///
/// Same invariant as `TieredBlock`: `rates_per_therm.len() == thresholds_therms.len() + 1`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GasTieredBlock {
    pub season: SeasonFilter,
    pub thresholds_therms: Vec<f64>,
    pub rates_per_therm: Vec<f64>,
}

impl GasTieredBlock {
    pub fn validate(&self) -> Result<(), HaresError> {
        validate_tiered(
            &self.thresholds_therms,
            &self.rates_per_therm,
            "thresholds_therms",
            "rates_per_therm",
        )
    }
}

/// Complete gas utility tariff.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct GasTariff {
    pub name: Option<String>,
    pub tiered_rates: Vec<GasTieredBlock>,
    pub fixed_charges: FixedCharges,
    pub billing_cycle: BillingCycle,
    pub seasonal_split: Option<SeasonalSplit>,
}

impl GasTariff {
    pub fn validate(&self) -> Result<(), HaresError> {
        for tb in &self.tiered_rates {
            tb.validate()?;
        }
        self.fixed_charges.validate()?;
        if let BillingCycle::Custom(days) = self.billing_cycle {
            if days == 0 {
                return Err(HaresError::Tariff(
                    "BillingCycle::Custom days must be > 0".into(),
                ));
            }
        }
        if let Some(ss) = &self.seasonal_split {
            ss.validate()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use hares_types::{DayFilter, TimeWindow};

    use super::*;

    #[test]
    fn electric_tariff_default_roundtrip() {
        let tariff = ElectricTariff::default();
        let json = serde_json::to_string(&tariff).unwrap();
        let back: ElectricTariff = serde_json::from_str(&json).unwrap();
        assert_eq!(back, tariff);
        assert!(tariff.tou_schedule.is_empty());
        assert!(tariff.energy_rates.is_empty());
        assert!(tariff.demand_rates.is_empty());
        assert!(tariff.tiered_rates.is_empty());
        assert_eq!(tariff.fixed_charges, FixedCharges::default());
        assert_eq!(tariff.billing_cycle, BillingCycle::Monthly);
    }

    #[test]
    fn electric_tariff_full_roundtrip() {
        let tariff = ElectricTariff {
            name: Some("SCE TOU-D-4-9PM".into()),
            tou_schedule: vec![
                TouPeriod {
                    name: "on-peak".into(),
                    schedule: vec![TimeWindow::new(DayFilter::Weekdays, 960, 1260, 0.0)],
                    season: SeasonFilter::Summer,
                },
                TouPeriod {
                    name: "mid-peak".into(),
                    schedule: vec![TimeWindow::new(DayFilter::Weekdays, 480, 960, 0.0)],
                    season: SeasonFilter::Summer,
                },
                TouPeriod {
                    name: "off-peak".into(),
                    schedule: vec![TimeWindow::new(DayFilter::Any, 0, 480, 0.0)],
                    season: SeasonFilter::All,
                },
            ],
            energy_rates: vec![
                EnergyRate {
                    period_name: "on-peak".into(),
                    season: SeasonFilter::Summer,
                    rate_per_kwh: 0.45,
                },
                EnergyRate {
                    period_name: "mid-peak".into(),
                    season: SeasonFilter::Summer,
                    rate_per_kwh: 0.30,
                },
                EnergyRate {
                    period_name: "off-peak".into(),
                    season: SeasonFilter::All,
                    rate_per_kwh: 0.12,
                },
            ],
            demand_rates: vec![DemandRate {
                period_name: Some("on-peak".into()),
                season: SeasonFilter::Summer,
                rate_per_kw: 18.50,
                ratchet: Some(RatchetConfig {
                    lookback_months: 11,
                    minimum_fraction: 0.85,
                }),
            }],
            tiered_rates: vec![
                TieredBlock {
                    season: SeasonFilter::Summer,
                    thresholds_kwh: vec![500.0, 1000.0],
                    rates_per_kwh: vec![0.10, 0.15, 0.25],
                },
                TieredBlock {
                    season: SeasonFilter::Winter,
                    thresholds_kwh: vec![700.0],
                    rates_per_kwh: vec![0.09, 0.14],
                },
            ],
            export_rate: ExportRate {
                mode: ExportMode::NetMetering,
                tou_credits: vec![EnergyRate {
                    period_name: "on-peak".into(),
                    season: SeasonFilter::Summer,
                    rate_per_kwh: 0.45,
                }],
            },
            fixed_charges: FixedCharges {
                monthly_usd: 12.50,
                daily_usd: 0.0,
            },
            minimum_charge: Some(10.0),
            minimum_charge_excludes_export: true,
            billing_cycle: BillingCycle::Monthly,
            seasonal_split: Some(SeasonalSplit::new(6, 9).unwrap()),
            demand_tou_schedule: Vec::new(),
            demand_window_minutes: 15,
            rtp_schedule: None,
            cpp_config: None,
            ev_tou_period_name: None,
        };

        let json = serde_json::to_string(&tariff).unwrap();
        let back: ElectricTariff = serde_json::from_str(&json).unwrap();
        assert_eq!(back, tariff);
        assert!(tariff.validate().is_ok());
    }

    #[test]
    fn cpp_config_validate_rejects_bad_rate() {
        let config = CppConfig {
            event_rate_per_kwh: -1.0,
            event_count_limit: 15,
            event_schedule: vec![0; 8760],
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn cpp_config_validate_rejects_nan_rate() {
        let config = CppConfig {
            event_rate_per_kwh: f64::NAN,
            event_count_limit: 15,
            event_schedule: vec![0; 8760],
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn cpp_config_validate_rejects_wrong_length_schedule() {
        let config = CppConfig {
            event_rate_per_kwh: 1.50,
            event_count_limit: 15,
            event_schedule: vec![0; 100],
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn cpp_config_validate_accepts_valid() {
        let config = CppConfig {
            event_rate_per_kwh: 1.50,
            event_count_limit: 15,
            event_schedule: vec![0; 8760],
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn electric_tariff_rtp_validate_rejects_short_schedule() {
        let tariff = ElectricTariff {
            rtp_schedule: Some(vec![0.10; 100]),
            ..Default::default()
        };
        assert!(tariff.validate().is_err());
    }

    #[test]
    fn electric_tariff_rtp_validate_rejects_negative_price() {
        let mut prices = vec![0.10; 8760];
        prices[500] = -0.05;
        let tariff = ElectricTariff {
            rtp_schedule: Some(prices),
            ..Default::default()
        };
        assert!(tariff.validate().is_err());
    }

    #[test]
    fn electric_tariff_ev_period_name_unknown_rejected() {
        let tariff = ElectricTariff {
            ev_tou_period_name: Some("nonexistent".into()),
            ..Default::default()
        };
        assert!(tariff.validate().is_err());
    }

    #[test]
    fn electric_tariff_ev_period_name_valid_accepted() {
        let tariff = ElectricTariff {
            tou_schedule: vec![TouPeriod {
                name: "ev-off-peak".into(),
                schedule: vec![TimeWindow::new(DayFilter::Any, 0, 480, 0.0)],
                season: SeasonFilter::All,
            }],
            energy_rates: vec![EnergyRate {
                period_name: "ev-off-peak".into(),
                season: SeasonFilter::All,
                rate_per_kwh: 0.06,
            }],
            ev_tou_period_name: Some("ev-off-peak".into()),
            ..Default::default()
        };
        assert!(tariff.validate().is_ok());
    }

    #[test]
    fn electric_tariff_rtp_validate_accepts_valid() {
        let tariff = ElectricTariff {
            rtp_schedule: Some(vec![0.10; 8760]),
            ..Default::default()
        };
        assert!(tariff.validate().is_ok());
    }

    #[test]
    fn gas_tariff_roundtrip() {
        let tariff = GasTariff {
            name: Some("PG&E Gas Baseline".into()),
            tiered_rates: vec![
                GasTieredBlock {
                    season: SeasonFilter::Winter,
                    thresholds_therms: vec![25.0, 50.0],
                    rates_per_therm: vec![1.05, 1.35, 1.85],
                },
                GasTieredBlock {
                    season: SeasonFilter::Summer,
                    thresholds_therms: vec![15.0],
                    rates_per_therm: vec![0.95, 1.25],
                },
            ],
            fixed_charges: FixedCharges {
                monthly_usd: 10.00,
                daily_usd: 0.0,
            },
            billing_cycle: BillingCycle::Monthly,
            seasonal_split: Some(SeasonalSplit::new(6, 9).unwrap()),
        };

        let json = serde_json::to_string(&tariff).unwrap();
        let back: GasTariff = serde_json::from_str(&json).unwrap();
        assert_eq!(back, tariff);
        assert!(tariff.validate().is_ok());
    }

    #[test]
    fn export_mode_variants() {
        let modes = vec![
            ExportMode::NetMetering,
            ExportMode::NetBilling,
            ExportMode::HourlySchedule(vec![0.08; 8760]),
            ExportMode::FlatRate(0.08),
            ExportMode::None,
        ];
        for mode in modes {
            let json = serde_json::to_string(&mode).unwrap();
            let back: ExportMode = serde_json::from_str(&json).unwrap();
            assert_eq!(back, mode);
        }
    }

    #[test]
    fn tiered_block_rate_count_invariant() {
        let valid = TieredBlock {
            season: SeasonFilter::All,
            thresholds_kwh: vec![500.0, 1000.0],
            rates_per_kwh: vec![0.10, 0.15, 0.25],
        };
        assert!(valid.validate().is_ok());

        let too_few_rates = TieredBlock {
            season: SeasonFilter::All,
            thresholds_kwh: vec![500.0, 1000.0],
            rates_per_kwh: vec![0.10, 0.15],
        };
        assert!(too_few_rates.validate().is_err());

        let too_many_rates = TieredBlock {
            season: SeasonFilter::All,
            thresholds_kwh: vec![500.0],
            rates_per_kwh: vec![0.10, 0.15, 0.25],
        };
        assert!(too_many_rates.validate().is_err());
    }

    #[test]
    fn tiered_block_rejects_unsorted_thresholds() {
        let unsorted = TieredBlock {
            season: SeasonFilter::All,
            thresholds_kwh: vec![1000.0, 500.0],
            rates_per_kwh: vec![0.10, 0.15, 0.25],
        };
        assert!(unsorted.validate().is_err());
    }

    #[test]
    fn tiered_block_no_thresholds_single_rate() {
        let flat = TieredBlock {
            season: SeasonFilter::All,
            thresholds_kwh: vec![],
            rates_per_kwh: vec![0.12],
        };
        assert!(flat.validate().is_ok());
    }

    #[test]
    fn tiered_block_empty_rates_rejected() {
        let empty = TieredBlock {
            season: SeasonFilter::All,
            thresholds_kwh: vec![],
            rates_per_kwh: vec![],
        };
        assert!(empty.validate().is_err());
    }

    #[test]
    fn gas_tiered_block_rate_count_invariant() {
        let valid = GasTieredBlock {
            season: SeasonFilter::Winter,
            thresholds_therms: vec![25.0],
            rates_per_therm: vec![1.05, 1.35],
        };
        assert!(valid.validate().is_ok());

        let invalid = GasTieredBlock {
            season: SeasonFilter::Winter,
            thresholds_therms: vec![25.0],
            rates_per_therm: vec![1.05],
        };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn demand_rate_validate_rejects_invalid() {
        let bad = DemandRate {
            period_name: None,
            season: SeasonFilter::All,
            rate_per_kw: -1.0,
            ratchet: None,
        };
        assert!(bad.validate().is_err());

        let bad_ratchet = DemandRate {
            period_name: Some("peak".into()),
            season: SeasonFilter::Summer,
            rate_per_kw: 18.0,
            ratchet: Some(RatchetConfig {
                lookback_months: 0,
                minimum_fraction: 0.85,
            }),
        };
        assert!(bad_ratchet.validate().is_err());
    }

    #[test]
    fn ratchet_config_validate_rejects_invalid_fraction() {
        let negative = RatchetConfig {
            lookback_months: 11,
            minimum_fraction: -0.1,
        };
        assert!(negative.validate().is_err());

        let nan = RatchetConfig {
            lookback_months: 11,
            minimum_fraction: f64::NAN,
        };
        assert!(nan.validate().is_err());
    }

    #[test]
    fn ratchet_config_accepts_above_one() {
        let above = RatchetConfig {
            lookback_months: 11,
            minimum_fraction: 1.1,
        };
        assert!(above.validate().is_ok());
    }

    #[test]
    fn fixed_charges_validate_rejects_negative() {
        let bad = FixedCharges {
            monthly_usd: -1.0,
            daily_usd: 0.0,
        };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn electric_tariff_validate_rejects_invalid_minimum_charge() {
        let tariff = ElectricTariff {
            minimum_charge: Some(-5.0),
            ..Default::default()
        };
        assert!(tariff.validate().is_err());
    }

    #[test]
    fn electric_tariff_validate_rejects_nan_energy_rate() {
        let tariff = ElectricTariff {
            energy_rates: vec![EnergyRate {
                period_name: "peak".into(),
                season: SeasonFilter::All,
                rate_per_kwh: f64::NAN,
            }],
            ..Default::default()
        };
        assert!(tariff.validate().is_err());
    }

    #[test]
    fn export_mode_default_is_none() {
        assert_eq!(ExportMode::default(), ExportMode::None);
    }

    #[test]
    fn fixed_charges_default_is_zero() {
        let fc = FixedCharges::default();
        assert!(fc.monthly_usd == 0.0);
        assert!(fc.daily_usd == 0.0);
    }

    #[test]
    fn electric_tariff_validate_rejects_negative_energy_rate() {
        let tariff = ElectricTariff {
            energy_rates: vec![EnergyRate {
                period_name: "peak".into(),
                season: SeasonFilter::All,
                rate_per_kwh: -0.05,
            }],
            ..Default::default()
        };
        assert!(tariff.validate().is_err());
    }

    #[test]
    fn export_rate_validate_rejects_invalid_flat_rate() {
        let er = ExportRate {
            mode: ExportMode::FlatRate(-1.0),
            tou_credits: vec![],
        };
        assert!(er.validate().is_err());

        let nan = ExportRate {
            mode: ExportMode::FlatRate(f64::NAN),
            tou_credits: vec![],
        };
        assert!(nan.validate().is_err());
    }

    #[test]
    fn export_rate_validate_rejects_nan_tou_credit() {
        let er = ExportRate {
            mode: ExportMode::NetMetering,
            tou_credits: vec![EnergyRate {
                period_name: "peak".into(),
                season: SeasonFilter::Summer,
                rate_per_kwh: f64::NAN,
            }],
        };
        assert!(er.validate().is_err());
    }

    #[test]
    fn export_rate_validate_accepts_valid() {
        let er = ExportRate {
            mode: ExportMode::FlatRate(0.08),
            tou_credits: vec![EnergyRate {
                period_name: "peak".into(),
                season: SeasonFilter::Summer,
                rate_per_kwh: 0.45,
            }],
        };
        assert!(er.validate().is_ok());
    }

    #[test]
    fn gas_tiered_block_rejects_unsorted_thresholds() {
        let unsorted = GasTieredBlock {
            season: SeasonFilter::All,
            thresholds_therms: vec![50.0, 25.0],
            rates_per_therm: vec![1.05, 1.35, 1.85],
        };
        assert!(unsorted.validate().is_err());
    }

    #[test]
    fn billing_cycle_custom_zero_rejected() {
        let tariff = ElectricTariff {
            billing_cycle: BillingCycle::Custom(0),
            ..Default::default()
        };
        assert!(tariff.validate().is_err());

        let gas = GasTariff {
            billing_cycle: BillingCycle::Custom(0),
            ..Default::default()
        };
        assert!(gas.validate().is_err());
    }

    #[test]
    fn billing_cycle_custom_valid_accepted() {
        let tariff = ElectricTariff {
            billing_cycle: BillingCycle::Custom(14),
            ..Default::default()
        };
        assert!(tariff.validate().is_ok());
    }

    #[test]
    fn energy_rate_validate_standalone() {
        let valid = EnergyRate {
            period_name: "peak".into(),
            season: SeasonFilter::Summer,
            rate_per_kwh: 0.45,
        };
        assert!(valid.validate().is_ok());

        let inf = EnergyRate {
            period_name: "peak".into(),
            season: SeasonFilter::All,
            rate_per_kwh: f64::INFINITY,
        };
        assert!(inf.validate().is_err());
    }

    #[test]
    fn demand_rate_validate_rejects_nan() {
        let bad = DemandRate {
            period_name: None,
            season: SeasonFilter::All,
            rate_per_kw: f64::NAN,
            ratchet: None,
        };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn demand_rate_validate_rejects_infinity() {
        let bad = DemandRate {
            period_name: None,
            season: SeasonFilter::All,
            rate_per_kw: f64::INFINITY,
            ratchet: None,
        };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn fixed_charges_validate_rejects_nan() {
        let bad = FixedCharges {
            monthly_usd: f64::NAN,
            daily_usd: 0.0,
        };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn electric_tariff_validate_checks_tou_schedule() {
        let tariff = ElectricTariff {
            tou_schedule: vec![TouPeriod {
                name: "bad".into(),
                schedule: vec![TimeWindow {
                    day: DayFilter::Any,
                    start_minute: 500,
                    end_minute: 500,
                    value: 0.0,
                    noise: None,
                    min_value: None,
                    max_value: None,
                }],
                season: SeasonFilter::All,
            }],
            ..Default::default()
        };
        assert!(tariff.validate().is_err());
    }

    // M1: DemandRate referencing an unknown period name fails validation.
    #[test]
    fn electric_tariff_validate_rejects_unknown_demand_period() {
        let tariff = ElectricTariff {
            demand_rates: vec![DemandRate {
                period_name: Some("nonexistent".into()),
                season: SeasonFilter::All,
                rate_per_kw: 10.0,
                ratchet: None,
            }],
            ..Default::default()
        };
        assert!(
            tariff.validate().is_err(),
            "validate() should reject a DemandRate referencing an unknown period name"
        );
    }

    #[test]
    fn electric_tariff_validate_checks_export_rate() {
        let tariff = ElectricTariff {
            export_rate: ExportRate {
                mode: ExportMode::FlatRate(-1.0),
                tou_credits: vec![],
            },
            ..Default::default()
        };
        assert!(tariff.validate().is_err());
    }

    #[test]
    fn export_rate_hourly_schedule_rejects_wrong_length() {
        let er = ExportRate {
            mode: ExportMode::HourlySchedule(vec![0.08; 100]),
            tou_credits: vec![],
        };
        assert!(er.validate().is_err());
    }

    #[test]
    fn export_rate_hourly_schedule_rejects_negative_price() {
        let mut schedule = vec![0.08; 8760];
        schedule[500] = -0.05;
        let er = ExportRate {
            mode: ExportMode::HourlySchedule(schedule),
            tou_credits: vec![],
        };
        assert!(er.validate().is_err());
    }

    #[test]
    fn export_rate_hourly_schedule_rejects_nan_price() {
        let mut schedule = vec![0.08; 8760];
        schedule[100] = f64::NAN;
        let er = ExportRate {
            mode: ExportMode::HourlySchedule(schedule),
            tou_credits: vec![],
        };
        assert!(er.validate().is_err());
    }

    #[test]
    fn export_rate_hourly_schedule_accepts_valid() {
        let er = ExportRate {
            mode: ExportMode::HourlySchedule(vec![0.08; 8760]),
            tou_credits: vec![],
        };
        assert!(er.validate().is_ok());
    }

    #[test]
    fn export_rate_hourly_schedule_accepts_zero_prices() {
        let er = ExportRate {
            mode: ExportMode::HourlySchedule(vec![0.0; 8760]),
            tou_credits: vec![],
        };
        assert!(er.validate().is_ok());
    }
}
