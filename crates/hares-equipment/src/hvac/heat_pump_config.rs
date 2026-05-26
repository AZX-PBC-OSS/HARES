//! Typed configuration structs for heat pump HVAC equipment.

use serde::{Deserialize, Serialize};

use super::core_config::default_one;
use super::heat_pump::defrost::DefrostConfig;
use super::heating_config::DuctConfig;

fn default_min_compressor_fraction() -> f64 {
    0.25
}
use super::speed_control::SpeedControlMode;
use crate::config::EquipmentTypedConfig;
use hares_types::{FuelType, ScheduleSourceConfig};

/// Fields shared between `HeatPumpHeaterConfig` and `HeatPumpCoolerConfig`.
///
/// `deny_unknown_fields` is intentionally omitted: serde `flatten` is
/// incompatible with `deny_unknown_fields` on both the inner (flattened) struct
/// and the outer struct that flattens it. Neither struct uses it.
/// See <https://serde.rs/attr-flatten.html> and <https://github.com/serde-rs/serde/issues/2384>.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HeatPumpCommonConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heating_capacity_w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heating_eir: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_heating_capacities_w: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_heating_eirs: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_fuel: Option<FuelType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_capacity_w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_eir: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction_heating_load_served: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooling_capacity_w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooling_eir: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_cooling_capacities_w: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_cooling_eirs: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction_cooling_load_served: Option<f64>,
    #[serde(default = "default_one")]
    pub number_of_speeds: u8,
    #[serde(default)]
    pub is_mini_split: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shr: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w_per_cfm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub airflow_m3_s_per_w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heating_setpoint_c: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooling_setpoint_c: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hysteresis_c: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heating_setpoint_source: Option<ScheduleSourceConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooling_setpoint_source: Option<ScheduleSourceConfig>,
    #[serde(flatten)]
    pub duct: DuctConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biquadratic_x1_min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biquadratic_x1_max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biquadratic_x2_min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biquadratic_x2_max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ff_min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ff_max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plf_min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plf_max: Option<f64>,
    /// Minimum compressor speed as a fraction of rated capacity for mini-split heat pumps.
    /// Default 0.25 matches the original hardcoded value; OCHRE uses 0.40 for MSHP Heater
    /// (from "HVAC Multispeed Parameters.csv"). EnergyPlus sets minimum compressor capacity
    /// via the ratio of Speed 1 to Speed N `Gross Rated Heating Capacity` fields in
    /// `Coil:Heating:DX:MultiSpeed` — there is no single "minimum fraction" parameter.
    #[serde(default = "default_min_compressor_fraction")]
    pub min_compressor_fraction: f64,
    /// Fractional EIR reduction at part load: `eir[i] = rated_eir * (1 - benefit * (1 - frac[i]))`.
    /// Default `None` = 0.0 (constant EIR, current behavior). Values of 0.1–0.2 are typical
    /// for inverter compressors where COP improves at lower compressor speeds due to reduced
    /// pressure ratio (AHRI 210/240 variable-speed test procedure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eir_part_load_benefit: Option<f64>,
    /// Number of discrete electric-resistance backup heating stages.
    /// Residential ER backup is typically a single binary element (1 stage) or
    /// 2–3 sequenced strips activated by an outdoor thermostat (ASHRAE HVAC Systems
    /// and Equipment 2020 Ch.9). Each stage is an on/off resistive element; there is
    /// no continuous modulation within a stage. Valid range: 1–4.
    /// AHRI 210/240 rates supplemental ER at nominal capacity, consistent with
    /// discrete staged operation rather than modulation.
    #[serde(default = "default_one")]
    pub er_stages: u8,
    /// Refrigerant charge defect ratio `(InstalledCharge - DesignCharge) / DesignCharge`.
    /// Applied multiplicatively to all rated capacity and EIR values during equipment init.
    /// Correction factors: ANSI/RESNET/ACCA 310-2020.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charge_defect_ratio: Option<f64>,

    // ── Ground-loop circulation pump (GSHP only) ───
    /// Vertical borehole loop depth [m]. Default 60 m (≈200 ft), typical for
    /// residential vertical ground loops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pump_loop_depth_m: Option<f64>,
    /// Inner diameter of the HDPE U-bend pipe [m]. Default 0.025 m (1″ nominal
    /// SDR11 pipe, ≈ 0.027 m OD, ≈ 0.022 m ID — HARES default rounds to 0.025).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pump_pipe_diameter_m: Option<f64>,
    /// Design volumetric flow rate through the circulation loop [m³/s].
    /// Default 0.00019 m³/s (≈3 US GPM per borehole), sized for a single
    /// borehole at 3 GPM/ton for a 1‑ton system. Scale for multi‑ton or
    /// multi‑borehole installations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pump_flow_rate_m3_per_s: Option<f64>,
    /// Hydraulic‑to‑shaft efficiency of the circulator pump [-].
    /// Default 0.35, typical for small wet‑rotor circulators at design point.
    /// Combined wire‑to‑water efficiency = `pump_efficiency * pump_motor_efficiency`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pump_efficiency: Option<f64>,
    /// Electrical‑to‑shaft efficiency of the pump motor [-].
    /// Default 0.40, typical for PSC fractional‑HP motors. ECM motors reach
    /// 0.70–0.85; set accordingly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pump_motor_efficiency: Option<f64>,
    /// Additional head loss beyond the borehole loop [m], covering the
    /// water‑to‑refrigerant heat exchanger, distribution headers, isolation
    /// valves, and strainer. Default 3.0 m for a typical residential 4‑ton
    /// brazed‑plate HX per ASHRAE HVAC Systems & Equipment 2020 Ch.9 Table 7.
    /// Set to 0.0 for borehole‑loop‑only head loss.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pump_system_head_loss_m: Option<f64>,
}

