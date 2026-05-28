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
    pub fn to_config(&self) -> EquipmentConfig {
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
        max_l2_power_kw: 11.5,
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
        max_l2_power_kw: 11.5,
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
        max_l2_power_kw: 6.6,
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
        capacity_kwh: 15.0,
        max_l2_power_kw: 7.2,
        dcfc_power_kw: None,
        chemistry: BatteryChemistry::Nmc,
        range_miles: 25.0,
        fuel_economy_kwh_per_mi: 0.680, // fueleconomy.gov 2024 Wrangler 4xe: 68 kWh/100mi electric
        degradation_per_year: 0.015,
        vehicle_type: VehicleType::Phev,
    },
    VehicleSpec {
        id: VehicleId::ToyotaRav4Prime,
        label: "Toyota RAV4 Prime",
        capacity_kwh: 14.4,
        max_l2_power_kw: 3.3,
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
/// Miles schedule parameters are stored as plain scalars since ScheduleSource
/// contains RNG state and cannot be `const`. Call `build_miles_schedule` to
/// construct the runtime ScheduleSource from a seed.
pub struct ArchetypePreset {
    pub id: EvArchetypeId,
    pub label: &'static str,
    pub charging_level: ChargingLevel,
    pub strategy: ChargingStrategy,
    pub plug_in_policy: PlugInPolicy,
    pub event_day_ratio: f64,
    pub arrival_fuzz_minutes: f64,
    pub departure_fuzz_minutes: f64,
    pub daily_drive_miles_mean: f64,
    pub daily_drive_miles_stddev: f64,
    pub daily_drive_miles_min: f64,
    /// Weekday miles mean for NoisyTimeWindows archetypes (None for simple).
    pub weekday_miles_mean: Option<f64>,
    /// Weekday miles stddev for NoisyTimeWindows archetypes.
    pub weekday_miles_stddev: Option<f64>,
    /// Weekend miles mean for NoisyTimeWindows archetypes.
    pub weekend_miles_mean: Option<f64>,
    /// Weekend miles stddev for NoisyTimeWindows archetypes.
    pub weekend_miles_stddev: Option<f64>,
    /// Departure time (minute of day): mean and stddev for Gaussian sampling.
    pub departure_minute_mean: f64,
    pub departure_minute_stddev: f64,
    /// Trip duration (minutes): mean and stddev for Gaussian sampling.
    pub duration_minutes_mean: f64,
    pub duration_minutes_stddev: f64,
}

impl ArchetypePreset {
    /// Build the runtime `ScheduleSource` for daily miles from this preset.
    ///
    /// Simple archetypes produce a `ScheduleSource::Stochastic` with Gaussian
    /// noise and a clamp floor. NoisyTimeWindows archetypes (WfhOccasional,
    /// WeekendWarrior) produce a `ScheduleSource::noisy_time_windows` with
    /// day-dependent Gaussian distributions.
    pub fn build_miles_schedule(&self, seed: [u8; 32]) -> ScheduleSource {
        if let (Some(wd_mean), Some(wd_std), Some(we_mean), Some(we_std)) = (
            self.weekday_miles_mean,
            self.weekday_miles_stddev,
            self.weekend_miles_mean,
            self.weekend_miles_stddev,
        ) {
            let min_val = if self.daily_drive_miles_min > 0.0 {
                Some(self.daily_drive_miles_min)
            } else {
                Some(0.0)
            };
            ScheduleSource::noisy_time_windows(
                vec![
                    TimeWindow::with_noise(
                        DayFilter::Weekdays,
                        0,
                        1440,
                        wd_mean,
                        DistributionKind::Gaussian {
                            mean: 0.0,
                            std_dev: wd_std,
                        },
                        min_val,
                        None,
                    ),
                    TimeWindow::with_noise(
                        DayFilter::Weekends,
                        0,
                        1440,
                        we_mean,
                        DistributionKind::Gaussian {
                            mean: 0.0,
                            std_dev: we_std,
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
                Some(0.0)
            };
            ScheduleSource::Stochastic {
                kind: DistributionKind::Gaussian {
                    mean: self.daily_drive_miles_mean,
                    std_dev: self.daily_drive_miles_stddev,
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
        arrival_fuzz_minutes: 30.0,
        departure_fuzz_minutes: 30.0,
        daily_drive_miles_mean: 38.0,
        daily_drive_miles_stddev: 13.0,
        daily_drive_miles_min: 0.0,
        weekday_miles_mean: None,
        weekday_miles_stddev: None,
        weekend_miles_mean: None,
        weekend_miles_stddev: None,
        departure_minute_mean: 480.0,
        departure_minute_stddev: 30.0,
        duration_minutes_mean: 600.0,
        duration_minutes_stddev: 60.0,
    },
    ArchetypePreset {
        id: EvArchetypeId::DailyCommuterL1,
        label: "Daily Commuter L1",
        charging_level: ChargingLevel::L1,
        strategy: ChargingStrategy::Immediate { target_soc: 1.0 },
        plug_in_policy: PlugInPolicy::Always,
        event_day_ratio: 0.90,
        arrival_fuzz_minutes: 30.0,
        departure_fuzz_minutes: 30.0,
        daily_drive_miles_mean: 25.0,
        daily_drive_miles_stddev: 9.0,
        daily_drive_miles_min: 0.0,
        weekday_miles_mean: None,
        weekday_miles_stddev: None,
        weekend_miles_mean: None,
        weekend_miles_stddev: None,
        departure_minute_mean: 480.0,
        departure_minute_stddev: 30.0,
        duration_minutes_mean: 600.0,
        duration_minutes_stddev: 60.0,
    },
    ArchetypePreset {
        id: EvArchetypeId::LongCommuterL2,
        label: "Long Commuter L2",
        charging_level: ChargingLevel::L2,
        strategy: ChargingStrategy::Immediate { target_soc: 1.0 },
        plug_in_policy: PlugInPolicy::Always,
        event_day_ratio: 0.70,
        arrival_fuzz_minutes: 30.0,
        departure_fuzz_minutes: 30.0,
        daily_drive_miles_mean: 75.0,
        daily_drive_miles_stddev: 20.0,
        daily_drive_miles_min: 10.0,
        weekday_miles_mean: None,
        weekday_miles_stddev: None,
        weekend_miles_mean: None,
        weekend_miles_stddev: None,
        departure_minute_mean: 420.0,
        departure_minute_stddev: 30.0,
        duration_minutes_mean: 660.0,
        duration_minutes_stddev: 60.0,
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
        arrival_fuzz_minutes: 45.0,
        departure_fuzz_minutes: 45.0,
        daily_drive_miles_mean: 0.0,
        daily_drive_miles_stddev: 0.0,
        daily_drive_miles_min: 0.0,
        weekday_miles_mean: Some(12.0),
        weekday_miles_stddev: Some(6.0),
        weekend_miles_mean: Some(25.0),
        weekend_miles_stddev: Some(10.0),
        departure_minute_mean: 600.0,
        departure_minute_stddev: 60.0,
        duration_minutes_mean: 180.0,
        duration_minutes_stddev: 45.0,
    },
    ArchetypePreset {
        id: EvArchetypeId::WfhL1Minimal,
        label: "WFH L1 Minimal",
        charging_level: ChargingLevel::L1,
        strategy: ChargingStrategy::Immediate { target_soc: 1.0 },
        plug_in_policy: PlugInPolicy::Always,
        event_day_ratio: 0.90,
        arrival_fuzz_minutes: 30.0,
        departure_fuzz_minutes: 30.0,
        daily_drive_miles_mean: 8.0,
        daily_drive_miles_stddev: 4.0,
        daily_drive_miles_min: 0.0,
        weekday_miles_mean: None,
        weekday_miles_stddev: None,
        weekend_miles_mean: None,
        weekend_miles_stddev: None,
        departure_minute_mean: 600.0,
        departure_minute_stddev: 60.0,
        duration_minutes_mean: 120.0,
        duration_minutes_stddev: 30.0,
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
        arrival_fuzz_minutes: 30.0,
        departure_fuzz_minutes: 30.0,
        daily_drive_miles_mean: 55.0,
        daily_drive_miles_stddev: 18.0,
        daily_drive_miles_min: 5.0,
        weekday_miles_mean: None,
        weekday_miles_stddev: None,
        weekend_miles_mean: None,
        weekend_miles_stddev: None,
        departure_minute_mean: 480.0,
        departure_minute_stddev: 30.0,
        duration_minutes_mean: 600.0,
        duration_minutes_stddev: 60.0,
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
        arrival_fuzz_minutes: 45.0,
        departure_fuzz_minutes: 45.0,
        daily_drive_miles_mean: 30.0,
        daily_drive_miles_stddev: 10.0,
        daily_drive_miles_min: 0.0,
        weekday_miles_mean: None,
        weekday_miles_stddev: None,
        weekend_miles_mean: None,
        weekend_miles_stddev: None,
        departure_minute_mean: 360.0,
        departure_minute_stddev: 45.0,
        duration_minutes_mean: 540.0,
        duration_minutes_stddev: 60.0,
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
        arrival_fuzz_minutes: 30.0,
        departure_fuzz_minutes: 30.0,
        daily_drive_miles_mean: 0.0,
        daily_drive_miles_stddev: 0.0,
        daily_drive_miles_min: 0.0,
        weekday_miles_mean: Some(10.0),
        weekday_miles_stddev: Some(5.0),
        weekend_miles_mean: Some(30.0),
        weekend_miles_stddev: Some(12.0),
        departure_minute_mean: 540.0,
        departure_minute_stddev: 60.0,
        duration_minutes_mean: 480.0,
        duration_minutes_stddev: 90.0,
    },
    ArchetypePreset {
        id: EvArchetypeId::WorkplaceCharger,
        label: "Workplace Charger",
        charging_level: ChargingLevel::L1,
        strategy: ChargingStrategy::Immediate { target_soc: 1.0 },
        plug_in_policy: PlugInPolicy::Always,
        event_day_ratio: 0.85,
        arrival_fuzz_minutes: 30.0,
        departure_fuzz_minutes: 30.0,
        daily_drive_miles_mean: 30.0,
        daily_drive_miles_stddev: 10.0,
        daily_drive_miles_min: 0.0,
        weekday_miles_mean: None,
        weekday_miles_stddev: None,
        weekend_miles_mean: None,
        weekend_miles_stddev: None,
        departure_minute_mean: 480.0,
        departure_minute_stddev: 30.0,
        duration_minutes_mean: 600.0,
        duration_minutes_stddev: 60.0,
    },
    ArchetypePreset {
        id: EvArchetypeId::RetireeL1,
        label: "Retiree L1",
        charging_level: ChargingLevel::L1,
        strategy: ChargingStrategy::Immediate { target_soc: 1.0 },
        plug_in_policy: PlugInPolicy::Always,
        event_day_ratio: 0.90,
        arrival_fuzz_minutes: 45.0,
        departure_fuzz_minutes: 45.0,
        daily_drive_miles_mean: 10.0,
        daily_drive_miles_stddev: 5.0,
        daily_drive_miles_min: 0.0,
        weekday_miles_mean: None,
        weekday_miles_stddev: None,
        weekend_miles_mean: None,
        weekend_miles_stddev: None,
        departure_minute_mean: 600.0,
        departure_minute_stddev: 90.0,
        duration_minutes_mean: 180.0,
        duration_minutes_stddev: 60.0,
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
        arrival_fuzz_minutes: 30.0,
        departure_fuzz_minutes: 30.0,
        daily_drive_miles_mean: 35.0,
        daily_drive_miles_stddev: 12.0,
        daily_drive_miles_min: 0.0,
        weekday_miles_mean: None,
        weekday_miles_stddev: None,
        weekend_miles_mean: None,
        weekend_miles_stddev: None,
        departure_minute_mean: 480.0,
        departure_minute_stddev: 30.0,
        duration_minutes_mean: 600.0,
        duration_minutes_stddev: 60.0,
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
        arrival_fuzz_minutes: 30.0,
        departure_fuzz_minutes: 30.0,
        daily_drive_miles_mean: 38.0,
        daily_drive_miles_stddev: 13.0,
        daily_drive_miles_min: 0.0,
        weekday_miles_mean: None,
        weekday_miles_stddev: None,
        weekend_miles_mean: None,
        weekend_miles_stddev: None,
        departure_minute_mean: 480.0,
        departure_minute_stddev: 30.0,
        duration_minutes_mean: 600.0,
        duration_minutes_stddev: 60.0,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

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
        let cfg = spec.to_config();
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
            let cfg = spec.to_config();
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
            (VehicleId::TeslaModelYSr, 0.274, "123 MPGe combined → 33.7/123"),
            (VehicleId::TeslaModel3Lr, 0.260, "26 kWh/100mi"),
            (VehicleId::ChevyBoltEv, 0.280, "28 kWh/100mi"),
            (VehicleId::ChevyBoltEuv, 0.290, "29 kWh/100mi"),
            (VehicleId::FordMacheSr, 0.330, "33 kWh/100mi"),
            (VehicleId::FordMacheEr, 0.340, "34 kWh/100mi"),
            (VehicleId::FordLightningEr, 0.480, "48 kWh/100mi"),
            (VehicleId::HyundaiIoniq5Lr, 0.300, "30 kWh/100mi"),
            (VehicleId::NissanLeaf30, 0.300, "30 kWh/100mi"),
            (VehicleId::Jeep4xe, 0.680, "68 kWh/100mi elec"),
            (VehicleId::ToyotaRav4Prime, 0.359, "94 MPGe combined → 33.7/94"),
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
            assert!(
                preset.arrival_fuzz_minutes >= 0.0,
                "{}: arrival_fuzz must be non-negative",
                preset.label
            );
            assert!(
                preset.departure_fuzz_minutes >= 0.0,
                "{}: departure_fuzz must be non-negative",
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
                preset.weekday_miles_mean.is_some(),
                "{}: should have weekday_miles_mean",
                preset.label
            );
            assert!(
                preset.weekend_miles_mean.is_some(),
                "{}: should have weekend_miles_mean",
                preset.label
            );
        }
    }
}
