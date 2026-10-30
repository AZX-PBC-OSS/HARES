//! Typed configuration for battery storage equipment.

use serde::{Deserialize, Serialize};

use crate::config::EquipmentTypedConfig;

/// Typed configuration for a residential battery storage system.
///
/// `inverter_efficiency` is the one-way charge/discharge efficiency as a
/// fraction in (0, 1]. HPXML resolvers convert round-trip efficiency to
/// one-way by applying `sqrt()` once before constructing this struct.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatteryConfig {
    // Identity
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,

    // Core energy / power, SI (J, W)
    pub capacity_j: f64,
    pub max_charge_w: f64,
    pub max_discharge_w: f64,

    // Pack topology
    pub n_series: Option<u32>,
    pub n_parallel: Option<u32>,
    pub ah_cell: Option<f64>,
    pub v_cell: Option<f64>,
    pub cell_resistance_ohm: Option<f64>,
    /// Target pack voltage (V). Overrides the hardcoded 350 V used for
    /// ah_cell/v_cell topology derivation.
    pub pack_voltage_v: Option<f64>,

    // Chemistry
    pub chemistry: Option<String>,

    // Standby / self-discharge
    pub standby_power_w: Option<f64>,
    pub self_discharge_pct_per_day: Option<f64>,

    // SOC bounds and initial state
    pub min_soc: Option<f64>,
    pub max_soc: Option<f64>,
    pub initial_soc: Option<f64>,
    /// Initial cell temperature (°C). When `None`, the temperature is derived
    /// from the zone or ambient environment. When `Some`, overrides the
    /// ambient derivation to allow warm-start or cold-start scenarios.
    pub initial_cell_temp_c: Option<f64>,

    // Limits
    pub import_limit_w: Option<f64>,
    pub export_limit_w: Option<f64>,

    // Heater
    pub heater_power_w: Option<f64>,
    pub heater_threshold_c: Option<f64>,

    // Thermal model
    pub min_discharge_temp_c: Option<f64>,
    pub full_power_temp_c: Option<f64>,
    pub min_charge_temp_c: Option<f64>,
    pub cell_thermal_mass_j_per_k: Option<f64>,
    pub cell_ua_w_per_k: Option<f64>,

    // Efficiency -- one-way charge/discharge efficiency.
    // Overridden per-direction by charge_efficiency / discharge_efficiency.
    pub inverter_efficiency: Option<f64>,
    pub charge_efficiency: Option<f64>,
    pub discharge_efficiency: Option<f64>,

    // BMS / grid rules (serialised as string, parsed at init)
    pub bms_mode: Option<String>,
    pub grid_export_rule: Option<String>,
    // Reactive power / smart-inverter control (VPP / RL use cases).
    // power_factor defaults to 1.0 (no reactive); inverter_capacity_va
    // defaults to max(max_charge_w, max_discharge_w) at init time.
    pub power_factor: Option<f64>,
    pub inverter_capacity_va: Option<f64>,

    /// Whether the battery inverter can form an island bus during a utility
    /// outage. `None` (default) ⇒ true (grid-forming). Set `Some(false)` for
    /// grid-following-only batteries that cannot island the home even when
    /// charged and dischargeable.
    pub grid_forming: Option<bool>,

    // Actor-level dwell: minimum steps a mode/action must persist before it can change.
    // Defaults to 0 (no dwell enforcement) for backward compatibility.
    #[serde(default)]
    pub min_dwell_steps: usize,

    /// The inverter's IEEE 1547-2018 response: the assigned
    /// abnormal-performance category's default trip tables, the
    /// frequency-droop (frequency-Watt) parameters and the enter-service
    /// criteria, operator-settable per parameter. `None` (the default)
    /// leaves the layer unconfigured: the inverter behaves as the
    /// pre-1547 model did and the run's first grid excursion is warned
    /// with the excursion counted in `der1547_unconfigured_excursions`
    /// telemetry. The layer governs grid-connected operation: on a
    /// utility-outage step the outage model's contract governs instead
    /// (an island source is never tripped off its own island).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub der_1547: Option<crate::der1547::Der1547Settings>,
}

impl EquipmentTypedConfig for BatteryConfig {
    fn equipment_type_name() -> &'static str {
        "Battery"
    }
}

