//! EV vehicle catalog with factory methods for real-world vehicles,
//! plus behavioral archetype presets for EvDriverActor configuration.

use std::fmt;

use hares_types::DayFilter;
use hares_types::{
    BatteryChemistry, ChargingLevel, ChargingStrategy, DistributionKind, PlugInPolicy,
    ScheduleSource, TimeWindow, VehicleType,
};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use super::config::EvConfig;
use crate::EquipmentConfig;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VehicleId {
    TeslaModelYLr,
    TeslaModelYSr,
    TeslaModel3Lr,
    ChevyBoltEv,
    ChevyBoltEuv,
    FordMacheSr,
    FordMacheEr,
    FordLightningEr,
    HyundaiIoniq5Lr,
    NissanLeaf30,
    Jeep4xe,
    ToyotaRav4Prime,
    ChevyVoltGen1,
}

impl VehicleId {
    pub const ALL: &[VehicleId] = &[
        Self::TeslaModelYLr,
        Self::TeslaModelYSr,
        Self::TeslaModel3Lr,
        Self::ChevyBoltEv,
        Self::ChevyBoltEuv,
        Self::FordMacheSr,
        Self::FordMacheEr,
        Self::FordLightningEr,
        Self::HyundaiIoniq5Lr,
        Self::NissanLeaf30,
        Self::Jeep4xe,
        Self::ToyotaRav4Prime,
        Self::ChevyVoltGen1,
    ];

    pub fn spec(self) -> &'static VehicleSpec {
        &CATALOG[self as usize]
    }
}

impl fmt::Display for VehicleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::TeslaModelYLr => "TeslaModelYLr",
            Self::TeslaModelYSr => "TeslaModelYSr",
            Self::TeslaModel3Lr => "TeslaModel3Lr",
            Self::ChevyBoltEv => "ChevyBoltEv",
            Self::ChevyBoltEuv => "ChevyBoltEuv",
            Self::FordMacheSr => "FordMacheSr",
            Self::FordMacheEr => "FordMacheEr",
            Self::FordLightningEr => "FordLightningEr",
            Self::HyundaiIoniq5Lr => "HyundaiIoniq5Lr",
            Self::NissanLeaf30 => "NissanLeaf30",
            Self::Jeep4xe => "Jeep4xe",
            Self::ToyotaRav4Prime => "ToyotaRav4Prime",
            Self::ChevyVoltGen1 => "ChevyVoltGen1",
        };
        write!(f, "{s}")
    }
}

impl std::str::FromStr for VehicleId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized: String = s
            .chars()
            .filter(|c| *c != '_')
            .flat_map(|c| c.to_lowercase())
            .collect();
        match normalized.as_str() {
            "teslamodelyhr" | "teslamodelylr" => Ok(Self::TeslaModelYLr),
            "teslamodelysr" => Ok(Self::TeslaModelYSr),
            "teslamodel3lr" => Ok(Self::TeslaModel3Lr),
            "chevyboltev" => Ok(Self::ChevyBoltEv),
            "chevybolteuv" => Ok(Self::ChevyBoltEuv),
            "fordmachesr" => Ok(Self::FordMacheSr),
            "fordmacheer" => Ok(Self::FordMacheEr),
            "fordlightninger" => Ok(Self::FordLightningEr),
            "hyundaiioniq5lr" => Ok(Self::HyundaiIoniq5Lr),
            "nissanleaf30" => Ok(Self::NissanLeaf30),
            "jeep4xe" => Ok(Self::Jeep4xe),
            "toyotarav4prime" => Ok(Self::ToyotaRav4Prime),
            "chevyvoltgen1" => Ok(Self::ChevyVoltGen1),
            _ => Err(format!("unknown vehicle: {s}")),
        }
    }
}

pub struct VehicleSpec {
    pub id: VehicleId,
    pub label: &'static str,
    pub capacity_kwh: f64,
    /// AC input power rating for Level 2 charging (kW).
    pub max_l2_power_kw: f64,
    pub dcfc_power_kw: Option<f64>,
    pub chemistry: BatteryChemistry,
    pub range_miles: f64,
    /// EPA combined wall-to-wheels fuel economy (kWh per mile).
    /// Sourced from fueleconomy.gov combined kWh/100mi ÷ 100 for BEVs,
    /// fueleconomy.gov MPGe → 33.7/MPGe for PHEVs with no kWh/100mi data,
    /// and fueleconomy.gov combined kWh/100mi for PHEVs (electric-only).
    /// See docs/reviews/der-catalog/dercat-04-ev-vehicle-spec-catalog.md References.
    pub fuel_economy_kwh_per_mi: f64,
    pub degradation_per_year: f64,
    pub vehicle_type: VehicleType,
}

impl VehicleSpec {
    pub fn to_config(&self) -> crate::Result<EquipmentConfig> {
        let fuel_economy = self.fuel_economy_kwh_per_mi;

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            let dc_kwh_per_mi = self.capacity_kwh / self.range_miles;
            assert!(
                fuel_economy >= dc_kwh_per_mi,
                "{}: fuel_economy_kwh_per_mi ({:.6}) must be >= DC efficiency ({:.6})",
                self.label,
                fuel_economy,
                dc_kwh_per_mi,
            );
        }

        #[cfg(feature = "observe")]
        tracing::debug!(
            vehicle = self.label,
            capacity_kwh = self.capacity_kwh,
            range_miles = self.range_miles,
            dc_kwh_per_mi = self.capacity_kwh / self.range_miles,
            effective_wall_kwh_per_mi = fuel_economy,
            "EV catalog: wall-to-wheels fuel economy",
        );

        EquipmentConfig::from_typed(
            self.label.to_string(),
            "EV".to_string(),
            EvConfig {
                equipment_id: None,
                capacity_kwh: self.capacity_kwh,
                charging_level: Some("L2".to_string()),
                max_charging_power_kw: self.max_l2_power_kw,
                charging_efficiency: None,
                l1_current_a: None,
                l1_voltage_v: None,
                soc_max: None,
                initial_soc: None,
                battery_temp_c: None,
                min_charge_temp_c: None,
                full_power_temp_c: None,
                heater_power_w: None,
                heater_threshold_c: None,
                thermal_mass_j_per_k: None,
                ua_w_per_k: None,
                v2l_enabled: None,
                v2l_soc_reserve: None,
                v2l_max_discharge_kw: None,
                v2g_enabled: None,
                v2g_soc_reserve: None,
                v2g_max_discharge_kw: None,
                chemistry: Some(self.chemistry.as_config_str().to_string()),
                fuel_economy_kwh_per_mi: Some(fuel_economy),
                ready_soc: Some(1.0),
                charging_strategy: None,
                plug_in_policy: None,
                power_limit_kw: None,
                initial_connection_state: None,
                power_factor: None,
                charger_capacity_kva: None,
                cc_cv_transition_soc: None,
            },
        )
    }
}

pub fn by_id(id: &str) -> Option<&'static VehicleSpec> {
    let vid: VehicleId = id.parse().ok()?;
    Some(vid.spec())
}

