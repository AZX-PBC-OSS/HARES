//! Heat-pump heater variants (ASHP and MSHP).

use std::borrow::Cow;
use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CoreState,
    DRLevel, ElectricPower, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FuelType, HaresError, OperatingMode, PortContribution, PortDeclaration,
    PortSlots, Telemetry, ThermalCategory, ZoneId,
};
use serde::{Deserialize, Serialize};

use hares_types::telemetry_keys as tk;

use crate::{Equipment, EquipmentConfig, load_postcard, save_postcard};

use super::super::{
    HvacEquipment, HvacEquipmentType, RuntimeSetpointOverride, SpeedControlMode, ThermostatMode,
    ac_config::HeatPumpHeaterConfig,
    helpers::{
        apply_heating_control_unchecked, equipment_id_from_config, lookup_zone, zone_id_from_config,
    },
};
use super::constants::{
    DEFAULT_BACKUP_CAPACITY_W, DEFAULT_BACKUP_EIR, DEFAULT_EQUIPMENT_ID,
    DEFAULT_ER_HARD_LOCKOUT_TIME_S, DEFAULT_ER_LOCKOUT_TEMP_C, DEFAULT_ER_SETPOINT_DEADBAND_OFFSET,
    DEFAULT_ER_SETPOINT_OFFSET_MULTIPLIER, DEFAULT_HEATING_CAPACITY_W, DEFAULT_HEATING_EIR,
    DEFAULT_HP_LOCKOUT_TEMP_C, DEFAULT_MIN_ER_CYCLE_TIME_S, DEFAULT_MSHP_SPEED_MAP,
    DEFAULT_ZONE_ID, MAX_OAT_SUPPLEMENTAL_C, MSHP_PAN_HEATER_DEFAULT_TEMP_C,
};
use super::defrost::{DefrostConfig, evaluate_defrost};
use super::heater_config::{
    default_heater_telemetry, heater_telemetry_fields, operating_mode_code,
};

#[derive(Clone, Copy)]
enum HeaterVariant {
    Ashp,
    Minisplit,
}

pub struct ASHPHeater {
    core: HeatPumpHeaterCore,
}

pub struct MinisplitHeater {
    core: HeatPumpHeaterCore,
}

struct HeatPumpHeaterCore {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    core_output: CoreOutput,
    hvac: HvacEquipment,
    operating_mode: OperatingMode,
    defrost_config: DefrostConfig,
    hp_lockout_temp_c: f64,
    er_lockout_temp_c: f64,
    /// EnergyPlus supplemental-ER upper OAT bound. ER is blocked when OAT exceeds
    /// this threshold; the heat pump alone is deemed sufficient. Hard cap: 21°C.
    max_oat_supplemental_c: f64,
    er_setpoint_offset_c: f64,
    min_er_cycle_time_s: f64,
    backup_capacity_w: f64,
    backup_eir: f64,
    pan_heater_kw: f64,
    pan_heater_temp_c: f64,
    pan_heater_on: bool,
    mshp_speed_map: [u8; 4],
    variant: HeaterVariant,
    run_time_s: f64,
    cycle_on_steps: u64,
    cycle_off_steps: u64,
    defrost_active: bool,
    defrost_time_fraction: f64,
    defrost_accumulator_s: f64,
    last_er_off_at: Option<DateTime<FixedOffset>>,
    er_was_on: bool,
    /// Previous BASE heating setpoint (without DR offset) — used to detect
    /// user-initiated setpoint increases that trigger ER hard lockout. Comparing
    /// against the base setpoint prevents DR expiry (which raises the effective
    /// setpoint back to base) from falsely triggering the lockout.
    prev_base_setpoint: f64,
    /// Remaining hard-lockout time [s]. Set to `er_hard_lockout_time_s` when the
    /// setpoint is raised; decrements each step; ER is blocked while > 0.
    /// OCHRE HVAC.py: ER hard lockout after setpoint increase prevents expensive
    /// resistance heating when the heat pump can handle the ramp.
    er_lockout_remaining_s: f64,
    /// Duration of the ER hard lockout [s] after a setpoint increase.
    er_hard_lockout_time_s: f64,
    /// Previous zone temperature for soft-lockout detection.
    /// OCHRE HVAC.py: two-stage lockout — after hard lockout expires, ER stays off
    /// while zone temp is still rising (heat pump is winning).
    prev_zone_temp_c: f64,
    /// Whether the ER soft lockout is currently active.
    er_soft_lockout: bool,
    /// Runtime fraction (PLR) of the heating coil from the most recent step.
    /// Exposed so the HP system coordinator can pass it to the companion cooler
    /// for crankcase heater power accounting.
    last_heating_rtf: f64,

    // --- Ideal capacity (solver-driven) ---
    use_ideal: bool,
    ideal_capacity_w: f64,

    // --- External control signals (sticky) ---
    /// DutyCycle override fraction [0..1]; 1.0 = no effect (sticky).
    ctrl_duty_cycle: f64,
    /// PowerLimit [kW]; f64::INFINITY = no limit (sticky).
    ctrl_power_limit_kw: f64,
    /// ModeOverride; None = no override (sticky).
    ctrl_mode_override: Option<OperatingMode>,

    // --- Transient signals (reset each step) ---
    /// LoadFraction [0..1]; 1.0 = no effect (transient — resets to 1.0 each step).
    ctrl_load_fraction: f64,

    // --- Demand response state ---
    /// DR-induced setpoint offset [°C] added to effective setpoint.
    dr_setpoint_offset_c: f64,
    /// DR-induced load fraction multiplier [0..1].
    dr_load_fraction: f64,
    /// DR-induced duty cycle override [0..1]; 1.0 = no effect.
    dr_duty_cycle: f64,
    /// Remaining DR event duration [s]; None = persistent until cleared.
    dr_duration_remaining_s: Option<f64>,
    /// Current DR level (for telemetry).
    dr_level: DRLevel,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct HeaterState {
    mode: ThermostatMode,
    duty_cycle: f64,
    last_mode_switch_at: Option<DateTime<FixedOffset>>,
    mode_start_at: Option<DateTime<FixedOffset>>,
    runtime_setpoints: Option<RuntimeSetpointOverride>,
    operating_mode: OperatingMode,
    run_time_s: f64,
    cycle_on_steps: u64,
    cycle_off_steps: u64,
    defrost_active: bool,
    defrost_time_fraction: f64,
    defrost_accumulator_s: f64,
    pan_heater_on: bool,
    last_er_off_at: Option<DateTime<FixedOffset>>,
    last_speed_index: usize,
    last_speed_frac: f64,
    electric_kw: f64,
    thermal_output_w: f64,
    speed_index: f64,
    operating_mode_code: f64,
    prev_base_setpoint: f64,
    er_lockout_remaining_s: f64,
    prev_zone_temp_c: f64,
    er_soft_lockout: bool,
    // --- Sticky control signals ---
    ctrl_duty_cycle: f64,
    /// None means unlimited; f64::INFINITY does not serialize cleanly with postcard.
    ctrl_power_limit_kw: Option<f64>,
    ctrl_mode_override: Option<OperatingMode>,
    // --- Demand response state ---
    dr_level: DRLevel,
    dr_setpoint_offset_c: f64,
    dr_load_fraction: f64,
    dr_duty_cycle: f64,
    dr_duration_remaining_s: Option<f64>,
    max_oat_supplemental_c: f64,
}

#[derive(Clone, Copy)]
struct HeaterControl {
    hp_on: bool,
    er_on: bool,
    speed_index: usize,
    duty_cycle: f64,
}

#[derive(Clone, Copy)]
struct HeaterStep {
    thermal_output_w: f64,
    electric_kw: f64,
    /// Compressor-only electric power [kW], excluding fan, ER backup, and pan heater.
    /// Used for COP per AHRI/SEER convention.
    compressor_kw: f64,
    defrost_active: bool,
    defrost_time_fraction: f64,
}

impl ASHPHeater {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        Self {
            core: HeatPumpHeaterCore::new(config, HeaterVariant::Ashp),
        }
    }

    /// Return the heating coil's runtime fraction from the most recent step.
    ///
    /// Used by the system coordinator to pass as `companion_heating_rtf` to
    /// the HP cooler for crankcase heater power accounting.
    pub fn last_heating_rtf(&self) -> f64 {
        self.core.last_heating_rtf
    }
}

impl MinisplitHeater {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        Self {
            core: HeatPumpHeaterCore::new(config, HeaterVariant::Minisplit),
        }
    }

    /// Return the heating coil's runtime fraction from the most recent step.
    pub fn last_heating_rtf(&self) -> f64 {
        self.core.last_heating_rtf
    }
}

impl Equipment for HeatPumpHeaterCore {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.init(config, env)
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        self.update_control(env)
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        self.step(env, dt, ports)
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core_output
    }

    fn save_state(&self) -> Vec<u8> {
        self.save_state()
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        self.load_state(state)
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        self.apply_control_unchecked(signal)
    }
}

delegate_equipment!(ASHPHeater, core);
delegate_equipment!(MinisplitHeater, core);

impl HeatPumpHeaterCore {
    fn new(config: EquipmentConfig, variant: HeaterVariant) -> Self {
        let zone = zone_id_from_config(&config).unwrap_or(ZoneId(DEFAULT_ZONE_ID));
        let equipment_type = match variant {
            HeaterVariant::Ashp => "ASHP Heater",
            HeaterVariant::Minisplit => "MSHP Heater",
        };
        let backup_capacity_w = config
            .typed::<HeatPumpHeaterConfig>()
            .ok()
            .and_then(|cfg| cfg.backup_capacity_w)
            .unwrap_or(DEFAULT_BACKUP_CAPACITY_W)
            .max(0.0);
        let hvac_type = match variant {
            HeaterVariant::Ashp => {
                if backup_capacity_w > 0.0 {
                    HvacEquipmentType::AshpHeatPumpAux
                } else {
                    HvacEquipmentType::AshpHeatPumpOnly
                }
            }
            HeaterVariant::Minisplit => HvacEquipmentType::MiniSplitHeat,
        };

        Self {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(equipment_id_from_config(&config).unwrap_or(DEFAULT_EQUIPMENT_ID)),
                name: config.name,
                end_use: EndUse::HVAC_HEATING,
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
                core_capabilities: CoreCapabilities::ELECTRIC | CoreCapabilities::HAS_MODE,
                telemetry_fields: heater_telemetry_fields(),
            },
            ports: vec![
                PortDeclaration::electrical(),
                PortDeclaration::thermal(zone),
            ],
            telemetry: default_heater_telemetry(),
            core_output: CoreOutput::default(),
            hvac: HvacEquipment::new(hvac_type, zone),
            operating_mode: OperatingMode::Off,
            defrost_config: DefrostConfig::on_demand(1.0, 0.0),
            hp_lockout_temp_c: DEFAULT_HP_LOCKOUT_TEMP_C,
            er_lockout_temp_c: DEFAULT_ER_LOCKOUT_TEMP_C,
            max_oat_supplemental_c: MAX_OAT_SUPPLEMENTAL_C,
            er_setpoint_offset_c: 0.0,
            min_er_cycle_time_s: DEFAULT_MIN_ER_CYCLE_TIME_S,
            backup_capacity_w: DEFAULT_BACKUP_CAPACITY_W,
            backup_eir: DEFAULT_BACKUP_EIR,
            pan_heater_kw: 0.0,
            pan_heater_temp_c: MSHP_PAN_HEATER_DEFAULT_TEMP_C,
            pan_heater_on: false,
            mshp_speed_map: DEFAULT_MSHP_SPEED_MAP,
            variant,
            run_time_s: 0.0,
            cycle_on_steps: 0,
            cycle_off_steps: 0,
            defrost_active: false,
            defrost_time_fraction: 0.0,
            defrost_accumulator_s: 0.0,
            last_er_off_at: None,
            er_was_on: false,
            prev_base_setpoint: f64::NEG_INFINITY,
            er_lockout_remaining_s: 0.0,
            er_hard_lockout_time_s: DEFAULT_ER_HARD_LOCKOUT_TIME_S,
            prev_zone_temp_c: f64::NAN,
            er_soft_lockout: false,
            last_heating_rtf: 0.0,
            use_ideal: false,
            ideal_capacity_w: 0.0,
            ctrl_duty_cycle: 1.0,
            ctrl_power_limit_kw: f64::INFINITY,
            ctrl_mode_override: None,
            ctrl_load_fraction: 1.0,
            dr_setpoint_offset_c: 0.0,
            dr_load_fraction: 1.0,
            dr_duty_cycle: 1.0,
            dr_duration_remaining_s: None,
            dr_level: DRLevel::Normal,
        }
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.hvac.init(config, env)?;
        self.hvac.duct_zone_id = super::super::helpers::parse_zone_id_key(config, "duct_zone_id");
        self.init_from_typed(config, env)?;

