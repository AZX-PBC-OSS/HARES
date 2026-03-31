//! Battery product catalog with factory methods for commercial products.

use std::collections::HashMap;
use std::fmt;

use hares_types::BatteryChemistry;
use serde::{Deserialize, Serialize};

use crate::EquipmentConfig;
use crate::config::ConfigValue;

use super::{
    KEY_CAPACITY_KWH, KEY_CHARGE_EFFICIENCY, KEY_CHEMISTRY, KEY_DISCHARGE_EFFICIENCY,
    KEY_FULL_POWER_TEMP_C, KEY_HEATER_POWER_W, KEY_HEATER_THRESHOLD_C, KEY_MAX_CHARGE_KW,
    KEY_MAX_DISCHARGE_KW, KEY_MAX_SOC, KEY_MIN_CHARGE_TEMP_C, KEY_MIN_SOC,
    KEY_SELF_DISCHARGE_PCT_PER_DAY, KEY_STANDBY_POWER_W,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BatteryProductId {
    TeslaPw3,
    TeslaPw2,
    TeslaPw3X2,
    EnphaseIq5p,
    EnphaseIq5pX2,
    EnphaseIq10c,
    FranklinApower,
    FranklinApower2,
    FranklinApower2X2,
    SolaredgeHome,
    LgResu10h,
}

impl BatteryProductId {
    pub const ALL: &[BatteryProductId] = &[
        Self::TeslaPw3,
        Self::TeslaPw2,
        Self::TeslaPw3X2,
        Self::EnphaseIq5p,
        Self::EnphaseIq5pX2,
        Self::EnphaseIq10c,
        Self::FranklinApower,
        Self::FranklinApower2,
        Self::FranklinApower2X2,
        Self::SolaredgeHome,
        Self::LgResu10h,
    ];

    pub fn spec(self) -> &'static BatterySpec {
        &CATALOG[self as usize]
    }
}

impl fmt::Display for BatteryProductId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::TeslaPw3 => "TeslaPw3",
            Self::TeslaPw2 => "TeslaPw2",
            Self::TeslaPw3X2 => "TeslaPw3X2",
            Self::EnphaseIq5p => "EnphaseIq5p",
            Self::EnphaseIq5pX2 => "EnphaseIq5pX2",
            Self::EnphaseIq10c => "EnphaseIq10c",
            Self::FranklinApower => "FranklinApower",
            Self::FranklinApower2 => "FranklinApower2",
            Self::FranklinApower2X2 => "FranklinApower2X2",
            Self::SolaredgeHome => "SolaredgeHome",
            Self::LgResu10h => "LgResu10h",
        };
        write!(f, "{s}")
    }
}

impl std::str::FromStr for BatteryProductId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized: String = s
            .chars()
            .filter(|c| *c != '_')
            .flat_map(|c| c.to_lowercase())
            .collect();
        match normalized.as_str() {
            "teslapw3" => Ok(Self::TeslaPw3),
            "teslapw2" => Ok(Self::TeslaPw2),
            "teslapw3x2" => Ok(Self::TeslaPw3X2),
            "enphaseiq5p" => Ok(Self::EnphaseIq5p),
            "enphaseiq5px2" => Ok(Self::EnphaseIq5pX2),
            "enphaseiq10c" => Ok(Self::EnphaseIq10c),
            "franklinapower" => Ok(Self::FranklinApower),
            "franklinapower2" => Ok(Self::FranklinApower2),
            "franklinapower2x2" => Ok(Self::FranklinApower2X2),
            "solaredgehome" => Ok(Self::SolaredgeHome),
            "lgresu10h" => Ok(Self::LgResu10h),
            _ => Err(format!("unknown battery product: {s}")),
        }
    }
}

pub struct BatterySpec {
    pub id: BatteryProductId,
    pub label: &'static str,
    pub capacity_kwh: f64,
    pub max_charge_kw: f64,
    pub max_discharge_kw: f64,
    pub chemistry: BatteryChemistry,
    pub round_trip_efficiency: f64,
    pub standby_power_w: f64,
    pub self_discharge_pct_per_day: f64,
    pub min_soc: f64,
    pub max_soc: f64,
    /// Battery pack heater power (W). 0 = no heater (passive thermal only).
    pub heater_power_w: f64,
    /// Cell temp below which heater activates (°C).
    pub heater_threshold_c: f64,
    /// Cell temp below which charging is blocked (°C). Li-ion safety limit.
    pub min_charge_temp_c: f64,
    /// Cell temp above which full charge rate is available (°C).
    pub full_power_temp_c: f64,
}