impl BatteryConfig {
    /// Validate fields for physical plausibility.
    pub fn validate(&self) -> crate::Result<()> {
        use hares_types::HaresError;
        if !self.capacity_j.is_finite() || self.capacity_j <= 0.0 {
            return Err(HaresError::Equipment(
                "battery capacity_j must be finite and > 0".to_string(),
            ));
        }
        if !self.max_charge_w.is_finite() || self.max_charge_w <= 0.0 {
            return Err(HaresError::Equipment(
                "battery max_charge_w must be finite and > 0".to_string(),
            ));
        }
        if !self.max_discharge_w.is_finite() || self.max_discharge_w <= 0.0 {
            return Err(HaresError::Equipment(
                "battery max_discharge_w must be finite and > 0".to_string(),
            ));
        }
        if let (Some(min), Some(max)) = (self.min_soc, self.max_soc)
            && min >= max
        {
            return Err(HaresError::Equipment(
                "battery min_soc must be less than max_soc".to_string(),
            ));
        }
        if let Some(n) = self.n_series
            && n == 0
        {
            return Err(HaresError::Equipment(
                "battery n_series must be > 0".to_string(),
            ));
        }
        if let Some(n) = self.n_parallel
            && n == 0
        {
            return Err(HaresError::Equipment(
                "battery n_parallel must be > 0".to_string(),
            ));
        }
        for (name, val) in [
            ("import_limit_w", self.import_limit_w),
            ("export_limit_w", self.export_limit_w),
            ("heater_power_w", self.heater_power_w),
            ("standby_power_w", self.standby_power_w),
        ] {
            if let Some(v) = val
                && (!v.is_finite() || v < 0.0)
            {
                return Err(HaresError::Equipment(format!(
                    "battery {name} must be finite and >= 0"
                )));
            }
        }
        // Temperature fields must be finite: a NaN cell temperature falls
        // through both comparison branches of `linear_temp_derate` (lib.rs)
        // into the interpolation, yielding a NaN discharge derate; a NaN
        // `min_charge_temp_c` makes `charge_allowed`'s `>=` comparison false
        // forever, silently disabling the plating-cutoff guard. Physically
        // extreme but finite temperatures are legal — the runtime
        // plausibility envelope is the degradation guard's job, not the
        // config boundary's.
        for (name, val) in [
            ("initial_cell_temp_c", self.initial_cell_temp_c),
            ("min_discharge_temp_c", self.min_discharge_temp_c),
            ("full_power_temp_c", self.full_power_temp_c),
            ("min_charge_temp_c", self.min_charge_temp_c),
            ("heater_threshold_c", self.heater_threshold_c),
        ] {
            if let Some(t) = val
                && !t.is_finite()
            {
                return Err(HaresError::Equipment(format!(
                    "battery {name} must be finite, got {t}"
                )));
            }
        }
        // Both derate ramps must be ordered: with an inverted
        // min > full pair, `linear_temp_derate`'s `temp >= temp_max`
        // branch wins everywhere above temp_max, silently returning full
        // power across the band the configuration meant to derate. Equal
        // pairs are legal — step cutoffs, not ramps.
        for (min_name, min_c, full_name, full_c) in [
            (
                "min_discharge_temp_c",
                self.min_discharge_temp_c,
                "full_power_temp_c",
                self.full_power_temp_c,
            ),
            (
                "min_charge_temp_c",
                self.min_charge_temp_c,
                "full_power_temp_c",
                self.full_power_temp_c,
            ),
        ] {
            if let (Some(lo), Some(hi)) = (min_c, full_c)
                && lo > hi
            {
                return Err(HaresError::Equipment(format!(
                    "battery {min_name} ({lo} °C) must not exceed \
                     {full_name} ({hi} °C) — the derate ramp would be \
                     inverted and return full power across the derate band"
                )));
            }
        }
        for (name, val) in [
            ("inverter_efficiency", self.inverter_efficiency),
            ("charge_efficiency", self.charge_efficiency),
            ("discharge_efficiency", self.discharge_efficiency),
        ] {
            if let Some(v) = val
                && (!v.is_finite() || v <= 0.0 || v > 1.0)
            {
                return Err(HaresError::Equipment(format!(
                    "battery {name} must be finite and within (0, 1]"
                )));
            }
        }
        if let Some(pf) = self.power_factor
            && (!pf.is_finite() || pf <= 0.0 || pf > 1.0)
        {
            return Err(HaresError::Equipment(
                "battery power_factor must be finite and within (0, 1]".to_string(),
            ));
        }
        if let Some(kva) = self.inverter_capacity_va
            && (!kva.is_finite() || kva <= 0.0)
        {
            return Err(HaresError::Equipment(
                "battery inverter_capacity_va must be finite and > 0".to_string(),
            ));
        }
        Ok(())
    }
}