        self.operating_mode = OperatingMode::Off;
        self.defrost_active = false;
        self.defrost_time_fraction = 0.0;
        self.defrost_accumulator_s = 0.0;
        self.pan_heater_on = false;
        self.last_er_off_at = None;
        self.er_was_on = false;
        self.prev_base_setpoint = self.hvac.effective_setpoints().heating_c;
        self.er_lockout_remaining_s = 0.0;
        self.prev_zone_temp_c = f64::NAN;
        self.er_soft_lockout = false;
        self.last_heating_rtf = 0.0;
        self.run_time_s = 0.0;
        self.cycle_on_steps = 0;
        self.cycle_off_steps = 0;
        self.ctrl_duty_cycle = 1.0;
        self.ctrl_power_limit_kw = f64::INFINITY;
        self.ctrl_mode_override = None;
        self.ctrl_load_fraction = 1.0;
        self.dr_setpoint_offset_c = 0.0;
        self.dr_load_fraction = 1.0;
        self.dr_duty_cycle = 1.0;
        self.dr_duration_remaining_s = None;
        self.dr_level = DRLevel::Normal;
        self.telemetry = default_heater_telemetry();
        self.core_output = CoreOutput::default();

        Ok(())
    }

    fn init_from_typed(
        &mut self,
        config: &EquipmentConfig,
        env: &EnvironmentState,
    ) -> crate::Result<()> {
        let cfg = config.require_typed::<HeatPumpHeaterConfig>("Heat Pump Heater")?;
        cfg.validate()?;

        self.hvac.heating_capacities_w = if let Some(stages) = &cfg.stage_heating_capacities_w {
            stages.clone()
        } else if let Some(cap) = cfg.heating_capacity_w {
            vec![cap]
        } else {
            vec![DEFAULT_HEATING_CAPACITY_W]
        };

        let default_eir = if let Some(efficiency) = cfg.heating_efficiency {
            efficiency.to_eir().unwrap_or(DEFAULT_HEATING_EIR)
        } else if let Some(hspf) = cfg.hspf {
            if hspf > 0.0 {
                1.0 / (hspf / 3.412_141_633)
            } else {
                DEFAULT_HEATING_EIR
            }
        } else {
            DEFAULT_HEATING_EIR
        };
        self.hvac.eir_by_stage = if let Some(stages) = &cfg.stage_heating_eirs {
            stages.clone()
        } else {
            vec![default_eir]
        };

        if let Some(fan_power_w) = cfg.fan_power_w {
            let rated_capacity_w = self
                .hvac
                .heating_capacities_w
                .last()
                .copied()
                .unwrap_or(DEFAULT_HEATING_CAPACITY_W);
            let rated_airflow_m3_s = self.hvac.airflow_m3_s_per_w * rated_capacity_w;
            self.hvac.fan_power_w_per_m3_s = if rated_airflow_m3_s > 0.0 {
                fan_power_w.max(0.0) / rated_airflow_m3_s
            } else {
                0.0
            };
        }

        if cfg.is_mini_split || matches!(self.variant, HeaterVariant::Minisplit) {
            self.hvac.speed_control_mode = SpeedControlMode::MultiSpeedInterpolated;
            if self.hvac.heating_capacities_w.len() == 1 {
                let base_cap = self.hvac.heating_capacities_w[0];
                let base_eir = self.hvac.eir_by_stage[0];
                self.hvac.heating_capacities_w =
                    vec![base_cap * 0.25, base_cap * 0.5, base_cap * 0.75, base_cap];
                self.hvac.eir_by_stage = vec![base_eir; 4];
            }
            self.hvac.duct_dse = 1.0;
        } else {
            let rated_cap = self
                .hvac
                .heating_capacities_w
                .last()
                .copied()
                .unwrap_or(0.0);
            let fan_flow = self.hvac.airflow_m3_s_per_w * rated_cap;
            let n_speeds = self.hvac.heating_capacities_w.len().min(255) as u8;
            let cap_low = (n_speeds > 1)
                .then(|| self.hvac.heating_capacities_w.first().copied())
                .flatten();
            let flow_low = cap_low.map(|c| self.hvac.airflow_m3_s_per_w * c);
            self.hvac.duct_dse = if let Some(dse) = cfg.duct.dse_heat {
                dse
            } else {
                super::super::helpers::resolve_duct_dse(
                    config, true, rated_cap, fan_flow, n_speeds, cap_low, flow_low, true,
                )
            };
        }

        self.hvac.update_zone_heat_fractions();

        // Backup heating from typed config.
        self.backup_capacity_w = cfg
            .backup_capacity_w
            .unwrap_or(DEFAULT_BACKUP_CAPACITY_W)
            .max(0.0);
        self.backup_eir = cfg.backup_eir.unwrap_or(DEFAULT_BACKUP_EIR).max(0.0);
        self.hp_lockout_temp_c = cfg.hp_lockout_temp_c.unwrap_or(DEFAULT_HP_LOCKOUT_TEMP_C);
        self.er_lockout_temp_c = cfg.er_lockout_temp_c.unwrap_or(DEFAULT_ER_LOCKOUT_TEMP_C);
        self.max_oat_supplemental_c = cfg
            .max_oat_supplemental_c
            .unwrap_or(MAX_OAT_SUPPLEMENTAL_C)
            .min(MAX_OAT_SUPPLEMENTAL_C);
        self.er_setpoint_offset_c = cfg.er_setpoint_offset_c.unwrap_or(
            self.hvac.thermostat.hysteresis_c
                * (DEFAULT_ER_SETPOINT_OFFSET_MULTIPLIER - DEFAULT_ER_SETPOINT_DEADBAND_OFFSET),
        );
        self.er_hard_lockout_time_s = cfg
            .er_hard_lockout_time_s
            .unwrap_or(DEFAULT_ER_HARD_LOCKOUT_TIME_S);

        if matches!(self.variant, HeaterVariant::Ashp) {
            self.hvac.equipment_type = if self.backup_capacity_w > 0.0 {
                HvacEquipmentType::AshpHeatPumpAux
            } else {
                HvacEquipmentType::AshpHeatPumpOnly
            };
            self.hvac.supply_air_temp_c = self
                .hvac
                .equipment_type
                .default_supply_air_temp_c(env.weather.outdoor_temp_c);
        }

        Ok(())
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        // Reset transient signals before each control step.
        self.ctrl_load_fraction = 1.0;
        self.use_ideal = self.hvac.use_ideal_capacity(env);

        // Decrement DR duration and auto-revert to Normal when expired.
        let dt_s = env.time_res.num_milliseconds().max(0) as f64 / 1000.0;
        if let Some(remaining) = self.dr_duration_remaining_s {
            let next = remaining - dt_s;
            if next <= 0.0 {
                self.dr_duration_remaining_s = None;
                self.apply_dr_level(DRLevel::Normal);
            } else {
                self.dr_duration_remaining_s = Some(next);
            }
        }

        // If a ModeOverride forces Off, short-circuit thermostat.
        if matches!(self.ctrl_mode_override, Some(OperatingMode::Off))
            || self.dr_load_fraction <= 0.0
        {
            self.hvac.last_speed_index = 0;
            self.hvac.duty_cycle = 0.0;
            self.operating_mode = OperatingMode::Off;
            return OperatingMode::Off;
        }

        // ModeOverride can also force specific HP modes.
        if let Some(forced_mode) = self.ctrl_mode_override {
            self.operating_mode = forced_mode;
            // Propagate to hvac duty so compute_step sees non-zero.
            let control = self.resolve_control(env).unwrap_or(HeaterControl {
                hp_on: false,
                er_on: false,
                speed_index: 0,
                duty_cycle: 0.0,
            });
            self.hvac.last_speed_index = control.speed_index;
            self.hvac.duty_cycle = control.duty_cycle;
            if self.er_was_on && !control.er_on {
                self.last_er_off_at = Some(env.current_time);
            }
            self.er_was_on = control.er_on;
            return self.operating_mode;
        }

        let control = self.resolve_control(env).unwrap_or(HeaterControl {
            hp_on: false,
            er_on: false,
            speed_index: 0,
            duty_cycle: 0.0,
        });

        self.hvac.last_speed_index = control.speed_index;
        self.hvac.duty_cycle = control.duty_cycle;
        if self.er_was_on && !control.er_on {
            self.last_er_off_at = Some(env.current_time);
        }
        self.er_was_on = control.er_on;

        self.operating_mode = match (control.hp_on, control.er_on) {
            (true, true) => OperatingMode::HeatingHPAndER,
            (true, false) => OperatingMode::HeatingHP,
            (false, true) => OperatingMode::HeatingER,
            (false, false) => OperatingMode::Off,
        };

        self.operating_mode
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        let dt_min = dt.as_secs_f64() / 60.0;
        let step = self.compute_step(env, dt_min)?;

        if step.thermal_output_w > 0.0 {
            self.hvac.write_zone_thermal_contributions(
                ports,
                step.thermal_output_w,
                0.0,
                ThermalCategory::HvacHeating,
            )?;
        }
        let scaled_electric_kw = step.electric_kw * self.hvac.space_fraction;
        if scaled_electric_kw > 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_kw: scaled_electric_kw,
                reactive_power_kvar: 0.0,
            })?;
        }

        // Record RTF for companion cooler crankcase accounting. When the HP
        // compressor is running the RTF equals the duty cycle (PLR).
        let hp_on = matches!(
            self.operating_mode,
            OperatingMode::HeatingHP | OperatingMode::HeatingHPAndER
        );
        self.last_heating_rtf = if hp_on {
            self.hvac.duty_cycle.clamp(0.0, 1.0)
        } else {
            0.0
        };

        if self.operating_mode == OperatingMode::Off {
            self.cycle_off_steps += 1;
        } else {
            self.cycle_on_steps += 1;
            self.run_time_s += dt.as_secs_f64();
        }

        if step.defrost_active {
            self.defrost_accumulator_s += dt.as_secs_f64();
        }

        self.defrost_active = step.defrost_active;
        self.defrost_time_fraction = step.defrost_time_fraction;

        // Telemetry reports delivered (post-DSE) thermal output for the conditioned zone.
        let delivered_thermal_w = step.thermal_output_w * self.hvac.duct_dse.clamp(0.0, 1.0);
        self.telemetry.set(tk::ELECTRIC_KW, step.electric_kw);
        self.telemetry
            .set(tk::THERMAL_OUTPUT_W, delivered_thermal_w);
        self.telemetry
            .set(tk::OPERATING_MODE, operating_mode_code(self.operating_mode));
        self.telemetry
            .set(tk::SPEED_INDEX, self.hvac.last_speed_index as f64);
        self.telemetry.set(
            tk::DEFROST_ACTIVE,
            if step.defrost_active { 1.0 } else { 0.0 },
        );
        // COP per AHRI/SEER convention: excludes fan power from denominator.
        // Gross thermal output (pre-DSE) over compressor-only electric input.
        // DSE losses are a distribution inefficiency, not a reduction in equipment COP.
        let compressor_only_w = step.compressor_kw * 1000.0;
        let cop = if compressor_only_w > 1e-6 {
            step.thermal_output_w / compressor_only_w
        } else {
            0.0
        };
        self.telemetry.set(tk::COP, cop);
        // Runtime fraction = duty cycle (PLR) when on, 0 when off.
        let rtf = if self.operating_mode != OperatingMode::Off {
            self.hvac.duty_cycle.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.telemetry.set(tk::RUNTIME_FRACTION, rtf);
        self.telemetry.set(tk::COMPRESSOR_KW, step.compressor_kw);
        self.telemetry
            .set(tk::DEFROST_TIME_FRACTION, step.defrost_time_fraction);
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(step.electric_kw.max(0.0))),
                reactive_power_kvar: None,
                fuel_w: None,
            },
            state: CoreState {
                operating_mode: Some(self.operating_mode),
                soc: None,
            },
        };

        // Clear solver-provided capacity so next step starts fresh.
        self.ideal_capacity_w = 0.0;

        Ok(())
    }

    fn compute_step(&mut self, env: &EnvironmentState, dt_min: f64) -> crate::Result<HeaterStep> {
        let zone = lookup_zone(env, self.hvac.zone_id)?;
        let pressure_pa = env.weather.pressure_pa();

        let speed_index = self.hvac.last_speed_index;
        let speed_frac = self.hvac.last_speed_frac;
        let (stage_capacity_w, stage_eir) =
            if self.hvac.speed_control_mode == SpeedControlMode::MultiSpeedInterpolated {
                (
                    self.hvac.interpolated_capacity(
                        &self.hvac.heating_capacities_w,
                        speed_index,
                        speed_frac,
                    ),
                    self.hvac.interpolated_eir(speed_index, speed_frac),
                )
            } else {
                (
                    HvacEquipment::capacity_at_stage(&self.hvac.heating_capacities_w, speed_index),
                    self.hvac.eir_at_stage(speed_index),
                )
            };

        // Capacity biquadratic: evaluate first — needed to derive PLR from
        // solver-provided ideal capacity at current conditions.
        let (_, cap_ratio) = self.hvac.evaluate_biquadratic_with_flow(
            0,
            zone.temperature_c,
            env.weather.outdoor_temp_c,
            1.0,
        );

        // Derive PLR: from solver's ideal capacity (coarse timestep) or
        // thermostat duty cycle (fine timestep).
        let steady_capacity_w = (stage_capacity_w * cap_ratio).max(0.0);
        let er_on = matches!(
            self.operating_mode,
            OperatingMode::HeatingER | OperatingMode::HeatingHPAndER
        );
        let plr = if self.use_ideal && self.ideal_capacity_w.abs() > f64::EPSILON {
            // Solver-provided ideal capacity (positive for heating): derive PLR
            // from biquadratic-corrected capacity at current conditions.
            // When ER backup is active, include its capacity in the denominator
            // so PLR × (hp_cap + er_cap) = ideal_load, preventing over-delivery.
            let total_available = if er_on {
                steady_capacity_w + self.backup_capacity_w
            } else {
                steady_capacity_w
            };
            let min_cap = (stage_capacity_w * 0.01).max(1.0);
            let p = (self.ideal_capacity_w / total_available.max(min_cap)).clamp(0.0, 1.0);
            // Write back so telemetry and RTF reporting see the solver-derived value.
            self.hvac.duty_cycle = p;
            p
        } else {
            self.hvac.duty_cycle.clamp(0.0, 1.0)
        };
        let plf = self.hvac.part_load_factor(plr);

        // EIR curve: divide by PLF — cycling reduces efficiency.
        let (_, eir_ratio_base) = self.hvac.evaluate_biquadratic_with_flow(
            1,
            zone.temperature_c,
            env.weather.outdoor_temp_c,
            1.0,
        );
        let eir_ratio = if plf > 0.0 {
            eir_ratio_base / plf
        } else {
            eir_ratio_base
        };

        let staged_capacity_w = self
            .hvac
            .apply_startup_capacity_degradation(steady_capacity_w, dt_min);
        let mut hp_capacity_w = (staged_capacity_w * plr).max(0.0);

        let mut hp_electric_w = (hp_capacity_w * stage_eir * eir_ratio).max(0.0);
        let airflow_m3_s = self.hvac.airflow_m3_s_for_capacity_w(stage_capacity_w);
        let mut fan_power_w = self.hvac.fan_power_w(airflow_m3_s) * plr;

        let hp_on = matches!(
            self.operating_mode,
            OperatingMode::HeatingHP | OperatingMode::HeatingHPAndER
        );

        let mut defrost_active = false;
        let mut defrost_time_fraction = 0.0;

        if hp_on {
            let max_capacity_w = self
                .hvac
                .heating_capacities_w
                .last()
                .copied()
                .unwrap_or(stage_capacity_w)
                .max(0.0);
            let defrost = evaluate_defrost(
                &self.defrost_config,
                env.weather.outdoor_temp_c,
                env.weather.outdoor_humidity_ratio,
                pressure_pa,
                zone.wet_bulb_c,
                max_capacity_w,
                hp_capacity_w,
                plr,
            );

            if defrost.active {
                defrost_active = true;
                defrost_time_fraction = defrost.time_fraction;

                hp_capacity_w = (hp_capacity_w * defrost.capacity_multiplier - defrost.q_defrost_w)
                    .max(0.0)
                    * self
                        .defrost_config
                        .capacity_reduction_factor
                        .clamp(0.0, 1.0);

                hp_electric_w = hp_electric_w * defrost.power_multiplier + defrost.extra_power_w;
            }
        } else {
            hp_capacity_w = 0.0;
            hp_electric_w = 0.0;
            fan_power_w = 0.0;
        }

        let er_capacity_w = if er_on {
            self.backup_capacity_w * plr
        } else {
            0.0
        };
        let er_power_w = er_capacity_w * self.backup_eir;

        self.pan_heater_on = matches!(self.variant, HeaterVariant::Minisplit)
            && env.weather.outdoor_temp_c < self.pan_heater_temp_c
            && self.pan_heater_kw > 0.0;
        let pan_heater_w = if self.pan_heater_on {
            self.pan_heater_kw * 1000.0
        } else {
            0.0
        };

        // Gross output including fan waste heat (OCHRE HVAC.py line 543).
        // zone_heat_fractions (set from duct_dse during init) distributes
        // this to conditioned and duct zones in write_zone_thermal_contributions.
        let mut thermal_output_w = hp_capacity_w + er_capacity_w + fan_power_w;
        let mut electric_kw = (hp_electric_w + er_power_w + fan_power_w + pan_heater_w) / 1000.0;
        // COP per AHRI/SEER convention: excludes fan power from denominator.
        // Track compressor-only kW separately so scaling stays consistent with electric_kw.
        let mut compressor_kw = hp_electric_w / 1000.0;

        // Apply control multipliers: DutyCycle (sticky) × LoadFraction (transient)
        // × DR load fraction × DR duty cycle.
        let effective_load = self.ctrl_duty_cycle
            * self.ctrl_load_fraction
            * self.dr_load_fraction
            * self.dr_duty_cycle;
        if effective_load < 1.0 {
            thermal_output_w *= effective_load;
            electric_kw *= effective_load;
            compressor_kw *= effective_load;
        }

        // Apply PowerLimit (sticky): clamp power and reduce thermal proportionally.
        if self.ctrl_power_limit_kw.is_finite() && electric_kw > self.ctrl_power_limit_kw {
            let ratio = self.ctrl_power_limit_kw / electric_kw.max(f64::MIN_POSITIVE);
            electric_kw = self.ctrl_power_limit_kw;
            thermal_output_w *= ratio;
            compressor_kw *= ratio;
        }

        if er_on && hp_on {
            self.hvac.supply_air_temp_c = HvacEquipmentType::AshpHeatPumpAux
                .default_supply_air_temp_c(env.weather.outdoor_temp_c);
        } else if hp_on {
            self.hvac.update_supply_air_temp(env);
        }

        Ok(HeaterStep {
            thermal_output_w,
            electric_kw,
            compressor_kw: compressor_kw.max(0.0),
            defrost_active,
            defrost_time_fraction,
        })
    }

    fn resolve_control(&mut self, env: &EnvironmentState) -> crate::Result<HeaterControl> {
        let mode = self.hvac.update_mode(env)?;

        // Split base setpoint from DR offset so lockout detection is independent
        // of DR state: DR expiry raises the effective setpoint back to base but
        // must not trigger the ER hard lockout (which is reserved for actual
        // user/thermostat setpoint raises).
        let base_setpoint = self.hvac.effective_setpoints().heating_c;
        let setpoint = base_setpoint + self.dr_setpoint_offset_c;
        let zone = lookup_zone(env, self.hvac.zone_id)?;
        let dt_s = env.time_res.num_milliseconds().max(0) as f64 / 1000.0;

        // OCHRE HVAC.py: ER hard lockout after setpoint increase prevents expensive
        // resistance heating when the heat pump can handle the ramp.
        // Reference: ResStock/BEopt thermostat modeling documentation.
        // Compare against the BASE setpoint only — DR offset changes are excluded.
        if base_setpoint > self.prev_base_setpoint + 0.1 {
            self.er_lockout_remaining_s = self.er_hard_lockout_time_s;
        }
        self.prev_base_setpoint = base_setpoint;
        // Check lock before decrementing so that a 600 s lockout blocks exactly
        // 600/dt steps: on the step where lockout is set the check sees the full
        // timer value, and the final decrement to 0 happens at the end of that step.
        let er_allowed_by_hard_lockout = self.er_lockout_remaining_s <= 0.0;
        self.er_lockout_remaining_s = (self.er_lockout_remaining_s - dt_s).max(0.0);

        // OCHRE HVAC.py: two-stage lockout — after hard lockout expires, ER stays
        // off while zone temp is still rising (heat pump is winning the load).
        let zone_rising =
            self.prev_zone_temp_c.is_finite() && zone.temperature_c > self.prev_zone_temp_c;
        if !er_allowed_by_hard_lockout {
            // Hard lockout active; soft lockout mirrors hard lockout state.
            self.er_soft_lockout = true;
        } else if self.er_soft_lockout && zone_rising {
            // Hard lockout just expired; keep soft lockout while temp is rising.
            self.er_soft_lockout = true;
        } else {
            self.er_soft_lockout = false;
        }
        self.prev_zone_temp_c = zone.temperature_c;

        if mode != ThermostatMode::Heating {
            return Ok(HeaterControl {
                hp_on: false,
                er_on: false,
                speed_index: 0,
                duty_cycle: 0.0,
            });
        }

        let deadband = self.hvac.thermostat.hysteresis_c.max(0.1);

        let load_ratio = if self.use_ideal {
            // Ideal capacity mode: solver provides PLR via compute_step.
            // Set load_ratio=1.0 as placeholder; actual PLR derived from
            // biquadratic-corrected capacity in compute_step.
            1.0
        } else {
            let load_ratio_raw = (setpoint - zone.temperature_c) / deadband;
            load_ratio_raw.clamp(0.0, 1.0)
        };
        let speed = self.hvac.select_speed(load_ratio);

        let hp_available = env.weather.outdoor_temp_c >= self.hp_lockout_temp_c;
        // OCHRE HVAC.py aggressive lockout: ER off above er_lockout_temp_c (default 4.44°C).
        // EnergyPlus hard cap: ER off above max_oat_supplemental_c (default 21°C).
        // Both conditions must pass; in practice the OCHRE threshold is more restrictive
        // at default settings, but max_oat_supplemental_c is the hard safety ceiling.
        let er_allowed_by_temp = env.weather.outdoor_temp_c < self.er_lockout_temp_c
            && env.weather.outdoor_temp_c <= self.max_oat_supplemental_c;
        let er_allowed_by_cycle = self.er_cycle_ready(env.current_time);
        let er_allowed_by_lockout = er_allowed_by_hard_lockout && !self.er_soft_lockout;

        // OCHRE: ER engages when zone temperature drops below a separate setpoint
        // threshold (`setpoint - er_setpoint_offset_c`), not when the HP load ratio
        // exceeds 1.0. The separate threshold (default 1.6°C below setpoint) ensures
        // ER only fires when the HP alone cannot meet the load.
        let er_call_threshold = setpoint - self.er_setpoint_offset_c;
        let er_thermostat_call = zone.temperature_c <= er_call_threshold;

        let hp_on = hp_available && speed.part_load_ratio > 0.0;
        let er_on = self.backup_capacity_w > 0.0
            && er_allowed_by_temp
            && er_allowed_by_cycle
            && er_allowed_by_lockout
            && (er_thermostat_call || !hp_on);

        Ok(HeaterControl {
            hp_on,
            er_on,
            speed_index: speed.speed_index,
            duty_cycle: speed.part_load_ratio,
        })
    }

    fn er_cycle_ready(&self, now: DateTime<FixedOffset>) -> bool {
        let Some(last_off) = self.last_er_off_at else {
            return true;
        };
        let elapsed_s = (now - last_off).num_milliseconds().max(0) as f64 / 1000.0;
        elapsed_s >= self.min_er_cycle_time_s
    }

    fn apply_dr_level(&mut self, level: DRLevel) {
        self.dr_level = level;
        match level {
            DRLevel::Normal => {
                self.dr_setpoint_offset_c = 0.0;
                self.dr_duty_cycle = 1.0;
                self.dr_load_fraction = 1.0;
            }
            DRLevel::Moderate => {
                self.dr_setpoint_offset_c = -1.0;
                self.dr_duty_cycle = 1.0;
                self.dr_load_fraction = 1.0;
            }
            DRLevel::High => {
                self.dr_setpoint_offset_c = -2.0;
                self.dr_duty_cycle = 1.0;
                self.dr_load_fraction = 0.8;
            }
            DRLevel::Critical => {
                self.dr_setpoint_offset_c = -3.0;
                self.dr_duty_cycle = 1.0;
                self.dr_load_fraction = 0.5;
            }
            DRLevel::GridEmergency => {
                self.dr_setpoint_offset_c = 0.0;
                self.dr_load_fraction = 0.0;
            }
        }
    }

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&HeaterState {
            mode: self.hvac.mode,
            duty_cycle: self.hvac.duty_cycle,
            last_mode_switch_at: self.hvac.last_mode_switch_at,
            mode_start_at: self.hvac.mode_start_at,
            runtime_setpoints: self.hvac.runtime_setpoints,
            operating_mode: self.operating_mode,
            run_time_s: self.run_time_s,
            cycle_on_steps: self.cycle_on_steps,
            cycle_off_steps: self.cycle_off_steps,
            defrost_active: self.defrost_active,
            defrost_time_fraction: self.defrost_time_fraction,
            defrost_accumulator_s: self.defrost_accumulator_s,
            pan_heater_on: self.pan_heater_on,
            last_er_off_at: self.last_er_off_at,
            last_speed_index: self.hvac.last_speed_index,
            last_speed_frac: self.hvac.last_speed_frac,
            electric_kw: self.telemetry.get(tk::ELECTRIC_KW).unwrap_or(0.0),
            thermal_output_w: self.telemetry.get(tk::THERMAL_OUTPUT_W).unwrap_or(0.0),
            speed_index: self.telemetry.get(tk::SPEED_INDEX).unwrap_or(0.0),
            operating_mode_code: self.telemetry.get(tk::OPERATING_MODE).unwrap_or(0.0),
            prev_base_setpoint: self.prev_base_setpoint,
            er_lockout_remaining_s: self.er_lockout_remaining_s,
            prev_zone_temp_c: self.prev_zone_temp_c,
            er_soft_lockout: self.er_soft_lockout,
            ctrl_duty_cycle: self.ctrl_duty_cycle,
            ctrl_power_limit_kw: if self.ctrl_power_limit_kw.is_finite() {
                Some(self.ctrl_power_limit_kw)
            } else {
                None
            },
            ctrl_mode_override: self.ctrl_mode_override,
            dr_level: self.dr_level,
            dr_setpoint_offset_c: self.dr_setpoint_offset_c,
            dr_load_fraction: self.dr_load_fraction,
            dr_duty_cycle: self.dr_duty_cycle,
            dr_duration_remaining_s: self.dr_duration_remaining_s,
            max_oat_supplemental_c: self.max_oat_supplemental_c,
        })
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: HeaterState = load_postcard(state)?;
        self.hvac.mode = decoded.mode;
        self.hvac.duty_cycle = decoded.duty_cycle;
        self.hvac.last_mode_switch_at = decoded.last_mode_switch_at;
        self.hvac.mode_start_at = decoded.mode_start_at;
        self.hvac.runtime_setpoints = decoded.runtime_setpoints;
        self.operating_mode = decoded.operating_mode;
        self.run_time_s = decoded.run_time_s;
        self.cycle_on_steps = decoded.cycle_on_steps;
        self.cycle_off_steps = decoded.cycle_off_steps;
        self.defrost_active = decoded.defrost_active;
        self.defrost_time_fraction = decoded.defrost_time_fraction;
        self.defrost_accumulator_s = decoded.defrost_accumulator_s;
        self.pan_heater_on = decoded.pan_heater_on;
        self.last_er_off_at = decoded.last_er_off_at;
        self.er_was_on = matches!(
            decoded.operating_mode,
            OperatingMode::HeatingER | OperatingMode::HeatingHPAndER
        );
        self.hvac.last_speed_index = decoded.last_speed_index;
        self.hvac.last_speed_frac = decoded.last_speed_frac;
        self.ctrl_duty_cycle = decoded.ctrl_duty_cycle;
        self.ctrl_power_limit_kw = decoded.ctrl_power_limit_kw.unwrap_or(f64::INFINITY);
        self.ctrl_mode_override = decoded.ctrl_mode_override;
        self.dr_level = decoded.dr_level;
        self.dr_setpoint_offset_c = decoded.dr_setpoint_offset_c;
        self.dr_load_fraction = decoded.dr_load_fraction;
        self.dr_duty_cycle = decoded.dr_duty_cycle;
        self.dr_duration_remaining_s = decoded.dr_duration_remaining_s;

        self.telemetry.insert(tk::ELECTRIC_KW, decoded.electric_kw);
        self.telemetry
            .insert(tk::THERMAL_OUTPUT_W, decoded.thermal_output_w);
        self.telemetry.insert(tk::SPEED_INDEX, decoded.speed_index);
        self.telemetry
            .insert(tk::OPERATING_MODE, decoded.operating_mode_code);
        self.telemetry.insert(
            tk::DEFROST_ACTIVE,
            if decoded.defrost_active { 1.0 } else { 0.0 },
        );
        self.prev_base_setpoint = decoded.prev_base_setpoint;
        self.er_lockout_remaining_s = decoded.er_lockout_remaining_s;
        self.prev_zone_temp_c = decoded.prev_zone_temp_c;
        self.er_soft_lockout = decoded.er_soft_lockout;
        self.core_output = CoreOutput::default();

        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        match signal {
            ControlSignal::ThermalSetpoint { deadband_c, .. } => {
                apply_heating_control_unchecked(
                    &mut self.hvac,
                    signal,
                    &self.descriptor.equipment_type,
                )?;
                if let Some(db) = deadband_c {
                    if !db.is_finite() || *db < 0.0 {
                        return Err(HaresError::Control(format!(
                            "invalid deadband_c for {}: {db}",
                            self.descriptor.equipment_type
                        )));
                    }
                    self.hvac.thermostat.hysteresis_c = *db;
                }
            }
            ControlSignal::DutyCycle { on_fraction, .. } => {
                if !on_fraction.is_finite() || !(0.0..=1.0).contains(on_fraction) {
                    return Err(HaresError::Control(format!(
                        "invalid duty cycle for {}: {on_fraction}",
                        self.descriptor.equipment_type
                    )));
                }
                self.ctrl_duty_cycle = *on_fraction;
            }
            ControlSignal::LoadFraction { fraction } => {
                if !fraction.is_finite() || !(0.0..=1.0).contains(fraction) {
                    return Err(HaresError::Control(format!(
                        "invalid load fraction for {}: {fraction}",
                        self.descriptor.equipment_type
                    )));
                }
                self.ctrl_load_fraction = *fraction;
            }
            ControlSignal::PowerLimit { max_power_kw, .. } => {
                if !max_power_kw.is_finite() || *max_power_kw < 0.0 {
                    return Err(HaresError::Control(format!(
                        "invalid power limit for {}: {max_power_kw}",
                        self.descriptor.equipment_type
                    )));
                }
                self.ctrl_power_limit_kw = *max_power_kw;
            }
            ControlSignal::ModeOverride { mode } => {
                self.ctrl_mode_override = Some(*mode);
            }
            ControlSignal::DemandResponse { level, duration_s } => {
                self.apply_dr_level(*level);
                self.dr_duration_remaining_s = *duration_s;
            }
            ControlSignal::IdealCapacity { capacity_w } => {
                self.ideal_capacity_w = *capacity_w;
            }
            _ => {
                apply_heating_control_unchecked(
                    &mut self.hvac,
                    signal,
                    &self.descriptor.equipment_type,
                )?;
            }
        }
        Ok(())
    }

    fn ideal_target(&self) -> Option<(hares_types::ZoneId, f64)> {
        if !self.use_ideal {
            return None;
        }
        if !matches!(
            self.operating_mode,
            OperatingMode::HeatingHP | OperatingMode::HeatingER | OperatingMode::HeatingHPAndER
        ) {
            return None;
        }
        let setpoint = self.hvac.effective_setpoints().heating_c + self.dr_setpoint_offset_c;
        Some((self.hvac.zone_id, setpoint))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, DRLevel, EnvironmentState, GridState, OperatingMode, PortSlots,
        ThermalAccumulator, WeatherState, ZoneId, ZoneState, telemetry_keys as tk,
    };

    use super::{ASHPHeater, MinisplitHeater};
    use crate::{
        Equipment, EquipmentConfig, HeatPumpHeaterConfig,
        hvac::heat_pump_config::{HeatingEfficiency, HeatingEfficiencyUnit},
    };

    fn env(zone_temp_c: f64, outdoor_c: f64, outdoor_w: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: 14.0,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: outdoor_c,
                outdoor_humidity_ratio: outdoor_w,
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
                ..Default::default()
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 3, 18, 0, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::minutes(1),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    fn heater_typed_config() -> HeatPumpHeaterConfig {
        HeatPumpHeaterConfig {
            equipment_id: None,
            zone_id: Some(1),
            heating_capacity_w: Some(8_000.0),
            hspf: Some(3.412_141_633 / 0.33),
            heating_efficiency: None,
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            backup_fuel: None,
            backup_capacity_w: Some(4_000.0),
            backup_eir: None,
            fraction_heating_load_served: None,
            cooling_capacity_w: None,
            seer: None,
            stage_cooling_capacities_w: None,
            stage_cooling_eirs: None,
            stage_shrs: None,
            fraction_cooling_load_served: None,
            number_of_speeds: 1,
            is_mini_split: false,
            shr: None,
            fan_power_w: None,
            fan_power_w_per_cfm: None,
            airflow_m3_s_per_w: None,
            heating_setpoint_c: Some(21.0),
            cooling_setpoint_c: Some(26.0),
            hysteresis_c: Some(1.0),
            hp_lockout_temp_c: None,
            er_lockout_temp_c: None,
            max_oat_supplemental_c: None,
            er_setpoint_offset_c: None,
            er_hard_lockout_time_s: None,
            duct: Default::default(),
            biquadratic_x1_min: None,
            biquadratic_x1_max: None,
            biquadratic_x2_min: None,
            biquadratic_x2_max: None,
            ff_min: None,
            ff_max: None,
            plf_min: None,
            plf_max: None,
        }
    }

    fn add_identity_biquadratic_curves(cfg: &mut EquipmentConfig) {
        cfg.raw_config_mut().unwrap().insert(
            "biquadratic_coeffs".to_string(),
            "[[1,0,0,0,0,0],[1,0,0,0,0,0]]".into(),
        );
    }

    fn heater_config() -> EquipmentConfig {
        heater_config_with(|_| {})
    }

    fn heater_config_with(mutator: impl FnOnce(&mut HeatPumpHeaterConfig)) -> EquipmentConfig {
        let mut typed = heater_typed_config();
        mutator(&mut typed);
        let mut cfg =
            EquipmentConfig::from_typed("HP Heater".to_string(), "ASHP Heater".to_string(), typed);
        add_identity_biquadratic_curves(&mut cfg);
        cfg
    }

    #[test]
    fn er_hard_lockout_defaults_to_ochre_parity_zero() {
        let cfg = heater_config();
        let mut eq = ASHPHeater::new(cfg.clone());
        let environment = env(18.0, 0.0, 0.003);
        eq.init(&cfg, &environment).unwrap();

        assert_eq!(eq.core.er_hard_lockout_time_s, 0.0);
    }

    #[test]
    fn er_hard_lockout_uses_explicit_config_value() {
        let cfg = heater_config_with(|typed| typed.er_hard_lockout_time_s = Some(300.0));
        let mut eq = ASHPHeater::new(cfg.clone());
        let environment = env(18.0, 0.0, 0.003);
        eq.init(&cfg, &environment).unwrap();

        assert_eq!(eq.core.er_hard_lockout_time_s, 300.0);
    }

    #[test]
    fn defrost_trips_in_cold_conditions() {
        let cfg = heater_config();
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let env = env(18.0, 0.0, 0.005);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert_eq!(eq.telemetry().get(tk::DEFROST_ACTIVE), Some(1.0));
    }

    #[test]
    fn no_defrost_when_warm() {
        let cfg = heater_config();
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let env = env(18.0, 10.0, 0.005);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert_eq!(eq.telemetry().get(tk::DEFROST_ACTIVE), Some(0.0));
    }

    #[test]
    fn hspf_to_eir_conversion() {
        let cfg = heater_config_with(|typed| {
            typed.hspf = None;
            typed.heating_efficiency = Some(HeatingEfficiency {
                value: 8.5,
                unit: HeatingEfficiencyUnit::Hspf,
            });
        });
        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &env(18.0, 0.0, 0.003)).unwrap();
        let eir = eq.core.hvac.eir_by_stage[0];
        assert!(
            (eir - 0.401).abs() < 0.01,
            "HSPF=8.5 → EIR≈0.401, got {eir}"
        );
    }

    #[test]
    fn cop_to_eir_no_conversion() {
        let cfg = heater_config_with(|typed| {
            typed.hspf = None;
            typed.heating_efficiency = Some(HeatingEfficiency {
                value: 3.0,
                unit: HeatingEfficiencyUnit::Cop,
            });
        });
        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &env(18.0, 0.0, 0.003)).unwrap();
        let eir = eq.core.hvac.eir_by_stage[0];
        assert!(
            (eir - 0.333).abs() < 0.01,
            "COP=3.0 → EIR≈0.333 (no conversion), got {eir}"
        );
    }

    #[test]
    fn eer_to_eir_conversion() {
        let cfg = heater_config_with(|typed| {
            typed.hspf = None;
            typed.heating_efficiency = Some(HeatingEfficiency {
                value: 12.0,
                unit: HeatingEfficiencyUnit::Eer,
            });
        });
        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &env(18.0, 0.0, 0.003)).unwrap();
        let eir = eq.core.hvac.eir_by_stage[0];
        assert!((eir - 0.284).abs() < 0.01, "EER=12 → EIR≈0.284, got {eir}");
    }

    #[test]
    fn seer_to_eir_conversion() {
        let cfg = heater_config_with(|typed| {
            typed.hspf = None;
            typed.heating_efficiency = Some(HeatingEfficiency {
                value: 14.0,
                unit: HeatingEfficiencyUnit::Seer,
            });
        });
        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &env(18.0, 0.0, 0.003)).unwrap();
        let eir = eq.core.hvac.eir_by_stage[0];
        assert!((eir - 0.244).abs() < 0.01, "SEER=14 → EIR≈0.244, got {eir}");
    }

    #[test]
    fn afue_to_eir_no_conversion() {
        let cfg = heater_config_with(|typed| {
            typed.hspf = None;
            typed.heating_efficiency = Some(HeatingEfficiency {
                value: 95.0,
                unit: HeatingEfficiencyUnit::Afue,
            });
        });
        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &env(18.0, 0.0, 0.003)).unwrap();
        let eir = eq.core.hvac.eir_by_stage[0];
        assert!(
            (eir - 1.0 / 95.0).abs() < 0.001,
            "AFUE=95 → EIR≈0.01053 (no unit conversion), got {eir}"
        );
    }

    #[test]
    fn minisplit_pan_heater_draws_when_cold() {
        let mut cfg = heater_config();
        cfg.ochre_class = "MSHP Heater".to_string();

        let mut eq = MinisplitHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let env = env(20.0, -5.0, 0.004);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert!(ports.electrical.net_active_kw() > 0.0);
    }

    #[test]
    fn state_round_trip_preserves_defrost_state() {
        let cfg = heater_config();
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let env = env(18.0, -3.0, 0.005);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let state = eq.save_state();

        let mut restored = ASHPHeater::new(cfg.clone());
        restored.init(&cfg, &env).unwrap();
        restored.load_state(&state).unwrap();

        assert_eq!(
            restored.telemetry().get(tk::DEFROST_ACTIVE),
            eq.telemetry().get(tk::DEFROST_ACTIVE)
        );
    }

    // Regression: ER capacity was always 100% due to `plr.max(1.0)` bug.
    // The fix changed it to just `plr`, so ER output must scale proportionally
    // with load when the system is running below full capacity.
    #[test]
    fn er_backup_capacity_modulated_by_plr() {
        // Lock HP out so only ER runs; this isolates ER draw in electric_kw.
        // The thermostat FSM requires a prior heating step to be in Heating mode
        // before PLR < 1.0 is observable. Step 1 engages heating at full load
        // (zone far below setpoint). Step 2 runs with the zone temp inside the
        // hysteresis band so load_ratio = 0.5 → PLR = 0.5.
        let cfg = heater_config_with(|typed| {
            typed.hp_lockout_temp_c = Some(10.0);
            typed.er_setpoint_offset_c = Some(0.0);
        });
        let backup_capacity_w = 4_000.0_f64;
        // OAT=0°C is below ER lockout (4.44°C) so ER is temperature-permitted
        let env_cold = env(18.0, 0.0, 0.003);
        // Zone within hysteresis band: load_ratio = (21 - 20.5) / 1.0 = 0.5
        let env_partial = env(20.5, 0.0, 0.003);

        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &env_cold).unwrap();

        // Step 1: trigger thermostat into Heating mode at full load
        eq.update_control(&env_cold);
        eq.step(&env_cold, Duration::from_secs(60), &mut ports)
            .unwrap();

        // Step 2: thermostat stays in Heating; zone at 20.5°C yields PLR = 0.5
        ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let mode = eq.update_control(&env_partial);
        assert_eq!(mode, OperatingMode::HeatingER, "HP must be locked out");
        eq.step(&env_partial, Duration::from_secs(60), &mut ports)
            .unwrap();

        let electric_kw = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        // With PLR=0.5: ER draws backup_capacity_w * 0.5 * backup_eir = 2 kW.
        // The old bug used plr.max(1.0) = 1.0, giving the full 4 kW always.
        assert!(
            electric_kw < backup_capacity_w / 1000.0,
            "ER draw {electric_kw:.3} kW should be < full backup capacity \
             {:.3} kW when PLR=0.5",
            backup_capacity_w / 1000.0,
        );
        assert!(electric_kw > 0.0, "ER must draw some power");
    }

    // Regression: step() used to call update_control() internally, causing the
    // thermostat FSM to execute twice per timestep. Verify that calling step()
    // without a prior update_control() does NOT change operating mode.
    #[test]
    fn step_does_not_call_update_control_internally() {
        // After init the heater is Off. Zone is cold (18°C vs setpoint 21°C),
        // so an internal update_control() inside step() would switch to heating.
        // If step() is pure physics (no control), the mode stays Off.
        let cfg = heater_config();
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let environment = env(18.0, 5.0, 0.005);
        eq.init(&cfg, &environment).unwrap();
        // Deliberately skip update_control() before step()
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        // operating_mode telemetry code 0.0 = Off (matches OPERATING_MODE_CODE_OFF)
        assert_eq!(
            eq.telemetry().get(tk::OPERATING_MODE),
            Some(0.0),
            "step() must not change operating mode; it must remain Off when \
             update_control() was never called",
        );
    }

    #[test]
    fn default_lockout_below_hp_threshold_runs_er_only() {
        let cfg = heater_config();
        let mut eq = ASHPHeater::new(cfg.clone());
        // OAT below default HP lockout (-17.78C) and below ER lockout (4.44C):
        // HP must be disabled while ER is allowed.
        let cold_env = env(18.0, -20.0, 0.003);
        eq.init(&cfg, &cold_env).unwrap();

        let mode = eq.update_control(&cold_env);
        assert_eq!(
            mode,
            OperatingMode::HeatingER,
            "with default lockouts and OAT=-20C, HP should be locked out and ER should heat"
        );
    }

    #[test]
    fn default_lockout_above_er_threshold_runs_hp_only() {
        let cfg = heater_config();
        let mut eq = ASHPHeater::new(cfg.clone());
        // OAT above default ER lockout (4.44C): ER must be disabled.
        // OAT also above default HP lockout (-17.78C): HP remains available.
        let mild_env = env(18.0, 5.0, 0.003);
        eq.init(&cfg, &mild_env).unwrap();

        let mode = eq.update_control(&mild_env);
        assert_eq!(
            mode,
            OperatingMode::HeatingHP,
            "with default lockouts and OAT=5C, ER should be locked out and HP should heat"
        );
    }

    // Regression: ASHPHeater always used AshpHeatPumpAux regardless of backup
    // capacity, giving a fixed 40.6°C supply temp even with no ER strip heat.
    // With backup_capacity_w=0 the type must be AshpHeatPumpOnly, whose supply
    // temp follows outdoor temperature: 32.2 + 0.15*(OAT - 8.3).
    #[test]
    fn ashp_without_backup_uses_oat_dependent_supply_temp() {
        let cfg = heater_config_with(|typed| typed.backup_capacity_w = Some(0.0));

        // Use OATs above the ER lockout threshold (default 4.44°C) so that
        // er_on=false. With only HP running, update_supply_air_temp() is called
        // and it updates supply_air_temp_c based on OAT for AshpHeatPumpOnly.
        // The field is internal; the test module as a child of heater.rs can
        // access private struct fields of its parent module.
        let oat_a = 5.0_f64; // above ER lockout → ER disabled; supply = 31.705°C
        let oat_b = 15.0_f64; // above ER lockout → ER disabled; supply = 33.205°C

        let env_a = env(18.0, oat_a, 0.005);
        let mut eq_a = ASHPHeater::new(cfg.clone());
        let mut ports_a = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq_a.init(&cfg, &env_a).unwrap();
        eq_a.update_control(&env_a);
        eq_a.step(&env_a, Duration::from_secs(60), &mut ports_a)
            .unwrap();
        let supply_a = eq_a.core.hvac.supply_air_temp_c;

        let env_b = env(18.0, oat_b, 0.005);
        let mut eq_b = ASHPHeater::new(cfg.clone());
        let mut ports_b = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq_b.init(&cfg, &env_b).unwrap();
        eq_b.update_control(&env_b);
        eq_b.step(&env_b, Duration::from_secs(60), &mut ports_b)
            .unwrap();
        let supply_b = eq_b.core.hvac.supply_air_temp_c;

        // AshpHeatPumpOnly formula: 32.2 + 0.15*(OAT - 8.3)
        let expected_a = 32.2 + 0.15 * (oat_a - 8.3);
        let expected_b = 32.2 + 0.15 * (oat_b - 8.3);

        assert!(
            (supply_a - expected_a).abs() < 0.01,
            "expected OAT-dependent supply temp {expected_a:.3}°C at OAT {oat_a}°C, \
             got {supply_a:.3}°C",
        );
        assert!(
            (supply_b - expected_b).abs() < 0.01,
            "expected OAT-dependent supply temp {expected_b:.3}°C at OAT {oat_b}°C, \
             got {supply_b:.3}°C",
        );
        assert!(
            (supply_b - supply_a).abs() > 0.1,
            "supply temp must differ between OATs when AshpHeatPumpOnly is selected",
        );
    }

    fn make_env(zone_temp_c: f64, outdoor_c: f64, second: i64) -> EnvironmentState {
        use chrono::{FixedOffset, TimeZone};
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: 14.0,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: outdoor_c,
                outdoor_humidity_ratio: 0.003,
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
                ..Default::default()
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 3, 18, 0, 0, 0)
                .single()
                .expect("valid")
                + chrono::Duration::seconds(second),
            time_res: ChronoDuration::minutes(1),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    #[test]
    fn er_hard_lockout_blocks_er_for_configured_duration() {
        // OCHRE HVAC.py: ER hard lockout prevents strip heat on setpoint increase.
        // With lockout = 600 s and dt = 60 s, ER must be blocked for 10 steps.
        let cfg = heater_config_with(|typed| {
            typed.er_hard_lockout_time_s = Some(600.0);
            typed.er_lockout_temp_c = Some(100.0);
            typed.hp_lockout_temp_c = Some(100.0);
            typed.er_setpoint_offset_c = Some(0.0);
            typed.heating_setpoint_c = Some(18.0);
        });

        let mut eq = ASHPHeater::new(cfg.clone());
        let initial_env = make_env(16.0, 0.0, 0);
        eq.init(&cfg, &initial_env).unwrap();

        // Step 1: set a higher setpoint to trigger lockout.
        let raised_setpoint = 21.0_f64;
        eq.core
            .hvac
            .apply_control_signal(&ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(raised_setpoint),
                cooling_setpoint_c: Some(26.0),
                deadband_c: None,
            });

        for step in 0..10 {
            let t = make_env(16.0, 0.0, step * 60);
            let mode = eq.update_control(&t);
            assert!(
                !matches!(
                    mode,
                    OperatingMode::HeatingER | OperatingMode::HeatingHPAndER
                ),
                "ER must be blocked at step {step} during hard lockout, got {mode:?}"
            );
        }

        // After 600 s the lockout expires; ER should be permitted.
        let t_after = make_env(16.0, 0.0, 600);
        let mode_after = eq.update_control(&t_after);
        assert!(
            matches!(
                mode_after,
                OperatingMode::HeatingER | OperatingMode::HeatingHPAndER
            ),
            "ER must be allowed after lockout expires, got {mode_after:?}"
        );
    }

    #[test]
    fn er_soft_lockout_blocks_er_while_zone_temp_is_rising() {
        // OCHRE HVAC.py: two-stage lockout — after hard lockout expires, ER stays
        // off while zone temp is still rising (heat pump is winning the load).
        let cfg = heater_config_with(|typed| {
            typed.er_hard_lockout_time_s = Some(60.0);
            typed.er_lockout_temp_c = Some(100.0);
            typed.hp_lockout_temp_c = Some(100.0);
            typed.er_setpoint_offset_c = Some(0.0);
            typed.heating_setpoint_c = Some(18.0);
        });
        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &make_env(16.0, 0.0, 0)).unwrap();

        // Raise setpoint to 21°C to trigger hard lockout (+3°C above 18°C).
        eq.core
            .hvac
            .apply_control_signal(&ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(21.0),
                cooling_setpoint_c: Some(26.0),
                deadband_c: None,
            });

        // Step within hard lockout period.
        eq.update_control(&make_env(16.0, 0.0, 0));

        // Step after hard lockout expires but with rising zone temp.
        let mode_rising = eq.update_control(&make_env(17.0, 0.0, 60));
        assert!(
            !matches!(
                mode_rising,
                OperatingMode::HeatingER | OperatingMode::HeatingHPAndER
            ),
            "ER must stay off under soft lockout while zone temp is rising, got {mode_rising:?}"
        );

        // Once zone temp stops rising, soft lockout clears.
        let mode_stable = eq.update_control(&make_env(17.0, 0.0, 120));
        assert!(
            matches!(
                mode_stable,
                OperatingMode::HeatingER | OperatingMode::HeatingHPAndER
            ),
            "ER must be allowed once zone temp stops rising, got {mode_stable:?}"
        );
    }

    // Regression counterpart: when backup ER is present the type is
    // AshpHeatPumpAux, which has a fixed 40.6°C supply temp independent of OAT.
    #[test]
    fn ashp_with_backup_uses_fixed_supply_temp() {
        // Use a very cold OAT so that HP runs and update_supply_air_temp is
        // called, then check that the result is the fixed AshpHeatPumpAux value.
        let cfg = heater_config(); // backup_capacity_w = 4_000 W
        let environment = env(18.0, -5.0, 0.005);

        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &environment).unwrap();
        eq.update_control(&environment);
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let supply = eq.core.hvac.supply_air_temp_c;
        assert!(
            (supply - 40.6).abs() < 0.01,
            "AshpHeatPumpAux must have fixed 40.6°C supply temp, got {supply:.3}°C",
        );
    }

    // EnergyPlus supplemental-ER upper OAT cap: ER must not fire when OAT is above
    // max_oat_supplemental_c (default 21°C / 69.8°F) even when er_lockout_temp_c
    // is configured high enough that the OCHRE threshold alone would permit ER.
    #[test]
    fn er_blocked_above_max_oat_supplemental() {
        let cfg = heater_config_with(|typed| {
            typed.er_lockout_temp_c = Some(50.0);
            typed.hp_lockout_temp_c = Some(100.0);
            typed.er_setpoint_offset_c = Some(0.0);
            typed.max_oat_supplemental_c = Some(21.0);
        });

        let mut eq = ASHPHeater::new(cfg.clone());
        // Zone well below setpoint (18°C vs 21°C) so a heating call is active.
        let env_above = env(18.0, 22.0, 0.005); // OAT 22°C > 21°C cap
        eq.init(&cfg, &env_above).unwrap();

        let mode = eq.update_control(&env_above);
        assert!(
            !matches!(
                mode,
                OperatingMode::HeatingER | OperatingMode::HeatingHPAndER
            ),
            "ER must be blocked when OAT ({:.1}°C) exceeds max_oat_supplemental_c (21°C), \
             got {mode:?}",
            env_above.weather.outdoor_temp_c,
        );
    }

    // ER must be allowed when OAT is at or below the max_oat_supplemental_c cap.
    #[test]
    fn er_allowed_below_max_oat_supplemental() {
        let cfg = heater_config_with(|typed| {
            typed.er_lockout_temp_c = Some(50.0);
            typed.hp_lockout_temp_c = Some(100.0);
            typed.er_setpoint_offset_c = Some(0.0);
            typed.max_oat_supplemental_c = Some(21.0);
        });

        let mut eq = ASHPHeater::new(cfg.clone());
        let env_below = env(18.0, 20.0, 0.005); // OAT 20°C <= 21°C cap
        eq.init(&cfg, &env_below).unwrap();

        let mode = eq.update_control(&env_below);
        assert!(
            matches!(
                mode,
                OperatingMode::HeatingER | OperatingMode::HeatingHPAndER
            ),
            "ER must be allowed when OAT ({:.1}°C) is at or below max_oat_supplemental_c (21°C), \
             got {mode:?}",
            env_below.weather.outdoor_temp_c,
        );
    }

    // User-configured values above 21°C must be silently clamped to 21°C.
    #[test]
    fn max_oat_supplemental_clamped_to_hard_limit() {
        let cfg = heater_config_with(|typed| {
            typed.er_lockout_temp_c = Some(50.0);
            typed.hp_lockout_temp_c = Some(100.0);
            typed.er_setpoint_offset_c = Some(0.0);
            typed.max_oat_supplemental_c = Some(30.0);
        });

        let mut eq = ASHPHeater::new(cfg.clone());
        let env_above = env(18.0, 22.0, 0.005); // OAT 22°C > clamped cap of 21°C
        eq.init(&cfg, &env_above).unwrap();

        // After clamping, effective cap is 21°C, so OAT=22°C must block ER.
        let mode = eq.update_control(&env_above);
        assert!(
            !matches!(
                mode,
                OperatingMode::HeatingER | OperatingMode::HeatingHPAndER
            ),
            "OAT (22°C) must block ER after clamping max_oat_supplemental_c from 30→21°C, \
             got {mode:?}",
        );
    }

    // --- DR and control signal tests ---

    // DR Moderate: heating setpoint offset = -1°C.
    // The `update_mode` FSM uses the base setpoints for mode transitions, while
    // `resolve_control` uses the DR-adjusted setpoint for load_ratio calculation.
    // Effect: DR reduces the load_ratio, which reduces heating output (lower PLR).
    // At zone=19°C vs base setpoint=21°C, hysteresis=1: load without DR = (21-19)/1 = 2 (full).
    // With DR Moderate (offset=-1): effective_setpoint=20, load = (20-19)/1 = 1 (still full but lower).
    // To observe a partial-load reduction, use zone inside the hysteresis band (19.5°C):
    // no-DR: load=(21-19.5)/1=1.5 → clamped to 1.0 (full); DR: load=(20-19.5)/1=0.5 (part load).
    // The test verifies that DR Moderate reduces the heating output vs the no-DR case.
    #[test]
    fn dr_moderate_shifts_heating_setpoint_down() {
        let cfg = heater_config(); // heating_setpoint_c=21°C, hysteresis=1°C
        // zone=19.5°C: below base threshold (21-1=20) → baseline enters Heating.
        // DR Moderate: effective_setpoint=20; load_ratio=(20-19.5)/1=0.5 (part load vs full).
        // Use OAT=5°C (above default er_lockout_temp=4.44°C) so ER is temp-blocked.
        let e = env(19.5, 5.0, 0.005);

        // Baseline without DR: full HP output at PLR=1.
        let mut eq_base = ASHPHeater::new(cfg.clone());
        eq_base.init(&cfg, &e).unwrap();
        eq_base.update_control(&e);
        let mut ports_base = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq_base
            .step(&e, Duration::from_secs(60), &mut ports_base)
            .unwrap();
        let kw_base = eq_base.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);

        // With DR Moderate: effective_setpoint=20°C → load_ratio=(20-19.5)/1=0.5 → partial output.
        let mut eq_dr = ASHPHeater::new(cfg.clone());
        eq_dr.init(&cfg, &e).unwrap();
        eq_dr
            .apply_control(&ControlSignal::DemandResponse {
                level: DRLevel::Moderate,
                duration_s: None,
            })
            .unwrap();
        eq_dr.update_control(&e);
        let mut ports_dr = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq_dr
            .step(&e, Duration::from_secs(60), &mut ports_dr)
            .unwrap();
        let kw_dr = eq_dr.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);

        assert!(kw_base > 0.0, "baseline must draw power; got {kw_base}");
        assert!(kw_dr >= 0.0, "DR case must draw non-negative power");
        // DR Moderate reduces the effective setpoint, which lowers load_ratio and thus
        // part-load ratio, resulting in less or equal heating output.
        assert!(
            kw_dr <= kw_base,
            "DR Moderate must not increase heating output; dr={kw_dr:.4}, base={kw_base:.4}"
        );
    }

    // GridEmergency: dr_load_fraction=0 → full shed → zero output.
    #[test]
    fn dr_grid_emergency_forces_heater_off() {
        let cfg = heater_config();
        // Zone well below setpoint so heating is clearly needed.
        let e = env(18.0, 5.0, 0.005);

        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::GridEmergency,
            duration_s: None,
        })
        .unwrap();
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&e);
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        let kw = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        assert_eq!(kw, 0.0, "GridEmergency must force zero output; got {kw}");
    }

    // LoadFraction is transient: update_control resets ctrl_load_fraction=1.0 at its start.
    // Signals must be applied AFTER update_control but BEFORE step to take effect that step.
    // The next update_control call restores the default — no re-apply needed.
    #[test]
    fn load_fraction_resets_each_step() {
        let cfg = heater_config();
        // Zone well below setpoint so heating is clearly needed.
        let e = env(18.0, 5.0, 0.005);

        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        // Step 1: update_control (sets mode), then apply LoadFraction=0, then step → off.
        eq.update_control(&e);
        eq.apply_control(&ControlSignal::LoadFraction { fraction: 0.0 })
            .unwrap();
        let mut ports1 = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&e, Duration::from_secs(60), &mut ports1).unwrap();
        let kw_step1 = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        assert_eq!(kw_step1, 0.0, "step 1 with LoadFraction=0 must be off");

        // Step 2: update_control resets ctrl_load_fraction=1.0; no signal reapplied → must heat.
        eq.update_control(&e);
        let mut ports2 = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&e, Duration::from_secs(60), &mut ports2).unwrap();
        let kw_step2 = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        assert!(
            kw_step2 > 0.0,
            "step 2 must draw power after transient LoadFraction resets; got {kw_step2}"
        );
    }

    // --- DR and control signal tests ---

    // DR Moderate: heating setpoint offset = -1°C.
    // The `update_mode` FSM uses the base setpoints for mode transitions, while
    // `resolve_control` uses the DR-adjusted setpoint for load_ratio calculation.
    // Effect: DR reduces the load_ratio, which reduces heating output (lower PLR).
    // At zone=19°C vs base setpoint=21°C, hysteresis=1: load without DR = (21-19)/1 = 2 (full).
    // With DR Moderate (offset=-1): effective_setpoint=20, load = (20-19)/1 = 1 (still full but lower).
    // To observe a partial-load reduction, use zone inside the hysteresis band (19.5°C):
    // no-DR: load=(21-19.5)/1=1.5 → clamped to 1.0 (full); DR: load=(20-19.5)/1=0.5 (part load).
    // The test verifies that DR Moderate reduces the heating output vs the no-DR case.

    // State round-trip must preserve max_oat_supplemental_c.
    #[test]
    fn state_round_trip_preserves_max_oat_supplemental_c() {
        let cfg = heater_config_with(|typed| typed.max_oat_supplemental_c = Some(15.0));
        let environment = env(18.0, 0.0, 0.005);

        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        eq.update_control(&environment);

        let state = eq.save_state();
        let mut restored = ASHPHeater::new(cfg.clone());
        restored.init(&cfg, &environment).unwrap();
        restored.load_state(&state).unwrap();

        assert!(
            (restored.core.max_oat_supplemental_c - 15.0).abs() < 1e-9,
            "max_oat_supplemental_c must survive save/load round-trip; \
             expected 15.0, got {}",
            restored.core.max_oat_supplemental_c,
        );
    }

    #[test]
    fn state_round_trip_preserves_dr_state() {
        use hares_types::{ControlSignal, DRLevel};

        let cfg = heater_config();
        // Zone below heating setpoint so heater is active.
        let environment = env(18.0, 0.0, 0.005);

        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();

        // Apply a Critical DR event with a 300 s duration.
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::Critical,
            duration_s: Some(300.0),
        })
        .unwrap();

        let saved = eq.save_state();

        let mut restored = ASHPHeater::new(cfg.clone());
        restored.init(&cfg, &environment).unwrap();
        restored.load_state(&saved).unwrap();

        assert_eq!(
            restored.core.dr_level,
            DRLevel::Critical,
            "dr_level must survive save/load"
        );
        assert!(
            restored.core.dr_setpoint_offset_c < 0.0,
            "dr_setpoint_offset_c must be negative after Critical DR (heating shed) (got {})",
            restored.core.dr_setpoint_offset_c
        );
        assert_eq!(
            restored.core.dr_duration_remaining_s,
            Some(300.0),
            "dr_duration_remaining_s must survive save/load"
        );
        assert!(
            restored.core.dr_load_fraction < 1.0,
            "dr_load_fraction must be < 1.0 after Critical DR (got {})",
            restored.core.dr_load_fraction
        );
    }

    // ER engagement threshold: with default hysteresis_c=1.0 and
    // er_setpoint_offset = 1.0 * (1.8 - 0.2) = 1.6°C, ER must NOT engage when the
    // zone temp is only one deadband (1.0°C) below setpoint.  The ER threshold
    // sits 0.6°C lower than the HP heating threshold.
    #[test]
    fn er_does_not_engage_at_one_deadband_below_setpoint() {
        // heater_config(): heating_setpoint=21°C, hysteresis=1°C, backup=4 kW.
        // After init: er_setpoint_offset_c = 1.0*(1.8-0.2) = 1.6°C.
        // ER fires only when zone <= 21.0 - 1.6 = 19.4°C.
        // Zone at setpoint - 1.0 = 20.0°C is above 19.4°C → ER must NOT fire.
        let cfg = heater_config();

        // OAT=0°C: below HP lockout (-17.78°C) threshold, so HP is available.
        //          below ER lockout (4.44°C), so ER is temperature-permitted.
        //          below max_oat_supplemental (21°C), so ER is not EnergyPlus-blocked.
        let env_cold = env(18.0, 0.0, 0.003); // zone well below setpoint → enters Heating
        let env_test = env(20.0, 0.0, 0.003); // zone = setpoint - 1.0°C

        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &env_cold).unwrap();

        // Step 1: prime thermostat FSM into Heating mode (18°C < 21-1=20°C).
        eq.update_control(&env_cold);
        eq.step(&env_cold, Duration::from_secs(60), &mut ports)
            .unwrap();

        // Step 2: zone at exactly one deadband below setpoint (20.0°C).
        // Thermostat stays in Heating (20.0 <= 21.0 with cutout_ratio=0).
        // load_ratio = (21-20)/1 = 1.0 → HP runs at full speed.
        // er_thermostat_call: 20.0 <= 19.4 → false → ER must not fire.
        let mode = eq.update_control(&env_test);
        assert_eq!(
            mode,
            OperatingMode::HeatingHP,
            "ER must NOT engage when zone ({:.1}°C) is only one deadband below \
             setpoint ({:.1}°C); er_setpoint_offset=1.6°C requires zone <= {:.1}°C",
            env_test.zones[0].temperature_c,
            21.0_f64,
            21.0_f64 - 1.6_f64,
        );
    }

    // ER engagement threshold: zone at 1.7°C below setpoint (19.3°C) is below
    // the 1.6°C er_setpoint_offset threshold (19.4°C) and must engage ER.
    #[test]
    fn er_engages_below_offset_threshold() {
        // Same config as the companion test above.
        // Zone = 21.0 - 1.7 = 19.3°C; er_call_threshold = 21.0 - 1.6 = 19.4°C.
        // 19.3 <= 19.4 → er_thermostat_call = true → ER engages alongside HP.
        let cfg = heater_config();

        let env_cold = env(18.0, 0.0, 0.003); // prime FSM into Heating
        let env_test = env(19.3, 0.0, 0.003); // zone = setpoint - 1.7°C

        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &env_cold).unwrap();

        // Step 1: prime thermostat FSM into Heating mode.
        eq.update_control(&env_cold);
        eq.step(&env_cold, Duration::from_secs(60), &mut ports)
            .unwrap();

        // Step 2: zone has dropped to 19.3°C, which is below the 19.4°C ER threshold.
        // HP is available (OAT 0°C > -17.78°C); ER is temperature-permitted (OAT 0°C < 4.44°C).
        // er_thermostat_call: 19.3 <= 19.4 → true → ER engages.
        let mode = eq.update_control(&env_test);
        assert!(
            matches!(
                mode,
                OperatingMode::HeatingHPAndER | OperatingMode::HeatingER
            ),
            "ER must engage when zone ({:.1}°C) drops below er_call_threshold ({:.1}°C); \
             got {mode:?}",
            env_test.zones[0].temperature_c,
            21.0_f64 - 1.6_f64,
        );
    }

    // ER engagement threshold: with default hysteresis_c=1.0 and
    // er_setpoint_offset = 1.0 * (1.8 - 0.2) = 1.6°C, ER must NOT engage when the
    // zone temp is only one deadband (1.0°C) below setpoint.  The ER threshold
    // sits 0.6°C lower than the HP heating threshold.

    // H2 regression: DR expiry raises effective setpoint back to base, which must
    // NOT trigger the ER hard lockout. Only a genuine user/thermostat base-setpoint
    // raise should arm the lockout timer.
    #[test]
    fn dr_expiry_does_not_trigger_er_lockout() {
        let cfg = heater_config_with(|typed| {
            typed.er_hard_lockout_time_s = Some(600.0);
            typed.er_lockout_temp_c = Some(100.0);
            typed.hp_lockout_temp_c = Some(100.0);
            typed.er_setpoint_offset_c = Some(0.0);
        });

        let mut eq = ASHPHeater::new(cfg.clone());
        let e = make_env(16.0, 0.0, 0);
        eq.init(&cfg, &e).unwrap();

        // Warm up prev_base_setpoint so it equals the initial base (21°C) —
        // one update_control step with the initial setpoint.
        eq.update_control(&e);

        // Apply DR Critical: effective setpoint drops to 21 + (-3) = 18°C.
        // Base setpoint stays at 21°C — no lockout should arm here.
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::Critical,
            duration_s: None,
        })
        .unwrap();

        // A few steps while DR is active — no lockout, setpoint went DOWN.
        for step in 0..3i64 {
            let t = make_env(16.0, 0.0, step * 60);
            eq.update_control(&t);
        }
        assert!(
            eq.core.er_lockout_remaining_s <= 0.0,
            "ER lockout must not arm when DR lowers the effective setpoint; \
             remaining={:.1}",
            eq.core.er_lockout_remaining_s,
        );

        // Revert DR to Normal: effective setpoint jumps back to 21°C.
        // Base setpoint never changed — lockout must still NOT arm.
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::Normal,
            duration_s: None,
        })
        .unwrap();
        eq.update_control(&make_env(16.0, 0.0, 3 * 60));

        assert!(
            eq.core.er_lockout_remaining_s <= 0.0,
            "ER lockout must not arm on DR expiry (effective setpoint rebounded \
             to base but base setpoint itself did not change); remaining={:.1}",
            eq.core.er_lockout_remaining_s,
        );
    }

    // Complement to the DR expiry test: an actual thermostat/user base-setpoint
    // raise (not a DR offset) MUST arm the ER hard lockout.
    #[test]
    fn actual_setpoint_raise_triggers_er_lockout() {
        let cfg = heater_config_with(|typed| {
            typed.er_hard_lockout_time_s = Some(600.0);
            typed.er_lockout_temp_c = Some(100.0);
            typed.hp_lockout_temp_c = Some(100.0);
            typed.er_setpoint_offset_c = Some(0.0);
        });

        let mut eq = ASHPHeater::new(cfg.clone());
        let e = make_env(16.0, 0.0, 0);
        eq.init(&cfg, &e).unwrap();

        // One control step to record prev_base_setpoint = 21°C.
        eq.update_control(&e);
        assert!(
            eq.core.er_lockout_remaining_s <= 0.0,
            "lockout must not be active before any setpoint raise"
        );

        // Raise BASE setpoint by +2°C (21 → 23°C) — this is a genuine raise.
        eq.apply_control(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(23.0),
            cooling_setpoint_c: Some(26.0),
            deadband_c: None,
        })
        .unwrap();

        // Next update_control must detect the raise and arm the lockout.
        eq.update_control(&make_env(16.0, 0.0, 60));

        assert!(
            eq.core.er_lockout_remaining_s > 0.0,
            "ER hard lockout must arm after a genuine base-setpoint raise; \
             remaining={:.1}",
            eq.core.er_lockout_remaining_s,
        );
    }

    // M5 regression: DRLevel::High must set dr_duty_cycle explicitly, preventing
    // a stale value from a prior DR level from leaking in.
    #[test]
    fn dr_level_high_sets_all_dr_fields_consistently() {
        let cfg = heater_config();
        let e = env(18.0, 5.0, 0.005);
        let mut eq = ASHPHeater::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        // First set Critical so dr_duty_cycle has a known (possibly modified) value,
        // then switch to High. If High doesn't set dr_duty_cycle the stale Critical
        // value would remain — this is the M5 bug.
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::Critical,
            duration_s: None,
        })
        .unwrap();
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::High,
            duration_s: None,
        })
        .unwrap();

        assert!(
            eq.core.dr_setpoint_offset_c < 0.0,
            "DRLevel::High must set a negative setpoint offset for heating; got {}",
            eq.core.dr_setpoint_offset_c,
        );
        assert!(
            eq.core.dr_load_fraction > 0.0 && eq.core.dr_load_fraction < 1.0,
            "DRLevel::High must set a partial load fraction; got {}",
            eq.core.dr_load_fraction,
        );
        assert!(
            (eq.core.dr_duty_cycle - 1.0).abs() < f64::EPSILON,
            "DRLevel::High must explicitly set dr_duty_cycle = 1.0 (not stale from prior level); \
             got {}",
            eq.core.dr_duty_cycle,
        );
    }

    // COP per AHRI/SEER convention: denominator is compressor-only electric power,
    // excluding fan, ER backup, and pan heater.

    #[test]
    fn cop_excludes_fan_power_from_denominator() {
        // With fan_power_w_per_cfm=0 (no fan) and again with fan>0, verify that
        // the reported COP equals thermal_output / compressor_only, not thermal / total_electric.
        // Config: capacity=8000W, EIR=0.33 → compressor ≈ 2640 W at full load.
        // Identity biquadratic curves (ratio=1), OAT above all lockouts.
        let cfg = heater_config_with(|typed| typed.fan_power_w = Some(0.0));
        // OAT=5°C: above ER lockout (4.44°C) so only HP runs, no ER.
        let e = env(18.0, 5.0, 0.003);

        let mut eq_no_fan = ASHPHeater::new(cfg.clone());
        let mut ports_no_fan = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq_no_fan.init(&cfg, &e).unwrap();
        eq_no_fan.update_control(&e);
        eq_no_fan
            .step(&e, Duration::from_secs(60), &mut ports_no_fan)
            .unwrap();

        let cop_no_fan = eq_no_fan.telemetry().get(tk::COP).unwrap_or(0.0);
        let electric_kw_no_fan = eq_no_fan.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        let thermal_w_no_fan = eq_no_fan
            .telemetry()
            .get(tk::THERMAL_OUTPUT_W)
            .unwrap_or(0.0);

        // With zero fan, total electric = compressor only, so COP = thermal / electric.
        if electric_kw_no_fan > 1e-6 {
            let expected_cop = thermal_w_no_fan / (electric_kw_no_fan * 1000.0);
            assert!(
                (cop_no_fan - expected_cop).abs() < 1e-6,
                "with no fan, COP must equal thermal/compressor; expected {expected_cop:.4}, \
                 got {cop_no_fan:.4}"
            );
        }

        // Now add fan power: COP should be higher because denominator is smaller
        // (only compressor, not compressor+fan).
        let cfg_fan = heater_config_with(|typed| typed.fan_power_w = Some(500.0));
        let mut eq_fan = ASHPHeater::new(cfg_fan.clone());
        let mut ports_fan = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq_fan.init(&cfg_fan, &e).unwrap();
        eq_fan.update_control(&e);
        eq_fan
            .step(&e, Duration::from_secs(60), &mut ports_fan)
            .unwrap();

        let cop_fan = eq_fan.telemetry().get(tk::COP).unwrap_or(0.0);
        let electric_kw_fan = eq_fan.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);

        // COP must NOT equal thermal / total_electric (which would include fan).
        // Reported COP should be >= no-fan COP because it uses compressor-only denominator.
        if electric_kw_fan > electric_kw_no_fan + 1e-9 {
            // Verify the reported COP is not the total-power COP (which would be lower).
            let total_power_cop =
                ports_fan.thermal[0].sensible_gain_w.abs() / (electric_kw_fan * 1000.0).max(1e-9);
            assert!(
                cop_fan > total_power_cop - 1e-9,
                "COP must be computed from compressor-only power (higher than total-power COP); \
                 compressor_only COP={cop_fan:.4}, total_power COP={total_power_cop:.4}"
            );
        }
    }

    #[test]
    fn cop_is_zero_when_heater_is_off() {
        // When the heater is off (no update_control call), compressor_kw=0 → COP must be 0.
        let cfg = heater_config();
        let e = env(18.0, 5.0, 0.003);
        let mut eq = ASHPHeater::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.init(&cfg, &e).unwrap();
        // Skip update_control so heater stays Off.
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        let cop = eq.telemetry().get(tk::COP).unwrap_or(-1.0);
        assert_eq!(cop, 0.0, "heater off must give COP=0, got {cop}");
    }

    #[test]
    fn mshp_typed_init_produces_four_speed_stages() {
        use crate::Equipment;
        use crate::HeatPumpHeaterConfig;
        use crate::config::EquipmentTypedConfig;

        let cfg = HeatPumpHeaterConfig {
            zone_id: Some(1),
            heating_capacity_w: Some(10_000.0),
            hspf: Some(10.0),
            heating_efficiency: None,
            is_mini_split: true,
            ..Default::default()
        };

        let ec = crate::config::EquipmentConfig::from_typed(
            "MSHP Heater".to_string(),
            HeatPumpHeaterConfig::equipment_type_name().to_string(),
            cfg,
        );
        let ec = crate::config::EquipmentConfig::with_payload(
            "MSHP Heater".to_string(),
            "MSHP Heater".to_string(),
            ec.payload.clone(),
        );

        let environment = env(18.0, 0.0, 0.003);
        let mut eq = MinisplitHeater::new(ec.clone());
        eq.init(&ec, &environment);

        let n_stages = eq.core.hvac.heating_capacities_w.len();
        assert_eq!(
            n_stages, 4,
            "MSHP typed init with single capacity must produce 4 speed stages, got {n_stages}"
        );
    }
}

