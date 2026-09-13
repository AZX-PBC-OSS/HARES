//! Typed configuration structs for heating equipment.

use hares_types::{FluidType, FuelType, ScheduleSourceConfig};
use serde::{Deserialize, Serialize};

use crate::config::EquipmentTypedConfig;

pub use super::core_config::DuctConfig;
use super::core_config::default_one;
use super::core_config::equipment_type_name;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HvacSetpointConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heating_setpoint_c: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heating_setpoint_source: Option<ScheduleSourceConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooling_setpoint_c: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooling_setpoint_source: Option<ScheduleSourceConfig>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GasFurnaceConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    /// Required; resolver errors if absent.
    pub capacity_w: f64,
    /// Required; resolver errors if absent.
    pub afue: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w: Option<f64>,
    #[serde(default = "default_one")]
    pub number_of_speeds: u8,
    /// Per-stage heating capacities [W]. Overrides `capacity_w` when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_heating_capacities_w: Option<Vec<f64>>,
    /// Per-stage energy input ratios (EIR = 1/AFUE per stage). Overrides `afue` when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_heating_eirs: Option<Vec<f64>>,
    #[serde(default)]
    pub setpoint: HvacSetpointConfig,
    #[serde(default)]
    pub ducts: DuctConfig,
}

impl EquipmentTypedConfig for GasFurnaceConfig {
    fn equipment_type_name() -> &'static str {
        equipment_type_name::GAS_FURNACE
    }
}

impl Default for GasFurnaceConfig {
    fn default() -> Self {
        Self {
            equipment_id: None,
            zone_id: None,
            capacity_w: 0.0,
            afue: 0.80,
            fan_power_w: None,
            number_of_speeds: 1,
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            setpoint: HvacSetpointConfig::default(),
            ducts: DuctConfig::default(),
        }
    }
}