/// The the battery's config's fields as the input schema publishes them.
impl hares_types::DescribeFields for BatteryConfig {
    fn fields() -> Vec<hares_types::FieldSchema> {
        crate::describe_fields::field_schemas::<Self>(&[
            (
                "equipment_id",
                hares_types::FieldMeta {
                    description: Some("the explicit equipment id the assembly assigns"),
                    ..Default::default()
                },
            ),
            (
                "zone_id",
                hares_types::FieldMeta {
                    description: Some("the zone the equipment sits in"),
                    ..Default::default()
                },
            ),
            (
                "capacity_j",
                hares_types::FieldMeta {
                    description: Some("the pack's capacity, J"),
                    ..Default::default()
                },
            ),
            (
                "max_charge_w",
                hares_types::FieldMeta {
                    description: Some("the maximum charge power, watts"),
                    ..Default::default()
                },
            ),
            (
                "max_discharge_w",
                hares_types::FieldMeta {
                    description: Some("the maximum discharge power, watts"),
                    ..Default::default()
                },
            ),
            (
                "n_series",
                hares_types::FieldMeta {
                    description: Some("the series cell count"),
                    ..Default::default()
                },
            ),
            (
                "n_parallel",
                hares_types::FieldMeta {
                    description: Some("the parallel cell count"),
                    ..Default::default()
                },
            ),
            (
                "ah_cell",
                hares_types::FieldMeta {
                    description: Some("the cell's capacity, ampere-hours"),
                    ..Default::default()
                },
            ),
            (
                "v_cell",
                hares_types::FieldMeta {
                    description: Some("the cell's nominal voltage, volts"),
                    ..Default::default()
                },
            ),
            (
                "cell_resistance_ohm",
                hares_types::FieldMeta {
                    description: Some("the cell's internal resistance, ohms"),
                    ..Default::default()
                },
            ),
            (
                "pack_voltage_v",
                hares_types::FieldMeta {
                    description: Some("the pack's nominal voltage, volts"),
                    ..Default::default()
                },
            ),
            (
                "chemistry",
                hares_types::FieldMeta {
                    description: Some("the cell chemistry"),
                    ..Default::default()
                },
            ),
            (
                "standby_power_w",
                hares_types::FieldMeta {
                    description: Some("the standby draw, watts"),
                    ..Default::default()
                },
            ),
            (
                "self_discharge_pct_per_day",
                hares_types::FieldMeta {
                    description: Some("the self-discharge rate, percent per day"),
                    ..Default::default()
                },
            ),
            (
                "min_soc",
                hares_types::FieldMeta {
                    description: Some("the minimum state of charge"),
                    ..Default::default()
                },
            ),
            (
                "max_soc",
                hares_types::FieldMeta {
                    description: Some("the maximum state of charge"),
                    ..Default::default()
                },
            ),
            (
                "initial_soc",
                hares_types::FieldMeta {
                    description: Some("the initial state of charge"),
                    ..Default::default()
                },
            ),
            (
                "initial_cell_temp_c",
                hares_types::FieldMeta {
                    description: Some("the cells' initial temperature, Celsius"),
                    ..Default::default()
                },
            ),
            (
                "import_limit_w",
                hares_types::FieldMeta {
                    description: Some("the grid import limit, watts"),
                    ..Default::default()
                },
            ),
            (
                "export_limit_w",
                hares_types::FieldMeta {
                    description: Some("the grid export limit, watts"),
                    ..Default::default()
                },
            ),
            (
                "heater_power_w",
                hares_types::FieldMeta {
                    description: Some("the battery heater's power, watts"),
                    ..Default::default()
                },
            ),
            (
                "heater_threshold_c",
                hares_types::FieldMeta {
                    description: Some("the battery heater's threshold, Celsius"),
                    ..Default::default()
                },
            ),
            (
                "min_discharge_temp_c",
                hares_types::FieldMeta {
                    description: Some("the minimum discharge temperature, Celsius"),
                    ..Default::default()
                },
            ),
            (
                "full_power_temp_c",
                hares_types::FieldMeta {
                    description: Some("the temperature above which full power flows, Celsius"),
                    ..Default::default()
                },
            ),
            (
                "min_charge_temp_c",
                hares_types::FieldMeta {
                    description: Some("the minimum charge temperature, Celsius"),
                    ..Default::default()
                },
            ),
            (
                "cell_thermal_mass_j_per_k",
                hares_types::FieldMeta {
                    description: Some("the cell's thermal mass, J/K"),
                    ..Default::default()
                },
            ),
            (
                "cell_ua_w_per_k",
                hares_types::FieldMeta {
                    description: Some("the cell's UA, W/K"),
                    ..Default::default()
                },
            ),
            (
                "inverter_efficiency",
                hares_types::FieldMeta {
                    description: Some("the inverter's efficiency"),
                    ..Default::default()
                },
            ),
            (
                "charge_efficiency",
                hares_types::FieldMeta {
                    description: Some("the charge path's efficiency"),
                    ..Default::default()
                },
            ),
            (
                "discharge_efficiency",
                hares_types::FieldMeta {
                    description: Some("the discharge path's efficiency"),
                    ..Default::default()
                },
            ),
            (
                "bms_mode",
                hares_types::FieldMeta {
                    description: Some("the battery management strategy"),
                    ..Default::default()
                },
            ),
            (
                "grid_export_rule",
                hares_types::FieldMeta {
                    description: Some("the grid export rule"),
                    ..Default::default()
                },
            ),
            (
                "power_factor",
                hares_types::FieldMeta {
                    description: Some("the power factor"),
                    ..Default::default()
                },
            ),
            (
                "inverter_capacity_va",
                hares_types::FieldMeta {
                    description: Some("the inverter's capacity, volt-amperes"),
                    ..Default::default()
                },
            ),
            (
                "grid_forming",
                hares_types::FieldMeta {
                    description: Some("whether the unit is grid-forming"),
                    ..Default::default()
                },
            ),
            (
                "min_dwell_steps",
                hares_types::FieldMeta {
                    description: Some("the minimum steps between setpoint changes"),
                    ..Default::default()
                },
            ),
            (
                "der_1547",
                hares_types::FieldMeta {
                    description: Some(
                        "the inverter's IEEE 1547-2018 response: the assigned \
                         abnormal-performance category (I, II or III) whose \
                         default trip, frequency-droop and enter-service \
                         settings govern, plus optional per-parameter \
                         overrides; absent means unconfigured (the pre-1547 \
                         behavior, excursions warned once and counted); the \
                         layer governs grid-connected operation only",
                    ),
                    ..Default::default()
                },
            ),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigPayload, EquipmentConfig};

    fn minimal_battery_config() -> BatteryConfig {
        BatteryConfig {
            der_1547: None,
            equipment_id: None,
            zone_id: None,
            capacity_j: 13.5 * 3.6e6,
            max_charge_w: 5000.0,
            max_discharge_w: 5000.0,
            n_series: None,
            n_parallel: None,
            ah_cell: None,
            v_cell: None,
            cell_resistance_ohm: None,
            pack_voltage_v: None,
            chemistry: None,
            standby_power_w: None,
            self_discharge_pct_per_day: None,
            min_soc: None,
            max_soc: None,
            initial_soc: None,
            initial_cell_temp_c: None,
            import_limit_w: None,
            export_limit_w: None,
            heater_power_w: None,
            heater_threshold_c: None,
            min_discharge_temp_c: None,
            full_power_temp_c: None,
            min_charge_temp_c: None,
            cell_thermal_mass_j_per_k: None,
            cell_ua_w_per_k: None,
            inverter_efficiency: None,
            charge_efficiency: None,
            discharge_efficiency: None,
            bms_mode: None,
            grid_export_rule: None,
            power_factor: None,
            inverter_capacity_va: None,
            grid_forming: None,
            min_dwell_steps: 0,
        }
    }

    #[test]
    fn battery_config_round_trips_via_equipment_config() {
        let cfg = minimal_battery_config();
        let ec = EquipmentConfig::from_typed(
            "test_battery".to_string(),
            "Battery".to_string(),
            cfg.clone(),
        )
        .unwrap();
        assert!(ec.is_typed());
        let recovered: BatteryConfig = ec.typed().unwrap();
        assert_eq!(recovered.capacity_j, cfg.capacity_j);
        assert_eq!(recovered.max_charge_w, cfg.max_charge_w);
    }

    #[test]
    fn battery_config_rejects_unknown_fields() {
        let json = serde_json::json!({
            "capacity_j": 48600000.0,
            "max_charge_w": 5000.0,
            "max_discharge_w": 5000.0,
            "unknown_field": 42.0
        });
        let ec = EquipmentConfig::with_payload(
            "test".to_string(),
            "Battery".to_string(),
            ConfigPayload::Typed {
                type_name: "Battery".to_string(),
                version: 1,
                data: json,
            },
        );
        let result: crate::Result<BatteryConfig> = ec.typed();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown field"));
    }

    #[test]
    fn battery_config_validate_rejects_zero_capacity() {
        let mut cfg = minimal_battery_config();
        cfg.capacity_j = 0.0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn battery_config_validate_rejects_inverted_soc_bounds() {
        let mut cfg = minimal_battery_config();
        cfg.min_soc = Some(0.9);
        cfg.max_soc = Some(0.1);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn battery_config_validate_rejects_out_of_range_efficiency() {
        let mut cfg = minimal_battery_config();
        cfg.inverter_efficiency = Some(1.5);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn battery_config_validate_passes_for_valid_config() {
        let cfg = minimal_battery_config();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn battery_config_validate_rejects_out_of_range_power_factor() {
        let mut cfg = minimal_battery_config();
        cfg.power_factor = Some(1.5);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn battery_config_validate_rejects_zero_power_factor() {
        let mut cfg = minimal_battery_config();
        cfg.power_factor = Some(0.0);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn battery_config_validate_rejects_negative_inverter_capacity() {
        let mut cfg = minimal_battery_config();
        cfg.inverter_capacity_va = Some(-1.0);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn battery_config_validate_accepts_power_factor_and_inverter_capacity() {
        let mut cfg = minimal_battery_config();
        cfg.power_factor = Some(0.95);
        cfg.inverter_capacity_va = Some(7000.0);
        assert!(cfg.validate().is_ok());
    }

    /// Temperature fields must be finite and both derate ramps ordered —
    /// the same validation class as the EV's config (a NaN temperature
    /// poisons `linear_temp_derate` and `charge_allowed` comparisons
    /// silently; an inverted min > full ramp returns full power across
    /// the derate band). Finite extremes and equal (step-cutoff) pairs
    /// stay legal.
    #[test]
    fn battery_config_validate_rejects_non_finite_or_inverted_temperature_fields() {
        let rejects = |mutate: &dyn Fn(&mut BatteryConfig)| {
            let mut cfg = minimal_battery_config();
            mutate(&mut cfg);
            cfg.validate()
        };

        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                rejects(&|c| c.initial_cell_temp_c = Some(bad)).is_err(),
                "non-finite initial_cell_temp_c ({bad}) must be rejected"
            );
            assert!(
                rejects(&|c| c.min_discharge_temp_c = Some(bad)).is_err(),
                "non-finite min_discharge_temp_c ({bad}) must be rejected"
            );
            assert!(
                rejects(&|c| c.full_power_temp_c = Some(bad)).is_err(),
                "non-finite full_power_temp_c ({bad}) must be rejected"
            );
            assert!(
                rejects(&|c| c.min_charge_temp_c = Some(bad)).is_err(),
                "non-finite min_charge_temp_c ({bad}) must be rejected"
            );
            assert!(
                rejects(&|c| c.heater_threshold_c = Some(bad)).is_err(),
                "non-finite heater_threshold_c ({bad}) must be rejected"
            );
        }

        assert!(
            rejects(&|c| {
                c.min_charge_temp_c = Some(20.0);
                c.full_power_temp_c = Some(10.0);
            })
            .is_err(),
            "inverted min_charge_temp_c > full_power_temp_c must be rejected"
        );
        assert!(
            rejects(&|c| {
                c.min_discharge_temp_c = Some(-15.0);
                c.full_power_temp_c = Some(-20.0);
            })
            .is_err(),
            "inverted min_discharge_temp_c > full_power_temp_c must be rejected"
        );

        // Boundary-legal: physically extreme but finite temperatures, and
        // equal (step-cutoff) ramp pairs.
        assert!(rejects(&|c| c.initial_cell_temp_c = Some(-40.0)).is_ok());
        assert!(rejects(&|c| c.min_discharge_temp_c = Some(-25.0)).is_ok());
        assert!(rejects(&|c| c.full_power_temp_c = Some(60.0)).is_ok());
        assert!(rejects(&|c| c.heater_threshold_c = Some(45.0)).is_ok());
        assert!(
            rejects(&|c| {
                c.min_charge_temp_c = Some(10.0);
                c.full_power_temp_c = Some(10.0);
            })
            .is_ok(),
            "an equal min/full pair is a legal step cutoff"
        );
    }
}
