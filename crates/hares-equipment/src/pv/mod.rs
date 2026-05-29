//! Photovoltaic panel equipment model.

mod array_config;
pub mod config;
mod lut;
pub mod shading;
pub mod soiling;

pub use array_config::{ModuleType, PvArray, PvArraySpec, surface_id_for_orientation};
use array_config::{normalize_azimuth, parse_u32_from_f64};
pub use config::PvConfig;
use lut::PvLut;

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use chrono::{Datelike, Timelike};
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CorePerformance,
    CoreState, ElectricPower, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FuelType, HaresError, InverterPriority, OperatingMode, PortContribution,
    PortDeclaration, PortSlots, SurfaceIrradiance, Telemetry, TelemetryField, telemetry_keys as tk,
};
use serde::{Deserialize, Serialize};

use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

use crate::config::KEY_EQUIPMENT_ID;

const DEFAULT_NOCT_C: f64 = 47.0;
const DEFAULT_INVERTER_EFFICIENCY: f64 = 0.96;
const DEFAULT_SURFACE_RESOLUTION_DEG: f64 = 5.0;
const DEFAULT_T_REF_C: f64 = 25.0;
const DEFAULT_POWER_FACTOR: f64 = 1.0;
/// Temperature coefficient of power for the default Standard module type (PVWatts v8).
const DEFAULT_GAMMA_PER_C: f64 = -0.0047;
const DEFAULT_SYSTEM_LOSSES_FRACTION: f64 = 0.14;
const IRRADIANCE_AT_STC_W_M2: f64 = 1_000.0;
const NOCT_REFERENCE_TEMP_C: f64 = 20.0;
const NOCT_REFERENCE_IRRADIANCE_W_M2: f64 = 800.0;

/// Wind-corrected SAM-NOCT heat loss coefficients.
///
/// The SAM-NOCT cell temperature model (used by PVWatts v8) adds a wind
/// correction factor to the basic NOCT formula:
///
///   T_cell = T_amb + (E_POA / 800) * (NOCT - 20) * (9.5 / (5.7 + 3.8 * WS))
///
/// At the NOCT reference wind speed of 1 m/s the factor is exactly 1.0,
/// so the model degrades gracefully to the plain NOCT formula when wind
/// data is unavailable (pass WS = 1.0).
///
/// References:
///   - PVWatts v8 Technical Reference, NREL/TP-7A40-80694
///   - PVPMC: <https://pvpmc.sandia.gov/modeling-guide/2-dc-module-iv/cell-temperature/noct-cell-temperature/>
const NOCT_WIND_NUMERATOR: f64 = 9.5;
const NOCT_WIND_CONSTANT: f64 = 5.7;
const NOCT_WIND_COEFFICIENT: f64 = 3.8;

/// Compute cell temperature using the SAM-NOCT model with wind correction.
///
/// Returns °C.  At `wind_speed_m_s = 1.0` the result is identical to the
/// basic NOCT formula (backward-compatible).
#[inline]
fn cell_temperature_noct_wind(
    ambient_temp_c: f64,
    irradiance_w_m2: f64,
    noct_c: f64,
    wind_speed_m_s: f64,
) -> f64 {
    let noct_factor = (noct_c - NOCT_REFERENCE_TEMP_C) / NOCT_REFERENCE_IRRADIANCE_W_M2;
    let wind_correction = NOCT_WIND_NUMERATOR
        / (NOCT_WIND_CONSTANT + NOCT_WIND_COEFFICIENT * wind_speed_m_s.max(0.0));
    ambient_temp_c + irradiance_w_m2 * noct_factor * wind_correction
}

#[derive(Clone, Debug)]
struct ArrayStepOutput {
    dc_power_kw: f64,
    ac_power_kw: f64,
    irradiance_w_m2: f64,
    cell_temp_c: f64,
}

#[derive(Serialize, Deserialize)]
struct PvCheckpoint {
    power_limit_kw: Option<f64>,
    curtailment_fraction: f64,
    q_setpoint_kvar: f64,
    inverter_priority: InverterPriority,
    power_factor: f64,
    soiling_config: Option<soiling::SoilingConfig>,
    soiling_state: Option<soiling::SoilingState>,
    shading_model: shading::ShadingModel,
}

pub struct PV {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    core_output: CoreOutput,
    arrays: Vec<PvArray>,
    surface_resolution_deg: f64,
    inverter_efficiency: f64,
    /// AC output cap imposed by physical inverter size (DC/AC ratio scenarios).
    inverter_capacity_kw: Option<f64>,
    power_factor: f64,
    system_losses_fraction: f64,
    power_limit_kw: Option<f64>,
    curtailment_fraction: f64,
    inverter_priority: InverterPriority,
    inverter_min_pf: Option<f64>,
    q_setpoint_kvar: f64,
    luts_by_surface: HashMap<u32, PvLut>,
    last_ac_power_kw: f64,
    soiling_config: Option<soiling::SoilingConfig>,
    soiling_state: Option<soiling::SoilingState>,
    shading_model: shading::ShadingModel,
    init_error: Option<HaresError>,
}

impl PV {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let id = parse_u32_from_f64(config.get_f64(KEY_EQUIPMENT_ID)).unwrap_or(0);
        let (arrays, init_error) = if config.is_typed() {
            (vec![], None)
        } else {
            (
                vec![],
                Some(HaresError::Equipment(
                    "PV requires typed config; raw config is unsupported".to_string(),
                )),
            )
        };
        let shading_model = shading::parse_shading_config(&config);

        let descriptor = EquipmentDescriptor {
            id: EquipmentId(id),
            name: config.name,
            end_use: EndUse::PV,
            equipment_type: Cow::Borrowed("PV"),
            zone: None,
            fuel: FuelType::Electric,
            stage: ExecutionStage::Independent,
            control_capabilities: ControlCapabilities::POWER_LIMIT
                | ControlCapabilities::POWER_SETPOINT
                | ControlCapabilities::CURTAILMENT_PERCENT
                | ControlCapabilities::REACTIVE_SETPOINT
                | ControlCapabilities::POWER_FACTOR_SETPOINT
                | ControlCapabilities::INVERTER_PRIORITY_MODE,
            core_capabilities: CoreCapabilities::ELECTRIC | CoreCapabilities::REACTIVE,
            telemetry_fields: telemetry_fields(),
        };

        let mut telemetry = Telemetry::with_capacity(9);
        telemetry.insert(tk::DC_POWER_KW, 0.0);
        telemetry.insert(tk::AC_POWER_KW, 0.0);
        telemetry.insert(tk::REACTIVE_POWER_KVAR, 0.0);
        telemetry.insert(tk::CELL_TEMP_C, 0.0);
        telemetry.insert(tk::IRRADIANCE_W_M2, 0.0);
        telemetry.insert(tk::INVERTER_EFFICIENCY, DEFAULT_INVERTER_EFFICIENCY);
        telemetry.insert(tk::CURTAILMENT_KW, 0.0);
        telemetry.insert(tk::INVERTER_CLIPPING_KW, 0.0);
        telemetry.insert(tk::SOILING_RATIO, 1.0);
        telemetry.insert(tk::SHADING_FACTOR, 1.0);

        Self {
            descriptor,
            ports: vec![PortDeclaration::electrical()],
            telemetry,
            core_output: CoreOutput::default(),
            arrays,
            surface_resolution_deg: DEFAULT_SURFACE_RESOLUTION_DEG,
            inverter_efficiency: DEFAULT_INVERTER_EFFICIENCY,
            inverter_capacity_kw: None,
            power_factor: DEFAULT_POWER_FACTOR,
            system_losses_fraction: DEFAULT_SYSTEM_LOSSES_FRACTION,
            power_limit_kw: None,
            curtailment_fraction: 0.0,
            inverter_priority: InverterPriority::Var,
            inverter_min_pf: Some(0.8),
            q_setpoint_kvar: 0.0,
            luts_by_surface: HashMap::new(),
            last_ac_power_kw: 0.0,
            soiling_config: None,
            soiling_state: None,
            shading_model,
            init_error,
        }
    }

    fn step_one_array(
        &self,
        env: &EnvironmentState,
        irr: &SurfaceIrradiance,
        array: &PvArray,
        soiling_ratio: f64,
        shading_factor: f64,
    ) -> ArrayStepOutput {
        // Soiling and shading reduce effective irradiance reaching the cells.
        // Applied before cell temperature and power calculations so that a
        // shaded/soiled panel also runs cooler (less absorbed irradiance as heat).
        let irradiance_w_m2 = (irr.direct_w_m2 + irr.diffuse_w_m2 + irr.reflected_w_m2).max(0.0)
            * soiling_ratio
            * shading_factor;
        let ambient_temp_c = env.weather.outdoor_temp_c;

        if let Some(lut) = array
            .surface_id
            .and_then(|surface_id| self.luts_by_surface.get(&surface_id))
        {
            let month = f64::from(env.current_time.month() as u8);
            let hour = f64::from(env.current_time.hour() as u8)
                + f64::from(env.current_time.minute() as u8) / 60.0
                + f64::from(env.current_time.second() as u8) / 3600.0;
            // SAM LUTs are indexed by horizontal irradiance from weather data,
            // not tilted-surface (POA) values.
            let ghi = env.weather.ghi_w_m2.max(0.0);
            let dni = env.weather.dni_w_m2.max(0.0);
            let dhi = env.weather.dhi_w_m2.max(0.0);
            // SAM LUTs are indexed on weather-station irradiance (GHI/DNI/DHI),
            // not POA, so soiling can't be folded into the LUT inputs. Apply
            // the soiling/shading ratios as a post-LUT power derating instead.
            let ac_power_kw = lut
                .interpolate(month, hour, ghi, dni, dhi, ambient_temp_c)
                .max(0.0)
                * soiling_ratio
                * shading_factor;
            let dc_power_kw = ac_power_kw / self.inverter_efficiency.max(1e-9);
            let cell_temp_c = cell_temperature_noct_wind(
                ambient_temp_c,
                irradiance_w_m2,
                array.noct_c,
                env.weather.wind_speed_m_s,
            );
            return ArrayStepOutput {
                dc_power_kw,
                ac_power_kw,
                irradiance_w_m2,
                cell_temp_c,
            };
        }

        let cell_temp_c = cell_temperature_noct_wind(
            ambient_temp_c,
            irradiance_w_m2,
            array.noct_c,
            env.weather.wind_speed_m_s,
        );

        let gamma = array.module_type.gamma_per_c();
        let temp_derate = (1.0 + gamma * (cell_temp_c - DEFAULT_T_REF_C)).max(0.0);
        let mut dc_power_kw =
            array.capacity_kw * (irradiance_w_m2 / IRRADIANCE_AT_STC_W_M2) * temp_derate;
        dc_power_kw *= 1.0 - self.system_losses_fraction;
        let ac_power_kw = (dc_power_kw * self.inverter_efficiency).max(0.0);

        ArrayStepOutput {
            dc_power_kw,
            ac_power_kw,
            irradiance_w_m2,
            cell_temp_c,
        }
    }

    fn total_dc_capacity(&self) -> f64 {
        self.arrays.iter().map(|a| a.capacity_kw).sum()
    }

    fn apply_inverter_limits(&self, p_kw: f64, q_kvar: f64) -> (f64, f64) {
        let inv_cap = match self.inverter_capacity_kw {
            Some(cap) => cap,
            None => {
                // When inverter capacity is not configured, default to total
                // DC capacity (1:1 DC/AC ratio). OCHRE PV.py:122 defaults
                // inverter_capacity to capacity. Defence: init_typed() always
                // resolves this, so this branch is a safety net for code paths
                // that bypass init.
                let cap = self.total_dc_capacity();
                if cap <= 0.0 {
                    return (p_kw, q_kvar);
                }
                cap
            }
        };

        let s = (p_kw * p_kw + q_kvar * q_kvar).sqrt();
        if s <= inv_cap {
            return (p_kw, q_kvar);
        }

        match self.inverter_priority {
            InverterPriority::Watt => {
                let p_out = p_kw.min(inv_cap);
                let q_max = (inv_cap * inv_cap - p_out * p_out).max(0.0).sqrt();
                let mut q_out = q_kvar.clamp(-q_max, q_max);
                if let Some(min_pf) = self.inverter_min_pf {
                    q_out = enforce_min_pf(p_out, q_out, min_pf);
                }
                (p_out, q_out)
            }
            InverterPriority::Var => {
                let mut q_abs = q_kvar.abs();
                if let Some(min_pf) = self.inverter_min_pf {
                    // OCHRE: max_q_capacity = min_pf_factor * min_pf * inverter_capacity
                    // where min_pf_factor = tan(acos(min_pf)) / min_pf
                    // simplifies to: max_q_capacity = tan(acos(min_pf)) * inverter_capacity
                    let max_q_cap = min_pf.acos().sin() * inv_cap;
                    // OCHRE: max_q_pf = min_pf_factor * |p|
                    // simplifies to: max_q_pf = tan(acos(min_pf)) * |p| / min_pf... no
                    // Actually from OCHRE: max_q_pf = self.inverter_min_pf_factor * -p
                    // where min_pf_factor = tan(acos(min_pf)) (the ratio Q/P at min PF)
                    let max_q_pf = min_pf.acos().tan() * p_kw;
                    q_abs = q_abs.min(max_q_cap).min(max_q_pf);
                } else {
                    q_abs = q_abs.min(inv_cap);
                }
                let q_out = if self.q_setpoint_kvar >= 0.0 {
                    q_abs
                } else {
                    -q_abs
                };
                let p_max = (inv_cap * inv_cap - q_out * q_out).max(0.0).sqrt();
                let p_out = p_kw.min(p_max);
                (p_out, q_out)
            }
            InverterPriority::Cpf => {
                let scale = inv_cap / s;
                (p_kw * scale, q_kvar * scale)
            }
        }
    }
}

