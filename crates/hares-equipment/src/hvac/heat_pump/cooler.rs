//! Heat-pump cooler variants.

use std::borrow::Cow;
use std::time::Duration;

use hares_physics::constants::KW_TO_W;
use hares_physics::ground::SourceTemperature;
use hares_physics::units::power_kw_to_w;
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreOutput, EndUse, EnvironmentState,
    EquipmentDescriptor, EquipmentId, ExecutionStage, FuelType, HaresError, OperatingMode,
    PortContribution, PortDeclaration, PortSlots, Telemetry,
};

use hares_types::telemetry_keys as tk;

use crate::{Equipment, EquipmentConfig};

use super::super::ac_config::{CentralAirConditionerConfig, HeatPumpCoolerConfig};
use super::super::air_conditioner::AirConditioner;
use super::super::heating_config::HvacSetpointConfig;
use super::super::helpers::{equipment_id_from_config, zone_id_from_config_or_default};
use super::constants::DEFAULT_EQUIPMENT_ID;

/// MSHP crankcase heater: 15 W rated, activates at or below 0 °C.
/// Distinct from central AC default (50 W / 12.8 °C) per OCHRE conventions.
const MSHP_CRANKCASE_HEATER_KW: f64 = 0.015;
const MSHP_CRANKCASE_HEATER_THRESHOLD_C: f64 = 0.0;

pub struct HpCooler {
    inner: AirConditioner,
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    is_mshp: bool,
    /// RTF of the companion heating coil from the previous step.
    /// Set by the system coordinator after the heater step so that the cooler
    /// can compute crankcase power using `max(cooling_rtf, heating_rtf)`.
    companion_heating_rtf: Option<f64>,
    /// Whether zone_id was explicitly set in config or fell back to ZoneId(1).
    zone_id_explicit: bool,
}

impl HpCooler {
    fn build(config: EquipmentConfig, equipment_type: &'static str, is_mshp: bool) -> Self {
        let mut inner = AirConditioner::new(config.clone());
        if is_mshp {
            let mshp_type = crate::hvac::hvac_core::HvacEquipmentType::MiniSplitCool;
            inner.core.hvac.config.equipment_type = mshp_type;
            inner.core.hvac.config.airflow_m3_s_per_w = mshp_type.default_airflow_m3_s_per_w();
            // Apply MSHP crankcase defaults at construction so that step() uses
            // correct values (15 W / 0 °C) even if init() has not been called yet.
            inner.set_crankcase_defaults_if_unconfigured(
                &config,
                MSHP_CRANKCASE_HEATER_KW,
                MSHP_CRANKCASE_HEATER_THRESHOLD_C,
            );
        } else {
            let ashp_cool_type = crate::hvac::hvac_core::HvacEquipmentType::AshpHeatPumpCooling;
            inner.core.hvac.config.equipment_type = ashp_cool_type;
            inner.core.hvac.config.airflow_m3_s_per_w = ashp_cool_type.default_airflow_m3_s_per_w();
        }
        let (zone, zone_id_explicit) = zone_id_from_config_or_default(&config, &config.name);
        Self {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(equipment_id_from_config(&config).unwrap_or(DEFAULT_EQUIPMENT_ID)),
                name: config.name,
                end_use: EndUse::HVAC_COOLING,
                equipment_type: Cow::Borrowed(equipment_type),
                zone: Some(zone),
                fuel: FuelType::Electric,
                stage: ExecutionStage::Thermal,
                control_capabilities: ControlCapabilities::THERMAL_SETPOINT
                    | ControlCapabilities::THERMAL_SETPOINT_DELTA
                    | ControlCapabilities::DUTY_CYCLE
                    | ControlCapabilities::LOAD_FRACTION
                    | ControlCapabilities::POWER_LIMIT
                    | ControlCapabilities::MODE_OVERRIDE
                    | ControlCapabilities::DEMAND_RESPONSE
                    | ControlCapabilities::IDEAL_CAPACITY,
                core_capabilities: CoreCapabilities::ELECTRIC
                    | CoreCapabilities::HAS_MODE
                    | CoreCapabilities::THERMAL
                    | CoreCapabilities::HAS_SPEED
                    | CoreCapabilities::HAS_SETPOINT
                    | CoreCapabilities::HAS_COP
                    | CoreCapabilities::REACTIVE,
                telemetry_fields: inner.descriptor().telemetry_fields.clone(),
                zone_type: None,
            },
            ports: inner.ports().to_vec(),
            inner,
            is_mshp,
            companion_heating_rtf: None,
            zone_id_explicit,
        }
    }

    #[must_use]
    pub fn ashp_cooler(config: EquipmentConfig) -> Self {
        Self::build(config, "ASHP Cooler", false)
    }

    #[must_use]
    pub fn mshp_cooler(config: EquipmentConfig) -> Self {
        Self::build(config, "MSHP Cooler", true)
    }

    /// Provide the companion heating coil's RTF from the most recent step.
    ///
    /// Call this after the heater's `step()` completes and before calling this
    /// cooler's `step()`, so that crankcase heater power accounts for HP heating
    /// mode operation. Pass `None` to clear the companion RTF (standalone AC mode).
    pub fn set_companion_heating_rtf(&mut self, rtf: Option<f64>) {
        self.companion_heating_rtf = rtf;
    }

    /// Return the cooling coil's runtime fraction from the most recent step.
    ///
    /// Used by the system coordinator to pass to the companion heater side.
    pub fn last_cooling_rtf(&self) -> f64 {
        self.inner.core.last_cooling_rtf
    }

    fn typed_hp_to_central_ac_config(
        source: &EquipmentConfig,
        hp_cfg: &HeatPumpCoolerConfig,
    ) -> crate::Result<EquipmentConfig> {
        let eir = hp_cfg
            .common
            .cooling_eir
            .or_else(|| {
                hp_cfg
                    .common
                    .stage_cooling_eirs
                    .as_ref()
                    .and_then(|eirs| eirs.first().copied())
            })
            .ok_or_else(|| {
                HaresError::Equipment(
                    "HeatPumpCoolerConfig requires cooling_eir or stage_cooling_eirs".to_string(),
                )
            })?;

        let capacity_w = hp_cfg
            .common
            .cooling_capacity_w
            .or_else(|| {
                hp_cfg
                    .common
                    .stage_cooling_capacities_w
                    .as_ref()
                    .and_then(|caps| caps.last().copied())
            })
            .unwrap_or(8_000.0);

        let mapped = CentralAirConditionerConfig {
            equipment_id: hp_cfg.common.equipment_id,
            zone_id: hp_cfg.common.zone_id,
            capacity_w,
            eir,
            shr: hp_cfg.common.shr,
            number_of_speeds: hp_cfg.effective_number_of_speeds(),
            stage_capacities_w: hp_cfg.common.stage_cooling_capacities_w.clone(),
            stage_eirs: hp_cfg.common.stage_cooling_eirs.clone(),
            stage_shrs: hp_cfg.stage_shrs.clone(),
            fan_power_w: hp_cfg.common.fan_power_w,
            fan_power_w_per_cfm: hp_cfg.common.fan_power_w_per_cfm,
            setpoint: HvacSetpointConfig {
                heating_setpoint_c: hp_cfg.common.setpoint.heating_setpoint_c,
                heating_setpoint_source: hp_cfg.common.setpoint.heating_setpoint_source.clone(),
                cooling_setpoint_c: hp_cfg.common.setpoint.cooling_setpoint_c,
                cooling_setpoint_source: hp_cfg.common.setpoint.cooling_setpoint_source.clone(),
            },
            hysteresis_c: hp_cfg.common.hysteresis_c,
            airflow_m3_s_per_w: hp_cfg.common.airflow_m3_s_per_w,
            fraction_load_served: hp_cfg.common.fraction_cooling_load_served,
            crankcase_heater_kw: hp_cfg.crankcase_heater_kw,
            crankcase_heater_threshold_c: hp_cfg.crankcase_heater_threshold_c,
            crankcase_capacity_curve_coeffs: None,
            duct: hp_cfg.common.duct.clone(),
            system_type: hp_cfg
                .common
                .is_mini_split
                .then(|| "mini-split".to_string()),
            startup_cd: hp_cfg.derived_cooling_startup_cd(),
            biquadratic_x1_min: hp_cfg.common.biquadratic_x1_min,
            biquadratic_x1_max: hp_cfg.common.biquadratic_x1_max,
            biquadratic_x2_min: hp_cfg.common.biquadratic_x2_min,
            biquadratic_x2_max: hp_cfg.common.biquadratic_x2_max,
            ff_min: hp_cfg.common.ff_min,
            ff_max: hp_cfg.common.ff_max,
            plf_min: hp_cfg.common.plf_min,
            plf_max: hp_cfg.common.plf_max,
            charge_defect_ratio: hp_cfg.common.charge_defect_ratio,
            // Minimum OAT for cooling compressor operation [°C].
            // Below this temperature the compressor is locked out so that an
            // economizer can provide free cooling. Default 10.0 °C per ASHRAE
            // 90.1-2022 §6.5.1.4 economizer changeover guidance.
            // EnergyPlus `UnitarySystem.cc:12778`: `OutsideDryBulbTemp > m_MinOATCompressorCooling`.
            min_oat_compressor_cooling_c: Some(hp_cfg.min_oat_cooling_c),
        };

        let mut cfg = EquipmentConfig::from_typed(
            source.name.clone(),
            "Air Conditioner".to_string(),
            mapped,
        )?;
        // Propagate the instance ZIP/power-factor sidecar so overrides on the
        // original HP cooler config (e.g. a pf-0 sentinel for Rule R1 twin
        // tests, or a user "zip" override) reach the inner AirConditioner that
        // resolves its ZIP through the mapped "Air Conditioner" config.
        cfg.zip = source.zip;
        Ok(cfg)
    }
}

impl Equipment for HpCooler {
    fn descriptor(&self) -> &hares_types::EquipmentDescriptor {
        &self.descriptor
    }

