use serde::{Deserialize, Serialize};

use crate::EquipmentConfig;

use super::config::KEY_DRIVER_ARCHETYPE;
use super::schedule::EventDistributionRow;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum DriverArchetype {
    Commuter,
    ShiftWorker,
    WorkFromHome,
    WeekendWarrior,
    SeniorRetiree,
    SchoolRunFamily,
    SingleCarSharedHousehold,
}

impl DriverArchetype {
    pub(super) fn from_config(config: &EquipmentConfig) -> Self {
        match config
            .get_str(KEY_DRIVER_ARCHETYPE)
            .unwrap_or("commuter")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "shift_worker" | "shiftworker" | "shift" => Self::ShiftWorker,
            "work_from_home" | "workfromhome" | "wfh" => Self::WorkFromHome,
            "weekend_warrior" | "weekendwarrior" => Self::WeekendWarrior,
            "senior_retiree" | "seniorretiree" | "retiree" => Self::SeniorRetiree,
            "school_run_family" | "schoolrunfamily" => Self::SchoolRunFamily,
            "single_car_shared_household"
            | "singlecarsharedhousehold"
            | "shared_household"
            | "sharedhousehold" => Self::SingleCarSharedHousehold,
            _ => Self::Commuter,
        }
    }
}

pub(super) fn default_distribution(archetype: DriverArchetype) -> Vec<EventDistributionRow> {
    match archetype {
        DriverArchetype::Commuter => vec![EventDistributionRow {
            arrival_minute: 18 * 60,
            duration_minutes: 12 * 60,
            start_soc: 0.4,
            weight: 1.0,
        }],
        DriverArchetype::ShiftWorker => vec![
            EventDistributionRow {
                arrival_minute: 7 * 60,
                duration_minutes: 8 * 60,
                start_soc: 0.45,
                weight: 0.5,
            },
            EventDistributionRow {
                arrival_minute: 21 * 60,
                duration_minutes: 9 * 60,
                start_soc: 0.35,
                weight: 0.5,
            },
        ],
        DriverArchetype::WorkFromHome => vec![EventDistributionRow {
            arrival_minute: 15 * 60,
            duration_minutes: 6 * 60,
            start_soc: 0.7,
            weight: 1.0,
        }],
        DriverArchetype::WeekendWarrior => vec![
            EventDistributionRow {
                arrival_minute: 19 * 60,
                duration_minutes: 11 * 60,
                start_soc: 0.45,
                weight: 0.7,
            },
            EventDistributionRow {
                arrival_minute: 21 * 60,
                duration_minutes: 10 * 60,
                start_soc: 0.30,
                weight: 0.3,
            },
        ],
        DriverArchetype::SeniorRetiree => vec![EventDistributionRow {
            arrival_minute: 14 * 60,
            duration_minutes: 8 * 60,
            start_soc: 0.65,
            weight: 1.0,
        }],
        DriverArchetype::SchoolRunFamily => vec![
            EventDistributionRow {
                arrival_minute: 9 * 60 + 30,
                duration_minutes: 5 * 60,
                start_soc: 0.60,
                weight: 0.45,
            },
            EventDistributionRow {
                arrival_minute: 16 * 60 + 30,
                duration_minutes: 7 * 60,
                start_soc: 0.50,
                weight: 0.55,
            },
        ],
        DriverArchetype::SingleCarSharedHousehold => vec![
            EventDistributionRow {
                arrival_minute: 17 * 60,
                duration_minutes: 6 * 60,
                start_soc: 0.45,
                weight: 0.5,
            },
            EventDistributionRow {
                arrival_minute: 20 * 60,
                duration_minutes: 10 * 60,
                start_soc: 0.40,
                weight: 0.5,
            },
        ],
    }
}