impl Default for HeatPumpCommonConfig {
    fn default() -> Self {
        Self {
            equipment_id: None,
            zone_id: None,
            heating_capacity_w: None,
            heating_eir: None,
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            backup_fuel: None,
            backup_capacity_w: None,
            backup_eir: None,
            fraction_heating_load_served: None,
            cooling_capacity_w: None,
            cooling_eir: None,
            stage_cooling_capacities_w: None,
            stage_cooling_eirs: None,
            fraction_cooling_load_served: None,
            number_of_speeds: 1,
            is_mini_split: false,
            shr: None,
            fan_power_w: None,
            fan_power_w_per_cfm: None,
            airflow_m3_s_per_w: None,
            heating_setpoint_c: None,
            cooling_setpoint_c: None,
            hysteresis_c: None,
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
            duct: DuctConfig::default(),
            biquadratic_x1_min: None,
            biquadratic_x1_max: None,
            biquadratic_x2_min: None,
            biquadratic_x2_max: None,
            ff_min: None,
            ff_max: None,
            plf_min: None,
            plf_max: None,
            min_compressor_fraction: 0.25,
            eir_part_load_benefit: None,
            er_stages: 1,
            charge_defect_ratio: None,
            pump_loop_depth_m: None,
            pump_pipe_diameter_m: None,
            pump_flow_rate_m3_per_s: None,
            pump_efficiency: None,
            pump_motor_efficiency: None,
            pump_system_head_loss_m: None,
        }
    }
}

/// Typed configuration for heat-pump heaters (ASHP and MSHP heating side).
///
/// For mini-splits, `number_of_speeds` is forced to 4 in the equipment init path,
/// not in this struct -- the struct records user intent; the equipment enforces the rule.
///
/// `deny_unknown_fields` is intentionally omitted: serde `flatten` is
/// incompatible with `deny_unknown_fields` on both the inner and outer struct.
/// See <https://serde.rs/attr-flatten.html> and <https://github.com/serde-rs/serde/issues/2384>.
/// The `heater_ignores_cooler_only_field_stage_shrs` and
/// `cooler_ignores_heater_only_field_hp_lockout` tests guard against key leakage
/// between heater and cooler structs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HeatPumpHeaterConfig {
    #[serde(flatten)]
    pub common: HeatPumpCommonConfig,
    /// Heat-pump lockout below this outdoor temperature [C].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hp_lockout_temp_c: Option<f64>,
    /// Electric-resistance lockout below this outdoor temperature [C].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub er_lockout_temp_c: Option<f64>,
    /// EnergyPlus supplemental-ER upper OAT cap [C].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_oat_supplemental_c: Option<f64>,
    /// ER call threshold offset below the heating setpoint [C].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub er_setpoint_offset_c: Option<f64>,
    /// ER hard lockout duration after a setpoint raise [s].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub er_hard_lockout_time_s: Option<f64>,
    /// Heating-side sensible heat ratio. Default 1.0 (all-sensible) matching
    /// OCHRE HVAC.py:458-462 which returns SHR=1 for all heating modes.
    /// When < 1.0, a small positive latent gain is emitted during
    /// reverse-cycle defrost from indoor-coil surface moisture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heating_shr: Option<f64>,
    /// Ratio of heating capacity at 17°F (-8.33°C) to rated capacity at
    /// 47°F (8.33°C), from HPXML HeatingCapacity17F / HeatingCapacity.
    /// AHRI 210/240 H3 low-ambient rating point for ASHPs.
    /// When present, the capacity biquadratic curve coefficients are scaled
    /// linearly so the H3 evaluation matches this ratio. A pre-scaling
    /// deviation >10% produces a warning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_ratio_at_17f: Option<f64>,
    /// Defrost configuration: control mode, strategy, timing, capacity.
    /// Flattened into the same JSON namespace as heater-only fields.
    /// All fields default to the OnDemand/ReverseCycle values matching
    /// the previous `DefrostConfig::on_demand(1.0, 0.0)` hardcoded path.
    #[serde(flatten)]
    pub defrost: DefrostConfig,
}