fn enforce_min_pf(p: f64, q: f64, min_pf: f64) -> f64 {
    if p == 0.0 {
        return 0.0;
    }
    if p < 0.0 || min_pf >= 1.0 {
        return q;
    }
    let max_q_abs = p * (min_pf.acos().tan());
    q.clamp(-max_q_abs, max_q_abs)
}

impl PV {
    fn init_typed(
        &mut self,
        config: &EquipmentConfig,
        env: &EnvironmentState,
    ) -> crate::Result<()> {
        let c = config.require_typed::<PvConfig>("PV")?;
        c.validate()?;

        // Determine which path to use: multi-array or single-array.
        if let Some(ref array_specs) = c.arrays {
            // Multi-array path: create one PvArray per PvArraySpec.
            let base = PvArray::default();
            self.arrays = array_specs
                .iter()
                .map(|spec| {
                    let tilt_deg = spec.tilt_deg.unwrap_or(base.tilt_deg);
                    let azimuth_deg =
                        normalize_azimuth(spec.azimuth_deg.unwrap_or(base.azimuth_deg));
                    let noct_c = spec.noct_c.unwrap_or(base.noct_c);
                    let module_type = spec
                        .module_type
                        .as_deref()
                        .map(ModuleType::from_str)
                        .unwrap_or(base.module_type);
                    let array = PvArray {
                        tilt_deg,
                        azimuth_deg,
                        capacity_kw: spec.capacity_kw,
                        noct_c,
                        module_type,
                        surface_id: None,
                        sam_lut_path: spec.sam_lut_path.clone(),
                        attached_boundary_id: spec.attached_boundary_id,
                    };
                    array.validate()?;
                    Ok(array)
                })
                .collect::<crate::Result<Vec<_>>>()?;
        } else {
            // Single-array path: existing behaviour, backward-compatible.
            let base = PvArray::default();
            let tilt_deg = c.tilt_deg.unwrap_or(base.tilt_deg);
            let azimuth_deg = normalize_azimuth(c.azimuth_deg.unwrap_or(base.azimuth_deg));
            let noct_c = c.noct_c.unwrap_or(base.noct_c);
            let module_type = c
                .module_type
                .as_deref()
                .map(ModuleType::from_str)
                .unwrap_or(base.module_type);

            let array = PvArray {
                tilt_deg,
                azimuth_deg,
                capacity_kw: c.capacity_kw,
                noct_c,
                module_type,
                surface_id: None,
                sam_lut_path: c.sam_lut_path.clone(),
                attached_boundary_id: None,
            };
            array.validate()?;

            self.arrays = vec![array];
        }

        self.surface_resolution_deg = c
            .surface_resolution_deg
            .unwrap_or(DEFAULT_SURFACE_RESOLUTION_DEG);
        if !self.surface_resolution_deg.is_finite() || self.surface_resolution_deg <= 0.0 {
            return Err(HaresError::Equipment(
                "PV surface_resolution_deg must be finite and > 0".to_string(),
            ));
        }

        self.inverter_efficiency = c
            .inverter_efficiency
            .unwrap_or(DEFAULT_INVERTER_EFFICIENCY)
            .clamp(0.0, 1.0);

        self.inverter_capacity_kw = c.inverter_capacity_kw;
        self.power_factor = c
            .power_factor
            .unwrap_or(DEFAULT_POWER_FACTOR)
            .clamp(0.0, 1.0);
        self.system_losses_fraction = c
            .system_losses_fraction
            .unwrap_or(DEFAULT_SYSTEM_LOSSES_FRACTION);

        self.luts_by_surface.clear();
        for array in &mut self.arrays {
            let surface_id = surface_id_for_orientation(
                array.tilt_deg,
                array.azimuth_deg,
                self.surface_resolution_deg,
            )?;
            array.surface_id = Some(surface_id);
            let Some(_entry) = env
                .weather
                .solar_irradiance
                .iter()
                .find(|entry| entry.surface_id == surface_id)
            else {
                return Err(HaresError::Equipment(format!(
                    "PV array tilt={} azimuth={} (surface_id={}) has no matching SurfaceIrradiance entry",
                    array.tilt_deg, array.azimuth_deg, surface_id
                )));
            };
            if let Some(path) = array.sam_lut_path.as_deref() {
                let lut = PvLut::from_path(Path::new(path))?;
                self.luts_by_surface.insert(surface_id, lut);
            }
        }

        // Invariant check: arrays must exist and have positive capacity.
        self.check_invariants()?;

        // Resolve inverter capacity default.
        // OCHRE PV.py:122 defaults inverter_capacity to capacity (1:1 DC/AC
        // ratio). HARES follows this to avoid unrealistic zero-clipping
        // behaviour when the user omits inverter_capacity_kw. NREL SAM
        // documentation notes typical residential DC-to-AC ratios of 1.0–1.5;
        // EnergyPlus PVWatts v8 default is 1.1 (PVWatts.cc:88).
        let total_dc = self.total_dc_capacity();
        if self.inverter_capacity_kw.is_none() {
            self.inverter_capacity_kw = Some(total_dc);
        }
        let inv_cap = self.inverter_capacity_kw.unwrap_or(total_dc);
        let dc_ac_ratio = total_dc / inv_cap;

        tracing::info!(
            total_dc_kw = total_dc,
            inverter_capacity_kw = inv_cap,
            dc_ac_ratio = format_args!("{dc_ac_ratio:.2}"),
            "PV DC/AC ratio",
        );
        if dc_ac_ratio < 1.0 {
            tracing::warn!(
                dc_ac_ratio = format_args!("{dc_ac_ratio:.2}"),
                "PV inverter oversized relative to DC capacity; DC/AC ratio < 1.0",
            );
        } else if dc_ac_ratio > 1.5 {
            tracing::warn!(
                dc_ac_ratio = format_args!("{dc_ac_ratio:.2}"),
                "PV DC/AC ratio > 1.5; aggressive clipping may occur during high-irradiance conditions",
            );
        }

        // Observer capture: record array breakdown and inverter sizing.
        #[cfg(feature = "observe")]
        {
            let per_array_capacity: Vec<f64> = self.arrays.iter().map(|a| a.capacity_kw).collect();
            let total_capacity: f64 = per_array_capacity.iter().sum();
            tracing::debug!(
                array_count = self.arrays.len(),
                total_capacity_kw = total_capacity,
                ?per_array_capacity,
                dc_ac_ratio = dc_ac_ratio,
                inverter_capacity_kw = inv_cap,
                "PV initialised",
            );
        }

        self.soiling_config = None;
        self.soiling_state = None;

        self.telemetry
            .set(tk::INVERTER_EFFICIENCY, self.inverter_efficiency);
        self.telemetry.set(tk::DC_POWER_KW, 0.0);
        self.telemetry.set(tk::AC_POWER_KW, 0.0);
        self.telemetry.set(tk::REACTIVE_POWER_KVAR, 0.0);
        self.telemetry
            .set(tk::CELL_TEMP_C, env.weather.outdoor_temp_c);
        self.telemetry.set(tk::IRRADIANCE_W_M2, 0.0);
        self.telemetry.set(tk::CURTAILMENT_KW, 0.0);
        self.telemetry.set(tk::INVERTER_CLIPPING_KW, 0.0);
        self.telemetry.set(tk::SOILING_RATIO, 1.0);
        self.core_output = CoreOutput::default();
        Ok(())
    }