    fn rename(&mut self, name: String) {
        self.descriptor.name = name;
    }

    fn zone_id_explicit(&self) -> bool {
        self.zone_id_explicit
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        let typed_hp_cfg = config.require_typed::<HeatPumpCoolerConfig>("Heat Pump Cooler")?;
        typed_hp_cfg.validate()?;
        let mapped = Self::typed_hp_to_central_ac_config(config, &typed_hp_cfg)?;
        self.inner.init(&mapped, env)?;
        let n_speeds = self.inner.core.hvac.config.cooling_capacities_w.len();
        if let Some(shrs) = &typed_hp_cfg.stage_shrs {
            if !shrs.is_empty() && shrs.len() != n_speeds {
                return Err(HaresError::Equipment(format!(
                    "stage_shrs length {} does not match speed stage count {}",
                    shrs.len(),
                    n_speeds
                )));
            }
            self.inner.core.stage_shrs = shrs.clone();
        }

        if self.is_mshp {
            // MSHP crankcase heater: 15 W / 0 °C, overriding central AC defaults
            // (50 W / 12.8 °C) unless the user explicitly configured them.
            self.inner.set_crankcase_defaults_if_unconfigured(
                config,
                MSHP_CRANKCASE_HEATER_KW,
                MSHP_CRANKCASE_HEATER_THRESHOLD_C,
            );
        }
        // Re-sync ports after inner init may have added duct/basement zone thermals.
        self.ports = self.inner.ports().to_vec();
        Ok(())
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        self.inner.update_control(env)
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        // Pass companion_heating_rtf so crankcase uses max(cooling_rtf, heating_rtf)
        // when this cooler is part of an HP system.
        self.inner
            .core
            .step(env, dt, ports, self.companion_heating_rtf)
    }

    fn telemetry(&self) -> &Telemetry {
        self.inner.telemetry()
    }

    fn core_output(&self) -> &CoreOutput {
        self.inner.core_output()
    }

    fn resolved_zip(&self) -> Option<hares_types::zip::ZipLoad> {
        // Primary component: the compressor ZIP resolved by the inner
        // AirConditioner (the user "zip" sidecar is propagated to it).
        self.inner.resolved_zip()
    }

    fn save_state(&self) -> crate::Result<Vec<u8>> {
        self.inner.save_state()
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        self.inner.load_state(state)
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        self.inner.apply_control_unchecked(signal)
    }

    fn ideal_target(&self) -> Option<(hares_types::ZoneId, f64)> {
        self.inner.ideal_target()
    }
}

// ---------------------------------------------------------------------------
// Ground-source heat pump cooler
// ---------------------------------------------------------------------------

pub struct GshpCooler {
    inner: AirConditioner,
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    core_output: CoreOutput,
    pump_loop_depth_m: f64,
    pump_pipe_diameter_m: f64,
    pump_flow_rate_m3_per_s: f64,
    pump_efficiency: f64,
    pump_motor_efficiency: f64,
    pump_system_head_loss_m: f64,
    /// Rule R1 reactive-only ZIP for the ground-loop pump contribution:
    /// the pump motor component (pf 0.84, `hvac::reactive::LOOP_PUMP_ZIP`),
    /// honouring the unit-level pf=0 sentinel. The compressor/fan/crankcase
    /// reactive comes from the inner [`AirConditioner`].
    pump_zip: hares_types::zip::ZipLoad,
    /// Whether zone_id was explicitly set in config or fell back to ZoneId(1).
    zone_id_explicit: bool,
}

impl GshpCooler {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let mut inner = AirConditioner::new(config.clone());
        let gshp_type = crate::hvac::hvac_core::HvacEquipmentType::GshpHeatPumpCooling;
        inner.core.hvac.config.equipment_type = gshp_type;
        inner.core.hvac.config.airflow_m3_s_per_w = gshp_type.default_airflow_m3_s_per_w();
        // GSHP: compressor is indoors; no crankcase heater needed.
        inner.set_crankcase_defaults_if_unconfigured(&config, 0.0, f64::NEG_INFINITY);
        // Source temperature: use transient borehole g-function model.
        // Default borehole config: 60 m depth, 2.0 W/m·K soil, 0.05 m²/day diffusivity.
        inner.core.source_temp = SourceTemperature::BoreholeGFunction {
            model: std::sync::Arc::new(hares_physics::borehole::BoreholeGFunctionModel::new(
                hares_physics::borehole::BoreholeConfig::default(),
            )),
        };
        let (zone, zone_id_explicit) = zone_id_from_config_or_default(&config, &config.name);
        Self {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(equipment_id_from_config(&config).unwrap_or(DEFAULT_EQUIPMENT_ID)),
                name: config.name,
                end_use: EndUse::HVAC_COOLING,
                equipment_type: Cow::Borrowed("GSHP Cooler"),
                zone: Some(zone),
                fuel: FuelType::Electric,
                stage: ExecutionStage::Thermal,
                control_capabilities: ControlCapabilities::THERMAL_SETPOINT
                    | ControlCapabilities::THERMAL_SETPOINT_DELTA
                    | ControlCapabilities::DUTY_CYCLE
                    | ControlCapabilities::LOAD_FRACTION
                    | ControlCapabilities::POWER_LIMIT
                    | ControlCapabilities::MODE_OVERRIDE
                    | ControlCapabilities::DEMAND_RESPONSE
                    | ControlCapabilities::IDEAL_CAPACITY,
                core_capabilities: CoreCapabilities::ELECTRIC
                    | CoreCapabilities::HAS_MODE
                    | CoreCapabilities::THERMAL
                    | CoreCapabilities::HAS_SPEED
                    | CoreCapabilities::HAS_SETPOINT
                    | CoreCapabilities::HAS_COP
                    | CoreCapabilities::REACTIVE,
                telemetry_fields: inner.descriptor().telemetry_fields.clone(),
                zone_type: None,
            },
            ports: inner.ports().to_vec(),
            inner,
            core_output: CoreOutput::default(),
            pump_loop_depth_m: 60.0,
            pump_pipe_diameter_m: 0.025,
            pump_flow_rate_m3_per_s: 0.00019,
            pump_efficiency: 0.35,
            pump_motor_efficiency: 0.40,
            pump_system_head_loss_m: 3.0,
            pump_zip: hares_types::zip::ZipLoad::constant_power(),
            zone_id_explicit,
        }
    }

    fn typed_gshp_to_central_ac_config(
        source: &EquipmentConfig,
        hp_cfg: &HeatPumpCoolerConfig,
    ) -> crate::Result<EquipmentConfig> {
        let eir = hp_cfg
            .common
            .cooling_eir
            .or_else(|| {
                hp_cfg
                    .common
                    .stage_cooling_eirs
                    .as_ref()
                    .and_then(|eirs| eirs.first().copied())
            })
            .ok_or_else(|| {
                HaresError::Equipment(
                    "HeatPumpCoolerConfig requires cooling_eir or stage_cooling_eirs".to_string(),
                )
            })?;

        let capacity_w = hp_cfg
            .common
            .cooling_capacity_w
            .or_else(|| {
                hp_cfg
                    .common
                    .stage_cooling_capacities_w
                    .as_ref()
                    .and_then(|caps| caps.last().copied())
            })
            .unwrap_or(8_000.0);

        let mapped = CentralAirConditionerConfig {
            equipment_id: hp_cfg.common.equipment_id,
            zone_id: hp_cfg.common.zone_id,
            capacity_w,
            eir,
            shr: hp_cfg.common.shr,
            number_of_speeds: hp_cfg.effective_number_of_speeds(),
            stage_capacities_w: hp_cfg.common.stage_cooling_capacities_w.clone(),
            stage_eirs: hp_cfg.common.stage_cooling_eirs.clone(),
            stage_shrs: hp_cfg.stage_shrs.clone(),
            fan_power_w: hp_cfg.common.fan_power_w,
            fan_power_w_per_cfm: hp_cfg.common.fan_power_w_per_cfm,
            setpoint: HvacSetpointConfig {
                heating_setpoint_c: hp_cfg.common.setpoint.heating_setpoint_c,
                heating_setpoint_source: hp_cfg.common.setpoint.heating_setpoint_source.clone(),
                cooling_setpoint_c: hp_cfg.common.setpoint.cooling_setpoint_c,
                cooling_setpoint_source: hp_cfg.common.setpoint.cooling_setpoint_source.clone(),
            },
            hysteresis_c: hp_cfg.common.hysteresis_c,
            airflow_m3_s_per_w: hp_cfg.common.airflow_m3_s_per_w,
            fraction_load_served: hp_cfg.common.fraction_cooling_load_served,
            // Crankcase values are overridden post-init (compressor indoors,
            // no crankcase heater needed). Not set here to avoid redundancy.
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            duct: hp_cfg.common.duct.clone(),
            system_type: None,
            startup_cd: hp_cfg.derived_cooling_startup_cd(),
            biquadratic_x1_min: hp_cfg.common.biquadratic_x1_min,
            biquadratic_x1_max: hp_cfg.common.biquadratic_x1_max,
            biquadratic_x2_min: hp_cfg.common.biquadratic_x2_min,
            biquadratic_x2_max: hp_cfg.common.biquadratic_x2_max,
            ff_min: hp_cfg.common.ff_min,
            ff_max: hp_cfg.common.ff_max,
            plf_min: hp_cfg.common.plf_min,
            plf_max: hp_cfg.common.plf_max,
            charge_defect_ratio: hp_cfg.common.charge_defect_ratio,
            // GSHP: compressor is indoors on a ground loop; OAT lockout does not apply.
            min_oat_compressor_cooling_c: None,
        };

        let mut cfg = EquipmentConfig::from_typed(
            source.name.clone(),
            "Air Conditioner".to_string(),
            mapped,
        )?;
        // Propagate the instance ZIP/power-factor sidecar so overrides on the
        // original HP cooler config (e.g. a pf-0 sentinel for Rule R1 twin
        // tests, or a user "zip" override) reach the inner AirConditioner that
        // resolves its ZIP through the mapped "Air Conditioner" config.
        cfg.zip = source.zip;
        Ok(cfg)
    }

    /// Return the cooling coil's runtime fraction from the most recent step.
    pub fn last_cooling_rtf(&self) -> f64 {
        self.inner.core.last_cooling_rtf
    }
}