static CATALOG: &[VehicleSpec] = &[
    VehicleSpec {
        id: VehicleId::TeslaModelYLr,
        label: "Tesla Model Y LR AWD",
        capacity_kwh: 77.0,
        max_l2_power_kw: 11.5,
        dcfc_power_kw: Some(250.0),
        chemistry: BatteryChemistry::Nca,
        range_miles: 310.0,
        fuel_economy_kwh_per_mi: 0.280, // fueleconomy.gov 2023 Model Y LR AWD: 28 kWh/100mi
        degradation_per_year: 0.027,
        vehicle_type: VehicleType::Bev,
    },
    VehicleSpec {
        id: VehicleId::TeslaModelYSr,
        label: "Tesla Model Y AWD (LFP)",
        capacity_kwh: 57.5,
        max_l2_power_kw: 11.5,
        dcfc_power_kw: Some(170.0),
        chemistry: BatteryChemistry::Lfp,
        range_miles: 260.0,
        fuel_economy_kwh_per_mi: 0.274, // fueleconomy.gov 2023 Model Y AWD (SR): 123 MPGe combined → 33.7/123
        degradation_per_year: 0.012,
        vehicle_type: VehicleType::Bev,
    },
    VehicleSpec {
        id: VehicleId::TeslaModel3Lr,
        label: "Tesla Model 3 LR AWD",
        capacity_kwh: 75.0,
        max_l2_power_kw: 11.5,
        dcfc_power_kw: Some(250.0),
        chemistry: BatteryChemistry::Nmc,
        range_miles: 358.0,
        fuel_economy_kwh_per_mi: 0.260, // fueleconomy.gov 2023 Model 3 LR AWD: 26 kWh/100mi
        degradation_per_year: 0.023,
        vehicle_type: VehicleType::Bev,
    },
    VehicleSpec {
        id: VehicleId::ChevyBoltEv,
        label: "Chevy Bolt EV",
        capacity_kwh: 65.0,
        max_l2_power_kw: 11.5,
        dcfc_power_kw: Some(55.0),
        chemistry: BatteryChemistry::Nmc,
        range_miles: 259.0,
        fuel_economy_kwh_per_mi: 0.280, // fueleconomy.gov 2023 Bolt EV: 28 kWh/100mi
        degradation_per_year: 0.023,
        vehicle_type: VehicleType::Bev,
    },
    VehicleSpec {
        id: VehicleId::ChevyBoltEuv,
        label: "Chevy Bolt EUV",
        capacity_kwh: 65.0,
        max_l2_power_kw: 11.5,
        dcfc_power_kw: Some(55.0),
        chemistry: BatteryChemistry::Nmc,
        range_miles: 247.0,
        fuel_economy_kwh_per_mi: 0.290, // fueleconomy.gov 2023 Bolt EUV: 29 kWh/100mi
        degradation_per_year: 0.023,
        vehicle_type: VehicleType::Bev,
    },
    VehicleSpec {
        id: VehicleId::FordMacheSr,
        label: "Ford Mach-E SR RWD",
        capacity_kwh: 72.0,
        max_l2_power_kw: 11.0, // Ford Mach-E OBC: 11 kW AC input (48A × 240V; DC battery output ~10.5 kW). Source: Wikipedia, EV-Database.org.
        dcfc_power_kw: Some(150.0),
        chemistry: BatteryChemistry::Lfp,
        range_miles: 250.0,
        fuel_economy_kwh_per_mi: 0.330, // fueleconomy.gov 2023 Mach-E SR RWD LFP: 33 kWh/100mi
        degradation_per_year: 0.012,
        vehicle_type: VehicleType::Bev,
    },
    VehicleSpec {
        id: VehicleId::FordMacheEr,
        label: "Ford Mach-E ER RWD",
        capacity_kwh: 88.0,
        max_l2_power_kw: 11.0, // Ford Mach-E OBC: 11 kW AC input (48A × 240V; DC battery output ~10.5 kW). Source: Wikipedia, EV-Database.org.
        dcfc_power_kw: Some(150.0),
        chemistry: BatteryChemistry::Nmc,
        range_miles: 312.0,
        fuel_economy_kwh_per_mi: 0.340, // fueleconomy.gov 2023 Mach-E ER RWD: 34 kWh/100mi
        degradation_per_year: 0.023,
        vehicle_type: VehicleType::Bev,
    },
    VehicleSpec {
        id: VehicleId::FordLightningEr,
        label: "Ford F-150 Lightning ER",
        capacity_kwh: 131.0,
        max_l2_power_kw: 11.5,
        dcfc_power_kw: Some(150.0),
        chemistry: BatteryChemistry::Nmc,
        range_miles: 320.0,
        fuel_economy_kwh_per_mi: 0.480, // fueleconomy.gov 2023 F-150 Lightning 4WD ER: 48 kWh/100mi
        degradation_per_year: 0.023,
        vehicle_type: VehicleType::Bev,
    },
    VehicleSpec {
        id: VehicleId::HyundaiIoniq5Lr,
        label: "Hyundai IONIQ 5 LR AWD",
        capacity_kwh: 77.4,
        max_l2_power_kw: 11.0,
        dcfc_power_kw: Some(230.0),
        chemistry: BatteryChemistry::Nmc,
        range_miles: 266.0,
        fuel_economy_kwh_per_mi: 0.300, // fueleconomy.gov 2023 IONIQ 5 LR AWD: 30 kWh/100mi combined, 266 mi EPA range
        degradation_per_year: 0.023,
        vehicle_type: VehicleType::Bev,
    },
    VehicleSpec {
        id: VehicleId::NissanLeaf30,
        label: "Nissan Leaf S 30kWh",
        capacity_kwh: 27.5,
        max_l2_power_kw: 3.3, // S trim standard OBC: 3.6 kW (3.3 kW net). 6.6 kW OBC is SV/SL trim or Quick Charge Package option. Source: Nissan Leaf specifications, docs/reviews/der-catalog/dercat-04-ev-vehicle-spec-catalog.md Finding 4.
        dcfc_power_kw: Some(50.0),
        chemistry: BatteryChemistry::Nmc,
        range_miles: 107.0,
        fuel_economy_kwh_per_mi: 0.300, // fueleconomy.gov 2016 Leaf 30kWh: 30 kWh/100mi
        degradation_per_year: 0.035,
        vehicle_type: VehicleType::Bev,
    },
    VehicleSpec {
        id: VehicleId::Jeep4xe,
        label: "Jeep Wrangler 4xe",
        capacity_kwh: 14.0, // Jeep Wrangler 4xe: 17.3 kWh gross; ~14 kWh usable EV-only (≈3 kWh reserved for hybrid mode). Source: Stellantis/Jeep press release 2021, Jeep 4xe owner documentation.
        max_l2_power_kw: 7.2,
        dcfc_power_kw: None,
        chemistry: BatteryChemistry::Nmc,
        range_miles: 21.0, // fueleconomy.gov 2024 Wrangler 4xe PHEV: 21 mi EPA all-electric range.
        fuel_economy_kwh_per_mi: 0.680, // fueleconomy.gov 2024 Wrangler 4xe: 68 kWh/100mi electric
        degradation_per_year: 0.015,
        vehicle_type: VehicleType::Phev,
    },
    VehicleSpec {
        id: VehicleId::ToyotaRav4Prime,
        label: "Toyota RAV4 Prime",
        capacity_kwh: 14.4,
        max_l2_power_kw: 6.6,
        dcfc_power_kw: None,
        chemistry: BatteryChemistry::Nmc,
        range_miles: 42.0,
        fuel_economy_kwh_per_mi: 0.359, // fueleconomy.gov 2023 RAV4 Prime: 94 MPGe combined → 33.7/94
        degradation_per_year: 0.015,
        vehicle_type: VehicleType::Phev,
    },
    VehicleSpec {
        id: VehicleId::ChevyVoltGen1,
        label: "Chevy Volt Gen1",
        capacity_kwh: 10.9,
        max_l2_power_kw: 3.3,
        dcfc_power_kw: None,
        chemistry: BatteryChemistry::Nmc,
        range_miles: 38.0,
        fuel_economy_kwh_per_mi: 0.350, // fueleconomy.gov 2013-2015 Volt: 35 kWh/100mi
        degradation_per_year: 0.015,
        vehicle_type: VehicleType::Phev,
    },
];

// ── EV Archetype Presets ──────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EvArchetypeId {
    DailyCommuterL2,
    DailyCommuterL1,
    LongCommuterL2,
    WfhOccasional,
    WfhL1Minimal,
    HeavyUseSuv,
    ShiftWorker,
    WeekendWarrior,
    WorkplaceCharger,
    RetireeL1,
    PhevCommuter,
    TouOptimizerCa,
}

impl EvArchetypeId {
    pub const ALL: &[EvArchetypeId] = &[
        Self::DailyCommuterL2,
        Self::DailyCommuterL1,
        Self::LongCommuterL2,
        Self::WfhOccasional,
        Self::WfhL1Minimal,
        Self::HeavyUseSuv,
        Self::ShiftWorker,
        Self::WeekendWarrior,
        Self::WorkplaceCharger,
        Self::RetireeL1,
        Self::PhevCommuter,
        Self::TouOptimizerCa,
    ];