impl BatterySpec {
    /// Build an `EquipmentConfig` from this catalog entry.
    ///
    /// Splits `round_trip_efficiency` symmetrically: charge and discharge
    /// efficiency are each `sqrt(rte)`. This is valid for AC-coupled systems
    /// where inverter losses dominate and are symmetric.
    pub fn to_config(&self) -> EquipmentConfig {
        let eta = self.round_trip_efficiency.sqrt();
        let mut data = HashMap::new();
        data.insert(
            KEY_CAPACITY_KWH.into(),
            ConfigValue::Float(self.capacity_kwh),
        );
        data.insert(
            KEY_MAX_CHARGE_KW.into(),
            ConfigValue::Float(self.max_charge_kw),
        );
        data.insert(
            KEY_MAX_DISCHARGE_KW.into(),
            ConfigValue::Float(self.max_discharge_kw),
        );
        data.insert(KEY_CHARGE_EFFICIENCY.into(), ConfigValue::Float(eta));
        data.insert(KEY_DISCHARGE_EFFICIENCY.into(), ConfigValue::Float(eta));
        data.insert(
            KEY_CHEMISTRY.into(),
            ConfigValue::Text(self.chemistry.as_config_str().to_string()),
        );
        data.insert(
            KEY_STANDBY_POWER_W.into(),
            ConfigValue::Float(self.standby_power_w),
        );
        data.insert(
            KEY_SELF_DISCHARGE_PCT_PER_DAY.into(),
            ConfigValue::Float(self.self_discharge_pct_per_day),
        );
        data.insert(KEY_MIN_SOC.into(), ConfigValue::Float(self.min_soc));
        data.insert(KEY_MAX_SOC.into(), ConfigValue::Float(self.max_soc));
        data.insert(
            KEY_HEATER_POWER_W.into(),
            ConfigValue::Float(self.heater_power_w),
        );
        data.insert(
            KEY_HEATER_THRESHOLD_C.into(),
            ConfigValue::Float(self.heater_threshold_c),
        );
        data.insert(
            KEY_MIN_CHARGE_TEMP_C.into(),
            ConfigValue::Float(self.min_charge_temp_c),
        );
        data.insert(
            KEY_FULL_POWER_TEMP_C.into(),
            ConfigValue::Float(self.full_power_temp_c),
        );
        EquipmentConfig {
            name: self.label.to_string(),
            ochre_class: "Battery".to_string(),
            payload: crate::ConfigPayload::Raw { data },
        }
    }
}

pub fn by_id(id: &str) -> Option<&'static BatterySpec> {
    let pid: BatteryProductId = id.parse().ok()?;
    Some(pid.spec())
}