impl Equipment for GshpCooler {
    fn descriptor(&self) -> &hares_types::EquipmentDescriptor {
        &self.descriptor
    }

    fn rename(&mut self, name: String) {
        self.descriptor.name = name;
    }

    fn zone_id_explicit(&self) -> bool {
        self.zone_id_explicit
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        let typed_hp_cfg = config.require_typed::<HeatPumpCoolerConfig>("GSHP Cooler")?;
        typed_hp_cfg.validate()?;
        let mapped = Self::typed_gshp_to_central_ac_config(config, &typed_hp_cfg)?;
        self.inner.init(&mapped, env)?;
        // GSHP: compressor is indoors; no crankcase heater needed.
        // Override post-init because AirConditioner::init_from_typed unconditionally
        // writes the central-AC default (0.05 kW / 12.8°C) when crankcase_heater_kw
        // is None in the mapped config.
        self.inner.core.crankcase_rated_kw = 0.0;
        self.inner.core.crankcase_threshold_c = f64::NEG_INFINITY;

        // Ground-loop circulation pump parameters from typed config.
        self.pump_loop_depth_m = typed_hp_cfg
            .common
            .pump_loop_depth_m
            .unwrap_or(self.pump_loop_depth_m);
        self.pump_pipe_diameter_m = typed_hp_cfg
            .common
            .pump_pipe_diameter_m
            .unwrap_or(self.pump_pipe_diameter_m);
        self.pump_flow_rate_m3_per_s = typed_hp_cfg
            .common
            .pump_flow_rate_m3_per_s
            .unwrap_or(self.pump_flow_rate_m3_per_s);
        self.pump_efficiency = typed_hp_cfg
            .common
            .pump_efficiency
            .unwrap_or(self.pump_efficiency);
        self.pump_motor_efficiency = typed_hp_cfg
            .common
            .pump_motor_efficiency
            .unwrap_or(self.pump_motor_efficiency);
        self.pump_system_head_loss_m = typed_hp_cfg
            .common
            .pump_system_head_loss_m
            .unwrap_or(self.pump_system_head_loss_m);

        // Build BoreholeConfig from typed config, falling back to documented
        // defaults for any fields left at None. This replaces the default
        // BoreholeGFunctionModel constructed in new().
        let bh_dfl = hares_physics::borehole::BoreholeConfig::default();
        let bh_cfg = hares_physics::borehole::BoreholeConfig {
            borehole_depth_m: typed_hp_cfg
                .common
                .borehole_depth_m
                .unwrap_or(bh_dfl.borehole_depth_m),
            borehole_radius_m: typed_hp_cfg
                .common
                .borehole_radius_m
                .unwrap_or(bh_dfl.borehole_radius_m),
            shank_spacing_m: typed_hp_cfg
                .common
                .borehole_shank_spacing_m
                .unwrap_or(bh_dfl.shank_spacing_m),
            number_of_boreholes: typed_hp_cfg
                .common
                .number_of_boreholes
                .unwrap_or(bh_dfl.number_of_boreholes),
            soil_conductivity_w_per_m_k: typed_hp_cfg
                .common
                .borehole_soil_conductivity_w_per_m_k
                .unwrap_or(bh_dfl.soil_conductivity_w_per_m_k),
            soil_diffusivity_m2_per_day: typed_hp_cfg
                .common
                .borehole_soil_diffusivity_m2_per_day
                .unwrap_or(bh_dfl.soil_diffusivity_m2_per_day),
            grout_conductivity_w_per_m_k: typed_hp_cfg
                .common
                .borehole_grout_conductivity_w_per_m_k
                .unwrap_or(bh_dfl.grout_conductivity_w_per_m_k),
            pipe_outer_radius_m: typed_hp_cfg
                .common
                .borehole_pipe_outer_radius_m
                .unwrap_or(bh_dfl.pipe_outer_radius_m),
            pipe_inner_radius_m: typed_hp_cfg
                .common
                .borehole_pipe_inner_radius_m
                .unwrap_or(bh_dfl.pipe_inner_radius_m),
            pipe_conductivity_w_per_m_k: typed_hp_cfg
                .common
                .borehole_pipe_conductivity_w_per_m_k
                .unwrap_or(bh_dfl.pipe_conductivity_w_per_m_k),
        };
        self.inner.core.source_temp = SourceTemperature::BoreholeGFunction {
            model: std::sync::Arc::new(hares_physics::borehole::BoreholeGFunctionModel::new(
                bh_cfg,
            )),
        };

        self.pump_zip = crate::hvac::reactive::secondary_motor_zip(
            &crate::config::resolve_reactive_zip(config)?,
            crate::hvac::reactive::LOOP_PUMP_ZIP,
        );
        Ok(())
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        self.inner.update_control(env)
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        self.inner.step(env, dt, ports)?;

        // Record borehole heat exchange for transient ground model.
        // Cooling mode: heat is rejected TO the ground (positive Q in
        // Eskilson's convention). Q_condenser = cooling_output + compressor_power.
        let dt_s = dt.as_secs_f64();
        let tel = self.inner.telemetry();
        let coil_sens_w = tel.get(tk::COIL_SENSIBLE_COOLING_W).unwrap_or(0.0);
        let coil_lat_w = tel.get(tk::COIL_LATENT_COOLING_W).unwrap_or(0.0);
        // Gross compressor power (pre-space_fraction) to match the gross coil
        // values above; COMPRESSOR_KW telemetry is scaled by space_fraction.
        let compressor_kw = self.inner.core.last_compressor_kw;
        let borehole_heat_w = coil_sens_w + coil_lat_w + compressor_kw * KW_TO_W;
        self.inner
            .core
            .source_temp
            .record_source_heat_rate(borehole_heat_w, dt_s);

        // Ground-loop circulation pump: runs whenever the GSHP compressor is
        // active. `last_cooling_rtf > 0` means the compressor was on this step.
        // space_fraction scales the pump draw exactly like the inner unit's
        // compressor/fan/crankcase (the GSHP heater likewise applies
        // space_fraction to its combined electric_kw, pump included).
        let compressor_ran = self.inner.core.last_cooling_rtf > 0.0;
        let sf = self.inner.core.hvac.config.space_fraction;
        let pump_kw = if compressor_ran {
            hares_physics::pump::compute_ground_loop_pump_power_kw(
                self.pump_loop_depth_m,
                self.pump_pipe_diameter_m,
                self.pump_flow_rate_m3_per_s,
                self.pump_efficiency,
                self.pump_motor_efficiency,
                self.pump_system_head_loss_m,
            ) * sf
        } else {
            0.0
        };

        // Per-component reactive (see `hvac::reactive`): the ground-loop pump
        // is a circulation pump motor (pf 0.84), not a cooling compressor.
        // The inner AirConditioner already pushed the compressor/fan/crankcase
        // reactive and reported it in its CoreOutput; the pump contribution is
        // added on top so the port and CoreOutput stay consistent.
        let pump_q = self
            .pump_zip
            .reactive_kvar(pump_kw, env.grid.bus_voltage_pu());
        if pump_kw > 0.0 || pump_q != 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_w: power_kw_to_w(pump_kw),
                reactive_power_kvar: pump_q,
            })?;
        }

        // Publish pump power in telemetry.
        self.inner.set_telemetry(tk::PUMP_POWER_KW, pump_kw);

        // Rebuild the CoreOutput so it includes the pump's active and reactive
        // contribution (the inner CoreOutput covers only compressor + fan +
        // crankcase). This keeps port and CoreOutput consistent — the
        // debug-build validator would otherwise flag the pump delta.
        let inner_co = self.inner.core_output().clone();
        let inner_kw = match inner_co.flows.electric_kw {
            Some(hares_types::ElectricPower::Consumption(kw)) => kw,
            Some(hares_types::ElectricPower::Bidirectional(kw)) => kw,
            _ => 0.0,
        };
        let inner_q = inner_co.flows.reactive_power_kvar.unwrap_or(0.0);
        let total_kw = inner_kw + pump_kw;
        let total_q = inner_q + pump_q;
        // Fold the pump into the unit-total telemetry so port, CoreOutput, and
        // telemetry agree on both active and reactive power.
        self.inner.set_telemetry(tk::ELECTRIC_KW, total_kw);
        self.inner.set_telemetry(tk::REACTIVE_POWER_KVAR, total_q);
        let mut combined = inner_co;
        combined.flows.electric_kw =
            Some(hares_types::ElectricPower::Consumption(total_kw.max(0.0)));
        combined.flows.reactive_power_kvar = Some(total_q);
        self.core_output = combined;

        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        self.inner.telemetry()
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core_output
    }

    fn resolved_zip(&self) -> Option<hares_types::zip::ZipLoad> {
        // Primary component: the compressor ZIP resolved by the inner
        // AirConditioner (the user "zip" sidecar is propagated to it); the
        // loop-pump component ZIP is secondary.
        self.inner.resolved_zip()
    }

    fn save_state(&self) -> crate::Result<Vec<u8>> {
        self.inner.save_state()
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        self.inner.load_state(state)
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        self.inner.apply_control_unchecked(signal)
    }

    fn ideal_target(&self) -> Option<(hares_types::ZoneId, f64)> {
        self.inner.ideal_target()
    }
}

// ---------------------------------------------------------------------------
// Water-source heat pump cooler
// ---------------------------------------------------------------------------