    pub fn preset(self) -> &'static ArchetypePreset {
        &ARCHETYPE_CATALOG[self as usize]
    }
}

impl fmt::Display for EvArchetypeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::DailyCommuterL2 => "DailyCommuterL2",
            Self::DailyCommuterL1 => "DailyCommuterL1",
            Self::LongCommuterL2 => "LongCommuterL2",
            Self::WfhOccasional => "WfhOccasional",
            Self::WfhL1Minimal => "WfhL1Minimal",
            Self::HeavyUseSuv => "HeavyUseSuv",
            Self::ShiftWorker => "ShiftWorker",
            Self::WeekendWarrior => "WeekendWarrior",
            Self::WorkplaceCharger => "WorkplaceCharger",
            Self::RetireeL1 => "RetireeL1",
            Self::PhevCommuter => "PhevCommuter",
            Self::TouOptimizerCa => "TouOptimizerCa",
        };
        write!(f, "{s}")
    }
}

impl std::str::FromStr for EvArchetypeId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized: String = s
            .chars()
            .filter(|c| *c != '_')
            .flat_map(|c| c.to_lowercase())
            .collect();
        match normalized.as_str() {
            "dailycommuterl2" => Ok(Self::DailyCommuterL2),
            "dailycommuterl1" => Ok(Self::DailyCommuterL1),
            "longcommuterl2" => Ok(Self::LongCommuterL2),
            "wfhoccasional" => Ok(Self::WfhOccasional),
            "wfhl1minimal" => Ok(Self::WfhL1Minimal),
            "heavyusesuv" => Ok(Self::HeavyUseSuv),
            "shiftworker" => Ok(Self::ShiftWorker),
            "weekendwarrior" => Ok(Self::WeekendWarrior),
            "workplacecharger" => Ok(Self::WorkplaceCharger),
            "retireel1" => Ok(Self::RetireeL1),
            "phevcommuter" => Ok(Self::PhevCommuter),
            "touoptimizerca" => Ok(Self::TouOptimizerCa),
            _ => Err(format!("unknown EV archetype: {s}")),
        }
    }
}

/// Static preset data for an EV driver archetype.
///
/// Miles schedule parameters are stored as log-normal mu/sigma (log-space)
/// since daily vehicle miles travelled (VMT) follows a right-skewed
/// log-normal distribution in NHTS data, not a symmetric Gaussian.
/// FHWA, "Summary of Travel Trends: 2017 National Household Travel
/// Survey," U.S. DOT (FHWA-PL-18-019), 2018. Ch.2 Table 3b: daily VMT
/// per driver = 28.5 mi; Appendix A Figure A-8: trip-distance distribution
/// up to 50+ mi confirms right-skew. CV ≈ 0.7–0.8 derived from TRPMILES
/// microdata (NHTS Trip File Codebook v1.2, Aug 2020); sigma=0.65 yields
/// CV≈0.725 in log-space.
/// The log-normal has natural [0,∞) support, eliminating the need to clamp
/// negative Gaussian draws. Call `build_miles_schedule` to construct the
/// runtime ScheduleSource from a seed.
///
/// Departure-time and duration parameters remain Gaussian because those
/// quantities are adequately modelled by symmetric distributions.
pub struct ArchetypePreset {
    pub id: EvArchetypeId,
    pub label: &'static str,
    pub charging_level: ChargingLevel,
    pub strategy: ChargingStrategy,
    pub plug_in_policy: PlugInPolicy,
    pub event_day_ratio: f64,
    /// Log-space mu parameter for daily miles log-normal distribution.
    /// Mean = exp(mu + sigma²/2).
    pub daily_drive_miles_mu: f64,
    /// Log-space sigma parameter for daily miles log-normal distribution.
    /// Controls right-skew: CV = sqrt(exp(sigma²) − 1).
    pub daily_drive_miles_sigma: f64,
    pub daily_drive_miles_min: f64,
    /// Weekday miles mu for NoisyTimeWindows archetypes (None for simple).
    pub weekday_miles_mu: Option<f64>,
    /// Weekday miles sigma for NoisyTimeWindows archetypes.
    pub weekday_miles_sigma: Option<f64>,
    /// Weekend miles mu for NoisyTimeWindows archetypes.
    pub weekend_miles_mu: Option<f64>,
    /// Weekend miles sigma for NoisyTimeWindows archetypes.
    pub weekend_miles_sigma: Option<f64>,
    /// Departure time (minute of day): mean and stddev for Gaussian sampling.
    pub departure_minute_mean: f64,
    pub departure_minute_stddev: f64,
    /// Trip duration (minutes): mean and stddev for Gaussian sampling.
    /// Used as the fallback when arrival is not directly sampled.
    pub duration_minutes_mean: f64,
    pub duration_minutes_stddev: f64,
    /// Arrival time (minute of day): mean and stddev for Gaussian sampling.
    /// `None` means fall back to the current departure+duration derivation.
    pub arrival_minute_mean: Option<f64>,
    pub arrival_minute_stddev: Option<f64>,
}

impl ArchetypePreset {
    /// Build the runtime `ScheduleSource` for daily miles from this preset.
    ///
    /// Simple archetypes produce a `ScheduleSource::Stochastic` with log-normal
    /// noise and a floor clamp. NoisyTimeWindows archetypes (WfhOccasional,
    /// WeekendWarrior) produce a `ScheduleSource::noisy_time_windows` with
    /// day-dependent log-normal distributions.
    ///
    /// NHTS 2017 data shows daily VMT has strong right skew; a log-normal
    /// with mu ≈ 3.35, sigma ≈ 0.65 yields mean ≈ 35 mi and realistic
    /// 95th/99th percentiles (~80 mi / ~120 mi). Log-normal's natural [0,∞)
    /// support eliminates the negative-draw clamping artefact present with
    /// the original Gaussian parameterisation.
    ///
    /// FHWA, "Summary of Travel Trends: 2017 National Household Travel
    /// Survey," U.S. DOT (FHWA-PL-18-019), 2018. Ch.2 Table 3b: daily VMT
    /// per driver = 28.5 mi; Appendix A Figure A-8: right-skewed trip
    /// distribution. CV ≈ 0.7–0.8 derived from TRPMILES microdata (NHTS
    /// Trip File Codebook v1.2, Aug 2020). sigma=0.65 yields CV≈0.725
    /// within this range. Mu values derived from archetype target means.
    pub fn build_miles_schedule(&self, seed: [u8; 32]) -> ScheduleSource {
        if let (Some(wd_mu), Some(wd_sigma), Some(we_mu), Some(we_sigma)) = (
            self.weekday_miles_mu,
            self.weekday_miles_sigma,
            self.weekend_miles_mu,
            self.weekend_miles_sigma,
        ) {
            // For log-normal, the full distribution is carried via
            // DistributionKind::LogNormal{mu,sigma} with offset=0.0
            // (offset + sample = LogNormal(mu,sigma)). This differs from
            // the old Gaussian approach where offset held the mean and
            // DistributionKind held the spread only.
            let min_val = if self.daily_drive_miles_min > 0.0 {
                Some(self.daily_drive_miles_min)
            } else {
                None
            };
            ScheduleSource::noisy_time_windows(
                vec![
                    TimeWindow::with_noise(
                        DayFilter::Weekdays,
                        0,
                        1440,
                        0.0,
                        DistributionKind::LogNormal {
                            mu: wd_mu,
                            sigma: wd_sigma,
                        },
                        min_val,
                        None,
                    ),
                    TimeWindow::with_noise(
                        DayFilter::Weekends,
                        0,
                        1440,
                        0.0,
                        DistributionKind::LogNormal {
                            mu: we_mu,
                            sigma: we_sigma,
                        },
                        min_val,
                        None,
                    ),
                ],
                Some(0.0),
                seed,
            )
        } else {
            let clamp_min = if self.daily_drive_miles_min > 0.0 {
                Some(self.daily_drive_miles_min)
            } else {
                None
            };
            ScheduleSource::Stochastic {
                kind: DistributionKind::LogNormal {
                    mu: self.daily_drive_miles_mu,
                    sigma: self.daily_drive_miles_sigma,
                },
                seed,
                draw_count: 0,
                rng: ChaCha8Rng::from_seed(seed),
                clamp_min,
                clamp_max: None,
            }
        }
    }