    /// Validate that the PV model is in a consistent state.
    ///
    /// Checks that at least one array exists and every array has positive
    /// capacity. Gated behind `cfg(any(debug_assertions, feature =
    /// "check_invariants"))` so it compiles to nothing in production release
    /// builds without the feature flag.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    fn check_invariants(&self) -> crate::Result<()> {
        if self.arrays.is_empty() {
            return Err(HaresError::InvariantViolation {
                check_name: "pv_has_arrays".to_string(),
                value: 0.0,
                tolerance: 0.0,
            });
        }
        for array in self.arrays.iter() {
            array.validate()?;
        }
        Ok(())
    }

    /// Stub for unchecked builds — the body is eliminated by the compiler.
    #[cfg(not(any(debug_assertions, feature = "check_invariants")))]
    fn check_invariants(&self) -> crate::Result<()> {
        Ok(())
    }
}

impl Equipment for PV {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        // Surface any deferred parse error from PV::new before doing full init.
        if let Some(e) = self.init_error.take() {
            return Err(e);
        }
        self.init_typed(config, env)
    }

    fn update_control(&mut self, _env: &EnvironmentState) -> OperatingMode {
        if self.last_ac_power_kw > 0.0 {
            OperatingMode::Standby
        } else {
            OperatingMode::Off
        }
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        _dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        // Advance soiling model with current-timestep rainfall.
        let soiling_ratio = match (&self.soiling_config, &mut self.soiling_state) {
            (Some(cfg), Some(state)) => {
                state.step(cfg, env.weather.rainfall_m, env.time_step_secs(), false)
            }
            _ => 1.0,
        };

        // Compute shading factor from current solar position.
        let shading_factor = self.shading_model.shading_factor(
            env.weather.solar_altitude_deg,
            env.weather.solar_azimuth_deg,
            env.current_time.month(),
        );

        let mut total_dc_power_kw = 0.0;
        let mut total_ac_power_kw = 0.0;
        let mut total_irradiance_weighted = 0.0;
        let mut total_cell_temp_weighted = 0.0;
        let mut total_capacity_kw = 0.0;

        for array in &self.arrays {
            let surface_id = array.surface_id.ok_or_else(|| {
                HaresError::Equipment(
                    "PV array is missing surface_id; call init() before step()".to_string(),
                )
            })?;
            let irr = env
                .weather
                .solar_irradiance
                .iter()
                .find(|entry| entry.surface_id == surface_id)
                .ok_or_else(|| {
                    HaresError::Equipment(format!(
                        "PV surface_id {} not found in weather.solar_irradiance",
                        surface_id
                    ))
                })?;

            let output = self.step_one_array(env, irr, array, soiling_ratio, shading_factor);
            total_dc_power_kw += output.dc_power_kw;
            total_ac_power_kw += output.ac_power_kw;
            total_irradiance_weighted += output.irradiance_w_m2 * array.capacity_kw;
            total_cell_temp_weighted += output.cell_temp_c * array.capacity_kw;
            total_capacity_kw += array.capacity_kw;
        }

        // Apply operator curtailment fraction.
        if self.curtailment_fraction > 0.0 {
            total_ac_power_kw *= 1.0 - self.curtailment_fraction;
        }

        // Apply operator curtailment limit (control signal).
        let unclipped_ac_kw = total_ac_power_kw;
        if let Some(limit_kw) = self.power_limit_kw {
            total_ac_power_kw = total_ac_power_kw.min(limit_kw.max(0.0));
        }
        let curtailment_kw = (unclipped_ac_kw - total_ac_power_kw).max(0.0);

        // Compute reactive power from Q setpoint or static power factor.
        let mut reactive_power_kvar = self.q_setpoint_kvar;
        if reactive_power_kvar == 0.0 && self.power_factor < 1.0 {
            reactive_power_kvar = total_ac_power_kw * (self.power_factor.acos().tan());
        }

        // Apply smart inverter limits (handles clipping and priority).
        let (final_p_kw, final_q_kvar) =
            self.apply_inverter_limits(total_ac_power_kw, reactive_power_kvar);
        let inverter_clipping_kw = (total_ac_power_kw - final_p_kw).max(0.0);

        // Observer capture: record per-timestep inverter clipping events.
        #[cfg(feature = "observe")]
        if inverter_clipping_kw > 0.0 {
            tracing::debug!(
                clipped_kw = inverter_clipping_kw,
                ac_power_kw = final_p_kw,
                "PV inverter clipping",
            );
        }

        ports.accumulate(&PortContribution::Electrical {
            active_power_kw: -final_p_kw,
            reactive_power_kvar: -final_q_kvar,
        })?;

        let mean_irradiance_w_m2 = if total_capacity_kw > 0.0 {
            total_irradiance_weighted / total_capacity_kw
        } else {
            0.0
        };
        let mean_cell_temp_c = if total_capacity_kw > 0.0 {
            total_cell_temp_weighted / total_capacity_kw
        } else {
            env.weather.outdoor_temp_c
        };

        self.last_ac_power_kw = final_p_kw;
        self.telemetry.set(tk::DC_POWER_KW, total_dc_power_kw);
        self.telemetry.set(tk::AC_POWER_KW, final_p_kw);
        self.telemetry.set(tk::REACTIVE_POWER_KVAR, final_q_kvar);
        self.telemetry.set(tk::CELL_TEMP_C, mean_cell_temp_c);
        self.telemetry
            .set(tk::IRRADIANCE_W_M2, mean_irradiance_w_m2);
        self.telemetry
            .set(tk::INVERTER_EFFICIENCY, self.inverter_efficiency);
        self.telemetry.set(tk::CURTAILMENT_KW, curtailment_kw);
        self.telemetry
            .set(tk::INVERTER_CLIPPING_KW, inverter_clipping_kw);
        self.telemetry.set(tk::SOILING_RATIO, soiling_ratio);
        self.telemetry.set(tk::SHADING_FACTOR, shading_factor);
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Generation(final_p_kw.max(0.0))),
                reactive_power_kvar: Some(final_q_kvar),
                fuel_w: None,
                thermal_output_w: None,
                sensible_cooling_w: None,
                latent_cooling_w: None,
            },
            state: CoreState {
                operating_mode: None,
                soc: None,
                speed_index: None,
                setpoint_c: None,
            },
            performance: CorePerformance::default(),
        };

        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core_output
    }

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&PvCheckpoint {
            power_limit_kw: self.power_limit_kw,
            curtailment_fraction: self.curtailment_fraction,
            q_setpoint_kvar: self.q_setpoint_kvar,
            inverter_priority: self.inverter_priority,
            power_factor: self.power_factor,
            soiling_config: self.soiling_config.clone(),
            soiling_state: self.soiling_state.clone(),
            shading_model: self.shading_model.clone(),
        })
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: PvCheckpoint = load_postcard(state)?;
        self.power_limit_kw = decoded.power_limit_kw;
        self.curtailment_fraction = decoded.curtailment_fraction;
        self.q_setpoint_kvar = decoded.q_setpoint_kvar;
        self.inverter_priority = decoded.inverter_priority;
        self.power_factor = decoded.power_factor;
        self.soiling_config = decoded.soiling_config;
        self.soiling_state = decoded.soiling_state;
        self.shading_model = decoded.shading_model;
        self.core_output = CoreOutput::default();
        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        match signal {
            ControlSignal::PowerLimit { max_power_kw, .. } => {
                if !max_power_kw.is_finite() {
                    return Err(HaresError::Control(
                        "PV PowerLimit max_power_kw must be finite".to_string(),
                    ));
                }
                self.power_limit_kw = Some((*max_power_kw).max(0.0));
                Ok(())
            }
            ControlSignal::PowerSetpoint {
                active_power_kw,
                reactive_power_kvar,
            } => {
                if !active_power_kw.is_finite() {
                    return Err(HaresError::Control(
                        "PV PowerSetpoint active_power_kw must be finite".to_string(),
                    ));
                }
                // OCHRE semantics: p_set_point = max(max_generation, p_set) where
                // max_generation is negative (generation convention). In HARES'
                // positive-generation convention, active_power_kw is an upper bound
                // on AC output -- store it in power_limit_kw.
                self.power_limit_kw = Some(active_power_kw.max(0.0));
                if let Some(q) = reactive_power_kvar {
                    if !q.is_finite() {
                        return Err(HaresError::Control(
                            "PV PowerSetpoint reactive_power_kvar must be finite".to_string(),
                        ));
                    }
                    self.q_setpoint_kvar = *q;
                }
                Ok(())
            }
            ControlSignal::CurtailmentPercent { percent } => {
                if !percent.is_finite() || *percent < 0.0 || *percent > 100.0 {
                    return Err(HaresError::Control(
                        "PV CurtailmentPercent must be in [0, 100]".to_string(),
                    ));
                }
                self.curtailment_fraction = *percent / 100.0;
                Ok(())
            }
            ControlSignal::ReactiveSetpoint { kvar } => {
                if !kvar.is_finite() {
                    return Err(HaresError::Control(
                        "PV ReactiveSetpoint kvar must be finite".to_string(),
                    ));
                }
                self.q_setpoint_kvar = *kvar;
                Ok(())
            }
            ControlSignal::PowerFactorSetpoint { power_factor } => {
                if !power_factor.is_finite() || *power_factor <= 0.0 || *power_factor > 1.0 {
                    return Err(HaresError::Control(
                        "PV PowerFactorSetpoint must be in (0, 1]".to_string(),
                    ));
                }
                self.power_factor = *power_factor;
                self.q_setpoint_kvar = 0.0;
                Ok(())
            }
            ControlSignal::InverterPriorityMode { priority } => {
                self.inverter_priority = *priority;
                Ok(())
            }
            _ => Err(HaresError::Control(format!(
                "PV does not handle control signal: {signal:?}"
            ))),
        }
    }
}

pub fn register_with_registry(registry: &mut EquipmentRegistry) {
    registry.register("PV", Box::new(|config| Box::new(PV::new(config))));
}