impl GasFurnaceConfig {
    /// Validate that `afue` is finite and within the physically meaningful
    /// range [0.0, 1.0]. Values outside this range violate the Second Law of
    /// Thermodynamics (AFUE > 1.0) or are physically meaningless (AFUE < 0.0).
    /// References: HPXML 4.2 §3.8.2 AFUE data type with range [0, 1];
    /// EnergyPlus `Boilers.cc:296` warns when boiler efficiency exceeds 1.0.
    pub fn validate(&self) -> crate::Result<()> {
        use hares_types::HaresError;
        if !self.afue.is_finite() {
            return Err(HaresError::Equipment(format!(
                "GasFurnaceConfig: afue must be finite, got {}",
                self.afue
            )));
        }
        if self.afue < 0.0 || self.afue > 1.0 {
            return Err(HaresError::Equipment(format!(
                "GasFurnaceConfig: afue must be in [0.0, 1.0], got {}",
                self.afue
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElectricFurnaceConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    /// Required; resolver errors if absent.
    pub capacity_w: f64,
    /// Electric input ratio [W/W] = electric input power divided by delivered
    /// thermal output. Unity is ideal resistive conversion.
    pub eir: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w: Option<f64>,
    #[serde(default = "default_one")]
    pub number_of_speeds: u8,
    pub setpoint: HvacSetpointConfig,
    #[serde(default)]
    pub ducts: DuctConfig,
}

impl EquipmentTypedConfig for ElectricFurnaceConfig {
    fn equipment_type_name() -> &'static str {
        equipment_type_name::ELECTRIC_FURNACE
    }
}

impl Default for ElectricFurnaceConfig {
    fn default() -> Self {
        Self {
            equipment_id: None,
            zone_id: None,
            capacity_w: 0.0,
            eir: 1.0,
            fan_power_w: None,
            number_of_speeds: 1,
            setpoint: HvacSetpointConfig::default(),
            ducts: DuctConfig::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GasBoilerConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    pub loop_id: Option<u16>,
    /// Required; resolver errors if absent.
    pub capacity_w: f64,
    /// Required; resolver errors if absent.
    pub afue: f64,
    /// Hydronic loop design flow rate [kg/s]. Default 0.5 kg/s (~8 gpm),
    /// an engineering estimate from ASHRAE HVAC Systems and Equipment Ch.13
    /// residential hydronic sizing conventions (historical US standard:
    /// 1 gpm per 10 000 Btu/h). Not sourced from HPXML; applied as a
    /// configuration default when the HPXML file omits this field.
    #[serde(default = "default_flow_rate_kg_s")]
    pub flow_rate_kg_s: f64,
    /// Hydronic loop design return temperature [°C]. Default 70.0 °C, a
    /// safe non-condensing return temperature that avoids sustained flue-gas
    /// condensation and corrosion (ASHRAE HVAC Systems and Equipment Ch.32
    /// "Boilers": non-condensing boilers require return ≥ 70 °C).
    /// Not sourced from HPXML; applied as a configuration default when the
    /// HPXML file omits this field.
    #[serde(default = "default_return_temp_c")]
    pub return_temp_c: f64,
    #[serde(default = "default_fluid_type")]
    pub fluid_type: FluidType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w: Option<f64>,
    #[serde(default = "default_one")]
    pub number_of_speeds: u8,
    #[serde(default)]
    pub setpoint: HvacSetpointConfig,
    /// Condensing boiler mode. Inferred from AFUE > 0.90 (OCHRE convention).
    /// Condensing boilers use a 6-coefficient efficiency curve and lower return
    /// water temperature (~65 °C / 150 °F); non-condensing boilers use 10
    /// coefficients and a higher outlet temperature (~82 °C / 180 °F).
    /// OCHRE HVAC.py GasBoiler class: `condensing = eir_max < 1 / 0.9`.
    #[serde(default)]
    pub condensing: bool,
}

impl EquipmentTypedConfig for GasBoilerConfig {
    fn equipment_type_name() -> &'static str {
        equipment_type_name::GAS_BOILER
    }
}

impl Default for GasBoilerConfig {
    fn default() -> Self {
        Self {
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            capacity_w: 0.0,
            afue: 0.80,
            flow_rate_kg_s: default_flow_rate_kg_s(),
            return_temp_c: default_return_temp_c(),
            fluid_type: default_fluid_type(),
            fan_power_w: None,
            number_of_speeds: 1,
            setpoint: HvacSetpointConfig::default(),
            condensing: false,
        }
    }
}

impl GasBoilerConfig {
    /// Validate that `afue` is finite and within the physically meaningful
    /// range [0.0, 1.0]. Values outside this range violate the Second Law of
    /// Thermodynamics (AFUE > 1.0) or are physically meaningless (AFUE < 0.0).
    /// References: HPXML 4.2 §3.8.2 AFUE data type with range [0, 1];
    /// EnergyPlus `Boilers.cc:296` warns when boiler efficiency exceeds 1.0.
    pub fn validate(&self) -> crate::Result<()> {
        use hares_types::HaresError;
        if !self.afue.is_finite() {
            return Err(HaresError::Equipment(format!(
                "GasBoilerConfig: afue must be finite, got {}",
                self.afue
            )));
        }
        if self.afue < 0.0 || self.afue > 1.0 {
            return Err(HaresError::Equipment(format!(
                "GasBoilerConfig: afue must be in [0.0, 1.0], got {}",
                self.afue
            )));
        }
        // ASHRAE HVAC Systems and Equipment Ch.32: non-condensing boilers require
        // return temperature ≥ 70 °C to avoid flue-gas condensation and corrosion.
        // Condensing boilers operate with return temperature below the ~55 °C
        // flue-gas dewpoint. A contradictory (condensing, return_temp_c) pair is a
        // configuration error — the mismatch would produce physically invalid
        // results during simulation.
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            if !self.condensing && self.return_temp_c < 55.0 {
                return Err(HaresError::Equipment(format!(
                    "GasBoilerConfig: condensing=false requires return_temp_c >= 55 °C, got {} °C",
                    self.return_temp_c
                )));
            }
            if self.condensing && self.return_temp_c >= 70.0 {
                return Err(HaresError::Equipment(format!(
                    "GasBoilerConfig: condensing=true expects return_temp_c < 70 °C, got {} °C",
                    self.return_temp_c
                )));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElectricBoilerConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    pub loop_id: Option<u16>,
    /// Required; resolver errors if absent.
    pub capacity_w: f64,
    /// Electric input ratio [W/W] = electric input power divided by delivered
    /// thermal output. Unity is ideal resistive conversion.
    pub eir: f64,
    /// Hydronic loop design flow rate [kg/s]. Default 0.5 kg/s (~8 gpm),
    /// an engineering estimate from ASHRAE HVAC Systems and Equipment Ch.13
    /// residential hydronic sizing conventions (historical US standard:
    /// 1 gpm per 10 000 Btu/h). Not sourced from HPXML; applied as a
    /// configuration default when the HPXML file omits this field.
    #[serde(default = "default_flow_rate_kg_s")]
    pub flow_rate_kg_s: f64,
    /// Hydronic loop design return temperature [°C]. Default 70.0 °C, a
    /// safe non-condensing return temperature that avoids sustained flue-gas
    /// condensation and corrosion (ASHRAE HVAC Systems and Equipment Ch.32
    /// "Boilers": non-condensing boilers require return ≥ 70 °C).
    /// Not sourced from HPXML; applied as a configuration default when the
    /// HPXML file omits this field.
    #[serde(default = "default_return_temp_c")]
    pub return_temp_c: f64,
    #[serde(default = "default_fluid_type")]
    pub fluid_type: FluidType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w: Option<f64>,
    #[serde(default = "default_one")]
    pub number_of_speeds: u8,
    #[serde(default)]
    pub setpoint: HvacSetpointConfig,
}

impl EquipmentTypedConfig for ElectricBoilerConfig {
    fn equipment_type_name() -> &'static str {
        equipment_type_name::ELECTRIC_BOILER
    }
}

impl Default for ElectricBoilerConfig {
    fn default() -> Self {
        Self {
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            capacity_w: 0.0,
            eir: 1.0,
            flow_rate_kg_s: default_flow_rate_kg_s(),
            return_temp_c: default_return_temp_c(),
            fluid_type: default_fluid_type(),
            fan_power_w: None,
            number_of_speeds: 1,
            setpoint: HvacSetpointConfig::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElectricBaseboardConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    /// Required; resolver errors if absent.
    pub capacity_w: f64,
    /// Electric input ratio [W/W] = electric input power divided by delivered
    /// thermal output. Unity is ideal resistive conversion.
    pub eir: f64,
    #[serde(default)]
    pub setpoint: HvacSetpointConfig,
}

impl EquipmentTypedConfig for ElectricBaseboardConfig {
    fn equipment_type_name() -> &'static str {
        equipment_type_name::ELECTRIC_BASEBOARD
    }
}

impl Default for ElectricBaseboardConfig {
    fn default() -> Self {
        Self {
            equipment_id: None,
            zone_id: None,
            capacity_w: 0.0,
            eir: 1.0,
            setpoint: HvacSetpointConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdealHvacConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    #[serde(default)]
    pub setpoint: HvacSetpointConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadband_c: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_speeds: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ideal_capacity_mode: Option<IdealCapacityModeConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heating_capacity_w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooling_capacity_w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shr: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction_heating_load_served: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction_cooling_load_served: Option<f64>,
    /// Rated fan power [W]. When non-zero, fan electrical consumption is
    /// computed as `capacity * eir * fan_power_ratio` in ideal-capacity mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rated_fan_power_w: Option<f64>,
    /// Energy input ratio (1/COP). Defaults to 1.0 (ideal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rated_eir: Option<f64>,
    /// Minimum capacity [W]. Below this threshold the unit shuts off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_min_w: Option<f64>,
    /// Override fuel type for the equipment descriptor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fuel_type: Option<FuelType>,
    /// Biquadratic capacity correction curve coefficients [a,b,c,d,e,f].
    /// Default identity: CAP_FT = 1.0 (no correction).
    /// EnergyPlus: `Q_corrected = Q_rated × CAP_FT(T_indoor, T_outdoor)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_biquadratic_coeffs: Option<String>,
    /// Biquadratic EIR correction curve coefficients [a,b,c,d,e,f].
    /// Default identity: EIR_FT = 1.0 (no correction).
    /// EnergyPlus: `EIR_corrected = EIR_rated × EIR_FT(T_indoor, T_outdoor)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eir_biquadratic_coeffs: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IdealCapacityModeConfig {
    Auto,
    On,
    Off,
}

impl EquipmentTypedConfig for IdealHvacConfig {
    fn equipment_type_name() -> &'static str {
        equipment_type_name::IDEAL_HVAC
    }
}

fn default_flow_rate_kg_s() -> f64 {
    // ASHRAE HVAC Systems and Equipment Ch.13 "Hydronic Heating and Cooling":
    // historical US residential standard of 1 gpm per 10 000 Btu/h,
    // approximately 0.5 kg/s for a typical 60 000 Btu/h boiler.
    // Engineering estimate; not a canonical standard default.
    0.5
}

fn default_return_temp_c() -> f64 {
    // ASHRAE HVAC Systems and Equipment Ch.32 "Boilers": non-condensing boilers
    // must maintain return water temperature ≥ 70 °C to avoid sustained flue-gas
    // condensation and corrosion. 70.0 °C is a safe non-condensing default.
    // Engineering estimate; not a canonical standard default.
    70.0
}

fn default_fluid_type() -> FluidType {
    FluidType::Water
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigPayload, EquipmentConfig};

    #[test]
    fn setpoint_config_unknown_fields_are_rejected() {
        // HvacSetpointConfig deserializes from override JSON nested inside
        // the heating configs: a mistyped key must not be silently dropped.
        let with_typo = r#"{"heating_setpoint_c":21.0,"heating_setpoin_c":21.0}"#;
        let err = serde_json::from_str::<HvacSetpointConfig>(with_typo)
            .expect_err("unknown HvacSetpointConfig fields must be rejected");
        assert!(
            format!("{err}").contains("heating_setpoin_c"),
            "error must name the unknown field, got {err}"
        );
    }

    #[test]
    fn all_configs_round_trip_via_serde_json() {
        let gf = GasFurnaceConfig {
            capacity_w: 12_000.0,
            afue: 0.95,
            ducts: DuctConfig {
                dse_heat: Some(0.85),
                ..DuctConfig::default()
            },
            ..GasFurnaceConfig::default()
        };
        let ef = ElectricFurnaceConfig {
            capacity_w: 8_000.0,
            eir: 1.0,
            ..ElectricFurnaceConfig::default()
        };
        let gb = GasBoilerConfig {
            capacity_w: 15_000.0,
            afue: 0.88,
            ..GasBoilerConfig::default()
        };
        let eb = ElectricBoilerConfig {
            capacity_w: 10_000.0,
            eir: 0.98,
            ..ElectricBoilerConfig::default()
        };
        let bb = ElectricBaseboardConfig {
            capacity_w: 5_000.0,
            eir: 1.0,
            ..ElectricBaseboardConfig::default()
        };
        let ih = IdealHvacConfig {
            heating_capacity_w: Some(10_000.0),
            cooling_capacity_w: Some(8_000.0),
            shr: Some(0.75),
            rated_fan_power_w: Some(200.0),
            rated_eir: Some(1.0),
            capacity_min_w: Some(500.0),
            fuel_type: Some(FuelType::Electric),
            ..IdealHvacConfig::default()
        };

        fn assert_round_trip<T>(config: T)
        where
            T: EquipmentTypedConfig,
        {
            let value = serde_json::to_value(&config).unwrap();
            let recovered: T = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(recovered).unwrap(), value);
        }

        assert_round_trip(gf);
        assert_round_trip(ef);
        assert_round_trip(gb);
        assert_round_trip(eb);
        assert_round_trip(bb);
        assert_round_trip(ih);
    }

    /// Regression: heating setpoint fields must round-trip through JSON for
    /// every heating config so that HPXML-supplied setpoints reach
    /// `HvacEquipment::init` via the typed payload (resolve_hvac.rs populates
    /// these fields from the HPXML `HVACControl` element).
    #[test]
    fn heating_configs_preserve_setpoint_fields_through_json_round_trip() {
        let setpoint = 18.33; // 65 °F, a common HPXML setback value
        let gf = GasFurnaceConfig {
            capacity_w: 12_000.0,
            afue: 0.92,
            setpoint: HvacSetpointConfig {
                heating_setpoint_c: Some(setpoint),
                ..Default::default()
            },
            ..GasFurnaceConfig::default()
        };
        let ef = ElectricFurnaceConfig {
            capacity_w: 8_000.0,
            eir: 1.0,
            setpoint: HvacSetpointConfig {
                heating_setpoint_c: Some(setpoint),
                ..Default::default()
            },
            ..ElectricFurnaceConfig::default()
        };
        let gb = GasBoilerConfig {
            capacity_w: 15_000.0,
            afue: 0.88,
            setpoint: HvacSetpointConfig {
                heating_setpoint_c: Some(setpoint),
                ..Default::default()
            },
            ..GasBoilerConfig::default()
        };
        let eb = ElectricBoilerConfig {
            capacity_w: 10_000.0,
            eir: 1.0,
            setpoint: HvacSetpointConfig {
                heating_setpoint_c: Some(setpoint),
                ..Default::default()
            },
            ..ElectricBoilerConfig::default()
        };
        let bb = ElectricBaseboardConfig {
            capacity_w: 5_000.0,
            eir: 1.0,
            setpoint: HvacSetpointConfig {
                heating_setpoint_c: Some(setpoint),
                ..Default::default()
            },
            ..ElectricBaseboardConfig::default()
        };

        fn assert_setpoint_round_trip<T>(config: T, expected_setpoint: f64)
        where
            T: EquipmentTypedConfig + std::fmt::Debug,
        {
            let value = serde_json::to_value(&config).expect("serialize");
            let field = value
                .get("setpoint")
                .and_then(|s| s.get("heating_setpoint_c"))
                .and_then(serde_json::Value::as_f64)
                .expect("setpoint.heating_setpoint_c must appear in serialized form");
            assert!(
                (field - expected_setpoint).abs() < 1e-9,
                "setpoint.heating_setpoint_c must round-trip for {}: got {field}, expected {expected_setpoint}",
                T::equipment_type_name()
            );
            let recovered: T = serde_json::from_value(value.clone()).expect("deserialize");
            let reserialized = serde_json::to_value(&recovered).expect("reserialize");
            assert_eq!(reserialized, value);
        }

        assert_setpoint_round_trip(gf, setpoint);
        assert_setpoint_round_trip(ef, setpoint);
        assert_setpoint_round_trip(gb, setpoint);
        assert_setpoint_round_trip(eb, setpoint);
        assert_setpoint_round_trip(bb, setpoint);
    }

    #[test]
    fn gas_furnace_rejects_unknown_fields_with_key_name() {
        let data = serde_json::json!({
            "capacity_w": 12000.0,
            "afue": 0.96,
            "unknown_key": true,
        });
        let ec = EquipmentConfig::with_payload(
            "test".to_string(),
            "Gas Furnace".to_string(),
            ConfigPayload::Typed {
                type_name: "Gas Furnace".to_string(),
                version: 1,
                data,
            },
        );
        let result: crate::Result<GasFurnaceConfig> = ec.typed();
        let err = result.expect_err("unknown fields must be rejected");
        assert!(
            err.to_string().contains("unknown_key"),
            "error should name unknown key; got {err}"
        );
    }

    #[test]
    fn gas_furnace_validate_accepts_valid_afue() {
        for afue in [0.0_f64, 0.80, 1.0] {
            let cfg = GasFurnaceConfig {
                afue,
                capacity_w: 10_000.0,
                ..GasFurnaceConfig::default()
            };
            cfg.validate()
                .unwrap_or_else(|e| panic!("afue={afue} should be valid, got {e}"));
        }
    }

    #[test]
    fn gas_furnace_validate_rejects_invalid_afue() {
        for (afue, expected_substring) in [
            (-0.1_f64, "[0.0, 1.0]"),
            (1.1_f64, "[0.0, 1.0]"),
            (f64::NAN, "finite"),
            (f64::INFINITY, "finite"),
            (f64::NEG_INFINITY, "finite"),
        ] {
            let cfg = GasFurnaceConfig {
                afue,
                capacity_w: 10_000.0,
                ..GasFurnaceConfig::default()
            };
            let err = cfg
                .validate()
                .expect_err(&format!("afue={afue} should be invalid"));
            let msg = err.to_string();
            assert!(
                msg.contains(expected_substring),
                "error for afue={afue} should mention '{expected_substring}', got: {msg}",
            );
        }
    }

    #[test]
    fn gas_boiler_validate_accepts_valid_afue() {
        for afue in [0.0_f64, 0.80, 1.0] {
            let cfg = GasBoilerConfig {
                afue,
                capacity_w: 10_000.0,
                ..GasBoilerConfig::default()
            };
            cfg.validate()
                .unwrap_or_else(|e| panic!("afue={afue} should be valid, got {e}"));
        }
    }

    #[test]
    fn gas_boiler_validate_rejects_invalid_afue() {
        for (afue, expected_substring) in [
            (-0.1_f64, "[0.0, 1.0]"),
            (1.1_f64, "[0.0, 1.0]"),
            (f64::NAN, "finite"),
            (f64::INFINITY, "finite"),
            (f64::NEG_INFINITY, "finite"),
        ] {
            let cfg = GasBoilerConfig {
                afue,
                capacity_w: 10_000.0,
                ..GasBoilerConfig::default()
            };
            let err = cfg
                .validate()
                .expect_err(&format!("afue={afue} should be invalid"));
            let msg = err.to_string();
            assert!(
                msg.contains(expected_substring),
                "error for afue={afue} should mention '{expected_substring}', got: {msg}",
            );
        }
    }

    #[test]
    fn gas_boiler_default_condensing_and_return_temp_c_are_consistent() {
        let cfg = GasBoilerConfig::default();
        assert!(!cfg.condensing, "default must be non-condensing");
        assert!(
            cfg.return_temp_c >= 70.0,
            "non-condensing default return temp must be >= 70 °C, got {}",
            cfg.return_temp_c
        );
    }

    #[test]
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    fn gas_boiler_validate_rejects_contradictory_condensing_return_temp() {
        let base = GasBoilerConfig {
            capacity_w: 10_000.0,
            afue: 0.90,
            ..GasBoilerConfig::default()
        };
        // Non-condensing with condensing-level return temp
        let cfg = GasBoilerConfig {
            condensing: false,
            return_temp_c: 40.0,
            ..base.clone()
        };
        let err = cfg
            .validate()
            .expect_err("condensing=false + return_temp_c=40.0 must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("condensing=false") || msg.contains("return_temp_c"),
            "error must mention condensing and return_temp_c, got: {msg}"
        );
        // Condensing with non-condensing return temp
        let cfg2 = GasBoilerConfig {
            condensing: true,
            return_temp_c: 75.0,
            ..base
        };
        let err2 = cfg2
            .validate()
            .expect_err("condensing=true + return_temp_c=75.0 must be rejected");
        let msg2 = err2.to_string();
        assert!(
            msg2.contains("condensing=true") || msg2.contains("return_temp_c"),
            "error must mention condensing and return_temp_c, got: {msg2}"
        );
    }

    #[test]
    fn electric_boiler_default_return_temp_c_matches_non_condensing() {
        let cfg = ElectricBoilerConfig::default();
        assert!(
            cfg.return_temp_c >= 70.0,
            "ElectricBoilerConfig default return temp must be >= 70 °C, got {}",
            cfg.return_temp_c
        );
    }
}