    /// Build a `ScheduleSource` for departure time (minute of day) with Gaussian noise.
    pub fn build_departure_schedule(&self, seed: [u8; 32]) -> ScheduleSource {
        let mut dep_seed = seed;
        dep_seed[0] ^= 0xDE;
        ScheduleSource::Stochastic {
            kind: DistributionKind::Gaussian {
                mean: self.departure_minute_mean,
                std_dev: self.departure_minute_stddev,
            },
            seed: dep_seed,
            draw_count: 0,
            rng: ChaCha8Rng::from_seed(dep_seed),
            clamp_min: Some(0.0),
            clamp_max: Some(1439.0),
        }
    }

    /// Build a `ScheduleSource` for trip duration (minutes) with Gaussian noise.
    pub fn build_duration_schedule(&self, seed: [u8; 32]) -> ScheduleSource {
        let mut dur_seed = seed;
        dur_seed[1] ^= 0xD0;
        ScheduleSource::Stochastic {
            kind: DistributionKind::Gaussian {
                mean: self.duration_minutes_mean,
                std_dev: self.duration_minutes_stddev,
            },
            seed: dur_seed,
            draw_count: 0,
            rng: ChaCha8Rng::from_seed(dur_seed),
            clamp_min: Some(30.0),
            clamp_max: Some(1200.0),
        }
    }

    /// Build a `ScheduleSource` for arrival time (minute of day) with Gaussian noise.
    /// Returns `None` when `arrival_minute_mean` is `None` — callers should fall back
    /// to the current departure+duration derivation.
    pub fn build_arrival_schedule(&self, seed: [u8; 32]) -> Option<ScheduleSource> {
        let mean = self.arrival_minute_mean?;
        let stddev = self.arrival_minute_stddev?;
        let mut arr_seed = seed;
        arr_seed[2] ^= 0xAA;
        Some(ScheduleSource::Stochastic {
            kind: DistributionKind::Gaussian {
                mean,
                std_dev: stddev,
            },
            seed: arr_seed,
            draw_count: 0,
            rng: ChaCha8Rng::from_seed(arr_seed),
            clamp_min: Some(0.0),
            clamp_max: Some(1439.0),
        })
    }
}

pub fn archetype_by_id(id: &str) -> Option<&'static ArchetypePreset> {
    let aid: EvArchetypeId = id.parse().ok()?;
    Some(aid.preset())
}