fn telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: tk::DC_POWER_KW.to_string(),
            unit: "kW".to_string(),
            description: "Total PV DC output power before inverter efficiency and curtailment"
                .to_string(),
        },
        TelemetryField {
            name: tk::AC_POWER_KW.to_string(),
            unit: "kW".to_string(),
            description: "Total PV AC output power after inverter efficiency and curtailment"
                .to_string(),
        },
        TelemetryField {
            name: tk::REACTIVE_POWER_KVAR.to_string(),
            unit: "kVAR".to_string(),
            description: "Reactive power output derived from configured static power factor"
                .to_string(),
        },
        TelemetryField {
            name: tk::CELL_TEMP_C.to_string(),
            unit: "C".to_string(),
            description: "Capacity-weighted average PV cell temperature".to_string(),
        },
        TelemetryField {
            name: tk::IRRADIANCE_W_M2.to_string(),
            unit: "W/m2".to_string(),
            description: "Capacity-weighted average incident irradiance on PV arrays".to_string(),
        },
        TelemetryField {
            name: tk::INVERTER_EFFICIENCY.to_string(),
            unit: "-".to_string(),
            description: "Inverter efficiency applied to DC power".to_string(),
        },
        TelemetryField {
            name: tk::CURTAILMENT_KW.to_string(),
            unit: "kW".to_string(),
            description: "Power curtailed by active PowerLimit signal".to_string(),
        },
        TelemetryField {
            name: tk::INVERTER_CLIPPING_KW.to_string(),
            unit: "kW".to_string(),
            description: "Power lost to inverter AC capacity clipping (DC/AC ratio > 1)"
                .to_string(),
        },
        TelemetryField {
            name: tk::SOILING_RATIO.to_string(),
            unit: "-".to_string(),
            description: "PV soiling ratio (1.0 = clean, < 1.0 = soiled). Kimber model."
                .to_string(),
        },
        TelemetryField {
            name: tk::SHADING_FACTOR.to_string(),
            unit: "-".to_string(),
            description: "PV shading factor (1.0 = unshaded, 0.0 = fully shaded)".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use arrow::array::Float64Array;
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;
    use chrono::{FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, EnvironmentState, GridState, InverterPriority, PortSlots, SurfaceIrradiance,
        WeatherState, ZoneId, ZoneState, telemetry_keys as tk,
    };
    use parquet::arrow::ArrowWriter;

    use super::lut::PvLut;
    use super::{
        DEFAULT_GAMMA_PER_C, DEFAULT_NOCT_C, DEFAULT_POWER_FACTOR, DEFAULT_SYSTEM_LOSSES_FRACTION,
        Equipment, EquipmentConfig, ModuleType, NOCT_REFERENCE_IRRADIANCE_W_M2,
        NOCT_REFERENCE_TEMP_C, PV, PvArraySpec, PvConfig, cell_temperature_noct_wind,
        surface_id_for_orientation,
    };

    fn env_with_surfaces(
        surfaces: Vec<SurfaceIrradiance>,
        outdoor_temp_c: f64,
    ) -> EnvironmentState {
        env_with_surfaces_full(surfaces, outdoor_temp_c, 2.0)
    }

    fn env_with_surfaces_full(
        surfaces: Vec<SurfaceIrradiance>,
        outdoor_temp_c: f64,
        wind_speed_m_s: f64,
    ) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 21.0,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: 14.0,
                volume_m3: 250.0,
            }],
            weather: WeatherState {
                outdoor_temp_c,
                outdoor_humidity_ratio: 0.005,
                wind_speed_m_s,
                wind_dir_deg: 180.0,
                ground_temp_c: 12.0,
                sky_temp_c: 8.0,
                pressure_kpa: 101.325,
                solar_irradiance: surfaces,
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
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
                .expect("UTC offset")
                .with_ymd_and_hms(2026, 6, 21, 12, 0, 0)
                .single()
                .expect("valid timestamp"),
            time_res: chrono::Duration::minutes(1),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    fn base_pv_typed_config() -> PvConfig {
        PvConfig {
            equipment_id: Some(29),
            zone_id: None,
            capacity_kw: 5.0,
            tilt_deg: Some(30.0),
            azimuth_deg: Some(180.0),
            module_type: None,
            noct_c: Some(DEFAULT_NOCT_C),
            system_losses_fraction: Some(DEFAULT_SYSTEM_LOSSES_FRACTION),
            inverter_efficiency: Some(0.96),
            inverter_capacity_kw: None,
            power_factor: Some(DEFAULT_POWER_FACTOR),
            surface_resolution_deg: Some(5.0),
            sam_lut_path: None,
            arrays: None,
        }
    }

    fn config_single() -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "PV South".to_string(),
            "PV".to_string(),
            base_pv_typed_config(),
        )
    }

    fn config_single_with_losses(system_losses_fraction: f64) -> EquipmentConfig {
        let mut cfg = base_pv_typed_config();
        cfg.system_losses_fraction = Some(system_losses_fraction);
        EquipmentConfig::from_typed("PV South".to_string(), "PV".to_string(), cfg)
    }

    fn approx_eq(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "left={a}, right={b}");
    }

    fn unique_temp_path(prefix: &str, ext: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}_{}_{}.{}", std::process::id(), nanos, ext))
    }

    fn write_pv_lut_csv(path: &Path, ac_power_kw: f64) {
        let contents =
            format!("month,hour,ghi,dni,dhi,temp_c,ac_power_kw\n6,12,0,0,0,25,{ac_power_kw}\n");
        std::fs::write(path, contents).expect("write pv csv lut");
    }

    fn write_pv_lut_parquet(path: &Path, ac_power_kw: f64) {
        let schema = std::sync::Arc::new(Schema::new(vec![
            Field::new("month", DataType::Float64, false),
            Field::new("hour", DataType::Float64, false),
            Field::new("ghi", DataType::Float64, false),
            Field::new("dni", DataType::Float64, false),
            Field::new("dhi", DataType::Float64, false),
            Field::new("temp_c", DataType::Float64, false),
            Field::new("ac_power_kw", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                std::sync::Arc::new(Float64Array::from(vec![6.0])),
                std::sync::Arc::new(Float64Array::from(vec![12.0])),
                std::sync::Arc::new(Float64Array::from(vec![0.0])),
                std::sync::Arc::new(Float64Array::from(vec![0.0])),
                std::sync::Arc::new(Float64Array::from(vec![0.0])),
                std::sync::Arc::new(Float64Array::from(vec![25.0])),
                std::sync::Arc::new(Float64Array::from(vec![ac_power_kw])),
            ],
        )
        .expect("record batch");
        let file = std::fs::File::create(path).expect("create pv parquet lut");
        let mut writer = ArrowWriter::try_new(file, schema, None).expect("arrow writer");
        writer.write(&batch).expect("write parquet batch");
        writer.close().expect("close parquet writer");
    }

    #[test]
    fn descriptor_and_contract_match_ticket() {
        let pv = PV::new(config_single());
        assert_eq!(pv.descriptor().end_use, hares_types::EndUse::PV);
        assert_eq!(
            pv.descriptor().stage,
            hares_types::ExecutionStage::Independent
        );
        assert!(
            pv.descriptor()
                .control_capabilities
                .contains(hares_types::ControlCapabilities::POWER_LIMIT)
        );
        assert_eq!(pv.ports().len(), 1);
        assert_eq!(pv.ports()[0].port_type, hares_types::PortType::Electrical);
        assert!(
            pv.descriptor()
                .telemetry_fields
                .iter()
                .any(|f| f.name == tk::CURTAILMENT_KW)
        );
    }

    #[test]
    fn init_requires_matching_surface_id() {
        let mut pv = PV::new(config_single());
        let env = env_with_surfaces(vec![], 25.0);
        let err = pv.init(&config_single(), &env).unwrap_err();
        assert!(err.to_string().contains("no matching SurfaceIrradiance"));
    }

    #[test]
    fn zero_irradiance_outputs_zero_power() {
        let mut pv = PV::new(config_single());
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();
        let env = env_with_surfaces(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 0.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            25.0,
        );
        pv.init(&config_single(), &env).unwrap();

        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        approx_eq(pv.telemetry().get(tk::DC_POWER_KW).unwrap_or(-1.0), 0.0);
        approx_eq(pv.telemetry().get(tk::AC_POWER_KW).unwrap_or(-1.0), 0.0);
        approx_eq(ports.electrical.generation_power_kw, 0.0);
    }

    #[test]
    fn temperature_derating_matches_expected_fraction() {
        let cfg = config_single_with_losses(0.0);
        let mut pv = PV::new(cfg.clone());
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();

        // Force T_cell = T_ref + 40C at STC irradiance.
        // T_cell = T_amb + G * (NOCT-20)/800 * wind_corr.
        // At wind=1.0 m/s the wind correction is exactly 1.0.
        // with G=1000, NOCT=47 => increment = 33.75C, so T_amb=31.25C gives 65.0C.
        let env = env_with_surfaces_full(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 1_000.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            31.25,
            1.0,
        );
        pv.init(&cfg, &env).unwrap();

        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let dc = pv.telemetry().get(tk::DC_POWER_KW).unwrap_or(0.0);
        let expected_fraction = 1.0 + DEFAULT_GAMMA_PER_C * 40.0;
        approx_eq(dc, 5.0 * expected_fraction);
    }

    #[test]
    fn multi_array_sums_outputs() {
        let cfg = EquipmentConfig::from_typed(
            "PV Multi".to_string(),
            "PV".to_string(),
            PvConfig {
                capacity_kw: 5.0,
                arrays: Some(vec![
                    PvArraySpec {
                        capacity_kw: 3.0,
                        tilt_deg: Some(30.0),
                        azimuth_deg: Some(180.0),
                        module_type: None,
                        noct_c: Some(DEFAULT_NOCT_C),
                        sam_lut_path: None,
                        attached_boundary_id: None,
                    },
                    PvArraySpec {
                        capacity_kw: 2.0,
                        tilt_deg: Some(20.0),
                        azimuth_deg: Some(90.0),
                        module_type: None,
                        noct_c: Some(DEFAULT_NOCT_C),
                        sam_lut_path: None,
                        attached_boundary_id: None,
                    },
                ]),
                ..base_pv_typed_config()
            },
        );

        let sid0 = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();
        let sid1 = surface_id_for_orientation(20.0, 90.0, 5.0).unwrap();
        let env = env_with_surfaces(
            vec![
                SurfaceIrradiance {
                    surface_id: sid0,
                    direct_w_m2: 700.0,
                    diffuse_w_m2: 100.0,
                    reflected_w_m2: 0.0,
                    angle_of_incidence_rad: 0.0,
                },
                SurfaceIrradiance {
                    surface_id: sid1,
                    direct_w_m2: 200.0,
                    diffuse_w_m2: 50.0,
                    reflected_w_m2: 0.0,
                    angle_of_incidence_rad: 0.0,
                },
            ],
            20.0,
        );

        let mut pv = PV::new(cfg.clone());
        pv.init(&cfg, &env).unwrap();

        assert_eq!(pv.arrays.len(), 2);
        assert_eq!(pv.arrays[0].capacity_kw, 3.0);
        assert_eq!(pv.arrays[1].capacity_kw, 2.0);
        assert_eq!(pv.arrays[0].surface_id, Some(sid0));
        assert_eq!(pv.arrays[1].surface_id, Some(sid1));

        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert!(pv.telemetry().get(tk::AC_POWER_KW).unwrap_or(0.0) > 0.0);
        approx_eq(
            ports.electrical.generation_power_kw,
            -pv.telemetry().get(tk::AC_POWER_KW).unwrap_or(0.0),
        );
    }

    #[test]
    fn multi_array_normalizes_azimuth_360_to_0() {
        let cfg = EquipmentConfig::from_typed(
            "PV Multi 360".to_string(),
            "PV".to_string(),
            PvConfig {
                capacity_kw: 3.0,
                arrays: Some(vec![PvArraySpec {
                    capacity_kw: 3.0,
                    tilt_deg: Some(30.0),
                    azimuth_deg: Some(360.0),
                    module_type: None,
                    noct_c: Some(DEFAULT_NOCT_C),
                    sam_lut_path: None,
                    attached_boundary_id: None,
                }]),
                ..base_pv_typed_config()
            },
        );
        let sid = surface_id_for_orientation(30.0, 0.0, 5.0).unwrap();
        let env = env_with_surfaces(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 700.0,
                diffuse_w_m2: 100.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            20.0,
        );
        let mut pv = PV::new(cfg.clone());
        pv.init(&cfg, &env).unwrap();
        assert_eq!(pv.arrays.len(), 1);
        assert_eq!(pv.arrays[0].azimuth_deg, 0.0);
        assert_eq!(pv.arrays[0].surface_id, Some(sid));
    }

    #[test]
    fn power_limit_curtails_and_reports_telemetry() {
        let mut pv = PV::new(config_single());
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();
        let env = env_with_surfaces(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 1_000.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            20.0,
        );
        pv.init(&config_single(), &env).unwrap();

        pv.apply_control(&ControlSignal::PowerLimit {
            max_power_kw: 1.5,
            ramp_rate_kw_per_s: None,
        })
        .unwrap();

        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        approx_eq(pv.telemetry().get(tk::AC_POWER_KW).unwrap_or(0.0), 1.5);
        assert!(pv.telemetry().get(tk::CURTAILMENT_KW).unwrap_or(0.0) > 0.0);
        approx_eq(ports.electrical.generation_power_kw, -1.5);
    }

    #[test]
    fn save_and_load_state_round_trip_power_limit() {
        let mut pv = PV::new(config_single());
        pv.apply_control(&ControlSignal::PowerLimit {
            max_power_kw: 2.3,
            ramp_rate_kw_per_s: None,
        })
        .unwrap();

        let state = pv.save_state();
        let mut restored = PV::new(config_single());
        restored.load_state(&state).unwrap();
        let state2 = restored.save_state();
        assert_eq!(state, state2);
    }

    #[test]
    fn hpxml_keys_and_module_type_parse_into_array() {
        let cfg = EquipmentConfig::from_typed(
            "PV HPXML".to_string(),
            "PV".to_string(),
            PvConfig {
                equipment_id: None,
                zone_id: None,
                capacity_kw: 4.2,
                tilt_deg: Some(27.0),
                azimuth_deg: Some(200.0),
                module_type: Some("thin_film".to_string()),
                noct_c: Some(DEFAULT_NOCT_C),
                system_losses_fraction: Some(DEFAULT_SYSTEM_LOSSES_FRACTION),
                inverter_efficiency: Some(0.96),
                inverter_capacity_kw: None,
                power_factor: Some(DEFAULT_POWER_FACTOR),
                surface_resolution_deg: Some(5.0),
                sam_lut_path: None,
                arrays: None,
            },
        );
        let sid = surface_id_for_orientation(27.0, 200.0, 5.0).unwrap();
        let env = env_with_surfaces(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 0.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            20.0,
        );
        let mut pv = PV::new(cfg.clone());
        pv.init(&cfg, &env).unwrap();
        assert_eq!(pv.arrays.len(), 1);
        assert_eq!(pv.arrays[0].module_type, ModuleType::ThinFilm);
        assert_eq!(pv.arrays[0].capacity_kw, 4.2);
    }

    #[test]
    fn surface_id_quantization_is_deterministic() {
        let a = surface_id_for_orientation(32.4, 183.0, 5.0).unwrap();
        let b = surface_id_for_orientation(32.3, -177.0, 5.0).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn registry_registers_pv() {
        let registry = crate::EquipmentRegistry::new();
        assert!(registry.get("PV").is_some());
    }

    #[test]
    fn temperature_model_uses_noct_factor_definition() {
        let noct_factor = (DEFAULT_NOCT_C - NOCT_REFERENCE_TEMP_C) / NOCT_REFERENCE_IRRADIANCE_W_M2;
        approx_eq(noct_factor, 0.03375);
    }

    #[test]
    fn wind_correction_is_unity_at_noct_reference_speed() {
        // At NOCT reference wind speed (1 m/s), the wind correction factor
        // must be exactly 1.0, making the result identical to the basic NOCT model.
        let noct = DEFAULT_NOCT_C;
        let t_amb = 20.0;
        let irr = 800.0;
        let basic = t_amb + irr * (noct - NOCT_REFERENCE_TEMP_C) / NOCT_REFERENCE_IRRADIANCE_W_M2;
        let wind_adjusted = cell_temperature_noct_wind(t_amb, irr, noct, 1.0);
        approx_eq(basic, wind_adjusted);
    }

    #[test]
    fn higher_wind_speed_reduces_cell_temperature() {
        let noct = DEFAULT_NOCT_C;
        let t_amb = 25.0;
        let irr = 1000.0;
        let t_calm = cell_temperature_noct_wind(t_amb, irr, noct, 0.5);
        let t_moderate = cell_temperature_noct_wind(t_amb, irr, noct, 5.0);
        let t_windy = cell_temperature_noct_wind(t_amb, irr, noct, 10.0);
        // Cell temp must decrease monotonically with increasing wind speed.
        assert!(t_calm > t_moderate, "calm {t_calm} > moderate {t_moderate}");
        assert!(
            t_moderate > t_windy,
            "moderate {t_moderate} > windy {t_windy}"
        );
        // At 10 m/s the wind correction is 9.5/(5.7+38) = ~0.217, so cell temp
        // rise above ambient should be ~22% of the no-wind rise.
        let rise_calm = t_calm - t_amb;
        let rise_windy = t_windy - t_amb;
        assert!(
            rise_windy < rise_calm * 0.30,
            "windy rise {rise_windy} should be << calm rise {rise_calm}"
        );
    }

    #[test]
    fn wind_cooling_increases_pv_output() {
        // Higher wind → lower cell temp → less temperature derating → more power.
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();
        let mut cfg = base_pv_typed_config();
        cfg.equipment_id = Some(1);
        let cfg = EquipmentConfig::from_typed("PV Wind".to_string(), "PV".to_string(), cfg);

        // Calm day (0.5 m/s)
        let env_calm = env_with_surfaces_full(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 900.0,
                diffuse_w_m2: 100.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            25.0,
            0.5,
        );
        let mut pv_calm = PV::new(cfg.clone());
        pv_calm.init(&cfg, &env_calm).unwrap();
        let mut ports_calm = PortSlots::default();
        pv_calm
            .step(&env_calm, Duration::from_secs(60), &mut ports_calm)
            .unwrap();
        let power_calm = -ports_calm.electrical.generation_power_kw;

        // Windy day (8 m/s)
        let env_windy = env_with_surfaces_full(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 900.0,
                diffuse_w_m2: 100.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            25.0,
            8.0,
        );
        let mut pv_windy = PV::new(cfg.clone());
        pv_windy.init(&cfg, &env_windy).unwrap();
        let mut ports_windy = PortSlots::default();
        pv_windy
            .step(&env_windy, Duration::from_secs(60), &mut ports_windy)
            .unwrap();
        let power_windy = -ports_windy.electrical.generation_power_kw;

        assert!(
            power_windy > power_calm,
            "windy power {power_windy} should exceed calm power {power_calm}"
        );
    }

    // --- Regression tests for code-review fixes ---

    /// When DC output exceeds the inverter AC rating, output must be clamped to
    /// the inverter capacity and the difference tracked as `inverter_clipping_kw`.
    #[test]
    fn inverter_clipping_limits_ac_output() {
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();
        // 5 kW DC array at STC, 96% efficient inverter would give ~4.8 kW AC.
        // Set inverter_capacity_kw = 3.0 to force clipping.
        let mut cfg = base_pv_typed_config();
        cfg.equipment_id = Some(1);
        cfg.inverter_capacity_kw = Some(3.0);
        let cfg = EquipmentConfig::from_typed("PV Clip".to_string(), "PV".to_string(), cfg);

        let env = env_with_surfaces(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 1_000.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            25.0,
        );

        let mut pv = PV::new(cfg.clone());
        pv.init(&cfg, &env).unwrap();

        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let ac_kw = pv.telemetry().get(tk::AC_POWER_KW).unwrap_or(0.0);
        let clipping_kw = pv.telemetry().get(tk::INVERTER_CLIPPING_KW).unwrap_or(0.0);

        // AC output must be clamped at inverter rating.
        approx_eq(ac_kw, 3.0);
        // Clipping must be positive (actual DC-derived AC minus the cap).
        assert!(
            clipping_kw > 0.0,
            "expected inverter_clipping_kw > 0, got {clipping_kw}"
        );
        // Port contribution must reflect clamped value.
        approx_eq(ports.electrical.generation_power_kw, -3.0);
    }

    /// With power_factor=0.9, reactive power must equal P * tan(acos(0.9)).
    #[test]
    fn power_factor_produces_reactive_power() {
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();
        let mut cfg = base_pv_typed_config();
        cfg.equipment_id = Some(2);
        cfg.inverter_efficiency = Some(1.0);
        cfg.power_factor = Some(0.9);
        let cfg = EquipmentConfig::from_typed("PV Q".to_string(), "PV".to_string(), cfg);

        // At 25°C cell temp, 1000 W/m² → DC = 5 kW, AC = 5 kW (eff=1.0, T_derate at T_ref).
        let env = env_with_surfaces(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 1_000.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            25.0,
        );

        let mut pv = PV::new(cfg.clone());
        pv.init(&cfg, &env).unwrap();

        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let ac_kw = pv.telemetry().get(tk::AC_POWER_KW).unwrap_or(0.0);
        let q_kvar = pv.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap_or(0.0);

        assert!(
            q_kvar > 0.0,
            "expected reactive_power_kvar > 0, got {q_kvar}"
        );
        let expected_q = ac_kw * (0.9_f64.acos().tan());
        assert!(
            (q_kvar - expected_q).abs() < 1e-9,
            "expected q={expected_q}, got {q_kvar}"
        );
    }

    /// ThinFilm has a smaller gamma than Standard so it loses less power at elevated
    /// temperatures: at the same high cell temperature, ThinFilm must output more DC.
    #[test]
    fn module_type_gamma_affects_temperature_derating() {
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();

        let make_cfg = |module_type: &str| {
            let mut cfg = base_pv_typed_config();
            cfg.equipment_id = Some(3);
            cfg.inverter_efficiency = Some(1.0);
            cfg.system_losses_fraction = Some(0.0);
            cfg.module_type = Some(module_type.to_string());
            EquipmentConfig::from_typed(format!("PV {module_type}"), "PV".to_string(), cfg)
        };

        // T_amb = 31.25°C → T_cell = 31.25 + 1000*(47-20)/800 = 65°C (40°C above T_ref).
        // Use wind=1.0 so the wind correction factor is exactly 1.0.
        let env = env_with_surfaces_full(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 1_000.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            31.25,
            1.0,
        );

        let mut pv_std = PV::new(make_cfg("standard"));
        let cfg_std = make_cfg("standard");
        pv_std.init(&cfg_std, &env).unwrap();
        let mut ports = PortSlots::default();
        pv_std
            .step(&env, Duration::from_secs(60), &mut ports)
            .unwrap();
        let dc_standard = pv_std.telemetry().get(tk::DC_POWER_KW).unwrap_or(0.0);

        let mut pv_tf = PV::new(make_cfg("thin film"));
        let cfg_tf = make_cfg("thin film");
        pv_tf.init(&cfg_tf, &env).unwrap();
        let mut ports2 = PortSlots::default();
        pv_tf
            .step(&env, Duration::from_secs(60), &mut ports2)
            .unwrap();
        let dc_thinfilm = pv_tf.telemetry().get(tk::DC_POWER_KW).unwrap_or(0.0);

        // ThinFilm gamma is less negative → less derating → higher output at elevated temp.
        assert!(
            dc_thinfilm > dc_standard,
            "ThinFilm ({dc_thinfilm:.4} kW) should exceed Standard ({dc_standard:.4} kW) at elevated temperature"
        );

        // Verify exact values against PVWatts v8 gammas.
        // Standard: gamma = -0.0047, ThinFilm: gamma = -0.0020, delta_T = 40°C.
        approx_eq(dc_standard, 5.0 * (1.0 + (-0.0047_f64) * 40.0));
        approx_eq(dc_thinfilm, 5.0 * (1.0 + (-0.0020_f64) * 40.0));
    }

    /// Raw configs are not supported for PV; typed config is required.
    #[test]
    fn raw_config_rejected_in_init() {
        let mut raw = HashMap::new();
        raw.insert("equipment_id".to_string(), 4.0.into());
        raw.insert("capacity_kw".to_string(), (-1.0_f64).into());
        raw.insert("tilt_deg".to_string(), 30.0.into());
        raw.insert("azimuth_deg".to_string(), 180.0.into());
        raw.insert("surface_resolution_deg".to_string(), 5.0.into());
        let cfg = EquipmentConfig::raw("PV Bad".to_string(), "PV".to_string(), raw);

        // new() must not panic; the error is deferred.
        let mut pv = PV::new(cfg.clone());

        let env = env_with_surfaces(vec![], 25.0);
        let err = pv.init(&cfg, &env).unwrap_err();
        assert!(
            err.to_string().contains("typed config"),
            "expected typed-only error, got: {err}"
        );
    }

    #[test]
    fn typed_sam_lut_csv_drives_ac_output() {
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).expect("surface id");
        let env = env_with_surfaces(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 0.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            25.0,
        );
        let path = unique_temp_path("pv_lut", "csv");
        write_pv_lut_csv(&path, 2.75);

        let mut typed = base_pv_typed_config();
        typed.sam_lut_path = Some(path.to_string_lossy().into_owned());
        typed.inverter_efficiency = Some(1.0);
        let cfg = EquipmentConfig::from_typed("PV".to_string(), "PV".to_string(), typed);
        let mut pv = PV::new(cfg.clone());
        pv.init(&cfg, &env).expect("init pv csv lut");
        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports)
            .expect("step pv csv lut");
        approx_eq(pv.telemetry().get(tk::AC_POWER_KW).unwrap_or(-1.0), 2.75);
    }

    #[test]
    fn typed_sam_lut_parquet_drives_ac_output() {
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).expect("surface id");
        let env = env_with_surfaces(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 0.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            25.0,
        );
        let path = unique_temp_path("pv_lut", "parquet");
        write_pv_lut_parquet(&path, 3.10);

        let mut typed = base_pv_typed_config();
        typed.sam_lut_path = Some(path.to_string_lossy().into_owned());
        typed.inverter_efficiency = Some(1.0);
        let cfg = EquipmentConfig::from_typed("PV".to_string(), "PV".to_string(), typed);
        let mut pv = PV::new(cfg.clone());
        pv.init(&cfg, &env).expect("init pv parquet lut");
        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports)
            .expect("step pv parquet lut");
        approx_eq(pv.telemetry().get(tk::AC_POWER_KW).unwrap_or(-1.0), 3.10);
    }

    // --- System losses fraction tests ---

    #[test]
    fn system_losses_fraction_reduces_dc_power() {
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();
        let surfaces = vec![SurfaceIrradiance {
            surface_id: sid,
            direct_w_m2: 1_000.0,
            diffuse_w_m2: 0.0,
            reflected_w_m2: 0.0,
            angle_of_incidence_rad: 0.0,
        }];

        // Zero losses config.
        let cfg_zero = config_single_with_losses(0.0);
        let env = env_with_surfaces_full(surfaces.clone(), 25.0, 1.0);
        let mut pv_zero = PV::new(cfg_zero.clone());
        pv_zero.init(&cfg_zero, &env).unwrap();
        let mut ports = PortSlots::default();
        pv_zero
            .step(&env, Duration::from_secs(60), &mut ports)
            .unwrap();
        let dc_zero = pv_zero.telemetry().get(tk::DC_POWER_KW).unwrap();

        // Default 14% losses config.
        let cfg_default = config_single();
        let mut pv_default = PV::new(cfg_default.clone());
        pv_default.init(&cfg_default, &env).unwrap();
        let mut ports2 = PortSlots::default();
        pv_default
            .step(&env, Duration::from_secs(60), &mut ports2)
            .unwrap();
        let dc_default = pv_default.telemetry().get(tk::DC_POWER_KW).unwrap();

        approx_eq(dc_default, dc_zero * (1.0 - DEFAULT_SYSTEM_LOSSES_FRACTION));
    }

    #[test]
    fn system_losses_fraction_typed_config_is_accepted() {
        let mut typed = base_pv_typed_config();
        typed.equipment_id = Some(41);
        typed.system_losses_fraction = Some(0.10);
        let cfg = EquipmentConfig::from_typed("PV".to_string(), "PV".to_string(), typed);
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();
        let env = env_with_surfaces(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 0.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            25.0,
        );
        let mut pv = PV::new(cfg.clone());
        pv.init(&cfg, &env).unwrap();
        approx_eq(pv.system_losses_fraction, 0.10);
    }

    #[test]
    fn system_losses_out_of_range_rejected() {
        let mut typed = base_pv_typed_config();
        typed.equipment_id = Some(42);
        typed.system_losses_fraction = Some(1.0);
        let cfg = EquipmentConfig::from_typed("PV".to_string(), "PV".to_string(), typed);
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();
        let env = env_with_surfaces(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 0.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            25.0,
        );
        let mut pv = PV::new(cfg.clone());
        let err = pv.init(&cfg, &env).unwrap_err();
        assert!(
            err.to_string().contains("system_losses_fraction"),
            "expected typed validation error, got: {err}"
        );
    }

    // --- LUT nearest-neighbor normalization test ---

    #[test]
    fn lut_nearest_neighbor_uses_normalized_distance() {
        // Force NN fallback by placing entries at non-adjacent corners of a 3-point
        // month axis, so the query month=6 bracket (month indices 0,1) finds no data.
        // Point A: month=1, GHI=0, output=1.0
        // Point B: month=12, GHI=1200, output=5.0
        // The trilinear bracket for month=6 spans indices 0..1, but neither
        // (0,*,0,*,*,*) nor (1,*,0,*,*,*) exist for the GHI=100 bracket.
        // With normalization, month range=11, GHI range=1200.
        // Point A (month=1,GHI=0): norm_dist = ((6-1)/11)^2 + ((100-0)/1200)^2 = 0.207+0.007 = 0.214
        // Point B (month=12,GHI=1200): norm_dist = ((6-12)/11)^2 + ((100-1200)/1200)^2 = 0.298+0.840 = 1.138
        // So NN picks Point A.
        let lut = PvLut::from_raw(
            vec![1.0, 6.5, 12.0], // 3 month values so query=6 brackets [0,1] (1.0,6.5)
            vec![12.0],
            vec![0.0, 600.0, 1200.0], // 3 GHI values so query=100 brackets [0,1] (0,600)
            vec![500.0],
            vec![100.0],
            vec![25.0],
            vec![
                // Only populate entries at combos that DON'T match the bracket corners
                // for (month in {0,1}, ghi in {0,1}). The bracket tries (0,0),(0,1),(1,0),(1,1)
                // but we only have data at (0,2) and (2,0).
                ([0, 0, 2, 0, 0, 0], 1.0), // month=1, GHI=1200
                ([2, 0, 0, 0, 0, 0], 5.0), // month=12, GHI=0
            ],
        );
        let result = lut.interpolate(6.0, 12.0, 100.0, 500.0, 100.0, 25.0);
        // With normalization: query=(6,100) vs A=(1,1200) vs B=(12,0)
        // Norm month: (6-1)/(12-1)=0.455 for query, (1-1)/11=0 for A, (12-1)/11=1 for B
        // Norm GHI: (100-0)/1200=0.083 for query, (1200-0)/1200=1 for A, (0-0)/1200=0 for B
        // dist_A = (0.455-0)^2 + (0.083-1)^2 = 0.207 + 0.841 = 1.048
        // dist_B = (0.455-1)^2 + (0.083-0)^2 = 0.297 + 0.007 = 0.304
        // B wins (value=5.0)
        approx_eq(result, 5.0);
    }

    // --- Inverter model tests ---

    fn make_inverter_pv(inv_cap_kw: f64) -> (PV, EnvironmentState) {
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();
        let mut typed = base_pv_typed_config();
        typed.equipment_id = Some(30);
        typed.inverter_efficiency = Some(1.0);
        typed.system_losses_fraction = Some(0.0);
        typed.inverter_capacity_kw = Some(inv_cap_kw);
        typed.power_factor = Some(1.0);
        let cfg = EquipmentConfig::from_typed("PV".to_string(), "PV".to_string(), typed);
        let env = env_with_surfaces_full(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 1_000.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            25.0,
            1.0,
        );
        let mut pv = PV::new(cfg.clone());
        pv.init(&cfg, &env).unwrap();
        (pv, env)
    }

    #[test]
    fn inverter_watt_priority_preserves_p_reduces_q() {
        let (mut pv, env) = make_inverter_pv(4.0);
        pv.inverter_priority = InverterPriority::Watt;
        pv.q_setpoint_kvar = 3.0;
        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        let p = pv.telemetry().get(tk::AC_POWER_KW).unwrap();
        let q = pv.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
        let s = (p * p + q * q).sqrt();
        let p_raw = {
            let (mut pv_unlimited, env_unlimited) = make_inverter_pv(6.0);
            pv_unlimited.inverter_priority = InverterPriority::Watt;
            pv_unlimited.q_setpoint_kvar = 3.0;
            let mut ports_unlimited = PortSlots::default();
            pv_unlimited
                .step(
                    &env_unlimited,
                    Duration::from_secs(60),
                    &mut ports_unlimited,
                )
                .unwrap();
            pv_unlimited.telemetry().get(tk::AC_POWER_KW).unwrap()
        };
        let p_expected = p_raw.min(4.0);
        let q_max = (4.0_f64.powi(2) - p_expected.powi(2)).max(0.0).sqrt();
        let q_expected = super::enforce_min_pf(
            p_expected,
            3.0_f64.clamp(-q_max, q_max),
            pv.inverter_min_pf.unwrap(),
        );
        approx_eq(p, p_expected);
        approx_eq(q, q_expected);
        assert!(p <= 4.0 + 1e-9);
        assert!(s <= 4.0 + 1e-9, "S={s} exceeds inverter cap 4.0");
        assert!(q < 3.0, "Q={q} should be reduced from requested 3.0");
    }

    #[test]
    fn inverter_var_priority_preserves_q_reduces_p() {
        let (mut pv, env) = make_inverter_pv(4.0);
        pv.inverter_priority = InverterPriority::Var;
        pv.q_setpoint_kvar = 2.0;
        pv.inverter_min_pf = None;
        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        let p = pv.telemetry().get(tk::AC_POWER_KW).unwrap();
        let q = pv.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
        let s = (p * p + q * q).sqrt();
        approx_eq(q, 2.0);
        let max_p = (4.0_f64.powi(2) - 2.0_f64.powi(2)).sqrt();
        assert!((p - max_p).abs() < 1e-6, "P={p}, expected {max_p}");
        assert!(s <= 4.0 + 1e-9);
    }

    #[test]
    fn inverter_cpf_priority_scales_proportionally() {
        let (mut pv, env) = make_inverter_pv(3.0);
        pv.inverter_priority = InverterPriority::Cpf;
        pv.q_setpoint_kvar = 2.0;
        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        let p = pv.telemetry().get(tk::AC_POWER_KW).unwrap();
        let q = pv.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
        let s = (p * p + q * q).sqrt();
        assert!(s <= 3.0 + 1e-9, "S={s} exceeds inverter cap 3.0");
        let (mut pv_unlimited, env_unlimited) = make_inverter_pv(6.0);
        pv_unlimited.inverter_priority = InverterPriority::Cpf;
        pv_unlimited.q_setpoint_kvar = 2.0;
        let mut ports_unlimited = PortSlots::default();
        pv_unlimited
            .step(
                &env_unlimited,
                Duration::from_secs(60),
                &mut ports_unlimited,
            )
            .unwrap();
        let p_raw = pv_unlimited.telemetry().get(tk::AC_POWER_KW).unwrap();
        let q_raw = pv_unlimited
            .telemetry()
            .get(tk::REACTIVE_POWER_KVAR)
            .unwrap();
        let s_raw = (p_raw * p_raw + q_raw * q_raw).sqrt();
        let scale = 3.0 / s_raw;
        approx_eq(p, p_raw * scale);
        approx_eq(q, q_raw * scale);
        assert!(
            (p_raw / s_raw - p / s).abs() < 1e-9,
            "CPF should preserve power factor"
        );
    }

    #[test]
    fn inverter_under_capacity_no_clipping() {
        let (mut pv, env) = make_inverter_pv(6.0);
        pv.inverter_priority = InverterPriority::Watt;
        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        let clipping = pv.telemetry().get(tk::INVERTER_CLIPPING_KW).unwrap();
        approx_eq(clipping, 0.0);
    }

    // --- Control signal tests ---

    #[test]
    fn curtailment_percent_signal_reduces_output() {
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();
        let env = env_with_surfaces(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 1_000.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            25.0,
        );
        let cfg = config_single();
        let mut pv = PV::new(cfg.clone());
        pv.init(&cfg, &env).unwrap();

        let mut ports_full = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports_full)
            .unwrap();
        let ac_full = pv.telemetry().get(tk::AC_POWER_KW).unwrap();

        pv.apply_control(&ControlSignal::CurtailmentPercent { percent: 50.0 })
            .unwrap();
        let mut ports_half = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports_half)
            .unwrap();
        let ac_half = pv.telemetry().get(tk::AC_POWER_KW).unwrap();

        assert!(
            (ac_half - ac_full * 0.5).abs() < 0.01,
            "50% curtailment: got {ac_half}, expected ~{}",
            ac_full * 0.5
        );
    }

    #[test]
    fn reactive_setpoint_signal_sets_q() {
        let (mut pv, env) = make_inverter_pv(6.0);
        pv.inverter_min_pf = None;
        pv.apply_control(&ControlSignal::ReactiveSetpoint { kvar: 1.5 })
            .unwrap();
        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        let q = pv.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
        approx_eq(q, 1.5);
    }

    #[test]
    fn power_factor_setpoint_signal_computes_q() {
        let (mut pv, env) = make_inverter_pv(6.0);
        pv.inverter_min_pf = None;
        pv.apply_control(&ControlSignal::PowerFactorSetpoint { power_factor: 0.9 })
            .unwrap();
        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        let p = pv.telemetry().get(tk::AC_POWER_KW).unwrap();
        let q = pv.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
        let expected_q = p * (0.9_f64.acos().tan());
        assert!(
            (q - expected_q).abs() < 1e-6,
            "q={q}, expected {expected_q}"
        );
    }

    #[test]
    fn inverter_priority_mode_signal_changes_mode() {
        let mut pv = PV::new(config_single());
        assert_eq!(pv.inverter_priority, InverterPriority::Var);
        pv.apply_control(&ControlSignal::InverterPriorityMode {
            priority: InverterPriority::Watt,
        })
        .unwrap();
        assert_eq!(pv.inverter_priority, InverterPriority::Watt);
        pv.apply_control(&ControlSignal::InverterPriorityMode {
            priority: InverterPriority::Cpf,
        })
        .unwrap();
        assert_eq!(pv.inverter_priority, InverterPriority::Cpf);
    }

    // --- Telemetry field declarations test ---

    #[test]
    fn telemetry_fields_include_all_set_channels() {
        let pv = PV::new(config_single());
        let field_names: Vec<&str> = pv
            .descriptor()
            .telemetry_fields
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert!(field_names.contains(&tk::INVERTER_CLIPPING_KW));
        assert!(field_names.contains(&tk::REACTIVE_POWER_KVAR));
        assert!(field_names.contains(&tk::DC_POWER_KW));
        assert!(field_names.contains(&tk::AC_POWER_KW));
        assert!(field_names.contains(&tk::CURTAILMENT_KW));
    }

    // --- New tests for issue fixes ---

    #[test]
    fn checkpoint_round_trip_includes_all_fields() {
        let mut pv = PV::new(config_single());
        pv.power_limit_kw = Some(3.5);
        pv.curtailment_fraction = 0.25;
        pv.q_setpoint_kvar = 1.2;
        pv.inverter_priority = InverterPriority::Cpf;
        pv.power_factor = 0.85;

        let state = pv.save_state();
        let mut restored = PV::new(config_single());
        restored.load_state(&state).unwrap();

        assert_eq!(restored.power_limit_kw, Some(3.5));
        approx_eq(restored.curtailment_fraction, 0.25);
        approx_eq(restored.q_setpoint_kvar, 1.2);
        assert_eq!(restored.inverter_priority, InverterPriority::Cpf);
        approx_eq(restored.power_factor, 0.85);

        // Double round-trip: serialized bytes must be identical.
        assert_eq!(state, restored.save_state());
    }

    #[test]
    fn power_setpoint_stores_q_value() {
        let mut pv = PV::new(config_single());
        pv.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: 2.0,
            reactive_power_kvar: Some(0.75),
        })
        .unwrap();
        approx_eq(pv.q_setpoint_kvar, 0.75);
    }

    #[test]
    fn var_priority_respects_absolute_kvar_ceiling() {
        let (mut pv, env) = make_inverter_pv(4.0);
        pv.inverter_priority = InverterPriority::Var;
        pv.inverter_min_pf = Some(0.8);
        pv.q_setpoint_kvar = 3.0;
        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        let q = pv.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
        let max_q_cap = 0.8_f64.acos().sin() * 4.0;
        assert!(
            q <= max_q_cap + 1e-9,
            "Q={q} should not exceed absolute ceiling {max_q_cap}"
        );
        approx_eq(q, max_q_cap);
    }

    #[test]
    fn enforce_min_pf_returns_zero_q_at_zero_p() {
        let q = super::enforce_min_pf(0.0, 2.5, 0.8);
        approx_eq(q, 0.0);
    }

    #[test]
    fn system_losses_fraction_is_configurable() {
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();
        let surfaces = vec![SurfaceIrradiance {
            surface_id: sid,
            direct_w_m2: 1_000.0,
            diffuse_w_m2: 0.0,
            reflected_w_m2: 0.0,
            angle_of_incidence_rad: 0.0,
        }];

        let cfg = config_single_with_losses(0.05);
        let env = env_with_surfaces_full(surfaces.clone(), 25.0, 1.0);
        let mut pv = PV::new(cfg.clone());
        pv.init(&cfg, &env).unwrap();
        approx_eq(pv.system_losses_fraction, 0.05);

        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        let dc_5pct = pv.telemetry().get(tk::DC_POWER_KW).unwrap();

        // Compare with 20% losses.
        let cfg2 = config_single_with_losses(0.20);
        let mut pv2 = PV::new(cfg2.clone());
        pv2.init(&cfg2, &env).unwrap();
        let mut ports2 = PortSlots::default();
        pv2.step(&env, Duration::from_secs(60), &mut ports2)
            .unwrap();
        let dc_20pct = pv2.telemetry().get(tk::DC_POWER_KW).unwrap();

        // 5% losses should give more power than 20% losses.
        assert!(
            dc_5pct > dc_20pct,
            "5% losses ({dc_5pct}) should exceed 20% losses ({dc_20pct})"
        );
        // Ratio should be (1-0.05)/(1-0.20) = 0.95/0.80 = 1.1875
        let ratio = dc_5pct / dc_20pct;
        approx_eq(ratio, 0.95 / 0.80);
    }

    /// PowerSetpoint with active_power_kw=2.0 must limit AC generation to ≤ 2.0 kW
    /// even when unconstrained physics would produce ~5 kW.
    #[test]
    fn power_setpoint_active_power_limits_generation() {
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();
        // 5 kW array at STC, no losses, inverter eff=1.0 → ~5 kW unconstrained.
        let mut cfg = base_pv_typed_config();
        cfg.inverter_efficiency = Some(1.0);
        cfg.system_losses_fraction = Some(0.0);
        let cfg = EquipmentConfig::from_typed("PV Setpoint".to_string(), "PV".to_string(), cfg);

        // Use wind=1.0 and T_amb=25°C so T_cell = 25 + 1000*(47-20)/800 = 58.75°C.
        // Temperature derating is slight but the unconstrained AC output is well above 2 kW.
        let env = env_with_surfaces_full(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 1_000.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            25.0,
            1.0,
        );

        let mut pv = PV::new(cfg.clone());
        pv.init(&cfg, &env).unwrap();

        // Confirm unconstrained output is meaningfully above 2.0 kW.
        let mut ports_unconstrained = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports_unconstrained)
            .unwrap();
        let unconstrained_ac = pv.telemetry().get(tk::AC_POWER_KW).unwrap_or(0.0);
        assert!(
            unconstrained_ac > 2.0,
            "unconstrained AC power must be > 2.0 kW, got {unconstrained_ac}"
        );

        // Apply a PowerSetpoint limiting to 2.0 kW.
        pv.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: 2.0,
            reactive_power_kvar: None,
        })
        .unwrap();

        let mut ports_limited = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports_limited)
            .unwrap();
        let limited_ac = pv.telemetry().get(tk::AC_POWER_KW).unwrap_or(0.0);

        assert!(
            limited_ac <= 2.0 + 1e-9,
            "AC generation must be ≤ 2.0 kW after PowerSetpoint, got {limited_ac}"
        );
        // Port contribution must also respect the limit (negative = generation).
        assert!(
            -ports_limited.electrical.generation_power_kw <= 2.0 + 1e-9,
            "port generation_power_kw must be ≤ 2.0 kW, got {}",
            -ports_limited.electrical.generation_power_kw
        );
    }

    /// Verify that shading_model is properly saved and restored in checkpoints.
    #[test]
    fn checkpoint_round_trip_preserves_shading_model() {
        use super::shading::ShadingModel;
        let mut pv = PV::new(config_single());
        pv.shading_model = ShadingModel::FixedLoss {
            annual_fraction: 0.15,
        };

        let state = pv.save_state();
        let mut restored = PV::new(config_single());
        restored.load_state(&state).unwrap();

        match restored.shading_model {
            ShadingModel::FixedLoss { annual_fraction } => {
                approx_eq(annual_fraction, 0.15);
            }
            _ => panic!(
                "expected FixedLoss shading model, got {:?}",
                restored.shading_model
            ),
        }

        // Verify state bytes are identical after round-trip
        assert_eq!(state, restored.save_state());
    }

    /// Verify that shading_factor telemetry is present and reflects the model.
    #[test]
    fn shading_factor_appears_in_telemetry() {
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();
        let env = env_with_surfaces(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 1_000.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            25.0,
        );

        let cfg = EquipmentConfig::from_typed(
            "PV Shading".to_string(),
            "PV".to_string(),
            PvConfig {
                equipment_id: Some(1),
                capacity_kw: 5.0,
                tilt_deg: Some(30.0),
                azimuth_deg: Some(180.0),
                surface_resolution_deg: Some(5.0),
                ..base_pv_typed_config()
            },
        );

        let mut pv = PV::new(cfg.clone());
        pv.init(&cfg, &env).unwrap();
        pv.shading_model = super::shading::ShadingModel::FixedLoss {
            annual_fraction: 0.20,
        };

        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        // Verify telemetry field exists and equals expected value (1.0 - 0.20 = 0.80)
        let shading_factor = pv.telemetry().get(tk::SHADING_FACTOR).unwrap_or(-1.0);
        approx_eq(shading_factor, 0.80);

        // Verify the field is declared in descriptor
        let field_names: Vec<&str> = pv
            .descriptor()
            .telemetry_fields
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert!(
            field_names.contains(&tk::SHADING_FACTOR),
            "shading_factor must be in telemetry_fields"
        );
    }

    /// 5 kW panels, 4 kW inverter at full irradiance: AC output must be capped
    /// at the inverter rating. Use cold ambient (-10 °C) to drive cell temp below
    /// 25 °C so temp derating boosts DC above 5 kW, well above the 4 kW cap.
    #[test]
    fn inverter_capacity_4kw_clips_5kw_panels() {
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();
        let mut cfg = base_pv_typed_config();
        cfg.equipment_id = Some(1);
        cfg.inverter_efficiency = Some(1.0);
        cfg.system_losses_fraction = Some(0.0);
        cfg.inverter_capacity_kw = Some(4.0);
        let cfg =
            EquipmentConfig::from_typed("PV 5kW/4kW inverter".to_string(), "PV".to_string(), cfg);

        // Cold ambient (-10 °C) keeps cell temp well below 25 °C, giving positive
        // temp derating so DC > nameplate 5 kW and definitely above the 4 kW cap.
        // wind=2 m/s: cell_temp ≈ -10 + 1000*(47-20)/800 * 9.5/(5.7+7.6) ≈ -10+19.2 = 9.2 °C
        // temp_derate = 1 + (-0.0047)*(9.2-25) ≈ 1.074 → DC ≈ 5.37 kW > 4 kW cap.
        let env = env_with_surfaces(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 1_000.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            -10.0,
        );

        let mut pv = PV::new(cfg.clone());
        pv.init(&cfg, &env).unwrap();

        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let ac_kw = pv.telemetry().get(tk::AC_POWER_KW).unwrap_or(0.0);
        let dc_kw = pv.telemetry().get(tk::DC_POWER_KW).unwrap_or(0.0);
        let clipping_kw = pv.telemetry().get(tk::INVERTER_CLIPPING_KW).unwrap_or(0.0);

        // DC must exceed the 4 kW cap (cold boost ensures this).
        assert!(
            dc_kw > 4.0,
            "DC power must exceed 4 kW at cold ambient to exercise clipping, got {dc_kw:.3}"
        );
        // AC must be clamped at the 4 kW inverter rating.
        approx_eq(ac_kw, 4.0);
        // Clipping must be positive.
        assert!(
            clipping_kw > 0.0,
            "inverter_clipping_kw must be > 0, got {clipping_kw:.3}"
        );
        // Port contribution must reflect the clamped value.
        approx_eq(ports.electrical.generation_power_kw, -4.0);
    }

    /// When `inverter_capacity_kw` is None (the default), the system resolves
    /// to a 1:1 DC/AC ratio — total DC array capacity becomes the inverter
    /// rating. At cold ambient temperatures where DC power exceeds nameplate
    /// capacity, output must be clipped. This verifies that the default is no
    /// longer "no clipping."
    #[test]
    fn default_inverter_ratio_clips_cold_panels() {
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();
        let mut cfg = base_pv_typed_config();
        cfg.equipment_id = Some(1);
        cfg.inverter_efficiency = Some(1.0);
        cfg.system_losses_fraction = Some(0.0);
        // inverter_capacity_kw stays None — tests the default 1:1 resolving.
        let cfg =
            EquipmentConfig::from_typed("PV default ratio".to_string(), "PV".to_string(), cfg);

        // Cold ambient (-10 °C, wind 2 m/s) pushes cell temp to ~14 °C,
        // giving positive temperature derating so DC (~5.25 kW) exceeds
        // the default inverter capacity (5.0 kW).
        let env = env_with_surfaces(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 1_000.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            -10.0,
        );

        let mut pv = PV::new(cfg.clone());
        pv.init(&cfg, &env).unwrap();

        // After init, inverter_capacity_kw must be resolved to Some(total_dc).
        assert!(pv.inverter_capacity_kw.is_some());
        assert!((pv.inverter_capacity_kw.unwrap() - 5.0).abs() < 1e-9);

        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let ac_kw = pv.telemetry().get(tk::AC_POWER_KW).unwrap_or(0.0);
        let clipping_kw = pv.telemetry().get(tk::INVERTER_CLIPPING_KW).unwrap_or(0.0);

        // Default inverter capacity = total DC capacity (5.0 kW).
        assert!(ac_kw <= 5.0 + 1e-9, "AC {ac_kw} exceeded 5.0 kW cap");
        // Clipping must be positive (cold boost pushes DC above 5.0).
        assert!(
            clipping_kw > 0.0,
            "inverter_clipping_kw must be > 0, got {clipping_kw:.3}"
        );
        approx_eq(ports.electrical.generation_power_kw, -5.0);
    }

    /// With `inverter_capacity_kw = Some(3.0)` and total DC = 5.0 kW, AC
    /// output must be capped at the inverter rating. Cold ambient conditions
    /// ensure the unconstrained DC-derived AC well exceeds the 3 kW cap.
    #[test]
    fn inverter_3kw_caps_5kw_array() {
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();
        let mut cfg = base_pv_typed_config();
        cfg.equipment_id = Some(2);
        cfg.inverter_efficiency = Some(1.0);
        cfg.system_losses_fraction = Some(0.0);
        cfg.inverter_capacity_kw = Some(3.0);
        let cfg = EquipmentConfig::from_typed("PV 5kW/3kW".to_string(), "PV".to_string(), cfg);

        let env = env_with_surfaces(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 1_000.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            -10.0,
        );

        let mut pv = PV::new(cfg.clone());
        pv.init(&cfg, &env).unwrap();

        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let ac_kw = pv.telemetry().get(tk::AC_POWER_KW).unwrap_or(0.0);
        let clipping_kw = pv.telemetry().get(tk::INVERTER_CLIPPING_KW).unwrap_or(0.0);

        // AC must be capped at the 3 kW inverter rating.
        approx_eq(ac_kw, 3.0);
        assert!(
            clipping_kw > 0.0,
            "expected clipping with 3kW inverter, got {clipping_kw:.3}"
        );
        approx_eq(ports.electrical.generation_power_kw, -3.0);
    }
}