pub struct WshpCooler {
    inner: AirConditioner,
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    core_output: CoreOutput,
    pump_loop_depth_m: f64,
    pump_pipe_diameter_m: f64,
    pump_flow_rate_m3_per_s: f64,
    pump_efficiency: f64,
    pump_motor_efficiency: f64,
    pump_system_head_loss_m: f64,
    /// Rule R1 reactive-only ZIP for the water-loop pump contribution:
    /// the pump motor component (pf 0.84, `hvac::reactive::LOOP_PUMP_ZIP`),
    /// honouring the unit-level pf=0 sentinel. The compressor/fan/crankcase
    /// reactive comes from the inner [`AirConditioner`].
    pump_zip: hares_types::zip::ZipLoad,
    /// Whether zone_id was explicitly set in config or fell back to ZoneId(1).
    zone_id_explicit: bool,
}

impl WshpCooler {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let mut inner = AirConditioner::new(config.clone());
        let wshp_type = crate::hvac::hvac_core::HvacEquipmentType::WshpHeatPumpCooling;
        inner.core.hvac.config.equipment_type = wshp_type;
        inner.core.hvac.config.airflow_m3_s_per_w = wshp_type.default_airflow_m3_s_per_w();
        // WSHP: compressor is indoors; no crankcase heater needed.
        inner.set_crankcase_defaults_if_unconfigured(&config, 0.0, f64::NEG_INFINITY);
        // Source temperature: constant entering water temperature.
        inner.core.source_temp = SourceTemperature::Constant(10.0);
        let (zone, zone_id_explicit) = zone_id_from_config_or_default(&config, &config.name);
        Self {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(equipment_id_from_config(&config).unwrap_or(DEFAULT_EQUIPMENT_ID)),
                name: config.name,
                end_use: EndUse::HVAC_COOLING,
                equipment_type: Cow::Borrowed("WSHP Cooler"),
                zone: Some(zone),
                fuel: FuelType::Electric,
                stage: ExecutionStage::Thermal,
                control_capabilities: ControlCapabilities::THERMAL_SETPOINT
                    | ControlCapabilities::THERMAL_SETPOINT_DELTA
                    | ControlCapabilities::DUTY_CYCLE
                    | ControlCapabilities::LOAD_FRACTION
                    | ControlCapabilities::POWER_LIMIT
                    | ControlCapabilities::MODE_OVERRIDE
                    | ControlCapabilities::DEMAND_RESPONSE
                    | ControlCapabilities::IDEAL_CAPACITY,
                core_capabilities: CoreCapabilities::ELECTRIC
                    | CoreCapabilities::HAS_MODE
                    | CoreCapabilities::THERMAL
                    | CoreCapabilities::HAS_SPEED
                    | CoreCapabilities::HAS_SETPOINT
                    | CoreCapabilities::HAS_COP
                    | CoreCapabilities::REACTIVE,
                telemetry_fields: inner.descriptor().telemetry_fields.clone(),
                zone_type: None,
            },
            ports: inner.ports().to_vec(),
            inner,
            core_output: CoreOutput::default(),
            pump_loop_depth_m: 60.0,
            pump_pipe_diameter_m: 0.025,
            pump_flow_rate_m3_per_s: 0.00019,
            pump_efficiency: 0.35,
            pump_motor_efficiency: 0.40,
            pump_system_head_loss_m: 3.0,
            pump_zip: hares_types::zip::ZipLoad::constant_power(),
            zone_id_explicit,
        }
    }

    fn typed_wshp_to_central_ac_config(
        source: &EquipmentConfig,
        hp_cfg: &HeatPumpCoolerConfig,
    ) -> crate::Result<EquipmentConfig> {
        let eir = hp_cfg
            .common
            .cooling_eir
            .or_else(|| {
                hp_cfg
                    .common
                    .stage_cooling_eirs
                    .as_ref()
                    .and_then(|eirs| eirs.first().copied())
            })
            .ok_or_else(|| {
                HaresError::Equipment(
                    "HeatPumpCoolerConfig requires cooling_eir or stage_cooling_eirs".to_string(),
                )
            })?;

        let capacity_w = hp_cfg
            .common
            .cooling_capacity_w
            .or_else(|| {
                hp_cfg
                    .common
                    .stage_cooling_capacities_w
                    .as_ref()
                    .and_then(|caps| caps.last().copied())
            })
            .unwrap_or(8_000.0);

        let mapped = CentralAirConditionerConfig {
            equipment_id: hp_cfg.common.equipment_id,
            zone_id: hp_cfg.common.zone_id,
            capacity_w,
            eir,
            shr: hp_cfg.common.shr,
            number_of_speeds: hp_cfg.effective_number_of_speeds(),
            stage_capacities_w: hp_cfg.common.stage_cooling_capacities_w.clone(),
            stage_eirs: hp_cfg.common.stage_cooling_eirs.clone(),
            stage_shrs: hp_cfg.stage_shrs.clone(),
            fan_power_w: hp_cfg.common.fan_power_w,
            fan_power_w_per_cfm: hp_cfg.common.fan_power_w_per_cfm,
            setpoint: HvacSetpointConfig {
                heating_setpoint_c: hp_cfg.common.setpoint.heating_setpoint_c,
                heating_setpoint_source: hp_cfg.common.setpoint.heating_setpoint_source.clone(),
                cooling_setpoint_c: hp_cfg.common.setpoint.cooling_setpoint_c,
                cooling_setpoint_source: hp_cfg.common.setpoint.cooling_setpoint_source.clone(),
            },
            hysteresis_c: hp_cfg.common.hysteresis_c,
            airflow_m3_s_per_w: hp_cfg.common.airflow_m3_s_per_w,
            fraction_load_served: hp_cfg.common.fraction_cooling_load_served,
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            duct: hp_cfg.common.duct.clone(),
            system_type: None,
            startup_cd: hp_cfg.derived_cooling_startup_cd(),
            biquadratic_x1_min: hp_cfg.common.biquadratic_x1_min,
            biquadratic_x1_max: hp_cfg.common.biquadratic_x1_max,
            biquadratic_x2_min: hp_cfg.common.biquadratic_x2_min,
            biquadratic_x2_max: hp_cfg.common.biquadratic_x2_max,
            ff_min: hp_cfg.common.ff_min,
            ff_max: hp_cfg.common.ff_max,
            plf_min: hp_cfg.common.plf_min,
            plf_max: hp_cfg.common.plf_max,
            charge_defect_ratio: hp_cfg.common.charge_defect_ratio,
            // WSHP: compressor is indoors on a water loop; OAT lockout does not apply.
            min_oat_compressor_cooling_c: None,
        };

        let mut cfg = EquipmentConfig::from_typed(
            source.name.clone(),
            "Air Conditioner".to_string(),
            mapped,
        )?;
        // Propagate the instance ZIP/power-factor sidecar so overrides on the
        // original HP cooler config (e.g. a pf-0 sentinel for Rule R1 twin
        // tests, or a user "zip" override) reach the inner AirConditioner that
        // resolves its ZIP through the mapped "Air Conditioner" config.
        cfg.zip = source.zip;
        Ok(cfg)
    }

    pub fn last_cooling_rtf(&self) -> f64 {
        self.inner.core.last_cooling_rtf
    }
}

impl Equipment for WshpCooler {
    fn descriptor(&self) -> &hares_types::EquipmentDescriptor {
        &self.descriptor
    }

    fn rename(&mut self, name: String) {
        self.descriptor.name = name;
    }