static ARCHETYPE_CATALOG: &[ArchetypePreset] = &[
    ArchetypePreset {
        id: EvArchetypeId::DailyCommuterL2,
        label: "Daily Commuter L2",
        charging_level: ChargingLevel::L2,
        strategy: ChargingStrategy::Nightly {
            off_peak_start_hour: 22.0,
            off_peak_end_hour: 6.0,
            target_soc: 0.9,
        },
        plug_in_policy: PlugInPolicy::Always,
        event_day_ratio: 0.50,
        // FHWA, "Summary of Travel Trends: 2017 NHTS," (FHWA-PL-18-019),
        // 2018, Ch.2 Table 3b; TRPMILES microdata CV ≈ 0.7–0.8.
        // sigma=0.65 yields CV≈0.725 within this range.
        // mu=3.43 → mean=exp(3.43+0.65²/2)≈38 mi, 95th≈90 mi, 99th≈140 mi.
        daily_drive_miles_mu: 3.43,
        daily_drive_miles_sigma: 0.65,
        daily_drive_miles_min: 0.0,
        weekday_miles_mu: None,
        weekday_miles_sigma: None,
        weekend_miles_mu: None,
        weekend_miles_sigma: None,
        departure_minute_mean: 480.0,
        departure_minute_stddev: 30.0,
        duration_minutes_mean: 600.0,
        duration_minutes_stddev: 60.0,
        // NHTS 2017: arrival peak 17:00-17:30, tighter stddev than the
        // old 67-min effective stddev from departure+duration independent draws.
        arrival_minute_mean: Some(1020.0),
        arrival_minute_stddev: Some(30.0),
    },
    ArchetypePreset {
        id: EvArchetypeId::DailyCommuterL1,
        label: "Daily Commuter L1",
        charging_level: ChargingLevel::L1,
        strategy: ChargingStrategy::Immediate { target_soc: 1.0 },
        plug_in_policy: PlugInPolicy::Always,
        event_day_ratio: 0.90,
        daily_drive_miles_mu: 3.01,
        daily_drive_miles_sigma: 0.65,
        daily_drive_miles_min: 0.0,
        weekday_miles_mu: None,
        weekday_miles_sigma: None,
        weekend_miles_mu: None,
        weekend_miles_sigma: None,
        departure_minute_mean: 480.0,
        departure_minute_stddev: 30.0,
        duration_minutes_mean: 600.0,
        duration_minutes_stddev: 60.0,
        // NHTS 2017: arrival peak 17:00-17:30.
        arrival_minute_mean: Some(1020.0),
        arrival_minute_stddev: Some(30.0),
    },
    ArchetypePreset {
        id: EvArchetypeId::LongCommuterL2,
        label: "Long Commuter L2",
        charging_level: ChargingLevel::L2,
        strategy: ChargingStrategy::Immediate { target_soc: 1.0 },
        plug_in_policy: PlugInPolicy::Always,
        event_day_ratio: 0.70,
        // Floor of 10 mi retained as a genuine minimum-commute constraint.
        daily_drive_miles_mu: 4.11,
        daily_drive_miles_sigma: 0.65,
        daily_drive_miles_min: 10.0,
        weekday_miles_mu: None,
        weekday_miles_sigma: None,
        weekend_miles_mu: None,
        weekend_miles_sigma: None,
        departure_minute_mean: 420.0,
        departure_minute_stddev: 30.0,
        duration_minutes_mean: 660.0,
        duration_minutes_stddev: 60.0,
        // NHTS 2017: long commuters arrive slightly later ~17:30, wider spread.
        arrival_minute_mean: Some(1050.0),
        arrival_minute_stddev: Some(45.0),
    },
    ArchetypePreset {
        id: EvArchetypeId::WfhOccasional,
        label: "WFH Occasional",
        charging_level: ChargingLevel::L2,
        strategy: ChargingStrategy::LowSoc {
            threshold: 0.3,
            target_soc: 0.8,
        },
        plug_in_policy: PlugInPolicy::LowSoc { threshold: 0.3 },
        event_day_ratio: 0.20,
        // Simple daily params unused (overridden by weekday/weekend below)
        // when all four weekday/weekend fields are Some.
        // mu=0, sigma=0.65 → trivial fallback; NoisyTimeWindows path wins.
        daily_drive_miles_mu: 0.0,
        daily_drive_miles_sigma: 0.65,
        daily_drive_miles_min: 0.0,
        weekday_miles_mu: Some(2.27),
        weekday_miles_sigma: Some(0.65),
        weekend_miles_mu: Some(3.01),
        weekend_miles_sigma: Some(0.65),
        departure_minute_mean: 600.0,
        departure_minute_stddev: 60.0,
        duration_minutes_mean: 180.0,
        duration_minutes_stddev: 45.0,
        arrival_minute_mean: None,
        arrival_minute_stddev: None,
    },
    ArchetypePreset {
        id: EvArchetypeId::WfhL1Minimal,
        label: "WFH L1 Minimal",
        charging_level: ChargingLevel::L1,
        strategy: ChargingStrategy::Immediate { target_soc: 1.0 },
        plug_in_policy: PlugInPolicy::Always,
        event_day_ratio: 0.90,
        // mu=1.87 → mean=exp(1.87+0.65²/2)≈8 mi, realistic for minimal drivers.
        daily_drive_miles_mu: 1.87,
        daily_drive_miles_sigma: 0.65,
        daily_drive_miles_min: 0.0,
        weekday_miles_mu: None,
        weekday_miles_sigma: None,
        weekend_miles_mu: None,
        weekend_miles_sigma: None,
        departure_minute_mean: 600.0,
        departure_minute_stddev: 60.0,
        duration_minutes_mean: 120.0,
        duration_minutes_stddev: 30.0,
        arrival_minute_mean: None,
        arrival_minute_stddev: None,
    },
    ArchetypePreset {
        id: EvArchetypeId::HeavyUseSuv,
        label: "Heavy-Use SUV",
        charging_level: ChargingLevel::L2,
        strategy: ChargingStrategy::Nightly {
            off_peak_start_hour: 22.0,
            off_peak_end_hour: 6.0,
            target_soc: 0.9,
        },
        plug_in_policy: PlugInPolicy::Always,
        event_day_ratio: 0.60,
        // Floor of 5 mi: heavy-use driver, always at least a short trip.
        daily_drive_miles_mu: 3.80,
        daily_drive_miles_sigma: 0.65,
        daily_drive_miles_min: 5.0,
        weekday_miles_mu: None,
        weekday_miles_sigma: None,
        weekend_miles_mu: None,
        weekend_miles_sigma: None,
        departure_minute_mean: 480.0,
        departure_minute_stddev: 30.0,
        duration_minutes_mean: 600.0,
        duration_minutes_stddev: 60.0,
        // NHTS 2017: heavy-use commuter arrival ~17:00, wider spread.
        arrival_minute_mean: Some(1020.0),
        arrival_minute_stddev: Some(45.0),
    },
    ArchetypePreset {
        id: EvArchetypeId::ShiftWorker,
        label: "Shift Worker",
        charging_level: ChargingLevel::L2,
        strategy: ChargingStrategy::PreDeparture {
            target_soc: 0.9,
            departure_schedule: vec![],
        },
        plug_in_policy: PlugInPolicy::Always,
        event_day_ratio: 0.50,
        daily_drive_miles_mu: 3.19,
        daily_drive_miles_sigma: 0.65,
        daily_drive_miles_min: 0.0,
        weekday_miles_mu: None,
        weekday_miles_sigma: None,
        weekend_miles_mu: None,
        weekend_miles_sigma: None,
        departure_minute_mean: 360.0,
        departure_minute_stddev: 45.0,
        duration_minutes_mean: 540.0,
        duration_minutes_stddev: 60.0,
        arrival_minute_mean: None,
        arrival_minute_stddev: None,
    },
    ArchetypePreset {
        id: EvArchetypeId::WeekendWarrior,
        label: "Weekend Warrior",
        charging_level: ChargingLevel::L2,
        strategy: ChargingStrategy::LowSoc {
            threshold: 0.3,
            target_soc: 0.8,
        },
        plug_in_policy: PlugInPolicy::LowSoc { threshold: 0.3 },
        event_day_ratio: 0.30,
        // Simple daily params unused (overridden by weekday/weekend).
        daily_drive_miles_mu: 0.0,
        daily_drive_miles_sigma: 0.65,
        daily_drive_miles_min: 0.0,
        weekday_miles_mu: Some(2.09),
        weekday_miles_sigma: Some(0.65),
        weekend_miles_mu: Some(3.19),
        weekend_miles_sigma: Some(0.65),
        departure_minute_mean: 540.0,
        departure_minute_stddev: 60.0,
        duration_minutes_mean: 480.0,
        duration_minutes_stddev: 90.0,
        arrival_minute_mean: None,
        arrival_minute_stddev: None,
    },
    ArchetypePreset {
        id: EvArchetypeId::WorkplaceCharger,
        label: "Workplace Charger",
        charging_level: ChargingLevel::L1,
        strategy: ChargingStrategy::Immediate { target_soc: 1.0 },
        plug_in_policy: PlugInPolicy::Always,
        event_day_ratio: 0.85,
        daily_drive_miles_mu: 3.19,
        daily_drive_miles_sigma: 0.65,
        daily_drive_miles_min: 0.0,
        weekday_miles_mu: None,
        weekday_miles_sigma: None,
        weekend_miles_mu: None,
        weekend_miles_sigma: None,
        departure_minute_mean: 480.0,
        departure_minute_stddev: 30.0,
        duration_minutes_mean: 600.0,
        duration_minutes_stddev: 60.0,
        arrival_minute_mean: None,
        arrival_minute_stddev: None,
    },
    ArchetypePreset {
        id: EvArchetypeId::RetireeL1,
        label: "Retiree L1",
        charging_level: ChargingLevel::L1,
        strategy: ChargingStrategy::Immediate { target_soc: 1.0 },
        plug_in_policy: PlugInPolicy::Always,
        event_day_ratio: 0.90,
        // mu=2.09 → mean=exp(2.09+0.65²/2)≈10 mi, realistic for retirees.
        daily_drive_miles_mu: 2.09,
        daily_drive_miles_sigma: 0.65,
        daily_drive_miles_min: 0.0,
        weekday_miles_mu: None,
        weekday_miles_sigma: None,
        weekend_miles_mu: None,
        weekend_miles_sigma: None,
        departure_minute_mean: 600.0,
        departure_minute_stddev: 90.0,
        duration_minutes_mean: 180.0,
        duration_minutes_stddev: 60.0,
        arrival_minute_mean: None,
        arrival_minute_stddev: None,
    },
    ArchetypePreset {
        id: EvArchetypeId::PhevCommuter,
        label: "PHEV Commuter",
        charging_level: ChargingLevel::L2,
        strategy: ChargingStrategy::Nightly {
            off_peak_start_hour: 22.0,
            off_peak_end_hour: 6.0,
            target_soc: 0.9,
        },
        plug_in_policy: PlugInPolicy::Always,
        event_day_ratio: 0.50,
        daily_drive_miles_mu: 3.34,
        daily_drive_miles_sigma: 0.65,
        daily_drive_miles_min: 0.0,
        weekday_miles_mu: None,
        weekday_miles_sigma: None,
        weekend_miles_mu: None,
        weekend_miles_sigma: None,
        departure_minute_mean: 480.0,
        departure_minute_stddev: 30.0,
        duration_minutes_mean: 600.0,
        duration_minutes_stddev: 60.0,
        // NHTS 2017: PHEV commuter arrival ~17:00.
        arrival_minute_mean: Some(1020.0),
        arrival_minute_stddev: Some(30.0),
    },
    ArchetypePreset {
        id: EvArchetypeId::TouOptimizerCa,
        label: "TOU Optimizer CA",
        charging_level: ChargingLevel::L2,
        strategy: ChargingStrategy::TouAware {
            target_soc: 0.9,
            departure_schedule: vec![],
            charge_buffer_hours: 2.0,
        },
        plug_in_policy: PlugInPolicy::Always,
        event_day_ratio: 0.50,
        daily_drive_miles_mu: 3.43,
        daily_drive_miles_sigma: 0.65,
        daily_drive_miles_min: 0.0,
        weekday_miles_mu: None,
        weekday_miles_sigma: None,
        weekend_miles_mu: None,
        weekend_miles_sigma: None,
        departure_minute_mean: 480.0,
        departure_minute_stddev: 30.0,
        duration_minutes_mean: 600.0,
        duration_minutes_stddev: 60.0,
        // NHTS 2017: TOU-optimizer commuter arrival ~17:00.
        arrival_minute_mean: Some(1020.0),
        arrival_minute_stddev: Some(30.0),
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;
    use chrono::{FixedOffset, TimeZone};
    use hares_types::{EnvironmentState, GridState, WeatherState, ZoneState};

    fn sample_env() -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: hares_types::ZoneId(1),
                temperature_c: 21.0,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 10.0,
                outdoor_humidity_ratio: 0.005,
                outdoor_wet_bulb_c: 10.0,
                outdoor_enthalpy_j_kg: 0.0,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 12.0,
                sky_temp_c: 8.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![],
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                solar_altitude_deg: 0.0,
                solar_azimuth_deg: 180.0,
                mains_temp_c: 15.0,
                rainfall_m: 0.0,
                ground_albedo: 0.2,
                ground_t_mean_c: 10.0,
                ground_t_amplitude_c: 0.0,
                ground_phase_day: 35.0,
                day_of_year: 1.0,
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
                island_bus_voltage_pu: None,
            },
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .unwrap(),
            time_res: ChronoDuration::minutes(1),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    #[test]
    fn all_catalog_specs_valid() {
        assert_eq!(CATALOG.len(), 13);
        for (i, spec) in CATALOG.iter().enumerate() {
            assert_eq!(
                spec.id as usize, i,
                "catalog order mismatch for {}",
                spec.label
            );
            assert!(
                spec.capacity_kwh > 0.0,
                "{}: capacity must be positive",
                spec.label
            );
            assert!(
                spec.max_l2_power_kw > 0.0,
                "{}: L2 power must be positive",
                spec.label
            );
            assert!(
                spec.range_miles > 0.0,
                "{}: range must be positive",
                spec.label
            );
            assert!(
                spec.degradation_per_year > 0.0 && spec.degradation_per_year < 1.0,
                "{}: degradation must be in (0,1)",
                spec.label
            );
            assert!(
                spec.fuel_economy_kwh_per_mi > 0.0,
                "{}: fuel_economy_kwh_per_mi must be positive",
                spec.label
            );
            let dc = spec.capacity_kwh / spec.range_miles;
            assert!(
                spec.fuel_economy_kwh_per_mi >= dc,
                "{}: fuel_economy_kwh_per_mi ({:.6}) must be >= DC eff ({:.6})",
                spec.label,
                spec.fuel_economy_kwh_per_mi,
                dc,
            );
        }
    }

    #[test]
    fn phev_vehicles_have_no_dcfc() {
        for spec in CATALOG
            .iter()
            .filter(|s| s.vehicle_type == VehicleType::Phev)
        {
            assert!(
                spec.dcfc_power_kw.is_none(),
                "{}: PHEV should not have DCFC",
                spec.label
            );
        }
    }

    #[test]
    fn bev_count_and_phev_count() {
        let bev_count = CATALOG
            .iter()
            .filter(|s| s.vehicle_type == VehicleType::Bev)
            .count();
        let phev_count = CATALOG
            .iter()
            .filter(|s| s.vehicle_type == VehicleType::Phev)
            .count();
        assert_eq!(bev_count, 10);
        assert_eq!(phev_count, 3);
    }

    #[test]
    fn from_str_round_trips() {
        for &id in VehicleId::ALL {
            let s = id.to_string();
            let parsed: VehicleId = s.parse().unwrap();
            assert_eq!(parsed, id);
        }
    }

    #[test]
    fn from_str_case_insensitive_underscore_tolerant() {
        assert_eq!(
            "tesla_model_y_lr".parse::<VehicleId>().unwrap(),
            VehicleId::TeslaModelYLr
        );
        assert_eq!(
            "TESLA_MODEL_Y_LR".parse::<VehicleId>().unwrap(),
            VehicleId::TeslaModelYLr
        );
        assert_eq!(
            "TeslaModelYLr".parse::<VehicleId>().unwrap(),
            VehicleId::TeslaModelYLr
        );
        assert_eq!(
            "chevy_volt_gen1".parse::<VehicleId>().unwrap(),
            VehicleId::ChevyVoltGen1
        );
    }

    #[test]
    fn by_id_lookup() {
        let spec = by_id("tesla_model_y_lr").unwrap();
        assert_eq!(spec.id, VehicleId::TeslaModelYLr);
        assert!((spec.capacity_kwh - 77.0).abs() < f64::EPSILON);
        assert!(by_id("nonexistent").is_none());
    }

    #[test]
    fn to_config_sets_fuel_economy() {
        let spec = VehicleId::TeslaModelYLr.spec();
        let cfg = spec.to_config().unwrap();
        let typed: EvConfig = cfg.typed().unwrap();
        let fuel_econ = typed.fuel_economy_kwh_per_mi.unwrap();
        // EPA wall-to-wheels: 0.280 kWh/mi (fueleconomy.gov 28 kWh/100mi)
        let expected = spec.fuel_economy_kwh_per_mi;
        assert!((fuel_econ - expected).abs() < 1e-10);
        assert!(fuel_econ > spec.capacity_kwh / spec.range_miles);
        assert_eq!(typed.chemistry.as_deref(), Some("nca"));
        assert_eq!(typed.charging_level.as_deref(), Some("L2"));
    }

    #[test]
    fn fuel_economy_stored_for_all_vehicles() {
        for &id in VehicleId::ALL {
            let spec = id.spec();
            let cfg = spec.to_config().unwrap();
            let typed: EvConfig = cfg.typed().unwrap();
            let actual = typed.fuel_economy_kwh_per_mi.unwrap();
            let expected = spec.fuel_economy_kwh_per_mi;
            assert!(
                (actual - expected).abs() < 1e-10,
                "{}: got {actual:.6}, expected {expected:.6}",
                spec.label,
            );
            let dc_kwh_per_mi = spec.capacity_kwh / spec.range_miles;
            assert!(
                actual >= dc_kwh_per_mi,
                "{}: wall-to-wheels {actual:.6} must be >= DC efficiency {dc_kwh_per_mi:.6}",
                spec.label,
            );
            // EPA wall-to-wheels must be > DC (charging losses exist).
            assert!(
                actual > dc_kwh_per_mi || spec.vehicle_type == VehicleType::Phev,
                "{}: wall-to-wheels must exceed DC efficiency (charging losses)",
                spec.label,
            );
        }
    }

    #[test]
    fn fuel_economy_sourced_from_epa_ratings() {
        // Each value in the catalog must have a fueleconomy.gov source
        // documented in the inline comment. This test verifies that every
        // vehicle's stored fuel_economy_kwh_per_mi matches the EPA wall-to-wheels
        // value cited in docs/reviews/der-catalog/dercat-04-ev-vehicle-spec-catalog.md
        // and in the fueleconomy.gov inline source comments.

        // EPA data: fueleconomy.gov combined kWh/100mi for BEVs,
        // puedeconomy.gov MPGe → 33.7/MPGe for plug-in hybrids using
        // the EPA conversion of 33.7 kWh per gallon gasoline equivalent.
        let _epa_kwh_per_gallon: f64 = 33.7;

        let expected: &[(VehicleId, f64, &str)] = &[
            (VehicleId::TeslaModelYLr, 0.280, "28 kWh/100mi"),
            (
                VehicleId::TeslaModelYSr,
                0.274,
                "123 MPGe combined → 33.7/123",
            ),
            (VehicleId::TeslaModel3Lr, 0.260, "26 kWh/100mi"),
            (VehicleId::ChevyBoltEv, 0.280, "28 kWh/100mi"),
            (VehicleId::ChevyBoltEuv, 0.290, "29 kWh/100mi"),
            (VehicleId::FordMacheSr, 0.330, "33 kWh/100mi"),
            (VehicleId::FordMacheEr, 0.340, "34 kWh/100mi"),
            (VehicleId::FordLightningEr, 0.480, "48 kWh/100mi"),
            (VehicleId::HyundaiIoniq5Lr, 0.300, "30 kWh/100mi"),
            (VehicleId::NissanLeaf30, 0.300, "30 kWh/100mi"),
            (VehicleId::Jeep4xe, 0.680, "68 kWh/100mi elec"),
            (
                VehicleId::ToyotaRav4Prime,
                0.359,
                "94 MPGe combined → 33.7/94",
            ),
            (VehicleId::ChevyVoltGen1, 0.350, "35 kWh/100mi"),
        ];

        for &(id, epa_value, source) in expected {
            let spec = id.spec();
            let delta = (spec.fuel_economy_kwh_per_mi - epa_value).abs();
            assert!(
                delta < 1e-6,
                "{}: stored {:.6} does not match EPA {:.6} ({}). Update the catalog entry.",
                spec.label,
                spec.fuel_economy_kwh_per_mi,
                epa_value,
                source,
            );
        }
    }

    #[test]
    fn ioniq5_awd_range_is_epa_combined() {
        let range = VehicleId::HyundaiIoniq5Lr.spec().range_miles;
        // EPA combined AWD rating for 2023 IONIQ 5 LR AWD is 266 mi.
        // Acceptable range 250–270 catches regressions (e.g. back to WLTP 303).
        // Source: fueleconomy.gov 2023 IONIQ 5 LR AWD.
        assert!(
            (250.0..=270.0).contains(&range),
            "IONIQ 5 LR AWD range {range} outside expected EPA band 250–270 mi",
        );
    }

    #[test]
    fn wrangler_4xe_phev_all_electric_range_is_epa_21_mi() {
        // EPA all-electric range for the 2024–2025 Wrangler 4xe PHEV is 21 mi.
        // Source: fueleconomy.gov; docs/reviews/der-catalog/dercat-04-ev-vehicle-spec-catalog.md Finding 5.
        assert_eq!(
            VehicleId::Jeep4xe.spec().range_miles,
            21.0,
            "Wrangler 4xe PHEV all-electric range must be exactly 21 mi (EPA)",
        );
    }

    #[test]
    fn mach_e_obc_power_is_11_kw() {
        // Ford Mustang Mach-E onboard charger AC input rating is 11 kW
        // (48A × 240V). Both SR LFP and ER NMC variants use the same OBC.
        // Source: Wikipedia, EV-Database.org. Confirmed in
        // docs/reviews/der-catalog/dercat-04-ev-vehicle-spec-catalog.md Finding 3.
        let sr = VehicleId::FordMacheSr.spec();
        let er = VehicleId::FordMacheEr.spec();
        assert_eq!(sr.max_l2_power_kw, 11.0, "Mach-E SR L2 AC power");
        assert_eq!(er.max_l2_power_kw, 11.0, "Mach-E ER L2 AC power");
    }

    #[test]
    fn leaf_s_obc_power_is_3_3_kw() {
        // Nissan Leaf S 30kWh base trim came standard with a 3.3 kW (3.6 kW)
        // onboard charger. The 6.6 kW charger was standard only on SV and SL
        // trims, or as part of the Quick Charge Package option on the S trim.
        // Source: Nissan Leaf specifications. Confirmed in
        // docs/reviews/der-catalog/dercat-04-ev-vehicle-spec-catalog.md Finding 4.
        let spec = VehicleId::NissanLeaf30.spec();
        assert_eq!(spec.max_l2_power_kw, 3.3, "Nissan Leaf S 30kWh L2 AC power");
    }

    #[test]
    fn rav4_prime_obc_power_is_6_6_kw() {
        // Toyota RAV4 Prime standard OBC is 6.6 kW for 2022+ model years
        // (all trims) and 2021 XSE. Only the 2021 SE base model had 3.3 kW.
        // 6.6 kW is the most representative default.
        // Source: Toyota specifications, Wikipedia. Confirmed in
        // docs/reviews/der-catalog/dercat-04-ev-vehicle-spec-catalog.md Finding 6.
        let spec = VehicleId::ToyotaRav4Prime.spec();
        assert_eq!(spec.max_l2_power_kw, 6.6, "Toyota RAV4 Prime L2 AC power");
    }

    // ── Archetype tests ──────────────────────────────────────────────

    #[test]
    fn all_archetype_presets_valid() {
        assert_eq!(ARCHETYPE_CATALOG.len(), 12);
        for (i, preset) in ARCHETYPE_CATALOG.iter().enumerate() {
            assert_eq!(
                preset.id as usize, i,
                "archetype catalog order mismatch for {}",
                preset.label
            );
            assert!(
                preset.event_day_ratio > 0.0 && preset.event_day_ratio <= 1.0,
                "{}: event_day_ratio must be in (0,1]",
                preset.label
            );
        }
    }

    #[test]
    fn archetype_from_str_round_trips() {
        for &id in EvArchetypeId::ALL {
            let s = id.to_string();
            let parsed: EvArchetypeId = s.parse().unwrap();
            assert_eq!(parsed, id);
        }
    }

    #[test]
    fn archetype_from_str_case_insensitive() {
        assert_eq!(
            "daily_commuter_l2".parse::<EvArchetypeId>().unwrap(),
            EvArchetypeId::DailyCommuterL2
        );
        assert_eq!(
            "DAILY_COMMUTER_L2".parse::<EvArchetypeId>().unwrap(),
            EvArchetypeId::DailyCommuterL2
        );
        assert_eq!(
            "WfhOccasional".parse::<EvArchetypeId>().unwrap(),
            EvArchetypeId::WfhOccasional
        );
    }

    #[test]
    fn archetype_by_id_lookup() {
        let preset = archetype_by_id("daily_commuter_l2").unwrap();
        assert_eq!(preset.id, EvArchetypeId::DailyCommuterL2);
        assert!(archetype_by_id("nonexistent").is_none());
    }

    #[test]
    fn archetype_strategy_variants_correct() {
        assert!(matches!(
            EvArchetypeId::DailyCommuterL2.preset().strategy,
            ChargingStrategy::Nightly { .. }
        ));
        assert!(matches!(
            EvArchetypeId::DailyCommuterL1.preset().strategy,
            ChargingStrategy::Immediate { .. }
        ));
        assert!(matches!(
            EvArchetypeId::WfhOccasional.preset().strategy,
            ChargingStrategy::LowSoc { .. }
        ));
        assert!(matches!(
            EvArchetypeId::ShiftWorker.preset().strategy,
            ChargingStrategy::PreDeparture { .. }
        ));
        assert!(matches!(
            EvArchetypeId::TouOptimizerCa.preset().strategy,
            ChargingStrategy::TouAware { .. }
        ));
    }

    #[test]
    fn archetype_build_miles_schedule_deterministic() {
        let preset = EvArchetypeId::DailyCommuterL2.preset();
        let seed = [42u8; 32];
        let s1 = preset.build_miles_schedule(seed);
        let s2 = preset.build_miles_schedule(seed);
        assert_eq!(s1, s2);
    }

    #[test]
    fn archetype_noisy_tw_archetypes_have_weekday_weekend() {
        for &id in &[EvArchetypeId::WfhOccasional, EvArchetypeId::WeekendWarrior] {
            let preset = id.preset();
            assert!(
                preset.weekday_miles_mu.is_some(),
                "{}: should have weekday_miles_mu",
                preset.label
            );
            assert!(
                preset.weekend_miles_mu.is_some(),
                "{}: should have weekend_miles_mu",
                preset.label
            );
        }
    }

    #[test]
    fn archetype_all_presets_have_log_normal_params() {
        for &id in EvArchetypeId::ALL {
            let preset = id.preset();
            assert!(
                preset.daily_drive_miles_sigma > 0.0,
                "{}: sigma must be positive for LogNormal",
                preset.label
            );
            assert!(
                preset.daily_drive_miles_mu.is_finite(),
                "{}: mu must be finite",
                preset.label
            );
        }
    }

    #[test]
    fn build_miles_schedule_emits_log_normal() {
        for &id in EvArchetypeId::ALL {
            let preset = id.preset();
            let schedule = preset.build_miles_schedule([0u8; 32]);
            match schedule {
                ScheduleSource::Stochastic { kind, .. } => {
                    assert!(
                        matches!(kind, DistributionKind::LogNormal { .. }),
                        "{}: expected LogNormal, got {:?}",
                        preset.label,
                        kind
                    );
                }
                ScheduleSource::TimeWindows { windows, .. } => {
                    for w in &windows {
                        if let Some(ref noise) = w.noise {
                            assert!(
                                matches!(noise, DistributionKind::LogNormal { .. }),
                                "{}: expected LogNormal in TimeWindows noise, got {:?}",
                                preset.label,
                                noise
                            );
                        }
                    }
                }
                _ => panic!("{}: unexpected ScheduleSource variant", preset.label),
            }
        }
    }

    #[test]
    fn log_normal_analytical_mean_preserves_calibration() {
        // Verify that switching from Gaussian to LogNormal preserves the
        // intended fleet mean within ±5% for all simple (non-TW) archetypes.
        // Original Gaussian means serve as the calibration target.
        let calibration_targets: &[(EvArchetypeId, f64)] = &[
            (EvArchetypeId::DailyCommuterL2, 38.0),
            (EvArchetypeId::DailyCommuterL1, 25.0),
            (EvArchetypeId::LongCommuterL2, 75.0),
            (EvArchetypeId::WfhL1Minimal, 8.0),
            (EvArchetypeId::HeavyUseSuv, 55.0),
            (EvArchetypeId::ShiftWorker, 30.0),
            (EvArchetypeId::WorkplaceCharger, 30.0),
            (EvArchetypeId::RetireeL1, 10.0),
            (EvArchetypeId::PhevCommuter, 35.0),
            (EvArchetypeId::TouOptimizerCa, 38.0),
        ];
        for &(id, target) in calibration_targets {
            let preset = id.preset();
            let schedule = preset.build_miles_schedule([1u8; 32]);
            let mean = schedule.mean();
            let rel_error = (mean - target).abs() / target;
            assert!(
                rel_error < 0.05,
                "{}: analytical mean {:.3} deviates from target {:.1} by {:.1}%",
                preset.label,
                mean,
                target,
                rel_error * 100.0
            );
        }
    }

    #[test]
    fn log_normal_never_produces_negative_draws() {
        let preset = EvArchetypeId::DailyCommuterL2.preset();
        let mut schedule = preset.build_miles_schedule([42u8; 32]);
        let env = sample_env();
        for _ in 0..10_000 {
            let v = schedule.value_at(&env).unwrap();
            assert!(v >= 0.0, "log-normal draw produced negative value: {v}");
        }
    }

    #[test]
    fn log_normal_empirical_moments_daily_commuter_l2() {
        // Sample 100_000 draws from DailyCommuterL2 log-normal and verify:
        // - mean ≈ 38 mi (within 5%)
        // - median < mean (right skew confirmed)
        // - P(draw < 0) = 0
        // - 95th percentile ≤ 95 mi  (theoretical ≈ 90 mi; 95 allows sampling noise)
        // - 99th percentile ≤ 160 mi (theoretical ≈ 140 mi; 160 allows sampling noise)
        let preset = EvArchetypeId::DailyCommuterL2.preset();
        let mut schedule = preset.build_miles_schedule([7u8; 32]);
        let env = sample_env();

        let n = 100_000;
        let mut samples: Vec<f64> = Vec::with_capacity(n);
        for _ in 0..n {
            samples.push(schedule.value_at(&env).unwrap().max(0.0));
        }

        let mean: f64 = samples.iter().sum::<f64>() / n as f64;
        assert!(
            (mean - 38.0).abs() / 38.0 < 0.05,
            "empirical mean {mean:.2} deviates from 38.0 beyond 5%"
        );

        samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = samples[n / 2];
        assert!(
            median < mean,
            "median {median:.2} not less than mean {mean:.2} (right skew not confirmed)"
        );

        // P(draw < 0) = 0 by log-normal support
        assert!(samples.first().unwrap() >= &0.0);

        let p95 = samples[(n as f64 * 0.95) as usize];
        assert!(
            p95 <= 95.0,
            "95th percentile {p95:.1} exceeds 95 mi threshold"
        );

        let p99 = samples[(n as f64 * 0.99) as usize];
        assert!(
            p99 <= 160.0,
            "99th percentile {p99:.1} exceeds 160 mi threshold"
        );
    }

    #[test]
    fn noisy_time_windows_mean_is_nonzero() {
        // Regression: before the ScheduleSource::mean() fix for TimeWindows
        // with noise, WfhOccasional and WeekendWarrior returned mean()=0.0
        // because only the `value` field was summed (both are 0.0) and the
        // log-normal distribution mean was ignored. Verify both archetypes
        // now return a realistic non-zero daily miles mean.
        for &id in &[EvArchetypeId::WfhOccasional, EvArchetypeId::WeekendWarrior] {
            let preset = id.preset();
            let schedule = preset.build_miles_schedule([1u8; 32]);
            let mean = schedule.mean();
            assert!(
                mean > 5.0,
                "{}: mean()={mean} should be >5 mi/day (was 0 before noise-aware fix)",
                preset.label
            );
        }
    }

    // ── Arrival-time sampling tests ──────────────────────────────────

    #[test]
    fn build_arrival_schedule_returns_some_for_commuter_presets() {
        let commuter_ids = &[
            EvArchetypeId::DailyCommuterL2,
            EvArchetypeId::DailyCommuterL1,
            EvArchetypeId::LongCommuterL2,
            EvArchetypeId::HeavyUseSuv,
            EvArchetypeId::PhevCommuter,
            EvArchetypeId::TouOptimizerCa,
        ];
        for &id in commuter_ids {
            let preset = id.preset();
            assert!(
                preset.arrival_minute_mean.is_some(),
                "{}: commuter preset should have arrival_minute_mean set",
                preset.label
            );
            assert!(
                preset.arrival_minute_stddev.is_some(),
                "{}: commuter preset should have arrival_minute_stddev set",
                preset.label
            );
            let schedule = preset.build_arrival_schedule([42u8; 32]);
            assert!(
                schedule.is_some(),
                "{}: build_arrival_schedule should return Some for commuter preset",
                preset.label
            );
        }
    }

    #[test]
    fn build_arrival_schedule_returns_none_for_non_commuter_presets() {
        let non_commuter_ids = &[
            EvArchetypeId::WfhOccasional,
            EvArchetypeId::WfhL1Minimal,
            EvArchetypeId::ShiftWorker,
            EvArchetypeId::WeekendWarrior,
            EvArchetypeId::WorkplaceCharger,
            EvArchetypeId::RetireeL1,
        ];
        for &id in non_commuter_ids {
            let preset = id.preset();
            assert!(
                preset.arrival_minute_mean.is_none(),
                "{}: non-commuter preset should have arrival_minute_mean=None, got Some",
                preset.label
            );
            assert!(
                preset.arrival_minute_stddev.is_none(),
                "{}: non-commuter preset should have arrival_minute_stddev=None, got Some",
                preset.label
            );
            assert!(
                preset.build_arrival_schedule([42u8; 32]).is_none(),
                "{}: build_arrival_schedule should return None for non-commuter preset",
                preset.label
            );
        }
    }

    /// For DailyCommuterL2 with arrival mean 1020 (17:00) and stddev 30,
    /// sample 100,000 arrival times and verify mean ≈ 1020 and
    /// P(arrival < departure) ≈ 0 (given departure at 480±30, the gap is
    /// large enough that no arrival sample should precede departure).
    /// NHTS 2017 daily VMT data confirms morning departures cluster at
    /// 7:00–8:00 with evening arrivals at 17:00–17:30; the ~540-minute
    /// gap makes arrival-before-departure astronomically unlikely.
    #[test]
    fn daily_commuter_l2_arrival_samples_match_nhts_params() {
        let preset = EvArchetypeId::DailyCommuterL2.preset();
        let mut schedule = preset.build_arrival_schedule([42u8; 32]).unwrap();
        let env = sample_env();

        let n = 100_000;
        let mut samples: Vec<f64> = Vec::with_capacity(n);
        for _ in 0..n {
            let sample = schedule.value_at(&env).unwrap().clamp(0.0, 1439.0);
            samples.push(sample);
        }

        let mean: f64 = samples.iter().sum::<f64>() / n as f64;
        // arrival_minute_mean = 1020; allow 2% sampling noise at 100k draws.
        assert!(
            (mean - 1020.0).abs() / 1020.0 < 0.05,
            "arrival empirical mean {mean:.1} deviates from 1020.0 beyond 5%"
        );

        // stddev ≈ 30; allow generous sampling noise.
        let variance: f64 =
            samples.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1) as f64;
        let stddev = variance.sqrt();
        assert!(
            stddev > 20.0 && stddev < 50.0,
            "arrival empirical stddev {stddev:.1} outside [20, 50] band"
        );

        // Verify NO samples are before the earliest plausible departure.
        // Departure mean is 480 (08:00) with stddev 30; 6σ departure upper
        // bound is ~660 (11:00). Arrival before departure would require a
        // draw below 660, which for N(1020,30) is many σ away.
        let departure_upper_bound = 660.0;
        let early_arrivals = samples
            .iter()
            .filter(|&&x| x < departure_upper_bound)
            .count();
        assert!(
            early_arrivals == 0,
            "found {early_arrivals} arrival samples before 660-min departure upper bound"
        );
    }

    #[test]
    fn arrival_schedule_deterministic_given_same_seed() {
        let preset = EvArchetypeId::DailyCommuterL2.preset();
        let seed = [42u8; 32];
        let s1 = preset.build_arrival_schedule(seed);
        let s2 = preset.build_arrival_schedule(seed);
        assert_eq!(
            s1, s2,
            "build_arrival_schedule must be deterministic given same seed"
        );
    }
}