impl EquipmentTypedConfig for HeatPumpHeaterConfig {
    fn equipment_type_name() -> &'static str {
        super::core_config::equipment_type_name::ASHP_HEATER
    }
}

impl HeatPumpHeaterConfig {
    /// Returns the effective number of speeds, forcing 4 when `is_mini_split` is true.
    pub fn effective_number_of_speeds(&self) -> u8 {
        if self.common.is_mini_split {
            4
        } else {
            self.common.number_of_speeds
        }
    }

    /// Validate that capacity and efficiency fields are finite and positive when present.
    pub fn validate(&self) -> crate::Result<()> {
        use hares_types::HaresError;
        let check = |v: f64, field: &str| -> crate::Result<()> {
            if !v.is_finite() || v <= 0.0 {
                Err(HaresError::Equipment(format!(
                    "HeatPumpHeaterConfig: {field} must be finite and positive, got {v}"
                )))
            } else {
                Ok(())
            }
        };
        if let Some(v) = self.common.heating_capacity_w {
            check(v, "heating_capacity_w")?;
        }
        if let Some(v) = self.common.heating_eir {
            check(v, "heating_eir")?;
        }
        if let Some(v) = self.common.cooling_capacity_w {
            check(v, "cooling_capacity_w")?;
        }
        if let Some(v) = self.common.cooling_eir {
            check(v, "cooling_eir")?;
        }
        if let Some(v) = self.common.backup_capacity_w {
            if v.is_nan() || (v < 0.0 && v.is_finite()) {
                return Err(HaresError::Equipment(format!(
                    "HeatPumpHeaterConfig: backup_capacity_w must be finite and non-negative, got {v}"
                )));
            }
        }
        for (name, value) in [
            ("heating_setpoint_c", self.common.heating_setpoint_c),
            ("cooling_setpoint_c", self.common.cooling_setpoint_c),
            ("hysteresis_c", self.common.hysteresis_c),
            ("airflow_m3_s_per_w", self.common.airflow_m3_s_per_w),
            ("hp_lockout_temp_c", self.hp_lockout_temp_c),
            ("er_lockout_temp_c", self.er_lockout_temp_c),
            ("max_oat_supplemental_c", self.max_oat_supplemental_c),
            ("er_setpoint_offset_c", self.er_setpoint_offset_c),
            ("er_hard_lockout_time_s", self.er_hard_lockout_time_s),
            ("heating_shr", self.heating_shr),
        ] {
            if let Some(v) = value
                && !v.is_finite()
            {
                return Err(HaresError::Equipment(format!(
                    "HeatPumpHeaterConfig: {name} must be finite, got {v}"
                )));
            }
        }
        if let Some(v) = self.common.hysteresis_c
            && v < 0.0
        {
            return Err(HaresError::Equipment(
                "HeatPumpHeaterConfig: hysteresis_c must be >= 0".to_string(),
            ));
        }
        if let Some(v) = self.common.airflow_m3_s_per_w
            && v <= 0.0
        {
            return Err(HaresError::Equipment(
                "HeatPumpHeaterConfig: airflow_m3_s_per_w must be > 0".to_string(),
            ));
        }
        if let Some(v) = self.er_hard_lockout_time_s
            && v < 0.0
        {
            return Err(HaresError::Equipment(
                "HeatPumpHeaterConfig: er_hard_lockout_time_s must be >= 0".to_string(),
            ));
        }
        if let Some(v) = self.heating_shr
            && !(0.0..=1.0).contains(&v)
        {
            return Err(HaresError::Equipment(format!(
                "HeatPumpHeaterConfig: heating_shr must be in [0, 1], got {v}"
            )));
        }
        if !self.common.is_mini_split {
            let n_speeds = self.effective_number_of_speeds() as usize;
            if n_speeds >= 2 {
                if let Some(stages) = &self.common.stage_heating_capacities_w {
                    if stages.len() != n_speeds {
                        return Err(HaresError::Equipment(format!(
                            "HeatPumpHeaterConfig: stage_heating_capacities_w has {} elements but number_of_speeds is {}; lengths must match",
                            stages.len(),
                            n_speeds,
                        )));
                    }
                }
                if let Some(stages) = &self.common.stage_heating_eirs {
                    if stages.len() != n_speeds {
                        return Err(HaresError::Equipment(format!(
                            "HeatPumpHeaterConfig: stage_heating_eirs has {} elements but number_of_speeds is {}; lengths must match",
                            stages.len(),
                            n_speeds,
                        )));
                    }
                }
            }
        }
        if self.common.is_mini_split {
            let v = self.common.min_compressor_fraction;
            if !v.is_finite() || !(0.1..=0.5).contains(&v) {
                return Err(HaresError::Equipment(format!(
                    "HeatPumpHeaterConfig: min_compressor_fraction must be in [0.1, 0.5], got {v}"
                )));
            }
        }
        if let Some(v) = self.common.eir_part_load_benefit {
            if !v.is_finite() || !(0.0..=1.0).contains(&v) {
                return Err(HaresError::Equipment(format!(
                    "HeatPumpHeaterConfig: eir_part_load_benefit must be in [0.0, 1.0], got {v}"
                )));
            }
            if v > 0.5 {
                tracing::warn!(
                    benefit = v,
                    "HeatPumpHeaterConfig: eir_part_load_benefit={v} exceeds 0.5; \
                     measured COP improvement at minimum stage vs rated is typically 30–50% \
                     for inverter compressors (AHRI 210/240, NREL field studies)"
                );
            }
        }
        if !(1..=4).contains(&self.common.er_stages) {
            return Err(HaresError::Equipment(format!(
                "HeatPumpHeaterConfig: er_stages must be in [1, 4], got {}",
                self.common.er_stages
            )));
        }
        self.defrost
            .validate()
            .map_err(|e| HaresError::Equipment(format!("HeatPumpHeaterConfig: {e}")))?;
        Ok(())
    }
}