    fn zone_id_explicit(&self) -> bool {
        self.zone_id_explicit
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        let typed_hp_cfg = config.require_typed::<HeatPumpCoolerConfig>("WSHP Cooler")?;
        typed_hp_cfg.validate()?;
        let mapped = Self::typed_wshp_to_central_ac_config(config, &typed_hp_cfg)?;
        self.inner.init(&mapped, env)?;
        // WSHP: compressor is indoors; no crankcase heater needed.
        self.inner.core.crankcase_rated_kw = 0.0;
        self.inner.core.crankcase_threshold_c = f64::NEG_INFINITY;

        self.pump_loop_depth_m = typed_hp_cfg
            .common
            .pump_loop_depth_m
            .unwrap_or(self.pump_loop_depth_m);
        self.pump_pipe_diameter_m = typed_hp_cfg
            .common
            .pump_pipe_diameter_m
            .unwrap_or(self.pump_pipe_diameter_m);
        self.pump_flow_rate_m3_per_s = typed_hp_cfg
            .common
            .pump_flow_rate_m3_per_s
            .unwrap_or(self.pump_flow_rate_m3_per_s);
        self.pump_efficiency = typed_hp_cfg
            .common
            .pump_efficiency
            .unwrap_or(self.pump_efficiency);
        self.pump_motor_efficiency = typed_hp_cfg
            .common
            .pump_motor_efficiency
            .unwrap_or(self.pump_motor_efficiency);
        self.pump_system_head_loss_m = typed_hp_cfg
            .common
            .pump_system_head_loss_m
            .unwrap_or(self.pump_system_head_loss_m);

        if let Some(ewt) = typed_hp_cfg.common.enter_water_temp_c {
            self.inner.core.source_temp = SourceTemperature::Constant(ewt);
        }

        self.pump_zip = crate::hvac::reactive::secondary_motor_zip(
            &crate::config::resolve_reactive_zip(config)?,
            crate::hvac::reactive::LOOP_PUMP_ZIP,
        );
        Ok(())
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        self.inner.update_control(env)
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        self.inner.step(env, dt, ports)?;

        // space_fraction scales the pump draw exactly like the inner unit's
        // compressor/fan (see GshpCooler::step).
        let compressor_ran = self.inner.core.last_cooling_rtf > 0.0;
        let sf = self.inner.core.hvac.config.space_fraction;
        let pump_kw = if compressor_ran {
            hares_physics::pump::compute_ground_loop_pump_power_kw(
                self.pump_loop_depth_m,
                self.pump_pipe_diameter_m,
                self.pump_flow_rate_m3_per_s,
                self.pump_efficiency,
                self.pump_motor_efficiency,
                self.pump_system_head_loss_m,
            ) * sf
        } else {
            0.0
        };

        // Per-component reactive (see `hvac::reactive`): the water-loop pump
        // is a circulation pump motor (pf 0.84), not a cooling compressor.
        // See GshpCooler::step for the full rationale.
        let pump_q = self
            .pump_zip
            .reactive_kvar(pump_kw, env.grid.bus_voltage_pu());
        if pump_kw > 0.0 || pump_q != 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_w: power_kw_to_w(pump_kw),
                reactive_power_kvar: pump_q,
            })?;
        }

        self.inner.set_telemetry(tk::PUMP_POWER_KW, pump_kw);

        // Rebuild the CoreOutput so it includes the pump's active and reactive
        // contribution (the inner CoreOutput covers only compressor + fan +
        // crankcase). This keeps port and CoreOutput consistent.
        let inner_co = self.inner.core_output().clone();
        let inner_kw = match inner_co.flows.electric_kw {
            Some(hares_types::ElectricPower::Consumption(kw)) => kw,
            Some(hares_types::ElectricPower::Bidirectional(kw)) => kw,
            _ => 0.0,
        };
        let inner_q = inner_co.flows.reactive_power_kvar.unwrap_or(0.0);
        let total_kw = inner_kw + pump_kw;
        let total_q = inner_q + pump_q;
        self.inner.set_telemetry(tk::ELECTRIC_KW, total_kw);
        self.inner.set_telemetry(tk::REACTIVE_POWER_KVAR, total_q);
        let mut combined = inner_co;
        combined.flows.electric_kw =
            Some(hares_types::ElectricPower::Consumption(total_kw.max(0.0)));
        combined.flows.reactive_power_kvar = Some(total_q);
        self.core_output = combined;

        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        self.inner.telemetry()
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core_output
    }

    fn resolved_zip(&self) -> Option<hares_types::zip::ZipLoad> {
        // Primary component: the compressor ZIP resolved by the inner
        // AirConditioner (the user "zip" sidecar is propagated to it); the
        // loop-pump component ZIP is secondary.
        self.inner.resolved_zip()
    }

    fn save_state(&self) -> crate::Result<Vec<u8>> {
        self.inner.save_state()
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        self.inner.load_state(state)
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        self.inner.apply_control_unchecked(signal)
    }

    fn ideal_target(&self) -> Option<(hares_types::ZoneId, f64)> {
        self.inner.ideal_target()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlCapabilities, ControlSignal, EndUse, EnvironmentState, ExecutionStage, FuelType,
        GridState, PortSlots, ThermalAccumulator, WeatherState, ZoneId, ZoneState,
    };

    use super::{
        super::super::super::Equipment, super::super::super::EquipmentConfig, GshpCooler, HpCooler,
    };
    use crate::HvacSetpointConfig;
    use crate::config::ConfigPayload;
    use crate::hvac::SpeedControlMode;

    fn cooling_env(zone_temp_c: f64, outdoor_c: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio: 0.010,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: outdoor_c,
                outdoor_humidity_ratio: 0.005,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 15.0,
                sky_temp_c: 20.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![],
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                solar_altitude_deg: 0.0,
                ..Default::default()
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
                .expect("UTC offset")
                .with_ymd_and_hms(2026, 7, 15, 14, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::minutes(1),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    fn base_config() -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "HP Cooler".to_string(),
            "ASHP Cooler".to_string(),
            crate::HeatPumpCoolerConfig {
                common: crate::HeatPumpCommonConfig {
                    equipment_id: None,
                    zone_id: Some(1),
                    heating_capacity_w: None,
                    heating_eir: None,
                    stage_heating_capacities_w: None,
                    stage_heating_eirs: None,
                    backup_fuel: None,
                    backup_capacity_w: None,
                    backup_eir: None,
                    fraction_heating_load_served: None,
                    cooling_capacity_w: Some(8_000.0),
                    cooling_eir: Some(0.33),
                    stage_cooling_capacities_w: None,
                    stage_cooling_eirs: None,
                    fraction_cooling_load_served: None,
                    number_of_speeds: 1,
                    is_mini_split: false,
                    shr: None,
                    fan_power_w: None,
                    fan_power_w_per_cfm: None,
                    airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_CENTRAL_AC_M3_S_PER_W),
                    setpoint: HvacSetpointConfig::default(),
                    hysteresis_c: None,
                    duct: Default::default(),
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
                    ..Default::default()
                },
                stage_shrs: None,
                crankcase_heater_kw: None,
                crankcase_heater_threshold_c: None,
                min_oat_cooling_c: 10.0,
            },
        )
        .unwrap()
    }

    fn typed_config(data: serde_json::Value, ochre_class: &str) -> EquipmentConfig {
        EquipmentConfig::with_payload(
            "HP Cooler".to_string(),
            ochre_class.to_string(),
            ConfigPayload::Typed {
                type_name: "ASHP Cooler".to_string(),
                version: 1,
                data,
            },
        )
    }

    /// Zone above cooling setpoint -- cooler must remove heat (negative thermal
    /// contribution) and draw positive electrical power.
    #[test]
    fn ashp_cooler_cools_when_zone_above_setpoint() {
        let cfg = base_config();
        let mut eq = HpCooler::ashp_cooler(cfg.clone());
        let env = cooling_env(28.0, 35.0); // zone well above 24 C setpoint
        eq.init(&cfg, &env).unwrap();
        eq.apply_control(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(18.0),
            cooling_setpoint_c: Some(24.0),
            deadband_c: None,
        })
        .unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        // Cooling removes heat: thermal contribution to zone is negative
        assert!(
            ports.thermal[0].sensible_gain_w < 0.0,
            "expected negative sensible gain (cooling), got {}",
            ports.thermal[0].sensible_gain_w
        );
        // Compressor + fan draws electricity
        assert!(
            ports.electrical.net_active_w() > 0.0,
            "expected positive electrical draw, got {}",
            ports.electrical.net_active_w()
        );
    }

    /// mshp_cooler() must produce valid equipment with correct type label and
    /// end-use, proving the constructor path is distinct from ashp_cooler().
    #[test]
    fn mshp_cooler_construction_produces_valid_descriptor() {
        let cfg = base_config();
        let eq = HpCooler::mshp_cooler(cfg);

        assert_eq!(eq.descriptor().equipment_type, "MSHP Cooler");
        assert_eq!(eq.descriptor().end_use, EndUse::HVAC_COOLING);
        assert_eq!(eq.descriptor().fuel, FuelType::Electric);
        assert_eq!(eq.descriptor().stage, ExecutionStage::Thermal);
        assert!(
            eq.descriptor()
                .control_capabilities
                .contains(ControlCapabilities::THERMAL_SETPOINT)
        );
        // Must have at least one port for thermal and one for electrical
        assert!(
            eq.ports().len() >= 2,
            "expected at least 2 ports, got {}",
            eq.ports().len()
        );
    }

    /// MSHP cooler must use the canonical SI airflow ratio for mini-split cooling.
    #[test]
    fn mshp_cooler_uses_mini_split_cool_default_airflow_ratio() {
        let cfg = base_config();
        let eq = HpCooler::mshp_cooler(cfg);
        assert_eq!(
            eq.inner.core.hvac.config.equipment_type,
            crate::hvac::hvac_core::HvacEquipmentType::MiniSplitCool,
        );
        let expected = crate::hvac::hvac_core::AIRFLOW_MSHP_COOLING_M3_S_PER_W;
        assert!(
            (eq.inner.core.hvac.config.airflow_m3_s_per_w - expected).abs() < 1e-12,
            "MSHP cooler airflow mismatch, got {} m3/s/W",
            eq.inner.core.hvac.config.airflow_m3_s_per_w
        );
    }

    /// Zone between heating and cooling setpoints -- deadband -- no electrical
    /// draw and no thermal load.
    #[test]
    fn cooler_in_deadband_produces_zero_output() {
        let cfg = base_config();
        let mut eq = HpCooler::ashp_cooler(cfg.clone());
        // Zone at 21 C, well inside the 18–24 C deadband
        let env = cooling_env(21.0, 25.0);
        eq.init(&cfg, &env).unwrap();
        eq.apply_control(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(18.0),
            cooling_setpoint_c: Some(24.0),
            deadband_c: None,
        })
        .unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        // Outdoor temp is above crankcase heater threshold (12.8 C), so no
        // crankcase draw either. Expect exactly zero electrical and thermal.
        assert_eq!(
            ports.thermal[0].sensible_gain_w, 0.0,
            "deadband: expected zero thermal output"
        );
        assert_eq!(
            ports.electrical.net_active_w(),
            0.0,
            "deadband: expected zero electrical draw"
        );
    }

    /// Control signal application: sending a ThermalSetpoint signal must be
    /// accepted (capability gate passes) and must not error.
    #[test]
    fn apply_control_accepts_thermal_setpoint_signal() {
        let cfg = base_config();
        let mut eq = HpCooler::ashp_cooler(cfg);
        let signal = ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(18.0),
            cooling_setpoint_c: Some(25.0),
            deadband_c: None,
        };
        eq.apply_control(&signal)
            .expect("ThermalSetpoint should be accepted by HpCooler");
    }

    #[test]
    fn hp_cooler_declares_wrapped_air_conditioner_control_capabilities() {
        let cfg = base_config();
        let eq = HpCooler::ashp_cooler(cfg);
        let caps = eq.descriptor().control_capabilities;

        assert!(caps.contains(ControlCapabilities::THERMAL_SETPOINT));
        assert!(caps.contains(ControlCapabilities::THERMAL_SETPOINT_DELTA));
        assert!(caps.contains(ControlCapabilities::DUTY_CYCLE));
        assert!(caps.contains(ControlCapabilities::LOAD_FRACTION));
        assert!(caps.contains(ControlCapabilities::POWER_LIMIT));
        assert!(caps.contains(ControlCapabilities::MODE_OVERRIDE));
        assert!(caps.contains(ControlCapabilities::DEMAND_RESPONSE));
        assert!(caps.contains(ControlCapabilities::IDEAL_CAPACITY));
    }

    /// OCHRE parity: cooler must be OFF at initialization when zone_temp == cooling_setpoint.
    ///
    /// OCHRE HVAC.py: turn_on = setpoint + deadband * (1 - offset) = 24.4 + 1.0 * 0.8 = 25.2
    /// With zone_temp = 24.4 <= 25.2, OCHRE stays OFF (mode_prev = "Off", neither
    /// turn-on nor turn-off condition fires → keeps current "Off" mode).
    /// HARES must match: thermostat stays in Deadband, no electrical draw, no cooling.
    #[test]
    fn cooler_off_when_zone_temp_at_setpoint_at_init() {
        let cfg = typed_config(
            serde_json::json!({
                "zone_id": 1,
                "cooling_capacity_w": 8_000.0,
                "cooling_eir": 0.33,
                "number_of_speeds": 1,
                "is_mini_split": false
            }),
            "ASHP Cooler",
        );

        // Zone at exactly the cooling setpoint (24.4°C).
        // turn_on = 24.4 + 1.0 * (1 - 0.2) = 25.2 → zone NOT above threshold → cooler must be OFF.
        let env = cooling_env(24.4, 35.0);

        let mut eq = HpCooler::ashp_cooler(cfg.clone());
        eq.init(&cfg, &env).unwrap();
        eq.apply_control(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(18.0),
            cooling_setpoint_c: Some(24.4),
            deadband_c: None,
        })
        .unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert_eq!(
            ports.thermal[0].sensible_gain_w, 0.0,
            "cooler must produce zero thermal output when zone_temp (24.4°C) == cooling_setpoint; \
             OCHRE turn-on threshold = 25.2°C -- got {:.3} W",
            ports.thermal[0].sensible_gain_w
        );
        assert_eq!(
            ports.electrical.net_active_w(),
            0.0,
            "cooler must draw 0 kW when zone_temp (24.4°C) <= turn-on threshold (25.2°C); \
             got {:.6} kW",
            ports.electrical.net_active_w()
        );
    }

    /// MSHP crankcase defaults (15 W / 0 °C) must be active even when step() is
    /// called without a prior init().  The distinguishing condition uses an outdoor
    /// temp between the MSHP threshold (0 °C) and the central-AC default (12.8 °C):
    ///   - MSHP correct:  5 °C >= 0 °C  → crankcase = 0.0 kW
    ///   - Central-AC wrong: 5 °C < 12.8 °C → crankcase = 0.05 kW (50 W)
    #[test]
    fn mshp_cooler_uses_correct_crankcase_defaults_without_init() {
        let cfg = base_config();
        let mut eq = HpCooler::mshp_cooler(cfg);

        // Zone in deadband (21 °C, between default heating=20 °C / cooling=24 °C),
        // outdoor at 5 °C -- above the MSHP 0 °C crankcase threshold but below the
        // central-AC 12.8 °C threshold. No init() call.
        let env = cooling_env(21.0, 5.0);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert_eq!(
            ports.electrical.net_active_w(),
            0.0,
            "MSHP crankcase must be inactive at 5 °C (threshold 0 °C); \
             central-AC default (12.8 °C) would produce 0.05 kW -- got {}",
            ports.electrical.net_active_w()
        );
    }

    #[test]
    fn typed_minisplit_overrides_to_four_speeds() {
        let cfg = typed_config(
            serde_json::json!({
                "zone_id": 1,
                "cooling_capacity_w": 8000.0,
                "cooling_eir": 16.0,
                "number_of_speeds": 1,
                "is_mini_split": true
            }),
            "MSHP Cooler",
        );
        let mut eq = HpCooler::mshp_cooler(cfg.clone());
        let env = cooling_env(28.0, 35.0);
        eq.init(&cfg, &env).unwrap();

        assert_eq!(
            eq.inner.core.hvac.config.speed_control_mode,
            SpeedControlMode::VariableSpeedIdeal
        );
        assert_eq!(
            eq.inner.core.hvac.runtime.startup.c_d, 0.0,
            "typed mini-split cooling must derive zero startup Cd"
        );
    }

    #[test]
    fn typed_stage_shrs_are_propagated_for_heat_pump_cooling() {
        let cfg = typed_config(
            serde_json::json!({
                "zone_id": 1,
                "cooling_capacity_w": 8000.0,
                "cooling_eir": 16.0,
                "stage_shrs": [0.81]
            }),
            "ASHP Cooler",
        );
        let mut eq = HpCooler::ashp_cooler(cfg.clone());
        let env = cooling_env(28.0, 35.0);
        eq.init(&cfg, &env).unwrap();

        assert_eq!(eq.inner.core.stage_shrs, vec![0.81]);
    }

    #[test]
    fn minisplit_cooling_retains_fractional_runtime_near_setpoint() {
        let cfg = typed_config(
            serde_json::json!({
                "zone_id": 1,
                "cooling_capacity_w": 8000.0,
                "cooling_eir": 16.0,
                "number_of_speeds": 1,
                "is_mini_split": true,
                "hysteresis_c": 0.0
            }),
            "MSHP Cooler",
        );
        let mut eq = HpCooler::mshp_cooler(cfg.clone());
        // Slightly above setpoint (24.0 C) so load_fraction=0.2.
        // For 4-stage MSHP (fractions 0.25,0.5,0.75,1.0), this is below
        // lowest-stage fraction, so duty should be fractional (<1).
        let env = cooling_env(24.1, 35.0);
        eq.init(&cfg, &env).unwrap();
        eq.apply_control(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(18.0),
            cooling_setpoint_c: Some(24.0),
            deadband_c: Some(0.0),
        })
        .unwrap();

        eq.update_control(&env);
        let duty = eq.inner.core.hvac.runtime.duty_cycle;
        assert!(
            duty > 0.0 && duty < 1.0,
            "mini-split variable-speed cooling should preserve fractional runtime near setpoint, got duty={duty}"
        );
    }

    // -------------------------------------------------------------------
    // GSHP cooler regression tests
    // -------------------------------------------------------------------

    /// After `init()`, GSHP cooler must have crankcase heater disabled:
    /// crankcase_rated_kw == 0.0, crankcase_threshold_c == NEG_INFINITY.
    /// The compressor is indoors; no crankcase heater is needed.
    /// Regression test for the finding where crankcase overrides were placed
    /// before `inner.init()` and overwritten by `AirConditioner::init_from_typed`.
    #[test]
    fn gshp_cooler_crankcase_disabled_after_init() {
        let cfg = EquipmentConfig::from_typed(
            "gshp_cooler".to_string(),
            "GSHP Cooler".to_string(),
            crate::HeatPumpCoolerConfig {
                common: crate::HeatPumpCommonConfig {
                    zone_id: Some(1),
                    cooling_capacity_w: Some(8_000.0),
                    cooling_eir: Some(0.33),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        let mut eq = GshpCooler::new(cfg.clone());
        let env = cooling_env(21.0, 10.0);
        eq.init(&cfg, &env).unwrap();

        assert_eq!(
            eq.inner.core.crankcase_rated_kw, 0.0,
            "GSHP crankcase rated kW must be 0.0 after init (compressor indoors), \
             got {} kW",
            eq.inner.core.crankcase_rated_kw
        );
        assert_eq!(
            eq.inner.core.crankcase_threshold_c,
            f64::NEG_INFINITY,
            "GSHP crankcase threshold must be NEG_INFINITY after init (never active), \
             got {} °C",
            eq.inner.core.crankcase_threshold_c
        );
    }

    /// GSHP cooler in deadband with OAT below central-AC crankcase threshold
    /// (12.8 °C) must draw zero electrical power — the crankcase heater
    /// should never fire for indoor-compressor equipment.
    #[test]
    fn gshp_cooler_no_crankcase_power_at_cold_ambient() {
        let cfg = EquipmentConfig::from_typed(
            "gshp_cooler".to_string(),
            "GSHP Cooler".to_string(),
            crate::HeatPumpCoolerConfig {
                common: crate::HeatPumpCommonConfig {
                    zone_id: Some(1),
                    cooling_capacity_w: Some(8_000.0),
                    cooling_eir: Some(0.33),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        let mut eq = GshpCooler::new(cfg.clone());
        // Zone in deadband (21 °C, between default heating=20 °C / cooling=24 °C),
        // outdoor at 5 °C — below central-AC crankcase threshold of 12.8 °C.
        let env = cooling_env(21.0, 5.0);
        eq.init(&cfg, &env).unwrap();
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert_eq!(
            ports.electrical.net_active_w(),
            0.0,
            "GSHP crankcase must be inactive at 5 °C OAT (threshold NEG_INFINITY); \
             central-AC default (12.8 °C) would produce 0.05 kW — got {}",
            ports.electrical.net_active_w()
        );
    }

    /// Borehole parameters wired through `HeatPumpCommonConfig` must reach
    /// `BoreholeGFunctionModel` and influence `compute_entering_water_temp`.
    /// Deeper boreholes (120 m vs default 60 m) halve the per-unit-length
    /// heat injection rate, yielding a lower entering water temperature in
    /// cooling mode. This test covers the full wiring chain:
    /// `HeatPumpCommonConfig` → `init` → `BoreholeConfig` →
    /// `BoreholeGFunctionModel`.
    #[test]
    fn gshp_cooler_borehole_depth_reaches_ewt() {
        let env = cooling_env(26.0, 10.0);

        // Default borehole depth (60 m from BoreholeConfig::default()).
        let cfg_default = EquipmentConfig::from_typed(
            "gshp_default".to_string(),
            "GSHP Cooler".to_string(),
            crate::HeatPumpCoolerConfig {
                common: crate::HeatPumpCommonConfig {
                    zone_id: Some(1),
                    cooling_capacity_w: Some(8_000.0),
                    cooling_eir: Some(0.33),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        let mut eq_default = GshpCooler::new(cfg_default.clone());
        eq_default.init(&cfg_default, &env).unwrap();

        // Custom borehole depth (120 m).
        let cfg_deep = EquipmentConfig::from_typed(
            "gshp_deep".to_string(),
            "GSHP Cooler".to_string(),
            crate::HeatPumpCoolerConfig {
                common: crate::HeatPumpCommonConfig {
                    zone_id: Some(1),
                    cooling_capacity_w: Some(8_000.0),
                    cooling_eir: Some(0.33),
                    borehole_depth_m: Some(120.0),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        let mut eq_deep = GshpCooler::new(cfg_deep.clone());
        eq_deep.init(&cfg_deep, &env).unwrap();

        // Record identical heat injection (5 kW for 1 h) in both models.
        // Cooling mode injects heat INTO the ground: positive Q per Eskilson.
        let heat_rate_w = 5_000.0;
        let dt_s = 3600.0;
        eq_default
            .inner
            .core
            .source_temp
            .record_source_heat_rate(heat_rate_w, dt_s);
        eq_deep
            .inner
            .core
            .source_temp
            .record_source_heat_rate(heat_rate_w, dt_s);

        // q_per_unit = Q / H / N — deeper borehole has half the per-unit-
        // length heat rate, so the resistive temperature rise is smaller and
        // the entering water temperature is lower.
        let ewt_default = eq_default.inner.core.source_temp.compute(&env);
        let ewt_deep = eq_deep.inner.core.source_temp.compute(&env);
        assert!(
            ewt_deep < ewt_default,
            "deeper borehole (120 m) should yield lower entering water temperature \
             in cooling mode than default (60 m); got deep={ewt_deep:.3}, default={ewt_default:.3}"
        );
    }

    /// D2ACooler (ASHP/air-air cooler) must apply ThermalSetpointDelta by
    /// adjusting the runtime setpoint via the catch-all → HvacEquipment path.
    #[test]
    fn hp_cooler_thermal_setpoint_delta_adjusts_cooling_setpoint() {
        let cfg = base_config();
        let mut eq = HpCooler::ashp_cooler(cfg.clone());
        let env = cooling_env(28.0, 35.0);
        eq.init(&cfg, &env).unwrap();

        assert!(
            eq.descriptor()
                .control_capabilities
                .contains(ControlCapabilities::THERMAL_SETPOINT_DELTA),
            "HpCooler must declare THERMAL_SETPOINT_DELTA capability"
        );

        let baseline = eq.inner.core.hvac.effective_setpoints();

        // Dispatch ThermalSetpointDelta: raise cooling setpoint by 0.5 °C.
        eq.apply_control(&ControlSignal::ThermalSetpointDelta {
            heating_delta_c: None,
            cooling_delta_c: Some(0.5),
        })
        .unwrap();

        let adjusted = eq.inner.core.hvac.effective_setpoints();
        let expected_cooling = baseline.cooling_c + 0.5;
        assert!(
            (adjusted.cooling_c - expected_cooling).abs() < 1e-9,
            "cooling setpoint must increase by 0.5 °C: baseline={}, expected={}, got={}",
            baseline.cooling_c,
            expected_cooling,
            adjusted.cooling_c,
        );
        // Heating setpoint unchanged — delta only specified for cooling.
        assert_eq!(
            adjusted.heating_c, baseline.heating_c,
            "heating setpoint must not change when only cooling_delta_c is specified"
        );
    }

    /// GshpCooler must apply ThermalSetpointDelta by adjusting the runtime
    /// setpoint via the catch-all → HvacEquipment path.
    #[test]
    fn gshp_cooler_thermal_setpoint_delta_adjusts_cooling_setpoint() {
        use crate::{HeatPumpCommonConfig, HeatPumpCoolerConfig};

        let cfg = EquipmentConfig::from_typed(
            "GSHP Cooler Test".to_string(),
            "GSHP Cooler".to_string(),
            HeatPumpCoolerConfig {
                common: HeatPumpCommonConfig {
                    zone_id: Some(1),
                    cooling_capacity_w: Some(8_000.0),
                    cooling_eir: Some(0.33),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        let mut eq = GshpCooler::new(cfg.clone());
        let env = cooling_env(28.0, 35.0);
        eq.init(&cfg, &env).unwrap();

        assert!(
            eq.descriptor()
                .control_capabilities
                .contains(ControlCapabilities::THERMAL_SETPOINT_DELTA),
            "GshpCooler must declare THERMAL_SETPOINT_DELTA capability"
        );

        let baseline = eq.inner.core.hvac.effective_setpoints();

        // Dispatch ThermalSetpointDelta: raise cooling setpoint by 0.5 °C.
        eq.apply_control(&ControlSignal::ThermalSetpointDelta {
            heating_delta_c: None,
            cooling_delta_c: Some(0.5),
        })
        .unwrap();

        let adjusted = eq.inner.core.hvac.effective_setpoints();
        let expected_cooling = baseline.cooling_c + 0.5;
        assert!(
            (adjusted.cooling_c - expected_cooling).abs() < 1e-9,
            "cooling setpoint must increase by 0.5 °C: baseline={}, expected={}, got={}",
            baseline.cooling_c,
            expected_cooling,
            adjusted.cooling_c,
        );
        // Heating setpoint unchanged — delta only specified for cooling.
        assert_eq!(
            adjusted.heating_c, baseline.heating_c,
            "heating setpoint must not change when only cooling_delta_c is specified"
        );
    }

    /// ASHP cooler uses the configured `min_oat_cooling_c` default (10.0 °C)
    /// rather than the standalone AC default (-25 °C). Verified by supplying OAT
    /// at 5 °C (below the 10 °C threshold) and asserting the cooler locks out:
    /// a standalone AC default (-25 °C) would permit cooling, but the HP default
    /// (10 °C) blocks it.
    #[test]
    fn ashp_cooler_uses_hp_cooling_lockout_default() {
        use crate::{HeatPumpCommonConfig, HeatPumpCoolerConfig};

        let typed_hp = EquipmentConfig::from_typed(
            "test".to_string(),
            "ASHP Cooler".to_string(),
            HeatPumpCoolerConfig {
                common: HeatPumpCommonConfig {
                    zone_id: Some(1),
                    cooling_capacity_w: Some(8_000.0),
                    cooling_eir: Some(0.33),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        let mut cooler = HpCooler::ashp_cooler(typed_hp.clone());
        // Init with a warm env so the cooler's internals are fully set up.
        cooler.init(&typed_hp, &cooling_env(28.0, 35.0)).unwrap();

        // 5 °C is above the standalone AC default (-25 °C) but below the
        // HP cooling lockout default (10.0 °C). The HP default should lock
        // out the compressor.
        // ASHRAE 90.1-2022 §6.5.1.4: economizer changeover occurs at ~10 °C.
        let env_cold = cooling_env(28.0, 5.0);
        let mode = cooler.update_control(&env_cold);
        assert_eq!(
            mode,
            super::OperatingMode::Off,
            "ASHP cooler must lock out at 5 °C OAT (HP default 10.0 °C cooling lockout)",
        );
    }

    /// When OAT is above the configured `min_oat_cooling_c`, the HP cooler
    /// operates normally (Cooling mode).
    #[test]
    fn ashp_cooler_cools_when_oat_above_min_cooling_lockout() {
        use crate::{HeatPumpCommonConfig, HeatPumpCoolerConfig};

        let typed_hp = EquipmentConfig::from_typed(
            "test".to_string(),
            "ASHP Cooler".to_string(),
            HeatPumpCoolerConfig {
                common: HeatPumpCommonConfig {
                    zone_id: Some(1),
                    cooling_capacity_w: Some(8_000.0),
                    cooling_eir: Some(0.33),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        let mut cooler = HpCooler::ashp_cooler(typed_hp.clone());
        // Init with a warm env.
        cooler.init(&typed_hp, &cooling_env(28.0, 35.0)).unwrap();

        // Zone at 28 °C (above 24 °C cooling setpoint), OAT at 20 °C (above 10 °C lockout).
        let env_warm = cooling_env(28.0, 20.0);
        cooler
            .apply_control(&ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(18.0),
                cooling_setpoint_c: Some(24.0),
                deadband_c: None,
            })
            .unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        cooler.update_control(&env_warm);
        cooler
            .step(&env_warm, Duration::from_secs(60), &mut ports)
            .unwrap();

        assert!(
            ports.thermal[0].sensible_gain_w < 0.0,
            "expected sensible cooling (negative thermal gain) at 20 °C OAT, got {}",
            ports.thermal[0].sensible_gain_w
        );
        assert!(
            ports.electrical.net_active_w() > 0.0,
            "expected positive electrical draw during cooling, got {}",
            ports.electrical.net_active_w()
        );
    }

    /// When OAT drops below `min_oat_cooling_c`, the cooling compressor is
    /// locked out: zero sensible cooling output and zero compressor power.
    #[test]
    fn ashp_cooler_locked_out_below_min_cooling_lockout() {
        use crate::{HeatPumpCommonConfig, HeatPumpCoolerConfig};

        let typed_hp = EquipmentConfig::from_typed(
            "test".to_string(),
            "ASHP Cooler".to_string(),
            HeatPumpCoolerConfig {
                common: HeatPumpCommonConfig {
                    zone_id: Some(1),
                    cooling_capacity_w: Some(8_000.0),
                    cooling_eir: Some(0.33),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        let mut cooler = HpCooler::ashp_cooler(typed_hp.clone());
        cooler.init(&typed_hp, &cooling_env(28.0, 35.0)).unwrap();

        // OAT at 5 °C (< 10 °C lockout), zone well above cooling setpoint.
        let env_cold = cooling_env(28.0, 5.0);
        cooler
            .apply_control(&ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(18.0),
                cooling_setpoint_c: Some(24.0),
                deadband_c: None,
            })
            .unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        cooler.update_control(&env_cold);
        cooler
            .step(&env_cold, Duration::from_secs(60), &mut ports)
            .unwrap();

        assert_eq!(
            ports.thermal[0].sensible_gain_w, 0.0,
            "cooling compressor must be locked out (zero sensible output) at 5 °C OAT",
        );
        // Crankcase heater (50 W default) activates at 5 °C (< 12.78 °C threshold).
        // The compressor is locked out but the crankcase heater prevents oil migration.
        assert!(
            (ports.electrical.net_active_w() - 50.0).abs() < 1.0,
            "crankcase heater (50 W) should be active at 5 °C OAT even when compressor is locked out, got {}",
            ports.electrical.net_active_w()
        );
    }

    /// Reactive-power contract for the ASHP cooler (delegated to the inner
    /// AirConditioner): per-component Q (compressor pf 0.96, fan pf 0.87,
    /// crankcase resistive Q=0), REACTIVE declared, port/CoreOutput/telemetry
    /// agree. Off ⇒ Q == 0.
    #[test]
    fn ashp_cooler_reactive_power_pf_and_channels_agree() {
        let cfg = base_config();
        let mut eq = HpCooler::ashp_cooler(cfg.clone());
        let env = cooling_env(28.0, 35.0);
        eq.init(&cfg, &env).unwrap();
        eq.apply_control(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(18.0),
            cooling_setpoint_c: Some(24.0),
            deadband_c: None,
        })
        .unwrap();
        assert!(
            eq.descriptor()
                .core_capabilities
                .contains(hares_types::CoreCapabilities::REACTIVE),
            "ASHP cooler must declare REACTIVE"
        );
        eq.update_control(&env);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let p_kw = ports.electrical.load_power_w / 1000.0;
        assert!(p_kw > 0.0, "cooling call must draw real power");
        let compressor_kw = eq
            .telemetry()
            .get(hares_types::telemetry_keys::COMPRESSOR_KW)
            .expect("compressor");
        let fan_kw = eq
            .telemetry()
            .get(hares_types::telemetry_keys::FAN_KW)
            .expect("fan");
        assert!(compressor_kw > 0.0 && fan_kw > 0.0);
        let q = ports.electrical.reactive_power_kvar;
        // Per-component: Q = P_comp·tan(acos(0.96)) + P_fan·tan(acos(0.87)).
        let expected = compressor_kw * 0.96_f64.acos().tan() + fan_kw * 0.87_f64.acos().tan();
        assert!(
            (q - expected).abs() < 1e-9,
            "per-component Q must be comp·tan(acos(0.96)) + fan·tan(acos(0.87)): \
             q={q}, expected={expected}"
        );
        assert_eq!(
            eq.core_output()
                .flows
                .reactive_power_kvar
                .expect("Some")
                .to_bits(),
            q.to_bits(),
            "CoreOutput Q must equal port Q"
        );
        assert_eq!(
            eq.telemetry()
                .get(hares_types::telemetry_keys::REACTIVE_POWER_KVAR)
                .expect("telemetry Q")
                .to_bits(),
            q.to_bits(),
            "telemetry Q must equal port Q"
        );
        hares_types::validate_core_contract(eq.descriptor(), eq.core_output())
            .expect("core contract must hold with REACTIVE declared");

        // Off case: zone in deadband (21 °C) ⇒ no cooling call ⇒ Q == 0.
        let off_env = cooling_env(21.0, 20.0);
        eq.update_control(&off_env);
        let mut off_ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&off_env, Duration::from_secs(60), &mut off_ports)
            .unwrap();
        assert_eq!(
            off_ports.electrical.reactive_power_kvar, 0.0,
            "off ⇒ Q == 0"
        );
    }

    /// Reactive-power contract for the GSHP cooler: per-component Q — the
    /// inner AirConditioner contributes compressor (pf 0.96) + fan (pf 0.87),
    /// and the ground-loop pump adds its circulation-pump-motor component
    /// (pf 0.84, `hvac::reactive::LOOP_PUMP_ZIP`). Port/CoreOutput/telemetry
    /// all agree on the combined value.
    #[test]
    fn gshp_cooler_reactive_power_folds_pump_into_unit_total() {
        let cfg = EquipmentConfig::from_typed(
            "gshp_cooler".to_string(),
            "GSHP Cooler".to_string(),
            crate::HeatPumpCoolerConfig {
                common: crate::HeatPumpCommonConfig {
                    zone_id: Some(1),
                    cooling_capacity_w: Some(8_000.0),
                    cooling_eir: Some(0.33),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        let mut eq = GshpCooler::new(cfg.clone());
        let env = cooling_env(28.0, 10.0);
        eq.init(&cfg, &env).unwrap();
        eq.apply_control(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(18.0),
            cooling_setpoint_c: Some(24.0),
            deadband_c: None,
        })
        .unwrap();
        assert!(
            eq.descriptor()
                .core_capabilities
                .contains(hares_types::CoreCapabilities::REACTIVE),
            "GSHP cooler must declare REACTIVE"
        );
        assert_eq!(
            eq.pump_zip,
            crate::hvac::reactive::LOOP_PUMP_ZIP,
            "loop pump component uses the pump motor ZIP (pf 0.84)"
        );
        eq.update_control(&env);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let p_kw = ports.electrical.load_power_w / 1000.0;
        assert!(p_kw > 0.0, "GSHP cooling call must draw real power");
        let compressor_kw = eq
            .telemetry()
            .get(hares_types::telemetry_keys::COMPRESSOR_KW)
            .expect("compressor");
        let fan_kw = eq
            .telemetry()
            .get(hares_types::telemetry_keys::FAN_KW)
            .expect("fan");
        let pump_kw = eq
            .telemetry()
            .get(hares_types::telemetry_keys::PUMP_POWER_KW)
            .expect("pump");
        assert!(
            compressor_kw > 0.0 && fan_kw > 0.0 && pump_kw > 0.0,
            "compressor, fan, and loop pump must all draw power: \
             comp={compressor_kw} fan={fan_kw} pump={pump_kw}"
        );
        let q = ports.electrical.reactive_power_kvar;
        // Per-component: compressor 0.96, fan 0.87, loop pump 0.84.
        let expected = compressor_kw * 0.96_f64.acos().tan()
            + fan_kw * 0.87_f64.acos().tan()
            + pump_kw * 0.84_f64.acos().tan();
        assert!(
            (q - expected).abs() < 1e-9,
            "per-component Q must be comp·tan(acos(0.96)) + fan·tan(acos(0.87)) \
             + pump·tan(acos(0.84)): q={q}, expected={expected}, p={p_kw}"
        );
        // Port, CoreOutput, and telemetry must all carry the combined value.
        let co_q = eq
            .core_output()
            .flows
            .reactive_power_kvar
            .expect("CoreOutput Q Some");
        assert_eq!(
            co_q.to_bits(),
            q.to_bits(),
            "CoreOutput Q must equal port Q"
        );
        let tel_q = eq
            .telemetry()
            .get(hares_types::telemetry_keys::REACTIVE_POWER_KVAR)
            .expect("telemetry Q");
        assert_eq!(
            tel_q.to_bits(),
            q.to_bits(),
            "telemetry Q must equal port Q"
        );
        // CoreOutput active must also include the pump (folded unit total).
        let co_kw = match eq.core_output().flows.electric_kw {
            Some(hares_types::ElectricPower::Consumption(kw)) => kw,
            other => panic!("expected Consumption, got {other:?}"),
        };
        assert!(
            (co_kw - p_kw).abs() < 1e-9,
            "CoreOutput active must equal port active (pump folded in): co={co_kw} port={p_kw}"
        );
        hares_types::validate_core_contract(eq.descriptor(), eq.core_output())
            .expect("core contract must hold with REACTIVE declared");
    }

    /// Rule R1 regression for the ASHP cooler: twin instances — one with the
    /// class pf 0.96, one with a constant-power sidecar override — must produce
    /// bit-identical real power at every step and voltage.
    #[test]
    fn ashp_cooler_real_power_bit_identical_with_and_without_reactive_zip() {
        let config_pf = base_config();
        let mut config_nopf = config_pf.clone();
        config_nopf.zip = Some(hares_types::zip::ZipLoad::constant_power());

        let mut eq_pf = HpCooler::ashp_cooler(config_pf.clone());
        let mut eq_nopf = HpCooler::ashp_cooler(config_nopf.clone());
        let mut env = cooling_env(28.0, 35.0);
        eq_pf.init(&config_pf, &env).unwrap();
        eq_nopf.init(&config_nopf, &env).unwrap();
        for eq in [&mut eq_pf, &mut eq_nopf] {
            eq.apply_control(&ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(18.0),
                cooling_setpoint_c: Some(24.0),
                deadband_c: None,
            })
            .unwrap();
        }

        let mut any_reactive = false;
        for (i, v) in [1.0, 0.95, 1.05, 1.0, 0.9, 1.1].iter().enumerate() {
            env.grid.voltage_pu = *v;
            eq_pf.update_control(&env);
            eq_nopf.update_control(&env);
            let mut ports_pf = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                humidity: vec![hares_types::HumidityAccumulator::new(ZoneId(1))],
                ..PortSlots::default()
            };
            let mut ports_nopf = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                humidity: vec![hares_types::HumidityAccumulator::new(ZoneId(1))],
                ..PortSlots::default()
            };
            eq_pf
                .step(&env, Duration::from_secs(60), &mut ports_pf)
                .unwrap();
            eq_nopf
                .step(&env, Duration::from_secs(60), &mut ports_nopf)
                .unwrap();
            assert_eq!(
                ports_pf.electrical.load_power_w.to_bits(),
                ports_nopf.electrical.load_power_w.to_bits(),
                "step {i} (v={v}): real power diverged between pf and no-pf twins"
            );
            assert_eq!(
                ports_nopf.electrical.reactive_power_kvar, 0.0,
                "pf-0 twin must produce zero reactive power"
            );
            if ports_pf.electrical.reactive_power_kvar != 0.0 {
                any_reactive = true;
            }
            env.current_time += ChronoDuration::minutes(1);
        }
        assert!(
            any_reactive,
            "the pf 0.96 twin must produce reactive power while cooling"
        );
    }
}