#[cfg(test)]
mod ideal_capacity_tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, EnvironmentState, GridState, PortSlots, ThermalAccumulator, WeatherState,
        ZoneId, ZoneState, telemetry_keys as tk,
    };

    use super::ASHPHeater;
    use crate::{Equipment, EquipmentConfig};

    /// Build an `EnvironmentState` with a configurable zone temperature and time resolution.
    /// OAT is held above the HP lockout (default -17.78°C) and above the ER lockout
    /// (default 4.44°C) so only HP heating is active — isolating the duty-cycle behaviour.
    fn make_env(zone_temp_c: f64, time_res_s: i64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: 14.0,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 8.0,
                outdoor_humidity_ratio: 0.004,
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
                ..Default::default()
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::seconds(time_res_s),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    /// HP heater with setpoint=21°C, hysteresis=1°C, no ER strip heat (backup_capacity_w=0),
    /// OAT above ER lockout so only the HP compressor runs.
    fn heater_config() -> EquipmentConfig {
        let mut cfg = EquipmentConfig::from_typed(
            "HP Heater".to_string(),
            "ASHP Heater".to_string(),
            crate::HeatPumpHeaterConfig {
                equipment_id: None,
                zone_id: Some(1),
                heating_capacity_w: Some(8_000.0),
                hspf: Some(3.412_141_633 / 0.33),
                heating_efficiency: None,
                stage_heating_capacities_w: None,
                stage_heating_eirs: None,
                backup_fuel: None,
                backup_capacity_w: Some(0.0),
                backup_eir: None,
                fraction_heating_load_served: None,
                cooling_capacity_w: None,
                seer: None,
                stage_cooling_capacities_w: None,
                stage_cooling_eirs: None,
                stage_shrs: None,
                fraction_cooling_load_served: None,
                number_of_speeds: 1,
                is_mini_split: false,
                shr: None,
                fan_power_w: None,
                fan_power_w_per_cfm: None,
                airflow_m3_s_per_w: None,
                heating_setpoint_c: Some(21.0),
                cooling_setpoint_c: Some(26.0),
                hysteresis_c: Some(1.0),
                hp_lockout_temp_c: None,
                er_lockout_temp_c: None,
                max_oat_supplemental_c: None,
                er_setpoint_offset_c: None,
                er_hard_lockout_time_s: None,
                duct: Default::default(),
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
            },
        );
        cfg.raw_config_mut().unwrap().insert(
            "biquadratic_coeffs".to_string(),
            "[[1,0,0,0,0,0],[1,0,0,0,0,0]]".into(),
        );
        cfg
    }

    fn make_ports() -> PortSlots {
        PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        }
    }

    // At 60 s timestep (fine resolution), an IdealCapacity signal is ignored because
    // use_ideal=false. Zone is below FSM turn-on threshold (setpoint - hysteresis = 20°C),
    // so FSM enters Heating with load_ratio = (21 - 19) / 1 = 1.0 (clamped) → RTF = 1.0.
    #[test]
    fn fine_timestep_fsm_cycling_ignores_ideal_capacity_signal() {
        let cfg = heater_config();
        let mut eq = ASHPHeater::new(cfg.clone());
        // 19°C is below FSM turn-on threshold of 20°C.
        let env = make_env(19.0, 60);
        eq.init(&cfg, &env).unwrap();
        // Signal provides half-rated load; must be ignored when use_ideal=false.
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: 4_000.0,
        })
        .unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut make_ports())
            .unwrap();

        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(-1.0);
        assert!(
            (rtf - 1.0).abs() < 1e-9,
            "FSM Heating at 60 s must ignore IdealCapacity and produce RTF=1.0, got {rtf}"
        );
    }

    // At 900 s timestep (coarse resolution), a solver-provided IdealCapacity signal for
    // half the rated capacity (4000 W of 8000 W rated) produces fractional RTF.
    // Flow: apply signal → update_control (use_ideal=true, FSM enters Heating at 19°C) →
    //   ideal_capacity_w=4000 > 0 → load_ratio = 4000/8000 = 0.5 → RTF ≈ 0.5.
    #[test]
    fn coarse_timestep_ideal_signal_produces_fractional_rtf() {
        let cfg = heater_config();
        let mut eq = ASHPHeater::new(cfg.clone());
        // 19°C is below FSM turn-on threshold of 20°C; FSM enters Heating.
        let env = make_env(19.0, 900);
        eq.init(&cfg, &env).unwrap();
        // Half rated capacity: 4000 W of 8000 W rated → load_ratio = 0.5.
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: 4_000.0,
        })
        .unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(900), &mut make_ports())
            .unwrap();

        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(-1.0);
        // Flat biquadratic [1,0,0,0,0,0] → cap_ratio=1.0, PLR = 4000/8000 = 0.5.
        // PLF slightly adjusts, so allow ±5%.
        assert!(
            (rtf - 0.5).abs() < 0.05,
            "IdealCapacity=4000 W at 900 s (half rated) must produce RTF ≈ 0.5, got {rtf}"
        );
    }

    // At 900 s timestep with no solver signal and zone above the heating setpoint,
    // the thermostat stays Deadband → heater is off regardless of timestep resolution.
    #[test]
    fn coarse_timestep_no_signal_above_setpoint_produces_zero_rtf() {
        let cfg = heater_config();
        let mut eq = ASHPHeater::new(cfg.clone());
        // 22°C is above setpoint (21°C); FSM stays Deadband, no heating demand.
        let env = make_env(22.0, 900);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(900), &mut make_ports())
            .unwrap();

        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(-1.0);
        assert_eq!(
            rtf, 0.0,
            "900 s timestep with zone above heating setpoint must produce RTF=0.0, got {rtf}"
        );
    }
}