static CATALOG: &[BatterySpec] = &[
    BatterySpec {
        id: BatteryProductId::TeslaPw3,
        label: "Tesla Powerwall 3",
        capacity_kwh: 13.5,
        max_charge_kw: 11.5,
        max_discharge_kw: 11.5,
        chemistry: BatteryChemistry::Lfp,
        round_trip_efficiency: 0.90,
        standby_power_w: 10.0,
        self_discharge_pct_per_day: 0.05,
        min_soc: 0.05,
        max_soc: 1.0,
        heater_power_w: 300.0,
        heater_threshold_c: 5.0,
        min_charge_temp_c: 0.0,
        full_power_temp_c: 10.0,
    },
    BatterySpec {
        id: BatteryProductId::TeslaPw2,
        label: "Tesla Powerwall 2",
        capacity_kwh: 13.5,
        max_charge_kw: 5.0,
        max_discharge_kw: 5.0,
        chemistry: BatteryChemistry::Nmc,
        round_trip_efficiency: 0.90,
        standby_power_w: 10.0,
        self_discharge_pct_per_day: 0.067,
        min_soc: 0.10,
        max_soc: 1.0,
        heater_power_w: 1500.0,
        heater_threshold_c: 5.0,
        min_charge_temp_c: 0.0,
        full_power_temp_c: 10.0,
    },
    BatterySpec {
        id: BatteryProductId::TeslaPw3X2,
        label: "Tesla Powerwall 3 \u{00d7}2",
        capacity_kwh: 27.0,
        max_charge_kw: 23.0,
        max_discharge_kw: 23.0,
        chemistry: BatteryChemistry::Lfp,
        round_trip_efficiency: 0.90,
        standby_power_w: 20.0,
        self_discharge_pct_per_day: 0.05,
        min_soc: 0.05,
        max_soc: 1.0,
        heater_power_w: 600.0,
        heater_threshold_c: 5.0,
        min_charge_temp_c: 0.0,
        full_power_temp_c: 10.0,
    },
    BatterySpec {
        id: BatteryProductId::EnphaseIq5p,
        label: "Enphase IQ 5P",
        capacity_kwh: 5.0,
        max_charge_kw: 3.84,
        max_discharge_kw: 3.84,
        chemistry: BatteryChemistry::Lfp,
        round_trip_efficiency: 0.90,
        standby_power_w: 15.0,
        self_discharge_pct_per_day: 0.05,
        min_soc: 0.05,
        max_soc: 1.0,
        heater_power_w: 0.0,
        heater_threshold_c: 0.0,
        min_charge_temp_c: -20.0,
        full_power_temp_c: 15.0,
    },
    BatterySpec {
        id: BatteryProductId::EnphaseIq5pX2,
        label: "Enphase IQ 5P \u{00d7}2",
        capacity_kwh: 10.0,
        max_charge_kw: 7.68,
        max_discharge_kw: 7.68,
        chemistry: BatteryChemistry::Lfp,
        round_trip_efficiency: 0.90,
        standby_power_w: 30.0,
        self_discharge_pct_per_day: 0.05,
        min_soc: 0.05,
        max_soc: 1.0,
        heater_power_w: 0.0,
        heater_threshold_c: 0.0,
        min_charge_temp_c: -20.0,
        full_power_temp_c: 15.0,
    },
    BatterySpec {
        id: BatteryProductId::EnphaseIq10c,
        label: "Enphase IQ 10C",
        capacity_kwh: 10.0,
        max_charge_kw: 7.08,
        max_discharge_kw: 7.08,
        chemistry: BatteryChemistry::Lfp,
        round_trip_efficiency: 0.90,
        standby_power_w: 15.0,
        self_discharge_pct_per_day: 0.05,
        min_soc: 0.05,
        max_soc: 1.0,
        heater_power_w: 0.0,
        heater_threshold_c: 0.0,
        min_charge_temp_c: -20.0,
        full_power_temp_c: 15.0,
    },
    BatterySpec {
        id: BatteryProductId::FranklinApower,
        label: "FranklinWH aPower",
        capacity_kwh: 13.6,
        max_charge_kw: 5.0,
        max_discharge_kw: 5.0,
        chemistry: BatteryChemistry::Lfp,
        round_trip_efficiency: 0.89,
        standby_power_w: 30.0,
        self_discharge_pct_per_day: 0.05,
        min_soc: 0.05,
        max_soc: 1.0,
        heater_power_w: 300.0,
        heater_threshold_c: 5.0,
        min_charge_temp_c: -20.0,
        full_power_temp_c: 15.0,
    },
    BatterySpec {
        id: BatteryProductId::FranklinApower2,
        label: "FranklinWH aPower 2",
        capacity_kwh: 15.0,
        max_charge_kw: 10.0,
        max_discharge_kw: 10.0,
        chemistry: BatteryChemistry::Lfp,
        round_trip_efficiency: 0.90,
        standby_power_w: 30.0,
        self_discharge_pct_per_day: 0.05,
        min_soc: 0.05,
        max_soc: 1.0,
        heater_power_w: 300.0,
        heater_threshold_c: 5.0,
        min_charge_temp_c: -20.0,
        full_power_temp_c: 15.0,
    },
    BatterySpec {
        id: BatteryProductId::FranklinApower2X2,
        label: "FranklinWH aPower 2 \u{00d7}2",
        capacity_kwh: 30.0,
        max_charge_kw: 20.0,
        max_discharge_kw: 20.0,
        chemistry: BatteryChemistry::Lfp,
        round_trip_efficiency: 0.90,
        standby_power_w: 60.0,
        self_discharge_pct_per_day: 0.05,
        min_soc: 0.05,
        max_soc: 1.0,
        heater_power_w: 600.0,
        heater_threshold_c: 5.0,
        min_charge_temp_c: -20.0,
        full_power_temp_c: 15.0,
    },
    BatterySpec {
        id: BatteryProductId::SolaredgeHome,
        label: "SolarEdge Home Battery",
        capacity_kwh: 9.7,
        max_charge_kw: 5.0,
        max_discharge_kw: 5.0,
        chemistry: BatteryChemistry::Nmc,
        round_trip_efficiency: 0.945,
        standby_power_w: 5.0,
        self_discharge_pct_per_day: 0.067,
        min_soc: 0.10,
        max_soc: 1.0,
        heater_power_w: 0.0,
        heater_threshold_c: 0.0,
        min_charge_temp_c: -10.0,
        full_power_temp_c: 15.0,
    },
    BatterySpec {
        id: BatteryProductId::LgResu10h,
        label: "LG RESU 10H",
        capacity_kwh: 9.3,
        max_charge_kw: 5.0,
        max_discharge_kw: 5.0,
        chemistry: BatteryChemistry::Nmc,
        round_trip_efficiency: 0.95,
        standby_power_w: 5.0,
        self_discharge_pct_per_day: 0.067,
        min_soc: 0.10,
        max_soc: 1.0,
        heater_power_w: 0.0,
        heater_threshold_c: 0.0,
        min_charge_temp_c: -10.0,
        full_power_temp_c: 15.0,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_catalog_specs_valid() {
        assert_eq!(CATALOG.len(), 11);
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
                spec.max_charge_kw > 0.0,
                "{}: charge power must be positive",
                spec.label
            );
            assert!(
                spec.max_discharge_kw > 0.0,
                "{}: discharge power must be positive",
                spec.label
            );
            assert!(
                spec.round_trip_efficiency > 0.0 && spec.round_trip_efficiency <= 1.0,
                "{}: RTE must be in (0,1]",
                spec.label
            );
            assert!(spec.min_soc >= 0.0 && spec.min_soc < spec.max_soc);
            assert!(spec.max_soc <= 1.0);
        }
    }

    #[test]
    fn from_str_round_trips() {
        for &id in BatteryProductId::ALL {
            let s = id.to_string();
            let parsed: BatteryProductId = s.parse().unwrap();
            assert_eq!(parsed, id);
        }
    }

    #[test]
    fn from_str_case_insensitive_underscore_tolerant() {
        assert_eq!(
            "tesla_pw3".parse::<BatteryProductId>().unwrap(),
            BatteryProductId::TeslaPw3
        );
        assert_eq!(
            "TESLA_PW3".parse::<BatteryProductId>().unwrap(),
            BatteryProductId::TeslaPw3
        );
        assert_eq!(
            "TeslaPw3".parse::<BatteryProductId>().unwrap(),
            BatteryProductId::TeslaPw3
        );
        assert_eq!(
            "lg_resu_10h".parse::<BatteryProductId>().unwrap(),
            BatteryProductId::LgResu10h
        );
    }

    #[test]
    fn by_id_lookup() {
        let spec = by_id("tesla_pw3").unwrap();
        assert_eq!(spec.id, BatteryProductId::TeslaPw3);
        assert!((spec.capacity_kwh - 13.5).abs() < f64::EPSILON);
        assert!(by_id("nonexistent").is_none());
    }

    #[test]
    fn to_config_splits_rte() {
        let spec = BatteryProductId::TeslaPw3.spec();
        let cfg = spec.to_config();
        let eta = 0.90_f64.sqrt();
        let charge_eff = cfg.get_f64("charge_efficiency").unwrap();
        let discharge_eff = cfg.get_f64("discharge_efficiency").unwrap();
        assert!((charge_eff - eta).abs() < 1e-10);
        assert!((discharge_eff - eta).abs() < 1e-10);
        assert_eq!(cfg.get_str("chemistry").unwrap(), "lfp");
    }

    #[test]
    fn to_config_splits_rte_correctly_for_all_products() {
        for spec in CATALOG {
            let cfg = spec.to_config();
            let expected_eta = spec.round_trip_efficiency.sqrt();
            let charge = cfg.get_f64("charge_efficiency").unwrap();
            let discharge = cfg.get_f64("discharge_efficiency").unwrap();
            assert!(
                (charge - expected_eta).abs() < 1e-10,
                "{}: charge_eff {charge} != sqrt({}) = {expected_eta}",
                spec.label,
                spec.round_trip_efficiency
            );
            assert!(
                (discharge - expected_eta).abs() < 1e-10,
                "{}: discharge_eff {discharge} != sqrt({}) = {expected_eta}",
                spec.label,
                spec.round_trip_efficiency
            );
        }
    }
}
