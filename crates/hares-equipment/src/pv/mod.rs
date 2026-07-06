//! Photovoltaic panel equipment model.
//!
//! # Reactive-power sign convention (§3.14 of the PF implementation plan)
//!
//! PV uses ONE signed bus reactive power `Q` on every channel (port,
//! [`CoreOutput`], telemetry): **positive = inductive/absorbing vars,
//! negative = supplying vars**, matching [`CoreFlows::reactive_power_kvar`]
//! and the house total. A generating PV at pf < 1 *supplies* vars, so its
//! baseline bus Q is negative (`-|P_gen| · tan(acos(pf))`). A commanded
//! [`ControlSignal::ReactiveSetpoint`] passes through as-commanded (positive
//! = absorb). Active power remains `Generation(+|P|)` on [`CoreOutput`] and
//! negated on the port (generation subtracts from the load accumulator).

mod array_config;
pub mod config;
mod lut;
pub mod shading;
pub mod soiling;

pub use array_config::{ArrayType, ModuleType, PvArray, PvArraySpec, surface_id_for_orientation};
use array_config::{normalize_azimuth, parse_u32_from_f64};
pub use config::PvConfig;
use lut::{InterpolationMethod, PvLut};

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use chrono::Datelike;
use hares_physics::units::power_kw_to_w;
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CorePerformance,
    CoreState, ElectricPower, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FuelType, HaresError, InverterPriority, OperatingMode, PortContribution,
    PortDeclaration, PortSlots, SurfaceIrradiance, Telemetry, TelemetryField, telemetry_keys as tk,
    zip::ZipLoad,
};
use serde::{Deserialize, Serialize};

use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_versioned, try_save_versioned};

use crate::config::KEY_EQUIPMENT_ID;

/// Nominal operating cell temperature for SAM PVWatts v8 open-rack (array_type=0).
///
/// SAM PVWatts v8 defaults: OpenRack = 45°C, RoofMounted = 49°C,
/// InsulatedBack = 49°C (`cmod_pvwattsv5.cpp:195-197`, `lib_pvwatts.h:26`).
/// HARES uses the open-rack default (most common residential configuration).
const DEFAULT_NOCT_C: f64 = 45.0;
const DEFAULT_INVERTER_EFFICIENCY: f64 = 0.96;
const DEFAULT_SURFACE_RESOLUTION_DEG: f64 = 5.0;
const DEFAULT_T_REF_C: f64 = 25.0;
const DEFAULT_POWER_FACTOR: f64 = 1.0;
/// Temperature coefficient of power for the default Standard module type (PVWatts v8).
const DEFAULT_GAMMA_PER_C: f64 = -0.0047;
/// PVWatts v5 default DC-side system loss fraction (14%).
///
/// The PVWatts v5 default system losses of 14% are defined multiplicatively
/// from component loss multipliers (NREL/TP-7A40-80694, Dobos 2014):
///
/// | Component              | Multiplier |
/// |------------------------|------------|
/// | Soiling                | 0.98       |
/// | Shading                | 0.97       |
/// | Snow                   | 1.00       |
/// | Mismatch               | 0.98       |
/// | Wiring                 | 0.98       |
/// | Connections            | 0.995      |
/// | Light-induced degradation | 0.985   |
/// | Nameplate rating       | 0.99       |
/// | Age                    | 1.00       |
/// | Availability           | 0.97       |
///
/// Product: 0.98×0.97×0.98×0.98×0.995×0.985×0.99×0.97 ≈ 0.86 → loss ≈ 14%.
/// HARES applies this as a linear post-temperature DC derate for simplicity
/// (Δ ≈ 0.07 percentage points vs multiplicative on the 14% default).
const DEFAULT_SYSTEM_LOSSES_FRACTION: f64 = 0.14;
/// Static soiling multiplier in the PVWatts v5 default loss breakdown.
///
/// The 0.98 soiling component (2% loss) is baked into the PVWatts v5 14%
/// default system losses. When a dynamic soiling model (e.g., Kimber) is
/// active, this static component must be removed from
/// `system_losses_fraction` to avoid double-counting.
///
/// Reference: NREL/TP-7A40-80694 (PVWatts Version 5 Manual, Dobos 2014),
/// Table 2: DC-to-AC Derate Factors. Soiling = 0.98 (2.0%).
const PVWATTS_SOILING_COMPONENT: f64 = 0.02;
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

/// Compute direct-path (non-LUT) DC and AC power for a single array.
///
/// Used by the invariant check and observer histogram to compare LUT-path
/// results against the equivalent direct computation. Only compiled when
/// at least one of `test`, `debug_assertions`, `check_invariants`, or
/// `observe` is active — in stripped release builds the function is dead.
#[cfg(any(
    test,
    debug_assertions,
    feature = "check_invariants",
    feature = "observe"
))]
#[inline]
fn compute_direct_power(
    array: &PvArray,
    irradiance_w_m2: f64,
    ambient_temp_c: f64,
    wind_speed_m_s: f64,
    system_losses_fraction: f64,
    inverter_efficiency: f64,
) -> (f64, f64) {
    let cell_temp_c = cell_temperature_noct_wind(
        ambient_temp_c,
        irradiance_w_m2,
        array.noct_c,
        wind_speed_m_s,
    );
    let gamma = array.module_type.gamma_per_c();
    let temp_derate = (1.0 + gamma * (cell_temp_c - DEFAULT_T_REF_C)).max(0.0);
    let mut dc_power_kw =
        array.capacity_kw * (irradiance_w_m2 / IRRADIANCE_AT_STC_W_M2) * temp_derate;
    dc_power_kw *= 1.0 - system_losses_fraction;
    let ac_power_kw = (dc_power_kw * inverter_efficiency).max(0.0);
    (dc_power_kw, ac_power_kw)
}