/// Typed configuration for heat-pump coolers (ASHP and MSHP cooling side).
///
/// Contains the same fields as `HeatPumpHeaterConfig` but registers under the
/// "ASHP Cooler" equipment type name so that typed configs round-trip correctly.
///
/// `deny_unknown_fields` is intentionally omitted: serde `flatten` is
/// incompatible with `deny_unknown_fields` on both the inner and outer struct.
/// See <https://serde.rs/attr-flatten.html> and <https://github.com/serde-rs/serde/issues/2384>.
/// The `cooler_ignores_heater_only_field_hp_lockout` and
/// `heater_ignores_cooler_only_field_stage_shrs` tests guard against key leakage.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HeatPumpCoolerConfig {
    #[serde(flatten)]
    pub common: HeatPumpCommonConfig,
    /// Per-stage sensible heat ratios.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_shrs: Option<Vec<f64>>,
    /// Crankcase heater rated power [kW]. Applied as parasitic load when the
    /// compressor is off and outdoor air temperature is below the threshold.
    /// ASHP default 0.050 kW (50 W) at 12.78°C (55°F); MSHP default 0.015 kW
    /// (15 W) at 0°C (32°F). OCHRE HVAC.py AirConditioner class.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crankcase_heater_kw: Option<f64>,
    /// Crankcase heater activation threshold [°C]. The heater draws power when
    /// OAT < this threshold and the compressor is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crankcase_heater_threshold_c: Option<f64>,
}

impl EquipmentTypedConfig for HeatPumpCoolerConfig {
    fn equipment_type_name() -> &'static str {
        super::core_config::equipment_type_name::ASHP_COOLER
    }
}

impl HeatPumpCoolerConfig {
    /// Returns the effective number of speeds, forcing 4 when `is_mini_split` is true.
    pub fn effective_number_of_speeds(&self) -> u8 {
        if self.common.is_mini_split {
            4
        } else {
            self.common.number_of_speeds
        }
    }

    /// Cooling-side control mode derived from the typed config.
    ///
    /// Mini-split coolers are modeled as continuously variable-speed equipment.
    pub fn cooling_speed_control_mode(&self) -> SpeedControlMode {
        let n = self.effective_number_of_speeds();
        match n {
            1 => SpeedControlMode::SingleSpeed,
            2 => SpeedControlMode::TwoSpeedSetpoint,
            n if n >= 4 => SpeedControlMode::VariableSpeedIdeal,
            _ => SpeedControlMode::SingleSpeed,
        }
    }

    /// Derived startup degradation coefficient for typed cooling operation.
    ///
    /// Single-speed coolers retain the generic HVAC-core default unless an explicit
    /// typed startup Cd is available upstream.
    pub fn derived_cooling_startup_cd(&self) -> Option<f64> {
        match self.cooling_speed_control_mode() {
            SpeedControlMode::VariableSpeedIdeal => Some(0.0),
            SpeedControlMode::TwoSpeedSetpoint
            | SpeedControlMode::TwoSpeedTime
            | SpeedControlMode::TwoSpeedAlternating => Some(0.11),
            SpeedControlMode::SingleSpeed | SpeedControlMode::MultiSpeedInterpolated => None,
        }
    }