#[derive(Clone, Debug)]
#[cfg_attr(
    not(feature = "observe"),
    allow(dead_code)
    // Why: fields dc_power_kw_before_losses and lut_path_active are only read
    // inside #[cfg(feature = "observe")] blocks in step(). Without the feature
    // they are written but never read, triggering dead_code. Gating the fields
    // themselves behind #[cfg(feature = "observe")] would require conditional
    // construction at every call site (LUT path, non-LUT path, test helpers),
    // which is more invasive than a single suppression.
)]
struct ArrayStepOutput {
    dc_power_kw: f64,
    ac_power_kw: f64,
    irradiance_w_m2: f64,
    cell_temp_c: f64,
    interp_method: Option<InterpolationMethod>,
    dc_power_kw_before_losses: f64,
    lut_path_active: bool,
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
    /// Effective system loss fraction after soiling-component reconciliation.
    ///
    /// When the Kimber soiling model is active (`soiling_config` is `Some`),
    /// the PVWatts static soiling component (2% = 0.02) is automatically
    /// subtracted from this value to prevent double-counting. Set in
    /// `init_typed()`.
    effective_system_losses_fraction: f64,
    power_limit_kw: Option<f64>,
    curtailment_fraction: f64,
    inverter_priority: InverterPriority,
    inverter_min_pf: Option<f64>,
    q_setpoint_kvar: f64,
    /// ZIP carrier for the inverter power-factor used to derive the baseline
    /// reactive output. Only [`ZipLoad::tan_phi`] is consumed — PV is a
    /// generator, not a voltage-scaled ZIP load, so the reactive polynomial is
    /// not applied; the inverter holds the displacement PF. Kept as a
    /// [`ZipLoad`] so the `tan(acos(pf))` formula is shared with the rest of
    /// HARES via [`hares_types::zip`] rather than duplicated inline. Updated
    /// on init and whenever a [`ControlSignal::PowerFactorSetpoint`] arrives.
    zip_pf: ZipLoad,
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
            zone_type: None,
        };

        let mut telemetry = Telemetry::with_capacity(12);
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
        telemetry.insert(tk::PV_LUT_INTERP_METHOD, 0.0);
        telemetry.insert(tk::PV_LUT_NN_FALLBACK_COUNT, 0.0);

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
            effective_system_losses_fraction: DEFAULT_SYSTEM_LOSSES_FRACTION,
            power_limit_kw: None,
            curtailment_fraction: 0.0,
            inverter_priority: InverterPriority::Var,
            inverter_min_pf: Some(0.8),
            q_setpoint_kvar: 0.0,
            zip_pf: ZipLoad::constant_power(),
            luts_by_surface: HashMap::new(),
            last_ac_power_kw: 0.0,
            soiling_config: None,
            soiling_state: None,
            shading_model,
            init_error,
        }
    }

    /// Compute DC and AC power for a single PV array.
    ///
    /// Two paths exist depending on whether a SAM PVWatts lookup table (LUT)
    /// is configured for this array's surface:
    ///
    /// **Non-LUT path**: computes cell temperature via the SAM-NOCT wind
    /// model, applies temperature derating, then system losses and inverter
    /// efficiency to produce DC and AC power from tilted-surface irradiance.
    ///
    /// **LUT path**: delegates power prediction to a pre-computed SAM PVWatts
    /// LUT that maps solar geometry (zenith, azimuth), irradiance components
    /// (GHI, DNI, DHI), and ambient temperature to AC power. The LUT is
    /// assumed to have been generated **with** the PVWatts v5 default 14%
    /// system losses and 96% inverter efficiency baked in (standard SAM
    /// practice). HARES reverses these baked-in values using the LUT's
    /// embedded SAM metadata (`inv_eff` / `losses`), then re-applies its own
    /// configurable `system_losses_fraction` and `inverter_efficiency` for
    /// consistency with the non-LUT path. Legacy LUTs without SAM metadata
    /// bypass the correction (results may be biased).
    ///
    /// Both paths apply soiling and shading reduction before the power
    /// calculation. The LUT path additionally applies soiling/shading on top
    /// of the LUT's own AC power output.
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
            // Solar zenith from altitude: zenith = 90° - altitude.
            // Clamped to 0° minimum (sun at zenith is 0°).
            let solar_zenith_deg = (90.0 - env.weather.solar_altitude_deg).max(0.0);
            let solar_azimuth_deg = env.weather.solar_azimuth_deg;
            let ghi = env.weather.ghi_w_m2.max(0.0);
            let dni = env.weather.dni_w_m2.max(0.0);
            let dhi = env.weather.dhi_w_m2.max(0.0);
            let (lut_ac_kw, interp_method) = lut.interpolate(
                solar_zenith_deg,
                solar_azimuth_deg,
                ghi,
                dni,
                dhi,
                ambient_temp_c,
            );
            let ac_power_kw = lut_ac_kw.max(0.0) * soiling_ratio * shading_factor;

            let cell_temp_c = cell_temperature_noct_wind(
                ambient_temp_c,
                irradiance_w_m2,
                lut.sam_noct_c().unwrap_or(array.noct_c),
                env.weather.wind_speed_m_s,
            );

            let sam_inv_eff = lut.sam_inv_eff();
            let sam_losses = lut.sam_losses();

            // T-0086: SAM's PVWatts already applies its own internal inverter
            // efficiency (inv_eff) and system losses (losses) when producing
            // the AC output stored in the LUT. Recover the true DC power by
            // dividing these out, then re-apply HARES' configured values so
            // both LUT and non-LUT paths use the same sequence:
            //   DC → system_losses → inverter_efficiency → AC
            //
            // SAM PVWatts v8 defaults: inv_eff = 96%, losses = 14%
            // (NREL/TP-7A40-80694). SSC declares both with unit "%"
            // (cmod_pvwattsv5.cpp:53-54). The Python adapter divides by
            // 100 so the Rust consumer receives fraction form (0.96, 0.14).
            let dc_true = if sam_inv_eff > 0.0 && sam_inv_eff <= 1.0 {
                ac_power_kw / sam_inv_eff / (1.0 - sam_losses).max(1e-9)
            } else {
                ac_power_kw / self.inverter_efficiency.max(1e-9)
            };

            let (dc_power_kw, ac_power_kw) = if sam_inv_eff > 0.0 && sam_inv_eff <= 1.0 {
                let dc_power_kw = dc_true * (1.0 - self.effective_system_losses_fraction);
                let ac_power_kw = dc_power_kw * self.inverter_efficiency;
                (dc_power_kw, ac_power_kw.max(0.0))
            } else {
                // Legacy LUT without SAM metadata — fall back to pre-T-0086
                // behaviour. Results will be biased by ~4-18% due to double-
                // applied inverter efficiency and missing system losses.
                (dc_true, ac_power_kw)
            };

            // T-0107 invariant check: warn when system_losses_fraction
            // deviates from the PVWatts v5 default by more than
            // 1 percentage point. The LUT embeds the SAM default losses;
            // a large deviation may cause inconsistent results between
            // LUT and non-LUT paths.
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            {
                let abs_diff = (self.system_losses_fraction - DEFAULT_SYSTEM_LOSSES_FRACTION).abs();
                if abs_diff > 0.01 {
                    // Why: gated behind check_invariants — using
                    // tracing::warn! because this is a diagnostic, not
                    // a correctness guarantee. The user may deliberately
                    // configure a different loss value.
                    tracing::warn!(
                        system_losses_fraction = self.system_losses_fraction,
                        pvwatts_default = DEFAULT_SYSTEM_LOSSES_FRACTION,
                        abs_diff = abs_diff,
                        "PV system_losses_fraction deviates from PVWatts v5 default \
                         by >1pp. If the SAM LUT was generated with default losses, the \
                         LUT and non-LUT paths may produce inconsistent results.",
                    );
                }
            }

            // Invariant check: in debug/invariant builds, compare LUT-path
            // AC against the direct-path AC computed from the same array
            // specification. This catches metadata-aware correction bugs.
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            {
                let (_, ac_direct) = compute_direct_power(
                    array,
                    irradiance_w_m2,
                    ambient_temp_c,
                    env.weather.wind_speed_m_s,
                    self.effective_system_losses_fraction,
                    self.inverter_efficiency,
                );
                let diff = if ac_direct > 0.0 {
                    (ac_power_kw - ac_direct).abs() / ac_direct
                } else if ac_power_kw > 0.0 {
                    1.0
                } else {
                    0.0
                };
                // T-0086 requires 0.1% relative tolerance. SAM's PVWatts v8
                // uses the same NOCT cell temperature model as HARES and the
                // same DC = capacity*(POA/STC)*temp_derate formula, so the
                // two paths should agree closely when SAM_inv_eff ≈ HARES_inv_eff
                // and SAM_losses ≈ HARES_losses. The LUT transposition from
                // GHI/DNI/DHI to POA may differ from the weather file's POA.
                if diff >= 0.001 {
                    // Why: this is gated behind check_invariants — using
                    // tracing::error! instead of assert! because the feature
                    // can be enabled in release builds and an invariant
                    // diagnostic should not abort the simulation.
                    tracing::error!(
                        lut_ac_kw = ac_power_kw,
                        direct_ac_kw = ac_direct,
                        diff_ratio = diff,
                        "PV LUT vs direct AC power mismatch {diff:.6} exceeds 0.1% threshold",
                    );
                }
            }

            // Observer capture: record LUT vs direct AC power ratio.
            #[cfg(feature = "observe")]
            {
                let (_, ac_direct) = compute_direct_power(
                    array,
                    irradiance_w_m2,
                    ambient_temp_c,
                    env.weather.wind_speed_m_s,
                    self.effective_system_losses_fraction,
                    self.inverter_efficiency,
                );
                let ratio = if ac_direct > 0.0 {
                    ac_power_kw / ac_direct
                } else {
                    f64::NAN
                };
                tracing::debug!(
                    pv_lut_vs_direct_ac_diff_ratio = ratio,
                    lut_ac_kw = ac_power_kw,
                    direct_ac_kw = ac_direct,
                    "PV LUT vs direct comparison",
                );
            }

            // dc_power_kw_before_losses: the raw DC recovered from the LUT
            // AC output before HARES' own system_losses_fraction is applied.
            let dc_before_losses = dc_true;

            return ArrayStepOutput {
                dc_power_kw,
                ac_power_kw,
                irradiance_w_m2,
                cell_temp_c,
                interp_method: Some(interp_method),
                dc_power_kw_before_losses: dc_before_losses,
                lut_path_active: true,
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
        let dc_before_losses =
            array.capacity_kw * (irradiance_w_m2 / IRRADIANCE_AT_STC_W_M2) * temp_derate;
        let mut dc_power_kw = dc_before_losses;
        dc_power_kw *= 1.0 - self.effective_system_losses_fraction;
        let ac_power_kw = (dc_power_kw * self.inverter_efficiency).max(0.0);

        ArrayStepOutput {
            dc_power_kw,
            ac_power_kw,
            irradiance_w_m2,
            cell_temp_c,
            interp_method: None,
            dc_power_kw_before_losses: dc_before_losses,
            lut_path_active: false,
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
                // Preserve the sign of the signed bus_q passed in (positive =
                // absorbing, negative = supplying). The previous code derived
                // the sign from `self.q_setpoint_kvar`, which broke under the
                // unified §3.14 convention: baseline generation has
                // q_setpoint = 0 but bus_q < 0 (supplying), so the old test
                // `q_setpoint >= 0` flipped the sign back to absorbing.
                let q_out = if q_kvar >= 0.0 { q_abs } else { -q_abs };
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
                    let module_type = spec
                        .module_type
                        .as_deref()
                        .map(ModuleType::from_str)
                        .unwrap_or(base.module_type);
                    let array_type = spec
                        .array_type
                        .as_deref()
                        .map(ArrayType::from_str)
                        .transpose()?
                        .unwrap_or(base.array_type);
                    let noct_c = spec.noct_c.unwrap_or(array_type.noct_c());
                    let array = PvArray {
                        tilt_deg,
                        azimuth_deg,
                        capacity_kw: spec.capacity_kw,
                        noct_c,
                        module_type,
                        array_type,
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
            let module_type = c
                .module_type
                .as_deref()
                .map(ModuleType::from_str)
                .unwrap_or(base.module_type);
            let array_type = c
                .array_type
                .as_deref()
                .map(ArrayType::from_str)
                .transpose()?
                .unwrap_or(base.array_type);
            let noct_c = c.noct_c.unwrap_or(array_type.noct_c());

            let array = PvArray {
                tilt_deg,
                azimuth_deg,
                capacity_kw: c.capacity_kw,
                noct_c,
                module_type,
                array_type,
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
        // Carrier for the baseline displacement PF; tan_phi() is used in step()
        // to compute the supplying-vars reactive output. pf=1.0 (default) →
        // tan_phi = 0 → no reactive output.
        self.zip_pf = ZipLoad::reactive_only(0.0, 0.0, 1.0, self.power_factor);
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
                #[cfg(any(debug_assertions, feature = "check_invariants"))]
                check_lut_location(&lut, path);
                #[cfg(not(any(debug_assertions, feature = "check_invariants")))]
                let _ = path;

                // T-0086: warn once at load time if the LUT lacks SAM's
                // internal inverter efficiency and system losses metadata.
                // Legacy LUTs (pre-T-0086 Python adapter) and CSV LUTs omit
                // these fields; the correction defaults to the pre-T-0086
                // fallback, which may produce 4-18% systematic bias.
                let sam_inv_eff = lut.sam_inv_eff();
                if sam_inv_eff <= 0.0 || sam_inv_eff > 1.0 {
                    tracing::warn!(
                        lut_path = %path,
                        sam_inv_eff = sam_inv_eff,
                        "PV LUT lacks SAM inverter efficiency metadata; \
                         fallback applies HARES inverter_efficiency to SAM AC output. \
                         Re-generate LUT with updated sam_pv.py adapter.",
                    );
                }

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

        // T-0108: When the Kimber soiling model is active, subtract the
        // PVWatts static soiling component (2% = 0.02) from
        // system_losses_fraction to prevent double-counting:
        //   1. Kimber dynamic soiling → applied as irradiance reduction
        //   2. system_losses_fraction → applied as DC derate
        // The PVWatts v5 default 14% includes a 0.98 soiling multiplier;
        // without reconciliation, both would apply simultaneously.
        let user_configured_losses = c.system_losses_fraction.is_some();
        self.effective_system_losses_fraction = if self.soiling_config.is_some() {
            let adjusted = (self.system_losses_fraction - PVWATTS_SOILING_COMPONENT).max(0.0);
            if user_configured_losses {
                // The user provided their own system_losses_fraction but the
                // static soiling component was auto-removed. Warn so they can
                // verify the effective value matches their intent.
                tracing::warn!(
                    configured = self.system_losses_fraction,
                    removed = PVWATTS_SOILING_COMPONENT,
                    effective = adjusted,
                    "PV system_losses_fraction reduced by PVWatts static soiling component \
                     (2%) because dynamic Kimber soiling model is active. \
                     Verify effective_system_losses_fraction matches intent.",
                );
            }
            adjusted
        } else {
            self.system_losses_fraction
        };

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

/// Validate LUT location metadata on load.
///
/// Gated behind `debug_assertions` or `feature = "check_invariants"` so the
/// check compiles to nothing in production release builds. Logs the embedded
/// latitude/longitude; warns if metadata is absent (both ≈ 0.0) since that
/// indicates a pre-T-0085 LUT that was regenerated without location metadata.
#[cfg(any(debug_assertions, feature = "check_invariants"))]
fn check_lut_location(lut: &PvLut, path: &str) {
    let lut_lat = lut.latitude_deg();
    let lut_lon = lut.longitude_deg();
    if lut_lat.abs() < 1e-9 && lut_lon.abs() < 1e-9 {
        tracing::warn!(
            lut_path = %path,
            "PV LUT missing location metadata (lat/lon ≈ 0.0); \
             re-generate with updated sam_pv.py adapter",
        );
    } else {
        tracing::info!(
            lut_path = %path,
            lut_latitude_deg = lut_lat,
            lut_longitude_deg = lut_lon,
            "PV LUT loaded with location metadata",
        );
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

        // T-0108 invariant: when soiling is active, verify the soiling ratio
        // is plausible and that combined soiling (dynamic + any residual static)
        // does not exceed 35%, which would indicate a likely misconfiguration.
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            debug_assert!(
                (0.0..=1.0).contains(&soiling_ratio),
                "soiling_ratio {soiling_ratio} out of range [0.0, 1.0]"
            );
            let static_soiling_in_losses = if self.soiling_config.is_some() {
                0.0 // removed by reconciliation
            } else {
                PVWATTS_SOILING_COMPONENT
            };
            let combined = soiling_ratio * (1.0 - static_soiling_in_losses);
            if combined < 0.65 {
                tracing::warn!(
                    soiling_ratio = soiling_ratio,
                    static_soiling_removed = self.soiling_config.is_some(),
                    combined = combined,
                    "PV combined soiling exceeds 35% (>{:.2}); check soiling config and loss parameters",
                    1.0 - combined,
                );
            }
        }

        let mut total_dc_power_kw = 0.0;
        let mut total_ac_power_kw = 0.0;
        let mut total_irradiance_weighted = 0.0;
        let mut total_cell_temp_weighted = 0.0;
        let mut total_capacity_kw = 0.0;
        let mut lut_nn_fallback = false;
        #[cfg(feature = "observe")]
        let (mut any_lut_path, mut total_dc_before_losses) = (false, 0.0);

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
            if output.interp_method == Some(InterpolationMethod::NearestNeighbor) {
                lut_nn_fallback = true;
            }
            #[cfg(feature = "observe")]
            {
                if output.lut_path_active {
                    any_lut_path = true;
                }
                total_dc_before_losses += output.dc_power_kw_before_losses;
            }
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

        // Compute ONE signed bus reactive power [kVAR] used identically for
        // the port push, CoreOutput, and telemetry (§3.14 sign convention:
        // positive = inductive/absorbing vars, negative = supplying vars).
        //
        // Control precedence (plan §1): (1) a nonzero `q_setpoint_kvar`
        //     (from ReactiveSetpoint or PowerSetpoint.reactive_power_kvar)
        //     is an absolute override, passed through as-commanded — positive
        //     = absorbing, negative = supplying; (2) else the displacement
        //     power factor (baseline `PvConfig.power_factor` or the latest
        //     PowerFactorSetpoint) produces `Q = -|P_gen| · tan(acos(pf))` —
        //     a generating inverter at pf < 1 *supplies* vars, so bus Q is
        //     negative; (3) pf = 1.0 (default) → tan_phi = 0 → Q = 0.
        let bus_q_kvar = if self.q_setpoint_kvar != 0.0 {
            self.q_setpoint_kvar
        } else {
            // `total_ac_power_kw` is the positive generation magnitude; the
            // negative sign encodes "supplying vars to the bus."
            -total_ac_power_kw * self.zip_pf.tan_phi()
        };

        // Apply smart inverter limits (handles clipping and priority). The
        // function receives the signed bus_q and returns a magnitude-clamped
        // signed q that respects the inverter's apparent-power rating.
        let (final_p_kw, final_q_kvar) = self.apply_inverter_limits(total_ac_power_kw, bus_q_kvar);
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

        // T-0107 observer capture: record which code path was taken and
        // the DC power before system_losses_fraction was applied. This
        // allows downstream monitoring to compute the effective derate
        // factor and detect path-specific bias.
        // T-0108: also capture soiling reconciliation state.
        #[cfg(feature = "observe")]
        {
            tracing::debug!(
                lut_path_active = any_lut_path,
                system_losses_fraction = self.system_losses_fraction,
                effective_system_losses_fraction = self.effective_system_losses_fraction,
                soiling_ratio = soiling_ratio,
                static_soiling_component_removed = self.soiling_config.is_some(),
                dc_power_kw_before_losses = total_dc_before_losses,
                total_dc_power_kw = total_dc_power_kw,
                "PV step completed",
            );
        }

        // Port push: active power is negated (generation subtracts from the
        // load accumulator), but reactive power is pushed signed and
        // **un-negated** so the port reactive delta equals CoreOutput and
        // telemetry (positive = absorbing, negative = supplying). The
        // debug-build `validate_port_core_electrical_consistency` check
        // requires these three channels to agree exactly.
        ports.accumulate(&PortContribution::Electrical {
            active_power_w: power_kw_to_w(-final_p_kw),
            reactive_power_kvar: final_q_kvar,
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
        self.telemetry
            .set(tk::PV_LUT_INTERP_METHOD, f64::from(lut_nn_fallback));
        if lut_nn_fallback {
            let prev = self
                .telemetry
                .get(tk::PV_LUT_NN_FALLBACK_COUNT)
                .unwrap_or(0.0);
            self.telemetry.set(tk::PV_LUT_NN_FALLBACK_COUNT, prev + 1.0);
        }
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

    fn save_state(&self) -> crate::Result<Vec<u8>> {
        try_save_versioned(
            &PvCheckpoint {
                power_limit_kw: self.power_limit_kw,
                curtailment_fraction: self.curtailment_fraction,
                q_setpoint_kvar: self.q_setpoint_kvar,
                inverter_priority: self.inverter_priority,
                power_factor: self.power_factor,
                soiling_config: self.soiling_config.clone(),
                soiling_state: self.soiling_state.clone(),
                shading_model: self.shading_model.clone(),
            },
            Self::checkpoint_version(),
            "PV",
        )
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: PvCheckpoint = load_versioned(
            state,
            Self::checkpoint_version(),
            "PV",
            self.descriptor().id,
        )?;
        self.power_limit_kw = decoded.power_limit_kw;
        self.curtailment_fraction = decoded.curtailment_fraction;
        self.q_setpoint_kvar = decoded.q_setpoint_kvar;
        self.inverter_priority = decoded.inverter_priority;
        self.power_factor = decoded.power_factor;
        self.soiling_config = decoded.soiling_config;
        self.soiling_state = decoded.soiling_state;
        self.shading_model = decoded.shading_model;
        // Recompute effective losses: soiling_config may have been restored
        // from a checkpoint where the Kimber model was active, and
        // init_typed() computed the default (no-soiling) value before
        // load_state() was called.
        self.effective_system_losses_fraction = if self.soiling_config.is_some() {
            (self.system_losses_fraction - PVWATTS_SOILING_COMPONENT).max(0.0)
        } else {
            self.system_losses_fraction
        };
        self.core_output = CoreOutput::default();
        // CoreOutput cannot be reconstructed from checkpoint data because
        // PV generation depends on live irradiance and environmental
        // conditions (cell temperature, DC power curves) that are not
        // stored in PvCheckpoint.  Reconstruction would require a
        // checkpoint format change.  See ticket Known Limitations.
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
                ..
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
                // HARES PowerFactorSetpoint keeps the (0, 1] *magnitude*
                // semantics validated in `hares_types::control_signal`
                // (`[0, 1]`, further restricted to `(0, 1]` here). The sign of
                // the resulting reactive power follows the supplying convention
                // adopted in §3.14 of the implementation plan: a generating PV
                // at pf < 1 *supplies* vars, so bus Q = -|P| · tan(acos(pf))
                // (negative). This differs from OCHRE `PV.py:194-196`, which
                // encodes the gen-P/consume-Q case via a *negative* signed pf.
                // HARES uses an unsigned PF here and reserves
                // [`ControlSignal::ReactiveSetpoint`] (positive = absorbing)
                // for commanding var *absorption*. The validation is not
                // changed — only the produced sign is now consistent with the
                // rest of HARES.
                self.power_factor = *power_factor;
                self.zip_pf = ZipLoad::reactive_only(0.0, 0.0, 1.0, *power_factor);
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

    fn rename(&mut self, name: String) {
        self.descriptor.name = name;
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
            description: "Signed bus reactive power (positive = inductive/absorbing, \
                negative = supplying). Baseline: -|P|·tan(acos(pf)) at the configured \
                displacement power factor; override: commanded ReactiveSetpoint /
                PowerSetpoint.reactive_power_kvar, passed through as-commanded."
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
        TelemetryField {
            name: tk::PV_LUT_INTERP_METHOD.to_string(),
            unit: "-".to_string(),
            description: "PV LUT interpolation method: 0.0 = multilinear, 1.0 = nearest-neighbor"
                .to_string(),
        },
        TelemetryField {
            name: tk::PV_LUT_NN_FALLBACK_COUNT.to_string(),
            unit: "count".to_string(),
            description: "Cumulative count of nearest-neighbor fallbacks in PV LUT interpolation"
                .to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    use arrow::array::Float64Array;
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;
    use chrono::{FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, ElectricPower, EnvironmentState, GridState, InverterPriority, PortSlots,
        SurfaceIrradiance, WeatherState, ZoneId, ZoneState, telemetry_keys as tk,
        validate_port_core_electrical_consistency,
    };
    use parquet::arrow::ArrowWriter;
    use parquet::file::metadata::KeyValue;
    use parquet::file::properties::WriterProperties;

    use super::lut::{InterpolationMethod, PvLut};
    use super::{
        ArrayType, DEFAULT_GAMMA_PER_C, DEFAULT_INVERTER_EFFICIENCY, DEFAULT_NOCT_C,
        DEFAULT_POWER_FACTOR, DEFAULT_SYSTEM_LOSSES_FRACTION, Equipment, EquipmentConfig,
        ModuleType, NOCT_REFERENCE_IRRADIANCE_W_M2, NOCT_REFERENCE_TEMP_C, PV,
        PVWATTS_SOILING_COMPONENT, PvArray, PvArraySpec, PvConfig, cell_temperature_noct_wind,
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
            array_type: None,
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
        .unwrap()
    }

    fn config_single_with_losses(system_losses_fraction: f64) -> EquipmentConfig {
        let mut cfg = base_pv_typed_config();
        cfg.system_losses_fraction = Some(system_losses_fraction);
        EquipmentConfig::from_typed("PV South".to_string(), "PV".to_string(), cfg).unwrap()
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
        let contents = format!(
            "solar_zenith_deg,solar_azimuth_deg,ghi,dni,dhi,temp_c,ac_power_kw\n\
             30,180,0,0,0,25,{ac_power_kw}\n"
        );
        std::fs::write(path, contents).expect("write pv csv lut");
    }

    fn write_pv_lut_parquet(path: &Path, ac_power_kw: f64) {
        let schema = std::sync::Arc::new(Schema::new(vec![
            Field::new("solar_zenith_deg", DataType::Float64, false),
            Field::new("solar_azimuth_deg", DataType::Float64, false),
            Field::new("ghi", DataType::Float64, false),
            Field::new("dni", DataType::Float64, false),
            Field::new("dhi", DataType::Float64, false),
            Field::new("temp_c", DataType::Float64, false),
            Field::new("ac_power_kw", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                std::sync::Arc::new(Float64Array::from(vec![30.0])),
                std::sync::Arc::new(Float64Array::from(vec![180.0])),
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

    /// Write a Parquet LUT with embedded SAM metadata (inv_eff, losses).
    fn write_pv_lut_parquet_with_meta(
        path: &Path,
        ac_power_kw: f64,
        sam_inv_eff: f64,
        sam_losses: f64,
    ) {
        let schema = std::sync::Arc::new(Schema::new(vec![
            Field::new("solar_zenith_deg", DataType::Float64, false),
            Field::new("solar_azimuth_deg", DataType::Float64, false),
            Field::new("ghi", DataType::Float64, false),
            Field::new("dni", DataType::Float64, false),
            Field::new("dhi", DataType::Float64, false),
            Field::new("temp_c", DataType::Float64, false),
            Field::new("ac_power_kw", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                std::sync::Arc::new(Float64Array::from(vec![30.0])),
                std::sync::Arc::new(Float64Array::from(vec![180.0])),
                std::sync::Arc::new(Float64Array::from(vec![0.0])),
                std::sync::Arc::new(Float64Array::from(vec![0.0])),
                std::sync::Arc::new(Float64Array::from(vec![0.0])),
                std::sync::Arc::new(Float64Array::from(vec![25.0])),
                std::sync::Arc::new(Float64Array::from(vec![ac_power_kw])),
            ],
        )
        .expect("record batch");
        let file = std::fs::File::create(path).expect("create pv parquet lut");
        let props = WriterProperties::builder()
            .set_key_value_metadata(Some(vec![
                KeyValue::new(
                    "harvest_lut_sam_inv_eff".to_string(),
                    format!("{}", sam_inv_eff),
                ),
                KeyValue::new(
                    "harvest_lut_sam_losses".to_string(),
                    format!("{}", sam_losses),
                ),
            ]))
            .build();
        let mut writer = ArrowWriter::try_new(file, schema, Some(props)).expect("arrow writer");
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
        approx_eq(ports.electrical.generation_power_w, 0.0);
    }

    #[test]
    fn temperature_derating_matches_expected_fraction() {
        let cfg = config_single_with_losses(0.0);
        let mut pv = PV::new(cfg.clone());
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();

        // Force T_cell = T_ref + 40C at STC irradiance.
        // T_cell = T_amb + G * (NOCT-20)/800 * wind_corr.
        // At wind=1.0 m/s the wind correction is exactly 1.0.
        // with G=1000, NOCT=45 => increment = 31.25C, so T_amb=33.75C gives 65.0C.
        let env = env_with_surfaces_full(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 1_000.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            33.75,
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
                        array_type: None,
                        sam_lut_path: None,
                        attached_boundary_id: None,
                    },
                    PvArraySpec {
                        capacity_kw: 2.0,
                        tilt_deg: Some(20.0),
                        azimuth_deg: Some(90.0),
                        module_type: None,
                        noct_c: Some(DEFAULT_NOCT_C),
                        array_type: None,
                        sam_lut_path: None,
                        attached_boundary_id: None,
                    },
                ]),
                ..base_pv_typed_config()
            },
        )
        .unwrap();

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
            ports.electrical.generation_power_w,
            -pv.telemetry().get(tk::AC_POWER_KW).unwrap_or(0.0) * 1000.0,
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
                    array_type: None,
                    sam_lut_path: None,
                    attached_boundary_id: None,
                }]),
                ..base_pv_typed_config()
            },
        )
        .unwrap();
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
        approx_eq(ports.electrical.generation_power_w, -1500.0);
    }

    #[test]
    fn save_and_load_state_round_trip_power_limit() {
        let mut pv = PV::new(config_single());
        pv.apply_control(&ControlSignal::PowerLimit {
            max_power_kw: 2.3,
            ramp_rate_kw_per_s: None,
        })
        .unwrap();

        let state = pv.save_state().unwrap();
        let mut restored = PV::new(config_single());
        restored.load_state(&state).unwrap();
        let state2 = restored.save_state().unwrap();
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
                array_type: None,
                system_losses_fraction: Some(DEFAULT_SYSTEM_LOSSES_FRACTION),
                inverter_efficiency: Some(0.96),
                inverter_capacity_kw: None,
                power_factor: Some(DEFAULT_POWER_FACTOR),
                surface_resolution_deg: Some(5.0),
                sam_lut_path: None,
                arrays: None,
            },
        )
        .unwrap();
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
        approx_eq(noct_factor, 0.03125);
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
        let cfg =
            EquipmentConfig::from_typed("PV Wind".to_string(), "PV".to_string(), cfg).unwrap();

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
        let power_calm = -ports_calm.electrical.generation_power_w;

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
        let power_windy = -ports_windy.electrical.generation_power_w;

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
        let cfg =
            EquipmentConfig::from_typed("PV Clip".to_string(), "PV".to_string(), cfg).unwrap();

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
        approx_eq(ports.electrical.generation_power_w, -3000.0);
    }

    /// With power_factor=0.9, a *generating* PV supplies vars, so the signed
    /// bus Q must equal `-|P| · tan(acos(0.9))` (negative) on every channel:
    /// port == CoreOutput == telemetry.
    #[test]
    fn power_factor_produces_reactive_power() {
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();
        let mut cfg = base_pv_typed_config();
        cfg.equipment_id = Some(2);
        cfg.inverter_efficiency = Some(1.0);
        cfg.power_factor = Some(0.9);
        let cfg = EquipmentConfig::from_typed("PV Q".to_string(), "PV".to_string(), cfg).unwrap();

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
        let co_q = pv
            .core_output()
            .flows
            .reactive_power_kvar
            .expect("PV REACTIVE cap => CoreOutput Q must be Some");

        // Generating PV at pf<1 supplies vars → bus Q is negative.
        assert!(
            q_kvar < 0.0,
            "expected negative reactive_power_kvar (supplying), got {q_kvar}"
        );
        let expected_q = -ac_kw * (0.9_f64.acos().tan());
        assert!(
            (q_kvar - expected_q).abs() < 1e-9,
            "expected q={expected_q}, got {q_kvar}"
        );
        // Unified convention: telemetry, CoreOutput, and port reactive delta
        // must all carry the same signed value.
        approx_eq(q_kvar, co_q);
        approx_eq(ports.electrical.reactive_power_kvar, q_kvar);
        // Active power: port generation is negative; CoreOutput is Generation(+|P|).
        approx_eq(ports.electrical.generation_power_w, -ac_kw * 1000.0);
        assert!(matches!(
            pv.core_output().flows.electric_kw,
            Some(ElectricPower::Generation(_))
        ));
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
            EquipmentConfig::from_typed(format!("PV {module_type}"), "PV".to_string(), cfg).unwrap()
        };

        // T_amb = 33.75°C → T_cell = 33.75 + 1000*(45-20)/800 = 65°C (40°C above T_ref).
        // Use wind=1.0 so the wind correction factor is exactly 1.0.
        let env = env_with_surfaces_full(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 1_000.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            33.75,
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
        // LUT entry: zenith=30°, azimuth=180° → solar_altitude=60°, azimuth=180°.
        let mut env = env_with_surfaces(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 0.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            25.0,
        );
        env.weather.solar_altitude_deg = 60.0;
        env.weather.solar_azimuth_deg = 180.0;
        let path = unique_temp_path("pv_lut", "csv");
        write_pv_lut_csv(&path, 2.75);

        let mut typed = base_pv_typed_config();
        typed.sam_lut_path = Some(path.to_string_lossy().into_owned());
        typed.inverter_efficiency = Some(1.0);
        let cfg = EquipmentConfig::from_typed("PV".to_string(), "PV".to_string(), typed).unwrap();
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
        // LUT entry: zenith=30°, azimuth=180° → solar_altitude=60°, azimuth=180°.
        let mut env = env_with_surfaces(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 0.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            25.0,
        );
        env.weather.solar_altitude_deg = 60.0;
        env.weather.solar_azimuth_deg = 180.0;
        let path = unique_temp_path("pv_lut", "parquet");
        write_pv_lut_parquet(&path, 3.10);

        let mut typed = base_pv_typed_config();
        typed.sam_lut_path = Some(path.to_string_lossy().into_owned());
        typed.inverter_efficiency = Some(1.0);
        let cfg = EquipmentConfig::from_typed("PV".to_string(), "PV".to_string(), typed).unwrap();
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
        let cfg = EquipmentConfig::from_typed("PV".to_string(), "PV".to_string(), typed).unwrap();
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
        let cfg = EquipmentConfig::from_typed("PV".to_string(), "PV".to_string(), typed).unwrap();
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
        // zenith axis, so the query zenith=45 bracket (indices 0,1) finds no data.
        // Point A: zenith=10, GHI=1200, output=1.0
        // Point B: zenith=80, GHI=0, output=5.0
        // The trilinear bracket for zenith=45 spans indices 0..1, but neither
        // (0,*,0,*,*,*) nor (1,*,0,*,*,*) exist for the GHI=100 bracket.
        // With normalization, zenith range=70, GHI range=1200.
        // Point A (zenith=10,GHI=1200): norm_dist = ((45-10)/70)^2 + ((100-1200)/1200)^2 = 0.25+0.84 = 1.09
        // Point B (zenith=80,GHI=0): norm_dist = ((45-80)/70)^2 + ((100-0)/1200)^2 = 0.25+0.007 = 0.257
        // So NN picks B (value=5.0).
        let lut = PvLut::from_raw(
            vec![10.0, 40.0, 80.0], // 3 zenith values → query=45 brackets [1,2] (40,80) — no, query=45
            vec![180.0],            // azimuth values unchanged
            vec![0.0, 600.0, 1200.0], // 3 GHI values → query=100 brackets [0,1] (0,600)
            vec![500.0],
            vec![100.0],
            vec![25.0],
            vec![
                ([0, 0, 2, 0, 0, 0], 1.0), // zenith=10, azimuth=180, GHI=1200
                ([2, 0, 0, 0, 0, 0], 5.0), // zenith=80, azimuth=180, GHI=0
            ],
        );
        let (result, _) = lut.interpolate(45.0, 180.0, 100.0, 500.0, 100.0, 25.0);
        // query=(45,100) vs A=(10,1200) vs B=(80,0)
        // Norm zenith: (45-10)/(80-10)=0.5 for query, (10-10)/70=0 for A, (80-10)/70=1 for B
        // Norm GHI: (100-0)/1200=0.083 for query, (1200-0)/1200=1 for A, (0-0)/1200=0 for B
        // dist_A = (0.5-0)^2 + (0.083-1)^2 = 0.25 + 0.841 = 1.091
        // dist_B = (0.5-1)^2 + (0.083-0)^2 = 0.25 + 0.007 = 0.257
        // B wins (value=5.0)
        approx_eq(result, 5.0);
    }

    #[test]
    fn lut_sparse_grid_falls_back_to_nearest_neighbor() {
        // Sparse LUT: only 2 entries in a 6-point axis bracket — far fewer
        // than the 2^6 = 64 corners needed for multi-linear.  Querying
        // between the gap forces nearest-neighbor fallback.
        let lut = PvLut::from_raw(
            vec![20.0, 40.0, 60.0],
            vec![160.0, 180.0, 200.0],
            vec![200.0, 500.0, 800.0],
            vec![150.0, 450.0, 750.0],
            vec![50.0, 150.0, 250.0],
            vec![15.0, 25.0, 35.0],
            // Only 2 corners populated (~7% of 64) — multi-linear must fail.
            vec![
                ([0, 0, 0, 0, 0, 0], 1.5), // (20,160,200,150,50,15)
                ([2, 2, 2, 2, 2, 2], 4.0), // (60,200,800,750,250,35)
            ],
        );
        let (_val, method) = lut.interpolate(40.0, 180.0, 500.0, 450.0, 150.0, 25.0);
        assert_eq!(
            method,
            InterpolationMethod::NearestNeighbor,
            "sparse LUT with <64 corners must fall back to nearest-neighbor"
        );
    }

    #[test]
    fn lut_dense_grid_uses_multilinear_interpolation() {
        // Dense LUT: all 64 corners of every 2-element bracket are
        // populated.  Multi-linear interpolation must succeed.
        let zens = vec![20.0, 70.0];
        let azims = vec![180.0];
        let ghis = vec![200.0, 800.0];
        let dnis = vec![150.0, 600.0];
        let dhis = vec![50.0, 200.0];
        let temps = vec![25.0];
        // Populate all 2^6 = 64 entries.
        let mut entries = Vec::with_capacity(64);
        for zi in [0usize, 1] {
            for ai in [0usize] {
                for gi in [0usize, 1] {
                    for di in [0usize, 1] {
                        for dhi in [0usize, 1] {
                            {
                                let ti = 0usize;
                                let val =
                                    (zi as f64) * 10.0 + (gi as f64) * 2.0 + (di as f64) * 0.5;
                                entries.push(([zi, ai, gi, di, dhi, ti], val));
                            }
                        }
                    }
                }
            }
        }
        let lut = PvLut::from_raw(zens, azims, ghis, dnis, dhis, temps, entries);
        let (_val, method) = lut.interpolate(45.0, 180.0, 500.0, 375.0, 125.0, 25.0);
        assert_eq!(
            method,
            InterpolationMethod::Multilinear,
            "dense LUT with all 64 corners must use multi-linear interpolation"
        );
    }

    #[test]
    fn lut_interpolation_method_telemetry_updated_on_fallback() {
        // Verify that after a step with a sparse LUT (NN fallback),
        // the telemetry method is set to 1.0 and the fallback count
        // increments.

        let path = unique_temp_path("pv_lut_nn_telemetry", "parquet");
        // Write a sparse Parquet LUT with only 2 entries in a 6-element grid.
        let schema = Arc::new(Schema::new(vec![
            Field::new("solar_zenith_deg", DataType::Float64, false),
            Field::new("solar_azimuth_deg", DataType::Float64, false),
            Field::new("ghi", DataType::Float64, false),
            Field::new("dni", DataType::Float64, false),
            Field::new("dhi", DataType::Float64, false),
            Field::new("temp_c", DataType::Float64, false),
            Field::new("ac_power_kw", DataType::Float64, false),
        ]));
        let zeniths = Float64Array::from(vec![20.0, 60.0]);
        let azimuths = Float64Array::from(vec![180.0, 180.0]);
        let ghis = Float64Array::from(vec![200.0, 800.0]);
        let dnis = Float64Array::from(vec![150.0, 600.0]);
        let dhis = Float64Array::from(vec![50.0, 200.0]);
        let temps = Float64Array::from(vec![25.0, 25.0]);
        let acs = Float64Array::from(vec![1.5, 4.0]);
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(zeniths),
                Arc::new(azimuths),
                Arc::new(ghis),
                Arc::new(dnis),
                Arc::new(dhis),
                Arc::new(temps),
                Arc::new(acs),
            ],
        )
        .unwrap();
        let file = std::fs::File::create(&path).unwrap();
        let kv = vec![
            KeyValue {
                key: "harvest_lut_latitude_deg".into(),
                value: Some("40.0".into()),
            },
            KeyValue {
                key: "harvest_lut_longitude_deg".into(),
                value: Some("-105.0".into()),
            },
        ];
        let props = WriterProperties::builder()
            .set_key_value_metadata(Some(kv))
            .build();
        let mut writer = ArrowWriter::try_new(file, batch.schema(), Some(props)).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();

        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();
        let mut typed = base_pv_typed_config();
        typed.equipment_id = Some(30);
        typed.sam_lut_path = Some(path.to_string_lossy().into_owned());
        typed.inverter_efficiency = Some(1.0);
        typed.system_losses_fraction = Some(0.0);
        let cfg = EquipmentConfig::from_typed("PV".to_string(), "PV".to_string(), typed).unwrap();
        let env = env_with_surfaces_full(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 800.0,
                diffuse_w_m2: 100.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            25.0,
            1.0,
        );
        let mut pv = PV::new(cfg.clone());
        pv.init(&cfg, &env).expect("init pv with sparse lut");
        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports)
            .expect("step pv with sparse lut");
        assert!(
            pv.telemetry().get(tk::PV_LUT_INTERP_METHOD).unwrap_or(0.0) >= 0.5,
            "sparse LUT must set interp method to nearest-neighbor (1.0)"
        );
        assert!(
            pv.telemetry()
                .get(tk::PV_LUT_NN_FALLBACK_COUNT)
                .unwrap_or(0.0)
                > 0.0,
            "sparse LUT must increment NN fallback count"
        );
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
        let cfg = EquipmentConfig::from_typed("PV".to_string(), "PV".to_string(), typed).unwrap();
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

    /// PowerFactorSetpoint updates the displacement PF used for the baseline
    /// supplying-vars computation. A generating PV at the commanded pf < 1
    /// supplies vars, so bus Q = `-|P| · tan(acos(pf))` (negative). The
    /// setpoint also zeros any prior ReactiveSetpoint, re-engaging the PF
    /// baseline path.
    #[test]
    fn power_factor_setpoint_signal_computes_q() {
        let (mut pv, env) = make_inverter_pv(6.0);
        pv.inverter_min_pf = None;
        pv.apply_control(&ControlSignal::PowerFactorSetpoint { power_factor: 0.9 })
            .unwrap();
        // PowerFactorSetpoint must clear a prior ReactiveSetpoint so the PF
        // baseline path runs.
        assert_eq!(pv.q_setpoint_kvar, 0.0);
        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        let p = pv.telemetry().get(tk::AC_POWER_KW).unwrap();
        let q = pv.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
        let expected_q = -p * (0.9_f64.acos().tan());
        assert!(
            (q - expected_q).abs() < 1e-6,
            "q={q}, expected {expected_q} (negative = supplying)"
        );
        assert!(q < 0.0, "generating PV at pf<1 must supply vars (Q<0)");
        // Port, CoreOutput, and telemetry agree on the signed value.
        approx_eq(ports.electrical.reactive_power_kvar, q);
        approx_eq(pv.core_output().flows.reactive_power_kvar.expect("Some"), q);
    }

    /// §3.14 sign-pinning: baseline pf<1 generation produces negative bus Q
    /// (supplying vars) and the signed value is identical across port,
    /// CoreOutput, and telemetry. Also exercises the real dwelling-level
    /// `validate_port_core_electrical_consistency` validator against the
    /// equipment's own pre/post port snapshots — the same check the dwelling
    /// step runs in debug builds. This is the equipment-level mirror of the
    /// dwelling-level PV reactive test (extending the dwelling PV harness with
    /// a real pf<1 PV + weather/surfaces setup is not cheap; the validator is
    /// public and called directly here for an equivalent, stronger guarantee).
    #[test]
    fn pv_baseline_pf_supplies_vars_and_passes_port_core_validator() {
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();
        let mut cfg = base_pv_typed_config();
        cfg.equipment_id = Some(11);
        cfg.inverter_efficiency = Some(1.0);
        cfg.system_losses_fraction = Some(0.0);
        // Large enough that neither active clipping nor the kVA limit binds
        // (S needed at pf 0.9 is ~5.6 kVA), small enough that the DC-to-AC
        // ratio (5.0 / 6.0 ≈ 0.83) stays inside the validated [0.8, 2.0].
        cfg.inverter_capacity_kw = Some(6.0);
        cfg.power_factor = Some(0.9);
        let cfg =
            EquipmentConfig::from_typed("PV Sign".to_string(), "PV".to_string(), cfg).unwrap();

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
        pv.inverter_min_pf = None; // isolate the PF baseline path from min-pf clamping

        let mut ports = PortSlots::default();
        let pre = ports.electrical; // Copy snapshot — all zeros before step
        pv.step(&env, Duration::from_secs(60), &mut ports)
            .expect("step must succeed");

        let ac_kw = pv.telemetry().get(tk::AC_POWER_KW).unwrap();
        let q_telem = pv.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
        let q_core = pv
            .core_output()
            .flows
            .reactive_power_kvar
            .expect("REACTIVE cap => CoreOutput Q Some");

        // Generating PV at pf<1 supplies vars → bus Q is negative.
        assert!(
            ac_kw > 0.0,
            "test precondition: PV must be generating, got AC={ac_kw}"
        );
        assert!(
            q_telem < 0.0,
            "baseline pf<1 must supply vars (Q<0), got {q_telem}"
        );
        let expected_q = -ac_kw * (0.9_f64.acos().tan());
        approx_eq(q_telem, expected_q);

        // Unified convention: port == CoreOutput == telemetry (signed).
        approx_eq(ports.electrical.reactive_power_kvar, q_telem);
        approx_eq(q_core, q_telem);

        // Active power: port generation is negative; CoreOutput is Generation(+).
        approx_eq(ports.electrical.generation_power_w, -ac_kw * 1000.0);
        assert!(matches!(
            pv.core_output().flows.electric_kw,
            Some(ElectricPower::Generation(_))
        ));

        // The dwelling-level validator must accept the unified push.
        validate_port_core_electrical_consistency(
            pv.descriptor(),
            pv.core_output(),
            pre,
            &ports.electrical,
        )
        .expect("baseline pf<1 generation must satisfy the port/core consistency validator");
    }

    /// §3.14: a positive ReactiveSetpoint (absorbing) passes through
    /// as-commanded on every channel, and survives the consistency validator.
    #[test]
    fn pv_reactive_setpoint_positive_passthrough() {
        let (mut pv, env) = make_inverter_pv(6.0);
        pv.inverter_min_pf = None;
        pv.apply_control(&ControlSignal::ReactiveSetpoint { kvar: 1.5 })
            .unwrap();
        let mut ports = PortSlots::default();
        let pre = ports.electrical;
        pv.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let q = pv.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
        assert!(
            q > 0.0,
            "positive ReactiveSetpoint must absorb (Q>0), got {q}"
        );
        approx_eq(q, 1.5);
        approx_eq(ports.electrical.reactive_power_kvar, q);
        approx_eq(pv.core_output().flows.reactive_power_kvar.expect("Some"), q);
        validate_port_core_electrical_consistency(
            pv.descriptor(),
            pv.core_output(),
            pre,
            &ports.electrical,
        )
        .expect("positive ReactiveSetpoint must satisfy the consistency validator");
    }

    /// §3.14: a negative ReactiveSetpoint (supplying) passes through
    /// as-commanded on every channel, sign preserved through the inverter's
    /// Var-priority limiter, and survives the consistency validator.
    #[test]
    fn pv_reactive_setpoint_negative_passthrough() {
        let (mut pv, env) = make_inverter_pv(6.0);
        pv.inverter_min_pf = None;
        pv.apply_control(&ControlSignal::ReactiveSetpoint { kvar: -1.5 })
            .unwrap();
        let mut ports = PortSlots::default();
        let pre = ports.electrical;
        pv.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let q = pv.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
        assert!(
            q < 0.0,
            "negative ReactiveSetpoint must supply (Q<0), got {q}"
        );
        approx_eq(q, -1.5);
        approx_eq(ports.electrical.reactive_power_kvar, q);
        approx_eq(pv.core_output().flows.reactive_power_kvar.expect("Some"), q);
        validate_port_core_electrical_consistency(
            pv.descriptor(),
            pv.core_output(),
            pre,
            &ports.electrical,
        )
        .expect("negative ReactiveSetpoint must satisfy the consistency validator");
    }

    /// §3.14 control precedence: PowerFactorSetpoint zeros a prior
    /// ReactiveSetpoint so the PF baseline path re-engages (negative Q for
    /// generating PV at pf<1).
    #[test]
    fn pv_power_factor_setpoint_zeros_q_setpoint() {
        let (mut pv, env) = make_inverter_pv(6.0);
        pv.inverter_min_pf = None;
        // First command an absorbing ReactiveSetpoint.
        pv.apply_control(&ControlSignal::ReactiveSetpoint { kvar: 1.5 })
            .unwrap();
        assert_eq!(pv.q_setpoint_kvar, 1.5);
        // Then a PowerFactorSetpoint must clear it and re-engage the PF path.
        pv.apply_control(&ControlSignal::PowerFactorSetpoint { power_factor: 0.85 })
            .unwrap();
        assert_eq!(pv.q_setpoint_kvar, 0.0);
        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        let p = pv.telemetry().get(tk::AC_POWER_KW).unwrap();
        let q = pv.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
        let expected_q = -p * (0.85_f64.acos().tan());
        approx_eq(q, expected_q);
        assert!(
            q < 0.0,
            "PF baseline after PowerFactorSetpoint must supply vars"
        );
    }

    /// §3.14: inverter apparent-power limit clamps reactive magnitude for both
    /// signs (absorbing and supplying) while preserving the sign. With a large
    /// |Q| command and small P, |S| = sqrt(P²+Q²) must not exceed the inverter
    /// kVA rating.
    #[test]
    fn pv_inverter_clamps_reactive_both_signs() {
        for &q_cmd in &[8.0_f64, -8.0_f64] {
            let (mut pv, env) = make_inverter_pv(4.0);
            pv.inverter_min_pf = None;
            pv.inverter_priority = InverterPriority::Var;
            pv.apply_control(&ControlSignal::ReactiveSetpoint { kvar: q_cmd })
                .unwrap();
            let mut ports = PortSlots::default();
            pv.step(&env, Duration::from_secs(60), &mut ports).unwrap();
            let p = pv.telemetry().get(tk::AC_POWER_KW).unwrap();
            let q = pv.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
            let s = (p * p + q * q).sqrt();
            assert!(
                s <= 4.0 + 1e-6,
                "q_cmd={q_cmd}: |S|={s} must respect inverter cap 4.0"
            );
            // Sign preserved (|q| reduced from 8.0 toward the cap).
            assert!(
                q.signum() == q_cmd.signum(),
                "q_cmd={q_cmd}: sign must be preserved, got q={q}"
            );
            assert!(
                q.abs() < 8.0,
                "q_cmd={q_cmd}: |Q|={0} must be clamped below 8.0",
                q.abs()
            );
            // Unified convention still holds.
            approx_eq(ports.electrical.reactive_power_kvar, q);
            approx_eq(pv.core_output().flows.reactive_power_kvar.expect("Some"), q);
        }
    }

    #[test]
    fn inverter_priority_mode_signal_changes_mode() {
        let mut pv = PV::new(config_single());
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
        assert!(field_names.contains(&tk::PV_LUT_INTERP_METHOD));
        assert!(field_names.contains(&tk::PV_LUT_NN_FALLBACK_COUNT));
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

        let state = pv.save_state().unwrap();
        let mut restored = PV::new(config_single());
        restored.load_state(&state).unwrap();

        assert_eq!(restored.power_limit_kw, Some(3.5));
        approx_eq(restored.curtailment_fraction, 0.25);
        approx_eq(restored.q_setpoint_kvar, 1.2);
        assert_eq!(restored.inverter_priority, InverterPriority::Cpf);
        approx_eq(restored.power_factor, 0.85);

        // Double round-trip: serialized bytes must be identical.
        assert_eq!(state, restored.save_state().unwrap());
    }

    #[test]
    fn power_setpoint_stores_q_value() {
        let mut pv = PV::new(config_single());
        pv.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: 2.0,
            reactive_power_kvar: Some(0.75),
            min_soc: None,
            max_soc: None,
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
        let cfg =
            EquipmentConfig::from_typed("PV Setpoint".to_string(), "PV".to_string(), cfg).unwrap();

        // Use wind=1.0 and T_amb=25°C so T_cell = 25 + 1000*(45-20)/800 = 56.25°C.
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
            min_soc: None,
            max_soc: None,
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
            -ports_limited.electrical.generation_power_w <= 2000.0 + 1e-3,
            "port generation_power_w must be ≤ 2000.0 W (2.0 kW), got {}",
            -ports_limited.electrical.generation_power_w
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

        let state = pv.save_state().unwrap();
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
        assert_eq!(state, restored.save_state().unwrap());
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
        )
        .unwrap();

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
            EquipmentConfig::from_typed("PV 5kW/4kW inverter".to_string(), "PV".to_string(), cfg)
                .unwrap();

        // Cold ambient (-10 °C) keeps cell temp well below 25 °C, giving positive
        // temp derating so DC > nameplate 5 kW and definitely above the 4 kW cap.
        // wind=2 m/s: cell_temp ≈ -10 + 1000*(45-20)/800 * 9.5/(5.7+7.6) ≈ -10+17.9 = 7.9 °C
        // temp_derate = 1 + (-0.0047)*(7.9-25) ≈ 1.080 → DC ≈ 5.40 kW > 4 kW cap.
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
        approx_eq(ports.electrical.generation_power_w, -4000.0);
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
            EquipmentConfig::from_typed("PV default ratio".to_string(), "PV".to_string(), cfg)
                .unwrap();

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
        approx_eq(ports.electrical.generation_power_w, -5000.0);
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
        let cfg =
            EquipmentConfig::from_typed("PV 5kW/3kW".to_string(), "PV".to_string(), cfg).unwrap();

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
        approx_eq(ports.electrical.generation_power_w, -3000.0);
    }

    // --- T-0085: solar-position-aware LUT tests ---

    /// A LUT indexed on solar zenith must produce different AC power at
    /// different sun positions for the same horizontal irradiance inputs.
    /// This confirms that the solar-position axes are functional: PVWatts
    /// internally translates GHI/DNI/DHI to tilted-surface irradiance
    /// based on sun geometry, so entries at different zenith angles encode
    /// different POA-tilted→AC mappings.
    #[test]
    fn lut_solar_position_affects_power_output() {
        // Two entries at different zenith angles (same azimuth, same
        // irradiance/temperature) with different AC power — simulating
        // how PVWatts produces more POA at low zenith (sun overhead)
        // than at high zenith (sun near horizon).
        let lut = PvLut::from_raw(
            vec![20.0, 70.0], // zenith: low=overhead, high=near horizon
            vec![180.0],      // azimuth: due south
            vec![800.0],      // ghi
            vec![600.0],      // dni
            vec![200.0],      // dhi
            vec![25.0],       // temp
            vec![
                ([0, 0, 0, 0, 0, 0], 4.2), // zenith=20° → high POA → high power
                ([1, 0, 0, 0, 0, 0], 1.3), // zenith=70° → low POA → low power
            ],
        );

        // Query at exactly the entry points — interpolate returns the exact
        // entry value when the query matches a bracket point.
        let (power_overhead, _) = lut.interpolate(20.0, 180.0, 800.0, 600.0, 200.0, 25.0);
        let (power_horizon, _) = lut.interpolate(70.0, 180.0, 800.0, 600.0, 200.0, 25.0);

        approx_eq(power_overhead, 4.2);
        approx_eq(power_horizon, 1.3);
        assert!(
            power_overhead > power_horizon,
            "overhead zenith=20° power ({power_overhead:.2}) must exceed horizon zenith=70° power ({power_horizon:.2})"
        );
    }

    /// Integration test: the same LUT queried with identical horizontal
    /// irradiance (GHI=800, DNI=600, DHI=200, temp=25) but different
    /// solar geometries produces different AC power. This verifies the
    /// fix works end-to-end — the LUT no longer treats all sun positions
    /// identically.
    #[test]
    fn lut_different_solar_geometries_produce_different_power() {
        // Build a 4-entry LUT across two zenith values and two GHI values.
        // At zenith=20°, high GHI gives 5.0 kW, low GHI gives 0.5 kW.
        // At zenith=70°, high GHI gives 2.0 kW, low GHI gives 0.1 kW.
        let lut = PvLut::from_raw(
            vec![20.0, 70.0],
            vec![180.0],
            vec![200.0, 800.0],
            vec![150.0, 600.0],
            vec![50.0, 200.0],
            vec![25.0],
            vec![
                ([0, 0, 0, 0, 0, 0], 0.5), // zenith=20, GHI=200
                ([0, 0, 1, 1, 1, 0], 5.0), // zenith=20, GHI=800
                ([1, 0, 0, 0, 0, 0], 0.1), // zenith=70, GHI=200
                ([1, 0, 1, 1, 1, 0], 2.0), // zenith=70, GHI=800
            ],
        );

        // At GHI=800: zenith=20° → 5.0 kW, zenith=70° → 2.0 kW
        let (p_low, _) = lut.interpolate(20.0, 180.0, 800.0, 600.0, 200.0, 25.0);
        let (p_high, _) = lut.interpolate(70.0, 180.0, 800.0, 600.0, 200.0, 25.0);

        approx_eq(p_low, 5.0);
        approx_eq(p_high, 2.0);
        assert!(
            (p_low - p_high).abs() > 0.0,
            "zenith=20° power ({p_low:.2}) must differ from zenith=70° power ({p_high:.2})"
        );

        // Verify the difference exceeds 2% of the higher value — per the
        // ticket's acceptance threshold.
        let diff_pct = (p_low - p_high).abs() / p_low * 100.0;
        assert!(
            diff_pct > 2.0,
            "power difference {diff_pct:.1}% must exceed 2% threshold"
        );
    }

    /// CSV LUT files don't carry location metadata (Parquet file-level
    /// key-value metadata is Parquet-only). The loader defaults to
    /// lat=lon=0.0, which the invariant check emits a warning for.
    #[test]
    fn lut_csv_defaults_location_to_zero() {
        let path = unique_temp_path("pv_lut_no_meta", "csv");
        write_pv_lut_csv(&path, 2.5);
        let lut = PvLut::from_path(&path).expect("load csv lut");
        approx_eq(lut.latitude_deg(), 0.0);
        approx_eq(lut.longitude_deg(), 0.0);
    }

    // --- T-0086: LUT metadata-aware inverter efficiency & system losses ---

    /// LUT with SAM metadata (inv_eff=0.96, losses=0.14), HARES configured
    /// with the same values. The corrected LUT-path AC power must equal the
    /// raw LUT AC power (correction is a no-op when SAM values match HARES).
    #[test]
    fn lut_metadata_matching_values_produces_identity_correction() {
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).expect("surface id");

        // LUT entry: zenith=30°, azimuth=180°, temp=25°C — at zero irradiance
        // so AC power = 0.0. Use a single entry for simplicity.
        // The correction math: AC_corrected = AC_lut / inv_eff / (1-losses) * (1-losses) * inv_eff
        // When SAM and HARES values match, this simplifies to AC_lut.
        let path = unique_temp_path("pv_lut_t0086_match", "parquet");
        let lut_ac = 3.80;
        write_pv_lut_parquet_with_meta(&path, lut_ac, 0.96, 0.14);

        let mut typed = base_pv_typed_config();
        typed.sam_lut_path = Some(path.to_string_lossy().into_owned());
        typed.inverter_efficiency = Some(0.96);
        typed.system_losses_fraction = Some(0.14);
        let cfg = EquipmentConfig::from_typed("PV".to_string(), "PV".to_string(), typed).unwrap();

        let mut env = env_with_surfaces(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 0.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            25.0,
        );
        // Weather must match LUT entry coordinates for exact hit.
        env.weather.solar_altitude_deg = 60.0; // zenith = 90-60 = 30
        env.weather.solar_azimuth_deg = 180.0;
        env.weather.ghi_w_m2 = 0.0;
        env.weather.dni_w_m2 = 0.0;
        env.weather.dhi_w_m2 = 0.0;

        let mut pv = PV::new(cfg.clone());
        pv.init(&cfg, &env).expect("init pv");
        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports)
            .expect("step pv");

        let ac = pv.telemetry().get(tk::AC_POWER_KW).unwrap_or(-1.0);
        // When SAM=HARES, corrected AC must equal the raw LUT AC.
        approx_eq(ac, lut_ac);
    }

    /// LUT with SAM metadata (inv_eff=0.96, losses=0.14), but HARES
    /// configured with different values (inverter_efficiency=0.90,
    /// system_losses_fraction=0.10). The corrected AC must match the
    /// algebraic expectation: AC_lut / SAM_inv_eff / (1-SAM_losses)
    /// * (1-HARES_losses) * HARES_inv_eff.
    #[test]
    fn lut_metadata_different_values_adjusts_output() {
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).expect("surface id");

        let lu_ac = 3.80;
        let path = unique_temp_path("pv_lut_t0086_diff", "parquet");
        write_pv_lut_parquet_with_meta(&path, lu_ac, 0.96, 0.14);

        let mut typed = base_pv_typed_config();
        typed.sam_lut_path = Some(path.to_string_lossy().into_owned());
        typed.inverter_efficiency = Some(0.90);
        typed.system_losses_fraction = Some(0.10);
        let cfg = EquipmentConfig::from_typed("PV".to_string(), "PV".to_string(), typed).unwrap();

        let mut env = env_with_surfaces(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 0.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            25.0,
        );
        env.weather.solar_altitude_deg = 60.0;
        env.weather.solar_azimuth_deg = 180.0;
        env.weather.ghi_w_m2 = 0.0;
        env.weather.dni_w_m2 = 0.0;
        env.weather.dhi_w_m2 = 0.0;

        let mut pv = PV::new(cfg.clone());
        pv.init(&cfg, &env).expect("init pv");
        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports)
            .expect("step pv");

        let ac = pv.telemetry().get(tk::AC_POWER_KW).unwrap_or(-1.0);
        // Correction: dc_true = 3.80 / 0.96 / (1-0.14) = 3.80 / 0.96 / 0.86 = 4.5988
        // dc_harves = 4.5988 * (1-0.10) = 4.1389
        // ac_harves = 4.1389 * 0.90 = 3.725
        let expected_ac = lu_ac / 0.96 / (1.0 - 0.14) * (1.0 - 0.10) * 0.90;
        approx_eq(ac, expected_ac);
    }

    /// Legacy LUT (CSV, no metadata) falls back to pre-T-0086 behavior:
    /// DC = AC / HARES_inv_eff, and AC passes through unchanged.
    /// The output must match the old (pre-fix) computation.
    #[test]
    fn legacy_lut_no_metadata_falls_back_to_old_behavior() {
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).expect("surface id");

        let path = unique_temp_path("pv_lut_t0086_legacy", "csv");
        write_pv_lut_csv(&path, 2.75);

        let mut typed = base_pv_typed_config();
        typed.sam_lut_path = Some(path.to_string_lossy().into_owned());
        typed.inverter_efficiency = Some(0.96);
        let cfg = EquipmentConfig::from_typed("PV".to_string(), "PV".to_string(), typed).unwrap();

        let mut env = env_with_surfaces(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 0.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            25.0,
        );
        env.weather.solar_altitude_deg = 60.0;
        env.weather.solar_azimuth_deg = 180.0;

        let mut pv = PV::new(cfg.clone());
        pv.init(&cfg, &env).expect("init pv");
        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports)
            .expect("step pv");

        let ac = pv.telemetry().get(tk::AC_POWER_KW).unwrap_or(-1.0);
        let dc = pv.telemetry().get(tk::DC_POWER_KW).unwrap_or(-1.0);

        // Pre-T-0086 legacy math: AC = LUT AC, DC = AC / inv_eff.
        approx_eq(ac, 2.75);
        approx_eq(dc, 2.75 / 0.96);
    }

    /// Verify that the LUT metadata fields are correctly read from a
    /// Parquet file written with SAM configuration embedded.
    #[test]
    fn lut_parquet_reads_sam_metadata() {
        let path = unique_temp_path("pv_lut_t0086_read_meta", "parquet");
        write_pv_lut_parquet_with_meta(&path, 2.5, 0.92, 0.12);
        let lut = PvLut::from_path(&path).expect("load lut");
        approx_eq(lut.sam_inv_eff(), 0.92);
        approx_eq(lut.sam_losses(), 0.12);
    }

    /// CSV LUTs always default SAM metadata to zero (no key-value
    /// metadata support in CSV format).
    #[test]
    fn lut_csv_defaults_sam_metadata_to_zero() {
        let path = unique_temp_path("pv_lut_t0086_csv", "csv");
        write_pv_lut_csv(&path, 1.0);
        let lut = PvLut::from_path(&path).expect("load csv lut");
        approx_eq(lut.sam_inv_eff(), 0.0);
        approx_eq(lut.sam_losses(), 0.0);
    }

    /// Verify that the compute_direct_power helper produces results
    /// identical to the non-LUT path at STC with zero losses and unity
    /// inverter efficiency.
    #[test]
    fn compute_direct_power_matches_step_one_array() {
        let array = PvArray {
            tilt_deg: 30.0,
            azimuth_deg: 180.0,
            capacity_kw: 5.0,
            noct_c: DEFAULT_NOCT_C,
            module_type: ModuleType::Standard,
            array_type: ArrayType::OpenRack,
            surface_id: Some(1),
            sam_lut_path: None,
            attached_boundary_id: None,
        };

        // At STC: irradiance=1000, temp=25, wind=1.0, losses=0, inv_eff=1.0
        let (dc, ac) = super::compute_direct_power(&array, 1000.0, 25.0, 1.0, 0.0, 1.0);

        // T_cell = 25 + 1000*(45-20)/800 = 25 + 31.25 = 56.25
        // temp_derate = 1 + (-0.0047)*(56.25-25) = 1 - 0.146875 = 0.853125
        // DC = 5.0 * 1.0 * 0.853125 = 4.265625 kW
        let t_cell = cell_temperature_noct_wind(25.0, 1000.0, DEFAULT_NOCT_C, 1.0);
        let derate = 1.0 + DEFAULT_GAMMA_PER_C * (t_cell - 25.0);
        let expected_dc = 5.0 * derate;
        let expected_ac = expected_dc * 1.0;

        approx_eq(dc, expected_dc);
        approx_eq(ac, expected_ac);
    }

    // --- T-0087: ArrayType and NOCT default tests ---

    #[test]
    fn default_noct_c_matches_sam_open_rack() {
        // SAM PVWatts v8 `lib_pvwatts.h:26`: PVWATTS_INOCT = 45.0 + 273.15 [K].
        // HARES uses the open-rack default as the most common residential
        // configuration, matching SAM's array_type=0.
        approx_eq(DEFAULT_NOCT_C, 45.0);
    }

    #[test]
    fn array_type_noct_mapping_matches_sam() {
        // SAM PVWatts v8 `cmod_pvwattsv5.cpp:195-197`:
        //   array_type=0 (open rack)    → NOCT = 45°C
        //   array_type=1 (roof mount)   → NOCT = 49°C
        //   array_type=2 (insulated)    → NOCT = 49°C
        assert_eq!(ArrayType::OpenRack.noct_c(), 45.0);
        assert_eq!(ArrayType::RoofMounted.noct_c(), 49.0);
        assert_eq!(ArrayType::InsulatedBack.noct_c(), 49.0);
    }

    #[test]
    fn array_type_from_str_is_case_insensitive() {
        assert_eq!(
            ArrayType::from_str("OpenRack").unwrap(),
            ArrayType::OpenRack
        );
        assert_eq!(
            ArrayType::from_str("open_rack").unwrap(),
            ArrayType::OpenRack
        );
        assert_eq!(
            ArrayType::from_str("OPEN RACK").unwrap(),
            ArrayType::OpenRack
        );
        assert_eq!(
            ArrayType::from_str("RoofMounted").unwrap(),
            ArrayType::RoofMounted
        );
        assert_eq!(
            ArrayType::from_str("roof_mounted").unwrap(),
            ArrayType::RoofMounted
        );
        assert_eq!(
            ArrayType::from_str("ROOF MOUNTED").unwrap(),
            ArrayType::RoofMounted
        );
        assert_eq!(
            ArrayType::from_str("InsulatedBack").unwrap(),
            ArrayType::InsulatedBack
        );
        assert_eq!(
            ArrayType::from_str("insulated_back").unwrap(),
            ArrayType::InsulatedBack
        );
        assert_eq!(
            ArrayType::from_str("INSULATED BACK").unwrap(),
            ArrayType::InsulatedBack
        );
        assert!(ArrayType::from_str("unknown").is_err());
    }

    #[test]
    fn init_derives_noct_from_array_type() {
        // When noct_c is None and array_type is RoofMounted, the derived
        // NOCT should be 49°C (not the default 45°C).
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();
        let mut cfg = base_pv_typed_config();
        cfg.noct_c = None;
        cfg.array_type = Some("roof_mounted".to_string());
        let cfg =
            EquipmentConfig::from_typed("PV Roof".to_string(), "PV".to_string(), cfg).unwrap();
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
        assert_eq!(pv.arrays.len(), 1);
        assert_eq!(pv.arrays[0].noct_c, 49.0);
        assert_eq!(pv.arrays[0].array_type, ArrayType::RoofMounted);
    }

    #[test]
    fn explicit_noct_c_overrides_array_type() {
        // When both noct_c and array_type are set, explicit noct_c wins.
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();
        let mut cfg = base_pv_typed_config();
        cfg.noct_c = Some(42.0);
        cfg.array_type = Some("roof_mounted".to_string());
        let cfg =
            EquipmentConfig::from_typed("PV Explicit".to_string(), "PV".to_string(), cfg).unwrap();
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
        assert_eq!(pv.arrays.len(), 1);
        // Explicit noct_c should win, not the array_type-derived 49°C.
        assert_eq!(pv.arrays[0].noct_c, 42.0);
    }

    /// Regression: at STC (ambient=25°C, G=1000 W/m², wind=1 m/s) with
    /// OpenRack (NOCT=45°C), the cell temperature from the SAM-NOCT model
    /// should match 56.25°C within 0.5°C.
    ///
    /// SAM PVWatts v8: T_cell = T_amb + G/800 * (NOCT-20) at reference wind.
    /// OpenRack (NOCT=45°C): T_cell = 25 + 1000/800 * 25 = 56.25°C.
    #[test]
    fn open_rack_cell_temp_matches_sam_at_stc() {
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();
        let mut cfg = base_pv_typed_config();
        cfg.noct_c = None;
        cfg.array_type = Some("OpenRack".to_string());
        cfg.equipment_id = Some(1);
        let cfg =
            EquipmentConfig::from_typed("PV OpenRack".to_string(), "PV".to_string(), cfg).unwrap();
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

        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let cell_temp_c = pv.telemetry().get(tk::CELL_TEMP_C).unwrap_or(-1.0);
        // SAM PVWatts v8 OpenRack at STC: T_cell = 56.25°C
        let expected = 56.25;
        assert!(
            (cell_temp_c - expected).abs() < 0.5,
            "cell temp {cell_temp_c:.2}°C should be within 0.5°C of SAM OpenRack NOCT {expected}°C"
        );
    }

    /// PvLut with SAM array_type metadata uses the correct NOCT for cell
    /// temperature ancillary calculation in the LUT path.
    #[test]
    fn lut_array_type_metadata_drives_cell_temp() {
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).expect("surface id");
        let mut env = env_with_surfaces(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 800.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            30.0,
        );
        env.weather.solar_altitude_deg = 60.0;
        env.weather.solar_azimuth_deg = 180.0;
        env.weather.wind_speed_m_s = 1.0;

        let path = unique_temp_path("pv_lut", "parquet");
        write_pv_lut_parquet_with_array_type(&path, 3.0, 0.96, 0.14, 1);

        let mut cfg = base_pv_typed_config();
        cfg.noct_c = Some(45.0); // OpenRack NOCT
        cfg.array_type = Some("OpenRack".to_string());
        cfg.sam_lut_path = Some(path.to_string_lossy().into_owned());
        cfg.inverter_efficiency = Some(1.0);
        let cfg =
            EquipmentConfig::from_typed("PV LUT AT".to_string(), "PV".to_string(), cfg).unwrap();
        let mut pv = PV::new(cfg.clone());
        pv.init(&cfg, &env)
            .expect("init pv lut with array_type metadata");
        let mut ports = PortSlots::default();
        pv.step(&env, Duration::from_secs(60), &mut ports)
            .expect("step pv lut with array_type metadata");

        // LUT has array_type=1 (RoofMounted, NOCT=49°C). The cell temp
        // ancillary should use 49°C, not the configured 45°C.
        let cell_temp_c = pv.telemetry().get(tk::CELL_TEMP_C).unwrap_or(-1.0);
        // At 30°C ambient, 800 W/m², wind=1.0:
        // NOCT=45 → T_cell = 30 + 800*(45-20)/800 = 55°C
        // NOCT=49 → T_cell = 30 + 800*(49-20)/800 = 59°C
        assert!(
            cell_temp_c > 57.0,
            "cell temp {cell_temp_c:.2}°C should reflect LUT RoofMounted NOCT (49°C), \
             not config OpenRack (45°C); expected > 57°C"
        );
    }

    /// Write a Parquet LUT with embedded SAM metadata including array_type.
    fn write_pv_lut_parquet_with_array_type(
        path: &std::path::Path,
        ac_power_kw: f64,
        sam_inv_eff: f64,
        sam_losses: f64,
        sam_array_type: u8,
    ) {
        use arrow::array::Float64Array;
        use arrow::datatypes::{DataType, Field, Schema};
        use arrow::record_batch::RecordBatch;
        use parquet::arrow::ArrowWriter;
        use parquet::file::metadata::KeyValue;
        use parquet::file::properties::WriterProperties;

        let schema = std::sync::Arc::new(Schema::new(vec![
            Field::new("solar_zenith_deg", DataType::Float64, false),
            Field::new("solar_azimuth_deg", DataType::Float64, false),
            Field::new("ghi", DataType::Float64, false),
            Field::new("dni", DataType::Float64, false),
            Field::new("dhi", DataType::Float64, false),
            Field::new("temp_c", DataType::Float64, false),
            Field::new("ac_power_kw", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                std::sync::Arc::new(Float64Array::from(vec![30.0])),
                std::sync::Arc::new(Float64Array::from(vec![180.0])),
                std::sync::Arc::new(Float64Array::from(vec![0.0])),
                std::sync::Arc::new(Float64Array::from(vec![0.0])),
                std::sync::Arc::new(Float64Array::from(vec![0.0])),
                std::sync::Arc::new(Float64Array::from(vec![25.0])),
                std::sync::Arc::new(Float64Array::from(vec![ac_power_kw])),
            ],
        )
        .expect("record batch");
        let file = std::fs::File::create(path).expect("create pv parquet lut");
        let props = WriterProperties::builder()
            .set_key_value_metadata(Some(vec![
                KeyValue::new(
                    "harvest_lut_sam_inv_eff".to_string(),
                    format!("{}", sam_inv_eff),
                ),
                KeyValue::new(
                    "harvest_lut_sam_losses".to_string(),
                    format!("{}", sam_losses),
                ),
                KeyValue::new(
                    "harvest_lut_sam_array_type".to_string(),
                    format!("{}", sam_array_type),
                ),
            ]))
            .build();
        let mut writer = ArrowWriter::try_new(file, schema, Some(props)).expect("arrow writer");
        writer.write(&batch).expect("write parquet batch");
        writer.close().expect("close parquet writer");
    }

    // --- T-0107: LUT vs non-LUT parity with matching system_losses_fraction ---

    /// When HARES `system_losses_fraction` and `inverter_efficiency` match the
    /// SAM values embedded in the LUT metadata, both the LUT and non-LUT paths
    /// produce identical `dc_power_kw` for the same array configuration at the
    /// same irradiance and temperature conditions.
    ///
    /// The test uses a synthetic Parquet LUT with one entry at STC-equivalent
    /// conditions (POA ≈ 1000 W/m², ambient=25°C, wind=1 m/s). The LUT's AC
    /// output is set to what the non-LUT path computes at these conditions, so
    /// the LUT correction is a no-op (identity) and both paths converge.
    #[test]
    fn lut_non_lut_parity_matching_losses() {
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).expect("surface id");

        // STC-like conditions. Use wind=1.0 so wind correction factor is 1.0,
        // making the NOCT formula identical to the basic model:
        //   T_cell = 25 + 1000*(45-20)/800 = 56.25°C
        //   temp_derate = 1 + (-0.0047)*(56.25-25) = 0.853125
        //   DC_no_losses = 5.0 * 1.0 * 0.853125 = 4.265625 kW
        //   DC = 4.265625 * (1-0.14) = 3.6684375 kW
        //   AC = 3.6684375 * 0.96 = 3.5217 kW
        let capacity_kw = 5.0;
        let sam_losses = 0.14;
        let sam_inv_eff = 0.96;
        let hares_losses = sam_losses;
        let hares_inv_eff = sam_inv_eff;
        let ambient_c = 25.0;

        let t_cell = cell_temperature_noct_wind(ambient_c, 1000.0, DEFAULT_NOCT_C, 1.0);
        let derate = 1.0 + DEFAULT_GAMMA_PER_C * (t_cell - 25.0);
        let expected_dc_no_losses = capacity_kw * derate; // 4.265625
        let expected_dc = expected_dc_no_losses * (1.0 - hares_losses); // 3.6684375
        let expected_ac = expected_dc * hares_inv_eff;

        // LUT AC output at the matching coordinates: set to expected_ac so
        // the LUT correction is identity.
        let path = unique_temp_path("pv_lut_t0107_parity", "parquet");
        write_pv_lut_parquet_with_meta(&path, expected_ac, sam_inv_eff, sam_losses);

        // Non-LUT PV: no LUT path, confirms baseline.
        let cfg_no_lut = EquipmentConfig::from_typed(
            "PV NoLUT".to_string(),
            "PV".to_string(),
            PvConfig {
                equipment_id: Some(1),
                capacity_kw,
                tilt_deg: Some(30.0),
                azimuth_deg: Some(180.0),
                module_type: Some("Standard".to_string()),
                noct_c: Some(DEFAULT_NOCT_C),
                system_losses_fraction: Some(hares_losses),
                inverter_efficiency: Some(hares_inv_eff),
                surface_resolution_deg: Some(5.0),
                ..base_pv_typed_config()
            },
        )
        .unwrap();
        let mut pv_no_lut = PV::new(cfg_no_lut.clone());
        let env = env_with_surfaces_full(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 1_000.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            ambient_c,
            1.0,
        );
        pv_no_lut.init(&cfg_no_lut, &env).unwrap();
        let mut ports = PortSlots::default();
        pv_no_lut
            .step(&env, Duration::from_secs(60), &mut ports)
            .unwrap();
        let dc_no_lut = pv_no_lut.telemetry().get(tk::DC_POWER_KW).unwrap();
        approx_eq(dc_no_lut, expected_dc);

        // LUT PV: same config plus LUT path.
        let mut cfg_lut = base_pv_typed_config();
        cfg_lut.equipment_id = Some(2);
        cfg_lut.capacity_kw = capacity_kw;
        cfg_lut.system_losses_fraction = Some(hares_losses);
        cfg_lut.inverter_efficiency = Some(hares_inv_eff);
        cfg_lut.sam_lut_path = Some(path.to_string_lossy().into_owned());
        let cfg_lut =
            EquipmentConfig::from_typed("PV LUT".to_string(), "PV".to_string(), cfg_lut).unwrap();

        // Weather must match LUT entry coordinates: zenith=30 (altitude=60),
        // azimuth=180, GHI=DNI=DHI=0 (LUT has a single entry at these coords).
        // The LUT skips the non-LUT power formula and returns AC directly.
        let mut env_lut = env_with_surfaces_full(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 1_000.0, // non-zero for cell temp ancillary calc
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            ambient_c,
            1.0,
        );
        env_lut.weather.solar_altitude_deg = 60.0; // zenith = 30
        env_lut.weather.solar_azimuth_deg = 180.0;
        env_lut.weather.ghi_w_m2 = 0.0;
        env_lut.weather.dni_w_m2 = 0.0;
        env_lut.weather.dhi_w_m2 = 0.0;

        let mut pv_lut = PV::new(cfg_lut.clone());
        pv_lut.init(&cfg_lut, &env_lut).unwrap();
        let mut ports_lut = PortSlots::default();
        pv_lut
            .step(&env_lut, Duration::from_secs(60), &mut ports_lut)
            .unwrap();
        let dc_lut = pv_lut.telemetry().get(tk::DC_POWER_KW).unwrap();

        // The LUT path should produce the same DC power as the non-LUT path
        // because the LUT metadata correction is a no-op when SAM and HARES
        // parameters match.
        approx_eq(dc_lut, dc_no_lut);
    }

    /// Regression: when system_losses_fraction is changed from the PVWatts
    /// default (0.14) but the LUT encodes the default losses, both paths
    /// still produce consistent DC because the correction un-derates and
    /// re-derates.
    #[test]
    fn lut_non_lut_consistency_with_custom_losses() {
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).expect("surface id");

        let capacity_kw = 5.0;
        let sam_losses = 0.14;
        let sam_inv_eff = 0.96;
        let hares_losses = 0.05; // custom, different from SAM default
        let hares_inv_eff = 0.92; // custom, different from SAM default
        let ambient_c = 25.0;

        // Non-LUT path: what the direct formula produces with custom losses.
        let t_cell = cell_temperature_noct_wind(ambient_c, 1000.0, DEFAULT_NOCT_C, 1.0);
        let derate = 1.0 + DEFAULT_GAMMA_PER_C * (t_cell - 25.0);
        let dc_no_losses = capacity_kw * derate;
        let expected_dc = dc_no_losses * (1.0 - hares_losses);

        // Compute what the LUT's raw AC would be if SAM uses default losses.
        // SAM: AC_lut = DC_no_losses * (1 - sam_losses) * sam_inv_eff
        let ac_lut = dc_no_losses * (1.0 - sam_losses) * sam_inv_eff;

        // LUT-path correction: dc_true = AC_lut / sam_inv_eff / (1 - sam_losses)
        //                    = dc_no_losses
        // HARES DC = dc_true * (1 - hares_losses) = dc_no_losses * (1 - 0.05)
        // This should exactly match the non-LUT path.

        let path = unique_temp_path("pv_lut_t0107_custom", "parquet");
        write_pv_lut_parquet_with_meta(&path, ac_lut, sam_inv_eff, sam_losses);

        // LUT PV.
        let mut cfg_lut = base_pv_typed_config();
        cfg_lut.equipment_id = Some(2);
        cfg_lut.capacity_kw = capacity_kw;
        cfg_lut.system_losses_fraction = Some(hares_losses);
        cfg_lut.inverter_efficiency = Some(hares_inv_eff);
        cfg_lut.sam_lut_path = Some(path.to_string_lossy().into_owned());
        let cfg_lut =
            EquipmentConfig::from_typed("PV LUT".to_string(), "PV".to_string(), cfg_lut).unwrap();

        let mut env_lut = env_with_surfaces_full(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 1_000.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            ambient_c,
            1.0,
        );
        env_lut.weather.solar_altitude_deg = 60.0;
        env_lut.weather.solar_azimuth_deg = 180.0;
        env_lut.weather.ghi_w_m2 = 0.0;
        env_lut.weather.dni_w_m2 = 0.0;
        env_lut.weather.dhi_w_m2 = 0.0;

        let mut pv_lut = PV::new(cfg_lut.clone());
        pv_lut.init(&cfg_lut, &env_lut).unwrap();
        let mut ports_lut = PortSlots::default();
        pv_lut
            .step(&env_lut, Duration::from_secs(60), &mut ports_lut)
            .unwrap();
        let dc_lut = pv_lut.telemetry().get(tk::DC_POWER_KW).unwrap();

        approx_eq(dc_lut, expected_dc);
    }

    /// Verify that `dc_power_kw_before_losses` is populated correctly in
    /// both LUT and non-LUT paths. The field is internal to `step_one_array`
    /// so verification is indirect: final `dc_power_kw` is checked against
    /// the expected formula. In the non-LUT path `before_losses` equals
    /// `capacity * POA/STC * temp_derate`; in the LUT path it equals
    /// `ac_lut / sam_inv_eff / (1 - sam_losses)`, the raw DC recovered
    /// before HARES' `system_losses_fraction` is applied.
    #[test]
    fn dc_power_kw_before_losses_populated() {
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).expect("surface id");
        let ambient_c = 25.0;

        let t_cell = cell_temperature_noct_wind(ambient_c, 1000.0, DEFAULT_NOCT_C, 1.0);
        let derate = 1.0 + DEFAULT_GAMMA_PER_C * (t_cell - 25.0);
        let expected_dc_no_losses = 5.0 * derate;
        let expected_dc = expected_dc_no_losses * (1.0 - DEFAULT_SYSTEM_LOSSES_FRACTION);

        // Non-LUT path: before_losses should equal expected_dc_no_losses,
        // and final DC = expected_dc_no_losses * (1 - DEFAULT_SYSTEM_LOSSES_FRACTION).
        let cfg_no_lut = config_single();
        let env = env_with_surfaces_full(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 1_000.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            ambient_c,
            1.0,
        );
        let mut pv_no_lut = PV::new(cfg_no_lut.clone());
        pv_no_lut.init(&cfg_no_lut, &env).unwrap();
        let mut ports = PortSlots::default();
        pv_no_lut
            .step(&env, Duration::from_secs(60), &mut ports)
            .unwrap();
        let dc_final = pv_no_lut.telemetry().get(tk::DC_POWER_KW).unwrap();
        approx_eq(dc_final, expected_dc);

        // LUT path: the LUT's AC output encodes SAM's default losses and
        // inverter efficiency. The LUT metadata-aware correction recovers
        // dc_true = ac_lut / sam_inv_eff / (1 - sam_losses) which should
        // equal expected_dc_no_losses when the LUT AC matches the non-LUT
        // path formula. dc_before_losses is set to dc_true, and final DC
        // = dc_true * (1 - hares_losses) = expected_dc.
        let sam_losses = DEFAULT_SYSTEM_LOSSES_FRACTION;
        let sam_inv_eff = DEFAULT_INVERTER_EFFICIENCY;
        let ac_lut = expected_dc_no_losses * (1.0 - sam_losses) * sam_inv_eff;
        let path = unique_temp_path("pv_lut_before_losses", "parquet");
        write_pv_lut_parquet_with_meta(&path, ac_lut, sam_inv_eff, sam_losses);

        let mut cfg_lut = base_pv_typed_config();
        cfg_lut.equipment_id = Some(2);
        cfg_lut.sam_lut_path = Some(path.to_string_lossy().into_owned());
        let cfg_lut =
            EquipmentConfig::from_typed("PV LUT".to_string(), "PV".to_string(), cfg_lut).unwrap();

        let mut env_lut = env_with_surfaces_full(
            vec![SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: 1_000.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            }],
            ambient_c,
            1.0,
        );
        env_lut.weather.solar_altitude_deg = 60.0; // zenith = 30
        env_lut.weather.solar_azimuth_deg = 180.0;
        env_lut.weather.ghi_w_m2 = 0.0;
        env_lut.weather.dni_w_m2 = 0.0;
        env_lut.weather.dhi_w_m2 = 0.0;

        let mut pv_lut = PV::new(cfg_lut.clone());
        pv_lut.init(&cfg_lut, &env_lut).unwrap();
        let mut ports_lut = PortSlots::default();
        pv_lut
            .step(&env_lut, Duration::from_secs(60), &mut ports_lut)
            .unwrap();
        let dc_lut = pv_lut.telemetry().get(tk::DC_POWER_KW).unwrap();
        approx_eq(dc_lut, expected_dc);
    }

    // --- T-0108: Soiling reconciliation tests ---

    /// When the Kimber soiling model is active, the PVWatts static soiling
    /// component (2%) is removed from `system_losses_fraction`. With 3% Kimber
    /// soiling (soiling_ratio=0.97) and effective losses of 0.12 (14%-2%),
    /// the DC power should closely match what 1.0 soiling_ratio and 14%
    /// losses produce. The slight difference arises because soiling reduces
    /// irradiance before cell-temperature calc, while losses derate DC after.
    #[test]
    fn soiling_reconciliation_produces_equivalent_dc_power() {
        let sid = surface_id_for_orientation(30.0, 180.0, 5.0).unwrap();
        let surfaces = vec![SurfaceIrradiance {
            surface_id: sid,
            direct_w_m2: 1_000.0,
            diffuse_w_m2: 0.0,
            reflected_w_m2: 0.0,
            angle_of_incidence_rad: 0.0,
        }];
        let env = env_with_surfaces_full(surfaces.clone(), 25.0, 1.0);

        // PV A: no soiling, system_losses = 0.14, effective = 0.14
        let cfg_a = config_single();
        let mut pv_a = PV::new(cfg_a.clone());
        pv_a.init(&cfg_a, &env).unwrap();
        // Ensure no soiling is active.
        pv_a.soiling_config = None;
        pv_a.soiling_state = None;
        pv_a.effective_system_losses_fraction = pv_a.system_losses_fraction;

        let irr_a = env
            .weather
            .solar_irradiance
            .iter()
            .find(|e| e.surface_id == sid)
            .unwrap();
        let out_a = pv_a.step_one_array(&env, irr_a, &pv_a.arrays[0], 1.0, 1.0);
        let dc_a = out_a.dc_power_kw;

        // PV B: soiling active, system_losses = 0.14, effective = 0.12
        let cfg_b = config_single();
        let mut pv_b = PV::new(cfg_b.clone());
        pv_b.init(&cfg_b, &env).unwrap();
        // Simulate soiling reconciliation.
        pv_b.soiling_config = Some(super::soiling::SoilingConfig::default());
        pv_b.effective_system_losses_fraction =
            (pv_b.system_losses_fraction - PVWATTS_SOILING_COMPONENT).max(0.0);

        let irr_b = env
            .weather
            .solar_irradiance
            .iter()
            .find(|e| e.surface_id == sid)
            .unwrap();
        let out_b = pv_b.step_one_array(&env, irr_b, &pv_b.arrays[0], 0.97, 1.0);
        let dc_b = out_b.dc_power_kw;

        // Both should produce DC within a 2% relative tolerance: the layered
        // application (irradiance-level soiling vs DC-level loss) produces a
        // small non-linear difference (~0.7% at 3% soiling).
        let ratio = dc_b / dc_a;
        assert!(
            (ratio - 1.0).abs() < 0.02,
            "DC power with Kimber soiling (ratio=0.97, eff_losses=0.12) = {dc_b:.6} kW \
             vs baseline (ratio=1.0, eff_losses=0.14) = {dc_a:.6} kW, ratio={ratio:.6}"
        );
    }

    /// When soiling config is active, effective_system_losses_fraction must equal
    /// system_losses_fraction minus the PVWatts static soiling component (0.02).
    #[test]
    fn effective_losses_reduced_when_soiling_active() {
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

        // No soiling: effective_losses = system_losses_fraction
        let cfg = config_single();
        let mut pv = PV::new(cfg.clone());
        pv.init(&cfg, &env).unwrap();
        approx_eq(
            pv.effective_system_losses_fraction,
            pv.system_losses_fraction,
        );

        // With soiling: effective_losses = system_losses_fraction - 0.02
        pv.soiling_config = Some(super::soiling::SoilingConfig::default());
        pv.effective_system_losses_fraction =
            (pv.system_losses_fraction - PVWATTS_SOILING_COMPONENT).max(0.0);
        approx_eq(
            pv.effective_system_losses_fraction,
            DEFAULT_SYSTEM_LOSSES_FRACTION - PVWATTS_SOILING_COMPONENT,
        );
    }

    /// The PVWatts soiling component constant (0.02) matches the documented
    /// 2% soiling loss in the PVWatts v5 default breakdown.
    #[test]
    fn soiling_component_matches_pvwatts_default_breakdown() {
        // PVWatts v5 default losses product:
        // 0.98 * 0.97 * 0.98 * 0.98 * 0.995 * 0.985 * 0.99 * 0.97 ≈ 0.8603
        // So 1 - product ≈ 0.1397 ≈ 14%.
        // The soiling component alone: 1 - 0.98 = 0.02.
        approx_eq(PVWATTS_SOILING_COMPONENT, 0.02);
        approx_eq(1.0 - PVWATTS_SOILING_COMPONENT, 0.98);
    }
}