    /// Validate that cooling-side fields are finite and positive when present.
    pub fn validate(&self) -> crate::Result<()> {
        use hares_types::HaresError;

        let check_positive = |value: f64, field: &str| -> crate::Result<()> {
            if !value.is_finite() || value <= 0.0 {
                Err(HaresError::Equipment(format!(
                    "HeatPumpCoolerConfig: {field} must be finite and positive, got {value}"
                )))
            } else {
                Ok(())
            }
        };

        if let Some(value) = self.common.cooling_capacity_w {
            check_positive(value, "cooling_capacity_w")?;
        }
        if let Some(value) = self.common.cooling_eir {
            check_positive(value, "cooling_eir")?;
        }
        if let Some(value) = self.common.heating_capacity_w {
            check_positive(value, "heating_capacity_w")?;
        }
        if let Some(value) = self.common.heating_eir {
            check_positive(value, "heating_eir")?;
        }
        if let Some(value) = self.common.backup_capacity_w
            && (!value.is_finite() || value < 0.0)
        {
            return Err(HaresError::Equipment(format!(
                "HeatPumpCoolerConfig: backup_capacity_w must be finite and non-negative, got {value}"
            )));
        }

        for (name, value) in [
            ("heating_setpoint_c", self.common.heating_setpoint_c),
            ("cooling_setpoint_c", self.common.cooling_setpoint_c),
            ("hysteresis_c", self.common.hysteresis_c),
            ("airflow_m3_s_per_w", self.common.airflow_m3_s_per_w),
        ] {
            if let Some(value) = value
                && !value.is_finite()
            {
                return Err(HaresError::Equipment(format!(
                    "HeatPumpCoolerConfig: {name} must be finite, got {value}"
                )));
            }
        }

        if let Some(value) = self.common.hysteresis_c
            && value < 0.0
        {
            return Err(HaresError::Equipment(
                "HeatPumpCoolerConfig: hysteresis_c must be >= 0".to_string(),
            ));
        }
        if let Some(value) = self.common.airflow_m3_s_per_w
            && value <= 0.0
        {
            return Err(HaresError::Equipment(
                "HeatPumpCoolerConfig: airflow_m3_s_per_w must be > 0".to_string(),
            ));
        }

        if self.common.is_mini_split {
            let v = self.common.min_compressor_fraction;
            if !v.is_finite() || !(0.1..=0.5).contains(&v) {
                return Err(HaresError::Equipment(format!(
                    "HeatPumpCoolerConfig: min_compressor_fraction must be in [0.1, 0.5], got {v}"
                )));
            }
        }
        if let Some(v) = self.common.eir_part_load_benefit {
            if !v.is_finite() || !(0.0..=1.0).contains(&v) {
                return Err(HaresError::Equipment(format!(
                    "HeatPumpCoolerConfig: eir_part_load_benefit must be in [0.0, 1.0], got {v}"
                )));
            }
            if v > 0.5 {
                tracing::warn!(
                    benefit = v,
                    "HeatPumpCoolerConfig: eir_part_load_benefit={v} exceeds 0.5; \
                     measured COP improvement at minimum stage vs rated is typically 30–50% \
                     for inverter compressors (AHRI 210/240, NREL field studies)"
                );
            }
        }

        Ok(())
    }
}

/// Alias for `HeatPumpHeaterConfig`; prefer the full name for new code.
pub type HeatPumpConfig = HeatPumpHeaterConfig;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigPayload, EquipmentConfig};

    fn typed_config<T: EquipmentTypedConfig>(config: T) -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "test".to_string(),
            T::equipment_type_name().to_string(),
            config,
        )
    }

    #[test]
    fn heat_pump_heater_config_round_trips() {
        let cfg = HeatPumpHeaterConfig {
            common: HeatPumpCommonConfig {
                equipment_id: Some(1),
                zone_id: Some(1),
                heating_capacity_w: Some(10_000.0),
                heating_eir: Some(9.0),
                stage_heating_capacities_w: Some(vec![5_000.0, 10_000.0]),
                stage_heating_eirs: Some(vec![0.28, 0.25]),
                backup_fuel: Some(FuelType::Electric),
                backup_capacity_w: Some(5_000.0),
                backup_eir: Some(1.0),
                fraction_heating_load_served: Some(1.0),
                cooling_capacity_w: Some(12_000.0),
                cooling_eir: Some(16.0),
                stage_cooling_capacities_w: Some(vec![6_000.0, 12_000.0]),
                stage_cooling_eirs: Some(vec![0.25, 0.22]),
                fraction_cooling_load_served: Some(1.0),
                number_of_speeds: 1,
                is_mini_split: true,
                shr: Some(0.75),
                fan_power_w: Some(300.0),
                fan_power_w_per_cfm: None,
                airflow_m3_s_per_w: Some(4.0e-5),
                heating_setpoint_c: Some(21.0),
                cooling_setpoint_c: Some(26.0),
                hysteresis_c: Some(1.0),
                heating_setpoint_source: None,
                cooling_setpoint_source: None,
                duct: DuctConfig::default(),
                biquadratic_x1_min: Some(12.0),
                biquadratic_x1_max: Some(24.0),
                biquadratic_x2_min: Some(18.0),
                biquadratic_x2_max: Some(46.0),
                ff_min: Some(0.6),
                ff_max: Some(1.2),
                plf_min: Some(0.7),
                plf_max: Some(1.0),
                min_compressor_fraction: 0.25,
                eir_part_load_benefit: None,
                er_stages: 1,
                charge_defect_ratio: None,
                ..Default::default()
            },
            hp_lockout_temp_c: Some(-17.8),
            er_lockout_temp_c: Some(4.4),
            max_oat_supplemental_c: Some(21.0),
            er_setpoint_offset_c: Some(1.6),
            er_hard_lockout_time_s: Some(600.0),
            heating_shr: None,
            capacity_ratio_at_17f: None,
            defrost: DefrostConfig::default(),
        };
        let ec = typed_config(cfg.clone());
        let recovered: HeatPumpHeaterConfig = ec.typed().unwrap();
        assert!(
            (recovered.common.cooling_eir.unwrap() - cfg.common.cooling_eir.unwrap()).abs() < 1e-12
        );
        assert!(recovered.common.is_mini_split);
        assert_eq!(
            recovered.common.biquadratic_x1_min,
            cfg.common.biquadratic_x1_min
        );
        assert_eq!(recovered.common.ff_min, cfg.common.ff_min);
    }

    #[test]
    fn heat_pump_cooler_effective_control_mode_tracks_typed_intent() {
        let mut cfg = HeatPumpCoolerConfig {
            common: HeatPumpCommonConfig {
                cooling_capacity_w: Some(12_000.0),
                cooling_eir: Some(0.25),
                number_of_speeds: 2,
                ..HeatPumpCommonConfig::default()
            },
            stage_shrs: None,
            ..HeatPumpCoolerConfig::default()
        };

        assert_eq!(
            cfg.cooling_speed_control_mode(),
            SpeedControlMode::TwoSpeedSetpoint
        );
        assert_eq!(cfg.derived_cooling_startup_cd(), Some(0.11));

        cfg.common.number_of_speeds = 4;
        cfg.common.is_mini_split = true;

        assert_eq!(cfg.effective_number_of_speeds(), 4);
        assert_eq!(
            cfg.cooling_speed_control_mode(),
            SpeedControlMode::VariableSpeedIdeal
        );
        assert_eq!(cfg.derived_cooling_startup_cd(), Some(0.0));
    }

    #[test]
    fn heat_pump_cooler_config_round_trips() {
        let cfg = HeatPumpCoolerConfig {
            common: HeatPumpCommonConfig {
                equipment_id: Some(2),
                zone_id: Some(1),
                heating_capacity_w: Some(9_000.0),
                heating_eir: Some(8.8),
                backup_capacity_w: Some(5_000.0),
                backup_eir: Some(1.0),
                fraction_heating_load_served: Some(1.0),
                cooling_capacity_w: Some(12_000.0),
                cooling_eir: Some(16.0),
                stage_cooling_capacities_w: Some(vec![6_000.0, 12_000.0]),
                stage_cooling_eirs: Some(vec![0.25, 0.22]),
                fraction_cooling_load_served: Some(1.0),
                number_of_speeds: 2,
                shr: Some(0.75),
                fan_power_w: Some(300.0),
                airflow_m3_s_per_w: Some(4.0e-5),
                heating_setpoint_c: Some(21.0),
                cooling_setpoint_c: Some(26.0),
                hysteresis_c: Some(1.0),
                duct: DuctConfig::default(),
                biquadratic_x1_min: Some(12.0),
                biquadratic_x1_max: Some(24.0),
                biquadratic_x2_min: Some(18.0),
                biquadratic_x2_max: Some(46.0),
                ff_min: Some(0.6),
                ff_max: Some(1.2),
                plf_min: Some(0.7),
                plf_max: Some(1.0),
                ..HeatPumpCommonConfig::default()
            },
            stage_shrs: Some(vec![0.78, 0.72]),
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
        };
        let ec =
            EquipmentConfig::from_typed("test".to_string(), "ASHP Cooler".to_string(), cfg.clone());
        let recovered: HeatPumpCoolerConfig = ec.typed().unwrap();
        assert!(
            (recovered.common.cooling_eir.unwrap() - cfg.common.cooling_eir.unwrap()).abs() < 1e-12
        );
        assert_eq!(recovered.common.number_of_speeds, 2);
        assert_eq!(recovered.stage_shrs, cfg.stage_shrs);
    }

    #[test]
    fn heat_pump_mini_split_effective_speeds() {
        let cfg = HeatPumpHeaterConfig {
            common: HeatPumpCommonConfig {
                cooling_capacity_w: Some(12_000.0),
                cooling_eir: Some(16.0),
                is_mini_split: true,
                ..HeatPumpCommonConfig::default()
            },
            ..Default::default()
        };
        assert_eq!(cfg.effective_number_of_speeds(), 4);
    }

    #[test]
    fn heat_pump_number_of_speeds_defaults_to_one() {
        let json = serde_json::json!({
            "cooling_capacity_w": 12000.0,
            "cooling_eir": 16.0,
        });
        let cfg: HeatPumpHeaterConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.common.number_of_speeds, 1);
    }

    /// serde flatten is incompatible with `deny_unknown_fields`, so truly
    /// unknown keys are silently ignored rather than rejected. This test
    /// documents that behavior — unknown keys do not cause an error.
    #[test]
    fn heat_pump_heater_ignores_unknown_fields() {
        let data = serde_json::json!({
            "cooling_capacity_w": 12000.0,
            "cooling_eir": 16.0,
            "unknown_key": true,
        });
        let ec = EquipmentConfig::with_payload(
            "test".to_string(),
            "ASHP Heater".to_string(),
            ConfigPayload::Typed {
                type_name: "ASHP Heater".to_string(),
                version: 1,
                data,
            },
        );
        let cfg: HeatPumpHeaterConfig = ec.typed().unwrap();
        assert_eq!(cfg.common.cooling_eir.unwrap(), 16.0);
    }

    #[test]
    fn heat_pump_heater_validate_rejects_negative_capacity() {
        let cfg = HeatPumpHeaterConfig {
            common: HeatPumpCommonConfig {
                heating_capacity_w: Some(-1.0),
                ..HeatPumpCommonConfig::default()
            },
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn heat_pump_heater_config_serde_json_round_trip() {
        let cfg = HeatPumpHeaterConfig {
            common: HeatPumpCommonConfig {
                equipment_id: Some(1),
                zone_id: Some(1),
                heating_capacity_w: Some(10_000.0),
                heating_eir: Some(9.0),
                stage_heating_capacities_w: Some(vec![5_000.0, 10_000.0]),
                stage_heating_eirs: Some(vec![0.28, 0.25]),
                backup_fuel: Some(FuelType::Electric),
                backup_capacity_w: Some(5_000.0),
                backup_eir: Some(1.0),
                fraction_heating_load_served: Some(1.0),
                cooling_capacity_w: Some(12_000.0),
                cooling_eir: Some(16.0),
                stage_cooling_capacities_w: Some(vec![6_000.0, 12_000.0]),
                stage_cooling_eirs: Some(vec![0.25, 0.22]),
                fraction_cooling_load_served: Some(1.0),
                number_of_speeds: 2,
                is_mini_split: false,
                shr: Some(0.75),
                fan_power_w: Some(300.0),
                fan_power_w_per_cfm: None,
                airflow_m3_s_per_w: Some(4.0e-5),
                heating_setpoint_c: Some(21.0),
                cooling_setpoint_c: Some(26.0),
                hysteresis_c: Some(1.0),
                heating_setpoint_source: None,
                cooling_setpoint_source: None,
                duct: DuctConfig::default(),
                biquadratic_x1_min: Some(12.0),
                biquadratic_x1_max: Some(24.0),
                biquadratic_x2_min: Some(18.0),
                biquadratic_x2_max: Some(46.0),
                ff_min: Some(0.6),
                ff_max: Some(1.2),
                plf_min: Some(0.7),
                plf_max: Some(1.0),
                min_compressor_fraction: 0.25,
                eir_part_load_benefit: None,
                er_stages: 1,
                charge_defect_ratio: None,
                ..Default::default()
            },
            hp_lockout_temp_c: Some(-17.8),
            er_lockout_temp_c: Some(4.4),
            max_oat_supplemental_c: Some(21.0),
            er_setpoint_offset_c: Some(1.6),
            er_hard_lockout_time_s: Some(600.0),
            heating_shr: None,
            capacity_ratio_at_17f: None,
            defrost: DefrostConfig::default(),
        };
        let value = serde_json::to_value(&cfg).unwrap();
        let recovered: HeatPumpHeaterConfig = serde_json::from_value(value.clone()).unwrap();
        let reserialized = serde_json::to_value(&recovered).unwrap();
        assert_eq!(value, reserialized);
    }

    #[test]
    fn heat_pump_cooler_config_serde_json_round_trip() {
        let cfg = HeatPumpCoolerConfig {
            common: HeatPumpCommonConfig {
                equipment_id: Some(2),
                zone_id: Some(1),
                heating_capacity_w: Some(9_000.0),
                heating_eir: Some(8.8),
                backup_capacity_w: Some(5_000.0),
                backup_eir: Some(1.0),
                fraction_heating_load_served: Some(1.0),
                cooling_capacity_w: Some(12_000.0),
                cooling_eir: Some(16.0),
                stage_cooling_capacities_w: Some(vec![6_000.0, 12_000.0]),
                stage_cooling_eirs: Some(vec![0.25, 0.22]),
                fraction_cooling_load_served: Some(1.0),
                number_of_speeds: 2,
                shr: Some(0.75),
                fan_power_w: Some(300.0),
                airflow_m3_s_per_w: Some(4.0e-5),
                heating_setpoint_c: Some(21.0),
                cooling_setpoint_c: Some(26.0),
                hysteresis_c: Some(1.0),
                duct: DuctConfig::default(),
                biquadratic_x1_min: Some(12.0),
                biquadratic_x1_max: Some(24.0),
                biquadratic_x2_min: Some(18.0),
                biquadratic_x2_max: Some(46.0),
                ff_min: Some(0.6),
                ff_max: Some(1.2),
                plf_min: Some(0.7),
                plf_max: Some(1.0),
                ..HeatPumpCommonConfig::default()
            },
            stage_shrs: Some(vec![0.78, 0.72]),
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
        };
        let value = serde_json::to_value(&cfg).unwrap();
        let recovered: HeatPumpCoolerConfig = serde_json::from_value(value.clone()).unwrap();
        let reserialized = serde_json::to_value(&recovered).unwrap();
        assert_eq!(value, reserialized);
    }

    // --- Regression tests for ticket 009 ---

    #[test]
    fn number_of_speeds_defaults_to_one_heater() {
        let json = serde_json::json!({ "cooling_capacity_w": 10_000.0, "cooling_eir": 16.0 });
        let cfg: HeatPumpHeaterConfig = serde_json::from_value(json).unwrap();
        assert_eq!(
            cfg.common.number_of_speeds, 1,
            "default_one() must return 1 for number_of_speeds"
        );
    }

    #[test]
    fn number_of_speeds_defaults_to_one_cooler() {
        let json = serde_json::json!({ "cooling_capacity_w": 10_000.0, "cooling_eir": 16.0 });
        let cfg: HeatPumpCoolerConfig = serde_json::from_value(json).unwrap();
        assert_eq!(
            cfg.common.number_of_speeds, 1,
            "default_one() must return 1 for number_of_speeds (cooler)"
        );
    }

    /// serde flatten is incompatible with `deny_unknown_fields` on the outer struct,
    /// so cross-type fields like `stage_shrs` are silently ignored by
    /// `HeatPumpHeaterConfig` deserialization rather than rejected.
    /// See <https://serde.rs/attr-flatten.html>.
    #[test]
    fn heater_ignores_cooler_only_field_stage_shrs() {
        let json = serde_json::json!({
            "cooling_capacity_w": 10_000.0,
            "cooling_eir": 16.0,
            "stage_shrs": [0.75, 0.72],
        });
        let cfg: HeatPumpHeaterConfig = serde_json::from_value(json).unwrap();
        assert!(
            cfg.hp_lockout_temp_c.is_none(),
            "stage_shrs is silently dropped; heater-only fields remain None"
        );
    }

    /// Same constraint as above: heater-only field `hp_lockout_temp_c` is
    /// silently ignored by `HeatPumpCoolerConfig` deserialization.
    #[test]
    fn cooler_ignores_heater_only_field_hp_lockout() {
        let json = serde_json::json!({
            "cooling_capacity_w": 10_000.0,
            "cooling_eir": 16.0,
            "hp_lockout_temp_c": -17.8,
        });
        let cfg: HeatPumpCoolerConfig = serde_json::from_value(json).unwrap();
        assert!(
            cfg.stage_shrs.is_none(),
            "hp_lockout_temp_c is silently dropped; cooler-only fields remain None"
        );
    }

    #[test]
    fn heat_pump_equipment_type_name_literals() {
        assert_eq!(HeatPumpHeaterConfig::equipment_type_name(), "ASHP Heater",);
        assert_eq!(HeatPumpCoolerConfig::equipment_type_name(), "ASHP Cooler",);
    }

    #[test]
    fn heat_pump_cooler_validate_rejects_min_compressor_fraction_out_of_range() {
        let mut cfg_below = HeatPumpCoolerConfig {
            common: HeatPumpCommonConfig {
                cooling_capacity_w: Some(12_000.0),
                cooling_eir: Some(0.25),
                is_mini_split: true,
                min_compressor_fraction: 0.05,
                ..HeatPumpCommonConfig::default()
            },
            stage_shrs: None,
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
        };
        let err = cfg_below.validate().unwrap_err();
        assert!(
            err.to_string()
                .contains("min_compressor_fraction must be in [0.1, 0.5]"),
            "cooler validation must reject min_compressor_fraction < 0.1; got: {err}",
        );

        cfg_below.common.min_compressor_fraction = 0.55;
        let err = cfg_below.validate().unwrap_err();
        assert!(
            err.to_string()
                .contains("min_compressor_fraction must be in [0.1, 0.5]"),
            "cooler validation must reject min_compressor_fraction > 0.5; got: {err}",
        );
    }

    #[test]
    fn min_compressor_fraction_defaults_to_25_percent() {
        let json = serde_json::json!({
            "heating_capacity_w": 10_000.0,
            "heating_eir": 0.25,
        });
        let cfg: HeatPumpHeaterConfig = serde_json::from_value(json).unwrap();
        assert!(
            (cfg.common.min_compressor_fraction - 0.25).abs() < 1e-9,
            "default min_compressor_fraction must be 0.25; got {}",
            cfg.common.min_compressor_fraction,
        );
    }

    #[test]
    fn eir_part_load_benefit_defaults_to_none() {
        let json = serde_json::json!({
            "heating_capacity_w": 10_000.0,
            "heating_eir": 0.25,
        });
        let cfg: HeatPumpHeaterConfig = serde_json::from_value(json).unwrap();
        assert!(
            cfg.common.eir_part_load_benefit.is_none(),
            "default eir_part_load_benefit must be None; got {:?}",
            cfg.common.eir_part_load_benefit,
        );
    }
}
